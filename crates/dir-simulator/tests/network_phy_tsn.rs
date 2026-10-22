//! Experimental half-duplex shaping acts on actual sampled MAC attempts.
use dir_simulator::{ControlStatus, RunOptions, RunSession, RunState, prepare};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite/t1s-shaped")
}
fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dir-phy-tsn-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
fn run() -> (PathBuf, Value) {
    let root = scratch();
    let (mut session, _) = RunSession::new(
        prepare(&fixture().join("model.ini")).unwrap(),
        RunOptions::default(),
    )
    .unwrap();
    assert_eq!(
        session.run_until_yield().unwrap(),
        RunState::Completed,
        "{:?}",
        session.snapshot().common.diagnostics
    );
    assert_eq!(
        session.export(&root.join("result")).unwrap().status,
        ControlStatus::Applied
    );
    let value =
        serde_json::from_slice(&fs::read(root.join("result/results.json")).unwrap()).unwrap();
    (root, value)
}
fn physical<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    v["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.phy" && r["data"]["kind"] == kind)
        .collect()
}
#[test]
fn actual_half_duplex_tx_release_drives_credit_and_closed_gate_freezes_it() {
    let (root, v) = run();
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let mut starts = physical(&v, "TxStarted");
    starts.sort_by_key(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap());
    assert_eq!(
        starts.len(),
        2,
        "two genuine attempts without artificial repeated serialization"
    );
    let at: Vec<u64> = starts
        .iter()
        .map(|r| r["time_ps"].as_str().unwrap().parse().unwrap())
        .collect();
    assert!(at[0] < 1_000_000);
    assert!(
        (500_000_000..500_100_000).contains(&at[1]),
        "second attempt must wait for credit and actual fitting gate: {at:?}"
    );
    assert_eq!(physical(&v, "decoded_ingress").len(), 4);
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let mut credits: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.tsn.credit" && r["data"]["port"] == "Net.s1.tx_b")
        .collect();
    credits.sort_by_key(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap());
    let credit = |r: &&Value| {
        let n: r#i128 = r["data"]["magnitude"].as_str().unwrap().parse().unwrap();
        if r["data"]["sign"] == "negative" {
            -n
        } else {
            n
        }
    };
    let at100 = credits
        .iter()
        .rev()
        .find(|r| r["time_ps"] == "100000000")
        .expect("closing edge");
    let at200 = credits
        .iter()
        .find(|r| r["time_ps"] == "200000000")
        .expect("opening edge");
    assert_eq!(
        credit(at100),
        credit(at200),
        "closed physical interval contributes no CBS area"
    );
    // Event cuts are actual observable sample completions. Derive debt from
    // TxStarted to physical release, not admission/ideal planned EOF.
    let released = rows
        .iter()
        .find(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["from_port"] == "Net.s1.tx_b"
                && r["data"]["frame_id"] == "data:0"
        })
        .unwrap()["data"]["release_ps"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let admitted = physical(&v, "admitted")
        .iter()
        .map(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap())
        .min()
        .unwrap();
    let debt_at100 = 2_500_000i128 * i128::from(at[0] - admitted)
        - (7_500_000i128) * i128::from(released - at[0])
        + 2_500_000i128 * i128::from(100_000_000 - released);
    assert_eq!(
        credit(at100),
        debt_at100,
        "independent actual-attempt physical bit/s integral"
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn half_duplex_shaping_requires_the_named_experimental_policy() {
    let root = scratch();
    let mut v: Value =
        serde_json::from_slice(&fs::read(fixture().join("model.json")).unwrap()).unwrap();
    v.as_object_mut().unwrap().remove("physical_mac_policy");
    fs::write(root.join("model.json"), v.to_string()).unwrap();
    fs::copy(fixture().join("workload.json"), root.join("workload.json")).unwrap();
    let ini = fs::read_to_string(fixture().join("model.ini"))
        .unwrap()
        .replace(
            "../t1s-multidrop/models",
            fixture().join("../t1s-multidrop/models").to_str().unwrap(),
        );
    fs::write(root.join("model.ini"), ini).unwrap();
    let error = prepare(&root.join("model.ini"))
        .expect_err("an implicit half-duplex shaping policy must reject");
    assert!(
        error.message.contains("experimental.half_duplex_tsn"),
        "{}",
        error.message
    );
    fs::remove_dir_all(root).unwrap();
}
