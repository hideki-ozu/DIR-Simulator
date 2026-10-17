use super::super::{mvrp as mrp, rstp as stp};
use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn fixture_event(raw: &str) -> &str {
    match raw {
        "rNew" => "RNew",
        "rJoinIn" => "RJoinIn",
        "rIn" => "RIn",
        "rJoinMt" => "RJoinMt",
        "rMt" => "RMt",
        "rLv" => "RLv",
        "rLA" => "RLA",
        "Re-declare" => "Redeclare",
        "periodic" => "Periodic",
        "tx" => "Tx",
        "txLA" => "TxLA",
        "txLAF" => "TxLAF",
        "leavetimer" => "LeaveExpired",
        "leavealltimer" => "LeaveAllExpired",
        "periodictimer" => "PeriodicExpired",
        "periodicEnabled" => "Enabled",
        "periodicDisabled" => "Disabled",
        raw => raw,
    }
}
fn auxiliary_event(event: &str) -> mrp::AuxiliaryEvent {
    use mrp::AuxiliaryEvent::*;
    match event {
        "Begin" => Begin,
        "ReceiveLeaveAll" => ReceiveLeaveAll,
        "LeaveAllExpired" => LeaveAllExpired,
        "PeriodicExpired" => PeriodicExpired,
        "Tx" => Tx,
        "Enabled" => Enabled,
        "Disabled" => Disabled,
        _ => panic!("{event}"),
    }
}
fn app_control(control: &str) -> mrp::ApplicantControl {
    match control {
        "normal" => mrp::ApplicantControl::Normal,
        "new_only" => mrp::ApplicantControl::NewOnly,
        "non_participant" => mrp::ApplicantControl::NonParticipant,
        _ => panic!("{control}"),
    }
}
fn reg_control(control: &str) -> mrp::RegistrarControl {
    match control {
        "normal" => mrp::RegistrarControl::Normal,
        "fixed_ignore_new" => mrp::RegistrarControl::FixedIgnoreNew,
        "fixed_propagate_new" => mrp::RegistrarControl::FixedPropagateNew,
        "forbidden" => mrp::RegistrarControl::Forbidden,
        _ => panic!("{control}"),
    }
}
fn decoded<T: serde::de::DeserializeOwned>(s: &str) -> T {
    serde_json::from_value(json!(s)).unwrap()
}

