//! Validated AXI transaction topology and workload.
#[derive(Debug, Clone)]
pub struct AxiManager {
    pub id: String,
    pub max_outstanding: u64,
    pub b_ready: String,
    pub r_ready: String,
}
#[derive(Debug, Clone)]
pub struct AxiTransaction {
    pub operation: String,
    pub address: u64,
    pub beats: u64,
    pub write_data: Vec<String>,
    pub write_strobes: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct AxiGenerator {
    pub id: String,
    pub source: usize,
    pub times_ps: Vec<u64>,
    pub transaction: AxiTransaction,
}
#[derive(Debug, Clone)]
pub struct AxiErrorRange {
    pub start: u64,
    pub end: u64,
    pub access: String,
}
#[derive(Debug, Clone)]
pub struct AxiRam {
    pub id: String,
    pub base: u64,
    pub size: u64,
    pub initial: Vec<u8>,
    pub aw_ready: String,
    pub w_ready: String,
    pub ar_ready: String,
    pub read_latency_cycles: u64,
    pub write_response_cycles: u64,
    pub error_ranges: Vec<AxiErrorRange>,
}
#[derive(Debug, Clone)]
pub struct PreparedAxi {
    pub managers: Vec<AxiManager>,
    pub interconnect: String,
    pub ram: AxiRam,
    pub clock_period_ps: u64,
    pub generators: Vec<AxiGenerator>,
}
