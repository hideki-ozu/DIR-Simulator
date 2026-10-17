//! Side-effect-free wire primitives. No expected fixture is used by this module.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropReason(pub String);
pub type DecodeResult<T> = Result<T, DropReason>;
pub fn bad(s: &str) -> DropReason {
    DropReason(s.into())
}
pub fn u16be(b: &[u8], p: usize) -> DecodeResult<u16> {
    let s = b.get(p..p + 2).ok_or_else(|| bad("message_length"))?;
    Ok(u16::from_be_bytes([s[0], s[1]]))
}
pub fn checksum(b: &[u8]) -> u16 {
    let mut sum = 0u64;
    for pair in b.chunks(2) {
        sum += ((pair[0] as u64) << 8) | pair.get(1).copied().unwrap_or(0) as u64;
    }
    while sum >> 16 != 0 {
        sum = (sum & 65535) + (sum >> 16);
    }
    !(sum as u16)
}
pub fn crc32(b: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for x in b {
        crc ^= *x as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
        }
    }
    !crc
}
pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(b.len() * 2);
    for byte in b {
        write!(&mut out, "{byte:02x}").unwrap();
    }
    out
}
#[cfg(test)]
pub fn unhex(s: &str) -> DecodeResult<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 || !s.bytes().all(|x| x.is_ascii_hexdigit()) {
        return Err(bad("hex"));
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).map_err(|_| bad("hex")))
        .collect()
}
#[cfg(test)]
pub fn mac(s: &str) -> DecodeResult<[u8; 6]> {
    if s.len() != 17
        || !s.as_bytes().iter().enumerate().all(|(i, c)| {
            if i % 3 == 2 {
                *c == b':'
            } else {
                c.is_ascii_hexdigit()
            }
        })
    {
        return Err(bad("mac"));
    }
    let b = unhex(&s.replace(':', ""))?;
    Ok(b.try_into().unwrap())
}
pub fn frame(da: [u8; 6], sa: [u8; 6], kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64.max(payload.len() + 18));
    out.extend(da);
    out.extend(sa);
    out.extend(kind.to_be_bytes());
    out.extend(payload);
    out.resize(out.len().max(60), 0);
    out.extend(crc32(&out).to_le_bytes());
    out
}
pub fn client(bytes: &[u8], da: [u8; 6], kind: Option<u16>, max: usize) -> DecodeResult<&[u8]> {
    if bytes.len() < 64 || bytes.len() > max + 18 || bytes.len() > 1518 {
        return Err(bad("frame_bounds"));
    }
    let end = bytes.len() - 4;
    if crc32(&bytes[..end]).to_le_bytes() != bytes[end..] {
        return Err(bad("fcs"));
    }
    if bytes[..6] != da || bytes[6] & 1 != 0 || bytes[6..12].iter().all(|b| *b == 0) {
        return Err(bad("protocol_malformed"));
    }
    let field = u16be(bytes, 12)?;
    if let Some(k) = kind {
        if field != k {
            return Err(bad("protocol_malformed"));
        }
        Ok(&bytes[14..end])
    } else {
        if !(3..=1500).contains(&field) || 14 + field as usize > end {
            return Err(bad("frame_bounds"));
        }
        Ok(&bytes[14..14 + field as usize])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_crc() {
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
    }
    #[test]
    fn odd_checksum() {
        assert_eq!(checksum(&[0x01, 0x02, 0x03]), 0xfbfd);
    }
}
