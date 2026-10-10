//! Prepared private coordinator payloads for the composed network profiles.
use super::ethernet::{PreparedEthernet, dynamic::PreparedDynamicEthernet};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PreparedNetwork {
    pub ethernet: PreparedEthernet,
    pub dynamic: Option<PreparedDynamicEthernet>,
    pub tsn: Option<super::ethernet::tsn::PreparedTsn>,
    pub bridge: Option<super::can_ethernet::PreparedCanEthernet>,
    pub config: Value,
    pub workload: Value,
}
