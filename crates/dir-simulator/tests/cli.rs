use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-cli-{}-{}",
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
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(args)
        .output()
        .unwrap()
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn help_version_argument_errors_and_validate() {
    assert!(cli(&["--help"]).status.success());
    assert!(cli(&["run", "--help"]).status.success());
    assert_eq!(
        String::from_utf8(cli(&["--version"]).stdout).unwrap(),
        "dir-simulator 0.1.0\n"
    );
    for args in [
        vec![],
        vec!["bad"],
        vec!["validate"],
        vec!["validate", "--config"],
        vec!["validate", "--config", "a", "--config", "b"],
        vec!["run", "--config", "a"],
        vec!["--version", "--help"],
        vec!["validate", "--config=a"],
    ] {
        let out = cli(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(json(&out.stderr)["code"], "E-0001");
    }
    let out = cli(&[
        "validate",
        "--config",
        text(&root().join("examples/can/baseline.ini")),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json(&out.stdout)["node_count"], "4");
    assert_eq!(json(&out.stdout)["channel_count"], "6");
}

#[test]
fn validate_counts_simple_instances_without_gateway_compounds() {
    for (config, nodes, channels) in [
        ("examples/can/baseline.ini", "4", "6"),
        ("examples/gateway/fanout.ini", "9", "12"),
    ] {
        let out = cli(&["validate", "--config", text(&root().join(config))]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(json(&out.stdout)["node_count"], nodes, "{config}");
        assert_eq!(json(&out.stdout)["channel_count"], channels, "{config}");
    }
}

#[test]
fn run_publishes_results_and_refuses_to_overwrite() {
    let temp = Temp::new();
    let destination = temp.0.join("results");
    let config = root().join("docs/verification/fixtures/can/competition.ini");
    let args = [
        "run",
        "--config",
        text(&config),
        "--output",
        text(&destination),
    ];
    let out = cli(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    assert_eq!(json(&out.stdout)["termination"], "events_exhausted");
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 5);
    let before = fs::read(destination.join("results.json")).unwrap();
    let results = json(&before);
    assert_eq!(results["simulation"]["requests"][0]["eof_ps"], "100000000");
    let out = cli(&args);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out.stderr)["code"], "E-0003");
    assert_eq!(before, fs::read(destination.join("results.json")).unwrap());
}

#[test]
fn execution_limit_exports_consistent_partial_results() {
    let temp = Temp::new();
    let config = temp.0.join("limited.ini");
    let fixtures = root().join("docs/verification/fixtures/can");
    fs::write(&config, format!("[General]\nnetwork = demo.Main\nned-path = \"{}/models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\nworkload = \"{}/eof-boundary.json\"\nmax-events = 4\n", fixtures.display(), fixtures.display())).unwrap();
    let destination = temp.0.join("results");
    let out = cli(&[
        "run",
        "--config",
        text(&config),
        "--output",
        text(&destination),
    ]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json(&out.stdout)["partial"], true);
    let result = json(&fs::read(destination.join("results.json")).unwrap());
    let sim = &result["simulation"];
    assert_eq!(sim["end_ps"], "100000000");
    assert_eq!(sim["requests"][0]["status"], "in_flight");
    assert_eq!(sim["receivers"], serde_json::json!([]));
    let diagnostics = json(&fs::read(destination.join("diagnostics.jsonl")).unwrap());
    assert_eq!(diagnostics["code"], "E-0004");
    assert_eq!(diagnostics["time_ps"], sim["end_ps"]);
}

#[cfg(target_os = "linux")]
#[test]
fn output_failure_keeps_the_preceding_execution_diagnostic() {
    let temp = Temp::new();
    let config = temp.0.join("limited.ini");
    let fixtures = root().join("docs/verification/fixtures/can");
    fs::write(&config, format!("[General]\nnetwork = demo.Main\nned-path = \"{}/models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\nworkload = \"{}/competition.json\"\nmax-events = 1\n", fixtures.display(), fixtures.display())).unwrap();
    let destination = temp.0.join("results");
    // Limit only the CLI child, after fixture creation. Ignoring SIGXFSZ turns
    // the first file write into a regular I/O error without a permissions race.
    let out = Command::new("/bin/sh")
        .args([
            "-c",
            "set -e; trap \"\" XFSZ; ulimit -f 0; exec \"$@\"",
            "dir-limit",
        ])
        .arg(env!("CARGO_BIN_EXE_dir-simulator"))
        .args([
            "run",
            "--config",
            text(&config),
            "--output",
            text(&destination),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let diagnostics: Vec<Value> = String::from_utf8(out.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["code"], "E-0004");
    assert_eq!(diagnostics[0]["message"], "max-events exceeded");
    assert_eq!(diagnostics[1]["code"], "E-0003");
    let report = json(&out.stdout);
    assert_eq!(report["termination"], "output_failed");
    assert_eq!(report["exit_code"], 4);
    assert!(report["manifest_path"].is_null());
    assert!(!destination.join("manifest.json").exists());
    assert!(!destination.join(".dir-simulator.lock").exists());
}

#[test]
fn errors_do_not_destroy_inputs_or_existing_directories() {
    let temp = Temp::new();
    let config = root().join("examples/can/baseline.ini");
    let destination = temp.0.join("occupied");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep.txt"), "keep").unwrap();
    let out = cli(&[
        "run",
        "--config",
        text(&config),
        "--output",
        text(&destination),
    ]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).unwrap(),
        "keep"
    );
    let out = cli(&[
        "run",
        "--config",
        "/missing-dir-scenario.ini",
        "--output",
        text(&temp.0.join("unused")),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!temp.0.join("unused").exists());
}

#[test]
fn examples_show_contention_then_overload() {
    let mut stats = Vec::new();
    for name in ["baseline", "contention", "overload"] {
        let p = dir_simulator::prepare(&root().join(format!("examples/can/{name}.ini"))).unwrap();
        let s = dir_simulator::runtime::simulate(&p).unwrap();
        assert!(!s.common.partial);
        let dropped = s
            .can
            .requests
            .iter()
            .filter(|r| r.status == "dropped")
            .count();
        let queue_max = s
            .common
            .points
            .iter()
            .filter(|p| p.metric == "queue_length")
            .map(|p| p.value)
            .max()
            .unwrap();
        stats.push((dropped, queue_max));
    }
    assert_eq!(stats[0].0, 0);
    assert!(stats[1].1 > stats[0].1);
    assert!(stats[2].0 > stats[1].0);
}

#[test]
fn prepared_inputs_are_not_reopened_during_run() {
    let temp = Temp::new();
    let config = temp.0.join("scenario.ini");
    let workload = temp.0.join("workload.json");
    let fixtures = root().join("docs/verification/fixtures/can");
    let expected_workload = fs::read_to_string(fixtures.join("competition.json")).unwrap();
    fs::write(&workload, &expected_workload).unwrap();
    fs::write(&config, format!("[General]\nnetwork = demo.Main\nned-path = \"{}/models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\nworkload = \"workload.json\"\n", fixtures.display())).unwrap();
    let prepared = dir_simulator::prepare(&config).unwrap();
    fs::remove_file(&config).unwrap();
    fs::remove_file(&workload).unwrap();
    let report = dir_simulator::run(prepared, &temp.0.join("results")).unwrap();
    assert_eq!(report.exit_code, 0);
    let result = json(&fs::read(report.output_path.join("results.json")).unwrap());
    assert_eq!(
        result["simulation"]["requests"].as_array().unwrap().len(),
        2
    );
    assert!(
        result["metadata"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["content_utf8"] == expected_workload)
    );
}
