//! Deterministic pair-local transitions shared by legacy and composed adapters. No scheduler.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum Change {
    SignalOn(usize, usize),
    SignalOff(usize, usize),
    Eof(usize, usize),
    Release(usize, usize),
    JamEnd(usize, usize),
    Ifg(usize, u64),
    Backoff(usize, usize),
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Mac {
    pub(crate) emitting: Option<usize>,
    pub(crate) carrier: Option<usize>,
    pub(crate) busy: bool,
    pub(crate) ifg_ready: bool,
    pub(crate) idle_generation: u64,
    pub(crate) backoff_until: Option<u64>,
    pub(crate) generation: u64,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Pair {
    pub(crate) macs: [Mac; 2],
    pub(crate) timeline: BTreeMap<u64, Vec<Change>>,
    pub(crate) reservation: Option<(u64, u64)>,
    pub(crate) generation: u64,
}

pub fn backoff_slots(seed: u64, transfer_id: &str, number: u64) -> u64 {
    let mut hash = Sha256::new();
    hash.update(b"dir.ethernet.beb.v1\0");
    hash.update(seed.to_string().as_bytes());
    hash.update([0]);
    hash.update(transfer_id.as_bytes());
    hash.update([0]);
    hash.update(number.to_string().as_bytes());
    let bytes = hash.finalize();
    let value = u64::from_be_bytes(bytes[..8].try_into().unwrap());
    value & ((1u64 << number.min(10)) - 1)
}
impl Pair {
    pub(crate) fn change(&mut self, time: u64, change: Change) {
        self.timeline.entry(time).or_default().push(change);
    }
    pub(crate) fn cancel_attempt(&mut self, attempt: usize) {
        self.timeline.retain(|_, changes| {
            changes.retain(|change| !matches!(change, Change::Eof(_, a) | Change::Release(_, a) | Change::SignalOff(_, a) if *a == attempt));
            !changes.is_empty()
        });
    }
    pub(crate) fn cancel_ifg(&mut self, side: usize) {
        self.timeline.retain(|_, changes| {
            changes.retain(|change| !matches!(change, Change::Ifg(s, _) if *s == side));
            !changes.is_empty()
        });
    }
}

impl Default for Pair {
    fn default() -> Self {
        Self {
            macs: std::array::from_fn(|_| Mac {
                ifg_ready: true,
                ..Mac::default()
            }),
            timeline: BTreeMap::new(),
            reservation: None,
            generation: 0,
        }
    }
}
impl Pair {
    /// Apply half-open endpoints first. Simultaneous starts observe the same carrier snapshot.
    pub(crate) fn settle_signals(&mut self, changes: &[Change]) {
        for change in changes {
            match *change {
                Change::SignalOff(side, a) if self.macs[side].carrier == Some(a) => {
                    self.macs[side].carrier = None
                }
                Change::Eof(side, a) | Change::JamEnd(side, a)
                    if self.macs[side].emitting == Some(a) =>
                {
                    self.macs[side].emitting = None
                }
                _ => {}
            }
        }
        for change in changes {
            if let Change::SignalOn(side, a) = *change {
                self.macs[side].carrier = Some(a);
            }
        }
    }
    pub(crate) fn ready(&self, side: usize, now: u64, half: bool) -> bool {
        let m = &self.macs[side];
        !half || (!m.busy && m.ifg_ready && m.backoff_until.is_none_or(|t| t <= now))
    }
}
/// Clause4.2.8 policy: <=100Mb/s retains the shipped optional retry policy;
/// 1000Mb/s must end retries after a collision beyond its 4096-bit slot.
pub(crate) fn late_collision(rate: u64, sof: u64, collision: u64) -> (bool, bool) {
    let slot_bits = if rate == 1_000_000_000 {
        4096u128
    } else {
        512u128
    };
    let late = u128::from(collision.saturating_sub(sof)) * u128::from(rate)
        > slot_bits * 1_000_000_000_000;
    (late, late && rate == 1_000_000_000)
}
impl Pair {
    /// Current emitters only; endpoints already settled by settle_signals.
    pub(crate) fn collisions(
        &self,
        now: u64,
        bit: u64,
        mut sof: impl FnMut(usize) -> Option<u64>,
    ) -> Result<Vec<(usize, usize, u64, u64)>, &'static str> {
        let mut out = Vec::new();
        for side in 0..2 {
            let m = &self.macs[side];
            if let (Some(a), Some(_)) = (m.emitting, m.carrier) {
                if let Some(sof) = sof(a) {
                    let start = now.max(
                        sof.checked_add(64 * bit)
                            .ok_or("Ethernet jam time overflow")?,
                    );
                    let end = start
                        .checked_add(32 * bit)
                        .ok_or("Ethernet jam time overflow")?;
                    out.push((side, a, start, end));
                }
            }
        }
        Ok(out)
    }
    pub(crate) fn settle_idle(&mut self, now: u64, bit: u64) -> Result<(), &'static str> {
        for side in 0..2 {
            let busy = self.macs[side].emitting.is_some() || self.macs[side].carrier.is_some();
            let before = self.macs[side].busy;
            self.macs[side].busy = busy;
            if busy {
                self.macs[side].ifg_ready = false;
                self.cancel_ifg(side);
            } else if before {
                self.macs[side].ifg_ready = false;
                self.macs[side].idle_generation = self.macs[side]
                    .idle_generation
                    .checked_add(1)
                    .ok_or("Ethernet IFG generation overflow")?;
                self.change(
                    now.checked_add(96 * bit)
                        .ok_or("Ethernet IFG time overflow")?,
                    Change::Ifg(side, self.macs[side].idle_generation),
                );
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_slot_threshold_and_speed_policy() {
        assert_eq!(late_collision(100_000_000, 0, 5_120_000), (false, false));
        assert_eq!(late_collision(100_000_000, 0, 5_120_001), (true, false));
        assert_eq!(late_collision(1_000_000_000, 0, 4_096_000), (false, false));
        assert_eq!(late_collision(1_000_000_000, 0, 4_096_001), (true, true));
    }
    #[test]
    fn half_open_eof_and_carrier_end_precede_collision() {
        let mut pair = Pair::default();
        pair.macs[0].emitting = Some(0);
        pair.macs[0].carrier = Some(1);
        pair.settle_signals(&[Change::Eof(0, 0)]);
        assert!(
            pair.collisions(1_000_000, 10_000, |_| Some(0))
                .unwrap()
                .is_empty()
        );
        pair.macs[0].emitting = Some(0);
        pair.macs[0].carrier = Some(1);
        pair.settle_signals(&[Change::SignalOff(0, 1)]);
        assert!(
            pair.collisions(1_000_000, 10_000, |_| Some(0))
                .unwrap()
                .is_empty()
        );
    }
}
