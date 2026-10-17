// Included only in dynamic.rs::tests. Every observed value comes from production code.
fn design_vectors(name: &str) -> Vec<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../docs/proposals/fixtures/{name}-v1.json"));
    serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap()["vectors"]
        .as_array()
        .unwrap()
        .clone()
}
fn dec(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn vector_metric_descriptors_points_gauges_bucket_and_overflow() {
    use crate::{registry::*, runtime::metric_v2::MetricStateV2};
    use MetricAggregation as A;
    use MetricSampling as S;
    fn reg(sampling: S, aggregation: A, initial: BTreeMap<String, u64>) -> Registry {
        let mut r = Registry::empty();
        r.register_metric_v2(
            MetricDescriptorV2 {
                name: "v".into(),
                unit: "items".into(),
                version: 1,
                value_kind: "integer".into(),
                sampling,
                aggregation,
            },
            vec!["a".into()],
            initial,
        )
        .unwrap();
        r.register_profile(ProfileDescriptor {
            name: "vector".into(),
            implementation_version: "1".into(),
            modules: vec![],
            channels: vec![],
            events: vec![],
            metrics: vec!["v".into()],
            model_records: vec![],
            output_schema_version: 3,
            validate: |_, _| Ok(()),
        })
        .unwrap();
        r
    }
    fn publish(s: &mut MetricStateV2, r: &Registry, t: u64, values: &[u64]) {
        let effects = values
            .iter()
            .map(|v| Effect::MetricV2 {
                metric: "v".into(),
                target: "a".into(),
                value: *v,
            })
            .collect::<Vec<_>>();
        let d = s.preflight(r, 4, t, Some(t), &effects).unwrap();
        s.apply(d);
    }
    for v in design_vectors("control-metric-checkpoint") {
        let i = &v["input"];
        let e = &v["expected"];
        let id = v["id"].as_str().unwrap();
        match v["kind"].as_str().unwrap() {
            "metric_descriptor" => {
                let sampling = if i["sampling"] == "point" {
                    S::Point
                } else {
                    S::Gauge
                };
                let aggregation = match i["aggregation"].as_str().unwrap() {
                    "sum" => A::Sum,
                    "max" => A::Max,
                    _ => A::TimeMean,
                };
                let initial = i["initial_gauges"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), dec(v)))
                    .collect();
                let result = Registry::empty().register_metric_v2(
                    MetricDescriptorV2 {
                        name: "v".into(),
                        unit: "items".into(),
                        version: 1,
                        value_kind: "integer".into(),
                        sampling,
                        aggregation,
                    },
                    vec!["a".into()],
                    initial,
                );
                assert_eq!(
                    result.is_ok(),
                    e["prepare_valid"].as_bool().unwrap(),
                    "{id}"
                );
            }
            "point_summary" => {
                for (a, field) in [
                    (A::Count, "count"),
                    (A::Sum, "sum"),
                    (A::Min, "min"),
                    (A::Max, "max"),
                    (A::Mean, "mean_numerator"),
                ] {
                    let r = reg(S::Point, a, BTreeMap::new());
                    let mut s = MetricStateV2::new(&r, "vector");
                    for p in i["samples"].as_array().unwrap() {
                        publish(&mut s, &r, dec(&p["t"]), &[dec(&p["v"])]);
                    }
                    let totals = &s.accumulators[0].total;
                    assert_eq!(
                        totals.count.to_string(),
                        e["count"].as_str().unwrap(),
                        "{id}"
                    );
                    assert_eq!(totals.sum.to_string(), e["sum"].as_str().unwrap(), "{id}");
                    assert_eq!(json!(totals.min.map(|n| n.to_string())), e["min"], "{id}");
                    assert_eq!(json!(totals.max.map(|n| n.to_string())), e["max"], "{id}");
                    assert_eq!(
                        totals.count.to_string(),
                        e["mean_denominator"].as_str().unwrap(),
                        "{id}"
                    );
                    let rows = s.summary(&r, dec(&i["horizon_ps"]), 4).unwrap();
                    let expected = if a == A::Mean {
                        if e[field].is_null() {
                            Value::Null
                        } else {
                            json!(dec(&e[field]) as f64 / dec(&e["mean_denominator"]) as f64)
                        }
                    } else {
                        e[field].clone()
                    };
                    assert_eq!(rows[0]["value"], expected, "{id} {a:?}");
                }
            }
            "gauge_integral" | "gauge_resume" | "frozen_export" => {
                let r = reg(S::Gauge, A::TimeMean, BTreeMap::from([("a".into(), 2)]));
                let mut s = MetricStateV2::new(&r, "vector");
                let h = if i["horizon_ps"].is_string() {
                    dec(&i["horizon_ps"])
                } else {
                    dec(&i["pause_ps"])
                };
                if v["kind"] == "frozen_export" {
                    publish(&mut s, &r, 3, &[4]);
                    publish(&mut s, &r, 7, &[9, 1]);
                } else {
                    for p in i["changes"].as_array().unwrap() {
                        if dec(&p["t"]) <= h {
                            publish(&mut s, &r, dec(&p["t"]), &[dec(&p["v"])]);
                        }
                    }
                }
                let before = s.clone();
                for _ in 0..2 {
                    let rows = s.summary(&r, h, 4).unwrap();
                    let area = if v["kind"] == "gauge_resume" {
                        dec(&e["pause_area"])
                    } else if v["kind"] == "frozen_export" {
                        dec(&e["view_area"])
                    } else {
                        dec(&e["area"])
                    };
                    assert_eq!(
                        rows[0]["value"],
                        if h == 0 {
                            Value::Null
                        } else {
                            json!(area as f64 / h as f64)
                        },
                        "{id}"
                    );
                    if let Some(windows) = e["windows"].as_array() {
                        assert_eq!(rows.len(), windows.len() + 1, "{id}");
                        for (row, w) in rows[1..].iter().zip(windows) {
                            assert_eq!(row["start_ps"], w["start"], "{id}");
                            assert_eq!(row["end_ps"], w["end"], "{id}");
                            assert_eq!(
                                row["value"],
                                json!(
                                    dec(&w["area"]) as f64
                                        / (dec(&w["end"]) - dec(&w["start"])) as f64
                                ),
                                "{id}"
                            );
                        }
                        assert_eq!(h.to_string(), e["duration"].as_str().unwrap(), "{id}");
                    }
                }
                assert_eq!(s, before, "{id}: frozen view must not mutate");
                if v["kind"] == "frozen_export" {
                    assert_eq!(s.accumulators[0].last_time, dec(&e["live_last_time_ps"]));
                    assert_eq!(
                        s.accumulators[0].total.area.to_string(),
                        e["live_area"].as_str().unwrap()
                    );
                }
                if v["kind"] == "gauge_resume" {
                    let rows = s.summary(&r, dec(&i["finish_ps"]), 4).unwrap();
                    assert_eq!(
                        rows[0]["value"],
                        json!(dec(&e["final_area"]) as f64 / dec(&i["finish_ps"]) as f64),
                        "{id}"
                    );
                    assert_eq!(s, before, "{id}");
                }
            }
            "timestamp_bucket" => {
                let r = reg(S::Point, A::Min, BTreeMap::new());
                let mut s = MetricStateV2::new(&r, "vector");
                for p in i["points"].as_array().unwrap() {
                    publish(&mut s, &r, dec(&p["t"]), &[dec(&p["v"])]);
                }
                let before = s.clone();
                let rows = s.summary(&r, dec(&i["horizon_ps"]), 4).unwrap();
                assert_eq!(rows[0]["value"], e["summary_min"], "{id}");
                assert_eq!(rows[1]["value"], e["window_min"], "{id}");
                assert_eq!(
                    s.accumulators[0].bucket_time,
                    Some(dec(&e["held_bucket_time"]))
                );
                assert_eq!(
                    s.accumulators[0].bucket.min,
                    Some(dec(&e["held_bucket_min"]))
                );
                assert_eq!(s, before);
            }
            "checked_sum" => {
                let r = reg(S::Point, A::Sum, BTreeMap::new());
                let mut s = MetricStateV2::new(&r, "vector");
                s.accumulators[0].total.sum = i["accumulator"].as_str().unwrap().parse().unwrap();
                let before = s.clone();
                let result = s.preflight(
                    &r,
                    4,
                    1,
                    Some(0),
                    &[Effect::MetricV2 {
                        metric: "v".into(),
                        target: "a".into(),
                        value: dec(&i["new_value"]),
                    }],
                );
                assert!(result.is_err(), "{id}");
                assert_eq!(s, before, "{id}");
                assert_eq!(
                    s.accumulators[0].total.sum.to_string(),
                    e["accumulator_after"].as_str().unwrap()
                );
            }
            _ => {}
        }
    }
}

