use super::*;
use crate::runtime::ethernet::crc32;
pub(crate) const SMD_S: [u8; 4] = [0xe6, 0x4c, 0x7f, 0xb3];
pub(crate) const SMD_C: [u8; 4] = [0x61, 0x52, 0x9e, 0x2a];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PacketKind {
    Express,
    Start { frame_count: u8 },
    Continuation { frame_count: u8, frag_count: u8 },
    Verify,
    Respond,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedPacket<'a> {
    pub kind: PacketKind,
    pub data: &'a [u8],
    pub crc: [u8; 4],
}
/// Reflected CRC representation: invert the first TWO ordinary FCS wire octets.
pub(crate) fn mcrc(prefix: &[u8]) -> [u8; 4] {
    (crc32(prefix) ^ 0x0000_ffff).to_le_bytes()
}
pub(crate) fn valid_mac(mac: &[u8]) -> bool {
    mac.len() >= 64 && mac[mac.len() - 4..] == crc32(&mac[..mac.len() - 4]).to_le_bytes()
}
pub(crate) fn encode_express(mac: &[u8]) -> MergeResult<Vec<u8>> {
    if !valid_mac(mac) {
        return Err(MergeError::InvalidMacFcs);
    }
    let mut bytes = vec![0x55; 7];
    bytes.push(0xd5);
    bytes.extend_from_slice(mac);
    Ok(bytes)
}
pub(crate) fn encode_control(kind: PacketKind) -> MergeResult<Vec<u8>> {
    let smd = match kind {
        PacketKind::Verify => 7,
        PacketKind::Respond => 0x19,
        _ => return Err(MergeError::InvalidState),
    };
    let mut bytes = vec![0x55; 7];
    bytes.push(smd);
    bytes.extend_from_slice(&[0; 60]);
    bytes.extend_from_slice(&[0xf7, 0x76, 0x12, 0x04]);
    Ok(bytes)
}
pub(crate) fn encode_fragment(
    mac: &[u8],
    start: usize,
    end: usize,
    frame_count: u8,
    continuation: Option<u8>,
    active: bool,
    peer_add_frag_size: u8,
) -> MergeResult<Vec<u8>> {
    if !valid_mac(mac) {
        return Err(MergeError::InvalidMacFcs);
    }
    if frame_count > 3
        || peer_add_frag_size > 3
        || continuation.is_some_and(|n| n > 3)
        || start > end
        || end > mac.len() - 4
        || end - start < 60
        || (start == 0) != continuation.is_none()
        || (!active && (start != 0 || end != mac.len() - 4))
    {
        return Err(MergeError::InvalidState);
    }
    let final_packet = end == mac.len() - 4;
    if !final_packet
        && (end - start < 64 * (1 + usize::from(peer_add_frag_size)) - 4 || mac.len() - end < 64)
    {
        return Err(MergeError::ShortPacket);
    }
    let mut bytes = vec![0x55; if continuation.is_some() { 6 } else { 7 }];
    if let Some(n) = continuation {
        bytes.push(SMD_C[usize::from(frame_count)]);
        bytes.push(SMD_S[usize::from(n)]);
    } else {
        bytes.push(if active {
            SMD_S[usize::from(frame_count)]
        } else {
            0xd5
        });
    }
    bytes.extend_from_slice(&mac[start..end]);
    if final_packet {
        bytes.extend_from_slice(&mac[mac.len() - 4..]);
    } else {
        bytes.extend_from_slice(&mcrc(&mac[..end]));
    }
    Ok(bytes)
}
pub(crate) fn decode_packet(bytes: &[u8]) -> MergeResult<DecodedPacket<'_>> {
    if bytes.len() < 72 {
        return Err(MergeError::ShortPacket);
    }
    if bytes[..6] != [0x55; 6] {
        return Err(MergeError::InvalidPreamble);
    }
    let kind = if let Some(frame) = SMD_C.iter().position(|s| *s == bytes[6]) {
        let frag = SMD_S
            .iter()
            .position(|s| *s == bytes[7])
            .ok_or(MergeError::InvalidFragmentCount)?;
        PacketKind::Continuation {
            frame_count: frame as u8,
            frag_count: frag as u8,
        }
    } else {
        if bytes[6] != 0x55 {
            return Err(MergeError::InvalidDelimiter);
        }
        match bytes[7] {
            0xd5 => PacketKind::Express,
            7 => PacketKind::Verify,
            0x19 => PacketKind::Respond,
            b => PacketKind::Start {
                frame_count: SMD_S
                    .iter()
                    .position(|s| *s == b)
                    .ok_or(MergeError::InvalidDelimiter)? as u8,
            },
        }
    };
    let data = &bytes[8..bytes.len() - 4];
    let crc = bytes[bytes.len() - 4..].try_into().unwrap();
    if matches!(kind, PacketKind::Verify | PacketKind::Respond) && crc != mcrc(data) {
        return Err(MergeError::FrameCheckError);
    }
    Ok(DecodedPacket { kind, data, crc })
}
