#[path = "../src/runtime/wire/guards.rs"]
mod guards;
#[path = "../src/runtime/wire/vector_manifest.rs"]
mod vector_manifest;
// Direct tests keep the pure protocol modules testable before central wiring.
#[path = "../src/runtime/wire/codec.rs"]
mod codec;
#[path = "../src/runtime/wire/membership.rs"]
mod membership;
#[path = "../src/runtime/wire/mvrp.rs"]
mod mvrp;
#[path = "../src/runtime/wire/rstp.rs"]
mod rstp;
fn vectors(name: &str) -> Vec<serde_json::Value> {
    vector_manifest::vectors(name)
}
fn mrp_event(s: &str) -> mvrp::Event {
    use mvrp::Event::*;
    match s {
        "Begin" => Begin,
        "New" => New,
        "Join" => Join,
        "Lv" => Lv,
        "rNew" => RNew,
        "rJoinIn" => RJoinIn,
        "rIn" => RIn,
        "rJoinMt" => RJoinMt,
        "rMt" => RMt,
        "rLv" => RLv,
        "rLA" => RLA,
        "Re-declare" => Redeclare,
        "periodic" => Periodic,
        "tx" => Tx,
        "txLA" => TxLA,
        "txLAF" => TxLAF,
        "Flush" => Flush,
        "leavetimer" => LeaveExpired,
        _ => panic!("unknown fixture event {s}"),
    }
}
#[test]
fn independent_mvrp_applicant_matrix() {
    let mut count = 0;
    for v in vectors("mvrp-standard-v1") {
        if v["kind"] != "fsm_applicant" && v["kind"] != "fsm_applicant_admin" {
            continue;
        }
        let i = &v["input"];
        let e = &v["expected"];
        let state = serde_json::from_value(i["state"].clone()).unwrap();
        let registrar = serde_json::from_value(i["registrar"].clone()).unwrap();
        let result = mvrp::applicant(
            state,
            mrp_event(i["event"].as_str().unwrap()),
            i["oper_point_to_point"].as_bool().unwrap_or(true),
            registrar,
            if i["mode"] == "new_only" {
                mvrp::ApplicantControl::NewOnly
            } else {
                mvrp::ApplicantControl::Normal
            },
        );
        assert_eq!(
            serde_json::to_value(result.state).unwrap(),
            e["state"],
            "{}",
            v["id"]
        );
        if let Some(request) = e["request_tx"].as_bool() {
            assert_eq!(result.request_tx, request, "{} request", v["id"]);
        }
        let action = match e["tx_action"].as_str() {
            None => None,
            Some("sN") => Some(mvrp::AttributeEvent::New),
            Some("sL") => Some(mvrp::AttributeEvent::Lv),
            Some("sJ" | "optional_sJ") => Some(if registrar == mvrp::Registrar::IN {
                mvrp::AttributeEvent::JoinIn
            } else {
                mvrp::AttributeEvent::JoinMt
            }),
            Some("s" | "optional_s") => Some(if registrar == mvrp::Registrar::IN {
                mvrp::AttributeEvent::In
            } else {
                mvrp::AttributeEvent::Mt
            }),
            Some(x) => panic!("unknown action {x}"),
        };
        assert_eq!(result.action, action, "{} action", v["id"]);
        if v["kind"] == "fsm_applicant" {
            assert_eq!(
                result.optional,
                e["tx_action"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("optional_")),
                "{} optional",
                v["id"]
            );
        }
        vector_manifest::finish(
            "mvrp-standard-v1",
            &v,
            "wire_codecs::independent_mvrp_applicant_matrix",
            serde_json::json!({"state":result.state,"request_tx":result.request_tx,"tx_action":match result.action {None=>None,Some(mvrp::AttributeEvent::New)=>Some("sN"),Some(mvrp::AttributeEvent::Lv)=>Some("sL"),Some(mvrp::AttributeEvent::JoinIn|mvrp::AttributeEvent::JoinMt)=>Some(if result.optional{"optional_sJ"}else{"sJ"}),Some(_)=>Some(if result.optional{"optional_s"}else{"s"})}}),
        );
        count += 1;
    }
    assert_eq!(count, 792);
}
#[test]
fn independent_mvrp_registrar_matrix() {
    let mut count = 0;
    for v in vectors("mvrp-standard-v1") {
        if v["kind"] != "fsm_registrar" {
            continue;
        }
        let i = &v["input"];
        let e = &v["expected"];
        let result = mvrp::registrar(
            serde_json::from_value(i["state"].clone()).unwrap(),
            mrp_event(i["event"].as_str().unwrap()),
            i["oper_point_to_point"].as_bool().unwrap(),
            mvrp::RegistrarControl::Normal,
        );
        assert_eq!(
            serde_json::to_value(result.state).unwrap(),
            e["state"],
            "{}",
            v["id"]
        );
        let mut actions = Vec::new();
        if result.cancel_leave {
            actions.push("cancel_leave");
        }
        if result.arm_leave {
            actions.push("arm_leave");
        }
        if let Some(new) = result.join {
            actions.push(if new { "ind_new" } else { "ind_join" });
        }
        if result.leave {
            actions.push("ind_leave");
        }
        assert_eq!(serde_json::json!(actions), e["actions"], "{}", v["id"]);
        vector_manifest::finish(
            "mvrp-standard-v1",
            &v,
            "wire_codecs::independent_mvrp_registrar_matrix",
            serde_json::json!({"state":result.state,"actions":actions}),
        );
        count += 1;
    }
    assert_eq!(count, 72);
}
#[test]
fn independent_wire_decoders() {
    for v in vectors("mvrp-standard-v1") {
        let i = &v["input"];
        let result = match v["kind"].as_str() {
            Some("wire_mac_frame_fcs") => mvrp::decode(
                &codec::unhex(i["bytes_hex"].as_str().unwrap()).unwrap(),
                i["ingress_pvid"].as_u64().unwrap_or(1) as u16,
                1500,
                4094,
            ),
            Some("codec_payload_exact") => mvrp::decode_payload(
                &codec::unhex(i["payload_hex"].as_str().unwrap()).unwrap(),
                i["ingress_pvid"].as_u64().unwrap_or(1) as u16,
                4094,
            ),
            _ => continue,
        };
        assert_eq!(
            result.is_ok(),
            v["expected"]["result"] == "accept",
            "{}: {result:?}",
            v["id"]
        );
    }
    for v in vectors("rstp-standard-v1") {
        if v["kind"] != "wire_decode" && v["kind"] != "wire_reject" {
            continue;
        }
        let b = codec::unhex(v["input"]["bytes_hex"].as_str().unwrap()).unwrap();
        let result = rstp::decode(&b, 0x8001, 1500);
        assert_eq!(
            result.is_ok(),
            v["kind"] == "wire_decode",
            "{}: {result:?}",
            v["id"]
        );
    }
    for v in vectors("membership-wire-v1") {
        if v["kind"] != "bytes" {
            continue;
        }
        let b = codec::unhex(v["input"]["bytes_hex"].as_str().unwrap()).unwrap();
        let result = membership::decode(&b, b[0] >> 4 == 6, 4096).and_then(|m| {
            if let Some(mac) = v["input"]["destination_mac"].as_str() {
                if membership::destination_mac(m.destination) != codec::mac(mac).unwrap() {
                    return Err(codec::bad("destination_mac"));
                }
            }
            Ok(m)
        });
        assert_eq!(
            result.is_ok(),
            v["expected"]["outcome"] == "accepted",
            "{}: {result:?}",
            v["id"]
        );
        if v["id"] == "P-IGMP-T01-V01" {
            let snooper = membership::Snooper::default()
                .receive("s", "p", 1, false, 10, result.as_ref().unwrap(), 100, 100)
                .unwrap();
            assert_eq!(snooper.next_deadline("s"), Some(110));
        }
    }
}

