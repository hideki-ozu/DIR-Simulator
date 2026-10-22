//! Exact node-local clocks. Presentation rounding never changes timer eligibility.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClockError {
    InvalidRate,
    NonCanonicalTime,
    PastGlobalTime,
    TimeOverflow,
    EpochOverflow,
}
impl std::fmt::Display for ClockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "clock: {self:?}")
    }
}
impl std::error::Error for ClockError {}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalTime {
    #[serde(with = "signed_decimal")]
    numerator: i128,
    denominator: u64,
}
impl LocalTime {
    pub fn integer(ps: i128) -> Self {
        Self {
            numerator: ps,
            denominator: 1,
        }
    }
    pub fn fraction(numerator: i128, denominator: u64) -> Result<Self, ClockError> {
        if denominator == 0 {
            return Err(ClockError::NonCanonicalTime);
        }
        let divisor = gcd(numerator.unsigned_abs(), denominator as u128);
        Ok(Self {
            numerator: numerator / (divisor as i128),
            denominator: denominator / (divisor as u64),
        })
    }
    pub fn numerator(&self) -> i128 {
        self.numerator
    }
    pub fn denominator(&self) -> u64 {
        self.denominator
    }
    pub fn floor_ps(&self) -> Result<i128, ClockError> {
        self.validate()?;
        Ok(self.numerator.div_euclid(self.denominator as i128))
    }
    pub fn ceil_ps(&self) -> Result<i128, ClockError> {
        let floor = self.floor_ps()?;
        floor
            .checked_add(i128::from(
                self.numerator.rem_euclid(self.denominator as i128) != 0,
            ))
            .ok_or(ClockError::TimeOverflow)
    }
    pub fn validate(&self) -> Result<(), ClockError> {
        if self.denominator == 0
            || gcd(self.numerator.unsigned_abs(), self.denominator as u128) != 1
        {
            Err(ClockError::NonCanonicalTime)
        } else {
            Ok(())
        }
    }
    fn plus(&self, num: i128, den: u64) -> Result<Self, ClockError> {
        self.validate()?;
        if den == 0 {
            return Err(ClockError::NonCanonicalTime);
        }
        let g = gcd(self.denominator as u128, den as u128) as u64;
        let a = den / g;
        let b = self.denominator / g;
        let numerator = self
            .numerator
            .checked_mul(a as i128)
            .and_then(|n| num.checked_mul(b as i128).and_then(|m| n.checked_add(m)))
            .ok_or(ClockError::TimeOverflow)?;
        let denominator = self
            .denominator
            .checked_mul(a)
            .ok_or(ClockError::TimeOverflow)?;
        Self::fraction(numerator, denominator)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeClock {
    anchor_global_ps: u64,
    anchor_local: LocalTime,
    rate_num: u64,
    rate_den: u64,
    generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockWake {
    pub global_ps: u64,
    pub generation: u64,
    pub local_deadline: LocalTime,
}
impl NodeClock {
    pub fn new(
        global_ps: u64,
        local_ps: i128,
        rate_num: u64,
        rate_den: u64,
    ) -> Result<Self, ClockError> {
        let (rate_num, rate_den) = rate(rate_num, rate_den)?;
        Ok(Self {
            anchor_global_ps: global_ps,
            anchor_local: LocalTime::integer(local_ps),
            rate_num,
            rate_den,
            generation: 0,
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn rate(&self) -> (u64, u64) {
        (self.rate_num, self.rate_den)
    }
    pub fn local_at(&self, global_ps: u64) -> Result<LocalTime, ClockError> {
        self.validate()?;
        let elapsed = global_ps
            .checked_sub(self.anchor_global_ps)
            .ok_or(ClockError::PastGlobalTime)?;
        let num = (elapsed as i128)
            .checked_mul(self.rate_num as i128)
            .ok_or(ClockError::TimeOverflow)?;
        self.anchor_local.plus(num, self.rate_den)
    }
    /// The first global tick at/after now whose local time reaches the deadline.
    /// A backward local step never rewinds the global scheduler.
    pub fn deadline(
        &self,
        now_ps: u64,
        local_deadline: LocalTime,
    ) -> Result<ClockWake, ClockError> {
        local_deadline.validate()?;
        let now = self.local_at(now_ps)?;
        let common = gcd(local_deadline.denominator as u128, now.denominator as u128) as u64;
        let a = now.denominator / common;
        let b = local_deadline.denominator / common;
        let numerator = local_deadline
            .numerator
            .checked_mul(a as i128)
            .and_then(|n| {
                now.numerator
                    .checked_mul(b as i128)
                    .and_then(|m| n.checked_sub(m))
            })
            .ok_or(ClockError::TimeOverflow)?;
        let delta = LocalTime::fraction(
            numerator,
            local_deadline
                .denominator
                .checked_mul(a)
                .ok_or(ClockError::TimeOverflow)?,
        )?;
        let ticks = if delta.numerator <= 0 {
            0
        } else {
            let mut n = delta.numerator as u128;
            let mut q = self.rate_den as u128;
            let mut d = delta.denominator as u128;
            let mut p = self.rate_num as u128;
            let g = gcd(n, p);
            n /= g;
            p /= g;
            let g = gcd(q, d);
            q /= g;
            d /= g;
            let numerator = n.checked_mul(q).ok_or(ClockError::TimeOverflow)?;
            let denominator = d.checked_mul(p).ok_or(ClockError::TimeOverflow)?;
            numerator / denominator + u128::from(numerator % denominator != 0)
        };
        let ticks = u64::try_from(ticks).map_err(|_| ClockError::TimeOverflow)?;
        Ok(ClockWake {
            global_ps: now_ps.checked_add(ticks).ok_or(ClockError::TimeOverflow)?,
            generation: self.generation,
            local_deadline,
        })
    }
    pub fn wake_is_current(&self, wake: &ClockWake) -> bool {
        wake.generation == self.generation
    }
    pub fn step_to(&mut self, global_ps: u64, local: LocalTime) -> Result<(), ClockError> {
        self.local_at(global_ps)?;
        local.validate()?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(ClockError::EpochOverflow)?;
        self.anchor_global_ps = global_ps;
        self.anchor_local = local;
        self.generation = generation;
        Ok(())
    }
    pub fn step_by(&mut self, global_ps: u64, offset_ps: i128) -> Result<(), ClockError> {
        let local = self.local_at(global_ps)?.plus(offset_ps, 1)?;
        self.step_to(global_ps, local)
    }
    pub fn slew(&mut self, global_ps: u64, rate_num: u64, rate_den: u64) -> Result<(), ClockError> {
        let (rate_num, rate_den) = rate(rate_num, rate_den)?;
        let local = self.local_at(global_ps)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(ClockError::EpochOverflow)?;
        self.anchor_global_ps = global_ps;
        self.anchor_local = local;
        self.rate_num = rate_num;
        self.rate_den = rate_den;
        self.generation = generation;
        Ok(())
    }
    /// One clock generation commits a packet-derived phase and frequency
    /// estimate atomically, so TAS/PSFP never observe an intermediate law.
    pub fn reanchor_and_rate(
        &mut self,
        global_ps: u64,
        local: LocalTime,
        rate_num: u64,
        rate_den: u64,
    ) -> Result<(), ClockError> {
        self.local_at(global_ps)?;
        local.validate()?;
        let (rate_num, rate_den) = rate(rate_num, rate_den)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(ClockError::EpochOverflow)?;
        self.anchor_global_ps = global_ps;
        self.anchor_local = local;
        self.rate_num = rate_num;
        self.rate_den = rate_den;
        self.generation = generation;
        Ok(())
    }
    pub fn validate(&self) -> Result<(), ClockError> {
        self.anchor_local.validate()?;
        let pair = rate(self.rate_num, self.rate_den)?;
        if pair != (self.rate_num, self.rate_den) {
            return Err(ClockError::InvalidRate);
        }
        Ok(())
    }
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let next = a % b;
        a = b;
        b = next;
    }
    a
}
fn rate(p: u64, q: u64) -> Result<(u64, u64), ClockError> {
    if p == 0 || q == 0 {
        return Err(ClockError::InvalidRate);
    }
    let g = gcd(p as u128, q as u128) as u64;
    Ok((p / g, q / g))
}
mod signed_decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(n: &i128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&n.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i128, D::Error> {
        let text = String::deserialize(d)?;
        let n = text.parse::<i128>().map_err(serde::de::Error::custom)?;
        if n.to_string() != text {
            return Err(serde::de::Error::custom("noncanonical signed decimal"));
        }
        Ok(n)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_rational_inverse_is_first_eligible_global_tick() {
        for p in 1..=11 {
            for q in 1..=9 {
                for deadline in -4..=24 {
                    let clock = NodeClock::new(7, -5, p, q).unwrap();
                    let wake = clock.deadline(7, LocalTime::integer(deadline)).unwrap();
                    let expected = (7..=300)
                        .find(|g| ((*g - 7) as i128) * (p as i128) >= (deadline + 5) * (q as i128))
                        .unwrap();
                    assert_eq!(wake.global_ps, expected, "p={p} q={q} local={deadline}");
                }
            }
        }
    }
    #[test]
    fn fractional_slew_and_bidirectional_steps_preserve_causality() {
        let mut clock = NodeClock::new(0, -2, 2, 3).unwrap();
        assert_eq!(
            clock.local_at(1).unwrap(),
            LocalTime::fraction(-4, 3).unwrap()
        );
        let old = clock.deadline(1, LocalTime::integer(3)).unwrap();
        clock.slew(1, 5, 7).unwrap();
        assert!(!clock.wake_is_current(&old));
        assert_eq!(
            clock.local_at(1).unwrap(),
            LocalTime::fraction(-4, 3).unwrap()
        );
        assert_eq!(
            clock.local_at(2).unwrap(),
            LocalTime::fraction(-13, 21).unwrap()
        );
        clock.step_by(2, 100).unwrap();
        assert_eq!(
            clock.deadline(2, LocalTime::integer(10)).unwrap().global_ps,
            2
        );
        clock.step_by(2, -200).unwrap();
        assert!(clock.deadline(2, LocalTime::integer(10)).unwrap().global_ps > 2);
        assert_eq!(clock.local_at(1), Err(ClockError::PastGlobalTime));
        let encoded = serde_json::to_vec(&clock).unwrap();
        let decoded: NodeClock = serde_json::from_slice(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(clock, decoded);
    }
    #[test]
    fn invalid_or_nonrepresentable_state_is_rejected_atomically() {
        assert_eq!(NodeClock::new(0, 0, 0, 1), Err(ClockError::InvalidRate));
        assert_eq!(LocalTime::fraction(1, 0), Err(ClockError::NonCanonicalTime));
        let mut clock = NodeClock::new(0, 0, 1, 1).unwrap();
        let before = clock.clone();
        assert_eq!(clock.slew(1, 1, 0), Err(ClockError::InvalidRate));
        assert_eq!(clock, before);
        assert_eq!(
            clock.deadline(u64::MAX, LocalTime::integer(i128::MAX)),
            Err(ClockError::TimeOverflow)
        );
        assert!(
            serde_json::from_str::<LocalTime>(r#"{"numerator":"-0","denominator":1}"#).is_err()
        );
        let invalid: LocalTime =
            serde_json::from_str(r#"{"numerator":"2","denominator":2}"#).unwrap();
        assert_eq!(invalid.validate(), Err(ClockError::NonCanonicalTime));
        assert_eq!(invalid.floor_ps(), Err(ClockError::NonCanonicalTime));
        let minclock = NodeClock::new(0, i128::MIN, 1, 1).unwrap();
        assert_eq!(
            minclock
                .deadline(0, LocalTime::integer(i128::MIN))
                .unwrap()
                .global_ps,
            0
        );
        let min = LocalTime::integer(i128::MIN);
        assert_eq!(min.ceil_ps().unwrap(), i128::MIN);
    }
}
