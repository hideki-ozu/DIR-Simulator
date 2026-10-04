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
