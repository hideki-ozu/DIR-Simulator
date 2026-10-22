use super::*;

fn prepared_mstp() -> PreparedWire {
    let mut p = super::tests::prepared(&[WireProtocol::Rstp]);
    let mut map = vec![0u16; 4096];
    map[10] = 1;
    for adapter in &mut p.adapters {
        let WireOptions::Rstp(r) = &adapter.options else {
            unreachable!()
        };
        adapter.options = WireOptions::Mstp(MstpConfig {
            bridge_id: r.bridge_id,
            max_age: r.max_age,
            forward_delay: r.forward_delay,
            tx_hold_count: r.tx_hold_count,
            max_hops: 20,
            region_name: "region".into(),
            region_revision: 1,
            vlan_map: map.clone(),
            instances: vec![MstpInstanceConfig {
                mstid: 1,
                bridge_priority: 32768,
            }],
            ports: r
                .ports
                .iter()
                .map(|port| MstpPortConfig {
                    port: port.port.clone(),
                    port_id: port.port_id,
                    external_path_cost: port.external_path_cost,
                    internal_path_cost: port.external_path_cost,
                    admin_edge: port.admin_edge,
                })
                .collect(),
        });
        adapter.protocol = WireProtocol::Mstp;
        adapter.id = format!("{}-Mstp", adapter.bridge);
    }
    p
}

#[test]
fn production_mstp_wire_adapter_receives_and_projects_per_vid() {
    let mut state = WireState::new(Arc::new(prepared_mstp()));
    let mut view = WireView::default();
    view.oper_point_to_point_mac.insert("a/p".into(), false);
    view.oper_point_to_point_mac.insert("b/p".into(), false);
    let mut d = state.plan(0, WireEvent::Initialize, &view).unwrap();
    assert!(d.flush.is_empty());
    d.complete_host_records(0, 1, 1, vec![], vec![]).unwrap();
    let tree_records: Vec<_> = d
        .records
        .iter()
        .filter(|record| record.kind == "ethernet.control.wire.tree_policy")
        .collect();
    assert_eq!(tree_records.len(), 4);
    let a_vlan = tree_records
        .iter()
        .find(|record| record.data["component_id"] == "a-Mstp" && record.data["mstid"] == 1)
        .unwrap();
    assert_eq!(a_vlan.data["mapped_vids"], serde_json::json!([10]));
    assert_eq!(a_vlan.data["policy"]["role"], "Designated");
    assert_eq!(a_vlan.data["boundary"], false);
    assert_eq!(a_vlan.data["region_revision"], 1);
    let a_packet = d.tx.iter().find(|t| t.port == "a/p").unwrap().bytes.clone();
    assert!(matches!(
        mstp::decode(&a_packet, 0x8001, 1500).unwrap(),
        mstp::Received::Mst(_)
    ));
    assert!(
        !d.next.mstp["a-Mstp"].ports["a/p"]
            .config
            .oper_point_to_point_mac
    );
    assert_eq!(
        d.tree_policies[&("a-Mstp".into(), 1, "a/p".into())].role,
        "Designated"
    );
    state.apply(d);
    let d = state
        .plan(
            1,
            WireEvent::Receive {
                protocol: WireProtocol::Mstp,
                ingress_port: "b/in".into(),
                packet_id: "received-a-bpdu".into(),
                bytes: a_packet,
                vid: None,
                destination_mac: None,
                external_injection: false,
            },
            &view,
        )
        .unwrap();
    assert_eq!(
        d.next.mstp["b-Mstp"].trees[&mstp::Mstid(0)]
            .root_port
            .as_deref(),
        Some("b/p")
    );
    assert_eq!(
        d.next.mstp["b-Mstp"].trees[&mstp::Mstid(1)]
            .root_port
            .as_deref(),
        Some("b/p")
    );
    assert!(!d.next.mstp["b-Mstp"].ports["b/p"].boundary);
    assert_eq!(
        d.tree_policies[&("b-Mstp".into(), 1, "b/p".into())].role,
        "Root"
    );
    state.apply(d);
    assert_eq!(state.tree_policy("b/p", 10).unwrap().role, "Root");
    assert!(state.validate_checkpoint(1).is_ok());
    let snapshot = state.runtime_snapshot();
    assert!(snapshot["mstp"]["b-Mstp"]["ports"]["b/p"]["neighbors"].is_array());
    let frozen = state.checkpoint_fields();
    let restored =
        WireState::restore_fields(&frozen, WireState::new(state.prepared.clone())).unwrap();
    assert_eq!(restored.runtime_snapshot(), snapshot);
}

