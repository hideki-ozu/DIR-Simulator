//! Forced 10BASE-T Clause 14 MAU link-test and jabber state.
//!
//! The MAU samples its receive pair independently of the MAC. A normal link
//! pulse is two positive Manchester-chip intervals followed by silence;
//! packet carrier needs sustained activity and cannot be created by a pulse.
//! Timers use the physical sample clock, so checkpoints preserve their phase.
use serde::{Deserialize, Serialize};

const PULSE_INTERVAL_MS: u64 = 16;
const LINK_LOSS_MS: u64 = 100;
const LINK_TEST_MIN_MS: u64 = 5;
const LINK_TEST_MAX_MS: u64 = 100;
const XMIT_MAX_MS: u64 = 40;
const UNJAB_MS: u64 = 500;
const RECOVERY_PULSES: u8 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    pub packet_carrier: bool,
    pub link_ok: bool,
    pub jabber: bool,
    pub accepted_pulse: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mau {
    samples_per_chip: u32,
    next_sample: u64,
    tx_idle_samples: u64,
    pulse_chips_remaining: u8,
    rx_nonzero_samples: u64,
    rx_positive_run: bool,
    rx_quiet_samples: u64,
    pulse_candidate_samples: u64,
    since_rx_activity: u64,
    since_last_pulse: Option<u64>,
    consecutive_pulses: u8,
    link_ok: bool,
    xmit_samples: u64,
    unjab_samples: u64,
    jabber: bool,
}

