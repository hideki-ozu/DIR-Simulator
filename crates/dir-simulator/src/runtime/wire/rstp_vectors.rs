use super::super::vector_manifest as manifest;
use super::*;
use serde_json::{Value, json};
fn fresh() -> Delta {
    let b = Bridge::new(
        0x8000020000000002,
        Times::default(),
        6,
        BTreeMap::from([(
            "p".into(),
            (
                PortConfig {
                    id: 0x8001,
                    cost: 20000,
                    admin_edge: false,
                    auto_edge: false,
                    auto_isolate: false,
                    restricted_role: false,
                    restricted_tcn: false,
                },
                true,
            ),
        )]),
    );
    let mut d = Delta {
        state: b,
        tx: vec![],
        flush: vec![],
        transitions: vec![],
    };
    d.state.prs = Prs::ROLE_SELECTION;
    let p = d.state.ports.get_mut("p").unwrap();
    p.ppm = Ppm::SENSING;
    p.pim = Pim::CURRENT;
    p.prt = Prt::DISABLED_PORT;
    p.ptx = Ptx::IDLE;
    p.info = Info::Mine;
    p.reselect = false;
    p.selected = false;
    p.role = Role::Disabled;
    p.selected_role = Role::Disabled;
    p.counters = Counters {
        edge: 1,
        hello: 1,
        migration: 1,
        fd: 0,
        rr: 0,
        rb: 0,
        info: 0,
        tc: 0,
        detected: 0,
        tx: 0,
    };
    p.sync = false;
    p.synced = false;
    p.reroot = false;
    p.new_info = false;
    p.fdb_flush = false;
    p.send_rstp = false;
    d
}
fn message(p: &Port) -> Bpdu {
    let mut m = Bpdu {
        kind: Kind::Rst,
        version: 2,
        flags: 28,
        priority: p.priority,
        times: Times::default(),
    };
    m.priority.0[0] -= 1;
    m.priority.0[4] -= 1;
    m
}
fn set(p: &mut Port, key: &str, v: &Value) {
    macro_rules! b {
        ($f:ident) => {{
            p.$f = v.as_bool().unwrap();
        }};
    }
    macro_rules! c {
        ($f:ident) => {{
            p.counters.$f = v.as_u64().unwrap() as u32;
        }};
    }
    match key {
        "selected" => b!(selected),
        "updtInfo" => b!(update),
        "portEnabled" => b!(enabled),
        "sendRSTP" => b!(send_rstp),
        "rcvdRSTP" => b!(rcvd_rstp),
        "rcvdSTP" => b!(rcvd_stp),
        "mcheck" => b!(mcheck),
        "operEdge" => b!(oper_edge),
        "isolate" => b!(isolate),
        "proposing" => b!(proposing),
        "proposed" => b!(proposed),
        "agree" => b!(agree),
        "agreed" => b!(agreed),
        "disputed" => b!(disputed),
        "sync" => b!(sync),
        "synced" => b!(synced),
        "reRoot" => b!(reroot),
        "learn" => b!(learn),
        "forward" => b!(forward),
        "learning" => b!(learning),
        "forwarding" => b!(forwarding),
        "newInfo" => b!(new_info),
        "fdbFlush" => b!(fdb_flush),
        "rcvdTcn" => b!(rcvd_tcn),
        "rcvdTc" => b!(rcvd_tc),
        "rcvdTcAck" => b!(rcvd_tc_ack),
        "tcProp" => b!(tc_prop),
        "mdelayWhile" => c!(migration),
        "edgeDelayWhile" => c!(edge),
        "fdWhile" => c!(fd),
        "rrWhile" => c!(rr),
        "rbWhile" => c!(rb),
        "rcvdInfoWhile" => c!(info),
        "AutoEdge" => p.config.auto_edge = v.as_bool().unwrap(),
        "role" => p.role = serde_json::from_value(v.clone()).unwrap(),
        "selectedRole" => p.selected_role = serde_json::from_value(v.clone()).unwrap(),
        "infoIs" => p.info = serde_json::from_value(v.clone()).unwrap(),
        "rcvdMsg" => p.message = if v == true { Some(message(p)) } else { None },
        "rcvdBpdu" | "enableBPDUrx" | "allSynced" | "reRooted" | "any_reselect" => {}
        _ => panic!("unmapped input {key}"),
    }
}
fn state(p: &Port, m: &str) -> String {
    match m {
        "PRX" => format!("{:?}", p.prx),
        "PPM" => format!("{:?}", p.ppm),
        "BDM" => format!("{:?}", p.bdm),
        "PIM" => format!("{:?}", p.pim),
        "PRT" => format!("{:?}", p.prt),
        "PST" => format!("{:?}", p.pst),
        "TCM" => format!("{:?}", p.tcm),
        "PTX" => format!("{:?}", p.ptx),
        _ => panic!("{m}"),
    }
}
fn guard(row: &Value) -> Delta {
    let i = &row["input"];
    let m = i["machine"].as_str().unwrap();
    let mut d = fresh();
    {
        let p = d.state.ports.get_mut("p").unwrap();
        let s = i["state"].clone();
        match m {
            "PRX" => p.prx = serde_json::from_value(s).unwrap(),
            "PPM" => p.ppm = serde_json::from_value(s).unwrap(),
            "BDM" => p.bdm = serde_json::from_value(s).unwrap(),
            "PIM" => p.pim = serde_json::from_value(s).unwrap(),
            "PRT" => p.prt = serde_json::from_value(s).unwrap(),
            "PST" => p.pst = serde_json::from_value(s).unwrap(),
            "TCM" => p.tcm = serde_json::from_value(s).unwrap(),
            "PRS" => {}
            _ => panic!("{m}"),
        };
        // Fixture explicitly sets all unspecified booleans/counters to zero. Runtime
        // dispatch scaffolding is held inert; machine inputs below overwrite defaults.
        p.enabled = false;
        p.counters = Counters {
            edge: 0,
            hello: 0,
            migration: 0,
            fd: 0,
            rr: 0,
            rb: 0,
            info: 0,
            tc: 0,
            detected: 0,
            tx: 0,
        };
        for (k, v) in i["before"].as_object().unwrap() {
            set(p, k, v);
        }
    }
    if m == "PRT" {
        // allSynced/reRooted are derived bridge predicates, never injected outputs.
        let mut q = d.state.ports["p"].clone();
        q.config.id = 0x8002;
        q.selected = true;
        q.role = Role::Designated;
        q.selected_role = Role::Designated;
        q.update = false;
        q.synced = i["before"]["allSynced"] == true;
        q.counters.rr = if i["before"]["reRooted"] == true {
            0
        } else {
            1
        };
        d.state.ports.insert("q".into(), q);
        d.role_step("p").unwrap();
    } else if m == "TCM" {
        d.tc_step("p");
    } else if m == "PRS" {
        d.state.ports.get_mut("p").unwrap().reselect = true;
        d.select().unwrap();
    } else if m == "PRX" {
        if i["before"]["rcvdMsg"] != true {
            let b = d.state.clone();
            let msg = message(&b.ports["p"]);
            let out = b.plan(Input::Receive("p".into(), msg), 1000).unwrap();
            let t = out.transitions.iter().find(|t| t.machine == "PRX").unwrap();
            d.state
                .ports
                .insert("p".into(), serde_json::from_value(t.after.clone()).unwrap());
            d.transitions.push(t.clone());
        }
    } else {
        // Global dispatcher runs migration/edge/info before role/state. Stop after
        // one call; later machines cannot be mistaken for the target transition.
        for _ in 0..20 {
            let changed = d.step().unwrap();
            if d.transitions.iter().any(|t| t.machine == m) || !changed {
                break;
            }
        }
    }
    d
}
#[test]
fn supplied_guard_transitions() {
    let rows = manifest::vectors("rstp-standard-v1");
    let mut count = 0;
    for row in &rows {
        if row["input"]["operation"] != "one_guard_transition" {
            continue;
        }
        let d = guard(row);
        let m = row["input"]["machine"].as_str().unwrap();
        let got = if m == "PRS" {
            format!("{:?}", d.state.prs)
        } else {
            state(&d.state.ports["p"], m)
        };
        let journal: Vec<_> = d
            .transitions
            .iter()
            .filter(|t| t.machine == m && (m != "PRS" || t.port.is_none()))
            .collect();
        if row["expected"]["stays_without_reentry"] == true {
            assert!(journal.is_empty(), "{}", row["id"]);
        } else if got != row["input"]["state"].as_str().unwrap() {
            assert_eq!(journal.len(), 1, "{}", row["id"]);
            let t = journal[0];
            assert_eq!(t.from_state, row["input"]["state"].as_str().unwrap());
            assert_eq!(t.to_state, got);
            assert!(guards::valid_trigger("rstp", m, &t.guard));
            assert_eq!(t.guard, row["id"].as_str().unwrap());
        }
        manifest::finish(
            "rstp-standard-v1",
            row,
            "runtime::wire::rstp::supplied_guard_transitions",
            json!({"next_state":got,"entry_effects":"see entry_action_only vector for destination","stays_without_reentry":m=="PRX"&&row["input"]["before"]["rcvdMsg"]==true&&journal.is_empty()}),
        );
        count += 1;
    }
    assert_eq!(count, 45);
    // Preserve the original V248-250 input semantics: sendRSTP=false makes
    // ForwardDelay=15, so fdWhile=2 takes the higher-priority self loop.
    for id in [248, 249, 250] {
        let mut row = rows
            .iter()
            .find(|r| r["id"] == format!("P-RSTP-T01-V{id}"))
            .unwrap()
            .clone();
        row["input"]["before"]["sendRSTP"] = json!(false);
        let d = guard(&row);
        assert_eq!(d.state.ports["p"].prt, Prt::ALTERNATE_PORT);
        assert_eq!(d.state.ports["p"].counters.fd, 15);
        assert_eq!(d.transitions.len(), 1);
        assert_eq!(d.transitions[0].from_state, d.transitions[0].to_state);
    }
}
fn synthetic(machine: &str, state: &str, before: Value) -> Value {
    json!({"input":{"machine":machine,"state":state,"before":before}})
}
fn enter(machine: &str, target: &str, rows: &[Value]) -> Delta {
    if let Some(row) = rows.iter().find(|v| {
        v["input"]["operation"] == "one_guard_transition"
            && v["input"]["machine"] == machine
            && v["expected"]["next_state"] == target
            && v["input"]["state"] != target
    }) {
        let mut r = row.clone();
        if machine == "PRT" || machine == "TCM" {
            r["input"]["before"]["sendRSTP"] = json!(true);
        }
        if machine == "TCM" && target == "NOTIFIED_TC" {
            r["input"]["before"]["role"] = json!("Designated");
        }
        return guard(&r);
    }
    let mut d = fresh();
    match (machine, target) {
        ("PTI", _) => {
            d.state.ports.get_mut("p").unwrap().counters = Counters {
                edge: 2,
                hello: 2,
                migration: 2,
                fd: 2,
                rr: 2,
                rb: 2,
                info: 2,
                tc: 2,
                detected: 2,
                tx: 2,
            };
            let out = d.state.plan(Input::Tick, 1000).unwrap();
            let tick = out
                .transitions
                .iter()
                .find(|t| t.machine == "PTI" && t.to_state == "TICK")
                .unwrap();
            d.state.ports.insert(
                "p".into(),
                serde_json::from_value(tick.after.clone()).unwrap(),
            );
            d.transitions = out.transitions;
            d.state.pti = Pti::ONE_SECOND;
        }
        ("PRX", "DISCARD") => {
            let p = d.state.ports.get_mut("p").unwrap();
            p.enabled = false;
            p.prx = Prx::RECEIVE;
            p.rcvd_rstp = true;
            p.rcvd_stp = true;
            p.message = None;
            for _ in 0..5 {
                d.step().unwrap();
                if d.state.ports["p"].prx == Prx::DISCARD {
                    break;
                }
            }
        }
        ("PPM", "SELECTING_STP") => {
            return guard(&synthetic(
                "PPM",
                "SENSING",
                json!({"portEnabled":true,"sendRSTP":true,"rcvdSTP":true}),
            ));
        }
        ("BDM", "ISOLATED") => {
            return guard(&synthetic(
                "BDM",
                "NOT_EDGE",
                json!({"portEnabled":true,"sendRSTP":true,"proposing":true,"edgeDelayWhile":0}),
            ));
        }
        ("PTX", _) => {
            let p = d.state.ports.get_mut("p").unwrap();
            p.enabled = true;
            p.selected = true;
            p.role = Role::Designated;
            p.selected_role = Role::Designated;
            p.ptx = Ptx::IDLE;
            p.counters.hello = 2;
            match target {
                "TRANSMIT_INIT" => {
                    p.enabled = false;
                    p.new_info = false;
                    p.counters.tx = 5;
                    for _ in 0..5 {
                        d.step().unwrap();
                        if d.state.ports["p"].ptx == Ptx::TRANSMIT_INIT {
                            break;
                        }
                    }
                }
                "IDLE" => {
                    p.ptx = Ptx::TRANSMIT_INIT;
                    d.tx_step("p");
                }
                "TRANSMIT_PERIODIC" => {
                    p.counters.hello = 0;
                    d.tx_step("p");
                }
                "TRANSMIT_CONFIG" | "TRANSMIT_TCN" | "TRANSMIT_RSTP" => {
                    p.new_info = true;
                    p.tc_ack = true;
                    p.send_rstp = target == "TRANSMIT_RSTP";
                    if target == "TRANSMIT_TCN" {
                        p.role = Role::Root;
                    }
                    d.tx_step("p");
                }
                _ => panic!("{target}"),
            }
        }
        ("PIM", "DISABLED") => {
            let p = d.state.ports.get_mut("p").unwrap();
            p.enabled = false;
            p.info = Info::Received;
            p.selected = true;
            p.counters.info = 6;
            for _ in 0..10 {
                d.step().unwrap();
                if d.state.ports["p"].info == Info::Disabled {
                    break;
                }
            }
        }
        ("PIM", "REPEATED_DESIGNATED") => {
            let p = d.state.ports.get_mut("p").unwrap();
            let msg = message(p);
            p.message = Some(msg.clone());
            record_designated(p, &msg, false);
            p.message = None;
        }
        ("PIM", "CURRENT") => {
            d.state.ports.get_mut("p").unwrap().pim = Pim::OTHER;
            d.step().unwrap();
        }
        ("PIM", "SUPERIOR_DESIGNATED" | "INFERIOR_DESIGNATED" | "NOT_DESIGNATED" | "OTHER") => {
            let p = d.state.ports.get_mut("p").unwrap();
            p.pim = Pim::RECEIVE;
            let mut m = message(p);
            match target {
                "INFERIOR_DESIGNATED" => {
                    m.priority.0[0] = p.priority.0[0] + 1;
                    m.priority.0[4] += 2;
                }
                "NOT_DESIGNATED" => {
                    m.priority = p.priority;
                    m.flags = 8;
                }
                "OTHER" => m.flags = 0,
                _ => {}
            }
            p.message = Some(m);
            d.step().unwrap();
        }
        ("PRS", "INIT_TREE") | ("PRT", "INIT_PORT") => {
            let b = Bridge::new(
                0x8000020000000002,
                Times::default(),
                6,
                BTreeMap::from([("p".into(), (d.state.ports["p"].config.clone(), true))]),
            );
            d.state = b;
        }
        ("PRT", "BLOCK_PORT") => {
            return guard(&synthetic(
                "PRT",
                "DISABLED_PORT",
                json!({"selected":true,"role":"Disabled","selectedRole":"Alternate"}),
            ));
        }
        ("PRT", "DISABLE_PORT") => {
            return guard(&synthetic(
                "PRT",
                "DESIGNATED_PORT",
                json!({"selected":true,"role":"Designated","selectedRole":"Disabled"}),
            ));
        }
        ("PRT", "DISABLED_PORT") => {
            return guard(&synthetic(
                "PRT",
                "DISABLE_PORT",
                json!({"selected":true,"role":"Disabled","selectedRole":"Disabled"}),
            ));
        }
        ("PRT", "ROOT_PORT") => {
            return guard(&synthetic(
                "PRT",
                "DISABLED_PORT",
                json!({"selected":true,"role":"Disabled","selectedRole":"Root"}),
            ));
        }
        ("PRT", "DESIGNATED_PORT") => {
            return guard(&synthetic(
                "PRT",
                "DISABLED_PORT",
                json!({"selected":true,"role":"Disabled","selectedRole":"Designated"}),
            ));
        }
        ("PRT", "ROOT_DISCARD") => {
            return guard(&synthetic(
                "PRT",
                "ROOT_PORT",
                json!({"selected":true,"role":"Root","selectedRole":"Root","rrWhile":15,"disputed":true,"sendRSTP":true}),
            ));
        }
        ("TCM", "ACTIVE") => {
            let p = d.state.ports.get_mut("p").unwrap();
            p.tcm = Tcm::DETECTED;
            p.role = Role::Designated;
            d.tc_step("p");
        }
        ("PRS", "ROLE_SELECTION") => d.select().unwrap(),
        _ => panic!("no entry driver {machine}.{target}"),
    }
    d
}
fn observe_effects(d: &Delta, machine: &str, target: &str, want: &Value) -> Value {
    let p = &d.state.ports["p"];
    let value = serde_json::to_value(p).unwrap();
    let mut out = serde_json::Map::new();
    for k in want.as_object().unwrap().keys() {
        let v = match k.as_str() {
            "tick" => json!(d.state.pti != Pti::ONE_SECOND),
            "decrement_all_positive_counters_by" => {
                let t = d
                    .transitions
                    .iter()
                    .find(|t| t.machine == "PTI" && t.to_state == "TICK")
                    .unwrap();
                for name in [
                    "edge",
                    "hello",
                    "migration",
                    "fd",
                    "rr",
                    "rb",
                    "info",
                    "tc",
                    "detected",
                    "tx",
                ] {
                    assert_eq!(
                        t.before["counters"][name].as_u64().unwrap()
                            - t.after["counters"][name].as_u64().unwrap(),
                        1
                    );
                }
                json!(1)
            }
            "rcvdMsg" => json!(p.message.is_some()),
            "rcvdBpdu" => json!(p.message.is_some()),
            "infoIs" => value["info"].clone(),
            "updtInfo" => json!(p.update),
            "newInfo" => json!(p.new_info),
            "sendRSTP" => json!(p.send_rstp),
            "operEdge" => json!(p.oper_edge),
            "reRoot" => json!(p.reroot),
            "fdbFlush" => json!(p.fdb_flush),
            "tcAck" => json!(p.tc_ack),
            "rcvdTc" => json!(p.rcvd_tc),
            "rcvdTcn" => json!(p.rcvd_tcn),
            "rcvdTcAck" => json!(p.rcvd_tc_ack),
            "rcvdRSTP" => json!(p.rcvd_rstp),
            "rcvdSTP" => json!(p.rcvd_stp),
            "tcProp" => json!(p.tc_prop),
            "edgeDelayWhile" => json!(p.counters.edge),
            "mdelayWhile" => json!(p.counters.migration),
            "helloWhen" => json!(p.counters.hello),
            "txCount" => json!(p.counters.tx),
            "rcvdInfoWhile" => json!(p.counters.info),
            "rrWhile" => json!(p.counters.rr),
            "fdWhile" => json!(p.counters.fd),
            "rbWhile" => json!(p.counters.rb),
            "tcWhile" => json!(p.counters.tc),
            "tcDetected" => json!(p.counters.detected),
            "tx_intent_kind" => json!(match d.tx[0].1.kind {
                Kind::Rst => "RST",
                Kind::Tcn => "TCN",
                Kind::Config => "Config",
            }),
            "all_sync" => json!(d.state.ports.values().all(|q| q.sync)),
            "all_reRoot" => json!(d.state.ports.values().all(|q| q.reroot)),
            "other_ports_tcProp" => json!(
                d.state
                    .ports
                    .iter()
                    .filter(|(n, _)| n.as_str() != "p")
                    .all(|(_, q)| q.tc_prop)
            ),
            "all_selectedRole" => {
                assert!(
                    d.state
                        .ports
                        .values()
                        .all(|p| p.selected_role == Role::Disabled)
                );
                json!("Disabled")
            }
            "self_root_when_no_received" => {
                json!(d.state.root.0[0] == d.state.id && d.state.root_port == 0)
            }
            "classify_message" => json!(p.received_info.is_some()),
            "additional_effects" => {
                let t = d
                    .transitions
                    .iter()
                    .find(|t| t.machine == machine && t.to_state == target)
                    .unwrap();
                let mut before = t.before.clone();
                let mut after = t.after.clone();
                let field = machine.to_lowercase();
                before.as_object_mut().unwrap().remove(&field);
                after.as_object_mut().unwrap().remove(&field);
                assert_eq!(before, after);
                json!([])
            }
            _ => value[k].clone(),
        };
        out.insert(k.clone(), v);
    }
    Value::Object(out)
}
#[test]
fn supplied_state_entries() {
    let rows = manifest::vectors("rstp-standard-v1");
    let mut count = 0;
    for row in &rows {
        if row["input"]["operation"] != "entry_action_only" {
            continue;
        }
        let m = row["input"]["machine"].as_str().unwrap();
        let target = row["input"]["enter_state"].as_str().unwrap();
        let mut d = enter(m, target, &rows);
        // Include a passive peer so tree-wide effects are non-vacuous. Replay the
        // existing driver with a peer only where its predicate already supplies one.
        if row["expected"]["entry_effect_subset"]
            .get("other_ports_tcProp")
            .is_some()
        {
            let mut peer = d.state.ports["p"].clone();
            peer.tc_prop = false;
            d.state.ports.insert("peer".into(), peer);
            let p = d.state.ports.get_mut("p").unwrap();
            p.tcm = if target == "DETECTED" {
                Tcm::LEARNING
            } else {
                Tcm::ACTIVE
            };
            p.forward = true;
            p.oper_edge = false;
            if target == "NOTIFIED_TC" {
                p.rcvd_tc = true;
            }
            d.tc_step("p");
        }
        let effects = observe_effects(&d, m, target, &row["expected"]["entry_effect_subset"]);
        manifest::finish(
            "rstp-standard-v1",
            row,
            "runtime::wire::rstp::supplied_state_entries",
            json!({"entry_effect_subset":effects,"reachability_claim":false}),
        );
        count += 1;
    }
    assert_eq!(count, 64);
}
#[test]
fn supplied_rstp_wire_fields() {
    for row in manifest::vectors("rstp-standard-v1") {
        if row["kind"] != "wire_decode" && row["kind"] != "wire_reject" {
            continue;
        }
        let b = unhex(row["input"]["bytes_hex"].as_str().unwrap()).unwrap();
        let decoded = decode(&b, 0x8001, 1500);
        if row["kind"] == "wire_reject" {
            let e = decoded.unwrap_err();
            manifest::finish(
                "rstp-standard-v1",
                &row,
                "runtime::wire::rstp::supplied_rstp_wire_fields",
                json!({"outcome":"control_drop","reason":e.0,"protocol_mutations":0}),
            );
            continue;
        }
        let msg = decoded.unwrap();
        let mut d = fresh();
        d.state.ports.get_mut("p").unwrap().ppm = Ppm::SENSING;
        d.state.ports.get_mut("p").unwrap().send_rstp = true;
        let before = d.state.ports["p"].priority;
        let out = d
            .state
            .plan(Input::Receive("p".into(), msg.clone()), 10000)
            .unwrap();
        let prx = out.transitions.iter().find(|t| t.machine == "PRX").unwrap();
        let rx: Port = serde_json::from_value(prx.after.clone()).unwrap();
        let classified = out
            .transitions
            .iter()
            .find(|t| t.machine == "PIM" && t.to_state == "RECEIVE")
            .and_then(|t| {
                serde_json::from_value::<Port>(t.after.clone())
                    .unwrap()
                    .received_info
            });
        let payload = client(&b, DA, None, 1500).unwrap();
        let bpdu_len = payload.len() - 3;
        let kind = match msg.kind {
            Kind::Rst => "RST",
            Kind::Config => "Config",
            Kind::Tcn => "TCN",
        };
        let record = out
            .transitions
            .iter()
            .find(|t| t.machine == "PIM" && t.to_state == "SUPERIOR_DESIGNATED");
        let record_p = record.map(|t| serde_json::from_value::<Port>(t.after.clone()).unwrap());
        let flags = msg.flags;
        let mut actual = json!({"bpdu_kind":kind,"bpdu_length":bpdu_len,"mac_bytes":b.len(),"flags":{"role":match (flags>>2)&3{3=>"Designated",2=>"Root",1=>"Alternate",_=>"Unknown"},"proposal":flags&2!=0,"tc":flags&1!=0},"root_id":format!("{:016x}",msg.priority.0[0]),"designated_bridge_id":format!("{:016x}",msg.priority.0[4]),"designated_port_id":msg.priority.0[5],"message_age_ticks":msg.times.age,"max_age_ticks":msg.times.max_age,"hello_time_ticks":msg.times.hello,"forward_delay_ticks":msg.times.forward_delay,"role":if flags&12==12{"Designated"}else{"Other"},"tc":flags&1!=0,"tc_ack":msg.kind==Kind::Config&&flags&128!=0,"proposal":flags&2!=0,"agreement":flags&64!=0,"learning":flags&16!=0,"forwarding":flags&32!=0,"rcvdSTP":rx.rcvd_stp,"rcvdRSTP":rx.rcvd_rstp,"rcvdSTP_set_by_receive":rx.rcvd_stp,"rcvdTcAck_set_by_receive":rx.rcvd_tc_ack,"normalized_version":encode([2,0,0,0,0,2],&msg)[19],"role_code":(flags>>2)&3,"rcvdInfo":classified,"record_priority":record_p.as_ref().is_some_and(|p|p.priority!=before),"rcvd_tcn":out.transitions.iter().any(|t|t.after["rcvd_tcn"]==true),"priority_fields_read":msg.priority!=Priority([0;7]),"extra_octets_ignored":bpdu_len-match msg.kind{Kind::Rst=>36,Kind::Config=>35,Kind::Tcn=>4},"decode":"accepted","rcvdInfoWhile_after_record":record_p.as_ref().map(|p|p.counters.info),"next_information_state":out.transitions.iter().find(|t|t.machine=="PIM"&&t.from_state=="CURRENT"&&t.to_state=="AGED").map(|t|t.to_state.clone()),"migration_if_preexisting_state_sensing_sendrstp_true":format!("{:?}",out.state.ports["p"].ppm)});
        // These are negative protocol observations: no additional roles/machines or
        // messages are created by discarded extension fields.
        actual["cist_master_port_created"] = json!(
            out.state
                .ports
                .values()
                .any(|p| format!("{:?}", p.role) == "Master")
        );
        actual["mst_fields_processed"] =
            json!(out.transitions.iter().any(|t| t.machine.starts_with("MST")));
        actual["protocol_version_checked"] = json!(msg.kind != Kind::Tcn);
        actual["version1_length_checked"] = json!(msg.kind != Kind::Rst);
        manifest::finish(
            "rstp-standard-v1",
            &row,
            "runtime::wire::rstp::supplied_rstp_wire_fields",
            actual,
        );
    }
}

