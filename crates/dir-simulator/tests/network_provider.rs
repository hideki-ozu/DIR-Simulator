//! Customer isolation and real stacked bytes through the ordinary network owner.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

#[test]
fn overlapping_customer_addresses_remain_isolated_through_provider_service() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/provider-l2/model.ini");
    let scratch = std::env::temp_dir().join(format!("dir-provider-network-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    run(prepare(&fixture).unwrap(), &scratch.join("result")).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(scratch.join("result/results.json")).unwrap()).unwrap();
    let records = result["simulation"]["model_records"].as_array().unwrap();
    let frames: BTreeMap<_, _> = records
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.frame")
        .map(|r| (r["record_id"].as_str().unwrap(), &r["data"]))
        .collect();
    let expected = BTreeMap::from([
        ("learn_a:0", "Net.a0.rx"),
        ("learn_b:0", "Net.b0.rx"),
        ("a_data:0", "Net.a3.rx"),
        ("b_data:0", "Net.b1.rx"),
        ("a1_data:0", "Net.a3.rx"),
        ("a2_data:0", "Net.a3.rx"),
    ]);
    let deliveries: Vec<_> = records
        .iter()
        .filter(|r| {
            r["schema_name"] == "ethernet.dynamic.reception" && r["data"]["status"] == "received"
        })
        .collect();
    assert_eq!(deliveries.len(), expected.len());
    for (frame, ingress) in &expected {
        let received: Vec<_> = deliveries
            .iter()
            .filter(|r| r["data"]["frame_id"] == *frame)
            .collect();
        assert_eq!(
            received.len(),
            1,
            "exactly one actual endpoint receives {frame}"
        );
        assert_eq!(
            received[0]["data"]["ingress"], *ingress,
            "no cross-tenant delivery"
        );
        let final_tx = records
            .iter()
            .find(|r| {
                r["schema_name"] == "ethernet.dynamic.transfer"
                    && r["record_id"] == received[0]["data"]["transfer_id"]
            })
            .unwrap();
        assert_eq!(
            final_tx["data"]["wire"]["mac_hex"], frames[frame]["mac_hex"],
            "restored C-tag/payload/FCS"
        );
    }
    let pnps: Vec<_> = records
        .iter()
        .filter(|r| {
            r["schema_name"] == "ethernet.dynamic.transfer"
                && r["data"]["from_port"]
                    .as_str()
                    .is_some_and(|p| p.ends_with(".tx_pnp"))
        })
        .collect();
    assert_eq!(
        pnps.len(),
        6,
        "internal handoffs must not create physical transmissions"
    );
    for tx in pnps {
        let data = &tx["data"];
        let frame_id = data["frame_id"].as_str().unwrap();
        let original = frames[frame_id];
        let wire = &data["wire"];
        assert!(wire["tag"].is_null());
        let tags = wire["tags"].as_array().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0]["tpid"], "34984");
        assert_eq!(tags[1]["tpid"], "33024");
        assert_eq!(tags[1]["tci"], original["tag"]);
        assert_eq!(
            tags[0]["tci"]["vid"],
            if frame_id.starts_with("b_") || frame_id == "learn_b:0" {
                "200"
            } else {
                "100"
            }
        );
        let actual = wire["mac_hex"].as_str().unwrap();
        let old = original["mac_hex"].as_str().unwrap();
        assert_eq!(&actual[..24], &old[..24]);
        assert_eq!(&actual[24..28], "88a8");
        assert_eq!(
            &actual[32..actual.len() - 8],
            &old[24..old.len() - 8],
            "inner customer bytes unchanged"
        );
        assert_eq!(wire["mac_bytes"], "72");
        let time = |key: &str| data[key].as_str().unwrap().parse::<u64>().unwrap();
        assert_eq!(
            time("eof_ps") - time("sof_ps"),
            640_000,
            "actual preamble +72 MAC bytes at1Gbps"
        );
        assert_eq!(time("release_ps") - time("eof_ps"), 96_000, "actual IFG");
    }
    assert_eq!(records.iter().filter(|r| r["schema_name"] == "dir.network.provider"
        && r["data"]["kind"] == "handoff").count(), 12);
    fs::remove_dir_all(&scratch).unwrap();
}
