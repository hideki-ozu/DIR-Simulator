//! Validated physical BPDU envelopes; component authorization remains with the owner.
use super::codec::{DecodeResult, bad, crc32, frame, hex, u16be};
use crate::types::{
    ethernet::{EthernetTag, EthernetVlanTag, EthernetWireFrame},
    wire::TreeDomain,
};

#[derive(Debug)]
pub(crate) struct BpduEnvelope<'a> {
    pub domain: TreeDomain,
    pub source: [u8; 6],
    pub tags: Vec<EthernetTag>,
    pub llc: &'a [u8],
    original: &'a [u8],
}
impl BpduEnvelope<'_> {
    /// The ordinary provider egress can retag this validated Length/LLC frame
    /// without routing it through an EtherType-only serializer.
    pub(crate) fn into_wire_frame(self) -> EthernetWireFrame {
        let mac = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":")
        };
        let end = self.original.len() - 4;
        let header_bytes = 14 + self.tags.len() * 4;
        let tag = (self.tags.len() == 1 && self.tags[0].tpid == EthernetTag::C_TPID)
            .then(|| self.tags[0].tci);
        EthernetWireFrame {
            src_mac: mac(&self.source),
            dst_mac: mac(&destination(self.domain)),
            ether_type: self.llc.len() as u16,
            data_hex: hex(self.llc),
            tag,
            tags: if tag.is_some() { Vec::new() } else { self.tags },
            pad_bytes: (end - header_bytes - self.llc.len()) as u64,
            mac_bytes: self.original.len() as u64,
            fcs_hex: hex(&self.original[end..]),
            mac_hex: hex(self.original),
        }
    }
}

fn destination(domain: TreeDomain) -> [u8; 6] {
    [
        1,
        0x80,
        0xc2,
        0,
        0,
        match domain {
            TreeDomain::Customer => 0,
            TreeDomain::Provider => 8,
        },
    ]
}

/// Locally generated BPDUs are untagged. Service relay tags are added by the
/// S-component's ordinary egress transformation after service admission.
pub(crate) fn encode_local(
    domain: TreeDomain,
    source: [u8; 6],
    llc: &[u8],
) -> DecodeResult<Vec<u8>> {
    if source[0] & 1 != 0 || source == [0; 6] {
        return Err(bad("protocol_malformed"));
    }
    if llc.len() < 3 || llc[..3] != [0x42, 0x42, 3] {
        return Err(bad("llc"));
    }
    if llc.len() > 1500 {
        return Err(bad("frame_bounds"));
    }
    Ok(frame(destination(domain), source, llc.len() as u16, llc))
}

