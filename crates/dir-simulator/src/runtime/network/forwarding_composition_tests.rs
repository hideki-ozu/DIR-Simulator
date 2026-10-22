use super::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn fixture() -> (crate::PreparedSimulation, PathBuf) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/network-composite/shared-provider-frer");
    let directory = std::env::temp_dir().join(format!(
        "dir-forwarding-composition-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(directory.join("models/ethdistributed")).unwrap();
    for file in [
        "model.ini",
        "model.json",
        "workload.json",
        "models/ethdistributed/Main.ned",
    ] {
        std::fs::copy(source.join(file), directory.join(file)).unwrap();
    }
    let prepared = crate::prepare(&directory.join("model.ini")).unwrap();
    (prepared, directory)
}

#[test]
fn shared_actual_receiver_provider_stack_recovery_and_suffix() {
    let (prepared, directory) = fixture();
    let snapshot = crate::runtime::simulate(&prepared).unwrap();
    assert_ne!(snapshot.common.termination, "execution_failed");
    let rows = snapshot.ethernet.as_ref().unwrap();
    let shared_tx = rows
        .transfers
        .iter()
        .filter(|t| t.from_port == "Net.r.tx_a")
        .collect::<Vec<_>>();
    assert_eq!(shared_tx.len(), 1, "carrier fanout must allocate one TX");
    let tx = shared_tx[0];
    let receivers = rows
        .receptions
        .iter()
        .filter(|r| r.transfer_id == tx.transfer_id)
        .collect::<Vec<_>>();
    assert_eq!(receivers.len(), 2);
    assert!(
        receivers
            .iter()
            .any(|r| r.device == "Net.a" && r.status == "forwarded")
    );
    assert!(
        receivers.iter().any(|r| r.device == "Net.b"
            && r.reason.as_deref() == Some("replication_unintended_receiver"))
    );
    let wire = &tx.wire;
    assert_eq!(&wire.mac_hex[24..56], "88a800648100000af1c1000000000800");
    let source = rows
        .transfers
        .iter()
        .find(|t| t.from_port == "Net.s.tx")
        .unwrap();
    assert_eq!(wire.mac_bytes, source.wire.mac_bytes + 10);
    assert_eq!(tx.sof_ps.unwrap(), source.arrival_ps.unwrap() + 2000);
    assert_eq!(
        tx.eof_ps.unwrap() - tx.sof_ps.unwrap(),
        (wire.mac_bytes + 8) * 8000
    );
    let suffix = rows
        .transfers
        .iter()
        .find(|t| t.from_port == "Net.e.tx_d")
        .unwrap();
    assert_eq!(suffix.wire.mac_hex, source.wire.mac_hex);
    assert_eq!(
        rows.receptions
            .iter()
            .filter(|r| r.device == "Net.d" && r.status == "received")
            .count(),
        1
    );
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    assert_eq!(
        records
            .values()
            .filter(|r| r.schema.name == "dir.redundancy.recovery")
            .count(),
        2
    );
    assert_eq!(
        records
            .values()
            .filter(|r| r.schema.name == "dir.redundancy.recovery"
                && r.data["continuation_created"] == true)
            .count(),
        1
    );
    let mut rejected_extra = prepared.clone();
    let network = std::sync::Arc::make_mut(
        rejected_extra
            .registered
            .as_mut()
            .unwrap()
            .network
            .as_mut()
            .unwrap(),
    );
    let extra = network
        .provider_l2
        .as_mut()
        .unwrap()
        .ports
        .get_mut("Net.b.tx_x")
        .unwrap();
    extra.pvid = 101;
    extra.member_vids = BTreeSet::from([101]);
    let rejected = crate::runtime::simulate(&rejected_extra).unwrap();
    assert_ne!(rejected.common.termination, "execution_failed");
    assert_eq!(
        rejected
            .ethernet
            .as_ref()
            .unwrap()
            .receptions
            .iter()
            .filter(|r| r.device == "Net.d" && r.status == "received")
            .count(),
        1,
        "an extra receiver's provider ingress drop cannot retire the intended branch"
    );
    let mut model: Value =
        serde_json::from_slice(&std::fs::read(directory.join("model.json")).unwrap()).unwrap();
    model["replication"]["streams"][0]["egress_paths"][0]["ports"] =
        json!(["Net.r.tx_a", "Net.h.tx_a", "Net.a.tx_e"]);
    std::fs::write(
        directory.join("model.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    assert!(
        crate::prepare(&directory.join("model.ini"))
            .unwrap_err()
            .message
            .contains("physical junction")
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn active_multicast_and_explicit_wire_host_coexist_with_separate_unicast_frer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = root.join("examples/network-composite/active-multicast");
    let directory = std::env::temp_dir().join(format!(
        "dir-active-frer-wire-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let mut model: Value =
        serde_json::from_slice(&std::fs::read(source.join("model.json")).unwrap()).unwrap();
    let replication: Value = serde_json::from_slice(
        &std::fs::read(root.join("examples/network-composite/model.json")).unwrap(),
    )
    .unwrap();
    model["replication"] = replication["replication"].clone();
    model["multicast_active"]["interfaces"]
        .as_array_mut()
        .unwrap()
        .retain(|interface| interface["id"].as_str().unwrap().ends_with('4'));
    model["multicast_active"]["changes"]
        .as_array_mut()
        .unwrap()
        .retain(|change| change["at_ps"] == "0" && change["interface"] == "receiver4");
    model["wire_control"] = json!({"port_identities":[], "adapters":[{"id":"independent-mld", "protocol":"mld",
        "protocol_revision":"mld-rfc9777-2025","ports":["Net.s1.rx_a"],"role":"passive_conservative_snooper_v1",
        "max_pdu_bytes":1500,"max_sources_per_record":16,"max_groups":16}], "packets":[]});
    let mut workload: Value =
        serde_json::from_slice(&std::fs::read(source.join("workload.json")).unwrap()).unwrap();
    workload["generators"]
        .as_array_mut()
        .unwrap()
        .retain(|g| g["id"] == "ipv4");
    workload["generators"][0]["times_ps"] = json!(["5000000"]);
    let unicast: Value = serde_json::from_slice(
        &std::fs::read(root.join("examples/frer-rtag/distributed/workload.json")).unwrap(),
    )
    .unwrap();
    let mut unicast = unicast["generators"][0].clone();
    unicast["node"] = json!("Net.a");
    workload["generators"].as_array_mut().unwrap().push(unicast);
    std::fs::write(
        directory.join("model.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    let ini = format!(
        "[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 10000000ps\n",
        root.join("examples/ethernet/dynamic/models").display()
    );
    std::fs::write(directory.join("model.ini"), ini).unwrap();
    let mut observed = Vec::new();
    for loss in [false, true] {
        workload["controls"] = if loss {
            json!([{"id":"report-loss", "at_ps":"0", "kind":"link_set","link":"B", "up":false}])
        } else {
            json!([])
        };
        std::fs::write(
            directory.join("workload.json"),
            serde_json::to_vec_pretty(&workload).unwrap(),
        )
        .unwrap();
        let prepared = crate::prepare(&directory.join("model.ini")).unwrap();
        let wire = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .wire
            .as_ref()
            .unwrap();
        assert!(wire.adapters.iter().any(|a| a.id == "independent-mld"));
        assert!(
            wire.adapters
                .iter()
                .any(|a| a.protocol == crate::types::wire::WireProtocol::Igmp)
        );
        let snapshot = crate::runtime::simulate(&prepared).unwrap();
        assert_ne!(snapshot.common.termination, "execution_failed");
        let records = &snapshot.registered.as_ref().unwrap().model_records;
        assert!(
            records
                .values()
                .any(|r| r.schema.name == "dir.redundancy.origin")
        );
        let accepted = records.values().any(|r| {
            r.schema.name == "ethernet.wire.membership"
                && r.data["message_kind"] == "report"
                && r.data["outcome"] == "accepted"
                && r.data["ingress_port"] == "Net.s1.tx_b"
        });
        let delivered = snapshot
            .ethernet
            .as_ref()
            .unwrap()
            .receptions
            .iter()
            .any(|r| {
                r.device == "Net.b" && r.status == "received" && r.frame_id.starts_with("ipv4:")
            });
        observed.push((accepted, delivered));
    }
    assert_eq!(
        observed,
        vec![(true, true), (false, false)],
        "lost actual reports must not create memberships"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn provider_fanout_atomicity_and_checkpoint_byte_receiver_proofs() {
    use crate::runtime::session::{
        CheckpointLimits, ControlCommand, RunOptions, RunSession, RunState,
    };
    for cut in ["prefix", "recovered"] {
        let (prepared, directory) = fixture();
        let (mut session, controller) = RunSession::new(
            prepared.clone(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            assert_eq!(session.advance_one().unwrap(), RunState::Running);
            let snapshot = session.observe().unwrap();
            let records = &snapshot.registered.as_ref().unwrap().model_records;
            let ready = if cut == "prefix" {
                snapshot
                    .ethernet
                    .as_ref()
                    .unwrap()
                    .receptions
                    .iter()
                    .any(|r| {
                        r.ingress == "Net.r.rx_s"
                            && r.status == "processing"
                            && r.planned_ready_ps.is_some_and(|ready| {
                                snapshot.common.last_event_time_ps.unwrap_or(0) >= ready
                            })
                    })
            } else {
                records.values().any(|r| {
                    r.schema.name == "dir.redundancy.recovery"
                        && r.data["continuation_created"] == true
                })
            };
            if ready {
                break;
            }
        }
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let checkpoint_path = directory.join("checkpoint");
        session
            .checkpoint(&checkpoint_path, CheckpointLimits::default())
            .unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(checkpoint_path.join("state.json")).unwrap())
                .unwrap();
        let horizon = session.snapshot().common.last_event_time_ps.unwrap();
        let network = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .clone();
        let state = NetworkState::restore_checkpoint(
            network.clone(),
            &saved["network_backend"]["state"],
            horizon,
        )
        .unwrap();
        if cut == "prefix" {
            let selected = saved["fes"].as_array().unwrap().iter().find_map(|entry| {
                let envelope = &entry["work"]["envelope"];
                let payload: Vec<u8> = serde_json::from_value(envelope["payload"].clone()).ok()?;
                let event: Event = serde_json::from_slice(&payload).ok()?;
                matches!(event, Event::Complete(ri) if state.receptions[ri].row.ingress == "Net.r.rx_s")
                    .then(|| (envelope, payload))
            }).unwrap();
            let envelope = Envelope {
                event_id: selected.0["event_id"].as_str().unwrap().parse().unwrap(),
                source: "@network".into(),
                destination: "@network".into(),
                request_id: None,
                time_ps: selected.0["time_ps"].as_str().unwrap().parse().unwrap(),
                input_port: None,
                schema: Schema::new(selected.0["schema"]["name"].as_str().unwrap(), 1),
                payload: selected.1,
            };
            let records: Vec<(String, crate::registry::ModelRecord)> =
                checkpoint::decode(saved["network_backend"]["records"].clone()).unwrap();
            let before = state.checkpoint();
            crate::allocation::fail_next_reservation("network_fanout_second_branch");
            let (after, effects) = crate::runtime::registered::vector_network_failed_callback(
                prepared,
                state,
                records.into_iter().map(|(_, record)| record).collect(),
                vec![envelope],
            );
            assert!(
                crate::allocation::reservation_checkpoint("network_fanout_second_branch").is_ok()
            );
            assert_eq!(
                after.checkpoint(),
                before,
                "provider FDB, queues, sequence and timers must roll back together"
            );
            assert_eq!(effects["committed_effects"], 0);
        } else {
            let mut mutated = state.checkpoint();
            let extra = mutated["core"]["receptions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|rx| rx["row"]["reason"] == "replication_unintended_receiver")
                .unwrap();
            extra["row"]["status"] = json!("received");
            extra["row"]["reason"] = Value::Null;
            assert!(NetworkState::restore_checkpoint(network.clone(), &mutated, horizon).is_err());
            let mut mutated = state.checkpoint();
            let transfer = mutated["core"]["transfers"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|t| t["row"]["from_port"] == "Net.a.tx_e")
                .unwrap();
            let wire: EthernetWireFrame =
                serde_json::from_value(transfer["row"]["wire"].clone()).unwrap();
            let mut tags = wire.canonical_tags().unwrap();
            tags[0].tci.pcp = 1;
            transfer["row"]["wire"] = serde_json::to_value(
                crate::runtime::ethernet::retag_wire_frame(&wire, &tags).unwrap(),
            )
            .unwrap();
            transfer["row"]["priority"] = json!(1);
            assert!(NetworkState::restore_checkpoint(network, &mutated, horizon).is_err());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn legacy_endpoint_frer_preserves_provider_stack_and_ignores_extra_mac() {
    let (prepared, directory) = fixture();
    drop(prepared);
    let mut model: Value =
        serde_json::from_slice(&std::fs::read(directory.join("model.json")).unwrap()).unwrap();
    let stream = model["replication"]["streams"][0].as_object_mut().unwrap();
    stream.remove("placement");
    stream.insert("replicate_at".into(), json!("Net.s"));
    stream.insert("eliminate_at".into(), json!("Net.d"));
    stream.insert(
        "egress_paths".into(),
        json!([
        {"id":"a","ports":["Net.s.tx","Net.r.tx_a","Net.a.tx_e","Net.e.tx_d"]},
        {"id":"b","ports":["Net.s.tx","Net.r.tx_b","Net.b.tx_e","Net.e.tx_d"]}]),
    );
    std::fs::write(
        directory.join("model.json"),
        serde_json::to_vec_pretty(&model).unwrap(),
    )
    .unwrap();
    let prepared = crate::prepare(&directory.join("model.ini")).unwrap();
    let snapshot = crate::runtime::simulate(&prepared).unwrap();
    assert_ne!(snapshot.common.termination, "execution_failed");
    let rows = snapshot.ethernet.as_ref().unwrap();
    assert!(rows.receptions.iter().any(|r| r.ingress == "Net.b.rx_x"
        && r.reason.as_deref() == Some("replication_unintended_receiver")));
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    assert_eq!(
        records
            .values()
            .filter(|r| r.schema.name == "dir.redundancy.recovery" && r.data["delivered"] == true)
            .count(),
        1
    );
    let branches = rows
        .transfers
        .iter()
        .filter(|t| t.from_port == "Net.a.tx_e" || t.from_port == "Net.b.tx_e")
        .collect::<Vec<_>>();
    assert_eq!(branches.len(), 2);
    for transfer in branches {
        assert_eq!(
            &transfer.wire.mac_hex[24..56],
            "88a800648100000af1c1000000000800"
        );
        assert_eq!(frer_wire::receive(&transfer.wire).unwrap(), 0);
    }
    std::fs::remove_dir_all(directory).unwrap();
}
