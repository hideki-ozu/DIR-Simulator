use super::*;
use crate::runtime::wire::vector_manifest as manifest;
fn order_case() -> (
    crate::PreparedSimulation,
    NetworkState,
    crate::registry::Envelope,
) {
    let p = crate::prepare(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet-wire/active.ini"),
    )
    .unwrap();
    let registered = p.registered.as_ref().unwrap();
    let mut network = registered.network.as_ref().unwrap().as_ref().clone();
    let wire = network.wire.as_mut().unwrap();
    let a = wire
        .adapters
        .iter()
        .find(|a| a.protocol == WireProtocol::Rstp)
        .unwrap()
        .clone();
    let target = a.ports.last().unwrap().clone();
    let port = wire.ports[&target].clone();
    let msg = crate::runtime::wire::rstp::Bpdu {
        kind: crate::runtime::wire::rstp::Kind::Rst,
        version: 2,
        flags: 8 | 64,
        priority: crate::runtime::wire::rstp::Priority([
            u64::MAX,
            0,
            u64::MAX,
            0,
            u64::MAX,
            0x8001,
            0x8001,
        ]),
        times: Default::default(),
    };
    let bytes = crate::runtime::wire::rstp::encode([2, 0, 0, 0, 0, 0xfe], &msg);
    let index = wire.packets.len();
    wire.packets.push(crate::types::wire::PreparedWirePacket {
        id: "map-order".into(),
        adapter_id: a.id.clone(),
        ingress_port: port.ingress.clone(),
        output_port: target.clone(),
        arrival_ps: 1,
        bytes,
        vid: None,
        source_mac: None,
        destination_mac: None,
        boundary: None,
    });
    let mrp = wire
        .adapters
        .iter()
        .find(|candidate| candidate.protocol == WireProtocol::Mvrp && candidate.bridge == a.bridge)
        .unwrap();
    let source = mrp.ports.iter().find(|p| p.as_str() != target).unwrap();
    let source_port = &wire.ports[source];
    wire.packets.push(crate::types::wire::PreparedWirePacket {
        id: "map-withdrawal".into(),
        adapter_id: mrp.id.clone(),
        ingress_port: source_port.ingress.clone(),
        output_port: source.clone(),
        arrival_ps: 2,
        bytes: crate::runtime::wire::mvrp::encode(
            [2, 0, 0, 0, 0, 0xfd],
            &[(100, crate::runtime::wire::mvrp::AttributeEvent::Lv)],
            false,
            1500,
        )
        .unwrap()
        .0,
        vid: None,
        source_mac: None,
        destination_mac: None,
        boundary: None,
    });
    let mut state = NetworkState::new(Arc::new(network)).unwrap();
    let mut batch = crate::registry::EffectBatch::default();
    let mut next = 0;
    let channels = BTreeMap::new();
    let live = std::collections::BTreeSet::new();
    let mut c = Context {
        now: 0,
        config: &registered.models[0],
        registry: &registered.registry,
        profile: registered.registry.profile(&p.common.profile).unwrap(),
        channels: &channels,
        channel_configs: &[],
        live_timers: &live,
        executing_timer: None,
        next_id: &mut next,
        batch: &mut batch,
    };
    let d = state.initialize(&mut c, p.common.time_limit_ps).unwrap();
    state.reserve(&d).unwrap();
    state.apply(d);
    let host = state.wire.as_mut().unwrap();
    host.protocol.vector_seed_order_case(&a.id, &target);
    host.integration_stages.clear();
    host.pending_policy_commit = false;
    let envelope = crate::registry::Envelope {
        event_id: 999,
        source: "@network".into(),
        destination: "@network".into(),
        request_id: None,
        time_ps: 1,
        input_port: None,
        schema: crate::registry::Schema::new("dir.network.event", 1),
        payload: serde_json::to_vec(&Event::Wire(HostEvent::Packet(index))).unwrap(),
    };
    (p, state, envelope)
}
#[test]
fn supplied_bridge_map_order_uses_actual_atomic_publication() {
    let (p, state, envelope) = order_case();
    let (state, publication) =
        crate::runtime::registered::vector_wire_successful_callback(p, state, envelope);
    assert_eq!(publication["committed_events"], 1);
    let stages = state.wire_vector_stages();
    let order: Vec<_> = stages
        .iter()
        .filter(|s| s.as_str() != "rstp_settled")
        .collect();
    let row = manifest::vectors("bridge-integration-v1")
        .into_iter()
        .find(|v| v["id"] == "P-CORE-T02-V201")
        .unwrap();
    manifest::finish(
        "bridge-integration-v1",
        &row,
        "runtime::network::wire::supplied_bridge_map_order_uses_actual_atomic_publication",
        json!({"order":order}),
    );
    let records = publication["records"].as_array().unwrap();
    assert!(records.iter().any(|r| {
        r[1]["schema"]["name"] == "ethernet.control.wire.event"
            && r[1]["data"]["effects"].as_array().is_some_and(|effects| {
                effects.iter().any(|e| {
                    e["kind"] == "mac_flush" && e["reason"] == "mvrp_new" && e["completed"] == true
                })
            })
    }));
    if let Ok(root) = std::env::var("WIRE_ORDER_EVIDENCE_DIR") {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            std::path::Path::new(&root).join("bridge-map-order.json"),
            serde_json::to_vec_pretty(
                &json!({"case":row["id"],"stages":stages,"publication":publication}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}
#[test]
fn supplied_membership_last_journal_reservation_is_atomic() {
    for v6 in [false, true] {
        let mut p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/ethernet-wire/passive.ini"),
        )
        .unwrap();
        p.common.time_limit_ps = 200_000_000;
        let mut registered = p.registered.as_ref().unwrap().clone();
        let mut network = registered.network.as_ref().unwrap().as_ref().clone();
        let wire = network.wire.as_mut().unwrap();
        let protocol = if v6 {
            WireProtocol::Mld
        } else {
            WireProtocol::Igmp
        };
        let adapter = wire
            .adapters
            .iter()
            .find(|a| a.protocol == protocol)
            .unwrap()
            .id
            .clone();
        let mut packet = wire
            .packets
            .iter()
            .find(|p| p.adapter_id == adapter)
            .unwrap()
            .clone();
        let source: std::net::IpAddr = if v6 { "2001:db8::1" } else { "192.0.2.1" }
            .parse()
            .unwrap();
        let second_source: std::net::IpAddr = if v6 { "2001:db8::2" } else { "192.0.2.2" }
            .parse()
            .unwrap();
        let mut bytes = crate::runtime::wire::tests::report_bytes(
            v6,
            1,
            &std::collections::BTreeSet::from([source]),
        );
        let second = crate::runtime::wire::tests::report_bytes(
            v6,
            2,
            &std::collections::BTreeSet::from([second_source]),
        );
        let off = if v6 { 48 } else { 24 };
        bytes[off + 6..off + 8].copy_from_slice(&2u16.to_be_bytes());
        bytes.extend(&second[off + 8..]);
        crate::runtime::wire::tests::repair_ip(&mut bytes, v6);
        packet.bytes = bytes;
        packet.id = "atomic-report".into();
        packet.arrival_ps = 100_000_000;
        let old = packet.boundary.as_ref().unwrap();
        let mac_bytes = tagged_ip(
            packet.source_mac.unwrap(),
            packet.destination_mac.unwrap(),
            packet.vid.unwrap(),
            protocol,
            &packet.bytes,
        )
        .len() as u64;
        let duration =
            crate::input::wire::vector_serialization(mac_bytes + 8, old.bitrate_bps).unwrap();
        let ifg = crate::input::wire::vector_serialization(12, old.bitrate_bps).unwrap();
        packet.boundary = Some(crate::types::wire::BoundaryPlan {
            start_ps: packet.arrival_ps - duration,
            arrival_ps: packet.arrival_ps,
            release_ps: packet.arrival_ps + ifg,
            mac_bytes,
            bitrate_bps: old.bitrate_bps,
        });
        let start = packet.boundary.as_ref().unwrap().start_ps;
        wire.packets = vec![packet.clone()];
        registered.network = Some(Arc::new(network.clone()));
        p.registered = Some(registered.clone());
        let mut state = NetworkState::new(Arc::new(network)).unwrap();
        let mut batch = crate::registry::EffectBatch::default();
        let mut next = 0;
        let channels = BTreeMap::new();
        let live = std::collections::BTreeSet::new();
        let mut c = Context {
            now: 0,
            config: &registered.models[0],
            registry: &registered.registry,
            profile: registered.registry.profile(&p.common.profile).unwrap(),
            channels: &channels,
            channel_configs: &[],
            live_timers: &live,
            executing_timer: None,
            next_id: &mut next,
            batch: &mut batch,
        };
        let d = state.initialize(&mut c, p.common.time_limit_ps).unwrap();
        state.reserve(&d).unwrap();
        state.apply(d);
        let envelope = |at, which, id| crate::registry::Envelope {
            event_id: id,
            source: "@network".into(),
            destination: "@network".into(),
            request_id: None,
            time_ps: at,
            input_port: None,
            schema: crate::registry::Schema::new("dir.network.event", 1),
            payload: serde_json::to_vec(&Event::Wire(which)).unwrap(),
        };
        let (state, prefix) = crate::runtime::registered::vector_wire_successful_callback(
            p.clone(),
            state,
            envelope(start, HostEvent::BoundaryStart(0), 900),
        );
        let before = state.wire_snapshot(packet.arrival_ps).unwrap();
        let dynamic = state.dynamic.as_ref().unwrap().snapshot_tables();
        let epoch = state.dynamic.as_ref().unwrap().policy_epoch();
        let prefix_records: Vec<crate::registry::ModelRecord> = prefix["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| serde_json::from_value(r[1].clone()).unwrap())
            .collect();
        let rows_before = prefix_records.len();
        // This fails the final journal merge reservation after all protocol, lease,
        // forwarding and audit effects have been planned, in the actual host preflight.
        crate::allocation::fail_next_reservation("commit_record_merge");
        let (after, evidence) = crate::runtime::registered::vector_network_failed_callback(
            p,
            state,
            prefix_records,
            vec![envelope(packet.arrival_ps, HostEvent::Packet(0), 901)],
        );
        assert_eq!(after.wire_snapshot(packet.arrival_ps).unwrap(), before);
        assert_eq!(after.dynamic.as_ref().unwrap().snapshot_tables(), dynamic);
        let row = manifest::vectors("membership-wire-v1")
            .into_iter()
            .find(|r| {
                r["id"]
                    == if v6 {
                        "P-MLD-T01-V16"
                    } else {
                        "P-IGMP-T01-V16"
                    }
            })
            .unwrap();
        let after_snapshot = after.wire_snapshot(packet.arrival_ps).unwrap().unwrap();
        let before_snapshot = before.unwrap();
        manifest::finish(
            "membership-wire-v1",
            &row,
            "runtime::network::wire::supplied_membership_last_journal_reservation_is_atomic",
            json!({"callback":if evidence["run_state"]=="Failed"{"failed"}else{"unexpected"},"membership_changes":after_snapshot["protocol"]["leases"].as_array().unwrap().len()-before_snapshot["protocol"]["leases"].as_array().unwrap().len(),"router_changes":after_snapshot["protocol"]["routers"].as_array().unwrap().len()-before_snapshot["protocol"]["routers"].as_array().unwrap().len(),"epoch_increment":after.dynamic.as_ref().unwrap().policy_epoch()-epoch,"new_audits":evidence["journal_rows"].as_u64().unwrap()-rows_before as u64,"new_copies":after_snapshot["tx"].as_array().unwrap().len()-before_snapshot["tx"].as_array().unwrap().len(),"success_prefix_preserved":after_snapshot==before_snapshot}),
        );
    }
}

#[test]
fn tree_batch_late_preflight_failure_publishes_no_stages_or_policy() {
    let (p, state, envelope) = order_case();
    let before = state.wire_snapshot(envelope.time_ps).unwrap();
    let dynamic = state.dynamic.as_ref().unwrap().snapshot_tables();
    assert!(state.wire_vector_stages().is_empty());
    crate::allocation::fail_next_reservation("commit_effects");
    let (after, evidence) = crate::runtime::registered::vector_network_failed_callback(
        p,
        state,
        vec![],
        vec![envelope],
    );
    assert_eq!(after.wire_snapshot(1).unwrap(), before);
    assert_eq!(after.dynamic.as_ref().unwrap().snapshot_tables(), dynamic);
    assert!(after.wire_vector_stages().is_empty());
    assert_eq!(evidence["committed_effects"], 0);
    assert_eq!(evidence["journal_rows"], 0);
    assert_eq!(evidence["fes_pending"], 1);
}

#[test]
fn actual_host_map_new_and_withdrawal_preserve_demand_changes() {
    let (p, state, envelope) = order_case();
    let (state, new) =
        crate::runtime::registered::vector_wire_successful_callback(p.clone(), state, envelope);
    let rows = new["records"].as_array().unwrap();
    assert!(
        rows.iter().any(
            |r| r[1]["schema"]["name"] == "ethernet.control.wire.transition"
                && r[1]["data"]["machine"] == "Applicant"
                && r[1]["data"]["changes"]["attribute"]["after"] == "100"
                && r[1]["data"]["changes"]["demand"] == json!({"before":false,"after":true})
        ),
        "actual New callback lost demand change"
    );
    let index = state
        .prepared
        .wire
        .as_ref()
        .unwrap()
        .packets
        .iter()
        .position(|packet| packet.id == "map-withdrawal")
        .unwrap();
    let envelope = crate::registry::Envelope {
        event_id: 1000,
        source: "@network".into(),
        destination: "@network".into(),
        request_id: None,
        time_ps: 2,
        input_port: None,
        schema: crate::registry::Schema::new("dir.network.event", 1),
        payload: serde_json::to_vec(&Event::Wire(HostEvent::Packet(index))).unwrap(),
    };
    let (_, withdrawal) =
        crate::runtime::registered::vector_wire_successful_callback(p, state, envelope);
    assert!(
        withdrawal["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |r| r[1]["schema"]["name"] == "ethernet.control.wire.transition"
                    && r[1]["data"]["machine"] == "Applicant"
                    && r[1]["data"]["changes"]["attribute"]["after"] == "100"
                    && r[1]["data"]["changes"]["demand"] == json!({"before":true,"after":false})
            ),
        "actual withdrawal callback lost demand change"
    );
}
