//! Actual phased vector timers, repeated physical suffix delivery and isolated restart.
use dir_simulator::registry::Registry;
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint, run,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scenario(PathBuf);
impl Scenario {
    fn new(fail_first: bool) -> Self {
        let root = std::env::var_os("DIR_FRER_NORMATIVE_EVIDENCE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!(
                "dir-frer-normative-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.join("models/ethdistributed")).unwrap();
        let base =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/frer-rtag/distributed");
        let mut config: Value =
            serde_json::from_slice(&fs::read(base.join("model.json")).unwrap()).unwrap();
        config["replication"]["streams"][0]["recovery_algorithm"] = json!("vector_8021cb_2017");
        let mut workload: Value =
            serde_json::from_slice(&fs::read(base.join("workload.json")).unwrap()).unwrap();
        if fail_first {
            workload["controls"] = json!([{"id":"suffix-down","at_ps":"0","kind":"link_set","link":"ED","up":false},{"id":"suffix-up","at_ps":"1500000000","kind":"link_set","link":"ED","up":true}]);
        }
        fs::write(
            root.join("model.json"),
            serde_json::to_vec_pretty(&config).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("workload.json"),
            serde_json::to_vec_pretty(&workload).unwrap(),
        )
        .unwrap();
        let ned=fs::read_to_string(base.join("models/ethdistributed/Main.ned")).unwrap().replace("network Net {","channel Slow { parameters: @class(\"dir.ethernet.Link\"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(2000000000ps); }\nnetwork Net {").replace("b.tx_e --> ethdistributed.Wire --> e.rx_b", "b.tx_e --> ethdistributed.Slow --> e.rx_b").replace("e.tx_b --> ethdistributed.Wire --> b.rx_e", "e.tx_b --> ethdistributed.Slow --> b.rx_e");
        fs::write(root.join("models/ethdistributed/Main.ned"), ned).unwrap();
        fs::write(root.join("scenario.ini"),"[General]\nnetwork = ethdistributed.Net\nned-path = \"models\"\nmodel-profile = \"ethernet.frer.rtag.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 4000000000ps\n").unwrap();
        Self(root)
    }
}
impl Drop for Scenario {
    fn drop(&mut self) {
        if std::env::var_os("DIR_FRER_NORMATIVE_EVIDENCE").is_none() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn result(root: &Path, folder: &str) -> Value {
    serde_json::from_slice(&fs::read(root.join(folder).join("results.json")).unwrap()).unwrap()
}
fn rows<'a>(v: &'a Value, schema: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .map(|r| &r["data"])
        .collect()
}
fn canonical(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("run_id");
    value.as_object_mut().unwrap().remove("control");
    for k in ["started_at_utc", "finished_at_utc"] {
        value["metadata"].as_object_mut().unwrap().remove(k);
    }
    value
}
#[test]
fn production_two_rounds_and_failed_first_suffix_then_late_copy() {
    for fail in [false, true] {
        let s = Scenario::new(fail);
        run(
            prepare(&s.0.join("scenario.ini")).unwrap(),
            &s.0.join("run"),
        )
        .unwrap();
        let v = result(&s.0, "run");
        let origins = rows(&v, "dir.redundancy.origin");
        assert_eq!(origins.len(), 1);
        let o = origins[0];
        assert_eq!(o["accepted_count"], "2");
        assert_eq!(o["completed_count"], if fail { "1" } else { "2" });
        assert_eq!(o["continuation_rounds"].as_array().unwrap().len(), 2);
        assert_eq!(
            rows(&v, "ethernet.dynamic.reception")
                .iter()
                .filter(|r| r["ingress"] == "Net.d.rx" && r["status"] == "received")
                .count(),
            if fail { 1 } else { 2 }
        );
        assert_eq!(
            v["simulation"]["records"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["metric"] == "ethernet.delivery_ps" && r["target"] == "Net.d")
                .count(),
            if fail { 1 } else { 2 }
        );
        assert!(rows(&v, "dir.redundancy.timeout").len() >= 2);
    }
}
fn child(root: &Path, mode: &str, boundary: &str) {
    let o = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "normative_checkpoint_worker", "--nocapture"])
        .env("DIR_FRER_WORKER", mode)
        .env("DIR_FRER_ROOT", root)
        .env("DIR_FRER_BOUNDARY", boundary)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{mode}/{boundary}: {} {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
#[test]
fn normative_checkpoint_worker() {
    let Ok(mode) = std::env::var("DIR_FRER_WORKER") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_FRER_ROOT").unwrap());
    if mode == "save" {
        let boundary = std::env::var("DIR_FRER_BOUNDARY").unwrap();
        let (mut s, c) = RunSession::new(
            prepare(&root.join("scenario.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            let view = s.observe().unwrap();
            let origin = view
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .find(|r| r.schema.name == "dir.redundancy.origin");
            let ready = match boundary.as_str() {
                "before" => origin.is_some_and(|o| {
                    o.data["accepted_count"] == "1" && o.data["completed_count"] == "0"
                }),
                "first-complete" => origin.is_some_and(|o| o.data["completed_count"] == "1"),
                "timeout" => view
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .values()
                    .any(|r| r.schema.name == "dir.redundancy.timeout"),
                "second" => origin.is_some_and(|o| {
                    o.data["accepted_count"] == "2" && o.data["completed_count"] == "1"
                }),
                "endpoint-first" => origin.is_some_and(|o| o.data["completed_count"] == "1"),
                _ => panic!("boundary"),
            };
            if ready {
                break;
            }
        }
        c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        let ack = s
            .checkpoint(&root.join("bundle"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        s.resume(s.pause_token().unwrap()).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&root.join("original")).unwrap().status,
            ControlStatus::Applied
        );
    } else {
        let (mut s, token) = restore_checkpoint(
            &root.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        s.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&root.join("restored")).unwrap().status,
            ControlStatus::Applied
        );
    }
}
#[test]
fn phased_timer_and_rounds_resume_in_fresh_process() {
    for boundary in ["before", "first-complete", "timeout", "second"] {
        let s = Scenario::new(false);
        child(&s.0, "save", boundary);
        child(&s.0, "resume", boundary);
        assert_eq!(
            canonical(result(&s.0, "original")),
            canonical(result(&s.0, "restored")),
            "{boundary}"
        );
    }
}

fn endpoint_scenario() -> Scenario {
    let s = Scenario::new(false);
    let path = s.0.join("model.json");
    let mut model: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let stream = &mut model["replication"]["streams"][0];
    stream.as_object_mut().unwrap().remove("placement");
    stream["replicate_at"] = json!("Net.s");
    stream["eliminate_at"] = json!("Net.d");
    stream["egress_paths"] = json!([{"id":"a","ports":["Net.s.tx","Net.r.tx_a","Net.a.tx_e","Net.e.tx_d"]},{"id":"b","ports":["Net.s.tx","Net.r.tx_b","Net.b.tx_e","Net.e.tx_d"]}]);
    fs::write(&path, serde_json::to_vec_pretty(&model).unwrap()).unwrap();
    s
}
#[test]
fn endpoint_acceptance_after_timeout_is_real_repeated_delivery() {
    let s = endpoint_scenario();
    run(
        prepare(&s.0.join("scenario.ini")).unwrap(),
        &s.0.join("run"),
    )
    .unwrap();
    let v = result(&s.0, "run");
    assert_eq!(rows(&v, "dir.redundancy.origin")[0]["completed_count"], "2");
    assert_eq!(
        rows(&v, "dir.redundancy.recovery")
            .iter()
            .filter(|r| r["delivered"] == true)
            .count(),
        2
    );
    assert_eq!(
        v["simulation"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["metric"] == "ethernet.delivery_ps" && r["target"] == "Net.d")
            .count(),
        2
    );
    assert_eq!(
        rows(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["ingress"] == "Net.d.rx" && r["status"] == "received")
            .count(),
        2
    );
}
fn rewrite(path: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let mut state: Value =
        serde_json::from_slice(&fs::read(path.join("state.json")).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(path.join("state.json"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["path"] == "state.json")
        .unwrap();
    entry["bytes"] = json!(bytes.len().to_string());
    entry["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(
        path.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}
#[test]
fn checkpoint_mutants_cannot_forge_timer_reset_history_generator_or_rounds() {
    let s = Scenario::new(false);
    child(&s.0, "save", "second");
    let original = fs::read(s.0.join("bundle/state.json")).unwrap();
    let manifest = fs::read(s.0.join("bundle/manifest.json")).unwrap();
    for mutation in [
        "deadline",
        "generation",
        "history",
        "reset",
        "generator",
        "missing-round",
        "cross-round",
        "missing-recovery",
    ] {
        fs::write(s.0.join("bundle/state.json"), &original).unwrap();
        fs::write(s.0.join("bundle/manifest.json"), &manifest).unwrap();
        rewrite(&s.0.join("bundle"), |v| {
            let r = &mut v["network_backend"]["state"]["replication"];
            match mutation {
                "deadline" => r["streams"][0]["recovery_deadline_ps"] = json!(3999999999u64),
                "generation" => r["streams"][0]["timer_generation"] = json!(9),
                "history" => r["streams"][0]["history"]["words"][0] = json!(7),
                "reset" => {
                    r["streams"][0]["reset_events"] = json!([]);
                    r["streams"][0]["reset_count"] = json!(1);
                }
                "generator" => {
                    r["sequence_allocations"][0]["sequence"] = json!(2);
                    r["streams"][0]["next"] = json!(3);
                }
                other => {
                    let flow = r["distributed"]
                        .as_object_mut()
                        .unwrap()
                        .values_mut()
                        .next()
                        .unwrap();
                    match other {
                        "missing-round" => {
                            flow["rounds"].as_array_mut().unwrap().pop();
                        }
                        "cross-round" => {
                            flow["rounds"][1]["input_reception"] =
                                flow["rounds"][0]["input_reception"].clone()
                        }
                        "missing-recovery" => {
                            flow["recoveries"].as_array_mut().unwrap().pop();
                        }
                        _ => unreachable!(),
                    }
                }
            }
        });
        assert!(
            restore_checkpoint(
                &s.0.join("bundle"),
                Registry::default(),
                CheckpointLimits::default()
            )
            .is_err(),
            "accepted {mutation}"
        );
    }
}

#[test]
fn same_tick_timeout_runs_before_real_reception() {
    let s = Scenario::new(false);
    let path = s.0.join("models/ethdistributed/Main.ned");
    let ned = fs::read_to_string(&path)
        .unwrap()
        .replace("default(2000000000ps)", "default(998072000ps)");
    fs::write(path, ned).unwrap();
    run(
        prepare(&s.0.join("scenario.ini")).unwrap(),
        &s.0.join("run"),
    )
    .unwrap();
    let v = result(&s.0, "run");
    let recovery = v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["schema_name"] == "dir.redundancy.recovery" && r["data"]["decision"] == "accept_reset"
        })
        .unwrap();
    assert_eq!(recovery["time_ps"], "1000000000");
    assert!(
        v["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["schema_name"] == "dir.redundancy.timeout" && r["time_ps"] == "1000000000")
    );
    assert_eq!(rows(&v, "dir.redundancy.origin")[0]["completed_count"], "2");
}
#[test]
fn vector_configuration_rejects_capacity_one_and_fractional_millisecond() {
    for (field, value) in [
        ("history_capacity", json!(1)),
        ("recovery_timeout_ps", json!("999999999")),
    ] {
        let s = Scenario::new(false);
        let path = s.0.join("model.json");
        let mut m: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        m["replication"]["streams"][0][field] = value;
        fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(
            prepare(&s.0.join("scenario.ini")).is_err(),
            "accepted {field}"
        );
    }
}
#[test]
fn endpoint_recovery_journal_resumes_and_rejects_omitted_input() {
    for boundary in ["endpoint-first", "timeout"] {
        let s = endpoint_scenario();
        child(&s.0, "save", boundary);
        child(&s.0, "resume", boundary);
        assert_eq!(
            canonical(result(&s.0, "original")),
            canonical(result(&s.0, "restored"))
        );
        rewrite(&s.0.join("bundle"), |v| {
            v["network_backend"]["state"]["replication"]["endpoint_recoveries"] = json!([]);
        });
        assert!(
            restore_checkpoint(
                &s.0.join("bundle"),
                Registry::default(),
                CheckpointLimits::default()
            )
            .is_err()
        );
    }
}
#[test]
fn accepted_late_copy_can_continue_while_first_suffix_is_still_in_flight() {
    let s = Scenario::new(false);
    let path = s.0.join("models/ethdistributed/Main.ned");
    let ned=fs::read_to_string(&path).unwrap().replace("network Net {","channel SuffixDelay { parameters: @class(\"dir.ethernet.Link\"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(3000000000ps); }\nnetwork Net {").replace("e.tx_d --> ethdistributed.Wire --> d.rx","e.tx_d --> ethdistributed.SuffixDelay --> d.rx").replace("d.tx --> ethdistributed.Wire --> e.rx_d","d.tx --> ethdistributed.SuffixDelay --> e.rx_d");
    fs::write(path, ned).unwrap();
    let ini = s.0.join("scenario.ini");
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace("4000000000ps", "6000000000ps"),
    )
    .unwrap();
    run(prepare(&ini).unwrap(), &s.0.join("run")).unwrap();
    let v = result(&s.0, "run");
    let origin = rows(&v, "dir.redundancy.origin")[0];
    let rounds = origin["continuation_rounds"].as_array().unwrap();
    assert_eq!(origin["completed_count"], "2");
    assert!(
        rounds[1]["created_ps"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            < rounds[0]["suffix"]["terminal_ps"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
}
#[test]
fn public_normative_sample_runs_with_two_physical_deliveries() {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/frer-rtag/normative/model.ini");
    let s = Scenario::new(false);
    run(prepare(&source).unwrap(), &s.0.join("public-sample")).unwrap();
    let v = result(&s.0, "public-sample");
    assert_eq!(rows(&v, "dir.redundancy.origin")[0]["completed_count"], "2");
    assert_eq!(
        rows(&v, "ethernet.dynamic.reception")
            .iter()
            .filter(|r| r["ingress"] == "Net.d.rx" && r["status"] == "received")
            .count(),
        2
    );
}

#[test]
fn committed_timeout_cannot_be_reintroduced_in_a_later_delta() {
    for delta in [1, 0] {
        let before = Scenario::new(false);
        child(&before.0, "save", "first-complete");
        let prior: Value =
            serde_json::from_slice(&fs::read(before.0.join("bundle/state.json")).unwrap()).unwrap();
        let stream = prior["network_backend"]["state"]["replication"]["streams"][0].clone();
        let after = Scenario::new(false);
        child(&after.0, "save", "timeout");
        rewrite(&after.0.join("bundle"), |state| {
            let deadline = stream["recovery_deadline_ps"].as_u64().unwrap();
            assert_eq!(state["last_event_time_ps"], deadline.to_string());
            assert_eq!(state["last_committed_key"]["delta"], "0");
            let timer_id = state["next_event_id"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            let sequence = state["next_sequence"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            state["next_event_id"] = json!((timer_id + 1).to_string());
            state["next_sequence"] = json!((sequence + 1).to_string());
            let core = &mut state["network_backend"]["state"]["core"];
            let generation = core["control_generation"].as_u64().unwrap() + 1;
            assert!(core["control"].is_null());
            core["control_generation"] = json!(generation);
            core["control"] = json!([deadline, {"owner":"@network", "id":timer_id}]);
            core["due"].as_array_mut().unwrap().push(json!({
                "at": deadline, "kind":{"RecoveryTimeout": {
                    "stream":0, "generation":stream["timer_generation"]
                }}
            }));
            state["network_backend"]["state"]["replication"]["streams"][0] = stream;
            state["network_backend"]["records"]
                .as_array_mut()
                .unwrap()
                .retain(|row| row[1]["schema"]["name"] != "dir.redundancy.timeout");
            state["journal"]["model_records"]
                .as_object_mut()
                .unwrap()
                .retain(|_, row| row["schema"]["name"] != "dir.redundancy.timeout");
            let token = json!({"owner":"@network", "id":timer_id.to_string()});
            state["live_timers"]
                .as_array_mut()
                .unwrap()
                .push(token.clone());
            state["fes"].as_array_mut().unwrap().push(json!({
                "time_ps":deadline.to_string(), "delta":delta.to_string(), "phase":0,
                "sequence":sequence.to_string(), "work":{
                    "tag":"Delivery", "token":token,
                    "envelope":{
                        "event_id":timer_id.to_string(), "source":"@network",
                        "destination":"@network", "input_port":null,
                        "request_id":null, "time_ps":deadline.to_string(),
                        "schema":{"name":"dir.network.control", "version":1},
                        "payload":serde_json::to_vec(&json!({"Control":generation})).unwrap()
                    }
                }
            }));
        });
        let restored = restore_checkpoint(
            &after.0.join("bundle"),
            Registry::default(),
            CheckpointLimits::default(),
        );
        assert!(
            restored.is_err(),
            "committed recovery expiry was accepted as pending at delta {delta}"
        );
    }
}
