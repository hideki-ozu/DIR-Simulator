//! Sample-clock Clause 9 repeater core for one supported three-port Clause 14 star.
//! The caller supplies recovered 20 Mchip/s Manchester decisions from each
//! independent link receiver, then transmits the returned regenerated chips
//! through each link transmitter. No MAC-address or completed-frame buffering
//! is involved; forwarding begins while the source is still sending preamble.
use super::super::Result;
use super::super::medium::Origin;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

pub const CONTRACT: &str = "ieee8023.repeater10.2022";
fn default_contract() -> String {
    CONTRACT.into()
}
fn is_default_contract(value: &str) -> bool {
    value == CONTRACT
}
const MIN_FRAGMENT_CHIPS: u32 = 96 * 2;
const MIN_JAM_CHIPS: u32 = 62 * 2;
const MAX_RETIME_CHIPS: usize = 8 * 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub mac_endpoint: String,
    pub mau_endpoint: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(
        default = "default_contract",
        skip_serializing_if = "is_default_contract"
    )]
    pub execution_contract: String,
    /// Exactly three explicit station-MAU <-> repeater-MAU links.
    pub pairs: Vec<Pair>,
    /// Exact retiming latency in 50 ns Manchester half-bit chips.
    pub retime_chips: usize,
    /// Clause 9.6.6 CCLimit; 10BASE-T requires a value greater than 30.
    pub consecutive_collision_limit: u16,
    /// Clause 9.6.6 Tw5, 450..560 bit times.
    pub reconnect_bits: u16,
    /// Clause 9.6.6 Tw6, 1000..30000 bit times.
    pub excessive_collision_bits: u16,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        let unique: BTreeSet<_> = self
            .pairs
            .iter()
            .flat_map(|p| [&p.mac_endpoint, &p.mau_endpoint])
            .collect();
        if self.execution_contract != CONTRACT
            || self.pairs.len() != 3
            || self.pairs.iter().any(|p| {
                p.mac_endpoint.is_empty()
                    || p.mau_endpoint.is_empty()
                    || p.mac_endpoint.len() > 128
                    || p.mau_endpoint.len() > 128
            })
            || unique.len() != 2 * self.pairs.len()
            || self.retime_chips == 0
            || self.retime_chips > MAX_RETIME_CHIPS
            || self.consecutive_collision_limit <= 30
            || !(450..=560).contains(&self.reconnect_bits)
            || !(1000..=30_000).contains(&self.excessive_collision_bits)
        {
            return Err("invalid Clause 9 10BASE-T repeater port/timer configuration".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortObservation {
    /// Recovered Manchester chip, when the local receiver produces a decision.
    pub chip: Option<i8>,
    /// MAU DataIn carrier, independent of a chip decision on this sample.
    pub data_in: bool,
    /// MAU CollIn signal_quality_error, including single-port collision.
    pub collision: bool,
    pub link_ok: bool,
    /// Actual admitted MAC attempts contributing to this received chip/carrier.
    pub origins: BTreeSet<Origin>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Generator {
    RepeatedData,
    RepeaterJam,
    FragmentExtension,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputChip {
    pub level: i8,
    pub origins: BTreeSet<Origin>,
    pub generator: Generator,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepResult {
    pub outputs: Vec<Option<OutputChip>>,
    pub events: Vec<Event>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Forwarding { input_port: String },
    LinkPulse { input_port: String },
    Collision,
    FragmentExtended,
    Partitioned { port: String },
    Reconnected { port: String },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Idle,
    Forward,
    Jam,
    Wait,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repeater {
    config: Config,
    phase: Phase,
    input: Option<usize>,
    pending: VecDeque<OutputChip>,
    episode_origins: BTreeSet<Origin>,
    jam_generator: Generator,
    emitted_chips: u32,
    jam_chips: u32,
    wait_chips: u32,
    collision_chips: Vec<u32>,
    collision_count: Vec<u16>,
    partitioned: Vec<bool>,
    reconnect_chips: Vec<u32>,
    active_before: Vec<bool>,
    collision_before: Vec<bool>,
    episode_collision: Vec<bool>,
}
impl Repeater {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        let n = config.pairs.len();
        Ok(Self {
            config,
            phase: Phase::Idle,
            input: None,
            pending: VecDeque::new(),
            episode_origins: BTreeSet::new(),
            jam_generator: Generator::RepeaterJam,
            emitted_chips: 0,
            jam_chips: 0,
            wait_chips: 0,
            collision_chips: vec![0; n],
            collision_count: vec![0; n],
            partitioned: vec![false; n],
            reconnect_chips: vec![0; n],
            active_before: vec![false; n],
            collision_before: vec![false; n],
            episode_collision: vec![false; n],
        })
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn partitioned(&self, index: usize) -> Result<bool> {
        self.partitioned
            .get(index)
            .copied()
            .ok_or("unknown repeater port".into())
    }
    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }
    /// One simultaneous recovered-chip observation per independent Clause 14
    /// MAU. Outputs are decisions for a future driver boundary, never an
    /// algebraic contribution to the input samples just observed.
    pub fn step(&mut self, inputs: &[PortObservation]) -> Result<StepResult> {
        if inputs.len() != self.config.pairs.len()
            || inputs
                .iter()
                .any(|v| v.chip.is_some_and(|chip| ![-1, 1].contains(&chip)))
        {
            return Err("invalid repeater MAU observation vector".into());
        }
        let n = inputs.len();
        let mut events = vec![];
        let raw_active: Vec<_> = inputs.iter().map(|v| v.data_in && v.link_ok).collect();
        let pre_partition_active: Vec<_> = raw_active
            .iter()
            .enumerate()
            .filter_map(|(i, &active)| (active && !self.partitioned[i]).then_some(i))
            .collect();
        let physical_collision: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.link_ok && (v.collision || raw_active[i] && pre_partition_active.len() > 1)
            })
            .collect();
        for i in 0..n {
            if !self.partitioned[i] && (raw_active[i] || physical_collision[i]) {
                self.episode_origins
                    .extend(inputs[i].origins.iter().cloned());
            }
            if physical_collision[i] {
                self.episode_collision[i] = true;
                self.collision_chips[i] = self.collision_chips[i].saturating_add(1);
                if !self.collision_before[i] {
                    self.collision_count[i] = self.collision_count[i].saturating_add(1);
                }
            } else {
                self.collision_chips[i] = 0;
            }
            if self.partitioned[i] {
                // Tw5 observes raw DIPresent/CIPresent even though this port
                // is gated away from the repeater's main data state machine.
                self.reconnect_chips[i] = if raw_active[i] && !inputs[i].collision {
                    self.reconnect_chips[i].saturating_add(1)
                } else {
                    0
                };
                if self.reconnect_chips[i] >= self.config.reconnect_bits as u32 * 2 {
                    self.partitioned[i] = false;
                    self.collision_count[i] = 0;
                    self.reconnect_chips[i] = 0;
                    self.episode_collision[i] = false;
                    events.push(Event::Reconnected {
                        port: self.config.pairs[i].mau_endpoint.clone(),
                    });
                }
            } else if self.collision_count[i] >= self.config.consecutive_collision_limit
                || self.collision_chips[i] >= self.config.excessive_collision_bits as u32 * 2
            {
                self.partitioned[i] = true;
                self.reconnect_chips[i] = 0;
                events.push(Event::Partitioned {
                    port: self.config.pairs[i].mau_endpoint.clone(),
                });
            } else if self.active_before[i] && !raw_active[i] {
                if !self.episode_collision[i] {
                    self.collision_count[i] = 0;
                }
                self.episode_collision[i] = false;
            }
        }
        let contenders: Vec<_> = raw_active
            .iter()
            .enumerate()
            .filter_map(|(i, &active)| (active && !self.partitioned[i]).then_some(i))
            .collect();
        let collision = contenders.len() > 1
            || physical_collision
                .iter()
                .enumerate()
                .any(|(i, &collision)| collision && !self.partitioned[i])
            || self.phase == Phase::Forward && contenders.iter().any(|&i| Some(i) != self.input);
        if collision && self.phase != Phase::Jam {
            self.phase = Phase::Jam;
            self.pending.clear();
            self.jam_chips = 0;
            self.jam_generator = Generator::RepeaterJam;
            events.push(Event::Collision);
        }
        if self.phase == Phase::Idle && contenders.len() == 1 {
            self.phase = Phase::Forward;
            self.input = Some(contenders[0]);
            self.emitted_chips = 0;
            self.pending.clear();
            events.push(Event::Forwarding {
                input_port: self.config.pairs[contenders[0]].mau_endpoint.clone(),
            });
        }
        // A Clause 14 normal link pulse is a short positive MDI pulse. The
        // retime buffer holds it until its end, so it cannot become a MAC
        // fragment or synthesized repeater jam.
        if self.phase == Phase::Forward
            && self.input.is_some_and(|source| !raw_active[source])
            && self.emitted_chips == 0
            && !self.pending.is_empty()
            && self.pending.len() <= 2
            && self.pending.iter().all(|chip| chip.level == 1)
        {
            events.push(Event::LinkPulse {
                input_port: self.config.pairs[self.input.unwrap()].mau_endpoint.clone(),
            });
            self.phase = Phase::Idle;
            self.input = None;
            self.pending.clear();
            self.episode_origins.clear();
        }
        let mut outputs = vec![None; n];
        match self.phase {
            Phase::Idle => {}
            Phase::Forward => {
                let source = self
                    .input
                    .ok_or("repeater forward state lacks input port")?;
                if let Some(chip) = inputs[source].chip {
                    self.pending.push_back(OutputChip {
                        level: chip,
                        origins: inputs[source].origins.clone(),
                        generator: Generator::RepeatedData,
                    });
                }
                if self.pending.len() > self.config.retime_chips
                    || !raw_active[source] && !self.pending.is_empty()
                {
                    let chip = self.pending.pop_front().unwrap();
                    for (i, out) in outputs.iter_mut().enumerate() {
                        if i != source {
                            *out = Some(chip.clone());
                        }
                    }
                    self.emitted_chips = self.emitted_chips.saturating_add(1);
                } else if !raw_active[source] && self.pending.is_empty() {
                    if self.emitted_chips < MIN_FRAGMENT_CHIPS {
                        self.phase = Phase::Jam;
                        self.jam_chips = 0;
                        self.jam_generator = Generator::FragmentExtension;
                        events.push(Event::FragmentExtended);
                    } else {
                        self.phase = Phase::Wait;
                        self.wait_chips = 0;
                        self.collision_count[source] = 0;
                    }
                }
            }
            Phase::Jam => {
                // Clause 9.5.6.2 first 62 jam bits: 1,0,1,0... .
                let bit = (self.jam_chips / 2) % 2 == 0;
                let level = match (bit, self.jam_chips % 2) {
                    (true, 0) | (false, 1) => 1,
                    _ => -1,
                };
                outputs.fill(Some(OutputChip {
                    level,
                    origins: self.episode_origins.clone(),
                    generator: self.jam_generator,
                }));
                self.jam_chips = self.jam_chips.saturating_add(1);
                if self.jam_chips >= MIN_JAM_CHIPS
                    && self.jam_chips >= MIN_FRAGMENT_CHIPS.saturating_sub(self.emitted_chips)
                    && contenders.is_empty()
                {
                    self.phase = Phase::Wait;
                    self.wait_chips = 0;
                }
            }
            Phase::Wait => {
                self.wait_chips += 1;
                if self.wait_chips >= 4 && contenders.is_empty() {
                    self.phase = Phase::Idle;
                    self.input = None;
                    self.emitted_chips = 0;
                    self.episode_origins.clear();
                }
            }
        }
        self.active_before = raw_active;
        self.collision_before = physical_collision;
        Ok(StepResult { outputs, events })
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| format!("repeater checkpoint: {e}"))
    }
    pub fn restore(config: &Config, bytes: &[u8]) -> Result<Self> {
        let saved: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("repeater restore: {e}"))?;
        if &saved.config != config
            || saved.collision_count.len() != config.pairs.len()
            || saved.collision_chips.len() != config.pairs.len()
            || saved.partitioned.len() != config.pairs.len()
            || saved.reconnect_chips.len() != config.pairs.len()
            || saved.active_before.len() != config.pairs.len()
            || saved.collision_before.len() != config.pairs.len()
            || saved.episode_collision.len() != config.pairs.len()
            || saved.pending.len() > config.retime_chips
            || saved
                .pending
                .iter()
                .any(|chip| ![-1, 1].contains(&chip.level))
            || saved.input.is_some_and(|i| i >= config.pairs.len())
        {
            return Err("invalid or configuration-mismatched repeater checkpoint".into());
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            execution_contract: CONTRACT.into(),
            pairs: (0..3)
                .map(|i| Pair {
                    mac_endpoint: format!("m{i}"),
                    mau_endpoint: format!("r{i}"),
                })
                .collect(),
            retime_chips: 4,
            consecutive_collision_limit: 31,
            reconnect_bits: 500,
            excessive_collision_bits: 1000,
        }
    }
    fn idle() -> PortObservation {
        PortObservation {
            chip: None,
            data_in: false,
            collision: false,
            link_ok: true,
            origins: BTreeSet::new(),
        }
    }
    fn data(port: usize, chip: i8) -> PortObservation {
        PortObservation {
            chip: Some(chip),
            data_in: true,
            collision: false,
            link_ok: true,
            origins: [Origin {
                frame_id: format!("f{port}"),
                source_endpoint: format!("m{port}"),
                attempt: 0,
            }]
            .into(),
        }
    }
    #[test]
    fn repeats_active_chips_with_real_origin_before_frame_ends() {
        let mut repeater = Repeater::new(config()).unwrap();
        let chips = crate::runtime::ethernet::phy::wire::encode_bytes(&[0x55; 8]);
        let mut forwarded = vec![];
        for &chip in &chips {
            let step = repeater.step(&[data(0, chip), idle(), idle()]).unwrap();
            assert_eq!(step.outputs[0], None);
            assert_eq!(step.outputs[1], step.outputs[2]);
            if let Some(chip) = &step.outputs[1] {
                assert_eq!(chip.generator, Generator::RepeatedData);
                assert_eq!(chip.origins, data(0, 1).origins);
                forwarded.push(chip.level);
            }
        }
        assert_eq!(forwarded, chips[..chips.len() - 4]);
        let checkpoint = repeater.checkpoint().unwrap();
        let mut restored = Repeater::restore(&config(), &checkpoint).unwrap();
        for _ in 0..70 {
            assert_eq!(
                repeater.step(&[idle(), idle(), idle()]).unwrap(),
                restored.step(&[idle(), idle(), idle()]).unwrap()
            );
        }
        assert_eq!(
            repeater.checkpoint().unwrap(),
            restored.checkpoint().unwrap()
        );
    }
    #[test]
    fn jam_starts_with_62_alternating_bits_and_two_real_origins() {
        let mut repeater = Repeater::new(config()).unwrap();
        let first = repeater.step(&[data(0, 1), data(1, -1), idle()]).unwrap();
        assert_eq!(first.events, [Event::Collision]);
        assert_eq!(first.outputs[0], first.outputs[1]);
        assert_eq!(first.outputs[1], first.outputs[2]);
        assert_eq!(first.outputs[0].as_ref().unwrap().origins.len(), 2);
        let mut jam = vec![first.outputs[0].as_ref().unwrap().level];
        for _ in 1..124 {
            let step = repeater.step(&[idle(), idle(), idle()]).unwrap();
            jam.push(step.outputs[0].as_ref().unwrap().level);
        }
        for (bit, pair) in jam.chunks_exact(2).enumerate() {
            assert_eq!(pair, if bit % 2 == 0 { &[1, -1] } else { &[-1, 1] });
        }
    }
    #[test]
    fn short_positive_link_pulse_is_not_repeated_or_fragment_extended() {
        let mut repeater = Repeater::new(config()).unwrap();
        for _ in 0..2 {
            let step = repeater.step(&[data(0, 1), idle(), idle()]).unwrap();
            assert!(step.outputs.iter().all(Option::is_none));
        }
        let step = repeater.step(&[idle(), idle(), idle()]).unwrap();
        assert!(step.outputs.iter().all(Option::is_none));
        assert_eq!(
            step.events,
            [Event::LinkPulse {
                input_port: "r0".into()
            }]
        );
        assert!(repeater.is_idle());
        for _ in 0..10 {
            let step = repeater.step(&[idle(), idle(), idle()]).unwrap();
            assert!(step.outputs.iter().all(Option::is_none));
            assert!(step.events.is_empty());
        }
    }
    #[test]
    fn single_port_collision_partitions_and_tw5_requires_collision_free_activity() {
        let mut repeater = Repeater::new(config()).unwrap();
        let mut partition = false;
        for _ in 0..31 {
            let mut collision = data(0, 1);
            collision.collision = true;
            let step = repeater.step(&[collision, idle(), idle()]).unwrap();
            partition |= step
                .events
                .iter()
                .any(|e| matches!(e, Event::Partitioned { port } if port == "r0"));
            for _ in 0..210 {
                repeater.step(&[idle(), idle(), idle()]).unwrap();
            }
        }
        assert!(partition);
        assert!(repeater.partitioned(0).unwrap());
        for _ in 0..900 {
            repeater.step(&[data(0, 1), idle(), idle()]).unwrap();
        }
        let mut collision = data(0, 1);
        collision.collision = true;
        repeater.step(&[collision, idle(), idle()]).unwrap();
        assert!(repeater.partitioned(0).unwrap());
        let mut reconnected = false;
        for _ in 0..1000 {
            let step = repeater.step(&[data(0, 1), idle(), idle()]).unwrap();
            reconnected |= step
                .events
                .iter()
                .any(|e| matches!(e, Event::Reconnected { port } if port == "r0"));
        }
        assert!(reconnected);
        assert!(!repeater.partitioned(0).unwrap());
    }
}
