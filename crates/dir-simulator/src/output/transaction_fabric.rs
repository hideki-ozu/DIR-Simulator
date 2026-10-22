//! Prefix-safe inspection helpers for registered fabric output.
use crate::registry::{ModelRecord, RegisteredSnapshot};
pub fn channel_records(snapshot: &RegisteredSnapshot) -> Vec<&ModelRecord> {
    let mut records: Vec<_> = snapshot
        .model_records
        .values()
        .filter(|r| r.schema.name == "fabric.Record" && r.data["kind"] == "channel")
        .collect();
    records.sort_by_key(|r| {
        (
            r.time_ps,
            r.id.strip_prefix("event-")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0),
        )
    });
    records
}
