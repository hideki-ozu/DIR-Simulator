//! CAN FD committed rows; planned boundaries never imply committed completion.
#[derive(Debug, Clone)]
pub struct CanFdRequest {
    pub request_id: String,
    pub generator: usize,
    pub ordinal: u64,
    pub time_ps: u64,
    pub generated_ps: u64,
    pub planned_ready_ps: u64,
    pub ready_ps: Option<u64>,
    pub sof_ps: Option<u64>,
    pub eof_ps: Option<u64>,
    pub release_ps: Option<u64>,
    pub planned_eof_ps: Option<u64>,
    pub planned_release_ps: Option<u64>,
    pub state: String,
    pub drop_reason: Option<String>,
}
#[derive(Debug, Clone)]
pub struct CanFdReception {
    pub request: usize,
    pub receiver: usize,
    pub time_ps: u64,
    pub planned_arrival_ps: u64,
    pub planned_completed_ps: u64,
    pub arrival_ps: Option<u64>,
    pub completed_ps: Option<u64>,
    pub state: String,
}
#[derive(Debug, Clone, Default)]
pub struct CanFdSnapshot {
    pub requests: Vec<CanFdRequest>,
    pub receptions: Vec<CanFdReception>,
}
