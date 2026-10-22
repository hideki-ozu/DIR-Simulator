//! Actual VLAN-specific forwarding on a physical cycle, including a CIST-blocked port.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

#[test]
fn mstis_forward_and_learn_on_their_own_tree_instead_of_cist() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/mstp-region/model.ini");
    let scratch = std::env::temp_dir().join(format!("dir-mstp-cycle-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    run(prepare(&fixture).unwrap(), &scratch.join("result")).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(scratch.join("result/results.json")).unwrap()).unwrap();
    let records = result["simulation"]["model_records"].as_array().unwrap();
    let transfers: BTreeMap<_, _> = records
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
        .map(|r| (r["record_id"].as_str().unwrap(), &r["data"]))
        .collect();
    for (frame, expected_ports) in [
        (
            "vid10:0",
            vec!["Net.a.tx", "Net.s1.tx_b", "Net.s2.tx_c", "Net.s3.tx_a"],
        ),
        ("vid20:0", vec!["Net.a.tx", "Net.s1.tx_c", "Net.s3.tx_a"]),
    ] {
        let received: Vec<_> = records
            .iter()
            .filter(|r| {
                r["schema_name"] == "ethernet.dynamic.reception"
                    && r["data"]["frame_id"] == frame
                    && r["data"]["ingress"] == "Net.c.rx"
                    && r["data"]["status"] == "received"
            })
            .collect();
        assert_eq!(received.len(), 1, "one actual delivery for {frame}");
        let mut current = received[0]["data"]["transfer_id"].as_str().unwrap();
        let final_wire = &transfers[current]["wire"];
        let original = records
            .iter()
            .find(|r| r["schema_name"] == "ethernet.dynamic.frame" && r["record_id"] == frame)
            .unwrap();
        assert_eq!(
            final_wire["mac_hex"], original["data"]["mac_hex"],
            "actual bytes preserved for {frame}"
        );
        let mut ports = Vec::new();
        loop {
            let transfer = transfers[current];
            assert!(transfer["sof_ps"].is_string() && transfer["eof_ps"].is_string());
            ports.push(transfer["from_port"].as_str().unwrap());
            assert!(ports.len() <= 8, "no cyclic parent chain");
            let Some(parent) = transfer["parent_transfer_id"].as_str() else {
                break;
            };
            current = parent;
        }
        ports.reverse();
        assert_eq!(ports, expected_ports, "actual path for {frame}");
    }
    let tree = |id: u64| {
        records
            .iter()
            .rev()
            .find(|r| {
                r["schema_name"] == "ethernet.control.wire.tree_policy"
                    && r["data"]["port"] == "Net.s3.tx_c"
                    && r["data"]["mstid"].as_u64() == Some(id)
            })
            .unwrap()
    };
    assert_eq!(tree(0)["data"]["policy"]["role"], "Alternate");
    assert_eq!(tree(0)["data"]["policy"]["forwarding"], false);
    assert_eq!(tree(1)["data"]["policy"]["forwarding"], true);
    assert!(
        records
            .iter()
            .filter(|r| r["schema_name"] == "ethernet.dynamic.policy")
            .filter_map(|r| r["data"]["changes"].as_array())
            .flatten()
            .any(|change| change["table"] == "mac"
                && change["key"]["switch"] == "Net.s3"
                && change["key"]["vid"] == "10"
                && change["key"]["mac"] == "02:00:00:00:00:01"
                && change["after"]["value"] == "Net.s3.tx_c"),
        "source learning follows the VID10 tree through the CIST-blocked ingress"
    );
    fs::remove_dir_all(&scratch).unwrap();
}