impl Mau {
    pub fn new(samples_per_chip: u32) -> Self {
        Self {
            samples_per_chip,
            next_sample: 0,
            tx_idle_samples: 0,
            pulse_chips_remaining: 0,
            rx_nonzero_samples: 0,
            rx_positive_run: true,
            rx_quiet_samples: 0,
            pulse_candidate_samples: 0,
            since_rx_activity: 0,
            since_last_pulse: None,
            consecutive_pulses: 0,
            // Clause 14.2.1.7 explicitly permits Link Test Pass on PowerOn.
            link_ok: true,
            xmit_samples: 0,
            unjab_samples: 0,
            jabber: false,
        }
    }
    fn samples_for_ms(&self, ms: u64) -> u64 {
        ms * 20_000 * self.samples_per_chip as u64
    }
    pub fn link_ok(&self) -> bool {
        self.link_ok
    }
    pub fn next_sample(&self) -> u64 {
        self.next_sample
    }
    pub fn jabber(&self) -> bool {
        self.jabber
    }
    pub fn packet_carrier(&self) -> bool {
        self.rx_nonzero_samples >= 3 * self.samples_per_chip as u64
            || !self.rx_positive_run && self.rx_nonzero_samples >= self.samples_per_chip as u64
    }
    /// Called only at a transmitter chip boundary when no MAC/repeater data
    /// is pending. The generated pulse has no MAC Origin.
    pub fn idle_chip(&mut self) -> i8 {
        if self.pulse_chips_remaining > 0 {
            self.pulse_chips_remaining -= 1;
            return 1;
        }
        if self.tx_idle_samples >= self.samples_for_ms(PULSE_INTERVAL_MS) {
            self.tx_idle_samples = 0;
            self.pulse_chips_remaining = 1;
            return 1;
        }
        0
    }
    /// Observe one already-resolved physical sample. `tx_data` excludes this
    /// MAU's own link pulse; `rx_level` is the independent receive pair.
    pub fn observe(&mut self, tx_data: bool, rx_level: i64, threshold_uv: i64) -> Observation {
        self.next_sample = self.next_sample.saturating_add(1);
        self.tx_idle_samples = if tx_data {
            0
        } else {
            self.tx_idle_samples.saturating_add(1)
        };
        if tx_data {
            self.xmit_samples = self.xmit_samples.saturating_add(1);
            self.unjab_samples = 0;
            if self.xmit_samples > self.samples_for_ms(XMIT_MAX_MS) {
                self.jabber = true;
            }
        } else {
            self.xmit_samples = 0;
            if self.jabber {
                self.unjab_samples = self.unjab_samples.saturating_add(1);
                if self.unjab_samples >= self.samples_for_ms(UNJAB_MS) {
                    self.jabber = false;
                    self.unjab_samples = 0;
                }
            }
        }

        let positive = rx_level >= threshold_uv;
        let active = rx_level.unsigned_abs() >= threshold_uv as u64;
        let mut accepted_pulse = false;
        if active {
            if self.rx_nonzero_samples == 0 {
                self.rx_positive_run = positive;
            } else {
                self.rx_positive_run &= positive;
            }
            self.rx_nonzero_samples = self.rx_nonzero_samples.saturating_add(1);
            self.rx_quiet_samples = 0;
            self.pulse_candidate_samples = 0;
            if self.packet_carrier() {
                self.since_rx_activity = 0;
                self.link_ok = true;
                self.consecutive_pulses = 0;
            }
        } else {
            if self.rx_nonzero_samples > 0 {
                if self.rx_positive_run
                    && (self.samples_per_chip as u64..=2 * self.samples_per_chip as u64)
                        .contains(&self.rx_nonzero_samples)
                {
                    self.pulse_candidate_samples = self.rx_nonzero_samples;
                }
                self.rx_nonzero_samples = 0;
            }
            self.rx_quiet_samples = self.rx_quiet_samples.saturating_add(1);
            if self.pulse_candidate_samples > 0
                && self.rx_quiet_samples >= 3 * self.samples_per_chip as u64
            {
                let valid_spacing = self.since_last_pulse.is_none_or(|last| {
                    let elapsed = self.next_sample.saturating_sub(last);
                    elapsed >= self.samples_for_ms(LINK_TEST_MIN_MS)
                        && elapsed <= self.samples_for_ms(LINK_TEST_MAX_MS)
                });
                self.consecutive_pulses = if valid_spacing {
                    self.consecutive_pulses
                        .saturating_add(1)
                        .min(RECOVERY_PULSES)
                } else {
                    0
                };
                self.since_last_pulse = Some(self.next_sample);
                self.pulse_candidate_samples = 0;
                if valid_spacing {
                    accepted_pulse = true;
                    self.since_rx_activity = 0;
                    if self.consecutive_pulses >= RECOVERY_PULSES {
                        self.link_ok = true;
                    }
                }
            }
            self.since_rx_activity = self.since_rx_activity.saturating_add(1);
            if self.since_rx_activity >= self.samples_for_ms(LINK_LOSS_MS) {
                self.link_ok = false;
            }
        }
        Observation {
            packet_carrier: self.packet_carrier(),
            link_ok: self.link_ok,
            jabber: self.jabber,
            accepted_pulse,
        }
    }
    pub fn validate(&self, samples_per_chip: u32) -> bool {
        self.samples_per_chip == samples_per_chip
            && self.pulse_chips_remaining <= 1
            && self.consecutive_pulses <= RECOVERY_PULSES
            && self.pulse_candidate_samples <= 2 * samples_per_chip as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_pulse_is_generated_at_sixteen_ms_and_does_not_make_packet_carrier() {
        let mut tx = Mau::new(16);
        let mut rx = Mau::new(16);
        for _ in 0..320_000 {
            assert_eq!(tx.idle_chip(), 0);
            for _ in 0..16 {
                tx.observe(false, 0, 100_000);
            }
        }
        for _ in 0..2 {
            assert_eq!(tx.idle_chip(), 1);
            for _ in 0..16 {
                let o = rx.observe(false, 1_000_000, 100_000);
                assert!(!o.packet_carrier);
            }
        }
        let mut accepted = false;
        for _ in 0..48 {
            accepted |= rx.observe(false, 0, 100_000).accepted_pulse;
        }
        assert!(accepted);
        assert!(rx.link_ok());
    }
    #[test]
    fn sustained_manchester_activity_is_data_not_a_pulse() {
        let mut mau = Mau::new(16);
        for chip in 0..40 {
            for _ in 0..16 {
                let result = mau.observe(
                    false,
                    if chip % 2 == 0 { 1_000_000 } else { -1_000_000 },
                    100_000,
                );
                if chip >= 3 {
                    assert!(result.packet_carrier);
                }
                assert!(!result.accepted_pulse);
            }
        }
    }
    #[test]
    fn link_loss_requires_three_spaced_physical_pulses_to_recover() {
        let mut mau = Mau::new(16);
        for _ in 0..mau.samples_for_ms(LINK_LOSS_MS) {
            mau.observe(false, 0, 100_000);
        }
        assert!(!mau.link_ok());
        let pulse = |mau: &mut Mau| {
            for _ in 0..32 {
                assert!(!mau.observe(false, 1_000_000, 100_000).packet_carrier);
            }
            let mut accepted = false;
            for _ in 0..48 {
                accepted |= mau.observe(false, 0, 100_000).accepted_pulse;
            }
            accepted
        };
        assert!(pulse(&mut mau));
        assert!(!mau.link_ok());
        for _ in 0..mau.samples_for_ms(1) {
            mau.observe(false, 0, 100_000);
        }
        assert!(!pulse(&mut mau));
        assert!(!mau.link_ok());
        for _ in 0..mau.samples_for_ms(PULSE_INTERVAL_MS) {
            mau.observe(false, 0, 100_000);
        }
        assert!(pulse(&mut mau));
        assert!(!mau.link_ok());
        for _ in 0..mau.samples_for_ms(PULSE_INTERVAL_MS) {
            mau.observe(false, 0, 100_000);
        }
        assert!(pulse(&mut mau));
        assert!(!mau.link_ok());
        for _ in 0..mau.samples_for_ms(PULSE_INTERVAL_MS) {
            mau.observe(false, 0, 100_000);
        }
        assert!(pulse(&mut mau));
        assert!(mau.link_ok());
    }
    #[test]
    fn jabber_interrupts_after_forty_ms_and_releases_after_half_second_idle() {
        let mut mau = Mau::new(16);
        for _ in 0..mau.samples_for_ms(XMIT_MAX_MS) {
            mau.observe(true, 0, 100_000);
        }
        assert!(!mau.jabber());
        mau.observe(true, 0, 100_000);
        assert!(mau.jabber());
        for _ in 0..mau.samples_for_ms(UNJAB_MS) - 1 {
            mau.observe(false, 0, 100_000);
        }
        assert!(mau.jabber());
        mau.observe(false, 0, 100_000);
        assert!(!mau.jabber());
    }
}
