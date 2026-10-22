//! R-TAG wire transform for the explicitly selected 802.1CB-2017 tag format.
//! This module owns bytes; frame metadata is derived only after FCS validation.
use crate::runtime::ethernet::crc32;
use crate::types::ethernet::EthernetWireFrame;

const RTAG: [u8; 2] = [0xf1, 0xc1];

fn decode_hex(input: &str) -> Result<Vec<u8>, &'static str> {
    if input.len() % 2 != 0 || !input.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid_hex");
    }
    input
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).map_err(|_| "invalid_hex")?;
            u8::from_str_radix(s, 16).map_err(|_| "invalid_hex")
        })
        .collect()
}
fn hex(input: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        write!(&mut out, "{byte:02x}").expect("String write");
    }
    out
}
fn tag_offset(bytes: &[u8]) -> Result<usize, &'static str> {
    if bytes.len() < 18 {
        return Err("truncated_frame");
    }
    let mut offset = 12;
    let mut tags = 0;
    while bytes[offset..offset + 2] == [0x81, 0x00] || bytes[offset..offset + 2] == [0x88, 0xa8] {
        if tags == 2 || offset + 6 > bytes.len() - 4 {
            return Err("truncated_tag");
        }
        offset += 4;
        tags += 1;
    }
    Ok(offset)
}
pub(super) fn transmit(
    base: &EthernetWireFrame,
    sequence: u16,
) -> Result<EthernetWireFrame, &'static str> {
    let mut bytes = decode_hex(&base.mac_hex)?;
    if bytes.len() < 18 || bytes.len() != base.mac_bytes as usize {
        return Err("invalid_frame_length");
    }
    let received_fcs = bytes.split_off(bytes.len() - 4);
    if received_fcs != crc32(&bytes).to_le_bytes() {
        return Err("invalid_fcs");
    }
    let offset = tag_offset(&bytes)?;
    if bytes[offset..offset + 2] == RTAG {
        return Err("nested_rtag_unsupported");
    }
    let mut tag = Vec::from(RTAG);
    tag.extend([0, 0]); // 802.1CB-2017 reserved bits transmit as zero.
    tag.extend(sequence.to_be_bytes());
    bytes.splice(offset..offset, tag);
    let fcs = crc32(&bytes).to_le_bytes();
    bytes.extend(fcs);
    let mut frame = base.clone();
    frame.mac_bytes = bytes.len() as u64;
    frame.fcs_hex = hex(&fcs);
    frame.mac_hex = hex(&bytes);
    Ok(frame)
}
pub(super) fn receive(frame: &EthernetWireFrame) -> Result<u16, &'static str> {
    let bytes = decode_hex(&frame.mac_hex)?;
    if bytes.len() != frame.mac_bytes as usize || bytes.len() < 24 {
        return Err("invalid_frame_length");
    }
    let offset = tag_offset(&bytes)?;
    if bytes.len() < offset + 10 || bytes[offset..offset + 2] != RTAG {
        return Err("missing_rtag");
    }
    let content = &bytes[..bytes.len() - 4];
    if bytes[bytes.len() - 4..] != crc32(content).to_le_bytes() {
        return Err("invalid_fcs");
    }
    if bytes[offset + 6..offset + 8] == RTAG {
        return Err("nested_rtag_unsupported");
    }
    if bytes[offset + 6..offset + 8] != frame.ether_type.to_be_bytes() {
        return Err("inner_type_mismatch");
    }
    Ok(u16::from_be_bytes([bytes[offset + 4], bytes[offset + 5]]))
}

