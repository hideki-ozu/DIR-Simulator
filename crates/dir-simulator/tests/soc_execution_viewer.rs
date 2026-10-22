//! Export real paused production prefixes for the standalone Viewer.
use dir_simulator::{prepare, runtime::session::*};
use serde_json::Value;
#[test]
fn paused_soc_and_ddr_exports_have_exact_control_horizon_and_sequence() {
    let supplied = std::env::var_os("DIR_SOC_VIEWER_FIXTURES");
    let root = supplied
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("dir-soc-viewer-prefix-{}", std::process::id()))
        });
    std::fs::create_dir(&root).unwrap();
    for name in ["soc-execution", "ddr", "mailbox"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../examples/soc-execution/{name}.ini"));
        let (mut session, controller) =
            RunSession::new(prepare(&path).unwrap(), RunOptions::default()).unwrap();
        for _ in 0..50 {
            session.advance_one().unwrap();
        }
        controller.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        assert_eq!(session.state(), RunState::Paused);
        let dir = root.join(name);
        session.export(&dir).unwrap();
        let raw: Value =
            serde_json::from_slice(&std::fs::read(dir.join("results.json")).unwrap()).unwrap();
        assert_eq!(raw["schema_version"], 3);
        assert_eq!(raw["control"]["state"], "Paused");
        let h = raw["simulation"]["end_ps"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_eq!(raw["control"]["horizon_ps"], raw["simulation"]["end_ps"]);
        assert_eq!(
            raw["control"]["commit_ordinal"],
            raw["simulation"]["committed_events"]
        );
        let rows = raw["simulation"]["model_records"].as_array().unwrap();
        assert!(!rows.is_empty());
        for row in rows {
            assert!(row["time_ps"].as_str().unwrap().parse::<u64>().unwrap() <= h);
            assert!(
                row["data"]["effect_seq"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .is_ok()
            );
        }
    }
    if supplied.is_none() {
        std::fs::remove_dir_all(root).unwrap();
    }
}
