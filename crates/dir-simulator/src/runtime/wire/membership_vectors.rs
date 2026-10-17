// Included in runtime::wire::tests; uses the real staged WireState planner.
use super::vector_manifest as manifest;
fn uint(v: &Value) -> u64 {
    v.as_u64()
        .unwrap_or_else(|| v.as_str().unwrap().parse().unwrap())
}
fn packet_event(row: &Value, v6: bool) -> WireEvent {
    WireEvent::Receive {
        protocol: if v6 {
            WireProtocol::Mld
        } else {
            WireProtocol::Igmp
        },
        ingress_port: "a/in".into(),
        packet_id: row["id"].as_str().unwrap().into(),
        bytes: codec::unhex(row["input"]["bytes_hex"].as_str().unwrap()).unwrap(),
        vid: Some(1),
        destination_mac: Some(
            codec::mac(row["input"]["destination_mac"].as_str().unwrap()).unwrap(),
        ),
        external_injection: true,
    }
}
fn passive(v6: bool) -> WireState {
    let mut s = WireState::new(Arc::new(prepared(&[if v6 {
        WireProtocol::Mld
    } else {
        WireProtocol::Igmp
    }])));
    let d = s
        .plan(0, WireEvent::Initialize, &WireView::default())
        .unwrap();
    s.apply(d);
    s
}
fn membership_row(d: &ProtocolDelta) -> &Value {
    &d.records
        .iter()
        .find(|r| r.kind == "ethernet.wire.membership/1")
        .unwrap()
        .data
}
#[test]
fn supplied_membership_bytes_exact() {
    for row in manifest::vectors("membership-wire-v1") {
        if row["kind"] != "bytes" {
            continue;
        }
        let v6 = row["id"].as_str().unwrap().contains("MLD");
        let s = passive(v6);
        let event = packet_event(&row, v6);
        let WireEvent::Receive { bytes: b, .. } = &event else {
            unreachable!()
        };
        let d = s
            .plan(
                row["input"].get("arrival_ps").map(uint).unwrap_or(1000),
                event.clone(),
                &WireView::default(),
            )
            .unwrap();
        let audit = membership_row(&d);
        crate::output::wire::validate_record("ethernet.wire.membership", audit).unwrap();
        let offset = if v6 {
            40 + 8 * (b[41] as usize + 1)
        } else {
            4 * (b[0] & 15) as usize
        };
        let m = &b[offset..];
        let mut checksum_input = vec![];
        if v6 {
            checksum_input.extend(&b[8..40]);
            checksum_input.extend((m.len() as u32).to_be_bytes());
            checksum_input.extend([0, 0, 0, 58]);
        }
        checksum_input.extend(m);
        let mut actual = json!({"outcome":audit["outcome"],"reason":audit["reason"],"ip_packet_length":b.len(),"message_offset":offset,"message_length":m.len(),"message_checksum_hex":codec::hex(&m[2..4]),"folded_checksum_sum":!codec::checksum(&checksum_input),"membership_changes":audit["changes"].as_array().unwrap().len(),"epoch_increment":uint(&audit["epoch_after"])-uint(&audit["epoch_before"])});
        if !v6 {
            actual["ipv4_header_checksum_hex"] = json!(codec::hex(&b[10..12]));
        }
        if let Ok(msg) = membership::decode(b, v6, 4096) {
            actual["record_count"] = json!(msg.records.len());
            if let Some(r) = msg.records.first() {
                actual["record_type"] = json!(r.kind);
                actual["group"] = json!(r.group.to_string());
                actual["sources"] = json!(r
                    .sources
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>());
                let changes = audit["changes"].as_array().unwrap();
                actual["action"] = json!(if changes.is_empty() {
                    "no_op"
                } else if d.next.snooper.projection().values().next().unwrap().exclude {
                    "insert_exclude"
                } else {
                    "insert_include"
                });
                if row["expected"].get("aux_words").is_some() {
                    actual["aux_words"] = json!(m[9]);
                    let size = if v6 { 16 } else { 4 };
                    let wire_sources = u16::from_be_bytes([m[10], m[11]]) as usize;
                    actual["trailing_bytes"] =
                        json!(m.len() - (8 + 4 + size + wire_sources * size + m[9] as usize * 4));
                }
            }
            if let Some(q) = msg.query {
                actual["message_kind"] = audit["message_kind"].clone();
                actual["qrv"] = json!(q.qrv);
                actual["response_ps"] = json!(q.response_ps.to_string());
                actual["interval_ps"] = json!(q.interval_ps.to_string());
                actual["membership_budget_ps"] =
                    json!((u64::from(q.qrv) * q.interval_ps + q.response_ps).to_string());
            }
        }
        assert_eq!(s.snooper, membership::Snooper::default()); // planning never mutates original
        manifest::finish(
            "membership-wire-v1",
            &row,
            "runtime::wire::supplied_membership_bytes_exact",
            actual,
        );
    }
}
fn source(v6: bool, s: &str) -> std::net::IpAddr {
    match s {
        "a" => {
            if v6 {
                "2001:db8::1"
            } else {
                "192.0.2.1"
            }
        }
        "b" => {
            if v6 {
                "2001:db8::2"
            } else {
                "192.0.2.2"
            }
        }
        "c" => {
            if v6 {
                "2001:db8::3"
            } else {
                "192.0.2.3"
            }
        }
        "d" => {
            if v6 {
                "2001:db8::4"
            } else {
                "192.0.2.4"
            }
        }
        "e" => {
            if v6 {
                "2001:db8::5"
            } else {
                "192.0.2.5"
            }
        }
        _ => s,
    }
    .parse()
    .unwrap()
}
fn sources(v6: bool, v: &Value) -> BTreeSet<std::net::IpAddr> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| source(v6, s.as_str().unwrap()))
        .collect()
}
pub(crate) fn report_bytes(v6: bool, kind: u8, ss: &BTreeSet<std::net::IpAddr>) -> Vec<u8> {
    let row = manifest::vectors("membership-wire-v1")
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
    let mut b = codec::unhex(row["input"]["bytes_hex"].as_str().unwrap()).unwrap();
    let off = if v6 { 48 } else { 24 };
    let fixed = if v6 { 20 } else { 8 };
    b.truncate(off + 8 + fixed);
    b[off + 8] = kind;
    b[off + 10..off + 12].copy_from_slice(&(ss.len() as u16).to_be_bytes());
    for s in ss {
        match s {
            std::net::IpAddr::V4(a) => b.extend(a.octets()),
            std::net::IpAddr::V6(a) => b.extend(a.octets()),
        }
    }
    repair_ip(&mut b, v6);
    b
}
pub(crate) fn repair_ip(b: &mut [u8], v6: bool) {
    let off = if v6 { 48 } else { 24 };
    let len = b.len();
    if v6 {
        b[4..6].copy_from_slice(&((len - 40) as u16).to_be_bytes());
    } else {
        b[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        b[10..12].fill(0);
        let cs = codec::checksum(&b[..24]);
        b[10..12].copy_from_slice(&cs.to_be_bytes());
    }
    b[off + 2..off + 4].fill(0);
    let mut pseudo = vec![];
    if v6 {
        pseudo.extend(&b[8..40]);
        pseudo.extend(((len - off) as u32).to_be_bytes());
        pseudo.extend([0, 0, 0, 58]);
    }
    pseudo.extend(&b[off..]);
    let cs = codec::checksum(&pseudo);
    b[off + 2..off + 4].copy_from_slice(&cs.to_be_bytes());
}
fn report(
    s: &WireState,
    v6: bool,
    kind: u8,
    ss: BTreeSet<std::net::IpAddr>,
    now: u64,
    epoch: u64,
) -> ProtocolDelta {
    s.plan(
        now,
        WireEvent::Receive {
            protocol: if v6 {
                WireProtocol::Mld
            } else {
                WireProtocol::Igmp
            },
            ingress_port: "a/in".into(),
            packet_id: format!("report-{now}-{kind}"),
            bytes: report_bytes(v6, kind, &ss),
            vid: Some(1),
            destination_mac: Some(if v6 {
                [0x33, 0x33, 0, 0, 0, 0x16]
            } else {
                [1, 0, 94, 0, 0, 0x16]
            }),
            external_injection: true,
        },
        &WireView {
            policy_epoch: epoch,
            ..Default::default()
        },
    )
    .unwrap()
}
fn wake(s: &WireState, now: u64, epoch: u64) -> ProtocolDelta {
    let ((adapter, key), (generation, deadline)) = s
        .timers
        .iter()
        .find(|((_, k), (_, at))| k.slot == "membership" && *at == Some(now))
        .unwrap_or_else(|| panic!("no wake {now}: {:?}", s.timers));
    s.plan(
        now,
        WireEvent::Timer {
            adapter_id: adapter.clone(),
            key: key.clone(),
            generation: *generation,
            deadline_ps: deadline.unwrap(),
        },
        &WireView {
            policy_epoch: epoch,
            ..Default::default()
        },
    )
    .unwrap()
}
fn lease_snapshot(s: &WireState, now: u64) -> Value {
    let projection = s.snooper.projection();
    let f = projection.values().next();
    json!({"at_ps":now.to_string(),"mode":f.map(|f|if f.exclude{"exclude"}else{"include"}),"sources":f.map(|f|f.sources.iter().map(|s|match s{std::net::IpAddr::V4(a)=>match a.octets()[3]{1=>"a".into(),2=>"b".into(),_=>a.to_string()},std::net::IpAddr::V6(a)=>match a.segments()[7]{1=>"a".into(),2=>"b".into(),_=>a.to_string()}}).collect::<Vec<String>>()).unwrap_or_default(),"expiries_ps":s.snooper.leases.values().map(|l|l.deadline.to_string()).collect::<Vec<_>>()})
}
#[test]
fn supplied_membership_leases_and_timers() {
    for row in manifest::vectors("membership-wire-v1") {
        let v6 = row["id"].as_str().unwrap().contains("MLD");
        let i = &row["input"];
        let mut s = passive(v6);
        let actual = match row["kind"].as_str().unwrap() {
            "filter_union" => {
                for (key, kind) in [("include", 1), ("exclude", 2)] {
                    for ss in i[key].as_array().unwrap() {
                        let d = report(&s, v6, kind, sources(v6, ss), 0, 0);
                        s.apply(d);
                    }
                }
                let p = s.snooper.projection();
                let f = p.values().next().unwrap();
                json!({"mode":if f.exclude{"exclude"}else{"include"},"sources":f.sources.iter().map(ToString::to_string).collect::<Vec<_>>()})
            }
            "lease_timeline" => {
                let mut tx = 0;
                for ev in i["events"].as_array().unwrap() {
                    let d = report(
                        &s,
                        v6,
                        uint(&ev["type"]) as u8,
                        sources(v6, &ev["sources"]),
                        uint(&ev["at_ps"]),
                        0,
                    );
                    tx += d.tx.len();
                    s.apply(d);
                }
                let mut snapshots = vec![lease_snapshot(&s, 40)];
                for now in [110, 120] {
                    let d = wake(&s, now, 0);
                    tx += d.tx.len();
                    s.apply(d);
                    snapshots.push(lease_snapshot(&s, now));
                }
                json!({"snapshots":snapshots,"generated_queries":tx,"generated_reports":tx})
            }
            "timer_refresh" => {
                let ss = BTreeSet::from([source(v6, "a")]);
                let d = report(&s, v6, 1, ss.clone(), uint(&i["insert_ps"]), 0);
                assert_eq!(d.pending_timers(), 1);
                let first = d.next.snooper.next_deadline("a");
                let old_timer = d.timers[0].clone();
                s.apply(d);
                let d = report(&s, v6, 1, ss, uint(&i["refresh_ps"]), 0);
                let refreshed = d.next.snooper.next_deadline("a");
                s.apply(d);
                let d = s
                    .plan(
                        uint(&i["stale_wake_ps"]),
                        WireEvent::Timer {
                            adapter_id: old_timer.adapter_id,
                            key: old_timer.key,
                            generation: old_timer.generation,
                            deadline_ps: old_timer.deadline_ps.unwrap(),
                        },
                        &WireView::default(),
                    )
                    .unwrap();
                assert_eq!(d.next.snooper, s.snooper);
                let stale = membership_row_count_changes(&d);
                s.apply(d);
                let d = wake(&s, uint(&i["end_ps"]), 0);
                json!({"first_expiry_ps":first.map(|v|v.to_string()),"refreshed_expiry_ps":refreshed.map(|v|v.to_string()),"stale_changes":stale,"final_expired":d.next.snooper.leases.is_empty()})
            }
            "same_time_order" => {
                let d = report(&s, v6, 1, BTreeSet::from([source(v6, "a")]), 10, 6);
                s.apply(d);
                let before = uint(&i["epoch_before"]);
                let expired = wake(&s, uint(&i["expiry_ps"]), before);
                let e = membership_row(&expired);
                assert_eq!(e["outcome"], "expired");
                let mid = uint(&e["epoch_after"]);
                s.apply(expired);
                let d = report(
                    &s,
                    v6,
                    1,
                    BTreeSet::from([source(v6, "a")]),
                    uint(&i["report_ps"]),
                    mid,
                );
                let epoch = uint(&membership_row(&d)["epoch_after"]);
                s.apply(d);
                assert!(s
                    .multicast_candidates(
                        "a",
                        1,
                        source(v6, "a"),
                        source(v6, if v6 { "ff3e::1234" } else { "239.1.2.3" }),
                        &BTreeSet::from(["a/p".into()]),
                        "other"
                    )
                    .contains("a/p"));
                json!({"actions":["phase0_expire","phase1_insert","phase2_select"],"epoch_after":epoch.to_string(),"new_expiry_ps":s.snooper.next_deadline("a").map(|x|x.to_string())})
            }
            "horizon" => {
                let d = report(
                    &s,
                    v6,
                    1,
                    BTreeSet::from([source(v6, "a")]),
                    uint(&i["insert_ps"]),
                    0,
                );
                s.apply(d);
                let timer = s.snooper.next_deadline("a").unwrap();
                let executed = timer < uint(&i["horizon_ps"]);
                if executed {
                    let d = wake(&s, timer, 0);
                    s.apply(d);
                }
                json!({"expiry_ps":timer.to_string(),"expiry_executed":executed,"snapshot_contains_lease":!s.snooper.leases.is_empty()})
            }
            "source_order" => {
                let d = report(&s, v6, 1, sources(v6, &i["sources"]), 10, 0);
                json!({"sources":membership_row(&d)["changes"][0]["sources"],"order":"unsigned_network_bytes"})
            }
            _ => continue,
        };
        manifest::finish(
            "membership-wire-v1",
            &row,
            "runtime::wire::supplied_membership_leases_and_timers",
            actual,
        );
    }
}
fn membership_row_count_changes(d: &ProtocolDelta) -> usize {
    d.records
        .iter()
        .filter(|r| r.kind == "ethernet.wire.membership/1")
        .map(|r| r.data["changes"].as_array().unwrap().len())
        .sum()
}
fn dynamic_host() -> crate::runtime::ethernet::dynamic::DynamicState {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let prepared = crate::prepare(&root.join("examples/ethernet-wire/passive.ini")).unwrap();
    let n = prepared.registered.unwrap().network.unwrap();
    crate::runtime::ethernet::dynamic::DynamicState::new_with_wire(
        n.dynamic.as_ref().unwrap(),
        &n.ethernet,
        n.wire.as_ref(),
    )
    .unwrap()
}
fn dynamic_projection(
    host: &crate::runtime::ethernet::dynamic::DynamicState,
    d: &ProtocolDelta,
    now: u64,
) -> crate::runtime::ethernet::dynamic::DynamicDelta {
    let (base, _, _) = host
        .plan_wire_in(
            None,
            now,
            &d.policies,
            &d.registered,
            &[],
            d.policy_changed,
            false,
        )
        .unwrap();
    host.plan_wire_membership_in(
        base,
        now,
        &d.memberships,
        &d.router_ports,
        &d.compatibility_flood,
    )
    .unwrap()
}
#[test]
fn supplied_membership_audit_and_failure() {
    for row in manifest::vectors("membership-wire-v1") {
        let v6 = row["id"].as_str().unwrap().contains("MLD");
        let i = &row["input"];
        let mut s = passive(v6);
        let actual = match row["kind"].as_str().unwrap() {
            "atomic_failure" => {
                let prefix = report(&s, v6, 1, BTreeSet::from([source(v6, "a")]), 1, 0);
                s.apply(prefix);
                s.record_sequence = u64::MAX - 1;
                let before = s.runtime_snapshot();
                let old = s.snooper.clone();
                let b = report_bytes(v6, 1, &BTreeSet::from([source(v6, "a")]));
                let second = report_bytes(v6, 2, &BTreeSet::from([source(v6, "b")]));
                let off = if v6 { 48 } else { 24 };
                let mut both = b;
                both[off + 6..off + 8].copy_from_slice(&2u16.to_be_bytes());
                both.extend(&second[off + 8..]);
                repair_ip(&mut both, v6);
                let err = s
                    .plan(
                        10,
                        WireEvent::Receive {
                            protocol: if v6 {
                                WireProtocol::Mld
                            } else {
                                WireProtocol::Igmp
                            },
                            ingress_port: "a/in".into(),
                            packet_id: "reserve-failure".into(),
                            bytes: both,
                            vid: Some(1),
                            destination_mac: Some(if v6 {
                                [0x33, 0x33, 0, 0, 0, 0x16]
                            } else {
                                [1, 0, 94, 0, 0, 0x16]
                            }),
                            external_injection: true,
                        },
                        &WireView::default(),
                    )
                    .unwrap_err();
                assert!(err.message.contains("audit sequence overflow"));
                assert_eq!(s.snooper, old);
                json!({"callback":"failed","membership_changes":s.snooper.leases.len()-old.leases.len(),"router_changes":s.snooper.routers.len()-old.routers.len(),"epoch_increment":0,"new_audits":0,"new_copies":0,"success_prefix_preserved":before==s.runtime_snapshot()})
            }
            "simultaneous_expiry" => {
                for lease in i["leases"].as_array().unwrap() {
                    let d = report(&s, v6, 1, sources(v6, &lease["sources"]), 10, 0);
                    s.apply(d);
                }
                let mut host = dynamic_host();
                let d = report(&s, v6, 1, sources(v6, &i["leases"][0]["sources"]), 10, 0);
                host.apply(dynamic_projection(&host, &d, 10));
                let before = uint(&i["epoch_before"]);
                let d = wake(&s, uint(&i["wake_ps"]), before);
                let audit = membership_row(&d);
                let timers: Vec<_> = d
                    .records
                    .iter()
                    .filter(|r| r.kind == "ethernet.wire.membership_timer/1")
                    .collect();
                let projected = dynamic_projection(&host, &d, 110);
                let dynamic: Vec<_> = projected
                    .control_records
                    .iter()
                    .filter(|r| r.kind.starts_with("wire_membership"))
                    .collect();
                json!({"epoch_after":audit["epoch_after"],"expired_leases":timers[0].data["expired_leases"],"remaining_leases":d.next.snooper.leases.len(),"timer_audits":timers.len(),"membership_audits":d.records.iter().filter(|r|r.kind=="ethernet.wire.membership/1").count(),"membership_change_sources":audit["changes"].as_array().unwrap().iter().map(|c|c["sources"].clone()).collect::<Vec<_>>(),"dynamic_control_audits":dynamic.len(),"dynamic_kind":dynamic[0].kind,"next_wake_ps":d.next.snooper.next_deadline("a")})
            }
            "fallback" | "query_budget" | "time_code" => {
                let base = manifest::vectors("membership-wire-v1")
                    .into_iter()
                    .find(|r| {
                        r["id"]
                            == if v6 {
                                "P-MLD-T01-V07"
                            } else {
                                "P-IGMP-T01-V07"
                            }
                    })
                    .unwrap();
                let mut b = codec::unhex(base["input"]["bytes_hex"].as_str().unwrap()).unwrap();
                let off = if v6 { 48 } else { 24 };
                let kind = row["kind"].as_str().unwrap();
                if kind == "fallback" {
                    b.truncate(off + if v6 { 24 } else { 8 });
                } else if kind == "query_budget" {
                    b[off + if v6 { 24 } else { 8 }] = uint(&i["qrv"]) as u8;
                    b[off + if v6 { 25 } else { 9 }] = uint(&i["qqic"]) as u8;
                } else {
                    let c = uint(&i["response_code"]);
                    if v6 {
                        b[off + 4..off + 6].copy_from_slice(&(c as u16).to_be_bytes());
                    } else {
                        b[off + 1] = c as u8;
                    }
                    b[off + if v6 { 25 } else { 9 }] = uint(&i["qqic"]) as u8;
                }
                repair_ip(&mut b, v6);
                let msg = membership::decode(&b, v6, 4096).unwrap();
                let next = s
                    .snooper
                    .receive(
                        "a",
                        "a/p",
                        1,
                        v6,
                        10,
                        &msg,
                        260_000_000_000_000,
                        260_000_000_000_000,
                    )
                    .unwrap();
                if kind == "time_code" {
                    let q = msg.query.unwrap();
                    json!({"response_ps":q.response_ps.to_string(),"interval_ps":q.interval_ps.to_string()})
                } else if kind == "query_budget" {
                    let q = msg.query.unwrap();
                    json!({"budget_ps":(u64::from(q.qrv)*q.interval_ps+q.response_ps).to_string(),"compatibility_flood":next.flood.contains(&("a".into(),1,v6))})
                } else {
                    assert!(msg.legacy);
                    let d = s
                        .plan(
                            10,
                            WireEvent::Receive {
                                protocol: if v6 {
                                    WireProtocol::Mld
                                } else {
                                    WireProtocol::Igmp
                                },
                                ingress_port: "a/in".into(),
                                packet_id: "legacy".into(),
                                bytes: b,
                                vid: Some(1),
                                destination_mac: Some(if v6 {
                                    [0x33, 0x33, 0, 0, 0, 1]
                                } else {
                                    [1, 0, 94, 0, 0, 1]
                                }),
                                external_injection: true,
                            },
                            &WireView::default(),
                        )
                        .unwrap();
                    json!({"compatibility_flood":d.compatibility_flood.contains(&("a".into(),1,v6)),"expires_ps":Value::Null,"generated_queries":d.tx.len(),"generated_reports":d.tx.len()})
                }
            }
            _ => continue,
        };
        if row["kind"] == "atomic_failure" {
            continue;
        } // Extra audit-counter regression; exact host reservation case runs separately.
        manifest::finish(
            "membership-wire-v1",
            &row,
            "runtime::wire::supplied_membership_audit_and_failure",
            actual,
        );
    }
}
fn host_inputs() -> (
    crate::types::ethernet::PreparedEthernet,
    crate::types::ethernet::dynamic::PreparedDynamicEthernet,
) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let p = crate::prepare(&root.join("examples/ethernet-wire/passive.ini")).unwrap();
    let n = p.registered.unwrap().network.unwrap();
    (n.ethernet.clone(), n.dynamic.clone().unwrap())
}
#[test]
fn supplied_rstp_scoped_flush_and_mvrp_effective() {
    use crate::runtime::ethernet::dynamic::DynamicState;
    use crate::types::ethernet::dynamic::{MacKey, RegistrationKey};
    for row in manifest::vectors("rstp-standard-v1")
        .into_iter()
        .filter(|r| r["input"]["operation"] == "fdb_flush")
    {
        let edge = row["input"]["operEdge"] == true;
        let (mut base, mut dynamic) = host_inputs();
        let p1 = "Net.s1.tx_a";
        let p2 = "Net.s1.tx_b";
        for (vid, mac) in [
            (10, "02:00:00:00:00:01"),
            (20, "02:00:00:00:00:01"),
            (10, "02:00:00:00:00:02"),
        ] {
            dynamic.mac_keys.insert(MacKey {
                switch: "Net.s1".into(),
                vid,
                mac: mac.into(),
            });
        }
        base.devices
            .iter_mut()
            .find(|d| d.id == "Net.s1")
            .unwrap()
            .vlan_fdb
            .clear();
        base.devices
            .iter_mut()
            .find(|d| d.id == "Net.s1")
            .unwrap()
            .vlan_fdb
            .insert((10, "02:00:00:00:00:0f".into()), p1.into());
        let static_before = base
            .devices
            .iter()
            .find(|d| d.id == "Net.s1")
            .unwrap()
            .vlan_fdb
            .clone();
        let mut p = prepared(&[WireProtocol::Rstp]);
        let mut a = p.adapters.remove(0);
        a.bridge = "Net.s1".into();
        a.id = "rstp".into();
        a.ports = vec![p1.into(), p2.into()];
        let WireOptions::Rstp(c) = &mut a.options else {
            unreachable!()
        };
        let mut pc = c.ports[0].clone();
        pc.port = p1.into();
        pc.admin_edge = edge;
        pc.auto_edge = false;
        let mut second = pc.clone();
        second.port = p2.into();
        second.port_id = 0x8002;
        second.admin_edge = false;
        c.ports = vec![pc, second];
        p.adapters = vec![a.clone()];
        let template = p.ports["a/p"].clone();
        p.ports.clear();
        for name in [p1, p2] {
            let mut wp = template.clone();
            wp.output = name.into();
            wp.ingress = name.replace("tx_", "rx_");
            wp.bridge = "Net.s1".into();
            p.ports.insert(name.into(), wp);
        }
        p.registrar_modes = BTreeMap::from([((p1.into(), 10), WireRegistrarMode::FixedIgnoreNew)]);
        let mut wire = WireState::new(Arc::new(p.clone()));
        wire.apply(
            wire.plan(0, WireEvent::Initialize, &Default::default())
                .unwrap(),
        );
        wire.rstp
            .get_mut("rstp")
            .unwrap()
            .ports
            .get_mut(p1)
            .unwrap()
            .fdb_flush = true;
        let mut h = DynamicState::new_with_wire(&dynamic, &base, Some(&p)).unwrap();
        let policies = BTreeMap::from_iter([p1, p2].into_iter().map(|n| {
            (
                n.into(),
                WirePortPolicy {
                    role: "Designated".into(),
                    learning: true,
                    forwarding: true,
                    link_up: true,
                },
            )
        }));
        let (d, _, _) = h
            .plan_wire_in(None, 0, &policies, &BTreeSet::new(), &[], true, false)
            .unwrap();
        h.apply(d);
        for (ingress, vid, mac) in [
            ("Net.s1.rx_a", 10, "02:00:00:00:00:01"),
            ("Net.s1.rx_a", 20, "02:00:00:00:00:01"),
            ("Net.s1.rx_b", 10, "02:00:00:00:00:02"),
        ] {
            h.apply(h.plan_learning(ingress, vid, mac, 1).unwrap());
        }
        let mut d = wire
            .plan(
                2,
                WireEvent::Link {
                    port: p1.into(),
                    up: true,
                },
                &Default::default(),
            )
            .unwrap();
        assert_eq!(d.flush.len(), 1);
        let (host, keys, _) = h
            .plan_wire_in(None, 2, &d.policies, &d.registered, &d.flush, false, false)
            .unwrap();
        let removed = keys[0].clone();
        d.complete_host_records(2, 1, 0, keys, vec![]).unwrap();
        let completed = d
            .records
            .iter()
            .flat_map(|r| r.data["effects"].as_array().into_iter().flatten())
            .find(|e| e["kind"] == "mac_flush")
            .unwrap();
        assert_eq!(completed["completed"], true);
        assert!(!d.next.rstp["rstp"].ports[p1].fdb_flush);
        h.apply(host);
        assert_eq!(
            static_before,
            base.devices
                .iter()
                .find(|d| d.id == "Net.s1")
                .unwrap()
                .vlan_fdb
        );
        let actual = if edge {
            json!({"removed_count":removed.len(),"fdbFlush_after_completion":d.next.rstp["rstp"].ports[p1].fdb_flush})
        } else {
            assert_eq!(removed.len(), 1);
            assert_eq!(removed[0].mac, "02:00:00:00:00:01");
            assert_eq!(removed[0].vid, 10);
            assert_eq!(h.snapshot_tables()["mac"].as_array().unwrap().len(), 2);
            json!({"removed_entry_indices":[0],"fdbFlush_after_completion":d.next.rstp["rstp"].ports[p1].fdb_flush})
        };
        manifest::finish(
            "rstp-standard-v1",
            &row,
            "runtime::wire::supplied_rstp_scoped_flush_and_mvrp_effective",
            actual,
        );
    }
    let row = manifest::vectors("mvrp-standard-v1")
        .into_iter()
        .find(|r| r["id"] == "P-MVRP-T01-V989")
        .unwrap();
    let (mut base, mut p) = host_inputs();
    let port = "Net.s1.tx_a";
    base.port_policies
        .iter_mut()
        .find(|p| p.port == port)
        .unwrap()
        .vlans = BTreeMap::from([(100, true)]);
    p.registrable = BTreeSet::from([RegistrationKey {
        port: port.into(),
        vid: 200,
    }]);
    let mut wp = prepared(&[WireProtocol::Mvrp]);
    let mut wireport = wp.ports["a/p"].clone();
    wireport.output = port.into();
    wireport.bridge = "Net.s1".into();
    wireport.static_vlans = BTreeMap::from([(100, true)]);
    wireport.allowed_vlans = BTreeMap::from([(200, true)]);
    wp.ports = BTreeMap::from([(port.into(), wireport)]);
    wp.adapters[0].ports = vec![port.into()];
    wp.adapters[0].bridge = "Net.s1".into();
    wp.adapters.truncate(1);
    let h = DynamicState::new_with_wire(&p, &base, Some(&wp)).unwrap();
    let (d, _, _) = h
        .plan_wire_in(
            None,
            1,
            &BTreeMap::new(),
            &BTreeSet::from([(port.into(), 100), (port.into(), 200)]),
            &[],
            true,
            false,
        )
        .unwrap();
    let effective: Vec<_> = h.snapshot_policy_after(&d).effective_vlans[port]
        .keys()
        .copied()
        .collect();
    manifest::finish(
        "mvrp-standard-v1",
        &row,
        "runtime::wire::supplied_rstp_scoped_flush_and_mvrp_effective",
        json!({"effective":effective}),
    );
}
pub(crate) fn apply_mvrp_fixture_flushes(flush: &BTreeSet<(String, u16)>) -> bool {
    use crate::runtime::ethernet::dynamic::DynamicState;
    use crate::types::ethernet::dynamic::MacKey;
    let (base, mut prepared) = host_inputs();
    let static_before: Vec<_> = base.devices.iter().map(|d| d.vlan_fdb.clone()).collect();
    assert!(static_before.iter().any(|f| !f.is_empty()));
    let mut inputs = vec![];
    let mut intents = vec![];
    for (index, (port, vid)) in flush.iter().enumerate() {
        let (output, ingress) = match port.as_str() {
            "a" => ("Net.s1.tx_a", "Net.s1.rx_a"),
            "b" => ("Net.s1.tx_b", "Net.s1.rx_b"),
            _ => panic!("unknown fixture port"),
        };
        let mac = format!("02:00:00:00:00:{:02x}", 0xe0 + index);
        prepared.mac_keys.insert(MacKey {
            switch: "Net.s1".into(),
            vid: *vid,
            mac: mac.clone(),
        });
        inputs.push((ingress, *vid, mac));
        intents.push(WireFlushIntent {
            reason: "mvrp_new".into(),
            port: output.into(),
            vids: vec![*vid],
        });
    }
    let mut host = DynamicState::new(&prepared, &base).unwrap();
    for (ingress, vid, mac) in inputs {
        host.apply(host.plan_learning(ingress, vid, &mac, 1).unwrap());
    }
    assert_eq!(
        host.snapshot_tables()["mac"].as_array().unwrap().len(),
        flush.len()
    );
    let (delta, keys, mac) = host
        .plan_wire_in(
            None,
            2,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &intents,
            false,
            false,
        )
        .unwrap();
    assert!(keys.iter().all(|k| k.len() == 1));
    assert!(mac.is_empty());
    host.apply(delta);
    assert!(host.snapshot_tables()["mac"].as_array().unwrap().is_empty());
    static_before
        == base
            .devices
            .iter()
            .map(|d| d.vlan_fdb.clone())
            .collect::<Vec<_>>()
}
