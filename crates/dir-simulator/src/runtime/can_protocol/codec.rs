//! Bosch CAN 2.0 Part B digital frame encoder and independently parsed receiver.
use super::types::Frame;
use crate::registry::{ModelError, ModelResult};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub bits: Vec<u8>,
    pub arbitration: Vec<u8>,
    pub stuff_positions: Vec<usize>,
    pub crc: u16,
    pub crc_delimiter: usize,
    pub ack: usize,
}
fn bits(v: &mut Vec<u8>, n: u32, width: u32) {
    v.extend((0..width).rev().map(|b| ((n >> b) & 1) as u8));
}
fn crc(raw: &[u8]) -> u16 {
    let mut c = 0;
    for &b in raw {
        let feedback = b as u16 ^ (c >> 14);
        c = (c << 1) & 0x7fff;
        if feedback != 0 {
            c ^= 0x4599;
        }
    }
    c
}
pub fn encode(f: &Frame) -> ModelResult<Wire> {
    if f.id > if f.extended { 0x1fffffff } else { 0x7ff }
        || f.dlc > 8
        || if f.remote {
            !f.data.is_empty()
        } else {
            f.data.len() != f.dlc as usize
        }
    {
        return Err("invalid Classical CAN frame/id/DLC/payload".into());
    }
    let mut arbitration = vec![];
    bits(
        &mut arbitration,
        if f.extended { f.id >> 18 } else { f.id },
        11,
    );
    if f.extended {
        arbitration.extend([1, 1]);
        bits(&mut arbitration, f.id & 0x3ffff, 18);
        arbitration.push(u8::from(f.remote));
    } else {
        arbitration.extend([u8::from(f.remote), 0]);
    }
    let mut raw = vec![0];
    raw.extend(&arbitration);
    raw.push(0);
    if f.extended {
        raw.push(0);
    }
    bits(&mut raw, f.dlc as u32, 4);
    for &b in &f.data {
        bits(&mut raw, b as u32, 8);
    }
    let crc = crc(&raw);
    bits(&mut raw, crc as u32, 15);
    let (mut out, mut positions) = (vec![], vec![]);
    let (mut last, mut run) = (2, 0);
    for b in raw {
        out.push(b);
        run = if last == b { run + 1 } else { 1 };
        last = b;
        if run == 5 {
            positions.push(out.len());
            out.push(1 - b);
            last = 1 - b;
            run = 1;
        }
    }
    let delimiter = out.len();
    out.extend([1, 0, 1, 1, 1, 1, 1, 1, 1, 1]);
    Ok(Wire {
        bits: out,
        arbitration,
        stuff_positions: positions,
        crc,
        crc_delimiter: delimiter,
        ack: delimiter + 1,
    })
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detection {
    pub bit: usize,
    pub kind: String,
}
/// Parse the sampled wire, including destuffing, field lengths, CRC and fixed bits.
/// A missing ACK is a transmitter condition and is checked separately.
struct Inspection {
    frame: Option<Frame>,
    ack: Option<usize>,
}
fn inspection(wire: &[u8], receiver_valid: bool) -> Result<Inspection, Detection> {
    let bad = |bit, kind: &str| Detection {
        bit,
        kind: kind.into(),
    };
    let (mut raw, mut at, mut last, mut run) = (Vec::new(), 0, 2, 0);
    let mut required = usize::MAX;
    loop {
        if at >= wire.len() {
            return Ok(Inspection {
                frame: None,
                ack: None,
            });
        }
        let b = wire[at];
        if b > 1 {
            return Err(bad(at, "form"));
        }
        raw.push(b);
        run = if last == b { run + 1 } else { 1 };
        last = b;
        at += 1;
        if run == 5 {
            if at >= wire.len() {
                return Ok(Inspection {
                    frame: None,
                    ack: None,
                });
            }
            if wire[at] == last {
                return Err(bad(at, "stuff"));
            }
            last = wire[at];
            run = 1;
            at += 1;
        }
        if raw.len() == 1 && raw[0] != 0 {
            return Err(bad(0, "form"));
        }
        if raw.len() >= 19 {
            let ext = raw[13] == 1;
            let control = if ext { 39 } else { 19 };
            if raw.len() >= control {
                let dlc = value(&raw[control - 4..control]) as usize;
                if dlc > 8 {
                    return Err(bad(at - 1, "form"));
                }
                let remote = raw[if ext { 32 } else { 12 }] == 1;
                required = control + if remote { 0 } else { dlc * 8 } + 15;
            }
        }
        if raw.len() == required {
            break;
        }
    }
    let ext = raw[13] == 1;
    let control = if ext { 39 } else { 19 };
    // Part B 3.2.1: r0/r1 are transmitted dominant but receivers accept
    // either value. SRR still distinguishes extended arbitration.
    if ext && raw[12] != 1 {
        return Err(bad(at, "form"));
    }
    let expected = crc(&raw[..raw.len() - 15]);
    let got = value(&raw[raw.len() - 15..]) as u16;
    if at >= wire.len() {
        return Ok(Inspection {
            frame: None,
            ack: None,
        });
    }
    if wire.get(at) != Some(&1) {
        return Err(bad(at, "form"));
    }
    if expected != got {
        return Err(bad(at, "crc"));
    }
    for i in at + 2..at + 9 {
        if i >= wire.len() {
            return Ok(Inspection {
                frame: None,
                ack: Some(at + 1),
            });
        }
        if wire.get(i) != Some(&1) {
            return Err(bad(i, "form"));
        }
    }
    if wire.len() < at + if receiver_valid { 9 } else { 10 } {
        return Ok(Inspection {
            frame: None,
            ack: Some(at + 1),
        });
    }
    let id = if ext {
        (value(&raw[1..12]) << 18) | value(&raw[14..32])
    } else {
        value(&raw[1..12])
    };
    let remote = raw[if ext { 32 } else { 12 }] == 1;
    let dlc = value(&raw[control - 4..control]) as u8;
    let data = if remote {
        vec![]
    } else {
        raw[control..control + dlc as usize * 8]
            .chunks_exact(8)
            .map(|b| value(b) as u8)
            .collect()
    };
    Ok(Inspection {
        ack: Some(at + 1),
        frame: Some(Frame {
            id,
            extended: ext,
            remote,
            dlc,
            data,
        }),
    })
}
/// Whether the next sampled bit is this receiver's ACK slot, established
/// from its own destuffed control field and verified CRC/delimiter.
pub fn acknowledgement_ready(wire: &[u8]) -> bool {
    inspection(wire, false).is_ok_and(|status| status.ack == Some(wire.len()))
}
pub fn inspect(wire: &[u8]) -> Result<Option<Frame>, Detection> {
    inspection(wire, false).map(|status| status.frame)
}
/// Bosch Part B message validation: a receiver validates at the penultimate EOF sample.
pub fn receiver_valid(wire: &[u8]) -> Result<Option<Frame>, Detection> {
    inspection(wire, true).map(|status| status.frame)
}
fn value(bits: &[u8]) -> u32 {
    bits.iter().fold(0, |v, &b| (v << 1) | b as u32)
}
impl From<Detection> for ModelError {
    fn from(d: Detection) -> Self {
        Self(format!("{} error at bit {}", d.kind, d.bit))
    }
}

pub fn decode(wire: &[u8]) -> Result<Frame, Detection> {
    inspect(wire)?.ok_or_else(|| Detection {
        bit: wire.len(),
        kind: "form".into(),
    })
}