#[test]
fn dedicated_fixture_ids_use_only_input_identity_and_real_reducers() {
    // Fixtures are test-only provenance. Neither production lookup nor this
    // identity test accesses their expected-output fields.
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../docs/proposals/fixtures/mvrp-standard-v1.json"
    ))
    .unwrap();
    let mut counts = BTreeMap::new();
    let mut nonbegin = BTreeMap::new();
    for row in fixture["vectors"].as_array().unwrap() {
        let i = &row["input"];
        let kind = row["kind"].as_str().unwrap();
        let (machine, event, before, after, ac, rc, p2p, room) = match kind {
            "fsm_applicant" | "fsm_applicant_admin" | "fsm_tx_capacity" => {
                let source = i["state"].as_str().unwrap();
                let event = fixture_event(i["event"].as_str().unwrap());
                let registrar = i["registrar"].as_str().unwrap_or("MT");
                let control = i["mode"].as_str().unwrap_or("normal");
                let p2p = i["oper_point_to_point"].as_bool().unwrap_or(true);
                let room = i["room"].as_bool().unwrap_or(true);
                let actual = mrp::applicant_tx(
                    decoded(source),
                    decoded(event),
                    p2p,
                    decoded(registrar),
                    app_control(control),
                    room,
                );
                (
                    "Applicant",
                    event,
                    json!({"applicant":source,"registrar":registrar}),
                    json!({"applicant":actual.state,"registrar":registrar,"request_tx":actual.request_tx}),
                    control,
                    "normal",
                    p2p,
                    room,
                )
            }
            "fsm_registrar" | "fsm_registrar_admin" => {
                let source = i["state"].as_str().unwrap_or("MT");
                let event = fixture_event(i["event"].as_str().unwrap());
                let control = i["mode"].as_str().unwrap_or("normal");
                let p2p = i["oper_point_to_point"].as_bool().unwrap_or(true);
                let actual =
                    mrp::registrar(decoded(source), decoded(event), p2p, reg_control(control));
                (
                    "Registrar",
                    event,
                    json!({"registrar":source}),
                    json!({"registrar":actual.state}),
                    "normal",
                    control,
                    p2p,
                    true,
                )
            }
            "fsm_leaveall" | "fsm_periodic" => {
                let source = i["state"].as_str().unwrap();
                let event = if i["event"] == "rLA" {
                    "ReceiveLeaveAll"
                } else {
                    fixture_event(i["event"].as_str().unwrap())
                };
                let machine = if kind == "fsm_leaveall" {
                    "LeaveAll"
                } else {
                    "Periodic"
                };
                let actual = if machine == "LeaveAll" {
                    mrp::leaveall(decoded(source), auxiliary_event(event))
                } else {
                    mrp::periodic(decoded(source), auxiliary_event(event))
                };
                (
                    machine,
                    event,
                    json!({"state":source}),
                    json!({"state":actual.state}),
                    "normal",
                    "normal",
                    true,
                    true,
                )
            }
            _ => continue,
        };
        let id = mvrp(machine, event, &before, &after, ac, rc, p2p, room);
        assert_eq!(
            id,
            if event == "Begin" {
                "BEGIN"
            } else {
                row["id"].as_str().unwrap()
            }
        );
        assert!(valid_trigger("mvrp", machine, &id));
        *counts.entry(machine).or_insert(0) += 1;
        if event != "Begin" {
            *nonbegin.entry(machine).or_insert(0) += 1;
        }
    }
    assert_eq!(
        counts,
        BTreeMap::from([
            ("Applicant", 804),
            ("Registrar", 90),
            ("LeaveAll", 8),
            ("Periodic", 8)
        ])
    );
    assert_eq!(
        nonbegin,
        BTreeMap::from([
            ("Applicant", 752),
            ("Registrar", 84),
            ("LeaveAll", 6),
            ("Periodic", 6)
        ])
    );
}

fn check_cell(
    machine: &'static str,
    row: usize,
    id: &str,
    event: &str,
    seen: &mut BTreeSet<String>,
) {
    assert_eq!(mrp_catalog().cells[machine][row - 1], id);
    assert!(valid_trigger("mvrp", machine, id));
    if event == "Begin" {
        assert_eq!(id, "BEGIN");
    } else {
        assert!(
            seen.insert(id.to_owned()),
            "duplicate {machine} cell {row}: {id}"
        );
    }
}
#[test]
fn every_finite_input_cell_resolves_purely_and_is_unique_except_begin() {
    let mut seen = BTreeSet::new();
    let mut row = 0;
    for source in APPLICANTS {
        for event in EVENTS {
            for control in APPLICANT_CONTROLS {
                for registrar in REGISTRARS {
                    for p2p in [false, true] {
                        for room in [false, true] {
                            row += 1;
                            let before = json!({"applicant":source,"registrar":"MT","demand":true});
                            let actual = mrp::applicant_tx(
                                decoded(source),
                                decoded(event),
                                p2p,
                                decoded(registrar),
                                app_control(control),
                                room,
                            );
                            let after = json!({"applicant":actual.state,"registrar":registrar,"request_tx":actual.request_tx});
                            let inputs = (before.clone(), after.clone());
                            let id = mvrp(
                                "Applicant",
                                event,
                                &before,
                                &after,
                                control,
                                "normal",
                                p2p,
                                room,
                            );
                            check_cell("Applicant", row, &id, event, &mut seen);
                            assert_eq!((before.clone(), after.clone()), inputs);
                            // Destination Applicant state/action is never an identity oracle.
                            let mut different = after;
                            different["applicant"] = json!("not a state");
                            different["request_tx"] = Value::Null;
                            assert_eq!(
                                id,
                                mvrp(
                                    "Applicant",
                                    event,
                                    &before,
                                    &different,
                                    control,
                                    "forbidden",
                                    p2p,
                                    room
                                )
                            );
                        }
                    }
                }
            }
        }
    }
    assert_eq!((row, seen.len()), (7776, 7344));
    seen.clear();
    row = 0;
    for source in REGISTRARS {
        for event in EVENTS {
            for control in REGISTRAR_CONTROLS {
                for p2p in [false, true] {
                    row += 1;
                    let before = json!({"registrar":source});
                    let actual =
                        mrp::registrar(decoded(source), decoded(event), p2p, reg_control(control));
                    let after = json!({"registrar":actual.state});
                    let id = mvrp(
                        "Registrar",
                        event,
                        &before,
                        &after,
                        "normal",
                        control,
                        p2p,
                        true,
                    );
                    check_cell("Registrar", row, &id, event, &mut seen);
                    assert_eq!(
                        id,
                        mvrp(
                            "Registrar",
                            event,
                            &before,
                            &Value::Null,
                            "non_participant",
                            control,
                            p2p,
                            false
                        )
                    );
                }
            }
        }
    }
    assert_eq!((row, seen.len()), (432, 408));
    for (machine, events) in [("LeaveAll", LEAVEALL_EVENTS), ("Periodic", PERIODIC_EVENTS)] {
        seen.clear();
        row = 0;
        for source in AUXILIARY_STATES {
            for event in events {
                row += 1;
                let actual = if machine == "LeaveAll" {
                    mrp::leaveall(decoded(source), auxiliary_event(event))
                } else {
                    mrp::periodic(decoded(source), auxiliary_event(event))
                };
                let id = mvrp(
                    machine,
                    event,
                    &json!({"state":source}),
                    &json!({"state":actual.state}),
                    "normal",
                    "normal",
                    true,
                    true,
                );
                check_cell(machine, row, &id, event, &mut seen);
            }
        }
        assert_eq!((row, seen.len()), (8, 6));
    }
    assert!(std::ptr::eq(mrp_catalog(), mrp_catalog()));
}

