use super::*;
use crate::types::ethernet::{EthernetTag, EthernetVlanTag, EthernetWireFrame};
pub(super) fn hex(b: &[u8]) -> String {
    crate::runtime::wire::codec::hex(b)
}
pub(super) fn bytes(s: &str) -> MergeResult<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(MergeError::InvalidMacFcs);
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|c| {
            std::str::from_utf8(c)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or(MergeError::InvalidMacFcs)
        })
        .collect()
}
/// Byte-derived Ethernet metadata; padding cannot be distinguished from an
/// Ethernet-II payload, so received trailing padding remains in data_hex.
pub(super) fn decode(b: &[u8]) -> MergeResult<EthernetWireFrame> {
    if !valid_mac(b) {
        return Err(MergeError::InvalidMacFcs);
    }
    let end = b.len() - 4;
    let mut at = 12;
    let mut tags = vec![];
    while at + 4 <= end && [0x8100, 0x88a8].contains(&u16::from_be_bytes([b[at], b[at + 1]])) {
        let tpid = u16::from_be_bytes([b[at], b[at + 1]]);
        let t = u16::from_be_bytes([b[at + 2], b[at + 3]]);
        tags.push(EthernetTag {
            tpid,
            tci: EthernetVlanTag {
                pcp: (t >> 13) as u8,
                dei: ((t >> 12) & 1) as u8,
                vid: t & 4095,
            },
        });
        at += 4;
    }
    if at + 2 > end {
        return Err(MergeError::ShortPacket);
    }
    let mut kind = u16::from_be_bytes([b[at], b[at + 1]]);
    if kind == 0xf1c1 {
        if at + 8 > end {
            return Err(MergeError::ShortPacket);
        }
        at += 6;
        kind = u16::from_be_bytes([b[at], b[at + 1]]);
    }
    at += 2;
    let one = if tags.len() == 1 && tags[0].tpid == 0x8100 {
        Some(tags.remove(0).tci)
    } else {
        None
    };
    let mac = |s: &[u8]| {
        s.iter()
            .map(|v| format!("{v:02x}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    Ok(EthernetWireFrame {
        src_mac: mac(&b[6..12]),
        dst_mac: mac(&b[..6]),
        ether_type: kind,
        data_hex: hex(&b[at..end]),
        tag: one,
        tags,
        pad_bytes: 0,
        mac_bytes: b.len() as u64,
        fcs_hex: hex(&b[end..]),
        mac_hex: hex(b),
    })
}
