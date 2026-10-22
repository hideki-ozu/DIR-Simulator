//! IEEE 802.3-2022 Clause 99 reducers. No queue, FES, or physical serializer is
//! owned here: the enclosing NetworkState must reserve every returned packet.
use serde::{Deserialize, Serialize};

mod codec;
mod rx;
mod tx;
mod verify;
pub(crate) use codec::*;
pub(crate) use rx::*;
pub(crate) use tx::*;
pub(crate) use verify::*;

pub(crate) type MergeResult<T> = Result<T, MergeError>;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MergeError {
    InvalidConfiguration,
    InvalidMacFcs,
    InvalidPreamble,
    InvalidDelimiter,
    InvalidFragmentCount,
    ShortPacket,
    WrongEpoch,
    FrameCheckError,
    NoAssembly,
    CounterMismatch,
    ReceiveLimit,
    InvalidState,
    Overflow,
    InvalidCheckpoint,
    MissingCapabilityEvidence,
}

/// This initial implementation deliberately supports only ideal full-duplex
/// 100 Mb/s and 1 Gb/s physical directions, with verification enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedMergePort {
    pub bitrate: u64,
    pub full_duplex: bool,
    pub advertised_add_frag_size: u8,
    pub verify_time_ms: u8,
    pub admin_enabled: bool,
    pub max_rx_mac_octets: usize,
}
impl PreparedMergePort {
    #[cfg(test)]
    pub fn validate(&self) -> MergeResult<()> {
        if !self.full_duplex
            || ![100_000_000, 1_000_000_000].contains(&self.bitrate)
            || self.advertised_add_frag_size > 3
            || !(1..=128).contains(&self.verify_time_ms)
            || self.max_rx_mac_octets < 64
        {
            return Err(MergeError::InvalidConfiguration);
        }
        Ok(())
    }
}
pub(crate) fn bit_duration(bits: u64, bitrate: u64) -> MergeResult<u64> {
    if ![100_000_000, 1_000_000_000].contains(&bitrate) {
        return Err(MergeError::InvalidConfiguration);
    }
    u64::try_from(u128::from(bits) * 1_000_000_000_000u128 / u128::from(bitrate))
        .map_err(|_| MergeError::Overflow)
}
pub(crate) fn checked_time(a: u64, b: u64) -> MergeResult<u64> {
    a.checked_add(b).ok_or(MergeError::Overflow)
}

#[cfg(test)]
mod tests;

mod ingress;
pub(crate) mod integration;
mod lldp;
mod phy;

mod checkpoint;
mod journal;
mod shaper;
mod tas;
