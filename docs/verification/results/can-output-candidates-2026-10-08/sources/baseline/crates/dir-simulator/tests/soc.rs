//! Product execution against independently authored shared-bus/AHB/XY fixtures.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new(case: &str) -> Self {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/soc");
        let path = std::env::temp_dir().join(format!(
            "dir-soc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("models").join(case).join("demo")).unwrap();
        for suffix in ["ini", "model.json", "workload.json"] {
            fs::copy(
                root.join(format!("{case}.{suffix}")),
                path.join(format!("{case}.{suffix}")),
            )
            .unwrap();
        }
        for entry in fs::read_dir(root.join("models").join(case).join("demo")).unwrap() {
            let e = entry.unwrap();
            fs::copy(
                e.path(),
                path.join("models")
                    .join(case)
                    .join("demo")
                    .join(e.file_name()),
            )
            .unwrap();
        }
        Self(path)
    }
    fn mutate(&self, case: &str, part: &str, f: impl FnOnce(&mut Value)) {
        let path = self.0.join(format!("{case}.{part}.json"));
        let mut v = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        f(&mut v);
        fs::write(path, v.to_string()).unwrap();
    }
    fn limit(&self, case: &str, t: u64) {
        let p = self.0.join(format!("{case}.ini"));
        let s = fs::read_to_string(&p).unwrap();
        let s = s
            .lines()
            .map(|line| {
                if line.starts_with("sim-time-limit") {
                    format!("sim-time-limit = {t}ps")
                } else {
                    line.into()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(p, s).unwrap();
    }
    fn execute(&self, case: &str) -> Value {
        let report = run(
            prepare(&self.0.join(format!("{case}.ini"))).unwrap(),
            &self.0.join("results"),
        )
        .unwrap();
        assert_eq!(report.exit_code, 0);
        serde_json::from_slice(&fs::read(self.0.join("results/results.json")).unwrap()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn rows(v: &Value, schema: &str) -> Vec<Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .cloned()
        .collect()
}
fn metric(v: &Value, target: &str, name: &str) -> Value {
    v["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["target"] == target && r["metric"] == name)
        .unwrap()["value"]
        .clone()
}
fn d(n: &Value) -> Value {
    n.as_u64()
        .map(|n| json!(n.to_string()))
        .unwrap_or(Value::Null)
}
#[test]
fn zero_delay_channels_are_valid_and_nonzero_delays_reject() {
    for case in ["soc-round-robin", "ahb-wait-error", "noc-xy"] {
        let temp = Temp::new(case);
        let file = temp.0.join("models").join(case).join("demo/Main.ned");
        let text = fs::read_to_string(&file).unwrap();
        let text = text.replacen(" --> ", " --> demo.Wire --> ", 1)
            + "\nchannel Wire { parameters: @class(\"dir.link.FixedDelay\"); double delay @unit(s) = default(0ps); }\n";
        fs::write(&file, &text).unwrap();
        assert!(
            prepare(&temp.0.join(format!("{case}.ini"))).is_ok(),
            "{case}"
        );
        fs::write(&file, text.replace("default(0ps)", "default(1ps)")).unwrap();
        assert_eq!(
            prepare(&temp.0.join(format!("{case}.ini")))
                .unwrap_err()
                .code,
            "E-0001"
        );
    }
}

#[test]
fn all_analytic_fixtures_execute() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/soc/scenarios.json");
    let cases: Value = serde_json::from_slice(&fs::read(root).unwrap()).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let temp = Temp::new(name);
        let v = temp.execute(name);
        let p = if name.starts_with("soc") {
            "soc"
        } else if name.starts_with("ahb") {
            "ahb"
        } else {
            "noc"
        };
        let tx = rows(&v, &format!("{p}.transaction"));
        for expected in case["expected"]["transactions"].as_array().unwrap() {
            let row = tx
                .iter()
                .find(|r| r["record_id"] == expected["id"])
                .unwrap();
            assert_eq!(row["data"]["start_ps"], d(&expected["start"]), "{name}");
            assert_eq!(row["data"]["completed_ps"], d(&expected["end"]), "{name}");
            assert_eq!(row["data"]["status"], expected["status"], "{name}");
            assert_eq!(row["data"]["response"], expected["response"], "{name}");
            if expected["active_plan"].is_null() {
                assert!(row["data"]["active_plan"].is_null());
            } else {
                assert_eq!(
                    row["data"]["active_plan"]["resource"], expected["active_plan"]["resource"],
                    "{name} active resource"
                );
                for field in ["hop", "start_ps", "planned_end_ps"] {
                    assert_eq!(
                        row["data"]["active_plan"][field],
                        d(&expected["active_plan"][field])
                    );
                }
            }
        }
        let bits = case["expected"]["delivered_bits"].as_u64().unwrap();
        assert_eq!(
            metric(&v, "$all", &format!("{p}.delivered_bits")),
            json!(bits.to_string())
        );
        if let Some(expected_busy) = case["expected"]["busy_ps"].as_u64() {
            let horizon = case["T"].as_u64().unwrap();
            let decimal = |n: &Value| n.as_str().unwrap().parse::<u64>().unwrap();
            let completed_busy: u64 = rows(&v, &format!("{p}.transfer"))
                .iter()
                .filter(|r| r["subject"] == "Main.bus")
                .map(|r| {
                    decimal(&r["data"]["end_ps"])
                        .min(horizon)
                        .saturating_sub(decimal(&r["data"]["start_ps"]))
                })
                .sum();
            let active_busy: u64 = tx
                .iter()
                .map(|r| &r["data"]["active_plan"])
                .filter(|plan| plan["resource"] == "Main.bus")
                .map(|plan| {
                    decimal(&plan["planned_end_ps"])
                        .min(horizon)
                        .saturating_sub(decimal(&plan["start_ps"]))
                })
                .sum();
            assert_eq!(
                completed_busy + active_busy,
                expected_busy,
                "{name} busy_ps"
            );
            assert_eq!(
                metric(&v, "Main.bus", &format!("{p}.utilization")),
                json!(expected_busy as f64 / horizon as f64),
                "{name} utilization"
            );
        }
        if let Some(hops) = case["expected"]["hops"].as_array() {
            let transfers = rows(&v, &format!("{p}.transfer"));
            assert_eq!(transfers.len(), hops.len());
            for h in hops {
                assert!(transfers.iter().any(|r| r["request_id"] == h[0]
                    && r["subject"] == h[1]
                    && r["data"]["start_ps"] == d(&h[2])
                    && r["data"]["end_ps"] == d(&h[3])));
            }
        }
    }
}
#[test]
fn strict_validation_includes_unexecuted_workload() {
    for (case, part, key) in [
        ("soc-round-robin", "model", "sources"),
        ("ahb-wait-error", "model", "targets"),
        ("noc-xy", "model", "routers"),
    ] {
        let t = Temp::new(case);
        t.mutate(case, part, |v| v[key][0]["unknown"] = json!(0));
        assert_eq!(
            prepare(&t.0.join(format!("{case}.ini"))).unwrap_err().code,
            "E-0001"
        );
    }
    let c = "ahb-wait-error";
    let t = Temp::new(c);
    t.limit(c, 0);
    t.mutate(c, "workload", |v| {
        v["generators"][0]["transaction"]["address"] = json!(1)
    });
    assert!(prepare(&t.0.join(format!("{c}.ini"))).is_err());
}
#[test]
fn ahb_stop_in_every_stage_preserves_busy_and_no_completion() {
    for h in [5, 15, 25, 35] {
        let c = "ahb-wait-error";
        let t = Temp::new(c);
        t.limit(c, h);
        let v = t.execute(c);
        let tx = rows(&v, "ahb.transaction");
        let row = tx.iter().find(|r| r["record_id"] == "a:0").unwrap();
        assert_eq!(row["data"]["status"], "active");
        assert_eq!(row["data"]["active_plan"]["planned_end_ps"], "40");
        assert_eq!(metric(&v, "Main.bus", "ahb.utilization"), json!(1.0));
        assert_eq!(metric(&v, "$all", "ahb.delivered_bits"), json!("0"));
        assert_eq!(metric(&v, "$all", "ahb.latency_mean_ps"), Value::Null);
    }
}
#[test]
fn fixed_priority_and_fifo_selection() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    t.mutate(c, "model", |v| {
        v["arbitration"] = json!("fixed_priority");
        v["sources"][0]["priority"] = json!(5);
        v["sources"][1]["priority"] = json!(0);
    });
    let v = t.execute(c);
    let tx = rows(&v, "soc.transaction");
    assert_eq!(
        tx.iter().find(|r| r["record_id"] == "b:0").unwrap()["data"]["start_ps"],
        "0"
    );
    assert_eq!(
        tx.iter().find(|r| r["record_id"] == "a:0").unwrap()["data"]["start_ps"],
        "30"
    );
}
#[test]
fn zero_horizon_empty_metrics_and_descriptors() {
    let c = "noc-xy";
    let t = Temp::new(c);
    t.limit(c, 0);
    let v = t.execute(c);
    assert_eq!(v["metadata"]["metrics"].as_array().unwrap().len(), 13);
    assert_eq!(metric(&v, "$all", "noc.generated"), json!("0"));
    assert_eq!(metric(&v, "$all", "noc.throughput_bps"), Value::Null);
    assert_eq!(
        metric(&v, "Main.r00:local_in", "noc.queue_mean"),
        Value::Null
    );
    assert!(rows(&v, "noc.transaction").is_empty());
}
#[test]
fn simultaneous_opposite_links_are_independent() {
    let c = "noc-backpressure";
    let t = Temp::new(c);
    t.mutate(c,"workload",|v|{v["generators"]=json!([{ "id":"a","kind":"noc.explicit.v1","node":"Main.e00","times":["0ps"],"transaction":{"destination":"Main.e10","bytes":4}},{"id":"b","kind":"noc.explicit.v1","node":"Main.e10","times":["0ps"],"transaction":{"destination":"Main.e00","bytes":4}}]);});
    let v = t.execute(c);
    let transfers = rows(&v, "noc.transfer");
    for resource in ["Main.r00:out_east", "Main.r10:out_west"] {
        let r = transfers.iter().find(|r| r["subject"] == resource).unwrap();
        assert_eq!(r["data"]["start_ps"], "0");
        assert_eq!(r["data"]["end_ps"], "10");
    }
    assert_eq!(metric(&v, "$all", "noc.delivered_bits"), json!("64"));
}
#[test]
fn bus_decode_crossing_and_error_cycles() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    t.mutate(c,"workload",|v|{v["generators"]=json!([{"id":"cross","kind":"soc.explicit.v1","node":"Main.m0","times":["0ps"],"transaction":{"operation":"read","address":28,"bytes":8}}]);});
    let v = t.execute(c);
    let tx = rows(&v, "soc.transaction");
    assert_eq!(tx[0]["data"]["target"], Value::Null);
    assert_eq!(tx[0]["data"]["completed_ps"], "20");
    assert_eq!(tx[0]["data"]["response"], "ERROR");
    assert_eq!(metric(&v, "$all", "soc.errors"), json!("1"));
}

fn noc_generator(id: &str, source: &str, dest: &str, bytes: u64) -> Value {
    json!({"id":id,"kind":"noc.explicit.v1","node":source,"times":["0ps"],"transaction":{"destination":dest,"bytes":bytes}})
}
#[test]
fn noc_rr_head_of_line_and_same_router_parallel_outputs() {
    let c = "noc-xy";
    let t = Temp::new(c);
    t.limit(c, 80);
    t.mutate(c, "model", |v| {
        v["input_capacity"] = json!(2);
        v["source_capacity"] = json!(4);
    });
    t.mutate(c, "workload", |v| {
        v["generators"] = json!([
            noc_generator("a", "Main.e00", "Main.e10", 4),
            noc_generator("b", "Main.e10", "Main.e10", 16),
            noc_generator("c", "Main.e10", "Main.e10", 4),
            noc_generator("d", "Main.e10", "Main.e00", 4)
        ])
    });
    let v = t.execute(c);
    let tx = rows(&v, "noc.transaction");
    for (id, start, end) in [
        ("a:0", 0, 50),
        ("b:0", 0, 40),
        ("c:0", 50, 60),
        ("d:0", 50, 70),
    ] {
        let r = tx.iter().find(|r| r["record_id"] == id).unwrap();
        assert_eq!(r["data"]["start_ps"], json!(start.to_string()));
        assert_eq!(r["data"]["completed_ps"], json!(end.to_string()));
    }
    assert_eq!(metric(&v, "$all", "noc.delivered_bits"), json!("224"));
    let transfers = rows(&v, "noc.transfer");
    assert!(transfers.iter().any(|r| r["request_id"] == "d:0"
        && r["subject"] == "Main.r10:out_west"
        && r["data"]["start_ps"] == "50"));
}
#[test]
fn noc_three_parallel_outputs_and_final_delivery_only() {
    let c = "noc-xy";
    let t = Temp::new(c);
    t.limit(c, 30);
    t.mutate(c, "model", |v| {
        v["input_capacity"] = json!(2);
        v["source_capacity"] = json!(2);
    });
    t.mutate(c, "workload", |v| {
        v["generators"] = json!([
            noc_generator("a", "Main.e00", "Main.e10", 4),
            noc_generator("b", "Main.e00", "Main.e01", 4),
            noc_generator("c", "Main.e10", "Main.e00", 4)
        ])
    });
    let v = t.execute(c);
    for r in rows(&v, "noc.transaction") {
        assert_eq!(r["data"]["start_ps"], "0");
        assert_eq!(r["data"]["completed_ps"], "20");
    }
    assert_eq!(metric(&v, "$all", "noc.delivered_bits"), json!("96"));
    for resource in [
        "Main.r00:out_east",
        "Main.r00:out_north",
        "Main.r10:out_west",
    ] {
        assert_eq!(metric(&v, resource, "noc.utilization"), json!(1.0 / 3.0));
    }
}
#[test]
fn exact_queue_means_sample_denominators_and_window_boundary() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    let v = t.execute(c);
    for (target, queue, wait, latency, count) in [
        ("Main.m0", 0.6, 30.0, 60.0, "2"),
        ("Main.m1", 0.3, 30.0, 60.0, "1"),
    ] {
        assert_eq!(metric(&v, target, "soc.queue_mean"), json!(queue));
        assert_eq!(metric(&v, target, "soc.wait_mean_ps"), json!(wait));
        assert_eq!(metric(&v, target, "soc.latency_mean_ps"), json!(latency));
        for name in ["soc.wait_mean_ps", "soc.latency_mean_ps"] {
            let r = v["simulation"]["summary"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["target"] == target && r["metric"] == name)
                .unwrap();
            assert_eq!(r["sample_count"], count);
        }
    }
    assert_eq!(metric(&v, "Main.m0", "soc.queue_max"), json!("2"));
    let windows = v["simulation"]["records"].as_array();
    if let Some(w) = windows {
        assert!(w.iter().all(|r| r["time_ps"].is_null()));
    }
    let c = "noc-xy";
    let t = Temp::new(c);
    t.limit(c, 25);
    let v = t.execute(c);
    assert_eq!(metric(&v, "$all", "noc.delivered_bits"), json!("0"));
    assert_eq!(
        metric(&v, "Main.r00:out_east", "noc.utilization"),
        json!(0.8)
    );
    assert_eq!(
        metric(&v, "Main.r10:out_north", "noc.utilization"),
        json!(0.2)
    );
    assert_eq!(metric(&v, "Main.r10:in_west", "noc.queue_mean"), json!(0.0));
    assert_eq!(metric(&v, "Main.r10:in_west", "noc.queue_max"), json!("1"));
}
#[test]
fn invalid_topology_ranges_bool_duplicate_json_and_empty_load() {
    for mode in 0..6 {
        let c = "noc-xy";
        let t = Temp::new(c);
        t.mutate(c, "model", |v| match mode {
            0 => {
                v["routers"][1]["x"] = v["routers"][0]["x"].clone();
                v["routers"][1]["y"] = v["routers"][0]["y"].clone();
            }
            1 => v["routers"][0]["node"] = json!("Main.unknown"),
            2 => v["input_capacity"] = json!(true),
            3 => v["source_capacity"] = json!(0),
            4 => v["clock_period"] = json!("0ps"),
            _ => v["endpoints"][0]["router"] = json!("Main.r11"),
        });
        assert!(prepare(&t.0.join(format!("{c}.ini"))).is_err());
    }
    let c = "soc-round-robin";
    let t = Temp::new(c);
    let path = t.0.join(format!("{c}.model.json"));
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replacen(
            "\"schema_version\": 1",
            "\"schema_version\": 1, \"schema_version\": 1",
            1,
        ),
    )
    .unwrap();
    assert!(prepare(&t.0.join(format!("{c}.ini"))).is_err());
    for c in ["soc-round-robin", "ahb-wait-error", "noc-xy"] {
        let t = Temp::new(c);
        t.mutate(c, "workload", |v| v["generators"] = json!([]));
        let v = t.execute(c);
        let p = if c.starts_with("soc") {
            "soc"
        } else if c.starts_with("ahb") {
            "ahb"
        } else {
            "noc"
        };
        assert_eq!(metric(&v, "$all", &format!("{p}.generated")), json!("0"));
        assert_eq!(
            metric(&v, "$all", &format!("{p}.throughput_bps")),
            json!(0.0)
        );
        assert_eq!(
            metric(&v, "$all", &format!("{p}.latency_mean_ps")),
            Value::Null
        );
    }
}
#[test]
fn failure_prefix_keeps_completion_at_h_but_excludes_half_open_window() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    let ini = t.0.join(format!("{c}.ini"));
    let text = fs::read_to_string(&ini).unwrap();
    fs::write(&ini, format!("{text}\nmax-events = 5\n")).unwrap();
    let report = run(prepare(&ini).unwrap(), &t.0.join("results")).unwrap();
    assert_ne!(report.exit_code, 0);
    let v: Value =
        serde_json::from_slice(&fs::read(t.0.join("results/results.json")).unwrap()).unwrap();
    assert_eq!(metric(&v, "$all", "soc.completed"), json!("1"));
    assert_eq!(metric(&v, "$all", "soc.delivered_bits"), json!("64"));
    let tx = rows(&v, "soc.transaction");
    assert_eq!(
        tx.iter().find(|r| r["record_id"] == "b:0").unwrap()["data"]["status"],
        "pending"
    );
    let points = v["simulation"]["records"]
        .as_array()
        .or_else(|| v["simulation"]["points"].as_array())
        .unwrap();
    let bits: u64 = points
        .iter()
        .filter(|r| r["target"] == "$all" && r["metric"] == "soc.delivered_bits")
        .map(|r| r["value"].as_str().unwrap().parse::<u64>().unwrap())
        .sum();
    assert_eq!(bits, 0);
}
#[test]
fn multi_target_shared_resource_gap_and_full_interval_decode() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    t.limit(c, 150);
    let ned = t.0.join("models").join(c).join("demo/Main.ned");
    let text=fs::read_to_string(&ned).unwrap().replace("input response_t;","input response_t; output request_t1; input response_t1;").replace("t: demo.Target;","t: demo.Target; t1: demo.Target;").replace("t.response --> bus.response_t;","t.response --> bus.response_t; bus.request_t1 --> t1.request; t1.response --> bus.response_t1;");
    fs::write(ned, text).unwrap();
    t.mutate(c,"model",|v|{v["sources"][0]["capacity"]=json!(8);v["targets"]=json!([{"node":"Main.t","base":0,"size":16,"service_cycles":1,"error_ranges":[]},{"node":"Main.t1","base":32,"size":16,"service_cycles":3,"error_ranges":[]}]);});
    t.mutate(c,"workload",|v|v["generators"]=json!([( "a",0,8),("b",32,8),("c",20,6),("d",12,8),("e",44,8)].into_iter().map(|(id,address,bytes)|json!({"id":id,"kind":"soc.explicit.v1","node":"Main.m0","times":["0ps"],"transaction":{"operation":"read","address":address,"bytes":bytes}})).collect::<Vec<_>>()));
    let v = t.execute(c);
    let rows = rows(&v, "soc.transaction");
    for (id, start, end, response) in [
        ("a:0", 0, 30, "OKAY"),
        ("b:0", 30, 80, "OKAY"),
        ("c:0", 80, 100, "ERROR"),
        ("d:0", 100, 120, "ERROR"),
        ("e:0", 120, 140, "ERROR"),
    ] {
        let r = rows.iter().find(|r| r["record_id"] == id).unwrap();
        assert_eq!(r["data"]["start_ps"], json!(start.to_string()));
        assert_eq!(r["data"]["completed_ps"], json!(end.to_string()));
        assert_eq!(r["data"]["response"], response);
    }
    assert_eq!(metric(&v, "$all", "soc.wait_mean_ps"), json!(66.0));
    assert_eq!(metric(&v, "$all", "soc.latency_mean_ps"), json!(94.0));
    assert_eq!(metric(&v, "$all", "soc.delivered_bits"), json!("128"));
}
#[test]
fn arithmetic_overflow_keeps_generated_pending_prefix() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    t.mutate(c, "model", |v| {
        v["clock_period"] = json!("18446744073709551615ps")
    });
    let ini = t.0.join(format!("{c}.ini"));
    let report = run(prepare(&ini).unwrap(), &t.0.join("results")).unwrap();
    assert_ne!(report.exit_code, 0);
    let v: Value =
        serde_json::from_slice(&fs::read(t.0.join("results/results.json")).unwrap()).unwrap();
    assert_eq!(metric(&v, "$all", "soc.generated"), json!("3"));
    assert_eq!(metric(&v, "$all", "soc.pending"), json!("3"));
    assert_eq!(metric(&v, "$all", "soc.active"), json!("0"));
    assert!(rows(&v, "soc.transfer").is_empty());
    assert!(
        rows(&v, "soc.transaction")
            .iter()
            .all(|r| r["data"]["active_plan"].is_null())
    );
}
#[test]
fn single_manager_ahb_error_phase_and_exact_completion_boundary() {
    for h in [45, 50, 51] {
        let c = "ahb-wait-error";
        let t = Temp::new(c);
        t.limit(c, h);
        let p = t.0.join("models").join(c).join("demo/Main.ned");
        let text = fs::read_to_string(&p)
            .unwrap()
            .replace("input request_m1; output response_m1;", "")
            .replace("m1: demo.Source;", "")
            .replace(
                "m1.request --> bus.request_m1; bus.response_m1 --> m1.response;",
                "",
            );
        fs::write(p, text).unwrap();
        t.mutate(c, "model", |v| {
            v["managers"].as_array_mut().unwrap().truncate(1);
        });
        t.mutate(c, "workload", |v| {
            v["generators"].as_array_mut().unwrap().truncate(1);
            v["generators"][0]["transaction"]["address"] = json!(16);
        });
        let v = t.execute(c);
        let row = &rows(&v, "ahb.transaction")[0];
        if h <= 50 {
            assert_eq!(row["data"]["status"], "active");
            assert_eq!(row["data"]["active_plan"]["planned_end_ps"], "50");
            assert_eq!(metric(&v, "Main.bus", "ahb.utilization"), json!(1.0));
        } else {
            assert_eq!(row["data"]["status"], "completed");
            assert_eq!(row["data"]["response"], "ERROR");
            assert_eq!(row["data"]["completed_ps"], "50");
            assert_eq!(metric(&v, "$all", "ahb.errors"), json!("1"));
            assert_eq!(metric(&v, "$all", "ahb.latency_mean_ps"), json!(50.0));
        }
        assert_eq!(metric(&v, "$all", "ahb.delivered_bits"), json!("0"));
    }
}
#[test]
fn priority_tie_path_and_nonpreemption() {
    for late in [false, true] {
        let c = "soc-round-robin";
        let t = Temp::new(c);
        t.mutate(c, "model", |v| {
            v["arbitration"] = json!("fixed_priority");
            v["sources"][0]["priority"] = json!(if late { 1 } else { 0 });
            v["sources"][1]["priority"] = json!(0);
        });
        if late {
            t.mutate(c, "workload", |v| {
                v["generators"][0]["times"] = json!(["0ps"]);
                v["generators"][1]["times"] = json!(["10ps"]);
            });
        }
        let v = t.execute(c);
        let rows = rows(&v, "soc.transaction");
        let b = rows.iter().find(|r| r["record_id"] == "b:0").unwrap();
        assert_eq!(b["data"]["start_ps"], if late { "30" } else { "60" });
        if late {
            assert_eq!(metric(&v, "Main.m1", "soc.wait_mean_ps"), json!(20.0));
        }
    }
}

