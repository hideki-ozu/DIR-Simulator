//! Clause 98 high-speed-mode BASE-T1 Auto-Negotiation page coding.
//!
//! This is the on-wire DME page and base-link-codeword layer. Arbitration,
//! half-duplex page scheduling, and PMA handoff must use decoded pages from
//! the sampled receive path; constructing a page does not establish a link.
use super::super::Result;
use super::t1_gigabit::Role;
use serde::{Deserialize, Serialize};

pub const PAGE_BITS: usize = 48;
pub const CRC_BITS: usize = 16;
pub const DME_POSITIONS: usize = 157;
/// At a 750 MBd PMA with 32 samples per symbol, one 30 ns HSM position is
/// exactly 22.5 PMA symbols, or 720 numerical samples.
pub const SAMPLES_PER_POSITION_750M_32: usize = 720;
const START_DELIMITER_POSITIVE: [i8; 26] = [
    1, -1, 1, 1, -1, -1, 1, -1, -1, -1, -1, 1, -1, 1, -1, -1, -1, -1, 1, 1, -1, -1, -1, 1, -1, 1,
];

fn check_binary(bits: &[u8]) -> Result<()> {
    if bits.iter().any(|&bit| bit > 1) {
        return Err("Clause 98 page requires binary data bits".into());
    }
    Ok(())
}

