//! Wire records distinguish planned media, successful protocol changes and actual TX.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WirePortPolicy {
    pub role: String,
    pub learning: bool,
    pub forwarding: bool,
    pub link_up: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireTimerKey {
    pub protocol: String,
    pub bridge: String,
    pub port: String,
    pub slot: String,
    pub attribute: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireTimerRecord {
    pub key: WireTimerKey,
    pub token: String,
    pub deadline_ps: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireEventRecord {
    pub schema_version: u32,
    pub adapter_id: String,
    pub protocol: String,
    pub protocol_revision: String,
    pub bridge: String,
    pub port: String,
    pub event_id: String,
    pub cause_id: Option<String>,
    pub event_kind: String,
    pub time_ps: String,
    pub phase: u8,
    pub batch_ordinal: String,
    pub packet_id: Option<String>,
    pub bytes_hex: Option<String>,
    pub mac_bytes: Option<u32>,
    pub outcome: String,
    pub reason: Option<String>,
    pub policy_epoch_before: String,
    pub policy_epoch_after: String,
    pub protocol_generation_before: String,
    pub protocol_generation_after: String,
    pub timer: Option<WireTimerRecord>,
    pub effects: Vec<Value>,
    pub external_injection: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireTransitionRecord {
    pub schema_version: u32,
    pub adapter_id: String,
    pub bridge: String,
    pub port: Option<String>,
    pub protocol: String,
    pub time_ps: String,
    pub callback_ordinal: String,
    pub transition_ordinal: String,
    pub machine: String,
    pub from_state: String,
    pub to_state: String,
    pub trigger: String,
    pub changes: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireBoundaryRecord {
    pub effect_seq: String,
    pub packet_id: String,
    pub ingress_port: String,
    pub time_ps: String,
    pub event: String,
    pub status: String,
    pub actual_start_ps: Option<String>,
    pub actual_arrival_ps: Option<String>,
    pub actual_release_ps: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WirePolicyRecord {
    pub schema_version: u32,
    pub time_ps: String,
    pub policy_epoch: String,
    pub topology_generation: String,
    pub ports: BTreeMap<String, WirePortPolicy>,
    pub registered: Vec<Value>,
    pub mac: Vec<Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMembershipRecord {
    pub effect_seq: String,
    pub packet_id: Option<String>,
    pub protocol_revision: String,
    pub role: String,
    pub switch: String,
    pub ingress_port: String,
    pub vid: u16,
    pub family: String,
    pub time_ps: String,
    pub epoch_before: String,
    pub epoch_after: String,
    pub outcome: String,
    pub reason: Option<String>,
    pub message_kind: String,
    pub byte_length: u32,
    pub record_count: u32,
    pub query: Option<Value>,
    pub compatibility_flood: bool,
    pub changes: Vec<Value>,
    pub forwarded_ports: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMembershipTimerRecord {
    pub effect_seq: String,
    pub switch: String,
    pub time_ps: String,
    pub scheduled_ps: String,
    pub wake_generation: String,
    pub outcome: String,
    pub epoch_before: String,
    pub epoch_after: String,
    pub expired_leases: u32,
    pub affected_partitions: u32,
    pub projected_controls: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMacKey {
    pub switch: String,
    pub vid: u16,
    pub mac: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireMacEntry {
    pub switch: String,
    pub vid: u16,
    pub mac: String,
    pub port: String,
    pub expires_at_ps: String,
    pub generation: String,
}
