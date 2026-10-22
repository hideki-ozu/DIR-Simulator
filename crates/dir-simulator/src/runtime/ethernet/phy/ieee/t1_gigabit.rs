//! Clause 97.3.2.2.12/.13 side-stream scrambling and 3B2T PAM3 mapping.
//! These are continuous PCS components; the caller retains them across MAC
//! frame and 45-block PHY-frame boundaries.
use super::super::Result;
use super::rs9;
use serde::{Deserialize, Serialize};

pub const PAM3_SYMBOLS_PER_PHY_FRAME: usize = 2700;
pub const BITS_PER_PHY_FRAME: usize = 4050;
pub const INFO_BITS_PER_PHY_FRAME: usize = 3654;
pub const BLOCK_BITS: usize = 81;
pub const BLOCKS_PER_PHY_FRAME: usize = 45;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Master,
    Slave,
}

/// Figure 97-9 circuit: output S14, feedback S14 XOR S3 (MASTER) or
/// S14 XOR S10 (SLAVE), feedback inserted at S0. Seeds are 15 nonzero bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideScrambler {
    role: Role,
    state: u16,
}
impl SideScrambler {
    pub fn new(role: Role, seed: u16) -> Result<Self> {
        if seed == 0 || seed > 0x7fff {
            return Err("1000BASE-T1 scrambler seed must have 15 nonzero bits".into());
        }
        Ok(Self { role, state: seed })
    }
    pub fn state(self) -> u16 {
        self.state
    }
    pub fn next_mask_bit(&mut self) -> u8 {
        let output = ((self.state >> 14) & 1) as u8;
        let other_tap = match self.role {
            Role::Master => 3,
            Role::Slave => 10,
        };
        let feedback = output ^ ((self.state >> other_tap) & 1) as u8;
        self.state = ((self.state << 1) | feedback as u16) & 0x7fff;
        output
    }
    pub fn apply_bit(&mut self, bit: u8) -> Result<u8> {
        if bit > 1 {
            return Err("1000BASE-T1 PCS input is not one bit".into());
        }
        Ok(bit ^ self.next_mask_bit())
    }
    pub fn apply_bits(&mut self, bits: &mut [u8]) -> Result<()> {
        for bit in bits {
            *bit = self.apply_bit(*bit)?;
        }
        Ok(())
    }
}

