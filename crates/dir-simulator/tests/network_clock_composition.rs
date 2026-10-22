//! Independent physical-credit calculation with actual clock and management callbacks.
use dir_simulator::registry::Registry;
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint,
};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/clock-cbs/model.ini")
}
fn scratch() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "dir-clock-cbs-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn value(s: &mut RunSession, root: &std::path::Path) -> Value {
    assert_eq!(s.export(root).unwrap().status, ControlStatus::Applied);
    serde_json::from_slice(&fs::read(root.join("results.json")).unwrap()).unwrap()
}
fn assert_trace(v: &Value) {
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let mut sof: Vec<u64> = rows
        .iter()
        .filter(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer" && r["data"]["from_port"] == "Net.a.tx"
        })
        .map(|r| {
            r["data"]["sof_ps"]
                .as_str()
                .expect("all four actual transfers must start")
                .parse()
                .unwrap()
        })
        .collect();
    sof.sort_unstable();
    assert_eq!(
        sof,
        [0, 3_500_000, 5_876_000, 12_564_000],
        "physical bit/s integral, clock steps and local GCL guard"
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["status"] == "received")
            .count(),
        4
    );
    for at in [400_000, 500_000, 1_500_000, 3_000_000] {
        let at_text = at.to_string();
        assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.clock"
            && r["data"]["cause"] == "step"
            && r["time_ps"].as_str() == Some(at_text.as_str())));
    }
}
#[test]
fn local_rate_steps_and_global_gcl_preserve_physical_cbs_credit() {
    let root = scratch();
    let (mut session, _) =
        RunSession::new(prepare(&fixture()).unwrap(), RunOptions::default()).unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
    assert_trace(&value(&mut session, &root.join("result")));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn clock_cbs_source_prefix_restores_busy_and_stepped_gate_states() {
    for cut in [400_000, 3_000_000, 5_000_000, 6_000_000] {
        let root = scratch();
        let (mut session, controller) = RunSession::new(
            prepare(&fixture()).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        while session.snapshot().common.last_event_time_ps.unwrap_or(0) < cut {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        assert_eq!(
            session
                .checkpoint(&root.join("bundle"), CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut restored, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        session.resume(session.pause_token().unwrap()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        let a = value(&mut session, &root.join("a"));
        let b = value(&mut restored, &root.join("b"));
        assert_trace(&a);
        assert_trace(&b);
        assert_eq!(
            a["simulation"]["model_records"],
            b["simulation"]["model_records"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
