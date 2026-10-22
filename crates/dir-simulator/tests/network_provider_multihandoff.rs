//! A shared provider service can hand a frame between two local C components.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{fs, path::PathBuf};

#[test]
fn broadcast_reaches_a_second_customer_component_through_two_logical_handoffs() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/provider-l2");
    let scratch =
        std::env::temp_dir().join(format!("dir-provider-multihandoff-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    let mut ini = fs::read_to_string(source.join("model.ini")).unwrap();
    ini = ini.replace(
        "ned-path = \"models\"",
        &format!("ned-path = \"{}\"", source.join("models").display()),
    );
    fs::write(scratch.join("model.ini"), ini).unwrap();
    let mut model: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    for mapping in model["provider_l2"]["mappings"].as_array_mut().unwrap() {
        if mapping["c_component"].as_str().unwrap().ends_with(".cb") {
            mapping["s_vid"] = 100.into();
        }
    }
    for (_, port) in model["provider_l2"]["ports"].as_object_mut().unwrap() {
        if port["role"] == "customer_network"
            && port["component"].as_str().unwrap().ends_with(".s")
            && port["port"].as_str().unwrap().ends_with("cnp_b")
        {
            port["pvid"] = 100.into();
            port["member_vids"] = serde_json::json!([100]);
        }
    }
    fs::write(
        scratch.join("model.json"),
        serde_json::to_vec(&model).unwrap(),
    )
    .unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
    let generator = workload["generators"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|g| g["id"] == "a1_data")
        .unwrap();
    generator["frame"]["dst_mac"] = "ff:ff:ff:ff:ff:ff".into();
    fs::write(
        scratch.join("workload.json"),
        serde_json::to_vec(&workload).unwrap(),
    )
    .unwrap();
    run(
        prepare(&scratch.join("model.ini")).unwrap(),
        &scratch.join("result"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(scratch.join("result/results.json")).unwrap()).unwrap();
    let records = result["simulation"]["model_records"].as_array().unwrap();
    assert!(
        records
            .iter()
            .any(|r| r["schema_name"] == "ethernet.dynamic.reception"
                && r["data"]["frame_id"] == "a1_data:0"
                && r["data"]["ingress"] == "Net.b0.rx"
                && r["data"]["status"] == "received")
    );
    for (component, peer) in [("Net.east.ca", "Net.east.s"), ("Net.east.s", "Net.east.cb")] {
        assert!(
            records
                .iter()
                .any(|r| r["schema_name"] == "dir.network.provider"
                    && r["data"]["kind"] == "handoff"
                    && r["data"]["frame_id"] == "a1_data:0"
                    && r["data"]["component"] == component
                    && r["data"]["peer"] == peer),
            "missing {component} to {peer} internal handoff"
        );
    }
    fs::remove_dir_all(scratch).unwrap();
}