#[test]
fn checkpoint_accepts_received_regional_hops_above_local_max_hops() {
    let mut state = WireState::new(Arc::new(prepared_mstp()));
    let view = WireView::default();
    let initialized = state.plan(0, WireEvent::Initialize, &view).unwrap();
    let a_packet = initialized
        .tx
        .iter()
        .find(|tx| tx.port == "a/p")
        .unwrap()
        .bytes
        .clone();
    state.apply(initialized);
    let mstp::Received::Mst(mut bpdu) = mstp::decode(&a_packet, 0x8001, 1500).unwrap() else {
        panic!("expected MSTP BPDU")
    };
    bpdu.remaining_hops = 40;
    let bytes = mstp::encode([2, 0, 0, 0, 0, 9], &bpdu).unwrap();
    state.apply(
        state
            .plan(
                1,
                WireEvent::Receive {
                    protocol: WireProtocol::Mstp,
                    ingress_port: "b/in".into(),
                    packet_id: "received-large-hop-budget".into(),
                    bytes,
                    vid: None,
                    destination_mac: None,
                    external_injection: false,
                },
                &view,
            )
            .unwrap(),
    );
    assert_eq!(state.mstp["b-Mstp"].root_hops, 39);
    state.validate_checkpoint(1).unwrap();
    let frozen = state.checkpoint_fields();
    WireState::restore_fields(&frozen, WireState::new(state.prepared.clone())).unwrap();
}

#[test]
fn foreign_msti_record_is_ignored_before_checkpoint() {
    let mut state = WireState::new(Arc::new(prepared_mstp()));
    let view = WireView::default();
    let initialized = state.plan(0, WireEvent::Initialize, &view).unwrap();
    let a_packet = initialized
        .tx
        .iter()
        .find(|tx| tx.port == "a/p")
        .unwrap()
        .bytes
        .clone();
    state.apply(initialized);
    let mstp::Received::Mst(mut bpdu) = mstp::decode(&a_packet, 0x8001, 1500).unwrap() else {
        panic!("expected MSTP BPDU")
    };
    bpdu.region.revision += 1;
    bpdu.msti.push(mstp::MstiMessage {
        mstid: mstp::Mstid(2),
        flags: 0x0c,
        priority: mstp::MstiPriority {
            regional_root: 0x8002_0011_2233_4455,
            internal_cost: 0,
            designated_bridge: 0x8002_0011_2233_4455,
            designated_port: 0x8001,
            receiving_port: 0x8001,
        },
        remaining_hops: 19,
    });
    let bytes = mstp::encode([2, 0, 0, 0, 0, 9], &bpdu).unwrap();
    state.apply(
        state
            .plan(
                1,
                WireEvent::Receive {
                    protocol: WireProtocol::Mstp,
                    ingress_port: "b/in".into(),
                    packet_id: "foreign-extra-msti".into(),
                    bytes,
                    vid: None,
                    destination_mac: None,
                    external_injection: false,
                },
                &view,
            )
            .unwrap(),
    );
    assert!(
        state.mstp["b-Mstp"].ports["b/p"]
            .neighbors
            .values()
            .all(|n| n.msti.is_empty())
    );
    state.validate_checkpoint(1).unwrap();
    let frozen = state.checkpoint_fields();
    WireState::restore_fields(&frozen, WireState::new(state.prepared.clone())).unwrap();
}

#[test]
fn rst_neighbor_is_cist_boundary_in_production_adapter() {
    let mut state = WireState::new(Arc::new(prepared_mstp()));
    let view = WireView::default();
    state.apply(state.plan(0, WireEvent::Initialize, &view).unwrap());
    let legacy = rstp::Bpdu {
        kind: rstp::Kind::Rst,
        version: 2,
        flags: 0x0c,
        priority: rstp::Priority([
            0x1000_0011_2233_4444,
            0,
            0x1000_0011_2233_4444,
            0,
            0x1000_0011_2233_4444,
            0x8001,
            0x8001,
        ]),
        times: rstp::Times::default(),
    };
    let bytes = rstp::encode([2, 0, 0, 0, 0, 9], &legacy);
    let d = state
        .plan(
            1,
            WireEvent::Receive {
                protocol: WireProtocol::Mstp,
                ingress_port: "b/in".into(),
                packet_id: "legacy-rst".into(),
                bytes,
                vid: None,
                destination_mac: None,
                external_injection: false,
            },
            &view,
        )
        .unwrap();
    assert!(d.next.mstp["b-Mstp"].ports["b/p"].boundary);
    assert_eq!(
        d.next.mstp["b-Mstp"].trees[&mstp::Mstid(1)].ports["b/p"].role,
        mstp::Role::Master
    );
}