/// Verify received bytes before any retagging. A tagged customer BPDU may be
/// transit traffic at an S-component; this parser does not authorize delivery
/// to a local protocol or remove its service tag.
pub(crate) fn decode(bytes: &[u8], max_pdu_bytes: usize) -> DecodeResult<BpduEnvelope<'_>> {
    if bytes.len() < 64 || bytes.len() > 1526 {
        return Err(bad("frame_bounds"));
    }
    let end = bytes.len() - 4;
    if crc32(&bytes[..end]).to_le_bytes() != bytes[end..] {
        return Err(bad("fcs"));
    }
    let domain = match &bytes[..6] {
        [1, 0x80, 0xc2, 0, 0, 0] => TreeDomain::Customer,
        [1, 0x80, 0xc2, 0, 0, 8] => TreeDomain::Provider,
        _ => return Err(bad("protocol_malformed")),
    };
    let source: [u8; 6] = bytes[6..12].try_into().unwrap();
    if source[0] & 1 != 0 || source == [0; 6] {
        return Err(bad("protocol_malformed"));
    }
    let mut offset = 12;
    let mut tags: Vec<EthernetTag> = Vec::with_capacity(2);
    let mut field = u16be(bytes, offset)?;
    while matches!(field, EthernetTag::C_TPID | EthernetTag::S_TPID) {
        if tags.len() == 2
            || offset + 6 > end
            || tags
                .first()
                .is_some_and(|t| t.tpid != EthernetTag::S_TPID || field != EthernetTag::C_TPID)
        {
            return Err(bad("protocol_malformed"));
        }
        let tci = u16be(bytes, offset + 2)?;
        if tci & 0x0fff == 4095 {
            return Err(bad("protocol_malformed"));
        }
        tags.push(EthernetTag {
            tpid: field,
            tci: EthernetVlanTag {
                vid: tci & 0x0fff,
                pcp: (tci >> 13) as u8,
                dei: ((tci >> 12) & 1) as u8,
            },
        });
        offset += 4;
        field = u16be(bytes, offset)?;
    }
    if bytes.len() > 1518 + 4 * tags.len()
        || bytes.len() > max_pdu_bytes.saturating_add(18 + 4 * tags.len())
        || !(3..=1500).contains(&field)
        || field as usize > max_pdu_bytes
        || offset + 2 + field as usize > end
    {
        return Err(bad("frame_bounds"));
    }
    let llc = &bytes[offset + 2..offset + 2 + field as usize];
    if llc[..3] != [0x42, 0x42, 3] {
        return Err(bad("llc"));
    }
    Ok(BpduEnvelope {
        domain,
        source,
        tags,
        llc,
        original: bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::wire::{codec::unhex, rstp};

    // Independently generated with Python zlib.crc32, including original MAC padding.
    const CUSTOMER: &str = "0180c200000002000000000100074242030000008000000000000000000000000000000000000000000000000000000000000000000000000000000060a37bc8";
    const PROVIDER: &str = "0180c2000008020000000001000742420300000080000000000000000000000000000000000000000000000000000000000000000000000000000000a13d1703";
    const SERVICE: &str = "0180c200000002000000000188a8b0640007424203000000800000000000000000000000000000000000000000000000000000000000000000000000000000005cd5b709";
    const LLC: [u8; 7] = [0x42, 0x42, 3, 0, 0, 0, 0x80];

    #[test]
    fn customer_and_provider_addresses_have_independent_literal_fcs() {
        for (domain, expected) in [
            (TreeDomain::Customer, CUSTOMER),
            (TreeDomain::Provider, PROVIDER),
        ] {
            let bytes = encode_local(domain, [2, 0, 0, 0, 0, 1], &LLC).unwrap();
            assert_eq!(hex(&bytes), expected);
            let envelope = decode(&bytes, 1500).unwrap();
            assert_eq!(envelope.domain, domain);
            assert_eq!(envelope.source, [2, 0, 0, 0, 0, 1]);
            assert!(envelope.tags.is_empty());
            assert_eq!(envelope.llc, LLC);
        }
        let customer = unhex(CUSTOMER).unwrap();
        let provider = unhex(PROVIDER).unwrap();
        assert_eq!(
            rstp::decode(&customer, 0x8001, 1500).unwrap().kind,
            rstp::Kind::Tcn
        );
        assert!(
            rstp::decode(&provider, 0x8001, 1500).is_err(),
            "legacy customer decoder stays strict"
        );
    }

    #[test]
    fn service_envelope_preserves_customer_llc_and_requires_valid_received_fcs() {
        let bytes = unhex(SERVICE).unwrap();
        let envelope = decode(&bytes, 1500).unwrap();
        assert_eq!(envelope.domain, TreeDomain::Customer);
        assert_eq!(envelope.llc, LLC);
        assert_eq!(
            envelope.tags,
            [EthernetTag {
                tpid: EthernetTag::S_TPID,
                tci: EthernetVlanTag {
                    vid: 100,
                    pcp: 5,
                    dei: 1
                }
            }]
        );
        let wire = decode(&unhex(CUSTOMER).unwrap(), 1500)
            .unwrap()
            .into_wire_frame();
        assert_eq!(wire.ether_type, 7);
        assert_eq!(wire.pad_bytes, 39);
        let relayed = crate::runtime::ethernet::retag_wire_frame(&wire, &envelope.tags).unwrap();
        assert_eq!(relayed.mac_hex, SERVICE);
        let mut corrupt = bytes.clone();
        corrupt[18] ^= 1;
        assert_eq!(decode(&corrupt, 1500).unwrap_err(), bad("fcs"));
        for (offset, replacement) in [
            (14, &[0xbf, 0xff][..]),
            (16, &[0x88, 0xa8][..]),
            (18, &[0x43][..]),
        ] {
            let mut malformed = bytes.clone();
            malformed[offset..offset + replacement.len()].copy_from_slice(replacement);
            let end = malformed.len() - 4;
            let fcs = crc32(&malformed[..end]).to_le_bytes();
            malformed[end..].copy_from_slice(&fcs);
            assert!(decode(&malformed, 1500).is_err());
        }
        assert!(decode(&bytes, 6).is_err());
        assert!(encode_local(TreeDomain::Provider, [0; 6], &LLC).is_err());
        assert!(encode_local(TreeDomain::Customer, [1, 0, 0, 0, 0, 1], &LLC).is_err());
    }

    #[test]
    fn double_tag_and_nonzero_padding_survive_validated_roundtrip() {
        // Independent Python zlib CRC, with a deliberately nonzero MAC pad.
        let literal = "0180c200000002000000000188a8b0648100600a000742420300000080000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122e12f4454";
        let bytes = unhex(literal).unwrap();
        let parsed = decode(&bytes, 1500).unwrap();
        let tags = parsed.tags.clone();
        assert_eq!(tags.len(), 2);
        let frame = parsed.into_wire_frame();
        assert_eq!(frame.ether_type, 7);
        assert_eq!(frame.pad_bytes, 35);
        assert_eq!(frame.mac_hex, literal);
        assert_eq!(
            crate::runtime::ethernet::retag_wire_frame(&frame, &tags)
                .unwrap()
                .mac_hex,
            literal
        );
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end], 1500).is_err(), "truncated at {end}");
        }
    }
}