#[test]
fn vector_crc_and_wire_time_use_product_arithmetic() {
    for v in design_vectors("bridge-wire") {
        match v["kind"].as_str().unwrap() {
            "crc_arithmetic" => {
                let data = v["input"]["bytes_hex"].as_str().unwrap();
                let bytes = (0..data.len())
                    .step_by(2)
                    .map(|p| u8::from_str_radix(&data[p..p + 2], 16).unwrap())
                    .collect::<Vec<_>>();
                let crc = super::super::crc32(&bytes);
                assert_eq!(
                    bytes.len(),
                    v["expected"]["length_bytes"].as_u64().unwrap() as usize
                );
                assert_eq!(format!("{crc:08x}"), v["expected"]["crc32_hex"]);
                assert_eq!(
                    crate::runtime::wire::codec::hex(&crc.to_le_bytes()),
                    v["expected"]["fcs_little_endian_hex"]
                );
            }
            "wire_time" => {
                let i = &v["input"];
                let mac = i["mac_bytes"].as_u64().unwrap();
                let pre = i["preamble_sfd_bytes"].as_u64().unwrap();
                let ifg = i["ifg_bytes"].as_u64().unwrap();
                let rate = dec(&i["rate_bps"]);
                let sof = dec(&i["sof_ps"]);
                assert_eq!(
                    (sof + super::super::duration((mac + pre) * 8, rate).unwrap()).to_string(),
                    v["expected"]["eof_ps"]
                );
                assert_eq!(
                    (sof + super::super::duration((mac + pre + ifg) * 8, rate).unwrap())
                        .to_string(),
                    v["expected"]["release_ps"]
                );
            }
            _ => {}
        }
    }
}

#[test]
fn vector_registration_static_wire_authorized_union() {
    for v in design_vectors("bridge-wire")
        .into_iter()
        .filter(|v| v["kind"] == "registration_union")
    {
        let (mut base, mut p) = fixture();
        let port = "Net.s1.tx_a";
        let policy = base
            .port_policies
            .iter_mut()
            .find(|p| p.port == port)
            .unwrap();
        policy.vlans = v["input"]["static_vids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n.as_u64().unwrap() as u16, true))
            .collect();
        p.registrable = v["input"]["authorized_vids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| RegistrationKey {
                port: port.into(),
                vid: n.as_u64().unwrap() as u16,
            })
            .collect();
        let mut s = DynamicState::new_with_wire(&p, &base, Some(&wire_config(&base))).unwrap();
        let policies = BTreeMap::from([(port.into(), wire_policy(true, true))]);
        let registered = v["input"]["wire_registered_vids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (port.into(), n.as_u64().unwrap() as u16))
            .collect();
        let (d, _, _) = s
            .plan_wire_in(None, 0, &policies, &registered, &[], true, false)
            .unwrap();
        s.apply(d);
        for (registered, field, t) in [
            (registered, "effective_before", 1),
            (BTreeSet::new(), "effective_after", 2),
        ] {
            let (d, _, _) = s
                .plan_wire_in(None, t, &policies, &registered, &[], true, false)
                .unwrap();
            s.apply(d);
            let actual = (1..=30u16)
                .filter(|vid| s.eligible_egress(port, *vid, &s.snapshot_policy()).is_ok())
                .collect::<Vec<_>>();
            assert_eq!(json!(actual), v["expected"][field], "{}", v["id"]);
        }
    }
}

#[test]
fn vector_bridge_flush_and_relearning_real_partitions() {
    use crate::runtime::wire::WireFlushIntent;
    for v in design_vectors("bridge-integration")
        .into_iter()
        .filter(|v| v["kind"] == "bridge_fdb_flush" || v["kind"] == "bridge_relearning")
    {
        let (mut base, mut p) = fixture();
        let a = "Net.s1.tx_a";
        let b = "Net.s1.tx_b";
        let mut ids = BTreeMap::new();
        if v["kind"] == "bridge_fdb_flush" {
            // Synthetic names are mapped to valid individual MACs and connected switch ports.
            for (n, e) in v["input"]["entries"].as_array().unwrap().iter().enumerate() {
                let mac = format!("02:00:00:00:10:{:02x}", n + 1);
                let vid = e["vid"].as_u64().unwrap() as u16;
                let port = if e["port"] == "a" { a } else { b };
                ids.insert((vid, mac.clone()), e["id"].as_str().unwrap().to_string());
                if e["static"].as_bool().unwrap() {
                    base.devices[3].vlan_fdb.insert((vid, mac), port.into());
                } else {
                    p.mac_keys.insert(MacKey {
                        switch: "Net.s1".into(),
                        vid,
                        mac,
                    });
                }
            }
        }
        let mut s = DynamicState::new_with_wire(&p, &base, Some(&wire_config(&base))).unwrap();
        let policies = BTreeMap::from([
            (a.into(), wire_policy(true, true)),
            (b.into(), wire_policy(true, true)),
        ]);
        let (d, _, _) = s
            .plan_wire_in(None, 0, &policies, &BTreeSet::new(), &[], true, false)
            .unwrap();
        s.apply(d);
        if v["kind"] == "bridge_relearning" {
            let key = MacKey {
                switch: "Net.s1".into(),
                vid: 10,
                mac: "02:00:00:00:00:01".into(),
            };
            s.mac
                .generations
                .insert(key.clone(), dec(&v["input"]["generation_before"]) - 1);
            s.apply(s.plan_learning(a, 10, &key.mac, 0).unwrap());
            assert_eq!(s.mac.entries[&key].generation, 7);
            let (d, _, _) = s
                .plan_wire_in(
                    None,
                    1,
                    &policies,
                    &BTreeSet::new(),
                    &[WireFlushIntent {
                        reason: "rstp_topology_change".into(),
                        port: a.into(),
                        vids: vec![10],
                    }],
                    false,
                    false,
                )
                .unwrap();
            s.apply(d);
            s.apply(s.plan_learning(a, 10, &key.mac, 20).unwrap());
            assert_eq!(
                s.mac.entries[&key].generation.to_string(),
                v["expected"]["generation_after"]
            );
            s.apply(s.plan_controls(1000).unwrap());
            assert_eq!(s.mac.entries[&key].generation, 8);
            assert_eq!(s.mac.entries[&key].expires_at, 1020);
        } else {
            for e in v["input"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["static"] == false)
            {
                let (key, _) = ids
                    .iter()
                    .find(|(_, id)| **id == e["id"].as_str().unwrap())
                    .unwrap();
                let port = if e["port"] == "a" { a } else { b };
                s.apply(s.plan_learning(port, key.0, &key.1, 0).unwrap());
            }
            let static_before = base.devices[3].vlan_fdb.clone();
            let live_before = s.mac.entries.len();
            let rstp = v["input"]["kind"] == "rstp";
            let port = if v["input"]["target_port"] == "a" {
                a
            } else {
                b
            };
            // The coordinator receives this typed intent from the protocol kernel. Mask/edge
            // generation is independently exercised by vector_bridge_coordinator_flush_masks.
            let vids = if rstp {
                if port == b { vec![] } else { vec![10] }
            } else {
                vec![v["input"]["target_vid"].as_u64().unwrap() as u16]
            };
            let (d, removed, mac) = s
                .plan_wire_in(
                    None,
                    1,
                    &policies,
                    &BTreeSet::new(),
                    &[WireFlushIntent {
                        reason: if rstp {
                            "rstp_topology_change"
                        } else {
                            "mvrp_new"
                        }
                        .into(),
                        port: port.into(),
                        vids,
                    }],
                    false,
                    false,
                )
                .unwrap();
            assert_eq!(s.mac.entries.len(), live_before, "staged only");
            let removed_ids = removed[0]
                .iter()
                .map(|k| ids[&(k.vid, k.mac.clone())].clone())
                .collect::<Vec<_>>();
            assert_eq!(json!(removed_ids), v["expected"]["removed"], "{}", v["id"]);
            s.apply(d);
            assert_eq!(s.mac.entries.len(), mac.len());
            let mut kept = s
                .mac
                .entries
                .keys()
                .map(|k| ids[&(k.vid, k.mac.clone())].clone())
                .collect::<Vec<_>>();
            kept.extend(base.devices[3].vlan_fdb.keys().map(|k| ids[k].clone()));
            kept.sort();
            let mut expected = v["expected"]["kept"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            expected.sort();
            assert_eq!(kept, expected, "{}", v["id"]);
            assert_eq!(s.base.devices[3].vlan_fdb, static_before);
        }
    }
}

type VectorEvent = (String, u64, u8, u64, u64);
thread_local! { static VECTOR_EVENTS: std::cell::RefCell<Vec<VectorEvent>>=const{std::cell::RefCell::new(Vec::new())}; static VECTOR_ORDER:std::cell::RefCell<Vec<String>>=const{std::cell::RefCell::new(Vec::new())}; static VECTOR_OVERFLOW:std::cell::Cell<bool>=const{std::cell::Cell::new(false)}; }
struct VectorEventModel;
impl crate::registry::Model for VectorEventModel {
    fn initialize(&mut self, c: &mut crate::registry::Context<'_>) -> crate::registry::ModelResult {
        for (id, t, phase, _, _) in VECTOR_EVENTS
            .with(|e| e.borrow().clone())
            .into_iter()
            .filter(|e| e.2 < 2)
        {
            c.schedule_at(
                t,
                crate::registry::Schema::new(format!("phase{phase}"), 1),
                id.into_bytes(),
            )?;
        }
        Ok(())
    }
    fn on_event(
        &mut self,
        event: &crate::registry::Envelope,
        c: &mut crate::registry::Context<'_>,
    ) -> crate::registry::ModelResult {
        VECTOR_ORDER.with(|o| {
            o.borrow_mut()
                .push(String::from_utf8(event.payload.clone()).unwrap())
        });
        if VECTOR_OVERFLOW.with(std::cell::Cell::get) {
            c.schedule_after(1, crate::registry::Schema::new("phase0", 1), vec![])?;
        }
        Ok(())
    }
    fn on_arbitration(
        &mut self,
        resource: &str,
        _: &mut crate::registry::Context<'_>,
    ) -> crate::registry::ModelResult {
        VECTOR_ORDER.with(|o| o.borrow_mut().push(resource.into()));
        Ok(())
    }
}
fn vector_engine(
    events: Vec<(String, u64, u8, u64, u64)>,
    h: u64,
) -> crate::runtime::registered::Engine {
    use crate::registry::*;
    VECTOR_EVENTS.with(|e| *e.borrow_mut() = events.clone());
    VECTOR_ORDER.with(|o| o.borrow_mut().clear());
    let mut r = Registry::empty();
    let schemas = (0..2)
        .map(|p| Schema::new(format!("phase{p}"), 1))
        .collect::<Vec<_>>();
    for (phase, schema) in schemas.iter().enumerate() {
        r.register_event(EventDescriptor {
            schema: schema.clone(),
            phase: phase as u8,
            validate: |_| Ok(()),
        })
        .unwrap();
    }
    r.register_module(
        "VectorModel",
        |_| Ok(Box::new(VectorEventModel)),
        ModuleDescriptor {
            implementation_key: "VectorModel".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: schemas.clone(),
            resources: vec![],
        },
    )
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "vector.engine".into(),
        implementation_version: "1".into(),
        modules: vec!["VectorModel".into()],
        channels: vec![],
        events: schemas,
        metrics: vec![],
        model_records: vec![],
        output_schema_version: 2,
        validate: |_, _| Ok(()),
    })
    .unwrap();
    let root = std::env::temp_dir().join(format!(
        "dir-vector-engine-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("v")).unwrap();
    std::fs::write(root.join("v/v.ned"),"package v; simple V { parameters: @class(\"VectorModel\"); } network VectorNet { submodules: a: v.V; }").unwrap();
    std::fs::write(root.join("run.ini"),format!("[General]\nnetwork=v.VectorNet\nned-path=\".\"\nmodel-profile=\"vector.engine\"\nsim-time-limit={h}ps\n")).unwrap();
    let p = crate::prepare_with_registry(&root.join("run.ini"), r).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let mut engine = crate::runtime::registered::Engine::new(Arc::new(p)).unwrap();
    engine.initialize().unwrap();
    // Test-only queue fixture preserves typed Work, real timer identities and schemas.
    let mut queue = BTreeMap::new();
    for (_, work) in std::mem::take(&mut engine.queue) {
        let crate::runtime::registered::Work::Delivery { ref envelope, .. } = work else {
            panic!("unexpected arbitration")
        };
        let id = std::str::from_utf8(&envelope.payload).unwrap();
        let (_, time, phase, delta, seq) = events.iter().find(|e| e.0 == id).unwrap();
        queue.insert((*time, *delta, *phase, *seq), work);
    }
    for (id, time, phase, delta, seq) in events.iter().filter(|e| e.2 == 2) {
        queue.insert(
            (*time, *delta, *phase, *seq),
            crate::runtime::registered::Work::Arbitration {
                owner: engine.registered.models[0].id.clone(),
                resource: id.clone(),
            },
        );
    }
    engine.queue = queue;
    engine
}
#[test]
fn vector_engine_phase_delta_timer_and_half_open_boundaries() {
    for v in design_vectors("common-contract")
        .into_iter()
        .filter(|v| v["kind"] == "phase_order")
    {
        let events = v["input"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap().into(),
                    dec(&e["time_ps"]),
                    e["phase"].as_u64().unwrap() as u8,
                    e["delta"].as_u64().unwrap(),
                    e["seq"].as_u64().unwrap(),
                )
            })
            .collect();
        let mut engine = vector_engine(events, 11);
        while engine.step() {}
        let actual = VECTOR_ORDER.with(|o| o.borrow().clone());
        assert_eq!(json!(actual), v["expected"]["order"], "{}", v["id"]);
    }
    let mut e = vector_engine(vec![("timer".into(), 100, 0, 0, 0)], 101);
    assert_eq!(e.snapshot.common.committed_events, 0); // observation at 99: no callback before deadline
    e.prepared = Arc::new({
        let mut p = (*e.prepared).clone();
        p.common.time_limit_ps = 100;
        p
    });
    assert!(!e.step());
    assert_eq!(e.snapshot.common.committed_events, 0);
    e.prepared = Arc::new({
        let mut p = (*e.prepared).clone();
        p.common.time_limit_ps = 101;
        p
    });
    assert!(e.step());
    assert_eq!(e.last_committed_key.unwrap().0, 100);
    assert_eq!(e.snapshot.common.committed_events, 1);
    assert!(!e.step());
    assert_eq!(e.snapshot.common.committed_events, 1);
    let mut e = vector_engine(
        vec![
            ("timer".into(), 100, 0, 0, 0),
            ("arrival".into(), 100, 1, 0, 1),
        ],
        100,
    );
    assert!(!e.step());
    assert_eq!(e.snapshot.common.committed_events, 0);
    assert_eq!(e.snapshot.common.pending_events, 2);
    let mut e = vector_engine(
        vec![
            ("timer".into(), 100, 0, 0, 0),
            ("receive".into(), 100, 1, 0, 1),
            ("select".into(), 100, 2, 0, 2),
        ],
        101,
    );
    while e.step() {}
    assert_eq!(
        VECTOR_ORDER.with(|o| o.borrow().clone()),
        ["timer", "receive", "select"]
    );
}
#[test]
fn vector_timer_overflow_rolls_back_callback_effects() {
    // Time MAX cannot execute with the ordinary half-open u64 horizon; invoke the
    // production callback transaction through an initialization Context at MAX.
    use crate::registry::*;
    let e = vector_engine(vec![], u64::MAX);
    let config = &e.registered.models[0];
    let mut batch = EffectBatch::default();
    let mut next_id = 0;
    let mut context = Context {
        now: u64::MAX,
        config,
        registry: &e.registered.registry,
        profile: &e.registered.registry.profiles["vector.engine"],
        channels: &e.channels,
        channel_configs: &e.registered.channels,
        live_timers: &e.live,
        executing_timer: None,
        next_id: &mut next_id,
        batch: &mut batch,
    };
    assert!(
        context
            .schedule_after(1, Schema::new("phase0", 1), vec![])
            .is_err()
    );
    assert!(batch.effects.is_empty());
    assert_eq!(next_id, 0);
}

