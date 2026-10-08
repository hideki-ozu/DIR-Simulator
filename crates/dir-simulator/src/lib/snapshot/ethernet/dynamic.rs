//! Policy records are owned values staged with the same commit as policy changes.
use serde::Serialize;
use serde_json::Value;
#[derive(Debug, Clone, Serialize)]
pub struct DynamicControlRecord {
    #[serde(skip)]
    pub synthetic: bool,
    pub control_id: String,
    pub kind: String,
    pub scheduled_ps: u64,
    pub applied_ps: Option<u64>,
    pub batch_ordinal: u64,
    pub epoch_before: u64,
    pub epoch_after: u64,
    pub generation: Option<u64>,
    pub outcome: String,
    pub key: Value,
    pub before: Value,
    pub after: Value,
}
#[derive(Debug, Clone, Serialize)]
pub struct DynamicPolicyRecord {
    pub time_ps: u64,
    pub policy_epoch: u64,
    pub topology_generation: u64,
    pub initial: bool,
    pub changes: Vec<Value>,
}