#[test]
fn validation_is_exact_scoped_and_rejects_replaced_fallback_cells() {
    for &(machine, section, size, _) in MRP_TABLES {
        for (offset, id) in mrp_catalog().cells[machine].iter().enumerate() {
            assert!(valid_trigger("mvrp", machine, id));
            for bad in [
                format!("{id}x"),
                format!(" {id}"),
                format!("{id}/1"),
                format!("{id}\n"),
            ] {
                assert!(!valid_trigger("mvrp", machine, &bad));
            }
            let fallback = format!("{machine}/rule/{section}/{}", offset + 1);
            assert_eq!(valid_trigger("mvrp", machine, &fallback), id == &fallback);
            for &(other, _, _, _) in MRP_TABLES {
                if other != machine && id != "BEGIN" {
                    assert!(!valid_trigger("mvrp", other, id));
                }
            }
        }
        for bad in [
            "New".into(),
            "Join".into(),
            "Begin".into(),
            "P-MVRP-T01-V".into(),
            format!("{machine}/rule/{section}/0"),
            format!("{machine}/rule/{section}/01"),
            format!("{machine}/rule/{section}/+1"),
            format!("{machine}/rule/{section}/{}", size + 1),
            format!("{machine}/rule/Table10-3/1"),
        ] {
            assert!(!valid_trigger("mvrp", machine, &bad), "accepted {bad}");
        }
    }
    for (protocol, machine) in [
        ("mvrp", "unknown"),
        ("rstp", "unknown"),
        ("other", "Applicant"),
        ("rstp", "Applicant"),
        ("mvrp", "PPM"),
    ] {
        assert!(!valid_trigger(protocol, machine, "BEGIN"));
    }
    for (machine, id) in [
        ("PRX", "P-RSTP-T01-V219"),
        ("PPM", "P-RSTP-T01-V218"),
        ("PRT", "PRT/rule/13.37/26"),
        ("TCM", "TCM/rule/13.39/11"),
        ("TCM", "TCM/rule/13.39/04"),
        ("PPM", "PPM/rule/13.32/7"),
    ] {
        assert!(!valid_trigger("rstp", machine, id));
    }
}

