//! Actual packet forwarding across MST region and protocol boundaries.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

fn decimal(value: &Value) -> u64 {
    value
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| value.as_u64())
        .unwrap()
}

#[test]
fn differing_region_identity_map_and_rstp_peer_use_boundary_forwarding() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = root.join("examples/network-composite/mstp-region");
    for variant in ["name", "revision", "map", "rstp"] {
        eprintln!("checking actual MST boundary {variant}");
        let scratch = std::env::temp_dir().join(format!(
            "dir-mstp-boundary-{}-{variant}",
            std::process::id()
        ));
        fs::create_dir_all(&scratch).unwrap();
        let mut model: Value =
            serde_json::from_slice(&fs::read(fixture.join("model.json")).unwrap()).unwrap();
        // The baseline has fixed per-instance routes. Unknown unicast makes
        // these boundary variants use their newly selected physical trees.
        for bridge in model["base"]["config"]["base"]["config"]["switches"]
            .as_array_mut()
            .unwrap()
        {
            bridge["fdb"] = serde_json::json!([]);
        }
        let peer = model["wire_control"]["adapters"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|a| a["id"] == "mstp_s3")
            .unwrap();
        match variant {
            "name" => peer["mstp"]["region"]["name"] = "other-region".into(),
            "revision" => peer["mstp"]["region"]["revision"] = 2.into(),
            "map" => peer["mstp"]["region"]["vlan_to_msti"] = serde_json::json!({"10":2,"20":1}),
            "rstp" => {
                let old = peer.as_object_mut().unwrap().remove("mstp").unwrap();
                let mut ports = old["port_parameters"].clone();
                for port in ports.as_array_mut().unwrap() {
                    port.as_object_mut().unwrap().remove("internal_path_cost");
                }
                peer["protocol"] = "rstp".into();
                peer["protocol_revision"] = "ieee802.1q-2022.rstp.initial.v1".into();
                peer["rstp"] = serde_json::json!({"bridge_address":old["bridge_address"],
                    "bridge_priority":32768,"port_parameters":ports});
            }
            _ => unreachable!(),
        }
        fs::write(scratch.join("model.json"), model.to_string()).unwrap();
        fs::copy(fixture.join("workload.json"), scratch.join("workload.json")).unwrap();
        fs::write(scratch.join("model.ini"), format!(
            "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 36000000000000ps\n",
            root.join("examples/ethernet/dynamic/models").display())).unwrap();
        run(
            prepare(&scratch.join("model.ini")).unwrap(),
            &scratch.join("result"),
        )
        .unwrap();
        let result: Value =
            serde_json::from_slice(&fs::read(scratch.join("result/results.json")).unwrap())
                .unwrap();
        let records = result["simulation"]["model_records"].as_array().unwrap();
        let tx: BTreeMap<_, _> = records
            .iter()
            .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
            .map(|r| (r["record_id"].as_str().unwrap(), &r["data"]))
            .collect();
        for frame in ["vid10:0", "vid20:0"] {
            let original = records
                .iter()
                .find(|r| r["schema_name"] == "ethernet.dynamic.frame" && r["record_id"] == frame)
                .unwrap();
            let delivered: Vec<_> = records
                .iter()
                .filter(|r| {
                    r["schema_name"] == "ethernet.dynamic.reception"
                        && r["data"]["frame_id"] == frame
                        && r["data"]["status"] == "received"
                })
                .collect();
            assert_eq!(delivered.len(), 1, "{variant}/{frame}");
            assert_eq!(delivered[0]["data"]["ingress"], "Net.c.rx");
            let mut id = delivered[0]["data"]["transfer_id"].as_str().unwrap();
            let mut hops = 0;
            loop {
                let row = tx[id];
                hops += 1;
                assert!(hops <= tx.len(), "physical ancestry is acyclic");
                assert!(row["sof_ps"].is_string() && row["eof_ps"].is_string());
                assert_eq!(row["wire"]["mac_hex"], original["data"]["mac_hex"]);
                let Some(parent) = row["parent_transfer_id"].as_str() else {
                    break;
                };
                id = parent;
            }
        }
        if variant != "rstp" {
            // The superior designated ports keep these LANs in the s1/s2
            // region; the ports of the other region observe the boundary.
            for boundary in ["Net.s3.tx_b", "Net.s3.tx_c"] {
                let last = records
                    .iter()
                    .filter(|r| {
                        r["schema_name"] == "ethernet.control.wire.tree_policy"
                            && r["data"]["port"] == boundary
                            && r["data"]["mstid"] == 0
                    })
                    .max_by_key(|r| decimal(&r["time_ps"]))
                    .unwrap();
                assert_eq!(last["data"]["boundary"], true, "{variant}/{boundary}");
            }
            assert!(
                records
                    .iter()
                    .any(|r| r["schema_name"] == "ethernet.control.wire.tree_policy"
                        && r["data"]["boundary"] == true
                        && r["data"]["policy"]["role"] == "Master"),
                "external root path gives an actual MSTI Master port: {variant}"
            );
        } else {
            // The RST peer transmits version 2 and accepts the CIST carried
            // by the MSTP version 3 BPDU on the shared LAN.
            for (adapter, port, version) in [
                ("mstp_s3", "Net.s3.tx_b", "02"),
                ("mstp_s1", "Net.s1.tx_c", "03"),
            ] {
                assert!(
                    records
                        .iter()
                        .any(|r| r["schema_name"] == "ethernet.control.wire.event"
                            && r["data"]["adapter_id"] == adapter
                            && r["data"]["port"] == port
                            && r["data"]["event_kind"] == "tx_started"
                            && r["data"]["bytes_hex"]
                                .as_str()
                                .is_some_and(|bytes| bytes.get(38..40) == Some(version))),
                    "{adapter} transmits version {version} on {port}"
                );
            }
            assert!(
                records
                    .iter()
                    .any(|r| r["schema_name"] == "ethernet.control.wire.event"
                        && r["data"]["adapter_id"] == "mstp_s3"
                        && r["data"]["port"] == "Net.s3.tx_b"
                        && r["data"]["event_kind"] == "receive"
                        && r["data"]["bytes_hex"]
                            .as_str()
                            .is_some_and(|bytes| bytes.get(38..40) == Some("03"))
                        && r["data"]["outcome"] == "changed"),
                "RSTP peer decodes the CIST from a real version 3 BPDU"
            );
        }
        fs::remove_dir_all(&scratch).unwrap();
    }
}