fn minimal_wire_input() -> (PreparedEthernet, Value, Value) {
    let (mut base, _) = fixture();
    // Connected bridge with two endpoint links: synthetic a/b -> tx_a/tx_b.
    base.port_policies.retain(|p| {
        ["Net.a.tx", "Net.b.tx", "Net.s1.tx_a", "Net.s1.tx_b"].contains(&p.port.as_str())
    });
    base.directions.retain(|d| {
        ["Net.a.tx", "Net.b.tx", "Net.s1.tx_a", "Net.s1.tx_b"].contains(&d.from_port.as_str())
    });
    for d in &mut base.directions {
        if d.from_port == "Net.b.tx" {
            d.destination = 3;
            d.to_port = "Net.s1.rx_b".into();
        }
        if d.from_port == "Net.s1.tx_b" {
            d.destination = 1;
            d.to_port = "Net.b.rx".into();
        }
    }
    let mut c: Value = serde_json::from_str(include_str!(
        "../../../../../examples/ethernet-wire/active-model.json"
    ))
    .unwrap();
    c["adapters"]
        .as_array_mut()
        .unwrap()
        .retain(|a| a["id"] == "mvrp-s1");
    c["adapters"][0]["ports"] = json!(["Net.s1.tx_a", "Net.s1.tx_b"]);
    c["adapters"][0]["local_declarations"] = json!([]);
    c["port_identities"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["port"] == "Net.s1.tx_a" || p["port"] == "Net.s1.tx_b");
    let w = json!({"schema_name":"dir.ethernet_control_wire.workload","schema_version":1,"ethernet":{"schema_version":4,"generators":[],"controls":[]},"packets":[]});
    (base, c, w)
}
#[test]
fn vector_bridge_catalog_defaults_fixed_override_and_restricted_registration() {
    use crate::runtime::wire::{mvrp::*, mvrp_state::*};
    use crate::types::wire::*;
    for v in design_vectors("bridge-integration")
        .into_iter()
        .filter(|v| v["kind"] == "bridge_catalog")
    {
        let (mut base, mut c, w) = minimal_wire_input();
        base.port_policies
            .iter_mut()
            .filter(|p| p.port.starts_with("Net.s1."))
            .for_each(|p| p.vlans.clear());
        base.port_policies
            .iter_mut()
            .find(|p| p.port == "Net.s1.tx_a")
            .unwrap()
            .vlans
            .insert(10, true);
        c["ethernet"]["dynamic"]["registrable"] =
            json!([{"port":"Net.s1.tx_b","vid":20,"tagged":true}]);
        c["adapters"][0]["port_parameters"] =
            json!([{"port":"Net.s1.tx_b","restricted_vlan_registration":true}]);
        c["adapters"][0]["registrar_controls"]=Value::Array(v["input"]["overrides"].as_array().unwrap().iter().map(|r|json!({"port":if r["port"]=="a"{"Net.s1.tx_a"}else{"Net.s1.tx_b"},"vid":r["vid"],"mode":r["mode"]})).collect());
        let result = crate::input::wire::prepare(&c, &w, &base);
        assert_eq!(
            result.is_ok(),
            v["expected"]["accept"].as_bool().unwrap(),
            "{}",
            v["id"]
        );
        if let Ok(p) = result {
            assert_eq!(
                json!(
                    p.svre_exists
                        .iter()
                        .filter(|(b, _)| b == "Net.s1")
                        .map(|(_, v)| *v)
                        .collect::<Vec<_>>()
                ),
                v["expected"]["svre_vids"]
            );
            for (name, port) in [("a", "Net.s1.tx_a"), ("b", "Net.s1.tx_b")] {
                for vid in [10, 20] {
                    let mode = p
                        .registrar_modes
                        .get(&(port.into(), vid))
                        .copied()
                        .unwrap_or(WireRegistrarMode::Normal);
                    assert_eq!(
                        serde_json::to_value(mode).unwrap(),
                        v["expected"]["modes"][format!("{name}/{vid}")]
                    );
                }
            }
            let adapter = &p.adapters[0];
            let mut s = State::new(adapter);
            s = s.plan(&p, adapter, 0, Input::Initialize).unwrap().state;
            let d = s
                .plan(
                    &p,
                    adapter,
                    1,
                    Input::Receive(
                        "Net.s1.tx_b".into(),
                        DecodedMvrp {
                            version: 0,
                            vectors: vec![Vector {
                                leave_all: false,
                                attributes: vec![(10, AttributeEvent::JoinIn)],
                            }],
                        },
                    ),
                )
                .unwrap();
            assert!(
                d.state.ports["Net.s1.tx_b"].attributes[&10].registered,
                "restricted normal uses bridge VID entry even outside A"
            );
        } else {
            assert!(result.unwrap_err().message.contains("fixed mode"));
        }
    }
}
#[test]
fn vector_hex_decode_and_malformed_batch_use_product_codec_planner() {
    use crate::runtime::wire::{WireEvent, WireState, WireView, codec::*};
    use crate::types::wire::*;
    let bytes = unhex("00Aa10ff").unwrap();
    assert_eq!(bytes, [0, 170, 16, 255]);
    assert_eq!(bytes.len(), 4);
    assert_eq!(hex(&bytes), "00aa10ff");
    assert_eq!(
        client(&bytes, [1, 128, 194, 0, 0, 0], None, 1500)
            .unwrap_err()
            .0,
        "frame_bounds"
    );
    let (base, c, w) = minimal_wire_input();
    let p = crate::input::wire::prepare(&c, &w, &base).unwrap();
    let adapter = p.adapters[0].clone();
    let mut s = WireState::new(Arc::new(p));
    let d = s
        .plan(0, WireEvent::Initialize, &WireView::default())
        .unwrap();
    s.apply(d);
    // VID20 valid followed by reserved VID4095 in a single real encoded MVRP vector.
    // Encoding the wire input is setup; decode and state publication are production.
    let payload = [0, 1, 2, 0, 1, 0, 20, 0, 0, 1, 15, 255, 0, 0, 0, 0, 0];
    let frame = frame(
        [1, 128, 194, 0, 0, 0x21],
        [2, 0, 0, 0, 0, 1],
        0x88f5,
        &payload,
    );
    let before = s.runtime_snapshot();
    let d = s
        .plan(
            1,
            WireEvent::Receive {
                protocol: WireProtocol::Mvrp,
                ingress_port: adapter.ports[0].clone(),
                packet_id: "malformed".into(),
                bytes: frame,
                vid: None,
                destination_mac: None,
                external_injection: true,
            },
            &WireView::default(),
        )
        .unwrap();
    assert!(d.registered.is_empty());
    assert!(d.flush.is_empty());
    assert!(d.tx.is_empty());
    assert!(d.timers.is_empty());
    assert!(
        d.records.iter().any(
            |r| r.data["outcome"] == "control_drop" && r.data["reason"] == "protocol_malformed"
        ),
        "{:?}",
        d.records
    );
    assert_eq!(s.runtime_snapshot(), before);
}

