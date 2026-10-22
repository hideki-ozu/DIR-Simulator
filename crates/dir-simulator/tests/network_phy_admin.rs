//! Administrative link ownership reaches the actual trained physical pair.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{fs, path::Path};

#[test]
#[ignore = "runs actual continuous sampled Clause97 startup before an idle administrative cut"]
fn idle_admin_down_withdraws_actual_phy_readiness_and_bilateral_merge_capabilities() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = repo.join("examples/network-composite/gigabit-merge");
    let root = std::env::var_os("DIR_PHY_ADMIN_EVIDENCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("dir-phy-admin-{}", std::process::id()));
    fs::create_dir_all(root.join("models/ethtsn")).unwrap();
    fs::copy(
        source.join("models/ethtsn/Main.ned"),
        root.join("models/ethtsn/Main.ned"),
    )
    .unwrap();
    fs::copy(source.join("model.json"), root.join("model.json")).unwrap();
    let mut work: Value =
        serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
    work["controls"] = json!([{"id":"idle-down", "at_ps":"5500000000",
        "kind":"link_set", "link":"a-b", "up":false}]);
    fs::write(
        root.join("workload.json"),
        serde_json::to_vec_pretty(&work).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("model.ini"),
        fs::read_to_string(source.join("model.ini"))
            .unwrap()
            .replace("6012000000ps", "5501000000ps"),
    )
    .unwrap();
    run(prepare(&root.join("model.ini")).unwrap(), &root.join("out")).unwrap();
    let v: Value =
        serde_json::from_slice(&fs::read(root.join("out/results.json")).unwrap()).unwrap();
    assert_eq!(v["simulation"]["termination"], "time_limit");
    let rows = v["simulation"]["model_records"].as_array().unwrap();
    let merge: Vec<_> = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.mac_merge.verification")
        .map(|r| &r["data"])
        .collect();
    assert!(
        merge.iter().any(|r| r["p_active"] == true),
        "must acquire real peer capability before idle loss"
    );
    for port in ["Net.a.tx", "Net.b.tx"] {
        let last = merge.iter().rev().find(|r| r["port"] == port).unwrap();
        assert_eq!(last["p_active"], false);
        assert!(last["peer_evidence"].is_null());
        assert!(last["capability_expires_ps"].is_null());
    }
    assert!(rows.iter().any(|r| r["schema_name"] == "dir.network.phy"
        && r["data"]["kind"] == "TxReady"
        && r["data"]["ready"] == false));
    assert!(merge.iter().any(|r| r["cause"] == "actual_phy_link_reset"));
    assert!(
        rows.iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.control"
                && r["data"]["kind"] == "link_set"
                && r["data"]["outcome"] == "changed")
    );
    if std::env::var_os("DIR_PHY_ADMIN_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}
