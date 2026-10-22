//! Production-path shared MAC service with three physical bridge attachments.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network-composite/shared-lan")
}
fn scratch() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "dir-shared-lan-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
#[test]
fn three_bridge_shared_lan_has_one_mac_reservation_and_two_receptions_per_tx() {
    let path = scratch();
    run(
        prepare(&fixture().join("model.ini")).unwrap(),
        &path.join("result"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(path.join("result/results.json")).unwrap()).unwrap();
    let records = result["simulation"]["model_records"].as_array().unwrap();
    let segments: Vec<_> = records
        .iter()
        .filter(|r| r["schema_name"] == "dir.network.segment")
        .collect();
    let mut starts: Vec<_> = segments
        .iter()
        .filter(|r| r["data"]["kind"] == "tx_started")
        .collect();
    let releases: Vec<_> = segments
        .iter()
        .filter(|r| r["data"]["kind"] == "tx_released")
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(releases.len(), 2);
    starts.sort_by_key(|r| r["time_ps"].as_str().unwrap().parse::<u64>().unwrap());
    for pair in starts.windows(2) {
        let first = pair[0]["time_ps"].as_str().unwrap().parse::<u64>().unwrap();
        let second = pair[1]["time_ps"].as_str().unwrap().parse::<u64>().unwrap();
        let release = releases
            .iter()
            .find(|r| r["data"]["owner"] == pair[0]["data"]["owner"])
            .unwrap();
        let released = release["time_ps"].as_str().unwrap().parse::<u64>().unwrap();
        assert!(first < released && released <= second);
    }
    for start in starts {
        let owner = &start["data"]["owner"];
        let recipients: Vec<_> = segments
            .iter()
            .filter(|r| r["data"]["kind"] == "receive" && &r["data"]["owner"] == owner)
            .map(|r| r["data"]["receiver_port"].as_str().unwrap())
            .collect();
        assert_eq!(recipients, ["Net.s2.rx_b", "Net.s3.rx_b"]);
    }
    assert!(!records.iter().any(|r| {
        r["schema_name"] == "ethernet.dynamic.reception"
            && r["data"]["ingress"]
                .as_str()
                .is_some_and(|port| port.starts_with("Net.hub."))
    }));
    assert_eq!(
        records
            .iter()
            .filter(|r| r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["ingress"] == "Net.b.rx"
                && r["data"]["status"] == "received")
            .count(),
        2
    );
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn shared_lan_rejects_duplicate_or_mismatched_physical_carriers() {
    let path = scratch();
    let source = fixture();
    let mut ini = fs::read_to_string(source.join("model.ini")).unwrap();
    ini = ini.replace(
        "ned-path = \"models\"",
        &format!("ned-path = \"{}\"", source.join("models").display()),
    );
    fs::write(path.join("model.ini"), ini).unwrap();
    fs::copy(source.join("workload.json"), path.join("workload.json")).unwrap();
    let baseline: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    for case in ["duplicate", "bitrate", "endpoint"] {
        let mut model = baseline.clone();
        match case {
            "duplicate" => {
                let first = model["shared_lans"][0]["ports"][0].clone();
                model["shared_lans"][0]["ports"][2] = first;
            }
            "bitrate" => model["shared_lans"][0]["bitrate_bps"] = "2000000000".into(),
            "endpoint" => model["shared_lans"][0]["ports"][2] = "Net.a.tx".into(),
            _ => unreachable!(),
        }
        fs::write(path.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
        assert!(prepare(&path.join("model.ini")).is_err(), "{case}");
    }
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn same_bridge_dual_lan_attachment_reaches_backup_and_mixed_tree_peers_decode_bpdus() {
    let path = scratch();
    run(
        prepare(&fixture().join("backup-mstp.ini")).unwrap(),
        &path.join("backup"),
    )
    .unwrap();
    let backup: Value =
        serde_json::from_slice(&fs::read(path.join("backup/results.json")).unwrap()).unwrap();
    let records = backup["simulation"]["model_records"].as_array().unwrap();
    for mstid in [0, 1, 2] {
        assert!(
            records
                .iter()
                .any(|r| r["schema_name"] == "ethernet.control.wire.tree_policy"
                    && r["data"]["bridge"] == "Backup.s1"
                    && r["data"]["port"] == "Backup.s1.tx_c"
                    && r["data"]["mstid"] == mstid
                    && r["data"]["policy"]["role"] == "Backup")
        );
    }
    run(
        prepare(&fixture().join("mixed-tree.ini")).unwrap(),
        &path.join("mixed"),
    )
    .unwrap();
    let mixed: Value =
        serde_json::from_slice(&fs::read(path.join("mixed/results.json")).unwrap()).unwrap();
    let records = mixed["simulation"]["model_records"].as_array().unwrap();
    assert!(records.iter().any(|r| {
        r["schema_name"] == "ethernet.control.wire.event"
            && r["data"]["event_kind"] == "receive"
            && r["data"]["adapter_id"] == "rstp-s3"
            && r["data"]["packet_id"]
                .as_str()
                .is_some_and(|id| id.contains("/mstp-"))
    }));
    assert!(records.iter().any(|r| {
        r["schema_name"] == "ethernet.control.wire.event"
            && r["data"]["event_kind"] == "receive"
            && r["data"]["adapter_id"] == "mstp-s1"
            && r["data"]["packet_id"]
                .as_str()
                .is_some_and(|id| id.contains("/rstp-"))
    }));
    fs::remove_dir_all(path).unwrap();
}
