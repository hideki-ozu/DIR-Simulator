//! Product checks for the Ethernet QoS contract using independent fixture expectations.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-ethernet-qos-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs/verification/fixtures/ethernet-qos")
}

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/ethernet")
}

fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn rows<'a>(result: &'a Value, schema: &str) -> Vec<&'a Value> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["schema_name"] == schema)
        .collect()
}

fn scenario(name: &str) -> (Value, PathBuf) {
    let catalog = load(&fixtures().join("scenarios.json"));
    let row = catalog["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("missing fixture scenario {name}"))
        .clone();
    let config = fixtures().join(row["config"].as_str().unwrap());
    (row["expected"].clone(), config)
}

fn run_config(config: &Path) -> Value {
    let output = Temp::new();
    let prepared = prepare(config).unwrap_or_else(|error| panic!("{}: {error}", config.display()));
    let report = run(prepared, &output.0).unwrap();
    assert_eq!(report.exit_code, 0, "{}", config.display());
    load(&output.0.join("results.json"))
}

fn summary<'a>(result: &'a Value, target: &str, metric: &str) -> &'a Value {
    result["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["target"] == target && row["metric"] == metric)
        .unwrap_or_else(|| panic!("missing {target} {metric}"))
}

#[test]
fn strict_priority_and_fifo_schedulers_match_golden_source_order() {
    for name in ["strict-priority", "fifo"] {
        let (expected, config) = scenario(name);
        let result = run_config(&config);
        assert_eq!(result["metadata"]["model_profile"], "ethernet.l2.qos.v1");
        let mut source = rows(&result, "ethernet.transfer")
            .into_iter()
            .filter(|row| row["data"]["from_port"] == "Main.a.tx")
            .collect::<Vec<_>>();
        source.sort_by_key(|row| {
            row["data"]["sof_ps"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        });
        assert_eq!(
            source
                .iter()
                .map(|row| row["data"]["frame_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected["source_order"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::as_str)
                .map(Option::unwrap)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            source
                .iter()
                .map(|row| row["data"]["sof_ps"].as_str().unwrap())
                .collect::<Vec<_>>(),
            expected["sof_ps"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::as_str)
                .map(Option::unwrap)
                .collect::<Vec<_>>()
        );
        assert!(
            rows(&result, "ethernet.frame")
                .iter()
                .all(|row| row["schema_version"] == 2)
        );
        assert!(
            rows(&result, "ethernet.transfer")
                .iter()
                .all(|row| row["schema_version"] == 2)
        );
        assert!(
            rows(&result, "ethernet.reception")
                .iter()
                .all(|row| row["schema_version"] == 1)
        );
    }
}

#[test]
fn byte_and_device_queue_capacities_match_golden_admission() {
    let (expected, config) = scenario("byte-capacity");
    let result = run_config(&config);
    let mut source = rows(&result, "ethernet.transfer")
        .into_iter()
        .filter(|row| row["data"]["from_port"] == "Main.a.tx")
        .collect::<Vec<_>>();
    source.sort_by_key(|row| row["data"]["frame_id"].as_str().unwrap().to_owned());
    assert_eq!(source.len(), 3);
    assert_eq!(source[0]["data"]["status"], expected["source_statuses"][0]);
    let frame_id = source[0]["data"]["frame_id"].as_str().unwrap();
    let frame = rows(&result, "ethernet.frame")
        .into_iter()
        .find(|row| row["record_id"] == frame_id)
        .unwrap();
    assert_eq!(frame["data"]["mac_bytes"], expected["mac_bytes"]);
    assert_eq!(
        source
            .iter()
            .map(|row| row["data"]["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected["source_statuses"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .map(Option::unwrap)
            .collect::<Vec<_>>()
    );

    let (expected, config) = scenario("device-capacity");
    let result = run_config(&config);
    let source = rows(&result, "ethernet.transfer")
        .into_iter()
        .filter(|row| row["data"]["from_port"] == "Main.a.tx")
        .collect::<Vec<_>>();
    assert_eq!(source.len(), 3);
    let dropped = source
        .iter()
        .find(|row| row["data"]["status"] == "dropped")
        .unwrap();
    assert_eq!(dropped["record_id"], expected["drop_id"]);
    assert_eq!(dropped["data"]["drop_reason"], "queue_full");
    assert_eq!(
        source
            .iter()
            .filter(|row| row["data"]["from_port"] == "Main.a.tx"
                && row["data"]["status"] == "serialized")
            .count(),
        2
    );
}

#[test]
fn active_frame_is_nonpreemptive_and_periodic_burst_times_are_exact() {
    let (expected, config) = scenario("nonpreemptive");
    let result = run_config(&config);
    let source = rows(&result, "ethernet.transfer");
    let low = source
        .iter()
        .find(|row| row["record_id"] == "low:0@Main.a.tx")
        .unwrap();
    let high = source
        .iter()
        .find(|row| row["record_id"] == "high:0@Main.a.tx")
        .unwrap();
    assert_eq!(low["data"]["eof_ps"], expected["low_eof_ps"]);
    assert_eq!(low["data"]["release_ps"], expected["low_release_ps"]);
    assert_eq!(high["data"]["sof_ps"], expected["high_sof_ps"]);

    let (expected, config) = scenario("periodic-burst");
    let result = run_config(&config);
    let actual = rows(&result, "ethernet.frame")
        .into_iter()
        .map(|row| {
            (
                row["record_id"].as_str().unwrap(),
                row["data"]["generated_ps"].as_str().unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        actual.len(),
        expected["generated"].as_object().unwrap().len()
    );
    for (id, time) in expected["generated"].as_object().unwrap() {
        assert_eq!(actual.get(id.as_str()).copied(), time.as_str(), "{id}");
    }
}

#[test]
fn flow_metrics_check_deadlines_nearest_rank_and_empty_samples() {
    let (expected, config) = scenario("flow-statistics");
    let result = run_config(&config);
    for (metric, field) in [
        ("ethernet.flow.generated", "generated"),
        ("ethernet.flow.received", "received"),
        ("ethernet.flow.deadline_sample_count", "deadline_samples"),
        ("ethernet.flow.deadline_missed", "deadline_missed"),
        ("ethernet.flow.delivery_p50_ps", "delivery_p50_ps"),
        ("ethernet.flow.delivery_p95_ps", "delivery_p95_ps"),
        ("ethernet.flow.delivery_p99_ps", "delivery_p99_ps"),
        ("ethernet.flow.delivery_jitter_ps", "delivery_jitter_ps"),
    ] {
        assert_eq!(
            summary(&result, "@flow:flow_stats", metric)["value"],
            expected[field],
            "{metric}"
        );
    }
    assert!(
        (summary(
            &result,
            "@flow:flow_stats",
            "ethernet.flow.deadline_miss_ratio"
        )["value"]
            .as_f64()
            .unwrap()
            - expected["deadline_miss_ratio"].as_f64().unwrap())
        .abs()
            < 1e-12
    );

    let records = result["simulation"]["records"].as_array().unwrap();
    let component = |metric: &str| {
        records
            .iter()
            .find(|row| {
                row["target"] == "Main.b"
                    && row["metric"] == metric
                    && row["request_id"] == "stats:0"
                    && row["receiver"] == "Main.b"
            })
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(component("ethernet.queue_wait_ps"), "0");
    assert_eq!(component("ethernet.serialization_ps"), "1152000");
    assert_eq!(component("ethernet.propagation_ps"), "2000");
    assert_eq!(component("ethernet.processing_ps"), "2000");

    let (expected, config) = scenario("flow-filtered");
    let result = run_config(&config);
    assert_eq!(
        summary(&result, "@flow:flow_filtered", "ethernet.flow.received")["value"],
        expected["received"]
    );
    assert_eq!(
        summary(&result, "@flow:flow_filtered", "ethernet.flow.filtered")["value"],
        expected["filtered"]
    );
    for metric in [
        "ethernet.flow.delivery_mean_ps",
        "ethernet.flow.delivery_p50_ps",
        "ethernet.flow.delivery_jitter_ps",
        "ethernet.flow.deadline_miss_ratio",
    ] {
        assert!(
            summary(&result, "@flow:flow_filtered", metric)["value"].is_null(),
            "{metric}"
        );
    }
    assert_eq!(
        summary(
            &result,
            "@flow:flow_filtered",
            "ethernet.flow.deadline_sample_count"
        )["value"],
        expected["deadline_samples"]
    );

    let (expected, config) = scenario("flow-single");
    let result = run_config(&config);
    assert_eq!(
        summary(
            &result,
            "@flow:flow_single",
            "ethernet.flow.delivery_mean_ps"
        )["value"]
            .as_f64(),
        Some(
            expected["delivery_mean_ps"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
        )
    );
    assert_eq!(
        summary(
            &result,
            "@flow:flow_single",
            "ethernet.flow.delivery_jitter_ps"
        )["value"],
        expected["delivery_jitter_ps"]
    );
    assert_eq!(
        summary(
            &result,
            "@flow:flow_single",
            "ethernet.flow.deadline_sample_count"
        )["value"],
        expected["deadline_samples"]
    );
    assert!(
        summary(
            &result,
            "@flow:flow_single",
            "ethernet.flow.deadline_miss_ratio"
        )["value"]
            .is_null()
    );

    let (expected, config) = scenario("fanout-copy-drop");
    let result = run_config(&config);
    for (metric, field) in [
        ("ethernet.flow.generated", "generated"),
        ("ethernet.flow.copy_dropped", "copy_dropped"),
        ("ethernet.flow.received", "received"),
    ] {
        assert_eq!(
            summary(&result, "@flow:flow_fanout", metric)["value"],
            expected[field],
            "{metric}"
        );
    }

    let (expected, config) = scenario("unfinished");
    let result = run_config(&config);
    assert_eq!(
        summary(
            &result,
            "@flow:flow_unfinished",
            "ethernet.flow.unfinished_copies"
        )["value"],
        expected["unfinished_copies"]
    );
    assert_eq!(
        summary(
            &result,
            "@flow:flow_unfinished",
            "ethernet.flow.deadline_sample_count"
        )["value"],
        expected["deadline_samples"]
    );
    assert_eq!(
        summary(&result, "@flow:flow_unfinished", "ethernet.flow.received")["value"],
        expected["received"]
    );
}

#[test]
fn public_ethernet_examples_prepare_and_execute() {
    for name in ["unicast", "duplex", "qos-priority", "qos-periodic-burst"] {
        let output = Temp::new();
        let config = examples().join(format!("{name}.ini"));
        let prepared = prepare(&config).unwrap_or_else(|error| panic!("{name}: {error}"));
        let report = run(prepared, &output.0).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(report.exit_code, 0, "{name}");
        let result = load(&output.0.join("results.json"));
        assert!(
            !result["simulation"]["model_records"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}
