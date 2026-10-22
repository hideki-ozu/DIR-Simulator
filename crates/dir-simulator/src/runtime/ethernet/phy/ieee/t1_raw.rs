//! Causal xMII ingress and systematic Clause 97 data streaming.
//! The first PHY frame is actual coded idle.  Its last 360 PAM3 symbols
//! overlap the first sixty 8 ns xMII slots, giving a 480 ns ingress lead.
use super::super::Result;
use super::rs9;
use super::t1_gigabit::{
    self, BLOCKS_PER_PHY_FRAME, Control, Gmii, PAM3_SYMBOLS_PER_PHY_FRAME, Role, SideScrambler,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const GMII_PER_FRAME: usize = BLOCKS_PER_PHY_FRAME * 10;
const BLOCK_PAM3: usize = 54;
const FIRST_INGRESS_SYMBOL: u64 = 2340;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTransmitStream {
    scrambler: SideScrambler,
    startup_idle: VecDeque<i8>,
    gmii: VecDeque<Gmii>,
    segment: VecDeque<i8>,
    info: Vec<u16>,
    emitted_symbols: u64,
    consumed_slots: u64,
    /// Delimiters accepted at xMII, awaiting their actual ten-octet layout.
    delimiter_slots: VecDeque<u64>,
    delimiter_ends: VecDeque<RawDelimiterEnd>,
}
impl RawTransmitStream {
    pub fn validate(&self) -> bool {
        self.startup_idle.len() <= PAM3_SYMBOLS_PER_PHY_FRAME
            && self.gmii.len() <= 64
            && self.segment.len() <= 270
            && self.delimiter_slots.len() <= 64
            && self.delimiter_ends.len() <= 64
            && self
                .delimiter_slots
                .iter()
                .all(|&slot| slot < self.consumed_slots)
            && self.delimiter_ends.iter().all(|marker| {
                marker.slot < self.consumed_slots
                    && marker.end_symbol > self.emitted_symbols.saturating_sub(54)
            })
            && self.info.len() == rs9::DATA_SYMBOLS
            && self.info.iter().all(|&symbol| symbol <= 0x1ff)
            && self.consumed_slots
                == self
                    .emitted_symbols
                    .saturating_sub(FIRST_INGRESS_SYMBOL)
                    .div_ceil(6)
    }
    pub fn new(role: Role, seed: u16) -> Result<Self> {
        let mut scrambler = SideScrambler::new(role, seed)?;
        let idle = t1_gigabit::encode_80b81b(&[Gmii::Control(Control::InterFrameReady); 10]);
        let blocks = [idle; BLOCKS_PER_PHY_FRAME];
        let startup_idle = t1_gigabit::encode_phy_frame(&blocks, 0, &mut scrambler)?;
        Ok(Self {
            scrambler,
            startup_idle: startup_idle.into(),
            gmii: VecDeque::new(),
            segment: VecDeque::new(),
            info: vec![0; rs9::DATA_SYMBOLS],
            emitted_symbols: 0,
            consumed_slots: 0,
            delimiter_slots: VecDeque::new(),
            delimiter_ends: VecDeque::new(),
        })
    }
    pub fn emitted_symbols(&self) -> u64 {
        self.emitted_symbols
    }
    pub fn consumed_slots(&self) -> u64 {
        self.consumed_slots
    }
    pub fn ingress_due(&self) -> bool {
        self.emitted_symbols >= FIRST_INGRESS_SYMBOL
            && (self.emitted_symbols - FIRST_INGRESS_SYMBOL) % 6 == 0
    }
    pub fn symbols_to_next_ingress(&self) -> u64 {
        if self.emitted_symbols < FIRST_INGRESS_SYMBOL {
            FIRST_INGRESS_SYMBOL - self.emitted_symbols
        } else {
            (6 - (self.emitted_symbols - FIRST_INGRESS_SYMBOL) % 6) % 6
        }
    }
    pub fn push_gmii(&mut self, value: Gmii) -> Result<()> {
        self.push_gmii_marked(value, false)
    }
    pub fn push_gmii_marked(&mut self, value: Gmii, delimiter: bool) -> Result<()> {
        if !self.ingress_due() || self.gmii.len() >= 64 {
            return Err("1000BASE-T1 xMII input missed its bounded 8 ns slot".into());
        }
        if delimiter {
            if !matches!(value, Gmii::Data(_)) || self.delimiter_slots.len() >= 64 {
                return Err("invalid bounded raw delimiter marker".into());
            }
            self.delimiter_slots.push_back(self.consumed_slots);
        }
        self.gmii.push_back(value);
        self.consumed_slots += 1;
        Ok(())
    }
    /// Call only after the renderer completes the current UI.
    pub fn take_completed_delimiter(&mut self) -> Option<RawDelimiterEnd> {
        if self
            .delimiter_ends
            .front()
            .is_some_and(|marker| marker.end_symbol <= self.emitted_symbols)
        {
            self.delimiter_ends.pop_front()
        } else {
            None
        }
    }
    fn encoded_symbols(&mut self, mut bits: Vec<u8>) -> Result<VecDeque<i8>> {
        self.scrambler.apply_bits(&mut bits)?;
        let mut symbols = VecDeque::with_capacity(bits.len() * 2 / 3);
        for group in bits.chunks_exact(3) {
            symbols.extend(t1_gigabit::map_3b2t([group[0], group[1], group[2]])?);
        }
        Ok(symbols)
    }
    fn load_segment(&mut self, position: usize) -> Result<()> {
        match position.cmp(&(BLOCKS_PER_PHY_FRAME * BLOCK_PAM3)) {
            std::cmp::Ordering::Less => {
                if position % BLOCK_PAM3 != 0 {
                    return Err("1000BASE-T1 systematic block lost symbol phase".into());
                }
                let block = position / BLOCK_PAM3;
                if self.gmii.len() < 10 {
                    return Err("1000BASE-T1 xMII ingress violated 480 ns lead".into());
                }
                let block_start_slot = self.consumed_slots - self.gmii.len() as u64;
                let octets = std::array::from_fn(|_| self.gmii.pop_front().unwrap());
                while self
                    .delimiter_slots
                    .front()
                    .is_some_and(|&slot| slot < block_start_slot + 10)
                {
                    let slot = self.delimiter_slots.pop_front().unwrap();
                    let within = usize::try_from(
                        slot.checked_sub(block_start_slot)
                            .ok_or("raw delimiter fell behind sealed block")?,
                    )
                    .unwrap();
                    let end_symbol = self.emitted_symbols
                        + t1_gigabit::data_octet_end_symbol(&octets, within)? as u64;
                    if self.delimiter_ends.len() >= 64 {
                        return Err("raw sealed delimiter marker bound exceeded".into());
                    }
                    self.delimiter_ends
                        .push_back(RawDelimiterEnd { slot, end_symbol });
                }
                let bits = t1_gigabit::encode_80b81b(&octets);
                for (i, chunk) in bits.chunks_exact(9).enumerate() {
                    self.info[block * 9 + i] = chunk
                        .iter()
                        .enumerate()
                        .fold(0u16, |n, (b, bit)| n | ((*bit as u16) << b));
                }
                self.segment = self.encoded_symbols(bits.to_vec())?;
            }
            std::cmp::Ordering::Equal => {
                self.info[rs9::DATA_SYMBOLS - 1] = 0; // Optional OAM unavailable.
                let code = rs9::encode(&self.info)?;
                let mut bits = Vec::with_capacity((rs9::PARITY_SYMBOLS + 1) * 9);
                for symbol in &code[rs9::DATA_SYMBOLS - 1..] {
                    for bit in 0..9 {
                        bits.push(((symbol >> bit) & 1) as u8);
                    }
                }
                self.segment = self.encoded_symbols(bits)?;
            }
            std::cmp::Ordering::Greater => {
                return Err("1000BASE-T1 parity segment lost frame phase".into());
            }
        }
        Ok(())
    }
    pub fn next_symbol(&mut self) -> Result<i8> {
        let symbol = if self.emitted_symbols < PAM3_SYMBOLS_PER_PHY_FRAME as u64 {
            self.startup_idle
                .pop_front()
                .ok_or("1000BASE-T1 startup idle underrun")?
        } else {
            let position = ((self.emitted_symbols - PAM3_SYMBOLS_PER_PHY_FRAME as u64)
                % PAM3_SYMBOLS_PER_PHY_FRAME as u64) as usize;
            if self.segment.is_empty() {
                self.load_segment(position)?;
            }
            self.segment
                .pop_front()
                .ok_or("1000BASE-T1 segment underrun")?
        };
        self.emitted_symbols += 1;
        Ok(symbol)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDelimiterEnd {
    pub slot: u64,
    pub end_symbol: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivedRaw {
    pub bytes: Vec<u8>,
    pub start_slot: u64,
    pub terminator_slot: u64,
    pub decoded_at_symbol: u64,
    /// Physical delimiter end relative to the actual received data onset.
    pub delimiter_end_symbol: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PacketState {
    Idle,
    Preamble {
        bytes: Vec<u8>,
        start_slot: u64,
    },
    Packet {
        bytes: Vec<u8>,
        start_slot: u64,
        delimiter_end_symbol: u64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawReceiveStream {
    scrambler: SideScrambler,
    pam3: Vec<i8>,
    packet: PacketState,
    symbols_received: u64,
    slots_received: u64,
    skip_initial_idle: bool,
    initial_idle_verified: bool,
    maximum_bytes: usize,
}
impl RawReceiveStream {
    pub fn validate(&self) -> bool {
        self.pam3.len() < PAM3_SYMBOLS_PER_PHY_FRAME
            && self.pam3.iter().all(|&value| (-1..=1).contains(&value))
            && (!self.initial_idle_verified
                || !self.skip_initial_idle
                    && self.symbols_received >= PAM3_SYMBOLS_PER_PHY_FRAME as u64)
            && self.slots_received
                <= self.symbols_received / PAM3_SYMBOLS_PER_PHY_FRAME as u64 * GMII_PER_FRAME as u64
    }
    pub fn new(partner_role: Role, partner_seed: u16, maximum_bytes: usize) -> Result<Self> {
        if !(72..=16_400).contains(&maximum_bytes) {
            return Err("invalid raw mPacket receive bound".into());
        }
        Ok(Self {
            scrambler: SideScrambler::new(partner_role, partner_seed)?,
            pam3: Vec::with_capacity(PAM3_SYMBOLS_PER_PHY_FRAME),
            packet: PacketState::Idle,
            symbols_received: 0,
            slots_received: 0,
            skip_initial_idle: true,
            initial_idle_verified: false,
            maximum_bytes,
        })
    }
    pub fn initial_idle_verified(&self) -> bool {
        self.initial_idle_verified
    }
    pub fn symbols_received(&self) -> u64 {
        self.symbols_received
    }
    pub fn slots_received(&self) -> u64 {
        self.slots_received
    }
    pub fn push_symbol(&mut self, symbol: i8) -> Result<Vec<ReceivedRaw>> {
        self.pam3.push(symbol);
        self.symbols_received += 1;
        if self.pam3.len() < PAM3_SYMBOLS_PER_PHY_FRAME {
            return Ok(vec![]);
        }
        let expected_slots =
            (self.symbols_received / PAM3_SYMBOLS_PER_PHY_FRAME as u64 - 1) * GMII_PER_FRAME as u64;
        let impossible_pair = t1_gigabit::demap_phy_frame(&self.pam3).is_err();
        let decoded = t1_gigabit::decode_phy_frame(&self.pam3, &mut self.scrambler);
        self.pam3.clear();
        let frame = match decoded {
            Ok(frame) => frame,
            Err(error) => {
                if impossible_pair {
                    for _ in 0..t1_gigabit::BITS_PER_PHY_FRAME {
                        self.scrambler.next_mask_bit();
                    }
                }
                self.packet = PacketState::Idle;
                self.slots_received = expected_slots;
                self.skip_initial_idle = false;
                return Err(error);
            }
        };
        if frame.oam != 0 {
            self.packet = PacketState::Idle;
            self.slots_received = expected_slots;
            return Err("unsupported 1000BASE-T1 OAM data".into());
        }
        if self.skip_initial_idle {
            self.skip_initial_idle = false;
            if self.symbols_received != PAM3_SYMBOLS_PER_PHY_FRAME as u64
                || frame.blocks.iter().any(|block| {
                    t1_gigabit::decode_80b81b(block)
                        != Ok([Gmii::Control(Control::InterFrameReady); 10])
                })
            {
                return Err("raw initial codeword is not the paired startup idle frame".into());
            }
            self.initial_idle_verified = true;
            return Ok(vec![]);
        }
        let result = (|| -> Result<Vec<ReceivedRaw>> {
            let mut packets = Vec::new();
            let frame_base = self.symbols_received - PAM3_SYMBOLS_PER_PHY_FRAME as u64;
            for (block_index, block) in frame.blocks.into_iter().enumerate() {
                let octets = t1_gigabit::decode_80b81b(&block)?;
                for (within, &octet) in octets.iter().enumerate() {
                    let physical_end = if matches!(octet, Gmii::Data(_)) {
                        frame_base
                            + (block_index * BLOCK_PAM3
                                + t1_gigabit::data_octet_end_symbol(&octets, within)?)
                                as u64
                    } else {
                        0
                    };
                    if let Some(packet) = self.push_gmii(octet, physical_end)? {
                        packets.push(packet);
                    }
                }
            }
            Ok(packets)
        })();
        if result.is_err() {
            self.packet = PacketState::Idle;
            self.slots_received = expected_slots;
        }
        result
    }
    fn push_gmii(&mut self, octet: Gmii, physical_end: u64) -> Result<Option<ReceivedRaw>> {
        let slot = self.slots_received;
        self.slots_received += 1;
        match (&mut self.packet, octet) {
            (PacketState::Idle, Gmii::Data(0x55)) => {
                self.packet = PacketState::Preamble {
                    bytes: vec![0x55],
                    start_slot: slot,
                }
            }
            (PacketState::Preamble { bytes, .. }, Gmii::Data(0x55)) if bytes.len() < 7 => {
                bytes.push(0x55)
            }
            (PacketState::Preamble { bytes, .. }, Gmii::Data(value)) if bytes.len() >= 6 => {
                bytes.push(value);
                let PacketState::Preamble { bytes, start_slot } =
                    std::mem::replace(&mut self.packet, PacketState::Idle)
                else {
                    unreachable!()
                };
                self.packet = PacketState::Packet {
                    bytes,
                    start_slot,
                    delimiter_end_symbol: physical_end,
                };
            }
            (PacketState::Packet { bytes, .. }, Gmii::Data(value)) => {
                if bytes.len() >= self.maximum_bytes {
                    self.packet = PacketState::Idle;
                    return Err("raw mPacket exceeds configured bound".into());
                }
                bytes.push(value);
            }
            (
                PacketState::Packet { .. },
                Gmii::Control(Control::InterFrameReady | Control::InterFrameNotReady),
            ) => {
                let PacketState::Packet {
                    bytes,
                    start_slot,
                    delimiter_end_symbol,
                } = std::mem::replace(&mut self.packet, PacketState::Idle)
                else {
                    unreachable!()
                };
                if bytes.len() < 72 {
                    return Err("raw mPacket terminated below 72 octets".into());
                }
                return Ok(Some(ReceivedRaw {
                    bytes,
                    start_slot,
                    terminator_slot: slot,
                    decoded_at_symbol: self.symbols_received,
                    delimiter_end_symbol,
                }));
            }
            (PacketState::Packet { .. }, Gmii::Control(Control::Error)) => {
                self.packet = PacketState::Idle;
                return Err("raw mPacket terminated by PCS error".into());
            }
            (PacketState::Preamble { .. }, Gmii::Control(_)) => self.packet = PacketState::Idle,
            (PacketState::Idle, _) => {}
            _ => {
                self.packet = PacketState::Idle;
                return Err("raw mPacket preamble invalid".into());
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Literal coordinates independently calculated from the 81-bit block:
    /// data octet i ends at 2*ceil((8*i+9)/3); each block contributes54 UI.
    #[test]
    fn mapped_delimiters_have_golden_all_ten_phases_and_codeword_crossings() {
        let coordinates = [2744, 2750, 2754, 2760, 2766, 2772, 2776, 2782, 2788, 2792];
        for (start, expected) in (0..10).zip(coordinates).chain([(442, 5130), (443, 5406)]) {
            let packet = [vec![0x55; 7], vec![0xe6], vec![0x37; 64]].concat();
            let (markers, packets) = mapped_roundtrip(&[(start, packet.clone(), 7)], None);
            assert_eq!(
                markers,
                vec![RawDelimiterEnd {
                    slot: start as u64 + 7,
                    end_symbol: expected
                }]
            );
            assert_eq!(packets.len(), 1);
            assert_eq!(packets[0].bytes, packet);
            assert_eq!(packets[0].delimiter_end_symbol, expected);
            assert!(packets[0].decoded_at_symbol > expected);
        }
    }
    fn mapped_roundtrip(
        inputs: &[(usize, Vec<u8>, usize)],
        checkpoint: Option<u64>,
    ) -> (Vec<RawDelimiterEnd>, Vec<ReceivedRaw>) {
        let mut gmii = vec![Gmii::Control(Control::InterFrameReady); 1600];
        let mut delimiter_slots = Vec::new();
        for (start, packet, delimiter) in inputs {
            for (index, &value) in packet.iter().enumerate() {
                gmii[start + index] = Gmii::Data(value);
            }
            delimiter_slots.push(start + delimiter);
        }
        let mut tx = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut rx = RawReceiveStream::new(Role::Master, 0x2255, 16_400).unwrap();
        let mut markers = Vec::new();
        let mut packets = Vec::new();
        let mut slot = 0;
        for _ in 0..4 * PAM3_SYMBOLS_PER_PHY_FRAME {
            if tx.ingress_due() {
                tx.push_gmii_marked(gmii[slot], delimiter_slots.contains(&slot))
                    .unwrap();
                slot += 1;
            }
            let symbol = tx.next_symbol().unwrap();
            while let Some(marker) = tx.take_completed_delimiter() {
                assert_eq!(marker.end_symbol, tx.emitted_symbols());
                markers.push(marker);
            }
            packets.extend(rx.push_symbol(symbol).unwrap());
            if checkpoint == Some(tx.emitted_symbols()) {
                assert!(tx.validate() && rx.validate());
                tx = serde_json::from_slice(&serde_json::to_vec(&tx).unwrap()).unwrap();
                rx = serde_json::from_slice(&serde_json::to_vec(&rx).unwrap()).unwrap();
            }
        }
        (markers, packets)
    }
    #[test]
    fn mapped_delimiter_continuation_two_packets_and_next_codeword_terminator_survive_checkpoint() {
        let first = [vec![0x55; 7], vec![0xe6], vec![0x37; 64]].concat();
        let continuation = [vec![0x55; 6], vec![0x61, 0xe6], vec![0xa4; 64]].concat();
        let inputs = [
            (0, first.clone(), 7),
            (84, continuation, 6),
            (378, first, 7),
        ];
        let expected = mapped_roundtrip(&inputs, None);
        assert_eq!(
            expected.0.iter().map(|m| m.end_symbol).collect::<Vec<_>>(),
            [2744, 3192, 4786]
        );
        assert_eq!(expected.1.len(), 3);
        assert_eq!(expected.1[2].terminator_slot, 450);
        assert_eq!(expected.1[2].decoded_at_symbol, 8100);
        for cut in [2701, 2743, 2744, 5399, 8099] {
            assert_eq!(
                mapped_roundtrip(&inputs, Some(cut)),
                expected,
                "checkpoint {cut}"
            );
        }
    }
    #[test]
    fn mapped_delimiter_uses_sealed_mixed_control_layout() {
        // Octet7 has upper bits in the following 8-bit group when octet9 is idle.
        // Its final bit is bit69, hence the mapped end is48 UI (not44).
        let mut tx = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut slot = 0;
        let mut markers = Vec::new();
        for _ in 0..2800 {
            if tx.ingress_due() {
                let octet = if slot < 9 {
                    Gmii::Data(slot as u8)
                } else {
                    Gmii::Control(Control::InterFrameReady)
                };
                tx.push_gmii_marked(octet, slot == 7).unwrap();
                slot += 1;
            }
            tx.next_symbol().unwrap();
            while let Some(marker) = tx.take_completed_delimiter() {
                markers.push(marker);
            }
        }
        assert_eq!(
            markers,
            [RawDelimiterEnd {
                slot: 7,
                end_symbol: 2748
            }]
        );
    }
    #[test]
    fn later_valid_idle_cannot_replace_failed_initial_codeword_alignment() {
        let mut tx = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut symbols = Vec::new();
        for _ in 0..2 * PAM3_SYMBOLS_PER_PHY_FRAME {
            if tx.ingress_due() {
                tx.push_gmii(Gmii::Control(Control::InterFrameReady))
                    .unwrap();
            }
            symbols.push(tx.next_symbol().unwrap());
        }
        for rs_symbol in 0..23 {
            let at = rs_symbol * 6;
            let replacement = if symbols[at..at + 2] == [-1, -1] {
                [1, 1]
            } else {
                [-1, -1]
            };
            symbols[at..at + 2].copy_from_slice(&replacement);
        }
        let mut rx = RawReceiveStream::new(Role::Master, 0x2255, 16_400).unwrap();
        let mut failures = 0;
        for symbol in symbols {
            if rx.push_symbol(symbol).is_err() {
                failures += 1;
            }
        }
        assert_eq!(failures, 1);
        assert_eq!(rx.symbols_received(), 5400);
        assert!(!rx.initial_idle_verified());
        assert!(rx.validate());
    }
    #[test]
    fn incremental_systematic_stream_equals_whole_frame_for_mixed_gmii() {
        let mut gmii = vec![Gmii::Control(Control::InterFrameReady); 1000];
        for (i, octet) in gmii.iter_mut().enumerate().take(830).skip(15) {
            *octet = if i % 37 == 0 {
                Gmii::Control(Control::InterFrameReady)
            } else {
                Gmii::Data((i * 37) as u8)
            };
        }
        let mut streaming = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut actual = Vec::new();
        let mut next_slot = 0;
        for _ in 0..3 * PAM3_SYMBOLS_PER_PHY_FRAME {
            if streaming.ingress_due() {
                streaming.push_gmii(gmii[next_slot]).unwrap();
                next_slot += 1;
            }
            actual.push(streaming.next_symbol().unwrap());
        }
        let mut scrambler = SideScrambler::new(Role::Master, 0x2255).unwrap();
        let idle = t1_gigabit::encode_80b81b(&[Gmii::Control(Control::InterFrameReady); 10]);
        let mut expected =
            t1_gigabit::encode_phy_frame(&[idle; BLOCKS_PER_PHY_FRAME], 0, &mut scrambler).unwrap();
        for frame in 0..2 {
            let blocks = std::array::from_fn(|block| {
                let slots: [Gmii; 10] = gmii
                    [frame * GMII_PER_FRAME + block * 10..frame * GMII_PER_FRAME + block * 10 + 10]
                    .try_into()
                    .unwrap();
                t1_gigabit::encode_80b81b(&slots)
            });
            expected.extend(t1_gigabit::encode_phy_frame(&blocks, 0, &mut scrambler).unwrap());
        }
        assert_eq!(actual, expected);
        assert_eq!(streaming.consumed_slots(), 960);
    }
    #[test]
    fn two_raw_packets_in_one_codeword_keep_exact_bytes() {
        let a = [vec![0x55; 7], vec![0xe6], vec![0x37; 64]].concat();
        let b = [vec![0x55; 6], vec![0x61, 0xe6], vec![0xa4; 64]].concat();
        let mut gmii = vec![Gmii::Control(Control::InterFrameReady); 1000];
        for (start, packet) in [(0, &a), (84, &b)] {
            for (i, &byte) in packet.iter().enumerate() {
                gmii[start + i] = Gmii::Data(byte);
            }
        }
        let mut tx = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut rx = RawReceiveStream::new(Role::Master, 0x2255, 16_400).unwrap();
        let mut got = Vec::new();
        let mut source_slot = 0;
        for _ in 0..2 * PAM3_SYMBOLS_PER_PHY_FRAME {
            if tx.ingress_due() {
                tx.push_gmii(gmii[source_slot]).unwrap();
                source_slot += 1;
            }
            got.extend(rx.push_symbol(tx.next_symbol().unwrap()).unwrap());
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].bytes, a);
        assert_eq!(got[1].bytes, b);
        assert_eq!((got[0].start_slot, got[0].terminator_slot), (0, 72));
        assert_eq!((got[1].start_slot, got[1].terminator_slot), (84, 156));
    }
    #[test]
    fn raw_packet_survives_22_symbol_errors_and_rejects_23() {
        let packet = [vec![0x55; 7], vec![0xe6], vec![0x37; 64]].concat();
        let mut gmii = vec![Gmii::Control(Control::InterFrameReady); 1000];
        for (i, &byte) in packet.iter().enumerate() {
            gmii[i] = Gmii::Data(byte);
        }
        let mut tx = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut symbols = Vec::new();
        let mut source_slot = 0;
        for _ in 0..2 * PAM3_SYMBOLS_PER_PHY_FRAME {
            if tx.ingress_due() {
                tx.push_gmii(gmii[source_slot]).unwrap();
                source_slot += 1;
            }
            symbols.push(tx.next_symbol().unwrap());
        }
        for errors in [22, 23] {
            let mut damaged = symbols.clone();
            for rs_symbol in 0..errors {
                let at = PAM3_SYMBOLS_PER_PHY_FRAME + rs_symbol * 6;
                let replacement = if damaged[at..at + 2] == [-1, -1] {
                    [1, 1]
                } else {
                    [-1, -1]
                };
                damaged[at..at + 2].copy_from_slice(&replacement);
            }
            let mut rx = RawReceiveStream::new(Role::Master, 0x2255, 16_400).unwrap();
            let mut recovered = Vec::new();
            let mut failures = 0;
            for symbol in damaged {
                match rx.push_symbol(symbol) {
                    Ok(packets) => recovered.extend(packets),
                    Err(_) => failures += 1,
                }
            }
            if errors == 22 {
                assert_eq!(failures, 0);
                assert_eq!(recovered.len(), 1);
                assert_eq!(recovered[0].bytes, packet);
            } else {
                assert_eq!(failures, 1);
                assert!(recovered.is_empty());
            }
        }
    }
    #[test]
    fn checkpoint_preserves_partial_block_systematic_and_parity_prefixes() {
        let gmii = (0..1500)
            .map(|n| {
                if n % 17 == 0 {
                    Gmii::Control(Control::InterFrameReady)
                } else {
                    Gmii::Data((n * 13) as u8)
                }
            })
            .collect::<Vec<_>>();
        for cut in [2351, 3100, 5130, 5310] {
            let mut first = RawTransmitStream::new(Role::Slave, 0x315a).unwrap();
            let mut slot = 0;
            while first.emitted_symbols() < cut {
                if first.ingress_due() {
                    first.push_gmii(gmii[slot]).unwrap();
                    slot += 1;
                }
                first.next_symbol().unwrap();
            }
            assert!(first.validate());
            let saved = serde_json::to_vec(&first).unwrap();
            let mut restored: RawTransmitStream = serde_json::from_slice(&saved).unwrap();
            assert!(restored.validate());
            for _ in 0..500 {
                assert_eq!(first.ingress_due(), restored.ingress_due());
                if first.ingress_due() {
                    first.push_gmii(gmii[slot]).unwrap();
                    restored.push_gmii(gmii[slot]).unwrap();
                    slot += 1;
                }
                assert_eq!(
                    first.next_symbol().unwrap(),
                    restored.next_symbol().unwrap()
                );
            }
        }
    }
}
