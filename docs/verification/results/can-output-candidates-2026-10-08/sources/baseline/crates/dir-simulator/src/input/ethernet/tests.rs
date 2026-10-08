use super::*;
use serde_json::json;
fn model() -> PreparedEthernet {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/ethernet/unicast.ini");
    let mut p = crate::input::prepare(&path).unwrap().ethernet.unwrap();
    for device in &mut p.devices {
        device.fdb.clear();
    }
    p
}
fn config(p: &PreparedEthernet) -> Value {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/ethernet/model.json"
    ))
    .unwrap();
    value["schema_version"] = json!(2);
    value["outputs"]=Value::Array(p.directions.iter().rev().map(|d|json!({"port":d.from_port,"scheduler":"strict_priority","queues":(0..8).rev().map(|priority|json!({"priority":priority,"capacity_frames":"64","capacity_bytes":null})).collect::<Vec<_>>()})).collect());
    value
}
fn generator() -> Value {
    json!({"id":"a","node":"Main.a","kind":"ethernet.periodic.v1","frame":{"dst_mac":"02:00:00:00:00:02","ether_type":2048,"data":""},"flow_id":"flow","priority":3,"deadline_ps":"100","start_ps":"0","phase_ps":"1","period_ps":"10","end_ps":"31","count":"8"})
}
fn load_generator(value: Value) -> Result<PreparedEthernet> {
    let mut p = model();
    p.generators.clear();
    workload(
        &json!({"schema_version":2,"generators":[value]}).to_string(),
        &mut p,
        "ethernet.l2.qos.v1",
    )?;
    Ok(p)
}
#[test]
fn qos_config_canonicalizes_outputs_and_all_eight_queues() {
    let mut p = model();
    configure(&config(&p).to_string(), &mut p, "ethernet.l2.qos.v1").unwrap();
    assert_eq!(p.outputs.len(), 6);
    assert!(p.outputs.windows(2).all(|w| w[0].port < w[1].port));
    for o in p.outputs {
        assert_eq!(
            o.queues.iter().map(|q| q.priority).collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
    }
}
#[test]
fn qos_config_rejects_missing_duplicates_unknowns_and_noncanonical_capacities() {
    for mutation in 0..8 {
        let mut p = model();
        let mut v = config(&p);
        match mutation {
            0 => {
                v["outputs"].as_array_mut().unwrap().pop();
            }
            1 => v["outputs"][0]["queues"][0]["priority"] = json!(true),
            2 => v["outputs"][0]["queues"][0]["capacity_frames"] = json!("4294967296"),
            3 => v["outputs"][0]["queues"][0]["capacity_bytes"] = json!("01"),
            4 => v["outputs"][0]["queues"][0]
                .as_object_mut()
                .unwrap()
                .remove("capacity_bytes")
                .map(|_| ())
                .unwrap(),
            5 => v["outputs"][0]["queues"][1]["priority"] = json!(7),
            6 => v["outputs"][0]["scheduler"] = json!("weighted"),
            7 => v["outputs"][0]["unknown"] = json!(1),
            _ => unreachable!(),
        }
        assert!(
            configure(&v.to_string(), &mut p, "ethernet.l2.qos.v1").is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn qos_periodic_schedule_validates_nullable_bounds_and_exclusive_end() {
    let p = load_generator(generator()).unwrap();
    let g = &p.generators[0];
    assert_eq!(g.time(0), Some(1));
    assert_eq!(g.time(2), Some(21));
    assert_eq!(g.time(3), None);
    assert_eq!(g.priority, 3);
    assert_eq!(g.deadline_ps, Some(100));
    for field in ["end_ps", "count", "deadline_ps"] {
        let mut value = generator();
        value.as_object_mut().unwrap().remove(field);
        assert!(load_generator(value).is_err());
    }
    for (field, value) in [
        ("period_ps", json!("0")),
        ("phase_ps", json!("10")),
        ("count", json!("-1")),
        ("priority", json!(8)),
        ("flow_id", json!("bad-id")),
        ("deadline_ps", json!(true)),
    ] {
        let mut v = generator();
        v[field] = value;
        assert!(load_generator(v).is_err(), "{field}");
    }
}
#[test]
fn qos_burst_schedule_validates_last_offset_and_empty_count() {
    let mut g = generator();
    let map = g.as_object_mut().unwrap();
    for key in ["phase_ps", "count"] {
        map.remove(key);
    }
    map.insert("kind".into(), json!("ethernet.burst.v1"));
    map.insert("burst_count".into(), json!("2"));
    map.insert("frames_per_burst".into(), json!("3"));
    map.insert("spacing_ps".into(), json!("2"));
    let p = load_generator(g.clone()).unwrap();
    let schedule = &p.generators[0];
    assert_eq!(
        (0..7).map(|n| schedule.time(n)).collect::<Vec<_>>(),
        vec![
            Some(0),
            Some(2),
            Some(4),
            Some(10),
            Some(12),
            Some(14),
            None
        ]
    );
    g["spacing_ps"] = json!("5");
    assert!(load_generator(g.clone()).is_err());
    g["spacing_ps"] = json!("0");
    g["burst_count"] = json!("0");
    assert_eq!(load_generator(g).unwrap().generators[0].time(0), None);
}
#[test]
fn qos_shared_flow_requires_consistent_contract_even_empty_future_inputs() {
    for field in ["priority", "deadline_ps", "dst_mac"] {
        let mut p = model();
        p.generators.clear();
        let first = generator();
        let mut second = first.clone();
        second["id"] = json!("b");
        second["count"] = json!("0");
        match field {
            "priority" => second[field] = json!(4),
            "deadline_ps" => second[field] = json!("99"),
            _ => second["frame"][field] = json!("02:00:00:00:00:03"),
        }
        assert!(
            workload(
                &json!({"schema_version":2,"generators":[first,second]}).to_string(),
                &mut p,
                "ethernet.l2.qos.v1"
            )
            .is_err()
        );
    }
}
#[test]
fn qos_fields_and_kinds_remain_rejected_by_v1() {
    let mut p = model();
    let mut v = generator();
    assert!(
        workload(
            &json!({"schema_version":2,"generators":[v.clone()]}).to_string(),
            &mut p,
            "ethernet.l2.store-forward.v1"
        )
        .is_err()
    );
    v["kind"] = json!("ethernet.explicit.v1");
    v["times_ps"] = json!([]);
    for key in ["start_ps", "phase_ps", "period_ps", "end_ps", "count"] {
        v.as_object_mut().unwrap().remove(key);
    }
    assert!(
        workload(
            &json!({"schema_version":2,"generators":[v]}).to_string(),
            &mut p,
            "ethernet.l2.store-forward.v1"
        )
        .is_err()
    );
    assert!(
        configure(
            &config(&p).to_string(),
            &mut p,
            "ethernet.l2.store-forward.v1"
        )
        .is_err()
    );
}
#[test]
fn qos_config_duplicate_json_key_is_rejected_before_overwrite() {
    let mut p = model();
    assert!(configure("{\"schema_version\":2,\"schema_version\":2,\"endpoints\":[],\"switches\":[],\"outputs\":[]}",&mut p,"ethernet.l2.qos.v1").unwrap_err().message.contains("duplicate"));
}

fn vlan_config(p: &PreparedEthernet) -> Value {
    let mut v = config(p);
    v["schema_version"] = json!(3);
    for endpoint in v["endpoints"].as_array_mut().unwrap() {
        endpoint["multicast"] = json!([]);
    }
    for switch in v["switches"].as_array_mut().unwrap() {
        for entry in switch["fdb"].as_array_mut().unwrap() {
            entry["vid"] = json!(10);
        }
        switch["multicast"] = json!([]);
        switch["unknown_multicast"] = json!("flood");
    }
    v["ports"] = Value::Array(p.directions.iter().map(|d| json!({"port":d.from_port,"pvid":10,"admit":"all","default_priority":2,"vlans":[{"vid":10,"tagged":false},{"vid":20,"tagged":true}]})).collect());
    v
}
fn vlan_model() -> PreparedEthernet {
    let mut p = model();
    p.outputs.clear();
    p.generators.clear();
    configure(&vlan_config(&p).to_string(), &mut p, "ethernet.l2.vlan.v1").unwrap();
    p
}
fn vlan_generator() -> Value {
    let mut g = generator();
    g["frame"]["tag"] = Value::Null;
    g
}
#[test]
fn vlan_config_rejects_invalid_memberships_tables_and_strict_fields() {
    for mutation in 0..18 {
        let mut p = model();
        let mut v = vlan_config(&p);
        match mutation {
            0 => v["schema_version"] = json!(2),
            1 => {
                v["ports"].as_array_mut().unwrap().pop();
            }
            2 => v["ports"][1] = v["ports"][0].clone(),
            3 => v["ports"][0]["pvid"] = json!(true),
            4 => v["ports"][0]["pvid"] = json!(0),
            5 => v["ports"][0]["pvid"] = json!(4095),
            6 => v["ports"][0]["pvid"] = json!(11),
            7 => v["ports"][0]["vlans"][1]["tagged"] = json!(false),
            8 => v["ports"][0]["vlans"][1]["vid"] = json!(10),
            9 => v["ports"][0]["admit"] = json!("auto"),
            10 => v["ports"][0]["default_priority"] = json!(8),
            11 => v["switches"][0]["fdb"][0]["vid"] = json!(30),
            12 => v["switches"][0]["fdb"][0]["egress"] = json!("Main.a.tx"),
            13 => v["switches"][0]["unknown_multicast"] = json!("learn"),
            14 => {
                v["switches"][0]["multicast"] =
                    json!([{"vid":10,"dst_mac":"ff:ff:ff:ff:ff:ff","egresses":[]}])
            }
            15 => {
                v["switches"][0]["multicast"] =
                    json!([{"vid":10,"dst_mac":"01:80:c2:00:00:0f","egresses":[]}])
            }
            16 => {
                v["endpoints"][0]["multicast"] = json!([{"vid":30,"dst_mac":"01:00:5e:00:00:01"}])
            }
            17 => {
                v["ports"][0].as_object_mut().unwrap().remove("admit");
            }
            _ => unreachable!(),
        }
        assert!(
            configure(&v.to_string(), &mut p, "ethernet.l2.vlan.v1").is_err(),
            "mutation {mutation}"
        );
    }
}
#[test]
fn vlan_empty_future_workloads_still_validate_tag_and_source_contract() {
    for mutation in 0..12 {
        let mut p = vlan_model();
        let mut g = vlan_generator();
        g["count"] = json!("0");
        match mutation {
            0 => {
                g["frame"].as_object_mut().unwrap().remove("tag");
            }
            1 => g["frame"]["tag"] = json!({"vid":true,"pcp":3,"dei":0}),
            2 => g["frame"]["tag"] = json!({"vid":0,"pcp":3,"dei":0}),
            3 => g["frame"]["tag"] = json!({"vid":20,"pcp":8,"dei":0}),
            4 => g["frame"]["tag"] = json!({"vid":20,"pcp":3,"dei":2}),
            5 => g["frame"]["tag"] = json!({"vid":20,"pcp":true,"dei":0}),
            6 => g["frame"]["tag"] = json!({"vid":10,"pcp":3,"dei":0}),
            7 => g["frame"]["tag"] = json!({"vid":30,"pcp":3,"dei":0}),
            8 => g["frame"]["tag"] = json!({"vid":20,"pcp":2,"dei":0}),
            9 => g["frame"]["dst_mac"] = json!("01:80:c2:00:00:00"),
            10 => g["frame"]["ether_type"] = json!(0x8100),
            11 => g["frame"]["data"] = json!("ff".repeat(1501)),
            _ => unreachable!(),
        }
        assert!(
            workload(
                &json!({"schema_version":3,"generators":[g]}).to_string(),
                &mut p,
                "ethernet.l2.vlan.v1"
            )
            .is_err(),
            "mutation {mutation}"
        );
    }
    let mut p = vlan_model();
    let mut g = vlan_generator();
    g["frame"]["dst_mac"] = json!("01:00:5e:00:00:01");
    g["frame"]["tag"] = json!({"vid":20,"pcp":3,"dei":1});
    workload(
        &json!({"schema_version":3,"generators":[g.clone()]}).to_string(),
        &mut p,
        "ethernet.l2.vlan.v1",
    )
    .unwrap();
    assert_eq!(p.generators[0].source_vlan_id, Some(20));
    assert_eq!(p.generators[0].frame.tag.unwrap().dei, 1);
    let mut other = g.clone();
    other["id"] = json!("other");
    other["count"] = json!("0");
    other["frame"]["tag"]["dei"] = json!(0);
    assert!(
        workload(
            &json!({"schema_version":3,"generators":[g,other]}).to_string(),
            &mut vlan_model(),
            "ethernet.l2.vlan.v1"
        )
        .is_err()
    );
}
#[test]
fn vlan_group_membership_is_static_and_old_profile_rejects_schema3() {
    let mut p = model();
    let mut v = vlan_config(&p);
    v["switches"][0]["multicast"] = json!([{"vid":10,"dst_mac":"01:00:5E:00:00:01","egresses":["Main.sw.tx_c","Main.sw.tx_b"]}]);
    v["endpoints"][0]["multicast"] = json!([{"vid":10,"dst_mac":"01:00:5E:00:00:01"}]);
    configure(&v.to_string(), &mut p, "ethernet.l2.vlan.v1").unwrap();
    let sw = p.devices.iter().find(|d| d.kind == "switch").unwrap();
    assert_eq!(
        sw.multicast[&(10, "01:00:5e:00:00:01".into())],
        vec!["Main.sw.tx_b", "Main.sw.tx_c"]
    );
    for profile in ["ethernet.l2.store-forward.v1", "ethernet.l2.qos.v1"] {
        assert!(configure(&v.to_string(), &mut model(), profile).is_err());
        assert!(
            workload(
                &json!({"schema_version":3,"generators":[]}).to_string(),
                &mut model(),
                profile
            )
            .is_err()
        );
    }
}

#[test]
fn media_config_rejects_every_missing_physical_and_phy_field_without_panicking() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/ethernet-media/collision.ini");
    let base = crate::input::prepare(&path).unwrap().ethernet.unwrap();
    let original: Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/ethernet-media/collision.model.json"
    ))
    .unwrap();
    for field in ["id", "a", "b", "phy_mode", "duplex", "a_phy", "b_phy"] {
        let mut value = original.clone();
        value["physical_links"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let error = configure(
            &value.to_string(),
            &mut base.clone(),
            "ethernet.l2.store-forward.v2",
        )
        .unwrap_err();
        assert_eq!(error.code, "E-0001", "{field}");
        assert!(error.details.is_some());
    }
    for field in ["role", "tx_latency_ps", "rx_latency_ps"] {
        let mut value = original.clone();
        value["physical_links"][0]["a_phy"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            configure(
                &value.to_string(),
                &mut base.clone(),
                "ethernet.l2.store-forward.v2"
            )
            .is_err(),
            "{field}"
        );
    }
    for field in [
        "schema_version",
        "endpoints",
        "switches",
        "seed",
        "physical_links",
    ] {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            configure(
                &value.to_string(),
                &mut base.clone(),
                "ethernet.l2.store-forward.v2"
            )
            .is_err(),
            "{field}"
        );
    }
}
#[test]
fn media_profiles_keep_phy_allowlists_disjoint() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/original-network/t1.ini");
    let base = crate::input::prepare(&path).unwrap().ethernet.unwrap();
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/original-network/t1.model.json"
    ))
    .unwrap();
    assert!(
        configure(
            &value.to_string(),
            &mut base.clone(),
            "ethernet.l2.100base-t1.v1"
        )
        .is_ok()
    );
    assert_eq!(
        configure(
            &value.to_string(),
            &mut base.clone(),
            "ethernet.l2.store-forward.v2"
        )
        .unwrap_err()
        .details
        .unwrap()["rule"],
        "phy_mode"
    );
    value["physical_links"][0]["phy_mode"] = json!("100base-tx");
    assert_eq!(
        configure(
            &value.to_string(),
            &mut base.clone(),
            "ethernet.l2.100base-t1.v1"
        )
        .unwrap_err()
        .details
        .unwrap()["rule"],
        "phy_mode"
    );
}
