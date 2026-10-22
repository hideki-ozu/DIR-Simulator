//! A project fragment owner and a distinct sampled MAC share one network FES.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{fs, path::Path};

#[test]
fn scoped_project_fragments_preserve_ordinary_sampled_and_terminal_delivery() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::env::temp_dir().join(format!("dir-sparse-preemption-{}", std::process::id()));
    run(
        prepare(&repo.join("examples/network-composite/preemption-phy/model.ini")).unwrap(),
        &out,
    )
    .unwrap();
    let v: Value = serde_json::from_slice(&fs::read(out.join("results.json")).unwrap()).unwrap();
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let fragments: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.preemption.fragment")
        .collect();
    assert_eq!(fragments.len(), 4);
    assert_eq!(
        fragments
            .iter()
            .map(|r| r["data"]["payload_bytes"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap())
            .sum::<u64>(),
        68
    );
    let transfers: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.preemption.transfer")
        .collect();
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0]["data"]["from_port"], "Net.a.tx");
    let received: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.phy" && r["data"]["kind"] == "decoded_ingress")
        .collect();
    assert_eq!(received.len(), 2);
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["ingress"] == "Net.b.rx"
                && r["data"]["status"] == "received")
    );
    fs::remove_dir_all(out).unwrap();
}
