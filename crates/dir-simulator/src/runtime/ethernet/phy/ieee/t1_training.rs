//! Clause 97.3.4 PAM2 training sequence and sampled-symbol acquisition.
//!
//! The 33-bit side stream is independent of the 15-bit data scrambler.  The
//! latter is initialized from the InfoField only at the PAM2/PAM3 switch.
use super::super::Result;
use super::t1_gigabit::Role;
use super::t1_info::{INFO_BITS, InfoField};
use serde::{Deserialize, Serialize};

pub const PARTIAL_SYMBOLS: u64 = 180;
pub const PARTIALS_PER_FRAME: u64 = 15;
pub const SYMBOLS_PER_FRAME: u64 = PARTIAL_SYMBOLS * PARTIALS_PER_FRAME;
const MASK: u64 = (1u64 << 33) - 1;

/// Figure 97-10: Scr[32] is emitted, Scr[32] XOR Scr[12] (MASTER) or
/// Scr[19] (SLAVE) is shifted into Scr[0].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingScrambler {
    role: Role,
    state: u64,
}
impl TrainingScrambler {
    pub fn new(role: Role, seed: u64) -> Result<Self> {
        if seed == 0 || seed > MASK {
            return Err("1000BASE-T1 PAM2 training seed must be nonzero 33-bit".into());
        }
        Ok(Self { role, state: seed })
    }
    pub fn state(self) -> u64 {
        self.state
    }
    pub fn next_bit(&mut self) -> u8 {
        let output = ((self.state >> 32) & 1) as u8;
        let other = match self.role {
            Role::Master => 12,
            Role::Slave => 19,
        };
        let feedback = output ^ ((self.state >> other) & 1) as u8;
        self.state = ((self.state << 1) | feedback as u64) & MASK;
        output
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingTransmitter {
    scrambler: TrainingScrambler,
    symbol: u64,
}
impl TrainingTransmitter {
    pub fn new(role: Role, seed: u64) -> Result<Self> {
        Self::new_aligned(role, seed, 0)
    }
    /// The SLAVE starts with the master's next partial PHY frame count after
    /// acquiring the MASTER stream.  Startup is permitted only at a PHY-frame
    /// boundary in this bounded reference profile.
    pub fn new_aligned(role: Role, seed: u64, first_partial: u32) -> Result<Self> {
        if first_partial % PARTIALS_PER_FRAME as u32 != 0 {
            return Err("PAM2 training alignment is not a PHY-frame boundary".into());
        }
        Ok(Self {
            scrambler: TrainingScrambler::new(role, seed)?,
            symbol: first_partial as u64 * PARTIAL_SYMBOLS,
        })
    }
    pub fn symbol(&self) -> u64 {
        self.symbol
    }
    /// Returns the next PAM2 MDI symbol, with an InfoField on every 15th
    /// partial PHY frame.  Callers choose the current legal InfoField state.
    pub fn next_symbol(&mut self, info: &InfoField) -> Result<i8> {
        let mut bit = self.scrambler.next_bit();
        let within = self.symbol % SYMBOLS_PER_FRAME;
        let partial = within / PARTIAL_SYMBOLS;
        let position = within % PARTIAL_SYMBOLS;
        if partial != 14 && position == 0 {
            bit ^= 1;
        }
        if partial == 14 && position < INFO_BITS as u64 {
            bit ^= info.to_wire_bits()?[position as usize];
        }
        self.symbol += 1;
        Ok(if bit == 0 { 1 } else { -1 })
    }
}

/// Acquires the unknown 33-bit partner seed from the first 33 recovered PAM2
/// symbols.  It then checks framing/sequence and extracts CRC-protected
/// InfoFields from the actual partner symbol stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainingReceiver {
    partner_role: Role,
    symbol: u64,
    captured_initial: u64,
    scrambler: Option<TrainingScrambler>,
    info_bits: Vec<u8>,
    consecutive_good: u64,
    complete_fields: u32,
    pfc_base: Option<u32>,
}
impl TrainingReceiver {
    pub fn new(partner_role: Role) -> Self {
        Self {
            partner_role,
            symbol: 0,
            captured_initial: 0,
            scrambler: None,
            info_bits: vec![0; INFO_BITS],
            consecutive_good: 0,
            complete_fields: 0,
            pfc_base: None,
        }
    }
    pub fn complete_fields(&self) -> u32 {
        self.complete_fields
    }
    pub fn symbols_received(&self) -> u64 {
        self.symbol
    }
    pub fn absolute_partial_count(&self) -> Option<u32> {
        self.pfc_base
            .map(|base| base + (self.symbol / PARTIAL_SYMBOLS) as u32)
    }
    pub fn consecutive_good(&self) -> u64 {
        self.consecutive_good
    }
    pub fn push_symbol(&mut self, level: i8) -> Result<Option<InfoField>> {
        if ![-1, 1].contains(&level) {
            return Err("PAM2 training level is not binary".into());
        }
        let transmitted = u8::from(level == -1);
        let within = self.symbol % SYMBOLS_PER_FRAME;
        let partial = within / PARTIAL_SYMBOLS;
        let position = within % PARTIAL_SYMBOLS;
        let is_info = partial == 14 && position < INFO_BITS as u64;
        let bit = transmitted ^ u8::from(partial != 14 && position == 0);
        if self.symbol < 33 {
            self.captured_initial |= (bit as u64) << (32 - self.symbol);
            if self.symbol == 32 {
                let mut scrambler =
                    TrainingScrambler::new(self.partner_role, self.captured_initial)?;
                for _ in 0..33 {
                    scrambler.next_bit();
                }
                self.scrambler = Some(scrambler);
            }
        } else {
            let predicted = self
                .scrambler
                .as_mut()
                .ok_or("PAM2 training acquisition state absent")?
                .next_bit();
            if is_info {
                self.info_bits[position as usize] = transmitted ^ predicted;
            } else if bit != predicted {
                self.consecutive_good = 0;
                return Err("PAM2 training side-stream mismatch".into());
            }
            self.consecutive_good += 1;
        }
        self.symbol += 1;
        if within == 14 * PARTIAL_SYMBOLS + INFO_BITS as u64 - 1 {
            let bits: [u8; INFO_BITS] = self
                .info_bits
                .as_slice()
                .try_into()
                .map_err(|_| "saved PAM2 InfoField length mismatch")?;
            let info = InfoField::from_wire_bits(&bits, self.partner_role)?;
            let local_partial = ((self.symbol - 1) / PARTIAL_SYMBOLS) as u32;
            let base = *self
                .pfc_base
                .get_or_insert_with(|| info.pfc24.saturating_sub(local_partial));
            if base % PARTIALS_PER_FRAME as u32 != 0 || info.pfc24 != base + local_partial {
                return Err("PAM2 InfoField partial frame counter mismatch".into());
            }
            self.complete_fields += 1;
            return Ok(Some(info));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::super::t1_info::PmaState;
    use super::*;
    fn info(role: Role, pfc24: u32) -> InfoField {
        InfoField {
            role,
            pfc24,
            state: PmaState::Training,
            local_receiver_ok: false,
            enable_or_timing_lock: false,
            data_scrambler_seed: 0x315a,
            eee_capable: false,
            oam_capable: false,
            user_config: 0,
            data_switch_pfc24: 0,
        }
    }
    #[test]
    fn independently_seeded_partner_receiver_acquires_pam2_and_crc_infofield() {
        for role in [Role::Master, Role::Slave] {
            let mut tx = TrainingTransmitter::new(role, 0x1a2b_3c4d5).unwrap();
            let mut rx = TrainingReceiver::new(role);
            for n in 0..2 * SYMBOLS_PER_FRAME {
                let field = info(role, 14 + 15 * (n / SYMBOLS_PER_FRAME) as u32);
                let symbol = tx.next_symbol(&field).unwrap();
                if let Some(decoded) = rx.push_symbol(symbol).unwrap() {
                    assert_eq!(decoded, field);
                }
            }
            assert_eq!(rx.complete_fields(), 2);
            assert_eq!(rx.consecutive_good(), 2 * SYMBOLS_PER_FRAME - 33);
        }
    }
    #[test]
    fn disturbed_infofield_is_not_accepted() {
        let role = Role::Master;
        let mut tx = TrainingTransmitter::new(role, 0x1a2b_3c4d5).unwrap();
        let mut rx = TrainingReceiver::new(role);
        let mut rejected = false;
        for n in 0..SYMBOLS_PER_FRAME {
            let mut symbol = tx.next_symbol(&info(role, 14)).unwrap();
            if n == 14 * PARTIAL_SYMBOLS + 17 {
                symbol = -symbol;
            }
            if rx.push_symbol(symbol).is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected);
    }
    #[test]
    fn slave_start_at_master_next_phy_frame_preserves_pfc_alignment() {
        let role = Role::Slave;
        let mut tx = TrainingTransmitter::new_aligned(role, 0x1234_5678, 15).unwrap();
        let mut rx = TrainingReceiver::new(role);
        for _ in 0..SYMBOLS_PER_FRAME {
            let next = tx.next_symbol(&info(role, 29)).unwrap();
            if let Some(decoded) = rx.push_symbol(next).unwrap() {
                assert_eq!(decoded.pfc24, 29);
            }
        }
        assert_eq!(rx.complete_fields(), 1);
    }
}
