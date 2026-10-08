//! Product execution compared with independently authored AXI transaction vectors.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for item in fs::read_dir(source).unwrap() {
        let item = item.unwrap();
        let dest = destination.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_tree(&item.path(), &dest);
        } else {
            fs::copy(item.path(), dest).unwrap();
        }
    }
}
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-axi-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        copy_tree(&fixture(), &path);
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/axi")
}
fn load(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn execute(temp: &Temp, config: &str, exit: u8) -> Value {
    let report = run(
        prepare(&temp.0.join(config)).unwrap(),
        &temp.0.join("results"),
    )
    .unwrap();
    assert_eq!(report.exit_code, exit);
    load(&temp.0.join("results/results.json"))
}
fn rows<'a>(v: &'a Value, schema: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .collect()
}
fn number(v: &Value) -> Option<f64> {
    if v.is_null() {
        None
    } else if let Some(s) = v.as_str() {
        Some(s.parse().unwrap())
    } else if let Some(n) = v.as_f64() {
        Some(n)
    } else {
        Some(v["numerator"].as_f64().unwrap() / v["denominator"].as_f64().unwrap())
    }
}
fn integer(v: &Value) -> Option<u128> {
    if v.is_null() {
        None
    } else if let Some(s) = v.as_str() {
        Some(s.parse().unwrap())
    } else {
        Some(v.as_u64().expect("exact unsigned integer") as u128)
    }
}
fn discrete(actual: &Value, expected: &Value) {
    assert_eq!(integer(actual), integer(expected), "{actual} != {expected}");
}
fn equivalent(actual: &Value, expected: &Value) {
    match (number(actual), number(expected)) {
        (None, None) => {}
        (Some(a), Some(b)) => assert!(
            (a - b).abs() <= b.abs().max(1.0) * 1e-14,
            "{actual} != {expected}"
        ),
        _ => panic!("{actual} != {expected}"),
    }
}
#[test]
fn all_analytic_fixtures_execute_with_exact_handshakes_memory_and_metrics() {
    let scenarios = load(&fixture().join("scenarios.json"));
    for scenario in scenarios["scenarios"].as_array().unwrap() {
        let temp = Temp::new();
        let v = execute(&temp, scenario["config"].as_str().unwrap(), 0);
        let expected = &scenario["expected"];
        assert_eq!(
            v["simulation"]["termination"], expected["termination"],
            "{}",
            scenario["name"]
        );
        let transactions = rows(&v, "axi.transaction");
        assert_eq!(
            transactions.len(),
            expected["transactions"].as_array().unwrap().len()
        );
        for e in expected["transactions"].as_array().unwrap() {
            let actual = transactions
                .iter()
                .find(|r| r["request_id"] == e["request_id"])
                .unwrap();
            for (key, value) in e.as_object().unwrap() {
                if key == "request_id" {
                    continue;
                }
                if matches!(key.as_str(), "grant_ps" | "completed_ps") {
                    discrete(&actual["data"][key], value);
                } else {
                    assert_eq!(&actual["data"][key], value, "{} {key}", e["request_id"]);
                }
            }
        }
        let handshakes = rows(&v, "axi.handshake");
        assert_eq!(
            handshakes.len(),
            expected["handshakes"].as_array().unwrap().len()
        );
        for e in expected["handshakes"].as_array().unwrap() {
            let actual = handshakes
                .iter()
                .find(|r| {
                    r["request_id"] == e["request_id"]
                        && r["data"]["channel"] == e["channel"]
                        && integer(&r["data"]["beat"]) == integer(&e["beat"])
                })
                .unwrap();
            discrete(&actual["time_ps"], &e["time_ps"]);
            discrete(&actual["data"]["valid_since_ps"], &e["valid_since_ps"]);
        }
        let memory = rows(&v, "axi.memory")[0];
        assert_eq!(memory["data"]["data_hex"], expected["memory_hex"]);
        discrete(&memory["time_ps"], &expected["memory_time_ps"]);
        let summary = v["simulation"]["summary"].as_array().unwrap();
        let summaries = expected["metrics"]["summary"].as_array().unwrap();
        assert_eq!(summary.len(), summaries.len());
        for e in summaries {
            let actual = summary
                .iter()
                .find(|r| r["target"] == e["target"] && r["metric"] == e["metric"])
                .unwrap();
            if actual["value_kind"] == "integer" {
                discrete(&actual["value"], &e["value"]);
            } else {
                equivalent(&actual["value"], &e["value"]);
            }
            discrete(&actual["sample_count"], &e["sample_count"]);
            assert_eq!(actual["reason"], e["reason"]);
        }
        let records = v["simulation"]["records"].as_array().unwrap();
        for window in expected["metrics"]["windows"].as_array().unwrap() {
            for (metric, value) in window["values"].as_object().unwrap() {
                let actual = records
                    .iter()
                    .find(|r| {
                        r["metric"] == metric.as_str()
                            && integer(&r["start_ps"]) == integer(&window["start_ps"])
                            && integer(&r["end_ps"]) == integer(&window["end_ps"])
                    })
                    .unwrap();
                if actual["value_kind"] == "integer" {
                    discrete(&actual["value"], value);
                } else {
                    equivalent(&actual["value"], value);
                }
            }
        }
        assert_eq!(
            v["metadata"]["metrics"],
            load(&fixture().join("metrics.json"))["metrics"]
        );
    }
}
#[test]
fn invalid_transactions_are_rejected_including_after_time_limit() {
    let invalid = load(&fixture().join("invalid-transactions.json"));
    for mutation in invalid["mutations"].as_array().unwrap() {
        let temp = Temp::new();
        let path = temp.0.join("read-write.workload.json");
        let mut v = load(&path);
        v["generators"][0]["times"] = json!(["1000s"]);
        v["generators"][0]["transaction"][mutation["field"].as_str().unwrap()] =
            mutation["value"].clone();
        fs::write(path, v.to_string()).unwrap();
        assert_eq!(
            prepare(&temp.0.join("read-write.ini")).unwrap_err().code,
            "E-0001",
            "{}",
            mutation["name"]
        );
    }
}
#[test]
fn config_required_duplicate_unknown_and_numeric_types_are_strict() {
    for change in 0..9 {
        let temp = Temp::new();
        let path = temp.0.join("read-write.model.json");
        let mut v = load(&path);
        match change {
            0 => {
                v.as_object_mut().unwrap().remove("clock_period");
            }
            1 => v["extra"] = json!(0),
            2 => v["managers"][0]["max_outstanding"] = json!(true),
            3 => v["ram"]["aw_ready"] = json!("000"),
            4 => v["ram"]["initial"]
                .as_array_mut()
                .unwrap()
                .push(json!({"offset":1,"data":"aa"})),
            5 => {
                v["ram"]["error_ranges"] = json!([{"start":4096,"end":4100,"access":"read"},{"start":4098,"end":4102,"access":"write"}])
            }
            6 => v["ram"]["size"] = json!(7),
            7 => v["clock_period"] = json!("0ps"),
            _ => {}
        }
        let text = if change == 8 {
            v.to_string().replace(
                "\"schema_version\":1",
                "\"schema_version\":1,\"schema_version\":1",
            )
        } else {
            v.to_string()
        };
        fs::write(path, text).unwrap();
        assert_eq!(
            prepare(&temp.0.join("read-write.ini")).unwrap_err().code,
            "E-0001"
        );
    }
}
#[test]
fn callback_failure_retains_only_committed_write_prefix() {
    let temp = Temp::new();
    let ini = temp.0.join("read-write.ini");
    let text = fs::read_to_string(&ini).unwrap();
    fs::write(&ini, format!("{text}\nmax-events = 4\n")).unwrap();
    let v = execute(&temp, "read-write.ini", 3);
    assert_eq!(v["simulation"]["termination"], "execution_failed");
    assert_eq!(rows(&v, "axi.handshake").len(), 1);
    assert_eq!(
        rows(&v, "axi.memory")[0]["data"]["data_hex"],
        "0001020304050607000000000000000000000000000000000000000000000000"
    );
    assert_eq!(rows(&v, "axi.transaction")[0]["data"]["status"], "active");
}
#[test]
fn future_clock_edge_is_preserved_beyond_time_limit() {
    let temp = Temp::new();
    let model = temp.0.join("read-write.model.json");
    let mut m = load(&model);
    m["clock_period"] = json!("18446744073709551615ps");
    fs::write(model, m.to_string()).unwrap();
    let workload = temp.0.join("read-write.workload.json");
    let mut w = load(&workload);
    w["generators"].as_array_mut().unwrap().truncate(1);
    w["generators"][0]["times"] = json!(["1ps"]);
    fs::write(workload, w.to_string()).unwrap();
    let v = execute(&temp, "read-write.ini", 0);
    assert_eq!(
        rows(&v, "axi.transaction")[0]["data"]["eligible_ps"],
        "18446744073709551615"
    );
    assert_eq!(v["simulation"]["termination"], "time_limit");
}

