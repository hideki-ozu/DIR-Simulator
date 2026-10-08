use dir_simulator::{input::inspect_config, run_config};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dir-prepare-result-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn check_files(output: &Path) -> Value {
    let manifest: Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    for file in manifest["files"].as_array().unwrap() {
        let bytes = fs::read(output.join(file["name"].as_str().unwrap())).unwrap();
        assert_eq!(file["sha256"], format!("{:x}", Sha256::digest(&bytes)));
        assert_eq!(file["bytes"], bytes.len().to_string());
    }
    serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap()
}
#[test]
fn selected_profile_failures_publish_empty_results_with_registered_descriptors() {
    let root = root();
    let temp = Temp::new();
    let scenarios = [
        "examples/can/baseline.ini",
        "examples/gateway/fanout.ini",
        "examples/ethernet/unicast.ini",
        "examples/ethernet/qos-priority.ini",
        "examples/ethernet/vlan-unicast.ini",
        "examples/ethernet/media/collision.ini",
        "examples/ethernet/media/100base-t1.ini",
        "examples/canfd/precomputed.ini",
        "examples/axi/read-write.ini",
        "examples/soc/soc-round-robin.ini",
        "examples/soc/ahb-boundary.ini",
        "examples/soc/noc-xy.ini",
        "examples/memory-ipc/copy.ini",
    ];
    let mut exercised = 0;
    for (index, relative) in scenarios.iter().enumerate() {
        let source = root.join(relative);
        assert!(source.is_file(), "missing {relative}");
        let text = fs::read_to_string(&source).unwrap();
        let header = inspect_config(&text, &source, &root).unwrap();
        let mut config = String::from("[General]\n");
        for (key, value) in &header.general {
            let value = match key.as_str() {
                "ned-path" => header
                    .roots
                    .iter()
                    .map(|p| serde_json::to_string(&p.to_string_lossy()).unwrap())
                    .collect::<Vec<_>>()
                    .join(";"),
                "model-config" => {
                    serde_json::to_string(&header.model_config.as_ref().unwrap().to_string_lossy())
                        .unwrap()
                }
                "workload" => {
                    serde_json::to_string(&header.workload.as_ref().unwrap().to_string_lossy())
                        .unwrap()
                }
                "max-events" => "0".into(),
                _ => value.clone(),
            };
            config.push_str(&format!("{key} = {value}\n"));
        }
        if !header.general.contains_key("max-events") {
            config.push_str("max-events = 0\n");
        }
        let path = temp.0.join(format!("case-{index}.ini"));
        fs::write(&path, config).unwrap();
        let output = temp.0.join(format!("result-{index}"));
        let report = run_config(&path, &output).unwrap();
        assert_eq!(report.exit_code, 2);
        assert_eq!(report.termination, "prep_failed");
        let result = check_files(&output);
        let sim = &result["simulation"];
        assert_eq!(sim["committed_events"], "0");
        assert_eq!(sim["end_ps"], "0");
        assert_eq!(sim["records"], serde_json::json!([]));
        assert_eq!(result["metadata"]["config"], serde_json::json!([]));
        assert_eq!(result["metadata"]["initial_state"], serde_json::json!([]));
        let prepared = dir_simulator::prepare(&source).unwrap();
        let full = temp.0.join(format!("full-{index}"));
        dir_simulator::run(prepared, &full).unwrap();
        let success = check_files(&full);
        assert_eq!(
            result["metadata"]["metrics"], success["metadata"]["metrics"],
            "{relative}"
        );
        assert_eq!(
            result["metadata"]["model_schemas"], success["metadata"]["model_schemas"],
            "{relative}"
        );
        assert_eq!(
            result["metadata"]["window_ps"], success["metadata"]["window_ps"],
            "{relative}"
        );
        assert_eq!(
            result["metadata"]["models"][0]["version"], success["metadata"]["models"][0]["version"],
            "{relative}"
        );
        assert!(
            result["metadata"]["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["assumptions"].is_array() && m["version"].is_string())
        );
        assert_eq!(
            fs::read_to_string(output.join("diagnostics.jsonl")).unwrap(),
            format!(
                "{}\n",
                dir_simulator::output::diagnostic_json(&report.diagnostics[0])
            )
        );
        assert!(
            result["metadata"]["compiler"]
                .as_str()
                .unwrap()
                .starts_with("rustc 1.85.0")
        );
        exercised += 1;
    }
    assert_eq!(exercised, 13);
}
#[test]
fn unknown_profile_and_unreadable_config_use_schema_one_and_preserve_existing_files() {
    let temp = Temp::new();
    let config = temp.0.join("invalid.ini");
    fs::write(&config,"[General]\nnetwork=demo.Main\nned-path=\"missing\"\nsim-time-limit=1ms\nmodel-profile=\"unknown.profile.v1\"\n").unwrap();
    let output = temp.0.join("result");
    let report = run_config(&config, &output).unwrap();
    assert_eq!(report.exit_code, 2);
    let result = check_files(&output);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["metadata"]["models"], serde_json::json!([]));
    assert_eq!(result["simulation"]["requests"], serde_json::json!([]));
    let occupied = temp.0.join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::write(occupied.join("keep"), b"preserved").unwrap();
    let failure = run_config(&config, &occupied).unwrap_err();
    assert_eq!(failure.diagnostic.code, "E-0003");
    assert_eq!(failure.diagnostic.seq, 1);
    assert!(!failure.diagnostic.primary);
    assert_eq!(failure.prior_diagnostics[0].seq, 0);
    assert!(failure.prior_diagnostics[0].primary);
    assert_eq!(fs::read(occupied.join("keep")).unwrap(), b"preserved");
    let missing = run_config(&temp.0.join("missing.ini"), &temp.0.join("missing-result")).unwrap();
    assert_eq!(missing.exit_code, 2);
    assert_eq!(
        check_files(&missing.output_path)["metadata"]["sources"],
        serde_json::json!([])
    );
}
