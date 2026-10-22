//! MSTP configuration identifier and BPDU encoding, IEEE 802.1Q-2022 13.8, 14.4–14.5.
//! The physical-port receive, migration, and transmit state is owned by the bridge adapter.
use super::{codec::*, rstp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_MSTIS: usize = 64;
const DIGEST_KEY: [u8; 16] = [
    0x13, 0xac, 0x06, 0xa6, 0x2e, 0x47, 0xfd, 0x51, 0xf9, 0x5d, 0x2b, 0xa2, 0x43, 0xcd, 0x03, 0x46,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Mstid(pub u16);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionId {
    pub name: [u8; 32],
    pub revision: u16,
    pub digest: [u8; 16],
}

impl RegionId {
    pub fn from_vlan_map(name: &str, revision: u16, map: &[u16; 4096]) -> Result<Self, String> {
        let bytes = name.as_bytes();
        if bytes.len() > 32 {
            return Err("MST configuration name exceeds 32 encoded octets".into());
        }
        if map[0] != 0 || map[4095] != 0 {
            return Err("VID 0 and 4095 must map to CIST".into());
        }
        let active: BTreeSet<_> = map.iter().copied().filter(|id| *id != 0).collect();
        if active.len() > MAX_MSTIS || active.iter().any(|id| *id > 4094) {
            return Err("MST configuration has invalid or excess MSTIDs".into());
        }
        let mut table = [0u8; 8192];
        for (vid, id) in map.iter().enumerate() {
            table[2 * vid..2 * vid + 2].copy_from_slice(&id.to_be_bytes());
        }
        let mut fixed_name = [0; 32];
        fixed_name[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            name: fixed_name,
            revision,
            digest: hmac_md5(&DIGEST_KEY, &table),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CistPriority {
    pub root: u64,
    pub external_cost: u32,
    pub regional_root: u64,
    pub internal_cost: u32,
    pub designated_bridge: u64,
    pub designated_port: u16,
    pub receiving_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MstiPriority {
    pub regional_root: u64,
    pub internal_cost: u32,
    pub designated_bridge: u64,
    pub designated_port: u16,
    pub receiving_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MstiMessage {
    pub mstid: Mstid,
    pub flags: u8,
    pub priority: MstiPriority,
    pub remaining_hops: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bpdu {
    pub version: u8,
    pub flags: u8,
    pub cist: CistPriority,
    pub times: rstp::Times,
    pub region: RegionId,
    pub remaining_hops: u8,
    pub msti: Vec<MstiMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Legacy(rstp::Bpdu),
    Mst(Bpdu),
}

fn read_u16(b: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([b[offset], b[offset + 1]])
}
fn read_u32(b: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(b[offset..offset + 4].try_into().unwrap())
}
fn read_u64(b: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(b[offset..offset + 8].try_into().unwrap())
}

pub fn decode(wire: &[u8], receiving_port: u16, max: usize) -> DecodeResult<Received> {
    let payload = client(wire, rstp::DA, None, max)?;
    decode_llc(payload, receiving_port)
}

/// Decode LLC/BPDU bytes supplied by a validated component MAC envelope.
pub fn decode_llc(payload: &[u8], receiving_port: u16) -> DecodeResult<Received> {
    if payload.len() < 7 || payload[..3] != [0x42, 0x42, 3] {
        return Err(bad("llc"));
    }
    let b = &payload[3..];
    if b[..2] != [0, 0] {
        return Err(bad("protocol_id"));
    }
    if b[3] != 2 || b[2] < 3 {
        return rstp::decode_llc(payload, receiving_port).map(Received::Legacy);
    }
    // 14.5 permits Version 3 RST fallback. A 102-octet BPDU with a valid
    // Version 3 Length of 64 is a valid zero-MSTI MST BPDU.
    if b.len() < 38 || b.len() < 102 || b[35] != 0 {
        return rstp::decode_llc(payload, receiving_port).map(Received::Legacy);
    }
    let v3_len = read_u16(b, 36) as usize;
    if v3_len < 64 || (v3_len - 64) % 16 != 0 || (v3_len - 64) / 16 > MAX_MSTIS {
        return rstp::decode_llc(payload, receiving_port).map(Received::Legacy);
    }
    if b.len() < 38 + v3_len {
        return Err(bad("mst_truncated"));
    }
    if b[38] != 0 {
        return Err(bad("mcid_selector"));
    }
    let mut name = [0; 32];
    name.copy_from_slice(&b[39..71]);
    let mut digest = [0; 16];
    digest.copy_from_slice(&b[73..89]);
    let bridge = read_u64(b, 93);
    if bridge & 0x0fff_0000_0000_0000 != 0 {
        return Err(bad("cist_bridge_system_id"));
    }
    let mut msti = Vec::with_capacity((v3_len - 64) / 16);
    let mut seen = BTreeSet::new();
    for offset in (102..38 + v3_len).step_by(16) {
        let root = read_u64(b, offset + 1);
        let id = (root >> 48) as u16 & 0x0fff;
        if id == 0 || id > 4094 || !seen.insert(id) {
            return Err(bad("msti_id"));
        }
        let bridge_priority = (b[offset + 13] as u64 & 0xf0) << 56;
        let port_priority = (b[offset + 14] as u16 & 0xf0) << 8;
        msti.push(MstiMessage {
            mstid: Mstid(id),
            flags: b[offset],
            priority: MstiPriority {
                regional_root: root,
                internal_cost: read_u32(b, offset + 9),
                designated_bridge: bridge_priority
                    | (bridge & 0x0000_ffff_ffff_ffff)
                    | (id as u64 * (1u64 << 48)),
                designated_port: port_priority | (read_u16(b, 25) & 0x0fff),
                receiving_port,
            },
            remaining_hops: b[offset + 15],
        });
    }
    Ok(Received::Mst(Bpdu {
        version: b[2],
        flags: b[4] & 0x7f,
        cist: CistPriority {
            root: read_u64(b, 5),
            external_cost: read_u32(b, 13),
            regional_root: read_u64(b, 17),
            internal_cost: read_u32(b, 89),
            designated_bridge: bridge,
            designated_port: read_u16(b, 25),
            receiving_port,
        },
        times: rstp::Times {
            age: read_u16(b, 27),
            max_age: read_u16(b, 29),
            hello: read_u16(b, 31),
            forward_delay: read_u16(b, 33),
        },
        region: RegionId {
            name,
            revision: read_u16(b, 71),
            digest,
        },
        remaining_hops: b[101],
        msti,
    }))
}

pub fn encode(sa: [u8; 6], m: &Bpdu) -> Result<Vec<u8>, String> {
    let p = encode_llc(m)?;
    Ok(frame(rstp::DA, sa, p.len() as u16, &p))
}

/// Encode just LLC/BPDU bytes for an already selected component destination.
pub fn encode_llc(m: &Bpdu) -> Result<Vec<u8>, String> {
    if m.msti.len() > MAX_MSTIS || m.version < 3 {
        return Err("invalid MST BPDU version or instance count".into());
    }
    let mut p = vec![0x42, 0x42, 3, 0, 0, m.version, 2, m.flags & 0x7f];
    p.extend(m.cist.root.to_be_bytes());
    p.extend(m.cist.external_cost.to_be_bytes());
    p.extend(m.cist.regional_root.to_be_bytes());
    p.extend(m.cist.designated_port.to_be_bytes());
    for t in [
        m.times.age,
        m.times.max_age,
        m.times.hello,
        m.times.forward_delay,
    ] {
        p.extend(t.to_be_bytes());
    }
    p.push(0);
    p.extend((64 + 16 * m.msti.len() as u16).to_be_bytes());
    p.push(0);
    p.extend(m.region.name);
    p.extend(m.region.revision.to_be_bytes());
    p.extend(m.region.digest);
    p.extend(m.cist.internal_cost.to_be_bytes());
    if m.cist.designated_bridge & 0x0fff_0000_0000_0000 != 0 {
        return Err("CIST bridge system ID must be zero".into());
    }
    p.extend(m.cist.designated_bridge.to_be_bytes());
    p.push(m.remaining_hops);
    let mut last = 0;
    for instance in &m.msti {
        let id = instance.mstid.0;
        if id == 0
            || id > 4094
            || id <= last
            || (instance.priority.regional_root >> 48) as u16 & 0x0fff != id
        {
            return Err(
                "MSTI records must have unique increasing IDs matching regional root".into(),
            );
        }
        last = id;
        p.push(instance.flags);
        p.extend(instance.priority.regional_root.to_be_bytes());
        p.extend(instance.priority.internal_cost.to_be_bytes());
        p.push((instance.priority.designated_bridge >> 56) as u8 & 0xf0);
        p.push((instance.priority.designated_port >> 8) as u8 & 0xf0);
        p.push(instance.remaining_hops);
    }
    Ok(p)
}

fn hmac_md5(key: &[u8], message: &[u8]) -> [u8; 16] {
    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend((0..64).map(|i| key.get(i).copied().unwrap_or(0) ^ 0x36));
    inner.extend(message);
    let inner_hash = md5(&inner);
    let mut outer = Vec::with_capacity(80);
    outer.extend((0..64).map(|i| key.get(i).copied().unwrap_or(0) ^ 0x5c));
    outer.extend(inner_hash);
    md5(&outer)
}

fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut b = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    b.push(0x80);
    while b.len() % 64 != 56 {
        b.push(0);
    }
    b.extend(bits.to_le_bytes());
    let mut h = [0x67452301u32, 0xefcdab89, 0x98badcfe, 0x10325476];
    for chunk in b.chunks_exact(64) {
        let mut w = [0u32; 16];
        for i in 0..16 {
            w[i] = u32::from_le_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let [mut a, mut c, mut d, mut e] = h;
        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((c & d) | (!c & e), i)
            } else if i < 32 {
                ((e & c) | (!e & d), (5 * i + 1) % 16)
            } else if i < 48 {
                (c ^ d ^ e, (3 * i + 5) % 16)
            } else {
                (d ^ (c | !e), (7 * i) % 16)
            };
            let next = c.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(w[g])
                    .rotate_left(S[i]),
            );
            a = e;
            e = d;
            d = c;
            c = next;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(c);
        h[2] = h[2].wrapping_add(d);
        h[3] = h[3].wrapping_add(e);
    }
    let mut out = [0; 16];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Disabled,
    Root,
    Designated,
    Alternate,
    Backup,
    Master,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceConfig {
    pub mstid: Mstid,
    /// IEEE Bridge Priority, a multiple of 4096, in the high four bits.
    pub bridge_priority: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MstPortConfig {
    pub id: u16,
    pub external_cost: u32,
    pub internal_cost: u32,
    pub enabled: bool,
    pub oper_point_to_point_mac: bool,
    pub admin_edge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Neighbor {
    pub cist: CistPriority,
    pub times: rstp::Times,
    pub region: Option<RegionId>,
    pub cist_flags: u8,
    pub remaining_hops: u8,
    pub msti: BTreeMap<Mstid, MstiMessage>,
    pub age_seconds: u32,
    pub info_seconds: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationPhase {
    CheckingRstp,
    SelectingStp,
    Sensing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalPort {
    pub config: MstPortConfig,
    /// One migration state and one transmit hold counter for the whole BPDU.
    pub send_mstp: bool,
    pub send_stp: bool,
    pub tc_ack: bool,
    pub migration_phase: MigrationPhase,
    pub received_stp: bool,
    pub received_rstp: bool,
    pub migration_seconds: u8,
    pub hello_seconds: u8,
    pub tx_count: u32,
    pub boundary: bool,
    pub lan_region: Option<RegionId>,
    #[serde(with = "crate::runtime::network::checkpoint::map")]
    pub neighbors: BTreeMap<(u64, u16), Neighbor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreePortState {
    pub role: Role,
    pub learning: bool,
    pub forwarding: bool,
    pub agreed: bool,
    pub agree: bool,
    pub proposing: bool,
    pub proposed: bool,
    pub sync: bool,
    pub synced: bool,
    pub disputed: bool,
    pub remaining_hops: u8,
    pub delay_seconds: u32,
    pub topology_change: bool,
    pub tc_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeState {
    pub bridge_id: u64,
    pub cist_root: Option<CistPriority>,
    pub msti_root: Option<MstiPriority>,
    pub root_port: Option<String>,
    pub ports: BTreeMap<String, TreePortState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MstBridge {
    pub id: u64,
    pub region: RegionId,
    pub vlan_map: Vec<u16>,
    pub times: rstp::Times,
    pub max_hops: u8,
    pub root_times: rstp::Times,
    pub root_hops: u8,
    pub tx_hold: u32,
    pub ports: BTreeMap<String, PhysicalPort>,
    pub trees: BTreeMap<Mstid, TreeState>,
}

#[derive(Debug, Clone)]
pub enum MstInput {
    Initialize,
    Tick,
    Receive(String, Received),
    Link(String, bool),
}

#[derive(Debug, Clone)]
pub enum MstTx {
    Legacy(rstp::Bpdu),
    Mst(Bpdu),
}

#[derive(Debug, Clone)]
pub struct MstDelta {
    pub state: MstBridge,
    pub tx: Vec<(String, MstTx)>,
    pub flush: Vec<(Mstid, String)>,
}

fn bridge_id_for_instance(base: u64, mstid: Mstid, priority: u16) -> u64 {
    ((priority as u64 | mstid.0 as u64) << 48) | (base & 0x0000_ffff_ffff_ffff)
}
fn initial_port_state(hops: u8) -> TreePortState {
    TreePortState {
        role: Role::Disabled,
        learning: false,
        forwarding: false,
        agreed: false,
        agree: false,
        proposing: false,
        proposed: false,
        sync: false,
        synced: false,
        disputed: false,
        remaining_hops: hops,
        delay_seconds: 0,
        topology_change: false,
        tc_seconds: 0,
    }
}
fn flags_for(role: Role, state: &TreePortState) -> u8 {
    let bits = match role {
        Role::Root | Role::Master => 0x08,
        Role::Designated => 0x0c,
        Role::Alternate | Role::Backup => 0x04,
        Role::Disabled => 0,
    };
    bits | if state.topology_change { 1 } else { 0 }
        | if state.proposing { 2 } else { 0 }
        | if state.learning { 0x10 } else { 0 }
        | if state.forwarding { 0x20 } else { 0 }
        | if state.agree { 0x40 } else { 0 }
        | if role == Role::Master { 0x80 } else { 0 }
}

fn info_lifetime(hello: u16) -> u32 {
    3 * ((hello as u32 + 255) / 256).max(1)
}

fn advanced_age(age: u16) -> u32 {
    ((age as u32 + 256 + 128) / 256) * 256
}

fn tc_duration(times: rstp::Times, send_stp: bool) -> u32 {
    if send_stp {
        ((times.max_age as u32 + times.forward_delay as u32 + 255) / 256).max(1)
    } else {
        ((times.hello as u32 + 255) / 256).max(1) + 1
    }
}

fn forward_delay_seconds(times: rstp::Times, send_stp: bool) -> u32 {
    let delay = if send_stp {
        times.forward_delay
    } else {
        times.hello
    };
    ((delay as u32 + 255) / 256).max(1)
}

fn same_bridge_address(a: u64, b: u64) -> bool {
    a & 0x0000_ffff_ffff_ffff == b & 0x0000_ffff_ffff_ffff
}

fn settle_migration(port: &mut PhysicalPort) {
    match port.migration_phase {
        MigrationPhase::CheckingRstp if port.migration_seconds == 0 => {
            port.migration_phase = MigrationPhase::Sensing;
            port.received_stp = false;
            port.received_rstp = false;
        }
        MigrationPhase::Sensing
            if !port.config.enabled || !port.send_mstp && port.received_rstp =>
        {
            port.migration_phase = MigrationPhase::CheckingRstp;
            port.send_mstp = true;
            port.send_stp = false;
            port.migration_seconds = 3;
            port.received_stp = false;
            port.received_rstp = false;
        }
        MigrationPhase::Sensing if port.send_mstp && port.received_stp => {
            port.migration_phase = MigrationPhase::SelectingStp;
            port.send_mstp = false;
            port.send_stp = true;
            port.migration_seconds = 3;
            port.received_stp = false;
            port.received_rstp = false;
        }
        MigrationPhase::SelectingStp if port.migration_seconds == 0 || !port.config.enabled => {
            port.migration_phase = MigrationPhase::Sensing;
            port.received_stp = false;
            port.received_rstp = false;
        }
        _ => {}
    }
}

impl MstBridge {
    pub fn new(
        id: u64,
        region: RegionId,
        vlan_map: Vec<u16>,
        instances: Vec<InstanceConfig>,
        ports: BTreeMap<String, MstPortConfig>,
        times: rstp::Times,
        tx_hold: u32,
    ) -> Result<Self, String> {
        if vlan_map.len() != 4096 {
            return Err("MST VLAN map requires 4096 entries".into());
        }
        let map: &[u16; 4096] = vlan_map.as_slice().try_into().unwrap();
        let recalculated = RegionId::from_vlan_map(
            std::str::from_utf8(&region.name)
                .map_err(|_| "MST region name must be UTF-8")?
                .trim_end_matches('\0'),
            region.revision,
            map,
        )?;
        if recalculated != region {
            return Err("MST region digest does not match VLAN map".into());
        }
        let declared: BTreeSet<_> = instances.iter().map(|i| i.mstid).collect();
        if declared.len() != instances.len()
            || instances.len() > MAX_MSTIS
            || declared.iter().any(|id| id.0 == 0 || id.0 > 4094)
            || vlan_map
                .iter()
                .any(|id| *id != 0 && !declared.contains(&Mstid(*id)))
        {
            return Err("MST instance declarations do not cover VLAN map".into());
        }
        if instances.iter().any(|i| i.bridge_priority & 0x0fff != 0) {
            return Err("MST bridge priority must be a multiple of 4096".into());
        }
        if ports.is_empty()
            || ports
                .values()
                .any(|p| p.id == 0 || p.external_cost == 0 || p.internal_cost == 0)
        {
            return Err("MST port ID and path costs must be nonzero".into());
        }
        let ids: BTreeSet<_> = ports.values().map(|p| p.id).collect();
        if ids.len() != ports.len() {
            return Err("duplicate MST port ID".into());
        }
        let ports: BTreeMap<_, _> = ports
            .into_iter()
            .map(|(name, config)| {
                (
                    name,
                    PhysicalPort {
                        config,
                        send_mstp: true,
                        send_stp: false,
                        tc_ack: false,
                        migration_phase: MigrationPhase::CheckingRstp,
                        received_stp: false,
                        received_rstp: false,
                        migration_seconds: 3,
                        hello_seconds: 0,
                        tx_count: 0,
                        boundary: false,
                        lan_region: None,
                        neighbors: BTreeMap::new(),
                    },
                )
            })
            .collect();
        let mut trees = BTreeMap::new();
        for (mstid, bridge_id) in std::iter::once((Mstid(0), id)).chain(instances.iter().map(|i| {
            (
                i.mstid,
                bridge_id_for_instance(id, i.mstid, i.bridge_priority),
            )
        })) {
            trees.insert(
                mstid,
                TreeState {
                    bridge_id,
                    cist_root: None,
                    msti_root: None,
                    root_port: None,
                    ports: ports
                        .keys()
                        .map(|name| (name.clone(), initial_port_state(20)))
                        .collect(),
                },
            );
        }
        let mut bridge = Self {
            id,
            region,
            vlan_map,
            times,
            max_hops: 20,
            root_times: times,
            root_hops: 20,
            tx_hold,
            ports,
            trees,
        };
        bridge.select();
        Ok(bridge)
    }

    pub fn mstid_for_vid(&self, vid: u16) -> Option<Mstid> {
        self.vlan_map
            .get(vid as usize)
            .filter(|_| (1..=4094).contains(&vid))
            .copied()
            .map(Mstid)
    }

    pub fn plan(&self, input: MstInput, _max_steps: usize) -> Result<MstDelta, String> {
        let mut state = self.clone();
        let mut flush = BTreeSet::new();
        let mut transmit;
        let mut received_tc: Vec<(Mstid, String)> = Vec::new();
        let mut received_tcn = None;
        let mut received_tc_ack = None;
        match input {
            MstInput::Initialize => transmit = true,
            MstInput::Tick => {
                for port in state.ports.values_mut() {
                    port.hello_seconds = port.hello_seconds.saturating_sub(1);
                    port.tx_count = port.tx_count.saturating_sub(1);
                    port.migration_seconds = port.migration_seconds.saturating_sub(1);
                    settle_migration(port);
                    port.neighbors.retain(|_, n| {
                        n.age_seconds += 1;
                        n.info_seconds = n.info_seconds.saturating_sub(1);
                        n.info_seconds != 0
                    });
                }
                for tree in state.trees.values_mut() {
                    for p in tree.ports.values_mut() {
                        p.delay_seconds = p.delay_seconds.saturating_sub(1);
                        p.tc_seconds = p.tc_seconds.saturating_sub(1);
                        p.topology_change = p.tc_seconds != 0;
                    }
                }
                transmit = state.ports.values().any(|p| p.hello_seconds == 0);
            }
            MstInput::Link(name, up) => {
                let p = state.ports.get_mut(&name).ok_or("unknown MST port")?;
                p.config.enabled = up;
                if !up {
                    p.neighbors.clear();
                    p.lan_region = None;
                    p.migration_phase = MigrationPhase::CheckingRstp;
                    p.send_mstp = true;
                    p.send_stp = false;
                    p.migration_seconds = 3;
                }
                transmit = up;
            }
            MstInput::Receive(name, message) => {
                let p = state.ports.get_mut(&name).ok_or("unknown MST port")?;
                if !p.config.enabled {
                    return Ok(MstDelta {
                        state,
                        tx: vec![],
                        flush: vec![],
                    });
                }
                let n = match message {
                    Received::Legacy(m) => {
                        if m.kind == rstp::Kind::Tcn {
                            received_tcn = Some(name.clone());
                            p.received_stp = true;
                            // TCN carries no priority vector. Retain the current
                            // neighbor information while notifying CIST TCM.
                            p.hello_seconds = 0;
                            None
                        } else {
                            if m.kind == rstp::Kind::Config {
                                p.received_stp = true;
                                if m.flags & 0x80 != 0 {
                                    received_tc_ack = Some(name.clone());
                                }
                            } else {
                                p.received_rstp = true;
                            }
                            if m.flags & 1 != 0 {
                                received_tc
                                    .extend(state.trees.keys().map(|id| (*id, name.clone())));
                            }
                            Some(Neighbor {
                                cist: CistPriority {
                                    root: m.priority.0[0],
                                    external_cost: m.priority.0[1] as u32,
                                    regional_root: m.priority.0[2],
                                    internal_cost: 0,
                                    designated_bridge: m.priority.0[4],
                                    designated_port: m.priority.0[5] as u16,
                                    receiving_port: p.config.id,
                                },
                                times: m.times,
                                region: None,
                                cist_flags: m.flags,
                                remaining_hops: state.max_hops,
                                msti: BTreeMap::new(),
                                age_seconds: 0,
                                info_seconds: info_lifetime(state.times.hello),
                            })
                        }
                    }
                    Received::Mst(m) => {
                        p.received_rstp = true;
                        let same_region = m.region == state.region;
                        if m.flags & 1 != 0 {
                            if m.region == state.region {
                                received_tc.push((Mstid(0), name.clone()));
                            } else {
                                received_tc
                                    .extend(state.trees.keys().map(|id| (*id, name.clone())));
                            }
                        }
                        if m.region == state.region {
                            for record in &m.msti {
                                if record.flags & 1 != 0 && state.trees.contains_key(&record.mstid)
                                {
                                    received_tc.push((record.mstid, name.clone()));
                                }
                            }
                        }
                        Some(Neighbor {
                            cist: CistPriority {
                                receiving_port: p.config.id,
                                ..m.cist
                            },
                            times: m.times,
                            region: Some(m.region),
                            cist_flags: m.flags,
                            remaining_hops: m.remaining_hops,
                            msti: m
                                .msti
                                .into_iter()
                                .filter(|v| {
                                    same_region
                                        && v.remaining_hops > 1
                                        && state.trees.contains_key(&v.mstid)
                                })
                                .map(|v| (v.mstid, v))
                                .collect(),
                            age_seconds: 0,
                            info_seconds: info_lifetime(state.times.hello),
                        })
                    }
                };
                settle_migration(p);
                if let Some(n) = n {
                    let internal = n.region.as_ref() == Some(&state.region);
                    let valid = if internal {
                        n.remaining_hops > 1
                    } else {
                        advanced_age(n.times.age) <= n.times.max_age as u32
                    };
                    if valid {
                        p.neighbors
                            .insert((n.cist.designated_bridge, n.cist.designated_port), n);
                    } else {
                        p.neighbors
                            .remove(&(n.cist.designated_bridge, n.cist.designated_port));
                    }
                }
                transmit = true;
            }
        }
        let before = state.trees.clone();
        state.select();
        if let Some(name) = received_tcn {
            if state.trees[&Mstid(0)].ports[&name].role == Role::Designated {
                state.ports.get_mut(&name).unwrap().tc_ack = true;
                received_tc.extend(state.trees.keys().map(|id| (*id, name.clone())));
            }
        }
        for (id, tree) in &state.trees {
            for (name, port) in &tree.ports {
                if !before[id].ports[name].forwarding
                    && port.forwarding
                    && !state.ports[name].config.admin_edge
                {
                    received_tc.push((*id, name.clone()));
                }
            }
        }
        for (id, ingress) in received_tc.into_iter().collect::<BTreeSet<_>>() {
            let tree = state.trees.get_mut(&id).unwrap();
            for (name, port) in &mut tree.ports {
                if port.role != Role::Disabled && !state.ports[name].config.admin_edge {
                    if port.tc_seconds == 0 {
                        port.tc_seconds = tc_duration(state.root_times, state.ports[name].send_stp);
                    }
                    port.topology_change = true;
                    if name != &ingress {
                        flush.insert((id, name.clone()));
                    }
                    state.ports.get_mut(name).unwrap().hello_seconds = 0;
                    transmit = true;
                }
            }
        }
        for (id, tree) in &state.trees {
            for (name, p) in &tree.ports {
                if before[id].ports[name].forwarding && !p.forwarding {
                    flush.insert((*id, name.clone()));
                }
            }
        }
        // An acknowledged legacy TCN is complete even when the same
        // Configuration BPDU also carries TC for propagation to other ports.
        if let Some(name) = received_tc_ack {
            let cist = state.trees.get_mut(&Mstid(0)).unwrap();
            if cist.ports[&name].role == Role::Root {
                let port = cist.ports.get_mut(&name).unwrap();
                port.tc_seconds = 0;
                port.topology_change = false;
            }
        }
        let tx = if transmit { state.transmit() } else { vec![] };
        Ok(MstDelta {
            state,
            tx,
            flush: flush.into_iter().collect(),
        })
    }

    fn best_neighbor(&self, name: &str) -> Option<&Neighbor> {
        self.ports[name]
            .neighbors
            .values()
            .filter(|n| n.cist_flags & 0x0c == 0x0c)
            .filter(|n| !same_bridge_address(n.cist.designated_bridge, self.id))
            .min_by_key(|n| n.cist)
    }

    fn select(&mut self) {
        let mut best = CistPriority {
            root: self.id,
            external_cost: 0,
            regional_root: self.id,
            internal_cost: 0,
            designated_bridge: self.id,
            designated_port: 0,
            receiving_port: 0,
        };
        let mut root_port = None;
        let mut root_times = self.times;
        let mut root_hops = self.max_hops;
        for (name, p) in &self.ports {
            if !p.config.enabled {
                continue;
            }
            if let Some(n) = self.best_neighbor(name) {
                let mut candidate = n.cist;
                if n.region.as_ref() == Some(&self.region) {
                    candidate.internal_cost = candidate
                        .internal_cost
                        .saturating_add(p.config.internal_cost);
                } else {
                    candidate.external_cost = candidate
                        .external_cost
                        .saturating_add(p.config.external_cost);
                    candidate.regional_root = self.id;
                    candidate.internal_cost = 0;
                }
                if candidate < best {
                    best = candidate;
                    root_port = Some(name.clone());
                    root_times = n.times;
                    root_times.hello = self.times.hello;
                    if n.region.as_ref() == Some(&self.region) {
                        root_hops = n.remaining_hops.saturating_sub(1);
                    } else {
                        root_hops = self.max_hops;
                        root_times.age = advanced_age(n.times.age).min(u16::MAX as u32) as u16;
                    }
                }
            }
        }
        let fast_tc = tc_duration(root_times, false);
        self.root_times = root_times;
        self.root_hops = root_hops;
        let cist_root = root_port.clone();
        let cist_proposal = cist_root.as_ref().is_some_and(|root| {
            self.ports[root].config.oper_point_to_point_mac
                && self
                    .best_neighbor(root)
                    .is_some_and(|n| n.cist_flags & 0x02 != 0)
        });
        let cist = self.trees.get_mut(&Mstid(0)).unwrap();
        if cist.cist_root != Some(best) || cist.root_port != root_port {
            for (name, state) in &mut cist.ports {
                state.topology_change |= state.forwarding;
                if state.forwarding {
                    state.tc_seconds = fast_tc;
                }
                state.learning = false;
                state.forwarding = false;
                state.agreed = false;
                state.agree = false;
                state.delay_seconds = forward_delay_seconds(root_times, self.ports[name].send_stp);
            }
        }
        cist.cist_root = Some(best);
        cist.root_port = root_port;
        if cist_proposal {
            if let Some(root) = cist_root.as_ref() {
                for (name, state) in &mut cist.ports {
                    if name != root {
                        state.sync = true;
                        state.agreed = false;
                        state.agree = false;
                        state.proposing = false;
                        state.learning = false;
                        state.forwarding = false;
                        state.delay_seconds =
                            forward_delay_seconds(root_times, self.ports[name].send_stp);
                        state.synced = true;
                    }
                }
            }
        }
        let edge_ports = self
            .ports
            .iter()
            .filter_map(|(name, port)| port.config.admin_edge.then_some(name.clone()))
            .collect::<BTreeSet<_>>();
        for (name, p) in &mut self.ports {
            let neighbor = p
                .neighbors
                .values()
                .filter(|n| n.cist_flags & 0x0c == 0x0c)
                .min_by_key(|n| n.cist);
            let local = CistPriority {
                designated_bridge: self.id,
                designated_port: p.config.id,
                receiving_port: p.config.id,
                ..best
            };
            p.lan_region = if neighbor.is_none_or(|n| local <= n.cist) {
                Some(self.region.clone())
            } else {
                neighbor.and_then(|n| n.region.clone())
            };
            // The best CIST Designated Bridge determines the LAN's region;
            // an inferior foreign advertisement does not split that LAN.
            p.boundary = p.lan_region.as_ref() != Some(&self.region);
            let role = if !p.config.enabled {
                Role::Disabled
            } else if cist_root.as_deref() == Some(name) {
                Role::Root
            } else if neighbor.is_none_or(|n| local <= n.cist) {
                Role::Designated
            } else if neighbor.is_some_and(|n| {
                n.cist.designated_bridge & 0x0000_ffff_ffff_ffff == self.id & 0x0000_ffff_ffff_ffff
            }) {
                Role::Backup
            } else {
                Role::Alternate
            };
            let synced = other_ports_synced(&cist.ports, name, &edge_ports);
            let state = cist.ports.get_mut(name).unwrap();
            state.agreed = p.config.oper_point_to_point_mac
                && p.neighbors.values().any(|n| n.cist_flags & 0x40 != 0);
            state.proposed = neighbor.is_some_and(|n| n.cist_flags & 0x0e == 0x0e);
            if state.agreed {
                state.proposing = false;
            }
            state.disputed = p
                .neighbors
                .values()
                .any(|n| n.cist_flags & 0x0c == 0x0c && n.cist_flags & 0x10 != 0 && n.cist > local);
            state.remaining_hops = self.root_hops;
            set_role_state(
                state,
                role,
                p.config.admin_edge,
                p.config.oper_point_to_point_mac,
                synced,
                forward_delay_seconds(self.root_times, p.send_stp),
                tc_duration(self.root_times, p.send_stp),
            );
        }
        let cist_snapshot = self.trees[&Mstid(0)].clone();
        for (id, tree) in self.trees.iter_mut().filter(|(id, _)| **id != Mstid(0)) {
            let own = MstiPriority {
                regional_root: tree.bridge_id,
                internal_cost: 0,
                designated_bridge: tree.bridge_id,
                designated_port: 0,
                receiving_port: 0,
            };
            let mut best = own;
            let mut root_port = None;
            let mut tree_hops = self.max_hops;
            for (name, p) in &self.ports {
                if !p.config.enabled || p.boundary {
                    continue;
                }
                if let Some(n) = p
                    .neighbors
                    .values()
                    .filter(|n| n.region.as_ref() == Some(&self.region))
                    .filter(|n| !same_bridge_address(n.cist.designated_bridge, self.id))
                    .filter_map(|n| n.msti.get(id))
                    .filter(|n| n.flags & 0x0c == 0x0c)
                    .min_by_key(|n| n.priority)
                {
                    if n.remaining_hops <= 1 {
                        continue;
                    }
                    let mut candidate = n.priority;
                    candidate.internal_cost = candidate
                        .internal_cost
                        .saturating_add(p.config.internal_cost);
                    if candidate < best {
                        best = candidate;
                        root_port = Some(name.clone());
                        tree_hops = n.remaining_hops - 1;
                    }
                }
            }
            if tree.msti_root != Some(best) || tree.root_port != root_port {
                for (name, state) in &mut tree.ports {
                    state.topology_change |= state.forwarding;
                    if state.forwarding {
                        state.tc_seconds = fast_tc;
                    }
                    state.learning = false;
                    state.forwarding = false;
                    state.agreed = false;
                    state.agree = false;
                    state.delay_seconds =
                        forward_delay_seconds(self.root_times, self.ports[name].send_stp);
                }
            }
            tree.msti_root = Some(best);
            tree.root_port = root_port.clone();
            if let Some(root) = root_port.as_ref() {
                if self.ports[root].config.oper_point_to_point_mac
                    && self.ports[root]
                        .neighbors
                        .values()
                        .filter(|n| n.region.as_ref() == Some(&self.region))
                        .filter_map(|n| n.msti.get(id))
                        .any(|m| m.flags & 0x0e == 0x0e)
                {
                    for (name, state) in &mut tree.ports {
                        if name != root {
                            state.sync = true;
                            state.agreed = false;
                            state.agree = false;
                            state.proposing = false;
                            state.learning = false;
                            state.forwarding = false;
                            state.delay_seconds =
                                forward_delay_seconds(self.root_times, self.ports[name].send_stp);
                            state.synced = true;
                        }
                    }
                }
            }
            for (name, p) in &self.ports {
                let cist_role = cist_snapshot.ports[name].role;
                let neighbor = p
                    .neighbors
                    .values()
                    .filter(|n| n.region.as_ref() == Some(&self.region))
                    .filter(|n| n.msti.get(id).is_some_and(|m| m.flags & 0x0c == 0x0c))
                    .min_by_key(|n| n.msti[id].priority);
                let msti_message = neighbor.map(|n| &n.msti[id]);
                let local = MstiPriority {
                    designated_bridge: tree.bridge_id,
                    designated_port: p.config.id,
                    receiving_port: p.config.id,
                    ..best
                };
                let role = if !p.config.enabled {
                    Role::Disabled
                } else if p.boundary {
                    match cist_role {
                        Role::Root => Role::Master,
                        Role::Designated => Role::Designated,
                        Role::Backup => Role::Backup,
                        Role::Alternate => Role::Alternate,
                        _ => Role::Disabled,
                    }
                } else if root_port.as_deref() == Some(name) {
                    Role::Root
                } else if msti_message.is_none_or(|n| local <= n.priority) {
                    Role::Designated
                } else if msti_message.is_some_and(|n| {
                    n.priority.designated_bridge & 0x0000_ffff_ffff_ffff
                        == self.id & 0x0000_ffff_ffff_ffff
                }) {
                    Role::Backup
                } else {
                    Role::Alternate
                };
                let synced = other_ports_synced(&tree.ports, name, &edge_ports);
                let state = tree.ports.get_mut(name).unwrap();
                let current = cist_snapshot.cist_root.unwrap();
                state.agreed = p.config.oper_point_to_point_mac
                    && p.neighbors
                        .values()
                        .filter(|n| n.region.as_ref() == Some(&self.region))
                        .any(|n| {
                            n.cist.root == current.root
                                && n.cist.external_cost == current.external_cost
                                && n.cist.regional_root == current.regional_root
                                && n.msti.get(id).is_some_and(|m| m.flags & 0x40 != 0)
                        });
                state.proposed = msti_message.is_some_and(|n| n.flags & 0x0e == 0x0e);
                let external_cist_agreement = p.config.oper_point_to_point_mac
                    && p.neighbors.values().any(|n| {
                        n.region.as_ref() != Some(&self.region) && n.cist_flags & 0x40 != 0
                    });
                let external_cist_proposal = p.neighbors.values().any(|n| {
                    n.region.as_ref() != Some(&self.region) && n.cist_flags & 0x0e == 0x0e
                });
                if p.boundary || external_cist_agreement || external_cist_proposal {
                    let cist = &cist_snapshot.ports[name];
                    state.agreed = cist.agreed && (p.boundary || external_cist_agreement);
                    state.proposing = cist.proposing;
                    state.proposed =
                        cist.proposed || (p.boundary || external_cist_proposal) && cist.agree;
                    state.agree = cist.agree && (p.boundary || external_cist_proposal);
                }
                if state.agreed {
                    state.proposing = false;
                }
                let external_cist_dispute = cist_snapshot.ports[name].disputed
                    && p.neighbors.values().any(|n| {
                        n.region.as_ref() != Some(&self.region)
                            && n.cist_flags & 0x0c == 0x0c
                            && n.cist_flags & 0x10 != 0
                    });
                state.disputed = external_cist_dispute
                    || p.neighbors
                        .values()
                        .filter(|n| n.region.as_ref() == Some(&self.region))
                        .filter_map(|n| n.msti.get(id))
                        .any(|n| {
                            n.flags & 0x0c == 0x0c && n.flags & 0x10 != 0 && n.priority > local
                        });
                state.remaining_hops = tree_hops;
                set_role_state(
                    state,
                    role,
                    p.config.admin_edge,
                    p.config.oper_point_to_point_mac,
                    synced,
                    forward_delay_seconds(self.root_times, p.send_stp),
                    tc_duration(self.root_times, p.send_stp),
                );
            }
        }
    }

    fn transmit(&mut self) -> Vec<(String, MstTx)> {
        let mut out = Vec::new();
        for (name, p) in &mut self.ports {
            if !p.config.enabled || p.hello_seconds != 0 || p.tx_count >= self.tx_hold {
                continue;
            }
            let cist = &self.trees[&Mstid(0)];
            let cist_port = &cist.ports[name];
            let mut vector = cist.cist_root.unwrap();
            vector.designated_bridge = self.id;
            vector.designated_port = p.config.id;
            vector.receiving_port = p.config.id;
            let tx = if p.send_mstp {
                MstTx::Mst(Bpdu {
                    version: 3,
                    flags: flags_for(cist_port.role, cist_port),
                    cist: vector,
                    times: self.root_times,
                    region: self.region.clone(),
                    remaining_hops: cist_port.remaining_hops,
                    msti: self
                        .trees
                        .iter()
                        .filter(|(id, _)| **id != Mstid(0))
                        .map(|(id, tree)| {
                            let state = &tree.ports[name];
                            let mut priority = tree.msti_root.unwrap();
                            priority.designated_bridge = tree.bridge_id;
                            priority.designated_port = p.config.id;
                            MstiMessage {
                                mstid: *id,
                                flags: flags_for(state.role, state)
                                    | if state.role == Role::Master { 0x80 } else { 0 },
                                priority,
                                remaining_hops: state.remaining_hops,
                            }
                        })
                        .collect(),
                })
            } else if p.send_stp {
                if cist_port.role == Role::Root && cist_port.topology_change {
                    MstTx::Legacy(rstp::Bpdu {
                        kind: rstp::Kind::Tcn,
                        version: 0,
                        flags: 0,
                        priority: rstp::Priority([0; 7]),
                        times: self.root_times,
                    })
                } else {
                    MstTx::Legacy(rstp::Bpdu {
                        kind: rstp::Kind::Config,
                        version: 0,
                        flags: u8::from(cist_port.topology_change)
                            | if p.tc_ack { 0x80 } else { 0 },
                        priority: rstp::Priority([
                            vector.root,
                            vector.external_cost as u64,
                            vector.regional_root,
                            0,
                            vector.regional_root,
                            vector.designated_port as u64,
                            vector.receiving_port as u64,
                        ]),
                        times: self.root_times,
                    })
                }
            } else {
                MstTx::Legacy(rstp::Bpdu {
                    kind: rstp::Kind::Rst,
                    version: 2,
                    flags: flags_for(cist_port.role, cist_port) & 0x7f,
                    priority: rstp::Priority([
                        vector.root,
                        vector.external_cost as u64,
                        vector.regional_root,
                        0,
                        vector.regional_root,
                        vector.designated_port as u64,
                        vector.receiving_port as u64,
                    ]),
                    times: self.root_times,
                })
            };
            out.push((name.clone(), tx));
            p.tx_count += 1;
            p.tc_ack = false;
            p.hello_seconds = ((self.times.hello as u32 + 255) / 256).max(1) as u8;
        }
        out
    }
}

fn other_ports_synced(
    ports: &BTreeMap<String, TreePortState>,
    candidate: &str,
    edge_ports: &BTreeSet<String>,
) -> bool {
    ports
        .iter()
        .all(|(name, state)| name == candidate || edge_ports.contains(name) || !state.forwarding)
}

fn set_role_state(
    state: &mut TreePortState,
    role: Role,
    edge: bool,
    p2p: bool,
    synced: bool,
    fd: u32,
    tc_duration: u32,
) {
    if state.role != role {
        let was_forwarding = state.forwarding;
        state.role = role;
        state.learning = false;
        state.forwarding = false;
        state.agree = false;
        state.proposing = false;
        state.synced = false;
        state.delay_seconds = fd;
        state.topology_change |= was_forwarding;
        if was_forwarding {
            state.tc_seconds = tc_duration;
        }
    }
    if matches!(role, Role::Disabled | Role::Alternate | Role::Backup) {
        state.learning = false;
        state.forwarding = false;
        state.proposing = false;
        state.synced = true;
        return;
    }
    if state.sync {
        if state.forwarding {
            state.learning = false;
            state.forwarding = false;
            state.delay_seconds = fd;
        }
        state.synced = true;
        state.sync = false;
    }
    if state.disputed {
        state.learning = false;
        state.forwarding = false;
        state.agreed = false;
        state.delay_seconds = fd;
        return;
    }
    if role == Role::Designated && !state.forwarding && !state.agreed && !edge && p2p {
        state.proposing = true;
    }
    // Root/Master may transition rapidly after synchronization. A designated
    // port only uses received agreement on a point-to-point MAC service.
    let rapid = edge
        || p2p
            && (state.agreed
                || matches!(role, Role::Root | Role::Master) && state.proposed && synced);
    if rapid {
        let was_forwarding = state.forwarding;
        if p2p && matches!(role, Role::Root | Role::Master) && state.proposed && synced {
            state.agree = true;
            state.proposed = false;
            state.sync = false;
        }
        state.learning = true;
        state.forwarding = true;
        if !was_forwarding && !edge {
            state.tc_seconds = tc_duration;
            state.topology_change = true;
        }
        state.delay_seconds = 0;
    } else if state.delay_seconds == 0 {
        if state.learning {
            let was_forwarding = state.forwarding;
            state.forwarding = true;
            if !was_forwarding && !edge {
                state.tc_seconds = tc_duration;
                state.topology_change = true;
            }
        } else {
            state.learning = true;
            state.delay_seconds = fd;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarding_edge_port_does_not_block_proposal_synchronization() {
        let mut ports = BTreeMap::from([
            ("root".to_string(), initial_port_state(20)),
            ("host".to_string(), initial_port_state(20)),
            ("other".to_string(), initial_port_state(20)),
        ]);
        ports.get_mut("host").unwrap().forwarding = true;
        let edge_ports = BTreeSet::from(["host".to_string()]);
        assert!(other_ports_synced(&ports, "root", &edge_ports));
        ports.get_mut("other").unwrap().forwarding = true;
        assert!(!other_ports_synced(&ports, "root", &edge_ports));
    }

    #[test]
    fn standard_digest_vectors() {
        let mut map = [0; 4096];
        assert_eq!(
            RegionId::from_vlan_map("r", 0, &map).unwrap().digest,
            [
                0xac, 0x36, 0x17, 0x7f, 0x50, 0x28, 0x3c, 0xd4, 0xb8, 0x38, 0x21, 0xd8, 0xab, 0x26,
                0xde, 0x62
            ]
        );
        for id in &mut map[1..4095] {
            *id = 1;
        }
        assert_eq!(
            RegionId::from_vlan_map("r", 0, &map).unwrap().digest,
            [
                0xe1, 0x3a, 0x80, 0xf1, 0x1e, 0xd0, 0x85, 0x6a, 0xcd, 0x4e, 0xe3, 0x47, 0x69, 0x41,
                0xc7, 0x3b
            ]
        );
        for (vid, id) in map.iter_mut().enumerate().take(4095).skip(1) {
            *id = (vid as u16 % 32) + 1;
        }
        assert_eq!(
            RegionId::from_vlan_map("r", 0, &map).unwrap().digest,
            [
                0x9d, 0x14, 0x5c, 0x26, 0x7d, 0xbe, 0x9f, 0xb5, 0xd8, 0x93, 0x44, 0x1b, 0xe3, 0xba,
                0x08, 0xce
            ]
        );
    }
    fn sample_bpdu() -> Bpdu {
        let mut map = [0; 4096];
        map[10] = 1;
        Bpdu {
            version: 3,
            flags: 0x0c,
            cist: CistPriority {
                root: 0x8000_0011_2233_4455,
                external_cost: 0x0102_0304,
                regional_root: 0x8000_0011_2233_4455,
                internal_cost: 0x0506_0708,
                designated_bridge: 0x8000_0011_2233_4455,
                designated_port: 0x8001,
                receiving_port: 0x8001,
            },
            times: rstp::Times::default(),
            region: RegionId::from_vlan_map("region", 7, &map).unwrap(),
            remaining_hops: 19,
            msti: vec![MstiMessage {
                mstid: Mstid(1),
                flags: 0x0c,
                priority: MstiPriority {
                    regional_root: 0x1001_0011_2233_4455,
                    internal_cost: 0x0102_0304,
                    designated_bridge: 0x2001_0011_2233_4455,
                    designated_port: 0x9001,
                    receiving_port: 0x8001,
                },
                remaining_hops: 18,
            }],
        }
    }
    #[test]
    fn literal_mst_bpdu_fields_and_fallback() {
        let msg = sample_bpdu();
        let wire = encode([2, 0, 0, 0, 0, 1], &msg).unwrap();
        let payload = client(&wire, rstp::DA, None, 1500).unwrap();
        assert_eq!(payload, encode_llc(&msg).unwrap());
        assert_eq!(
            decode_llc(payload, 0x8001).unwrap(),
            Received::Mst(msg.clone())
        );
        let b = &payload[3..];
        assert_eq!(&b[..5], &[0, 0, 3, 2, 0x0c]);
        assert_eq!(&b[36..38], &[0, 80]);
        assert_eq!(&b[38..45], &[0, b'r', b'e', b'g', b'i', b'o', b'n']);
        assert_eq!(&b[71..73], &[0, 7]);
        assert_eq!(&b[89..93], &[5, 6, 7, 8]);
        assert_eq!(b[101], 19);
        assert_eq!(&b[102..105], &[0x0c, 0x10, 0x01]);
        assert_eq!(&b[111..115], &[1, 2, 3, 4]);
        assert_eq!(&b[115..118], &[0x20, 0x90, 18]);
        assert_eq!(decode(&wire, 0x8001, 1500).unwrap(), Received::Mst(msg));
        let mut malformed = b.to_vec();
        malformed[36..38].copy_from_slice(&65u16.to_be_bytes());
        let payload = [vec![0x42, 0x42, 3], malformed].concat();
        let fallback_wire = frame(rstp::DA, [2, 0, 0, 0, 0, 1], payload.len() as u16, &payload);
        assert!(matches!(
            decode(&fallback_wire, 0x8001, 1500).unwrap(),
            Received::Legacy(_)
        ));
        let mut zero = sample_bpdu();
        zero.msti.clear();
        let zero_wire = encode([2, 0, 0, 0, 0, 1], &zero).unwrap();
        let zero_payload = client(&zero_wire, rstp::DA, None, 1500).unwrap();
        assert_eq!(zero_payload.len() - 3, 102);
        assert!(matches!(
            decode(&zero_wire, 0x8001, 1500).unwrap(),
            Received::Mst(_)
        ));
    }
    #[test]
    fn sixty_four_records_fit_and_sixty_five_reject() {
        let mut msg = sample_bpdu();
        msg.msti = (1..=64)
            .map(|id| MstiMessage {
                mstid: Mstid(id),
                flags: 0x0c,
                priority: MstiPriority {
                    regional_root: ((0x1000u64 | id as u64) << 48) | 0x0011_2233_4455,
                    internal_cost: 20000,
                    designated_bridge: ((0x2000u64 | id as u64) << 48) | 0x0011_2233_4455,
                    designated_port: 0x8001,
                    receiving_port: 0x8001,
                },
                remaining_hops: 20,
            })
            .collect();
        let wire = encode([2, 0, 0, 0, 0, 1], &msg).unwrap();
        let Received::Mst(decoded) = decode(&wire, 0x8001, 1500).unwrap() else {
            panic!()
        };
        assert_eq!(decoded.msti.len(), 64);
        msg.msti.push(msg.msti[63].clone());
        assert!(encode([2, 0, 0, 0, 0, 1], &msg).is_err());
    }
    #[test]
    fn shared_lan_boundary_and_distinct_instance_roots() {
        let map = {
            let mut m = [0; 4096];
            m[10] = 1;
            m[20] = 2;
            m
        };
        let region = RegionId::from_vlan_map("region", 1, &map).unwrap();
        let configs = (1..=2)
            .map(|id| {
                (
                    format!("p{id}"),
                    MstPortConfig {
                        id: 0x8000 + id,
                        external_cost: 20000,
                        internal_cost: 20000,
                        enabled: true,
                        oper_point_to_point_mac: false,
                        admin_edge: false,
                    },
                )
            })
            .collect();
        let b = MstBridge::new(
            0x8000_0011_2233_4455,
            region.clone(),
            map.to_vec(),
            vec![
                InstanceConfig {
                    mstid: Mstid(1),
                    bridge_priority: 0x8000,
                },
                InstanceConfig {
                    mstid: Mstid(2),
                    bridge_priority: 0x8000,
                },
            ],
            configs,
            rstp::Times::default(),
            6,
        )
        .unwrap();
        let mut msg = sample_bpdu();
        msg.region = region.clone();
        msg.cist.root = 0x1000_0011_2233_4444;
        msg.cist.regional_root = 0x1000_0011_2233_4444;
        msg.cist.designated_bridge = 0x1000_0011_2233_4444;
        msg.msti[0].priority.regional_root = 0x1001_0011_2233_4444;
        let d = b
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(msg.clone())),
                1000,
            )
            .unwrap();
        assert_eq!(d.state.trees[&Mstid(0)].root_port.as_deref(), Some("p1"));
        assert_eq!(d.state.trees[&Mstid(1)].root_port.as_deref(), Some("p1"));
        assert!(!d.state.trees[&Mstid(0)].ports["p1"].agreed);
        let mut second = msg.clone();
        second.cist.root = 0x2000_0011_2233_4444;
        second.cist.regional_root = 0x2000_0011_2233_4444;
        second.cist.designated_bridge = 0x2000_0011_2233_4444;
        second.msti = vec![MstiMessage {
            mstid: Mstid(2),
            flags: 0x0c,
            priority: MstiPriority {
                regional_root: 0x1002_0011_2233_4444,
                internal_cost: 0,
                designated_bridge: 0x2002_0011_2233_4444,
                designated_port: 0x8001,
                receiving_port: 0x8002,
            },
            remaining_hops: 19,
        }];
        let d = d
            .state
            .plan(MstInput::Receive("p2".into(), Received::Mst(second)), 1000)
            .unwrap();
        assert_eq!(d.state.trees[&Mstid(1)].root_port.as_deref(), Some("p1"));
        assert_eq!(d.state.trees[&Mstid(2)].root_port.as_deref(), Some("p2"));
        msg.region = RegionId::from_vlan_map("region", 2, &map).unwrap();
        msg.cist.root = 0x0000_0011_2233_4444;
        msg.cist.regional_root = 0x0000_0011_2233_4444;
        msg.cist.designated_bridge = 0x0000_0011_2233_4444;
        let d = d
            .state
            .plan(MstInput::Receive("p1".into(), Received::Mst(msg)), 1000)
            .unwrap();
        assert!(d.state.ports["p1"].boundary);
        assert_eq!(d.state.trees[&Mstid(1)].ports["p1"].role, Role::Master);
    }

    #[test]
    fn legacy_tcn_acknowledges_and_flushes_other_cist_ports() {
        let map = [0; 4096];
        let region = RegionId::from_vlan_map("region", 1, &map).unwrap();
        let ports = (1..=2)
            .map(|id| {
                (
                    format!("p{id}"),
                    MstPortConfig {
                        id: 0x8000 + id,
                        external_cost: 20000,
                        internal_cost: 20000,
                        enabled: true,
                        oper_point_to_point_mac: false,
                        admin_edge: false,
                    },
                )
            })
            .collect();
        let mut bridge = MstBridge::new(
            0x8000_0011_2233_4455,
            region,
            map.to_vec(),
            vec![],
            ports,
            rstp::Times::default(),
            6,
        )
        .unwrap();
        for _ in 0..35 {
            bridge = bridge.plan(MstInput::Tick, 1000).unwrap().state;
        }
        assert!(bridge.trees[&Mstid(0)].ports["p2"].forwarding);
        let tcn = rstp::Bpdu {
            kind: rstp::Kind::Tcn,
            version: 0,
            flags: 0,
            priority: rstp::Priority([0; 7]),
            times: rstp::Times::default(),
        };
        let delta = bridge
            .plan(MstInput::Receive("p1".into(), Received::Legacy(tcn)), 1000)
            .unwrap();
        assert!(delta.flush.contains(&(Mstid(0), "p2".into())));
        assert!(delta.state.trees[&Mstid(0)].ports["p2"].topology_change);
        assert!(delta.tx.iter().any(|(port, tx)| port == "p1" &&
            matches!(tx, MstTx::Legacy(bpdu) if bpdu.kind == rstp::Kind::Config && bpdu.flags & 0x80 != 0)));
        let mut bridge = delta.state;
        for _ in 0..3 {
            bridge = bridge.plan(MstInput::Tick, 1000).unwrap().state;
        }
        assert!(!bridge.trees[&Mstid(0)].ports["p2"].topology_change);
    }

    fn two_port_bridge(map: [u16; 4096], p2p: bool) -> MstBridge {
        let instances = map
            .iter()
            .copied()
            .filter(|id| *id != 0)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| InstanceConfig {
                mstid: Mstid(id),
                bridge_priority: 0x8000,
            })
            .collect();
        let ports = (1..=2)
            .map(|id| {
                (
                    format!("p{id}"),
                    MstPortConfig {
                        id: 0x8000 + id,
                        external_cost: 20000,
                        internal_cost: 20000,
                        enabled: true,
                        oper_point_to_point_mac: p2p,
                        admin_edge: false,
                    },
                )
            })
            .collect();
        MstBridge::new(
            0x8000_0011_2233_4455,
            RegionId::from_vlan_map("region", 1, &map).unwrap(),
            map.to_vec(),
            instances,
            ports,
            rstp::Times::default(),
            6,
        )
        .unwrap()
    }

    fn superior_message(bridge: &MstBridge) -> Bpdu {
        let root = 0x1000_0011_2233_4444;
        Bpdu {
            version: 3,
            flags: 0x0c,
            cist: CistPriority {
                root,
                external_cost: 0,
                regional_root: root,
                internal_cost: 0,
                designated_bridge: root,
                designated_port: 0x8001,
                receiving_port: 0x8001,
            },
            times: rstp::Times::default(),
            region: bridge.region.clone(),
            remaining_hops: 5,
            msti: bridge
                .trees
                .keys()
                .filter(|id| id.0 != 0)
                .map(|id| {
                    let bridge_id =
                        ((0x1000u64 | id.0 as u64) << 48) | (root & 0x0000_ffff_ffff_ffff);
                    MstiMessage {
                        mstid: *id,
                        flags: 0x0c,
                        priority: MstiPriority {
                            regional_root: bridge_id,
                            internal_cost: 0,
                            designated_bridge: bridge_id,
                            designated_port: 0x8001,
                            receiving_port: 0x8001,
                        },
                        remaining_hops: 5,
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn received_information_expires_after_three_hello_times_and_transmits_root_times() {
        let mut map = [0; 4096];
        map[10] = 1;
        let bridge = two_port_bridge(map, false);
        let mut message = superior_message(&bridge);
        message.times.age = 256;
        message.times.forward_delay = 12 * 256;
        let d = bridge
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(message.clone())),
                1000,
            )
            .unwrap();
        assert_eq!(d.state.root_hops, 4);
        let (_, MstTx::Mst(out)) = d.tx.iter().find(|(p, _)| p == "p2").unwrap() else {
            panic!()
        };
        assert_eq!(out.remaining_hops, 4);
        assert_eq!(out.msti[0].remaining_hops, 4);
        assert_eq!(out.times.age, 256);
        assert_eq!(out.times.forward_delay, 12 * 256);
        let mut state = d.state;
        for _ in 0..5 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
        }
        assert_eq!(state.trees[&Mstid(0)].root_port.as_deref(), Some("p1"));
        state = state.plan(MstInput::Tick, 1000).unwrap().state;
        assert_eq!(state.trees[&Mstid(0)].root_port, None);
        assert_eq!(state.root_hops, 20);
        message.remaining_hops = 1;
        message.msti[0].remaining_hops = 1;
        let state = state
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap()
            .state;
        assert_eq!(state.trees[&Mstid(0)].root_port, None);
        assert_eq!(state.trees[&Mstid(1)].root_port, None);
    }

    #[test]
    fn inferior_foreign_region_does_not_partition_designated_lan_but_dispute_blocks_mstis() {
        let mut map = [0; 4096];
        map[10] = 1;
        let mut bridge = two_port_bridge(map, false);
        for _ in 0..35 {
            bridge = bridge.plan(MstInput::Tick, 1000).unwrap().state;
        }
        assert!(bridge.trees[&Mstid(1)].ports["p1"].forwarding);
        let mut message = superior_message(&bridge);
        message.region.revision += 1;
        message.cist.root = 0xf000_0011_2233_4444;
        message.cist.regional_root = message.cist.root;
        message.cist.designated_bridge = message.cist.root;
        message.flags = 0x1c;
        let state = bridge
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap()
            .state;
        assert!(!state.ports["p1"].boundary);
        assert!(state.trees[&Mstid(0)].ports["p1"].disputed);
        assert!(state.trees[&Mstid(1)].ports["p1"].disputed);
        assert!(!state.trees[&Mstid(1)].ports["p1"].forwarding);
    }

    #[test]
    fn msti_root_ignores_non_designated_message() {
        let mut map = [0; 4096];
        map[10] = 1;
        let bridge = two_port_bridge(map, false);
        let mut message = superior_message(&bridge);
        message.msti[0].flags = 0x08; // Root role, not Designated.
        let state = bridge
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap()
            .state;
        assert_eq!(state.trees[&Mstid(0)].root_port.as_deref(), Some("p1"));
        assert_eq!(state.trees[&Mstid(1)].root_port, None);
    }

    #[test]
    fn point_to_point_proposal_and_agreement_are_distinct_and_shared_port_waits() {
        let map = [0; 4096];
        let a = two_port_bridge(map, true);
        let b = MstBridge::new(
            0x9000_0011_2233_4456,
            a.region.clone(),
            map.to_vec(),
            vec![],
            (1..=2)
                .map(|id| {
                    (
                        format!("p{id}"),
                        MstPortConfig {
                            id: 0x8000 + id,
                            external_cost: 20000,
                            internal_cost: 20000,
                            enabled: true,
                            oper_point_to_point_mac: true,
                            admin_edge: false,
                        },
                    )
                })
                .collect(),
            rstp::Times::default(),
            6,
        )
        .unwrap();
        let a_init = a.plan(MstInput::Initialize, 1000).unwrap();
        let (_, MstTx::Mst(proposal)) = a_init.tx.iter().find(|(p, _)| p == "p1").unwrap() else {
            panic!()
        };
        assert_eq!(proposal.flags & 0x02, 0x02);
        assert!(!a_init.state.trees[&Mstid(0)].ports["p1"].proposed);
        assert!(a_init.state.trees[&Mstid(0)].ports["p1"].proposing);
        let b_response = b
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(proposal.clone())),
                1000,
            )
            .unwrap();
        assert_eq!(
            b_response.state.trees[&Mstid(0)].ports["p1"].role,
            Role::Root
        );
        assert!(b_response.state.trees[&Mstid(0)].ports["p1"].agree);
        let (_, MstTx::Mst(agreement)) = b_response.tx.iter().find(|(p, _)| p == "p1").unwrap()
        else {
            panic!()
        };
        assert_eq!(agreement.flags & 0x40, 0x40);
        let a_response = a_init
            .state
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(agreement.clone())),
                1000,
            )
            .unwrap();
        assert!(a_response.state.trees[&Mstid(0)].ports["p1"].forwarding);
        assert!(!a_response.state.trees[&Mstid(0)].ports["p1"].proposing);
        let mut shared = b;
        shared
            .ports
            .get_mut("p1")
            .unwrap()
            .config
            .oper_point_to_point_mac = false;
        let shared = shared
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(proposal.clone())),
                1000,
            )
            .unwrap();
        assert!(!shared.state.trees[&Mstid(0)].ports["p1"].forwarding);
        assert!(!shared.state.trees[&Mstid(0)].ports["p1"].agree);
    }

    #[test]
    fn repeated_root_proposal_synchronizes_a_previously_forwarding_other_port() {
        let mut state = two_port_bridge([0; 4096], true);
        let mut message = superior_message(&state);
        message.flags = 0x0c;
        state = state
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(message.clone())),
                1000,
            )
            .unwrap()
            .state;
        for tick in 1..=35 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
            if tick % 5 == 0 {
                state = state
                    .plan(
                        MstInput::Receive("p1".into(), Received::Mst(message.clone())),
                        1000,
                    )
                    .unwrap()
                    .state;
            }
        }
        assert!(state.trees[&Mstid(0)].ports["p2"].forwarding);
        message.flags |= 0x02;
        let d = state
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap();
        assert_eq!(d.state.trees[&Mstid(0)].ports["p1"].role, Role::Root);
        assert!(d.state.trees[&Mstid(0)].ports["p1"].agree);
        assert!(!d.state.trees[&Mstid(0)].ports["p2"].forwarding);
        assert!(d.flush.contains(&(Mstid(0), "p2".into())));
        let mut state = d.state;
        let mut message = superior_message(&state);
        message.flags = 0x0c;
        let mut forwarded = false;
        for tick in 1..=35 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
            if tick % 5 == 0 {
                state = state
                    .plan(
                        MstInput::Receive("p1".into(), Received::Mst(message.clone())),
                        1000,
                    )
                    .unwrap()
                    .state;
            }
            let p2 = &state.trees[&Mstid(0)].ports["p2"];
            if forwarded {
                assert!(
                    p2.forwarding,
                    "p2 reblocked after synchronization at tick {tick}"
                );
            }
            forwarded |= p2.forwarding;
        }
        assert!(
            forwarded,
            "p2 did not recover forwarding after synchronization"
        );
    }

    #[test]
    fn legacy_migration_waits_for_rapid_receive_after_sensing() {
        let mut state = two_port_bridge([0; 4096], false);
        for _ in 0..3 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
        }
        assert_eq!(state.ports["p1"].migration_phase, MigrationPhase::Sensing);
        let mut legacy = rstp::Bpdu {
            kind: rstp::Kind::Config,
            version: 0,
            flags: 0x0c,
            priority: rstp::Priority([
                0x1000_0011_2233_4444,
                0,
                0x1000_0011_2233_4444,
                0,
                0x1000_0011_2233_4444,
                0x8001,
                0x8001,
            ]),
            times: rstp::Times::default(),
        };
        state = state
            .plan(
                MstInput::Receive("p1".into(), Received::Legacy(legacy.clone())),
                1000,
            )
            .unwrap()
            .state;
        assert_eq!(
            state.ports["p1"].migration_phase,
            MigrationPhase::SelectingStp
        );
        assert!(state.ports["p1"].send_stp);
        for _ in 0..4 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
        }
        assert_eq!(state.ports["p1"].migration_phase, MigrationPhase::Sensing);
        assert!(state.ports["p1"].send_stp);
        legacy.kind = rstp::Kind::Rst;
        legacy.version = 2;
        legacy.flags = 0x0c;
        state = state
            .plan(
                MstInput::Receive("p1".into(), Received::Legacy(legacy)),
                1000,
            )
            .unwrap()
            .state;
        assert_eq!(
            state.ports["p1"].migration_phase,
            MigrationPhase::CheckingRstp
        );
        assert!(state.ports["p1"].send_mstp);
    }

    #[test]
    fn external_tc_reaches_all_instances_and_legacy_root_transmits_tcn() {
        let mut map = [0; 4096];
        map[10] = 1;
        map[20] = 2;
        let mut state = two_port_bridge(map, false);
        for _ in 0..35 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
        }
        let mut message = superior_message(&state);
        message.region.revision += 1;
        message.flags |= 1;
        let d = state
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap();
        for id in [0, 1, 2] {
            assert!(d.flush.contains(&(Mstid(id), "p2".into())));
        }
        let mut state = two_port_bridge(map, false);
        for _ in 0..3 {
            state = state.plan(MstInput::Tick, 1000).unwrap().state;
        }
        let legacy = rstp::Bpdu {
            kind: rstp::Kind::Config,
            version: 0,
            flags: 0x0c,
            priority: rstp::Priority([
                0x1000_0011_2233_4444,
                0,
                0x1000_0011_2233_4444,
                0,
                0x1000_0011_2233_4444,
                0x8001,
                0x8001,
            ]),
            times: rstp::Times::default(),
        };
        state = state
            .plan(
                MstInput::Receive("p1".into(), Received::Legacy(legacy)),
                1000,
            )
            .unwrap()
            .state;
        assert_eq!(state.trees[&Mstid(0)].ports["p1"].role, Role::Root);
        let tcn = rstp::Bpdu {
            kind: rstp::Kind::Tcn,
            version: 0,
            flags: 0,
            priority: rstp::Priority([0; 7]),
            times: rstp::Times::default(),
        };
        let d = state
            .plan(MstInput::Receive("p2".into(), Received::Legacy(tcn)), 1000)
            .unwrap();
        let (_, MstTx::Legacy(reply)) = d.tx.iter().find(|(p, _)| p == "p1").unwrap() else {
            panic!()
        };
        assert_eq!(reply.kind, rstp::Kind::Tcn);
        assert_eq!(d.state.trees[&Mstid(0)].ports["p2"].tc_seconds, 35);
        let mut ack = rstp::Bpdu {
            kind: rstp::Kind::Config,
            version: 0,
            flags: 0x8d,
            priority: rstp::Priority([
                0x1000_0011_2233_4444,
                0,
                0x1000_0011_2233_4444,
                0,
                0x1000_0011_2233_4444,
                0x8001,
                0x8001,
            ]),
            times: rstp::Times::default(),
        };
        let acknowledged = d
            .state
            .plan(
                MstInput::Receive("p1".into(), Received::Legacy(ack.clone())),
                1000,
            )
            .unwrap();
        assert_eq!(
            acknowledged.state.trees[&Mstid(0)].ports["p1"].tc_seconds,
            0
        );
        assert!(acknowledged.flush.contains(&(Mstid(0), "p2".into())));
        assert!(!acknowledged.state.trees[&Mstid(0)].ports["p1"].topology_change);
        ack.flags = 0x0c;
        let next = acknowledged
            .state
            .plan(MstInput::Receive("p1".into(), Received::Legacy(ack)), 1000)
            .unwrap();
        assert!(!next.tx.iter().any(|(port, tx)| {
            port == "p1" && matches!(tx, MstTx::Legacy(bpdu) if bpdu.kind == rstp::Kind::Tcn)
        }));
    }

    #[test]
    fn reflected_self_advertisement_cannot_become_a_root_path() {
        let mut map = [0; 4096];
        map[10] = 1;
        let bridge = two_port_bridge(map, false);
        let d = bridge
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(superior_message(&bridge))),
                1000,
            )
            .unwrap();
        let reflected =
            d.tx.iter()
                .find_map(|(port, tx)| match tx {
                    MstTx::Mst(bpdu) if port == "p2" => Some(bpdu.clone()),
                    _ => None,
                })
                .unwrap();
        let bridge = d
            .state
            .plan(
                MstInput::Receive("p2".into(), Received::Mst(reflected)),
                1000,
            )
            .unwrap()
            .state
            .plan(MstInput::Link("p1".into(), false), 1000)
            .unwrap()
            .state;
        assert!(bridge.trees[&Mstid(0)].root_port.is_none());
        assert!(bridge.trees[&Mstid(1)].root_port.is_none());
        assert_ne!(bridge.trees[&Mstid(0)].ports["p2"].role, Role::Root);
    }

    #[test]
    fn external_root_restarts_regional_hops_after_internal_candidate() {
        let bridge = two_port_bridge([0; 4096], false);
        let internal = superior_message(&bridge);
        let mut state = bridge
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(internal.clone())),
                1000,
            )
            .unwrap()
            .state;
        assert_eq!(state.root_hops, 4);
        let mut external = internal;
        external.region.revision += 1;
        external.cist.root = 0x0000_0011_2233_4444;
        state = state
            .plan(
                MstInput::Receive("p2".into(), Received::Mst(external)),
                1000,
            )
            .unwrap()
            .state;
        assert_eq!(state.trees[&Mstid(0)].root_port.as_deref(), Some("p2"));
        assert_eq!(state.root_hops, state.max_hops);
    }

    #[test]
    fn root_timing_controls_legacy_delay_and_rapid_uses_hello() {
        let bridge = two_port_bridge([0; 4096], false);
        let mut message = superior_message(&bridge);
        message.times.forward_delay = 12 * 256;
        let rapid = bridge
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(message.clone())),
                1000,
            )
            .unwrap()
            .state;
        assert_eq!(rapid.root_times.forward_delay, 12 * 256);
        assert_eq!(rapid.trees[&Mstid(0)].ports["p2"].delay_seconds, 2);
        let mut legacy = bridge;
        legacy.ports.get_mut("p2").unwrap().send_mstp = false;
        legacy.ports.get_mut("p2").unwrap().send_stp = true;
        let legacy = legacy
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap()
            .state;
        assert_eq!(legacy.trees[&Mstid(0)].ports["p2"].delay_seconds, 12);
    }

    #[test]
    fn external_cist_proposal_and_agreement_reach_msti_master() {
        let mut map = [0; 4096];
        map[10] = 1;
        let bridge = two_port_bridge(map, true);
        let mut message = superior_message(&bridge);
        message.region.revision += 1;
        message.flags |= 2;
        let state = bridge
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(message.clone())),
                1000,
            )
            .unwrap()
            .state;
        assert!(state.trees[&Mstid(0)].ports["p1"].agree);
        assert_eq!(state.trees[&Mstid(1)].ports["p1"].role, Role::Master);
        assert!(state.trees[&Mstid(1)].ports["p1"].agree);
        assert!(state.trees[&Mstid(1)].ports["p1"].forwarding);
        message.flags = 0x4c;
        let state = state
            .plan(MstInput::Receive("p1".into(), Received::Mst(message)), 1000)
            .unwrap()
            .state;
        assert!(state.trees[&Mstid(1)].ports["p1"].agreed);
    }

    #[test]
    fn external_cist_agreement_reaches_msti_designated_port() {
        let mut map = [0; 4096];
        map[10] = 1;
        let a = two_port_bridge(map, true);
        let b = MstBridge::new(
            0x9000_0011_2233_4456,
            RegionId::from_vlan_map("other-region", 1, &map).unwrap(),
            map.to_vec(),
            vec![InstanceConfig {
                mstid: Mstid(1),
                bridge_priority: 0x8000,
            }],
            a.ports
                .iter()
                .map(|(name, port)| (name.clone(), port.config.clone()))
                .collect(),
            rstp::Times::default(),
            6,
        )
        .unwrap();
        let a_init = a.plan(MstInput::Initialize, 1000).unwrap();
        let (_, MstTx::Mst(proposal)) = a_init.tx.iter().find(|(name, _)| name == "p1").unwrap()
        else {
            panic!("A must initiate a proposal")
        };
        let b_response = b
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(proposal.clone())),
                1000,
            )
            .unwrap();
        let (_, MstTx::Mst(agreement)) =
            b_response.tx.iter().find(|(name, _)| name == "p1").unwrap()
        else {
            panic!("B must return a CIST agreement")
        };
        assert_eq!(agreement.flags & 0x48, 0x48);
        let a_response = a_init
            .state
            .plan(
                MstInput::Receive("p1".into(), Received::Mst(agreement.clone())),
                1000,
            )
            .unwrap();
        let cist = &a_response.state.trees[&Mstid(0)].ports["p1"];
        let msti = &a_response.state.trees[&Mstid(1)].ports["p1"];
        assert_eq!(cist.role, Role::Designated);
        assert!(cist.agreed && cist.forwarding);
        assert_eq!(msti.role, Role::Designated);
        assert!(msti.agreed && msti.forwarding);
    }
}