#[test]
fn eligible_overflow_fails_before_generation_commit() {
    let temp = Temp::new();
    let model = temp.0.join("read-write.model.json");
    let mut m = load(&model);
    m["clock_period"] = json!("18446744073709551613ps");
    fs::write(model, m.to_string()).unwrap();
    let workload = temp.0.join("read-write.workload.json");
    let mut w = load(&workload);
    w["generators"].as_array_mut().unwrap().truncate(1);
    w["generators"][0]["times"] = json!(["18446744073709551614ps"]);
    fs::write(workload, w.to_string()).unwrap();
    let ini = temp.0.join("read-write.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace(
            "sim-time-limit = 100ns",
            "sim-time-limit = 18446744073709551615ps",
        )
        .replace(
            "metrics-window = 25ns",
            "metrics-window = 18446744073709551615ps",
        );
    fs::write(ini, text).unwrap();
    let v = execute(&temp, "read-write.ini", 3);
    assert_eq!(v["simulation"]["termination"], "execution_failed");
    assert!(rows(&v, "axi.transaction").is_empty());
    assert!(rows(&v, "axi.handshake").is_empty());
}

#[test]
fn failure_after_write_keeps_committed_ram_and_bit_metrics() {
    let temp = Temp::new();
    let ini = temp.0.join("read-write.ini");
    let text = fs::read_to_string(&ini).unwrap();
    fs::write(ini, format!("{text}\nmax-events = 6\n")).unwrap();
    let v = execute(&temp, "read-write.ini", 3);
    assert_eq!(rows(&v, "axi.handshake").len(), 3);
    assert_eq!(
        rows(&v, "axi.memory")[0]["data"]["data_hex"],
        "1101330304660688000000000000000000000000000000000000000000000000"
    );
    let written = v["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["metric"] == "axi.written_bits")
        .unwrap();
    assert_eq!(written["value"], "32");
    assert_eq!(rows(&v, "axi.transaction")[0]["data"]["status"], "active");
}

#[test]
fn sparse_clock_and_large_timestamps_remain_exact_beyond_binary64_integer_precision() {
    let temp = Temp::new();
    let model = temp.0.join("read-write.model.json");
    let mut m = load(&model);
    m["clock_period"] = json!("1ps");
    fs::write(model, m.to_string()).unwrap();
    let workload = temp.0.join("read-write.workload.json");
    let mut w = load(&workload);
    w["generators"].as_array_mut().unwrap().truncate(1);
    let start = 9007199254740993u64;
    w["generators"][0]["times"] = json!([format!("{start}ps")]);
    fs::write(workload, w.to_string()).unwrap();
    let ini = temp.0.join("read-write.ini");
    let text = fs::read_to_string(&ini)
        .unwrap()
        .replace(
            "sim-time-limit = 100ns",
            &format!("sim-time-limit = {}ps", start + 20),
        )
        .replace(
            "metrics-window = 25ns",
            &format!("metrics-window = {}ps", start + 20),
        );
    fs::write(ini, text).unwrap();
    let v = execute(&temp, "read-write.ini", 0);
    let tx = rows(&v, "axi.transaction")[0];
    assert_eq!(tx["data"]["grant_ps"], start.to_string());
    assert_eq!(tx["data"]["completed_ps"], (start + 4).to_string());
    let hs = rows(&v, "axi.handshake");
    for (channel, beat, offset) in [
        ("AW", None, 1),
        ("W", Some(0), 2),
        ("W", Some(1), 3),
        ("B", None, 4),
    ] {
        let r = hs
            .iter()
            .find(|r| r["data"]["channel"] == channel && integer(&r["data"]["beat"]) == beat)
            .unwrap();
        assert_eq!(r["time_ps"], (start + offset).to_string());
    }
    assert_eq!(v["simulation"]["committed_events"], "7");
}

#[test]
fn empty_and_zero_horizon_emit_exact_finite_metric_set() {
    for limit in ["0ps", "100ns"] {
        let temp = Temp::new();
        fs::write(
            temp.0.join("read-write.workload.json"),
            r#"{"schema_version":2,"generators":[]}"#,
        )
        .unwrap();
        let ini = temp.0.join("read-write.ini");
        let text = fs::read_to_string(&ini).unwrap().replace(
            "sim-time-limit = 100ns",
            &format!("sim-time-limit = {limit}"),
        );
        fs::write(ini, text).unwrap();
        let v = execute(&temp, "read-write.ini", 0);
        assert!(rows(&v, "axi.transaction").is_empty());
        assert!(rows(&v, "axi.handshake").is_empty());
        assert_eq!(rows(&v, "axi.memory").len(), 1);
        let records = v["simulation"]["records"].as_array().unwrap();
        if limit == "0ps" {
            assert!(records.is_empty());
        } else {
            assert_eq!(
                records.iter().filter(|r| r["time_ps"].is_string()).count(),
                4
            );
        }
        for row in v["simulation"]["summary"].as_array().unwrap() {
            if row["value_kind"] == "integer" {
                assert_eq!(row["value"], "0");
            } else if row["metric"] == "axi.wait_mean_ps"
                || row["metric"] == "axi.latency_mean_ps"
                || limit == "0ps"
            {
                assert!(row["value"].is_null());
            } else {
                assert_eq!(row["value"].as_f64(), Some(0.0));
            }
        }
    }
}

#[test]
fn zero_fixed_delay_is_accepted_and_positive_delay_or_crossed_suffix_rejected() {
    for change in 0..3 {
        let temp = Temp::new();
        let ned = temp.0.join("models/demo/Main.ned");
        let text = fs::read_to_string(&ned).unwrap();
        let text = if change == 2 {
            text.replace("m0.w --> bus.w_m0", "m0.w --> bus.w_m1")
                .replace("m1.w --> bus.w_m1", "m1.w --> bus.w_m0")
        } else {
            text.replace("package demo;", &format!("package demo;\nchannel Wire {{ parameters: @class(\"dir.link.FixedDelay\"); double delay @unit(s) = default({}ps); }}",if change==0{0}else{1})).replace("m0.aw --> bus.aw_m0","m0.aw --> demo.Wire --> bus.aw_m0")
        };
        fs::write(ned, text).unwrap();
        let result = prepare(&temp.0.join("read-write.ini"));
        if change == 0 {
            assert!(result.is_ok(), "{:?}", result.err());
        } else {
            assert_eq!(result.unwrap_err().code, "E-0001");
        }
    }
}

#[test]
fn compound_manager_wrapper_preserves_five_path_topology_and_initial_state() {
    let temp = Temp::new();
    let ned = temp.0.join("models/demo/Main.ned");
    let text = fs::read_to_string(&ned)
        .unwrap()
        .replace(
            "network Main {",
            r#"module Wrapped {
 gates: output aw; output w; input b; output ar; input r;
 submodules: manager: demo.Manager;
 connections:
  manager.aw --> aw;
  manager.w --> w;
  manager.ar --> ar;
  b --> manager.b;
  r --> manager.r;
}
network Main {"#,
        )
        .replace("m0: demo.Manager", "m0: demo.Wrapped");
    fs::write(ned, text).unwrap();
    for file in ["read-write.model.json", "read-write.workload.json"] {
        let path = temp.0.join(file);
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("Main.m0", "Main.m0.manager");
        fs::write(path, text).unwrap();
    }
    let v = execute(&temp, "read-write.ini", 0);
    assert_eq!(rows(&v, "axi.transaction")[0]["subject"], "Main.m0.manager");
    let initial = v["metadata"]["initial_state"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["instance"] == "Main.m0")
        .unwrap();
    assert_eq!(initial["state"], "{}");
}
