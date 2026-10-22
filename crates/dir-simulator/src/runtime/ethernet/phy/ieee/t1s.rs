//! Clause 147 data-nibble PCS and DME building blocks for 10BASE-T1S.
//!
//! Table 147-1 assigns 5B code groups. Figure 147-6 defines the
//! self-synchronizing x^17+x^14+1 scrambler; 147.4.2 sends each 5B group LSB
//! first with a start transition per 80 ns bit and a mid-bit transition for 1.
use super::super::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const CODED_BIT_PS: u64 = 80_000;
pub const HALF_BIT_PS: u64 = CODED_BIT_PS / 2;
pub const CONTRACT: &str = "ieee8023.10base_t1s.multidrop.2022";
pub const DATA_GROUPS: [u8; 16] = [
    0b11110, 0b01001, 0b10100, 0b10101, 0b01010, 0b01011, 0b01110, 0b01111, 0b10010, 0b10011,
    0b10110, 0b10111, 0b11010, 0b11011, 0b11100, 0b11101,
];
pub const SILENCE: u8 = 0b11111;
pub const SYNC_COMMIT: u8 = 0b11000;
pub const ESD_ERROR: u8 = 0b10001;
pub const ESD_HEARTBEAT: u8 = 0b01101;
pub const ESD_OK_BURST: u8 = 0b00111;
pub const SSD: u8 = 0b00100;
pub const BEACON: u8 = 0b01000;
pub const ESD_JABBER: u8 = 0b11001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Data(u8),
    Silence,
    SyncCommit,
    EsdError,
    EsdHeartbeat,
    EsdOkBurst,
    Ssd,
    Beacon,
    EsdJabber,
}
pub fn decode_group(code: u8) -> Result<Group> {
    if let Some(nibble) = DATA_GROUPS.iter().position(|&group| group == code) {
        return Ok(Group::Data(nibble as u8));
    }
    match code {
        SILENCE => Ok(Group::Silence),
        SYNC_COMMIT => Ok(Group::SyncCommit),
        ESD_ERROR => Ok(Group::EsdError),
        ESD_HEARTBEAT => Ok(Group::EsdHeartbeat),
        ESD_OK_BURST => Ok(Group::EsdOkBurst),
        SSD => Ok(Group::Ssd),
        BEACON => Ok(Group::Beacon),
        ESD_JABBER => Ok(Group::EsdJabber),
        _ => Err("invalid 10BASE-T1S 5B code group".into()),
    }
}
pub fn encode_group(group: Group) -> Result<u8> {
    match group {
        Group::Data(value) => DATA_GROUPS
            .get(value as usize)
            .copied()
            .ok_or("10BASE-T1S data nibble exceeds 4 bits".into()),
        Group::Silence => Ok(SILENCE),
        Group::SyncCommit => Ok(SYNC_COMMIT),
        Group::EsdError => Ok(ESD_ERROR),
        Group::EsdHeartbeat => Ok(ESD_HEARTBEAT),
        Group::EsdOkBurst => Ok(ESD_OK_BURST),
        Group::Ssd => Ok(SSD),
        Group::Beacon => Ok(BEACON),
        Group::EsdJabber => Ok(ESD_JABBER),
    }
}

