//! Prepared, bounded IEEE 802.1AS-2020 packet profile.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedGptp {
    pub nodes: Vec<GptpNode>,
    /// Indexed by the physical Ethernet direction. Every enabled direction
    /// has its reverse direction enabled as well.
    pub ports: Vec<Option<GptpPort>>,
    pub reverse: Vec<usize>,
    pub sync_interval_ps: u64,
    pub announce_interval_ps: u64,
    pub pdelay_interval_ps: u64,
    pub announce_receipt_timeout: u32,
    pub sync_receipt_timeout: u32,
    pub allowed_lost_responses: u32,
    pub mean_link_delay_threshold_ps: u64,
    pub priority: u8,
    pub max_pending_exchanges: usize,
    pub max_journal_entries: usize,
    pub log_sync_interval: i8,
    pub log_announce_interval: i8,
    pub log_pdelay_req_interval: i8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GptpNode {
    pub device: usize,
    pub clock_identity: [u8; 8],
    pub priority1: u8,
    pub clock_class: u8,
    pub clock_accuracy: u8,
    pub offset_scaled_log_variance: u16,
    pub priority2: u8,
    pub time_source: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GptpPort {
    pub node: usize,
    pub port_number: u16,
    pub source_mac: [u8; 6],
}
