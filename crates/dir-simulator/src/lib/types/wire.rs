//! Prepared wire adapters. Raw data payloads remain owned by Ethernet.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeDomain {
    Customer,
    Provider,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ComponentPortKey {
    pub component: String,
    pub port: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortAttachment {
    Physical {
        output: String,
        ingress: String,
        direction: usize,
    },
    Internal {
        iss_id: String,
        peer: ComponentPortKey,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentTreeBinding {
    pub component: String,
    pub device: String,
    pub domain: TreeDomain,
    pub ports: BTreeMap<String, PortAttachment>,
    /// Number of distinct customer interfaces attached to each PEP service.
    pub service_cardinality: BTreeMap<String, usize>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireProtocol {
    MacControl,
    Igmp,
    Mld,
    Rstp,
    Mstp,
    Mvrp,
}
impl WireProtocol {
    pub fn revision(self) -> &'static str {
        match self {
            Self::MacControl => "ieee802.3-2022.annex31b",
            Self::Igmp => "igmp-rfc9776-2025",
            Self::Mld => "mld-rfc9777-2025",
            Self::Rstp => "ieee802.1q-2022.rstp.initial.v1",
            Self::Mstp => "ieee802.1q-2022.mstp.initial.v1",
            Self::Mvrp => "ieee802.1q-2022.mvrp.initial.v1",
        }
    }
    pub fn active(self) -> bool {
        matches!(self, Self::Rstp | Self::Mstp | Self::Mvrp)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WirePort {
    pub output: String,
    pub ingress: String,
    pub bridge: String,
    pub pvid: u16,
    pub direction: usize,
    pub bitrate_bps: u64,
    pub source_mac: Option<[u8; 6]>,
    pub static_vlans: BTreeMap<u16, bool>,
    pub allowed_vlans: BTreeMap<u16, bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MembershipConfig {
    pub max_sources_per_record: usize,
    pub max_groups: usize,
    pub max_filter_leases: usize,
    pub max_sources_total: usize,
    pub membership_interval_ps: u64,
    pub router_interval_ps: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RstpPortConfig {
    pub port: String,
    pub port_id: u16,
    pub external_path_cost: u32,
    pub admin_edge: bool,
    pub auto_edge: bool,
    pub auto_isolate: bool,
    pub restricted_role: bool,
    pub restricted_tcn: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RstpConfig {
    pub bridge_id: u64,
    pub max_age: u16,
    pub forward_delay: u16,
    pub tx_hold_count: u32,
    pub ports: Vec<RstpPortConfig>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MstpInstanceConfig {
    pub mstid: u16,
    pub bridge_priority: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MstpPortConfig {
    pub port: String,
    pub port_id: u16,
    pub external_path_cost: u32,
    pub internal_path_cost: u32,
    pub admin_edge: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MstpConfig {
    pub bridge_id: u64,
    pub max_age: u16,
    pub forward_delay: u16,
    pub tx_hold_count: u32,
    pub max_hops: u8,
    pub region_name: String,
    pub region_revision: u16,
    pub vlan_map: Vec<u16>,
    pub instances: Vec<MstpInstanceConfig>,
    pub ports: Vec<MstpPortConfig>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireRegistrarMode {
    Normal,
    FixedIgnoreNew,
    FixedPropagateNew,
    Forbidden,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApplicantMode {
    Normal,
    NewOnly,
    NonParticipant,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MvrpPortConfig {
    pub port: String,
    pub join_time_ps: u64,
    pub leave_time_ps: u64,
    pub leaveall_time_ps: u64,
    pub periodic_enabled: bool,
    pub applicant_control: WireApplicantMode,
    pub restricted_vlan_registration: bool,
    pub transmit_zero: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MvrpConfig {
    pub max_attributes_per_pdu: usize,
    pub ports: Vec<MvrpPortConfig>,
    pub registrar_controls: BTreeMap<(String, u16), WireRegistrarMode>,
    pub local_declarations: BTreeSet<(String, u16)>,
    pub random_seed: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireOptions {
    Membership(MembershipConfig),
    Rstp(RstpConfig),
    Mstp(MstpConfig),
    Mvrp(MvrpConfig),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWireAdapter {
    pub id: String,
    pub protocol: WireProtocol,
    pub bridge: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_binding: Option<ComponentTreeBinding>,
    pub ports: Vec<String>,
    pub max_pdu_bytes: usize,
    pub max_state_entries: usize,
    pub max_pending_timers: usize,
    pub max_pending_tx: usize,
    pub max_control_steps: usize,
    pub options: WireOptions,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryPlan {
    pub start_ps: u64,
    pub arrival_ps: u64,
    pub release_ps: u64,
    pub mac_bytes: u64,
    pub bitrate_bps: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWirePacket {
    pub id: String,
    pub adapter_id: String,
    pub ingress_port: String,
    pub output_port: String,
    pub arrival_ps: u64,
    pub bytes: Vec<u8>,
    pub vid: Option<u16>,
    pub source_mac: Option<[u8; 6]>,
    pub destination_mac: Option<[u8; 6]>,
    pub boundary: Option<BoundaryPlan>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedWire {
    pub adapters: Vec<PreparedWireAdapter>,
    pub ports: BTreeMap<String, WirePort>,
    pub packets: Vec<PreparedWirePacket>,
    pub svre_exists: BTreeSet<(String, u16)>,
    pub registrar_modes: BTreeMap<(String, u16), WireRegistrarMode>,
}
impl PreparedWire {
    pub fn owner(&self, protocol: WireProtocol, port: &str) -> Option<&PreparedWireAdapter> {
        self.adapters.iter().find(|a| {
            a.protocol == protocol
                && a.ports.iter().any(|p| {
                    p == port
                        || self
                            .ports
                            .get(p)
                            .is_some_and(|physical| physical.ingress == port)
                })
        })
    }
    pub fn adapter(&self, id: &str) -> Option<&PreparedWireAdapter> {
        self.adapters.iter().find(|a| a.id == id)
    }
}
