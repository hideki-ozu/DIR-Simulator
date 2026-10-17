use super::tests::{config, ethernet, workload};
use super::*;
use crate::runtime::wire::vector_manifest as manifest;
use serde_json::json;
fn active_config() -> Value {
    let mut c = config();
    c["port_identities"] = json!([{"port":"s/p","source_mac":"02:00:00:00:00:01"}]);
    c["adapters"] = json!([{"id":"mrp","protocol":"mvrp","protocol_revision":WireProtocol::Mvrp.revision(),"ports":["s/p"],"max_pdu_bytes":1500,"max_state_entries":8188,"max_pending_timers":10000,"max_pending_tx":64,"max_control_steps":100000,"max_attributes_per_pdu":4094}]);
    c
}
fn second(eth: &mut PreparedEthernet, c: &mut Value) {
    let mut policy = eth.port_policies[0].clone();
    policy.port = "s/q".into();
    policy.ingress = "s/qin".into();
    eth.port_policies.push(policy);
    let mut out = eth.directions[0].clone();
    out.channel_id = "out2".into();
    out.from_port = "s/q".into();
    out.to_port = "h/in2".into();
    eth.directions.push(out);
    let mut input = eth.directions[1].clone();
    input.channel_id = "in2".into();
    input.from_port = "h/out2".into();
    input.to_port = "s/qin".into();
    eth.directions.push(input);
    c["port_identities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"port":"s/q","source_mac":"02:00:00:00:00:02"}));
    c["adapters"][0]["ports"]
        .as_array_mut()
        .unwrap()
        .push(json!("s/q"));
}
#[test]
fn supplied_mvrp_preparation() {
    for row in manifest::vectors("mvrp-standard-v1") {
        if row["kind"] != "prepare_rule" && row["kind"] != "timer_arithmetic" {
            continue;
        }
        let i = &row["input"];
        let id = row["id"].as_str().unwrap();
        let mut eth = ethernet();
        let mut c = active_config();
        eth.port_policies[0].vlans.clear();
        if let Some(static_s) = i["static_s"].as_array() {
            for vid in static_s {
                eth.port_policies[0]
                    .vlans
                    .insert(vid.as_u64().unwrap() as u16, true);
            }
        }
        if let Some(allowed) = i["registrable_a"].as_array() {
            for value in allowed {
                let (port, vid) = if value.is_object() {
                    ("s/p", value["vid"].as_u64().unwrap())
                } else {
                    ("s/p", value.as_u64().unwrap())
                };
                c["ethernet"]["dynamic"]["registrable"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"port":port,"vid":vid,"tagged":true}));
            }
        }
        let overrides = i.get("overrides").or_else(|| i.get("registrar_controls"));
        if let Some(overrides) = overrides {
            c["adapters"][0]["registrar_controls"] = json!(
                overrides
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| json!({"port":"s/p","vid":r["vid"],"mode":r["mode"]}))
                    .collect::<Vec<_>>()
            );
        }
        if id == "P-MVRP-T01-V1077" {
            second(&mut eth, &mut c);
        }
        if let Some(v) = i.get("max_pdu_bytes") {
            c["adapters"][0]["max_pdu_bytes"] = v.clone();
        }
        if let Some(v) = i.get("max_state_entries") {
            second(&mut eth, &mut c);
            c["adapters"][0]["max_state_entries"] = v.clone();
        }
        if row["kind"] == "timer_arithmetic" {
            c["adapters"][0]["port_parameters"] = json!([{"port":"s/p","join_time_cs":i["join_time_cs"],"leave_time_cs":i["leave_time_cs"],"leaveall_time_cs":i["leaveall_time_cs"]}]);
        }
        let result = prepare(&c, &workload(), &eth);
        let actual = match result {
            Err(e) => {
                let reason = if e.message.contains("max_pdu_bytes") {
                    "client_minimum_46"
                } else if e.message.contains("capacity for all") {
                    "state_capacity"
                } else if i["static_s"].as_array().is_some_and(|s| !s.is_empty()) {
                    "static_membership_admin_conflict"
                } else {
                    "fixed_without_static_membership"
                };
                assert!(
                    e.message.contains("max_pdu_bytes")
                        || e.message.contains("capacity for all")
                        || e.message.contains("fixed mode"),
                    "{e:?}"
                );
                if id == "P-MVRP-T01-V1075" {
                    json!({"result":"prepare_reject","required_state_entries":c["adapters"][0]["ports"].as_array().unwrap().len()*4094})
                } else {
                    json!({"result":"prepare_reject","reason":reason})
                }
            }
            Ok(p) => {
                let mut catalog = serde_json::Map::new();
                for vid in [100u16, 200] {
                    if p.svre_exists.contains(&("s".into(), vid)) {
                        catalog.insert(
                            vid.to_string(),
                            json!(
                                p.registrar_modes
                                    .get(&("s/p".into(), vid))
                                    .copied()
                                    .unwrap_or(WireRegistrarMode::Normal)
                            ),
                        );
                    }
                }
                let mask: Vec<_> = p
                    .registrar_modes
                    .iter()
                    .filter(|(_, m)| {
                        matches!(
                            m,
                            WireRegistrarMode::FixedIgnoreNew
                                | WireRegistrarMode::FixedPropagateNew
                        )
                    })
                    .map(|((_, vid), _)| *vid)
                    .collect();
                let WireOptions::Mvrp(m) = &p.adapters[0].options else {
                    unreachable!()
                };
                let pc = &m.ports[0];
                let mut actual = json!({"catalog":catalog,"fixed_mask":mask,"absent_vid_300":if !p.svre_exists.contains(&("s".into(),300))&&!p.registrar_modes.contains_key(&("s/p".into(),300)){"normal_without_svre"}else{"present"},"registration_permitted":p.svre_exists.contains(&("s".into(),200)),"join_ps":pc.join_time_ps.to_string(),"leave_ps":pc.leave_time_ps.to_string(),"leaveall_open_interval_ps":[pc.leaveall_time_ps.to_string(),(pc.leaveall_time_ps*3/2).to_string()],"periodic_ps":"1000000000000"});
                if id == "P-MVRP-T01-V1077" {
                    let mut p = p;
                    let WireOptions::Mvrp(m) = &mut p.adapters[0].options else {
                        unreachable!()
                    };
                    m.ports
                        .iter_mut()
                        .for_each(|c| c.restricted_vlan_registration = true);
                    let a = &p.adapters[0];
                    let state = crate::runtime::wire::mvrp_state::State::new(a);
                    let d = state
                        .plan(
                            &p,
                            a,
                            1,
                            crate::runtime::wire::mvrp_state::Input::Receive(
                                "s/q".into(),
                                crate::runtime::wire::mvrp::DecodedMvrp {
                                    version: 0,
                                    vectors: vec![crate::runtime::wire::mvrp::Vector {
                                        leave_all: false,
                                        attributes: vec![(
                                            100,
                                            crate::runtime::wire::mvrp::AttributeEvent::JoinIn,
                                        )],
                                    }],
                                },
                            ),
                        )
                        .unwrap();
                    actual = json!({"svre_exists":p.svre_exists.contains(&("s".into(),100)),"mode_b":p.registrar_modes.get(&("s/q".into(),100)).copied().unwrap_or(WireRegistrarMode::Normal),"wire_registered_b":d.state.ports["s/q"].attributes[&100].registered,"effective_b":d.state.ports["s/q"].attributes[&100].registered&&p.ports["s/q"].allowed_vlans.contains_key(&100),"map_indication":d.trace.iter().any(|t|t.port=="s/p"&&t.event=="Join"&&t.vid==Some(100))});
                }
                actual
            }
        };
        manifest::finish(
            "mvrp-standard-v1",
            &row,
            "input::wire::supplied_mvrp_preparation",
            actual,
        );
    }
}
#[test]
fn supplied_membership_boundaries() {
    use crate::runtime::wire::boundary::{BoundaryState, Event};
    for row in manifest::vectors("membership-wire-v1") {
        let i = &row["input"];
        let v6 = row["id"].as_str().unwrap().contains("MLD");
        let kind = row["kind"].as_str().unwrap();
        if !matches!(
            kind,
            "wire_timing" | "malformed_boundary" | "boundary_prefix" | "boundary_overlap"
        ) {
            continue;
        }
        let u = |v: &Value| {
            v.as_u64()
                .unwrap_or_else(|| v.as_str().unwrap().parse().unwrap())
        };
        let mut eth = ethernet();
        for d in &mut eth.directions {
            d.bitrate_bps = 1_000_000_000;
        }
        let mut c = config();
        if v6 {
            c["adapters"][0]["id"] = json!("mld");
            c["adapters"][0]["protocol"] = json!("mld");
            c["adapters"][0]["protocol_revision"] = json!(WireProtocol::Mld.revision());
        }
        let base = manifest::vectors("membership-wire-v1")
            .into_iter()
            .find(|r| {
                r["id"]
                    == if v6 {
                        "P-MLD-T01-V01"
                    } else {
                        "P-IGMP-T01-V01"
                    }
            })
            .unwrap();
        let mut packet = json!({"id":"case","adapter_id":if v6{"mld"}else{"igmp"},"ingress_port":"s/in","arrival_ps":"1000000","encapsulation":"ip_packet","bytes_hex":base["input"]["bytes_hex"],"vid":1,"source_mac":"02:00:00:00:00:01","destination_mac":if v6{"33:33:00:00:00:16"}else{"01:00:5e:00:00:16"}});
        let mut w = workload();
        let actual = match kind {
            "wire_timing" | "malformed_boundary" => {
                if kind == "malformed_boundary" {
                    packet["bytes_hex"] = i["bytes_hex"].clone();
                }
                packet["arrival_ps"] = i["arrival_ps"].clone();
                w["packets"] = json!([packet]);
                let p = prepare(&c, &w, &eth).unwrap();
                let packet = &p.packets[0];
                let plan = packet.boundary.as_ref().unwrap();
                let mut b = BoundaryState::default();
                b = b
                    .plan(plan, "case", "s/in", Event::Start, plan.start_ps, 1)
                    .unwrap()
                    .0;
                b = b
                    .plan(plan, "case", "s/in", Event::Arrival, plan.arrival_ps, 2)
                    .unwrap()
                    .0;
                let metrics = b.metrics(plan, plan.arrival_ps).unwrap();
                let mut actual = json!({"mac_bytes":plan.mac_bytes,"wire_duration_ps":(plan.arrival_ps-plan.start_ps).to_string(),"source_start_ps":plan.start_ps.to_string(),"release_ps":plan.release_ps.to_string()});
                if kind == "malformed_boundary" {
                    let mut state =
                        crate::runtime::wire::WireState::new(std::sync::Arc::new(p.clone()));
                    state.apply(
                        state
                            .plan(
                                0,
                                crate::runtime::wire::WireEvent::Initialize,
                                &Default::default(),
                            )
                            .unwrap(),
                    );
                    let d = state
                        .plan(
                            plan.arrival_ps,
                            crate::runtime::wire::WireEvent::Receive {
                                protocol: if v6 {
                                    WireProtocol::Mld
                                } else {
                                    WireProtocol::Igmp
                                },
                                ingress_port: "s/in".into(),
                                packet_id: "case".into(),
                                bytes: packet.bytes.clone(),
                                vid: Some(1),
                                destination_mac: packet.destination_mac,
                                external_injection: true,
                            },
                            &Default::default(),
                        )
                        .unwrap();
                    let audit = &d
                        .records
                        .iter()
                        .find(|r| r.kind == "ethernet.wire.membership/1")
                        .unwrap()
                        .data;
                    actual = json!({"prepare":"accepted","decoded_byte_length":packet.bytes.len(),"mac_bytes":plan.mac_bytes,"planned_start_ps":plan.start_ps.to_string(),"planned_arrival_ps":plan.arrival_ps.to_string(),"planned_release_ps":plan.release_ps.to_string(),"arrival_outcome":audit["outcome"],"reason":audit["reason"],"membership_changes":d.memberships.len(),"completed_rx_frames":metrics.completed_frames,"completed_rx_mac_bytes":metrics.completed_mac_bytes});
                }
                actual
            }
            "boundary_prefix" => {
                let plan = BoundaryPlan {
                    start_ps: u(&i["start_ps"]),
                    arrival_ps: u(&i["arrival_ps"]),
                    release_ps: u(&i["release_ps"]),
                    mac_bytes: u(&i["mac_bytes"]),
                    bitrate_bps: 1_000_000_000,
                };
                let mut cases = vec![];
                for h in i["horizons_ps"].as_array().unwrap() {
                    let h = u(h);
                    let mut s = BoundaryState::default();
                    let mut records = vec![];
                    for (at, event) in [
                        (plan.start_ps, Event::Start),
                        (plan.arrival_ps, Event::Arrival),
                        (plan.release_ps, Event::Release),
                    ] {
                        if at < h {
                            let (next, record) = s
                                .plan(&plan, "p", "s/in", event, at, records.len() as u64)
                                .unwrap();
                            s = next;
                            records.push(record);
                        }
                    }
                    let m = s.metrics(&plan, h).unwrap();
                    cases.push(json!({"horizon_ps":h.to_string(),"status":records.last().map(|r|r.status.as_str()).unwrap_or("planned"),"boundary_records":records.len(),"actual_start_ps":s.actual_start_ps.map(|v|v.to_string()),"actual_arrival_ps":s.actual_arrival_ps.map(|v|v.to_string()),"actual_release_ps":s.actual_release_ps.map(|v|v.to_string()),"rx_wire_ps":m.rx_wire_ps.to_string(),"ifg_ps":m.rx_ifg_ps.to_string(),"completed_rx_frames":m.completed_frames,"membership_callbacks":usize::from(s.actual_arrival_ps.is_some())}));
                }
                json!({"cases":cases})
            }
            "boundary_overlap" => {
                // Actual prepare reserves occupancy [start,release).
                packet["bytes_hex"] = json!(if v6 { "60" } else { "46" });
                let duration = serialization(68 + 8, 1_000_000_000).unwrap();
                assert_eq!(
                    duration + serialization(12, 1_000_000_000).unwrap(),
                    u(&i["candidate_duration_ps"])
                );
                let first_start = u(&i["first_interval_ps"][0]);
                let mut results = vec![];
                for start in i["candidate_starts_ps"].as_array().unwrap() {
                    let mut first = packet.clone();
                    first["arrival_ps"] = json!((first_start + duration).to_string());
                    let mut second = packet.clone();
                    second["id"] = json!("second");
                    second["arrival_ps"] = json!((u(start) + duration).to_string());
                    w["packets"] = json!([first, second]);
                    match prepare(&c, &w, &eth) {
                        Ok(_) => results.push("accepted"),
                        Err(e) => {
                            assert!(e.message.to_lowercase().contains("overlap"), "{e:?}");
                            results.push("BoundaryOverlap");
                        }
                    }
                }
                json!({"prepare_results":results})
            }
            _ => unreachable!(),
        };
        manifest::finish(
            "membership-wire-v1",
            &row,
            "input::wire::supplied_membership_boundaries",
            actual,
        );
    }
}
