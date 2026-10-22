use super::*;

fn cep_bridge() -> Bridge {
    let mut ports = BTreeMap::new();
    for (name, id, cost) in [
        ("cep", 0x2fff, 1),
        ("pep100", 0x2064, 128),
        ("pep200", 0x20c8, 128),
    ] {
        ports.insert(
            name.to_string(),
            (
                PortConfig {
                    id,
                    cost,
                    admin_edge: name != "cep",
                    auto_edge: false,
                    auto_isolate: false,
                    restricted_role: false,
                    restricted_tcn: false,
                },
                true,
            ),
        );
    }
    Bridge::new_cep(
        0xffff_0200_0000_1001,
        Times::default(),
        6,
        ports,
        "cep".into(),
        BTreeMap::from([("pep100".into(), 4), ("pep200".into(), 2)]),
    )
    .unwrap()
}

fn delta(state: Bridge) -> Delta {
    Delta {
        state,
        tx: vec![],
        flush: vec![],
        transitions: vec![],
    }
}

#[test]
fn cep_mode_uses_service_cardinality_and_peps_never_become_edge() {
    let bridge = cep_bridge();
    assert_eq!(bridge.cep_source_mac(), Some([2, 0, 0, 0, 0x10, 1]));
    assert!(!bridge.ports["pep100"].oper_point_to_point_mac);
    assert!(bridge.ports["pep200"].oper_point_to_point_mac);
    for name in ["pep100", "pep200"] {
        let p = &bridge.ports[name];
        assert!(!p.config.admin_edge && p.config.auto_edge);
        assert!(!p.oper_edge && p.bdm == Bdm::NOT_EDGE);
    }
    assert!(
        bridge
            .plan(Input::PointToPoint("pep100".into(), true), 10000)
            .is_err()
    );
    let two = bridge
        .plan(Input::CepServiceCardinality("pep100".into(), 2), 10000)
        .unwrap()
        .state;
    assert!(two.ports["pep100"].oper_point_to_point_mac);
    let mut shared = two
        .plan(Input::CepServiceCardinality("pep100".into(), 4), 10000)
        .unwrap()
        .state;
    assert!(!shared.ports["pep100"].oper_point_to_point_mac);
    shared.ports.get_mut("pep100").unwrap().counters.edge = 0;
    shared.ports.get_mut("pep100").unwrap().proposing = true;
    let ticked = shared.plan(Input::Tick, 10000).unwrap().state;
    assert!(!ticked.ports["pep100"].oper_edge);
    assert_eq!(ticked.ports["pep100"].bdm, Bdm::NOT_EDGE);
    let legacy = Bridge::new(1, Times::default(), 6, BTreeMap::new());
    let legacy_json = serde_json::to_value(legacy).unwrap();
    assert!(legacy_json.get("mode").is_none());
    assert!(legacy_json.get("root_source").is_none());
    let ports = cep_bridge()
        .ports
        .into_iter()
        .map(|(name, port)| (name, (port.config, port.enabled)))
        .collect();
    let legacy_port = Bridge::new(1, Times::default(), 6, ports);
    let port_json = serde_json::to_value(&legacy_port.ports["pep100"]).unwrap();
    assert!(port_json.get("oper_point_to_point_mac").is_none());
    let legacy_json = serde_json::to_value(&legacy_port).unwrap();
    assert!(
        legacy_json["ports"]["pep100"]
            .get("oper_point_to_point_mac")
            .is_none()
    );
    assert_eq!(
        serde_json::from_value::<Bridge>(legacy_json).unwrap(),
        legacy_port
    );
}

#[test]
fn two_peps_can_be_root_while_selected_vector_source_stays_unique() {
    let mut bridge = cep_bridge();
    for p in bridge.ports.values_mut() {
        p.info = Info::Mine;
    }
    let remote = 0x1000_0200_0000_1002;
    for (name, remote_port) in [("pep100", 0x8001), ("pep200", 0x8002)] {
        let p = bridge.ports.get_mut(name).unwrap();
        p.info = Info::Received;
        p.priority = Priority([
            remote,
            0,
            remote,
            0,
            remote,
            remote_port,
            p.config.id as u64,
        ]);
        p.times = Times::default();
    }
    let mut d = delta(bridge);
    d.select().unwrap();
    assert_eq!(d.state.root_source.as_deref(), Some("pep100"));
    assert_eq!(d.state.root_port, 0x2064);
    assert_eq!(d.state.ports["pep100"].selected_role, Role::Root);
    assert_eq!(d.state.ports["pep200"].selected_role, Role::Root);
    assert_eq!(d.state.ports["cep"].selected_role, Role::Designated);
    let mut bridge = d.state;
    bridge.ports.get_mut("pep100").unwrap().info = Info::Aged;
    bridge.ports.get_mut("pep200").unwrap().info = Info::Aged;
    let p = bridge.ports.get_mut("cep").unwrap();
    p.info = Info::Received;
    p.priority = Priority([remote, 0, remote, 0, remote, 0x8001, p.config.id as u64]);
    let mut d = delta(bridge);
    d.select().unwrap();
    assert_eq!(d.state.root_source.as_deref(), Some("cep"));
    assert_eq!(d.state.ports["pep100"].selected_role, Role::Designated);
    assert_eq!(d.state.ports["pep200"].selected_role, Role::Designated);
}

