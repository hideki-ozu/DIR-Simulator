use crate::types::Diagnostic;

/// Committed CAN request state. Native integers are converted to decimal strings by the exporter.
#[derive(Debug, Clone)]
pub struct Request {
    pub request_id: String,
    pub source: String,
    pub bus: String,
    pub status: String,
    pub generated_ps: u64,
    pub ready_ps: Option<u64>,
    pub sof_ps: Option<u64>,
    pub eof_ps: Option<u64>,
    pub planned_eof_ps: Option<u64>,
    pub planned_release_ps: Option<u64>,
    pub release_ps: Option<u64>,
    pub payload_bits: u64,
    pub frame_bits: u64,
    pub crc15: u16,
    pub stuff_bits: u64,
    pub bitrate_bps: u64,
}

#[derive(Debug, Clone)]
pub struct Receiver {
    pub request_id: String,
    pub receiver: String,
    pub status: String,
    pub observed_ps: Option<u64>,
    pub received_ps: Option<u64>,
}

/// Queue changes and latency observations in successful callback order.
#[derive(Debug, Clone)]
pub struct Point {
    pub event_seq: Option<u64>,
    pub effect_seq: Option<u64>,
    pub time_ps: u64,
    pub target: String,
    pub metric: String,
    pub value: u64,
    pub request_id: Option<String>,
    pub receiver: Option<String>,
}

#[derive(Debug)]
pub struct Snapshot {
    pub termination: String,
    pub partial: bool,
    pub end_ps: u64,
    pub last_event_time_ps: Option<u64>,
    pub committed_events: u64,
    pub pending_events: u64,
    pub bus_state: String,
    pub requests: Vec<Request>,
    pub receivers: Vec<Receiver>,
    pub points: Vec<Point>,
    pub diagnostics: Vec<Diagnostic>,
}