#[test]
fn unknown_discriminators_never_silently_choose_a_representative() {
    let before = json!({"applicant":"VO","registrar":"MT","state":"Active"});
    for (machine, event, b, a, ac, rc) in [
        (
            "unknown",
            "Begin",
            before.clone(),
            before.clone(),
            "normal",
            "normal",
        ),
        (
            "Applicant",
            "rawNew",
            before.clone(),
            before.clone(),
            "normal",
            "normal",
        ),
        (
            "Applicant",
            "New",
            json!({"applicant":"bad"}),
            before.clone(),
            "normal",
            "normal",
        ),
        (
            "Applicant",
            "New",
            before.clone(),
            json!({"registrar":"bad"}),
            "normal",
            "normal",
        ),
        (
            "Applicant",
            "New",
            before.clone(),
            before.clone(),
            "bad",
            "normal",
        ),
        (
            "Registrar",
            "New",
            before.clone(),
            before.clone(),
            "normal",
            "bad",
        ),
        (
            "Registrar",
            "New",
            json!({"registrar":"bad"}),
            before.clone(),
            "normal",
            "normal",
        ),
        (
            "LeaveAll",
            "Join",
            before.clone(),
            before.clone(),
            "normal",
            "normal",
        ),
        (
            "Periodic",
            "Begin",
            json!({"state":"bad"}),
            before.clone(),
            "normal",
            "normal",
        ),
    ] {
        assert!(
            std::panic::catch_unwind(|| mvrp(machine, event, &b, &a, ac, rc, true, true)).is_err()
        );
    }
    assert!(std::panic::catch_unwind(|| rstp("unknown", "A", "B", "BEGIN", None)).is_err());
}

