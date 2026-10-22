//! Registered runtime and export coverage for the explicit R-TAG wire profile.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/frer-rtag/model.ini")
}

#[test]
fn replicated_copies_transmit_tagged_mac_bytes_and_eliminate_once() {
    let destination = std::env::temp_dir().join(format!("dir-frer-rtag-{}", std::process::id()));
    let _ = fs::remove_dir_all(&destination);
    let prepared = prepare(&example()).unwrap();
    run(prepared, &destination).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(destination.join("results.json")).unwrap()).unwrap();
    assert_eq!(result["metadata"]["model_profile"], "ethernet.frer.rtag.v1");
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let transfers: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["schema_name"] == "ethernet.dynamic.transfer"
                && row["data"]["status"] == "serialized"
        })
        .collect();
    assert!(
        transfers.len() >= 3,
        "each branch must traverse actual links"
    );
    for transfer in &transfers {
        let wire = &transfer["data"]["wire"];
        let mac = wire["mac_hex"].as_str().unwrap();
        assert!(
            mac.contains("f1c100000000"),
            "R-TAG missing on hop {}",
            transfer["data"]["from_port"]
        );
        assert_eq!(wire["mac_bytes"], "74");
        assert!(transfer["data"]["eof_ps"].is_string());
    }
    let recoveries: Vec<_> = rows
        .iter()
        .filter(|row| row["schema_name"] == "dir.redundancy.recovery")
        .collect();
    assert_eq!(recoveries.len(), 2);
    assert_eq!(
        recoveries
            .iter()
            .filter(|row| row["data"]["delivered"] == true)
            .count(),
        1
    );
    assert_eq!(
        recoveries
            .iter()
            .filter(|row| row["data"]["decision"] == "duplicate")
            .count(),
        1
    );
    let _ = fs::remove_dir_all(destination);
}

