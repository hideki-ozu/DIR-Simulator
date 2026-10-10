//! AC0008-PATH-01..04 compare observables, not the implementation call path.
//! Path evidence is maintained independently in the reviewer matrix.
use dir_simulator::{prepare, prepare_with_registry, registry::Registry, run, run_config};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Output(PathBuf);
impl Output {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-ac0008-path-{}-{}",
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
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/can/competition.ini")
}
fn projection(destination: &Path) -> Value {
    let result: Value =
        serde_json::from_slice(&fs::read(destination.join("results.json")).unwrap()).unwrap();
    assert!(result["simulation"].is_object());
    // Metadata contains run ID, wall time and absolute paths; it is not compared.
    // JSON arrays remain in their recorded order, including events and records.
    result["simulation"].clone()
}
fn plain_reference(destination: &Path) -> Value {
    let prepared = prepare(&fixture()).unwrap();
    assert!(prepared.registered.is_none());
    let report = run(prepared, destination).unwrap();
    assert_eq!(report.exit_code, 0);
    projection(destination)
}
#[test]
fn classical_can_cli_run_matches_plain_observable_projection() {
    let output = Output::new();
    let expected = plain_reference(&output.0.join("reference"));
    let destination = output.0.join("cli");
    let execution = Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .arg("run")
        .arg("--config")
        .arg(fixture())
        .arg("--output")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(execution.status.success(), "{}", String::from_utf8_lossy(&execution.stderr));
    assert_eq!(projection(&destination), expected);
}
#[test]
fn classical_can_run_config_matches_plain_observable_projection() {
    let output = Output::new();
    let expected = plain_reference(&output.0.join("reference"));
    let destination = output.0.join("run-config");
    assert_eq!(run_config(&fixture(), &destination).unwrap().exit_code, 0);
    assert_eq!(projection(&destination), expected);
}
#[test]
fn classical_can_prepare_and_run_preserve_analytic_competition() {
    let output = Output::new();
    let prepared = prepare(&fixture()).unwrap();
    assert!(prepared.registered.is_none());
    let snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
    // Independent 500kbps CAN expectation, not copied from the reference run.
    let mut sent: Vec<_> = snapshot.can.requests.iter().collect();
    sent.sort_by_key(|r| r.sof_ps);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].request_id, "a:0");
    assert_eq!(sent[1].request_id, "b:0");
    assert_eq!(sent[0].sof_ps, Some(0));
    assert_eq!(sent[1].sof_ps, Some(106_000_000));
    assert_eq!(sent[0].eof_ps, Some(100_000_000));
    assert_eq!(sent[1].eof_ps, Some(200_000_000));
    let destination = output.0.join("prepare-run");
    assert_eq!(run(prepared, &destination).unwrap().exit_code, 0);
    let expected = plain_reference(&output.0.join("reference"));
    assert_eq!(projection(&destination), expected);
}
#[test]
fn classical_can_prepare_with_registry_and_run_uses_builtin_adapter() {
    let output = Output::new();
    let expected = plain_reference(&output.0.join("reference"));
    let prepared = prepare_with_registry(&fixture(), Registry::default()).unwrap();
    assert!(!prepared.registered.as_ref().unwrap().is_generic());
    let destination = output.0.join("registry-run");
    assert_eq!(run(prepared, &destination).unwrap().exit_code, 0);
    assert_eq!(projection(&destination), expected);
}
