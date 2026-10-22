//! Explicit IEEE 802.3-2022 Clause 99 adapter preparation; independent of DIR's abstract preemption.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedMacMerge {
    pub standard: String,
    pub ports: Vec<Option<MergePort>>,
    pub reverse: Vec<usize>,
    pub participating: Vec<bool>,
    pub lldp_interval_ps: u64,
    pub lldp_ttl_seconds: u16,
    pub max_rx_mac_octets: usize,
    pub max_pending_packets: usize,
    pub max_accounting_events: usize,
    pub max_accounting_octets: usize,
    pub faults: Vec<MergeFault>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergePort {
    pub source_mac: [u8; 6],
    pub express_mask: u8,
    pub preemptable_mask: u8,
    pub advertised_add_frag_size: u8,
    pub verify_time_ms: u8,
    pub admin_enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeFault {
    pub direction: usize,
    pub packet_ordinal: u64,
    pub action: MergeFaultAction,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum MergeFaultAction {
    Drop,
    Corrupt { octet: usize, xor: u8 },
    Truncate { octets: usize },
}
