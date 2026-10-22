//! Actual transition records, never synthesized from planned packet completion.
use crate::registry::{ModelRecord, RegisteredSnapshot};
pub fn flit_records(snapshot: &RegisteredSnapshot) -> Vec<&ModelRecord> {
    let mut records: Vec<_> = snapshot
        .model_records
        .values()
        .filter(|r| r.schema.name == "dir.noc.FlitRecord")
        .collect();
    records.sort_by_key(|r| (r.time_ps, r.data["effect_seq"].as_u64().unwrap_or(0)));
    records
}
