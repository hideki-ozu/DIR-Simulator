use super::*;
pub(super) fn prepared(protocols: &[WireProtocol]) -> PreparedWire {
    let mut ports = BTreeMap::new();
    let mut adapters = vec![];
    for (n, id) in [("a", 1u64), ("b", 2)] {
        let port = format!("{n}/p");
        ports.insert(
            port.clone(),
            WirePort {
                output: port.clone(),
                ingress: format!("{n}/in"),
                bridge: n.into(),
                pvid: 1,
                direction: (id - 1) as usize,
                bitrate_bps: 100_000_000,
                source_mac: Some([2, 0, 0, 0, 0, id as u8]),
                static_vlans: BTreeMap::new(),
                allowed_vlans: BTreeMap::from([(10, true)]),
            },
        );
        for protocol in protocols {
            let options = match protocol {
                WireProtocol::Rstp => WireOptions::Rstp(RstpConfig {
                    bridge_id: 0x8000000000000000 + id,
                    max_age: 5120,
                    forward_delay: 3840,
                    tx_hold_count: 6,
                    ports: vec![RstpPortConfig {
                        port: port.clone(),
                        port_id: 0x8001,
                        external_path_cost: 20000,
                        admin_edge: false,
                        auto_edge: true,
                        auto_isolate: false,
                        restricted_role: false,
                        restricted_tcn: false,
                    }],
                }),
                WireProtocol::Mvrp => WireOptions::Mvrp(MvrpConfig {
                    max_attributes_per_pdu: 4094,
                    ports: vec![MvrpPortConfig {
                        port: port.clone(),
                        join_time_ps: 200_000_000_000,
                        leave_time_ps: 600_000_000_000,
                        leaveall_time_ps: 0,
                        periodic_enabled: true,
                        applicant_control: WireApplicantMode::Normal,
                        restricted_vlan_registration: false,
                        transmit_zero: false,
                    }],
                    registrar_controls: BTreeMap::new(),
                    local_declarations: if n == "a" {
                        BTreeSet::from([(port.clone(), 10)])
                    } else {
                        BTreeSet::new()
                    },
                    random_seed: 0,
                }),
                _ => WireOptions::Membership(MembershipConfig {
                    max_sources_per_record: 4096,
                    max_groups: 128,
                    max_filter_leases: 128,
                    max_sources_total: 4096,
                    membership_interval_ps: 100,
                    router_interval_ps: 100,
                }),
            };
            adapters.push(PreparedWireAdapter {
                id: format!("{n}-{protocol:?}"),
                protocol: *protocol,
                bridge: n.into(),
                ports: vec![port.clone()],
                max_pdu_bytes: 1500,
                max_state_entries: 4094,
                max_pending_timers: 4097,
                max_pending_tx: 64,
                max_control_steps: 100_000,
                options,
            });
        }
    }
    PreparedWire {
        adapters,
        ports,
        packets: vec![],
        svre_exists: BTreeSet::from([("a".into(), 10), ("b".into(), 10)]),
        registrar_modes: BTreeMap::new(),
    }
}
#[test]
fn atomic_capacity_keeps_original() {
    let mut p = prepared(&[WireProtocol::Rstp]);
    p.adapters[0].max_control_steps = 1;
    let state = WireState::new(Arc::new(p));
    assert!(
        state
            .plan(0, WireEvent::Initialize, &WireView::default())
            .is_err()
    );
    assert!(!state.initialized);
    assert_eq!(state.callback, 0);
    assert!(state.timers.is_empty());
}
#[test]
fn malformed_receive_and_stale_timer_are_noop() {
    let p = Arc::new(prepared(&[WireProtocol::Rstp]));
    let mut s = WireState::new(p);
    let d = s
        .plan(0, WireEvent::Initialize, &WireView::default())
        .unwrap();
    let timer = d.timers[0].clone();
    s.apply(d);
    let before = s.rstp.clone();
    let d = s
        .plan(
            1,
            WireEvent::Receive {
                protocol: WireProtocol::Rstp,
                ingress_port: "a/in".into(),
                packet_id: "bad".into(),
                bytes: vec![0; 64],
                vid: None,
                destination_mac: None,
                external_injection: true,
            },
            &WireView::default(),
        )
        .unwrap();
    assert_eq!(d.next.rstp, before);
    assert!(d.tx.is_empty());
    assert!(!d.policy_changed);
    assert_eq!(d.records.last().unwrap().data["outcome"], "control_drop");
    let d = s
        .plan(
            2,
            WireEvent::Timer {
                adapter_id: timer.adapter_id,
                key: timer.key,
                generation: timer.generation + 1,
                deadline_ps: 2,
            },
            &WireView::default(),
        )
        .unwrap();
    assert_eq!(d.next.rstp, before);
    assert!(!d.policy_changed);
    assert!(d.tx.is_empty());
    assert_eq!(d.records.last().unwrap().data["outcome"], "stale");
}
#[test]
fn active_peer_exchange_converges_and_registers() {
    let mut s = WireState::new(Arc::new(prepared(&[
        WireProtocol::Rstp,
        WireProtocol::Mvrp,
    ])));
    let mut pending = vec![(0u64, 0u8, 0u64, WireEvent::Initialize)];
    let mut sequence = 0u64;
    let mut epoch = 0;
    let mut tx = 0;
    while !pending.is_empty() {
        pending.sort_by_key(|p| (p.0, p.1, p.2));
        let (now, _, _, event) = pending.remove(0);
        if now >= 4_000_000_000_000 {
            break;
        }
        let d = s
            .plan(
                now,
                event,
                &WireView {
                    policy_epoch: epoch,
                    ..Default::default()
                },
            )
            .unwrap();
        if d.policy_changed {
            epoch += 1;
        }
        for t in &d.timers {
            if let Some(at) = t.deadline_ps {
                sequence += 1;
                pending.push((
                    at,
                    0,
                    sequence,
                    WireEvent::Timer {
                        adapter_id: t.adapter_id.clone(),
                        key: t.key.clone(),
                        generation: t.generation,
                        deadline_ps: at,
                    },
                ));
            }
        }
        for t in &d.tx {
            tx += 1;
            let peer = if t.port == "a/p" { "b/in" } else { "a/in" };
            sequence += 1;
            pending.push((
                now + 10_000_000,
                1,
                sequence,
                WireEvent::Receive {
                    protocol: t.protocol,
                    ingress_port: peer.into(),
                    packet_id: t.packet_id.clone(),
                    bytes: t.bytes.clone(),
                    vid: None,
                    destination_mac: None,
                    external_injection: false,
                },
            ));
        }
        s.apply(d);
        assert!(sequence < 10000, "nonconvergent event generation");
    }
    assert!(tx > 4);
    assert_eq!(s.rstp["a-Rstp"].ports["a/p"].role, rstp::Role::Designated);
    assert_eq!(s.rstp["b-Rstp"].ports["b/p"].role, rstp::Role::Root);
    assert!(s.policies().values().all(|p| p.forwarding));
    assert!(s.registered().contains(&("b/p".into(), 10)));
    assert!(
        !s.registered().contains(&("a/p".into(), 10)),
        "local declaration is not a registration"
    );
}
#[test]
fn mvrp_local_leaveall_runs_real_tx() {
    let mut p = prepared(&[WireProtocol::Mvrp]);
    p.adapters.truncate(1);
    let a = &p.adapters[0];
    let mut state = mvrp_state::State::new(a);
    let mut d = state.plan(&p, a, 0, mvrp_state::Input::Initialize).unwrap();
    let port = d.state.ports.get_mut("a/p").unwrap();
    let attr = port.attributes.get_mut(&10).unwrap();
    attr.applicant = mvrp::Applicant::AA;
    attr.registrar = mvrp::Registrar::IN;
    attr.registered = true;
    port.leaveall_active = true;
    port.timers.insert(
        ("join".into(), None),
        mvrp_state::Timer {
            generation: 99,
            deadline: Some(1),
        },
    );
    state = d.state;
    let d = state
        .plan(
            &p,
            a,
            1,
            mvrp_state::Input::Timer {
                port: "a/p".into(),
                slot: "join".into(),
                vid: None,
                generation: 99,
            },
        )
        .unwrap();
    assert!(
        d.trace
            .iter()
            .any(|trace| trace.machine.as_deref() == Some("LeaveAll")
                && trace.event == "Tx"
                && trace.before["state"] == "Active"
                && trace.after["state"] == "Passive")
    );
    let attr = &d.state.ports["a/p"].attributes[&10];
    assert_eq!(attr.registrar, mvrp::Registrar::MT);
    assert!(!attr.registered);
    let msg = mvrp::decode(&d.tx[0].1, 1, 1500, 4094).unwrap();
    assert!(msg.vectors[0].leave_all);
    assert!(
        msg.vectors
            .iter()
            .any(|v| v.attributes.contains(&(10, mvrp::AttributeEvent::Mt)))
    );
    assert!(attr.request_tx);
}
#[test]
fn triangle_converges_without_a_central_tree() {
    let mut bridges: BTreeMap<String, rstp::Bridge> = (1..=3)
        .map(|id| {
            let ports = (1..=2)
                .map(|p| {
                    (
                        format!("p{p}"),
                        (
                            rstp::PortConfig {
                                id: 0x8000 + p,
                                cost: 20000,
                                admin_edge: false,
                                auto_edge: true,
                                auto_isolate: false,
                                restricted_role: false,
                                restricted_tcn: false,
                            },
                            true,
                        ),
                    )
                })
                .collect();
            (
                id.to_string(),
                rstp::Bridge::new(0x8000000000000000 + id, rstp::Times::default(), 6, ports),
            )
        })
        .collect();
    let peers = BTreeMap::from([
        (("1", "p1"), ("2", "p1")),
        (("2", "p1"), ("1", "p1")),
        (("1", "p2"), ("3", "p1")),
        (("3", "p1"), ("1", "p2")),
        (("2", "p2"), ("3", "p2")),
        (("3", "p2"), ("2", "p2")),
    ]);
    let mut pending: Vec<_> = (1..=3)
        .map(|n| (0u64, 0, n as u64, n.to_string(), rstp::Input::Initialize))
        .collect();
    let mut seq = 3;
    for tick in 1u64..=6 {
        for n in 1..=3 {
            seq += 1;
            pending.push((
                tick * 1_000_000_000_000,
                0,
                seq,
                n.to_string(),
                rstp::Input::Tick,
            ));
        }
    }
    let mut processed = 0;
    while !pending.is_empty() {
        pending.sort_by_key(|e| (e.0, e.1, e.2));
        let (now, _, _, name, input) = pending.remove(0);
        let d = bridges[&name].plan(input, 10000).unwrap();
        for (port, m) in d.tx {
            let (peer, ingress) = peers[&(name.as_str(), port.as_str())];
            seq += 1;
            pending.push((
                now + 1_000_000,
                1,
                seq,
                peer.into(),
                rstp::Input::Receive(ingress.into(), m),
            ));
        }
        bridges.insert(name, d.state);
        processed += 1;
        assert!(processed < 1000);
    }
    assert!(bridges.values().all(|b| b.root.0[0] == 0x8000000000000001));
    let ports: Vec<_> = bridges.values().flat_map(|b| b.ports.values()).collect();
    assert_eq!(
        ports.iter().filter(|p| p.role == rstp::Role::Root).count(),
        2
    );
    assert_eq!(
        ports
            .iter()
            .filter(|p| p.role == rstp::Role::Alternate)
            .count(),
        1
    );
    assert!(
        ports
            .iter()
            .filter(|p| p.role == rstp::Role::Alternate)
            .all(|p| !p.forwarding && !p.learning)
    );
    assert_eq!(ports.iter().filter(|p| p.forwarding).count(), 5);
}
#[test]
fn membership_families_share_one_wake_and_expire_before_refresh() {
    let mut state = WireState::new(Arc::new(prepared(&[WireProtocol::Igmp, WireProtocol::Mld])));
    let init = state
        .plan(0, WireEvent::Initialize, &WireView::default())
        .unwrap();
    state.apply(init);
    let sample: Value = serde_json::from_str(include_str!(
        "../../../../../examples/ethernet-wire/passive-workload.json"
    ))
    .unwrap();
    let mut wake = None;
    let mut report = None;
    for (i, protocol) in [WireProtocol::Igmp, WireProtocol::Mld]
        .into_iter()
        .enumerate()
    {
        let packet = &sample["packets"][i];
        let bytes = codec::unhex(packet["bytes_hex"].as_str().unwrap()).unwrap();
        let ev = WireEvent::Receive {
            protocol,
            ingress_port: "a/in".into(),
            packet_id: format!("report-{i}"),
            bytes,
            vid: Some(1),
            destination_mac: Some(codec::mac(packet["destination_mac"].as_str().unwrap()).unwrap()),
            external_injection: true,
        };
        if i == 0 {
            report = Some(ev.clone());
        }
        let d = state
            .plan(
                10,
                ev,
                &WireView {
                    policy_epoch: i as u64,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(d.policy_changed);
        if i == 0 {
            wake = Some(d.timers[0].clone());
        } else {
            assert!(d.timers.is_empty(), "same bridge deadline must coalesce");
        }
        state.apply(d);
    }
    let timer = wake.unwrap();
    let d = state
        .plan(
            110,
            WireEvent::Timer {
                adapter_id: timer.adapter_id,
                key: timer.key,
                generation: timer.generation,
                deadline_ps: 110,
            },
            &WireView {
                policy_epoch: 2,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(d.memberships.is_empty());
    assert!(d.policy_changed);
    let audit = d
        .records
        .iter()
        .find(|r| r.kind == "ethernet.wire.membership_timer/1")
        .unwrap();
    assert_eq!(audit.data["expired_leases"], 2);
    assert_eq!(audit.data["affected_partitions"], 2);
    assert_eq!(
        d.records
            .iter()
            .filter(|r| r.kind == "ethernet.wire.membership/1")
            .count(),
        2
    );
    state.apply(d);
    let d = state
        .plan(
            110,
            report.unwrap(),
            &WireView {
                policy_epoch: 3,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(d.memberships.len(), 1);
    assert!(
        d.next
            .snooper
            .leases
            .values()
            .all(|l| l.generation == 3 && l.deadline == 210)
    );
    assert_eq!(
        state.snooper.leases.len(),
        0,
        "plan must not change committed snapshot"
    );
}
#[test]
fn three_bridge_mvrp_map_stabilizes_with_static_sources() {
    let mut p = prepared(&[WireProtocol::Rstp, WireProtocol::Mvrp]);
    let rstp_template = p.adapters[0].clone();
    let mrp_template = p.adapters[1].clone();
    let port_template = p.ports["a/p"].clone();
    p.adapters.clear();
    p.ports.clear();
    p.svre_exists.clear();
    for (index, bridge) in ["a", "b", "c"].into_iter().enumerate() {
        let names: Vec<_> = (1..=3).map(|n| format!("{bridge}/p{n}")).collect();
        let mut rstp = rstp_template.clone();
        rstp.id = format!("{bridge}-Rstp");
        rstp.bridge = bridge.into();
        rstp.ports = names.clone();
        rstp.max_state_entries = 3;
        let WireOptions::Rstp(config) = &mut rstp.options else {
            unreachable!()
        };
        config.bridge_id = 0x8000000000000001 + index as u64;
        config.ports = (1..=3)
            .map(|n| RstpPortConfig {
                port: format!("{bridge}/p{n}"),
                port_id: 0x8000 + n,
                external_path_cost: 20000,
                admin_edge: n == 1,
                auto_edge: true,
                auto_isolate: false,
                restricted_role: false,
                restricted_tcn: false,
            })
            .collect();
        let mut mrp = mrp_template.clone();
        mrp.id = format!("{bridge}-Mvrp");
        mrp.bridge = bridge.into();
        mrp.ports = names.clone();
        mrp.max_state_entries = 3 * 4094;
        let WireOptions::Mvrp(config) = &mut mrp.options else {
            unreachable!()
        };
        let template = config.ports[0].clone();
        config.ports = names
            .iter()
            .map(|name| {
                let mut c = template.clone();
                c.port = name.clone();
                c
            })
            .collect();
        config.local_declarations = if bridge == "a" {
            BTreeSet::from([("a/p1".into(), 20)])
        } else {
            BTreeSet::new()
        };
        p.adapters.extend([rstp, mrp]);
        for name in names {
            let mut port = port_template.clone();
            port.output = name.clone();
            port.ingress = name.replace("/p", "/in");
            port.bridge = bridge.into();
            port.static_vlans = BTreeMap::from([(10, true)]);
            port.allowed_vlans = BTreeMap::from([(20, true)]);
            p.registrar_modes
                .insert((name.clone(), 10), WireRegistrarMode::FixedIgnoreNew);
            p.ports.insert(name, port);
        }
        p.svre_exists
            .extend([(bridge.into(), 10), (bridge.into(), 20)]);
    }
    let peer = BTreeMap::from([
        ("a/p2", "b/in2"),
        ("b/p2", "a/in2"),
        ("a/p3", "c/in2"),
        ("c/p2", "a/in3"),
        ("b/p3", "c/in3"),
        ("c/p3", "b/in3"),
    ]);
    let mut state = WireState::new(Arc::new(p));
    let mut pending = vec![(0u64, 0u8, 0u64, WireEvent::Initialize)];
    let mut sequence = 0;
    let mut count = 0;
    let mut epoch = 0;
    while !pending.is_empty() {
        pending.sort_by_key(|x| (x.0, x.1, x.2));
        let (now, _, _, event) = pending.remove(0);
        if now >= 4_000_000_000_000 {
            break;
        }
        let d = state
            .plan(
                now,
                event,
                &WireView {
                    policy_epoch: epoch,
                    ..Default::default()
                },
            )
            .unwrap();
        if d.policy_changed {
            epoch += 1;
        }
        for timer in &d.timers {
            if let Some(at) = timer.deadline_ps {
                sequence += 1;
                pending.push((
                    at,
                    0,
                    sequence,
                    WireEvent::Timer {
                        adapter_id: timer.adapter_id.clone(),
                        key: timer.key.clone(),
                        generation: timer.generation,
                        deadline_ps: at,
                    },
                ));
            }
        }
        for tx in &d.tx {
            if let Some(ingress) = peer.get(tx.port.as_str()) {
                sequence += 1;
                pending.push((
                    now + 1_000_000,
                    1,
                    sequence,
                    WireEvent::Receive {
                        protocol: tx.protocol,
                        ingress_port: (*ingress).into(),
                        packet_id: tx.packet_id.clone(),
                        bytes: tx.bytes.clone(),
                        vid: None,
                        destination_mac: None,
                        external_injection: false,
                    },
                ));
            }
        }
        state.apply(d);
        count += 1;
        assert!(
            count < 1000,
            "three-bridge protocol loop at {now} pending={} seq={sequence}",
            pending.len()
        );
    }
    assert!(
        state
            .registered()
            .iter()
            .any(|(port, vid)| port.starts_with("b/") && *vid == 20)
    );
    assert!(
        state
            .registered()
            .iter()
            .any(|(port, vid)| port.starts_with("c/") && *vid == 20)
    );
}

#[test]
fn mvrp_auxiliary_records_use_real_state_and_trigger() {
    let state = WireState::new(Arc::new(prepared(&[WireProtocol::Mvrp])));
    let delta = state
        .plan(0, WireEvent::Initialize, &WireView::default())
        .unwrap();
    for machine in ["LeaveAll", "Periodic"] {
        let record = delta
            .records
            .iter()
            .find(|record| {
                record.kind == "ethernet.control.wire.transition"
                    && record.data["machine"] == machine
            })
            .unwrap();
        assert_eq!(record.data["trigger"], "BEGIN");
        assert_eq!(record.data["from_state"], "Passive");
        assert_eq!(
            record.data["to_state"],
            if machine == "LeaveAll" {
                "Passive"
            } else {
                "Active"
            }
        );
    }
}

include!("membership_vectors.rs");

crate::runtime::ethernet::dynamic::tests::vector_wire_private_cases!();

crate::runtime::ethernet::dynamic::tests::vector_mvrp_fdb_completion_cases!();
include!("transition_tests.rs");
#[test]
fn paired_rstp_link_keeps_derived_tc_detected() {
    let mut s = WireState::new(Arc::new(prepared(&[
        WireProtocol::Rstp,
        WireProtocol::Mvrp,
    ])));
    s.apply(
        s.plan(0, WireEvent::Initialize, &Default::default())
            .unwrap(),
    );
    s.rstp
        .get_mut("a-Rstp")
        .unwrap()
        .ports
        .get_mut("a/p")
        .unwrap()
        .counters
        .detected = 3;
    s.mvrp
        .get_mut("a-Mvrp")
        .unwrap()
        .ports
        .get_mut("a/p")
        .unwrap()
        .tc_detected = 3;
    let d = s
        .plan(
            1,
            WireEvent::Link {
                port: "a/p".into(),
                up: true,
            },
            &Default::default(),
        )
        .unwrap();
    assert_eq!(
        d.next.mvrp["a-Mvrp"].ports["a/p"].tc_detected,
        d.next.rstp["a-Rstp"].ports["a/p"].counters.detected
    );
}

#[test]
fn staged_membership_selection_precedes_commit() {
    let s = passive(false);
    let d = report(&s, false, 1, BTreeSet::from([source(false, "a")]), 10, 0);
    let static_view = WireStaticMulticast {
        known: true,
        ..Default::default()
    };
    let eligible = BTreeSet::from(["a/p".into(), "other".into()]);
    assert!(
        s.multicast_candidates_with_static(
            "a",
            1,
            source(false, "a"),
            source(false, "239.1.2.3"),
            &eligible,
            "ingress",
            &static_view
        )
        .is_empty()
    );
    assert_eq!(
        d.multicast_candidates_with_static(
            "a",
            1,
            source(false, "a"),
            source(false, "239.1.2.3"),
            &eligible,
            "ingress",
            &static_view
        ),
        BTreeSet::from(["a/p".into()])
    );
    assert_eq!(d.pending_timers(), 1);
}
#[test]
fn supplied_active_bad_pdus_preserve_protocol_timer_and_tx_state() {
    for (name, kind) in [
        ("rstp-standard-v1", WireProtocol::Rstp),
        ("mvrp-standard-v1", WireProtocol::Mvrp),
    ] {
        for row in manifest::vectors(name) {
            let bytes = if row["kind"] == "wire_reject"
                || row["kind"] == "wire_mac_frame_fcs" && row["expected"]["result"] == "drop"
            {
                codec::unhex(row["input"]["bytes_hex"].as_str().unwrap()).unwrap()
            } else if row["id"] == "P-MVRP-T01-V992" {
                codec::frame(
                    mvrp::DA,
                    [2, 0, 0, 0, 0, 1],
                    0x88f5,
                    &[0, 1, 2, 0, 1, 0, 100, 36, 0x1f, 0xff, 0, 200],
                )
            } else {
                continue;
            };
            let mut state = WireState::new(Arc::new(prepared(&[kind])));
            state.apply(
                state
                    .plan(0, WireEvent::Initialize, &Default::default())
                    .unwrap(),
            );
            let before = state.runtime_snapshot();
            let d = state
                .plan(
                    1,
                    WireEvent::Receive {
                        protocol: kind,
                        ingress_port: "a/in".into(),
                        packet_id: row["id"].as_str().unwrap().into(),
                        bytes,
                        vid: None,
                        destination_mac: None,
                        external_injection: true,
                    },
                    &Default::default(),
                )
                .unwrap();
            assert_eq!(d.next.rstp, state.rstp);
            assert_eq!(d.next.mvrp, state.mvrp);
            assert_eq!(d.next.timers, state.timers);
            assert_eq!(d.next.generation, state.generation);
            assert_eq!(d.next.tx_ordinals, state.tx_ordinals);
            assert_eq!(
                d.next.registration_generations,
                state.registration_generations
            );
            assert!(
                d.tx.is_empty()
                    && d.timers.is_empty()
                    && d.flush.is_empty()
                    && d.forwards.is_empty()
            );
            assert!(!d.policy_changed && !d.topology_changed);
            assert_eq!(state.runtime_snapshot(), before);
            for r in &d.records {
                crate::output::wire::validate_record(&r.kind, &r.data).unwrap();
            }
            let audit = &d
                .records
                .iter()
                .find(|r| r.kind == "ethernet.control.wire.event")
                .unwrap()
                .data;
            assert_eq!(audit["outcome"], "control_drop");
            assert_eq!(audit["effects"], json!([]));
        }
    }
}
