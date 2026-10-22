//! Full-AHB2 synchronous pipeline workload; arbitration is a declared project policy.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: String,
    pub version: u32,
    pub cycle_ps: u64,
    pub data_bytes: usize,
    pub managers: Vec<Manager>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub responses: Vec<ResponseRule>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manager {
    pub id: String,
    pub priority: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub base: u64,
    pub size: u64,
    #[serde(default)]
    pub wait_cycles: u64,
    #[serde(default)]
    pub initial: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub schema: String,
    pub version: u32,
    pub requests: Vec<Request>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub request_id: String,
    pub source: String,
    pub at_ps: u64,
    pub operation: Operation,
    pub address: u64,
    pub size_bytes: usize,
    pub beats: usize,
    pub burst: Burst,
    #[serde(default)]
    pub locked: bool,
    /// Actual bus lanes; HSIZE/address determine which lanes transfer.
    #[serde(default)]
    pub write_data: Vec<Vec<u8>>,
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
    Single,
    Incr,
    Incr4,
    Incr8,
    Incr16,
    Wrap4,
    Wrap8,
    Wrap16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Response {
    Okay,
    Error,
    Retry,
    Split,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseRule {
    pub request_id: String,
    pub beat: usize,
    /// Zero-based actual attempt; unsuccessful attempts do not consume the beat.
    pub attempt: u32,
    pub response: Response,
    #[serde(default)]
    pub wait_cycles: u64,
    /// SPLIT only. HSPLIT is emitted after this many cycles from final SPLIT response.
    #[serde(default)]
    pub split_release_cycles: Option<u64>,
}
pub fn beat_address(request: &Request, beat: usize) -> u64 {
    let offset = beat as u64 * request.size_bytes as u64;
    if matches!(request.burst, Burst::Wrap4 | Burst::Wrap8 | Burst::Wrap16) {
        let span = (request.size_bytes * request.beats) as u64;
        let base = request.address / span * span;
        base + (request.address - base + offset) % span
    } else {
        request.address + offset
    }
}
