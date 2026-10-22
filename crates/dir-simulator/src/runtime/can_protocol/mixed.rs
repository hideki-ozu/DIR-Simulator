//! One coordinator over actual local controller clocks and physical wired-AND buses.
use super::{codec, confinement, input, random, time::Exact, types::*};
use crate::registry::*;
use crate::runtime::canfd::{
    codec as fd,
    receiver::{DecodedFrame, RateAction, Receiver, Sample},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[path = "gateway.rs"]
mod gateway;
const KEY: &str = "dir.can.CompoundProtocolV1";
fn event_schema() -> Schema {
    Schema::new("dir.can.CompoundTick", 1)
}
fn records() -> Schema {
    Schema::new("dir.can.CompoundRecord", 1)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Tick {
    generation: u64,
    at: Exact,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Event {
    Generate {
        generator: usize,
        time: u64,
    },
    Route {
        index: usize,
    },
    Cancel {
        index: usize,
    },
    Recover {
        bus: usize,
        node: usize,
    },
    Service {
        gateway: usize,
        serial: u64,
    },
    Start {
        bus: usize,
    },
    Boundary {
        bus: usize,
        node: usize,
        generation: u64,
    },
    Sample {
        bus: usize,
        node: usize,
        generation: u64,
    },
}
impl Event {
    fn rank(&self) -> u8 {
        match self {
            Self::Generate { .. }
            | Self::Route { .. }
            | Self::Cancel { .. }
            | Self::Recover { .. }
            | Self::Service { .. } => 0,
            Self::Start { .. } => 1,
            Self::Boundary { .. } => 2,
            Self::Sample { .. } => 3,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Queued {
    at: Exact,
    sequence: u64,
    event: Event,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Timer {
    token: TimerToken,
    tick: Tick,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Lineage {
    origin: String,
    parent: Option<String>,
    hop: u64,
    route_version: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    frame: ProtocolFrame,
    generated: u64,
    attempt: u64,
    lineage: Lineage,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Plan {
    bits: Vec<u8>,
    arbitration: Vec<u8>,
    stuff: Vec<usize>,
    ack: usize,
    crc: u32,
    esi: bool,
}
fn plan(frame: &ProtocolFrame, passive: bool) -> ModelResult<Plan> {
    if let Some(frame) = frame.cc() {
        let w = codec::encode(&frame)?;
        Ok(Plan {
            bits: w.bits,
            arbitration: w.arbitration,
            stuff: w.stuff_positions,
            ack: w.ack,
            crc: w.crc as u32,
            esi: false,
        })
    } else if let ProtocolFrame::IsoFd {
        id,
        extended,
        brs,
        data,
    } = frame
    {
        let w = fd::encode(&fd::Frame {
            id: *id,
            extended: *extended,
            brs: *brs,
            esi: passive,
            data: data.clone(),
        })
        .map_err(|e| ModelError(e.to_string()))?;
        Ok(Plan {
            bits: w.frame,
            arbitration: w.arbitration,
            stuff: w.dynamic_stuff_positions,
            ack: w.crc_delimiter_index + 1,
            crc: w.crc,
            esi: passive,
        })
    } else {
        unreachable!()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transmission {
    request: Request,
    plan: Plan,
    index: usize,
    sof: Exact,
    observed: Vec<u8>,
    failed: bool,
    settled: bool,
    error: Option<Exact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Flag {
    active: bool,
    transmitter: bool,
    kind: String,
    sent: u8,
    equal: u8,
    last: u8,
    dominant: u16,
    ack_deferred: bool,
    after_flag_seen: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "phase", deny_unknown_fields)]
enum Phase {
    Idle,
    Frame,
    EofLast,
    Flag { flag: Flag },
    Delimiter { flag: Flag, count: u8 },
    Intermission { count: u8, previous_tx: bool },
    Suspend { remaining: u8 },
    BusOff,
    MonitorWait { recessive: u8 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Clock {
    generation: u64,
    start: Exact,
    data: bool,
    next: Option<(Exact, bool)>,
    last_sample: Option<Exact>,
    from_sample: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    counters: confinement::Counters,
    phase: Phase,
    clock: Clock,
    drive: u8,
    drive_index: usize,
    drive_source: Option<(String, u64)>,
    ack_next: bool,
    ack_drive: bool,
    parser: Option<Receiver>,
    epoch: Option<u64>,
    tx: Option<Transmission>,
    pending: Vec<Request>,
    sample_serial: u64,
    frame_index: usize,
    recovery: bool,
    recessive: u8,
    idle_sequences: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Epoch {
    sof: Exact,
    sources: BTreeMap<String, Source>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    lineage: Lineage,
    attempt: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bus {
    nodes: Vec<Node>,
    level: u8,
    serial: u64,
    current_epoch: Option<u64>,
    epochs: BTreeMap<u64, Epoch>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ingress {
    serial: u64,
    frame: ProtocolFrame,
    lineage: Lineage,
    source_bus: usize,
    source_controller: usize,
    version: u64,
    rules: Vec<usize>,
    received: Exact,
    due: Exact,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Gateway {
    version: u64,
    queue: VecDeque<Ingress>,
    serial: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    initialized: bool,
    now: Exact,
    buses: Vec<Bus>,
    gateways: Vec<Gateway>,
    cursors: Vec<random::Cursor>,
    queue: Vec<Queued>,
    sequence: u64,
    timer: Option<Timer>,
    tick_generation: u64,
    cancelled: BTreeSet<String>,
    terminal: BTreeMap<String, String>,
    branch_serial: u64,
}
struct Compound {
    config: CompoundConfig,
    workload: CompoundWorkload,
    state: State,
}
fn encode<T: Serialize>(value: &T) -> ModelResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| ModelError(e.to_string()))
}
fn factory(config: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload) = input::parse_compound(&config.profile_input)?;
    let buses = config
        .buses
        .iter()
        .map(|b| Bus {
            level: 1,
            serial: 0,
            current_epoch: None,
            epochs: BTreeMap::new(),
            nodes: b
                .controllers
                .iter()
                .map(|c| Node {
                    counters: confinement::Counters {
                        tec: c.tec,
                        rec: c.rec,
                    },
                    phase: if c.tec >= 256 {
                        Phase::BusOff
                    } else {
                        Phase::Idle
                    },
                    clock: Clock {
                        generation: 0,
                        start: Exact::integer(0),
                        data: false,
                        next: None,
                        last_sample: None,
                        from_sample: false,
                    },
                    drive: 1,
                    drive_index: 0,
                    drive_source: None,
                    ack_next: false,
                    ack_drive: false,
                    parser: None,
                    epoch: None,
                    tx: None,
                    pending: vec![],
                    sample_serial: 0,
                    frame_index: 0,
                    recovery: c.automatic_recovery,
                    recessive: 0,
                    idle_sequences: 0,
                })
                .collect(),
        })
        .collect();
    let gateways = config
        .gateways
        .iter()
        .map(|g| Gateway {
            version: g.version,
            queue: VecDeque::new(),
            serial: 0,
        })
        .collect();
    let cursors = workload
        .generators
        .iter()
        .map(|g| random::Cursor::new(&g.schedule))
        .collect();
    Ok(Box::new(Compound {
        config,
        workload,
        state: State {
            initialized: false,
            now: Exact::integer(0),
            buses,
            gateways,
            cursors,
            queue: vec![],
            sequence: 0,
            timer: None,
            tick_generation: 0,
            cancelled: BTreeSet::new(),
            terminal: BTreeMap::new(),
            branch_serial: 0,
        },
    }))
}
impl Compound {
    fn record(&self, c: &mut Context<'_>, id: String, mut data: Value) -> ModelResult {
        data["execution_contract"] = json!("ideal-digital-can.v1");
        c.upsert_model_record(ModelRecord {
            schema: records(),
            id,
            subject: c.subject().into(),
            time_ps: c.now(),
            data,
        })
    }
    fn node_record(&self, c: &mut Context<'_>, b: usize, n: usize) -> ModelResult {
        let node = &self.state.buses[b].nodes[n];
        self.record(c,format!("controller:{}:{}",self.config.buses[b].id,self.config.buses[b].controllers[n].id),json!({"kind":"controller","bus":self.config.buses[b].id,"controller":self.config.buses[b].controllers[n].id,"tec":node.counters.tec,"rec":node.counters.rec,"state":node.counters.state(),"phase":node.phase,"recovery_idle_sequences":node.idle_sequences}))
    }
    fn period(&self, b: usize, data: bool) -> ModelResult<Exact> {
        Exact::new(
            1_000_000_000_000,
            if data {
                self.config.buses[b].data_bps
            } else {
                self.config.buses[b].nominal_bps
            } as u128,
        )
    }
    fn portion(&self, b: usize, data: bool, remainder: bool) -> ModelResult<Exact> {
        let f = if data {
            &self.config.buses[b].data_sample
        } else {
            &self.config.buses[b].nominal_sample
        };
        self.period(b, data)?
            .fraction(if remainder { f.den - f.num } else { f.num }, f.den)
    }
    fn enqueue(&mut self, at: Exact, event: Event) -> ModelResult {
        if at.compare(self.state.now)?.is_lt() {
            return Err("CAN private event precedes exact current time".into());
        }
        if matches!(event, Event::Start { .. })
            && self
                .state
                .queue
                .iter()
                .any(|e| e.at == at && e.event == event)
        {
            return Ok(());
        }
        self.state.sequence = self
            .state
            .sequence
            .checked_add(1)
            .ok_or("CAN local sequence overflow")?;
        self.state.queue.push(Queued {
            at,
            sequence: self.state.sequence,
            event,
        });
        Ok(())
    }
    fn arm(&mut self, b: usize, n: usize, at: Exact, sample: bool) -> ModelResult {
        self.state.queue.retain(|e|!matches!(e.event,Event::Boundary{bus,node,..}|Event::Sample{bus,node,..}if bus==b&&node==n));
        let clock = &mut self.state.buses[b].nodes[n].clock;
        clock.generation = clock
            .generation
            .checked_add(1)
            .ok_or("CAN clock generation overflow")?;
        clock.next = Some((at, sample));
        let generation = clock.generation;
        self.enqueue(
            at,
            if sample {
                Event::Sample {
                    bus: b,
                    node: n,
                    generation,
                }
            } else {
                Event::Boundary {
                    bus: b,
                    node: n,
                    generation,
                }
            },
        )
    }
    fn next_time(&self) -> ModelResult<Option<Exact>> {
        let mut next = None;
        for event in &self.state.queue {
            if match next {
                None => true,
                Some(time) => event.at.compare(time)?.is_lt(),
            } {
                next = Some(event.at);
            }
        }
        Ok(next)
    }
    fn publish_timer(&mut self, c: &mut Context<'_>) -> ModelResult {
        let next = self.next_time()?;
        if let Some(timer) = &self.state.timer {
            if next == Some(timer.tick.at) {
                return Ok(());
            }
            c.cancel(&timer.token);
            self.state.timer = None;
        }
        if let Some(at) = next {
            // Distinct exact instants may share an integer scheduler timestamp.
            // Keep one private exact instant per callback; scheduling at the current
            // integer time gives it a causal delta cycle without merging samples.
            self.state.tick_generation = self
                .state
                .tick_generation
                .checked_add(1)
                .ok_or("CAN tick generation overflow")?;
            let tick = Tick {
                generation: self.state.tick_generation,
                at,
            };
            let token = c.schedule_at(at.ceil()?, event_schema(), encode(&tick)?)?;
            self.state.timer = Some(Timer { token, tick });
        }
        Ok(())
    }
    fn next_generation(&mut self, g: usize) -> ModelResult {
        if let Some(time) = self.state.cursors[g].next(&self.workload.generators[g].schedule)? {
            self.enqueue(Exact::integer(time), Event::Generate { generator: g, time })?;
        }
        Ok(())
    }
    fn address(&self, bus: &str, node: &str) -> ModelResult<(usize, usize)> {
        let b = self
            .config
            .buses
            .iter()
            .position(|b| b.id == bus)
            .ok_or("unknown CAN bus")?;
        let n = self.config.buses[b]
            .controllers
            .iter()
            .position(|n| n.id == node)
            .ok_or("unknown CAN controller")?;
        Ok((b, n))
    }
    fn admission(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        request: Request,
    ) -> ModelResult<bool> {
        let cancelled = self.state.cancelled.contains(&request.id);
        let accepted = !cancelled
            && self.state.buses[b].nodes[n].pending.len()
                < self.config.buses[b].controllers[n].pending_capacity;
        self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"bus":self.config.buses[b].id,"controller":self.config.buses[b].controllers[n].id,"frame":request.frame,"generated_ps":request.generated.to_string(),"attempt":request.attempt,"state":if cancelled{"cancelled"}else if accepted{"queued"}else{"egress_drop"},"lineage":request.lineage}))?;
        if accepted {
            self.state.buses[b].nodes[n].pending.push(request);
            self.enqueue(self.state.now, Event::Start { bus: b })?;
        }
        Ok(accepted)
    }
    fn selected(&self, b: usize, n: usize) -> ModelResult<Option<usize>> {
        let node = &self.state.buses[b].nodes[n];
        if node.counters.state() == "bus_off"
            || self.config.buses[b].controllers[n].capability == "monitor_only"
        {
            return Ok(None);
        }
        let mut selected = None;
        for (i, r) in node.pending.iter().enumerate() {
            if self.state.cancelled.contains(&r.id) {
                continue;
            }
            let key = (
                plan(&r.frame, false)?.arbitration,
                r.generated,
                r.id.clone(),
            );
            if selected.as_ref().is_none_or(|(_, old)| key < *old) {
                selected = Some((i, key));
            }
        }
        Ok(selected.map(|(i, _)| i))
    }
    fn epoch(&mut self, b: usize, at: Exact) -> ModelResult<u64> {
        if let Some(id) = self.state.buses[b].current_epoch {
            if self.state.buses[b]
                .epochs
                .get(&id)
                .is_some_and(|e| e.sof == at)
            {
                return Ok(id);
            }
        }
        let bus = &mut self.state.buses[b];
        bus.serial = bus
            .serial
            .checked_add(1)
            .ok_or("CAN physical frame sequence overflow")?;
        let id = bus.serial;
        bus.epochs.insert(
            id,
            Epoch {
                sof: at,
                sources: BTreeMap::new(),
            },
        );
        bus.current_epoch = Some(id);
        Ok(id)
    }
    fn begin_transmitter(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        at: Exact,
        adopted: bool,
        epoch: u64,
    ) -> ModelResult {
        let Some(index) = self.selected(b, n)? else {
            return Ok(());
        };
        self.close_transmission(c, b, n, at, adopted)?;
        let mut request = self.state.buses[b].nodes[n].pending.remove(index);
        request.attempt = request
            .attempt
            .checked_add(1)
            .ok_or("CAN attempt overflow")?;
        let wire = plan(
            &request.frame,
            self.state.buses[b].nodes[n].counters.state() == "error_passive",
        )?;
        self.state.buses[b]
            .epochs
            .get_mut(&epoch)
            .ok_or("missing CAN physical frame lineage")?
            .sources
            .insert(
                request.id.clone(),
                Source {
                    lineage: request.lineage.clone(),
                    attempt: request.attempt,
                },
            );
        c.observe("can.protocol.attempts", c.subject().to_owned().as_str(), 1)?;
        self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"state":"transmitting","attempt":request.attempt,"lineage":request.lineage,"bus":self.config.buses[b].id}))?;
        self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"sof_ps":at.ceil()?.to_string(),"sof_num":at.num.to_string(),"sof_den":at.den.to_string(),"state":"transmitting","crc":wire.crc,"esi":wire.esi,"planned_wire_bits":wire.bits}))?;
        let node = &mut self.state.buses[b].nodes[n];
        node.tx = Some(Transmission {
            request,
            plan: wire,
            index: usize::from(adopted),
            sof: at,
            observed: if adopted { vec![0] } else { vec![] },
            failed: false,
            settled: false,
            error: None,
        });
        node.epoch = Some(epoch);
        node.phase = Phase::Frame;
        node.clock.data = false;
        if !adopted {
            node.clock.start = at;
            node.clock.from_sample = false;
            node.clock.last_sample = None;
        }
        if !adopted {
            node.parser = Some(Receiver::new(
                self.config.buses[b].controllers[n].capability != "cc_only",
            ));
            node.frame_index = 0;
            self.arm(b, n, at, false)?;
        }
        Ok(())
    }
    fn start(&mut self, c: &mut Context<'_>, b: usize) -> ModelResult {
        if self.state.buses[b].level != 1 {
            return Ok(());
        }
        let candidates: Vec<_> = self.state.buses[b]
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(n, node)| matches!(node.phase, Phase::Idle).then_some(n))
            .collect();
        let mut eligible = vec![];
        for n in candidates {
            if self.selected(b, n)?.is_some() {
                eligible.push(n);
            }
        }
        if eligible.is_empty() {
            return Ok(());
        }
        let at = self.state.now;
        let epoch = self.epoch(b, at)?;
        for n in eligible {
            self.begin_transmitter(c, b, n, at, false, epoch)?;
        }
        Ok(())
    }
    fn boundary(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        generation: u64,
    ) -> ModelResult {
        let at = self.state.now;
        if self.state.buses[b].nodes[n].clock.generation != generation {
            return Err("stale CAN boundary clock".into());
        }
        if matches!(
            self.state.buses[b].nodes[n].phase,
            Phase::BusOff | Phase::Idle | Phase::Suspend { .. }
        ) {
            self.close_transmission(c, b, n, at, false)?;
        }
        let suppress = self.ack_suppressed(b, n);
        let node = &mut self.state.buses[b].nodes[n];
        node.clock.next = None;
        node.clock.start = at;
        node.drive_index = node.frame_index;
        node.drive_source = node
            .tx
            .as_ref()
            .map(|tx| (tx.request.id.clone(), tx.request.attempt));
        node.ack_drive = false;
        node.drive = match &node.phase {
            Phase::Frame => {
                if let Some(tx) = &node.tx {
                    if tx.failed || tx.index == tx.plan.ack {
                        1
                    } else {
                        tx.plan.bits.get(tx.index).copied().unwrap_or(1)
                    }
                } else if node.ack_next
                    && !suppress
                    && self.config.buses[b].controllers[n].capability != "monitor_only"
                {
                    node.ack_drive = true;
                    0
                } else {
                    1
                }
            }
            Phase::Flag { flag }
                if flag.active && flag.sent < 6 && node.counters.state() != "bus_off" =>
            {
                0
            }
            _ => 1,
        };
        node.ack_next = false;
        if matches!(node.phase, Phase::Idle) {
            self.enqueue(at, Event::Start { bus: b })?;
            return Ok(());
        }
        if matches!(node.phase, Phase::BusOff) && !node.recovery {
            return Ok(());
        }
        let sample = at.add(self.portion(b, self.state.buses[b].nodes[n].clock.data, false)?)?;
        self.arm(b, n, sample, true)
    }
    fn ack_suppressed(&self, b: usize, n: usize) -> bool {
        self.config.faults.iter().any(|fault| {
            fault.kind == "ack_suppress"
                && fault
                    .receiver
                    .as_ref()
                    .is_none_or(|id| *id == self.config.buses[b].controllers[n].id)
                && self.state.buses[b]
                    .nodes
                    .iter()
                    .filter_map(|node| node.tx.as_ref())
                    .any(|tx| {
                        !tx.failed
                            && tx.request.id == fault.request_id
                            && tx.request.attempt == fault.attempt
                            && tx.index == tx.plan.ack
                    })
        })
    }
    fn resolve(&mut self, c: &mut Context<'_>) -> ModelResult {
        for b in 0..self.state.buses.len() {
            let old = self.state.buses[b].level;
            let level = self.physical_level(b);
            self.state.buses[b].level = level;
            if old == 1
                && level == 0
                && self.state.buses[b]
                    .nodes
                    .iter()
                    .any(|node| matches!(node.phase, Phase::Idle | Phase::Suspend { .. }))
            {
                let epoch = self.epoch(b, self.state.now)?;
                for n in 0..self.state.buses[b].nodes.len() {
                    if matches!(
                        self.state.buses[b].nodes[n].phase,
                        Phase::Idle | Phase::Suspend { .. }
                    ) {
                        if matches!(self.state.buses[b].nodes[n].phase, Phase::Idle)
                            && self.selected(b, n)?.is_some()
                        {
                            self.begin_transmitter(c, b, n, self.state.now, false, epoch)?;
                        } else {
                            let node = &mut self.state.buses[b].nodes[n];
                            node.phase = Phase::Frame;
                            node.tx = None;
                            node.parser = Some(Receiver::new(
                                self.config.buses[b].controllers[n].capability != "cc_only",
                            ));
                            node.epoch = Some(epoch);
                            node.frame_index = 0;
                            node.clock.data = false;
                            node.clock.start = self.state.now;
                            self.arm(
                                b,
                                n,
                                self.state.now.add(self.portion(b, false, false)?)?,
                                true,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn local_level(&self, b: usize, n: usize, base: u8) -> u8 {
        let mut level = base;
        for fault in &self.config.faults {
            if fault.kind != "flip"
                || fault.receiver.as_deref()
                    != Some(self.config.buses[b].controllers[n].id.as_str())
                || fault.bit != Some(self.state.buses[b].nodes[n].frame_index)
            {
                continue;
            }
            let source = self.state.buses[b].nodes[n]
                .epoch
                .and_then(|id| self.state.buses[b].epochs.get(&id))
                .and_then(|epoch| epoch.sources.get(&fault.request_id));
            if source.is_some_and(|source| source.attempt == fault.attempt) {
                level ^= 1;
            }
        }
        level
    }
    fn physical_level(&self, b: usize) -> u8 {
        let mut level = self.state.buses[b]
            .nodes
            .iter()
            .fold(1, |level, node| level & node.drive);
        for fault in &self.config.faults {
            if fault.kind == "flip"
                && fault.receiver.is_none()
                && self.state.buses[b].nodes.iter().any(|node| {
                    fault.bit == Some(node.drive_index)
                        && node.drive_source.as_ref().is_some_and(|(id, attempt)| {
                            *id == fault.request_id && *attempt == fault.attempt
                        })
                })
            {
                level ^= 1;
            }
        }
        level
    }
    fn detect(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        kind: &str,
        causes: Vec<String>,
        counted: bool,
    ) -> ModelResult {
        let node = &self.state.buses[b].nodes[n];
        let transmitter = match &node.phase {
            Phase::Flag { flag } | Phase::Delimiter { flag, .. } => flag.transmitter,
            _ => node.tx.as_ref().is_some_and(|tx| !tx.failed),
        };
        if self.config.buses[b].controllers[n].capability == "monitor_only" {
            let node = &mut self.state.buses[b].nodes[n];
            node.phase = Phase::MonitorWait { recessive: 0 };
            node.clock.data = false;
            node.parser = None;
            return Ok(());
        }
        let active = node.counters.state() == "error_active";
        let ack_deferred = transmitter && !active && kind == "ack";
        let stuff_exception = transmitter
            && kind == "stuff"
            && node.tx.as_ref().is_some_and(|tx| {
                tx.plan.stuff.contains(&tx.index)
                    && tx.plan.bits.get(tx.index) == Some(&1)
                    && tx.index - tx.plan.stuff.iter().filter(|&&i| i < tx.index).count()
                        < tx.plan.arbitration.len() + 1
            });
        if !counted {
            let counters = &mut self.state.buses[b].nodes[n].counters;
            if transmitter {
                counters.tx_error(ack_deferred, stuff_exception);
            } else {
                counters.rx_error(false);
            }
        }
        self.record(c,format!("error:{}:{}:{}",self.config.buses[b].id,self.config.buses[b].controllers[n].id,self.state.buses[b].nodes[n].sample_serial),json!({"kind":"error","bus":self.config.buses[b].id,"detector":self.config.buses[b].controllers[n].id,"error":kind,"causes":causes,"detected_ps":c.now().to_string(),"exact_num":self.state.now.num.to_string(),"exact_den":self.state.now.den.to_string(),"local_index":self.state.buses[b].nodes[n].frame_index,"flag_start_num":self.state.now.add(self.portion(b,false,true)?)?.num.to_string(),"flag_start_den":self.state.now.add(self.portion(b,false,true)?)?.den.to_string(),"flag_start_ps":self.state.now.add(self.portion(b,false,true)?)?.ceil()?.to_string(),"flag":if active{"active"}else{"passive"}}))?;
        if transmitter {
            self.fail_transmission(c, b, n)?;
        }
        let node = &mut self.state.buses[b].nodes[n];
        node.phase = Phase::Flag {
            flag: Flag {
                active,
                transmitter,
                kind: kind.into(),
                sent: 0,
                equal: 0,
                last: 2,
                dominant: 0,
                ack_deferred,
                after_flag_seen: false,
            },
        };
        node.clock.data = false;
        node.ack_next = false;
        if node.counters.state() == "bus_off" {
            node.phase = Phase::BusOff;
            node.parser = None;
            node.recessive = 0;
            node.idle_sequences = 0;
        }
        self.node_record(c, b, n)
    }
    fn fail_transmission(&mut self, c: &mut Context<'_>, b: usize, n: usize) -> ModelResult {
        let node = &mut self.state.buses[b].nodes[n];
        let Some(tx) = node.tx.as_mut() else {
            return Ok(());
        };
        if tx.settled {
            return Ok(());
        }
        tx.failed = true;
        tx.settled = true;
        tx.error = Some(self.state.now);
        let request = tx.request.clone();
        let sof = tx.sof;
        let observed = tx.observed.clone();
        if !self.state.cancelled.contains(&request.id) {
            node.pending.push(request.clone());
        }
        c.observe("can.protocol.failures", c.subject().to_owned().as_str(), 1)?;
        self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"attempt":request.attempt,"state":if self.state.cancelled.contains(&request.id){"cancelled"}else{"retry_pending"},"physical_outcome":"failed","lineage":request.lineage}))?;
        self.record(c,format!("attempt:{}:{}",request.id,request.attempt),json!({"kind":"attempt","request_id":request.id,"attempt":request.attempt,"state":"failed","sof_ps":sof.ceil()?.to_string(),"wire_bits":observed,"release_ps":Value::Null,"error_ps":c.now().to_string()}))
    }
    fn close_transmission(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        at: Exact,
        exclude_sof: bool,
    ) -> ModelResult {
        let Some(mut tx) = self.state.buses[b].nodes[n].tx.take() else {
            return Ok(());
        };
        if !tx.failed {
            self.state.buses[b].nodes[n].tx = Some(tx);
            return Ok(());
        }
        if exclude_sof {
            tx.observed.pop();
        }
        self.record(c,format!("attempt:{}:{}",tx.request.id,tx.request.attempt),json!({"kind":"attempt","request_id":tx.request.id,"attempt":tx.request.attempt,"state":"failed","sof_ps":tx.sof.ceil()?.to_string(),"release_ps":at.ceil()?.to_string(),"release_num":at.num.to_string(),"release_den":at.den.to_string(),"error_ps":tx.error.map(|time|time.ceil()).transpose()?.map(|time|time.to_string()),"wire_bits":tx.observed,"crc":tx.plan.crc,"esi":tx.plan.esi}))
    }
    fn lost(&mut self, c: &mut Context<'_>, b: usize, n: usize) -> ModelResult {
        let tx = self.state.buses[b].nodes[n]
            .tx
            .take()
            .ok_or("missing arbitration transmitter")?;
        if let Some(epoch) = self.state.buses[b].nodes[n].epoch {
            if let Some(epoch) = self.state.buses[b].epochs.get_mut(&epoch) {
                epoch.sources.remove(&tx.request.id);
            }
        }
        self.record(c,format!("attempt:{}:{}",tx.request.id,tx.request.attempt),json!({"kind":"attempt","request_id":tx.request.id,"attempt":tx.request.attempt,"state":"arbitration_lost","lost_bit":tx.index,"lost_ps":c.now().to_string(),"sof_ps":tx.sof.ceil()?.to_string(),"wire_bits":tx.observed}))?;
        let request = tx.request;
        let cancelled = self.state.cancelled.contains(&request.id);
        self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"attempt":request.attempt,"state":if cancelled{"cancelled"}else{"queued"},"physical_outcome":"arbitration_lost","lineage":request.lineage}))?;
        if !cancelled {
            self.state.buses[b].nodes[n].pending.push(request);
        }
        Ok(())
    }
    fn overload(&mut self, c: &mut Context<'_>, b: usize, n: usize) -> ModelResult {
        let node = &self.state.buses[b].nodes[n];
        let transmitter = match &node.phase {
            Phase::Intermission { previous_tx, .. } => *previous_tx,
            Phase::Flag { flag } | Phase::Delimiter { flag, .. } => flag.transmitter,
            _ => node.tx.as_ref().is_some_and(|tx| !tx.failed),
        };
        self.state.buses[b].nodes[n].phase = Phase::Flag {
            flag: Flag {
                active: true,
                transmitter,
                kind: "overload".into(),
                sent: 0,
                equal: 0,
                last: 2,
                dominant: 0,
                ack_deferred: false,
                after_flag_seen: false,
            },
        };
        self.state.buses[b].nodes[n].clock.data = false;
        self.record(c,format!("overload:{}:{}:{}",self.config.buses[b].id,self.config.buses[b].controllers[n].id,self.state.buses[b].nodes[n].sample_serial),json!({"kind":"overload","bus":self.config.buses[b].id,"controller":self.config.buses[b].controllers[n].id,"start_num":self.state.now.add(self.portion(b,false,true)?)?.num.to_string(),"start_den":self.state.now.add(self.portion(b,false,true)?)?.den.to_string(),"state":"signalled"}))
    }
    fn complete_transmitter(&mut self, c: &mut Context<'_>, b: usize, n: usize) -> ModelResult {
        let tx = self.state.buses[b].nodes[n]
            .tx
            .take()
            .ok_or("missing completed CAN transmitter")?;
        if self
            .config
            .cancellations
            .iter()
            .any(|cancel| cancel.request_id == tx.request.id)
            && !self.state.cancelled.contains(&tx.request.id)
        {
            self.state
                .terminal
                .insert(tx.request.id.clone(), "completed".into());
        }
        self.state.buses[b].nodes[n].counters.tx_success();
        self.record(c,format!("request:{}",tx.request.id),json!({"kind":"request","request_id":tx.request.id,"frame":tx.request.frame,"attempt":tx.request.attempt,"state":"completed","eof_ps":c.now().to_string(),"lineage":tx.request.lineage,"cancel_requested":self.state.cancelled.contains(&tx.request.id)}))?;
        self.record(c,format!("attempt:{}:{}",tx.request.id,tx.request.attempt),json!({"kind":"attempt","request_id":tx.request.id,"attempt":tx.request.attempt,"state":"completed","sof_ps":tx.sof.ceil()?.to_string(),"eof_ps":c.now().to_string(),"wire_bits":tx.observed,"crc":tx.plan.crc,"esi":tx.plan.esi}))?;
        self.state.buses[b].nodes[n].phase = Phase::Intermission {
            count: 0,
            previous_tx: true,
        };
        self.state.buses[b].nodes[n].parser = None;
        self.state.buses[b].nodes[n].clock.data = false;
        self.node_record(c, b, n)
    }
    fn receive(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        frame: DecodedFrame,
    ) -> ModelResult {
        let epoch = self.state.buses[b].nodes[n]
            .epoch
            .and_then(|id| self.state.buses[b].epochs.get(&id))
            .cloned()
            .ok_or("local receive has no physical SOF lineage")?;
        let sources: Vec<_> = epoch.sources.keys().cloned().collect();
        let local_id = format!(
            "{}:{}:{}",
            self.config.buses[b].id,
            self.config.buses[b].controllers[n].id,
            self.state.buses[b].nodes[n].sample_serial
        );
        c.observe(
            "can.protocol.receptions",
            c.subject().to_owned().as_str(),
            1,
        )?;
        self.record(c,format!("reception:{local_id}"),json!({"kind":"reception","bus":self.config.buses[b].id,"receiver":self.config.buses[b].controllers[n].id,"request_ids":sources,"frame":frame,"state":"received","valid_ps":c.now().to_string(),"valid_num":self.state.now.num.to_string(),"valid_den":self.state.now.den.to_string(),"local_frame_id":local_id}))?;
        let frame = match frame {
            DecodedFrame::Cc {
                id,
                extended,
                remote,
                dlc,
                data,
            } => ProtocolFrame::Cc {
                id,
                extended,
                remote,
                dlc,
                data,
            },
            DecodedFrame::IsoFd {
                id,
                extended,
                brs,
                data,
                ..
            } => ProtocolFrame::IsoFd {
                id,
                extended,
                brs,
                data,
            },
        };
        let mut lineage = epoch
            .sources
            .values()
            .next()
            .map(|source| source.lineage.clone())
            .unwrap_or(Lineage {
                origin: format!("local:{local_id}"),
                parent: None,
                hop: 0,
                route_version: None,
            });
        lineage.parent = epoch.sources.keys().next().cloned();
        self.gateway_receive(c, b, n, frame, lineage)
    }
    fn adopt(&mut self, c: &mut Context<'_>, b: usize, n: usize, allowed_tx: bool) -> ModelResult {
        let sof = self.state.buses[b].nodes[n].clock.start;
        let epoch = self.epoch(b, sof)?;
        self.close_transmission(c, b, n, sof, true)?;
        let mut parser = Receiver::new(self.config.buses[b].controllers[n].capability != "cc_only");
        parser
            .sample(Sample {
                level: 0,
                local_index: 0,
                exact_num: self.state.now.num,
                exact_den: self.state.now.den,
            })
            .map_err(ModelError)?;
        let node = &mut self.state.buses[b].nodes[n];
        node.phase = Phase::Frame;
        node.parser = Some(parser);
        node.frame_index = 0;
        node.epoch = Some(epoch);
        node.clock.data = false;
        self.record(c,format!("local-sof:{}:{}:{}",self.config.buses[b].id,self.config.buses[b].controllers[n].id,self.state.buses[b].nodes[n].sample_serial),json!({"kind":"local_sof","bus":self.config.buses[b].id,"controller":self.config.buses[b].controllers[n].id,"sof_num":sof.num.to_string(),"sof_den":sof.den.to_string(),"sof_ps":sof.ceil()?.to_string(),"sample_ps":c.now().to_string()}))?;
        if allowed_tx && self.selected(b, n)?.is_some() {
            self.begin_transmitter(c, b, n, sof, true, epoch)?;
        }
        Ok(())
    }
    fn flag_sample(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        mut flag: Flag,
        sample: u8,
        delimiter: Option<u8>,
    ) -> ModelResult {
        let monitor = self.config.buses[b].controllers[n].capability == "monitor_only";
        if delimiter.is_none() && flag.active && sample != 0 && !monitor {
            self.state.buses[b].nodes[n]
                .counters
                .flag_bit_error(flag.transmitter);
            self.detect(c, b, n, "bit", vec!["flag_bit".into()], true)?;
            return Ok(());
        }
        let inside = delimiter.is_none();
        if !flag.after_flag_seen && !inside {
            flag.after_flag_seen = true;
            if sample == 0 && !flag.transmitter && flag.kind != "overload" && !monitor {
                self.state.buses[b].nodes[n].counters.dominant_after_flag();
            }
        }
        if flag.active || !inside {
            flag.dominant = if sample == 0 {
                flag.dominant.saturating_add(1)
            } else {
                0
            };
            let threshold = if flag.active { 14 } else { 8 };
            if flag.dominant >= threshold && (flag.dominant - threshold) % 8 == 0 && !monitor {
                self.state.buses[b].nodes[n]
                    .counters
                    .dominant_extension(flag.transmitter);
            }
        }
        if flag.ack_deferred && inside && sample == 0 {
            flag.ack_deferred = false;
            self.state.buses[b].nodes[n].counters.tx_error(false, false);
        }
        if self.state.buses[b].nodes[n].counters.state() == "bus_off" {
            let node = &mut self.state.buses[b].nodes[n];
            node.phase = Phase::BusOff;
            node.clock.data = false;
            node.parser = None;
            node.ack_next = false;
            node.recessive = 0;
            node.idle_sequences = 0;
            return self.node_record(c, b, n);
        }
        if let Some(mut count) = delimiter {
            if sample == 1 {
                count += 1;
            } else if count == 7 {
                self.overload(c, b, n)?;
                return Ok(());
            } else if count > 0 {
                self.detect(c, b, n, "form", vec!["delimiter".into()], false)?;
                return Ok(());
            }
            self.state.buses[b].nodes[n].phase = if count == 8 {
                Phase::Intermission {
                    count: 0,
                    previous_tx: flag.transmitter,
                }
            } else {
                Phase::Delimiter { flag, count }
            };
        } else {
            flag.sent = flag.sent.saturating_add(1);
            flag.equal = if sample == flag.last {
                flag.equal + 1
            } else {
                1
            };
            flag.last = sample;
            self.state.buses[b].nodes[n].phase =
                if flag.active && flag.sent == 6 || !flag.active && flag.equal == 6 {
                    Phase::Delimiter { flag, count: 0 }
                } else {
                    Phase::Flag { flag }
                };
        }
        self.node_record(c, b, n)
    }
    fn sample(
        &mut self,
        c: &mut Context<'_>,
        b: usize,
        n: usize,
        generation: u64,
        level: u8,
    ) -> ModelResult {
        if self.state.buses[b].nodes[n].clock.generation != generation {
            return Err("stale CAN sample clock".into());
        }
        let phase = self.state.buses[b].nodes[n].phase.clone();
        let data = self.state.buses[b].nodes[n].clock.data;
        let start = self.state.buses[b].nodes[n].clock.start;
        let node = &mut self.state.buses[b].nodes[n];
        node.clock.next = None;
        node.sample_serial = node
            .sample_serial
            .checked_add(1)
            .ok_or("CAN sample sequence overflow")?;
        if let Some(tx) = node.tx.as_mut() {
            tx.observed.push(level);
        }
        let local_index = node.frame_index;
        let field: String;
        let mut immediate_error = false;
        let mut action = RateAction::None;
        match phase {
            Phase::Frame => {
                let parser = self.state.buses[b].nodes[n]
                    .parser
                    .as_mut()
                    .ok_or("CAN frame phase missing local parser")?;
                let observation = parser
                    .sample(Sample {
                        level,
                        local_index: parser.len(),
                        exact_num: self.state.now.num,
                        exact_den: self.state.now.den,
                    })
                    .map_err(ModelError)?;
                field = observation.field.clone();
                action = observation.rate_action.clone();
                let tx = self.state.buses[b].nodes[n]
                    .tx
                    .as_ref()
                    .filter(|tx| !tx.failed)
                    .cloned();
                let mut detected = None;
                if let Some(tx) = &tx {
                    let expected = if tx.index == tx.plan.ack {
                        1
                    } else {
                        tx.plan.bits.get(tx.index).copied().unwrap_or(1)
                    };
                    let arbitration = tx.index
                        - tx.plan.stuff.iter().filter(|&&i| i < tx.index).count()
                        < tx.plan.arbitration.len() + 1;
                    if tx.index == tx.plan.ack && level == 1 {
                        detected = Some(("ack".to_string(), vec!["ack".into()]));
                    } else if expected == 1
                        && level == 0
                        && arbitration
                        && !tx.plan.stuff.contains(&tx.index)
                    {
                        self.lost(c, b, n)?;
                    } else if level != expected && !(tx.index == tx.plan.ack && level == 0) {
                        let kind = observation
                            .detection
                            .as_ref()
                            .filter(|d| d.kind == "stuff")
                            .map_or("bit", |_| "stuff");
                        detected = Some((kind.into(), vec![kind.into()]));
                    }
                }
                if detected.is_none() {
                    if let Some(error) = &observation.detection {
                        detected = Some((error.kind.clone(), error.causes.clone()));
                    }
                }
                if self.state.buses[b].nodes[n].ack_drive && level == 1 {
                    detected = Some(("bit".into(), vec!["ack_drive".into()]));
                }
                if let Some((kind, causes)) = detected {
                    self.detect(c, b, n, &kind, causes, false)?;
                    immediate_error = true;
                } else {
                    if self.state.buses[b].nodes[n].ack_drive && level == 0 {
                        self.state.buses[b].nodes[n].counters.rx_success();
                        self.node_record(c, b, n)?;
                    }
                    self.state.buses[b].nodes[n].ack_next = observation.ack_intent;
                    if self.state.buses[b].nodes[n].tx.is_none() {
                        if let Some(frame) = observation.completed_frame {
                            self.receive(c, b, n, frame)?;
                            self.state.buses[b].nodes[n].phase = Phase::EofLast;
                        }
                    } else if observation.frame_end {
                        self.complete_transmitter(c, b, n)?;
                    }
                    if let Some(tx) = self.state.buses[b].nodes[n].tx.as_mut() {
                        if !tx.failed {
                            tx.index += 1;
                        }
                    }
                }
            }
            Phase::EofLast => {
                field = "eof_last".into();
                if level == 0 {
                    self.overload(c, b, n)?;
                    immediate_error = true;
                } else {
                    self.state.buses[b].nodes[n].phase = Phase::Intermission {
                        count: 0,
                        previous_tx: false,
                    };
                    self.state.buses[b].nodes[n].parser = None;
                }
            }
            Phase::Flag { flag } => {
                field = "error_flag".into();
                self.flag_sample(c, b, n, flag, level, None)?;
            }
            Phase::Delimiter { flag, count } => {
                field = "delimiter".into();
                self.flag_sample(c, b, n, flag, level, Some(count))?;
            }
            Phase::Intermission { count, previous_tx } => {
                field = "intermission".into();
                if level == 0 && count < 2 {
                    self.overload(c, b, n)?;
                    immediate_error = true;
                } else if level == 0 {
                    let allowed = !(previous_tx
                        && self.state.buses[b].nodes[n].counters.state() == "error_passive");
                    self.adopt(c, b, n, allowed)?;
                } else if count == 2 {
                    self.state.buses[b].nodes[n].phase = if previous_tx
                        && self.state.buses[b].nodes[n].counters.state() == "error_passive"
                    {
                        Phase::Suspend { remaining: 8 }
                    } else {
                        Phase::Idle
                    };
                } else {
                    self.state.buses[b].nodes[n].phase = Phase::Intermission {
                        count: count + 1,
                        previous_tx,
                    };
                }
            }
            Phase::Suspend { remaining } => {
                field = "suspend".into();
                if level == 0 {
                    self.adopt(c, b, n, false)?;
                } else {
                    self.state.buses[b].nodes[n].phase = if remaining == 1 {
                        Phase::Idle
                    } else {
                        Phase::Suspend {
                            remaining: remaining - 1,
                        }
                    };
                }
            }
            Phase::BusOff => {
                field = "bus_off".into();
                let node = &mut self.state.buses[b].nodes[n];
                if node.recovery {
                    if level == 0 {
                        node.recessive = 0;
                    } else {
                        node.recessive += 1;
                        if node.recessive == 11 {
                            node.recessive = 0;
                            node.idle_sequences += 1;
                            if node.idle_sequences == 128 {
                                node.idle_sequences = 0;
                                node.counters.tec = 0;
                                node.counters.rec = 0;
                                node.phase = Phase::Idle;
                            }
                        }
                    }
                }
                self.node_record(c, b, n)?;
            }
            Phase::MonitorWait { recessive } => {
                field = "monitor_wait".into();
                let recessive = if level == 1 { recessive + 1 } else { 0 };
                self.state.buses[b].nodes[n].phase = if recessive == 11 {
                    Phase::Idle
                } else {
                    Phase::MonitorWait { recessive }
                };
            }
            Phase::Idle => {
                field = "idle".into();
                if level == 0 {
                    self.adopt(c, b, n, true)?;
                }
            }
        }
        self.record(c,format!("sample:{}:{}:{}",self.config.buses[b].id,self.config.buses[b].controllers[n].id,self.state.buses[b].nodes[n].sample_serial),json!({"kind":"sample","bus":self.config.buses[b].id,"controller":self.config.buses[b].controllers[n].id,"local_index":local_index,"exact_num":self.state.now.num.to_string(),"exact_den":self.state.now.den.to_string(),"rate":if data{"data"}else{"nominal"},"field":field,"sample":level,"resolved_level":self.state.buses[b].level}))?;
        self.state.buses[b].nodes[n].frame_index = self.state.buses[b].nodes[n]
            .frame_index
            .checked_add(1)
            .ok_or("CAN local frame index overflow")?;
        self.state.buses[b].nodes[n].clock.last_sample = Some(self.state.now);
        self.state.buses[b].nodes[n].clock.from_sample =
            immediate_error || action != RateAction::None;
        let end = if immediate_error {
            self.state.now.add(self.portion(b, false, true)?)?
        } else {
            match action {
                RateAction::EnterData => {
                    self.state.buses[b].nodes[n].clock.data = true;
                    self.state.now.add(self.portion(b, true, true)?)?
                }
                RateAction::ReturnNominal => {
                    self.state.buses[b].nodes[n].clock.data = false;
                    self.state.now.add(self.portion(b, false, true)?)?
                }
                RateAction::None => start.add(self.period(b, data)?)?,
            }
        };
        self.arm(b, n, end, false)?;
        Ok(())
    }
    fn control(&mut self, c: &mut Context<'_>, event: Event) -> ModelResult {
        match event {
            Event::Generate { generator: g, time } => {
                let definition = self
                    .workload
                    .generators
                    .get(g)
                    .ok_or("compound generator out of bounds")?
                    .clone();
                let ordinal = self.state.cursors[g].ordinal;
                self.state.cursors[g].ordinal = self.state.cursors[g]
                    .ordinal
                    .checked_add(1)
                    .ok_or("CAN generator ordinal overflow")?;
                let (b, n) = self.address(&definition.bus, &definition.controller)?;
                let id = format!("{}:{ordinal}", definition.id);
                self.admission(
                    c,
                    b,
                    n,
                    Request {
                        id: id.clone(),
                        frame: definition.frame,
                        generated: time,
                        attempt: 0,
                        lineage: Lineage {
                            origin: id,
                            parent: None,
                            hop: 0,
                            route_version: None,
                        },
                    },
                )?;
                self.next_generation(g)
            }
            Event::Route { index } => self.replace_routes(c, index),
            Event::Service { gateway, serial } => self.gateway_service(c, gateway, serial),
            Event::Cancel { index } => {
                let cancel = self
                    .config
                    .cancellations
                    .get(index)
                    .ok_or("compound cancellation out of bounds")?
                    .clone();
                self.state.cancelled.insert(cancel.request_id.clone());
                let terminal = self.state.terminal.remove(&cancel.request_id);
                let mut active = None;
                let mut queued = None;
                for bus in &mut self.state.buses {
                    for node in &mut bus.nodes {
                        if let Some(tx) = &node.tx {
                            if tx.request.id == cancel.request_id {
                                active = Some((tx.request.clone(), tx.failed));
                            }
                        }
                        if let Some(index) =
                            node.pending.iter().position(|r| r.id == cancel.request_id)
                        {
                            queued = Some(node.pending.remove(index));
                        }
                    }
                }
                let state = if terminal.is_some() {
                    "too_late"
                } else if active.is_some() {
                    "active_retry_suppressed"
                } else if queued.is_some() {
                    "pending_removed"
                } else {
                    "future_suppressed"
                };
                if let Some((request, failed)) = active {
                    self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"attempt":request.attempt,"state":if failed{"cancelled"}else{"transmitting"},"cancel_requested":true,"physical_outcome":if failed{Some("failed")}else{None},"lineage":request.lineage}))?;
                } else if let Some(request) = queued {
                    self.record(c,format!("request:{}",request.id),json!({"kind":"request","request_id":request.id,"frame":request.frame,"attempt":request.attempt,"state":"cancelled","cancel_requested":true,"lineage":request.lineage}))?;
                }
                self.record(c,format!("cancellation:{}",cancel.request_id),json!({"kind":"cancellation","request_id":cancel.request_id,"state":state,"retry_suppressed":terminal.is_none()}))
            }
            Event::Recover { bus, node } => {
                let n = &mut self.state.buses[bus].nodes[node];
                n.recovery = true;
                n.recessive = 0;
                n.idle_sequences = 0;
                if matches!(n.phase, Phase::BusOff) {
                    self.arm(bus, node, self.state.now, false)?;
                }
                Ok(())
            }
            Event::Start { bus } => self.start(c, bus),
            Event::Boundary {
                bus,
                node,
                generation,
            } => self.boundary(c, bus, node, generation),
            Event::Sample { .. } => Err("sample dispatched through control path".into()),
        }
    }
    fn tick(&mut self, c: &mut Context<'_>, tick: Tick, event_id: u64) -> ModelResult {
        let owned = self
            .state
            .timer
            .take()
            .ok_or("compound tick has no owned timer")?;
        if owned.tick != tick || owned.token.id != event_id || tick.at.ceil()? != c.now() {
            return Err("compound tick payload/deadline mismatch".into());
        }
        self.state.now = tick.at;
        // Controls and all drive changes settle before any same-instant sample.
        loop {
            let next = self
                .state
                .queue
                .iter()
                .enumerate()
                .filter(|(_, e)| e.at == tick.at && e.event.rank() < 3)
                .min_by_key(|(_, e)| (e.event.rank(), e.sequence))
                .map(|(i, _)| i);
            let Some(index) = next else {
                break;
            };
            let event = self.state.queue.remove(index).event;
            self.control(c, event)?;
        }
        self.resolve(c)?;
        let mut samples = Vec::new();
        self.state.queue.retain(|event| {
            if event.at == tick.at && matches!(event.event, Event::Sample { .. }) {
                samples.push(event.clone());
                false
            } else {
                true
            }
        });
        samples.sort_by_key(|e| e.sequence);
        let frozen: Vec<_> = samples
            .into_iter()
            .map(|e| match e.event {
                Event::Sample {
                    bus,
                    node,
                    generation,
                } => (
                    bus,
                    node,
                    generation,
                    self.local_level(bus, node, self.state.buses[bus].level),
                ),
                _ => unreachable!(),
            })
            .collect();
        for (b, n, generation, level) in frozen {
            self.sample(c, b, n, generation, level)?;
        }
        for bus in &mut self.state.buses {
            let live: BTreeSet<_> = bus
                .nodes
                .iter()
                .filter(|node| {
                    node.tx.is_some()
                        || !matches!(
                            node.phase,
                            Phase::Idle | Phase::BusOff | Phase::MonitorWait { .. }
                        )
                })
                .filter_map(|node| node.epoch)
                .collect();
            bus.epochs.retain(|id, _| live.contains(id));
            if bus
                .current_epoch
                .is_some_and(|id| !bus.epochs.contains_key(&id))
            {
                bus.current_epoch = None;
            }
        }
        self.publish_timer(c)
    }
}
impl Model for Compound {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        self.state.initialized = true;
        for b in 0..self.config.buses.len() {
            for n in 0..self.config.buses[b].controllers.len() {
                self.node_record(c, b, n)?;
                if let Some(time) = self.config.buses[b].controllers[n].recover_at_ps {
                    self.enqueue(Exact::integer(time), Event::Recover { bus: b, node: n })?;
                }
                if matches!(self.state.buses[b].nodes[n].phase, Phase::BusOff)
                    && self.state.buses[b].nodes[n].recovery
                {
                    self.arm(b, n, Exact::integer(0), false)?;
                }
            }
        }
        for (index, cancel) in self.config.cancellations.clone().iter().enumerate() {
            self.enqueue(Exact::integer(cancel.at_ps), Event::Cancel { index })?;
        }
        for (index, update) in self.config.route_updates.clone().iter().enumerate() {
            self.enqueue(Exact::integer(update.at_ps), Event::Route { index })?;
        }
        for g in 0..self.workload.generators.len() {
            self.next_generation(g)?;
        }
        self.publish_timer(c)
    }
    fn on_event(&mut self, envelope: &Envelope, c: &mut Context<'_>) -> ModelResult {
        let saved = self.state.clone();
        let result = (|| {
            let tick: Tick =
                serde_json::from_slice(&envelope.payload).map_err(|e| ModelError(e.to_string()))?;
            self.tick(c, tick, envelope.event_id)
        })();
        if result.is_err() {
            self.state = saved;
        }
        result
    }
    fn checkpoint(&self, context: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        self.validate()?;
        if let Some(timer) = &self.state.timer {
            if context.encode_timer_ref(&timer.token)?.state != "live" {
                return Err("compound CAN timer is retired".into());
            }
        }
        if context.live_timers.len() != usize::from(self.state.timer.is_some()) {
            return Err("compound CAN timer ownership incomplete".into());
        }
        encode(&self.state)
    }
}
impl Compound {
    fn validate_request(&self, request: &Request, b: usize, n: usize) -> ModelResult {
        input::validate_protocol_frame(&request.frame)?;
        if let Some((generator, ordinal)) = request.id.rsplit_once(':') {
            if let Some((g, definition)) = self
                .workload
                .generators
                .iter()
                .enumerate()
                .find(|(_, definition)| definition.id == generator)
            {
                let ordinal: u64 = ordinal
                    .parse()
                    .map_err(|_| ModelError("invalid native CAN request ordinal".into()))?;
                if request.id != format!("{generator}:{ordinal}")
                    || ordinal >= self.state.cursors[g].ordinal
                    || definition.bus != self.config.buses[b].id
                    || definition.controller != self.config.buses[b].controllers[n].id
                    || request.frame != definition.frame
                    || request.lineage.origin != request.id
                    || request.lineage.parent.is_some()
                    || request.lineage.hop != 0
                    || request.lineage.route_version.is_some()
                    || matches!(&definition.schedule, Schedule::Explicit{times_ps} if times_ps.get(ordinal as usize) != Some(&request.generated))
                {
                    return Err("compound CAN native request/workload identity mismatch".into());
                }
                return Ok(());
            }
        }
        let parts: Vec<_> = request.id.split(':').collect();
        if parts.len() != 4
            || parts[0] != "branch"
            || !self
                .config
                .gateways
                .iter()
                .any(|gateway| gateway.id == parts[1])
            || parts[2].parse::<u64>().is_err()
            || parts[2].parse::<u64>().is_ok_and(|serial| {
                serial == 0 || serial > self.state.branch_serial || serial.to_string() != parts[2]
            })
            || request.lineage.hop == 0
            || request.lineage.hop > self.config.max_gateway_hops
            || request.lineage.parent.is_none()
            || request.lineage.route_version.is_none()
        {
            return Err("compound CAN branch identity/lineage mismatch".into());
        }
        Ok(())
    }
    fn validate(&self) -> ModelResult {
        if self.state.buses.len() != self.config.buses.len()
            || self.state.gateways.len() != self.config.gateways.len()
            || self.state.cursors.len() != self.workload.generators.len()
            || !self.state.now.valid()
        {
            return Err("compound CAN state arena invariant".into());
        }
        let mut sequences = BTreeSet::new();
        let mut clocks = BTreeMap::new();
        let mut services = BTreeMap::new();
        let mut generations = BTreeSet::new();
        let mut routes = BTreeSet::new();
        let mut cancellations = BTreeSet::new();
        let mut recoveries = BTreeSet::new();
        for entry in &self.state.queue {
            if !entry.at.valid()
                || entry.at.compare(self.state.now)?.is_lt()
                || entry.sequence == 0
                || entry.sequence > self.state.sequence
                || !sequences.insert(entry.sequence)
            {
                return Err("compound CAN local event ordering invariant".into());
            }
            match &entry.event {
                Event::Boundary {
                    bus,
                    node,
                    generation,
                }
                | Event::Sample {
                    bus,
                    node,
                    generation,
                } => {
                    let owner = self
                        .state
                        .buses
                        .get(*bus)
                        .and_then(|b| b.nodes.get(*node))
                        .ok_or("compound CAN clock owner absent")?;
                    if owner.clock.generation != *generation
                        || owner.clock.next
                            != Some((entry.at, matches!(entry.event, Event::Sample { .. })))
                        || clocks.insert((*bus, *node), entry.sequence).is_some()
                    {
                        return Err("compound CAN clock event mismatch".into());
                    }
                }
                Event::Generate { generator, time } => {
                    if *generator >= self.workload.generators.len()
                        || entry.at != Exact::integer(*time)
                        || !generations.insert(*generator)
                    {
                        return Err("compound CAN generator event invariant".into());
                    }
                    let cursor = &self.state.cursors[*generator];
                    let expected = match &self.workload.generators[*generator].schedule {
                        Schedule::Explicit { times_ps } => {
                            times_ps.get(cursor.ordinal as usize).copied()
                        }
                        Schedule::Uniform { .. } | Schedule::Geometric { .. } => Some(cursor.time),
                        Schedule::Bernoulli { tick_ps, .. } => cursor.time.checked_sub(*tick_ps),
                    };
                    if expected != Some(*time) || cursor.ordinal >= cursor.position {
                        return Err("compound CAN generator cursor/deadline mismatch".into());
                    }
                }
                Event::Route { index } => {
                    let update = self
                        .config
                        .route_updates
                        .get(*index)
                        .ok_or("compound CAN route event absent")?;
                    if entry.at != Exact::integer(update.at_ps) || !routes.insert(*index) {
                        return Err("compound CAN route deadline invariant".into());
                    }
                }
                Event::Cancel { index } => {
                    let cancel = self
                        .config
                        .cancellations
                        .get(*index)
                        .ok_or("compound CAN cancellation absent")?;
                    if entry.at != Exact::integer(cancel.at_ps)
                        || self.state.cancelled.contains(&cancel.request_id)
                        || !cancellations.insert(*index)
                    {
                        return Err("compound CAN cancellation invariant".into());
                    }
                }
                Event::Recover { bus, node } => {
                    if self
                        .config
                        .buses
                        .get(*bus)
                        .and_then(|b| b.controllers.get(*node))
                        .and_then(|c| c.recover_at_ps)
                        .map(Exact::integer)
                        != Some(entry.at)
                        || !recoveries.insert((*bus, *node))
                    {
                        return Err("compound CAN recovery deadline invariant".into());
                    }
                }
                Event::Service { gateway, serial } => {
                    let front = self
                        .state
                        .gateways
                        .get(*gateway)
                        .and_then(|g| g.queue.front())
                        .ok_or("compound CAN service owner absent")?;
                    if front.serial != *serial
                        || front.due != entry.at
                        || services.insert(*gateway, *serial).is_some()
                    {
                        return Err("compound CAN service owner/deadline invariant".into());
                    }
                }
                Event::Start { bus } => {
                    if *bus >= self.state.buses.len() {
                        return Err("compound CAN start bus absent".into());
                    }
                }
            }
        }
        for (g, (cursor, definition)) in self
            .state
            .cursors
            .iter()
            .zip(&self.workload.generators)
            .enumerate()
        {
            let maximum = match &definition.schedule {
                Schedule::Explicit { times_ps } => times_ps.len() as u64,
                Schedule::Uniform { count, .. }
                | Schedule::Bernoulli { count, .. }
                | Schedule::Geometric { count, .. } => *count,
            };
            if cursor.position > maximum || cursor.ordinal > cursor.position {
                return Err("compound CAN generator cursor bounds".into());
            }
            match definition.schedule {
                Schedule::Bernoulli { count, .. } => {
                    if self.state.initialized
                        && !generations.contains(&g)
                        && cursor.position != count
                    {
                        return Err("compound CAN exhausted Bernoulli cursor mismatch".into());
                    }
                }
                _ => {
                    if cursor.position != cursor.ordinal + u64::from(generations.contains(&g)) {
                        return Err("compound CAN generator reservation incomplete".into());
                    }
                }
            }
        }
        if self.state.cancelled.iter().any(|id| {
            !self
                .config
                .cancellations
                .iter()
                .any(|cancel| &cancel.request_id == id)
        }) || self.state.initialized
            && self
                .config
                .cancellations
                .iter()
                .enumerate()
                .any(|(index, cancel)| {
                    self.state.cancelled.contains(&cancel.request_id)
                        == cancellations.contains(&index)
                })
            || self.state.terminal.iter().any(|(id, state)| {
                state != "completed"
                    || self.state.cancelled.contains(id)
                    || !self
                        .config
                        .cancellations
                        .iter()
                        .any(|cancel| &cancel.request_id == id)
            })
            || !self.state.initialized
                && (!self.state.queue.is_empty() || self.state.timer.is_some())
        {
            return Err("compound CAN cancellation/initialization ownership invariant".into());
        }
        for (b, bus) in self.state.buses.iter().enumerate() {
            if bus.nodes.len() != self.config.buses[b].controllers.len()
                || bus.level > 1
                || bus
                    .current_epoch
                    .is_some_and(|id| !bus.epochs.contains_key(&id))
            {
                return Err("compound CAN bus topology invariant".into());
            }
            for (n, node) in bus.nodes.iter().enumerate() {
                if node.counters.tec > 256
                    || node.counters.rec > 255
                    || node.drive > 1
                    || node.recessive >= 11
                    || node.idle_sequences >= 128
                    || (node.counters.state() == "bus_off") != matches!(node.phase, Phase::BusOff)
                    || node.pending.len() > self.config.buses[b].controllers[n].pending_capacity + 1
                    || node.clock.next.is_some() != clocks.contains_key(&(b, n))
                    || !node.clock.start.valid()
                {
                    return Err("compound CAN controller state invariant".into());
                }
                let mut pending_ids = BTreeSet::new();
                for request in &node.pending {
                    self.validate_request(request, b, n)?;
                    if !pending_ids.insert(&request.id)
                        || self.state.cancelled.contains(&request.id)
                        || self.state.terminal.contains_key(&request.id)
                    {
                        return Err("compound CAN pending request ownership invariant".into());
                    }
                }
                if let Some(tx) = &node.tx {
                    self.validate_request(&tx.request, b, n)?;
                    if tx.failed != tx.settled
                        || tx.failed != tx.error.is_some()
                        || (!tx.failed && pending_ids.contains(&tx.request.id))
                    {
                        return Err("compound CAN transmission settlement invariant".into());
                    }
                }
                if let Some(time) = self.config.buses[b].controllers[n].recover_at_ps {
                    if self.state.initialized
                        && Exact::integer(time).compare(self.state.now)?.is_gt()
                        && !recoveries.contains(&(b, n))
                    {
                        return Err("compound CAN recovery reservation missing".into());
                    }
                }
                if matches!(node.phase, Phase::Frame | Phase::EofLast)
                    && (node.parser.is_none()
                        || node.epoch.is_none_or(|id| !bus.epochs.contains_key(&id)))
                {
                    return Err("compound CAN frame parser missing".into());
                }
                if let Some((at, sample)) = node.clock.next {
                    let expected = if !sample && node.clock.last_sample.is_none() {
                        node.clock.start
                    } else if sample {
                        node.clock
                            .start
                            .add(self.portion(b, node.clock.data, false)?)?
                    } else if node.clock.from_sample {
                        node.clock
                            .last_sample
                            .ok_or("CAN split/error boundary has no owning sample")?
                            .add(self.portion(b, node.clock.data, true)?)?
                    } else {
                        node.clock.start.add(self.period(b, node.clock.data)?)?
                    };
                    if at != expected {
                        return Err("CAN local clock deadline is not derived from its exact boundary/sample".into());
                    }
                }
                if let Some(parser) = &node.parser {
                    parser.validate().map_err(ModelError)?;
                    if parser.fd_capable()
                        != (self.config.buses[b].controllers[n].capability != "cc_only")
                        || matches!(node.phase, Phase::Frame | Phase::EofLast)
                            && (parser.data_phase_active() != node.clock.data
                                || parser.len() != node.frame_index)
                    {
                        return Err(
                            "CAN local clock rate differs from actual parsed BRS/CRC delimiter"
                                .into(),
                        );
                    }
                }
                if let Some(tx) = &node.tx {
                    if tx.plan != plan(&tx.request.frame, tx.plan.esi)?
                        || tx.index > tx.plan.bits.len()
                        || tx.observed.iter().any(|b| *b > 1)
                        || tx.request.attempt == 0
                        || (!tx.failed && tx.observed.len() != tx.index)
                        || (tx.failed && tx.observed.len() <= tx.index)
                        || !tx.sof.valid()
                    {
                        return Err("compound CAN transmitter wire invariant".into());
                    }
                }
                match &node.phase {
                    Phase::Flag { flag } | Phase::Delimiter { flag, .. }
                        if flag.active && flag.sent > 6 || flag.equal > 6 || flag.last > 2 =>
                    {
                        return Err("compound CAN local flag invariant".into());
                    }
                    Phase::Delimiter { count, .. } if *count > 7 => {
                        return Err("compound CAN delimiter invariant".into());
                    }
                    Phase::Intermission { count, .. } if *count > 2 => {
                        return Err("compound CAN intermission invariant".into());
                    }
                    Phase::Suspend { remaining } if *remaining == 0 || *remaining > 8 => {
                        return Err("compound CAN suspend invariant".into());
                    }
                    _ => {}
                }
            }
        }
        for (g, gateway) in self.state.gateways.iter().enumerate() {
            let mut expected_version = self.config.gateways[g].version;
            for (index, update) in self.config.route_updates.iter().enumerate() {
                if update.gateway == self.config.gateways[g].id
                    && !routes.contains(&index)
                    && self.state.initialized
                {
                    if Exact::integer(update.at_ps)
                        .compare(self.state.now)?
                        .is_gt()
                    {
                        return Err("compound CAN future route update reservation missing".into());
                    }
                    expected_version = update.version;
                }
            }
            if gateway.version != expected_version {
                return Err("compound CAN active route version/reservations mismatch".into());
            }
            let mut serial = 0;
            let mut due = None;
            for item in &gateway.queue {
                let rules = self.route_rules(g, item.version)?;
                if item.serial <= serial
                    || item.serial > gateway.serial
                    || item.source_bus >= self.state.buses.len()
                    || item.source_controller >= self.state.buses[item.source_bus].nodes.len()
                    || !item.received.valid()
                    || !item.due.valid()
                    || item.received.compare(item.due)?.is_gt()
                    || due.is_some_and(|previous: Exact| {
                        previous.compare(item.due).is_ok_and(|order| order.is_gt())
                    })
                    || item.version > gateway.version
                    || item.rules.is_empty()
                    || item.rules.iter().any(|&index| index >= rules.len())
                {
                    return Err("compound CAN Gateway ingress snapshot invariant".into());
                }
                let mut receive_version = self.config.gateways[g].version;
                for update in &self.config.route_updates {
                    if update.gateway == self.config.gateways[g].id
                        && !Exact::integer(update.at_ps).compare(item.received)?.is_gt()
                    {
                        receive_version = update.version;
                    }
                }
                let source_bus = &self.config.buses[item.source_bus].id;
                let mut matched: Vec<_> = rules
                    .iter()
                    .enumerate()
                    .filter_map(|(index, rule)| {
                        (rule.source_bus == *source_bus
                            && item.frame.id() & rule.id_mask == rule.id_value
                            && rule
                                .extended
                                .is_none_or(|extended| extended == item.frame.extended())
                            && rule
                                .remote
                                .is_none_or(|remote| remote == item.frame.remote()))
                        .then_some(index)
                    })
                    .collect();
                matched.sort_by_key(|&index| {
                    (
                        &rules[index].id,
                        &rules[index].destination_bus,
                        &rules[index].controller,
                    )
                });
                if item.version != receive_version
                    || item.rules != matched
                    || item
                        .due
                        .compare(
                            item.received
                                .add(Exact::integer(self.config.gateways[g].service_ps))?,
                        )?
                        .is_lt()
                    || !self.config.gateways[g].ports.iter().any(|port| {
                        port.bus == *source_bus
                            && port.controller
                                == self.config.buses[item.source_bus].controllers
                                    [item.source_controller]
                                    .id
                    })
                {
                    return Err(
                        "compound CAN Gateway received route snapshot/match mismatch".into(),
                    );
                }
                input::validate_protocol_frame(&item.frame)?;
                if let Some(previous) = due {
                    if item.due
                        != previous.add(Exact::integer(self.config.gateways[g].service_ps))?
                    {
                        return Err(
                            "compound CAN Gateway queued service arithmetic mismatch".into()
                        );
                    }
                } else if item.due.compare(self.state.now)?.is_lt() {
                    return Err("compound CAN Gateway front service is in the past".into());
                }
                serial = item.serial;
                due = Some(item.due);
            }
            if gateway.queue.len() > self.config.gateways[g].ingress_capacity
                || gateway.queue.is_empty() == services.contains_key(&g)
            {
                return Err("compound CAN Gateway queue/service invariant".into());
            }
        }
        let next = self.next_time()?;
        if self.state.timer.as_ref().map(|t| t.tick.at) != next
            || self
                .state
                .timer
                .as_ref()
                .is_some_and(|t| t.tick.generation != self.state.tick_generation)
        {
            return Err("compound CAN minimum timer invariant".into());
        }
        Ok(())
    }
}
pub(super) fn register(registry: &mut Registry) -> Result<(), crate::Diagnostic> {
    registry.register_event(EventDescriptor {
        schema: event_schema(),
        phase: 1,
        validate: |raw| {
            serde_json::from_slice::<Tick>(raw)
                .map(|_| ())
                .map_err(|e| ModelError(e.to_string()))
        },
    })?;
    registry.register_module(
        KEY,
        factory,
        ModuleDescriptor {
            implementation_key: KEY.into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![event_schema()],
            resources: vec![],
        },
    )?;
    registry.register_model_record(ModelRecordDescriptor {
        schema: records(),
        validate: super::output::validate_compound_record,
    })?;
    for profile in ["can.mixed.protocol.v1", "can.gateway.protocol.v1"] {
        registry.register_profile(ProfileDescriptor {
            name: profile.into(),
            implementation_version: "1".into(),
            modules: vec![KEY.into()],
            channels: vec![],
            events: vec![event_schema()],
            metrics: vec![
                "can.protocol.attempts".into(),
                "can.protocol.failures".into(),
                "can.protocol.receptions".into(),
            ],
            model_records: vec![records()],
            output_schema_version: 3,
            validate: |models, channels| {
                if models.len() == 1 && channels.is_empty() {
                    Ok(())
                } else {
                    Err("compound CAN requires one coordinator and no channels".into())
                }
            },
        })?;
        registry.register_profile_input_validator(profile, input::validate_compound)?;
        registry.register_execution_capabilities(
            profile,
            ExecutionCapabilities {
                cooperative_control: true,
                metrics_v2: false,
                checkpoint: true,
            },
        )?;
    }
    registry.register_model_checkpoint_codec(
        CheckpointCodecDescriptor {
            implementation_key: KEY.into(),
            implementation_version: "1".into(),
            codec_name: "dir.can.compound.state".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 64 * 1024 * 1024,
        },
        |config, version, raw, context| {
            if version != 1 {
                return Err("unsupported compound CAN codec version".into());
            }
            let (config, workload) = input::parse_compound(&config.profile_input)?;
            let state: State =
                serde_json::from_slice(raw).map_err(|e| ModelError(e.to_string()))?;
            let model = Compound {
                config,
                workload,
                state,
            };
            model.validate()?;
            if let Some(timer) = &model.state.timer {
                context.decode_timer_ref(&TimerReference {
                    version: 1,
                    owner: timer.token.owner.clone(),
                    id: timer.token.id.to_string(),
                    state: "live".into(),
                })?;
                let envelope = context.timer_envelope(&timer.token)?;
                if envelope.time_ps != timer.tick.at.ceil()?
                    || envelope.schema != event_schema()
                    || envelope.payload != encode(&timer.tick)?
                {
                    return Err("compound CAN actual FES envelope differs from owned timer".into());
                }
            }
            if context.live_timers.len() != usize::from(model.state.timer.is_some()) {
                return Err("compound CAN timer reservation completeness invariant".into());
            }
            Ok(Box::new(model))
        },
    )?;
    Ok(())
}
