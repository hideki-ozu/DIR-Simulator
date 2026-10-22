//! Clause 148 PLCA RS control across a sampled 10BASE-T1S segment.
//!
//! Each node runs its own Control, Data and Status state. Opportunity counters
//! advance from received BEACON/COMMIT/carrier observations, not from a domain
//! scheduler assigning ownership. The MAC frame queue is the Data delay line:
//! bytes are retained until Control grants a transmit opportunity.
use super::super::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const STATUS_TIMER_BITS: u64 = 130_090;
pub const BEACON_BITS: u64 = 20;
pub const BEACON_DETECT_BITS: u64 = 22;
pub const COMMIT_BITS: u64 = 288;
pub const PENDING_BITS: u64 = 512;
pub const INVALID_BEACON_BITS: u64 = 40;
pub const DELAY_LINE_BITS: u64 = 99 * 4;
/// A borrowed observation of the existing TX oscillator, with no clock state.
#[derive(Clone, Copy, Debug)]
pub struct MiiClock {
    sample: u64,
    period: u128,
    rate: u128,
}
impl MiiClock {
    pub(crate) fn new(sample: u64, samples_per_mac_bit: u64, clock_ppm: i32) -> Self {
        Self {
            sample,
            period: 4 * samples_per_mac_bit as u128 * 1_000_000,
            rate: (1_000_000i64 + clock_ppm as i64) as u128,
        }
    }
    fn edge(&self, lead: u128) -> bool {
        if self.sample == 0 {
            return lead == 0;
        }
        (self.sample as u128 * self.rate + lead) / self.period
            != ((self.sample - 1) as u128 * self.rate + lead) / self.period
    }
    pub(crate) fn mcd(&self) -> bool {
        self.edge(0)
    }
    pub(crate) fn pmcd(&self) -> bool {
        self.edge(self.period / 4)
    }
    pub(crate) fn after_nibbles(&self, start: u64, count: u64) -> u64 {
        let cycle = start as u128 * self.rate / self.period;
        ((cycle + count as u128) * self.period)
            .div_ceil(self.rate)
            .min(u64::MAX as u128) as u64
    }
    fn pmcd_after_nibbles(&self, count: u64) -> u64 {
        let lead = self.period / 4;
        let cycle = (self.sample as u128 * self.rate + lead) / self.period;
        (((cycle + count as u128) * self.period - lead).div_ceil(self.rate)).min(u64::MAX as u128)
            as u64
    }
    pub(crate) fn bit_samples(&self, bits: u64) -> u64 {
        (bits as u128 * self.period / 4)
            .div_ceil(self.rate)
            .min(u64::MAX as u128) as u64
    }
}
fn is_true(value: &bool) -> bool {
    *value
}
fn default_enabled() -> bool {
    true
}
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationConfig {
    pub endpoint: String,
    /// 255 disables Clause 148 on this endpoint.
    pub node_id: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    pub stations: Vec<StationConfig>,
    /// Coordinator's node-count attribute; sparse IDs are permitted.
    pub node_count: u8,
    pub to_timer_bits: u8,
    pub burst_timer_bits: u8,
    pub max_burst_count: u8,
    /// Explicit fault injection: two configured nodes may claim one TO.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fault_duplicate_ids: bool,
}
impl DomainConfig {
    pub fn validate(&self, endpoints: &[String]) -> Result<()> {
        let configured: Vec<_> = self.stations.iter().map(|x| x.endpoint.as_str()).collect();
        let expected: Vec<_> = endpoints.iter().map(String::as_str).collect();
        let ids: BTreeSet<_> = self
            .stations
            .iter()
            .filter(|x| x.node_id != 255)
            .map(|x| x.node_id)
            .collect();
        if configured != expected
            || self.node_count == 0
            || self.to_timer_bits == 0
            || (!self.fault_duplicate_ids
                && ids.len() != self.stations.iter().filter(|x| x.node_id != 255).count())
            || self
                .stations
                .iter()
                .any(|x| x.node_id != 255 && x.node_id >= self.node_count)
        {
            return Err("PLCA needs endpoint-ordered unique IDs below node_count, a nonzero opportunity timer and node_count; ID255 disables a station".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlState {
    Disable,
    Recover,
    SendBeacon,
    Syncing,
    Resync,
    WaitTo,
    EarlyReceive,
    Receive,
    Commit,
    Transmit,
    Burst,
    Yield,
    Abort,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataState {
    Normal,
    Idle,
    Receive,
    Hold,
    Transmit,
    Collide,
    DelayPending,
    Pending,
    WaitMac,
    Flush,
    WaitIdle,
    Abort,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Inactive,
    Active,
    Hysteresis,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// Transient view derived by the existing sampled source adapter.
    #[serde(skip)]
    pub clock: Option<MiiClock>,
    pub carrier: bool,
    pub beacon: bool,
    pub commit: bool,
    pub receiving_data: bool,
    pub queued_packet: bool,
    pub transmitting: bool,
    /// Source MAC TX_EN has ended while delayed PHY data/ESD may remain.
    #[serde(default)]
    pub mac_input_ended: bool,
    /// The existing Program has forwarded the final retained MAC nibble.
    #[serde(default)]
    pub mac_output_ended: bool,
    /// Free-running MII nibble-clock observation; false is the legacy edge.
    #[serde(default)]
    pub between_mac_clocks: bool,
    #[serde(default)]
    pub between_prescient_mac_clocks: bool,
    /// The existing PCS is still draining a control group at this MII edge.
    #[serde(default)]
    pub forwarding_blocked: bool,
    pub beacon_driving: bool,
    pub collision: bool,
    #[serde(default)]
    pub jamming: bool,
    /// Actual MAC IFG/backoff or administrative permission inhibits a new attempt.
    #[serde(default)]
    pub mac_deferred: bool,
    #[serde(default)]
    pub transmit_error: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub send_beacon: bool,
    pub permit_packet: bool,
    pub status_ok: bool,
    pub signal_error: bool,
    /// Only a pre-forwarding COLLIDE asks the existing MAC to start JAM.
    pub force_collision: bool,
    pub carrier_hold: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub config: StationConfig,
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    pub enabled: bool,
    pub control: ControlState,
    pub data: DataState,
    pub status: StatusState,
    pub cur_id: u8,
    /// Countdown in physical samples, kept across checkpoints.
    pub timer_samples: u64,
    pub status_timer_samples: u64,
    pub burst_count: u8,
    pub committed: bool,
    pub packet_pending: bool,
    pub observed_beacon: bool,
    pub observed_commit: bool,
    pub observed_carrier: bool,
    pub observed_rx_data: bool,
    pub observed_collision: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub signal_error: bool,
    pub to_timer_bits: u8,
    pub node_count: u8,
    pub burst_timer_bits: u8,
    pub max_burst_count: u8,
    pub samples_per_mac_bit: u64,
    #[serde(default)]
    pub data_timer_samples: u64,
    #[serde(default)]
    pub hold_samples: u64,
    #[serde(default)]
    pub invalid_beacon_samples: u64,
}
impl Node {
    pub fn new(
        station: StationConfig,
        domain: &DomainConfig,
        samples_per_mac_bit: u64,
    ) -> Result<Self> {
        if samples_per_mac_bit == 0 {
            return Err("invalid PLCA MAC bit sample count".into());
        }
        let control = if station.node_id == 255 {
            ControlState::Disable
        } else if station.node_id == 0 {
            ControlState::Recover
        } else {
            ControlState::Resync
        };
        Ok(Self {
            config: station,
            enabled: true,
            control,
            data: DataState::Normal,
            status: StatusState::Inactive,
            cur_id: 0,
            timer_samples: (domain.node_count as u64 * domain.to_timer_bits as u64 + BEACON_BITS)
                * samples_per_mac_bit,
            status_timer_samples: 0,
            burst_count: 0,
            committed: false,
            packet_pending: false,
            observed_beacon: false,
            observed_commit: false,
            observed_carrier: false,
            observed_rx_data: false,
            observed_collision: false,
            signal_error: false,
            to_timer_bits: domain.to_timer_bits,
            node_count: domain.node_count,
            burst_timer_bits: domain.burst_timer_bits,
            max_burst_count: domain.max_burst_count,
            samples_per_mac_bit,
            data_timer_samples: 0,
            hold_samples: 0,
            invalid_beacon_samples: 0,
        })
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled && self.control == ControlState::Disable {
            self.timer_samples = (self.node_count as u64 * self.to_timer_bits as u64 + BEACON_BITS)
                * self.samples_per_mac_bit;
        }
    }
    fn bit_samples(&self, bits: u64, observed: Observation) -> u64 {
        observed
            .clock
            .map_or(bits * self.samples_per_mac_bit, |clock| {
                clock.bit_samples(bits)
            })
    }
    fn enter_wait_to(&mut self, observed: Observation) {
        self.control = ControlState::WaitTo;
        self.timer_samples = self.bit_samples(self.to_timer_bits as u64, observed);
        self.committed = false;
    }
    fn next_opportunity(&mut self, observed: Observation) {
        self.committed = false;
        self.cur_id = self.cur_id.saturating_add(1);
        if self.config.node_id == 0 && self.cur_id >= self.node_count || self.cur_id == 255 {
            self.control = if self.config.node_id == 0 {
                ControlState::Recover
            } else {
                ControlState::Resync
            };
            if self.config.node_id == 0 {
                // A regular completed cycle starts the next BEACON promptly.
                // Initial startup and false-carrier recovery retain the full
                // one-cycle guard set in those transitions.
                self.timer_samples = 0;
            }
        } else {
            self.enter_wait_to(observed);
        }
    }
    fn update_status(&mut self, observed: Observation) {
        let active = match self.control {
            ControlState::Disable | ControlState::Recover | ControlState::Resync => false,
            ControlState::EarlyReceive => self.status == StatusState::Active,
            _ => true,
        };
        match (self.status, active) {
            (StatusState::Inactive, true) | (StatusState::Hysteresis, true) => {
                self.status = StatusState::Active;
            }
            (StatusState::Active, false) => {
                self.status = StatusState::Hysteresis;
                self.status_timer_samples = self.bit_samples(STATUS_TIMER_BITS, observed);
            }
            (StatusState::Hysteresis, false) => {
                self.status_timer_samples = self.status_timer_samples.saturating_sub(1);
                if self.status_timer_samples == 0 {
                    self.status = StatusState::Inactive;
                }
            }
            _ => {}
        }
    }
    /// One physical sample. The observation is from the local receive path at
    /// the preceding consumed sample; no knowledge of other queues is used.
    pub fn step(&mut self, observed: Observation) -> Decision {
        let mut decision = Decision::default();
        self.observed_beacon = observed.beacon;
        self.observed_commit = observed.commit;
        self.observed_carrier = observed.carrier;
        self.observed_rx_data = observed.receiving_data;
        self.observed_collision = observed.collision;
        // Data's packetPending, not the application queue, drives Control.
        // A fresh MAC request may begin HOLD on this sample; a retry remains
        // suppressed throughout JAM and the pending delay.
        self.packet_pending = matches!(self.data, DataState::Hold | DataState::Pending);
        if self.config.node_id == 255 || !self.enabled {
            self.control = ControlState::Disable;
            self.data = DataState::Normal;
            self.status = StatusState::Inactive;
            self.committed = false;
            self.cur_id = 0;
            self.packet_pending = false;
            self.data_timer_samples = 0;
            self.hold_samples = 0;
            decision.permit_packet = true;
            decision.signal_error = observed.collision;
            self.signal_error = decision.signal_error;
            return decision;
        }
        match self.control {
            ControlState::Disable => {
                self.control = if self.config.node_id == 0 {
                    ControlState::Recover
                } else {
                    ControlState::Resync
                };
            }
            ControlState::Recover => {
                self.timer_samples = self.timer_samples.saturating_sub(1);
                if self.timer_samples == 0
                    && !observed.carrier
                    && !observed.transmitting
                    && !observed.between_prescient_mac_clocks
                {
                    // A BEACON begins a new local TO cycle. Leaving the
                    // previous cycle's terminal cur_id in place made the
                    // coordinator issue the next BEACON after only one TO,
                    // while other stations were entering their own TOs.
                    self.cur_id = 0;
                    self.control = ControlState::SendBeacon;
                    self.timer_samples = observed
                        .clock
                        .map_or(BEACON_BITS * self.samples_per_mac_bit, |clock| {
                            clock.pmcd_after_nibbles(5).saturating_sub(clock.sample)
                        });
                    decision.send_beacon = true;
                }
            }
            ControlState::SendBeacon => {
                self.timer_samples = self.timer_samples.saturating_sub(1);
                if self.timer_samples == 0 {
                    self.control = ControlState::Syncing;
                }
            }
            ControlState::Resync => {
                if observed.beacon {
                    self.cur_id = 0;
                    self.control = ControlState::Syncing;
                } else if observed.carrier {
                    self.control = ControlState::EarlyReceive;
                    self.timer_samples = self.bit_samples(BEACON_DETECT_BITS, observed);
                }
            }
            ControlState::Syncing => {
                if observed.beacon {
                    self.invalid_beacon_samples = 0;
                } else if !observed.carrier {
                    self.invalid_beacon_samples = 0;
                    self.enter_wait_to(observed);
                } else if self.config.node_id != 0 {
                    self.invalid_beacon_samples += 1;
                    if self.invalid_beacon_samples >= INVALID_BEACON_BITS * self.samples_per_mac_bit
                    {
                        self.control = ControlState::Resync;
                        self.committed = false;
                        self.invalid_beacon_samples = 0;
                    }
                }
            }
            ControlState::WaitTo => {
                if observed.beacon && self.config.node_id != 0 {
                    self.cur_id = 0;
                    self.control = ControlState::Syncing;
                } else if observed.carrier {
                    self.control = ControlState::EarlyReceive;
                    self.timer_samples = self.bit_samples(BEACON_DETECT_BITS, observed);
                } else if self.cur_id == self.config.node_id {
                    if self.packet_pending {
                        self.control = ControlState::Commit;
                        self.committed = true;
                        self.burst_count = 0;
                        decision.permit_packet = true;
                    } else {
                        self.control = ControlState::Yield;
                    }
                } else {
                    self.timer_samples = self.timer_samples.saturating_sub(1);
                    if self.timer_samples == 0 {
                        self.next_opportunity(observed);
                    }
                }
            }
            ControlState::EarlyReceive => {
                self.timer_samples = self.timer_samples.saturating_sub(1);
                if observed.beacon && self.config.node_id != 0 {
                    self.cur_id = 0;
                    self.control = ControlState::Syncing;
                } else if (observed.commit || observed.receiving_data) && observed.carrier {
                    self.control = ControlState::Receive;
                } else if !observed.carrier && self.config.node_id == 0 {
                    self.control = ControlState::Recover;
                    self.timer_samples = self.bit_samples(
                        self.node_count as u64 * self.to_timer_bits as u64 + BEACON_BITS,
                        observed,
                    );
                } else if !observed.carrier && self.config.node_id != 0 {
                    if self.timer_samples == 0 {
                        self.next_opportunity(observed);
                    } else {
                        self.cur_id = 0;
                        self.control = ControlState::Syncing;
                    }
                }
            }
            ControlState::Receive => {
                if !observed.carrier {
                    self.next_opportunity(observed);
                }
            }
            ControlState::Commit => {
                decision.permit_packet = observed.queued_packet;
                if observed.transmitting {
                    self.control = ControlState::Transmit;
                    if self.burst_count >= self.max_burst_count {
                        self.committed = false;
                    }
                } else if !self.packet_pending && self.data != DataState::WaitMac {
                    self.control = ControlState::Abort;
                    self.committed = false;
                }
            }
            ControlState::Transmit => {
                if !observed.transmitting {
                    if self.burst_count < self.max_burst_count {
                        self.control = ControlState::Burst;
                        self.timer_samples =
                            self.bit_samples(self.burst_timer_bits as u64, observed);
                        self.burst_count += 1;
                    } else if !observed.carrier {
                        self.next_opportunity(observed);
                    }
                }
            }
            ControlState::Burst => {
                self.timer_samples = self.timer_samples.saturating_sub(1);
                if observed.transmitting {
                    self.control = ControlState::Transmit;
                    if self.burst_count >= self.max_burst_count {
                        self.committed = false;
                    }
                } else if self.timer_samples == 0 {
                    self.control = ControlState::Abort;
                    self.committed = false;
                }
            }
            ControlState::Yield => {
                self.timer_samples = self.timer_samples.saturating_sub(1);
                if self.timer_samples == 0 {
                    self.next_opportunity(observed);
                } else if observed.carrier {
                    self.control = ControlState::EarlyReceive;
                    self.timer_samples = self.bit_samples(BEACON_DETECT_BITS, observed);
                }
            }
            ControlState::Abort => {
                self.committed = false;
                if !observed.carrier && !observed.transmitting {
                    self.next_opportunity(observed);
                }
            }
        }
        self.update_status(observed);
        decision.status_ok = self.status != StatusState::Inactive;
        self.step_data(observed, &mut decision);
        self.signal_error = decision.signal_error;
        decision
    }
    pub fn mac_carrier_held(&self) -> bool {
        matches!(
            self.data,
            DataState::Hold
                | DataState::Collide
                | DataState::DelayPending
                | DataState::Pending
                | DataState::Transmit
                | DataState::Flush
                | DataState::Abort
        )
    }
    fn step_data(&mut self, observed: Observation, decision: &mut Decision) {
        if !decision.status_ok {
            self.data = DataState::Normal;
            self.packet_pending = false;
            self.data_timer_samples = 0;
            self.hold_samples = 0;
            decision.permit_packet = true;
            decision.signal_error = observed.collision;
            return;
        }
        decision.permit_packet = false;
        let receiving = observed.receiving_data || observed.commit;
        match self.data {
            DataState::Normal => self.data = DataState::Idle,
            DataState::Idle => {
                self.hold_samples = 0;
                if observed.queued_packet && !observed.mac_deferred {
                    self.data = if observed.carrier && !self.committed {
                        DataState::Collide
                    } else {
                        DataState::Hold
                    };
                } else if receiving && !self.committed {
                    self.data = DataState::Receive;
                }
            }
            DataState::Receive => {
                if !receiving {
                    self.data = DataState::Idle;
                } else if observed.queued_packet && !observed.mac_deferred {
                    self.data = DataState::Hold;
                }
            }
            DataState::Hold => {
                if observed.transmitting {
                    // Forwarding already began at the preceding physical cut.
                    self.data = DataState::Transmit;
                    if observed.collision {
                        self.hold_samples = 0;
                    }
                } else {
                    self.hold_samples = self.hold_samples.saturating_add(1);
                    if observed.transmit_error || !observed.queued_packet {
                        self.data = DataState::Abort;
                    } else if observed.collision
                        || receiving && !self.committed
                        || !self.committed
                            && observed.clock.map_or(
                                self.hold_samples >= DELAY_LINE_BITS * self.samples_per_mac_bit,
                                |clock| {
                                    clock.sample
                                        >= clock.after_nibbles(
                                            clock.sample.saturating_sub(self.hold_samples),
                                            99,
                                        )
                                },
                            )
                    {
                        self.data = DataState::Collide;
                    } else if self.committed {
                        decision.permit_packet = true;
                    }
                }
            }
            DataState::Transmit => {
                if observed.collision {
                    self.hold_samples = 0;
                }
                if (observed.mac_input_ended || !observed.transmitting && !observed.jamming)
                    && !observed.between_mac_clocks
                {
                    self.data = if self.hold_samples == 0 {
                        DataState::WaitIdle
                    } else {
                        DataState::Flush
                    };
                }
            }
            DataState::Flush => {
                // TX_EN ends after the retained tail; the same PCS Program
                // may still be producing ESD without further MAC nibbles.
                if (observed.mac_output_ended || !observed.transmitting)
                    && !observed.between_mac_clocks
                {
                    self.data = DataState::WaitIdle;
                }
            }
            DataState::WaitIdle => {
                self.hold_samples = 0;
                if !observed.between_mac_clocks {
                    if !observed.carrier {
                        self.data = DataState::Idle;
                    } else if observed.queued_packet
                        && !observed.mac_deferred
                        && !observed.forwarding_blocked
                    {
                        self.data = DataState::Transmit;
                        decision.permit_packet = true;
                    }
                }
            }
            DataState::Collide => {
                // ELSE re-enters COLLIDE and restarts pending_timer while
                // the MAC sends JAM. Its 512 bits run in DELAY_PENDING.
                self.data_timer_samples = self.bit_samples(PENDING_BITS, observed);
                self.hold_samples = 0;
                if !observed.transmitting && !observed.jamming || observed.mac_input_ended {
                    self.data = DataState::DelayPending;
                }
            }
            DataState::DelayPending => {
                self.data_timer_samples = self.data_timer_samples.saturating_sub(1);
                if self.data_timer_samples == 0 {
                    self.data = DataState::Pending;
                }
            }
            DataState::Pending => {
                self.data_timer_samples = self.bit_samples(COMMIT_BITS, observed);
                if self.committed {
                    self.data = DataState::WaitMac;
                    self.data_timer_samples = self.bit_samples(COMMIT_BITS, observed);
                }
            }
            DataState::WaitMac => {
                self.data_timer_samples = self.data_timer_samples.saturating_sub(1);
                if observed.queued_packet
                    && !observed.mac_deferred
                    && !observed.between_mac_clocks
                    && !observed.forwarding_blocked
                {
                    self.data = DataState::Transmit;
                    self.hold_samples = 0;
                    decision.permit_packet = true;
                } else if self.data_timer_samples == 0 {
                    self.data = DataState::WaitIdle;
                }
            }
            DataState::Abort => {
                if !observed.transmitting && !observed.queued_packet {
                    self.data = DataState::WaitIdle;
                }
            }
        }
        if self.data == DataState::Collide {
            self.data_timer_samples = self.bit_samples(PENDING_BITS, observed);
        }
        self.packet_pending = matches!(self.data, DataState::Hold | DataState::Pending);
        decision.signal_error = self.data == DataState::Collide
            || matches!(self.data, DataState::Transmit | DataState::Flush) && observed.collision;
        decision.force_collision = self.data == DataState::Collide && !observed.jamming;
        decision.carrier_hold = self.mac_carrier_held();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> DomainConfig {
        DomainConfig {
            stations: vec![
                StationConfig {
                    endpoint: "p0".into(),
                    node_id: 0,
                },
                StationConfig {
                    endpoint: "p1".into(),
                    node_id: 1,
                },
                StationConfig {
                    endpoint: "p2".into(),
                    node_id: 3,
                },
            ],
            node_count: 4,
            to_timer_bits: 32,
            burst_timer_bits: 128,
            max_burst_count: 1,
            fault_duplicate_ids: false,
        }
    }
    #[test]
    fn sparse_ids_and_disabled_node_validate() {
        let mut c = config();
        c.validate(&["p0".into(), "p1".into(), "p2".into()])
            .unwrap();
        c.stations[2].node_id = 255;
        c.validate(&["p0".into(), "p1".into(), "p2".into()])
            .unwrap();
        c.stations[2].node_id = 1;
        assert!(
            c.validate(&["p0".into(), "p1".into(), "p2".into()])
                .is_err()
        );
        c.fault_duplicate_ids = true;
        c.validate(&["p0".into(), "p1".into(), "p2".into()])
            .unwrap();
    }
    #[test]
    fn received_beacon_controls_local_opportunity_not_domain_queue() {
        let c = config();
        let mut node = Node::new(c.stations[1].clone(), &c, 40).unwrap();
        assert_eq!(node.control, ControlState::Resync);
        assert!(
            node.step(Observation {
                queued_packet: true,
                ..Default::default()
            })
            .permit_packet
        );
        node.step(Observation {
            carrier: true,
            beacon: true,
            queued_packet: true,
            ..Default::default()
        });
        assert_eq!(node.control, ControlState::Syncing);
        node.step(Observation {
            queued_packet: true,
            ..Default::default()
        });
        assert_eq!(node.control, ControlState::WaitTo);
        for _ in 0..c.to_timer_bits as u64 * 40 {
            node.step(Observation {
                queued_packet: true,
                ..Default::default()
            });
        }
        assert_eq!(node.cur_id, 1);
        assert!(
            node.step(Observation {
                queued_packet: true,
                ..Default::default()
            })
            .permit_packet
        );
        assert_eq!(node.control, ControlState::Commit);
    }
    #[test]
    fn missing_coordinator_reverts_to_csma_after_status_hysteresis() {
        let c = config();
        let mut node = Node::new(c.stations[1].clone(), &c, 1).unwrap();
        assert!(
            node.step(Observation {
                queued_packet: true,
                ..Default::default()
            })
            .permit_packet
        );
        node.step(Observation {
            carrier: true,
            beacon: true,
            ..Default::default()
        });
        node.step(Observation::default());
        assert_eq!(node.control, ControlState::WaitTo);
        assert_eq!(node.status, StatusState::Active);
        assert!(
            !node
                .step(Observation {
                    queued_packet: true,
                    ..Default::default()
                })
                .permit_packet
        );
        let mut recovered = false;
        for _ in 0..STATUS_TIMER_BITS + 255 * c.to_timer_bits as u64 + 100 {
            node.step(Observation::default());
            if node.status == StatusState::Inactive {
                assert_eq!(node.control, ControlState::Resync);
                assert!(
                    node.step(Observation {
                        queued_packet: true,
                        ..Default::default()
                    })
                    .permit_packet
                );
                recovered = true;
                break;
            }
        }
        assert!(recovered);
    }
    #[test]
    fn carrier_without_beacon_does_not_create_a_received_opportunity() {
        let c = config();
        let mut node = Node::new(c.stations[1].clone(), &c, 1).unwrap();
        node.step(Observation {
            carrier: true,
            beacon: true,
            ..Default::default()
        });
        node.step(Observation::default());
        assert_eq!(node.control, ControlState::WaitTo);
        node.step(Observation {
            carrier: true,
            ..Default::default()
        });
        assert_eq!(node.control, ControlState::EarlyReceive);
        for _ in 0..BEACON_DETECT_BITS {
            node.step(Observation {
                carrier: true,
                ..Default::default()
            });
        }
        assert_eq!(node.control, ControlState::EarlyReceive);
        node.step(Observation::default());
        assert_eq!(node.control, ControlState::WaitTo);
        assert_eq!(node.cur_id, 1);
        assert!(!node.committed);
    }
    #[test]
    fn pending_delay_commit_timeout_and_invalid_beacon_have_literal_bit_boundaries() {
        let c = config();
        let mut node = Node::new(c.stations[1].clone(), &c, 2).unwrap();
        node.control = ControlState::Receive;
        node.status = StatusState::Active;
        node.data = DataState::Hold;
        node.hold_samples = 396 * 2 - 1;
        let queued = Observation {
            queued_packet: true,
            mac_deferred: true,
            carrier: true,
            ..Default::default()
        };
        assert!(node.step(queued).force_collision);
        assert_eq!(node.data, DataState::Collide);
        assert_eq!(node.data_timer_samples, 1024);
        for _ in 0..63 {
            let d = node.step(Observation {
                jamming: true,
                ..queued
            });
            assert!(d.signal_error);
            assert!(!d.force_collision);
            assert_eq!(node.data, DataState::Collide);
        }
        node.step(queued);
        assert_eq!(node.data, DataState::DelayPending);
        assert_eq!(node.data_timer_samples, 1024);
        for _ in 0..1023 {
            node.step(queued);
            assert_eq!(node.data, DataState::DelayPending);
        }
        node.step(queued);
        assert_eq!(node.data, DataState::Pending);
        assert!(node.packet_pending);
        node.control = ControlState::Commit;
        node.committed = true;
        node.step(queued);
        assert_eq!(node.data, DataState::WaitMac);
        assert_eq!(node.data_timer_samples, 576);
        for _ in 0..575 {
            node.step(queued);
            assert_eq!(node.data, DataState::WaitMac);
        }
        node.step(queued);
        assert_eq!(node.data, DataState::WaitIdle);
        node.step(queued);
        assert!(!node.committed);
        assert_eq!(node.control, ControlState::Abort);
        let mut invalid = Node::new(c.stations[1].clone(), &c, 2).unwrap();
        invalid.step(Observation {
            beacon: true,
            carrier: true,
            ..Default::default()
        });
        for _ in 0..79 {
            invalid.step(Observation {
                carrier: true,
                ..Default::default()
            });
            assert_eq!(invalid.control, ControlState::Syncing);
        }
        invalid.step(Observation {
            carrier: true,
            ..Default::default()
        });
        assert_eq!(invalid.control, ControlState::Resync);
        assert!(!invalid.committed);
    }
    #[test]
    fn physical_col_latches_zero_delay_without_forcing_pending_or_restarting_mac_jam() {
        let c = config();
        let mut node = Node::new(c.stations[1].clone(), &c, 2).unwrap();
        node.control = ControlState::Transmit;
        node.status = StatusState::Active;
        node.data = DataState::Transmit;
        node.hold_samples = 80;
        let active = Observation {
            transmitting: true,
            queued_packet: true,
            carrier: true,
            ..Default::default()
        };
        let d = node.step(Observation {
            collision: true,
            ..active
        });
        assert_eq!(node.data, DataState::Transmit);
        assert_eq!(node.hold_samples, 0);
        assert!(d.signal_error);
        assert!(!d.force_collision);
        assert!(d.carrier_hold);
        let d = node.step(Observation {
            jamming: true,
            between_mac_clocks: true,
            ..active
        });
        assert_eq!(node.data, DataState::Transmit);
        assert_eq!(node.hold_samples, 0);
        assert!(!d.signal_error);
        assert!(!d.force_collision);
        node.step(Observation {
            jamming: true,
            mac_input_ended: true,
            between_mac_clocks: true,
            ..active
        });
        assert_eq!(
            node.data,
            DataState::Transmit,
            "MAC-off is sampled only at MCD"
        );
        node.step(Observation {
            jamming: true,
            mac_input_ended: true,
            ..active
        });
        assert_eq!(node.data, DataState::WaitIdle);
        assert_eq!(node.data_timer_samples, 0);
        assert!(!node.packet_pending);
        let mut boundary = Node::new(c.stations[1].clone(), &c, 2).unwrap();
        boundary.max_burst_count = 0;
        boundary.control = ControlState::Commit;
        boundary.status = StatusState::Active;
        boundary.committed = true;
        boundary.data = DataState::Hold;
        boundary.hold_samples = 396 * 2;
        boundary.step(active);
        assert_eq!(boundary.data, DataState::Transmit);
        assert!(
            !boundary.committed,
            "max burst count clears committed at real TX_EN"
        );
        let mut tail = Node::new(c.stations[1].clone(), &c, 2).unwrap();
        tail.control = ControlState::Transmit;
        tail.status = StatusState::Active;
        tail.data = DataState::Transmit;
        tail.hold_samples = 80;
        tail.step(Observation {
            mac_input_ended: true,
            ..active
        });
        assert_eq!(tail.data, DataState::Flush);
        tail.step(Observation {
            mac_input_ended: true,
            mac_output_ended: true,
            between_mac_clocks: true,
            ..active
        });
        assert_eq!(tail.data, DataState::Flush);
        tail.step(Observation {
            mac_input_ended: true,
            mac_output_ended: true,
            ..active
        });
        assert_eq!(
            tail.data,
            DataState::WaitIdle,
            "RS stops at the retained MAC tail, while PCS remains active"
        );
    }
    #[test]
    fn control_diagram_carrier_guards_burst_abort_and_noncoordinator_counter() {
        let c = config();
        let mut node = Node::new(c.stations[1].clone(), &c, 1).unwrap();
        node.status = StatusState::Active;
        node.control = ControlState::WaitTo;
        node.cur_id = c.node_count - 1;
        node.timer_samples = 1;
        node.step(Observation::default());
        assert_eq!(node.cur_id, c.node_count);
        assert_eq!(
            node.control,
            ControlState::WaitTo,
            "only coordinator uses node_count"
        );
        node.cur_id = 254;
        node.timer_samples = 1;
        node.step(Observation::default());
        assert_eq!(node.control, ControlState::Resync);
        node.control = ControlState::Yield;
        node.timer_samples = 2;
        node.step(Observation {
            carrier: true,
            ..Default::default()
        });
        assert_eq!(node.control, ControlState::EarlyReceive);
        assert_eq!(node.timer_samples, 22);
        node.control = ControlState::Transmit;
        node.burst_count = node.max_burst_count;
        node.step(Observation {
            carrier: true,
            ..Default::default()
        });
        assert_eq!(
            node.control,
            ControlState::Transmit,
            "must wait for CRS off at max count"
        );
        node.step(Observation::default());
        assert_ne!(node.control, ControlState::Transmit);
        node.cur_id = 0;
        node.control = ControlState::Burst;
        node.committed = true;
        node.timer_samples = 1;
        node.step(Observation {
            carrier: true,
            ..Default::default()
        });
        assert_eq!(node.control, ControlState::Abort);
        assert!(!node.committed);
        node.step(Observation {
            carrier: true,
            ..Default::default()
        });
        assert_eq!(node.control, ControlState::Abort);
        node.step(Observation::default());
        assert_eq!(node.control, ControlState::WaitTo);
        node.set_enabled(false);
        node.step(Observation::default());
        assert_eq!(node.cur_id, 0);
    }
}
