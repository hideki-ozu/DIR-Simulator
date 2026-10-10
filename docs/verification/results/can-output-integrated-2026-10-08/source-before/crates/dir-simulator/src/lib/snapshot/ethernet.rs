//! Committed Ethernet rows, including actual and separately planned boundaries.
use crate::types::ethernet::EthernetWireFrame;
#[derive(Debug, Clone)]
pub struct EthernetFrameRecord {
    pub frame_id: String,
    pub time_ps: u64,
    pub source: String,
    pub source_vlan_id: Option<u16>,
    pub wire: EthernetWireFrame,
    pub flow_id: Option<String>,
    pub priority: u8,
    pub deadline_ps: Option<u64>,
    pub generated_ps: u64,
    pub ready_ps: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct EthernetTransferRecord {
    pub transfer_id: String,
    pub time_ps: u64,
    pub frame_id: String,
    pub parent_transfer_id: Option<String>,
    pub queue_id: Option<String>,
    pub priority: u8,
    pub wire: EthernetWireFrame,
    pub vlan_id: Option<u16>,
    pub from_port: String,
    pub to_port: String,
    pub queued_ps: u64,
    pub sof_ps: Option<u64>,
    pub eof_ps: Option<u64>,
    pub release_ps: Option<u64>,
    pub arrival_ps: Option<u64>,
    pub planned_eof_ps: Option<u64>,
    pub planned_release_ps: Option<u64>,
    pub planned_arrival_ps: Option<u64>,
    pub status: String,
    pub drop_reason: Option<String>,
    pub media: Option<EthernetMediaTransfer>,
}
#[derive(Debug, Clone)]
pub struct EthernetReceptionRecord {
    pub reception_id: String,
    pub time_ps: u64,
    pub frame_id: String,
    pub transfer_id: String,
    pub device: String,
    pub ingress: String,
    pub vlan_id: Option<u16>,
    pub priority: u8,
    pub observed_ps: u64,
    pub ready_ps: Option<u64>,
    pub planned_ready_ps: Option<u64>,
    pub status: String,
    pub reason: Option<String>,
    pub egress_transfer_ids: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct EthernetSnapshot {
    pub frames: Vec<EthernetFrameRecord>,
    pub transfers: Vec<EthernetTransferRecord>,
    pub receptions: Vec<EthernetReceptionRecord>,
    pub attempts: Vec<EthernetAttemptRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct EthernetMediaTransfer {
    pub physical_link: String,
    pub attempt_count: u64,
    pub collision_count: u64,
    pub last_attempt_id: Option<String>,
    pub backoff_until_ps: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct EthernetAttemptRecord {
    pub attempt_id: String,
    pub transfer: usize,
    pub generation: u64,
    pub time_ps: u64,
    pub number: u64,
    pub sof_ps: u64,
    pub planned_eof_ps: u64,
    pub planned_release_ps: u64,
    pub planned_arrival_ps: u64,
    pub collision_ps: Option<u64>,
    pub planned_jam_start_ps: Option<u64>,
    pub planned_jam_end_ps: Option<u64>,
    pub jam_end_ps: Option<u64>,
    pub eof_ps: Option<u64>,
    pub release_ps: Option<u64>,
    pub arrival_ps: Option<u64>,
    pub backoff_slots: Option<u64>,
    pub backoff_until_ps: Option<u64>,
    pub status: String,
    pub planned_mdi_sof_ps: u64,
    pub planned_mdi_eof_ps: Option<u64>,
    pub planned_peer_mdi_sof_ps: u64,
    pub planned_peer_mdi_eof_ps: Option<u64>,
}
#[path = "ethernet/dynamic.rs"]
pub mod dynamic;
#[path = "ethernet/tsn.rs"]
pub mod tsn;
