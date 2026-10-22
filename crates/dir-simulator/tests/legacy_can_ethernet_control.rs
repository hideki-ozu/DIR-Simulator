//! Public cooperative-controller checks against the ordinary family exporters.
use dir_simulator::runtime::session::{ControlCommand, RunOptions, RunSession, RunState};
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-legacy-can-ethernet-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn read(&self, name: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.0.join(name).join("results.json")).unwrap())
            .unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}
const CASES: &[&str] = &[
    "examples/can/contention.ini",
    "examples/canfd/precomputed.ini",
    "examples/canfd/iso.ini",
    "docs/verification/fixtures/axi/read-write.ini",
    "examples/gateway/fanout.ini",
    "examples/ethernet/unicast.ini",
    "examples/ethernet/qos-priority.ini",
    "examples/ethernet/vlan-unicast.ini",
    "docs/verification/fixtures/ethernet-media/collision.ini",
    "examples/ethernet/media/100base-t1.ini",
];

#[test]
fn legacy_profiles_pause_without_callbacks_and_resume_to_ordinary_results() {
    for relative in CASES {
        let temp = Temp::new();
        let prepared = Arc::new(prepare(&fixture(relative)).unwrap());
        assert_eq!(
            run((*prepared).clone(), &temp.0.join("ordinary"))
                .unwrap()
                .exit_code,
            0,
            "{relative}"
        );
        let (mut session, controller) = RunSession::new(prepared, RunOptions::default()).unwrap();
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.advance_one().unwrap(), RunState::Paused);
        assert_eq!(session.snapshot().common.committed_events, 0);
        assert!(session.snapshot().common.points.is_empty());
        session.export(&temp.0.join("before-init")).unwrap();
        session.resume(session.pause_token().unwrap()).unwrap();
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
        assert_eq!(
            session.snapshot().common.committed_events,
            0,
            "initialization: {relative}"
        );
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
        assert_eq!(
            session.snapshot().common.committed_events,
            1,
            "first callback: {relative}"
        );
        controller.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        assert_eq!(session.state(), RunState::Paused);
        let committed = session.snapshot().common.committed_events;
        for name in ["paused-a", "paused-b"] {
            session.export(&temp.0.join(name)).unwrap();
            session.poll_control().unwrap();
            session.advance_one().unwrap();
            assert_eq!(
                session.snapshot().common.committed_events,
                committed,
                "{relative}"
            );
        }
        let a = temp.read("paused-a");
        let b = temp.read("paused-b");
        assert_eq!(a["schema_name"], "dir.controlled.legacy");
        assert_eq!(a["schema_version"], 4);
        assert_eq!(
            a["result"]["simulation"], b["result"]["simulation"],
            "repeated prefix: {relative}"
        );
        session.resume(session.pause_token().unwrap()).unwrap();
        for _ in 0..100_000 {
            let before = session.snapshot().common.committed_events;
            let state = session.advance_one().unwrap();
            let after = session.snapshot().common.committed_events;
            assert!(after == before || after == before + 1, "{relative}");
            if state != RunState::Running {
                break;
            }
        }
        assert_eq!(
            session.state(),
            RunState::Completed,
            "{relative}: {:?}",
            session.snapshot().common.diagnostics
        );
        session.export(&temp.0.join("completed")).unwrap();
        assert_eq!(
            temp.read("completed")["result"]["simulation"],
            temp.read("ordinary")["simulation"],
            "resumed canonical simulation: {relative}"
        );
    }
}

#[test]
fn legacy_checkpoint_remains_unsupported_and_zero_horizon_never_executes() {
    for relative in CASES {
        let mut prepared = prepare(&fixture(relative)).unwrap();
        prepared.common.time_limit_ps = 0;
        let prepared = Arc::new(prepared);
        let error = RunSession::new(
            prepared.clone(),
            RunOptions {
                checkpoint_enabled: true,
                ..RunOptions::default()
            },
        )
        .err()
        .expect("legacy codec is unsupported");
        assert_eq!(error.code, "UnsupportedCapability", "{relative}");
        let (mut session, _) = RunSession::new(prepared, RunOptions::default()).unwrap();
        session.advance_one().unwrap();
        session.advance_one().unwrap();
        assert_eq!(session.state(), RunState::Completed, "{relative}");
        assert_eq!(session.snapshot().common.committed_events, 0, "{relative}");
    }
}

#[test]
fn legacy_fixture_exports() {
    let Some(root) = std::env::var_os("DIR_LEGACY_VIEWER_FIXTURES").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir(&root).unwrap();
    let all = CASES.iter().copied().chain([
        "docs/verification/fixtures/soc/soc-round-robin.ini",
        "docs/verification/fixtures/soc/ahb-wait-error.ini",
        "docs/verification/fixtures/soc/noc-xy.ini",
        "docs/verification/fixtures/memory-ipc/sram-ports.ini",
    ]);
    for relative in all {
        let prepared = Arc::new(prepare(&fixture(relative)).unwrap());
        let profile = prepared.common.profile.clone();
        let target = root.join(&profile);
        std::fs::create_dir(&target).unwrap();
        let (mut session, controller) = RunSession::new(prepared, RunOptions::default()).unwrap();
        controller.submit(ControlCommand::Pause).unwrap();
        session.advance_one().unwrap();
        session.export(&target.join("before-init")).unwrap();
        session.resume(session.pause_token().unwrap()).unwrap();
        session.advance_one().unwrap();
        for _ in 0..4 {
            if session.advance_one().unwrap() != RunState::Running {
                break;
            }
        }
        controller.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        session.export(&target.join("prefix")).unwrap();
        if session.state() == RunState::Paused {
            session.resume(session.pause_token().unwrap()).unwrap();
        }
        session.run_until_yield().unwrap();
        assert_eq!(session.state(), RunState::Completed, "{profile}");
        session.export(&target.join("completed")).unwrap();
    }
}
