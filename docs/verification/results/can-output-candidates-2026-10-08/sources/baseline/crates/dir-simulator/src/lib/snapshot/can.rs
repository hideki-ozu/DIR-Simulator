//! Committed Classical CAN results; runtime queues and wire buffers stay private.
/// Committed CAN request state. Native integers are rendered by the exporter.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub request_id: String,
    pub source: String,
    pub bus: String,
    pub status: String,
    pub generated_ps: u64,
    pub ready_ps: Option<u64>,
    /// Actual TX queue admission; may follow ready_ps for a buffered GW copy.
    pub tx_enqueued_ps: Option<u64>,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Receiver {
    pub request_id: String,
    pub receiver: String,
    pub status: String,
    pub observed_ps: Option<u64>,
    pub received_ps: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct CanSnapshot {
    /// Completed ledger rows live in this owned journal during streamed runs.
    pub archive: Option<std::sync::Arc<crate::runtime::can_archive::CanArchive>>,
    pub bus_state: String,
    pub bus_states: Vec<String>,
    pub requests: Vec<Request>,
    pub receivers: Vec<Receiver>,
}
