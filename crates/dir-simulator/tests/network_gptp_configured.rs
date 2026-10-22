//! Configured MDI SFD adapter: independent physical/raw-clock and restoration gates.
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
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-gptp-configured-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn model() -> Value {
        serde_json::from_slice(
            &fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../examples/network-composite/gptp-configured/model.json"),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn ini(&self, model: &Value, horizon: u64) -> PathBuf {
        fs::write(
            self.0.join("model.json"),
            serde_json::to_vec(model).unwrap(),
        )
        .unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gptp")
            .canonicalize()
            .unwrap();
        let ini = self.0.join("model.ini");
        fs::write(&ini, format!("[General]\nnetwork = ethtsn.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"{}\"\nsim-time-limit = {horizon}ps\n",root.join("models/two").display(),root.join("two-node-workload.json").display())).unwrap();
        ini
    }
    fn run(&self, model: &Value, horizon: u64) -> Value {
        let out = self.0.join("out");
        run(prepare(&self.ini(model, horizon)).unwrap(), &out).unwrap();
        serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::env::var_os("DIR_KEEP_CONFIGURED_GPTP").is_none() {
            let _ = fs::remove_dir_all(&self.0);
        } else {
            eprintln!("fixture={}", self.0.display());
        }
    }
}
fn integer(v: &Value) -> i128 {
    v.as_str()
        .map(|s| s.parse().unwrap())
        .or_else(|| v.as_i64().map(i128::from))
        .unwrap()
}
fn ratio(v: &Value) -> f64 {
    integer(&v["num"]) as f64 / integer(&v["den"]) as f64
}
fn gp(result: &Value) -> Vec<&Value> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.gptp")
        .map(|r| &r["data"])
        .collect()
}

fn oracle(result: &Value, model: &Value, convergence: bool) {
    assert_eq!(result["simulation"]["partial"], false, "partial execution");
    let rows = gp(result);
    assert!(rows.iter().any(|r| r["kind"] == "tx_capture"));
    let media = &model["configured_media"]["physical_links"][0];
    for r in rows
        .iter()
        .filter(|r| r["kind"] == "tx_capture" || r["kind"] == "rx_capture")
    {
        let p = &r["details"]["configured"];
        assert_eq!(p["plane"], "MdiMacSfd");
        assert!(r["details"]["sampled"].is_null());
        let side = integer(&p["endpoint"][1]);
        let (tx, peer, source, destination) = if side == 0 {
            (&media["a_phy"], &media["b_phy"], 0, 1)
        } else {
            (&media["b_phy"], &media["a_phy"], 1, 0)
        };
        // Independent fixture physics: 1Gb/s=1000ps/bit, 100ns propagation.
        // Deliberately do not call the production timeline or raw-clock helpers.
        let sof = integer(&p["sof_ps"]);
        let mdi = sof + integer(&tx["tx_latency_ps"]);
        let peer_sof = mdi + 100_000;
        let reference = if r["kind"] == "tx_capture" {
            mdi + 64_000
        } else {
            peer_sof + 64_000
        };
        assert_eq!(integer(&p["mdi_sof_ps"]), mdi);
        assert_eq!(integer(&p["peer_sof_ps"]), peer_sof);
        assert_eq!(integer(&p["reference_ps"]), reference);
        assert_eq!(integer(&p["observed_at_ps"]), reference);
        assert_eq!(integer(&r["global_ps"]), reference);
        assert_eq!(
            integer(&p["arrival_planned_ps"]),
            integer(&p["eof_planned_ps"])
                + integer(&tx["tx_latency_ps"])
                + 100_000
                + integer(&peer["rx_latency_ps"])
        );
        let node = if r["kind"] == "tx_capture" {
            source
        } else {
            destination
        };
        let clk = &model["clocks"]["nodes"][node];
        let num = integer(&clk["offset_ps"]) * integer(&clk["rate_den"])
            + reference * integer(&clk["rate_num"]);
        let den = integer(&clk["rate_den"]);
        assert_eq!(
            integer(&r["details"]["raw_numerator"]) * den,
            num * integer(&r["details"]["raw_denominator"])
        );
    }
    for capture in rows
        .iter()
        .filter(|r| r["kind"] == "tx_capture" && (r["message_type"] == 0 || r["message_type"] == 3))
    {
        if rows
            .iter()
            .any(|r| r["kind"] == "physical_eof" && r["transfer_id"] == capture["transfer_id"])
        {
            let fup = if capture["message_type"] == 0 { 8 } else { 10 };
            let count = rows
                .iter()
                .filter(|r| {
                    r["kind"] == "packet_queued"
                        && r["message_type"] == fup
                        && r["source_clock_identity"] == capture["source_clock_identity"]
                        && r["sequence"] == capture["sequence"]
                })
                .count();
            assert_eq!(
                count, 1,
                "eligible FollowUp must be generated exactly once: {capture}"
            );
        }
    }
    if convergence {
        assert!(
            rows.iter().any(|r| r["kind"] == "clock_corrected"),
            "no convergence"
        );
        let qualified = rows
            .iter()
            .filter(|r| r["kind"] == "pdelay_complete" && r["details"]["as_capable"] == true)
            .collect::<Vec<_>>();
        assert!(!qualified.is_empty());
        for r in qualified {
            let peer = if r["details"]["port"] == "Net.a.tx" {
                1
            } else {
                0
            };
            let clk = &model["clocks"]["nodes"][peer];
            let expected =
                100_000.0 * integer(&clk["rate_num"]) as f64 / integer(&clk["rate_den"]) as f64;
            assert!(
                (ratio(&r["details"]["mean_link_delay"]) - expected).abs() < 0.1,
                "propagation oracle {r}"
            );
            assert!(r["details"]["wire_quantization"].is_null());
        }
    }
}
#[test]
fn configured_distinct_latencies_have_exact_mdi_references_and_propagation_only_pdelay() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["clocks"]["nodes"][0]["rate_num"] = json!("10003");
    m["clocks"]["nodes"][0]["rate_den"] = json!("10000");
    let result = f.run(&m, 3_000_000_000);
    oracle(&result, &m, true);
}
#[test]
fn configured_delayed_tx_survives_mac_release_and_next_attempt() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    let l = &mut m["configured_media"]["physical_links"][0];
    l["a_phy"]["tx_latency_ps"] = json!("2000000");
    l["b_phy"]["tx_latency_ps"] = json!("3000000");
    l["a_phy"]["rx_latency_ps"] = json!("4000000");
    l["b_phy"]["rx_latency_ps"] = json!("5000000");
    let result = f.run(&m, 3_000_000_000);
    oracle(&result, &m, true);
    let rows = gp(&result);
    assert!(rows.iter().any(|r| r["kind"] == "tx_capture"
        && integer(&r["global_ps"])
            > integer(&r["details"]["configured"]["eof_planned_ps"]) + 96_000));
    assert!(
        rows.iter().filter(|r| r["kind"] == "tx_capture").any(|r| {
            let sof = integer(&r["details"]["configured"]["sof_ps"]);
            let at = integer(&r["global_ps"]);
            rows.iter().any(|next| {
                next["kind"] == "physical_sof"
                    && next["source_clock_identity"] == r["source_clock_identity"]
                    && next["transfer_id"] != r["transfer_id"]
                    && integer(&next["global_ps"]) > sof
                    && integer(&next["global_ps"]) < at
            })
        }),
        "no actual later SOF before delayed historical capture"
    );
}
#[test]
fn configured_equal_time_eof_and_reference_are_deterministic() {
    // First PdelayReq has 72 MAC bytes; EOF=(8+72)*8ns, reference=64ns+TX latency.
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["configured_media"]["physical_links"][0]["a_phy"]["tx_latency_ps"] = json!("576000");
    let result = f.run(&m, 1_000_000_000);
    oracle(&result, &m, true);
    assert!(gp(&result).iter().any(|r| r["kind"] == "tx_capture"
        && integer(&r["global_ps"]) == integer(&r["details"]["configured"]["eof_planned_ps"])));
}
#[test]
fn configured_supported_fixture_is_public_and_half_duplex_is_specific_negative() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/gptp-configured/model.ini");
    prepare(&p).unwrap();
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["configured_media"]["physical_links"][0]["duplex"] = json!("half");
    let error = prepare(&f.ini(&m, 1_000_000)).err().unwrap();
    assert!(
        error.message.contains("full") || error.message.contains("duplex"),
        "{error:?}"
    );
}
fn snapshot_records(session: &RunSession) -> Value {
    serde_json::to_value(
        &session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records,
    )
    .unwrap()
}
fn rewrite_checkpoint(path: &Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let file = path.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    fs::write(file, &bytes).unwrap();
    let manifest_path = path.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|v| v["path"] == "state.json")
        .unwrap();
    entry["bytes"] = bytes.len().to_string().into();
    entry["sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let e = entry.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &to.join(e.file_name()));
        } else {
            fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}
