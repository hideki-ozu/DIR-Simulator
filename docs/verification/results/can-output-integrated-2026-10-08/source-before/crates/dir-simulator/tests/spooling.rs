use dir_simulator::{PreparedSimulation, output, prepare, run, runtime};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-spooling-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn result(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path.join("results.json")).unwrap()).unwrap()
}

fn high_load_config(root: &Path) -> PathBuf {
    let input = root.join("input");
    let ned = input.join("models/demo");
    std::fs::create_dir_all(&ned).unwrap();
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/can");
    std::fs::copy(examples.join("models/demo/Main.ned"), ned.join("Main.ned")).unwrap();

    let mut config = std::fs::read_to_string(examples.join("overload.ini")).unwrap();
    config = config.replace("sim-time-limit = 20ms", "sim-time-limit = 120ms");
    std::fs::write(input.join("overload.ini"), config).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&std::fs::read(examples.join("overload.json")).unwrap()).unwrap();
    for generator in workload["generators"].as_array_mut().unwrap() {
        generator["count"] = 2000.into();
    }
    std::fs::write(
        input.join("overload.json"),
        serde_json::to_vec(&workload).unwrap(),
    )
    .unwrap();
    input.join("overload.ini")
}

fn verify_manifest(path: &Path) {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(path.join("manifest.json")).unwrap()).unwrap();
    for file in manifest["files"].as_array().unwrap() {
        let name = file["name"].as_str().unwrap();
        let bytes = std::fs::read(path.join(name)).unwrap();
        assert_eq!(file["bytes"], bytes.len().to_string(), "{name}");
        assert_eq!(
            file["sha256"],
            format!("{:x}", Sha256::digest(&bytes)),
            "{name}"
        );
    }
}

fn compare_outputs(prepared: PreparedSimulation, base: &Path, label: &str) {
    std::fs::create_dir_all(base).unwrap();
    let memory_path = base.join("memory");
    std::fs::create_dir(&memory_path).unwrap();
    let memory_snapshot = runtime::simulate(&prepared).unwrap();
    output::export(&prepared, &memory_snapshot, &memory_path).unwrap();

    let disk_path = base.join("disk");
    let report = run(prepared, &disk_path).unwrap();
    assert!(!report.partial || report.exit_code == 3, "{label}");

    let memory = result(&memory_path);
    let disk = result(&disk_path);
    assert_eq!(
        memory["simulation"], disk["simulation"],
        "{label}: simulation"
    );
    for name in ["events.csv", "summary.csv"] {
        let memory_text = std::fs::read_to_string(memory_path.join(name)).unwrap();
        let disk_text = std::fs::read_to_string(disk_path.join(name)).unwrap();
        assert_eq!(
            memory_text.replace(memory["run_id"].as_str().unwrap(), "<run-id>"),
            disk_text.replace(disk["run_id"].as_str().unwrap(), "<run-id>"),
            "{label}: {name}"
        );
    }
    assert_eq!(
        std::fs::read(memory_path.join("diagnostics.jsonl")).unwrap(),
        std::fs::read(disk_path.join("diagnostics.jsonl")).unwrap(),
        "{label}: diagnostic prefix"
    );
    verify_manifest(&memory_path);
    verify_manifest(&disk_path);
    assert!(
        std::fs::read_dir(&disk_path).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".dir-")),
        "{label}: owned journals and sort runs must be removed after publication"
    );
}

#[test]
fn every_builtin_profile_has_identical_memory_and_spooled_outputs() {
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
        let prepared = prepare(&root().join("examples").join(config))
            .unwrap_or_else(|error| panic!("{config}: {error}"));
        compare_outputs(prepared, &temp.0, config);
    }
}

#[test]
fn spilled_can_observations_and_partial_failure_prefix_match_memory() {
    let temp = Temp::new();
    let config = high_load_config(&temp.0);
    let overload = prepare(&config).unwrap();
    let direct = runtime::simulate(&overload).unwrap();
    assert!(
        direct.common.points.len() > 4096,
        "fixture must cross the spool threshold, got {} observations",
        direct.common.points.len()
    );
    compare_outputs(overload, &temp.0.join("complete"), "CAN overload spill");

    let mut partial = prepare(&config).unwrap();
    partial.common.max_events = 1;
    compare_outputs(
        partial,
        &temp.0.join("partial"),
        "CAN partial failure prefix",
    );
}

#[test]
fn can_gateway_archives_match_all_valid_fixtures_and_edge_windows() {
    let fixtures = root().join("docs/verification/fixtures/gw");
    let mut configs: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "ini")
                && !p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("invalid")
        })
        .collect();
    configs.sort();
    for config in configs {
        let label = config.file_name().unwrap().to_string_lossy();
        let prepared = prepare(&config).unwrap();
        let temp = Temp::new();
        compare_outputs(prepared.clone(), &temp.0.join("normal"), &label);
        let mut shortened = prepared.clone();
        shortened.common.time_limit_ps = 0;
        compare_outputs(shortened, &temp.0.join("zero"), &format!("{label} H=0"));
        let mut short_windows = prepared;
        short_windows.common.metrics_window_ps = 13_000_000;
        compare_outputs(
            short_windows,
            &temp.0.join("windows"),
            &format!("{label} short windows"),
        );
    }
}

#[test]
fn archive_output_matches_every_partial_event_boundary() {
    for config in [
        "docs/verification/fixtures/can/competition.ini",
        "docs/verification/fixtures/gw/delay.ini",
        "docs/verification/fixtures/gw/multicast-drop.ini",
    ] {
        let prepared = prepare(&root().join(config)).unwrap();
        let count = runtime::simulate(&prepared)
            .unwrap()
            .common
            .committed_events;
        let temp = Temp::new();
        for max in 0..count {
            let mut partial = prepared.clone();
            partial.common.max_events = max;
            partial.common.metrics_window_ps = 13_000_000;
            compare_outputs(
                partial,
                &temp.0.join(max.to_string()),
                &format!("{config} max-events={max}"),
            );
        }
    }
}

#[test]
fn archived_delayed_receive_and_lexical_ids_match_materialized_output() {
    let mut prepared = prepare(&root().join("examples/can/baseline.ini")).unwrap();
    prepared.can.generators.truncate(1);
    prepared.can.generators[0].schedule = dir_simulator::types::Schedule::Periodic {
        start: 0,
        phase: 0,
        period: 1_000_000_000,
        end: None,
        count: Some(15),
    };
    prepared.common.time_limit_ps = 20_000_000_000;
    prepared.common.metrics_window_ps = 110_000_000;
    for controller in &mut prepared.can.controllers {
        controller.rx_processing_ps = 3_000_000_000;
    }
    let temp = Temp::new();
    compare_outputs(
        prepared.clone(),
        &temp.0.join("complete"),
        "lexical request IDs and delayed RX",
    );
    prepared.common.time_limit_ps = 1_500_000_000;
    compare_outputs(
        prepared.clone(),
        &temp.0.join("pending"),
        "receive remains pending beyond release",
    );
    for c in &mut prepared.can.controllers {
        c.queue_capacity = 0;
    }
    compare_outputs(
        prepared,
        &temp.0.join("zero-capacity"),
        "zero-capacity CAN queues",
    );
}