/// Most recently transmitted or received scrambled bit is state bit zero.
/// Nonzero reset seeds are caller supplied; state is serializable at cuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scrambler {
    state: u32,
}
impl Scrambler {
    pub fn new(seed: u32) -> Result<Self> {
        if seed == 0 || seed > 0x1ffff {
            return Err("10BASE-T1S scrambler requires a nonzero 17-bit seed".into());
        }
        Ok(Self { state: seed })
    }
    pub fn state(self) -> u32 {
        self.state
    }
    fn feedback(self) -> u8 {
        (((self.state >> 13) ^ (self.state >> 16)) & 1) as u8
    }
    fn shift(&mut self, scrambled_bit: u8) {
        self.state = ((self.state << 1) | scrambled_bit as u32) & 0x1ffff;
    }
    pub fn scramble_bit(&mut self, data_bit: u8) -> Result<u8> {
        if data_bit > 1 {
            return Err("scrambler input is not one bit".into());
        }
        let output = data_bit ^ self.feedback();
        self.shift(output);
        Ok(output)
    }
    pub fn descramble_bit(&mut self, wire_bit: u8) -> Result<u8> {
        if wire_bit > 1 {
            return Err("descrambler input is not one bit".into());
        }
        let output = wire_bit ^ self.feedback();
        self.shift(wire_bit);
        Ok(output)
    }
    pub fn scramble_nibble(&mut self, nibble: u8) -> Result<u8> {
        if nibble > 15 {
            return Err("10BASE-T1S nibble exceeds four bits".into());
        }
        let mut output = 0;
        for bit in 0..4 {
            output |= self.scramble_bit((nibble >> bit) & 1)? << bit;
        }
        Ok(output)
    }
    pub fn descramble_nibble(&mut self, nibble: u8) -> Result<u8> {
        if nibble > 15 {
            return Err("10BASE-T1S nibble exceeds four bits".into());
        }
        let mut output = 0;
        for bit in 0..4 {
            output |= self.descramble_bit((nibble >> bit) & 1)? << bit;
        }
        Ok(output)
    }
}

/// Two half-bit levels per coded bit. A zero level means high impedance.
/// The caller maps +/-1 to the configured differential TX pulse before the
/// numerical channel, and advances exactly one 40 ns half-bit per chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmeTransmitter {
    level: i8,
    active: bool,
}
impl Default for DmeTransmitter {
    fn default() -> Self {
        Self {
            level: -1,
            active: false,
        }
    }
}
impl DmeTransmitter {
    pub fn resume_after_interruption(&mut self, last_driven_chip: i8) -> Result<()> {
        if ![-1, 1].contains(&last_driven_chip) {
            return Err("DME resume needs last driven chip".into());
        }
        self.level = last_driven_chip;
        self.active = true;
        Ok(())
    }
    fn bit(&mut self, bit: u8, output: &mut Vec<i8>) -> Result<()> {
        if bit > 1 {
            return Err("DME input is not one bit".into());
        }
        self.level = -self.level;
        output.push(self.level);
        if bit == 1 {
            self.level = -self.level;
        }
        output.push(self.level);
        Ok(())
    }
    /// A transition from a driven group to SILENCE emits the mandatory extra
    /// encoded zero before release. Later SILENCE calls emit no driven chips.
    pub fn group(&mut self, code: u8) -> Result<Vec<i8>> {
        decode_group(code)?;
        let mut chips = Vec::with_capacity(12);
        if code == SILENCE {
            if self.active {
                self.bit(0, &mut chips)?;
            }
            self.active = false;
            chips.push(0);
            return Ok(chips);
        }
        self.active = true;
        for bit in 0..5 {
            self.bit((code >> bit) & 1, &mut chips)?;
        }
        Ok(chips)
    }
}
pub fn training_chips(first_level: i8) -> Result<Vec<i8>> {
    if ![-1, 1].contains(&first_level) {
        return Err("invalid DME first level".into());
    }
    let mut transmitter = DmeTransmitter::default();
    transmitter.resume_after_interruption(-first_level)?;
    let mut chips = Vec::with_capacity(40);
    for group in [SYNC_COMMIT, SYNC_COMMIT, SSD, SSD] {
        chips.extend(transmitter.group(group)?);
    }
    Ok(chips)
}

/// Decode aligned two-sample DME chips, checking every required bit-start
/// transition. Sampling/clock recovery and group acquisition belong to PMA.
pub fn decode_aligned_chips(chips: &[i8], prior_level: i8) -> Result<(u8, i8)> {
    if chips.len() != 10 || ![-1, 1].contains(&prior_level) {
        return Err("DME needs ten aligned chips and prior driven level".into());
    }
    let mut prior = prior_level;
    let mut group = 0;
    for (bit, pair) in chips.chunks_exact(2).enumerate() {
        let (first, second) = (pair[0], pair[1]);
        if ![-1, 1].contains(&first) || ![-1, 1].contains(&second) || first != -prior {
            return Err("DME start transition violation".into());
        }
        if second == -first {
            group |= 1 << bit;
        } else if second != first {
            return Err("DME midpoint violation".into());
        }
        prior = second;
    }
    decode_group(group)?;
    Ok((group, prior))
}