#[test]
fn configured_checkpoint_child() {
    let Some(bundle) = std::env::var_os("DIR_CONFIGURED_CP") else {
        return;
    };
    let p = prepare(Path::new(&std::env::var_os("DIR_CONFIGURED_INI").unwrap())).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut restored, token) =
        restore_checkpoint(Path::new(&bundle), registry, CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    fs::write(
        std::env::var_os("DIR_CONFIGURED_RESULT").unwrap(),
        serde_json::to_vec(&snapshot_records(&restored)).unwrap(),
    )
    .unwrap();
}
#[test]
fn configured_fresh_checkpoint_cuts_and_co_rehashed_mutants() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    let l = &mut m["configured_media"]["physical_links"][0];
    l["a_phy"]["tx_latency_ps"] = json!("2000000");
    l["b_phy"]["tx_latency_ps"] = json!("3000000");
    l["a_phy"]["rx_latency_ps"] = json!("4000000");
    l["b_phy"]["rx_latency_ps"] = json!("5000000");
    for cut in [
        "before_capture",
        "after_eof",
        "after_capture",
        "before_join",
        "after_join",
    ] {
        let ini = f.ini(&m, 1_000_000_000);
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
            if live.advance_one().unwrap() != RunState::Running {
                break;
            }
            let records = snapshot_records(&live);
            let rows = records
                .as_object()
                .unwrap()
                .values()
                .filter(|r| r["schema"]["name"] == "dir.network.gptp")
                .map(|r| &r["data"])
                .collect::<Vec<_>>();
            for row in &rows {
                let id = &row["transfer_id"];
                if id.is_null() {
                    continue;
                }
                let has = |kind: &str| {
                    rows.iter()
                        .any(|r| r["transfer_id"] == *id && r["kind"] == kind)
                };
                let is_sync = row["message_type"] == 0 || row["message_type"] == "0";
                let matched = match cut {
                    "before_capture" => {
                        has("physical_sof") && !has("tx_capture") && !has("physical_eof")
                    }
                    "after_eof" => has("physical_eof") && !has("tx_capture"),
                    "after_capture" => has("rx_capture") && !has("packet_received"),
                    "before_join" => is_sync && has("physical_eof") && !has("tx_capture"),
                    "after_join" => is_sync && has("tx_capture") && has("physical_eof"),
                    _ => unreachable!(),
                };
                if matched {
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
        assert!(found, "missing {cut}");
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(live.poll_control().unwrap(), RunState::Paused);
        let bundle = f.0.join(cut);
        live.checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
        let packets = saved["network_backend"]["state"]["gptp"]["packets"]
            .as_object()
            .unwrap();
        for p in packets.values() {
            if p["tx_eof"] == true && !p["tx"].is_null() {
                assert!(!p["tx_completed_at_ps"].is_null());
                let at = integer(&p["tx_completed_at_ps"]);
                let capture = integer(&p["tx"]["at_ps"]);
                let binding = &p["mac_source"]["OrdinaryAttempt"];
                let a = &saved["network_backend"]["state"]["configured_media"]["attempts"]
                    [integer(&binding["index"]) as usize];
                assert_eq!(at, capture.max(integer(&a["eof"])));
            }
        }
        if cut == "after_capture" {
            for mutation in [
                "attempt",
                "number",
                "direction",
                "reference",
                "config",
                "plane",
                "epoch",
                "completion",
                "event",
            ] {
                let mutant = f.0.join(format!("mutant-{mutation}"));
                copy_tree(&bundle, &mutant);
                rewrite_checkpoint(&mutant, |state| {
                    let packets = state["network_backend"]["state"]["gptp"]["packets"]
                        .as_object_mut()
                        .unwrap();
                    let p = packets.values_mut().find(|p| !p["rx"].is_null()).unwrap();
                    match mutation {
                        "attempt" => {
                            p["mac_source"]["OrdinaryAttempt"]["index"] = json!(9999);
                            p["rx"]["configured"]["source"]["OrdinaryAttempt"]["index"] =
                                json!(9999);
                        }
                        "number" => {
                            p["mac_source"]["OrdinaryAttempt"]["number"] = json!(2);
                            p["rx"]["configured"]["source"]["OrdinaryAttempt"]["number"] = json!(2);
                        }
                        "direction" => {
                            p["mac_source"]["OrdinaryAttempt"]["direction"] = json!(9999);
                            p["rx"]["configured"]["source"]["OrdinaryAttempt"]["direction"] =
                                json!(9999);
                        }
                        "reference" => {
                            let v = integer(&p["rx"]["at_ps"]) + 1;
                            p["rx"]["at_ps"] = json!(v as u64);
                            p["rx"]["configured"]["reference_ps"] = json!(v as u64);
                            p["rx"]["configured"]["observed_at_ps"] = json!(v as u64);
                        }
                        "config" => p["rx"]["configured"]["tx_latency_ps"] = json!(9999999),
                        "plane" => p["rx"]["configured"]["plane"] = json!("RawDecodeAvailability"),
                        "epoch" => {
                            p["link_epoch"] = json!(9999);
                            p["rx"]["link_epoch"] = json!(9999);
                        }
                        "completion" => p["tx_completed_at_ps"] = Value::Null,
                        "event" => {
                            let fes = state["fes"].as_array_mut().unwrap();
                            fes.clear();
                        }
                        _ => unreachable!(),
                    }
                });
                assert_eq!(
                    restore_checkpoint(&mutant, registry.clone(), CheckpointLimits::default())
                        .err()
                        .unwrap()
                        .code,
                    "CheckpointInvalid",
                    "{mutation}"
                );
            }
        }
        let result = f.0.join(format!("result-{cut}.json"));
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "configured_checkpoint_child", "--nocapture"])
            .env("DIR_CONFIGURED_CP", &bundle)
            .env("DIR_CONFIGURED_INI", &ini)
            .env("DIR_CONFIGURED_RESULT", &result)
            .status()
            .unwrap();
        assert!(status.success(), "fresh restore {cut}");
        live.resume(live.pause_token().unwrap()).unwrap();
        assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
        let fresh: Value = serde_json::from_slice(&fs::read(result).unwrap()).unwrap();
        assert_eq!(fresh, snapshot_records(&live), "{cut}");
    }
}
fn with_owner(mut m: Value, owner: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    if owner == "merge" {
        let template: Value =
            serde_json::from_slice(&fs::read(root.join("mac-merge/model.json")).unwrap()).unwrap();
        m["mac_merge"] = template["mac_merge"].clone();
    } else if owner == "abstract" {
        let template: Value = serde_json::from_slice(
            &fs::read(root.join("network-extensions/preemption-model.json")).unwrap(),
        )
        .unwrap();
        m["preemption"] = template["preemption"].clone();
    }
    m
}
#[test]
fn configured_merge_and_abstract_express_use_retained_actual_sources() {
    for owner in ["merge", "abstract"] {
        let f = Fixture::new();
        let mut m = with_owner(Fixture::model(), owner);
        m["limits"]["max_control_events"] = json!(10000);
        m["configured_media"]["physical_links"][0]["a_phy"]["tx_latency_ps"] = json!("2000000");
        m["configured_media"]["physical_links"][0]["b_phy"]["tx_latency_ps"] = json!("3000000");
        let result = f.run(&m, 3_000_000_000);
        oracle(&result, &m, true);
        let source = if owner == "merge" {
            "MergeExpress"
        } else {
            "AbstractExpress"
        };
        assert!(gp(&result).iter().any(|r| r["kind"] == "tx_capture"
            && r["details"]["configured"]["source"].get(source).is_some()));
    }
}
#[test]
fn ideal_abstract_express_declares_ideal_plane_and_keeps_propagation() {
    let f = Fixture::new();
    let mut m = with_owner(Fixture::model(), "abstract");
    m.as_object_mut().unwrap().remove("configured_media");
    let result = f.run(&m, 1_000_000_000);
    assert_eq!(result["simulation"]["partial"], false);
    let rows = gp(&result);
    assert!(rows.iter().any(|r| r["kind"] == "clock_corrected"));
    for r in rows
        .iter()
        .filter(|r| r["kind"] == "tx_capture" || r["kind"] == "rx_capture")
    {
        let proof = &r["details"]["express"];
        assert_eq!(proof["plane"], "IdealMacSfd");
        assert!(proof["endpoint"].is_null());
        assert!(r["details"]["configured"].is_null());
        let sof = integer(&proof["sof_ps"]);
        let reference = sof
            + 64_000
            + if r["kind"] == "rx_capture" {
                100_000
            } else {
                0
            };
        assert_eq!(integer(&proof["reference_ps"]), reference);
        assert_eq!(integer(&proof["tx_latency_ps"]), 0);
        assert_eq!(
            integer(&proof["arrival_planned_ps"]),
            integer(&proof["eof_planned_ps"]) + 100_000
        );
    }
}
fn custom_ini(f: &Fixture, m: &Value, w: &Value, horizon: u64) -> PathBuf {
    let ini = f.ini(m, horizon);
    let workload = f.0.join("workload.json");
    fs::write(&workload, serde_json::to_vec(w).unwrap()).unwrap();
    let text = fs::read_to_string(&ini).unwrap();
    let line = text
        .lines()
        .find(|line| line.starts_with("workload ="))
        .unwrap();
    fs::write(
        &ini,
        text.replace(line, &format!("workload = \"{}\"", workload.display())),
    )
    .unwrap();
    ini
}
fn custom_run(f: &Fixture, m: &Value, w: &Value, horizon: u64) -> Value {
    let ini = custom_ini(f, m, w, horizon);
    let out = f.0.join("out");
    let report = run(prepare(&ini).unwrap(), &out).unwrap();
    assert_eq!(
        report.termination, "time_limit",
        "{:?}",
        report.primary_diagnostic
    );
    serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap()
}
#[test]
fn configured_fault_restart_fences_captures_and_join_but_preserves_eof() {
    for fault_at in [32_000u64, 800_000, 2_100_000, 5_000_000] {
        let f = Fixture::new();
        let mut m = Fixture::model();
        m["configured_media"]["physical_links"][0]["a_phy"]["tx_latency_ps"] = json!("2000000");
        m["configured_media"]["physical_links"][0]["b_phy"]["tx_latency_ps"] = json!("3000000");
        m["configured_media"]["physical_links"][0]["a_phy"]["rx_latency_ps"] = json!("10000000");
        m["configured_media"]["physical_links"][0]["b_phy"]["rx_latency_ps"] = json!("11000000");
        let w = json!({"schema_version":4,"generators":[],"controls":[
            {"id":"down","at_ps":fault_at.to_string(),"kind":"link_set","link":"a-b","up":false},
            {"id":"up","at_ps":"20000000","kind":"link_set","link":"a-b","up":true}]});
        let result = custom_run(&f, &m, &w, 2_000_000_000);
        let rows = gp(&result);
        assert!(
            rows.iter()
                .any(|r| r["kind"] == "physical_eof" && integer(&r["global_ps"]) == 640_000),
            "physical EOF lost {fault_at}"
        );
        assert!(
            rows.iter()
                .any(|r| r["kind"] == "clock_corrected" && integer(&r["global_ps"]) > 20_000_000)
        );
        for r in rows
            .iter()
            .filter(|r| r["kind"] == "tx_capture" || r["kind"] == "rx_capture")
        {
            if r["details"]["link_epoch"] == "0" {
                assert!(integer(&r["global_ps"]) < i128::from(fault_at));
            }
        }
        assert!(
            rows.iter()
                .any(|r| r["kind"] == "packet_rejected" && r["reason"] == "stale_link_epoch")
        );
    }
}
#[test]
fn configured_pause_delays_actual_gptp_admission_and_convergence_recovers() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["mac_control"] = json!({"ports":[
        {"port":"Net.a.tx","station_mac":"02:00:00:00:00:01","rx_enabled":true,"tx_enabled":true,"policy":"ieee802.3.pause.v1"},
        {"port":"Net.b.tx","station_mac":"02:00:00:00:00:02","rx_enabled":true,"tx_enabled":true,"policy":"ieee802.3.pause.v1"}],
        "controls":[{"at_ps":"1000000000","kind":"pause_request","port":"Net.b.tx","quanta":1000}]});
    let result = f.run(&m, 3_000_000_000);
    oracle(&result, &m, true);
    let all = result["simulation"]["model_records"].as_array().unwrap();
    let loads = all
        .iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.mac_control" && r["data"]["kind"] == "timer_load"
        })
        .collect::<Vec<_>>();
    assert!(!loads.is_empty(), "actual PAUSE ingress missing");
    let start = integer(&loads[0]["time_ps"]);
    let end = start + 512_000_000;
    for r in gp(&result)
        .iter()
        .filter(|r| r["kind"] == "physical_sof" && r["source_clock_identity"] == "0000000000000001")
    {
        let at = integer(&r["global_ps"]);
        assert!(at < start || at >= end, "GP admitted in pause {r}");
    }
    assert!(
        gp(&result)
            .iter()
            .any(|r| r["kind"] == "clock_corrected" && integer(&r["global_ps"]) > end)
    );
}
#[test]
fn configured_released_frame_keeps_reference_when_gate_closes() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"initial_open","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[7]}]});
    m["configured_media"]["physical_links"][0]["a_phy"]["tx_latency_ps"] = json!("2000000");
    m["base"]["config"]["tsn"]["gcl_updates"] = json!([
        {"id":"close","submitted_at_ps":"50000","effective_at_ps":"1000000","port":"Net.a.tx","schedule":{"id":"closed","base_time_ps":"1000000","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[]}]}},
        {"id":"open","submitted_at_ps":"0","effective_at_ps":"5000000","port":"Net.a.tx","schedule":{"id":"opened","base_time_ps":"5000000","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[7]}]}}]);
    let result = f.run(&m, 1_000_000_000);
    oracle(&result, &m, true);
    let rows = gp(&result);
    assert!(rows.iter().any(|r| r["kind"] == "tx_capture"
        && r["source_clock_identity"] == "0000000000000001"
        && integer(&r["global_ps"]) == 2_064_000));
    assert!(rows.iter().any(|r| r["kind"] == "physical_eof"
        && r["source_clock_identity"] == "0000000000000001"
        && integer(&r["global_ps"]) == 640_000));
    assert!(
        result["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["schema_name"] == "ethernet.tsn.gate"
                && r["data"]["schedule_id"] == "closed"
                && integer(&r["time_ps"]) == 1_000_000)
    );
    assert!(!rows.iter().any(|r| r["kind"] == "physical_sof"
        && r["source_clock_identity"] == "0000000000000001"
        && (1_000_000..5_000_000).contains(&integer(&r["global_ps"]))));
}
#[test]
fn configured_express_fresh_restore_after_retirement_and_source_mutants() {
    for owner in ["merge", "abstract"] {
        let f = Fixture::new();
        let m = with_owner(Fixture::model(), owner);
        let ini = f.ini(&m, 1_000_000_000);
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
            if live.advance_one().unwrap() != RunState::Running {
                break;
            }
            let records = snapshot_records(&live);
            let rows = records
                .as_object()
                .unwrap()
                .values()
                .filter(|r| r["schema"]["name"] == "dir.network.gptp")
                .map(|r| &r["data"])
                .collect::<Vec<_>>();
            found = rows
                .iter()
                .any(|r| r["kind"] == "packet_received" && r["message_type"] == 0);
            if found {
                break;
            }
        }
        assert!(found, "no received Sync {owner}");
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(live.poll_control().unwrap(), RunState::Paused);
        let bundle = f.0.join("retired");
        live.checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(bundle.join("state.json")).unwrap()).unwrap();
        let backend = &saved["network_backend"]["state"];
        let packets = backend["gptp"]["packets"].as_object().unwrap();
        let (key, packet) = packets
            .iter()
            .find(|(_, p)| p["message"]["body"] == "Sync" && p["rx_validated"] == true)
            .unwrap();
        let kind = if owner == "merge" {
            "MergeExpress"
        } else {
            "AbstractExpress"
        };
        assert!(packet["mac_source"].get(kind).is_some());
        if owner == "merge" {
            let id = integer(&packet["mac_source"][kind]["packet_id"]).to_string();
            assert!(
                backend["mac_merge"]["packets"].get(&id).is_none(),
                "Merge source not yet pruned"
            );
        }
        for mutation in [
            "source-kind",
            "source-epoch",
            "source-id",
            "source-history",
            "missing-source",
            "duplicate-plane",
        ] {
            let path = f.0.join(mutation);
            copy_tree(&bundle, &path);
            rewrite_checkpoint(&path, |s| {
                let state = &mut s["network_backend"]["state"];
                let old = state["gptp"]["packets"][key]["mac_source"].clone();
                let mut changed = old.clone();
                match mutation {
                    "source-kind" => {
                        changed = json!({"OrdinaryAttempt":{"index":0,"transfer":key.parse::<usize>().unwrap(),"number":1,"direction":0,"physical_link":0,"side":0}});
                    }
                    "source-epoch" => changed[kind]["epoch"] = json!(9999),
                    "source-id" => {
                        if owner == "merge" {
                            changed[kind]["packet_id"] = json!(9999);
                        } else {
                            changed[kind]["transfer"] = json!(9999);
                        }
                    }
                    "source-history" => {}
                    "missing-source" => changed = Value::Null,
                    "duplicate-plane" => {
                        let c = state["gptp"]["packets"][key]["tx"]["configured"].clone();
                        state["gptp"]["packets"][key]["tx"]["express"] = c;
                    }
                    _ => unreachable!(),
                }
                let p = &mut state["gptp"]["packets"][key];
                p["mac_source"] = changed.clone();
                for side in ["tx", "rx"] {
                    if !p[side].is_null() {
                        p[side]["configured"]["source"] = changed.clone();
                    }
                }
                // Coforge matching retained producer identity to exercise the frozen
                // source binding, rather than relying only on duplicate-field checks.
                fn rewrite(v: &mut Value, old: &Value, new: &Value, history: bool) {
                    match v {
                        Value::Object(o) => {
                            if o.get("source") == Some(old) {
                                o.insert("source".into(), new.clone());
                                if history {
                                    o.insert("sof_ps".into(), json!(9999999));
                                }
                            }
                            for value in o.values_mut() {
                                rewrite(value, old, new, history)
                            }
                        }
                        Value::Array(a) => {
                            for value in a {
                                rewrite(value, old, new, history)
                            }
                        }
                        _ => {}
                    }
                }
                rewrite(state, &old, &changed, mutation == "source-history");
            });
            let error = restore_checkpoint(&path, registry.clone(), CheckpointLimits::default())
                .err()
                .unwrap();
            assert_eq!(error.code, "CheckpointInvalid", "{owner}/{mutation}");
            assert!(
                error
                    .message
                    .contains("exact frozen-input committed prefix")
                    || error.message.contains("causal prefix"),
                "coforged source did not reach independent source binding: {owner}/{mutation}: {error:?}"
            );
            eprintln!("{owner}/{mutation}: {}", error.message);
        }
        let result = f.0.join("fresh.json");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "configured_checkpoint_child", "--nocapture"])
            .env("DIR_CONFIGURED_CP", &bundle)
            .env("DIR_CONFIGURED_INI", &ini)
            .env("DIR_CONFIGURED_RESULT", &result)
            .status()
            .unwrap();
        assert!(status.success());
        live.resume(live.pause_token().unwrap()).unwrap();
        assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(result).unwrap()).unwrap(),
            snapshot_records(&live),
            "{owner}"
        );
    }
}
fn local_clock(rows: &[Value], at: i128) -> (i128, i128) {
    let d = &rows
        .iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.clock"
                && r["data"]["node"] == "Net.b"
                && integer(&r["data"]["global_ps"]) <= at
        })
        .max_by_key(|r| integer(&r["data"]["global_ps"]))
        .unwrap()["data"];
    let den = integer(&d["local_denominator"]) * integer(&d["rate_den"]);
    let num = integer(&d["local_numerator"]) * integer(&d["rate_den"])
        + (at - integer(&d["global_ps"]))
            * integer(&d["rate_num"])
            * integer(&d["local_denominator"]);
    (num, den)
}
#[test]
fn configured_packet_servo_drives_actual_tas_psfp_cbs_and_calendar_update() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    let schedule = |id: &str, open: u64| json!({"id":id,"base_time_ps":"0","cycle_time_ps":"100000000","entries":[{"duration_ps":open.to_string(),"open_priorities":[0,7]},{"duration_ps":(100000000-open).to_string(),"open_priorities":[7]}]});
    let tsn = &mut m["base"]["config"]["tsn"];
    tsn["outputs"][1]["tas"] = schedule("configured_initial", 20_000_000);
    tsn["outputs"][1]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"10000","lo_credit_bits":"10000"}]);
    tsn["streams"] = json!([{"id":"configured_psfp","ingress":"Net.b.rx","dst_mac":"02:00:00:00:00:02","vid":10,"priority":7,"max_sdu_bytes":"64","gate":{"base_time_ps":"0","cycle_time_ps":"5000000","entries":[{"duration_ps":"2500000","open":true},{"duration_ps":"2500000","open":false}]},"meter":null}]);
    tsn["gcl_updates"] = json!([{"id":"replace","submitted_at_ps":"2090000000","effective_at_ps":"2100000000","port":"Net.b.tx","schedule":schedule("configured_replaced",30_000_000)}]);
    let generator = |id: &str, node: &str, dst: &str, count: u64, step: u64| json!({"id":id,"node":node,"kind":"ethernet.explicit.v1","times_ps":(0..count).map(|k|(2_000_000_000+k*step).to_string()).collect::<Vec<_>>(),"frame":{"dst_mac":dst,"ether_type":2048,"data":"","tag":null,"ip_multicast":null},"flow_id":id,"priority":0,"deadline_ps":null});
    let w = json!({"schema_version":4,"generators":[generator("psfp","Net.a","02:00:00:00:00:02",36,3_600_000),generator("tas","Net.b","02:00:00:00:00:01",30,1_000_000)],"controls":[]});
    let result = custom_run(&f, &m, &w, 3_000_000_000);
    oracle(&result, &m, true);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let first = gp(&result)
        .iter()
        .filter(|r| r["kind"] == "clock_corrected")
        .map(|r| integer(&r["global_ps"]))
        .min()
        .unwrap();
    assert!(first < 2_000_000_000);
    let (mut pass, mut filter, mut changed, mut starts, mut sends, mut freezes) =
        (0, 0, 0, 0, 0, 0);
    for row in rows {
        let d = &row["data"];
        if row["schema_name"] == "ethernet.dynamic.reception"
            && d["frame_id"]
                .as_str()
                .is_some_and(|s| s.starts_with("psfp:"))
        {
            let at = integer(&d["observed_ps"]);
            let (num, den) = local_clock(rows, at);
            let open = num.rem_euclid(5_000_000 * den) < 2_500_000 * den;
            assert_eq!(d["status"] == "received", open, "PSFP {d}");
            let old =
                (at * 10001 + 1_500_000 * 10000).rem_euclid(5_000_000 * 10000) < 2_500_000 * 10000;
            changed += usize::from(old != open);
            pass += usize::from(open);
            filter += usize::from(!open);
        }
        if row["schema_name"] == "ethernet.dynamic.transfer"
            && d["frame_id"]
                .as_str()
                .is_some_and(|s| s.starts_with("tas:"))
            && !d["sof_ps"].is_null()
        {
            let at = integer(&d["sof_ps"]);
            let (num, den) = local_clock(rows, at);
            let width = if at < 2_100_000_000 {
                20_000_000
            } else {
                30_000_000
            };
            assert!(num.rem_euclid(100_000_000 * den) < width * den, "TAS {d}");
            starts += 1;
        }
        if row["schema_name"] == "ethernet.tsn.credit"
            && d["port"] == "Net.b.tx"
            && d["priority"] == "0"
            && integer(&row["time_ps"]) > first
        {
            let at = integer(&row["time_ps"]);
            let (num, den) = local_clock(rows, at);
            let width = if at < 2_100_000_000 {
                20_000_000
            } else {
                30_000_000
            };
            let open = num.rem_euclid(100_000_000 * den) < width * den;
            let slope = integer(&d["slope_bps"]);
            if slope > 0 {
                assert!(open, "CBS accumulates under closed corrected calendar {d}");
            }
            sends += usize::from(slope < 0);
            freezes += usize::from(!open && slope == 0);
        }
    }
    assert!(
        pass > 0 && filter > 0 && changed > 0 && starts >= 30 && sends > 0 && freezes > 0,
        "pass={pass} filter={filter} changed={changed} starts={starts} sends={sends} freezes={freezes}"
    );
    assert!(rows.iter().any(|r| r["schema_name"] == "ethernet.tsn.gate"
        && r["data"]["schedule_id"] == "configured_replaced"));
}
#[test]
fn configured_public_fixture_matches_independent_first_packet_literals() {
    let f = Fixture::new();
    let m = Fixture::model();
    let result = f.run(&m, 1_000_000);
    oracle(&result, &m, false);
    let expected: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/gptp-configured/expected.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for (side, clock) in [
        ("forward", "0000000000000001"),
        ("reverse", "0000000000000002"),
    ] {
        let rows = gp(&result);
        let tx = rows
            .iter()
            .find(|r| {
                r["kind"] == "tx_capture"
                    && r["source_clock_identity"] == clock
                    && r["message_type"] == 2
            })
            .unwrap();
        let rx = rows
            .iter()
            .find(|r| r["kind"] == "rx_capture" && r["transfer_id"] == tx["transfer_id"])
            .unwrap();
        let e = &expected[side];
        assert_eq!(tx["details"]["reference_ps"], e["tx_reference_ps"]);
        assert_eq!(rx["details"]["reference_ps"], e["rx_reference_ps"]);
        for (r, key) in [(*tx, "tx_raw"), (*rx, "rx_raw")] {
            assert_eq!(
                integer(&r["details"]["raw_numerator"]) * integer(&e[key]["den"]),
                integer(&e[key]["num"]) * integer(&r["details"]["raw_denominator"])
            );
        }
        assert_eq!(
            integer(&tx["details"]["configured"]["eof_planned_ps"]),
            integer(&e["eof_ps"])
        );
        assert_eq!(
            integer(&tx["details"]["configured"]["arrival_planned_ps"]),
            integer(&e["arrival_ps"])
        );
    }
    for name in [
        "model.ini",
        "merge.ini",
        "abstract.ini",
        "abstract-ideal.ini",
    ] {
        prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/gptp-configured")
                .join(name),
        )
        .unwrap();
    }
}
#[test]
fn configured_gptp_relay_composes_with_disjoint_merge_and_abstract_owners() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gptp")
        .canonicalize()
        .unwrap();
    for owner in ["merge", "abstract"] {
        let f = Fixture::new();
        let mut m: Value =
            serde_json::from_slice(&fs::read(root.join("three-node-model.json")).unwrap()).unwrap();
        let mut media = Fixture::model()["configured_media"].clone();
        media["physical_links"][0]["b"] = json!("Net.b.tx_a");
        m["configured_media"] = media;
        m = with_owner(m, owner);
        let key = if owner == "merge" {
            "mac_merge"
        } else {
            "preemption"
        };
        m[key]["outputs"][0]["port"] = json!("Net.b.tx_c");
        m[key]["outputs"][1]["port"] = json!("Net.c.tx");
        if owner == "merge" {
            m[key]["outputs"][0]["source_mac"] = json!("02:00:00:00:00:02");
            m[key]["outputs"][1]["source_mac"] = json!("02:00:00:00:00:03");
        }
        m["limits"]["max_control_events"] = json!(10000);
        let ini = f.ini(&m, 3_000_000_000);
        let text = fs::read_to_string(&ini).unwrap().replace(
            &root.join("models/two").display().to_string(),
            &root.join("models/chain").display().to_string(),
        );
        fs::write(&ini, text).unwrap();
        let out = f.0.join("out");
        let report = run(prepare(&ini).unwrap(), &out).unwrap();
        assert_eq!(
            report.termination, "time_limit",
            "{owner}: {:?}",
            report.primary_diagnostic
        );
        let result: Value =
            serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
        let rows = gp(&result);
        assert!(
            rows.iter()
                .any(|r| r["kind"] == "clock_corrected" && r["details"]["node"] == "Net.c"),
            "relay convergence {owner}"
        );
        assert!(rows.iter().any(|r| {
            r["kind"] == "tx_capture"
                && r["details"]["configured"]["source"]
                    .get("OrdinaryAttempt")
                    .is_some()
        }));
        let kind = if owner == "merge" {
            "MergeExpress"
        } else {
            "AbstractExpress"
        };
        assert!(
            rows.iter().any(|r| r["kind"] == "tx_capture"
                && r["details"]["express"]["source"].get(kind).is_some())
        );
    }
}
#[test]
fn gptp_rejects_preemptable_protocol_class_on_each_serializer() {
    for owner in ["merge", "abstract"] {
        let f = Fixture::new();
        let mut m = with_owner(Fixture::model(), owner);
        let key = if owner == "merge" {
            "mac_merge"
        } else {
            "preemption"
        };
        m[key]["outputs"][0]["express_classes"] = json!([6]);
        m[key]["outputs"][0]["preemptable_classes"] = json!([0, 1, 2, 3, 4, 5, 7]);
        let error = prepare(&f.ini(&m, 1_000_000)).expect_err("fragmented GP admitted");
        assert!(
            error.message.contains("gPTP")
                && error.message.to_ascii_lowercase().contains("express"),
            "{error:?}"
        );
    }
}
#[test]
fn configured_large_rx_latency_preserves_captures_without_inventing_receive() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["configured_media"]["physical_links"][0]["a_phy"]["rx_latency_ps"] = json!("100000000000");
    m["configured_media"]["physical_links"][0]["b_phy"]["rx_latency_ps"] = json!("110000000000");
    m["limits"]["max_control_events"] = json!(10000);
    let result = f.run(&m, 1_000_000_000);
    oracle(&result, &m, false);
    let rows = gp(&result);
    assert!(rows.iter().any(|r| r["kind"] == "rx_capture"));
    assert!(
        !rows
            .iter()
            .any(|r| r["kind"] == "packet_received" || r["kind"] == "clock_corrected"),
        "planned late arrival leaked protocol effects"
    );
}
#[test]
fn configured_zero_propagation_tied_tx_rx_references_are_deterministic() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    let l = &mut m["configured_media"]["physical_links"][0];
    l["a_phy"]["tx_latency_ps"] = json!("1000");
    l["a_phy"]["rx_latency_ps"] = json!("2000");
    l["b_phy"]["tx_latency_ps"] = json!("3000");
    l["b_phy"]["rx_latency_ps"] = json!("4000");
    m["clocks"]["nodes"][1]["rate_num"] = json!("1");
    m["clocks"]["nodes"][1]["rate_den"] = json!("1");
    let ini = f.ini(&m, 1_000_000_000);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gptp/models/two")
        .canonicalize()
        .unwrap();
    let models = f.0.join("models");
    fs::create_dir_all(models.join("ethtsn")).unwrap();
    fs::write(
        models.join("ethtsn/Main.ned"),
        fs::read_to_string(root.join("ethtsn/Main.ned"))
            .unwrap()
            .replace("default(100ns)", "default(0ps)"),
    )
    .unwrap();
    fs::write(
        &ini,
        fs::read_to_string(&ini)
            .unwrap()
            .replace(&root.display().to_string(), &models.display().to_string()),
    )
    .unwrap();
    let out = f.0.join("out");
    let report = run(prepare(&ini).unwrap(), &out).unwrap();
    assert_eq!(report.termination, "time_limit");
    let result: Value =
        serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
    let rows = gp(&result);
    assert!(rows.iter().any(|r| r["kind"] == "clock_corrected"));
    let mut pairs = 0;
    for tx in rows.iter().filter(|r| r["kind"] == "tx_capture") {
        if let Some(rx) = rows
            .iter()
            .find(|r| r["kind"] == "rx_capture" && r["transfer_id"] == tx["transfer_id"])
        {
            assert_eq!(tx["global_ps"], rx["global_ps"]);
            assert_eq!(integer(&tx["details"]["configured"]["propagation_ps"]), 0);
            pairs += 1;
        }
    }
    assert!(pairs > 10);
}
#[test]
fn configured_express_fresh_restore_on_both_sides_of_delayed_join() {
    for owner in ["merge", "abstract"] {
        for cut in ["eof_before_capture", "captured_before_arrival"] {
            let f = Fixture::new();
            let mut m = with_owner(Fixture::model(), owner);
            m["limits"]["max_control_events"] = json!(10000);
            let l = &mut m["configured_media"]["physical_links"][0];
            l["a_phy"]["tx_latency_ps"] = json!("2000000");
            l["b_phy"]["tx_latency_ps"] = json!("3000000");
            l["a_phy"]["rx_latency_ps"] = json!("4000000");
            l["b_phy"]["rx_latency_ps"] = json!("5000000");
            let ini = f.ini(&m, 1_000_000_000);
            let p = prepare(&ini).unwrap();
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
                if live.advance_one().unwrap() != RunState::Running {
                    break;
                }
                let records = snapshot_records(&live);
                let rows = records
                    .as_object()
                    .unwrap()
                    .values()
                    .filter(|r| r["schema"]["name"] == "dir.network.gptp")
                    .map(|r| &r["data"])
                    .collect::<Vec<_>>();
                found = rows.iter().filter(|r| r["message_type"] == 0).any(|r| {
                    let has = |kind: &str| {
                        rows.iter()
                            .any(|x| x["kind"] == kind && x["transfer_id"] == r["transfer_id"])
                    };
                    if cut == "eof_before_capture" {
                        has("physical_eof") && !has("tx_capture")
                    } else {
                        has("tx_capture") && has("rx_capture") && !has("packet_received")
                    }
                });
                if found {
                    break;
                }
            }
            assert!(found, "{owner}/{cut}");
            controller.submit(ControlCommand::Pause).unwrap();
            assert_eq!(live.poll_control().unwrap(), RunState::Paused);
            let bundle = f.0.join("checkpoint");
            live.checkpoint(&bundle, CheckpointLimits::default())
                .unwrap();
            let result = f.0.join("fresh.json");
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "configured_checkpoint_child", "--nocapture"])
                .env("DIR_CONFIGURED_CP", &bundle)
                .env("DIR_CONFIGURED_INI", &ini)
                .env("DIR_CONFIGURED_RESULT", &result)
                .status()
                .unwrap();
            assert!(status.success(), "{owner}/{cut}");
            live.resume(live.pause_token().unwrap()).unwrap();
            assert_eq!(live.run_until_yield().unwrap(), RunState::Completed);
            assert_eq!(
                serde_json::from_slice::<Value>(&fs::read(result).unwrap()).unwrap(),
                snapshot_records(&live),
                "{owner}/{cut}"
            );
        }
    }
}
#[test]
fn configured_real_packet_servo_closes_gate_during_actual_mac_frame_without_cancelling_it() {
    let f = Fixture::new();
    let mut m = Fixture::model();
    m["clocks"]["nodes"][1]["offset_ps"] = json!("500000000");
    m["limits"]["max_control_events"] = json!(10000);
    let tsn = &mut m["base"]["config"]["tsn"];
    tsn["outputs"][1]["tas"] = json!({"id":"gp_startup","base_time_ps":"0","cycle_time_ps":"2000000000","entries":[{"duration_ps":"2000000000","open_priorities":[7]}]});
    tsn["gcl_updates"] = json!([{"id":"data-calendar","submitted_at_ps":"0","effective_at_ps":"200000000","port":"Net.b.tx","schedule":{"id":"servo_sensitive","base_time_ps":"750000000","cycle_time_ps":"2000000000","entries":[{"duration_ps":"1000000000","open_priorities":[1,7]},{"duration_ps":"1000000000","open_priorities":[7]}]}}]);
    let w = json!({"schema_version":4,"generators":[{"id":"clock_step_data","node":"Net.b","kind":"ethernet.explicit.v1","times_ps":(0..180u64).map(|i|(250_000_000+i*10_000_000).to_string()).collect::<Vec<_>>(),"frame":{"dst_mac":"02:00:00:00:00:01","ether_type":2048,"data":"ab".repeat(1400),"tag":null,"ip_multicast":null},"flow_id":"clock_step_data","priority":1,"deadline_ps":null}],"controls":[]});
    let result = custom_run(&f, &m, &w, 3_000_000_000);
    oracle(&result, &m, true);
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let first = gp(&result)
        .into_iter()
        .filter(|r| r["kind"] == "clock_corrected" && r["details"]["node"] == "Net.b")
        .min_by_key(|r| integer(&r["global_ps"]))
        .unwrap();
    let at = integer(&first["global_ps"]);
    let old = 500_000_000 * 10000 + at * 10001;
    assert!(
        old >= 750_000_000 * 10000
            && (old - 750_000_000 * 10000).rem_euclid(2_000_000_000 * 10000)
                < 1_000_000_000 * 10000,
        "initial calendar was not open at real servo"
    );
    let (num, den) = local_clock(rows, at);
    let closed = num < 750_000_000 * den
        || (num - 750_000_000 * den).rem_euclid(2_000_000_000 * den) >= 1_000_000_000 * den;
    assert!(closed, "real packet correction did not close class1");
    let frame = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["frame_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("clock_step_data:"))
                && !r["data"]["eof_ps"].is_null()
                && integer(&r["data"]["sof_ps"]) < at
                && at < integer(&r["data"]["eof_ps"])
        })
        .expect("real servo did not intersect actual MAC frame");
    let d = &frame["data"];
    assert_eq!(
        integer(&d["eof_ps"]) - integer(&d["sof_ps"]),
        (8 + integer(&d["wire"]["mac_bytes"])) * 8000
    );
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "dir.network.configured_media"
                && r["data"]["kind"] == "gate_overrun"
                && r["data"]["work_id"] == frame["record_id"]
                && integer(&r["time_ps"]) == at),
        "no actual in-flight gate overrun evidence"
    );
    assert_eq!(d["status"], "serialized");
}
