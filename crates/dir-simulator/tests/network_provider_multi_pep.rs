//! Two PEPs on one CEP retain separate services through the production C/S owners.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt::Write, fs, path::PathBuf};

fn decimal(v: &Value) -> u64 {
    v.as_u64()
        .unwrap_or_else(|| v.as_str().unwrap().parse().unwrap())
}
fn checked_mac(wire: &Value) -> Vec<u8> {
    let hex = wire["mac_hex"].as_str().unwrap();
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bytes.len() as u64, decimal(&wire["mac_bytes"]));
    let mut crc = 0xffff_ffffu32;
    for byte in &bytes[..bytes.len() - 4] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
        }
    }
    let fcs = (!crc).to_le_bytes();
    assert_eq!(&bytes[bytes.len() - 4..], &fcs, "independent MAC FCS");
    assert_eq!(
        wire["fcs_hex"],
        fcs.iter().fold(String::new(), |mut hex, byte| {
            write!(&mut hex, "{byte:02x}").unwrap();
            hex
        })
    );
    bytes
}

#[test]
fn one_cep_uses_two_peps_without_cross_service_delivery_or_hidden_physical_handoffs() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/provider-l2/multi-pep.ini");
    let preserve = std::env::var_os("DIR_PROVIDER_MULTI_PEP_EVIDENCE");
    let scratch = preserve
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("provider-multi-pep-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();
    let output = scratch.join("result");
    run(
        prepare(&fixture).expect("two PEP bindings on one CEP must prepare"),
        &output,
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap();
    assert_eq!(result["simulation"]["termination"], "time_limit");
    assert_eq!(result["simulation"]["end_ps"], "45000000000000");
    let records = result["simulation"]["model_records"].as_array().unwrap();
    let frames = records
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.frame")
        .map(|r| (r["record_id"].as_str().unwrap(), &r["data"]))
        .collect::<BTreeMap<_, _>>();
    let transfers = records
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
        .map(|r| &r["data"])
        .collect::<Vec<_>>();
    let deliveries = records
        .iter()
        .filter(|r| {
            r["schema_name"] == "ethernet.dynamic.reception" && r["data"]["status"] == "received"
        })
        .map(|r| &r["data"])
        .collect::<Vec<_>>();
    let expected = BTreeMap::from([
        ("learn_a:0", ("Net.a0.rx", 100)),
        ("learn_b:0", ("Net.b0.rx", 200)),
        ("a_data:0", ("Net.a3.rx", 100)),
        ("b_data:0", ("Net.b1.rx", 200)),
        ("a1_data:0", ("Net.a3.rx", 100)),
        ("a2_data:0", ("Net.a3.rx", 100)),
        ("c20_east_to_west:0", ("Net.b1.rx", 300)),
        ("c20_west_to_east:0", ("Net.a0.rx", 300)),
    ]);
    let pnps = transfers
        .iter()
        .filter(|t| t["from_port"].as_str().unwrap().ends_with(".tx_pnp"))
        .copied()
        .collect::<Vec<_>>();
    fs::write(scratch.join("verification.json"),serde_json::to_vec_pretty(&json!({"fixture":fixture,"frames":frames.len(),"actual_endpoint_deliveries":deliveries.len(),"physical_data_transfers":transfers.len(),"pnp_data_transfers":pnps.len(),"deliveries":deliveries})).unwrap()).unwrap();
    assert_eq!(frames.len(), 8);
    assert_eq!(
        deliveries.len(),
        8,
        "original six and two C20 packets, without cross-service delivery"
    );
    assert_eq!(
        transfers.len(),
        26,
        "twenty original physical copies including initial S100 learning flood, plus six C20 copies; internal ISS adds none"
    );
    assert_eq!(pnps.len(), 8);
    for (id, (ingress, service)) in &expected {
        let received = deliveries
            .iter()
            .filter(|r| r["frame_id"] == *id)
            .collect::<Vec<_>>();
        assert_eq!(received.len(), 1, "exactly one actual delivery for {id}");
        assert_eq!(
            received[0]["ingress"], *ingress,
            "no cross-service endpoint delivery for {id}"
        );
        let final_tx = &records
            .iter()
            .find(|r| {
                r["schema_name"] == "ethernet.dynamic.transfer"
                    && r["record_id"] == received[0]["transfer_id"]
            })
            .unwrap()["data"];
        assert_eq!(
            final_tx["wire"]["mac_hex"], frames[id]["mac_hex"],
            "original C-tag, payload, padding and FCS restored for {id}"
        );
        checked_mac(&final_tx["wire"]);
        let pnp = pnps
            .iter()
            .filter(|t| t["frame_id"] == *id)
            .collect::<Vec<_>>();
        assert_eq!(pnp.len(), 1, "one physical provider crossing for {id}");
        let pnp = pnp[0];
        let wire = &pnp["wire"];
        let actual = checked_mac(wire);
        let original = checked_mac(frames[id]);
        assert!(wire["tag"].is_null());
        assert_eq!(wire["tags"].as_array().unwrap().len(), 2);
        assert_eq!(wire["tags"][0]["tpid"], "34984");
        assert_eq!(wire["tags"][0]["tci"]["vid"], service.to_string());
        assert_eq!(wire["tags"][1]["tpid"], "33024");
        assert_eq!(wire["tags"][1]["tci"], frames[id]["tag"]);
        assert_eq!(&actual[..12], &original[..12]);
        assert_eq!(&actual[12..14], &[0x88, 0xa8]);
        assert_eq!(
            &actual[16..actual.len() - 4],
            &original[12..original.len() - 4],
            "inner C bytes remain intact on the real PNP"
        );
        assert_eq!(decimal(&pnp["eof_ps"]) - decimal(&pnp["sof_ps"]), 640_000);
        assert_eq!(
            decimal(&pnp["release_ps"]) - decimal(&pnp["eof_ps"]),
            96_000
        );
    }
    for (component, adapter, cep, pep) in [
        (
            "Net.east.ca",
            "Net_east_ca_rstp",
            "Net.east.tx_a0",
            "Net.east.ca.pep300",
        ),
        (
            "Net.west.cb",
            "Net_west_cb_rstp",
            "Net.west.tx_b1",
            "Net.west.cb.pep300",
        ),
    ] {
        for port in [cep, pep] {
            assert!(
                records
                    .iter()
                    .any(|r| r["schema_name"] == "ethernet.control.wire.transition"
                        && r["data"]["adapter_id"] == adapter
                        && r["data"]["bridge"] == component.rsplit_once('.').unwrap().0
                        && r["data"]["port"] == port),
                "actual C protocol owner for {port}"
            );
        }
    }
    for side in ["east", "west"] {
        for port in [format!("Net.{side}.s.cnp300"), format!("Net.{side}.tx_pnp")] {
            assert!(
                records
                    .iter()
                    .any(|r| r["schema_name"] == "ethernet.control.wire.tree_policy"
                        && r["data"]["bridge"] == format!("Net.{side}")
                        && r["data"]["component_id"] == format!("provider_{side}_mstp")
                        && r["data"]["port"] == port
                        && decimal(&r["data"]["mstid"]) == 3
                        && r["data"]["mapped_vids"]
                            .as_array()
                            .is_some_and(|v| v.iter().any(|vid| decimal(vid) == 300))
                        && r["data"]["policy"]["forwarding"] == true),
                "actual S MSTI3 forwarding policy for {port}"
            );
        }
    }
    for t in &transfers {
        assert!(
            !t["from_port"].as_str().unwrap().contains(".pep")
                && !t["from_port"].as_str().unwrap().contains(".cnp")
        );
    }
    for tx in result["metadata"]["network_runtime"]["wire"]["tx"]
        .as_array()
        .unwrap()
    {
        let port = tx["port"].as_str().unwrap();
        assert!(
            !port.contains(".pep") && !port.contains(".cnp"),
            "internal ISS cannot consume a hidden physical boundary"
        );
    }
    for (id, port) in [
        ("c20_east_to_west:0", "Net.east.ca.pep300"),
        ("c20_west_to_east:0", "Net.west.cb.pep300"),
    ] {
        assert!(
            records
                .iter()
                .any(|r| r["schema_name"] == "dir.network.provider"
                    && r["data"]["kind"] == "handoff"
                    && r["data"]["frame_id"] == id
                    && r["data"]["port"] == port),
            "selected second PEP actual handoff for {id}"
        );
    }
    if preserve.is_none() {
        fs::remove_dir_all(&scratch).unwrap();
    }
}
