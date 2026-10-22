//! Production-path PAUSE/configured-media composition. Expected physical arithmetic is independent.
use dir_simulator::registry::Registry;
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint, run,
};
use serde_json::{Value, json};
use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn temp() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "dir-pause-media-{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn baseline() -> (Value, Value) {
    let p = repo().join("examples/network-composite/pause-media");
    (
        serde_json::from_slice(&fs::read(p.join("model.json")).unwrap()).unwrap(),
        serde_json::from_slice(&fs::read(p.join("workload.json")).unwrap()).unwrap(),
    )
}
fn fixture(p: &Path, m: &Value, w: &Value) {
    fs::write(p.join("model.json"), serde_json::to_vec_pretty(m).unwrap()).unwrap();
    fs::write(
        p.join("workload.json"),
        serde_json::to_vec_pretty(w).unwrap(),
    )
    .unwrap();
    fs::write(p.join("model.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",repo().join("examples/ethernet/dynamic/models").display())).unwrap();
}
fn execute(m: &Value, w: &Value) -> Value {
    let p = temp();
    fixture(&p, m, w);
    run(prepare(&p.join("model.ini")).unwrap(), &p.join("run")).unwrap();
    serde_json::from_slice(&fs::read(p.join("run/results.json")).unwrap()).unwrap()
}
fn rows(v: &Value) -> Vec<&Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .collect()
}
fn controls<'a>(v: &'a Value, kind: &str, port: &str) -> Vec<&'a Value> {
    let mut r: Vec<_> = rows(v)
        .into_iter()
        .filter(|r| {
            r["schema_name"] == "dir.network.mac_control"
                && r["data"]["kind"] == kind
                && r["data"]["port"] == port
        })
        .collect();
    r.sort_by_key(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap());
    r
}
fn sofs(v: &Value, port: &str) -> Vec<u64> {
    let mut found = std::collections::BTreeMap::new();
    for r in rows(v) {
        if r["schema_name"] == "ethernet.dynamic.transfer" && r["data"]["from_port"] == port {
            if let Some(t) = r["data"]["sof_ps"].as_str() {
                found.insert(r["record_id"].to_string(), t.parse().unwrap());
            }
        }
    }
    found.into_values().collect()
}
#[test]
fn sparse_media_pause_uses_actual_bytes_and_latencies_once() {
    let (m, w) = baseline();
    let v = execute(&m, &w);
    let rx = controls(&v, "receive", "Net.a.tx");
    assert_eq!(rx.len(), 1);
    // 64 byte MAC + 8 byte preamble at 1 Gb/s, 1000 ps propagation, peer TX345/RX234.
    let rx_time: u64 = rx[0]["time_ps"].as_str().unwrap().parse().unwrap();
    assert_eq!(rx_time, 576000 + 1000 + 345 + 234);
    let b = rx[0]["data"]["evidence"]["mac_hex"].as_str().unwrap();
    assert_eq!(b.len(), 128);
    assert!(b.starts_with("0180c2000001020000000101880800010004"));
    let load = controls(&v, "timer_load", "Net.a.tx");
    assert_eq!(load[0]["data"]["evidence"]["duration_ps"], "2048000");
    assert_eq!(
        sofs(&v, "Net.a.tx"),
        vec![rx_time + 2048000, rx_time + 2048000 + 704000]
    );
    assert!(
        !sofs(&v, "Net.s1.tx_b").is_empty(),
        "unconfigured direction must remain ideal"
    );
}
#[test]
fn zero_and_short_replacement_override_instead_of_max_or_add() {
    for q in [0u16, 1, 65535] {
        let (mut m, w) = baseline();
        m["mac_control"]["controls"] = json!([
 {"at_ps":"0","port":"Net.s1.tx_a","kind":"pause_request","quanta":100},
 {"at_ps":"672000","port":"Net.s1.tx_a","kind":"pause_request","quanta":q}]);
        let v = execute(&m, &w);
        let rx = controls(&v, "receive", "Net.a.tx");
        assert_eq!(rx.len(), 2);
        let at: u64 = rx[1]["time_ps"].as_str().unwrap().parse().unwrap();
        if q == 65535 {
            assert!(sofs(&v, "Net.a.tx").is_empty());
        } else {
            assert_eq!(sofs(&v, "Net.a.tx")[0], at + u64::from(q) * 512000);
        }
    }
}
#[test]
fn receive_during_frame_loads_at_mac_eof_before_ifg() {
    let (mut m, mut w) = baseline();
    w["generators"][0]["times_ps"] = json!(["0", "100"]);
    w["generators"][0]["frame"]["data"] = "ab".repeat(100).into();
    m["mac_control"]["controls"][0]["at_ps"] = "100".into();
    let v = execute(&m, &w);
    let rx = controls(&v, "receive", "Net.a.tx");
    assert_eq!(rx[0]["data"]["evidence"]["pending_mac_request"], true);
    let load = controls(&v, "timer_load", "Net.a.tx");
    assert_eq!(load[0]["time_ps"], "1040000");
    let starts = sofs(&v, "Net.a.tx");
    assert_eq!(starts, [0, 3088000]);
}
#[test]
fn pause_transmits_while_paused_and_closed_data_gates() {
    let (mut m, w) = baseline();
    m["mac_control"]["controls"] = json!([
 {"at_ps":"0","port":"Net.s1.tx_a","kind":"pause_request","quanta":10},
 {"at_ps":"800000","port":"Net.a.tx","kind":"pause_request","quanta":1}]);
    m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"closed","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[]}]});
    let v = execute(&m, &w);
    let starts = controls(&v, "tx_started", "Net.a.tx");
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["time_ps"], "800000");
    assert_eq!(controls(&v, "receive", "Net.s1.tx_a").len(), 1);
}
#[test]
fn scoped_validation_and_administration() {
    let (mut m, w) = baseline();
    m["mac_control"]["ports"][0]["pfc_enabled"] = true.into();
    let p = temp();
    fixture(&p, &m, &w);
    assert!(prepare(&p.join("model.ini")).is_err());
    let (mut m, w) = baseline();
    m["mac_control"]["controls"] = json!([
 {"at_ps":"0","port":"Net.s1.tx_a","kind":"pause_request","quanta":10},
 {"at_ps":"900000","port":"Net.a.tx","kind":"rx_enable","enabled":false}]);
    let v = execute(&m, &w);
    assert_eq!(sofs(&v, "Net.a.tx")[0], 900000);
}
#[test]
fn pause_checkpoint_process_worker() {
    let Ok(mode) = std::env::var("DIR_PAUSE_CP_MODE") else {
        return;
    };
    let p = PathBuf::from(std::env::var_os("DIR_PAUSE_CP_ROOT").unwrap());
    if mode == "save" {
        let (mut s, c) = RunSession::new(
            prepare(&p.join("model.ini")).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let n: usize = std::env::var("DIR_PAUSE_CP_N")
            .unwrap_or_else(|_| "0".into())
            .parse()
            .unwrap();
        if let Ok(kind) = std::env::var("DIR_PAUSE_CP_UNTIL") {
            let mut found = false;
            for _ in 0..4096 {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
                let snap = s.observe().unwrap();
                let net = snap.network.as_ref().unwrap();
                let media = &net["configured_media"];
                let ports = net["mac_control"]["ports"].as_array();
                found = match kind.as_str() {
                    "jam" => media["attempts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|a| a["status"] == "jamming"),
                    "backoff" => media["attempts"].as_array().unwrap().iter().any(|a| {
                        a["status"] == "collided"
                            && a["backoff_until"].as_str().is_some_and(|t| {
                                t.parse::<u64>().unwrap() > snap.common.last_event_time_ps.unwrap()
                            })
                    }),
                    "defer" => {
                        media["pairs"].as_array().unwrap().iter().any(|p| {
                            p["macs"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|m| !m["carrier"].is_null() && m["emitting"].is_null())
                        }) && media["retained"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|w| !w.is_null())
                            .count()
                            == 2
                    }
                    "pending" => ports
                        .unwrap()
                        .iter()
                        .any(|p| !p.is_null() && !p["pending"].is_null()),
                    "active" => ports
                        .unwrap()
                        .iter()
                        .any(|p| !p.is_null() && !p["timer"].is_null()),
                    "control" => {
                        ports.unwrap().iter().any(|p| p["ordinal"] == "1")
                            && media["attempts"].as_array().unwrap().is_empty()
                    }
                    _ => panic!("unknown cut {kind}"),
                };
                if found {
                    break;
                }
            }
            assert!(found, "missing representative {kind}");
        } else {
            for _ in 0..n {
                assert_eq!(s.advance_one().unwrap(), RunState::Running);
            }
        }
        c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        let ack = s
            .checkpoint(&p.join("cp"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let manifest: Value =
            serde_json::from_slice(&fs::read(p.join("cp/manifest.json")).unwrap()).unwrap();
        assert!(
            manifest["codec_catalog"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["codec_name"] == "dir.network.owned-state" && c["codec_version"] == 19)
        );
        s.resume(s.pause_token().unwrap()).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&p.join("original")).unwrap().status,
            ControlStatus::Applied
        );
    } else {
        let (mut s, token) = restore_checkpoint(
            &p.join("cp"),
            Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        s.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            s.export(&p.join("restored")).unwrap().status,
            ControlStatus::Applied
        );
    }
}
#[test]
fn fresh_process_control_pending_pause_and_media_resume() {
    for n in [1, 3, 5, 8, 12, 18] {
        let p = temp();
        let (m, w) = baseline();
        fixture(&p, &m, &w);
        for mode in ["save", "restore"] {
            let o = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "pause_checkpoint_process_worker", "--nocapture"])
                .env("DIR_PAUSE_CP_MODE", mode)
                .env("DIR_PAUSE_CP_ROOT", &p)
                .env("DIR_PAUSE_CP_N", n.to_string())
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{mode} n={n}: {} {}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        let read = |name| -> Value {
            serde_json::from_slice(&fs::read(p.join(name).join("results.json")).unwrap()).unwrap()
        };
        let a = read("original");
        let b = read("restored");
        assert_eq!(
            a["simulation"]["model_records"],
            b["simulation"]["model_records"]
        );
    }
}
#[test]
fn old_timer_runs_while_request_pending_and_zero_loads_only_at_mac_completion() {
    for q in [0u16, 1] {
        let (mut m, w) = baseline();
        m["mac_control"]["controls"] = json!([
 {"at_ps":"0","port":"Net.s1.tx_a","kind":"pause_request","quanta":10},
 {"at_ps":"672000","port":"Net.s1.tx_a","kind":"pause_request","quanta":q},
 {"at_ps":"800000","port":"Net.a.tx","kind":"pause_request","quanta":1}]);
        let v = execute(&m, &w);
        let rx = controls(&v, "receive", "Net.a.tx");
        assert_eq!(rx.len(), 2);
        assert_eq!(rx[1]["data"]["evidence"]["pending_mac_request"], true);
        assert_eq!(rx[1]["data"]["state"]["timer"]["expires_ps"], "5697579");
        let load = controls(&v, "timer_load", "Net.a.tx");
        assert_eq!(load.len(), 2);
        assert_eq!(load[1]["time_ps"], "1376000");
        assert_eq!(
            sofs(&v, "Net.a.tx")[0],
            (1376000 + u64::from(q) * 512000).max(1472000)
        );
    }
}
#[test]
fn receive_at_old_expiry_settles_before_phase_two_selection() {
    for q in [0u16, 1] {
        let (mut m, w) = baseline();
        m["mac_control"]["controls"] = json!([
 {"at_ps":"0","port":"Net.s1.tx_a","kind":"pause_request","quanta":2},
 {"at_ps":"1024000","port":"Net.s1.tx_a","kind":"pause_request","quanta":q}]);
        let v = execute(&m, &w);
        let rx = controls(&v, "receive", "Net.a.tx");
        assert_eq!(rx[1]["time_ps"], "1601579");
        assert_eq!(sofs(&v, "Net.a.tx")[0], 1601579 + u64::from(q) * 512000);
    }
}
fn half_fixture(p: &Path, simultaneous: bool) -> (Value, Value) {
    let mut m: Value =
        serde_json::from_slice(&fs::read(repo().join("examples/mac-merge/model.json")).unwrap())
            .unwrap();
    m.as_object_mut().unwrap().remove("mac_merge");
    m["configured_media"] = json!({"seed":"1","physical_links":[{"id":"ab","a":"Net.a.tx","b":"Net.b.tx","phy_mode":"100base-tx","duplex":"half","a_phy":{"role":"none","tx_latency_ps":"0","rx_latency_ps":"0"},"b_phy":{"role":"none","tx_latency_ps":"0","rx_latency_ps":"0"}}]});
    let generator = |id: &str, node: &str, dst: &str, times: Value| json!({"id":id,"node":node,"kind":"ethernet.explicit.v1","times_ps":times,"frame":{"dst_mac":dst,"ether_type":2048,"data":"","tag":null,"ip_multicast":null},"flow_id":id,"priority":0,"deadline_ps":null});
    let w = json!({"schema_version":4,"generators":[generator("ga","Net.a","02:00:00:00:00:02",json!(["0"])),generator("gb","Net.b","02:00:00:00:00:01",if simultaneous{json!(["0"])}else{json!(["1000000"])} )],"controls":[]});
    let dir = p.join("models");
    fs::create_dir_all(dir.join("ethtsn")).unwrap();
    let ned = fs::read_to_string(repo().join("examples/ethernet/tsn/models/ethtsn/Main.ned"))
        .unwrap()
        .replace("default(1Gbps)", "default(100Mbps)")
        .replace("default(0ps)", "default(100000ps)"); // endpoint processing is restored separately
    let ned = ned
        .replace(
            "txProcessingDelay @unit(s) = default(100000ps)",
            "txProcessingDelay @unit(s) = default(0ps)",
        )
        .replace(
            "rxProcessingDelay @unit(s) = default(100000ps)",
            "rxProcessingDelay @unit(s) = default(0ps)",
        );
    fs::write(dir.join("ethtsn/Main.ned"), ned).unwrap();
    fixture(p, &m, &w);
    let ini = fs::read_to_string(p.join("model.ini"))
        .unwrap()
        .replace("ethdynamic.Net", "ethtsn.Net")
        .replace(
            &repo()
                .join("examples/ethernet/dynamic/models")
                .display()
                .to_string(),
            &dir.display().to_string(),
        )
        .replace("10000000ps", "200000000ps");
    fs::write(p.join("model.ini"), ini).unwrap();
    (m, w)
}
#[test]
fn configured_half_duplex_collision_jam_backoff_and_carrier_deference() {
    for simultaneous in [true, false] {
        let p = temp();
        half_fixture(&p, simultaneous);
        run(prepare(&p.join("model.ini")).unwrap(), &p.join("run")).unwrap();
        let v: Value =
            serde_json::from_slice(&fs::read(p.join("run/results.json")).unwrap()).unwrap();
        let attempts = rows(&v)
            .into_iter()
            .filter(|r| r["schema_name"] == "dir.network.configured_media")
            .collect::<Vec<_>>();
        if simultaneous {
            let jam = attempts
                .iter()
                .filter(|r| r["data"]["kind"] == "jam_end" && r["data"]["attempt"]["number"] == "1")
                .collect::<Vec<_>>();
            assert_eq!(jam.len(), 2);
            for r in jam {
                assert_eq!(r["data"]["attempt"]["collision"], "100000");
                assert_eq!(r["data"]["attempt"]["jam_end"], "960000");
            }
            assert!(
                attempts
                    .iter()
                    .any(|r| r["data"]["kind"] == "start" && r["data"]["attempt"]["number"] == "2")
            );
        } else {
            assert!(!attempts.iter().any(|r| r["data"]["kind"] == "jam_end"));
            assert_eq!(sofs(&v, "Net.b.tx")[0], 6820000);
        }
        assert!(
            rows(&v)
                .iter()
                .any(|r| r["schema_name"] == "ethernet.dynamic.reception"
                    && r["data"]["status"] == "received")
        );
    }
}
#[test]
fn cbs_open_backlog_accumulates_during_pause_and_closed_gate_freezes() {
    let (mut m, w) = baseline();
    m["mac_control"]["controls"][0]["quanta"] = 10.into();
    m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"100000","lo_credit_bits":"100000"}]);
    m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"closed_then_open","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"1500000","open_priorities":[]},{"duration_ps":"8500000","open_priorities":[0]}]});
    let v = execute(&m, &w);
    let start = sofs(&v, "Net.a.tx")[0];
    assert_eq!(start, 5697579);
    let start_text = start.to_string();
    let credit = rows(&v)
        .into_iter()
        .filter(|r| r["schema_name"] == "ethernet.tsn.credit" && r["data"]["port"] == "Net.a.tx")
        .collect::<Vec<_>>();
    let at = credit
        .iter()
        .find(|r| {
            r["time_ps"].as_str() == Some(start_text.as_str())
                && r["data"]["slope_bps"] == "-750000000"
        })
        .unwrap_or_else(|| panic!("credit records: {credit:?}"));
    assert_eq!(
        at["data"]["magnitude"],
        (u128::from(start - 1500000) * 250000000u128).to_string()
    );
    assert!(
        credit
            .iter()
            .any(|r| r["time_ps"] == "1500000" && r["data"]["magnitude"] == "0")
    );
}
#[test]
fn sparse_ideal_configured_and_sampled_outputs_share_one_fes() {
    let (mut m, w) = baseline();
    let sampled: Value = serde_json::from_slice(
        &fs::read(repo().join("examples/network-composite/sampled-phy-model.json")).unwrap(),
    )
    .unwrap();
    m["phy_attachments"] = sampled["phy_attachments"].clone();
    m["configured_media"]["physical_links"][0]["a"] = "Net.c.tx".into();
    m["configured_media"]["physical_links"][0]["b"] = "Net.s3.tx_a".into();
    m["mac_control"]["controls"] =
        json!([{"at_ps":"1000000","port":"Net.a.tx","kind":"pause_request","quanta":1}]);
    let v = execute(&m, &w);
    assert_eq!(controls(&v, "receive", "Net.s1.tx_a").len(), 1);
    assert!(
        rows(&v)
            .iter()
            .any(|r| r["schema_name"] == "dir.network.phy")
    );
    assert!(!sofs(&v, "Net.s1.tx_b").is_empty());
}
#[test]
fn mac_control_bypasses_actual_negative_cbs_and_closed_gate() {
    let (mut m, mut w) = baseline();
    w["generators"][0]["times_ps"] = json!(["0", "900000"]);
    m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"close_after_frame","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"800000","open_priorities":[0]},{"duration_ps":"1200000","open_priorities":[]},{"duration_ps":"8000000","open_priorities":[0]}]});
    m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"100000","lo_credit_bits":"100000"}]);
    m["mac_control"]["controls"] = json!([{"at_ps":"0","port":"Net.s1.tx_a","kind":"pause_request","quanta":10},{"at_ps":"800000","port":"Net.a.tx","kind":"pause_request","quanta":1}]);
    let v = execute(&m, &w);
    assert_eq!(
        controls(&v, "tx_started", "Net.a.tx")[0]["time_ps"],
        "800000"
    );
    assert!(
        rows(&v)
            .into_iter()
            .any(|r| r["schema_name"] == "ethernet.tsn.credit"
                && r["data"]["port"] == "Net.a.tx"
                && r["time_ps"] == "800000"
                && r["data"]["sign"] == "negative")
    );
}
#[test]
fn jointly_rehashed_pause_bytes_receive_timer_and_due_mutant_rejects() {
    use sha2::{Digest, Sha256};
    let p = temp();
    let (m, w) = baseline();
    fixture(&p, &m, &w);
    let (mut s, c) = RunSession::new(
        prepare(&p.join("model.ini")).unwrap(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    loop {
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
        if s.observe()
            .unwrap()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "dir.network.mac_control" && r.data["kind"] == "timer_load")
        {
            break;
        }
    }
    c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    assert_eq!(
        s.checkpoint(&p.join("cp"), CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let mut saved: Value =
        serde_json::from_slice(&fs::read(p.join("cp/state.json")).unwrap()).unwrap();
    let backend = &saved["network_backend"]["state"];
    assert_eq!(backend["version"], 19);
    let port = backend["mac_control"]["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| !p["timer"].is_null())
        .unwrap();
    let old_end = port["timer"]["expires_ps"].as_u64().unwrap();
    let txs = backend["wire"]["host"]["tx"].as_object().unwrap();
    let old_bytes = txs.values().next().unwrap()["intent"]["bytes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_u64().unwrap() as u8)
        .collect::<Vec<_>>();
    let old_hex = old_bytes.iter().fold(String::new(), |mut text, byte| {
        write!(&mut text, "{byte:02x}").unwrap();
        text
    });
    let mut body = old_bytes[..60].to_vec();
    body[17] = 5;
    let body_hex = body.iter().fold(String::new(), |mut text, byte| {
        write!(&mut text, "{byte:02x}").unwrap();
        text
    });
    let o=Command::new("python3").args(["-c","import sys,zlib; b=bytes.fromhex(sys.argv[1]); print((b+zlib.crc32(b).to_bytes(4,'little')).hex())",&body_hex]).output().unwrap();
    assert!(o.status.success());
    let new_hex = String::from_utf8(o.stdout).unwrap().trim().to_owned();
    let new_bytes = (0..64)
        .map(|i| u8::from_str_radix(&new_hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>();
    fn replace(
        v: &mut Value,
        old_end: u64,
        old_hex: &str,
        new_hex: &str,
        old_bytes: &[u8],
        new_bytes: &[u8],
    ) {
        if *v == json!(old_bytes) {
            *v = json!(new_bytes);
            return;
        }
        match v {
            Value::String(s) if s == old_hex => *s = new_hex.to_owned(),
            Value::Array(a) => {
                for v in a {
                    replace(v, old_end, old_hex, new_hex, old_bytes, new_bytes)
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    if k == "quanta" && (*v == json!(4) || *v == json!("4")) {
                        *v = if v.is_string() { json!("5") } else { json!(5) }
                    } else if k == "expires_ps"
                        && (*v == json!(old_end) || *v == json!(old_end.to_string()))
                    {
                        *v = if v.is_string() {
                            json!((old_end + 512000).to_string())
                        } else {
                            json!(old_end + 512000)
                        }
                    } else if k == "duration_ps" && *v == "2048000" {
                        *v = json!("2560000")
                    } else {
                        replace(v, old_end, old_hex, new_hex, old_bytes, new_bytes)
                    }
                }
            }
            _ => {}
        }
    }
    replace(
        &mut saved, old_end, &old_hex, &new_hex, &old_bytes, &new_bytes,
    );
    for due in saved["network_backend"]["state"]["core"]["due"]
        .as_array_mut()
        .unwrap()
    {
        if due["kind"].get("PauseExpire").is_some() {
            due["at"] = json!(old_end + 512000);
        }
    }
    let bytes = serde_json::to_vec(&saved).unwrap();
    fs::write(p.join("cp/state.json"), &bytes).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(p.join("cp/manifest.json")).unwrap()).unwrap();
    let file = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|x| x["path"] == "state.json")
        .unwrap();
    file["bytes"] = json!(bytes.len().to_string());
    file["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    fs::write(
        p.join("cp/manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let error = match restore_checkpoint(
        &p.join("cp"),
        Registry::default(),
        CheckpointLimits::default(),
    ) {
        Ok(_) => panic!("accepted invented actual PAUSE receive/timer prefix"),
        Err(e) => e,
    };
    assert!(
        error.message.contains("causal") || error.message.contains("frozen-input"),
        "{}",
        error.message
    );
}
#[test]
fn fresh_process_representative_defer_jam_backoff_pending_active_and_control() {
    for kind in ["defer", "jam", "backoff", "pending", "active", "control"] {
        let p = temp();
        if matches!(kind, "defer" | "jam" | "backoff") {
            half_fixture(&p, kind != "defer");
        } else {
            let (mut m, mut w) = baseline();
            if kind == "pending" {
                w["generators"][0]["times_ps"] = json!(["0", "100"]);
                w["generators"][0]["frame"]["data"] = "ab".repeat(100).into();
                m["mac_control"]["controls"][0]["at_ps"] = "100".into();
            }
            fixture(&p, &m, &w);
        }
        for mode in ["save", "restore"] {
            let o = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "pause_checkpoint_process_worker", "--nocapture"])
                .env("DIR_PAUSE_CP_MODE", mode)
                .env("DIR_PAUSE_CP_ROOT", &p)
                .env("DIR_PAUSE_CP_UNTIL", kind)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{kind}/{mode}: {} {}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        let read = |name| -> Value {
            serde_json::from_slice(&fs::read(p.join(name).join("results.json")).unwrap()).unwrap()
        };
        assert_eq!(
            read("original")["simulation"]["model_records"],
            read("restored")["simulation"]["model_records"]
        );
    }
}
#[test]
fn sampled_mac_fcs_completion_loads_pending_pause_before_coded_tail_release() {
    let p = temp();
    let source = repo().join("examples/network-composite/pause-media/sampled.ini");
    run(prepare(&source).unwrap(), &p.join("run")).unwrap();
    let v: Value = serde_json::from_slice(&fs::read(p.join("run/results.json")).unwrap()).unwrap();
    let rx = controls(&v, "receive", "Net.a.tx");
    assert_eq!(rx.len(), 1);
    assert_eq!(rx[0]["data"]["evidence"]["pending_mac_request"], true);
    let loads = controls(&v, "timer_load", "Net.a.tx");
    assert_eq!(loads.len(), 1);
    let load = loads[0]["time_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let transfer = rows(&v)
        .into_iter()
        .find(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["from_port"] == "Net.a.tx"
                && r["data"]["queued_ps"] == "6000000000"
        })
        .unwrap();
    assert_eq!(transfer["data"]["eof_ps"], load.to_string());
    let release = transfer["data"]["release_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    assert!(
        load < release,
        "MAC request must finish before PCS/IFG release"
    );
    assert_eq!(loads[0]["data"]["evidence"]["duration_ps"], "5120000");
}
#[test]
fn ieee_generation_prohibitions_are_targeted_and_merge_receive_is_explicit() {
    let p = temp();
    let (mut m, w) = half_fixture(&p, false);
    m["mac_control"] = json!({"ports":[{"port":"Net.a.tx","station_mac":"02:00:00:00:00:01","rx_enabled":true,"tx_enabled":true,"policy":"ieee802.3.pause.v1"}],"controls":[]});
    fs::write(p.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    let e = prepare(&p.join("model.ini")).unwrap_err();
    assert_eq!(e.target.as_deref(), Some("/mac_control/ports"));
    assert!(e.message.contains("full duplex"));
    m["mac_control"]["ports"][0]["tx_enabled"] = false.into();
    fs::write(p.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    prepare(&p.join("model.ini")).unwrap();
    let _ = w;
    let (mut m, w) = baseline();
    let template: Value =
        serde_json::from_slice(&fs::read(repo().join("examples/mac-merge/model.json")).unwrap())
            .unwrap();
    m["mac_merge"] = template["mac_merge"].clone();
    m["mac_merge"]["outputs"][1]["port"] = "Net.s1.tx_a".into();
    let p = temp();
    fixture(&p, &m, &w);
    let e = prepare(&p.join("model.ini")).unwrap_err();
    assert_eq!(e.target.as_deref(), Some("/mac_control/ports"));
    assert!(e.message.contains("Clause99"));
    for row in m["mac_control"]["ports"].as_array_mut().unwrap() {
        row["tx_enabled"] = false.into();
        row["policy"] = "dir.experimental.merge_pause.v1".into();
    }
    m["mac_control"]["controls"] = json!([]);
    fixture(&p, &m, &w);
    prepare(&p.join("model.ini")).unwrap();
    let (mut m, w) = baseline();
    m["mac_merge"] = template["mac_merge"].clone();
    m["mac_merge"]["outputs"][1]["port"] = "Net.s1.tx_a".into();
    m["mac_control"]["ports"][0]["port"] = "Net.c.tx".into();
    m["mac_control"]["ports"][1]["port"] = "Net.s3.tx_a".into();
    m["mac_control"]["controls"][0]["port"] = "Net.s3.tx_a".into();
    fixture(&p, &m, &w);
    prepare(&p.join("model.ini")).unwrap();
}
#[test]
fn retained_collision_retry_survives_gate_close_and_records_actual_overrun() {
    let p = temp();
    let (mut m, _) = half_fixture(&p, true);
    m["physical_mac_policy"] = "dir.experimental.half_duplex_tsn.v1".into();
    m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"retry_gate","base_time_ps":"0","cycle_time_ps":"100000000","entries":[{"duration_ps":"10000000","open_priorities":[0]},{"duration_ps":"90000000","open_priorities":[]}]});
    m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"25000000","hi_credit_bits":"100000","lo_credit_bits":"100000"}]);
    fs::write(p.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    run(prepare(&p.join("model.ini")).unwrap(), &p.join("run")).unwrap();
    let v: Value = serde_json::from_slice(&fs::read(p.join("run/results.json")).unwrap()).unwrap();
    assert!(
        rows(&v)
            .into_iter()
            .any(|r| r["schema_name"] == "dir.network.configured_media"
                && r["data"]["kind"] == "gate_overrun")
    );
    let t = rows(&v)
        .into_iter()
        .find(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer" && r["data"]["from_port"] == "Net.a.tx"
        })
        .unwrap();
    assert!(!t["data"]["arrival_ps"].is_null());
    assert!(
        t["data"]["media"]["attempt_count"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 1
    );
}
#[test]
fn blocked_ordinary_protocol_fifo_head_does_not_block_mac_control() {
    let (mut m, w) = baseline();
    let wire: Value = serde_json::from_slice(
        &fs::read(repo().join("examples/ethernet-wire/active-model.json")).unwrap(),
    )
    .unwrap();
    let adapters = wire["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["protocol"] == "rstp")
        .cloned()
        .collect::<Vec<_>>();
    m["base"]["config"]["base"]["config"]["dynamic"]["convergence_ps"] = "0".into();
    m["wire_control"] =
        json!({"port_identities":wire["port_identities"],"adapters":adapters,"packets":[]});
    let output = m["base"]["config"]["tsn"]["outputs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["port"] == "Net.s1.tx_a")
        .unwrap();
    output["tas"] = json!({"id":"closed_protocol","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[]}]});
    let v = execute(&m, &w);
    assert_eq!(controls(&v, "tx_started", "Net.s1.tx_a").len(), 1);
    assert_eq!(controls(&v, "receive", "Net.a.tx").len(), 1);
    let wire = rows(&v)
        .into_iter()
        .filter(|r| {
            r["schema_name"] == "ethernet.control.wire.event" && r["data"]["port"] == "Net.s1.tx_a"
        })
        .collect::<Vec<_>>();
    assert!(wire.iter().any(|r| r["data"]["event_kind"] == "tx_queued"));
    assert!(!wire.iter().any(|r| r["data"]["event_kind"] == "tx_started"));
}
#[test]
fn pause_timer_uses_physical_clock_and_cbs_settles_clock_step_gate_windows() {
    let (mut m, w) = baseline();
    m["mac_control"]["controls"][0]["quanta"] = 10.into();
    m["clocks"] = json!({"nodes":[{"node":"Net.a","offset_ps":"0","rate_num":"1","rate_den":"1"}],"adjustments":[{"node":"Net.a","at_ps":"1000000","kind":"step","offset_ps":"1000000"}]});
    m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"100000","lo_credit_bits":"100000"}]);
    m["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"clock_gate","base_time_ps":"0","cycle_time_ps":"10000000","entries":[{"duration_ps":"2000000","open_priorities":[0]},{"duration_ps":"1000000","open_priorities":[]},{"duration_ps":"7000000","open_priorities":[0]}]});
    let v = execute(&m, &w);
    let start = sofs(&v, "Net.a.tx")[0];
    assert_eq!(start, 5697579);
    let start_text = start.to_string();
    let credit = rows(&v)
        .into_iter()
        .find(|r| {
            r["schema_name"] == "ethernet.tsn.credit"
                && r["data"]["port"] == "Net.a.tx"
                && r["time_ps"].as_str() == Some(start_text.as_str())
                && r["data"]["slope_bps"] == "-750000000"
        })
        .unwrap();
    assert_eq!(
        credit["data"]["magnitude"],
        (u128::from(start - 800000 - 1000000) * 250000000u128).to_string()
    );
}
#[test]
fn gcl_replacement_during_pause_freezes_existing_credit_at_effective_cut() {
    let (mut m, w) = baseline();
    m["mac_control"]["controls"][0]["quanta"] = 10.into();
    m["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"100000","lo_credit_bits":"100000"}]);
    m["base"]["config"]["tsn"]["gcl_updates"] = json!([{"id":"close_during_pause","submitted_at_ps":"0","effective_at_ps":"1500000","port":"Net.a.tx","schedule":{"id":"new_closed_gate","base_time_ps":"1500000","cycle_time_ps":"10000000","entries":[{"duration_ps":"10000000","open_priorities":[]}]}}]);
    let v = execute(&m, &w);
    assert!(sofs(&v, "Net.a.tx").is_empty());
    let credits = rows(&v)
        .into_iter()
        .filter(|r| r["schema_name"] == "ethernet.tsn.credit" && r["data"]["port"] == "Net.a.tx")
        .collect::<Vec<_>>();
    let at = credits.iter().find(|r| r["time_ps"] == "1500000").unwrap();
    assert_eq!(at["data"]["magnitude"], "175000000000000");
    assert_eq!(at["data"]["slope_bps"], "0");
    assert!(
        credits
            .iter()
            .filter(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap() > 1500000)
            .all(|r| r["data"]["magnitude"] == "175000000000000")
    );
}
#[test]
fn link_epoch_discards_old_inflight_pause_and_link_up_starts_unpaused() {
    let (m, mut w) = baseline();
    w["controls"] = json!([{"id":"down","at_ps":"100000","kind":"link_set","link":"A","up":false},{"id":"up","at_ps":"200000","kind":"link_set","link":"A","up":true}]);
    let v = execute(&m, &w);
    assert!(controls(&v, "timer_load", "Net.a.tx").is_empty());
    assert_eq!(sofs(&v, "Net.a.tx")[0], 800000);
    let stale = controls(&v, "receive", "Net.s1.tx_a");
    assert!(
        stale
            .iter()
            .any(|r| r["data"]["evidence"]["reason"] == "stale_link_epoch")
    );
}
#[test]
fn administrative_disable_cancels_pending_receive_and_queued_generation() {
    let (mut m, mut w) = baseline();
    w["generators"][0]["times_ps"] = json!(["0", "100"]);
    w["generators"][0]["frame"]["data"] = "ab".repeat(100).into();
    m["mac_control"]["controls"] = json!([{"at_ps":"100","port":"Net.s1.tx_a","kind":"pause_request","quanta":10},{"at_ps":"600000","port":"Net.a.tx","kind":"rx_enable","enabled":false}]);
    let v = execute(&m, &w);
    assert!(controls(&v, "timer_load", "Net.a.tx").is_empty());
    assert_eq!(sofs(&v, "Net.a.tx"), [0, 1136000]);
    let (mut m, mut w) = baseline();
    w["generators"][0]["times_ps"] = json!(["0"]);
    m["mac_control"]["controls"] = json!([{"at_ps":"100000","port":"Net.a.tx","kind":"pause_request","quanta":1},{"at_ps":"200000","port":"Net.a.tx","kind":"tx_enable","enabled":false}]);
    let v = execute(&m, &w);
    assert_eq!(controls(&v, "tx_queued", "Net.a.tx").len(), 1);
    assert!(controls(&v, "tx_started", "Net.a.tx").is_empty());
    let cancel = controls(&v, "tx_completed", "Net.a.tx");
    assert_eq!(
        cancel[0]["data"]["evidence"]["reason"],
        "generation_disabled"
    );
}
#[test]
fn fresh_process_sampled_pending_receive_restores_actual_control_proof() {
    let p = temp();
    let source = repo().join("examples/network-composite/pause-media/sampled.ini");
    let mut ini = fs::read_to_string(&source).unwrap();
    for key in ["ned-path", "model-config", "workload"] {
        let line = ini.lines().find(|l| l.starts_with(key)).unwrap().to_owned();
        let raw = line.split_once('=').unwrap().1.trim().trim_matches('"');
        let absolute = source.parent().unwrap().join(raw).canonicalize().unwrap();
        ini = ini.replace(&line, &format!("{key} = \"{}\"", absolute.display()));
    }
    fs::write(p.join("model.ini"), ini).unwrap();
    for mode in ["save", "restore"] {
        let o = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pause_checkpoint_process_worker", "--nocapture"])
            .env("DIR_PAUSE_CP_MODE", mode)
            .env("DIR_PAUSE_CP_ROOT", &p)
            .env("DIR_PAUSE_CP_UNTIL", "pending")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "sampled/{mode}: {} {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let read = |name| -> Value {
        serde_json::from_slice(&fs::read(p.join(name).join("results.json")).unwrap()).unwrap()
    };
    assert_eq!(
        read("original")["simulation"]["model_records"],
        read("restored")["simulation"]["model_records"]
    );
}
#[test]
fn numerical_phy_pause_quanta_use_exact_manchester_mac_rate() {
    let (mut m, w) = baseline();
    let sample: Value = serde_json::from_slice(
        &fs::read(repo().join("examples/network-composite/sampled-phy-model.json")).unwrap(),
    )
    .unwrap();
    m["phy_attachments"] = sample["phy_attachments"].clone();
    m["configured_media"]["physical_links"][0]["a"] = "Net.c.tx".into();
    m["configured_media"]["physical_links"][0]["b"] = "Net.s3.tx_a".into();
    m["mac_control"]["controls"][0]["quanta"] = 1.into();
    let v = execute(&m, &w);
    let load = controls(&v, "timer_load", "Net.a.tx");
    assert_eq!(load[0]["data"]["evidence"]["duration_ps"], "1024000");
    assert_eq!(
        load[0]["data"]["evidence"]["physical_rate_num"],
        "1000000000"
    );
    assert_eq!(load[0]["data"]["evidence"]["physical_rate_den"], "2");
}
#[test]
fn raw_mac_client_control_bytes_are_consumed_before_l2_learning() {
    let (mut m, mut w) = baseline();
    m["mac_control"]["controls"] = json!([]);
    w["generators"][0]["times_ps"] = json!(["0"]);
    w["generators"][0]["frame"]["ether_type"] = 0x8808.into();
    w["generators"][0]["frame"]["dst_mac"] = "02:00:00:00:01:01".into();
    w["generators"][0]["frame"]["data"] = "00010001".into();
    w["generators"][0]["frame"]["tag"] = Value::Null;
    let port = m["base"]["config"]["base"]["config"]["ports"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["port"] == "Net.a.tx")
        .unwrap();
    port["vlans"][0]["tagged"] = false.into();
    let v = execute(&m, &w);
    let rx = controls(&v, "receive", "Net.s1.tx_a");
    assert_eq!(rx.len(), 1);
    assert_eq!(rx[0]["data"]["evidence"]["quanta"], "1");
    assert_eq!(controls(&v, "timer_load", "Net.s1.tx_a").len(), 1);
    assert!(
        !rows(&v)
            .into_iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.reception")
    );
}
