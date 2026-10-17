//! Public exports preserve lineage and the last successful observation boundary.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Output(std::path::PathBuf);
impl Output {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-extension-publication-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn sample(name: &str, limit: Option<u64>) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut prepared =
        prepare(&root.join(format!("examples/network-extensions/{name}.ini"))).unwrap();
    if let Some(limit) = limit {
        prepared.common.max_events = limit;
    }
    let output = Output::new();
    let report = run(prepared, &output.0.join("results")).unwrap();
    assert_eq!(
        report.exit_code,
        if limit.is_some() { 3 } else { 0 },
        "{report:?}"
    );
    serde_json::from_slice(&fs::read(output.0.join("results/results.json")).unwrap()).unwrap()
}
#[test]
fn network_extension_envelopes_identify_the_original_request_and_owning_target() {
    for name in ["composite", "redundancy", "preemption"] {
        let result = sample(name, None);
        let records = result["simulation"]["model_records"].as_array().unwrap();
        let mut checked = 0;
        for record in records {
            let schema = record["schema_name"].as_str().unwrap();
            if !schema.starts_with("dir.redundancy.")
                && !schema.starts_with("dir.preemption.")
                && schema != "dir.composite.ingress"
            {
                continue;
            }
            checked += 1;
            assert_ne!(record["subject"], "@network", "{schema}");
            if schema == "dir.preemption.decision"
                && record["data"]["active_transfer_id"].is_null()
                && record["data"]["suspended_transfer_id"].is_null()
            {
                continue;
            }
            assert!(record["request_id"].is_string(), "{schema}: {record}");
            assert!(
                record["origin_request_id"].is_string(),
                "{schema}: {record}"
            );
            if schema.starts_with("dir.redundancy.") {
                assert_eq!(record["subject"], record["data"]["stream_id"]);
            }
        }
        assert!(checked > 0, "{name}");
    }
}
#[test]
fn rejected_candidate_time_does_not_extend_new_profile_busy_integrals() {
    let result = sample("preemption", Some(3));
    let simulation = &result["simulation"];
    assert_eq!(simulation["termination"], "execution_failed");
    assert_eq!(simulation["end_ps"], simulation["last_event_time_ps"]);
    let horizon = simulation["end_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    for record in simulation["model_records"].as_array().unwrap() {
        assert!(record["time_ps"].as_str().unwrap().parse::<u64>().unwrap() <= horizon);
    }
    let busy = simulation["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["metric"] == "preemption.port.busy_ps" && row["target"] == "Net.a.tx")
        .unwrap();
    assert!(busy["value"].as_str().unwrap().parse::<u64>().unwrap() <= horizon);
    // The rejected candidate is diagnosed separately in diagnostics.jsonl;
    // its timestamp is deliberately absent from successful result records.
}

#[test]
fn wire_export_validates_the_host_ordering_field_before_domain_projection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut prepared = prepare(&root.join("examples/ethernet-wire/active.ini")).unwrap();
    prepared.common.time_limit_ps = 0;
    let mut snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
    assert!(
        !snapshot.common.partial,
        "{:?}",
        snapshot.common.diagnostics
    );
    let key = snapshot
        .registered
        .as_ref()
        .unwrap()
        .model_records
        .iter()
        .find(|(_, row)| row.schema.name == "ethernet.control.wire.event")
        .map(|(key, _)| key.clone())
        .unwrap();
    let original =
        snapshot.registered.as_ref().unwrap().model_records[&key].data["effect_seq"].clone();
    let valid = Output::new();
    dir_simulator::output::export(&prepared, &snapshot, &valid.0).unwrap();
    assert!(valid.0.join("manifest.json").exists());
    for invalid in [
        None,
        Some(Value::Null),
        Some(serde_json::json!(0)),
        Some(serde_json::json!("01")),
        Some(serde_json::json!("18446744073709551616")),
    ] {
        let fields = snapshot
            .registered
            .as_mut()
            .unwrap()
            .model_records
            .get_mut(&key)
            .unwrap()
            .data
            .as_object_mut()
            .unwrap();
        if let Some(invalid) = invalid {
            fields.insert("effect_seq".into(), invalid);
        } else {
            fields.remove("effect_seq");
        }
        let output = Output::new();
        let error = dir_simulator::output::export(&prepared, &snapshot, &output.0).unwrap_err();
        assert!(
            error.message.contains("wire carrier effect_seq"),
            "{error:?}"
        );
        assert!(!output.0.join("manifest.json").exists());
        snapshot
            .registered
            .as_mut()
            .unwrap()
            .model_records
            .get_mut(&key)
            .unwrap()
            .data["effect_seq"] = original.clone();
    }
}