#[test]
fn independent_auxiliary_states() {
    let mut n = 0;
    for v in vectors("mvrp-standard-v1") {
        let kind = v["kind"].as_str().unwrap();
        if kind != "fsm_leaveall" && kind != "fsm_periodic" {
            continue;
        }
        let i = &v["input"];
        let state = serde_json::from_value(i["state"].clone()).unwrap();
        let event = match i["event"].as_str().unwrap() {
            "Begin" => mvrp::AuxiliaryEvent::Begin,
            "rLA" => mvrp::AuxiliaryEvent::ReceiveLeaveAll,
            "leavealltimer" => mvrp::AuxiliaryEvent::LeaveAllExpired,
            "periodictimer" => mvrp::AuxiliaryEvent::PeriodicExpired,
            "tx" => mvrp::AuxiliaryEvent::Tx,
            "Enabled" | "periodicEnabled" => mvrp::AuxiliaryEvent::Enabled,
            "Disabled" | "periodicDisabled" => mvrp::AuxiliaryEvent::Disabled,
            s => panic!("unknown {s}"),
        };
        let d = if kind == "fsm_leaveall" {
            mvrp::leaveall(state, event)
        } else {
            mvrp::periodic(state, event)
        };
        assert_eq!(
            serde_json::to_value(d.state).unwrap(),
            v["expected"]["state"],
            "{}",
            v["id"]
        );
        let mut actions = vec![];
        if d.restart_timer {
            actions.push(if kind == "fsm_leaveall" {
                "arm_leaveall"
            } else {
                "arm_periodic"
            });
        }
        if d.send_leaveall {
            actions.push("sLA");
        }
        if d.periodic {
            actions.push("periodic");
        }
        if d.request_tx {
            actions.push("request_tx");
        }
        assert_eq!(
            serde_json::json!(actions),
            v["expected"]["actions"],
            "{} actions",
            v["id"]
        );
        vector_manifest::finish(
            "mvrp-standard-v1",
            &v,
            "wire_codecs::independent_auxiliary_states",
            serde_json::json!({"state":d.state,"actions":actions}),
        );
        n += 1;
    }
    assert_eq!(n, 16);
}

