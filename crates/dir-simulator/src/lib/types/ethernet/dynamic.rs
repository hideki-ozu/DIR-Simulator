//! Immutable inputs and shared policy vocabulary for abstract dynamic Ethernet.
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IpFamily {
    Ipv4,
    Ipv6,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IpMulticast {
    pub family: IpFamily,
    pub source: IpAddr,
    pub group: IpAddr,
}
#[derive(Debug, Clone, Serialize)]
pub struct DynamicLink {
    pub id: String,
    pub ports: [String; 2],
    pub up: bool,
    pub cost: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct DynamicLimits {
    pub mac_entries: usize,
    pub membership_entries: usize,
    pub sources_per_entry: usize,
    pub registrations: usize,
    pub control_events: usize,
    pub pending_timers: usize,
    pub visits_per_frame: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    Include,
    Exclude,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MacKey {
    pub switch: String,
    pub vid: u16,
    pub mac: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct MembershipKey {
    pub switch: String,
    pub vid: u16,
    pub family: IpFamily,
    pub group: IpAddr,
    pub port: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RouterKey {
    pub switch: String,
    pub vid: u16,
    pub family: IpFamily,
    pub port: String,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RegistrationKey {
    pub port: String,
    pub vid: u16,
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlOp {
    MembershipSet {
        key: MembershipKey,
        mode: FilterMode,
        sources: BTreeSet<IpAddr>,
        expires_at: u64,
    },
    MembershipLeave {
        key: MembershipKey,
    },
    RouterSet {
        key: RouterKey,
        expires_at: u64,
    },
    RouterLeave {
        key: RouterKey,
    },
    VlanRegister {
        key: RegistrationKey,
        expires_at: u64,
    },
    VlanUnregister {
        key: RegistrationKey,
    },
    LinkSet {
        link: String,
        up: bool,
    },
}
impl ControlOp {
    pub fn subphase(&self) -> u8 {
        if matches!(self, Self::LinkSet { .. }) {
            0
        } else {
            1
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MembershipSet { .. } => "membership_set",
            Self::MembershipLeave { .. } => "membership_leave",
            Self::RouterSet { .. } => "router_set",
            Self::RouterLeave { .. } => "router_leave",
            Self::VlanRegister { .. } => "vlan_register",
            Self::VlanUnregister { .. } => "vlan_unregister",
            Self::LinkSet { .. } => "link_set",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct DynamicControl {
    pub id: String,
    pub at_ps: u64,
    pub input_index: usize,
    pub op: ControlOp,
}
#[derive(Debug, Clone)]
pub struct PreparedDynamicEthernet {
    pub mac_age_ps: u64,
    pub convergence_ps: u64,
    pub bridges: BTreeMap<String, u64>,
    pub links: Vec<DynamicLink>,
    pub registrable: BTreeSet<RegistrationKey>,
    pub limits: DynamicLimits,
    pub controls: Vec<DynamicControl>,
    pub generator_ip: BTreeMap<String, Option<IpMulticast>>,
    pub mac_keys: BTreeSet<MacKey>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lease<T> {
    pub expires_at: u64,
    pub generation: u64,
    pub value: T,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFilter {
    pub mode: FilterMode,
    pub sources: BTreeSet<IpAddr>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortRole {
    Root,
    Designated,
    Alternate,
    Disabled,
    Converging,
}
impl PortRole {
    pub fn forwarding(self) -> bool {
        matches!(self, Self::Root | Self::Designated)
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct PolicySnapshot {
    pub policy_epoch: u64,
    pub topology_generation: u64,
    pub roles: BTreeMap<String, PortRole>,
    pub link_up: BTreeMap<String, bool>,
    pub effective_vlans: BTreeMap<String, BTreeMap<u16, bool>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ineligible {
    LinkDown,
    StpDiscarding,
    VlanUnregistered,
}
impl Ineligible {
    pub fn reason(self) -> &'static str {
        match self {
            Self::LinkDown => "link_down",
            Self::StpDiscarding => "stp_discarding",
            Self::VlanUnregistered => "vlan_unregistered",
        }
    }
}
#[derive(Debug, Clone)]
pub struct EgressSelection {
    pub ports: Vec<String>,
    pub reason: Option<String>,
}
