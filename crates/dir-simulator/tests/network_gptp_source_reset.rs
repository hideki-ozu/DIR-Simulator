//! Ordered administrative source resets: public execution and checkpoint authority.
//! The timing oracle below uses fixture bitrate/bytes, never production helpers.
use dir_simulator::{
    CheckpointLimits, ControlCommand, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint, run,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const HORIZON: u64 = 1_000_000_000;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dir-gptp-reset-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn model(owner: &str) -> Value {
        let name = match owner {
            "ordinary" => "model.json",
            "abstract" => "abstract-model.json",
            "ideal" => "abstract-ideal-model.json",
            _ => unreachable!(),
        };
        let mut m: Value = serde_json::from_slice(
            &fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../examples/network-composite/gptp-configured")
                    .join(name),
            )
            .unwrap(),
        )
        .unwrap();
        m["limits"]["max_control_events"] = json!(10000);
        if owner != "ideal" {
            let l = &mut m["configured_media"]["physical_links"][0];
            l["a_phy"]["tx_latency_ps"] = json!("2000000");
            l["b_phy"]["tx_latency_ps"] = json!("3000000");
            l["a_phy"]["rx_latency_ps"] = json!("10000000");
            l["b_phy"]["rx_latency_ps"] = json!("11000000");
        }
        m
    }
    fn ini(&self, m: &Value, controls: Value) -> PathBuf {
        self.ini_until(m, controls, HORIZON)
    }
    fn ini_until(&self, m: &Value, controls: Value, horizon: u64) -> PathBuf {
        fs::write(self.0.join("model.json"), serde_json::to_vec(m).unwrap()).unwrap();
        fs::write(
            self.0.join("workload.json"),
            serde_json::to_vec(&json!({"schema_version":4,"generators":[],"controls":controls}))
                .unwrap(),
        )
        .unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gptp/models/two")
            .canonicalize()
            .unwrap();
        let p = self.0.join("model.ini");
        fs::write(&p, format!("[General]\nnetwork = ethtsn.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {horizon}ps\n", root.display())).unwrap();
        p
    }
    fn run(&self, m: &Value, controls: Value) -> Value {
        self.run_until(m, controls, HORIZON)
    }
    fn run_until(&self, m: &Value, controls: Value, horizon: u64) -> Value {
        let p = prepare(&self.ini_until(m, controls, horizon)).unwrap();
        let out = self.0.join("out");
        let report = run(p, &out).unwrap();
        assert_eq!(
            report.termination, "time_limit",
            "{:?}",
            report.primary_diagnostic
        );
        let result: Value =
            serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
        assert_eq!(result["simulation"]["partial"], false);
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("DIR_KEEP_SOURCE_RESET").is_some() {
            eprintln!("fixture={}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn n(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().map(|s| s.parse().unwrap()))
        .unwrap()
}
fn gp(result: &Value) -> Vec<&Value> {
    let mut rows = result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.gptp")
        .collect::<Vec<_>>();
    // Public storage order is lexical record_id order. Explicit effect_seq
    // disambiguates actual effects within a timestamp; IDs remain opaque.
    rows.sort_by_key(|r| (n(&r["time_ps"]), n(&r["data"]["effect_seq"])));
    rows.into_iter().map(|r| &r["data"]).collect()
}
fn controls(at: u64, values: &[bool]) -> Value {
    Value::Array(values.iter().enumerate().map(|(i, up)| json!({"id":format!("source-{i}"),"at_ps":at.to_string(),"kind":"link_set","link":"a-b","up":up})).collect())
}
fn assert_transitions(rows: &[&Value], at: u64, values: &[bool], epoch: u64) {
    let actual: Vec<_> = rows
        .iter()
        .filter(|r| r["kind"] == "link_transition" && n(&r["global_ps"]) == at)
        .map(|r| {
            (
                r["details"]["port"].as_str().unwrap(),
                r["details"]["up"].as_bool().unwrap(),
                n(&r["details"]["link_epoch"]),
            )
        })
        .collect();
    let expected: Vec<_> = values
        .iter()
        .enumerate()
        .flat_map(|(i, up)| {
            [
                ("Net.a.tx", *up, epoch + i as u64),
                ("Net.b.tx", *up, epoch + i as u64),
            ]
        })
        .collect();
    assert_eq!(actual, expected, "source order at {at}");
}
fn assert_fenced(rows: &[&Value], at: u64, old: &[&Value]) {
    for r in rows
        .iter()
        .filter(|r| r["kind"] == "tx_capture" || r["kind"] == "rx_capture")
    {
        if r["details"]["link_epoch"] == "0" {
            assert!(n(&r["global_ps"]) < at, "old capture: {r}");
        }
    }
    for p in old {
        assert!(
            !rows
                .iter()
                .any(|r| r["kind"] == "packet_received" && r["transfer_id"] == p["transfer_id"]),
            "stale body released: {p}"
        );
        if p["message_type"] == 0 || p["message_type"] == 3 {
            let followup = if p["message_type"] == 0 { 8 } else { 10 };
            assert!(
                !rows.iter().any(|r| r["kind"] == "packet_queued"
                    && r["message_type"] == followup
                    && r["source_clock_identity"] == p["source_clock_identity"]
                    && r["sequence"] == p["sequence"]),
                "stale join FollowUp: {p}"
            );
        }
    }
    assert!(
        rows.iter()
            .any(|r| r["kind"] == "clock_corrected" && n(&r["global_ps"]) > at),
        "fresh packets did not converge"
    );
}
#[test]
fn ordered_aba_preserves_actual_evidence_and_fences_old_callbacks() {
    for owner in ["ordinary", "abstract", "ideal"] {
        // First PdelayReq: SOF=0; (8 preamble + 72 MAC) octets at 1Gb/s => EOF=640ns.
        // Ideal arrival=740ns. Configured capture=2064/3064ns, arrival=13740ns in both directions.
        for at in [32_000, 640_000, 800_000, 2_100_000, 5_000_000] {
            let f = Fixture::new();
            let result = f.run(&Fixture::model(owner), controls(at, &[false, true]));
            let rows = gp(&result);
            assert_transitions(&rows, at, &[false, true], 1);
            let old: Vec<_> = rows
                .iter()
                .copied()
                .filter(|r| r["kind"] == "physical_sof" && n(&r["global_ps"]) == 0)
                .collect();
            assert_eq!(old.len(), 2, "actual old SOFs {owner}/{at}");
            for p in &old {
                let transfer = result["simulation"]["model_records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|r| {
                        (r["schema_name"] == "ethernet.dynamic.transfer"
                            || r["schema_name"] == "dir.preemption.transfer")
                            && r["record_id"] == p["transfer_id"]
                    })
                    .expect("GP identity must name an actual producer record_id");
                assert_eq!(n(&transfer["data"]["sof_ps"]), 0);
                if owner == "ideal" && at > 740_000 {
                    assert_eq!(transfer["data"]["status"], "completed");
                    assert_eq!(n(&transfer["data"]["arrival_ps"]), 740_000);
                    assert_eq!(
                        rows.iter()
                            .filter(|r| r["kind"] == "packet_received"
                                && r["transfer_id"] == p["transfer_id"])
                            .count(),
                        1,
                        "actual accepted body must remain historical evidence"
                    );
                } else if owner != "ordinary" {
                    assert_eq!(transfer["data"]["status"], "dropped");
                    assert_eq!(transfer["data"]["drop_reason"], "link_epoch_reset");
                    assert_eq!(n(&transfer["data"]["reset_ps"]), at);
                    assert!(transfer["data"]["arrival_ps"].is_null());
                }
                let eof: Vec<_> = rows
                    .iter()
                    .filter(|r| r["kind"] == "physical_eof" && r["transfer_id"] == p["transfer_id"])
                    .collect();
                if owner == "ordinary" || at >= 640_000 {
                    assert_eq!(eof.len(), 1, "lost actual EOF {owner}/{at}");
                    assert_eq!(n(&eof[0]["global_ps"]), 640_000);
                } else {
                    assert!(eof.is_empty(), "invented aborted EOF {owner}/{at}");
                }
                for (kind, expected) in [
                    (
                        "tx_capture",
                        if owner == "ideal" {
                            64_000
                        } else if p["source_clock_identity"] == "0000000000000001" {
                            2_064_000
                        } else {
                            3_064_000
                        },
                    ),
                    (
                        "rx_capture",
                        if owner == "ideal" {
                            164_000
                        } else if p["source_clock_identity"] == "0000000000000001" {
                            2_164_000
                        } else {
                            3_164_000
                        },
                    ),
                ] {
                    let capture: Vec<_> = rows
                        .iter()
                        .filter(|r| r["kind"] == kind && r["transfer_id"] == p["transfer_id"])
                        .collect();
                    assert_eq!(
                        capture.len(),
                        usize::from(expected < at),
                        "historical {kind} {owner}/{at}"
                    );
                    if let Some(r) = capture.first() {
                        assert_eq!(n(&r["global_ps"]), expected);
                    }
                }
            }
            assert!(
                rows.iter()
                    .any(|r| r["kind"] == "clock_corrected" && n(&r["global_ps"]) > at)
            );
            // EOF due completions run before source controls at an equal timestamp.
            // Ideal arrivals before later faults are already accepted and remain true.
            if owner != "ideal" || at <= 740_000 {
                assert_fenced(&rows, at, &old);
            }
            eprintln!("PASS ABA {owner} cut={at} old={}", old.len());
        }
    }
}
#[test]
fn repeated_down_is_idempotent_and_initial_down_up_down_is_ordered() {
    for owner in ["ordinary", "abstract", "ideal"] {
        let f = Fixture::new();
        let mut c = controls(32_000, &[false, false]);
        c.as_array_mut().unwrap().push(
            json!({"id":"recover","at_ps":"20000000","kind":"link_set","link":"a-b","up":true}),
        );
        let result = f.run(&Fixture::model(owner), c);
        let rows = gp(&result);
        assert_transitions(&rows, 32_000, &[false], 1);
        assert_transitions(&rows, 20_000_000, &[true], 2);
        assert!(rows.iter().any(|r| r["kind"] == "clock_corrected"));
        let f = Fixture::new();
        let mut m = Fixture::model(owner);
        m["base"]["config"]["base"]["config"]["dynamic"]["links"][0]["up"] = json!(false);
        let mut c = controls(32_000, &[true, false]);
        c.as_array_mut().unwrap().push(
            json!({"id":"recover","at_ps":"20000000","kind":"link_set","link":"a-b","up":true}),
        );
        let result = f.run(&m, c);
        let rows = gp(&result);
        assert_transitions(&rows, 0, &[false], 1);
        assert_transitions(&rows, 32_000, &[true, false], 2);
        assert_transitions(&rows, 20_000_000, &[true], 4);
        assert!(
            !rows
                .iter()
                .any(|r| r["kind"] == "physical_sof" && n(&r["global_ps"]) < 20_000_000)
        );
        assert!(rows.iter().any(|r| r["kind"] == "clock_corrected"));
    }
}

#[test]
fn actual_sync_reset_cannot_join_old_eof_with_delayed_capture_into_followup() {
    for owner in ["ordinary", "abstract"] {
        let baseline = Fixture::new();
        let model = Fixture::model(owner);
        let result = baseline.run(&model, json!([]));
        let rows = gp(&result);
        let sync = rows
            .iter()
            .find(|r| r["kind"] == "physical_sof" && r["message_type"] == 0)
            .unwrap();
        let sof = n(&sync["global_ps"]);
        let eof = rows
            .iter()
            .find(|r| r["kind"] == "physical_eof" && r["transfer_id"] == sync["transfer_id"])
            .unwrap();
        // Sync is padded to 64 MAC octets; 8 preamble octets at 1Gb/s.
        assert_eq!(n(&eof["global_ps"]) - sof, 576_000);
        for offset in [32_000, 800_000] {
            let at = sof + offset;
            let f = Fixture::new();
            let result = f.run_until(&model, controls(at, &[false, true]), 2_000_000_000);
            let rows = gp(&result);
            assert_transitions(&rows, at, &[false, true], 1);
            let old_sync = rows
                .iter()
                .find(|r| r["kind"] == "physical_sof" && r["transfer_id"] == sync["transfer_id"])
                .unwrap();
            assert_eq!(old_sync["message_type"], 0);
            assert_eq!(n(&old_sync["global_ps"]), sof);
            let eof = rows.iter().find(|r| {
                r["kind"] == "physical_eof" && r["transfer_id"] == old_sync["transfer_id"]
            });
            assert_eq!(eof.is_some(), owner == "ordinary" || offset > 576_000);
            assert_fenced(&rows, at, &[*old_sync]);
            eprintln!("PASS stale Sync join {owner} sof={sof} reset={at}");
        }
    }
}
fn records(s: &RunSession) -> Value {
    serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap()
}
fn copy_tree(a: &Path, b: &Path) {
    fs::create_dir_all(b).unwrap();
    for entry in fs::read_dir(a).unwrap() {
        let e = entry.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &b.join(e.file_name()));
        } else {
            fs::copy(e.path(), b.join(e.file_name())).unwrap();
        }
    }
}
fn rewrite_checkpoint(path: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let p = path.join("state.json");
    let mut s: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    change(&mut s);
    let bytes = serde_json::to_vec(&s).unwrap();
    fs::write(p, &bytes).unwrap();
    let p = path.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
    let e = m["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["path"] == "state.json")
        .unwrap();
    e["bytes"] = json!(bytes.len().to_string());
    e["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(p, serde_json::to_vec(&m).unwrap()).unwrap();
}
#[test]
fn source_reset_checkpoint_child() {
    let Some(bundle) = std::env::var_os("DIR_SOURCE_RESET_CP") else {
        return;
    };
    let p = prepare(Path::new(
        &std::env::var_os("DIR_SOURCE_RESET_INI").unwrap(),
    ))
    .unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut live, token) =
        restore_checkpoint(Path::new(&bundle), registry, CheckpointLimits::default()).unwrap();
    live.resume(token).unwrap();
    assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
    fs::write(
        std::env::var_os("DIR_SOURCE_RESET_RESULT").unwrap(),
        serde_json::to_vec(&records(&live)).unwrap(),
    )
    .unwrap();
}
#[test]
fn post_aba_pending_stale_arrival_fresh_restore_and_co_rehashed_mutants() {
    for owner in ["ordinary", "abstract"] {
        let f = Fixture::new();
        let ini = f.ini(&Fixture::model(owner), controls(5_000_000, &[false, true]));
        let p = prepare(&ini).unwrap();
        let registry = (*p.registered.as_ref().unwrap().registry).clone();
        let (mut live, controller) = RunSession::new(
            p,
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut found = false;
        for _ in 0..10000 {
            assert_eq!(live.advance_one().unwrap(), RunState::Running);
            found = records(&live).as_object().unwrap().values().any(|r| {
                r["schema"]["name"] == "dir.network.gptp"
                    && r["data"]["kind"] == "link_transition"
                    && r["data"]["details"]["up"] == true
                    && r["data"]["details"]["link_epoch"] == "2"
            });
            if found {
                break;
            }
        }
        assert!(found);
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(live.poll_control().unwrap(), RunState::Paused);
        let bundle = f.0.join("checkpoint");
        live.checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
        let state = &saved["network_backend"]["state"];
        let old: Vec<_> = state["gptp"]["packets"]
            .as_object()
            .unwrap()
            .values()
            .filter(|p| p["link_epoch"] == 0 && !p["mac_source"].is_null())
            .collect();
        assert_eq!(old.len(), 2);
        assert!(old.iter().all(|p| p["tx_eof"] == true
            && !p["tx"].is_null()
            && !p["rx"].is_null()
            && p["rx_validated"] == false));
        let links: Vec<_> = state["gptp"]["causes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"].get("Link").is_some())
            .collect();
        assert_eq!(links.len(), 4);
        assert!(
            !saved["fes"].as_array().unwrap().is_empty(),
            "pending callbacks absent"
        );
        for p in &old {
            let transfer = n(&p["transfer"]);
            let arrival = saved["fes"].as_array().unwrap().iter().find(|event| {
                let payload = &event["work"]["envelope"]["payload"];
                let Ok(bytes) = serde_json::from_value::<Vec<u8>>(payload.clone()) else {
                    return false;
                };
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    return false;
                };
                value["Arrival"] == transfer
            });
            let arrival = arrival.expect("old source must still have its actual pending Arrival");
            assert_eq!(n(&arrival["time_ps"]), 13_740_000);
            assert!(n(&saved["virtual_time_ps"]) < 13_740_000);
        }
        for mutation in [
            "cause-order",
            "cause-omitted",
            "epoch",
            "source-history",
            "source-identity",
            "false-completion",
        ] {
            let path = f.0.join(mutation);
            copy_tree(&bundle, &path);
            rewrite_checkpoint(&path, |s| {
                let state = &mut s["network_backend"]["state"];
                match mutation {
                    "cause-order" => {
                        let a = state["gptp"]["causes"].as_array_mut().unwrap();
                        let i = a
                            .iter()
                            .position(|c| c["kind"].get("Link").is_some())
                            .unwrap();
                        a.swap(i, i + 2);
                    }
                    "cause-omitted" => {
                        state["gptp"]["causes"]
                            .as_array_mut()
                            .unwrap()
                            .retain(|c| c["kind"].get("Link").is_none());
                    }
                    "epoch" => {
                        for p in state["gptp"]["ports"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .filter(|p| !p.is_null())
                        {
                            p["link_epoch"] = json!(0);
                        }
                    }
                    "source-history" => {
                        if owner == "ordinary" {
                            state["configured_media"]["attempts"][0]["eof"] = json!(640001);
                        } else {
                            let h = state["preemption"]["express_sources"]
                                .as_object_mut()
                                .unwrap();
                            let p = h.values_mut().next().unwrap();
                            p["eof_actual_ps"] = json!(640001);
                        }
                    }
                    "source-identity" => {
                        let packets = state["gptp"]["packets"].as_object_mut().unwrap();
                        let p = packets
                            .values_mut()
                            .find(|p| !p["mac_source"].is_null())
                            .unwrap();
                        let k = if owner == "ordinary" {
                            "OrdinaryAttempt"
                        } else {
                            "AbstractExpress"
                        };
                        let field = if owner == "ordinary" {
                            "index"
                        } else {
                            "epoch"
                        };
                        p["mac_source"][k][field] = json!(9999);
                        for side in ["tx", "rx"] {
                            p[side]["configured"]["source"][k][field] = json!(9999);
                        }
                    }
                    "false-completion" => {
                        let p = state["gptp"]["packets"]
                            .as_object_mut()
                            .unwrap()
                            .values_mut()
                            .find(|p| !p["mac_source"].is_null())
                            .unwrap();
                        p["rx_validated"] = json!(true);
                    }
                    _ => unreachable!(),
                }
            });
            let e = restore_checkpoint(&path, registry.clone(), CheckpointLimits::default())
                .err()
                .unwrap();
            assert_eq!(e.code, "CheckpointInvalid", "{owner}/{mutation}");
            assert!(
                e.message.contains("exact frozen-input committed prefix")
                    || e.message.contains("causal prefix"),
                "unexpected authority {owner}/{mutation}: {e:?}"
            );
            eprintln!("REJECT {owner}/{mutation}: {}", e.message);
        }
        let output = f.0.join("fresh.json");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_reset_checkpoint_child", "--nocapture"])
            .env("DIR_SOURCE_RESET_CP", &bundle)
            .env("DIR_SOURCE_RESET_INI", &ini)
            .env("DIR_SOURCE_RESET_RESULT", &output)
            .status()
            .unwrap();
        assert!(status.success());
        live.resume(live.pause_token().unwrap()).unwrap();
        assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(output).unwrap()).unwrap(),
            records(&live)
        );
        eprintln!("PASS fresh postABA stale-arrival {owner}");
    }
}