// The parent may invoke these cfg(test)-only macros inside the indicated private
// test modules. They introduce no production API and keep vector source ownership here.
macro_rules! vector_session_private_cases {
    () => {
        #[test]
        fn vector_mailbox_exact_mixed_slots_and_id_sentinel() {
            let make=|next_id|Controller{inner:Arc::new(Mutex::new(Mailbox{pending:VecDeque::new(),slots:BTreeMap::new(),next_id,checkpoint_gate:false,closed:false})),session_id:"vector".into()};
            let c=make(10);
            {
                let mut m=c.lock().unwrap();
                for id in 100..164 {
                    m.slots.insert(id,Slot{command:ControlCommand::Pause,ack:if id<132{Some(ControlAck{request_id:id,control_seq:id,boundary_seq:0,boundary_kind:BoundaryKind::PausedPoll,commit_ordinal:0,virtual_time_ps:0,state:RunState::Paused,status:ControlStatus::Applied,export_ordinal:None,checkpoint_receipt:None,error:None})}else{None}});
                    if id>=132{m.pending.push_back(id);}
                }
                assert_eq!(m.pending.len(),32);assert_eq!(m.slots.values().filter(|s|s.ack.is_some()).count(),32);
            }
            assert_eq!(c.submit(ControlCommand::Pause).unwrap_err().code,"Busy");let m=c.lock().unwrap();assert_eq!(m.slots.len(),64);assert_eq!(m.next_id,10);drop(m);
            let c=make(u64::MAX);assert_eq!(c.submit(ControlCommand::Pause).unwrap_err().code,"ResourceExhausted");assert_eq!(c.lock().unwrap().next_id,u64::MAX);assert!(c.lock().unwrap().slots.is_empty());
        }
        #[test]
        fn vector_checkpoint_gates_and_ordinal_faults() {
            let root=std::env::temp_dir().join(format!("dir-vector-private-{}",crate::output::new_run_identity().unwrap().run_id));std::fs::create_dir_all(root.join("ned/demo")).unwrap();std::fs::write(root.join("ned/demo/model.ned"),"package demo; simple Probe { parameters: @class(\"test.Probe\"); } network Demo { submodules: a: demo.Probe; }").unwrap();std::fs::write(root.join("scenario.ini"),"[General]\nnetwork=demo.Demo\nned-path=\"ned\"\nmodel-profile=\"test.spool.v1\"\nsim-time-limit=10ps\n").unwrap();
            for command in ["Stop","Resume","None"] {
                let (mut s,c)=RunSession::new(crate::prepare_with_registry(&root.join("scenario.ini"),registry()).unwrap(),RunOptions{checkpoint_enabled:true,..Default::default()}).unwrap();c.submit(ControlCommand::Pause).unwrap();s.poll_control().unwrap();
                match command {"Stop"=>{c.submit(ControlCommand::Stop).unwrap();},"Resume"=>{c.submit(ControlCommand::Resume(s.pause_token().unwrap())).unwrap();},_=>{}}
                // Owner checkpoint acceptance and gate close are one critical section.
                let target=root.join(format!("gate-{command}"));c.lock().unwrap().checkpoint_gate=true;assert_eq!(c.submit(ControlCommand::Pause).unwrap_err().code,"Busy");c.lock().unwrap().checkpoint_gate=false;let ack=s.checkpoint(&target,CheckpointLimits::default()).unwrap();assert_eq!(ack.status,if command=="None"{ControlStatus::Applied}else{ControlStatus::InvalidState});assert_eq!(s.state(),match command{"Stop"=>RunState::Stopped,"Resume"=>RunState::Running,_=>RunState::Paused});assert_eq!(target.exists(),command=="None");assert!(!c.lock().unwrap().checkpoint_gate);
            }
            // All three ordinal2 publication cases use the real owner API.
            for fault in [Some("before_publish"),Some("parent_sync"),None] {
                let (mut s,c)=RunSession::new(crate::prepare_with_registry(&root.join("scenario.ini"),registry()).unwrap(),RunOptions{checkpoint_enabled:true,..Default::default()}).unwrap();c.submit(ControlCommand::Pause).unwrap();s.poll_control().unwrap();s.checkpoint_ordinal=2;
                let target=root.join(format!("publication-{fault:?}"));crate::runtime::checkpoint_bundle::vector_set_io_fault(fault);let result=s.checkpoint(&target,CheckpointLimits::default());crate::runtime::checkpoint_bundle::vector_set_io_fault(None);let ack=result.unwrap();
                if fault==Some("before_publish"){assert_eq!(ack.status,ControlStatus::InvalidState);assert!(ack.checkpoint_receipt.is_none());assert!(!target.exists());assert_eq!(s.checkpoint_ordinal,2);}else{let receipt=ack.checkpoint_receipt.unwrap();assert_eq!(receipt.checkpoint_ordinal,2);assert!(target.exists());assert_eq!(s.checkpoint_ordinal,3);assert_eq!(receipt.durability,if fault.is_some(){CheckpointDurability::PublishedDurabilityUnknown}else{CheckpointDurability::Durable});assert!(crate::runtime::checkpoint_bundle::read_bundle(&target,&CheckpointLimits::default()).is_ok());}
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    };
}

#[test]
fn vector_bridge_binding_minimal_topology_prepare() {
    use crate::types::wire::*;
    for v in design_vectors("bridge-integration")
        .into_iter()
        .filter(|v| v["kind"] == "bridge_binding")
    {
        let (mut base, mut c, w) = minimal_wire_input();
        let duplicate = v["id"] == "P-CORE-T01-V202";
        let cross = v["id"] == "P-CORE-T01-V203";
        if !duplicate && !cross {
            // a.p and b.p each terminate on a separate single-port bridge.
            for p in &mut base.port_policies {
                if p.port == "Net.s1.tx_b" {
                    p.port = "Net.s2.tx_a".into();
                    p.ingress = "Net.s2.rx_a".into();
                    p.device = 4;
                }
            }
            for d in &mut base.directions {
                if d.from_port == "Net.s1.tx_b" {
                    d.from_port = "Net.s2.tx_a".into();
                    d.source = 4;
                }
                if d.from_port == "Net.b.tx" {
                    d.to_port = "Net.s2.rx_a".into();
                    d.destination = 4;
                }
            }
        }
        let active: Value = serde_json::from_str(include_str!(
            "../../../../../examples/ethernet-wire/active-model.json"
        ))
        .unwrap();
        let ports = if duplicate {
            vec!["Net.s1.tx_a", "Net.s1.tx_b"]
        } else if cross {
            vec!["Net.s1.tx_a"]
        } else {
            vec!["Net.s1.tx_a", "Net.s2.tx_a"]
        };
        if cross {
            base.port_policies
                .retain(|p| ["Net.a.tx", "Net.s1.tx_a"].contains(&p.port.as_str()));
            base.directions
                .retain(|d| ["Net.a.tx", "Net.s1.tx_a"].contains(&d.from_port.as_str()));
        }
        c["port_identities"] = json!(
            ports
                .iter()
                .enumerate()
                .map(|(n, p)| json!({"port":p,"source_mac":format!("02:00:00:00:00:{:02x}",n+1)}))
                .collect::<Vec<_>>()
        );
        let mut adapters = vec![];
        for (n, row) in v["input"]["adapters"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let protocol = row["protocol"].as_str().unwrap();
            let mut a = active["adapters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["protocol"] == protocol)
                .unwrap()
                .clone();
            a["id"] = json!(format!("binding-{n}"));
            a["ports"] = if duplicate {
                json!(ports)
            } else {
                json!([if cross { ports[0] } else { ports[n] }])
            };
            if protocol == "rstp" {
                a["rstp"]["bridge_address"] = json!(format!("0200000000{:02x}", n + 1));
                a["rstp"]["port_parameters"] = if duplicate {
                    json!(ports.iter().enumerate().map(|(n,p)|json!({"port":p,"port_number":n+1,"external_path_cost":20000,"admin_edge":false})).collect::<Vec<_>>())
                } else {
                    json!([{"port":if cross{ports[0]}else{ports[n]},"port_number":1,"external_path_cost":20000,"admin_edge":false}])
                };
            } else {
                a["local_declarations"] = json!([]);
            }
            adapters.push(a);
        }
        c["adapters"] = json!(adapters);
        let result = crate::input::wire::prepare(&c, &w, &base);
        assert_eq!(
            result.is_ok(),
            v["expected"]["accept"].as_bool().unwrap(),
            "{} {:?}",
            v["id"],
            result.as_ref().err()
        );
        if duplicate {
            assert!(
                result
                    .unwrap_err()
                    .message
                    .contains("duplicate wire protocol bridge owner")
            );
        } else {
            let p = result.unwrap();
            assert_eq!(p.adapters.len(), 2);
            assert!(p.ports.values().filter(|p| p.source_mac.is_some()).count() >= 1);
            if cross {
                assert_eq!(
                    p.owner(WireProtocol::Rstp, ports[0]).unwrap().bridge,
                    p.owner(WireProtocol::Mvrp, ports[0]).unwrap().bridge
                );
            }
        }
    }
}

#[test]
fn vector_checkpoint_manifest_exact_set_and_no_self_hash() {
    use crate::runtime::checkpoint_bundle::*;
    use sha2::{Digest, Sha256};
    let root = std::env::temp_dir().join(format!(
        "dir-vector-manifest-{}",
        crate::output::new_run_identity().unwrap().run_id
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("bundle");
    let blob = b"a".to_vec();
    let blob_path = format!("objects/{:x}.bin", Sha256::digest(&blob));
    let files = BTreeMap::from([
        ("state.json".into(), b"{\"fes\":[]}".to_vec()),
        (blob_path.clone(), blob),
    ]);
    let manifest =
        json!({"schema_name":"dir.checkpoint.bundle","schema_version":1,"state_path":"state.json"});
    write_bundle(
        &path,
        manifest,
        files.clone(),
        &CheckpointLimits::default(),
        2,
    )
    .unwrap();
    let (manifest, actual) = read_bundle(&path, &CheckpointLimits::default()).unwrap();
    assert_eq!(actual, files);
    let listed = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(listed, BTreeSet::from(["state.json", blob_path.as_str()]));
    assert!(!listed.contains("manifest.json"));
    std::fs::write(path.join("extra.bin"), b"extra").unwrap();
    let error = read_bundle(&path, &CheckpointLimits::default()).unwrap_err();
    assert!(
        error.0.contains("unsupported bundle file path"),
        "{}",
        error.0
    );
    std::fs::remove_file(path.join("extra.bin")).unwrap();
    std::fs::create_dir(path.join("inputs")).unwrap();
    std::fs::write(path.join("inputs/extra.bin"), b"extra").unwrap();
    let error = read_bundle(&path, &CheckpointLimits::default()).unwrap_err();
    assert!(error.0.contains("unlisted bundle file"), "{}", error.0);
    std::fs::remove_dir_all(root).unwrap();
}

macro_rules! vector_wire_private_cases {
    () => {
        #[test]
        fn vector_stale_timer_generation_one_vs_two_preserves_registration_epoch() {
            let p = prepared(&[WireProtocol::Mvrp]);
            let a = p.adapters[0].clone();
            let mut s = WireState::new(Arc::new(p));
            let d = s
                .plan(0, WireEvent::Initialize, &WireView::default())
                .unwrap();
            s.apply(d);
            let name = a.ports[0].clone();
            let attribute = s
                .mvrp
                .get_mut(&a.id)
                .unwrap()
                .ports
                .get_mut(&name)
                .unwrap()
                .attributes
                .get_mut(&20)
                .unwrap();
            attribute.registered = true;
            let key = WireTimerKey {
                protocol: "mvrp".into(),
                bridge: a.bridge.clone(),
                port: name.clone(),
                slot: "leave".into(),
                attribute: Some(20),
            };
            s.timers.insert((a.id.clone(), key.clone()), (2, Some(100)));
            let before = s.runtime_snapshot();
            let d = s
                .plan(
                    100,
                    WireEvent::Timer {
                        adapter_id: a.id,
                        key,
                        generation: 1,
                        deadline_ps: 100,
                    },
                    &WireView {
                        policy_epoch: 9,
                        ..Default::default()
                    },
                )
                .unwrap();
            let after = d.runtime_snapshot();
            for key in [
                "mvrp",
                "timer_ledger",
                "generations",
                "registration_generations",
            ] {
                assert_eq!(after[key], before[key], "{key}");
            }
            assert_eq!(d.registered, std::collections::BTreeSet::from([(name, 20)]));
            assert!(d.tx.is_empty() && d.flush.is_empty() && d.timers.is_empty());
            assert!(!d.policy_changed);
            let event = d
                .records
                .iter()
                .find(|r| r.kind == "ethernet.control.wire.event")
                .unwrap();
            assert_eq!(event.data["outcome"], "stale");
            assert_eq!(event.data["reason"], "timer_token");
            assert_eq!(event.data["policy_epoch_before"], "9");
            assert_eq!(event.data["policy_epoch_after"], "9");
            assert_eq!(event.data["effects"], json!([]));
            assert_eq!(s.runtime_snapshot(), before);
        }
        #[test]
        fn vector_bridge_coordinator_flush_masks_and_completion() {
            let mut p = prepared(&[WireProtocol::Rstp, WireProtocol::Mvrp]);
            // Reuse valid typed prepared ports; a/p and a/q form one bridge.
            let q = p.ports["b/p"].clone();
            let mut q = WirePort {
                output: "a/q".into(),
                ingress: "a/q-in".into(),
                bridge: "a".into(),
                ..q
            };
            q.static_vlans = BTreeMap::from([(20, true)]);
            p.ports.remove("b/p");
            p.ports.insert("a/q".into(), q);
            p.adapters.retain(|a| a.bridge == "a");
            for a in &mut p.adapters {
                a.ports.push("a/q".into());
                match &mut a.options {
                    WireOptions::Rstp(c) => {
                        let mut q = c.ports[0].clone();
                        q.port = "a/q".into();
                        q.port_id += 1;
                        q.admin_edge = true;
                        c.ports.push(q);
                    }
                    WireOptions::Mvrp(c) => {
                        let mut q = c.ports[0].clone();
                        q.port = "a/q".into();
                        c.ports.push(q);
                    }
                    _ => {}
                }
            }
            p.ports.get_mut("a/p").unwrap().static_vlans = BTreeMap::from([(10, true)]);
            p.svre_exists = std::collections::BTreeSet::from([("a".into(), 10), ("a".into(), 20)]);
            p.registrar_modes = BTreeMap::from([
                (("a/p".into(), 10), WireRegistrarMode::FixedIgnoreNew),
                (("a/q".into(), 20), WireRegistrarMode::FixedIgnoreNew),
            ]);
            let mut s = WireState::new(Arc::new(p));
            let d = s
                .plan(0, WireEvent::Initialize, &WireView::default())
                .unwrap();
            s.apply(d);
            let a = s
                .prepared
                .adapters
                .iter()
                .find(|a| a.protocol == WireProtocol::Rstp)
                .unwrap()
                .clone();
            let bridge = s.rstp.get_mut(&a.id).unwrap();
            for (name, port) in &mut bridge.ports {
                port.oper_edge = name == "a/q";
                port.fdb_flush = true;
            }
            let mut d = s
                .plan(
                    1,
                    WireEvent::Link {
                        port: "a/p".into(),
                        up: true,
                    },
                    &WireView::default(),
                )
                .unwrap();
            let nonedge = d
                .flush
                .iter()
                .find(|f| f.reason == "rstp_topology_change" && f.port == "a/p")
                .unwrap();
            assert_eq!(nonedge.vids, vec![10]);
            let edge = d
                .flush
                .iter()
                .find(|f| f.reason == "rstp_topology_change" && f.port == "a/q")
                .unwrap();
            assert!(edge.vids.is_empty());
            let keys = d.flush.iter().map(|_| vec![]).collect();
            d.complete_host_records(1, 1, 1, keys, vec![]).unwrap();
            let effects = d
                .records
                .iter()
                .filter_map(|r| r.data["effects"].as_array())
                .flatten()
                .filter(|e| e["kind"] == "mac_flush")
                .collect::<Vec<_>>();
            assert!(!effects.is_empty());
            assert!(effects.iter().all(|e| e["completed"] == true));
        }
    };
}

pub(crate) use vector_session_private_cases;
pub(crate) use vector_wire_private_cases;

macro_rules! vector_host_private_cases {
    () => {
        #[test]
        fn vector_host_learning_forwarding_and_vlan_gates() {
            let p = prepared();
            let registered = p.registered.as_ref().unwrap();
            for (learning, forwarding, eligible, learn, relay, reason) in [
                (false, false, true, false, false, Some("stp_discarding")),
                (true, false, true, true, false, Some("stp_learning")),
                (true, true, true, true, true, None),
                (true, true, false, false, false, Some("vlan_unregistered")),
            ] {
                let mut state = NetworkState::new(registered.network.clone().unwrap()).unwrap();
                let policies = BTreeMap::from([
                    (
                        "Net.s1.tx_a".into(),
                        WirePortPolicy {
                            role: "Designated".into(),
                            learning,
                            forwarding,
                            link_up: true,
                        },
                    ),
                    (
                        "Net.s1.tx_b".into(),
                        WirePortPolicy {
                            role: "Designated".into(),
                            learning: true,
                            forwarding: true,
                            link_up: true,
                        },
                    ),
                    (
                        "Net.s1.tx_c".into(),
                        WirePortPolicy {
                            role: "Designated".into(),
                            learning: true,
                            forwarding: true,
                            link_up: true,
                        },
                    ),
                ]);
                let (d, _, _) = state
                    .dynamic
                    .as_ref()
                    .unwrap()
                    .plan_wire_in(None, 0, &policies, &Default::default(), &[], true, true)
                    .unwrap();
                state.dynamic.as_mut().unwrap().apply(d);
                let vid = if eligible { 10 } else { 20 };
                let frame = crate::runtime::ethernet::serialize_vlan_frame(
                    "02:00:00:00:00:01",
                    "02:00:00:00:00:02",
                    2048,
                    "00",
                    Some(EthernetVlanTag {
                        vid,
                        pcp: 0,
                        dei: 0,
                    }),
                )
                .unwrap();
                state.frames.push(Frame {
                    generator: None,
                    row: EthernetFrameRecord {
                        frame_id: "vector-data".into(),
                        time_ps: 0,
                        source: "Net.a".into(),
                        source_vlan_id: Some(vid),
                        wire: frame,
                        flow_id: None,
                        priority: 0,
                        deadline_ps: None,
                        generated_ps: 0,
                        ready_ps: None,
                    },
                    ip: None,
                    visits: 0,
                    lineage: None,
                    branch_id: None,
                });
                let mut batch = crate::registry::EffectBatch::default();
                let mut next = 0;
                let channels = BTreeMap::new();
                let live = std::collections::BTreeSet::new();
                let mut c = Context {
                    now: 0,
                    config: &registered.models[0],
                    registry: &registered.registry,
                    profile: registered.registry.profile(&p.common.profile).unwrap(),
                    channels: &channels,
                    channel_configs: &[],
                    live_timers: &live,
                    executing_timer: None,
                    next_id: &mut next,
                    batch: &mut batch,
                };
                // Egress endpoint VID20 is statically permitted only for the synthetic
                // ineligible ingress case, so the frame reaches the actual switch gate.
                if !eligible {
                    let mut n = (*state.prepared).clone();
                    n.ethernet
                        .port_policies
                        .iter_mut()
                        .find(|p| p.port == "Net.a.tx")
                        .unwrap()
                        .vlans
                        .insert(20, true);
                    state.prepared = Arc::new(n);
                    state
                        .dynamic
                        .as_mut()
                        .unwrap()
                        .vector_allow_static("Net.a.tx", 20);
                }
                let mut d = state.delta();
                state.source_offer(0, &mut d, &mut c).unwrap();
                state.reserve(&d).unwrap();
                state.apply(d);
                let d = state.arbitrate(&mut c).unwrap();
                state.reserve(&d).unwrap();
                state.apply(d);
                assert!(state.transfers[0].row.planned_arrival_ps.is_some());
                c.now = state.transfers[0].row.planned_arrival_ps.unwrap();
                let mut d = state.delta();
                state.arrive(0, &mut d, &mut c).unwrap();
                state.reserve(&d).unwrap();
                state.apply(d);
                assert_eq!(
                    state.dynamic.as_ref().unwrap().snapshot_tables()["mac"]
                        .as_array()
                        .unwrap()
                        .len(),
                    usize::from(learn)
                );
                if state.receptions[0].row.planned_ready_ps.is_some()
                    && state.receptions[0].row.reason.is_none()
                {
                    c.now = state.receptions[0].row.planned_ready_ps.unwrap();
                    let mut d = state.delta();
                    state.complete(0, &mut d, &mut c).unwrap();
                    state.reserve(&d).unwrap();
                    state.apply(d);
                }
                assert_eq!(state.receptions[0].row.reason.as_deref(), reason);
                assert_eq!(state.transfers.len() > 1, relay);
            }
        }
        #[test]
        fn vector_host_waiting_two_drop_active_one_survives_and_control_lane_eligible() {
            let p = prepared();
            let registered = p.registered.as_ref().unwrap();
            let mut state = NetworkState::new(registered.network.clone().unwrap()).unwrap();
            let port = "Net.s1.tx_b";
            let policies = BTreeMap::from([(
                port.into(),
                WirePortPolicy {
                    role: "Designated".into(),
                    learning: true,
                    forwarding: true,
                    link_up: true,
                },
            )]);
            let (d, _, _) = state
                .dynamic
                .as_ref()
                .unwrap()
                .plan_wire_in(None, 0, &policies, &Default::default(), &[], true, true)
                .unwrap();
            state.dynamic.as_mut().unwrap().apply(d);
            let frame = crate::runtime::ethernet::serialize_vlan_frame(
                "02:00:00:00:00:01",
                "02:00:00:00:00:02",
                2048,
                "00",
                Some(EthernetVlanTag {
                    vid: 10,
                    pcp: 0,
                    dei: 0,
                }),
            )
            .unwrap();
            state.frames.push(Frame {
                generator: None,
                row: EthernetFrameRecord {
                    frame_id: "waiting".into(),
                    time_ps: 0,
                    source: "Net.a".into(),
                    source_vlan_id: Some(10),
                    wire: frame.clone(),
                    flow_id: None,
                    priority: 0,
                    deadline_ps: None,
                    generated_ps: 0,
                    ready_ps: None,
                },
                ip: None,
                visits: 0,
                lineage: None,
                branch_id: None,
            });
            let direction = state
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|d| d.from_port == port)
                .unwrap();
            let mut batch = crate::registry::EffectBatch::default();
            let mut next = 0;
            let channels = BTreeMap::new();
            let live = std::collections::BTreeSet::new();
            let mut c = Context {
                now: 0,
                config: &registered.models[0],
                registry: &registered.registry,
                profile: registered.registry.profile(&p.common.profile).unwrap(),
                channels: &channels,
                channel_configs: &[],
                live_timers: &live,
                executing_timer: None,
                next_id: &mut next,
                batch: &mut batch,
            };
            let mut d = state.delta();
            for visit in 0..3 {
                state
                    .offer(
                        CopyOrigin {
                            circuit: None,
                            frame: 0,
                            parent: None,
                            visit,
                        },
                        &frame,
                        l2::Classification {
                            vid: 10,
                            priority: 0,
                            dei: 0,
                        },
                        &[direction],
                        &mut d,
                        &mut c,
                    )
                    .unwrap();
            }
            state.reserve(&d).unwrap();
            state.apply(d);
            let d = state.arbitrate(&mut c).unwrap();
            state.reserve(&d).unwrap();
            state.apply(d);
            assert_eq!(state.queues[direction].waiting[0].len(), 2);
            let active = state.queues[direction].active.unwrap();
            assert!(state.transfers[active].row.sof_ps.is_some());
            let active_before = state.transfers[active].row.clone();
            let policies = BTreeMap::from([(
                port.into(),
                WirePortPolicy {
                    role: "Designated".into(),
                    learning: false,
                    forwarding: false,
                    link_up: true,
                },
            )]);
            let (d, _, _) = state
                .dynamic
                .as_ref()
                .unwrap()
                .plan_wire_in(None, 1, &policies, &Default::default(), &[], true, false)
                .unwrap();
            state.dynamic.as_mut().unwrap().apply(d);
            c.now = 1;
            let d = state.arbitrate(&mut c).unwrap();
            state.reserve(&d).unwrap();
            state.apply(d);
            assert!(state.queues[direction].waiting[0].is_empty());
            assert_eq!(state.queues[direction].active, Some(active));
            assert_eq!(state.transfers[active].row.status, active_before.status);
            assert_eq!(state.transfers[active].row.sof_ps, active_before.sof_ps);
            assert_eq!(
                state
                    .transfers
                    .iter()
                    .filter(|t| t.row.drop_reason.as_deref() == Some("stp_discarding"))
                    .count(),
                2
            );
            // Generic discard policy revalidates waiting frames without aborting media.
            assert_eq!(
                state
                    .dynamic
                    .as_ref()
                    .unwrap()
                    .wire_ingress_state(port, None),
                Some((false, false))
            );
            let policies = BTreeMap::from([(
                port.into(),
                WirePortPolicy {
                    role: "Designated".into(),
                    learning: true,
                    forwarding: false,
                    link_up: true,
                },
            )]);
            let (d, _, _) = state
                .dynamic
                .as_ref()
                .unwrap()
                .plan_wire_in(None, 1, &policies, &Default::default(), &[], true, false)
                .unwrap();
            state.dynamic.as_mut().unwrap().apply(d);
            assert_eq!(
                state
                    .dynamic
                    .as_ref()
                    .unwrap()
                    .wire_ingress_state(port, None),
                Some((true, false))
            );
            let mut d = state.delta();
            assert!(state.wire_host(&d).unwrap().queues[direction].is_empty());
            let a = state
                .prepared
                .wire
                .as_ref()
                .unwrap()
                .owner(WireProtocol::Rstp, port)
                .unwrap();
            let bytes = crate::runtime::wire::codec::frame(
                [1, 128, 194, 0, 0, 0],
                [2, 0, 0, 0, 0, 1],
                3,
                &[0x42, 0x42, 3],
            );
            let tx = ControlTx {
                intent: WireTxIntent {
                    adapter_id: a.id.clone(),
                    packet_id: "lane".into(),
                    port: port.into(),
                    protocol: WireProtocol::Rstp,
                    ordinal: 1,
                    bytes,
                    cause_id: "vector".into(),
                },
                payload: vec![],
                vid: None,
                destination: None,
                source: None,
                sof: None,
                eof: None,
                release: None,
                arrival: None,
                planned_eof: None,
                planned_release: None,
                planned_arrival: None,
                dropped: false,
            };
            state.wire_enqueue(tx, &mut d, &mut c).unwrap();
            assert_eq!(state.wire_host(&d).unwrap().queues[direction].len(), 1);
        }
        #[test]
        fn vector_host_fifo_capacity_rolls_back_policy_epoch_seven() {
            let p = prepared();
            let registered = p.registered.as_ref().unwrap();
            let mut n = (*registered.network.as_ref().unwrap().as_ref()).clone();
            for a in &mut n.wire.as_mut().unwrap().adapters {
                a.max_pending_tx = 1;
            }
            let mut state = NetworkState::new(Arc::new(n)).unwrap();
            state.dynamic.as_mut().unwrap().vector_set_epoch(7);
            let mut batch = crate::registry::EffectBatch::default();
            let mut next = 0;
            let channels = BTreeMap::new();
            let live = std::collections::BTreeSet::new();
            let mut c = Context {
                now: 0,
                config: &registered.models[0],
                registry: &registered.registry,
                profile: registered.registry.profile(&p.common.profile).unwrap(),
                channels: &channels,
                channel_configs: &[],
                live_timers: &live,
                executing_timer: None,
                next_id: &mut next,
                batch: &mut batch,
            };
            let port = "Net.s1.tx_a";
            let a = state
                .prepared
                .wire
                .as_ref()
                .unwrap()
                .owner(WireProtocol::Rstp, port)
                .unwrap();
            let adapter_id = a.id.clone();
            let make = |id| ControlTx {
                intent: WireTxIntent {
                    adapter_id: adapter_id.clone(),
                    packet_id: format!("capacity-{id}"),
                    port: port.into(),
                    protocol: WireProtocol::Rstp,
                    ordinal: id,
                    bytes: crate::runtime::wire::codec::frame(
                        [1, 128, 194, 0, 0, 0],
                        [2, 0, 0, 0, 0, 1],
                        3,
                        &[0x42, 0x42, 3],
                    ),
                    cause_id: "capacity".into(),
                },
                payload: vec![],
                vid: None,
                destination: None,
                source: None,
                sof: None,
                eof: None,
                release: None,
                arrival: None,
                planned_eof: None,
                planned_release: None,
                planned_arrival: None,
                dropped: false,
            };
            let mut d = state.delta();
            state.wire_enqueue(make(1), &mut d, &mut c).unwrap();
            state.reserve(&d).unwrap();
            state.apply(d);
            let before = state.wire.as_ref().unwrap().tx.len();
            let mut d = state.delta();
            let policies = BTreeMap::from([(
                port.into(),
                WirePortPolicy {
                    role: "Designated".into(),
                    learning: true,
                    forwarding: true,
                    link_up: true,
                },
            )]);
            let (dynamic, _, _) = state
                .dynamic
                .as_ref()
                .unwrap()
                .plan_wire_in(None, 1, &policies, &Default::default(), &[], true, false)
                .unwrap();
            d.dynamic = Some(dynamic);
            let e = state.wire_enqueue(make(2), &mut d, &mut c).unwrap_err();
            assert!(e.message.contains("FIFO capacity"));
            drop(d);
            assert_eq!(state.wire.as_ref().unwrap().tx.len(), before);
            assert_eq!(before, 1);
            assert_eq!(state.dynamic.as_ref().unwrap().policy_epoch(), 7);
        }
    };
}
pub(crate) use vector_host_private_cases;

impl DynamicState {
    pub(crate) fn vector_set_epoch(&mut self, epoch: u64) {
        self.epoch = epoch;
    }
    pub(crate) fn vector_allow_static(&mut self, port: &str, vid: u16) {
        let mut base = (*self.base).clone();
        base.port_policies
            .iter_mut()
            .find(|p| p.port == port)
            .unwrap()
            .vlans
            .insert(vid, true);
        self.base = Arc::new(base);
    }
}

#[test]
fn vector_mvrp_new_flush_exact_port_vid_kernel() {
    use crate::runtime::wire::{mvrp::*, mvrp_state::*};
    let (mut base, mut c, w) = minimal_wire_input();
    for policy in base
        .port_policies
        .iter_mut()
        .filter(|p| p.port.starts_with("Net.s1."))
    {
        policy.vlans = if policy.port.ends_with("tx_a") {
            BTreeMap::from([(10, true)])
        } else {
            BTreeMap::from([(20, true)])
        };
    }
    c["ethernet"]["dynamic"]["registrable"] = json!([{"port":"Net.s1.tx_a","vid":20,"tagged":true},{"port":"Net.s1.tx_b","vid":10,"tagged":true}]);
    let p = crate::input::wire::prepare(&c, &w, &base).unwrap();
    let a = &p.adapters[0];
    let s = State::new(a)
        .plan(&p, a, 0, Input::Initialize)
        .unwrap()
        .state;
    for (port, vid) in [("Net.s1.tx_a", 20), ("Net.s1.tx_b", 10)] {
        let d = s
            .plan(
                &p,
                a,
                1,
                Input::Receive(
                    port.into(),
                    DecodedMvrp {
                        version: 0,
                        vectors: vec![Vector {
                            leave_all: false,
                            attributes: vec![(vid, AttributeEvent::New)],
                        }],
                    },
                ),
            )
            .unwrap();
        assert!(d.flush.contains(&(port.into(), vid)));
        assert!(d.state.ports[port].attributes[&vid].registered);
    }
}

#[test]
fn vector_port_policy_learning_separate_from_generic_forwarding_gate() {
    let (base, p) = fixture();
    let mut s = DynamicState::new_with_wire(&p, &base, Some(&wire_config(&base))).unwrap();
    let policies = BTreeMap::from([("Net.s1.tx_a".into(), wire_policy(true, false))]);
    let (d, _, _) = s
        .plan_wire_in(None, 0, &policies, &BTreeSet::new(), &[], true, false)
        .unwrap();
    s.apply(d);
    let d = s
        .plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:01", 1)
        .unwrap();
    s.apply(d);
    assert_eq!(s.mac.entries.len(), 1);
    assert_eq!(
        s.wire_ingress_state("Net.s1.rx_a", None),
        Some((true, false))
    );
    let reason = s
        .eligible_egress("Net.s1.tx_a", 10, &s.snapshot_policy())
        .unwrap_err();
    assert_eq!(reason.reason(), "stp_discarding");
}

pub(crate) fn vector_preloaded_fdb() -> (crate::types::wire::PreparedWire, DynamicState) {
    let (mut base, mut p) = fixture();
    base.devices[3]
        .vlan_fdb
        .insert((10, "02:00:00:00:10:04".into()), "Net.s1.tx_a".into());
    for (vid, mac) in [
        (10, "02:00:00:00:10:01"),
        (20, "02:00:00:00:10:02"),
        (10, "02:00:00:00:10:03"),
    ] {
        p.mac_keys.insert(MacKey {
            switch: "Net.s1".into(),
            vid,
            mac: mac.into(),
        });
    }
    let (minimal, mut c, w) = minimal_wire_input();
    base.port_policies = minimal.port_policies;
    base.directions = minimal.directions;
    p.bridges.retain(|b, _| b == "Net.s1");
    p.links = vec![
        DynamicLink {
            id: "A".into(),
            ports: ["Net.a.tx".into(), "Net.s1.tx_a".into()],
            up: true,
            cost: 10,
        },
        DynamicLink {
            id: "B".into(),
            ports: ["Net.b.tx".into(), "Net.s1.tx_b".into()],
            up: true,
            cost: 10,
        },
    ];
    base.port_policies
        .iter_mut()
        .find(|p| p.port == "Net.s1.tx_b")
        .unwrap()
        .vlans = BTreeMap::from([(20, true)]);
    c["ethernet"]["dynamic"]["registrable"] = json!([{"port":"Net.s1.tx_a","vid":20,"tagged":true},{"port":"Net.s1.tx_b","vid":10,"tagged":true}]);
    let wire = crate::input::wire::prepare(&c, &w, &base).unwrap();
    let mut s = DynamicState::new_with_wire(&p, &base, Some(&wire)).unwrap();
    for (port, vid, mac) in [
        ("Net.s1.tx_a", 10, "02:00:00:00:10:01"),
        ("Net.s1.tx_a", 20, "02:00:00:00:10:02"),
        ("Net.s1.tx_b", 10, "02:00:00:00:10:03"),
    ] {
        s.apply(s.plan_learning(port, vid, mac, 0).unwrap());
    }
    (wire, s)
}
macro_rules! vector_mvrp_fdb_completion_cases {
    () => {
        #[test]
        fn vector_mvrp_new_exact_fdb_deletion_and_completed_effect() {
            for (port, vid, removed_mac) in [
                ("Net.s1.tx_a", 20, "02:00:00:00:10:02"),
                ("Net.s1.tx_b", 10, "02:00:00:00:10:03"),
            ] {
                let (p, mut dynamic) =
                    $crate::runtime::ethernet::dynamic::tests::vector_preloaded_fdb();
                let mut wire = WireState::new(Arc::new(p));
                let d = wire
                    .plan(0, WireEvent::Initialize, &WireView::default())
                    .unwrap();
                wire.apply(d);
                // This fixture isolates one New FDB request; MAP forwarding is
                // disabled, independently of physical/learning state of the FDB.
                for state in wire.mvrp.values_mut() {
                    for port in state.ports.values_mut() {
                        port.forwarding = false;
                    }
                }
                let (bytes, used) = mvrp::encode(
                    [2, 0, 0, 0, 0, 1],
                    &[(vid, mvrp::AttributeEvent::New)],
                    false,
                    1500,
                )
                .unwrap();
                assert_eq!(used, 1);
                let mut d = wire
                    .plan(
                        1,
                        WireEvent::Receive {
                            protocol: WireProtocol::Mvrp,
                            ingress_port: port.into(),
                            packet_id: "new".into(),
                            bytes,
                            vid: None,
                            destination_mac: None,
                            external_injection: true,
                        },
                        &WireView::default(),
                    )
                    .unwrap();
                let (delta, removed, mac) = dynamic
                    .plan_wire_in(
                        None,
                        1,
                        &d.policies,
                        &d.registered,
                        &d.flush,
                        d.policy_changed,
                        d.topology_changed,
                    )
                    .unwrap();
                let keys = removed.iter().flatten().collect::<Vec<_>>();
                assert_eq!(keys.len(), 1, "flush={:?}, removed={:?}", d.flush, removed);
                assert_eq!(keys[0].mac, removed_mac);
                assert_eq!(keys[0].vid, vid);
                dynamic.apply(delta);
                assert_eq!(mac.len(), 2);
                let kept = mac
                    .iter()
                    .map(|entry| entry.mac.as_str())
                    .collect::<BTreeSet<_>>();
                assert!(kept.contains("02:00:00:00:10:01"));
                assert!(kept.contains(if vid == 20 {
                    "02:00:00:00:10:03"
                } else {
                    "02:00:00:00:10:02"
                }));
                assert!(dynamic.vector_static_fdb_preserved());
                let epoch = dynamic.policy_epoch();
                d.complete_host_records(1, epoch, 0, removed, mac).unwrap();
                let effects = d
                    .records
                    .iter()
                    .filter_map(|r| r.data["effects"].as_array())
                    .flatten()
                    .filter(|e| {
                        e["kind"] == "mac_flush"
                            && e["reason"] == "mvrp_new"
                            && e["port"] == port
                            && e["vids"] == json!([vid])
                    })
                    .collect::<Vec<_>>();
                assert_eq!(effects.len(), 1);
                assert_eq!(effects[0]["completed"], true);
                assert_eq!(effects[0]["keys"][0]["mac"], removed_mac);
            }
        }
    };
}
pub(crate) use vector_mvrp_fdb_completion_cases;

impl DynamicState {
    pub(crate) fn vector_static_fdb_preserved(&self) -> bool {
        self.base.devices[3]
            .vlan_fdb
            .get(&(10, "02:00:00:00:10:04".into()))
            .is_some_and(|port| port == "Net.s1.tx_a")
    }
}