#[test]
fn received_bpdus_drive_multiple_pep_root_roles_in_real_reducer() {
    let mut bridge = cep_bridge().plan(Input::Initialize, 10000).unwrap().state;
    let remote = 0x1000_0200_0000_1002;
    for (name, designated_port) in [("pep100", 0x8001), ("pep200", 0x8002)] {
        let message = Bpdu {
            kind: Kind::Rst,
            version: 2,
            flags: 0x0c,
            priority: Priority([
                remote,
                0,
                remote,
                0,
                remote,
                designated_port,
                bridge.ports[name].config.id as u64,
            ]),
            times: Times::default(),
        };
        bridge = bridge
            .plan(Input::Receive(name.into(), message), 10000)
            .unwrap()
            .state;
    }
    assert_eq!(bridge.root_source.as_deref(), Some("pep100"));
    assert_eq!(bridge.ports["pep100"].role, Role::Root);
    assert_eq!(bridge.ports["pep200"].role, Role::Root);
    assert_eq!(bridge.ports["cep"].role, Role::Designated);
    let snap = serde_json::to_value(&bridge).unwrap();
    assert_eq!(snap["root_source"], "pep100");
    assert_eq!(serde_json::from_value::<Bridge>(snap).unwrap(), bridge);
}

fn root_entry(bridge: &mut Bridge, name: &str) {
    let p = bridge.ports.get_mut(name).unwrap();
    p.prt = Prt::ROOT_PORT;
    p.role = Role::Root;
    p.selected_role = Role::Root;
    p.selected = true;
    p.update = false;
    p.counters.rr = ceil(p.designated_times.forward_delay);
    p.counters.rb = 0;
    p.synced = false;
    p.sync = false;
    p.reroot = false;
}

#[test]
fn pep_sync_and_reroot_touch_only_self_and_cep() {
    let mut bridge = cep_bridge();
    root_entry(&mut bridge, "pep100");
    for p in bridge.ports.values_mut() {
        p.sync = false;
        p.reroot = false;
    }
    bridge.ports.get_mut("pep100").unwrap().proposed = true;
    let mut d = delta(bridge);
    d.role_step("pep100").unwrap();
    assert!(d.state.ports["pep100"].sync && d.state.ports["cep"].sync);
    assert!(!d.state.ports["pep200"].sync);

    let mut bridge = cep_bridge();
    root_entry(&mut bridge, "pep100");
    for p in bridge.ports.values_mut() {
        p.reroot = false;
    }
    bridge.ports.get_mut("pep100").unwrap().agree = true;
    let mut d = delta(bridge);
    d.role_step("pep100").unwrap();
    assert!(d.state.ports["pep100"].reroot && d.state.ports["cep"].reroot);
    assert!(!d.state.ports["pep200"].reroot);

    let mut bridge = cep_bridge();
    root_entry(&mut bridge, "pep100");
    bridge.ports.get_mut("cep").unwrap().synced = true;
    bridge.ports.get_mut("pep200").unwrap().synced = false;
    let mut d = delta(bridge);
    d.role_step("pep100").unwrap();
    assert_eq!(d.state.ports["pep100"].prt, Prt::ROOT_AGREED);
}

#[test]
fn pep_tc_propagates_to_cep_not_sibling_and_respects_restriction() {
    let mut bridge = cep_bridge();
    for p in bridge.ports.values_mut() {
        p.tcm = Tcm::ACTIVE;
        p.role = Role::Designated;
        p.tc_prop = false;
    }
    bridge.ports.get_mut("pep100").unwrap().rcvd_tc = true;
    let mut d = delta(bridge);
    d.tc_step("pep100");
    assert!(d.state.ports["cep"].tc_prop);
    assert!(!d.state.ports["pep200"].tc_prop);

    let mut bridge = cep_bridge();
    for p in bridge.ports.values_mut() {
        p.tcm = Tcm::ACTIVE;
        p.role = Role::Designated;
        p.tc_prop = false;
    }
    bridge.ports.get_mut("pep100").unwrap().rcvd_tc = true;
    bridge
        .ports
        .get_mut("pep100")
        .unwrap()
        .config
        .restricted_tcn = true;
    let mut d = delta(bridge);
    d.tc_step("pep100");
    assert!(!d.state.ports["cep"].tc_prop);

    let mut bridge = cep_bridge();
    for p in bridge.ports.values_mut() {
        p.tcm = Tcm::ACTIVE;
        p.role = Role::Designated;
        p.tc_prop = false;
    }
    bridge.ports.get_mut("cep").unwrap().rcvd_tc = true;
    let mut d = delta(bridge);
    d.tc_step("cep");
    assert!(d.state.ports["pep100"].tc_prop);
    assert!(d.state.ports["pep200"].tc_prop);
}
