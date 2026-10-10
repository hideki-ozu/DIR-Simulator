//! Committed AXI journal. RAM is retained once, never copied per event.
#[derive(Debug, Clone)]
pub struct AxiRequest {
    pub request_id: String,
    pub generator: usize,
    pub ordinal: u64,
    pub time_ps: u64,
    pub generated_ps: u64,
    pub eligible_ps: u64,
    pub status: String,
    pub grant_ps: Option<u64>,
    pub completed_ps: Option<u64>,
    pub response: Option<String>,
    pub read_data: Vec<String>,
    pub drop_reason: Option<String>,
}
#[derive(Debug, Clone)]
pub struct AxiHandshake {
    pub request: usize,
    pub channel: String,
    pub beat: Option<u64>,
    pub time_ps: u64,
    pub valid_since_ps: u64,
    pub address: Option<u64>,
    pub data_hex: Option<String>,
    pub wstrb: Option<u8>,
    pub last: Option<bool>,
    pub response: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct AxiSnapshot {
    pub requests: Vec<AxiRequest>,
    pub handshakes: Vec<AxiHandshake>,
    pub memory: Vec<u8>,
    pub memory_time_ps: u64,
}
