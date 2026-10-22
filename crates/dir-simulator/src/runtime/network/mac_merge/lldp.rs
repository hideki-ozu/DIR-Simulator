//! Bounded IEEE 802.1AB-2016 receive subset: mandatory TLVs identify the MSAP;
//! TTL zero deletes it, and each refresh replaces its optional information.
use super::*;
use crate::types::network::mac_merge::MergePort;
fn tlv(out: &mut Vec<u8>, kind: u16, value: &[u8]) {
    out.extend(((kind << 9) | (value.len() as u16)).to_be_bytes());
    out.extend(value);
}
pub(super) fn advertise(
    port: &str,
    cfg: &MergePort,
    enabled: bool,
    active: bool,
    ttl: u16,
) -> Vec<u8> {
    let mut b = vec![1, 0x80, 0xc2, 0, 0, 0x0e];
    b.extend(cfg.source_mac);
    b.extend([0x88, 0xcc]);
    let mut chassis = vec![4];
    chassis.extend(cfg.source_mac);
    tlv(&mut b, 1, &chassis);
    let mut id = vec![7];
    id.extend(port.as_bytes());
    tlv(&mut b, 2, &id);
    tlv(&mut b, 3, &ttl.to_be_bytes());
    let bits = 1
        | (u16::from(enabled) << 1)
        | (u16::from(active) << 2)
        | (u16::from(cfg.advertised_add_frag_size) << 3);
    let mut extension = vec![0, 0x12, 0x0f, 7];
    extension.extend(bits.to_be_bytes());
    tlv(&mut b, 127, &extension);
    tlv(&mut b, 0, &[]);
    b.resize(b.len().max(60), 0);
    b.extend(crate::runtime::ethernet::crc32(&b).to_le_bytes());
    b
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Observation {
    pub source_mac: [u8; 6],
    pub port_id: Vec<u8>,
    pub ttl: u16,
    pub capability: Option<PeerCapabilitiesObserved>,
}
impl Observation {
    pub fn matches(&self, peer: &PeerCapabilitiesObserved) -> bool {
        self.source_mac == peer.source_mac && self.port_id == peer.port_id
    }
}
pub(super) fn receive(
    ingress: usize,
    packet: &ValidatedMacIngress,
    evidence: String,
) -> MergeResult<Observation> {
    let b = &packet.bytes;
    if !valid_mac(b) || b[..6] != [1, 0x80, 0xc2, 0, 0, 0x0e] || b[12..14] != [0x88, 0xcc] {
        return Err(MergeError::MissingCapabilityEvidence);
    }
    let mut at = 14;
    let mut ttl = None;
    let mut port_id = vec![];
    let mut ended = false;
    for expected in [1, 2, 3] {
        if at + 2 > b.len() - 4 {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        let h = u16::from_be_bytes([b[at], b[at + 1]]);
        at += 2;
        let n = usize::from(h & 511);
        if h >> 9 != expected || n == 0 || at + n > b.len() - 4 {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        if expected < 3 && n < 2 {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        // Explicit selected subset: MAC-address chassis ID and locally assigned
        // port ID. Other valid LLDP identification forms remain unsupported.
        if expected == 1
            && (n != 7
                || b[at] != 4
                || b[at + 1..at + n] != b[6..12]
                || b[6] & 1 != 0
                || b[6..12].iter().all(|x| *x == 0))
        {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        if expected == 2 && (b[at] != 7 || n > 256) {
            return Err(MergeError::MissingCapabilityEvidence);
        }

        if expected == 2 {
            port_id = b[at..at + n].to_vec();
        }
        if expected == 3 {
            if n != 2 {
                return Err(MergeError::MissingCapabilityEvidence);
            }
            ttl = Some(u16::from_be_bytes([b[at], b[at + 1]]));
        }
        at += n;
    }
    while at + 2 <= b.len() - 4 {
        let h = u16::from_be_bytes([b[at], b[at + 1]]);
        at += 2;
        let n = usize::from(h & 511);
        if at + n > b.len() - 4 {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        if h >> 9 == 0 {
            if n != 0 {
                return Err(MergeError::MissingCapabilityEvidence);
            }
            ended = true;
            break;
        }
        if (1..=3).contains(&(h >> 9)) {
            return Err(MergeError::MissingCapabilityEvidence);
        }
        at += n;
    }
    if !ended {
        return Err(MergeError::MissingCapabilityEvidence);
    }
    let capability = PeerCapabilitiesObserved::from_received_lldp(ingress, packet, evidence).ok();
    Ok(Observation {
        source_mac: b[6..12].try_into().unwrap(),
        port_id,
        ttl: ttl.unwrap(),
        capability,
    })
}
