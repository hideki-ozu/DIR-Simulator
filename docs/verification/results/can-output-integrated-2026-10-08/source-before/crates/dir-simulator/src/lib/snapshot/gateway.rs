//! Gateway forwarding records and lineage, keyed by stable CAN request ID.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestLineage {
    pub origin_request_id: String,
    pub parent_request_id: Option<String>,
    pub gw_hops: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForwardRecord {
    pub rx_buffer: usize,
    pub forward_id: String,
    pub parent_request: usize,
    pub gateway: usize,
    pub ingress: usize,
    pub egress: Option<usize>,
    pub route_id: Option<String>,
    pub gw_hops: u32,
    pub received_ps: u64,
    pub planned_forward_ps: Option<u64>,
    pub forwarded_ps: Option<u64>,
    pub child_request_id: Option<String>,
    pub status: String,
    pub reason: Option<String>,
}

/// One ingress slot shared by all routed copies of a received frame.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RxBufferRecord {
    pub buffer_id: String,
    pub parent_request: usize,
    pub gateway: usize,
    pub ingress: usize,
    pub capacity: u64,
    pub received_ps: u64,
    pub released_ps: Option<u64>,
    pub status: String,
    pub reason: Option<String>,
    pub egress: Vec<usize>,
    pub remaining_egress: BTreeSet<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct GatewaySnapshot {
    pub forwards: Vec<ForwardRecord>,
    pub rx_buffers: Vec<RxBufferRecord>,
    pub request_lineage: BTreeMap<String, RequestLineage>,
}
