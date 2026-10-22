//! The control protocol consumes a received waveform, including collision/retry.
use dir_simulator::{ControlStatus, RunOptions, RunSession, RunState, prepare};
use std::{fs, path::Path, process::Command};

#[test]
fn actual_shared_phy_control_fifo_has_independent_bytes_crc_and_receiver_proofs() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = repo.join("examples/network-composite/t1s-control-fifo/model.ini");
    let output = std::env::temp_dir().join(format!("dir-phy-control-{}", std::process::id()));
    fs::create_dir_all(&output).unwrap();
    let (mut session, _) =
        RunSession::new(prepare(&source).unwrap(), RunOptions::default()).unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
    let export = session.export(&output.join("result")).unwrap();
    assert_eq!(export.status, ControlStatus::Applied, "{:?}", export.error);
    let oracle = Command::new("python3")
        .arg(repo.join("docs/verification/fixtures/ieee-phy/verify_control_result.py"))
        .arg(output.join("result/results.json"))
        .output()
        .unwrap();
    assert!(
        oracle.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&oracle.stdout),
        String::from_utf8_lossy(&oracle.stderr)
    );
    fs::remove_dir_all(output).unwrap();
}
