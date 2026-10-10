use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-event-timing-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn report(config: &Path, output: &Path, expected_exit: i32) -> Value {
    let result = Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(["run", "--config"])
        .arg(config)
        .arg("--output")
        .arg(output)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(expected_exit), "{result:?}");
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn cli_reports_event_loop_time_for_complete_and_partial_runs() {
    let temp = Temp::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixtures = root.join("docs/verification/fixtures/can");
    let complete = temp.0.join("complete");
    let successful = report(&fixtures.join("competition.ini"), &complete, 0);
    let seconds = successful["event_processing_wall_seconds"]
        .as_f64()
        .unwrap();
    assert!(seconds >= 0.0 && seconds.is_finite());
    let persisted: Value =
        serde_json::from_slice(&fs::read(complete.join("results.json")).unwrap()).unwrap();
    assert!(persisted.get("event_processing_wall_seconds").is_none());
    assert!(
        persisted["simulation"]
            .get("event_processing_wall_seconds")
            .is_none()
    );

    let limited_config = temp.0.join("limited.ini");
    fs::write(
        &limited_config,
        format!(
            "[General]\nnetwork = demo.Main\nned-path = \"{}/models\"\nsim-time-limit = 300us\nMain.bus.bitrate = 500kbps\nworkload = \"{}/competition.json\"\nmax-events = 1\n",
            fixtures.display(),
            fixtures.display()
        ),
    )
    .unwrap();
    let partial = report(&limited_config, &temp.0.join("partial"), 3);
    assert_eq!(partial["termination"], "execution_failed");
    let seconds = partial["event_processing_wall_seconds"].as_f64().unwrap();
    assert!(seconds >= 0.0 && seconds.is_finite());

    let prep_failed = report(&temp.0.join("missing.ini"), &temp.0.join("preparation"), 2);
    assert_eq!(prep_failed["termination"], "prep_failed");
    assert!(prep_failed["event_processing_wall_seconds"].is_null());
}