/// PMA group acquisition from sampled half-bit decisions. Two received SYNC
/// or BEACON groups establish a 5B boundary; every later group is checked for
/// a DME start transition at each coded bit. Receiver reset follows carrier
/// loss, so a new endpoint may start with either polarity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmeReceiver {
    search: VecDeque<i8>,
    aligned: bool,
    prior_level: i8,
    group_chips: Vec<i8>,
}
impl Default for DmeReceiver {
    fn default() -> Self {
        Self {
            search: VecDeque::new(),
            aligned: false,
            prior_level: -1,
            group_chips: vec![],
        }
    }
}
impl DmeReceiver {
    pub fn carrier_lost(&mut self) {
        self.search.clear();
        self.aligned = false;
        self.group_chips.clear();
    }
    /// Returns zero or two groups on acquisition, then one per ten chips.
    pub fn push_chip(&mut self, chip: i8) -> Result<Vec<u8>> {
        if ![-1, 1].contains(&chip) {
            return Err("DME receiver requires a driven half-bit decision".into());
        }
        if self.aligned {
            self.group_chips.push(chip);
            if self.group_chips.len() < 10 {
                return Ok(vec![]);
            }
            let (group, level) = decode_aligned_chips(&self.group_chips, self.prior_level)?;
            self.prior_level = level;
            self.group_chips.clear();
            return Ok(vec![group]);
        }
        self.search.push_back(chip);
        if self.search.len() > 20 {
            self.search.pop_front();
        }
        if self.search.len() == 20 {
            let chips = self.search.make_contiguous();
            let first_prior = -chips[0];
            if let Ok((first, level)) = decode_aligned_chips(&chips[..10], first_prior) {
                if let Ok((second, final_level)) = decode_aligned_chips(&chips[10..], level) {
                    if first == second && [SYNC_COMMIT, BEACON].contains(&first) {
                        self.aligned = true;
                        self.prior_level = final_level;
                        self.search.clear();
                        return Ok(vec![first, second]);
                    }
                }
            }
        }
        Ok(vec![])
    }
}

