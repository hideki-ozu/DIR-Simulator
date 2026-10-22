//! Continuous Clause 97 80B/81B + RS(450,406) + PAM3 MAC stream.
//! A PHY frame carries 450 GMII octets.  Idle control fills unused capacity;
//! MAC preamble, SFD, payload/FCS and IFG remain explicit in the PCS input.
use super::super::{Result, wire};
use super::t1_gigabit::{
    self, BLOCKS_PER_PHY_FRAME, Control, Gmii, PAM3_SYMBOLS_PER_PHY_FRAME, Role, SideScrambler,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const GMII_PER_PHY_FRAME: usize = BLOCKS_PER_PHY_FRAME * 10;
const MAC_PREAMBLE: [u8; 8] = [0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5];
const IFG_OCTETS: usize = 12;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    /// GMII byte slots from the beginning of continuous PAM3 data mode.
    pub preamble_start_slot: u64,
    pub sfd_end_slot: u64,
    pub fcs_end_slot: u64,
    /// Exact MDI PAM3 symbol boundaries after the mapped data octets.
    pub sfd_end_physical_symbol: u64,
    pub fcs_end_physical_symbol: u64,
    /// Full RS-coded PHY frame containing the last MAC FCS byte has left MDI.
    pub coded_end_symbol: u64,
}
impl SourceRange {
    pub fn preamble_start_symbol(self) -> u64 {
        self.preamble_start_slot * 6
    }
    pub fn sfd_end_symbol(self) -> u64 {
        self.sfd_end_physical_symbol
    }
    pub fn fcs_end_symbol(self) -> u64 {
        self.fcs_end_physical_symbol
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransmitStream {
    scrambler: SideScrambler,
    gmii: VecDeque<Gmii>,
    pam3: VecDeque<i8>,
    frames_encoded: u64,
    symbols_emitted: u64,
}
impl TransmitStream {
    pub fn new(role: Role, seed: u16) -> Result<Self> {
        Ok(Self {
            scrambler: SideScrambler::new(role, seed)?,
            gmii: VecDeque::new(),
            pam3: VecDeque::new(),
            frames_encoded: 0,
            symbols_emitted: 0,
        })
    }
    pub fn frames_encoded(&self) -> u64 {
        self.frames_encoded
    }
    pub fn pending_gmii(&self) -> usize {
        self.gmii.len()
    }
    pub fn pending_pam3(&self) -> usize {
        self.pam3.len()
    }
    pub fn symbols_emitted(&self) -> u64 {
        self.symbols_emitted
    }
    pub fn enqueue_mac(&mut self, bytes: &[u8], maximum: usize) -> Result<SourceRange> {
        wire::validate_mac(bytes, maximum)?;
        if !self.gmii.is_empty() {
            return Err("1000BASE-T1 PCS already owns a MAC request".into());
        }
        let start = self.frames_encoded * GMII_PER_PHY_FRAME as u64;
        let fcs_end = start + MAC_PREAMBLE.len() as u64 + bytes.len() as u64;
        let octet_at = |relative: usize| -> Gmii {
            if relative < MAC_PREAMBLE.len() {
                Gmii::Data(MAC_PREAMBLE[relative])
            } else if relative < MAC_PREAMBLE.len() + bytes.len() {
                Gmii::Data(bytes[relative - MAC_PREAMBLE.len()])
            } else {
                Gmii::Control(Control::InterFrameReady)
            }
        };
        let physical_end = |relative: usize| -> Result<u64> {
            let slot = start + relative as u64;
            let block_start = slot / 10 * 10;
            let block_relative = (block_start - start) as usize;
            let block = std::array::from_fn(|n| octet_at(block_relative + n));
            let within = (slot - block_start) as usize;
            Ok(
                (slot / GMII_PER_PHY_FRAME as u64) * PAM3_SYMBOLS_PER_PHY_FRAME as u64
                    + ((slot % GMII_PER_PHY_FRAME as u64) / 10) * 54
                    + t1_gigabit::data_octet_end_symbol(&block, within)? as u64,
            )
        };
        let range = SourceRange {
            preamble_start_slot: start,
            sfd_end_slot: start + MAC_PREAMBLE.len() as u64,
            fcs_end_slot: fcs_end,
            sfd_end_physical_symbol: physical_end(MAC_PREAMBLE.len() - 1)?,
            fcs_end_physical_symbol: physical_end(MAC_PREAMBLE.len() + bytes.len() - 1)?,
            coded_end_symbol: fcs_end.div_ceil(GMII_PER_PHY_FRAME as u64)
                * PAM3_SYMBOLS_PER_PHY_FRAME as u64,
        };
        self.gmii.extend(MAC_PREAMBLE.into_iter().map(Gmii::Data));
        self.gmii.extend(bytes.iter().copied().map(Gmii::Data));
        self.gmii.extend(std::iter::repeat_n(
            Gmii::Control(Control::InterFrameReady),
            IFG_OCTETS,
        ));
        Ok(range)
    }
    pub fn next_symbol(&mut self) -> Result<i8> {
        if self.pam3.is_empty() {
            let blocks = std::array::from_fn(|_| {
                let octets = std::array::from_fn(|_| {
                    self.gmii
                        .pop_front()
                        .unwrap_or(Gmii::Control(Control::InterFrameReady))
                });
                t1_gigabit::encode_80b81b(&octets)
            });
            self.pam3.extend(t1_gigabit::encode_phy_frame(
                &blocks,
                0,
                &mut self.scrambler,
            )?);
            self.frames_encoded += 1;
        }
        let symbol = self
            .pam3
            .pop_front()
            .ok_or("1000BASE-T1 PCS emitted empty PHY frame")?;
        self.symbols_emitted += 1;
        Ok(symbol)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MacState {
    Idle,
    Preamble {
        matched: usize,
        start_slot: u64,
    },
    Frame {
        bytes: Vec<u8>,
        sfd_end_slot: u64,
        sfd_end_physical_symbol: u64,
        fcs_end_physical_symbol: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivedMac {
    pub bytes: Vec<u8>,
    pub sfd_end_slot: u64,
    pub fcs_end_slot: u64,
    pub sfd_end_physical_symbol: u64,
    pub fcs_end_physical_symbol: u64,
    pub decoded_at_symbol: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiveStream {
    scrambler: SideScrambler,
    pam3: Vec<i8>,
    mac: MacState,
    maximum_mac_bytes: usize,
    frames_decoded: u64,
    corrected_symbols: u64,
    symbols_received: u64,
    gmii_slots_received: u64,
}
impl ReceiveStream {
    pub fn new(partner_role: Role, partner_seed: u16, maximum_mac_bytes: usize) -> Result<Self> {
        if !(64..=16_384).contains(&maximum_mac_bytes) {
            return Err("invalid 1000BASE-T1 MAC receive bound".into());
        }
        Ok(Self {
            scrambler: SideScrambler::new(partner_role, partner_seed)?,
            pam3: Vec::with_capacity(PAM3_SYMBOLS_PER_PHY_FRAME),
            mac: MacState::Idle,
            maximum_mac_bytes,
            frames_decoded: 0,
            corrected_symbols: 0,
            symbols_received: 0,
            gmii_slots_received: 0,
        })
    }
    pub fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }
    pub fn corrected_symbols(&self) -> u64 {
        self.corrected_symbols
    }
    pub fn push_symbol(&mut self, symbol: i8) -> Result<Vec<ReceivedMac>> {
        if !(-1..=1).contains(&symbol) {
            return Err("1000BASE-T1 receive PAM3 symbol outside alphabet".into());
        }
        self.pam3.push(symbol);
        self.symbols_received += 1;
        if self.pam3.len() < PAM3_SYMBOLS_PER_PHY_FRAME {
            return Ok(vec![]);
        }
        let impossible_pair = t1_gigabit::demap_phy_frame(&self.pam3).is_err();
        let frame = match t1_gigabit::decode_phy_frame(&self.pam3, &mut self.scrambler) {
            Ok(frame) => frame,
            Err(reason) => {
                // An invalid ternary pair fails before the core advances its
                // side scrambler.  Its period remains 4050 bits per PHY frame
                // regardless of decisions; advance those bits explicitly.
                if impossible_pair {
                    for _ in 0..t1_gigabit::BITS_PER_PHY_FRAME {
                        self.scrambler.next_mask_bit();
                    }
                }
                // Drop only the in-progress MAC, keeping the next 2700-symbol
                // frame boundary aligned after an uncorrectable RS codeword.
                self.pam3.clear();
                self.mac = MacState::Idle;
                self.gmii_slots_received += GMII_PER_PHY_FRAME as u64;
                return Err(reason);
            }
        };
        self.pam3.clear();
        if frame.oam != 0 {
            return Err("unsupported 1000BASE-T1 optional OAM data".into());
        }
        self.corrected_symbols += frame.corrected_symbols as u64;
        let decoded = (|| -> Result<Vec<ReceivedMac>> {
            let mut received = vec![];
            let frame_index = self.symbols_received / PAM3_SYMBOLS_PER_PHY_FRAME as u64 - 1;
            for (block_index, block) in frame.blocks.into_iter().enumerate() {
                let gmii = t1_gigabit::decode_80b81b(&block)?;
                for (octet_index, octet) in gmii.iter().copied().enumerate() {
                    let physical_end = if matches!(octet, Gmii::Data(_)) {
                        Some(
                            frame_index * PAM3_SYMBOLS_PER_PHY_FRAME as u64
                                + block_index as u64 * 54
                                + t1_gigabit::data_octet_end_symbol(&gmii, octet_index)? as u64,
                        )
                    } else {
                        None
                    };
                    if let Some(mac) = self.push_gmii(octet, physical_end)? {
                        received.push(mac);
                    }
                }
            }
            Ok(received)
        })();
        if decoded.is_err() {
            self.mac = MacState::Idle;
            self.gmii_slots_received = self.symbols_received / PAM3_SYMBOLS_PER_PHY_FRAME as u64
                * GMII_PER_PHY_FRAME as u64;
        }
        decoded
    }
    fn push_gmii(&mut self, octet: Gmii, physical_end: Option<u64>) -> Result<Option<ReceivedMac>> {
        let slot = self.gmii_slots_received;
        self.gmii_slots_received += 1;
        match (&mut self.mac, octet) {
            (MacState::Idle, Gmii::Data(0x55)) => {
                self.mac = MacState::Preamble {
                    matched: 1,
                    start_slot: slot,
                }
            }
            (MacState::Preamble { matched, .. }, Gmii::Data(0x55)) if *matched < 7 => *matched += 1,
            (MacState::Preamble { matched: 7, .. }, Gmii::Data(0xd5)) => {
                self.mac = MacState::Frame {
                    bytes: Vec::new(),
                    sfd_end_slot: slot + 1,
                    sfd_end_physical_symbol: physical_end.ok_or("missing SFD physical cut")?,
                    fcs_end_physical_symbol: physical_end.ok_or("missing SFD physical cut")?,
                }
            }
            (MacState::Preamble { .. }, Gmii::Control(_)) => self.mac = MacState::Idle,
            (
                MacState::Frame {
                    bytes,
                    fcs_end_physical_symbol,
                    ..
                },
                Gmii::Data(value),
            ) => {
                if bytes.len() >= self.maximum_mac_bytes {
                    self.mac = MacState::Idle;
                    return Err("1000BASE-T1 received MAC frame exceeds bound".into());
                }
                bytes.push(value);
                *fcs_end_physical_symbol = physical_end.ok_or("missing FCS physical cut")?;
            }
            (MacState::Frame { .. }, Gmii::Control(Control::Error)) => {
                self.mac = MacState::Idle;
                return Err("1000BASE-T1 PCS error control in MAC frame".into());
            }
            (MacState::Frame { .. }, Gmii::Control(_)) => {
                let MacState::Frame {
                    bytes,
                    sfd_end_slot,
                    sfd_end_physical_symbol,
                    fcs_end_physical_symbol,
                } = std::mem::replace(&mut self.mac, MacState::Idle)
                else {
                    unreachable!()
                };
                wire::validate_mac(&bytes, self.maximum_mac_bytes)?;
                self.frames_decoded += 1;
                return Ok(Some(ReceivedMac {
                    bytes,
                    sfd_end_slot,
                    fcs_end_slot: slot,
                    sfd_end_physical_symbol,
                    fcs_end_physical_symbol,
                    decoded_at_symbol: self.symbols_received,
                }));
            }
            (MacState::Idle, Gmii::Control(_)) => {}
            _ => {
                self.mac = MacState::Idle;
                return Err("1000BASE-T1 invalid GMII preamble or delimiter".into());
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ethernet::crc32;
    fn mac(size: usize) -> Vec<u8> {
        let mut bytes = (0..size - 4).map(|n| (n * 17) as u8).collect::<Vec<_>>();
        bytes.extend(crc32(&bytes).to_le_bytes());
        bytes
    }
    #[test]
    fn continuous_phy_frames_recover_short_and_cross_boundary_mac() {
        for size in [64, 1518] {
            let frame = mac(size);
            let mut tx = TransmitStream::new(Role::Master, 0x2255).unwrap();
            let mut rx = ReceiveStream::new(Role::Master, 0x2255, 1518).unwrap();
            let range = tx.enqueue_mac(&frame, 1518).unwrap();
            assert_eq!(range.preamble_start_slot, 0);
            assert_eq!(range.sfd_end_slot, 8);
            assert_eq!(range.fcs_end_slot, 8 + size as u64);
            assert_eq!(range.sfd_end_symbol(), 44);
            if size == 64 {
                // Slot 71 precedes the first IFG control in its 80B/81B
                // block; the final FCS bits end at PAM3 symbol 394.
                assert_eq!(range.fcs_end_symbol(), 394);
            }
            let mut got = vec![];
            for _ in 0..4 * PAM3_SYMBOLS_PER_PHY_FRAME {
                got.extend(rx.push_symbol(tx.next_symbol().unwrap()).unwrap());
            }
            assert_eq!(
                got.iter()
                    .map(|m: &ReceivedMac| &m.bytes)
                    .collect::<Vec<_>>(),
                vec![&frame]
            );
            assert_eq!(rx.corrected_symbols(), 0);
            assert_eq!(rx.frames_decoded(), 1);
            assert_eq!(got[0].sfd_end_slot, range.sfd_end_slot);
            assert_eq!(got[0].fcs_end_slot, range.fcs_end_slot);
            assert_eq!(got[0].sfd_end_physical_symbol, range.sfd_end_symbol());
            assert_eq!(got[0].fcs_end_physical_symbol, range.fcs_end_symbol());
            assert_eq!(got[0].decoded_at_symbol, range.coded_end_symbol);
        }
    }
}
