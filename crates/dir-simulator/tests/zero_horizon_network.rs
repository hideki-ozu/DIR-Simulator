//! A normal empty observation interval excludes initial network queue points.
use dir_simulator::{
    ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare, run, runtime,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const CONFIGS: [&str; 7] = [
    "examples/ethernet/dynamic/unicast.ini",
    "examples/ethernet/tsn/tas.ini",
    "examples/can-ethernet/r01.ini",
    "examples/network-extensions/composite.ini",
    "examples/network-extensions/redundancy.ini",
    "examples/network-extensions/preemption.ini",
    "examples/ethernet-wire/active.ini",
];
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "dir-zero-horizon-network-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn result(&self) -> Value {
        serde_json::from_slice(&fs::read(self.0.join("results.json")).unwrap()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn prepared(config: &str) -> dir_simulator::PreparedSimulation {
    prepare(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(config),
    )
    .unwrap()
}

#[test]
fn normal_zero_horizon_has_no_metric_records_in_every_network_profile() {
    for config in CONFIGS {
        let mut p = prepared(config);
        p.common.time_limit_ps = 0;
        let output = Temp::new();
        assert_eq!(run(p, &output.0).unwrap().exit_code, 0, "{config}");
        let result = output.result();
        let simulation = &result["simulation"];
        assert_eq!(simulation["end_ps"], "0", "{config}");
        assert_eq!(simulation["partial"], false, "{config}");
        assert_eq!(simulation["termination"], "time_limit", "{config}");
        assert_eq!(simulation["committed_events"], "0", "{config}");
        assert_eq!(simulation["records"], serde_json::json!([]), "{config}");
    }
}

#[test]
fn positive_horizon_keeps_initial_queue_points_in_every_network_profile() {
    for config in CONFIGS {
        let mut p = prepared(config);
        p.common.time_limit_ps = 1;
        let snapshot = runtime::simulate(&p).unwrap();
        assert!(!snapshot.common.partial, "{config}");
        let initial: Vec<_> = snapshot
            .common
            .points
            .iter()
            .filter(|point| point.event_seq.is_none())
            .collect();
        assert!(!initial.is_empty(), "{config}");
        assert!(
            initial.iter().all(|point| {
                point.time_ps == 0
                    && point.value == 0
                    && matches!(point.metric.as_str(), "queue_length" | "queue_bytes")
            }),
            "{config}"
        );
    }
}

#[test]
fn failed_zero_time_prefix_keeps_successfully_initialized_queue_points() {
    let mut p = prepared(CONFIGS[3]);
    // Reject the first event after initialization has committed successfully.
    p.common.max_events = 0;
    let output = Temp::new();
    assert_ne!(run(p, &output.0).unwrap().exit_code, 0);
    let result = output.result();
    let simulation = &result["simulation"];
    assert_eq!(simulation["end_ps"], "0");
    assert_eq!(simulation["partial"], true);
    assert_eq!(simulation["committed_events"], "0");
    let records = simulation["records"].as_array().unwrap();
    assert!(!records.is_empty());
    assert!(records.iter().all(|point| {
        point["time_ps"] == "0"
            && point["event_seq"].is_null()
            && matches!(
                point["metric"].as_str(),
                Some("queue_length" | "queue_bytes")
            )
    }));
}

#[test]
fn paused_zero_time_export_retains_initial_points_and_resume_completes() {
    let mut p = prepared(CONFIGS[3]);
    p.common.time_limit_ps = 1;
    let (mut session, controller) = RunSession::new(p, RunOptions::default()).unwrap();
    assert_eq!(session.advance_one().unwrap(), RunState::Running);
    let points = session.snapshot().common.points.clone();
    assert!(!points.is_empty());
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let output = Temp::new();
    assert_eq!(
        session.export(&output.0).unwrap().status,
        ControlStatus::Applied
    );
    let result = output.result();
    assert_eq!(result["simulation"]["end_ps"], "0");
    assert_eq!(result["simulation"]["termination"], "paused");
    assert_eq!(result["simulation"]["partial"], true);
    assert_eq!(
        result["simulation"]["records"].as_array().unwrap().len(),
        points.len()
    );
    assert_eq!(session.snapshot().common.points.len(), points.len());
    session.resume(session.pause_token().unwrap()).unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
}
