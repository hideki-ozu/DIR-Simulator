//! Real SoC phase and memory/IPC continuations through the public controller.
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
            "dir-soc-memory-control-{}-{}",
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
const CASES: &[&str] = &[
    "soc/soc-round-robin",
    "soc/ahb-wait-error",
    "soc/noc-xy",
    "memory-ipc/sram-ports",
];
fn fixture(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures")
        .join(format!("{case}.ini"))
}
#[test]
fn soc_and_memory_sessions_pause_each_phase_and_resume_to_canonical_results() {
    for case in CASES {
        let temp = Temp::new();
        let prepared = Arc::new(prepare(&fixture(case)).unwrap());
        assert_eq!(
            run((*prepared).clone(), &temp.0.join("ordinary"))
                .unwrap()
                .exit_code,
            0
        );
        let (mut session, controller) = RunSession::new(prepared, RunOptions::default()).unwrap();
        controller.submit(ControlCommand::Pause).unwrap();
        session.advance_one().unwrap();
        assert_eq!(session.state(), RunState::Paused);
        assert_eq!(session.snapshot().common.committed_events, 0);
        session.export(&temp.0.join("before-init")).unwrap();
        session.resume(session.pause_token().unwrap()).unwrap();
        session.advance_one().unwrap();
        assert_eq!(session.snapshot().common.committed_events, 0);
        for ordinal in 1..=3 {
            session.advance_one().unwrap();
            assert_eq!(
                session.snapshot().common.committed_events,
                ordinal,
                "{case}"
            );
            controller.submit(ControlCommand::Pause).unwrap();
            session.poll_control().unwrap();
            assert_eq!(session.state(), RunState::Paused);
            let name = format!("phase-{ordinal}");
            session.export(&temp.0.join(&name)).unwrap();
            session.poll_control().unwrap();
            session.advance_one().unwrap();
            assert_eq!(
                session.snapshot().common.committed_events,
                ordinal,
                "{case}"
            );
            let output = temp.read(&name);
            assert_eq!(output["schema_name"], "dir.controlled.legacy");
            assert_eq!(output["control"]["commit_ordinal"], ordinal.to_string());
            session.resume(session.pause_token().unwrap()).unwrap();
        }
        for _ in 0..100_000 {
            let before = session.snapshot().common.committed_events;
            let state = session.advance_one().unwrap();
            let after = session.snapshot().common.committed_events;
            assert!(after == before || after == before + 1);
            if state != RunState::Running {
                break;
            }
        }
        assert_eq!(
            session.state(),
            RunState::Completed,
            "{case}: {:?}",
            session.snapshot().common.diagnostics
        );
        session.export(&temp.0.join("completed")).unwrap();
        assert_eq!(
            temp.read("completed")["result"]["simulation"],
            temp.read("ordinary")["simulation"],
            "{case}"
        );
    }
}
#[test]
fn soc_and_memory_horizon_zero_and_unsupported_checkpoint_are_explicit() {
    for case in CASES {
        let mut prepared = prepare(&fixture(case)).unwrap();
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
        .expect("legacy checkpoint has no codec");
        assert_eq!(error.code, "UnsupportedCapability");
        let (mut session, _) = RunSession::new(prepared, RunOptions::default()).unwrap();
        session.advance_one().unwrap();
        session.advance_one().unwrap();
        assert_eq!(session.state(), RunState::Completed, "{case}");
        assert_eq!(session.snapshot().common.committed_events, 0);
    }
}

#[test]
fn soc_and_memory_stop_and_failure_keep_the_last_successful_boundary() {
    for case in CASES {
        let temp = Temp::new();
        let prepared = Arc::new(prepare(&fixture(case)).unwrap());
        let (mut session, controller) = RunSession::new(prepared, RunOptions::default()).unwrap();
        session.advance_one().unwrap();
        for _ in 0..3 {
            session.advance_one().unwrap();
        }
        let committed = session.snapshot().common.committed_events;
        let horizon = session.snapshot().common.last_event_time_ps.unwrap_or(0);
        controller.submit(ControlCommand::Stop).unwrap();
        session.poll_control().unwrap();
        assert_eq!(session.state(), RunState::Stopped);
        session.advance_one().unwrap();
        session.export(&temp.0.join("stopped")).unwrap();
        assert_eq!(session.snapshot().common.committed_events, committed);
        let output = temp.read("stopped");
        assert_eq!(output["control"]["state"], "Stopped");
        assert_eq!(output["control"]["horizon_ps"], horizon.to_string());
        assert_eq!(output["control"]["commit_ordinal"], committed.to_string());
        let mut prepared = prepare(&fixture(case)).unwrap();
        let maximum = if case.contains("noc") { 2 } else { 1 };
        prepared.common.max_events = maximum;
        let (mut session, _) = RunSession::new(Arc::new(prepared), RunOptions::default()).unwrap();
        for _ in 0..10 {
            if session.advance_one().unwrap() == RunState::Failed {
                break;
            }
        }
        assert_eq!(session.state(), RunState::Failed, "{case}");
        assert_eq!(session.snapshot().common.committed_events, maximum);
        let horizon = session.snapshot().common.last_event_time_ps.unwrap_or(0);
        session.export(&temp.0.join("failed")).unwrap();
        let output = temp.read("failed");
        assert_eq!(output["control"]["state"], "Failed");
        assert_eq!(output["control"]["horizon_ps"], horizon.to_string());
        assert_eq!(output["control"]["commit_ordinal"], maximum.to_string());
    }
}
