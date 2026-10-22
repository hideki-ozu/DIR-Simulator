//! Bounded 1000BASE-T1 Clause 97/98 link startup and continuous PCS service.
//! Actual MDI samples and recovered partner symbols advance every state.
use super::super::Result;
pub use super::t1_an::Event as AnEvent;
use super::t1_an::{self, Phase as AnPhase};
use super::t1_gigabit::Role;
use super::t1_info::{INFO_BITS, InfoField, PmaState};
use super::t1_raw::{RawReceiveStream, RawTransmitStream, ReceivedRaw};
pub use super::t1_stream::SourceRange;
use super::t1_stream::{ReceiveStream, ReceivedMac, TransmitStream};
use super::t1_training::{
    PARTIAL_SYMBOLS, PARTIALS_PER_FRAME, SYMBOLS_PER_FRAME, TrainingReceiver, TrainingTransmitter,
};
use serde::{Deserialize, Serialize};

pub const CONTRACT: &str = "ieee8023.1000base_t1.2022";
pub const MINWAIT_NS: u64 = 975_000;
pub const MAXWAIT_NS: u64 = 97_500_000;
pub const MIN_INFO_FIELDS: u32 = 256;
pub const DATA_SIGNAL_LOSS_UI: u64 = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationConfig {
    pub endpoint: String,
    pub autoneg: bool,
    pub preferred_master: bool,
    pub force_role: bool,
    pub nonce_low4: u8,
    pub an_random_seed: u64,
    pub training_seed: u64,
    pub data_seed: u16,
}
impl StationConfig {
    pub fn validate(&self) -> Result<()> {
        if self.endpoint.is_empty()
            || self.endpoint.len() > 128
            || self.nonce_low4 > 15
            || self.training_seed == 0
            || self.training_seed >= (1u64 << 33)
            || self.data_seed == 0
            || self.data_seed > 0x7fff
            || !self.autoneg && !self.force_role
        {
            return Err("invalid 1000BASE-T1 station role/AN/scrambler seed".into());
        }
        Ok(())
    }
    pub fn preferred_role(&self) -> Role {
        if self.preferred_master {
            Role::Master
        } else {
            Role::Slave
        }
    }
    fn base_page(&self) -> t1_an::Config {
        t1_an::Config {
            advertised_page: super::an_hsm::BasePage::new_1000base_t1(
                self.nonce_low4 | (u8::from(self.preferred_master) << 4),
                self.force_role,
            ),
            random_seed: self.an_random_seed,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub stations: Vec<StationConfig>,
    /// Explicit ideal-reference echo removal.  This is a simulator receiver
    /// configuration and is not hardware echo-canceller performance evidence.
    pub ideal_reference_echo_cancellation: bool,
}
impl Config {
    pub fn validate(&self, endpoints: &[String]) -> Result<()> {
        if self.stations.len() != 2
            || endpoints.len() != 2
            || self
                .stations
                .iter()
                .zip(endpoints)
                .any(|(station, endpoint)| &station.endpoint != endpoint)
            || !self.ideal_reference_echo_cancellation
        {
            return Err("1000BASE-T1 requires two ordered endpoints and explicit ideal reference echo cancellation".into());
        }
        for station in &self.stations {
            station.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    AutoNegotiation,
    Silent,
    Training,
    Countdown,
    DataIdle,
    Data,
    Fault,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    An(AnEvent),
    TrainingInfo(InfoField),
    DataReady,
    ReceiveMac(ReceivedMac),
    ReceiveRaw(ReceivedRaw),
    FrameRejected(String),
    LinkLost,
    /// Recoverable startup failure; no queued MAC request has been transmitted.
    Restarting(String),
    Fault(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    config: StationConfig,
    phase: Phase,
    role: Option<Role>,
    an: Option<t1_an::Node>,
    startup_sample: u64,
    minwait_end_sample: u64,
    maxwait_end_sample: u64,
    training_tx: Option<TrainingTransmitter>,
    training_rx: Option<TrainingReceiver>,
    training_tx_start_partial: u32,
    slave_start: Option<(u64, u32)>,
    remote_info: Option<InfoField>,
    remote_data_seed: Option<u16>,
    local_receiver_ok: bool,
    local_enable_or_lock: bool,
    tx_fields_in_phase: u32,
    countdown_switch_pfc: Option<u32>,
    remote_switch_pfc: Option<u32>,
    data_tx: Option<TransmitStream>,
    data_rx: Option<ReceiveStream>,
    raw_merge: bool,
    raw_tx: Option<RawTransmitStream>,
    raw_rx: Option<RawReceiveStream>,
    data_idle_end_sample: u64,
    rx_raw_active: bool,
    #[serde(default)]
    rx_quiet_samples: u64,
    maximum_mac_bytes: usize,
}
impl Port {
    pub fn new(
        config: StationConfig,
        samples_per_ui: u32,
        maximum_mac_bytes: usize,
    ) -> Result<Self> {
        config.validate()?;
        let role = (!config.autoneg).then(|| config.preferred_role());
        let an = config
            .autoneg
            .then(|| t1_an::Node::new(config.base_page(), samples_per_ui))
            .transpose()?;
        let samples_per_ns = samples_per_ui as u64 * 3 / 4;
        let mut result = Self {
            config,
            phase: if an.is_some() {
                Phase::AutoNegotiation
            } else {
                Phase::Silent
            },
            role,
            an,
            startup_sample: 0,
            minwait_end_sample: MINWAIT_NS * samples_per_ns,
            maxwait_end_sample: MAXWAIT_NS * samples_per_ns,
            training_tx: None,
            training_rx: None,
            training_tx_start_partial: 0,
            slave_start: None,
            remote_info: None,
            remote_data_seed: None,
            local_receiver_ok: false,
            local_enable_or_lock: false,
            tx_fields_in_phase: 0,
            countdown_switch_pfc: None,
            remote_switch_pfc: None,
            data_tx: None,
            data_rx: None,
            raw_merge: false,
            raw_tx: None,
            raw_rx: None,
            data_idle_end_sample: 0,
            rx_raw_active: false,
            rx_quiet_samples: 0,
            maximum_mac_bytes,
        };
        // Forced SLAVE listens to the MASTER training stream during SILENT.
        if role == Some(Role::Slave) {
            result.minwait_end_sample = 0;
        }
        Ok(result)
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn endpoint(&self) -> &str {
        &self.config.endpoint
    }
    pub fn role(&self) -> Option<Role> {
        self.role
    }
    pub fn ready(&self) -> bool {
        self.phase == Phase::Data
    }
    /// Restart the complete Clause 98/97 startup after an observed receive
    /// loss. The new timers are anchored at the physical sample cut; no prior
    /// training seed, page state, partial frame, or receive assembly survives.
    pub fn restart(&mut self, at_sample: u64, samples_per_ui: u32) -> Result<()> {
        let raw_merge = self.raw_merge;
        let mut next = Self::new(self.config.clone(), samples_per_ui, self.maximum_mac_bytes)?;
        if raw_merge {
            next.enable_raw_merge()?;
        }
        next.startup_sample = at_sample;
        let unit = next.samples_per_ns(samples_per_ui);
        next.minwait_end_sample = if next.role == Some(Role::Slave) {
            at_sample
        } else {
            at_sample + MINWAIT_NS * unit
        };
        next.maxwait_end_sample = at_sample + MAXWAIT_NS * unit;
        *self = next;
        Ok(())
    }
    pub fn data_tx(&self) -> Option<&TransmitStream> {
        self.data_tx.as_ref()
    }
    pub fn enable_raw_merge(&mut self) -> Result<()> {
        if self.data_tx.is_some() || self.raw_tx.is_some() {
            return Err("1000BASE-T1 raw Merge mode must be selected before SEND_DATA".into());
        }
        self.raw_merge = true;
        Ok(())
    }
    pub fn raw_tx(&self) -> Option<&RawTransmitStream> {
        self.raw_tx.as_ref()
    }
    pub fn raw_enabled(&self) -> bool {
        self.raw_merge
    }
    pub fn raw_state_valid(&self) -> bool {
        if self.raw_merge {
            self.data_tx.is_none()
                && self.data_rx.is_none()
                && self.raw_tx.as_ref().is_none_or(RawTransmitStream::validate)
                && self.raw_rx.as_ref().is_none_or(RawReceiveStream::validate)
        } else {
            self.raw_tx.is_none() && self.raw_rx.is_none()
        }
    }
    pub fn raw_tx_mut(&mut self) -> Option<&mut RawTransmitStream> {
        self.raw_tx.as_mut()
    }
    pub fn raw_rx_started(&self) -> bool {
        self.raw_rx.is_some()
    }
    pub fn startup_sample(&self) -> u64 {
        self.startup_sample
    }
    pub fn raw_source_switch_pfc24(&self) -> Option<u32> {
        self.countdown_switch_pfc
    }
    pub fn raw_received_switch_pfc24(&self) -> Option<u32> {
        self.remote_switch_pfc
    }
    pub fn raw_received_data_seed(&self) -> Option<u16> {
        self.remote_data_seed
    }
    pub fn raw_source_data_seed(&self) -> u16 {
        self.config.data_seed
    }
    pub fn raw_initial_idle_verified(&self) -> bool {
        self.raw_rx
            .as_ref()
            .is_some_and(RawReceiveStream::initial_idle_verified)
    }
    pub fn raw_rx_symbols(&self) -> Option<u64> {
        self.raw_rx.as_ref().map(RawReceiveStream::symbols_received)
    }
    pub fn raw_rx_slots(&self) -> Option<u64> {
        self.raw_rx.as_ref().map(RawReceiveStream::slots_received)
    }
    pub fn data_rx_started(&self) -> bool {
        self.data_rx.is_some() || self.raw_rx.is_some()
    }
    pub fn data_tx_mut(&mut self) -> Option<&mut TransmitStream> {
        self.data_tx.as_mut()
    }
    pub fn enqueue_mac(&mut self, bytes: &[u8]) -> Result<SourceRange> {
        if !self.ready() {
            return Err("1000BASE-T1 link has not entered SEND_DATA".into());
        }
        self.data_tx
            .as_mut()
            .ok_or("missing 1000BASE-T1 transmit PCS")?
            .enqueue_mac(bytes, self.maximum_mac_bytes)
    }
    fn samples_per_ns(&self, samples_per_ui: u32) -> u64 {
        samples_per_ui as u64 * 3 / 4
    }
    fn enter_silent(&mut self, now: u64, samples_per_ui: u32) {
        self.phase = Phase::Silent;
        self.startup_sample = now;
        self.minwait_end_sample = now + MINWAIT_NS * self.samples_per_ns(samples_per_ui);
        self.maxwait_end_sample = now + MAXWAIT_NS * self.samples_per_ns(samples_per_ui);
        self.rx_raw_active = false;
    }
    fn start_training(&mut self, first_partial: u32) -> Result<()> {
        let role = self.role.ok_or("1000BASE-T1 role unresolved")?;
        self.training_tx = Some(TrainingTransmitter::new_aligned(
            role,
            self.config.training_seed,
            first_partial,
        )?);
        self.training_tx_start_partial = first_partial;
        self.phase = Phase::Training;
        self.tx_fields_in_phase = 0;
        Ok(())
    }
    fn info(&self) -> Result<InfoField> {
        let pfc24 = (self
            .training_tx
            .as_ref()
            .ok_or("missing PMA training transmitter")?
            .symbol()
            / PARTIAL_SYMBOLS) as u32;
        Ok(InfoField {
            role: self.role.ok_or("missing 1000BASE-T1 role")?,
            pfc24,
            state: if self.phase == Phase::Countdown {
                PmaState::Countdown
            } else {
                PmaState::Training
            },
            // Table 97-8 requires the first transmitted SLAVE field to have
            // loc_rcvr_status=0 even if timing/receiver training completed
            // before the SLAVE's first aligned transmit frame.
            local_receiver_ok: self.local_receiver_ok
                && self.local_enable_or_lock
                && (self.phase == Phase::Countdown
                    || self.role == Some(Role::Master)
                    || self.tx_fields_in_phase > 1),
            enable_or_timing_lock: self.local_enable_or_lock,
            data_scrambler_seed: self.config.data_seed,
            eee_capable: false,
            oam_capable: false,
            user_config: 0,
            data_switch_pfc24: self.countdown_switch_pfc.unwrap_or(0),
        })
    }
    fn begin_data_tx(&mut self, now: u64, samples_per_ui: u32) -> Result<()> {
        let role = self.role.ok_or("1000BASE-T1 data role unresolved")?;
        if self.raw_merge {
            self.raw_tx = Some(RawTransmitStream::new(role, self.config.data_seed)?);
        } else {
            self.data_tx = Some(TransmitStream::new(role, self.config.data_seed)?);
        }
        self.phase = Phase::DataIdle;
        self.data_idle_end_sample = now + MINWAIT_NS * self.samples_per_ns(samples_per_ui);
        Ok(())
    }
    /// Returns the instantaneous AN DME level on every sample, or the next
    /// PAM2/PAM3 symbol only at the beginning of a 750 MBd UI.  The caller
    /// renders the latter through the existing sampled Transmitter.
    pub fn drive(&mut self, sample: u64, samples_per_ui: u32) -> Result<(Option<i8>, Vec<Event>)> {
        let mut events = vec![];
        if sample >= self.maxwait_end_sample && !matches!(self.phase, Phase::Data | Phase::Fault) {
            self.restart(sample, samples_per_ui)?;
            events.push(Event::Restarting(
                "1000BASE-T1 maxwait_timer expired".into(),
            ));
        }
        if self.phase == Phase::AutoNegotiation {
            let an = self.an.as_mut().ok_or("missing Clause 98 AN state")?;
            return Ok((Some(an.drive_sample()?), events));
        }
        if sample % samples_per_ui as u64 != 0 {
            return Ok((None, events));
        }
        if self.phase == Phase::Silent
            && self.role == Some(Role::Master)
            && sample >= self.minwait_end_sample
        {
            self.start_training(0)?;
        }
        if self.phase == Phase::Silent && self.slave_start.is_some_and(|(start, _)| sample >= start)
        {
            let (_, first_partial) = self.slave_start.take().unwrap();
            self.start_training(first_partial)?;
        }
        if matches!(self.phase, Phase::Training | Phase::Countdown) {
            let symbol = self.training_tx.as_ref().unwrap().symbol();
            if symbol % SYMBOLS_PER_FRAME == 0
                && self.phase == Phase::Training
                && self.local_receiver_ok
                && self.local_enable_or_lock
                && self
                    .remote_info
                    .as_ref()
                    .is_some_and(|info| info.local_receiver_ok)
                && self.tx_fields_in_phase >= MIN_INFO_FIELDS
                && sample >= self.minwait_end_sample
            {
                self.phase = Phase::Countdown;
                self.tx_fields_in_phase = 0;
                let first_partial = (symbol / PARTIAL_SYMBOLS) as u32;
                self.countdown_switch_pfc =
                    Some(first_partial + MIN_INFO_FIELDS * PARTIALS_PER_FRAME as u32);
            }
            if self.phase == Phase::Countdown
                && self.countdown_switch_pfc == Some((symbol / PARTIAL_SYMBOLS) as u32)
            {
                self.begin_data_tx(sample, samples_per_ui)?;
            } else {
                let within = symbol % SYMBOLS_PER_FRAME;
                if within == 14 * PARTIAL_SYMBOLS {
                    self.tx_fields_in_phase += 1;
                    if self.role == Some(Role::Master)
                        && self.phase == Phase::Training
                        && self.tx_fields_in_phase >= MIN_INFO_FIELDS
                    {
                        self.local_enable_or_lock = true;
                    }
                }
                let info = self.info()?;
                let level = self.training_tx.as_mut().unwrap().next_symbol(&info)?;
                return Ok((Some(level), events));
            }
        }
        if matches!(self.phase, Phase::DataIdle | Phase::Data) {
            if self.phase == Phase::DataIdle
                && sample >= self.data_idle_end_sample
                && self.data_rx_started()
            {
                self.phase = Phase::Data;
                events.push(Event::DataReady);
            }
            return Ok((
                Some(if self.raw_merge {
                    self.raw_tx
                        .as_mut()
                        .ok_or("missing raw 1000BASE-T1 transmitter")?
                        .next_symbol()?
                } else {
                    self.data_tx
                        .as_mut()
                        .ok_or("missing 1000BASE-T1 data transmitter")?
                        .next_symbol()?
                }),
                events,
            ));
        }
        Ok((Some(0), events))
    }
    /// Advances local Clause 98 and Clause 97 receive controls from actual
    /// sampled partner voltage and a recovered 750 MBd symbol (if any).
    pub fn observe(
        &mut self,
        sample: u64,
        received_uv: i64,
        threshold_uv: i64,
        recovered_symbol: Option<i8>,
        samples_per_ui: u32,
    ) -> Result<Vec<Event>> {
        let mut events = vec![];
        if matches!(self.phase, Phase::Data | Phase::DataIdle) {
            self.rx_quiet_samples = if received_uv.unsigned_abs() < threshold_uv as u64 {
                self.rx_quiet_samples.saturating_add(1)
            } else {
                0
            };
            if self.rx_quiet_samples >= DATA_SIGNAL_LOSS_UI * samples_per_ui as u64 {
                self.restart(sample + 1, samples_per_ui)?;
                events.push(Event::LinkLost);
                return Ok(events);
            }
        }
        if self.phase == Phase::AutoNegotiation {
            let an = self.an.as_mut().ok_or("missing Clause 98 AN state")?;
            for event in an.observe_sample(received_uv, threshold_uv)? {
                events.push(Event::An(event));
            }
            if an.phase() == AnPhase::Complete {
                let selected = an
                    .selected_role()
                    .ok_or("Clause 98 completed without resolved role")?;
                if !events
                    .iter()
                    .any(|e| matches!(e, Event::An(AnEvent::Completed(_))))
                {
                    events.push(Event::An(AnEvent::Completed(selected)));
                }
                self.role = Some(selected);
                self.enter_silent(sample + 1, samples_per_ui);
            } else if an.phase() == AnPhase::Fault {
                self.phase = Phase::Fault;
            }
            return Ok(events);
        }
        if matches!(self.phase, Phase::Fault) {
            return Ok(events);
        }
        let raw_active = received_uv.unsigned_abs() >= threshold_uv as u64;
        if !self.rx_raw_active && raw_active && self.training_rx.is_none() {
            if let Some(role) = self.role {
                let partner_role = match role {
                    Role::Master => Role::Slave,
                    Role::Slave => Role::Master,
                };
                self.training_rx = Some(TrainingReceiver::new(partner_role));
            }
        }
        self.rx_raw_active = raw_active;
        let Some(symbol) = recovered_symbol else {
            return Ok(events);
        };
        if self.raw_rx.is_some() {
            match self.raw_rx.as_mut().unwrap().push_symbol(symbol) {
                Ok(packets) => events.extend(packets.into_iter().map(Event::ReceiveRaw)),
                Err(reason) => events.push(Event::FrameRejected(reason)),
            }
            return Ok(events);
        }
        if self.data_rx.is_some() {
            match self.data_rx.as_mut().unwrap().push_symbol(symbol) {
                Ok(frames) => events.extend(frames.into_iter().map(Event::ReceiveMac)),
                Err(reason) => events.push(Event::FrameRejected(reason)),
            }
            return Ok(events);
        }
        let (received_info, receiver_good, absolute_partial) = {
            let Some(rx) = self.training_rx.as_mut() else {
                return Ok(events);
            };
            let info = match rx.push_symbol(symbol) {
                Ok(info) => info,
                Err(reason) => {
                    self.restart(sample + 1, samples_per_ui)?;
                    events.push(Event::Restarting(reason));
                    return Ok(events);
                }
            };
            (
                info,
                rx.consecutive_good() >= SYMBOLS_PER_FRAME - 33,
                rx.absolute_partial_count(),
            )
        };
        if let Some(info) = received_info {
            self.remote_info = Some(info);
            events.push(Event::TrainingInfo(info));
            self.local_receiver_ok = receiver_good;
            if info.state == PmaState::Training {
                self.remote_data_seed = Some(info.data_scrambler_seed);
            }
            if self.role == Some(Role::Slave) && info.enable_or_timing_lock {
                self.local_enable_or_lock = true;
                if self.phase == Phase::Silent {
                    // First decoded MASTER InfoField has PFC24=14+15k.
                    // The SLAVE begins only at the next MASTER PHY-frame
                    // boundary, 84 PAM2 UIs after its 96th field bit.
                    let remaining = SYMBOLS_PER_FRAME - (14 * PARTIAL_SYMBOLS + INFO_BITS as u64);
                    self.slave_start = Some((
                        sample + 1 + remaining * samples_per_ui as u64,
                        info.pfc24 + 1,
                    ));
                    self.minwait_end_sample = sample
                        + 1
                        + remaining * samples_per_ui as u64
                        + MINWAIT_NS * self.samples_per_ns(samples_per_ui);
                }
            }
            if info.state == PmaState::Countdown {
                self.remote_switch_pfc = Some(info.data_switch_pfc24);
            }
        }
        if let Some(switch) = self.remote_switch_pfc {
            if absolute_partial == Some(switch) {
                let partner_role = rx_role(self.role.ok_or("missing local role")?);
                let seed = self
                    .remote_data_seed
                    .ok_or("partner data scrambler seed unavailable")?;
                if self.raw_merge {
                    self.raw_rx = Some(RawReceiveStream::new(
                        partner_role,
                        seed,
                        self.maximum_mac_bytes + 8,
                    )?);
                } else {
                    self.data_rx = Some(ReceiveStream::new(
                        partner_role,
                        seed,
                        self.maximum_mac_bytes,
                    )?);
                }
            }
        }
        Ok(events)
    }
}
fn rx_role(local: Role) -> Role {
    match local {
        Role::Master => Role::Slave,
        Role::Slave => Role::Master,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ethernet::crc32;
    fn station(endpoint: &str, master: bool) -> StationConfig {
        StationConfig {
            endpoint: endpoint.into(),
            autoneg: false,
            preferred_master: master,
            force_role: true,
            nonce_low4: if master { 3 } else { 2 },
            an_random_seed: 1,
            training_seed: if master { 0x1a2b_3c4d5 } else { 0x1234_56789 },
            data_seed: if master { 0x2255 } else { 0x315a },
        }
    }
    fn run_pair_until_ready(master: &mut Port, slave: &mut Port, start: u64, ui: u32) -> u64 {
        for symbol in 0..5_000_000u64 {
            let sample = start + symbol * ui as u64;
            let (mv, _) = master.drive(sample, ui).unwrap();
            let (sv, _) = slave.drive(sample, ui).unwrap();
            for event in master
                .observe(
                    sample + ui as u64 - 1,
                    sv.unwrap_or(0) as i64 * 1_000_000,
                    100_000,
                    sv,
                    ui,
                )
                .unwrap()
            {
                if let Event::Fault(reason) = event {
                    panic!("master training fault: {reason}");
                }
            }
            for event in slave
                .observe(
                    sample + ui as u64 - 1,
                    mv.unwrap_or(0) as i64 * 1_000_000,
                    100_000,
                    mv,
                    ui,
                )
                .unwrap()
            {
                if let Event::Fault(reason) = event {
                    panic!("slave training fault: {reason}");
                }
            }
            if master.ready() && slave.ready() {
                return sample + ui as u64;
            }
        }
        panic!("forced partner pair did not recover to SEND_DATA");
    }
    #[test]
    fn data_phase_restart_discards_partial_mac_and_retrains_from_new_sample_epoch() {
        let ui = 16;
        let mut master = Port::new(station("m", true), ui, 1518).unwrap();
        let mut slave = Port::new(station("s", false), ui, 1518).unwrap();
        let mut sample = run_pair_until_ready(&mut master, &mut slave, 0, ui);
        let mut stale = vec![0xa5; 60];
        stale.extend(crc32(&stale).to_le_bytes());
        master.enqueue_mac(&stale).unwrap();
        for _ in 0..64 {
            let (mv, _) = master.drive(sample, ui).unwrap();
            let (sv, _) = slave.drive(sample, ui).unwrap();
            assert!(
                !slave
                    .observe(
                        sample + ui as u64 - 1,
                        mv.unwrap_or(0) as i64 * 1_000_000,
                        100_000,
                        mv,
                        ui
                    )
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, Event::ReceiveMac(_)))
            );
            master
                .observe(
                    sample + ui as u64 - 1,
                    sv.unwrap_or(0) as i64 * 1_000_000,
                    100_000,
                    sv,
                    ui,
                )
                .unwrap();
            sample += ui as u64;
        }
        master.restart(sample, ui).unwrap();
        slave.restart(sample, ui).unwrap();
        assert_eq!(master.phase(), Phase::Silent);
        assert_eq!(slave.phase(), Phase::Silent);
        assert!(master.data_tx().is_none());
        assert!(!slave.data_rx_started());
        sample = run_pair_until_ready(&mut master, &mut slave, sample, ui);
        let mut fresh = vec![0x5a; 60];
        fresh.extend(crc32(&fresh).to_le_bytes());
        master.enqueue_mac(&fresh).unwrap();
        let mut recovered = vec![];
        for _ in 0..3 * SYMBOLS_PER_FRAME {
            let (mv, _) = master.drive(sample, ui).unwrap();
            let (sv, _) = slave.drive(sample, ui).unwrap();
            recovered.extend(
                slave
                    .observe(
                        sample + ui as u64 - 1,
                        mv.unwrap_or(0) as i64 * 1_000_000,
                        100_000,
                        mv,
                        ui,
                    )
                    .unwrap()
                    .into_iter()
                    .filter_map(|event| match event {
                        Event::ReceiveMac(frame) => Some(frame.bytes),
                        _ => None,
                    }),
            );
            master
                .observe(
                    sample + ui as u64 - 1,
                    sv.unwrap_or(0) as i64 * 1_000_000,
                    100_000,
                    sv,
                    ui,
                )
                .unwrap();
            sample += ui as u64;
            if !recovered.is_empty() {
                break;
            }
        }
        assert_eq!(recovered, vec![fresh]);
    }
    #[test]
    fn sampled_data_signal_outage_requires_full_128_ui_before_restart() {
        let ui = 16;
        let mut port = Port::new(station("m", true), ui, 1518).unwrap();
        port.phase = Phase::Data;
        port.data_tx = Some(TransmitStream::new(Role::Master, 0x2255).unwrap());
        port.data_rx = Some(ReceiveStream::new(Role::Slave, 0x315a, 1518).unwrap());
        for sample in 0..DATA_SIGNAL_LOSS_UI * ui as u64 - 1 {
            assert!(
                port.observe(sample, 0, 100_000, None, ui)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(port.phase(), Phase::Data);
        }
        assert_eq!(
            port.observe(DATA_SIGNAL_LOSS_UI * ui as u64 - 1, 0, 100_000, None, ui)
                .unwrap(),
            vec![Event::LinkLost]
        );
        assert_eq!(port.phase(), Phase::Silent);
        assert!(port.data_tx.is_none() && port.data_rx.is_none());
    }
    #[test]
    fn forced_partner_training_reaches_data_only_after_wire_infofield_exchange() {
        for ui in [16u32, 32] {
            let mut master = Port::new(station("m", true), ui, 1518).unwrap();
            let mut slave = Port::new(station("s", false), ui, 1518).unwrap();
            let mut master_fields = 0;
            let mut slave_fields = 0;
            let mut ready = [false; 2];
            let mut rejected = [0; 2];
            let mut continuation_sample = 0;
            for symbol in 0..5_000_000u64 {
                let sample = symbol * ui as u64;
                let (mv, master_drive_events) = master.drive(sample, ui).unwrap();
                let (sv, slave_drive_events) = slave.drive(sample, ui).unwrap();
                ready[0] |= master_drive_events
                    .iter()
                    .any(|e| matches!(e, Event::DataReady));
                ready[1] |= slave_drive_events
                    .iter()
                    .any(|e| matches!(e, Event::DataReady));
                for event in master
                    .observe(
                        sample + ui as u64 - 1,
                        sv.unwrap_or(0) as i64 * 1_000_000,
                        100_000,
                        sv,
                        ui,
                    )
                    .unwrap()
                {
                    if matches!(event, Event::TrainingInfo(_)) {
                        master_fields += 1;
                    }
                    if matches!(event, Event::DataReady) {
                        ready[0] = true;
                    }
                    if matches!(event, Event::FrameRejected(_)) {
                        rejected[0] += 1;
                    }
                    if let Event::Fault(reason) = event {
                        panic!("master: {reason}");
                    }
                }
                for event in slave
                    .observe(
                        sample + ui as u64 - 1,
                        mv.unwrap_or(0) as i64 * 1_000_000,
                        100_000,
                        mv,
                        ui,
                    )
                    .unwrap()
                {
                    if matches!(event, Event::TrainingInfo(_)) {
                        slave_fields += 1;
                    }
                    if matches!(event, Event::DataReady) {
                        ready[1] = true;
                    }
                    if matches!(event, Event::FrameRejected(_)) {
                        rejected[1] += 1;
                    }
                    if let Event::Fault(reason) = event {
                        panic!("slave: {reason}");
                    }
                }
                if master.ready() && slave.ready() {
                    continuation_sample = sample + ui as u64;
                    break;
                }
            }
            assert!(master_fields >= MIN_INFO_FIELDS);
            assert!(slave_fields >= MIN_INFO_FIELDS);
            assert_eq!(
                ready,
                [true, true],
                "phase={:?}/{:?}, fields={master_fields}/{slave_fields}, rejected={rejected:?}",
                master.phase(),
                slave.phase()
            );
            assert!(master.ready() && slave.ready());
            eprintln!("forced pair both SEND_DATA at sample={continuation_sample} ui={ui}");
            assert_eq!(rejected, [0, 0]);
            let mut mac = vec![0xa5; 60];
            mac.extend(crc32(&mac).to_le_bytes());
            let range = master.enqueue_mac(&mac).unwrap();
            assert!(range.coded_end_symbol > range.fcs_end_symbol());
            let mut recovered = vec![];
            for symbol in 0..3 * SYMBOLS_PER_FRAME {
                let sample = continuation_sample + symbol * ui as u64;
                let (mv, _) = master.drive(sample, ui).unwrap();
                let (sv, _) = slave.drive(sample, ui).unwrap();
                for event in slave
                    .observe(
                        sample + ui as u64 - 1,
                        mv.unwrap_or(0) as i64 * 1_000_000,
                        100_000,
                        mv,
                        ui,
                    )
                    .unwrap()
                {
                    match event {
                        Event::ReceiveMac(frame) => recovered.push(frame.bytes),
                        Event::FrameRejected(reason) => panic!("PAM3/RS: {reason}"),
                        _ => {}
                    }
                }
                master
                    .observe(
                        sample + ui as u64 - 1,
                        sv.unwrap_or(0) as i64 * 1_000_000,
                        100_000,
                        sv,
                        ui,
                    )
                    .unwrap();
                if !recovered.is_empty() {
                    break;
                }
            }
            assert_eq!(recovered, vec![mac]);
        }
    }
}