#[test]
fn rtag_profile_rejects_ambiguous_history() {
    let root = std::env::temp_dir().join(format!("dir-frer-rtag-invalid-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let example_root = example().parent().unwrap().to_path_buf();
    let mut model: Value =
        serde_json::from_slice(&fs::read(example_root.join("model.json")).unwrap()).unwrap();
    model["replication"]["streams"][0]["history_capacity"] = Value::from(32768);
    fs::write(root.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
    let ini = format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.frer.rtag.v1\"\nmodel-config = \"model.json\"\nworkload = \"{}\"\nsim-time-limit = 10000000ps\n",
        example_root.join("../ethernet/dynamic/models").display(),
        example_root.join("workload.json").display()
    );
    fs::write(root.join("model.ini"), ini).unwrap();
    assert!(prepare(&root.join("model.ini")).is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn distributed_bridge_pair_uses_six_physical_transfers_and_actual_terminal_delivery() {
    let destination =
        std::env::temp_dir().join(format!("dir-frer-distributed-{}", std::process::id()));
    let _ = fs::remove_dir_all(&destination);
    let prepared = prepare(&example().parent().unwrap().join("distributed/model.ini")).unwrap();
    run(prepared, &destination).unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(destination.join("results.json")).unwrap()).unwrap();
    let rows = result["simulation"]["model_records"].as_array().unwrap();
    let transfers = rows
        .iter()
        .filter(|r| r["schema_name"] == "ethernet.dynamic.transfer")
        .map(|r| &r["data"])
        .collect::<Vec<_>>();
    assert_eq!(transfers.len(), 6);
    for transfer in &transfers {
        let branch = matches!(
            transfer["from_port"].as_str().unwrap(),
            "Net.r.tx_a" | "Net.r.tx_b" | "Net.a.tx_e" | "Net.b.tx_e"
        );
        assert_eq!(
            transfer["wire"]["mac_hex"]
                .as_str()
                .unwrap()
                .contains("f1c100000000"),
            branch
        );
        assert_eq!(
            transfer["wire"]["mac_bytes"],
            if branch { "74" } else { "68" }
        );
    }
    let at =
        |port: &str| transfers.iter().find(|t| t["from_port"] == port).unwrap()["sof_ps"].clone();
    assert_eq!(at("Net.s.tx"), "0");
    assert_eq!(at("Net.r.tx_a"), "611000");
    assert_eq!(at("Net.r.tx_b"), "611000");
    assert_eq!(at("Net.a.tx_e"), "1270000");
    assert_eq!(at("Net.b.tx_e"), "1270000");
    assert_eq!(at("Net.e.tx_d"), "1929000");
    let recoveries = rows
        .iter()
        .filter(|r| r["schema_name"] == "dir.redundancy.recovery")
        .collect::<Vec<_>>();
    assert_eq!(recoveries.len(), 2);
    assert_eq!(
        recoveries
            .iter()
            .filter(|r| r["data"]["continuation_created"] == true)
            .count(),
        1
    );
    assert!(recoveries.iter().all(|r| r["data"]["delivered"] == false));
    let origin = rows
        .iter()
        .find(|r| r["schema_name"] == "dir.redundancy.origin")
        .unwrap();
    assert_eq!(origin["data"]["completed_ps"], "2541000");
    assert_eq!(origin["data"]["completed_count"], "1");
    let _ = fs::remove_dir_all(destination);
}

fn distributed_variant(
    name: &str,
    horizon: u64,
    change: impl FnOnce(&mut Value, &mut Value),
) -> Value {
    distributed_variant_checked(name, horizon, None, change)
}
fn distributed_variant_checked(
    name: &str,
    horizon: u64,
    checkpoint_after: Option<u64>,
    change: impl FnOnce(&mut Value, &mut Value),
) -> Value {
    let source = example().parent().unwrap().join("distributed");
    let dir = std::env::temp_dir().join(format!(
        "dir-frer-distributed-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let mut model: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(source.join("workload.json")).unwrap()).unwrap();
    change(&mut model, &mut workload);
    fs::write(dir.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
    fs::write(
        dir.join("workload.json"),
        serde_json::to_vec(&workload).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("model.ini"), format!("[General]\nnetwork = ethdistributed.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.frer.rtag.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = {horizon}ps\n",source.join("models").display())).unwrap();
    let prepared = prepare(&dir.join("model.ini")).unwrap();
    run(prepared.clone(), &dir.join("output")).unwrap();
    if let Some(after) = checkpoint_after {
        use dir_simulator::{
            CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
            restore_checkpoint,
        };
        let (mut session, controller) = RunSession::new(
            prepared,
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        while session.snapshot().common.last_event_time_ps.unwrap_or(0) < after {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let bundle = dir.join("checkpoint");
        assert_eq!(
            session
                .checkpoint(&bundle, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut restored, token) = restore_checkpoint(
            &bundle,
            dir_simulator::registry::Registry::default(),
            CheckpointLimits::default(),
        )
        .unwrap();
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    }
    let result =
        serde_json::from_slice(&fs::read(dir.join("output/results.json")).unwrap()).unwrap();
    fs::remove_dir_all(dir).unwrap();
    result
}
fn distributed_rows(result: &Value, schema: &str) -> Vec<Value> {
    result["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["schema_name"] == schema)
        .map(|r| r["data"].clone())
        .collect()
}
fn distributed_limit(model: &mut Value, port: &str, limit: u64) {
    let output = model["base"]["config"]["base"]["config"]["outputs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|o| o["port"] == port)
        .unwrap();
    output["queues"][0]["capacity_bytes"] = Value::from(limit.to_string());
}
#[test]
fn distributed_prefix_and_actual_processing_gate_replication_and_delivery() {
    let prefix_drop = distributed_variant("prefixdrop", 10_000_000, |m, _| {
        distributed_limit(m, "Net.s.tx", 67)
    });
    assert_eq!(
        distributed_rows(&prefix_drop, "ethernet.dynamic.transfer").len(),
        1
    );
    assert!(distributed_rows(&prefix_drop, "dir.redundancy.recovery").is_empty());
    assert_eq!(
        prefix_drop["metadata"]["network_runtime"]["redundancy"]["streams"][0]["next_sequence"],
        "0"
    );
    for (name, horizon, transfers, recovery, delivery) in [
        ("prefixflight", 608_000, 1, 0, false),
        ("prefixprocessing", 610_000, 1, 0, false),
        ("branchflight", 1_926_000, 5, 0, false),
        ("recoveryprocessing", 1_928_000, 5, 0, false),
        ("suffixflight", 2_537_000, 6, 2, false),
        ("destinationprocessing", 2_540_000, 6, 2, false),
        ("terminal", 2_541_001, 6, 2, true),
    ] {
        let result = distributed_variant(name, horizon, |_, _| {});
        assert_eq!(
            distributed_rows(&result, "ethernet.dynamic.transfer").len(),
            transfers,
            "{name}"
        );
        assert_eq!(
            distributed_rows(&result, "dir.redundancy.recovery").len(),
            recovery,
            "{name}"
        );
        assert_eq!(
            distributed_rows(&result, "dir.redundancy.origin")[0]["completed_count"] == "1",
            delivery,
            "{name}"
        );
    }
}
#[test]
fn distributed_independent_branch_byte_admission_and_link_failure() {
    for (name, limit, expected) in [("73bytes", 73, 1), ("74bytes", 74, 2)] {
        let result = distributed_variant(name, 10_000_000, |m, _| {
            distributed_limit(m, "Net.r.tx_a", limit)
        });
        assert_eq!(
            distributed_rows(&result, "dir.redundancy.recovery").len(),
            expected
        );
        assert_eq!(
            distributed_rows(&result, "dir.redundancy.origin")[0]["completed_count"],
            "1"
        );
    }
    let result = distributed_variant(
        "linkfail",
        10_000_000,
        |_, w| w["controls"] = serde_json::json!([{"id":"fail-a","at_ps":"610000","kind":"link_set","link":"RA","up":false}]),
    );
    assert_eq!(
        distributed_rows(&result, "dir.redundancy.recovery").len(),
        1
    );
    assert_eq!(
        distributed_rows(&result, "dir.redundancy.origin")[0]["completed_count"],
        "1"
    );
}
#[test]
fn distributed_suffix_drop_and_late_timeout_duplicate_never_retry_continuation() {
    let result = distributed_variant("suffixdrop", 100_000_000, |m, _| {
        distributed_limit(m, "Net.e.tx_d", 67);
        m["replication"]["streams"][0]["recovery_timeout_ps"] = Value::from("1");
        // Slow B's first physical hop so its duplicate is received after reset.
        m["base"]["config"]["base"]["config"]["switches"][2]["forward_delay_ps"] =
            Value::from("1000000");
    });
    let recovery = distributed_rows(&result, "dir.redundancy.recovery");
    assert_eq!(recovery.len(), 2);
    assert_eq!(
        recovery
            .iter()
            .filter(|r| r["continuation_created"] == true)
            .count(),
        1
    );
    assert!(
        recovery
            .iter()
            .any(|r| r["window_decision"] == "accept_reset"
                && r["suppression_reason"] == "already_continued")
    );
    let transfers = distributed_rows(&result, "ethernet.dynamic.transfer");
    assert_eq!(
        transfers
            .iter()
            .filter(|t| t["from_port"] == "Net.e.tx_d")
            .count(),
        1
    );
    assert_eq!(
        distributed_rows(&result, "dir.redundancy.origin")[0]["completed_count"],
        "0"
    );
}

#[test]
fn distributed_ordinary_queue_contention_is_local_to_the_selected_output() {
    let result = distributed_variant("contention", 20_000_000, |m, w| {
        let base = &mut m["base"]["config"]["base"]["config"];
        for (index, port) in [(0, "Net.r.tx_a"), (1, "Net.a.tx_e"), (3, "Net.e.tx_d")] {
            base["switches"][index]["fdb"] =
                serde_json::json!([{"vid":10,"dst_mac":"02:00:00:00:00:02","egress":port}]);
        }
        let mut ordinary = w["generators"][0].clone();
        ordinary["id"] = Value::from("aaa_ordinary");
        ordinary["flow_id"] = Value::from("ordinary");
        ordinary["frame"]["data"] = Value::from("5a".repeat(256));
        w["generators"].as_array_mut().unwrap().insert(0, ordinary);
    });
    let origin = distributed_rows(&result, "dir.redundancy.origin")[0].clone();
    let transfers = distributed_rows(&result, "ethernet.dynamic.transfer");
    let branch = |port: &str| {
        transfers
            .iter()
            .find(|t| t["from_port"] == port && t["frame_id"] == origin["source_frame_id"])
            .unwrap()
    };
    let a = branch("Net.r.tx_a");
    let b = branch("Net.r.tx_b");
    let parse = |v: &Value| v.as_str().unwrap().parse::<u64>().unwrap();
    assert!(parse(&a["sof_ps"]) > parse(&b["sof_ps"]));
    assert_eq!(b["sof_ps"], b["queued_ps"]);
    let ordinary = transfers
        .iter()
        .find(|t| t["from_port"] == "Net.r.tx_a" && t["frame_id"] != origin["source_frame_id"])
        .unwrap();
    assert!(parse(&ordinary["release_ps"]) <= parse(&a["sof_ps"]));
    assert_eq!(origin["completed_count"], "1");
}

#[test]
fn distributed_invalid_placement_routes_and_compositions_are_rejected_at_their_pointer() {
    let source = example().parent().unwrap().join("distributed");
    for (name, target, change) in [
        ("adapter", "/replication/streams/0/placement", 0),
        ("prefix", "/replication/streams/0/placement/prefix_ports", 1),
        ("suffix", "/replication/streams/0/placement/suffix_ports", 2),
        ("branch", "/replication/streams/0/egress_paths/1", 3),
        ("preemption", "/preemption", 4),
        ("qcc", "/qcc", 5),
        ("provider", "/provider_l2", 6),
        ("wire", "/wire_control", 7),
        ("multicast", "/multicast_active", 8),
        ("shared", "/shared_lans", 9),
        ("phy", "/phy_attachments", 10),
    ] {
        let dir =
            std::env::temp_dir().join(format!("dir-frer-reject-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut model: Value =
            serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
        match change {
            0 => {
                model["replication"]["streams"][0]["placement"]["forwarding"] =
                    Value::from("dynamic")
            }
            1 => {
                model["replication"]["streams"][0]["placement"]["prefix_ports"] =
                    serde_json::json!([])
            }
            2 => {
                model["replication"]["streams"][0]["placement"]["suffix_ports"] =
                    serde_json::json!(["Net.r.tx_a"])
            }
            3 => {
                model["replication"]["streams"][0]["egress_paths"][1]["ports"] =
                    model["replication"]["streams"][0]["egress_paths"][0]["ports"].clone()
            }
            _ => {
                model["schema_name"] = Value::from("dir.ethernet_network_composite.config");
                model["preemption"] = Value::Null;
                model["clocks"] = Value::Null;
                model[target.trim_start_matches('/')] = serde_json::json!({"invalid":true});
            }
        }
        fs::write(dir.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
        fs::write(dir.join("model.ini"),format!("[General]\nnetwork = ethdistributed.Net\nned-path = \"{}\"\nmodel-profile = \"{}\"\nmodel-config = \"model.json\"\nworkload = \"{}\"\nsim-time-limit = 10000000ps\n",source.join("models").display(),if change>=4 {"ethernet.network.composite.v1"} else {"ethernet.frer.rtag.v1"},source.join("workload.json").display())).unwrap();
        let error = prepare(&dir.join("model.ini")).unwrap_err();
        assert_eq!(error.target.as_deref(), Some(target), "{name}: {error:?}");
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn distributed_distinct_branches_can_share_a_physical_output_and_contend() {
    let source = example().parent().unwrap().join("distributed");
    let dir = std::env::temp_dir().join(format!("dir-frer-shared-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("models/ethdistributed")).unwrap();
    let mut ned = fs::read_to_string(source.join("models/ethdistributed/Main.ned")).unwrap();
    ned=ned.replace("gates: output tx_r; input rx_r; output tx_e; input rx_e;", "gates: output tx_r; input rx_r; output tx_e; input rx_e; output tx_shared; input rx_shared;");
    let close = ned.rfind('}').unwrap();
    ned.insert_str(close,"  a.tx_shared --> ethdistributed.Wire --> b.rx_shared; b.tx_shared --> ethdistributed.Wire --> a.rx_shared;\n");
    fs::write(dir.join("models/ethdistributed/Main.ned"), ned).unwrap();
    let mut model: Value =
        serde_json::from_slice(&fs::read(source.join("model.json")).unwrap()).unwrap();
    model["replication"]["streams"][0]["egress_paths"][1]["ports"] =
        serde_json::json!(["Net.r.tx_a", "Net.a.tx_shared", "Net.b.tx_e"]);
    let base = &mut model["base"]["config"]["base"]["config"];
    for port in ["Net.a.tx_shared", "Net.b.tx_shared"] {
        let mut policy = base["ports"][0].clone();
        policy["port"] = Value::from(port);
        base["ports"].as_array_mut().unwrap().push(policy);
        let mut output = base["outputs"][0].clone();
        output["port"] = Value::from(port);
        base["outputs"].as_array_mut().unwrap().push(output);
    }
    base["dynamic"]["links"].as_array_mut().unwrap().push(serde_json::json!({"id":"AB","ports":["Net.a.tx_shared","Net.b.tx_shared"],"up":true,"cost":"10"}));
    for port in ["Net.a.tx_shared", "Net.b.tx_shared"] {
        model["base"]["config"]["tsn"]["outputs"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"port":port,"tas":null,"cbs":[]}));
    }
    fs::write(dir.join("model.json"), serde_json::to_vec(&model).unwrap()).unwrap();
    fs::write(dir.join("model.ini"),format!("[General]\nnetwork = ethdistributed.Net\nned-path = \"models\"\nmodel-profile = \"ethernet.frer.rtag.v1\"\nmodel-config = \"model.json\"\nworkload = \"{}\"\nsim-time-limit = 10000000ps\n",source.join("workload.json").display())).unwrap();
    run(
        prepare(&dir.join("model.ini")).unwrap(),
        &dir.join("output"),
    )
    .unwrap();
    let result: Value =
        serde_json::from_slice(&fs::read(dir.join("output/results.json")).unwrap()).unwrap();
    let transfers = distributed_rows(&result, "ethernet.dynamic.transfer");
    assert_eq!(transfers.len(), 7);
    let shared = transfers
        .iter()
        .filter(|t| t["from_port"] == "Net.r.tx_a")
        .collect::<Vec<_>>();
    assert_eq!(shared.len(), 2);
    let parse = |v: &Value| v.as_str().unwrap().parse::<u64>().unwrap();
    let mut shared = shared;
    shared.sort_by_key(|t| parse(&t["sof_ps"]));
    assert_eq!(shared[0]["release_ps"], shared[1]["sof_ps"]);
    assert_eq!(
        distributed_rows(&result, "dir.redundancy.origin")[0]["completed_count"],
        "1"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn distributed_final_stale_branch_terminates_an_unrecovered_flow() {
    let result = distributed_variant_checked("finalstale", 40_000_000, Some(22_000_000), |m, w| {
        m["base"]["config"]["base"]["config"]["switches"][2]["forward_delay_ps"] =
            Value::from("20000000");
        w["generators"][0]["times_ps"] = Value::from(
            (0..11)
                .map(|i| Value::from((i * 1_000_000u64).to_string()))
                .collect::<Vec<_>>(),
        );
        w["controls"] = serde_json::json!([
            {"id":"drop-old-a","at_ps":"610000","kind":"link_set","link":"RA","up":false},
            {"id":"restore-a","at_ps":"700000","kind":"link_set","link":"RA","up":true}
        ]);
    });
    let recoveries = distributed_rows(&result, "dir.redundancy.recovery");
    let stale = recoveries
        .iter()
        .find(|r| r["sequence"] == "0" && r["window_decision"] == "stale")
        .expect("old surviving B copy must fall outside the advanced history");
    let origins = distributed_rows(&result, "dir.redundancy.origin");
    let flow = origins
        .iter()
        .find(|o| o["origin_id"] == stale["origin_id"])
        .unwrap();
    assert_eq!(flow["completed_count"], "0");
    assert_eq!(flow["dropped"], true);
    assert!(flow["completed_ps"].is_string());
    assert_eq!(flow["continuation_created"], false);
}
