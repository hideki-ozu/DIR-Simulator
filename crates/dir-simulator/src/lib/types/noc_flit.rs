//! Project-defined finite FIFO wormhole network (not a hardware compliance claim).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlitConfig {
    pub schema: String,
    pub version: u32,
    pub topology: TopologyConfig,
    pub flit_bytes: usize,
    pub buffer_flits: usize,
    pub adaptive_vcs: usize,
    pub cycle_ps: u64,
    pub link_latency_cycles: u64,
    pub credit_latency_cycles: u64,
    pub escape_after_stalls: u64,
    pub ejection_capacity_flits: usize,
    pub ejection_service_cycles: u64,
    #[serde(default = "pending_default")]
    pub pending_packets: usize,
}
fn pending_default() -> usize {
    4
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyConfig {
    pub kind: TopologyKind,
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopologyKind {
    Mesh,
    Torus,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlitWorkload {
    pub schema: String,
    pub version: u32,
    #[serde(default)]
    pub packets: Vec<Packet>,
    #[serde(default)]
    pub faults: Vec<Fault>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub id: String,
    pub at_ps: u64,
    pub source: usize,
    pub destination: usize,
    pub payload: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub id: String,
    pub at_ps: u64,
    pub link_id: usize,
    pub up: bool,
}
impl Packet {
    pub fn flits(&self, bytes: usize) -> usize {
        self.payload.len().div_ceil(bytes).max(1)
    }
    pub fn bytes(&self, index: usize, bytes: usize) -> &[u8] {
        let start = (index * bytes).min(self.payload.len());
        &self.payload[start..(start + bytes).min(self.payload.len())]
    }
}