#[test]
fn future_generation_uses_one_dispatcher_and_failed_ordinal_stays_pending() {
    let c = "soc-round-robin";
    let t = Temp::new(c);
    t.limit(c, 1);
    t.mutate(c, "workload", |v| {
        v["generators"][0]["times"] = json!(vec!["10ps"; 100_000]);
        v["generators"][1]["times"] = json!(vec!["20ps"; 100_000]);
    });
    let prepared = prepare(&t.0.join(format!("{c}.ini"))).unwrap();
    let snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
    assert_eq!(snapshot.common.pending_events, 1);
    assert_eq!(snapshot.common.committed_events, 0);
    assert!(snapshot.soc.unwrap().transactions.is_empty());
    let t = Temp::new(c);
    let ini = t.0.join(format!("{c}.ini"));
    let text = fs::read_to_string(&ini).unwrap();
    fs::write(&ini, format!("{text}\nmax-events = 1\n")).unwrap();
    let snapshot = dir_simulator::runtime::simulate(&prepare(&ini).unwrap()).unwrap();
    assert!(snapshot.common.partial);
    assert_eq!(snapshot.common.end_ps, 0);
    assert_eq!(snapshot.common.committed_events, 1);
    // The failed ordinal shares one retained dispatcher with the other generators;
    // the committed predecessor also owns one pending arbitration wake.
    assert_eq!(snapshot.common.pending_events, 2);
    let state = snapshot.soc.unwrap();
    assert_eq!(state.transactions.len(), 1);
    assert_eq!(state.transactions[0].id, "a:0");
    assert_eq!(state.transactions[0].status, "pending");
    assert!(state.transfers.is_empty());
}
#[test]
fn long_sequential_histories_keep_fifo_capacity_and_completion_accounting() {
    for c in ["soc-round-robin", "noc-backpressure"] {
        let t = Temp::new(c);
        t.limit(c, 300_001);
        t.mutate(c, "workload", |v| {
            v["generators"].as_array_mut().unwrap().truncate(1);
            v["generators"][0]["times"] = json!(
                (0..10_000)
                    .map(|i| format!("{}ps", i * 30))
                    .collect::<Vec<_>>()
            );
            v["generators"][0]["transaction"]["bytes"] = json!(4);
        });
        let snapshot =
            dir_simulator::runtime::simulate(&prepare(&t.0.join(format!("{c}.ini"))).unwrap())
                .unwrap();
        assert!(!snapshot.common.partial);
        assert_eq!(snapshot.common.pending_events, 0);
        let state = snapshot.soc.unwrap();
        assert_eq!(state.transactions.len(), 10_000);
        assert!(state.transactions.iter().all(|r| r.status == "completed"));
        assert_eq!(
            state.transfers.len(),
            if c.starts_with("soc") { 10_000 } else { 20_000 }
        );
    }
}
#[test]
fn noc_arbitration_overflow_rolls_back_prior_grants_and_gauges_in_same_batch() {
    let c = "noc-xy";
    let t = Temp::new(c);
    t.limit(c, 1);
    t.mutate(c, "model", |v| {
        v["clock_period"] = json!("9223372036854775807ps");
        v["input_capacity"] = json!(2);
        v["source_capacity"] = json!(2);
    });
    t.mutate(c, "workload", |v| {
        v["generators"] = json!([
            noc_generator("a", "Main.e00", "Main.e10", 4),
            noc_generator("b", "Main.e00", "Main.e01", 65535),
        ])
    });
    let snapshot =
        dir_simulator::runtime::simulate(&prepare(&t.0.join(format!("{c}.ini"))).unwrap()).unwrap();
    assert!(snapshot.common.partial);
    assert_eq!(snapshot.common.diagnostics[0].code, "E-0004");
    assert_eq!(snapshot.common.pending_events, 1); // failed arbitration wake only
    let state = snapshot.soc.unwrap();
    assert_eq!(state.transactions.len(), 2);
    assert!(
        state
            .transactions
            .iter()
            .all(|r| r.status == "pending" && r.start_ps.is_none() && r.active_plan.is_none())
    );
    assert!(state.transfers.is_empty());
    assert!(state.busy.values().all(Vec::is_empty));
    assert_eq!(state.queues["Main.e00:source"].maximum, 2);
    assert!(
        state
            .queues
            .iter()
            .filter(|(id, _)| !id.ends_with(":source"))
            .all(|(_, g)| g.maximum == 0 && g.changes.is_empty())
    );
}
