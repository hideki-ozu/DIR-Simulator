//! Versioned inputs for the routed AXI4 transaction/channel execution model.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FabricConfig {
    pub schema: String,
    pub version: u32,
    pub cycle_ps: u64,
    pub data_bytes: usize,
    #[serde(default = "default_write_buffer")]
    pub write_buffer_beats: usize,
    pub managers: Vec<Manager>,
    pub interconnects: Vec<String>,
    pub targets: Vec<Target>,
}
fn default_write_buffer() -> usize {
    4
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manager {
    pub id: String,
    /// Ordered path to each target; every entry names a declared interconnect.
    pub routes: std::collections::BTreeMap<String, Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub base: u64,
    pub size: u64,
    pub kind: MemoryKind,
    pub read_latency_cycles: u64,
    pub write_response_cycles: u64,
    #[serde(default)]
    pub initial: Vec<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Sram,
    Ddr,
    Shared,
    Mailbox,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FabricWorkload {
    pub schema: String,
    pub version: u32,
    pub requests: Vec<Request>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub request_id: String,
    pub source: String,
    /// DMA uses the same memory owner and visibility point, bypassing AXI channels.
    #[serde(default)]
    pub transport: Transport,
    pub id: u32,
    pub at_ps: u64,
    pub operation: Operation,
    pub address: u64,
    pub size_bytes: usize,
    pub beats: usize,
    pub burst: Burst,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default)]
    pub address_delay_cycles: u64,
    #[serde(default)]
    pub data_delay_cycles: u64,
    #[serde(default)]
    pub response_stall_cycles: u64,
    #[serde(default)]
    pub write_data: Vec<WriteBeat>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    Axi,
    Dma,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Burst {
    Fixed,
    Incr,
    Wrap,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteBeat {
    /// Actual bus lanes, not packed transfer data.
    pub data: Vec<u8>,
    pub strobe: Vec<bool>,
}

/// Address and legal active byte lanes of a beat (AXI4 unaligned first beat).
pub fn beat_lanes(request: &Request, beat: usize, width: usize) -> (u64, Vec<(usize, u64)>) {
    let size = request.size_bytes as u64;
    let aligned = request.address / size * size;
    let address = match request.burst {
        Burst::Fixed => request.address,
        Burst::Incr if beat == 0 => request.address,
        Burst::Incr => aligned + beat as u64 * size,
        Burst::Wrap => {
            let span = size * request.beats as u64;
            let base = request.address / span * span;
            base + (request.address - base + beat as u64 * size) % span
        }
    };
    let end = address / size * size + size;
    (
        address,
        (address..end)
            .map(|a| ((a % width as u64) as usize, a))
            .collect(),
    )
}
