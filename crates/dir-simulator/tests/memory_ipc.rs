//! Independent committed byte/timing expectations for the product transaction engine.
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
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dir-memory-ipc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/memory-ipc")
}
fn execute(name: &str) -> Value {
    let t = Temp::new();
    let report = run(
        prepare(&fixture().join(format!("{name}.ini"))).unwrap(),
        &t.0,
    )
    .unwrap();
    assert_eq!(report.exit_code, 0, "{name}");
    serde_json::from_slice(&fs::read(t.0.join("results.json")).unwrap()).unwrap()
}
fn rows<'a>(v: &'a Value, schema: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .collect()
}
fn request<'a>(v: &'a Value, id: &str) -> &'a Value {
    &rows(v, "memory-ipc.request")
        .into_iter()
        .find(|r| r["record_id"] == id)
        .unwrap()["data"]
}
fn resource<'a>(v: &'a Value, schema: &str, node: &str) -> &'a Value {
    &rows(v, schema)
        .into_iter()
        .find(|r| r["subject"] == node)
        .unwrap()["data"]
}
fn metric<'a>(v: &'a Value, node: &str, id: &str) -> &'a Value {
    &v["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["target"] == node && r["metric"] == id)
        .unwrap()["value"]
}
#[test]
fn every_fixture_matches_independent_bytes_times_and_prefixes() {
    let scenarios: Value =
        serde_json::from_slice(&fs::read(fixture().join("scenarios.json")).unwrap()).unwrap();
    for case in scenarios["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let v = execute(id);
        assert_eq!(v["schema_version"], 2);
        let e = &case["expected"];
        match id {
            "ddr-row-refresh" => {
                for (i, name) in ["a:0", "b:0", "c:0", "d:0"].iter().enumerate() {
                    let q = request(&v, name);
                    assert_eq!(q["started_ps"], e["grants"][i]);
                    assert_eq!(q["completed_ps"], e["completions"][i]);
                    assert_eq!(q["row_hit"], e["row_hits"][i]);
                }
                let r = resource(&v, "memory-ipc.memory", "Main.ddr");
                assert_eq!(r["hex"], e["final_hex"]);
                assert_eq!(r["refresh_started_ps"], e["refresh_start"]);
                assert_eq!(r["refresh_ended_ps"], e["refresh_end"]);
                assert_eq!(request(&v, "b:0")["output_hex"], e["read_hex"][0]);
                assert_eq!(
                    metric(&v, "Main.ddr", "memory_ipc.committed_bytes"),
                    &e["committed_bytes"]
                );
                assert_eq!(
                    metric(&v, "Main.ddr", "memory_ipc.busy_ratio")
                        .as_f64()
                        .unwrap(),
                    27.0 / 33.0
                );
            }
            "ddr-stop" => {
                assert_eq!(request(&v, "a:0")["status"], e["first_status"]);
                assert_eq!(request(&v, "a:0")["completed_ps"], e["first_completion"]);
                assert_eq!(
                    resource(&v, "memory-ipc.memory", "Main.ddr")["hex"],
                    e["final_hex"]
                );
            }
            "ddr-errors" => {
                for (id, reason) in e["reasons"].as_object().unwrap() {
                    assert_eq!(&request(&v, id)["reason"], reason);
                }
                assert_eq!(request(&v, "a:0")["completed_ps"], e["first_completion"]);
            }
            "sram-ports" | "sram-stop" => {
                let r = resource(&v, "memory-ipc.memory", "Main.sram");
                assert_eq!(r["hex"], e["final_hex"]);
                assert_eq!(
                    metric(&v, "Main.sram", "memory_ipc.committed_bytes"),
                    &e["committed_bytes"]
                );
                if id == "sram-ports" {
                    for (i, name) in ["a:0", "b:0", "c:0"].iter().enumerate() {
                        let q = request(&v, name);
                        assert_eq!(q["port"], e["ports"][i]);
                        assert_eq!(q["started_ps"], e["grants"][i]);
                        assert_eq!(q["completed_ps"], e["completions"][i]);
                    }
                    assert_eq!(request(&v, "b:0")["output_hex"], e["read_hex"]);
                } else {
                    assert_eq!(request(&v, "c:0")["status"], e["last_status"]);
                    assert!(request(&v, "c:0")["completed_ps"].is_null());
                }
            }
            "shared-ownership" => {
                let r = resource(&v, "memory-ipc.shared", "Main.shared");
                assert_eq!(r["slots"][0]["state"], e["slot_state"]);
                assert_eq!(r["slots"][0]["hex"], e["slot_hex"]);
                for (id, reason) in e["reasons"].as_object().unwrap() {
                    assert_eq!(&request(&v, id)["reason"], reason);
                }
                assert_eq!(request(&v, "a:0")["completed_ps"], e["publish_completion"]);
                assert_eq!(request(&v, "c:0")["completed_ps"], e["consume_completion"]);
                assert_eq!(request(&v, "c:0")["output_hex"], e["read_hex"]);
            }
            "shared-stop" => {
                let r = resource(&v, "memory-ipc.shared", "Main.shared");
                assert_eq!(r["slots"][0]["state"], e["slot_state"]);
                assert_eq!(r["slots"][0]["owner"], e["owner"]);
                assert_eq!(r["slots"][0]["hex"], e["slot_hex"]);
            }
            "dma-memory-content" | "dma-stop" | "dma-fault-prefix" => {
                let parent = rows(&v, "memory-ipc.request")
                    .into_iter()
                    .find(|q| q["data"]["model"] == "dma")
                    .unwrap();
                let q = &parent["data"];
                assert_eq!(q["status"], e["status"]);
                assert_eq!(q["committed_bytes"], e["committed_bytes"]);
                assert_eq!(q["data_done_ps"], e["data_done"]);
                assert_eq!(q["notified_ps"], e["notified"]);
                if let Some(reason) = e.get("reason") {
                    assert_eq!(&q["reason"], reason);
                }
                assert_eq!(
                    resource(&v, "memory-ipc.memory", "Main.ddr")["hex"],
                    e["final_hex"]
                );
                let pid = parent["record_id"].as_str().unwrap();
                if let Some(completions) = e.get("write_completion") {
                    for i in 0..2 {
                        assert_eq!(
                            request(&v, &format!("{pid}/w/{i}"))["completed_ps"],
                            completions[i]
                        );
                    }
                }
                if let Some(completions) = e.get("read_completion") {
                    for i in 0..2 {
                        assert_eq!(
                            request(&v, &format!("{pid}/r/{i}"))["completed_ps"],
                            completions[i]
                        );
                    }
                }
                if id == "dma-fault-prefix" {
                    assert_eq!(q["completed_ps"], e["failed_ps"]);
                }
            }
            "dma-offer-order" => {
                assert_eq!(request(&v, "a:0")["completed_ps"], e["external_completion"]);
                assert_eq!(request(&v, "z:0/r/0")["reason"], e["child_reason"]);
                assert_eq!(request(&v, "z:0")["status"], e["parent_status"]);
                assert_eq!(request(&v, "z:0")["reason"], e["parent_reason"]);
                assert_eq!(request(&v, "z:0")["completed_ps"], e["parent_completed_ps"]);
            }
            "mailbox-fifo" | "mailbox-stop" => {
                let r = resource(&v, "memory-ipc.mailbox", "Main.mailbox");
                assert!(r["messages"].as_array().unwrap().is_empty());
                let notification = &rows(&v, "memory-ipc.notification")[0]["data"];
                assert_eq!(notification["planned_ps"], e["notify_planned"]);
                assert_eq!(notification["delivered_ps"], e["notify_delivered"]);
                assert_eq!(request(&v, "c:0")["output_hex"], e["received_hex"]);
                if id == "mailbox-fifo" {
                    for (i, name) in ["a:0", "b:0", "c:0", "d:0"].iter().enumerate() {
                        let q = request(&v, name);
                        assert_eq!(q["completed_ps"], e["completions"][i]);
                        assert_eq!(q["reason"], e["reasons"][i]);
                    }
                    assert_eq!(request(&v, "c:0")["message_id"], e["received_message_id"]);
                }
            }
            _ => panic!("unknown fixture {id}"),
        }
        for res in v["metadata"]["topology"]["resources"].as_array().unwrap() {
            let node = res["id"].as_str().unwrap();
            let offered = metric(&v, node, "memory_ipc.offered")
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            let completed = metric(&v, node, "memory_ipc.completed")
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            let rejected = metric(&v, node, "memory_ipc.rejected")
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            let pending = rows(&v, "memory-ipc.request")
                .into_iter()
                .filter(|r| {
                    r["subject"] == node
                        && !matches!(
                            r["data"]["status"].as_str().unwrap(),
                            "completed" | "rejected" | "failed"
                        )
                })
                .count() as u64;
            assert_eq!(offered, completed + rejected + pending);
        }
    }
}
fn modified(config: Value, workload: Value, extra: &str) -> Temp {
    let t = Temp::new();
    fs::create_dir_all(t.0.join("models/memoryipc")).unwrap();
    for file in fs::read_dir(fixture().join("models/memoryipc")).unwrap() {
        let file = file.unwrap();
        fs::copy(
            file.path(),
            t.0.join("models/memoryipc").join(file.file_name()),
        )
        .unwrap();
    }
    fs::write(t.0.join("model.json"), config.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), workload.to_string()).unwrap();
    fs::write(t.0.join("run.ini"),format!("[General]\nnetwork = memoryipc.Main\nned-path = \"models\"\nmodel-profile = \"memory.ipc.transaction.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 50ps\n{extra}")).unwrap();
    t
}
fn base() -> (Value, Value) {
    (
        serde_json::from_slice(&fs::read(fixture().join("ddr-row-refresh.model.json")).unwrap())
            .unwrap(),
        json!({"schema_version":2,"generators":[]}),
    )
}
#[test]
fn preparation_is_strict_and_validates_after_stop() {
    let (config, workload) = base();
    let mut cases = Vec::new();
    let mut c = config.clone();
    c["extra"] = json!(true);
    cases.push((c, workload.clone()));
    let mut c = config.clone();
    c["sram"][0]["ports"] = json!(0);
    cases.push((c, workload.clone()));
    let mut c = config.clone();
    c["ddr"][0]["width_bytes"] = json!(3);
    cases.push((c, workload.clone()));
    let mut c = config.clone();
    c["ddr"][0]["initial"] = json!([{"offset":0,"hex":"AA"}]);
    cases.push((c, workload.clone()));
    let mut c = config.clone();
    c["shared"][0]["producers"] = json!(["p", "p"]);
    cases.push((c, workload.clone()));
    let mut c = config.clone();
    c["dma"] = json!([]);
    cases.push((c, workload));
    for (config, workload) in cases {
        let t = modified(config, workload, "");
        assert_eq!(prepare(&t.0.join("run.ini")).unwrap_err().code, "E-0001");
    }
    let w = json!({"schema_version":2,"generators":[{"id":"a","kind":"memory-ipc.explicit.v1","node":"Main.sram","times":["100ps"],"request":{"op":"write","address":0,"length":2,"hex":"aa"}}]});
    let t = modified(config.clone(), w, "");
    assert!(prepare(&t.0.join("run.ini")).is_err());
}
#[test]
fn zero_horizon_empty_metrics_are_null_without_points() {
    let (c, w) = base();
    let t = modified(c, w, "");
    let ini = fs::read_to_string(t.0.join("run.ini"))
        .unwrap()
        .replace("sim-time-limit = 50ps", "sim-time-limit = 0ps");
    fs::write(t.0.join("run.ini"), ini).unwrap();
    let report = run(prepare(&t.0.join("run.ini")).unwrap(), &t.0.join("out")).unwrap();
    assert_eq!(report.exit_code, 0);
    let v: Value =
        serde_json::from_slice(&fs::read(t.0.join("out/results.json")).unwrap()).unwrap();
    assert!(v["simulation"]["records"].as_array().unwrap().is_empty());
    assert!(metric(&v, "Main.sram", "memory_ipc.busy_ratio").is_null());
    assert!(metric(&v, "Main.sram", "memory_ipc.queue_mean").is_null());
}
#[test]
fn independent_invalid_fixture_rules_are_diagnosed() {
    let definitions: Value =
        serde_json::from_slice(&fs::read(fixture().join("invalid-configs.json")).unwrap()).unwrap();
    let original: Value = serde_json::from_slice(
        &fs::read(fixture().join(definitions["base_config"].as_str().unwrap())).unwrap(),
    )
    .unwrap();
    for mutation in definitions["mutations"].as_array().unwrap() {
        let mut c = original.clone();
        let mut value = &mut c;
        for key in mutation["path"].as_array().unwrap() {
            value = if let Some(key) = key.as_str() {
                &mut value[key]
            } else {
                &mut value[key.as_u64().unwrap() as usize]
            };
        }
        *value = mutation["value"].clone();
        let t = modified(c, json!({"schema_version":2,"generators":[]}), "");
        let diagnostic = prepare(&t.0.join("run.ini")).unwrap_err();
        assert_eq!(diagnostic.code, "E-0001");
        assert_eq!(diagnostic.details.unwrap()["rule"], mutation["rule"]);
    }
}
fn run_modified(t: &Temp) -> Value {
    let report = run(prepare(&t.0.join("run.ini")).unwrap(), &t.0.join("out")).unwrap();
    assert!(report.partial);
    serde_json::from_slice(&fs::read(t.0.join("out/results.json")).unwrap()).unwrap()
}
#[test]
fn service_overflow_retains_admitted_request_and_unmodified_memory() {
    let (mut c, _) = base();
    c["sram"][0]["read_ps"] = json!(u64::MAX.to_string());
    let w = json!({"schema_version":2,"generators":[{"id":"a","kind":"memory-ipc.explicit.v1","node":"Main.sram","times":["1ps"],"request":{"op":"read","address":0,"length":2}}]});
    let t = modified(c, w, "");
    let v = run_modified(&t);
    assert_eq!(v["simulation"]["termination"], "execution_failed");
    assert_eq!(v["simulation"]["end_ps"], "1");
    let q = request(&v, "a:0");
    assert_eq!(q["status"], "queued");
    assert!(q["started_ps"].is_null());
    assert!(q["planned_completion_ps"].is_null());
    let r = resource(&v, "memory-ipc.memory", "Main.sram");
    assert_eq!(r["ports"], json!([null, null]));
    assert_eq!(r["queue"], json!(["a:0"]));
    assert_eq!(r["hex"], "00000000000000000000000000000000");
}
#[test]
fn failure_during_admission_preserves_awaiting_child_and_offered_conservation() {
    let c: Value =
        serde_json::from_slice(&fs::read(fixture().join("dma-offer-order.model.json")).unwrap())
            .unwrap();
    let w: Value =
        serde_json::from_slice(&fs::read(fixture().join("dma-offer-order.workload.json")).unwrap())
            .unwrap();
    let t = modified(c, w, "max-delta-cycles = 1");
    let v = run_modified(&t);
    assert_eq!(request(&v, "z:0/r/0")["status"], "awaiting_admission");
    assert_eq!(request(&v, "z:0")["status"], "reading");
    assert_eq!(metric(&v, "Main.sram", "memory_ipc.offered"), "2");
    assert_eq!(metric(&v, "Main.sram", "memory_ipc.rejected"), "0");
    assert_eq!(
        resource(&v, "memory-ipc.memory", "Main.sram")["queue"],
        json!([])
    );
    assert!(
        v["simulation"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["event_seq"].is_string())
            .all(|r| r["effect_seq"].is_string())
    );
}
#[test]
fn final_dma_write_and_notification_reservation_fail_atomically() {
    let mut c: Value =
        serde_json::from_slice(&fs::read(fixture().join("dma-memory-content.model.json")).unwrap())
            .unwrap();
    c["dma"][0]["notify_ps"] = json!(u64::MAX.to_string());
    let w = json!({"schema_version":2,"generators":[{"id":"dma","kind":"memory-ipc.explicit.v1","node":"Main.dma","times":["0ps"],"request":{"op":"copy","src":"Main.sram","dst":"Main.ddr","src_address":0,"dst_address":0,"length":4}}]});
    let t = modified(c, w, "");
    let v = run_modified(&t);
    assert_eq!(v["simulation"]["end_ps"], "12");
    assert_eq!(request(&v, "dma:0")["committed_bytes"], "0");
    assert_eq!(request(&v, "dma:0")["status"], "writing");
    assert!(request(&v, "dma:0/w/0")["completed_ps"].is_null());
    assert_eq!(
        resource(&v, "memory-ipc.memory", "Main.ddr")["hex"],
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
}
#[test]
fn dma_uses_row_boundary_chunks_and_same_memory_nonoverlap() {
    let (mut c, _) = base();
    c["ddr"][0]["initial"] = json!([{"offset":0,"hex":"010203040506"}]);
    c["ddr"][0]["refresh_interval_ps"] = json!("1000");
    c["dma"][0]["chunk_bytes"] = json!(5);
    let w = json!({"schema_version":2,"generators":[{"id":"a","kind":"memory-ipc.explicit.v1","node":"Main.dma","times":["0ps"],"request":{"op":"copy","src":"Main.ddr","dst":"Main.ddr","src_address":1,"dst_address":10,"length":5}},{"id":"b","kind":"memory-ipc.explicit.v1","node":"Main.dma","times":["1ps"],"request":{"op":"copy","src":"Main.ddr","dst":"Main.ddr","src_address":0,"dst_address":1,"length":3}}]});
    let t = modified(c, w, "");
    let report = run(prepare(&t.0.join("run.ini")).unwrap(), &t.0.join("out")).unwrap();
    assert_eq!(report.exit_code, 0);
    let v: Value =
        serde_json::from_slice(&fs::read(t.0.join("out/results.json")).unwrap()).unwrap();
    assert_eq!(request(&v, "b:0")["reason"], "address_error");
    assert_eq!(request(&v, "a:0")["committed_bytes"], "5");
    assert_eq!(request(&v, "a:0")["status"], "completed");
    for (i, length) in [2, 1, 2].iter().enumerate() {
        assert_eq!(
            request(&v, &format!("a:0/r/{i}"))["length"],
            length.to_string()
        );
        assert_eq!(
            request(&v, &format!("a:0/w/{i}"))["length"],
            length.to_string()
        );
    }
    let h = resource(&v, "memory-ipc.memory", "Main.ddr")["hex"]
        .as_str()
        .unwrap();
    assert_eq!(&h[20..30], "0203040506");
}
#[test]
fn metric_points_preserve_reservation_and_callback_effect_order() {
    let v = execute("sram-ports");
    let points = v["simulation"]["records"].as_array().unwrap();
    let completions = points
        .iter()
        .filter(|p| {
            p["target"] == "Main.sram"
                && p["metric"] == "memory_ipc.latency_ps"
                && p["time_ps"] == "4"
        })
        .collect::<Vec<_>>();
    assert_eq!(completions.len(), 2);
    assert_eq!(completions[0]["request_id"], "a:0");
    assert_eq!(completions[1]["request_id"], "b:0");
    assert_eq!(completions[0]["event_seq"], "7");
    assert_eq!(completions[1]["event_seq"], "7");
    assert_eq!(completions[0]["effect_seq"], "0");
    assert_eq!(completions[1]["effect_seq"], "1");
    let admissions = points
        .iter()
        .filter(|p| {
            p["target"] == "Main.sram"
                && p["metric"] == "memory_ipc.queue_length"
                && p["event_seq"] == "5"
        })
        .collect::<Vec<_>>();
    assert_eq!(admissions.len(), 2);
    for (i, p) in admissions.iter().enumerate() {
        assert_eq!(p["effect_seq"], i.to_string());
        assert_eq!(p["value"], (i + 1).to_string());
    }
    assert!(
        points
            .iter()
            .filter(|p| p["event_seq"].is_null())
            .all(|p| p["effect_seq"].is_null() && p["request_id"].is_null())
    );
}
#[test]
fn simultaneous_generator_prefix_uses_generator_then_ordinal() {
    let (c, _) = base();
    let req = json!({"op":"read","address":0,"length":1});
    let w = json!({"schema_version":2,"generators":[{"id":"b","kind":"memory-ipc.explicit.v1","node":"Main.sram","times":["0ps"],"request":req},{"id":"a","kind":"memory-ipc.explicit.v1","node":"Main.sram","times":["0ps","0ps"],"request":req}]});
    let t = modified(c, w, "max-events = 2");
    let v = run_modified(&t);
    let ids = rows(&v, "memory-ipc.request")
        .iter()
        .map(|q| q["record_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["a:0", "a:1"]);
    assert!(
        request(&v, "a:0")["status"] == "awaiting_admission"
            && request(&v, "a:1")["status"] == "awaiting_admission"
    );
}
#[test]
fn metadata_registries_sorted_and_cohort_owner_survives_later_port_reuse() {
    let (mut c, _) = base();
    c["sram"][0]["read_ps"] = json!("5");
    c["sram"][0]["write_ps"] = json!("2");
    let w = json!({"schema_version":2,"generators":[{"id":"a","kind":"memory-ipc.explicit.v1","node":"Main.sram","times":["0ps","3ps"],"request":{"op":"write","address":0,"length":1,"hex":"aa"}},{"id":"b","kind":"memory-ipc.explicit.v1","node":"Main.sram","times":["0ps"],"request":{"op":"read","address":0,"length":1}}]});
    let t = modified(c, w, "");
    let report = run(prepare(&t.0.join("run.ini")).unwrap(), &t.0.join("out")).unwrap();
    assert_eq!(report.exit_code, 0);
    let v: Value =
        serde_json::from_slice(&fs::read(t.0.join("out/results.json")).unwrap()).unwrap();
    assert_eq!(request(&v, "a:1")["completed_ps"], "5");
    assert_eq!(request(&v, "b:0")["completed_ps"], "5");
    assert_eq!(request(&v, "b:0")["output_hex"], "aa");
    for (array, key) in [
        ("metrics", "metric_id"),
        ("model_schemas", "schema_name"),
        ("message_schemas", "schema_name"),
    ] {
        let names = v["metadata"][array]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r[key].as_str().unwrap())
            .collect::<Vec<_>>();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
