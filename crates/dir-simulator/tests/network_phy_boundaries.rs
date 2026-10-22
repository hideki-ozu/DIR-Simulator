//! Named BoundaryV2: actual observable effects, elapsed prefixes and resource edges.
use dir_simulator::{ControlStatus, RunOptions, RunSession, RunState, prepare};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite/t1s-multidrop")
}
fn input(lookahead: u32, maximum: u64, horizon: u64, traffic: bool, trace: bool) -> PathBuf {
    let source = fixture();
    let root = std::env::temp_dir().join(format!(
        "dir-boundary-v2-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let mut config: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    config["phy_attachments"][0]["lookahead_samples"] = lookahead.into();
    config["phy_attachments"][0]["medium"]["maximum_samples"] = maximum.into();
    config["phy_attachments"][0]["medium"]["trace_samples"] = trace.into();
    // Exercise the deterministic noise generator without obscuring the signal.
    config["phy_attachments"][0]["medium"]["channel"]["noise_rms_uv"] =
        serde_json::json!([1, 1, 1]);
    fs::write(root.join("model.json"), config.to_string()).unwrap();
    let mut work: Value =
        serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
    if !traffic {
        work["generators"] = serde_json::json!([]);
    }
    fs::write(root.join("workload.json"), work.to_string()).unwrap();
    let ini = fs::read_to_string(source.join("model.ini"))
        .unwrap()
        .replace(
            "ned-path = \"models\"",
            &format!("ned-path = \"{}\"", source.join("models").display()),
        )
        .replace(
            "sim-time-limit = 80000000ps",
            &format!("sim-time-limit = {horizon}ps"),
        );
    fs::write(root.join("model.ini"), ini).unwrap();
    root
}
fn run(root: &std::path::Path) -> Value {
    let (mut session, _) = RunSession::new(
        prepare(&root.join("model.ini")).unwrap(),
        RunOptions::default(),
    )
    .unwrap();
    let state = session.run_until_yield().unwrap();
    assert!(matches!(state, RunState::Completed | RunState::Failed));
    let export = session.export(&root.join("result")).unwrap();
    assert_eq!(export.status, ControlStatus::Applied, "{:?}", export.error);
    serde_json::from_slice(&fs::read(root.join("result/results.json")).unwrap()).unwrap()
}
fn rows(value: &Value) -> Value {
    value["simulation"]["model_records"].clone()
}
fn physical(value: &Value, kind: &str) -> Vec<Value> {
    value["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["schema_name"] == "dir.network.phy" && row["data"]["kind"] == kind)
        .cloned()
        .collect()
}

#[test]
fn scalar_and_batched_named_policy_match_actual_receive_with_noise() {
    let scalar = input(1, 40000, 80000000, true, false);
    let batched = input(4096, 40000, 80000000, true, false);
    let a = run(&scalar);
    let b = run(&batched);
    assert_eq!(a["simulation"]["termination"], "time_limit");
    assert_eq!(b["simulation"]["termination"], "time_limit");
    assert_eq!(physical(&a, "decoded_ingress").len(), 2);
    assert_eq!(
        rows(&a),
        rows(&b),
        "FES counts differ, all actual semantic records must agree"
    );
    for root in [scalar, batched] {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn trace_exposes_every_sample_and_remains_scalar_equivalent() {
    let scalar = input(1, 100, 25000, false, true);
    let batched = input(4096, 100, 25000, false, true);
    let a = run(&scalar);
    let b = run(&batched);
    assert_eq!(rows(&a), rows(&b));
    assert!(
        !physical(&a, "Samples").is_empty(),
        "trace must prevent invisible sample batching"
    );
    for root in [scalar, batched] {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn final_permitted_sample_commits_and_exhaustion_obeys_exclusive_horizon() {
    // 400 MHz grid: completion(index)=index*2500ps. 9 is the last
    // permitted processed sample count; the tenth attempt is at25000ps.
    for lookahead in [1, 4096] {
        let at = input(lookahead, 9, 25000, false, true);
        let after = input(lookahead, 9, 25001, false, true);
        let a = run(&at);
        let b = run(&after);
        assert_eq!(a["simulation"]["termination"], "time_limit");
        assert_ne!(b["simulation"]["termination"], "time_limit");
        assert_eq!(physical(&a, "Samples").len(), 9);
        assert_eq!(
            physical(&b, "Samples").len(),
            9,
            "failure must retain last valid sample"
        );
        assert_eq!(
            rows(&a),
            rows(&b),
            "failed next attempt must have no committed physical effect"
        );
        for root in [at, after] {
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn actual_rx_on_last_permitted_sample_is_retained() {
    // Latest of the two actual receivers completes at62320000ps (index24928).
    let root = input(4096, 24928, 62322501, true, false);
    let value = run(&root);
    let ingress = physical(&value, "decoded_ingress");
    assert_eq!(
        ingress.len(),
        2,
        "last valid decode must commit before next sample failure"
    );
    assert!(ingress.iter().any(|r| r["time_ps"] == "62320000"));
    assert_ne!(value["simulation"]["termination"], "time_limit");
    fs::remove_dir_all(root).unwrap();
}
