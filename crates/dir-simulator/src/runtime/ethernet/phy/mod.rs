//! Deterministic numerical differential-voltage channel contract.
//!
//! This is a simulator accuracy contract, not an IEEE PHY codec or an electrical
//! cable/transceiver model. The production PHY adapter must preserve these live
//! histories and split batches at externally observable boundaries.
pub mod attachment;
pub mod channel;
pub mod execution;
pub mod ieee;
pub mod medium;
pub mod receiver;
pub mod wire;
use serde::{Deserialize, Serialize};
pub type Microvolts = i64;
pub const Q30: i64 = 1 << 30;
pub const MAX_ENDPOINTS: usize = 16;
pub const MAX_BLOCK_SAMPLES: usize = 16_384;
pub type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleGrid {
    pub symbol_rate_hz: u64,
    pub samples_per_ui: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExactSampleTime {
    pub numerator_ps: u128,
    pub denominator: u128,
}
impl SampleGrid {
    pub fn validate(self) -> Result<()> {
        if self.symbol_rate_hz == 0
            || self.symbol_rate_hz > 1_000_000_000
            || !(8..=128).contains(&self.samples_per_ui)
        {
            return Err("numerical PHY requires 1..1GHz symbol rate and 8..128 samples/UI".into());
        }
        Ok(())
    }
    pub fn time(self, sample_index: u64) -> Result<ExactSampleTime> {
        self.validate()?;
        Ok(ExactSampleTime {
            numerator_ps: (sample_index as u128)
                .checked_mul(1_000_000_000_000)
                .ok_or("sample time overflow")?,
            denominator: (self.symbol_rate_hz as u128)
                .checked_mul(self.samples_per_ui as u128)
                .ok_or("sample frequency overflow")?,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleBlock {
    pub grid: SampleGrid,
    pub start_index: u64,
    /// Endpoint order is the immutable channel configuration order. Units: µV.
    pub voltages: Vec<Vec<Microvolts>>,
}
impl SampleBlock {
    pub fn len(&self) -> usize {
        self.voltages.first().map_or(0, Vec::len)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn validate(&self) -> Result<()> {
        self.grid.validate()?;
        if self.voltages.is_empty()
            || self.voltages.len() > MAX_ENDPOINTS
            || self.len() > MAX_BLOCK_SAMPLES
            || self.voltages.iter().any(|v| {
                v.len() != self.len()
                    || v.iter()
                        .any(|&sample| sample.unsigned_abs() > 1_000_000_000)
            })
        {
            return Err("invalid numerical PHY sample block dimensions/voltage".into());
        }
        self.start_index
            .checked_add(self.len() as u64)
            .ok_or("sample index overflow")?;
        Ok(())
    }
}
pub(crate) fn rounded_q30(sum: i128) -> Result<i64> {
    let half = (Q30 / 2) as i128;
    let magnitude = sum.checked_abs().ok_or("FIR absolute overflow")?;
    let result = (magnitude.checked_add(half).ok_or("FIR rounding overflow")?) / (Q30 as i128);
    i64::try_from(if sum < 0 { -result } else { result }).map_err(|_| "FIR voltage overflow".into())
}
pub(crate) fn validate_taps(taps: &[i64]) -> Result<()> {
    if taps.is_empty()
        || taps.len() > 1024
        || taps
            .iter()
            .try_fold(0u64, |sum, &tap| sum.checked_add(tap.unsigned_abs()))
            .is_none_or(|sum| sum > 16 * Q30 as u64)
    {
        return Err("FIR requires 1..1024 finite Q30 taps with absolute gain <=16".into());
    }
    Ok(())
}
