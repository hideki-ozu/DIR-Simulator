//! Clause 97.4.2.4 twelve-octet PMA training InfoField.
//!
//! The wire sends octets in order and each octet least-significant bit first.
//! The caller XORs the resulting 96 bits with bits 2520..2615 of the 15th
//! PAM2 partial PHY frame; the format alone does not advance PHY Control.
use super::super::Result;
use super::an_hsm::crc16;
use super::t1_gigabit::Role;
use serde::{Deserialize, Serialize};

pub const INFO_OCTETS: usize = 12;
pub const INFO_BITS: usize = INFO_OCTETS * 8;
pub const PARTIAL_FRAMES_PER_PHY_FRAME: u32 = 15;
pub const INITIAL_PFC24: u32 = 14;
const SFD: [u8; 3] = [0xbb, 0xa7, 0x00];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PmaState {
    Training,
    Countdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfoField {
    pub role: Role,
    pub pfc24: u32,
    pub state: PmaState,
    pub local_receiver_ok: bool,
    /// MASTER: en_slave_tx; SLAVE: timing_lock_OK.
    pub enable_or_timing_lock: bool,
    /// Sent in TRAINING; S14 first. Must be nonzero.
    pub data_scrambler_seed: u16,
    /// Received optional capability bits are retained even though this
    /// bounded transmitter advertises both as disabled.
    pub eee_capable: bool,
    pub oam_capable: bool,
    pub user_config: u8,
    /// Sent in COUNTDOWN; multiple of 15 partial PHY frames.
    pub data_switch_pfc24: u32,
}
impl InfoField {
    fn validate(self) -> Result<()> {
        if self.pfc24 > 0x00ff_ffff
            || self.state == PmaState::Training
                && (self.data_scrambler_seed == 0 || self.data_scrambler_seed > 0x7fff)
            || self.data_switch_pfc24 > 0x00ff_ffff
            || self.user_config > 0x7f
            || self.state == PmaState::Countdown
                && (self.data_switch_pfc24 % PARTIAL_FRAMES_PER_PHY_FRAME != 0
                    || self.data_switch_pfc24 <= self.pfc24
                    || !self.local_receiver_ok
                    || !self.enable_or_timing_lock)
            || self.local_receiver_ok && !self.enable_or_timing_lock
        {
            return Err("invalid Clause 97 InfoField state, seed or PFC24".into());
        }
        Ok(())
    }
    pub fn to_bytes(self) -> Result<[u8; INFO_OCTETS]> {
        self.validate()?;
        if self.eee_capable || self.oam_capable || self.user_config != 0 {
            return Err(
                "bounded 1000BASE-T1 transmitter does not advertise EEE/OAM/user bits".into(),
            );
        }
        let mut octets = [0u8; INFO_OCTETS];
        octets[..3].copy_from_slice(&SFD);
        octets[3..6].copy_from_slice(&self.pfc24.to_le_bytes()[..3]);
        octets[6] = (if self.state == PmaState::Countdown {
            0x40
        } else {
            0
        }) | if self.local_receiver_ok { 0x20 } else { 0 }
            | if self.enable_or_timing_lock { 0x10 } else { 0 };
        if self.state == PmaState::Training {
            for bit in 0..8 {
                octets[7] |= (((self.data_scrambler_seed >> (14 - bit)) & 1) as u8) << bit;
            }
            for bit in 0..7 {
                octets[8] |= (((self.data_scrambler_seed >> (6 - bit)) & 1) as u8) << bit;
            }
            // EEEen, OAMen and user-configurable bits are unsupported and
            // accurately advertised as zero in this bounded PHY profile.
        } else {
            octets[7..10].copy_from_slice(&self.data_switch_pfc24.to_le_bytes()[..3]);
        }
        let bits = octets[3..10]
            .iter()
            .flat_map(|octet| (0..8).map(move |bit| (octet >> bit) & 1))
            .collect::<Vec<_>>();
        let crc = crc16(&bits)?;
        // Figure 97-23 emits S15 first, while each octet is sent LSB first.
        octets[10] = ((crc >> 8) as u8).reverse_bits();
        octets[11] = (crc as u8).reverse_bits();
        Ok(octets)
    }
    pub fn to_wire_bits(self) -> Result<[u8; INFO_BITS]> {
        let octets = self.to_bytes()?;
        Ok(std::array::from_fn(|n| (octets[n / 8] >> (n % 8)) & 1))
    }
    pub fn from_wire_bits(bits: &[u8; INFO_BITS], role: Role) -> Result<Self> {
        if bits.iter().any(|&bit| bit > 1) {
            return Err("Clause 97 InfoField wire bit out of range".into());
        }
        let mut octets = [0u8; INFO_OCTETS];
        for (n, &bit) in bits.iter().enumerate() {
            octets[n / 8] |= bit << (n % 8);
        }
        Self::from_bytes(&octets, role)
    }
    pub fn from_bytes(octets: &[u8; INFO_OCTETS], role: Role) -> Result<Self> {
        if octets[..3] != SFD {
            return Err("invalid Clause 97 InfoField SFD".into());
        }
        let bits = octets[3..10]
            .iter()
            .flat_map(|octet| (0..8).map(move |bit| (octet >> bit) & 1))
            .collect::<Vec<_>>();
        let crc = crc16(&bits)?;
        if octets[10] != ((crc >> 8) as u8).reverse_bits()
            || octets[11] != (crc as u8).reverse_bits()
        {
            return Err("Clause 97 InfoField CRC16 failed".into());
        }
        let state = match octets[6] >> 6 {
            0 => PmaState::Training,
            1 => PmaState::Countdown,
            _ => return Err("unsupported Clause 97 InfoField PMA state".into()),
        };
        if octets[6] & 0x0f != 0 {
            return Err("nonzero reserved Clause 97 InfoField message bits".into());
        }
        let mut seed = 0u16;
        for bit in 0..8 {
            seed |= (((octets[7] >> bit) & 1) as u16) << (14 - bit);
        }
        for bit in 0..7 {
            seed |= (((octets[8] >> bit) & 1) as u16) << (6 - bit);
        }
        let result = Self {
            role,
            pfc24: u32::from_le_bytes([octets[3], octets[4], octets[5], 0]),
            state,
            local_receiver_ok: octets[6] & 0x20 != 0,
            enable_or_timing_lock: octets[6] & 0x10 != 0,
            data_scrambler_seed: if state == PmaState::Training { seed } else { 1 },
            eee_capable: state == PmaState::Training && octets[8] & 0x80 != 0,
            oam_capable: state == PmaState::Training && octets[9] & 1 != 0,
            user_config: if state == PmaState::Training {
                octets[9] >> 1
            } else {
                0
            },
            data_switch_pfc24: if state == PmaState::Countdown {
                u32::from_le_bytes([octets[7], octets[8], octets[9], 0])
            } else {
                0
            },
        };
        result.validate()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn training_and_countdown_infofields_round_trip_with_crc() {
        let mut info = InfoField {
            role: Role::Master,
            pfc24: INITIAL_PFC24,
            state: PmaState::Training,
            local_receiver_ok: false,
            enable_or_timing_lock: false,
            data_scrambler_seed: 0x4e35,
            eee_capable: false,
            oam_capable: false,
            user_config: 0,
            data_switch_pfc24: 0,
        };
        let wire = info.to_bytes().unwrap();
        assert_eq!(&wire[..3], &SFD);
        assert_eq!(&wire[3..6], &[14, 0, 0]);
        assert_eq!(InfoField::from_bytes(&wire, Role::Master).unwrap(), info);
        assert_eq!(
            InfoField::from_wire_bits(&info.to_wire_bits().unwrap(), Role::Master).unwrap(),
            info
        );
        let mut remote = wire;
        remote[8] |= 0x80; // Partner can advertise EEE even when local cannot.
        remote[9] |= 0x01; // Partner can advertise OAM independently.
        let bits = remote[3..10]
            .iter()
            .flat_map(|octet| (0..8).map(move |bit| (octet >> bit) & 1))
            .collect::<Vec<_>>();
        let crc = crc16(&bits).unwrap();
        remote[10] = ((crc >> 8) as u8).reverse_bits();
        remote[11] = (crc as u8).reverse_bits();
        let partner = InfoField::from_bytes(&remote, Role::Slave).unwrap();
        assert!(partner.eee_capable && partner.oam_capable);
        assert!(partner.to_bytes().is_err());
        let mut corrupted = wire;
        corrupted[8] ^= 1;
        assert_eq!(
            InfoField::from_bytes(&corrupted, Role::Master).unwrap_err(),
            "Clause 97 InfoField CRC16 failed"
        );
        info.pfc24 = 899;
        info.state = PmaState::Countdown;
        info.local_receiver_ok = true;
        info.enable_or_timing_lock = true;
        info.data_switch_pfc24 = 915;
        let wire = info.to_bytes().unwrap();
        let decoded = InfoField::from_bytes(&wire, Role::Master).unwrap();
        assert_eq!(decoded.pfc24, 899);
        assert_eq!(decoded.state, PmaState::Countdown);
        assert_eq!(decoded.data_switch_pfc24, 915);
    }
}
