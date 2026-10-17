//! Normal prepare/run/output path, distinct from pure protocol fixture checks.
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
            "dir-wire-profiles-{}-{}",
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
    let base = root().join("examples/ethernet-wire");
    let read = |suffix| {
        serde_json::from_slice(&fs::read(base.join(format!("{name}-{suffix}.json"))).unwrap())
            .unwrap()
    };
    (read("model"), read("workload"))
}
fn fixture(c: &Value, w: &Value, horizon: u64) -> Temp {
    let t = Temp::new();
    fs::write(t.0.join("model.json"), c.to_string()).unwrap();
    fs::write(t.0.join("workload.json"), w.to_string()).unwrap();
    fs::write(t.0.join("run.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.control.wire.experimental.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {horizon}ps\n",root().join("examples/ethernet/dynamic/models").display())).unwrap();
    t
}
fn product(c: &Value, w: &Value, h: u64) -> Value {
    let t = fixture(c, w, h);
    let mut prepared = prepare(&t.0.join("run.ini")).unwrap();
    prepared.common.max_events = 2000;
    let report = run(prepared, &t.0.join("out")).unwrap();
    assert_eq!(report.exit_code, 0, "{report:?}");
    serde_json::from_slice(&fs::read(t.0.join("out/results.json")).unwrap()).unwrap()
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
#[test]
fn active_profiles_exchange_actual_wire_and_converge() {
    let (c, w) = sample("active");
    let v = product(&c, &w, 4_000_000_000_000);
    let events = rows(&v, "ethernet.control.wire.event");
    assert!(
        events
            .iter()
            .any(|r| r["protocol"] == "rstp" && r["event_kind"] == "tx_completed")
    );
    assert!(events.iter().any(|r| r["protocol"] == "mvrp"
        && r["event_kind"] == "receive"
        && r["external_injection"] == false
        && r["reason"].is_null()));
    let policy = rows(&v, "ethernet.control.wire.policy");
    let last = policy
        .into_iter()
        .max_by_key(|r| r["policy_epoch"].as_str().unwrap().parse::<u64>().unwrap())
        .expect("wire policy");
    let ports = last["ports"].as_object().unwrap();
    assert_eq!(ports.values().filter(|p| p["role"] == "Root").count(), 2);
    assert_eq!(
        ports.values().filter(|p| p["role"] == "Alternate").count(),
        1
    );
    assert!(!last["registered"].as_array().unwrap().is_empty());
}
#[test]
fn passive_reports_have_physical_boundary_and_membership() {
    let (c, w) = sample("passive");
    let v = product(&c, &w, 100_000_000);
    let boundary = rows(&v, "ethernet.wire.boundary");
    assert_eq!(boundary.len(), 6);
    let membership = rows(&v, "ethernet.wire.membership");
    assert!(membership.iter().any(|r| r["family"] == "ipv4"
        && r["outcome"] == "accepted"
        && !r["changes"].as_array().unwrap().is_empty()));
    assert!(membership.iter().any(|r| r["family"] == "ipv6"
        && r["outcome"] == "accepted"
        && !r["changes"].as_array().unwrap().is_empty()));
}
#[test]
fn malformed_report_completes_rx_but_changes_no_membership() {
    let (c, mut w) = sample("passive");
    w["packets"].as_array_mut().unwrap().truncate(1);
    w["packets"][0]["bytes_hex"] = json!("00");
    let v = product(&c, &w, 20_000_000);
    let membership = rows(&v, "ethernet.wire.membership");
    assert_eq!(membership.len(), 1);
    assert_eq!(membership[0]["outcome"], "control_drop");
    assert_eq!(membership[0]["changes"], json!([]));
    assert_eq!(
        rows(&v, "ethernet.wire.boundary")
            .iter()
            .filter(|r| r["event"] == "arrival")
            .count(),
        1
    );
    assert!(
        !rows(&v, "ethernet.control.wire.event")
            .iter()
            .any(|r| r["event_kind"] == "tx_started")
    );
}
#[test]
fn active_invalid_revision_and_partial_ownership_rejected() {
    let (c, w) = sample("active");
    let mut bad = c.clone();
    bad["adapters"][0]["protocol_revision"] = json!("unapproved");
    let t = fixture(&bad, &w, 100);
    assert!(prepare(&t.0.join("run.ini")).is_err());
    let mut bad = c;
    bad["adapters"][0]["ports"].as_array_mut().unwrap().pop();
    let t = fixture(&bad, &w, 100);
    assert!(prepare(&t.0.join("run.ini")).is_err());
}