/// Remove one validated R-TAG without reconstructing payload or padding.
pub(super) fn strip(frame: &EthernetWireFrame) -> Result<(EthernetWireFrame, u16), &'static str> {
    let sequence = receive(frame)?;
    let mut bytes = decode_hex(&frame.mac_hex)?;
    let offset = tag_offset(&bytes)?;
    bytes.truncate(bytes.len() - 4);
    bytes.drain(offset..offset + 6);
    let added_pad = 60usize.saturating_sub(bytes.len());
    bytes.resize(bytes.len().max(60), 0);
    let fcs = crc32(&bytes).to_le_bytes();
    bytes.extend(fcs);
    let mut base = frame.clone();
    base.pad_bytes = base
        .pad_bytes
        .checked_add(added_pad as u64)
        .ok_or("padding_length_overflow")?;
    base.mac_hex = hex(&bytes);
    base.mac_bytes = bytes.len() as u64;
    base.fcs_hex = hex(&fcs);
    Ok((base, sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strip_validates_fcs_and_preserves_payload_and_minimum_padding() {
        for vlan in [
            None,
            Some(crate::types::ethernet::EthernetVlanTag {
                vid: 7,
                pcp: 3,
                dei: 1,
            }),
        ] {
            for data in ["", "01", "0123456789abcdef"] {
                let base = crate::runtime::ethernet::serialize_vlan_frame(
                    "02:00:00:00:00:01",
                    "02:00:00:00:00:02",
                    0x0800,
                    data,
                    vlan,
                )
                .unwrap();
                let tagged = transmit(&base, 65535).unwrap();
                let (restored, sequence) = strip(&tagged).unwrap();
                assert_eq!(sequence, 65535);
                assert_eq!(restored.mac_hex, base.mac_hex);
                assert_eq!(restored.mac_bytes, base.mac_bytes);
                assert_eq!(restored.data_hex, base.data_hex);
                assert_eq!(restored.fcs_hex, base.fcs_hex);
                let mut corrupt = tagged.clone();
                corrupt.mac_hex.replace_range(0..2, "ff");
                assert_eq!(strip(&corrupt).unwrap_err(), "invalid_fcs");
            }
        }
    }
    #[test]
    fn vlan_rtag_bytes_and_fcs() {
        let original = crate::runtime::ethernet::serialize_vlan_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "12345678",
            Some(crate::types::ethernet::EthernetVlanTag {
                vid: 7,
                pcp: 3,
                dei: 0,
            }),
        )
        .unwrap();
        let tagged = transmit(&original, 0xfffe).unwrap();
        assert_eq!(tagged.mac_bytes, original.mac_bytes + 6);
        assert_eq!(&tagged.mac_hex[24..44], "81006007f1c10000fffe");
        assert_eq!(receive(&tagged), Ok(0xfffe));
        let mut corrupted = tagged.clone();
        corrupted.mac_hex.replace_range(48..50, "ff");
        assert_eq!(receive(&corrupted), Err("invalid_fcs"));
        let mut truncated = tagged;
        truncated.mac_hex.truncate(36);
        assert_eq!(receive(&truncated), Err("invalid_frame_length"));
    }

    #[test]
    fn receiver_ignores_reserved_bits_only_when_fcs_is_valid() {
        let original = crate::runtime::ethernet::serialize_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "00",
        )
        .unwrap();
        let mut tagged = transmit(&original, 65535).unwrap();
        assert_eq!(&tagged.mac_hex[24..36], "f1c10000ffff");
        let mut bytes = decode_hex(&tagged.mac_hex).unwrap();
        bytes[14] = 0x12;
        bytes[15] = 0x34;
        let crc = crc32(&bytes[..bytes.len() - 4]).to_le_bytes();
        let tail = bytes.len() - 4;
        bytes[tail..].copy_from_slice(&crc);
        tagged.mac_hex = hex(&bytes);
        tagged.fcs_hex = hex(&crc);
        assert_eq!(receive(&tagged), Ok(65535));
        bytes[tail - 1] ^= 1;
        tagged.mac_hex = hex(&bytes);
        assert_eq!(receive(&tagged), Err("invalid_fcs"));
    }
    #[test]
    fn rtag_follows_both_provider_and_customer_tags() {
        use crate::types::ethernet::{EthernetTag, EthernetVlanTag};
        let original = crate::runtime::ethernet::serialize_tag_stack_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x0800,
            "01020304",
            &[
                EthernetTag {
                    tpid: EthernetTag::S_TPID,
                    tci: EthernetVlanTag {
                        vid: 100,
                        pcp: 0,
                        dei: 0,
                    },
                },
                EthernetTag {
                    tpid: EthernetTag::C_TPID,
                    tci: EthernetVlanTag {
                        vid: 10,
                        pcp: 0,
                        dei: 0,
                    },
                },
            ],
        )
        .unwrap();
        let tagged = transmit(&original, 0x1234).unwrap();
        assert_eq!(&tagged.mac_hex[24..52], "88a800648100000af1c100001234");
        assert_eq!(receive(&tagged), Ok(0x1234));
    }
}

#[cfg(test)]
mod literal_wire_tests {
    use super::*;
    #[test]
    fn independently_calculated_crc_literals_and_minimum_external_strip() {
        for (header, fcs) in [
            (
                "020000000002020000000001f1c100001234080001020304",
                "df446ef4",
            ),
            (
                "02000000000202000000000181006007f1c100001234080001020304",
                "7bda467b",
            ),
            (
                "02000000000202000000000188a800648100000af1c100001234080001020304",
                "6e3b1aeb",
            ),
        ] {
            let mut literal = header.to_owned();
            literal.push_str(&"00".repeat(66 - header.len() / 2));
            literal.push_str(fcs);
            let mut frame = crate::runtime::ethernet::serialize_frame(
                "02:00:00:00:00:01",
                "02:00:00:00:00:02",
                0x800,
                "01020304",
            )
            .unwrap();
            frame.mac_hex = literal;
            frame.mac_bytes = 70;
            frame.fcs_hex = fcs.into();
            assert_eq!(receive(&frame), Ok(0x1234));
            assert_eq!(strip(&frame).unwrap().0.mac_bytes, 64);
        }
        let mut frame = crate::runtime::ethernet::serialize_frame(
            "02:00:00:00:00:01",
            "02:00:00:00:00:02",
            0x800,
            "01020304",
        )
        .unwrap();
        // Independent Python zlib literal: externally padded R-TAG body is 60 bytes.
        frame.mac_hex = format!(
            "020000000002020000000001f1c100001234080001020304{}fa8d03e2",
            "00".repeat(36)
        );
        frame.mac_bytes = 64;
        assert_eq!(strip(&frame).unwrap().0.mac_bytes, 64);
        assert_eq!(strip(&frame).unwrap().0.pad_bytes, frame.pad_bytes + 6);
        for (header, fcs, expected) in [
            (
                "020000000002020000000001f1c112341234080001020304",
                "93a388ad",
                Ok(0x1234),
            ),
            (
                "020000000002020000000001f1c100001234f1c100000000",
                "2637c399",
                Err("nested_rtag_unsupported"),
            ),
        ] {
            frame.mac_hex = format!("{header}{}{fcs}", "00".repeat(66 - header.len() / 2));
            frame.mac_bytes = 70;
            frame.fcs_hex = fcs.into();
            assert_eq!(receive(&frame), expected);
        }
    }
}