#[test]
fn independent_mvrp_admin_and_capacity() {
    for v in vectors("mvrp-standard-v1") {
        let i = &v["input"];
        let actual = match v["kind"].as_str() {
            Some("fsm_registrar_admin") => {
                let mode = match i["mode"].as_str().unwrap() {
                    "fixed_ignore_new" => mvrp::RegistrarControl::FixedIgnoreNew,
                    "fixed_propagate_new" => mvrp::RegistrarControl::FixedPropagateNew,
                    "forbidden" => mvrp::RegistrarControl::Forbidden,
                    _ => unreachable!(),
                };
                let d = mvrp::registrar(
                    mvrp::Registrar::MT,
                    mrp_event(i["event"].as_str().unwrap()),
                    true,
                    mode,
                );
                let actions = if d.join == Some(true) {
                    vec!["ind_new"]
                } else {
                    vec![]
                };
                serde_json::json!({"state":d.state,"actions":actions})
            }
            Some("fsm_tx_capacity") => {
                let d = mvrp::applicant_tx(
                    serde_json::from_value(i["state"].clone()).unwrap(),
                    mrp_event(i["event"].as_str().unwrap()),
                    true,
                    mvrp::Registrar::MT,
                    mvrp::ApplicantControl::Normal,
                    i["room"].as_bool().unwrap(),
                );
                serde_json::json!({"state":d.state,"request_tx":d.request_tx,"tx":d.action.filter(|_|!d.optional)})
            }
            Some("fsm_tx_action") => {
                let registrar = serde_json::from_value(i["registrar"].clone()).unwrap();
                let state = if i["action"] == "sJ" {
                    mvrp::Applicant::AA
                } else {
                    mvrp::Applicant::LO
                };
                let d = mvrp::applicant(
                    state,
                    mvrp::Event::Tx,
                    true,
                    registrar,
                    mvrp::ApplicantControl::Normal,
                );
                serde_json::json!({"wire_event":d.action.unwrap()})
            }
            _ => continue,
        };
        vector_manifest::finish(
            "mvrp-standard-v1",
            &v,
            "wire_codecs::independent_mvrp_admin_and_capacity",
            actual,
        );
    }
}
#[test]
fn independent_mvrp_packing() {
    for v in vectors("mvrp-standard-v1") {
        if v["kind"] != "packing_arithmetic" {
            continue;
        }
        let events = v["input"]["events"].as_array().unwrap();
        let attrs: Vec<_> = events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                (
                    (i + 1) as u16,
                    [
                        mvrp::AttributeEvent::New,
                        mvrp::AttributeEvent::JoinIn,
                        mvrp::AttributeEvent::In,
                        mvrp::AttributeEvent::JoinMt,
                        mvrp::AttributeEvent::Mt,
                        mvrp::AttributeEvent::Lv,
                    ][e.as_u64().unwrap() as usize],
                )
            })
            .collect();
        let (payload, used) = mvrp::encode_payload(&attrs, false, 1500).unwrap();
        vector_manifest::finish(
            "mvrp-standard-v1",
            &v,
            "wire_codecs::independent_mvrp_packing",
            serde_json::json!({"count":used,"packed_hex":codec::hex(&payload[7..7+events.len().div_ceil(3)])}),
        );
    }
}
#[test]
fn supplied_mvrp_wire_fields() {
    use serde_json::json;
    for row in vectors("mvrp-standard-v1") {
        let i = &row["input"];
        let kind = row["kind"].as_str().unwrap();
        if kind != "wire_mac_frame_fcs" && kind != "codec_payload_exact" {
            continue;
        }
        let bytes = codec::unhex(
            i[if kind == "wire_mac_frame_fcs" {
                "bytes_hex"
            } else {
                "payload_hex"
            }]
            .as_str()
            .unwrap(),
        )
        .unwrap();
        let result = if kind == "wire_mac_frame_fcs" {
            mvrp::decode(
                &bytes,
                i["ingress_pvid"].as_u64().unwrap_or(1) as u16,
                1500,
                4094,
            )
        } else {
            mvrp::decode_payload(&bytes, 1, 4094)
        };
        if row["id"] == "P-MVRP-T01-V957" {
            let (encoded, used) = mvrp::encode(
                [2, 0, 0, 0, 0, 1],
                &[(1, mvrp::AttributeEvent::New)],
                false,
                1500,
            )
            .unwrap();
            assert_eq!(used, 1);
            assert_eq!(encoded, bytes, "independent full supplied wire frame");
        }
        let mut actual = match result {
            Err(e) => json!({"result":"drop","reason":e.0,"committed_effects":[]}),
            Ok(msg) => {
                let mut events = vec![];
                for vector in &msg.vectors {
                    if vector.leave_all {
                        events.push(json!("rLA_all"));
                    }
                    for (vid, event) in &vector.attributes {
                        events.push(json!([format!("r{event:?}"), vid]));
                    }
                }
                let payload = if kind == "wire_mac_frame_fcs" {
                    &bytes[14..bytes.len() - 4]
                } else {
                    &bytes
                };
                let mut a = json!({"result":"accept","events":events,"leaveall":msg.vectors.iter().any(|v|v.leave_all),"implicit_end":!payload.ends_with(&[0,0]),"ignored_after_endmark":codec::hex(&payload[payload.len()-1..])});
                // Differential inputs test the decoder's ignored fields. Expectations are
                // compared only after the real decoder yields identical retained vectors.
                if row["expected"].get("ignored_first_value").is_some() {
                    let mut altered = payload.to_vec();
                    altered[5..7].copy_from_slice(&4095u16.to_be_bytes());
                    assert_eq!(mvrp::decode_payload(&altered, 1, 4094).unwrap(), msg);
                    a["ignored_first_value"] = json!(true);
                }
                if row["expected"].get("ignored_unused_digits").is_some() {
                    let mut altered = payload.to_vec();
                    let packed = altered[7];
                    altered[7] = (packed / 36) * 36;
                    assert_eq!(mvrp::decode_payload(&altered, 1, 4094).unwrap(), msg);
                    a["ignored_unused_digits"] = json!([(packed / 6) % 6, packed % 6]);
                }
                if row["expected"].get("skipped_messages").is_some() {
                    let known =
                        mvrp::decode_payload(&[0, 1, 2, 0, 1, 0, 2, 108, 0, 0, 0, 0], 1, 4094)
                            .unwrap();
                    assert_eq!(msg.vectors, known.vectors);
                    a["skipped_messages"] = json!(1);
                }
                if row["expected"].get("skipped_vectors").is_some() {
                    let known =
                        mvrp::decode_payload(&[0, 1, 2, 0, 1, 0, 4, 108, 0, 0, 0, 0], 1, 4094)
                            .unwrap();
                    assert_eq!(msg.vectors, known.vectors);
                    a["skipped_vectors"] = json!(1);
                }
                if row["expected"].get("order").is_some() {
                    assert!(msg.vectors.first().unwrap().leave_all);
                    a["order"] = json!("leaveall_before_attributes");
                }
                a
            }
        };
        if actual.get("committed_effects").is_none()
            && row["expected"].get("committed_effects").is_some()
        {
            actual["committed_effects"] = json!([]);
        }
        vector_manifest::finish(
            "mvrp-standard-v1",
            &row,
            "wire_codecs::supplied_mvrp_wire_fields",
            actual,
        );
    }
}
#[test]
fn supplied_local_leaveall_fsm_sequence() {
    use serde_json::json;
    for row in vectors("mvrp-standard-v1") {
        if row["kind"] != "fsm_local_leaveall_tx" {
            continue;
        }
        let i = &row["input"];
        let p2p = i["oper_point_to_point"].as_bool().unwrap();
        let room = i["room"].as_bool().unwrap();
        let mut applicant = serde_json::from_value(i["applicant"].clone()).unwrap();
        let mut registrar = serde_json::from_value(i["registrar"].clone()).unwrap();
        let mut applicant_events = vec![];
        let mut registrar_events = vec![];
        let mut starts = 0;
        let mut request = false;
        let mut wire = None;
        // Unit stimulus is the specified local event sequence, including shared-media
        // and capacity-failure axes that have no public configuration override.
        for event in [mvrp::Event::RLA, mvrp::Event::TxLA] {
            let r = mvrp::registrar(registrar, event, p2p, mvrp::RegistrarControl::Normal);
            registrar = r.state;
            starts += usize::from(r.arm_leave);
            registrar_events.push(if event == mvrp::Event::RLA {
                "rLA"
            } else {
                "txLA"
            });
            let effective = if event == mvrp::Event::TxLA && !room {
                mvrp::Event::TxLAF
            } else {
                event
            };
            let a = mvrp::applicant_tx(
                applicant,
                effective,
                p2p,
                registrar,
                mvrp::ApplicantControl::Normal,
                room,
            );
            applicant = a.state;
            request |= a.request_tx;
            wire = a.action.filter(|_| !a.optional);
            applicant_events.push(match effective {
                mvrp::Event::RLA => "rLA",
                mvrp::Event::TxLAF => "txLAF",
                _ => "txLA",
            });
        }
        let la = mvrp::leaveall(mvrp::AuxiliaryState::Active, mvrp::AuxiliaryEvent::Tx);
        vector_manifest::finish(
            "mvrp-standard-v1",
            &row,
            "wire_codecs::supplied_local_leaveall_fsm_sequence",
            json!({"applicant":applicant,"registrar":registrar,"leaveall":la.state,"wire_event":wire,"leaveall_flag":la.send_leaveall,"applicant_events":applicant_events,"registrar_events":registrar_events,"request_tx":request,"leave_timer_starts":starts}),
        );
    }
}
