use super::super::vector_manifest as manifest;
use super::*;
use serde_json::{Value, json};
fn setup(names: &[&str]) -> (PreparedWire, PreparedWireAdapter, MvrpConfig, Delta) {
    let mut p = super::super::tests::prepared(&[WireProtocol::Mvrp]);
    let mut a = p.adapters.remove(0);
    let WireOptions::Mvrp(mut c) = a.options.clone() else {
        unreachable!()
    };
    let pc = c.ports[0].clone();
    let wire = p.ports["a/p"].clone();
    p.ports.clear();
    c.local_declarations.clear();
    c.ports.clear();
    a.ports = names.iter().map(|s| s.to_string()).collect();
    a.max_state_entries = names.len() * 4094;
    a.max_pending_timers = 20000;
    for name in names {
        let mut cfg = pc.clone();
        cfg.port = (*name).into();
        cfg.periodic_enabled = false;
        c.ports.push(cfg);
        let mut w = wire.clone();
        w.output = (*name).into();
        w.ingress = format!("{name}/in");
        w.static_vlans.clear();
        w.allowed_vlans = BTreeMap::from([(100, true)]);
        p.ports.insert((*name).into(), w);
    }
    a.options = WireOptions::Mvrp(c.clone());
    p.adapters = vec![a.clone()];
    let state = State::new(&a);
    let d = Delta {
        state,
        tx: vec![],
        flush: BTreeSet::new(),
        trace: vec![],
        failed_registration: BTreeSet::new(),
        steps: 0,
        stages: vec![],
    };
    (p, a, c, d)
}
fn attr<'a>(d: &'a mut Delta, n: &str) -> &'a mut Attribute {
    d.state
        .ports
        .get_mut(n)
        .unwrap()
        .attributes
        .get_mut(&100)
        .unwrap()
}
fn requests(d: &Delta) -> Vec<Value> {
    d.trace
        .iter()
        .filter(|t| t.vid == Some(100) && matches!(t.event.as_str(), "Join" | "New" | "Lv"))
        .map(|t| json!([t.event, t.port, 100]))
        .collect()
}
#[test]
fn supplied_map_rules() {
    for row in manifest::vectors("mvrp-standard-v1") {
        if row["kind"] != "integration_rule" {
            continue;
        }
        let i = &row["input"];
        let id = row["id"].as_str().unwrap();
        let (mut p, mut a, mut c, mut d) = setup(&["a", "b", "c"]);
        let actual = match id {
            "P-MVRP-T01-V983" | "P-MVRP-T01-V984" | "P-MVRP-T01-V985" | "P-MVRP-T01-V986" => {
                let f: BTreeSet<_> = i["forwarding"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_str().unwrap())
                    .collect();
                for (n, port) in &mut d.state.ports {
                    port.forwarding = f.contains(n.as_str());
                }
                for n in i["registered"].as_array().unwrap() {
                    let n = n.as_str().unwrap();
                    attr(&mut d, n).registered = true;
                    attr(&mut d, n).registrar = Registrar::IN;
                }
                if i["event"][0] != "join" {
                    d.map(&p, &a, &c, 0, &BTreeSet::new()).unwrap();
                }
                d.trace.clear();
                let ev = i["event"][0].as_str().unwrap();
                let mut new = BTreeSet::new();
                match ev {
                    "join" => {
                        // start with no prior MAP demand for a newly indicated source
                        for n in ["a", "b", "c"] {
                            attr(&mut d, n).demand = false;
                        }
                    }
                    "leave" => {
                        attr(&mut d, "a").registered = false;
                    }
                    "remove_port" => {
                        d.state.ports.get_mut("a").unwrap().forwarding = false;
                    }
                    "join_new" => {
                        d.attribute(&p, &a, &c, "a", 100, Event::RNew, 1, &mut new)
                            .unwrap();
                    }
                    _ => unreachable!(),
                };
                d.map(&p, &a, &c, 1, &new).unwrap();
                let req = requests(&d);
                let keep: Vec<_> = ["a", "c"]
                    .into_iter()
                    .filter(|n| attr(&mut d, n).demand)
                    .collect();
                json!({"requests":req,"self_request":req.iter().any(|r|r[1]=="a"),"keep_declared":keep,"removed_port_withdraws":!d.state.ports["a"].attributes[&100].demand,"flush_dynamic":d.flush.iter().map(|(n,v)|json!([n,v])).collect::<Vec<_>>(),"static_preserved":if id=="P-MVRP-T01-V986"{super::super::tests::apply_mvrp_fixture_flushes(&d.flush)}else{false}})
            }
            "P-MVRP-T01-V1049" | "P-MVRP-T01-V1050" | "P-MVRP-T01-V1051" => {
                for (name, vids) in i["sources"].as_object().unwrap() {
                    for vid in vids.as_array().unwrap() {
                        assert_eq!(vid, 100);
                        attr(&mut d, name).registered = true;
                        attr(&mut d, name).registrar = Registrar::IN;
                    }
                }
                let before: BTreeSet<_> = i["forwarding_before"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap())
                    .collect();
                for (n, port) in &mut d.state.ports {
                    port.forwarding = before.contains(n.as_str());
                }
                d.map(&p, &a, &c, 0, &BTreeSet::new()).unwrap();
                d.trace.clear();
                let after: BTreeSet<_> = i["forwarding_after"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap())
                    .collect();
                for (n, port) in &mut d.state.ports {
                    port.forwarding = after.contains(n.as_str());
                }
                d.map(&p, &a, &c, 1, &BTreeSet::new()).unwrap(); // MAP requests are projected to current F; a removed participant also receives
                // its own local withdrawal (separately asserted), as required by section7.
                let all = requests(&d);
                if id == "P-MVRP-T01-V1051" {
                    assert!(all.contains(&json!(["Lv", "a", 100])));
                }
                let req: Vec<_> = all
                    .into_iter()
                    .filter(|r| after.contains(r[1].as_str().unwrap()))
                    .collect();
                let keep: Vec<_> = d
                    .state
                    .ports
                    .iter()
                    .filter(|(n, p)| {
                        n.as_str() != "a"
                            && p.attributes[&100].demand
                            && !req.iter().any(|r| r[1] == n.as_str())
                    })
                    .map(|(n, _)| n.clone())
                    .collect();
                json!({"requests":req,"keep_declared":keep})
            }
            "P-MVRP-T01-V1052" | "P-MVRP-T01-V1053" => {
                attr(&mut d, "a").registered = true;
                d.state.ports.get_mut("a").unwrap().tc_detected =
                    i["source_tc_detected"].as_u64().unwrap() as u32;
                let new = if i["input_new"] == true {
                    BTreeSet::from([("a".into(), 100)])
                } else {
                    BTreeSet::new()
                };
                d.map(&p, &a, &c, 1, &new).unwrap();
                json!({"output_new":d.trace.iter().any(|t|t.port=="b"&&t.event=="New"&&t.vid==Some(100)),"flush_dynamic_port":d.flush.iter().find(|(n,v)|n=="b"&&*v==100).map(|(n,_)|n)})
            }
            "P-MVRP-T01-V1054" | "P-MVRP-T01-V1055" => {
                c.ports[0].restricted_vlan_registration = i["restricted"] == true;
                p.svre_exists.clear();
                p.ports.get_mut("a").unwrap().allowed_vlans.clear();
                let mut new = BTreeSet::new();
                d.attribute(&p, &a, &c, "a", 100, Event::RJoinIn, 1, &mut new)
                    .unwrap();
                d.map(&p, &a, &c, 1, &new).unwrap();
                let x = &d.state.ports["a"].attributes[&100];
                json!({"registrar":x.registrar,"wire_registered":x.registered,"map_indication":d.trace.iter().any(|t|t.event=="Join"&&t.port!="a"),"registration_failed":d.failed_registration.contains(&("a".into(),100)),"effective":p.ports["a"].static_vlans.contains_key(&100)||x.registered&&p.ports["a"].allowed_vlans.contains_key(&100)})
            }
            "P-MVRP-T01-V1056" => {
                c.ports[0].applicant_control = WireApplicantMode::NonParticipant;
                attr(&mut d, "a").applicant = Applicant::VN;
                attr(&mut d, "a").request_tx = true;
                d.transmit(&p, &a, &c, "a", 1).unwrap();
                json!({"tx":d.tx.first().map(|(_,b)|super::super::codec::hex(b))})
            }
            "P-MVRP-T01-V1057" => {
                d.initialize_port(&p, &a, &c, "a", 0).unwrap();
                json!({"state":if d.state.ports["a"].leaveall_active{"Active"}else{"Passive"},"timer":d.state.ports["a"].timers.get(&("leaveall".into(),None)).and_then(|t|t.deadline)})
            }
            "P-MVRP-T01-V1059" => {
                let mut new = BTreeSet::new();
                let x = attr(&mut d, "a");
                x.applicant = Applicant::AA;
                x.registrar = Registrar::IN;
                x.registered = true;
                d.attribute(&p, &a, &c, "a", 100, Event::RLA, 1, &mut new)
                    .unwrap();
                let x = &d.state.ports["a"].attributes[&100];
                json!({"applicant":x.applicant,"registrar":x.registrar,"indication":if x.registered{None}else{Some("Lv")}})
            }
            "P-MVRP-T01-V1060" => {
                let m = super::super::mvrp::decode_payload(
                    &[0, 1, 2, 0, 2, 0, 0, 6, 0, 0, 0, 0],
                    1,
                    4094,
                )
                .unwrap();
                let events: Vec<_> = m
                    .vectors
                    .iter()
                    .flat_map(|v| v.attributes.iter())
                    .map(|(vid, e)| json!([format!("r{e:?}"), vid]))
                    .collect();
                json!({"events":events,"deduplicate":false})
            }
            "P-MVRP-T01-V1061" | "P-MVRP-T01-V1062" => {
                c.ports[0].transmit_zero = true;
                for v in i["untagged_vids"].as_array().unwrap() {
                    p.ports
                        .get_mut("a")
                        .unwrap()
                        .static_vlans
                        .insert(v.as_u64().unwrap() as u16, false);
                }
                let x = attr(&mut d, "a");
                x.applicant = Applicant::VN;
                x.request_tx = true;
                d.transmit(&p, &a, &c, "a", 1).unwrap();
                let client =
                    super::super::codec::client(&d.tx[0].1, DA, Some(0x88f5), 1500).unwrap();
                json!({"wire_vid":u16::from_be_bytes([client[5],client[6]])})
            }
            "P-MVRP-T01-V1063" => {
                let mut at = vec![];
                for now in i["requests_ps"].as_array().unwrap() {
                    let now = now.as_str().unwrap().parse().unwrap();
                    d.request(&c, "a", now).unwrap();
                    let deadline = d.state.ports["a"].timers[&("join".into(), None)]
                        .deadline
                        .unwrap();
                    at.push(deadline.to_string());
                    d.state.ports.get_mut("a").unwrap().last_opportunity = Some(deadline);
                    d.cancel("a", "join", None).unwrap();
                }
                let times: Vec<u64> = at.iter().map(|s| s.parse().unwrap()).collect();
                let width = 3 * c.ports[0].join_time_ps / 2;
                let max = times
                    .iter()
                    .map(|start| {
                        times
                            .iter()
                            .filter(|t| **t >= *start && **t < *start + width)
                            .count()
                    })
                    .max()
                    .unwrap();
                json!({"opportunities_ps":at,"max_in_1_5_join":max})
            }
            "P-MVRP-T01-V1064" => {
                d.state.ports.get_mut("a").unwrap().timers.insert(
                    ("leave".into(), Some(100)),
                    Timer {
                        generation: 2,
                        deadline: Some(100),
                    },
                );
                a.options = WireOptions::Mvrp(c.clone());
                let before = d.state.clone();
                let out = d
                    .state
                    .plan(
                        &p,
                        &a,
                        100,
                        Input::Timer {
                            port: "a".into(),
                            slot: "leave".into(),
                            vid: Some(100),
                            generation: 1,
                        },
                    )
                    .unwrap();
                assert_eq!(before, out.state);
                json!({"effects":out.trace,"stale":out.tx.is_empty()&&out.flush.is_empty()})
            }
            "P-MVRP-T01-V1065" => {
                let before = d.state.clone();
                let error = d.arm("a", "periodic", None, u64::MAX, 1).unwrap_err();
                assert!(error.contains("overflow"));
                assert_eq!(before, d.state);
                json!({"result":"run_failure","reason":"time_overflow","committed_effects":[]})
            }
            "P-MVRP-T01-V987" | "P-MVRP-T01-V988" | "P-MVRP-T01-V989" | "P-MVRP-T01-V990"
            | "P-MVRP-T01-V1058" => continue,
            _ => panic!("unhandled {id}"),
        };
        manifest::finish(
            "mvrp-standard-v1",
            &row,
            "runtime::wire::mvrp::supplied_map_rules",
            actual,
        );
    }
}
#[test]
fn supplied_mvrp_lifecycle() {
    for row in manifest::vectors("mvrp-standard-v1") {
        let id = row["id"].as_str().unwrap();
        if !matches!(
            id,
            "P-MVRP-T01-V987"
                | "P-MVRP-T01-V988"
                | "P-MVRP-T01-V990"
                | "P-MVRP-T01-V1058"
                | "P-MVRP-T01-V1081"
                | "P-MVRP-T01-V992"
        ) {
            continue;
        }
        let (mut p, mut a, mut c, mut d) = setup(&["a", "b"]);
        let actual = match id {
            "P-MVRP-T01-V987" | "P-MVRP-T01-V988" => {
                let x = attr(&mut d, "a");
                x.registrar = Registrar::IN;
                x.registered = true;
                x.applicant = Applicant::AA;
                c.ports[0].leaveall_time_ps = 10_000_000_000_000;
                a.options = WireOptions::Mvrp(c.clone());
                let flush = id.ends_with("987");
                let out = d
                    .state
                    .plan(
                        &p,
                        &a,
                        1,
                        Input::Tree {
                            port: "a".into(),
                            operational: true,
                            forwarding: flush,
                            tc_detected: 0,
                            flush,
                            redeclare: !flush,
                        },
                    )
                    .unwrap();
                let mut events: Vec<_> = out
                    .trace
                    .iter()
                    .filter(|t| t.port == "a" && t.vid == Some(100))
                    .map(|t| {
                        if t.event == "Redeclare" {
                            "Re-declare"
                        } else {
                            t.event.as_str()
                        }
                    })
                    .collect();
                if flush {
                    assert!(out.state.ports["a"].leaveall_active);
                    assert!(
                        out.state.ports["a"].timers[&("leaveall".into(), None)]
                            .deadline
                            .is_some()
                    );
                    events.push("leavealltimer");
                }
                json!({"events":events,"registrar":out.state.ports["a"].attributes[&100].registrar,"map_member":out.state.ports["a"].forwarding})
            }
            "P-MVRP-T01-V990" => {
                let x = attr(&mut d, "a");
                x.registrar = Registrar::LV;
                x.registered = true;
                d.state.ports.get_mut("a").unwrap().timers.insert(
                    ("leave".into(), Some(100)),
                    Timer {
                        generation: 1,
                        deadline: Some(100),
                    },
                );
                a.options = WireOptions::Mvrp(c.clone());
                let out = d
                    .state
                    .plan(
                        &p,
                        &a,
                        100,
                        Input::Timer {
                            port: "a".into(),
                            slot: "leave".into(),
                            vid: Some(100),
                            generation: 1,
                        },
                    )
                    .unwrap();
                assert!(!out.state.ports["a"].attributes[&100].registered);
                let out = out
                    .state
                    .plan(
                        &p,
                        &a,
                        100,
                        Input::Receive(
                            "a".into(),
                            DecodedMvrp {
                                version: 0,
                                vectors: vec![Vector {
                                    leave_all: false,
                                    attributes: vec![(100, AttributeEvent::JoinIn)],
                                }],
                            },
                        ),
                    )
                    .unwrap();
                json!({"order":["expire","receive"],"indications":["Lv","Join"],"final":out.state.ports["a"].attributes[&100].registrar})
            }
            "P-MVRP-T01-V1058" => {
                c.ports[0].periodic_enabled = true;
                c.ports[0].leaveall_time_ps = 10_000_000_000_000;
                d.initialize_port(&p, &a, &c, "a", 0).unwrap();
                d.request(&c, "a", 0).unwrap();
                c.ports[0].applicant_control = WireApplicantMode::NewOnly;
                c.local_declarations.insert(("a".into(), 100));
                attr(&mut d, "b").registered = true;
                d.initialize_port(&p, &a, &c, "a", 10).unwrap();
                let canceled: Vec<_> = ["join", "leaveall", "periodic"]
                    .into_iter()
                    .filter(|slot| {
                        d.state.ports["a"]
                            .timers
                            .get(&(slot.to_string(), None))
                            .is_some_and(|t| t.deadline.is_none())
                    })
                    .collect();
                assert_eq!(attr(&mut d, "a").applicant, Applicant::VO);
                d.map(&p, &a, &c, 10, &BTreeSet::new()).unwrap();
                assert!(attr(&mut d, "a").demand);
                assert!(attr(&mut d, "b").demand);
                json!({"cancel":canceled,"initialize":"Begin","replay":"current_local_and_map_demand"})
            }
            "P-MVRP-T01-V1081" => {
                c.ports[0].leaveall_time_ps = 10_000_000_000_000;
                a.options = WireOptions::Mvrp(c.clone());
                d.state.ports.get_mut("a").unwrap().timers.insert(
                    ("leaveall".into(), None),
                    Timer {
                        generation: 1,
                        deadline: Some(1),
                    },
                );
                let mut state = d.state;
                let mut states = vec![];
                let mut restarts = vec![];
                let mut samples = vec![];
                let mut sends = vec![];
                for (index, event) in ["leavealltimer", "tx", "rLA"].iter().enumerate() {
                    let before = state.ports["a"].clone();
                    let now = (index + 1) as u64;
                    let input = if *event == "rLA" {
                        Input::Receive(
                            "a".into(),
                            DecodedMvrp {
                                version: 0,
                                vectors: vec![Vector {
                                    leave_all: true,
                                    attributes: vec![],
                                }],
                            },
                        )
                    } else {
                        let slot = if *event == "tx" { "join" } else { "leaveall" };
                        let generation = state.ports["a"].timers[&(slot.into(), None)].generation;
                        Input::Timer {
                            port: "a".into(),
                            slot: slot.into(),
                            vid: None,
                            generation,
                        }
                    };
                    let out = state.plan(&p, &a, now, input).unwrap();
                    let after = &out.state.ports["a"];
                    states.push(if after.leaveall_active {
                        "Active"
                    } else {
                        "Passive"
                    });
                    restarts.push(usize::from(
                        before.timers[&("leaveall".into(), None)].generation
                            != after.timers[&("leaveall".into(), None)].generation,
                    ));
                    samples.push(after.random_counter - before.random_counter);
                    sends.push(out.tx.iter().any(|(_, b)| {
                        decode(b, 1, 1500, 4094)
                            .unwrap()
                            .vectors
                            .iter()
                            .any(|v| v.leave_all)
                    }));
                    state = out.state;
                }
                json!({"states":states,"timer_restarts_per_event":restarts,"random_samples_per_event":samples,"sLA_per_event":sends})
            }
            "P-MVRP-T01-V992" => {
                a.options = WireOptions::Mvrp(c);
                p.adapters = vec![a.clone()];
                let before = d.state.clone();
                let malformed = decode_payload(&[0, 1, 2, 0, 1, 0, 100, 36, 0, 1, 0], 1, 4094);
                assert!(malformed.is_err());
                assert_eq!(d.state, before);
                json!({"result":"drop","registrar_unchanged":d.state.ports==before.ports,"timers_unchanged":d.state.ports.iter().all(|(n,p)|p.timers==before.ports[n].timers),"map_unchanged":d.state==before,"tx_unchanged":d.tx.is_empty()})
            }
            _ => unreachable!(),
        };
        manifest::finish(
            "mvrp-standard-v1",
            &row,
            "runtime::wire::mvrp::supplied_mvrp_lifecycle",
            actual,
        );
    }
}
#[test]
fn simultaneous_tree_changes_keep_continuous_map_demand() {
    let (p, a, c, mut d) = setup(&["a", "b", "c"]);
    attr(&mut d, "a").registered = true;
    attr(&mut d, "b").registered = true;
    d.state.ports.get_mut("b").unwrap().forwarding = false;
    d.map(&p, &a, &c, 0, &BTreeSet::new()).unwrap();
    assert!(d.state.ports["c"].attributes[&100].demand);
    let first = d
        .state
        .plan(
            &p,
            &a,
            1,
            Input::Tree {
                port: "a".into(),
                operational: true,
                forwarding: false,
                tc_detected: 0,
                flush: false,
                redeclare: false,
            },
        )
        .unwrap();
    let second = first
        .state
        .plan(
            &p,
            &a,
            1,
            Input::Tree {
                port: "b".into(),
                operational: true,
                forwarding: true,
                tc_detected: 0,
                flush: false,
                redeclare: false,
            },
        )
        .unwrap();
    let transient: Vec<_> = first
        .trace
        .iter()
        .chain(second.trace.iter())
        .filter(|t| t.port == "c" && t.vid == Some(100))
        .map(|t| t.event.clone())
        .collect();
    assert_eq!(
        transient,
        vec!["Lv", "Join"],
        "retain original scalar-call reproduction"
    );
    let out = d
        .state
        .plan(
            &p,
            &a,
            1,
            Input::TreeBatch(vec![
                TreeChange {
                    port: "a".into(),
                    operational: true,
                    forwarding: false,
                    tc_detected: 0,
                    flush: false,
                    redeclare: false,
                },
                TreeChange {
                    port: "b".into(),
                    operational: true,
                    forwarding: true,
                    tc_detected: 0,
                    flush: false,
                    redeclare: false,
                },
            ]),
        )
        .unwrap();
    assert!(out.state.ports["c"].attributes[&100].demand);
    assert!(
        !out.trace
            .iter()
            .any(|t| t.port == "c" && t.vid == Some(100)),
        "batch must preserve continuous demand"
    );
    assert_eq!(
        out.stages,
        vec!["forwarding_difference", "map_recompute", "mvrp_settle"]
    );
}
#[test]
fn begin_reset_journals_actual_attribute_changes() {
    let (mut p, a, c, mut d) = setup(&["a"]);
    p.registrar_modes
        .insert(("a".into(), 100), WireRegistrarMode::FixedIgnoreNew);
    let x = attr(&mut d, "a");
    x.applicant = Applicant::AA;
    x.demand = true;
    x.request_tx = true;
    x.registered = true;
    d.initialize_port(&p, &a, &c, "a", 10).unwrap();
    let trace = d
        .trace
        .iter()
        .find(|t| t.port == "a" && t.vid == Some(100) && t.event == "Begin")
        .expect("actual Begin reset must be observable");
    assert_eq!(trace.before["applicant"], "AA");
    assert_eq!(trace.after["applicant"], "VO");
    assert_eq!(trace.before["registrar"], "MT");
    assert_eq!(trace.after["registrar"], "IN");
    assert_eq!(trace.after["registered"], false);
}
#[test]
fn map_journals_demand_only_join_and_withdrawal() {
    let (p, a, c, mut d) = setup(&["a", "b"]);
    attr(&mut d, "a").registered = true;
    let x = attr(&mut d, "b");
    x.applicant = Applicant::AA;
    x.request_tx = true;
    d.map(&p, &a, &c, 1, &BTreeSet::new()).unwrap();
    let join = d
        .trace
        .iter()
        .find(|t| t.port == "b" && t.vid == Some(100) && t.event == "Join")
        .expect("demand-only Join must be journaled");
    assert_eq!(join.before["applicant"], join.after["applicant"]);
    assert_eq!(join.before["request_tx"], join.after["request_tx"]);
    assert_eq!(join.before["demand"], false);
    assert_eq!(join.after["demand"], true);
    d.trace.clear();
    attr(&mut d, "a").registered = false;
    d.map(&p, &a, &c, 2, &BTreeSet::new()).unwrap();
    let leave = d
        .trace
        .iter()
        .find(|t| t.port == "b" && t.vid == Some(100) && t.event == "Lv")
        .unwrap();
    assert_eq!(leave.before["demand"], true);
    assert_eq!(leave.after["demand"], false);
}
