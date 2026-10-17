//! Product-path checks for the approved explicit composite/replication/preemption profiles.
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
            "dir-extension-profiles-{}-{}",
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
    let p = root().join("examples/network-extensions");
    let read = |suffix| {
        serde_json::from_slice(&fs::read(p.join(format!("{name}-{suffix}.json"))).unwrap()).unwrap()
    };
    (read("model"), read("workload"))
}
fn fixture(name: &str, config: &Value, workload: &Value) -> Temp {
    let t = Temp::new();
    fs::write(t.0.join("model.json"), config.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), workload.to_string()).unwrap();
    let (network, ned, profile) = match name {
        "preemption" => (
            "ethtsn.Net",
            "examples/ethernet/tsn/models",
            "ethernet.preemption.abstract.v1",
        ),
        "redundancy" => (
            "ethdynamic.Net",
            "examples/ethernet/dynamic/models",
            "ethernet.redundancy.abstract.v1",
        ),
        "composite" => (
            "bridge.R01",
            "examples/can-ethernet/models",
            "can.ethernet.tsn.v1",
        ),
        _ => panic!(),
    };
    fs::write(t.0.join("run.ini"),format!("[General]\nnetwork = {network}\nned-path = \"{}\"\nmodel-profile = \"{profile}\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 1000000000ps\n",root().join(ned).display())).unwrap();
    t
}
fn product(name: &str, config: &Value, workload: &Value, h: u64) -> Value {
    let f = fixture(name, config, workload);
    let mut p = prepare(&f.0.join("run.ini")).unwrap();
    p.common.time_limit_ps = h;
    let out = f.0.join("out");
    let report = run(p, &out).unwrap();
    assert_eq!(report.exit_code, 0, "{:?}", report);
    serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap()
}
fn rows<'a>(r: &'a Value, schema: &str) -> Vec<&'a Value> {
    r["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .map(|r| &r["data"])
        .collect()
}
fn slow(r: &Value) -> &Value {
    rows(r, "dir.preemption.transfer")
        .into_iter()
        .find(|v| v["frame_id"] == "slow:0")
        .unwrap()
}
fn metric<'a>(r: &'a Value, target: &str, name: &str) -> &'a Value {
    &r["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["metric"] == name && r["target"] == target)
        .unwrap_or_else(|| panic!("missing {name} for {target}"))["value"]
}
#[test]
fn legal_frame_preemption_matches_independent_design_vector() {
    let (c, w) = sample("preemption");
    let r = product("preemption", &c, &w, 2_000_000);
    let fragments = rows(&r, "dir.preemption.fragment");
    assert_eq!(fragments.len(), 3);
    for (f, (start, end, offset, len)) in fragments.iter().zip([
        (0, 352000, 0, 40),
        (1032000, 1384000, 40, 40),
        (1384000, 1576000, 80, 20),
    ]) {
        assert_eq!(f["start_ps"], start.to_string());
        assert_eq!(f["end_ps"], end.to_string());
        assert_eq!(f["offset_bytes"], offset.to_string());
        assert_eq!(f["payload_bytes"], len.to_string());
    }
    let fast = rows(&r, "dir.preemption.transfer")
        .into_iter()
        .find(|r| r["kind"] == "express")
        .unwrap();
    assert_eq!(fast["eof_ps"], "928000");
    assert_eq!(fast["release_ps"], "1024000");
    assert_eq!(slow(&r)["eof_ps"], "1576000");
    assert_eq!(slow(&r)["occupied_ps"], "896000");
    assert_eq!(slow(&r)["completed_mac_bytes"], "100");
    assert_eq!(metric(&r, "Net.a.tx", "preemption.port.busy_ps"), "1568000");
    assert_eq!(
        metric(&r, "Net.a.tx", "preemption.port.resume_gap_ps"),
        "8000"
    );
    assert_eq!(rows(&r, "ethernet.dynamic.transfer").len(), 0);
    assert_eq!(
        rows(&r, "dir.preemption.delivery")
            .iter()
            .map(|r| r["completed_count"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap())
            .sum::<u64>(),
        2
    );
}
#[test]
fn preemption_half_open_and_active_prefix_are_actual() {
    let (c, w) = sample("preemption");
    for (h, completed, fragments, busy, gap) in [
        (352000, "0", 1, "352000", "0"),
        (500000, "40", 1, "500000", "0"),
        (1024000, "40", 1, "1024000", "0"),
        (1032000, "40", 1, "1024000", "8000"),
        (1576000, "80", 3, "1568000", "8000"),
    ] {
        let r = product("preemption", &c, &w, h);
        let s = slow(&r);
        assert_eq!(s["eof_ps"], Value::Null, "H={h}");
        assert_eq!(s["completed_mac_bytes"], completed, "H={h}");
        assert_eq!(
            rows(&r, "dir.preemption.fragment").len(),
            fragments,
            "H={h}"
        );
        assert_eq!(
            metric(&r, "Net.a.tx", "preemption.port.busy_ps"),
            busy,
            "H={h}"
        );
        assert_eq!(
            metric(&r, "Net.a.tx", "preemption.port.resume_gap_ps"),
            gap,
            "H={h}"
        );
        let completed_fragments = rows(&r, "dir.preemption.fragment")
            .iter()
            .filter(|f| f["end_ps"].is_string())
            .count();
        assert_eq!(
            completed_fragments,
            if h <= 352000 {
                0
            } else if h < 1384001 {
                1
            } else {
                2
            }
        );
        let prefixes = rows(&r, "dir.preemption.transfer")
            .into_iter()
            .map(|v| v["occupied_ps"].as_str().unwrap().parse::<u64>().unwrap())
            .sum::<u64>();
        assert_eq!(prefixes, busy.parse::<u64>().unwrap(), "H={h}");
    }
}
#[test]
fn uninterrupted_fragments_and_low_numbered_express_priority() {
    let (c, mut w) = sample("preemption");
    w["generators"].as_array_mut().unwrap().pop();
    let r = product("preemption", &c, &w, 1_000_000);
    assert_eq!(slow(&r)["eof_ps"], "896000");
    assert!(rows(&r, "dir.preemption.gap").is_empty());
    let (mut c, mut w) = sample("preemption");
    for o in c["preemption"]["outputs"].as_array_mut().unwrap() {
        o["express_classes"] = json!([1]);
        o["preemptable_classes"] = json!([0, 2, 3, 4, 5, 6, 7]);
    }
    w["generators"][0]["priority"] = json!(6);
    w["generators"][1]["priority"] = json!(1);
    let r = product("preemption", &c, &w, 2_000_000);
    let express = rows(&r, "dir.preemption.transfer")
        .into_iter()
        .find(|r| r["kind"] == "express")
        .unwrap();
    assert_eq!(express["sof_ps"], "352000");
    assert_eq!(slow(&r)["eof_ps"], "1576000");
}
#[test]
fn express_arriving_during_gap_waits_then_starts_new_group() {
    let (c, mut w) = sample("preemption");
    let mut next = w["generators"][1].clone();
    next["id"] = json!("express2");
    next["times_ps"] = json!(["1028000"]);
    w["generators"].as_array_mut().unwrap().push(next);
    let r = product("preemption", &c, &w, 3_000_000);
    let express = rows(&r, "dir.preemption.transfer")
        .into_iter()
        .find(|r| r["frame_id"] == "express2:0")
        .unwrap();
    assert_eq!(express["sof_ps"], "1032000");
    let gaps = rows(&r, "dir.preemption.gap");
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[0]["end_ps"], "1032000");
    assert_eq!(gaps[1]["start_ps"], "1704000");
    assert_eq!(gaps[1]["end_ps"], "1712000");
    assert_eq!(slow(&r)["eof_ps"], "2256000");
}
#[test]
fn replication_uses_each_path_and_delivers_once() {
    let (c, w) = sample("redundancy");
    let r = product("redundancy", &c, &w, 10_000_000);
    let branches = rows(&r, "dir.redundancy.branch");
    assert_eq!(branches.len(), 2);
    assert_eq!(
        branches
            .iter()
            .filter(|b| b["status"] == "delivered")
            .count(),
        1
    );
    assert_eq!(
        branches
            .iter()
            .filter(|b| b["status"] == "duplicate")
            .count(),
        1
    );
    assert_eq!(rows(&r, "dir.redundancy.recovery").len(), 2);
    let origin = rows(&r, "dir.redundancy.origin")[0];
    assert_eq!(origin["target_count"], "1");
    assert_eq!(origin["completed_count"], "1");
    assert_eq!(rows(&r, "ethernet.dynamic.transfer").len(), 7);
    assert_eq!(metric(&r, "redundant", "redundancy.recovery.accepted"), "1");
    assert_eq!(
        metric(&r, "redundant", "redundancy.recovery.duplicate"),
        "1"
    );
    let mut reordered = c.clone();
    reordered["replication"]["streams"][0]["egress_paths"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let other = product("redundancy", &reordered, &w, 10_000_000);
    assert_eq!(
        rows(&r, "dir.redundancy.branch"),
        rows(&other, "dir.redundancy.branch")
    );
    assert_eq!(
        rows(&r, "dir.redundancy.origin"),
        rows(&other, "dir.redundancy.origin")
    );
}
#[test]
fn replication_admission_drop_and_sof_denominators() {
    let (c, w) = sample("redundancy");
    for (capacity, targets, completed, drops) in [(0, "0", "0", 2), (1, "1", "1", 1)] {
        let mut c = c.clone();
        c["base"]["config"]["base"]["config"]["outputs"][0]["queues"][0]["capacity_frames"] =
            json!(capacity.to_string());
        let r = product("redundancy", &c, &w, 10_000_000);
        let origins = rows(&r, "dir.redundancy.origin");
        assert_eq!(origins[0]["target_count"], targets);
        assert_eq!(origins[0]["completed_count"], completed);
        assert_eq!(
            rows(&r, "dir.redundancy.branch")
                .iter()
                .filter(|b| b["status"] == "dropped")
                .count(),
            drops
        );
    }
    let r = product("redundancy", &c, &w, 0);
    assert!(rows(&r, "dir.redundancy.origin").is_empty());
    let r = product("redundancy", &c, &w, 1);
    let origins = rows(&r, "dir.redundancy.origin");
    assert_eq!(origins[0]["target_count"], "1");
    assert_eq!(origins[0]["completed_count"], "0");
}
#[test]
fn exact_prepare_rejects_invalid_partitions_and_paths() {
    for (name, pointer, value) in [
        (
            "preemption",
            "/preemption/outputs/0/fragment_payload_bytes",
            json!(0),
        ),
        (
            "preemption",
            "/preemption/outputs/0/express_classes",
            json!([7, 7]),
        ),
        (
            "preemption",
            "/preemption/outputs/0/preemptable_classes",
            json!([0, 1, 2, 3, 4, 5, 6, 7]),
        ),
        (
            "preemption",
            "/preemption/outputs/0/resume_gap_ps",
            json!("01"),
        ),
        ("preemption", "/limits/max_control_events", json!(true)),
        (
            "redundancy",
            "/replication/streams/0/history_capacity",
            json!(0),
        ),
        (
            "redundancy",
            "/replication/streams/0/history_capacity",
            json!(65537),
        ),
        (
            "redundancy",
            "/replication/streams/0/egress_paths",
            json!([]),
        ),
        (
            "redundancy",
            "/replication/streams/0/egress_paths/0/ports",
            json!(["Net.s1.tx_b"]),
        ),
        (
            "redundancy",
            "/replication/streams/0/eliminate_at",
            json!("Net.a"),
        ),
        ("redundancy", "/schema_name", json!("unknown")),
        ("composite", "/bridge/can", json!({"fd":true})),
        ("composite", "/tsn", Value::Null),
    ] {
        let (mut c, w) = sample(name);
        *c.pointer_mut(pointer).unwrap() = value;
        let f = fixture(name, &c, &w);
        assert!(
            prepare(&f.0.join("run.ini")).is_err(),
            "accepted {name} {pointer}"
        );
    }
    let (mut c, w) = sample("preemption");
    c["base"]["config"]["tsn"]["outputs"][0]["cbs"] = json!([{"priority":0,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}]);
    let f = fixture("preemption", &c, &w);
    assert!(
        prepare(&f.0.join("run.ini"))
            .unwrap_err()
            .message
            .contains("CBS")
    );
}
fn synthetic_stream() -> Value {
    json!({"id":"synthetic","ingress":"R01.gw.eth.rx","dst_mac":"02:00:00:00:00:02","vid":10,"priority":3,"max_sdu_bytes":"128","gate":null,"meter":{"committed_rate_bps":"1","peak_rate_bps":"1","committed_burst_bytes":"1000","peak_burst_bytes":"1000","yellow_action":"drop"}})
}
#[test]
fn composite_polices_once_before_capacity_and_releases_before_gate_sof() {
    let (mut c, mut w) = sample("composite");
    c["tsn"]["streams"] = json!([synthetic_stream()]);
    c["bridge"]["gateways"][0]["rx_capacity"] = json!(10);
    c["ethernet"]["outputs"][0]["queues"][3]["capacity_frames"] = json!("1");
    c["tsn"]["outputs"][0]["tas"] = json!({"id":"late","base_time_ps":"0","cycle_time_ps":"1000000000","entries":[{"duration_ps":"500000000","open_priorities":[]},{"duration_ps":"500000000","open_priorities":[0,1,2,3,4,5,6,7]}]});
    w["can"]["generators"][0]["period"] = json!("1ns");
    w["can"]["generators"][0]["count"] = json!(3);
    w["can"]["generators"][0]
        .as_object_mut()
        .unwrap()
        .remove("end");
    let r = product("composite", &c, &w, 800_000_000);
    let ingresses = rows(&r, "dir.composite.ingress");
    assert_eq!(ingresses.len(), 3);
    assert!(ingresses.iter().all(|r| r["verdict"] == "pass"));
    let policing = rows(&r, "ethernet.tsn.policing");
    let synthetic = policing
        .iter()
        .filter(|r| r["stream_id"] == "synthetic")
        .collect::<Vec<_>>();
    assert_eq!(synthetic.len(), 3);
    assert!(synthetic.iter().all(|r| {
        ingresses
            .iter()
            .any(|i| i["ingress_id"] == r["reception_id"])
    }));
    let conversions = rows(&r, "dir.can_ethernet.conversion");
    assert_eq!(conversions.len(), 3);
    assert!(conversions.iter().all(|r| r["status"] == "released"));
    let admitted = rows(&r, "dir.can_ethernet.branch");
    let released = conversions
        .iter()
        .filter_map(|r| {
            r["released_ps"]
                .as_str()
                .and_then(|v| v.parse::<u64>().ok())
        })
        .min()
        .unwrap();
    assert!(released < 500_000_000);
    assert!(admitted.iter().all(|b| {
        b["sof_ps"]
            .as_str()
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n >= 500_000_000)
    }));
    let transfers = rows(&r, "ethernet.dynamic.transfer");
    assert_eq!(transfers.len(), 3);
    assert!(rows(&r, "ethernet.transfer").is_empty());
}
#[test]
fn composite_synthetic_psfp_drop_creates_no_child_and_releases_rx() {
    let (mut c, w) = sample("composite");
    let mut stream = synthetic_stream();
    stream["max_sdu_bytes"] = json!("1");
    c["tsn"]["streams"] = json!([stream]);
    let r = product("composite", &c, &w, 800_000_000);
    let ingresses = rows(&r, "dir.composite.ingress");
    assert_eq!(ingresses.len(), 1);
    assert_eq!(ingresses[0]["verdict"], "drop");
    assert_eq!(ingresses[0]["reason"], "psfp_max_sdu");
    let branches = rows(&r, "dir.can_ethernet.branch");
    assert_eq!(branches[0]["status"], "dropped");
    assert_eq!(branches[0]["reason"], "psfp_max_sdu");
    assert!(branches[0]["child_id"].is_null());
    assert!(rows(&r, "ethernet.dynamic.transfer").is_empty());
    assert_eq!(metric(&r, "R01.gw", "gateway.rx.occupancy"), "0");
}
#[test]
fn fragment_guard_accepts_equality_and_rejects_one_ps_short() {
    let (c, mut w) = sample("preemption");
    w["generators"].as_array_mut().unwrap().pop();
    for (open, starts) in [(351999, false), (352000, true), (352001, true)] {
        let mut c = c.clone();
        c["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"fit","base_time_ps":"0","cycle_time_ps":"2000000","entries":[{"duration_ps":open.to_string(),"open_priorities":[0]},{"duration_ps":(2000000-open).to_string(),"open_priorities":[]}]});
        let r = product("preemption", &c, &w, 1);
        assert_eq!(!rows(&r, "dir.preemption.fragment").is_empty(), starts);
    }
}
#[test]
fn empty_class_sets_minimum_fragments_and_sequence_window_edges_prepare() {
    let (c, w) = sample("preemption");
    for (express, preempt) in [
        (json!([]), json!([0, 1, 2, 3, 4, 5, 6, 7])),
        (json!([0, 1, 2, 3, 4, 5, 6, 7]), json!([])),
    ] {
        let mut c = c.clone();
        for o in c["preemption"]["outputs"].as_array_mut().unwrap() {
            o["express_classes"] = express.clone();
            o["preemptable_classes"] = preempt.clone();
            o["fragment_payload_bytes"] = json!(1);
            o["fragment_overhead_bytes"] = json!(0);
            o["resume_gap_ps"] = json!("0");
        }
        let f = fixture("preemption", &c, &w);
        assert!(prepare(&f.0.join("run.ini")).is_ok());
    }
    let (mut c, w) = sample("redundancy");
    for capacity in [1, 65536] {
        c["replication"]["streams"][0]["history_capacity"] = json!(capacity);
        let r = product("redundancy", &c, &w, 10_000_000);
        assert_eq!(
            rows(&r, "dir.redundancy.recovery")
                .iter()
                .filter(|r| r["delivered"] == true)
                .count(),
            1
        );
    }
}
#[test]
fn workload_controls_unknown_fields_and_duplicate_keys_reject() {
    for name in ["preemption", "redundancy"] {
        let (c, mut w) = sample(name);
        w["controls"] =
            json!([{"id":"change","kind":"link_set","at_ps":"0","link":"a-b","up":false}]);
        let f = fixture(name, &c, &w);
        assert!(prepare(&f.0.join("run.ini")).is_err());
        let (mut c, w) = sample(name);
        c["unknown"] = json!(null);
        let f = fixture(name, &c, &w);
        assert!(prepare(&f.0.join("run.ini")).is_err());
        let (c, w) = sample(name);
        let f = fixture(name, &c, &w);
        fs::write(
            f.0.join("model.json"),
            format!("{{\"schema_version\":1,{}", &c.to_string()[1..]),
        )
        .unwrap();
        assert!(prepare(&f.0.join("run.ini")).is_err());
    }
}
#[test]
fn runtime_pending_limit_fails_atomically_with_prefix_output() {
    let (mut c, mut w) = sample("redundancy");
    c["limits"]["max_pending_branches"] = json!(2);
    w["generators"][0]["times_ps"] = json!(["0", "0"]);
    let f = fixture("redundancy", &c, &w);
    let p = prepare(&f.0.join("run.ini")).unwrap();
    let report = run(p, &f.0.join("out")).unwrap();
    assert_ne!(report.exit_code, 0);
    assert!(report.partial);
    let r: Value =
        serde_json::from_slice(&fs::read(f.0.join("out/results.json")).unwrap()).unwrap();
    let origins = rows(&r, "dir.redundancy.origin");
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0]["sequence"], "0");
    assert_eq!(rows(&r, "dir.redundancy.branch").len(), 2);
}
#[test]
fn composite_design_prepare_vectors_report_the_original_json_pointer() {
    let vectors: Value = serde_json::from_slice(
        &fs::read(root().join("docs/proposals/fixtures/network-extension-v1.json")).unwrap(),
    )
    .unwrap();
    let vector = vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "P-COMPOSITE-T01-V01")
        .unwrap();
    for case in vector["input"]["cases"].as_array().unwrap() {
        let (mut c, w) = sample("composite");
        let pointer = case["pointer"].as_str().unwrap();
        let value = case.get("value").cloned().unwrap_or_else(|| {
            if pointer == "/bridge/ethernet" {
                json!({})
            } else {
                json!(1)
            }
        });
        if let Some(slot) = c.pointer_mut(pointer) {
            *slot = value;
        } else {
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            c.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), value);
        }
        let f = fixture("composite", &c, &w);
        let error = prepare(&f.0.join("run.ini")).unwrap_err();
        assert_eq!(error.target.as_deref(), Some(pointer), "{case}: {error:?}");
        assert_eq!(
            error.source.as_deref(),
            Some(f.0.join("model.json").to_str().unwrap())
        );
        assert!(error.line.is_some());
    }
}
#[test]
fn composite_and_preemption_pending_caps_preserve_the_successful_prefix() {
    let (mut c, mut w) = sample("composite");
    c["limits"]["max_pending_branches"] = json!(1);
    c["ethernet"]["outputs"][0]["queues"][3]["capacity_frames"] = json!("1");
    c["tsn"]["outputs"][0]["tas"] = json!({"id":"blocked","base_time_ps":"0","cycle_time_ps":"1000000000","entries":[{"duration_ps":"500000000","open_priorities":[]},{"duration_ps":"500000000","open_priorities":[3]}]});
    w["can"]["generators"][0]["count"] = json!(3);
    w["can"]["generators"][0]["period"] = json!("1ns");
    w["can"]["generators"][0]
        .as_object_mut()
        .unwrap()
        .remove("end");
    let f = fixture("composite", &c, &w);
    let report = run(prepare(&f.0.join("run.ini")).unwrap(), &f.0.join("out")).unwrap();
    assert_ne!(report.exit_code, 0);
    let r: Value =
        serde_json::from_slice(&fs::read(f.0.join("out/results.json")).unwrap()).unwrap();
    assert_eq!(rows(&r, "dir.can_ethernet.branch").len(), 1);
    let (mut c, w) = sample("preemption");
    c["limits"]["max_pending_branches"] = json!(1);
    let f = fixture("preemption", &c, &w);
    let report = run(prepare(&f.0.join("run.ini")).unwrap(), &f.0.join("out")).unwrap();
    assert_ne!(report.exit_code, 0);
    let r: Value =
        serde_json::from_slice(&fs::read(f.0.join("out/results.json")).unwrap()).unwrap();
    assert_eq!(rows(&r, "ethernet.dynamic.frame").len(), 1);
    assert!(
        rows(&r, "dir.preemption.transfer")
            .iter()
            .all(|t| t["frame_id"] == "slow:0")
    );
}
#[test]
fn composite_completed_branches_return_capacity_and_control_cap_is_semantic() {
    let (mut c, mut w) = sample("composite");
    c["limits"]["max_pending_branches"] = json!(1);
    w["can"]["generators"][0]["count"] = json!(2);
    w["can"]["generators"][0]["period"] = json!("1ms");
    w["can"]["generators"][0]
        .as_object_mut()
        .unwrap()
        .remove("end");
    let r = product("composite", &c, &w, 3_000_000_000);
    assert_eq!(rows(&r, "dir.can_ethernet.branch").len(), 2);
    let (mut c, w) = sample("preemption");
    c["limits"]["max_control_events"] = json!(1);
    let f = fixture("preemption", &c, &w);
    let report = run(prepare(&f.0.join("run.ini")).unwrap(), &f.0.join("out")).unwrap();
    assert_ne!(report.exit_code, 0);
    let r: Value =
        serde_json::from_slice(&fs::read(f.0.join("out/results.json")).unwrap()).unwrap();
    assert!(rows(&r, "dir.preemption.fragment").is_empty());
    assert!(
        rows(&r, "dir.preemption.transfer")
            .iter()
            .all(|t| t["sof_ps"].is_null())
    );
}
#[test]
fn suspended_guard_wait_is_idle_medium_and_not_an_extra_resume_gap() {
    let (mut c, mut w) = sample("preemption");
    w["generators"].as_array_mut().unwrap().pop();
    c["base"]["config"]["tsn"]["outputs"][0]["tas"] = json!({"id":"guard","base_time_ps":"0","cycle_time_ps":"2000000","entries":[{"duration_ps":"400000","open_priorities":[0]},{"duration_ps":"1600000","open_priorities":[]}]});
    let r = product("preemption", &c, &w, 500_000);
    assert_eq!(slow(&r)["completed_mac_bytes"], "40");
    assert_eq!(slow(&r)["occupied_ps"], "352000");
    assert!(rows(&r, "dir.preemption.gap").is_empty());
    assert!(
        rows(&r, "dir.preemption.decision")
            .iter()
            .any(|r| r["state"] == "Suspended" && r["wait_reason"] == "guard")
    );
    assert_eq!(metric(&r, "Net.a.tx", "preemption.port.busy_ps"), "352000");
}

include!("support/network_design_vectors.rs");
