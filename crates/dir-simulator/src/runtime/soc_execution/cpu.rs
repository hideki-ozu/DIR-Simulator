//! Pure RV32I 2.1 decode/execute. Memory and retirement are owned by the SoC timeline.
//! Reference: RISC-V unprivileged ISA v20240411, RV32I 2.1. Reserved FENCE fields
//! are deliberately ignored; all fences conservatively drain prior effects.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cause {
    InstructionAddressMisaligned,
    InstructionAccessFault,
    IllegalInstruction,
    LoadAddressMisaligned,
    LoadAccessFault,
    StoreAddressMisaligned,
    StoreAccessFault,
    EnvironmentCall,
    Breakpoint,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trap {
    pub cause: Cause,
    pub pc: u32,
    pub instruction: Option<u32>,
    pub tval: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Core {
    pub regs: [u32; 32],
    pub pc: u32,
    pub retired: u64,
    pub trap: Option<Trap>,
}
impl Core {
    pub fn new(pc: u32, sp: u32) -> Self {
        let mut regs = [0; 32];
        regs[2] = sp;
        Self {
            regs,
            pc,
            retired: 0,
            trap: None,
        }
    }
    pub fn retire(&mut self, rd: Option<(u8, u32)>, next_pc: u32) -> Result<(), String> {
        let retired = self
            .retired
            .checked_add(1)
            .ok_or("CPU retirement overflow")?;
        if let Some((r, v)) = rd {
            if r != 0 {
                self.regs[r as usize] = v;
            }
        }
        self.regs[0] = 0;
        self.pc = next_pc;
        self.retired = retired;
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.regs[0] != 0
            || self.pc % 4 != 0
            || self.trap.as_ref().is_some_and(|t| t.pc != self.pc)
        {
            Err("invalid CPU checkpoint registers/PC/trap".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    Retire {
        rd: Option<(u8, u32)>,
        next_pc: u32,
    },
    Load {
        address: u32,
        bytes: usize,
        signed: bool,
        rd: u8,
        next_pc: u32,
    },
    Store {
        address: u32,
        bytes: usize,
        value: u32,
        next_pc: u32,
    },
    Fence {
        next_pc: u32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub opcode: String,
    pub effect: Effect,
}
pub fn load_value(data: &[u8], signed: bool) -> u32 {
    let mut value = 0;
    for (i, b) in data.iter().enumerate() {
        value |= (*b as u32) << (8 * i);
    }
    if signed && data.len() < 4 {
        ((value << (32 - data.len() * 8)) as i32 >> (32 - data.len() * 8)) as u32
    } else {
        value
    }
}
fn sext(value: u32, bits: u32) -> u32 {
    ((value << (32 - bits)) as i32 >> (32 - bits)) as u32
}
pub fn evaluate(c: &Core, w: u32) -> Result<Action, Trap> {
    let fail = |cause, tval| Trap {
        cause,
        pc: c.pc,
        instruction: Some(w),
        tval,
    };
    let illegal = || fail(Cause::IllegalInstruction, w);
    let rd = ((w >> 7) & 31) as u8;
    let rs1 = ((w >> 15) & 31) as usize;
    let rs2 = ((w >> 20) & 31) as usize;
    let (a, b) = (c.regs[rs1], c.regs[rs2]);
    let f3 = (w >> 12) & 7;
    let f7 = w >> 25;
    let imm = sext(w >> 20, 12);
    let next = c.pc.wrapping_add(4);
    let retire = |name: &str, value: u32, next_pc: u32| Action {
        opcode: name.into(),
        effect: Effect::Retire {
            rd: Some((rd, value)),
            next_pc,
        },
    };
    let nop = |name: &str, next_pc: u32| Action {
        opcode: name.into(),
        effect: Effect::Retire { rd: None, next_pc },
    };
    let target = |value: u32| {
        if value % 4 == 0 {
            Ok(value)
        } else {
            Err(fail(Cause::InstructionAddressMisaligned, value))
        }
    };
    let action = match w & 127 {
        0x37 => retire("LUI", w & 0xfffff000, next),
        0x17 => retire("AUIPC", c.pc.wrapping_add(w & 0xfffff000), next),
        0x6f => {
            let offset = sext(
                ((w >> 31) << 20)
                    | (((w >> 12) & 255) << 12)
                    | (((w >> 20) & 1) << 11)
                    | (((w >> 21) & 1023) << 1),
                21,
            );
            retire("JAL", next, target(c.pc.wrapping_add(offset))?)
        }
        0x67 if f3 == 0 => retire("JALR", next, target(a.wrapping_add(imm) & !1)?),
        0x63 => {
            let offset = sext(
                ((w >> 31) << 12)
                    | (((w >> 7) & 1) << 11)
                    | (((w >> 25) & 63) << 5)
                    | (((w >> 8) & 15) << 1),
                13,
            );
            let (name, taken) = match f3 {
                0 => ("BEQ", a == b),
                1 => ("BNE", a != b),
                4 => ("BLT", (a as i32) < (b as i32)),
                5 => ("BGE", (a as i32) >= (b as i32)),
                6 => ("BLTU", a < b),
                7 => ("BGEU", a >= b),
                _ => return Err(illegal()),
            };
            nop(
                name,
                if taken {
                    target(c.pc.wrapping_add(offset))?
                } else {
                    next
                },
            )
        }
        0x03 => {
            let (name, bytes, signed) = match f3 {
                0 => ("LB", 1, true),
                1 => ("LH", 2, true),
                2 => ("LW", 4, true),
                4 => ("LBU", 1, false),
                5 => ("LHU", 2, false),
                _ => return Err(illegal()),
            };
            let address = a.wrapping_add(imm);
            if address % bytes as u32 != 0 {
                return Err(fail(Cause::LoadAddressMisaligned, address));
            }
            Action {
                opcode: name.into(),
                effect: Effect::Load {
                    address,
                    bytes,
                    signed,
                    rd,
                    next_pc: next,
                },
            }
        }
        0x23 => {
            let (name, bytes) = match f3 {
                0 => ("SB", 1),
                1 => ("SH", 2),
                2 => ("SW", 4),
                _ => return Err(illegal()),
            };
            let offset = sext(((w >> 25) << 5) | ((w >> 7) & 31), 12);
            let address = a.wrapping_add(offset);
            if address % bytes as u32 != 0 {
                return Err(fail(Cause::StoreAddressMisaligned, address));
            }
            Action {
                opcode: name.into(),
                effect: Effect::Store {
                    address,
                    bytes,
                    value: b,
                    next_pc: next,
                },
            }
        }
        0x13 => {
            let (name, value) = match f3 {
                0 => ("ADDI", a.wrapping_add(imm)),
                2 => ("SLTI", u32::from((a as i32) < (imm as i32))),
                3 => ("SLTIU", u32::from(a < imm)),
                4 => ("XORI", a ^ imm),
                6 => ("ORI", a | imm),
                7 => ("ANDI", a & imm),
                1 if f7 == 0 => ("SLLI", a << (rs2 as u32)),
                5 if f7 == 0 => ("SRLI", a >> (rs2 as u32)),
                5 if f7 == 32 => ("SRAI", ((a as i32) >> (rs2 as u32)) as u32),
                _ => return Err(illegal()),
            };
            retire(name, value, next)
        }
        0x33 => {
            let (name, value) = match (f7, f3) {
                (0, 0) => ("ADD", a.wrapping_add(b)),
                (32, 0) => ("SUB", a.wrapping_sub(b)),
                (0, 1) => ("SLL", a << (b & 31)),
                (0, 2) => ("SLT", u32::from((a as i32) < (b as i32))),
                (0, 3) => ("SLTU", u32::from(a < b)),
                (0, 4) => ("XOR", a ^ b),
                (0, 5) => ("SRL", a >> (b & 31)),
                (32, 5) => ("SRA", ((a as i32) >> (b & 31)) as u32),
                (0, 6) => ("OR", a | b),
                (0, 7) => ("AND", a & b),
                _ => return Err(illegal()),
            };
            retire(name, value, next)
        }
        0x0f if f3 == 0 => Action {
            opcode: "FENCE".into(),
            effect: Effect::Fence { next_pc: next },
        },
        0x73 if w == 0x00000073 => return Err(fail(Cause::EnvironmentCall, 0)),
        0x73 if w == 0x00100073 => return Err(fail(Cause::Breakpoint, c.pc)),
        _ => return Err(illegal()),
    };
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(c: &Core, w: u32) -> (String, u32, u32) {
        let a = evaluate(c, w).unwrap();
        match a.effect {
            Effect::Retire {
                rd: Some((_, v)),
                next_pc,
            } => (a.opcode, v, next_pc),
            _ => panic!("not register action"),
        }
    }
    fn r(f7: u32, f3: u32) -> u32 {
        (f7 << 25) | (2 << 20) | (1 << 15) | (f3 << 12) | (3 << 7) | 0x33
    }
    fn i(imm: u32, f3: u32) -> u32 {
        ((imm & 4095) << 20) | (1 << 15) | (f3 << 12) | (3 << 7) | 0x13
    }
    #[test]
    fn all_integer_ops_wrap_signed_shift_immediates_and_hints() {
        let mut c = Core::new(0x1000, 0);
        c.regs[1] = 0x80000001;
        c.regs[2] = 33;
        for (f7, f3, want, name) in [
            (0, 0, 0x80000022, "ADD"),
            (32, 0, 0x7fffffe0, "SUB"),
            (0, 1, 2, "SLL"),
            (0, 2, 1, "SLT"),
            (0, 3, 0, "SLTU"),
            (0, 4, 0x80000020, "XOR"),
            (0, 5, 0x40000000, "SRL"),
            (32, 5, 0xc0000000, "SRA"),
            (0, 6, 0x80000021, "OR"),
            (0, 7, 1, "AND"),
        ] {
            let (n, v, pc) = value(&c, r(f7, f3));
            assert_eq!((n.as_str(), v, pc), (name, want, 0x1004));
        }
        for (imm, f3, want, name) in [
            (4095, 0, 0x80000000, "ADDI"),
            (4095, 2, 1, "SLTI"),
            (4095, 3, 1, "SLTIU"),
            (4095, 4, 0x7ffffffe, "XORI"),
            (4095, 6, 0xffffffff, "ORI"),
            (4095, 7, 0x80000001, "ANDI"),
            (31, 1, 0x80000000, "SLLI"),
            (31, 5, 1, "SRLI"),
            (0x41f, 5, 0xffffffff, "SRAI"),
        ] {
            let (n, v, _) = value(&c, i(imm, f3));
            assert_eq!((n.as_str(), v), (name, want));
        }
        assert_eq!(value(&c, 0xabcde1b7).1, 0xabcde000);
        assert_eq!(value(&c, 0xfffff197).1, 0);
        let a = evaluate(&c, 0x00108013).unwrap();
        if let Effect::Retire { rd, next_pc } = a.effect {
            c.retire(rd, next_pc).unwrap();
        }
        assert_eq!(c.regs[0], 0);
        for fence in [0x0000000f, 0x8330000f, 0xfff0808f] {
            assert!(matches!(
                evaluate(&c, fence).unwrap().effect,
                Effect::Fence { .. }
            ));
        }
        for bad in [
            r(1, 0),
            r(32, 1),
            i(32, 1),
            0x0000100f,
            0x30002073,
            0xffffffff,
        ] {
            assert_eq!(
                evaluate(&c, bad).unwrap_err().cause,
                Cause::IllegalInstruction
            );
        }
    }
    #[test]
    fn memory_decode_alignment_zero_destination_and_sign_extension() {
        let mut c = Core::new(0, 0);
        c.regs[1] = 64;
        c.regs[2] = 0xdeadbeef;
        for (f3, bytes, signed, name) in [
            (0, 1, true, "LB"),
            (1, 2, true, "LH"),
            (2, 4, true, "LW"),
            (4, 1, false, "LBU"),
            (5, 2, false, "LHU"),
        ] {
            let a = evaluate(&c, (1 << 15) | (f3 << 12) | 0x03).unwrap();
            assert_eq!(a.opcode, name);
            assert_eq!(
                a.effect,
                Effect::Load {
                    address: 64,
                    bytes,
                    signed,
                    rd: 0,
                    next_pc: 4
                }
            );
        }
        for (f3, bytes, name) in [(0, 1, "SB"), (1, 2, "SH"), (2, 4, "SW")] {
            let a = evaluate(&c, (2 << 20) | (1 << 15) | (f3 << 12) | 0x23).unwrap();
            assert_eq!(a.opcode, name);
            assert_eq!(
                a.effect,
                Effect::Store {
                    address: 64,
                    bytes,
                    value: 0xdeadbeef,
                    next_pc: 4
                }
            );
        }
        assert_eq!(load_value(&[0x80], true), 0xffffff80);
        assert_eq!(load_value(&[0, 0x80], true), 0xffff8000);
        assert_eq!(load_value(&[0, 0x80], false), 32768);
        c.regs[1] = 65;
        assert_eq!(
            evaluate(&c, (1 << 15) | (2 << 12) | 0x03)
                .unwrap_err()
                .cause,
            Cause::LoadAddressMisaligned
        );
        assert_eq!(
            evaluate(&c, (2 << 20) | (1 << 15) | (1 << 12) | 0x23)
                .unwrap_err()
                .cause,
            Cause::StoreAddressMisaligned
        );
        assert_eq!(
            evaluate(&c, 0x73).unwrap_err().cause,
            Cause::EnvironmentCall
        );
        assert_eq!(evaluate(&c, 0x100073).unwrap_err().cause, Cause::Breakpoint);
    }
    #[test]
    fn branch_and_jump_targets_are_precise() {
        let mut c = Core::new(0x1000, 0);
        c.regs[1] = 0xffffffff;
        c.regs[2] = 1;
        for (f3, taken) in [
            (0, false),
            (1, true),
            (4, true),
            (5, false),
            (6, false),
            (7, true),
        ] {
            let word = (2 << 20) | (1 << 15) | (f3 << 12) | (4 << 8) | 0x63;
            let a = evaluate(&c, word).unwrap();
            if let Effect::Retire { next_pc, .. } = a.effect {
                assert_eq!(next_pc, if taken { 0x1008 } else { 0x1004 });
            } else {
                panic!();
            }
        }
        let untaken = (2 << 20) | (1 << 15) | (1 << 8) | 0x63;
        assert!(evaluate(&c, untaken).is_ok());
        assert_eq!(
            evaluate(&c, untaken | (1 << 12)).unwrap_err().cause,
            Cause::InstructionAddressMisaligned
        );
        assert_eq!(value(&c, 0x008001ef), ("JAL".into(), 0x1004, 0x1008));
        c.regs[1] = 0x2001;
        assert_eq!(value(&c, 0x000081e7).2, 0x2000);
        c.regs[1] = 0x2003;
        assert_eq!(evaluate(&c, 0x000081e7).unwrap_err().tval, 0x2002);
        c.pc = 0xfffffffc;
        c.regs[1] = u32::MAX;
        assert_eq!(value(&c, i(1, 0)).1, 0);
        assert_eq!(value(&c, i(1, 0)).2, 0);
    }
}
