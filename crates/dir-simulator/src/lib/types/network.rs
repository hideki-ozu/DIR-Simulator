//! Prepared private coordinator payloads for the composed network profiles.
use super::ethernet::{PreparedEthernet, dynamic::PreparedDynamicEthernet};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PreparedNetwork {
    pub ethernet: PreparedEthernet,
    pub dynamic: Option<PreparedDynamicEthernet>,
    pub tsn: Option<super::ethernet::tsn::PreparedTsn>,
    pub bridge: Option<super::can_ethernet::PreparedCanEthernet>,
    pub extensions: Option<PreparedNetworkExtensions>,
    pub wire: Option<super::wire::PreparedWire>,
    pub config: Value,
    pub workload: Value,
}

#[derive(Debug, Clone)]
pub struct NetworkLimits {
    pub max_pending_branches: u32,
    pub max_control_events: u32,
}
#[derive(Debug, Clone)]
pub struct PreparedNetworkExtensions {
    pub limits: NetworkLimits,
    pub replication: Option<PreparedReplication>,
    pub preemption: Option<PreparedPreemption>,
}
#[derive(Debug, Clone)]
pub struct PreparedReplication {
    pub streams: Vec<ReplicationStream>,
    pub generator_stream: std::collections::BTreeMap<String, usize>,
    pub shared_ports: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct ReplicationStream {
    pub stream_id: String,
    pub source_generator: String,
    pub replicate_at: String,
    pub eliminate_at: String,
    pub history_capacity: usize,
    pub initial_history_words: Vec<u64>,
    pub paths: Vec<ReplicationPath>,
}
#[derive(Debug, Clone)]
pub struct ReplicationPath {
    pub id: String,
    pub ports: Vec<String>,
    pub directions: Vec<usize>,
}
#[derive(Debug, Clone)]
pub struct PreparedPreemption {
    /// Same index as PreparedEthernet::directions.
    pub outputs: Vec<PreemptionOutput>,
}
#[derive(Debug, Clone)]
pub struct PreemptionOutput {
    pub port: String,
    pub express_mask: u8,
    pub preemptable_mask: u8,
    pub fragment_payload_bytes: u64,
    pub fragment_overhead_bytes: u64,
    pub resume_gap_ps: u64,
}
