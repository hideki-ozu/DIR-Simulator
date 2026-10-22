//! Composite timeline records, ordered across CPU, directory and physical NoC effects.
use crate::registry::{ModelRecord, RegisteredSnapshot};
pub fn execution_records(s: &RegisteredSnapshot) -> Vec<&ModelRecord> {
    let mut r: Vec<_> = s
        .model_records
        .values()
        .filter(|r| {
            matches!(
                r.schema.name.as_str(),
                "dir.soc.ExecutionRecord"
                    | "dir.noc.FlitRecord"
                    | "fabric.Record"
                    | "ahb.pipeline.Record"
            )
        })
        .collect();
    r.sort_by_key(|r| {
        (
            r.time_ps,
            r.data["effect_seq"]
                .as_u64()
                .or_else(|| r.data["effect_seq"].as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(u64::MAX),
        )
    });
    r
}