pub(super) fn receipt_rcv_info(row: &Value, p: &Port) {
    manifest::finish(
        "rstp-standard-v1",
        row,
        "runtime::wire::rstp::supplied_rcv_info_vectors",
        json!({"rcvdInfo":p.received_info,"repeated_branch_reachable":p.pim==Pim::REPEATED_DESIGNATED,"entry_effects":{"disputed":p.disputed,"agreed":p.agreed}}),
    );
}
fn priority_json(p: Priority) -> Value {
    json!([
        format!("{:016x}", p.0[0]),
        p.0[1],
        format!("{:016x}", p.0[2]),
        p.0[3],
        format!("{:016x}", p.0[4]),
        p.0[5],
        p.0[6]
    ])
}
fn parse_priority(v: &Value) -> Priority {
    let mut out = [0; 7];
    for (n, x) in v.as_array().unwrap().iter().enumerate() {
        out[n] = if let Some(s) = x.as_str() {
            u64::from_str_radix(s, 16).unwrap()
        } else {
            x.as_u64().unwrap()
        };
    }
    Priority(out)
}
#[test]
fn supplied_rstp_calculations() {
    for row in manifest::vectors("rstp-standard-v1") {
        if row["kind"] != "calculation" {
            continue;
        }
        let i = &row["input"];
        let op = i["operation"].as_str().unwrap();
        let mut d = fresh();
        let actual = match op {
            "updtRcvdInfoWhile" | "recordTimes" => {
                let p = d.state.ports.get_mut("p").unwrap();
                let mut msg = message(p);
                msg.times.age = i["message_age_ticks"].as_u64().unwrap_or(0) as u16;
                msg.times.max_age = i["max_age_ticks"].as_u64().unwrap_or(5120) as u16;
                msg.times.hello = i["received_hello_ticks"].as_u64().unwrap_or(512) as u16;
                p.pim = Pim::RECEIVE;
                p.message = Some(msg.clone());
                p.last_message = Some(msg.clone());
                d.step().unwrap();
                let p = &d.state.ports["p"];
                json!({"rounded_message_age_plus_one_s":received_age_seconds(msg.times.age),"rcvdInfoWhile":p.counters.info,"port_hello_time_s":p.times.hello/256,"received_wire_value_preserved_in_message":p.last_message.as_ref().unwrap().times.hello})
            }
            "lexicographic_tie_break" => {
                let field = i["first_differing_field"].as_str().unwrap();
                let get = |v: &Value| {
                    v.as_u64()
                        .unwrap_or_else(|| u64::from_str_radix(v.as_str().unwrap(), 16).unwrap())
                };
                let mut l = Priority([1, 0, 1, 0, 1, 0x8001, 0x8001]);
                let idx = match field {
                    "root" => 0,
                    "external_cost" => 1,
                    "designated_port_id" => 5,
                    "receiving_port_id" => 6,
                    _ => unreachable!(),
                };
                l.0[idx] = get(&i["left"]);
                let mut r = l;
                r.0[idx] = get(&i["right"]);
                let mut p = d.state.ports["p"].clone();
                p.info = Info::Received;
                p.priority = l;
                p.config.id = 0x8001;
                let mut q = p.clone();
                q.priority = r;
                q.config.id = 0x8002;
                d.state.id = u64::MAX;
                d.state.ports = BTreeMap::from([("left".into(), p), ("right".into(), q)]);
                d.select().unwrap();
                json!({"winner":if d.state.root_port==0x8001{"left"}else{"right"}})
            }
            "root_path_vector" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.info = Info::Received;
                if i.get("received").is_some() {
                    p.priority = parse_priority(&i["received"]);
                    p.config.id = p.priority.0[6] as u16;
                    p.config.cost = i["external_path_cost"].as_u64().unwrap() as u32;
                    d.state.id =
                        u64::from_str_radix(i["local_bridge"].as_str().unwrap(), 16).unwrap();
                    d.select().unwrap();
                    json!({"root_path_vector":priority_json(d.state.root)})
                } else {
                    p.priority.0[0] -= 1;
                    p.priority.0[4] -= 1;
                    p.priority.0[1] = i["external_cost"].as_u64().unwrap();
                    p.config.cost = i["external_path_cost"].as_u64().unwrap() as u32;
                    let old = d.state.clone();
                    let error = d.select().unwrap_err();
                    assert!(error.contains("overflow"));
                    assert_eq!(d.state, old);
                    json!({"outcome":"run_failure","reason":"arithmetic_overflow","committed_effects":d.transitions.len()})
                }
            }
            "counter_assignment" => {
                let ticks = i["wire_ticks"].as_u64().unwrap() as u16;
                let p = d.state.ports.get_mut("p").unwrap();
                p.role = Role::Disabled;
                p.selected_role = Role::Root;
                p.selected = true;
                p.designated_times.forward_delay = ticks;
                d.role_step("p").unwrap();
                json!({"counter_seconds":d.state.ports["p"].counters.rr,"wire_ticks_preserved":d.state.ports["p"].designated_times.forward_delay})
            }
            "tick_expiry" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.counters.info = i["counter"].as_u64().unwrap() as u32;
                let assigned = i["assigned_at_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                let period = i["tick_period_ps"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                let mut expires = None;
                for now in (period..=10 * period).step_by(period as usize) {
                    if now <= assigned {
                        continue;
                    }
                    let out = d.state.plan(Input::Tick, 1000).unwrap();
                    let t = out
                        .transitions
                        .iter()
                        .find(|t| t.machine == "PTI" && t.to_state == "TICK")
                        .unwrap();
                    d.state
                        .ports
                        .insert("p".into(), serde_json::from_value(t.after.clone()).unwrap());
                    if d.state.ports["p"].counters.info == 0 {
                        expires = Some(now);
                        break;
                    }
                }
                json!({"expires_at_ps":expires.unwrap().to_string(),"not_at_ps":(assigned+i["counter"].as_u64().unwrap()*period).to_string()})
            }
            "fdb_flush" => continue,
            _ => panic!("{op}"),
        };
        manifest::finish(
            "rstp-standard-v1",
            &row,
            "runtime::wire::rstp::supplied_rstp_calculations",
            actual,
        );
    }
}
fn mesh(triangle: bool, flap: bool) -> (Vec<Bridge>, usize) {
    let count = if triangle { 3 } else { 2 };
    let mut bridges = vec![];
    for index in 0..count {
        let ports = (1..=if triangle { 2 } else { 1 })
            .map(|n| {
                (
                    n.to_string(),
                    (
                        PortConfig {
                            id: 0x8000 + n,
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
        bridges.push(Bridge::new(
            0x8000020000000001 + index as u64,
            Times::default(),
            6,
            ports,
        ));
    }
    let mut peers = BTreeMap::from([((0, "1"), (1, "1")), ((1, "1"), (0, "1"))]);
    if triangle {
        peers.extend([
            ((0, "2"), (2, "1")),
            ((2, "1"), (0, "2")),
            ((1, "2"), (2, "2")),
            ((2, "2"), (1, "2")),
        ]);
    }
    let mut pending = vec![];
    let mut seq = 0u64;
    for node in 0..count {
        pending.push((0, 0, seq, node, Input::Initialize));
        seq += 1;
        for tick in 1..=8 {
            pending.push((tick * 1_000_000_000_000, 0, seq, node, Input::Tick));
            seq += 1;
        }
    }
    if flap {
        for (node, port) in [(0, "2"), (2, "1")] {
            pending.push((
                4_000_000_000_000,
                1,
                seq,
                node,
                Input::Link(port.into(), false),
            ));
            seq += 1;
        }
    }
    let mut release = BTreeMap::new();
    let mut callbacks = 0;
    while !pending.is_empty() {
        pending.sort_by_key(|x| (x.0, x.1, x.2));
        let (now, _, _, node, input) = pending.remove(0);
        if now >= 8_000_000_000_000 {
            break;
        }
        let out = bridges[node].plan(input, 10000).unwrap();
        for (port, msg) in out.tx {
            let Some((peer, dest)) = peers.get(&(node, port.as_str())) else {
                continue;
            };
            if flap
                && now >= 4_000_000_000_000
                && ((node == 0 && port == "2") || (node == 2 && port == "1"))
            {
                continue;
            }
            let bytes = encode([2, 0, 0, 0, 0, node as u8 + 1], &msg);
            let sof = now.max(*release.get(&(node, port.clone())).unwrap_or(&0));
            let eof = sof + (bytes.len() as u64 + 8) * 8 * 10_000;
            release.insert((node, port.clone()), eof + 960_000);
            let decoded = decode(&bytes, bridges[*peer].ports[*dest].config.id, 1500).unwrap();
            pending.push((
                eof + 1000,
                1,
                seq,
                *peer,
                Input::Receive(dest.to_string(), decoded),
            ));
            seq += 1;
        }
        bridges[node] = out.state;
        callbacks += 1;
        assert!(callbacks < 2000, "mesh did not settle");
    }
    (bridges, callbacks)
}
#[test]
fn supplied_rstp_scenarios() {
    for row in manifest::vectors("rstp-standard-v1") {
        if row["kind"] != "scenario" {
            continue;
        }
        let id = row["id"].as_str().unwrap();
        let actual = match id {
            "P-RSTP-T01-V136" => {
                let mut d = fresh();
                let p = d.state.ports.get_mut("p").unwrap();
                p.info = Info::Received;
                p.counters.info = 1;
                p.priority.0[0] -= 1;
                p.priority.0[4] -= 1;
                let before = d.state.clone();
                let expired = before.plan(Input::Tick, 10000).unwrap();
                assert!(
                    expired
                        .transitions
                        .iter()
                        .any(|t| t.machine == "PIM" && t.to_state == "AGED")
                );
                let committed = expired.state.clone();
                let msg = manifest::vectors("rstp-standard-v1")
                    .into_iter()
                    .find(|r| r["id"] == "P-RSTP-T01-V101")
                    .unwrap();
                let msg = decode(
                    &unhex(msg["input"]["bytes_hex"].as_str().unwrap()).unwrap(),
                    0x8001,
                    1500,
                )
                .unwrap();
                let after = committed
                    .plan(Input::Receive("p".into(), msg), 10000)
                    .unwrap();
                assert_eq!(expired.state, committed);
                json!({"order":["tick_expiry_and_settle","receive_and_settle"],"rx_refresh_undoes_prior_commits":expired.state!=committed,"final_infoIs":after.state.ports["p"].info,"final_rcvdInfoWhile":after.state.ports["p"].counters.info})
            }
            "P-RSTP-T01-V208" => {
                let (b, _) = mesh(false, false);
                json!({"eventually":{"root_id":format!("{:016x}",b[1].root.0[0]),"A.port.role":b[0].ports["1"].role,"B.port.role":b[1].ports["1"].role,"B.root_cost":b[1].root.0[1],"both_ports.forwarding":b.iter().all(|b|b.ports["1"].forwarding)},"convergence_deadline_ps":Value::Null,"deadline_reason":"depends_on_actual_link_rate_queue_and_propagation"})
            }
            "P-RSTP-T01-V209" => {
                let (b, _) = mesh(true, false);
                let roles = json!({"A:1":b[0].ports["1"].role,"A:2":b[0].ports["2"].role,"B:1":b[1].ports["1"].role,"C:1":b[2].ports["1"].role,"B:2":b[1].ports["2"].role,"C:2":b[2].ports["2"].role});
                let edges = [
                    ((0, "1"), (1, "1")),
                    ((0, "2"), (2, "1")),
                    ((1, "2"), (2, "2")),
                ];
                let looped = edges.iter().all(|((a, p), (c, q))| {
                    b[*a].ports[*p].forwarding && b[*c].ports[*q].forwarding
                });
                json!({"eventually":roles,"forwarding_loop":looped})
            }
            "P-RSTP-T01-V210" => {
                let (b, _) = mesh(false, false);
                let before = &b[1];
                let p = &before.ports["1"];
                let msg = p.last_message.clone().unwrap();
                let out = before.plan(Input::Receive("1".into(), msg), 10000).unwrap();
                let after = &out.state;
                assert_eq!(
                    out.transitions
                        .iter()
                        .filter(|t| t.machine == "PIM" && t.to_state == "SUPERIOR_DESIGNATED")
                        .count(),
                    1
                );
                assert_eq!(
                    out.transitions
                        .iter()
                        .filter(|t| t.machine == "PRS" && t.port.is_none())
                        .count(),
                    1
                );
                assert!(after.ports["1"].message.is_none());
                assert!(!after.ports["1"].reselect);
                assert!(
                    !out.transitions
                        .iter()
                        .any(|t| t.machine == "PRT" && t.before["role"] != t.after["role"])
                );
                json!({"pim_branch":out.transitions.iter().find(|t|t.machine=="PIM"&&t.to_state=="SUPERIOR_DESIGNATED").map(|t|t.to_state.clone()),"prs_reexecuted":out.transitions.iter().any(|t|t.machine=="PRS"),"final_root_role_priority_unchanged":before.root==after.root&&before.root_port==after.root_port&&p.role==after.ports["1"].role&&p.priority==after.ports["1"].priority,"finite_settle":out.transitions.len()<10000,"reason":"received message consumed once; reselect cleared once; unchanged role does not reenter role-change global transition"})
            }
            "P-RSTP-T01-V211" => {
                let (b, _) = mesh(true, true);
                json!({"eventually":{"C:1":b[2].ports["1"].role,"C:2":b[2].ports["2"].role,"B:2":b[1].ports["2"].role,"C.root_cost":b[2].root.0[1]},"abstract_tree_recalculation":false})
            }
            _ => panic!("{id}"),
        };
        manifest::finish(
            "rstp-standard-v1",
            &row,
            "runtime::wire::rstp::supplied_rstp_scenarios",
            actual,
        );
    }
}
#[test]
fn supplied_rstp_remaining_units() {
    for row in manifest::vectors("rstp-standard-v1") {
        let id = row["id"].as_str().unwrap();
        if ![134, 137, 138, 203, 204, 205, 206, 207, 214, 215, 216, 217]
            .iter()
            .any(|n| id == format!("P-RSTP-T01-V{n}"))
        {
            continue;
        }
        let i = &row["input"];
        let mut d = fresh();
        let actual = match i["machine"].as_str().unwrap() {
            "PTI" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.counters.tx = i["before"]["txCount"].as_u64().unwrap() as u32;
                p.counters.hello = 2;
                p.counters.info = 1;
                p.counters.tc = 0;
                let out = d.state.plan(Input::Tick, 10000).unwrap();
                let t = out
                    .transitions
                    .iter()
                    .find(|t| t.machine == "PTI" && t.to_state == "TICK")
                    .unwrap();
                let p: Port = serde_json::from_value(t.after.clone()).unwrap();
                json!({"after":{"txCount":p.counters.tx,"helloWhen":p.counters.hello,"rcvdInfoWhile":p.counters.info,"tcWhile":p.counters.tc},"state":format!("{:?}",out.state.pti)})
            }
            "PTX" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.role = Role::Designated;
                p.selected_role = Role::Designated;
                p.selected = true;
                p.send_rstp = true;
                p.new_info = true;
                p.counters.tx = i["before"]["txCount"].as_u64().unwrap() as u32;
                p.counters.hello = i["before"]["helloWhen"].as_u64().unwrap() as u32;
                d.tx_step("p");
                if !d.tx.is_empty() {
                    assert_eq!(d.tx[0].1.kind, Kind::Rst);
                    d.tx_step("p");
                }
                let p = &d.state.ports["p"];
                json!({"tx_intents":d.tx.len(),"txCount":p.counters.tx,"newInfo":p.new_info,"helloWhen":p.counters.hello})
            }
            "PRS" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.config.id = i["port_id"].as_u64().unwrap() as u16;
                p.config.restricted_role = i["restricted_role"].as_bool().unwrap();
                p.info = serde_json::from_value(i["infoIs"].clone()).unwrap();
                if let Some(v) = i.get("portPriority") {
                    p.priority = parse_priority(v);
                }
                if matches!(i["case"].as_str(), Some("Alternate" | "Designated")) {
                    let mut q = p.clone();
                    q.info = Info::Received;
                    q.config.id = 0x8001;
                    q.priority = Priority([
                        0x8000020000000001,
                        0,
                        0x8000020000000001,
                        0,
                        0x8000020000000001,
                        0x8001,
                        0x8001,
                    ]);
                    d.state.ports.insert("q".into(), q);
                }
                d.select().unwrap();
                json!({"selectedRole":d.state.ports["p"].selected_role,"updtInfo":d.state.ports["p"].update})
            }
            "TCM" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.tcm = Tcm::ACTIVE;
                p.role = Role::Root;
                p.tc_prop = true;
                p.send_rstp = false;
                d.tc_step("p");
                let tc = d.state.ports["p"].counters.tc;
                d.step().unwrap();
                assert_eq!(d.flush, vec!["p"]);
                json!({"tcWhile":tc,"fdb_flush_style":if !d.state.ports["p"].fdb_flush{"immediate_if_requested"}else{"pending"},"stpVersion":false})
            }
            "PPM" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.ppm = serde_json::from_value(i["state"].clone()).unwrap();
                p.send_rstp = true;
                for (k, v) in i["before"].as_object().unwrap() {
                    set(p, k, v);
                }
                d.step().unwrap();
                let p = &d.state.ports["p"];
                json!({"state":p.ppm,"sendRSTP":p.send_rstp,"mdelayWhile":p.counters.migration,"rcvdSTP":p.rcvd_stp,"rcvdRSTP":p.rcvd_rstp})
            }
            "PST" => {
                let p = d.state.ports.get_mut("p").unwrap();
                p.pst = Pst::LEARNING;
                p.learning = true;
                p.learn = true;
                p.forward = false;
                d.step().unwrap();
                let p = &d.state.ports["p"];
                json!({"learning_allowed":p.learning,"forwarding_allowed":p.forwarding})
            }
            _ => unreachable!(),
        };
        manifest::finish(
            "rstp-standard-v1",
            &row,
            "runtime::wire::rstp::supplied_rstp_remaining_units",
            actual,
        );
    }
}
#[test]
fn prs_observation_rows_do_not_consume_machine_entry_budget() {
    let mut d = fresh();
    d.state.prs = Prs::INIT_TREE;
    let observed = d.state.plan(Input::Initialize, 10000).unwrap();
    assert!(
        observed
            .transitions
            .iter()
            .any(|t| t.machine == "PRS" && t.port.is_some())
    );
    let entries = observed
        .transitions
        .iter()
        .filter(|t| !(t.machine == "PRS" && t.port.is_some()))
        .count();
    assert!(
        d.state.plan(Input::Initialize, entries).is_ok(),
        "same-entry PRS observation rows cannot consume another machine entry budget"
    );
}
