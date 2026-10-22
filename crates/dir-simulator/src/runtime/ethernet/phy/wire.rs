//! Project Manchester numerical wire contract, deliberately distinct from T1 PCS.
//! Canonical Ethernet MAC bytes include FCS and exclude preamble/SFD/IFG.
use super::*;
use crate::runtime::ethernet::crc32;
use serde::{Deserialize, Serialize};
pub const CONTRACT: &str = "dir.sampled-manchester.v1";
pub const PILOT_BYTES: usize = 16;
const PREAMBLE: [u8; 8] = [0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5];
pub fn validate_mac(bytes: &[u8], maximum: usize) -> Result<()> {
    if bytes.len() < 64 || bytes.len() > maximum || bytes.len() < 4 {
        return Err("canonical MAC frame length outside configured range".into());
    }
    let end = bytes.len() - 4;
    if crc32(&bytes[..end]).to_le_bytes() != bytes[end..] {
        return Err("canonical MAC frame has invalid FCS".into());
    }
    Ok(())
}
pub fn encode_bytes(bytes: &[u8]) -> Vec<i8> {
    let mut output = Vec::with_capacity(bytes.len() * 16);
    for &byte in bytes {
        for bit in 0..8 {
            let first = if byte & (1 << bit) != 0 { 1 } else { -1 };
            output.extend([first, -first]);
        }
    }
    output
}
pub fn pilot() -> Vec<i8> {
    encode_bytes(&[0x55; PILOT_BYTES])
}
pub fn encode_mac(bytes: &[u8], maximum: usize) -> Result<Vec<i8>> {
    validate_mac(bytes, maximum)?;
    let mut output = pilot();
    output.extend(encode_bytes(&PREAMBLE));
    output.extend(encode_bytes(bytes));
    Ok(output)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeEffect {
    Start,
    Abort(&'static str),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoder {
    first: Option<i8>,
    shift: u64,
    searched: u64,
    collecting: bool,
    partial: u8,
    partial_bits: u8,
    bytes: Vec<u8>,
    ending: bool,
    rejected: bool,
    maximum: usize,
}
impl Decoder {
    pub fn new(maximum: usize) -> Self {
        Self {
            first: None,
            shift: 0,
            searched: 0,
            collecting: false,
            partial: 0,
            partial_bits: 0,
            bytes: vec![],
            ending: false,
            rejected: false,
            maximum,
        }
    }
    pub fn collecting(&self) -> bool {
        self.collecting
    }
    pub fn push(&mut self, symbol: i8, locked: bool) -> Result<Option<DecodeEffect>> {
        if self.rejected {
            return Ok(None);
        }
        if symbol == 0 {
            self.first = None;
            if self.collecting {
                self.ending = true;
            }
            return Ok(None);
        }
        if !(-1..=1).contains(&symbol) {
            return Err("invalid sampled Manchester level".into());
        }
        if self.ending {
            self.rejected = true;
            return Ok(Some(DecodeEffect::Abort("carrier_gap_inside_frame")));
        }
        let Some(first) = self.first.take() else {
            self.first = Some(symbol);
            return Ok(None);
        };
        if first == symbol {
            if self.collecting {
                self.rejected = true;
                return Ok(Some(DecodeEffect::Abort("manchester_code_violation")));
            }
            // Before delimiter acquisition, slide one chip to find actual phase.
            self.first = Some(symbol);
            self.shift = 0;
            self.searched = 0;
            return Ok(None);
        }
        let bit = u8::from(first == 1);
        if !self.collecting {
            self.shift = (self.shift >> 1) | ((bit as u64) << 63);
            self.searched = self.searched.saturating_add(1);
            if self.searched >= 64 && self.shift == u64::from_le_bytes(PREAMBLE) {
                if !locked {
                    self.rejected = true;
                    return Ok(Some(DecodeEffect::Abort("unlocked_start_delimiter")));
                }
                self.collecting = true;
                return Ok(Some(DecodeEffect::Start));
            }
            return Ok(None);
        }
        self.partial |= bit << self.partial_bits;
        self.partial_bits += 1;
        if self.partial_bits == 8 {
            if self.bytes.len() >= self.maximum {
                self.rejected = true;
                return Ok(Some(DecodeEffect::Abort("maximum_mac_length_exceeded")));
            }
            self.bytes.push(self.partial);
            self.partial = 0;
            self.partial_bits = 0;
        }
        Ok(None)
    }
    pub fn finish(&mut self) -> Result<Vec<u8>> {
        if self.rejected || !self.collecting || self.partial_bits != 0 || self.first.is_some() {
            return Err("truncated_or_invalid_sampled_mac_frame".into());
        }
        validate_mac(&self.bytes, self.maximum)?;
        Ok(std::mem::take(&mut self.bytes))
    }
    pub fn validate(&self, maximum: usize) -> Result<()> {
        if self.maximum != maximum
            || self.bytes.len() > maximum
            || self.partial_bits >= 8
            || self.first.is_some_and(|s| s != 1 && s != -1)
            || self.partial_bits > 0 && self.partial >= (1u16 << self.partial_bits) as u8
            || self.partial_bits == 0 && self.partial != 0
        {
            return Err("invalid saved Manchester decoder".into());
        }
        Ok(())
    }
}