fn bridge() -> stp::Bridge {
    let bridge = stp::Bridge::new(
        0x8000020000000002,
        stp::Times::default(),
        6,
        BTreeMap::from([(
            "p".into(),
            (
                stp::PortConfig {
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
    bridge.plan(stp::Input::Initialize, 10000).unwrap().state
}
fn assert_transition(d: &stp::Delta, machine: &str, from: &str, to: &str, want: &str) {
    let t = d
        .transitions
        .iter()
        .find(|t| t.machine == machine && t.from_state == from && t.to_state == to)
        .unwrap_or_else(|| panic!("missing {machine} {from}->{to}"));
    let before: stp::Port = serde_json::from_value(t.before.clone()).unwrap();
    assert_eq!(t.guard, want);
    assert_eq!(
        rstp(machine, from, to, "actual-taken-rule", Some(&before)),
        want
    );
    assert!(valid_trigger("rstp", machine, want));
}
#[test]
fn rstp_real_migration_and_edge_branches_have_truthful_ids() {
    for (from, mcheck, enabled, expired, want) in [
        (
            stp::Ppm::SELECTING_STP,
            false,
            true,
            true,
            "P-RSTP-T01-V221",
        ),
        (
            stp::Ppm::SELECTING_STP,
            true,
            true,
            false,
            "PPM/rule/13.32/6",
        ),
        (
            stp::Ppm::SELECTING_STP,
            false,
            false,
            false,
            "PPM/rule/13.32/6",
        ),
        (
            stp::Ppm::SELECTING_STP,
            true,
            true,
            true,
            "PPM/rule/13.32/6",
        ),
        (stp::Ppm::SENSING, false, true, false, "P-RSTP-T01-V222"),
        (stp::Ppm::SENSING, true, true, false, "PPM/rule/13.32/4"),
        (stp::Ppm::SENSING, false, false, false, "PPM/rule/13.32/4"),
    ] {
        let mut b = bridge();
        let p = b.ports.get_mut("p").unwrap();
        p.ppm = from;
        p.mcheck = mcheck;
        p.enabled = enabled;
        p.counters.migration = if expired { 1 } else { 3 };
        p.send_rstp = false;
        p.rcvd_rstp = true;
        p.rcvd_stp = false;
        let d = b.plan(stp::Input::Tick, 10000).unwrap();
        assert_transition(
            &d,
            "PPM",
            &format!("{from:?}"),
            if from == stp::Ppm::SENSING {
                "CHECKING_RSTP"
            } else {
                "SENSING"
            },
            want,
        );
    }
    for (enabled, admin, auto, want) in [
        (true, false, true, "P-RSTP-T01-V223"),
        (false, true, false, "BDM/rule/13.33/3"),
        (false, true, true, "BDM/rule/13.33/3"),
    ] {
        let mut b = bridge();
        let p = b.ports.get_mut("p").unwrap();
        p.enabled = enabled;
        p.config.admin_edge = admin;
        p.config.auto_edge = auto;
        p.bdm = stp::Bdm::NOT_EDGE;
        p.oper_edge = false;
        p.counters.edge = 1;
        p.ppm = stp::Ppm::SENSING;
        p.send_rstp = true;
        p.proposing = true;
        let d = b.plan(stp::Input::Tick, 10000).unwrap();
        assert_transition(&d, "BDM", "NOT_EDGE", "EDGE", want);
    }
}

#[test]
fn rstp_real_transient_returns_keep_their_own_frozen_rows() {
    for (source, role, target, row) in [
        ("ROOT_DISCARD", stp::Role::Root, "ROOT_PORT", 5),
        ("ROOT_PROPOSED", stp::Role::Root, "ROOT_PORT", 6),
        ("ROOT_AGREED", stp::Role::Root, "ROOT_PORT", 7),
        ("ROOT_SYNCED", stp::Role::Root, "ROOT_PORT", 8),
        ("REROOT", stp::Role::Root, "ROOT_PORT", 9),
        ("REROOTED", stp::Role::Root, "ROOT_PORT", 10),
        ("ROOT_LEARN", stp::Role::Root, "ROOT_PORT", 11),
        ("ROOT_FORWARD", stp::Role::Root, "ROOT_PORT", 12),
        (
            "DESIGNATED_DISCARD",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            14,
        ),
        (
            "DESIGNATED_PROPOSE",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            15,
        ),
        (
            "DESIGNATED_AGREE",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            16,
        ),
        (
            "DESIGNATED_SYNCED",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            17,
        ),
        (
            "DESIGNATED_RETIRED",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            18,
        ),
        (
            "DESIGNATED_LEARN",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            19,
        ),
        (
            "DESIGNATED_FORWARD",
            stp::Role::Designated,
            "DESIGNATED_PORT",
            20,
        ),
        (
            "ALTERNATE_PROPOSED",
            stp::Role::Alternate,
            "ALTERNATE_PORT",
            23,
        ),
        (
            "ALTERNATE_AGREED",
            stp::Role::Alternate,
            "ALTERNATE_PORT",
            24,
        ),
        ("BACKUP_PORT", stp::Role::Backup, "ALTERNATE_PORT", 25),
    ] {
        let mut b = bridge();
        let p = b.ports.get_mut("p").unwrap();
        p.prt = decoded(source);
        p.role = role;
        p.selected_role = role;
        let d = b.plan(stp::Input::Tick, 10000).unwrap();
        assert_transition(&d, "PRT", source, target, &format!("PRT/rule/13.37/{row}"));
    }
    for (source, target, row) in [
        ("DETECTED", "ACTIVE", 5),
        ("NOTIFIED_TCN", "NOTIFIED_TC", 7),
        ("NOTIFIED_TC", "ACTIVE", 8),
        ("PROPAGATING", "ACTIVE", 9),
        ("ACKNOWLEDGED", "ACTIVE", 10),
    ] {
        let mut b = bridge();
        let p = b.ports.get_mut("p").unwrap();
        p.tcm = decoded(source);
        let d = b.plan(stp::Input::Tick, 10000).unwrap();
        assert_transition(&d, "TCM", source, target, &format!("TCM/rule/13.39/{row}"));
    }
}

#[test]
fn ambiguous_rstp_paths_require_actual_satisfying_before_conditions() {
    let b = bridge();
    let mut p = b.ports["p"].clone();
    p.enabled = true;
    p.mcheck = false;
    p.counters.migration = 2;
    p.send_rstp = true;
    p.rcvd_rstp = false;
    p.config.auto_edge = false;
    for (machine, from, to) in [
        ("PPM", "SELECTING_STP", "SENSING"),
        ("PPM", "SENSING", "CHECKING_RSTP"),
        ("BDM", "NOT_EDGE", "EDGE"),
    ] {
        assert!(std::panic::catch_unwind(|| rstp(machine, from, to, "actual", None)).is_err());
        assert!(std::panic::catch_unwind(|| rstp(machine, from, to, "actual", Some(&p))).is_err());
    }
}

#[test]
fn rstp_real_flag_clear_and_fdb_completion_are_distinct_rows() {
    let mut b = bridge();
    let p = b.ports.get_mut("p").unwrap();
    p.tcm = stp::Tcm::LEARNING;
    p.rcvd_tc = true;
    let d = b.plan(stp::Input::Tick, 10000).unwrap();
    assert_transition(&d, "TCM", "LEARNING", "LEARNING", "TCM/rule/13.39/4");
    let t = d
        .transitions
        .iter()
        .find(|t| t.guard == "TCM/rule/13.39/4")
        .unwrap();
    assert_eq!(t.before["rcvd_tc"], true);
    assert_eq!(t.after["rcvd_tc"], false);

    let mut b = bridge();
    b.ports.get_mut("p").unwrap().fdb_flush = true;
    let d = b.plan(stp::Input::Tick, 10000).unwrap();
    let t = d
        .transitions
        .iter()
        .find(|t| t.guard == "TCM/rule/DIR-RSTP-FDB-v1/1")
        .unwrap();
    assert_eq!(t.from_state, t.to_state);
    assert_eq!(t.before["fdb_flush"], true);
    assert_eq!(t.after["fdb_flush"], false);
    let before: stp::Port = serde_json::from_value(t.before.clone()).unwrap();
    assert_eq!(
        rstp(
            "TCM",
            &t.from_state,
            &t.to_state,
            "FDB_COMPLETE",
            Some(&before)
        ),
        t.guard
    );
    assert!(valid_trigger("rstp", "TCM", &t.guard));
}

#[test]
fn applicant_identity_uses_actual_post_registrar_input_in_the_reducer_order() {
    let before = json!({"applicant":"QA","registrar":"MT"});
    let r = mrp::registrar(
        mrp::Registrar::MT,
        mrp::Event::RJoinMt,
        true,
        mrp::RegistrarControl::Normal,
    );
    let a = mrp::applicant(
        mrp::Applicant::QA,
        mrp::Event::RJoinMt,
        true,
        r.state,
        mrp::ApplicantControl::Normal,
    );
    let after = json!({"applicant":a.state,"registrar":r.state});
    assert_eq!(after, json!({"applicant":"AA","registrar":"IN"}));
    assert_eq!(
        mvrp(
            "Registrar",
            "RJoinMt",
            &before,
            &after,
            "normal",
            "normal",
            true,
            true
        ),
        "P-MVRP-T01-V896"
    );
    assert_eq!(
        mvrp(
            "Applicant",
            "RJoinMt",
            &before,
            &after,
            "normal",
            "normal",
            true,
            true
        ),
        "P-MVRP-T01-V190"
    );
    // V382 is the fixture INPUT cell with Registrar MT at Applicant evaluation;
    // it must not label the actual Registrar-first IN path above.
    assert_eq!(
        mvrp(
            "Applicant",
            "RJoinMt",
            &before,
            &before,
            "normal",
            "normal",
            true,
            true
        ),
        "P-MVRP-T01-V382"
    );
}

#[test]
fn rstp_real_transmit_uct_returns_keep_their_transmit_rule_rows() {
    for (source, row) in [
        ("TRANSMIT_INIT", 2),
        ("TRANSMIT_PERIODIC", 3),
        ("TRANSMIT_RSTP", 4),
        ("TRANSMIT_TCN", 5),
        ("TRANSMIT_CONFIG", 6),
    ] {
        let mut b = bridge();
        let p = b.ports.get_mut("p").unwrap();
        p.ptx = decoded(source);
        p.new_info = false;
        p.counters.hello = 2;
        let d = b.plan(stp::Input::Tick, 10000).unwrap();
        assert_transition(&d, "PTX", source, "IDLE", &format!("PTX/rule/13.34/{row}"));
    }
}