/// Figure 98-3, register S15..S0 initially zero. The polynomial is
/// x^16+x^15+x^2+1; D0 enters first and S15 is emitted first.
pub fn crc16(bits: &[u8]) -> Result<u16> {
    check_binary(bits)?;
    let mut state = 0u16;
    for &bit in bits {
        let feedback = ((state >> 15) as u8) ^ bit;
        state <<= 1;
        if feedback != 0 {
            state ^= 0x8005;
        }
    }
    Ok(state)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BasePage {
    /// D5..D9, echoed nonce from the received page; zero before valid CRC.
    pub echoed_nonce: u8,
    /// D16..D20, including T4 MASTER preference.
    pub transmitted_nonce: u8,
    /// D10..D11, PAUSE and ASM_DIR.
    pub pause: u8,
    /// D12, forced rather than preferred MASTER/SLAVE selection.
    pub force_master_slave: bool,
    /// D13, remote fault.
    pub remote_fault: bool,
    /// D14, acknowledgement of a valid partner page.
    pub acknowledge: bool,
    /// D15. A receiver must process Next Pages before declaring completion.
    pub next_page: bool,
    /// A0..A26, transmitted in D21..D47. A2 advertises 1000BASE-T1.
    pub technology_abilities: u32,
}
impl BasePage {
    pub const ABILITY_1000BASE_T1: u32 = 1 << 2;
    pub fn new_1000base_t1(transmitted_nonce: u8, force_master_slave: bool) -> Self {
        Self {
            echoed_nonce: 0,
            transmitted_nonce,
            pause: 0,
            force_master_slave,
            remote_fault: false,
            acknowledge: false,
            next_page: false,
            technology_abilities: Self::ABILITY_1000BASE_T1,
        }
    }
    fn validate(&self) -> Result<()> {
        if self.echoed_nonce > 31
            || self.transmitted_nonce > 31
            || self.pause > 3
            || self.technology_abilities >= (1 << 27)
        {
            return Err("Clause 98 base page field out of range".into());
        }
        Ok(())
    }
    pub fn to_bits(self) -> Result<[u8; PAGE_BITS]> {
        self.validate()?;
        let mut bits = [0u8; PAGE_BITS];
        bits[0] = 1; // IEEE Std 802.3 selector S[4:0] = 00001.
        for (start, width, value) in [
            (5, 5, self.echoed_nonce as u32),
            (10, 2, self.pause as u32),
            (16, 5, self.transmitted_nonce as u32),
            (21, 27, self.technology_abilities),
        ] {
            for bit in 0..width {
                bits[start + bit] = ((value >> bit) & 1) as u8;
            }
        }
        bits[12] = u8::from(self.force_master_slave);
        bits[13] = u8::from(self.remote_fault);
        bits[14] = u8::from(self.acknowledge);
        bits[15] = u8::from(self.next_page);
        Ok(bits)
    }
    pub fn from_bits(bits: &[u8; PAGE_BITS]) -> Result<Self> {
        check_binary(bits)?;
        if bits[..5] != [1, 0, 0, 0, 0] {
            return Err("unsupported Clause 98 selector field".into());
        }
        let field = |start: usize, width: usize| -> u32 {
            (0..width).fold(0, |value, bit| value | ((bits[start + bit] as u32) << bit))
        };
        let result = Self {
            echoed_nonce: field(5, 5) as u8,
            pause: field(10, 2) as u8,
            force_master_slave: bits[12] != 0,
            remote_fault: bits[13] != 0,
            acknowledge: bits[14] != 0,
            next_page: bits[15] != 0,
            transmitted_nonce: field(16, 5) as u8,
            technology_abilities: field(21, 27),
        };
        result.validate()?;
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    LocalRole(Role),
    RegenerateNonce,
    ConfigurationFault,
    NoCommonMode,
    RemoteFault,
    NextPageRequired,
}
/// Table 98-4 role resolution, restricted to the implemented A2 data service.
/// A resolved role is not AN completion: arbitration still needs the received
/// acknowledgement and any requested Next Page exchange before PMA handoff.
pub fn resolve_1000base_t1(local: BasePage, remote: BasePage) -> Resolution {
    if local.next_page || remote.next_page {
        return Resolution::NextPageRequired;
    }
    if remote.remote_fault {
        return Resolution::RemoteFault;
    }
    if local.technology_abilities & remote.technology_abilities & BasePage::ABILITY_1000BASE_T1 == 0
    {
        return Resolution::NoCommonMode;
    }
    let local_pref_master = local.transmitted_nonce & 0x10 != 0;
    let remote_pref_master = remote.transmitted_nonce & 0x10 != 0;
    let local_master = match (local.force_master_slave, remote.force_master_slave) {
        (true, true) if local_pref_master == remote_pref_master => {
            return Resolution::ConfigurationFault;
        }
        (true, _) => local_pref_master,
        (_, true) => !remote_pref_master,
        (false, false) => {
            if local.transmitted_nonce == remote.transmitted_nonce {
                return Resolution::RegenerateNonce;
            }
            local.transmitted_nonce > remote.transmitted_nonce
        }
    };
    Resolution::LocalRole(if local_master {
        Role::Master
    } else {
        Role::Slave
    })
}

/// Voltage levels at 157 ideal transition positions spaced by 30 ns.
/// The first position begins with the mandatory quiet-to-active transition;
/// position 157 returns to high impedance at 156*T1 = 4680 ns. `positive_start` is the deterministic
/// externally supplied polarity choice for this particular page.
pub fn encode_page(bits: &[u8; PAGE_BITS], positive_start: bool) -> Result<[i8; DME_POSITIONS]> {
    check_binary(bits)?;
    let mut levels = [0i8; DME_POSITIONS];
    let sign = if positive_start { 1 } else { -1 };
    for (dst, value) in levels[..26].iter_mut().zip(START_DELIMITER_POSITIVE) {
        *dst = value * sign;
    }
    let crc = crc16(bits)?;
    let mut payload = [0u8; PAGE_BITS + CRC_BITS];
    payload[..PAGE_BITS].copy_from_slice(bits);
    for i in 0..CRC_BITS {
        payload[PAGE_BITS + i] = ((crc >> (15 - i)) & 1) as u8;
    }
    let mut level = levels[25];
    for (index, bit) in payload.iter().enumerate() {
        // Positions 27,29,...153 carry the obligatory clock transition.
        level = -level;
        levels[26 + 2 * index] = level;
        // Positions 28,30,...154 carry a transition for logical one.
        if *bit == 1 {
            level = -level;
        }
        levels[27 + 2 * index] = level;
    }
    levels[154] = -level; // Position 155 end delimiter transition.
    levels[155] = levels[154]; // Position 156 has no transition.
    levels[156] = 0; // Position 157 returns to electrical quiet.
    Ok(levels)
}

pub fn decode_page(levels: &[i8; DME_POSITIONS]) -> Result<[u8; PAGE_BITS]> {
    let sign = levels[0];
    if ![-1, 1].contains(&sign)
        || levels[..26]
            .iter()
            .zip(START_DELIMITER_POSITIVE)
            .any(|(&got, expected)| got != expected * sign)
        || levels[154] != -levels[153]
        || levels[155] != levels[154]
        || levels[156] != 0
    {
        return Err("invalid Clause 98 HSM DME delimiter".into());
    }
    let mut payload = [0u8; PAGE_BITS + CRC_BITS];
    let mut prior = levels[25];
    for (index, bit) in payload.iter_mut().enumerate() {
        let clock = levels[26 + 2 * index];
        let data = levels[27 + 2 * index];
        if clock != -prior || ![-1, 1].contains(&data) {
            return Err("invalid Clause 98 HSM DME clock transition".into());
        }
        *bit = u8::from(data != clock);
        prior = data;
    }
    let mut bits = [0u8; PAGE_BITS];
    bits.copy_from_slice(&payload[..PAGE_BITS]);
    let actual = crc16(&bits)?;
    for i in 0..CRC_BITS {
        if payload[PAGE_BITS + i] != ((actual >> (15 - i)) & 1) as u8 {
            return Err("Clause 98 HSM DME page CRC16 failed".into());
        }
    }
    Ok(bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hsm_page_has_exact_delimiters_positions_and_crc() {
        let base = BasePage::new_1000base_t1(0x13, true);
        let bits = base.to_bits().unwrap();
        assert_eq!(bits[23], 1); // A2 is D23.
        for positive in [false, true] {
            let levels = encode_page(&bits, positive).unwrap();
            assert_eq!(levels[0], if positive { 1 } else { -1 });
            assert_eq!(levels[156], 0);
            assert_eq!(decode_page(&levels).unwrap(), bits);
            let mut changed = levels;
            for level in &mut changed[27..156] {
                *level = -*level;
            }
            assert_eq!(
                decode_page(&changed).unwrap_err(),
                "Clause 98 HSM DME page CRC16 failed"
            );
        }
    }
    #[test]
    fn base_page_roles_conflicts_and_no_common_mode() {
        let master = BasePage::new_1000base_t1(0x13, true);
        let slave = BasePage::new_1000base_t1(0x02, true);
        assert_eq!(
            resolve_1000base_t1(master, slave),
            Resolution::LocalRole(Role::Master)
        );
        assert_eq!(
            resolve_1000base_t1(master, BasePage::new_1000base_t1(0x1a, true)),
            Resolution::ConfigurationFault
        );
        let mut unsupported = slave;
        unsupported.technology_abilities = 1 << 0;
        assert_eq!(
            resolve_1000base_t1(master, unsupported),
            Resolution::NoCommonMode
        );
        let mut preferred = master;
        preferred.force_master_slave = false;
        assert_eq!(
            resolve_1000base_t1(preferred, preferred),
            Resolution::RegenerateNonce
        );
        let mut next = slave;
        next.next_page = true;
        assert_eq!(
            resolve_1000base_t1(master, next),
            Resolution::NextPageRequired
        );
    }
}
