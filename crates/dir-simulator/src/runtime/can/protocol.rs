//! Pure Classical CAN serialization. This is an ideal frame timing model, not a PHY.
use crate::types::{Diagnostic, Frame};

#[derive(Debug, Clone)]
pub struct WireFrame {
    pub crc_input: Vec<u8>,
    pub crc15: u16,
    pub stuffed_region: Vec<u8>,
    pub stuff_positions: Vec<usize>,
    pub frame: Vec<u8>,
    pub arbitration: Vec<u8>,
}

fn bits(out: &mut Vec<u8>, value: u32, width: u32) {
    out.extend((0..width).rev().map(|i| ((value >> i) & 1) as u8));
}

pub fn serialize(frame: &Frame) -> Result<WireFrame, Diagnostic> {
    let extended = match frame.format.as_str() {
        "standard" if frame.id <= 0x7ff => false,
        "extended" if frame.id <= 0x1fffffff => true,
        _ => return Err(Diagnostic::prepare("invalid CAN format or identifier")),
    };
    if frame.data.len() > 16
        || frame.data.len() % 2 != 0
        || !frame.data.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Diagnostic::prepare(
            "CAN data must contain 0 to 8 bytes of hexadecimal data",
        ));
    }
    let mut arbitration = Vec::new();
    let mut crc_input = vec![0];
    if extended {
        bits(&mut arbitration, frame.id >> 18, 11);
        arbitration.extend([1, 1]);
        bits(&mut arbitration, frame.id & 0x3ffff, 18);
        arbitration.push(0);
        crc_input.extend(&arbitration);
        crc_input.extend([0, 0]);
    } else {
        bits(&mut arbitration, frame.id, 11);
        arbitration.extend([0, 0]);
        crc_input.extend(&arbitration);
        crc_input.push(0);
    }
    bits(&mut crc_input, (frame.data.len() / 2) as u32, 4);
    for i in (0..frame.data.len()).step_by(2) {
        let byte = u32::from_str_radix(&frame.data[i..i + 2], 16).expect("validated hex");
        bits(&mut crc_input, byte, 8);
    }
    let mut crc15 = 0u16;
    for &bit in &crc_input {
        let feedback = bit as u16 ^ (crc15 >> 14);
        crc15 = (crc15 << 1) & 0x7fff;
        if feedback != 0 {
            crc15 ^= 0x4599;
        }
    }
    let mut raw = crc_input.clone();
    bits(&mut raw, crc15 as u32, 15);
    let mut stuffed_region = Vec::new();
    let mut stuff_positions = Vec::new();
    let (mut last, mut count) = (2, 0);
    for bit in raw {
        stuffed_region.push(bit);
        count = if last == bit { count + 1 } else { 1 };
        last = bit;
        if count == 5 {
            stuff_positions.push(stuffed_region.len());
            last = 1 - bit;
            stuffed_region.push(last);
            count = 1;
        }
    }
    let mut wire = stuffed_region.clone();
    wire.extend([1, 0, 1, 1, 1, 1, 1, 1, 1, 1]);
    Ok(WireFrame {
        crc_input,
        crc15,
        stuffed_region,
        stuff_positions,
        frame: wire,
        arbitration,
    })
}

pub fn accepts(filter: &str, frame: &Frame) -> bool {
    if filter == "*" {
        return true;
    }
    if filter == "none" {
        return false;
    }
    let format = if frame.format == "standard" {
        "std"
    } else {
        "ext"
    };
    filter.split(',').any(|entry| {
        entry
            .split_once(":0x")
            .is_some_and(|(kind, id)| kind == format && u32::from_str_radix(id, 16) == Ok(frame.id))
    })
}

pub fn end_time(sof: u64, bits: u64, bitrate: u64) -> Result<u64, Diagnostic> {
    if bitrate == 0 {
        return Err(Diagnostic::execution("zero bitrate"));
    }
    let duration = (bits as u128 * 1_000_000_000_000).div_ceil(bitrate as u128);
    u64::try_from(sof as u128 + duration)
        .map_err(|_| Diagnostic::execution("CAN end time overflows u64 picoseconds"))
}
