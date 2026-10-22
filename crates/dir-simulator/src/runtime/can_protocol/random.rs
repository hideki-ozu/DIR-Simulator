//! SplitMix64 v1 with unbiased rejection mapping; no floating-point transforms.
use super::types::Schedule;
use crate::registry::ModelResult;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub rng: u64,
    pub ordinal: u64,
    pub position: u64,
    pub time: u64,
}
impl Cursor {
    pub fn new(s: &Schedule) -> Self {
        let (seed, time) = match s {
            Schedule::Explicit { times_ps } => (0, times_ps.first().copied().unwrap_or(0)),
            Schedule::Uniform { seed, start_ps, .. }
            | Schedule::Bernoulli { seed, start_ps, .. }
            | Schedule::Geometric { seed, start_ps, .. } => (*seed, *start_ps),
        };
        Self {
            rng: seed,
            ordinal: 0,
            position: 0,
            time,
        }
    }
    pub fn draw(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    pub fn bounded(&mut self, bound: u64) -> u64 {
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let r = self.draw();
            if r >= threshold {
                return r % bound;
            }
        }
    }
    /// Returns the next emission time and updates only this generator's RNG cursor.
    pub fn next(&mut self, s: &Schedule) -> ModelResult<Option<u64>> {
        let add = |a: u64, b: u64| {
            a.checked_add(b)
                .ok_or_else(|| crate::registry::ModelError("generator time overflow".into()))
        };
        match s {
            Schedule::Explicit { times_ps } => {
                let out = times_ps.get(self.position as usize).copied();
                self.position += u64::from(out.is_some());
                Ok(out)
            }
            Schedule::Uniform {
                count,
                min_ps,
                max_ps,
                ..
            } => {
                if self.position >= *count {
                    return Ok(None);
                }
                if self.position > 0 {
                    let delta = min_ps + self.bounded(max_ps - min_ps + 1);
                    self.time = add(self.time, delta)?;
                }
                self.position += 1;
                Ok(Some(self.time))
            }
            Schedule::Bernoulli {
                count,
                tick_ps,
                numerator,
                denominator,
                ..
            } => {
                while self.position < *count {
                    let t = self.time;
                    self.position += 1;
                    self.time = add(self.time, *tick_ps)?;
                    if self.bounded(*denominator) < *numerator {
                        return Ok(Some(t));
                    }
                }
                Ok(None)
            }
            Schedule::Geometric {
                count,
                tick_ps,
                numerator,
                denominator,
                ..
            } => {
                if self.position >= *count {
                    return Ok(None);
                }
                for _ in 0..1_000_000 {
                    self.time = add(self.time, *tick_ps)?;
                    if self.bounded(*denominator) < *numerator {
                        self.position += 1;
                        return Ok(Some(self.time));
                    }
                }
                Err("geometric sampling exceeds v1 trial limit".into())
            }
        }
    }
}
