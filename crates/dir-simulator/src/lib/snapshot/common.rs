//! Model-independent committed observations and termination state.
use crate::types::Diagnostic;

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
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommonSnapshot {
    pub termination: String,
    pub partial: bool,
    pub end_ps: u64,
    pub last_event_time_ps: Option<u64>,
    pub committed_events: u64,
    pub pending_events: u64,
    pub points: Vec<Point>,
    pub diagnostics: Vec<Diagnostic>,
}