/// Table 97-2. Tuple order is wire order T[0], T[1].
pub const B3T2: [(i8, i8); 8] = [
    (-1, -1),
    (-1, 0),
    (0, -1),
    (1, -1),
    (0, 1),
    (-1, 1),
    (1, 1),
    (1, 0),
];
pub fn map_3b2t(bits_lsb: [u8; 3]) -> Result<[i8; 2]> {
    if bits_lsb.iter().any(|&bit| bit > 1) {
        return Err("3B2T requires three binary bits".into());
    }
    let code = bits_lsb[0] | (bits_lsb[1] << 1) | (bits_lsb[2] << 2);
    let (a, b) = B3T2[code as usize];
    Ok([a, b])
}
pub fn demap_3b2t(ternary: [i8; 2]) -> Result<[u8; 3]> {
    let code = B3T2
        .iter()
        .position(|&(a, b)| [a, b] == ternary)
        .ok_or("invalid 1000BASE-T1 PAM3 symbol pair")? as u8;
    Ok([code & 1, (code >> 1) & 1, (code >> 2) & 1])
}
pub fn map_phy_frame(bits: &[u8]) -> Result<Vec<i8>> {
    if bits.len() != BITS_PER_PHY_FRAME {
        return Err("1000BASE-T1 requires 4050 coded bits per PHY frame".into());
    }
    let mut symbols = Vec::with_capacity(PAM3_SYMBOLS_PER_PHY_FRAME);
    for group in bits.chunks_exact(3) {
        symbols.extend(map_3b2t([group[0], group[1], group[2]])?);
    }
    Ok(symbols)
}
pub fn demap_phy_frame(symbols: &[i8]) -> Result<Vec<u8>> {
    if symbols.len() != PAM3_SYMBOLS_PER_PHY_FRAME {
        return Err("1000BASE-T1 requires 2700 PAM3 symbols per PHY frame".into());
    }
    let mut bits = Vec::with_capacity(BITS_PER_PHY_FRAME);
    for pair in symbols.chunks_exact(2) {
        bits.extend(demap_3b2t([pair[0], pair[1]])?);
    }
    Ok(bits)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    InterFrameNotReady,
    InterFrameReady,
    Error,
}
impl Control {
    fn bits(self) -> [u8; 3] {
        match self {
            Self::InterFrameNotReady => [0, 0, 0],
            Self::InterFrameReady => [0, 1, 0],
            Self::Error => [0, 0, 1],
        }
    }
    fn from_bits(bits: [u8; 3]) -> Result<Self> {
        match bits {
            [0, 0, 0] => Ok(Self::InterFrameNotReady),
            [0, 1, 0] => Ok(Self::InterFrameReady),
            [0, 0, 1] => Ok(Self::Error),
            _ => Err("unsupported 1000BASE-T1 control code".into()),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gmii {
    Data(u8),
    Control(Control),
}

#[derive(Clone, Copy)]
enum Source {
    Constant(u8),
    OctetBit(usize, usize),
}
fn layout(control: &[bool; 10]) -> [Source; BLOCK_BITS] {
    let mut result = [Source::Constant(0); BLOCK_BITS];
    result[0] = Source::Constant(u8::from(control.iter().any(|&c| c)));
    for n in 0..10 {
        let any_remaining = control[n..].iter().any(|&c| c);
        let prior_control = n == 0 || control[n - 1];
        let next = control[n..].iter().position(|&c| c).unwrap_or(0);
        let count = control[n..].iter().filter(|&&c| c).count();
        for bit in 0..5 {
            result[8 * n + 1 + bit] = if !any_remaining {
                Source::OctetBit(n, bit)
            } else if prior_control {
                Source::Constant(if bit < 4 {
                    ((next >> bit) & 1) as u8
                } else {
                    u8::from(count != 1)
                })
            } else {
                Source::OctetBit(n - 1, bit + 3)
            };
        }
        for bit in 0..3 {
            result[8 * n + 6 + bit] = Source::OctetBit(
                n,
                if !any_remaining || control[n] {
                    bit + 5
                } else {
                    bit
                },
            );
        }
    }
    result
}
fn td(octets: &[Gmii; 10]) -> [[u8; 8]; 10] {
    let mut result = [[0u8; 8]; 10];
    for (n, octet) in octets.iter().enumerate() {
        match octet {
            Gmii::Data(byte) => {
                for bit in 0..8 {
                    result[n][bit] = (byte >> bit) & 1;
                }
            }
            Gmii::Control(code) => {
                result[n][5..8].copy_from_slice(&code.bits());
            }
        }
    }
    result
}
/// Exact Clause 97.3.2.2.5 pseudo-code, B[0] first. Optional EEE LPI is
/// deliberately unavailable; caller must advertise that exclusion.
pub fn encode_80b81b(octets: &[Gmii; 10]) -> [u8; BLOCK_BITS] {
    let control = std::array::from_fn(|n| matches!(octets[n], Gmii::Control(_)));
    let source = layout(&control);
    let values = td(octets);
    std::array::from_fn(|bit| match source[bit] {
        Source::Constant(value) => value,
        Source::OctetBit(n, index) => values[n][index],
    })
}
/// First PAM3 symbol boundary after every mapped bit of a data octet in a
/// single 80B/81B block.  Control placement can move the final data bits into
/// a following bit position, so a uniform six-symbols-per-octet estimate is
/// not a physical timestamp.
pub fn data_octet_end_symbol(octets: &[Gmii; 10], octet: usize) -> Result<usize> {
    if octet >= octets.len() || !matches!(octets[octet], Gmii::Data(_)) {
        return Err("1000BASE-T1 timestamp requires a data octet".into());
    }
    let control = std::array::from_fn(|n| matches!(octets[n], Gmii::Control(_)));
    let last_bit = layout(&control)
        .iter()
        .enumerate()
        .filter_map(|(position, source)| match source {
            Source::OctetBit(n, _) if *n == octet => Some(position),
            _ => None,
        })
        .max()
        .ok_or("1000BASE-T1 data octet has no mapped bit")?;
    Ok((last_bit + 1).div_ceil(3) * 2)
}
/// Inverts the Clause 97 NEXT fields to locate control octets, then applies
/// the source layout to recover data and control bits.
pub fn decode_80b81b(bits: &[u8]) -> Result<[Gmii; 10]> {
    if bits.len() != BLOCK_BITS || bits.iter().any(|&bit| bit > 1) {
        return Err("80B/81B requires 81 binary bits".into());
    }
    let mut control = [false; 10];
    if bits[0] == 1 {
        let mut next_field_at = 0;
        loop {
            let next = (0..4).fold(0usize, |value, bit| {
                value | ((bits[8 * next_field_at + 1 + bit] as usize) << bit)
            });
            let position = next_field_at + next;
            if position >= 10 || control[position] {
                return Err("invalid 1000BASE-T1 NEXT control position".into());
            }
            control[position] = true;
            let more = bits[8 * next_field_at + 5] == 1;
            if !more {
                break;
            }
            next_field_at = position + 1;
            if next_field_at >= 10 {
                return Err("invalid 1000BASE-T1 NEXT continuation".into());
            }
        }
    }
    let source = layout(&control);
    let mut values = [[None; 8]; 10];
    for (position, entry) in source.iter().enumerate() {
        match *entry {
            Source::Constant(value) if bits[position] != value => {
                return Err("invalid 1000BASE-T1 block constant".into());
            }
            Source::Constant(_) => {}
            Source::OctetBit(n, bit) => {
                if values[n][bit].is_some_and(|v| v != bits[position]) {
                    return Err("conflicting 1000BASE-T1 bit mapping".into());
                }
                values[n][bit] = Some(bits[position]);
            }
        }
    }
    let mut decoded = [Gmii::Data(0); 10];
    for n in 0..10 {
        if control[n] {
            let code_bits = values[n][5..8]
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()
                .ok_or("missing 1000BASE-T1 control bits")?;
            let code = Control::from_bits([code_bits[0], code_bits[1], code_bits[2]])?;
            decoded[n] = Gmii::Control(code);
        } else {
            let data_bits = values[n]
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()
                .ok_or("missing 1000BASE-T1 data bits")?;
            decoded[n] = Gmii::Data(
                data_bits
                    .iter()
                    .enumerate()
                    .fold(0, |byte, (bit, &value)| byte | (value << bit)),
            );
        }
    }
    if encode_80b81b(&decoded) != bits {
        return Err("invalid 1000BASE-T1 80B/81B block".into());
    }
    Ok(decoded)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhyFrame {
    pub blocks: [[u8; BLOCK_BITS]; BLOCKS_PER_PHY_FRAME],
    /// Mandatory Clause 97 OAM position, even when optional OAM is disabled.
    pub oam: u16,
    pub corrected_symbols: usize,
}
pub fn encode_phy_frame(
    blocks: &[[u8; BLOCK_BITS]; BLOCKS_PER_PHY_FRAME],
    oam: u16,
    scrambler: &mut SideScrambler,
) -> Result<Vec<i8>> {
    if oam > 0x1ff || blocks.iter().flatten().any(|&bit| bit > 1) {
        return Err("invalid 1000BASE-T1 block bit or OAM symbol".into());
    }
    let mut data = [0u16; rs9::DATA_SYMBOLS];
    for (index, &bit) in blocks.iter().flatten().enumerate() {
        data[index / 9] |= (bit as u16) << (index % 9);
    }
    data[rs9::DATA_SYMBOLS - 1] = oam;
    let coded = rs9::encode(&data)?;
    let mut bits = Vec::with_capacity(BITS_PER_PHY_FRAME);
    for symbol in coded {
        for bit in 0..9 {
            bits.push(((symbol >> bit) & 1) as u8);
        }
    }
    scrambler.apply_bits(&mut bits)?;
    map_phy_frame(&bits)
}
pub fn decode_phy_frame(symbols: &[i8], scrambler: &mut SideScrambler) -> Result<PhyFrame> {
    let mut bits = demap_phy_frame(symbols)?;
    scrambler.apply_bits(&mut bits)?;
    let mut coded = [0u16; rs9::CODE_SYMBOLS];
    for (index, &bit) in bits.iter().enumerate() {
        coded[index / 9] |= (bit as u16) << (index % 9);
    }
    let decoded = rs9::decode(&coded)?;
    let mut blocks = [[0u8; BLOCK_BITS]; BLOCKS_PER_PHY_FRAME];
    for (index, bit) in blocks.iter_mut().flatten().enumerate() {
        *bit = ((decoded.data[index / 9] >> (index % 9)) & 1) as u8;
    }
    Ok(PhyFrame {
        blocks,
        oam: decoded.data[rs9::DATA_SYMBOLS - 1],
        corrected_symbols: decoded.corrected_symbols,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping_matches_literal_clause_97_table() {
        assert_eq!(
            B3T2,
            [
                (-1, -1),
                (-1, 0),
                (0, -1),
                (1, -1),
                (0, 1),
                (-1, 1),
                (1, 1),
                (1, 0)
            ]
        );
        for code in 0..8 {
            let bits = [code & 1, (code >> 1) & 1, (code >> 2) & 1];
            let pair = map_3b2t(bits).unwrap();
            assert_eq!(demap_3b2t(pair).unwrap(), bits);
        }
        assert!(demap_3b2t([0, 0]).is_err());
    }
    #[test]
    fn continuous_scrambler_same_seed_recovers_frame_boundaries() {
        for role in [Role::Master, Role::Slave] {
            let mut tx = SideScrambler::new(role, 0x4101).unwrap();
            let mut rx = SideScrambler::new(role, 0x4101).unwrap();
            let mut first = (0..BITS_PER_PHY_FRAME)
                .map(|i| (i % 7 == 0) as u8)
                .collect::<Vec<_>>();
            let original = first.clone();
            tx.apply_bits(&mut first).unwrap();
            let pam = map_phy_frame(&first).unwrap();
            let mut recovered = demap_phy_frame(&pam).unwrap();
            rx.apply_bits(&mut recovered).unwrap();
            assert_eq!(recovered, original);
            assert_eq!(tx.state(), rx.state());
            assert_ne!(tx.state(), 0x4101);
        }
    }
    #[test]
    fn all_data_and_mixed_control_blocks() {
        let data = std::array::from_fn(|n| Gmii::Data((n * 17 + 3) as u8));
        let bits = encode_80b81b(&data);
        assert_eq!(bits[0], 0);
        assert_eq!(decode_80b81b(&bits).unwrap(), data);
        for pattern in 1..1024usize {
            let mixed = std::array::from_fn(|n| {
                if pattern & (1 << n) != 0 {
                    Gmii::Control(match n % 3 {
                        0 => Control::InterFrameNotReady,
                        1 => Control::InterFrameReady,
                        _ => Control::Error,
                    })
                } else {
                    Gmii::Data((n * 19 + 11) as u8)
                }
            });
            let encoded = encode_80b81b(&mixed);
            assert_eq!(decode_80b81b(&encoded).unwrap(), mixed, "pattern={pattern}");
        }
    }
    #[test]
    fn whole_phy_frame_rs_scrambler_pam3() {
        let blocks = std::array::from_fn(|block| {
            encode_80b81b(&std::array::from_fn(|n| Gmii::Data((block * 13 + n) as u8)))
        });
        let mut tx = SideScrambler::new(Role::Master, 0x1234).unwrap();
        let mut rx = tx;
        let pam3 = encode_phy_frame(&blocks, 0x102, &mut tx).unwrap();
        assert_eq!(pam3.len(), PAM3_SYMBOLS_PER_PHY_FRAME);
        let decoded = decode_phy_frame(&pam3, &mut rx).unwrap();
        assert_eq!(decoded.blocks, blocks);
        assert_eq!(decoded.oam, 0x102);
        assert_eq!(decoded.corrected_symbols, 0);
        assert_eq!(tx, rx);
    }
}
