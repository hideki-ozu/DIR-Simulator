//! Actual component-owned topology change, with learned entries still unexpired.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[test]
fn physical_service_link_loss_flushes_actual_component_vid_entries() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = repo.join("examples/network-composite/provider-l2");
    let root = std::env::var_os("DIR_PROVIDER_TOPOLOGY_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("dir-provider-topology-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::copy(fixture.join("protocol-model.json"), root.join("model.json")).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(fixture.join("workload.json")).unwrap()).unwrap();
    // Protocol startup precedes data. Preserve the fixture's relative traffic
    // order while moving it past C and S component tree convergence.
    for generator in workload["generators"].as_array_mut().unwrap() {
        let second = match generator["id"].as_str().unwrap() {
            "learn_a" | "learn_b" => 40,
            "a_data" | "b_data" => 41,
            "a1_data" => 42,
            "a2_data" => 43,
            other => panic!("unexpected fixture generator {other}"),
        };
        generator["times_ps"] = json!([(second * 1_000_000_000_000u64).to_string()]);
    }
    workload["controls"] = json!([{
        "id":"service-core-down", "at_ps":"40500000000000",
        "kind":"link_set", "link":"PNP", "up":false
    }]);
    fs::write(root.join("workload.json"), workload.to_string()).unwrap();
    fs::write(root.join("model.ini"), format!(
        "[General]\nnetwork = ethprovider.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 40600000000000ps\n",
        fixture.join("models").display())).unwrap();
    run(
        prepare(&root.join("model.ini")).unwrap(),
        &root.join("result"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(root.join("result/results.json")).unwrap()).unwrap();
    assert_eq!(result["simulation"]["termination"], "time_limit");
    let records = result["simulation"]["model_records"].as_array().unwrap();
    // These two real packets learned source 02 on S100 and S200 at 40s.
    // The selected 300s age cannot expire either entry at the 40.5s link event.
    for frame in ["learn_a:0", "learn_b:0"] {
        assert!(
            records
                .iter()
                .any(|r| r["schema_name"] == "dir.network.provider"
                    && r["data"]["frame_id"] == frame
                    && r["data"]["kind"] == "ingress"
                    && r["data"]["component"] == "Net.east.s")
        );
    }
    for vid in [100, 200] {
        let matching: Vec<_> = records
            .iter()
            .filter(|r| {
                r["schema_name"] == "ethernet.control.wire.event"
                    && r["time_ps"] == "40500000000000"
                    && r["data"]["adapter_id"] == "provider_east_mstp"
                    && r["data"]["event_kind"] == "link"
            })
            .flat_map(|r| r["data"]["effects"].as_array().unwrap())
            .filter(|effect| {
                effect["kind"] == "mac_flush"
                    && effect["port"] == "Net.east.tx_pnp"
                    && effect["vids"] == json!([vid])
            })
            .collect();
        assert_eq!(matching.len(), 1, "one actual per-tree flush for S{vid}");
        assert_eq!(matching[0]["completed"], true);
        assert_eq!(
            matching[0]["keys"],
            json!([{
                "switch":"Net.east.s", "vid":vid, "mac":"02:00:00:00:00:02"
            }])
        );
    }
    // Host-link control remains valid, but unrelated policy mutations cannot
    // silently bypass the provider component owner through this adapter.
    workload["controls"][0] = json!({
        "id":"unowned-membership", "at_ps":"40500000000000",
        "kind":"membership_leave", "switch":"Net.east", "port":"Net.east.tx_a0",
        "vid":10, "family":"ipv4", "group":"239.1.1.1"
    });
    fs::write(root.join("workload.json"), workload.to_string()).unwrap();
    let error = prepare(&root.join("model.ini")).expect_err("reject unsupported provider mutation");
    assert!(error.message.contains("only link_set"), "{error:?}");
    if std::env::var_os("DIR_PROVIDER_TOPOLOGY_EVIDENCE").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}
