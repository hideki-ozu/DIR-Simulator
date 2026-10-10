// Compile the production CLI source against this test-instrumented library.
// This is an in-process source harness, not an instrumented shipped binary.
use crate as dir_simulator;
include!("main.rs");

#[test]
fn cli_validate_does_not_enter_runtime_and_run_positive_control_does() {
    use crate::runtime::test_probe::{self, Counts};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "dir-cli-runtime-boundary-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir(&scratch.0).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/can/competition.ini");
    let argv = |command: &str, config: &Path, output: Option<&Path>| {
        let mut args = vec![
            command.to_owned(),
            "--config".into(),
            config.to_str().unwrap().to_owned(),
        ];
        if let Some(output) = output {
            args.extend(["--output".into(), output.to_str().unwrap().to_owned()]);
        }
        args
    };

    test_probe::reset();
    assert_eq!(execute(&argv("validate", &fixture, None)).unwrap(), 0);
    assert_eq!(test_probe::read(), Counts::default());

    test_probe::reset();
    let missing = execute(&argv("validate", &scratch.0.join("missing.ini"), None));
    assert_eq!(missing.unwrap_err().code, "E-0001");
    assert_eq!(test_probe::read(), Counts::default());

    test_probe::reset();
    let output = scratch.0.join("run");
    assert_eq!(execute(&argv("run", &fixture, Some(&output))).unwrap(), 0);
    let counts = test_probe::read();
    assert_eq!(counts.facade, 1);
    assert_eq!(counts.builtin, 1);
    assert_eq!(counts.can_engine, 1);
    let results: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    let events = results["simulation"]["committed_events"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(events > 0);
    assert_eq!(counts.can_event_callbacks, events);
    let requests = results["simulation"]["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["request_id"], "a:0");
    assert_eq!(requests[1]["request_id"], "b:0");
}
