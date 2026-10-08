//! Reproduction metadata must reflect owned, resolved preparation inputs.
use dir_simulator::{input::RunIdentity, output, prepare, run, runtime};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-provenance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn execution_settings_adjusted_after_prepare_override_captured_source_values() {
    let temp = Temp::new();
    let mut prepared = prepare(&root().join("examples/can/baseline.ini")).unwrap();
    prepared.common.time_limit_ps = 0;
    prepared.common.metrics_window_ps = 7;
    prepared.common.max_events = 3;
    prepared.common.max_delta_cycles = 4;
    run(prepared, &temp.0.join("out")).unwrap();
    let result = load(&temp.0.join("out/results.json"));
    let config = result["metadata"]["config"].as_array().unwrap();
    for (key, expected) in [
        ("time-limit", "0ps"),
        ("metrics-window", "7ps"),
        ("max-events", "3"),
        ("max-delta-cycles", "4"),
    ] {
        assert_eq!(
            config.iter().find(|row| row["key"] == key).unwrap()["value"],
            expected
        );
    }
    assert_eq!(result["simulation"]["end_ps"], "0");
    assert_eq!(result["metadata"]["window_ps"], "7");
}
fn project(path: &Path) {
    fs::create_dir_all(path.join("one/demo")).unwrap();
    fs::create_dir_all(path.join("two/demo")).unwrap();
    let ned = fs::read_to_string(root().join("examples/can/models/demo/Main.ned")).unwrap();
    let (types, network) = ned.split_once("network Main").unwrap();
    fs::write(path.join("one/demo/common.ned"), types).unwrap();
    fs::write(
        path.join("two/demo/common.ned"),
        format!("package demo;\nnetwork Main{network}"),
    )
    .unwrap();
    fs::write(path.join("run.ini"),"[General]\nnetwork = demo.Main\nned-path = \"one\"; \"two\"\nsim-time-limit = 0ps\nMain.bus.bitrate = 500kbps\nMain.a.txProcessingDelay = 1us\n[Channel Main::a.tx]\ndelay = 2ns\n").unwrap();
}

#[test]
fn separate_roots_lineage_and_deleted_sources_survive_export() {
    let temp = Temp::new();
    let input = temp.0.join("input");
    project(&input);
    let mut prepared = prepare(&input.join("run.ini")).unwrap();
    prepared.common.run_identity = Some(RunIdentity {
        run_id: "550e8400-e29b-41d4-a716-446655440000".into(),
        started_at_utc: "2001-02-03T04:05:06Z".into(),
    });
    let snapshot = runtime::simulate(&prepared).unwrap();
    fs::remove_dir_all(&input).unwrap();
    let out = temp.0.join("out");
    fs::create_dir(&out).unwrap();
    output::export(&prepared, &snapshot, &out).unwrap();
    let result = load(&out.join("results.json"));
    let m = &result["metadata"];
    assert_eq!(m["started_at_utc"], "2001-02-03T04:05:06Z");
    assert_eq!(result["run_id"], "550e8400-e29b-41d4-a716-446655440000");
    let paths = m["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["logical_path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec!["config", "root0/demo/common.ned", "root1/demo/common.ned"]
    );
    let entries = m["value_provenance"].as_array().unwrap();
    let delay = entries
        .iter()
        .find(|v| v["key"] == "Main.a.txProcessingDelay")
        .unwrap();
    assert_eq!(delay["normalized_value"], "1000000ps");
    assert_eq!(delay["adopted_source"], "INI");
    assert_eq!(delay["adopted_span"]["source"], "config");
    assert_eq!(delay["declaration_span"]["source"], "root0/demo/common.ned");
    assert!(delay["default_span"]["start_byte"].is_number());
    let default = entries
        .iter()
        .find(|v| v["key"] == "Main.b.txProcessingDelay")
        .unwrap();
    assert_eq!(default["normalized_value"], "0ps");
    assert_eq!(default["adopted_source"], "default");
    let channel = entries
        .iter()
        .find(|v| v["key"] == "Main::a.tx.delay")
        .unwrap();
    assert_eq!(channel["normalized_value"], "2000ps");
    assert_eq!(m["initial_channel_state"].as_array().unwrap().len(), 6);
    assert_ne!(m["compiler"], "unknown");
    assert_ne!(m["toolchain"], "unknown");
    assert_ne!(m["cargo_lock_sha256"], "unknown");
}

#[test]
fn relocation_preserves_canonical_configuration_and_input_hashes() {
    let a = Temp::new();
    let b = Temp::new();
    project(&a.0.join("input"));
    project(&b.0.join("input"));
    for temp in [&a, &b] {
        let prepared = prepare(&temp.0.join("input/run.ini")).unwrap();
        run(prepared, &temp.0.join("out")).unwrap();
    }
    let av = load(&a.0.join("out/results.json"));
    let bv = load(&b.0.join("out/results.json"));
    assert_eq!(
        av["metadata"]["input_sha256"],
        bv["metadata"]["input_sha256"]
    );
    assert_eq!(
        av["metadata"]["config_sha256"],
        bv["metadata"]["config_sha256"]
    );
    assert_ne!(
        av["metadata"]["sources"][0]["canonical_path"],
        bv["metadata"]["sources"][0]["canonical_path"]
    );
}

#[test]
fn every_builtin_profile_shares_build_and_effective_reproduction_data() {
    for config in [
        "can/baseline.ini",
        "gateway/fanout.ini",
        "canfd/precomputed.ini",
        "axi/read-write.ini",
        "soc/soc-round-robin.ini",
        "soc/ahb-wait-error.ini",
        "soc/noc-xy.ini",
        "memory-ipc/copy.ini",
        "ethernet/unicast.ini",
        "ethernet/qos-priority.ini",
        "ethernet/vlan-unicast.ini",
        "ethernet/media/mixed.ini",
        "ethernet/media/100base-t1.ini",
    ] {
        let temp = Temp::new();
        let mut prepared = prepare(&root().join("examples").join(config)).unwrap();
        prepared.common.time_limit_ps = 0;
        run(prepared, &temp.0.join("out")).unwrap();
        let result = load(&temp.0.join("out/results.json"));
        let m = &result["metadata"];
        for key in [
            "compiler",
            "toolchain",
            "target_triple",
            "cargo_lock_sha256",
            "git_commit",
            "adoption_ledger_version",
            "build_source_sha256",
            "binary_sha256",
        ] {
            assert_ne!(m[key], "unknown", "{config}: {key}");
            assert!(m[key].is_string(), "{config}: {key}");
        }
        assert!(
            m["config"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["key"] == "@prepared-model"),
            "{config}"
        );
        assert!(
            !m["declarations"].as_array().unwrap().is_empty(),
            "{config}"
        );
        let initial = m["initial_state"].as_array().unwrap();
        let mut ids = initial
            .iter()
            .map(|v| v["instance"].as_str().unwrap())
            .collect::<Vec<_>>();
        let original = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), original, "{config}: duplicate initial state");
    }
}