/// Figure 147-4/5 packet mapping. The first four preamble nibbles become
/// SYNC,SYNC,SSD,SSD; subsequent nibbles are scrambled before Table 147-1
/// coding. The scrambler survives packet boundaries.
pub fn encode_mac_groups(mac_bytes: &[u8], scrambler: &mut Scrambler) -> Result<Vec<u8>> {
    if mac_bytes.len() < 64 || mac_bytes.len() > 16_384 {
        return Err("10BASE-T1S MAC frame length outside configured range".into());
    }
    let mut groups = Vec::with_capacity(4 + 12 + mac_bytes.len() * 2 + 2);
    groups.extend([SYNC_COMMIT, SYNC_COMMIT, SSD, SSD]);
    let preamble = [0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5];
    for (byte_index, &byte) in preamble.iter().chain(mac_bytes).enumerate() {
        let start = if byte_index < 2 { 2 } else { 0 };
        for nibble_index in start..2 {
            let nibble = (byte >> (nibble_index * 4)) & 0xf;
            groups.push(DATA_GROUPS[scrambler.scramble_nibble(nibble)? as usize]);
        }
    }
    groups.extend([ESD_HEARTBEAT, ESD_OK_BURST]);
    Ok(groups)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PcsRxState {
    WaitSync,
    Syncing,
    WaitSsd,
    Preamble,
    Data,
    End,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PcsRxEffect {
    Beacon,
    Commit,
    Start,
    Frame(Vec<u8>),
    Abort(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PcsReceiver {
    state: PcsRxState,
    scrambler: Scrambler,
    preamble_count: u8,
    nibble: Option<u8>,
    bytes: Vec<u8>,
    consecutive_beacons: u8,
    consecutive_sync: u8,
}
impl Default for PcsReceiver {
    fn default() -> Self {
        Self::new()
    }
}
impl PcsReceiver {
    pub fn new() -> Self {
        Self {
            state: PcsRxState::WaitSync,
            // Self-synchronizing after 17 received scrambled bits. The nine
            // preamble groups discarded by Figure 147-7 supply 36 bits.
            scrambler: Scrambler::new(1).expect("nonzero seed"),
            preamble_count: 0,
            nibble: None,
            bytes: vec![],
            consecutive_beacons: 0,
            consecutive_sync: 0,
        }
    }
    fn reset_frame(&mut self) {
        self.state = PcsRxState::WaitSync;
        self.preamble_count = 0;
        self.nibble = None;
        self.bytes.clear();
        self.consecutive_beacons = 0;
        self.consecutive_sync = 0;
    }
    pub fn active_frame(&self) -> bool {
        matches!(
            self.state,
            PcsRxState::Preamble | PcsRxState::Data | PcsRxState::End
        )
    }
    pub fn commit_active(&self) -> bool {
        self.state == PcsRxState::Syncing
    }
    pub fn beacon_active(&self) -> bool {
        self.state == PcsRxState::WaitSync && self.consecutive_beacons >= 2
    }
    pub fn data_state(&self) -> bool {
        self.state == PcsRxState::Data
    }
    pub fn carrier_lost(&mut self) -> Option<PcsRxEffect> {
        let interrupted = self.active_frame();
        self.reset_frame();
        interrupted.then(|| PcsRxEffect::Abort("10base_t1s_carrier_lost".into()))
    }
    /// One aligned PMA 5B indication. BEACON and COMMIT come only from
    /// received line symbols, never a controller-assigned opportunity.
    pub fn push(&mut self, code: u8) -> Result<Option<PcsRxEffect>> {
        let group = decode_group(code)?;
        if self.state == PcsRxState::WaitSync {
            self.consecutive_beacons = if group == Group::Beacon {
                self.consecutive_beacons.saturating_add(1)
            } else {
                0
            };
            self.consecutive_sync = if group == Group::SyncCommit {
                self.consecutive_sync.saturating_add(1)
            } else {
                0
            };
            if self.consecutive_beacons == 2 {
                return Ok(Some(PcsRxEffect::Beacon));
            }
            if self.consecutive_sync == 2 {
                self.state = PcsRxState::Syncing;
                return Ok(Some(PcsRxEffect::Commit));
            }
            return Ok(None);
        }
        match self.state {
            PcsRxState::WaitSync => unreachable!(),
            PcsRxState::Syncing => match group {
                Group::Ssd => {
                    self.state = PcsRxState::WaitSsd;
                    Ok(None)
                }
                Group::SyncCommit => Ok(None),
                _ => {
                    self.reset_frame();
                    Ok(Some(PcsRxEffect::Abort("10base_t1s_bad_ssd".into())))
                }
            },
            PcsRxState::WaitSsd => match group {
                Group::Ssd => {
                    self.state = PcsRxState::Preamble;
                    self.preamble_count = 0;
                    Ok(None)
                }
                _ => {
                    self.reset_frame();
                    Ok(Some(PcsRxEffect::Abort("10base_t1s_bad_ssd".into())))
                }
            },
            PcsRxState::Preamble => {
                let Group::Data(nibble) = group else {
                    self.reset_frame();
                    return Ok(Some(PcsRxEffect::Abort(
                        "10base_t1s_truncated_preamble".into(),
                    )));
                };
                let clear = self.scrambler.descramble_nibble(nibble)?;
                // Figure 147-7 discards nine groups for descrambler lock;
                // the remaining three must be 5, 5, D (last preamble nibble
                // and SFD). This checks packet delineation after convergence.
                let count = self.preamble_count;
                if count >= 9 && clear != [5, 5, 13][(count - 9) as usize] {
                    self.reset_frame();
                    return Ok(Some(PcsRxEffect::Abort("10base_t1s_bad_sfd".into())));
                }
                self.preamble_count += 1;
                if self.preamble_count == 12 {
                    self.state = PcsRxState::Data;
                    return Ok(Some(PcsRxEffect::Start));
                }
                Ok(None)
            }
            PcsRxState::Data => match group {
                Group::Data(nibble) => {
                    let clear = self.scrambler.descramble_nibble(nibble)?;
                    if let Some(low) = self.nibble.take() {
                        self.bytes.push(low | (clear << 4));
                        if self.bytes.len() > 16_384 {
                            self.reset_frame();
                            return Ok(Some(PcsRxEffect::Abort("10base_t1s_mac_too_long".into())));
                        }
                    } else {
                        self.nibble = Some(clear);
                    }
                    Ok(None)
                }
                Group::EsdHeartbeat if self.nibble.is_none() => {
                    self.state = PcsRxState::End;
                    Ok(None)
                }
                _ => {
                    self.reset_frame();
                    Ok(Some(PcsRxEffect::Abort("10base_t1s_bad_esd".into())))
                }
            },
            PcsRxState::End => {
                if group == Group::EsdOkBurst && self.bytes.len() >= 64 {
                    let bytes = std::mem::take(&mut self.bytes);
                    self.reset_frame();
                    Ok(Some(PcsRxEffect::Frame(bytes)))
                } else {
                    self.reset_frame();
                    Ok(Some(PcsRxEffect::Abort("10base_t1s_bad_esd".into())))
                }
            }
        }
    }
}

/// A control reference labels only chips already recovered from this receiver's
/// ADC windows. It is private to the sampled acquisition path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceivedControlReference {
    pub epoch: u64,
    pub phase: usize,
    pub first_ordinal: u64,
    pub sample_indices: Vec<u64>,
    pub groups: Vec<u8>,
    pub initial_level: i8,
    pub final_level: i8,
    pub chips: Vec<i8>,
}

/// Reconstruct a bounded reference from actual, phase-tagged received chips.
/// A zero breaks continuity; no samples are removed from a candidate stream.
/// Only the initial control pair, its continuation and two SSDs are labels.
pub(crate) fn received_control_reference(
    epoch: u64,
    phase: usize,
    chips: &[(u64, u64, i8)],
) -> Option<ReceivedControlReference> {
    for first in 0..chips.len().saturating_sub(19) {
        let prefix = &chips[first..first + 20];
        if prefix.iter().any(|(_, _, chip)| ![-1, 1].contains(chip)) {
            continue;
        }
        let levels = prefix.iter().map(|(_, _, chip)| *chip).collect::<Vec<_>>();
        let prior = -levels[0];
        let Ok((a, level)) = decode_aligned_chips(&levels[..10], prior) else {
            continue;
        };
        let Ok((b, mut level)) = decode_aligned_chips(&levels[10..], level) else {
            continue;
        };
        if a != b || ![SYNC_COMMIT, BEACON].contains(&a) {
            continue;
        }
        let mut groups = vec![a, b];
        let mut count = 20;
        let mut ssd = 0;
        while first + count + 10 <= chips.len() && count < 40 {
            let next = chips[first + count..first + count + 10]
                .iter()
                .map(|(_, _, c)| *c)
                .collect::<Vec<_>>();
            let Ok((group, end)) = decode_aligned_chips(&next, level) else {
                break;
            };
            let allowed = if a == BEACON {
                group == BEACON
            } else if ssd == 0 && group == SYNC_COMMIT {
                true
            } else if ssd < 2 && group == SSD {
                ssd += 1;
                true
            } else {
                false
            };
            if !allowed {
                break;
            }
            groups.push(group);
            count += 10;
            level = end;
        }
        let selected = &chips[first..first + count];
        return Some(ReceivedControlReference {
            epoch,
            phase,
            first_ordinal: selected[0].0,
            sample_indices: selected.iter().map(|(_, index, _)| *index).collect(),
            groups,
            initial_level: prior,
            final_level: level,
            chips: selected.iter().map(|(_, _, chip)| *chip).collect(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_table_group_and_invalid_codes() {
        for nibble in 0..16 {
            let code = encode_group(Group::Data(nibble)).unwrap();
            assert_eq!(decode_group(code).unwrap(), Group::Data(nibble));
        }
        for code in [
            SILENCE,
            SYNC_COMMIT,
            ESD_ERROR,
            ESD_HEARTBEAT,
            ESD_OK_BURST,
            SSD,
            BEACON,
            ESD_JABBER,
        ] {
            assert_eq!(encode_group(decode_group(code).unwrap()).unwrap(), code);
        }
        assert_eq!(
            (0..32).filter(|&code| decode_group(code).is_ok()).count(),
            24
        );
    }
    #[test]
    fn seed_alignment_and_error_propagation() {
        let mut tx = Scrambler::new(0x15555).unwrap();
        let mut rx = Scrambler::new(0x15555).unwrap();
        for i in 0..256 {
            let value = ((i * 13 + 7) & 15) as u8;
            assert_eq!(
                rx.descramble_nibble(tx.scramble_nibble(value).unwrap())
                    .unwrap(),
                value
            );
        }
        assert!(Scrambler::new(0).is_err());
        assert!(Scrambler::new(0x20000).is_err());
    }
    #[test]
    fn dme_bits_and_silence_release() {
        let mut tx = DmeTransmitter::default();
        let chips = tx.group(SSD).unwrap();
        assert_eq!(chips.len(), 10);
        assert_eq!(decode_aligned_chips(&chips, -1).unwrap().0, SSD);
        let idle = tx.group(SILENCE).unwrap();
        assert_eq!(idle.len(), 3);
        assert_eq!(idle[2], 0);
        assert_eq!(tx.group(SILENCE).unwrap(), vec![0]);
    }
    #[test]
    fn pcs_real_delimiters_and_self_sync() {
        let mut tx = Scrambler::new(0x17a21).unwrap();
        let mut rx = PcsReceiver::new();
        let bytes = vec![0x5a; 64];
        let groups = encode_mac_groups(&bytes, &mut tx).unwrap();
        assert_eq!(&groups[..4], &[SYNC_COMMIT, SYNC_COMMIT, SSD, SSD]);
        let mut received = None;
        for group in groups {
            if let Some(PcsRxEffect::Frame(frame)) = rx.push(group).unwrap() {
                received = Some(frame);
            }
        }
        assert_eq!(received, Some(bytes));
        assert!(!rx.active_frame());
        let next_bytes = vec![0xa6; 64];
        let mut next_received = None;
        for group in encode_mac_groups(&next_bytes, &mut tx).unwrap() {
            if let Some(PcsRxEffect::Frame(frame)) = rx.push(group).unwrap() {
                next_received = Some(frame);
            }
        }
        assert_eq!(next_received, Some(next_bytes));
        assert_eq!(rx.push(BEACON).unwrap(), None);
        assert_eq!(rx.push(BEACON).unwrap(), Some(PcsRxEffect::Beacon));
    }
    #[test]
    fn dme_acquires_received_control_boundary() {
        let mut tx = DmeTransmitter::default();
        let mut rx = DmeReceiver::default();
        let mut seen = vec![];
        for group in [SYNC_COMMIT, SYNC_COMMIT, SSD, SSD, DATA_GROUPS[5]] {
            for chip in tx.group(group).unwrap() {
                seen.extend(rx.push_chip(chip).unwrap());
            }
        }
        assert_eq!(seen, [SYNC_COMMIT, SYNC_COMMIT, SSD, SSD, DATA_GROUPS[5]]);
        rx.carrier_lost();
        let mut beacon_tx = DmeTransmitter::default();
        let mut beacons = vec![];
        for group in [BEACON, BEACON] {
            for chip in beacon_tx.group(group).unwrap() {
                beacons.extend(rx.push_chip(chip).unwrap());
            }
        }
        assert_eq!(beacons, [BEACON, BEACON]);
    }
}
