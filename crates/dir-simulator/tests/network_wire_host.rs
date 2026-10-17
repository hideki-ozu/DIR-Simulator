//! Actual shared-network host tests: control boundaries, cooperative freeze and FIFO occupancy.
use dir_simulator::{ControlCommand, RunOptions, RunSession, RunState, prepare, run, runtime};
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
            "dir-network-wire-host-{}-{}",
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
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn sample(name: &str) -> (Value, Value) {
    let p = root().join("examples/ethernet-wire");
    let read =
        |s| serde_json::from_slice(&fs::read(p.join(format!("{name}-{s}.json"))).unwrap()).unwrap();
    (read("model"), read("workload"))
}
fn fixture(c: &Value, w: &Value) -> Temp {
    let t = Temp::new();
    fs::write(t.0.join("model.json"), c.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), w.to_string()).unwrap();
    fs::write(t.0.join("run.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.control.wire.experimental.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 100000000ps\nmax-events = 2000\n",root().join("examples/ethernet/dynamic/models").display())).unwrap();
    t
}
fn host(snapshot: &dir_simulator::snapshot::Snapshot) -> &Value {
    &snapshot.network.as_ref().unwrap()["wire"]
}
#[test]
fn ready_pause_resume_and_stopped_boundary_export_remain_frozen() {
    let (c, w) = sample("passive");
    let f = fixture(&c, &w);
    let p = prepare(&f.0.join("run.ini")).unwrap();
    let (mut s, control) = RunSession::new(p, RunOptions::default()).unwrap();
    control.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    assert!(
        s.snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .is_empty()
    );
    s.resume(s.pause_token().unwrap()).unwrap();
    for _ in 0..100 {
        if host_opt(s.snapshot()).is_some_and(|h| h["boundaries"][0]["actual_start_ps"].is_string())
        {
            break;
        }
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
    }
    assert!(host(s.snapshot())["boundaries"][0]["actual_start_ps"].is_string());
    assert!(host(s.snapshot())["boundaries"][0]["actual_arrival_ps"].is_null());
    control.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    let frozen = host(s.snapshot()).clone();
    s.export(&f.0.join("pause1")).unwrap();
    s.export(&f.0.join("pause2")).unwrap();
    assert_eq!(host(s.snapshot()), &frozen);
    control.submit(ControlCommand::Stop).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Stopped);
    s.export(&f.0.join("stop")).unwrap();
    assert_eq!(host(s.snapshot()), &frozen);
    assert!(
        !s.snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "ethernet.wire.membership")
    );
}
fn host_opt(snapshot: &dir_simulator::snapshot::Snapshot) -> Option<&Value> {
    snapshot
        .network
        .as_ref()?
        .get("wire")
        .filter(|v| !v.is_null())
}
#[test]
fn pause_resume_protocol_state_and_records_match_synchronous_api() {
    let (c, w) = sample("passive");
    let f = fixture(&c, &w);
    let p = prepare(&f.0.join("run.ini")).unwrap();
    let expected = runtime::simulate(&p).unwrap();
    let (mut s, control) = RunSession::new(p, RunOptions::default()).unwrap();
    s.advance_one().unwrap();
    for _ in 0..3 {
        s.advance_one().unwrap();
    }
    control.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    s.resume(s.pause_token().unwrap()).unwrap();
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(
        serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap(),
        serde_json::to_value(&expected.registered.as_ref().unwrap().model_records).unwrap()
    );
    assert_eq!(host(s.snapshot())["protocol"], host(&expected)["protocol"]);
}
#[test]
fn external_rx_start_arrival_release_are_all_half_open() {
    let (c, mut w) = sample("passive");
    w["packets"].as_array_mut().unwrap().truncate(1);
    let f = fixture(&c, &w);
    let p = prepare(&f.0.join("run.ini")).unwrap();
    let (start_ps, arrival_ps, release_ps) = (9_392_000, 10_000_000, 10_096_000);
    for (h, start, arrival, release) in [
        (start_ps, false, false, false),
        (start_ps + 1, true, false, false),
        (arrival_ps, true, false, false),
        (arrival_ps + 1, true, true, false),
        (release_ps, true, true, false),
        (release_ps + 1, true, true, true),
    ] {
        let mut p = p.clone();
        p.common.time_limit_ps = h;
        let snapshot = runtime::simulate(&p).unwrap();
        assert!(
            !snapshot.common.partial,
            "{:?}",
            snapshot.common.diagnostics
        );
        let boundary = &host(&snapshot)["boundaries"][0];
        assert_eq!(boundary["actual_start_ps"].is_string(), start, "H={h}");
        assert_eq!(boundary["actual_arrival_ps"].is_string(), arrival, "H={h}");
        assert_eq!(boundary["actual_release_ps"].is_string(), release, "H={h}");
        assert_eq!(
            boundary["completed_frames"],
            if arrival { "1" } else { "0" }
        );
    }
}
#[test]
fn control_fifo_waits_for_started_data_and_shares_one_port_occupancy() {
    let (c, mut w) = sample("passive");
    w["packets"].as_array_mut().unwrap().truncate(1);
    w["ethernet"]["generators"] = json!([{"id":"opaque_data","node":"Net.b","kind":"ethernet.explicit.v1","times_ps":["0"],"frame":{"dst_mac":"02:00:00:00:00:03","ether_type":2048,"data":"00".repeat(982),"tag":{"vid":10,"pcp":0,"dei":0},"ip_multicast":null},"flow_id":"opaque_data","priority":0,"deadline_ps":null}]);
    let f = fixture(&c, &w);
    let p = prepare(&f.0.join("run.ini")).unwrap();
    let snapshot = runtime::simulate(&p).unwrap();
    assert!(
        !snapshot.common.partial,
        "{:?}",
        snapshot.common.diagnostics
    );
    let data = snapshot
        .ethernet
        .as_ref()
        .unwrap()
        .transfers
        .iter()
        .find(|t| t.from_port == "Net.s2.tx_c" && t.sof_ps.is_some())
        .unwrap();
    let release = data.release_ps.unwrap();
    let events = &snapshot.registered.as_ref().unwrap().model_records;
    assert!(
        events
            .values()
            .any(|r| r.schema.name == "ethernet.control.wire.event"
                && r.data["event_kind"] == "tx_queued"
                && r.data["port"] == "Net.s2.tx_c"
                && r.time_ps < release)
    );
    let control = host(&snapshot)["tx"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["port"] == "Net.s2.tx_c" && t["sof_ps"].is_string())
        .collect::<Vec<_>>();
    assert!(!control.is_empty());
    for tx in control {
        assert!(tx["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() >= release);
    }
    let mut spans = Vec::new();
    for t in snapshot
        .ethernet
        .as_ref()
        .unwrap()
        .transfers
        .iter()
        .filter(|t| t.from_port == "Net.s2.tx_c")
    {
        if let (Some(start), Some(end)) = (t.sof_ps, t.release_ps) {
            spans.push((start, end));
        }
    }
    for t in host(&snapshot)["tx"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["port"] == "Net.s2.tx_c")
    {
        if let (Some(start), Some(end)) = (t["sof_ps"].as_str(), t["release_ps"].as_str()) {
            spans.push((start.parse().unwrap(), end.parse().unwrap()));
        }
    }
    spans.sort_unstable();
    assert!(
        spans.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "{spans:?}"
    );
}

#[test]
fn learning_ingress_never_relays_after_forwarding_transition_during_processing() {
    let (mut c, mut w) = sample("active");
    c["ethernet"]["switches"][0]["forward_delay_ps"] = json!("20000000000000");
    c["adapters"]
        .as_array_mut()
        .unwrap()
        .retain(|a| a["protocol"] == "rstp");
    for port in c["adapters"][0]["rstp"]["port_parameters"]
        .as_array_mut()
        .unwrap()
    {
        port["admin_edge"] = json!(false);
        port["auto_edge"] = json!(port["port"] == "Net.s1.tx_a");
        port["auto_isolate"] = json!(false);
    }
    // Valid legacy TCNs keep this port non-edge across both real PST timers.
    w["packets"] = json!((0..45u64).map(|i| json!({
        "id":format!("legacy_tcn_{i}"), "adapter_id":"rstp-s1",
        "ingress_port":"Net.s1.rx_a", "arrival_ps":(500_000_000_000 + i * 1_000_000_000_000).to_string(),
        "encapsulation":"mac_frame_fcs",
        "bytes_hex":"0180c200000002000000000100074242030000008000000000000000000000000000000000000000000000000000000000000000000000000000000060a37bc8"
    })).collect::<Vec<_>>());
    w["ethernet"]["generators"] = json!([{
        "id":"learning_probe", "node":"Net.a", "kind":"ethernet.explicit.v1",
        "times_ps":["21000000000000"],
        "frame":{"dst_mac":"02:00:00:00:00:02", "ether_type":2048, "data":"",
            "tag":{"vid":10,"pcp":0,"dei":0}, "ip_multicast":null},
        "flow_id":"learning_probe", "priority":0, "deadline_ps":null
    }]);
    w["ethernet"]["controls"] = json!([]);
    let f = fixture(&c, &w);
    let mut p = prepare(&f.0.join("run.ini")).unwrap();
    let observed = 21_000_000_609_000;
    p.common.time_limit_ps = observed + 1;
    let prefix = runtime::simulate(&p).unwrap();
    assert!(!prefix.common.partial, "{:?}", prefix.common.diagnostics);
    let rx = &prefix.ethernet.as_ref().unwrap().receptions[0];
    assert_eq!(rx.status, "processing");
    assert_eq!(rx.observed_ps, observed);
    assert_eq!(rx.ready_ps, None);
    assert_eq!(rx.planned_ready_ps, Some(observed + 20_000_000_000_000));
    let dynamic = &prefix.network.as_ref().unwrap()["dynamic"];
    assert!(dynamic["mac"].as_array().unwrap().iter().any(|m| m["key"]
        == json!({"mac":"02:00:00:00:00:01","switch":"Net.s1","vid":"10"})
        && m["lease"]["value"] == "Net.s1.tx_a"));
    assert!(
        prefix
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.schema.name == "ethernet.dynamic.control"
                && r.data["kind"] == "mac_learn"
                && r.time_ps == observed
                && r.data["after"]["value"] == "Net.s1.tx_a")
    );

    p.common.time_limit_ps = 45_000_000_000_000;
    let snapshot = runtime::simulate(&p).unwrap();
    assert!(
        !snapshot.common.partial,
        "{:?}",
        snapshot.common.diagnostics
    );
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    for (from, to, at) in [
        ("DISCARDING", "LEARNING", 20_000_000_000_000),
        ("LEARNING", "FORWARDING", 35_000_000_000_000),
    ] {
        assert!(
            records
                .values()
                .any(|r| r.schema.name == "ethernet.control.wire.transition"
                    && r.data["machine"] == "PST"
                    && r.data["port"] == "Net.s1.tx_a"
                    && r.data["from_state"] == from
                    && r.data["to_state"] == to
                    && r.time_ps == at)
        );
    }
    let ethernet = snapshot.ethernet.as_ref().unwrap();
    let rx = &ethernet.receptions[0];
    assert_eq!(rx.status, "filtered");
    assert_eq!(rx.reason.as_deref(), Some("stp_learning"));
    assert_eq!(rx.ready_ps, rx.planned_ready_ps);
    assert!(rx.egress_transfer_ids.is_empty());
    assert_eq!(ethernet.transfers.len(), 1);
    assert_eq!(ethernet.receptions.len(), 1);
    assert!(!ethernet.receptions.iter().any(|r| r.device == "Net.b"));
}

#[test]
fn redundancy_snapshot_maps_only_actual_transfers_even_before_sof_and_after_drop() {
    let samples = root().join("examples/network-extensions");
    let read =
        |name| serde_json::from_slice::<Value>(&fs::read(samples.join(name)).unwrap()).unwrap();
    let config = read("redundancy-model.json");
    let workload = read("redundancy-workload.json");
    for (horizon, capacity, expected_count) in [
        (0, 64, 0),
        (1, 64, 2),
        (10_000_000, 64, 7),
        (10_000_000, 0, 2),
        // Paths are offered by ID: the four-hop alternate wins the single slot.
        (10_000_000, 1, 5),
    ] {
        let mut c = config.clone();
        c["base"]["config"]["base"]["config"]["outputs"][0]["queues"][0]["capacity_frames"] =
            json!(capacity.to_string());
        let f = fixture(&c, &workload);
        let ini = fs::read_to_string(f.0.join("run.ini")).unwrap().replace(
            "ethernet.control.wire.experimental.v1",
            "ethernet.redundancy.abstract.v1",
        );
        fs::write(f.0.join("run.ini"), ini).unwrap();
        let mut p = prepare(&f.0.join("run.ini")).unwrap();
        p.common.time_limit_ps = horizon;
        let out = f.0.join("out");
        let report = run(p, &out).unwrap();
        assert_eq!(report.exit_code, 0, "{report:?}");
        let result: Value =
            serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
        let circuits = result["metadata"]["network_runtime"]["redundancy"]["transfer_circuits"]
            .as_array()
            .unwrap();
        let records = result["simulation"]["model_records"].as_array().unwrap();
        let rows = |schema| {
            records
                .iter()
                .filter(move |r| r["schema_name"] == schema)
                .map(|r| &r["data"])
        };
        let transfers = rows("ethernet.dynamic.transfer").collect::<Vec<_>>();
        let branches = rows("dir.redundancy.branch").collect::<Vec<_>>();
        assert_eq!(
            circuits.len(),
            expected_count,
            "H={horizon}, capacity={capacity}"
        );
        assert_eq!(circuits.len(), transfers.len());
        let mut ids = std::collections::BTreeSet::new();
        for mapping in circuits {
            assert_eq!(mapping.as_object().unwrap().len(), 3);
            assert!(ids.insert(mapping["transfer_id"].as_str().unwrap()));
            let transfer = transfers
                .iter()
                .find(|t| t["copy_id"] == mapping["transfer_id"])
                .unwrap();
            let branch = branches
                .iter()
                .find(|b| b["branch_id"] == mapping["branch_id"])
                .unwrap();
            let hop = mapping["hop_index"]
                .as_str()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert_eq!(mapping["hop_index"], hop.to_string());
            assert_eq!(transfer["from_port"], branch["path_ports"][hop]);
            if hop == 0 {
                assert!(transfer["parent_transfer_id"].is_null());
            } else {
                let parent = circuits
                    .iter()
                    .find(|m| m["transfer_id"] == transfer["parent_transfer_id"])
                    .unwrap();
                assert_eq!(parent["branch_id"], mapping["branch_id"]);
                assert_eq!(parent["hop_index"], (hop - 1).to_string());
            }
        }
        if horizon == 1 {
            assert_eq!(
                transfers
                    .iter()
                    .filter(|t| t["sof_ps"].is_null() && t["status"] == "queued")
                    .count(),
                1
            );
            assert_eq!(circuits.iter().filter(|m| m["hop_index"] == "0").count(), 2);
        }
        if capacity < 2 {
            let drops = transfers
                .iter()
                .filter(|t| t["status"] == "dropped" && t["sof_ps"].is_null())
                .count();
            assert_eq!(drops, if capacity == 0 { 2 } else { 1 });
            assert!(
                branches
                    .iter()
                    .filter(|b| b["status"] == "dropped")
                    .all(|b| b["current_transfer_id"].is_null())
            );
        }
        if horizon == 10_000_000 {
            assert!(
                result["metadata"]["network_runtime"]["redundancy"]["branches"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
