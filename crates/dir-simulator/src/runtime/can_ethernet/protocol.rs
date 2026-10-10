//! DIR local experimental EtherType codec. It carries no internal lineage.
use crate::types::{
    Diagnostic, Frame,
    can_ethernet::{CanFormat, DirCanPacket},
};
pub const ETHERTYPE: u16 = 34997;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError {
    pub offset: usize,
    pub reason: &'static str,
}
fn bad(offset: usize, reason: &'static str) -> CodecError {
    CodecError { offset, reason }
}
pub fn from_frame(frame: &Frame) -> Result<DirCanPacket, Diagnostic> {
    let format = match frame.format.as_str() {
        "standard" => CanFormat::Standard,
        "extended" => CanFormat::Extended,
        _ => return Err(Diagnostic::prepare("unsupported CAN format")),
    };
    if frame.id > format.max_id()
        || frame.data.len() > 16
        || frame.data.len() % 2 != 0
        || !frame.data.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Diagnostic::prepare("invalid CAN format/ID/data"));
    }
    let data = (0..frame.data.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&frame.data[i..i + 2], 16).unwrap())
        .collect();
    Ok(DirCanPacket {
        format,
        id: frame.id,
        data,
    })
}
pub fn encode(packet: &DirCanPacket) -> Result<Vec<u8>, Diagnostic> {
    if packet.id > packet.format.max_id() || packet.data.len() > 8 {
        return Err(Diagnostic::execution("invalid internal DIR CAN packet"));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(11 + packet.data.len())
        .map_err(|_| Diagnostic::execution("codec allocation failed"))?;
    out.extend(b"DIRC");
    out.push(1);
    out.push(u8::from(packet.format == CanFormat::Extended));
    out.extend(packet.id.to_be_bytes());
    out.push(packet.data.len() as u8);
    out.extend(&packet.data);
    Ok(out)
}
pub fn decode(bytes: &[u8]) -> Result<DirCanPacket, CodecError> {
    if bytes.len() < 11 {
        return Err(bad(bytes.len(), "truncated_header"));
    }
    for (i, b) in b"DIRC".iter().enumerate() {
        if bytes[i] != *b {
            return Err(bad(i, "magic"));
        }
    }
    if bytes[4] != 1 {
        return Err(bad(4, "version"));
    }
    if bytes[5] & 0xfe != 0 {
        return Err(bad(5, "reserved_flags"));
    }
    let format = if bytes[5] & 1 == 0 {
        CanFormat::Standard
    } else {
        CanFormat::Extended
    };
    let id = u32::from_be_bytes(bytes[6..10].try_into().unwrap());
    if id > format.max_id() {
        return Err(bad(6, "identifier_range"));
    }
    let count = bytes[10] as usize;
    if count > 8 {
        return Err(bad(10, "dlc"));
    }
    let end = 11 + count;
    if bytes.len() < end {
        return Err(bad(bytes.len(), "truncated_data"));
    }
    if let Some(i) = bytes[end..].iter().position(|b| *b != 0) {
        return Err(bad(end + i, "nonzero_padding"));
    }
    Ok(DirCanPacket {
        format,
        id,
        data: bytes[11..end].to_vec(),
    })
}
pub fn decode_hex(hex: &str) -> Result<DirCanPacket, CodecError> {
    if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad(0, "hex"));
    }
    let bytes: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    decode(&bytes)
}
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_vectors() {
        for (format, id, data, expected) in [
            ("standard", 0, "", "4449524301000000000000"),
            ("standard", 291, "aabb", "4449524301000000012302aabb"),
            (
                "standard",
                2047,
                "ffffffffffffffff",
                "444952430100000007ff08ffffffffffffffff",
            ),
            (
                "extended",
                0x1fffffff,
                "ffffffffffffffff",
                "4449524301011fffffff08ffffffffffffffff",
            ),
        ] {
            let p = from_frame(&Frame {
                format: format.into(),
                id,
                data: data.into(),
            })
            .unwrap();
            let encoded = encode(&p).unwrap();
            assert_eq!(hex(&encoded), expected);
            assert_eq!(decode(&encoded).unwrap(), p);
            let mut padded = encoded;
            padded.resize(46, 0);
            assert_eq!(decode(&padded).unwrap(), p);
        }
    }
    #[test]
    fn every_invalid_field_and_padding() {
        let valid = encode(&DirCanPacket {
            format: CanFormat::Standard,
            id: 0,
            data: vec![],
        })
        .unwrap();
        for (offset, value) in [(0, 0), (4, 2), (5, 2), (6, 1), (10, 9)] {
            let mut v = valid.clone();
            v[offset] = value;
            assert_eq!(decode(&v).unwrap_err().offset, offset);
        }
        assert!(decode(&valid[..10]).is_err());
        let mut v = valid;
        v.push(1);
        assert_eq!(decode(&v).unwrap_err().reason, "nonzero_padding");
    }
}
