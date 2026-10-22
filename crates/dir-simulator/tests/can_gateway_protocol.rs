#[path = "support/can_compound.rs"]
mod support;
use serde_json::{Value, json};
use support::*;
fn setup(protocol: &str) -> Value {
    let mut c = config();
    c["buses"] = json!([bus("in"), bus("out")]);
    c["gateways"] = json!([{"id":"gw","ports":[{"bus":"in","controller":"b"},{"bus":"out","controller":"a"}],"ingress_capacity":4,"service_ps":5000000,"version":1,"rules":[rule("r","out",protocol,Some(0x100))]}]);
    c
}
fn rule(id: &str, bus: &str, protocol: &str, rewrite: Option<u32>) -> Value {
    json!({"id":id,"source_bus":"in","id_mask":0,"id_value":0,"destination_bus":bus,"controller":"a","protocol":protocol,"rewrite_id":rewrite,"brs":true})
}
fn source(frame: Value) -> Value {
    let mut w = workload(frame);
    w["generators"][0]["bus"] = json!("in");
    w
}
#[test]
fn rewritten_priority_and_crc_are_actual_downstream_bus_execution() {
    let mut c = setup("cc");
    c["buses"][1]["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c","capability":"fd_capable"}));
    let source_wire = dir_simulator::runtime::can_protocol::codec::encode(
        &dir_simulator::runtime::can_protocol::types::Frame {
            id: 0x500,
            extended: false,
            remote: false,
            dlc: 2,
            data: vec![18, 52],
        },
    )
    .unwrap();
    let native_time = (source_wire.bits.len() as u64 - 2) * 2_000_000 + 1_500_000 + 5_000_000;
    let mut w = source(cc(0x500));
    w["generators"].as_array_mut().unwrap().push(json!({"id":"native","bus":"out","controller":"c","frame":cc(0x200),"schedule":{"kind":"explicit.v1","times_ps":[native_time]}}));
    let s = Fixture::new(c, w, "can.gateway.protocol.v1", "2ms").run();
    let branches = rows(&s, "gateway_branch");
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0]["frame"]["id"], 0x100);
    assert_eq!(branches[0]["origin_request_id"], "g:0");
    assert_eq!(branches[0]["parent_request_id"], "g:0");
    let id = branches[0]["request_id"].as_str().unwrap();
    let attempts = rows(&s, "attempt");
    let outgoing = attempts
        .iter()
        .find(|a| a["request_id"] == id && a["state"] == "completed")
        .unwrap();
    assert_eq!(outgoing["sof_ps"], native_time.to_string());
    assert!(
        attempts
            .iter()
            .any(|a| a["request_id"] == "native:0" && a["state"] == "arbitration_lost")
    );
    let expected = dir_simulator::runtime::can_protocol::codec::encode(
        &dir_simulator::runtime::can_protocol::types::Frame {
            id: 0x100,
            extended: false,
            remote: false,
            dlc: 2,
            data: vec![18, 52],
        },
    )
    .unwrap();
    assert_eq!(outgoing["crc"], expected.crc);
    assert_eq!(outgoing["wire_bits"], json!(expected.bits));
}
#[test]
fn same_time_route_update_wins_before_rx_and_queued_ingress_keeps_that_version() {
    let mut c = setup("cc");
    c["route_updates"] = json!([
        {"gateway":"gw","at_ps":119500000,"version":2,"rules":[rule("new","out","cc",Some(0x50))]},
        {"gateway":"gw","at_ps":120000000,"version":3,"rules":[rule("later","out","cc",Some(0x60))]}
    ]);
    let s = Fixture::new(c, source(cc(0x123)), "can.gateway.protocol.v1", "2ms").run();
    let branches = rows(&s, "gateway_branch");
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0]["route_version"], 2);
    assert_eq!(branches[0]["frame"]["id"], 0x50);
    let ingress = rows(&s, "gateway_ingress");
    assert_eq!(ingress[0]["received_ps"], "119500000");
    assert_eq!(ingress[0]["route_version"], 2);
}
#[test]
fn independent_branch_overflow_preserves_successful_sibling_and_exact_conversion() {
    let mut c = setup("cc");
    c["buses"].as_array_mut().unwrap().push(bus("good"));
    c["buses"][1]["controllers"][0]["pending_capacity"] = json!(0);
    c["gateways"][0]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!({"bus":"good","controller":"a"}));
    c["gateways"][0]["rules"] = json!([
        rule("bad", "out", "cc", None),
        rule("good", "good", "iso_fd", None)
    ]);
    let s = Fixture::new(c, source(cc(0x123)), "can.gateway.protocol.v1", "2ms").run();
    let branches = rows(&s, "gateway_branch");
    assert_eq!(branches.len(), 2);
    assert!(branches.iter().any(|b| b["state"] == "egress_drop"));
    let successful = branches.iter().find(|b| b["state"] == "queued").unwrap();
    let id = successful["request_id"].as_str().unwrap();
    assert_eq!(successful["frame"]["data"], json!([18, 52]));
    assert_eq!(successful["frame"]["protocol"], "iso_fd");
    assert!(
        rows(&s, "request")
            .iter()
            .any(|r| r["request_id"] == id && r["state"] == "completed")
    );
}
#[test]
fn oversized_fd_and_rtr_conversions_reject_without_any_downstream_attempt() {
    let mut remote = cc(0x123);
    remote["remote"] = json!(true);
    remote["data"] = json!([]);
    for (protocol, frame) in [("cc", fd(0x123, true, 64)), ("iso_fd", remote)] {
        let s = Fixture::new(
            setup(protocol),
            source(frame),
            "can.gateway.protocol.v1",
            "2ms",
        )
        .run();
        let branch = rows(&s, "gateway_branch");
        assert_eq!(branch.len(), 1);
        assert_eq!(branch[0]["state"], "conversion_rejected");
        assert_eq!(rows(&s, "attempt").len(), 1);
    }
    let s = Fixture::new(
        setup("cc"),
        source(fd(0x123, true, 2)),
        "can.gateway.protocol.v1",
        "2ms",
    )
    .run();
    assert!(
        rows(&s, "gateway_branch")
            .iter()
            .any(|b| b["state"] == "queued")
    );
    assert_eq!(rows(&s, "attempt").len(), 2);
}
#[test]
fn hop_limit_terminates_real_two_gateway_loop() {
    let mut c = config();
    c["max_gateway_hops"] = json!(3);
    c["buses"] = json!([bus("in"), bus("out")]);
    c["buses"][0]["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"source","capability":"fd_capable"}));
    let mut back = rule("back", "in", "cc", None);
    back["source_bus"] = json!("out");
    back["controller"] = json!("b");
    c["gateways"] = json!([
        {"id":"ga","ports":[{"bus":"in","controller":"a"},{"bus":"out","controller":"a"}],"ingress_capacity":4,"service_ps":0,"version":1,"rules":[rule("forward","out","cc",None)]},
        {"id":"gb","ports":[{"bus":"in","controller":"b"},{"bus":"out","controller":"b"}],"ingress_capacity":4,"service_ps":0,"version":1,"rules":[back]}
    ]);
    let mut w = source(cc(0x123));
    w["generators"][0]["controller"] = json!("source");
    let s = Fixture::new(c, w, "can.gateway.protocol.v1", "2ms").run();
    assert!(
        rows(&s, "gateway_ingress")
            .iter()
            .any(|r| r["state"] == "hop_limit_drop")
    );
    assert_eq!(rows(&s, "gateway_branch").len(), 3);
    assert_eq!(rows(&s, "attempt").len(), 4);
}

#[test]
fn passive_transmitter_local_error_does_not_erase_actual_gateway_receive_ancestry() {
    let frame = dir_simulator::runtime::canfd::codec::Frame {
        id: 0x123,
        extended: false,
        brs: true,
        esi: true,
        data: vec![0x5a; 2],
    };
    let wire = dir_simulator::runtime::canfd::codec::encode(&frame).unwrap();
    let mut c = setup("cc");
    c["buses"][0]["controllers"][0]["tec"] = json!(128);
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":wire.crc_delimiter_index,"receiver":"a"}]);
    let s = Fixture::new(
        c,
        source(fd(0x123, true, 2)),
        "can.gateway.protocol.v1",
        "1ms",
    )
    .run();
    assert!(
        rows(&s, "error")
            .iter()
            .any(|r| r["detector"] == "a" && r["flag"] == "passive"),
        "{:?}",
        rows(&s, "error")
    );
    let reception = rows(&s, "reception")
        .into_iter()
        .find(|r| r["bus"] == "in")
        .unwrap();
    assert_eq!(reception["request_ids"], json!(["g:0"]));
    let branches = rows(&s, "gateway_branch");
    assert!(!branches.is_empty());
    assert!(
        branches
            .iter()
            .all(|r| r["origin_request_id"] == "g:0" && r["parent_request_id"] == "g:0")
    );
}

#[test]
fn checkpoint_before_route_replacement_and_queued_service_preserves_physical_branches() {
    use dir_simulator::runtime::session::*;
    let mut c = setup("iso_fd");
    c["gateways"][0]["service_ps"] = json!(100_000_000);
    c["route_updates"] = json!([{ "gateway":"gw","at_ps":119500000,"version":2,"rules":[rule("new","out","iso_fd",Some(0x50))] },{ "gateway":"gw","at_ps":150000000,"version":3,"rules":[rule("later","out","cc",Some(0x60))] }]);
    let f = Fixture::new(c, source(cc(0x123)), "can.gateway.protocol.v1", "2ms");
    let expected = f.run();
    for target in [118_000_000u64, 120_000_000, 151_000_000, 218_000_000] {
        let (mut session, controller) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..5000 {
            session.advance_one().unwrap();
            let registered = session.snapshot().registered.as_ref().unwrap();
            if registered
                .model_records
                .values()
                .any(|record| record.time_ps >= target)
            {
                break;
            }
        }
        controller.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        let path = f.0.join(format!("route-service-{target}"));
        let ack = session
            .checkpoint(&path, CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let (mut restored, token) =
            restore_checkpoint(&path, Fixture::registry(), CheckpointLimits::default()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(
            serde_json::to_value(
                &restored
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
            )
            .unwrap(),
            serde_json::to_value(&expected.model_records).unwrap()
        );
    }
}
