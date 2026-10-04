use super::*;
use serde_json::Value;
fn fixture(name: &str) -> PreparedSimulation {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/ethernet")
        .join(format!("{name}.ini"));
    crate::input::prepare(&path).unwrap()
}
#[test]
fn published_serializer_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/ethernet/vectors.json"
    ))
    .unwrap();
    for v in vectors["vectors"].as_array().unwrap() {
        let w = serialize_frame(
            v["src_mac"].as_str().unwrap(),
            v["dst_mac"].as_str().unwrap(),
            v["ether_type"].as_u64().unwrap() as u16,
            v["data"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(w.mac_hex, v["mac_hex"].as_str().unwrap());
        assert_eq!(w.fcs_hex, v["fcs_hex"].as_str().unwrap());
        assert_eq!(w.mac_bytes, v["mac_bytes"].as_u64().unwrap());
        assert_eq!(w.pad_bytes, v["pad_bytes"].as_u64().unwrap());
        assert_eq!(
            duration(8 * (w.mac_bytes + 8), 1_000_000_000)
                .unwrap()
                .to_string(),
            v["eof_ps"].as_str().unwrap()
        );
        assert_eq!(
            duration(8 * (w.mac_bytes + 20), 1_000_000_000)
                .unwrap()
                .to_string(),
            v["release_ps"].as_str().unwrap()
        );
    }
}
#[test]
fn published_scenarios_and_half_open_boundaries() {
    let scenarios: Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/ethernet/scenarios.json"
    ))
    .unwrap();
    for case in scenarios["scenarios"].as_array().unwrap().iter().take(10) {
        let p = fixture(case["name"].as_str().unwrap());
        let snapshot = simulate(&p).unwrap();
        assert!(!snapshot.common.partial, "{}", case["name"]);
        let s = snapshot.ethernet.unwrap();
        let e = &case["expected"];
        assert_eq!(s.frames.len() as u64, e["generated"].as_u64().unwrap());
        assert_eq!(s.transfers.len() as u64, e["offered"].as_u64().unwrap());
        for status in ["serialized", "dropped", "transmitting"] {
            if let Some(n) = e[status].as_u64() {
                assert_eq!(
                    s.transfers.iter().filter(|t| t.status == status).count() as u64,
                    n,
                    "{} {status}",
                    case["name"]
                );
            }
        }
        for status in ["received", "filtered", "processing"] {
            if let Some(n) = e[status].as_u64() {
                assert_eq!(
                    s.receptions.iter().filter(|r| r.status == status).count() as u64,
                    n,
                    "{} {status}",
                    case["name"]
                );
            }
        }
        if let Some(n) = e["receptions"].as_u64() {
            assert_eq!(s.receptions.len() as u64, n);
        }
        if let Some(deliveries) = e["deliveries"].as_array() {
            for delivery in deliveries {
                let r = s
                    .receptions
                    .iter()
                    .find(|r| {
                        r.frame_id == delivery["frame_id"].as_str().unwrap()
                            && r.device == delivery["node"].as_str().unwrap()
                    })
                    .unwrap();
                assert_eq!(
                    r.ready_ps.unwrap().to_string(),
                    delivery["received_ps"].as_str().unwrap()
                );
            }
        }
        if let Some(times) = e["transfer_times"].as_array() {
            for times in times {
                let r = s
                    .transfers
                    .iter()
                    .find(|r| r.transfer_id == times["id"].as_str().unwrap())
                    .unwrap();
                for (key, value) in [
                    ("sof_ps", r.sof_ps),
                    ("eof_ps", r.eof_ps),
                    ("release_ps", r.release_ps),
                    ("arrival_ps", r.arrival_ps),
                ] {
                    if let Some(expected) = times[key].as_str() {
                        assert_eq!(value.unwrap().to_string(), expected);
                    }
                }
            }
        }
    }
}
#[test]
fn arithmetic_failure_preserves_callback_prefix() {
    let mut p = fixture("unicast");
    p.common.time_limit_ps = u64::MAX;
    p.ethernet.as_mut().unwrap().generators[0].times_ps = vec![u64::MAX - 1];
    let s = simulate(&p).unwrap();
    assert!(s.common.partial);
    assert_eq!(s.common.diagnostics[0].code, "E-0004");
    let eth = s.ethernet.unwrap();
    assert_eq!(eth.frames.len(), 1);
    assert_eq!(eth.transfers.len(), 1);
    assert_eq!(eth.transfers[0].status, "queued");
    assert_eq!(eth.transfers[0].sof_ps, None);
    assert_eq!(eth.transfers[0].planned_eof_ps, None);
}
#[test]
fn source_processing_overflow_discards_generation() {
    let mut p = fixture("unicast");
    p.common.time_limit_ps = u64::MAX;
    p.ethernet.as_mut().unwrap().generators[0].times_ps = vec![u64::MAX - 1];
    p.ethernet.as_mut().unwrap().devices[0].tx_processing_delay_ps = 2;
    let s = simulate(&p).unwrap();
    assert!(s.common.partial);
    assert!(s.ethernet.unwrap().frames.is_empty());
    assert_eq!(s.common.committed_events, 1);
}
#[test]
fn processing_overflow_discards_arrival_atomically() {
    let mut p = fixture("unicast");
    p.ethernet
        .as_mut()
        .unwrap()
        .devices
        .iter_mut()
        .find(|d| d.kind == "switch")
        .unwrap()
        .forward_delay_ps = u64::MAX;
    let s = simulate(&p).unwrap();
    assert!(s.common.partial);
    let eth = s.ethernet.unwrap();
    assert_eq!(eth.transfers[0].eof_ps, Some(576000));
    assert_eq!(eth.transfers[0].arrival_ps, None);
    assert!(eth.receptions.is_empty());
}
#[test]
fn fixture_preparation_rejects_invalid_inputs() {
    for name in ["bad-fdb", "bad-payload"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/verification/fixtures/ethernet")
            .join(format!("{name}.ini"));
        assert_eq!(crate::input::prepare(&path).unwrap_err().code, "E-0001");
    }
}

#[test]
fn broadcast_fanout_drops_only_full_output() {
    let mut p = fixture("broadcast");
    let model = p.ethernet.as_mut().unwrap();
    model
        .devices
        .iter_mut()
        .find(|d| d.kind == "switch")
        .unwrap()
        .queue_capacity = 1;
    let source = model.devices.iter().position(|d| d.id == "Main.c").unwrap();
    let mut generator = model.generators[0].clone();
    generator.id = "c".into();
    generator.source = source;
    generator.frame = serialize_frame(
        model.devices[source].mac.as_ref().unwrap(),
        "ff:ff:ff:ff:ff:ff",
        2048,
        "",
    )
    .unwrap();
    model.generators.push(generator);
    let snapshot = simulate(&p).unwrap();
    let s = snapshot.ethernet.unwrap();
    assert_eq!(s.transfers.len(), 6);
    assert_eq!(
        s.transfers.iter().filter(|t| t.status == "dropped").count(),
        1
    );
    assert_eq!(
        s.transfers
            .iter()
            .find(|t| t.status == "dropped")
            .unwrap()
            .transfer_id,
        "c:0@Main.sw.tx_b"
    );
    assert_eq!(
        s.receptions
            .iter()
            .filter(|r| r.status == "received")
            .count(),
        3
    );
    let forwarded = s
        .receptions
        .iter()
        .filter(|r| r.status == "forwarded")
        .collect::<Vec<_>>();
    assert_eq!(forwarded.len(), 2);
    assert!(forwarded.iter().all(|r| r.egress_transfer_ids.len() == 2));
}
#[test]
fn fdb_same_ingress_filters_without_copy() {
    let mut p = fixture("unicast");
    p.ethernet
        .as_mut()
        .unwrap()
        .devices
        .iter_mut()
        .find(|d| d.kind == "switch")
        .unwrap()
        .fdb
        .insert("02:00:00:00:00:02".into(), "Main.sw.tx_a".into());
    let snapshot = simulate(&p).unwrap();
    let s = snapshot.ethernet.unwrap();
    assert_eq!(s.transfers.len(), 1);
    assert_eq!(s.receptions.len(), 1);
    assert_eq!(s.receptions[0].status, "filtered");
    assert_eq!(s.receptions[0].reason.as_deref(), Some("same_ingress"));
    assert_eq!(s.receptions[0].ready_ps, Some(579000));
}
#[test]
fn release_does_not_preempt_same_time_admission() {
    let mut p = fixture("source-full");
    p.ethernet.as_mut().unwrap().generators[0].times_ps = vec![0, 1, 672000];
    let snapshot = simulate(&p).unwrap();
    let s = snapshot.ethernet.unwrap();
    let second = s
        .transfers
        .iter()
        .find(|t| t.transfer_id == "a:1@Main.a.tx")
        .unwrap();
    assert_eq!(second.sof_ps, Some(672000));
    let third = s
        .transfers
        .iter()
        .find(|t| t.transfer_id == "a:2@Main.a.tx")
        .unwrap();
    assert_eq!(third.status, "dropped");
    assert_eq!(third.drop_reason.as_deref(), Some("queue_full"));
}
#[test]
fn endpoint_processing_stop_retains_planned_time_only() {
    let mut p = fixture("unicast");
    p.common.time_limit_ps = 1157000;
    p.ethernet
        .as_mut()
        .unwrap()
        .devices
        .iter_mut()
        .find(|d| d.id == "Main.b")
        .unwrap()
        .rx_processing_delay_ps = 1000;
    let snapshot = simulate(&p).unwrap();
    let s = snapshot.ethernet.unwrap();
    let r = s.receptions.iter().find(|r| r.device == "Main.b").unwrap();
    assert_eq!(r.status, "processing");
    assert_eq!(r.ready_ps, None);
    assert_eq!(r.planned_ready_ps, Some(1157000));
    assert_eq!(r.time_ps, 1156000);
}

#[test]
fn fanout_sequence_failure_preserves_all_candidates_and_reception() {
    let p = fixture("broadcast");
    let mut engine = initialize(&p).unwrap();
    engine.now = (0, 0, 1, 0);
    engine.handle(Event::Generate(0, 0)).unwrap();
    let source = engine.direction_indices[0];
    engine.now = (0, 0, 2, 1);
    engine.handle(Event::Start(source)).unwrap();
    engine.now = (576000, 0, 0, 2);
    engine.handle(Event::Eof(0)).unwrap();
    engine.now = (577000, 0, 1, 3);
    engine.handle(Event::Arrival(0)).unwrap();
    assert_eq!(engine.eth().receptions[0].status, "processing");
    assert_eq!(engine.eth().transfers.len(), 1);
    engine.dirty.clear();
    engine.sequence = u64::MAX - 1;
    engine.now = (579000, 0, 1, 4);
    let error = engine.handle(Event::Complete(0)).unwrap_err();
    assert_eq!(error.code, "E-0004");
    assert_eq!(engine.eth().receptions[0].status, "processing");
    assert_eq!(engine.eth().receptions[0].ready_ps, None);
    assert!(engine.eth().receptions[0].egress_transfer_ids.is_empty());
    assert_eq!(engine.eth().transfers.len(), 1);
    assert!(engine.dirty.is_empty());
    assert!(
        engine
            .queues
            .iter()
            .all(|q| q.waiting.iter().all(|q| q.is_empty()))
    );
}
#[test]
fn generation_dispatch_validates_future_input_without_reserving_all_events() {
    let mut p = fixture("unicast");
    p.ethernet.as_mut().unwrap().generators[0].times_ps = vec![p.common.time_limit_ps; 10000];
    let engine = initialize(&p).unwrap();
    assert!(engine.heap.is_empty());
    assert!(engine.future_generation);
    let snapshot = engine.run();
    assert_eq!(snapshot.common.pending_events, 1);
    assert_eq!(snapshot.common.committed_events, 0);
    assert_eq!(snapshot.common.termination, "time_limit");
}
#[test]
fn queue_points_use_reservation_sequence_and_local_effect_order() {
    let p = fixture("unicast");
    let mut engine = initialize(&p).unwrap();
    engine.now = (0, 0, 1, 37);
    engine.handle(Event::Generate(0, 0)).unwrap();
    engine.now = (1, 0, 1, 9);
    engine.handle(Event::Generate(0, 1)).unwrap();
    let points: Vec<_> = engine
        .snapshot
        .common
        .points
        .iter()
        .filter(|p| p.event_seq.is_some())
        .collect();
    assert_eq!(
        points.iter().map(|p| p.event_seq).collect::<Vec<_>>(),
        vec![Some(37), Some(9)]
    );
    assert!(points.iter().all(|p| p.effect_seq == Some(0)));
    assert_eq!(engine.snapshot.common.committed_events, 0);
}

#[test]
fn arrival_reservation_commits_with_eof_and_failure_preserves_prefix() {
    let p = fixture("unicast");
    let mut engine = initialize(&p).unwrap();
    engine.heap.clear();
    engine.now = (0, 0, 1, 0);
    engine.handle(Event::Generate(0, 0)).unwrap();
    let source = engine.direction_indices[0];
    engine.now = (0, 0, 2, 1);
    engine.handle(Event::Start(source)).unwrap();
    assert_eq!(engine.heap.len(), 2);
    assert!(
        !engine
            .heap
            .iter()
            .any(|e| matches!(e.0.1, Event::Arrival(_)))
    );
    engine.now = (576000, 0, 0, 2);
    let sequence = engine.sequence;
    engine.sequence = u64::MAX;
    assert_eq!(engine.handle(Event::Eof(0)).unwrap_err().code, "E-0004");
    assert_eq!(engine.eth().transfers[0].status, "transmitting");
    assert_eq!(engine.eth().transfers[0].eof_ps, None);
    assert_eq!(engine.heap.len(), 2);
    engine.sequence = sequence;
    engine.handle(Event::Eof(0)).unwrap();
    assert_eq!(engine.heap.len(), 3);
    assert!(
        engine
            .heap
            .iter()
            .any(|e| matches!(e.0.1, Event::Arrival(0)))
    );
}

fn qos_fixture(scheduler: &str) -> PreparedSimulation {
    let mut p = fixture("unicast");
    p.common.profile = "ethernet.l2.qos.v1".into();
    let model = p.ethernet.as_mut().unwrap();
    model.outputs = model
        .directions
        .iter()
        .map(|d| EthernetOutputConfig {
            port: d.from_port.clone(),
            scheduler: scheduler.into(),
            queues: (0..8)
                .map(|priority| EthernetQueueConfig {
                    priority,
                    capacity_frames: 64,
                    capacity_bytes: None,
                })
                .collect(),
        })
        .collect();
    for generator in &mut model.generators {
        generator.flow_id = Some(generator.id.clone());
    }
    p
}
#[test]
fn qos_strict_priority_is_nonpreemptive_and_fifo_preserves_offer_order() {
    for scheduler in ["fifo", "strict_priority"] {
        let mut p = qos_fixture(scheduler);
        p.common.time_limit_ps = 100_000_000;
        let model = p.ethernet.as_mut().unwrap();
        let mut active = model.generators[0].clone();
        active.id = "active".into();
        active.frame = serialize_frame(
            &active.frame.src_mac,
            &active.frame.dst_mac,
            2048,
            &"ff".repeat(1500),
        )
        .unwrap();
        let mut low = model.generators[0].clone();
        low.id = "low".into();
        low.times_ps = vec![1];
        let mut high = low.clone();
        high.id = "high".into();
        high.priority = 7;
        high.times_ps = vec![2];
        model.generators = vec![high, low, active];
        let s = simulate(&p).unwrap().ethernet.unwrap();
        let mut source = s
            .transfers
            .iter()
            .filter(|t| t.from_port == "Main.a.tx")
            .collect::<Vec<_>>();
        source.sort_by_key(|t| t.sof_ps);
        let order = source
            .iter()
            .map(|t| t.frame_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            if scheduler == "fifo" {
                vec!["active:0", "low:0", "high:0"]
            } else {
                vec!["active:0", "high:0", "low:0"]
            }
        );
        assert_eq!(source[0].sof_ps, Some(0));
        assert_eq!(source[0].release_ps, Some(12304000));
        assert_eq!(source[1].sof_ps, Some(12304000));
        assert_eq!(source[2].sof_ps, Some(12976000));
    }
}
#[test]
fn qos_capacity_bytes_counts_mac_and_excludes_active_frame() {
    let mut p = qos_fixture("fifo");
    let m = p.ethernet.as_mut().unwrap();
    m.generators[0].times_ps = vec![0, 1, 2];
    let output = m
        .outputs
        .iter_mut()
        .find(|o| o.port == "Main.a.tx")
        .unwrap();
    output.queues[0].capacity_frames = 1;
    output.queues[0].capacity_bytes = Some(64);
    let snapshot = simulate(&p).unwrap();
    let s = snapshot.ethernet.unwrap();
    let source = s
        .transfers
        .iter()
        .filter(|t| t.from_port == "Main.a.tx")
        .collect::<Vec<_>>();
    assert_eq!(source[0].status, "serialized");
    assert_eq!(source[1].status, "serialized");
    assert_eq!(source[2].status, "dropped");
    assert_eq!(source[2].queue_id.as_deref(), Some("Main.a.tx.queue.0"));
    for (target, metric, value) in [
        ("Main.a.tx.queue", "queue_length", 1),
        ("Main.a.tx.queue.0", "queue_length", 1),
        ("Main.a.tx.queue.0", "queue_bytes", 64),
    ] {
        assert!(
            snapshot
                .common
                .points
                .iter()
                .any(|point| point.request_id.as_deref() == Some("a:2")
                    && point.target == target
                    && point.metric == metric
                    && point.value == value)
        );
    }
    assert!(
        snapshot
            .common
            .points
            .iter()
            .any(|point| point.target == "Main.a.tx.queue.0"
                && point.metric == "queue_bytes"
                && point.value == 64)
    );
    let m = p.ethernet.as_mut().unwrap();
    m.outputs
        .iter_mut()
        .find(|o| o.port == "Main.a.tx")
        .unwrap()
        .queues[0]
        .capacity_bytes = Some(63);
    let s = simulate(&p).unwrap().ethernet.unwrap();
    assert_eq!(s.transfers.len(), 3);
    assert!(s.transfers.iter().all(|t| t.status == "dropped"));
}
#[test]
fn qos_device_capacity_bounds_all_priority_classes() {
    let mut p = qos_fixture("strict_priority");
    let m = p.ethernet.as_mut().unwrap();
    m.devices[0].queue_capacity = 1;
    let mut low = m.generators[0].clone();
    low.times_ps = vec![0, 2];
    let mut high = low.clone();
    high.id = "high".into();
    high.priority = 7;
    high.times_ps = vec![1];
    m.generators = vec![low, high];
    let s = simulate(&p).unwrap().ethernet.unwrap();
    let dropped = s.transfers.iter().find(|t| t.status == "dropped").unwrap();
    assert_eq!(dropped.transfer_id, "a:1@Main.a.tx");
    assert_eq!(dropped.priority, 0);
}
#[test]
fn qos_periodic_and_burst_generation_obey_count_end_and_global_stop() {
    let mut p = qos_fixture("fifo");
    p.common.time_limit_ps = 25;
    let m = p.ethernet.as_mut().unwrap();
    let mut periodic = m.generators[0].clone();
    periodic.id = "periodic".into();
    periodic.times_ps.clear();
    periodic.schedule = Some(EthernetSchedule::Periodic {
        start_ps: 0,
        phase_ps: 1,
        period_ps: 10,
        end_ps: Some(21),
        count: Some(5),
    });
    let mut burst = periodic.clone();
    burst.id = "burst".into();
    burst.schedule = Some(EthernetSchedule::Burst {
        start_ps: 0,
        period_ps: 20,
        burst_count: Some(2),
        frames_per_burst: 3,
        spacing_ps: 2,
        end_ps: Some(24),
    });
    m.generators = vec![periodic, burst];
    let s = simulate(&p).unwrap().ethernet.unwrap();
    let frames = s
        .frames
        .iter()
        .map(|f| (f.frame_id.as_str(), f.generated_ps))
        .collect::<Vec<_>>();
    assert_eq!(
        frames,
        vec![
            ("burst:0", 0),
            ("periodic:0", 1),
            ("burst:1", 2),
            ("burst:2", 4),
            ("periodic:1", 11),
            ("burst:3", 20),
            ("burst:4", 22)
        ]
    );
}
#[test]
fn qos_future_u128_schedule_is_not_truncated_to_u64() {
    let mut p = qos_fixture("fifo");
    p.common.time_limit_ps = u64::MAX;
    p.ethernet.as_mut().unwrap().generators[0].schedule = Some(EthernetSchedule::Periodic {
        start_ps: u64::MAX,
        phase_ps: 1,
        period_ps: 2,
        end_ps: None,
        count: None,
    });
    let s = simulate(&p).unwrap();
    assert!(!s.common.partial);
    assert_eq!(s.common.termination, "time_limit");
    assert_eq!(s.common.pending_events, 1);
    assert!(s.ethernet.unwrap().frames.is_empty());
}
#[test]
fn qos_fanout_byte_overflow_discards_entire_batch() {
    let mut p = qos_fixture("fifo");
    p.ethernet.as_mut().unwrap().generators[0].frame =
        serialize_frame("02:00:00:00:00:01", "ff:ff:ff:ff:ff:ff", 2048, "").unwrap();
    let mut engine = initialize(&p).unwrap();
    engine.now = (0, 0, 1, 0);
    engine.handle(Event::Generate(0, 0)).unwrap();
    let source = engine.direction_indices[0];
    engine.now = (0, 0, 2, 1);
    engine.handle(Event::Start(source)).unwrap();
    engine.now = (576000, 0, 0, 2);
    engine.handle(Event::Eof(0)).unwrap();
    engine.now = (577000, 0, 1, 3);
    engine.handle(Event::Arrival(0)).unwrap();
    let output = engine
        .model
        .directions
        .iter()
        .position(|d| d.from_port == "Main.sw.tx_c")
        .unwrap();
    engine.queues[output].bytes[0] = u64::MAX;
    engine.now = (579000, 0, 1, 4);
    assert_eq!(
        engine.handle(Event::Complete(0)).unwrap_err().code,
        "E-0004"
    );
    assert_eq!(engine.eth().transfers.len(), 1);
    assert_eq!(engine.eth().receptions[0].status, "processing");
    assert_eq!(engine.eth().receptions[0].ready_ps, None);
}

#[test]
fn qos_unbounded_same_time_burst_respects_max_events_without_expansion() {
    let mut p = qos_fixture("fifo");
    p.common.max_events = 5;
    p.ethernet.as_mut().unwrap().generators[0].schedule = Some(EthernetSchedule::Burst {
        start_ps: 0,
        period_ps: 1,
        burst_count: None,
        frames_per_burst: u64::MAX,
        spacing_ps: 0,
        end_ps: None,
    });
    let s = simulate(&p).unwrap();
    assert_eq!(s.common.committed_events, 5);
    assert_eq!(s.common.diagnostics[0].code, "E-0004");
    assert!(s.common.partial);
    assert_eq!(s.ethernet.unwrap().frames.len(), 2);
}

fn vlan_fixture() -> PreparedSimulation {
    let mut p = qos_fixture("strict_priority");
    p.common.profile = "ethernet.l2.vlan.v1".into();
    let model = p.ethernet.as_mut().unwrap();
    model.port_policies = model
        .directions
        .iter()
        .map(|d| {
            let reverse = model
                .directions
                .iter()
                .find(|r| r.source == d.destination && r.destination == d.source)
                .unwrap();
            EthernetPortPolicy {
                port: d.from_port.clone(),
                ingress: reverse.to_port.clone(),
                device: d.source,
                pvid: 10,
                admit: "all".into(),
                default_priority: if d.source == 3 { 2 } else { 0 },
                vlans: std::collections::BTreeMap::from([
                    (10, d.from_port == "Main.sw.tx_b"),
                    (20, true),
                ]),
            }
        })
        .collect();
    for device in &mut model.devices {
        device.fdb.clear();
    }
    let g = &mut model.generators[0];
    g.frame = serialize_vlan_frame(&g.frame.src_mac, "01:00:5e:00:00:01", 2048, "", None).unwrap();
    g.priority = 7;
    g.source_vlan_id = Some(10);
    p
}
#[test]
fn vlan_serializer_matches_independent_crc_lengths_and_tag_vectors() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../../../docs/verification/fixtures/ethernet-vlan/wire-vectors.json"
    ))
    .unwrap();
    for vector in vectors["vectors"].as_array().unwrap() {
        let i = &vector["input"];
        let tag = if i["vlan"].is_null() {
            None
        } else {
            Some(EthernetVlanTag {
                vid: i["vlan"]["vid"].as_u64().unwrap() as u16,
                pcp: i["vlan"]["pcp"].as_u64().unwrap() as u8,
                dei: i["vlan"]["dei"].as_u64().unwrap() as u8,
            })
        };
        let wire = serialize_vlan_frame(
            i["src_mac"].as_str().unwrap(),
            i["dst_mac"].as_str().unwrap(),
            u16::from_str_radix(i["inner_ethertype_hex"].as_str().unwrap(), 16).unwrap(),
            &i["payload"]["unit_hex"]
                .as_str()
                .unwrap()
                .repeat(i["payload"]["repeat"].as_u64().unwrap() as usize),
            tag,
        )
        .unwrap();
        let e = &vector["expected"];
        assert_eq!(
            wire.mac_bytes,
            e["mac_bytes"].as_u64().unwrap(),
            "{}",
            vector["name"]
        );
        assert_eq!(wire.pad_bytes, e["pad_bytes"].as_u64().unwrap());
        assert_eq!(wire.fcs_hex, e["fcs_hex"].as_str().unwrap());
        assert!(wire.mac_hex.starts_with(e["header_hex"].as_str().unwrap()));
        assert_eq!(
            duration((wire.mac_bytes + 8) * 8, 1_000_000_000).unwrap(),
            e["serialization_ps"].as_u64().unwrap()
        );
        assert_eq!(
            duration((wire.mac_bytes + 20) * 8, 1_000_000_000).unwrap(),
            e["occupied_ps"].as_u64().unwrap()
        );
    }
    assert!(serialize_frame("02:00:00:00:00:01", "01:00:5e:00:00:01", 2048, "").is_err());
}
#[test]
fn vlan_each_copy_owns_wire_classification_and_capacity_length() {
    let mut p = vlan_fixture();
    let m = p.ethernet.as_mut().unwrap();
    for id in ["Main.b", "Main.c"] {
        m.devices
            .iter_mut()
            .find(|d| d.id == id)
            .unwrap()
            .subscriptions
            .insert((10, "01:00:5e:00:00:01".into()));
    }
    let snapshot = simulate(&p).unwrap();
    assert!(!snapshot.common.partial);
    let s = snapshot.ethernet.unwrap();
    let b = s
        .transfers
        .iter()
        .find(|t| t.from_port == "Main.sw.tx_b")
        .unwrap();
    let c = s
        .transfers
        .iter()
        .find(|t| t.from_port == "Main.sw.tx_c")
        .unwrap();
    assert_eq!(s.frames[0].priority, 7);
    assert_eq!(b.priority, 2);
    assert_eq!(c.priority, 2);
    assert_eq!(
        b.wire.tag,
        Some(EthernetVlanTag {
            vid: 10,
            pcp: 2,
            dei: 0
        })
    );
    assert_eq!(b.wire.mac_bytes, 68);
    assert_eq!(c.wire.mac_bytes, 64);
    assert_eq!(b.eof_ps.unwrap() - b.sof_ps.unwrap(), 608000);
    assert_eq!(c.eof_ps.unwrap() - c.sof_ps.unwrap(), 576000);
    assert_ne!(b.wire.fcs_hex, c.wire.fcs_hex);
    assert_eq!(
        s.receptions
            .iter()
            .find(|r| r.device == "Main.b")
            .unwrap()
            .priority,
        2
    );
    assert_eq!(
        s.receptions
            .iter()
            .find(|r| r.device == "Main.c")
            .unwrap()
            .priority,
        0
    );
    assert!(
        snapshot
            .common
            .points
            .iter()
            .any(|p| p.target == "Main.sw.tx_b.queue.2"
                && p.metric == "queue_bytes"
                && p.value == 68)
    );
    p.ethernet
        .as_mut()
        .unwrap()
        .outputs
        .iter_mut()
        .find(|o| o.port == "Main.sw.tx_b")
        .unwrap()
        .queues[2]
        .capacity_bytes = Some(64);
    let s = simulate(&p).unwrap().ethernet.unwrap();
    assert_eq!(
        s.transfers.iter().filter(|t| t.status == "dropped").count(),
        1
    );
    let dropped = s.transfers.iter().find(|t| t.status == "dropped").unwrap();
    assert_eq!(dropped.from_port, "Main.sw.tx_b");
    assert_eq!(dropped.wire.mac_bytes, 68);
    assert_eq!(
        s.receptions
            .iter()
            .find(|r| r.device == "Main.sw")
            .unwrap()
            .status,
        "forwarded"
    );
}
#[test]
fn vlan_group_policies_membership_and_ingress_admission_have_single_reasons() {
    for case in 0..8 {
        let mut p = vlan_fixture();
        let m = p.ethernet.as_mut().unwrap();
        let sw = m.devices.iter_mut().find(|d| d.kind == "switch").unwrap();
        match case {
            0 => sw.unknown_multicast = "drop".into(),
            1 => {
                sw.multicast
                    .insert((10, "01:00:5e:00:00:01".into()), Vec::new());
            }
            2 => {
                sw.multicast.insert(
                    (10, "01:00:5e:00:00:01".into()),
                    vec!["Main.sw.tx_a".into()],
                );
            }
            3 => {
                sw.multicast.insert(
                    (10, "01:00:5e:00:00:01".into()),
                    vec!["Main.sw.tx_b".into()],
                );
            }
            4 => {
                for policy in &mut m.port_policies {
                    if matches!(policy.port.as_str(), "Main.sw.tx_b" | "Main.sw.tx_c") {
                        policy.vlans.remove(&10);
                    }
                }
            }
            5 => {
                m.port_policies
                    .iter_mut()
                    .find(|p| p.port == "Main.sw.tx_a")
                    .unwrap()
                    .admit = "tagged_only".into()
            }
            6 => {
                let policy = m
                    .port_policies
                    .iter_mut()
                    .find(|p| p.port == "Main.sw.tx_a")
                    .unwrap();
                policy.pvid = 20;
                policy.vlans.remove(&20);
            }
            7 => {
                m.port_policies
                    .iter_mut()
                    .find(|p| p.port == "Main.c.tx")
                    .unwrap()
                    .pvid = 20;
                m.devices
                    .iter_mut()
                    .find(|d| d.id == "Main.c")
                    .unwrap()
                    .subscriptions
                    .insert((20, "01:00:5e:00:00:01".into()));
            }
            _ => unreachable!(),
        }
        let s = simulate(&p).unwrap().ethernet.unwrap();
        let r = s.receptions.iter().find(|r| r.device == "Main.sw").unwrap();
        let expected = [
            Some("unknown_multicast"),
            Some("multicast_no_egress"),
            Some("multicast_no_egress"),
            None,
            Some("no_vlan_egress"),
            Some("ingress_frame_type"),
            Some("ingress_vlan_membership"),
            None,
        ][case];
        assert_eq!(r.reason.as_deref(), expected, "case {case}");
        if matches!(case, 5 | 6) {
            assert_eq!(r.ready_ps, Some(r.observed_ps));
            assert_eq!(r.planned_ready_ps, None);
        }
        if case == 3 {
            assert_eq!(s.transfers.len(), 2);
            assert_eq!(
                s.receptions
                    .iter()
                    .find(|r| r.device == "Main.b")
                    .unwrap()
                    .reason
                    .as_deref(),
                Some("multicast_not_subscribed")
            );
        }
        if case == 7 {
            let c = s.receptions.iter().find(|r| r.device == "Main.c").unwrap();
            assert_eq!(c.vlan_id, Some(20));
            assert_eq!(c.status, "received");
            assert_eq!(
                s.transfers
                    .iter()
                    .find(|t| t.from_port == "Main.sw.tx_c")
                    .unwrap()
                    .vlan_id,
                Some(10)
            );
        }
    }
}
#[test]
fn vlan_zero_delay_arrival_batch_failure_keeps_arrival_uncommitted() {
    let mut p = vlan_fixture();
    p.ethernet
        .as_mut()
        .unwrap()
        .devices
        .iter_mut()
        .find(|d| d.kind == "switch")
        .unwrap()
        .forward_delay_ps = 0;
    let mut e = initialize(&p).unwrap();
    e.now = (0, 0, 1, 0);
    e.handle(Event::Generate(0, 0)).unwrap();
    let d = e.direction_indices[0];
    e.now = (0, 0, 2, 1);
    e.handle(Event::Start(d)).unwrap();
    e.now = (576000, 0, 0, 2);
    e.handle(Event::Eof(0)).unwrap();
    let b = e
        .model
        .directions
        .iter()
        .position(|d| d.from_port == "Main.sw.tx_b")
        .unwrap();
    e.queues[b].bytes[2] = u64::MAX;
    let points = e.snapshot.common.points.len();
    let heap = e.heap.len();
    e.now = (577000, 0, 1, 3);
    assert_eq!(e.handle(Event::Arrival(0)).unwrap_err().code, "E-0004");
    assert_eq!(e.eth().transfers.len(), 1);
    assert_eq!(e.eth().transfers[0].arrival_ps, None);
    assert!(e.eth().receptions.is_empty());
    assert_eq!(e.snapshot.common.points.len(), points);
    assert_eq!(e.heap.len(), heap);
}
#[test]
fn vlan_generate_source_ready_offer_and_complete_failures_keep_callback_prefix() {
    for tx_delay in [0, 1] {
        let mut p = vlan_fixture();
        p.ethernet.as_mut().unwrap().devices[0].tx_processing_delay_ps = tx_delay;
        let mut e = initialize(&p).unwrap();
        e.now = (0, 0, 1, 0);
        if tx_delay == 0 {
            e.sequence = u64::MAX;
            assert!(e.handle(Event::Generate(0, 0)).is_err());
            assert!(e.eth().frames.is_empty());
            assert!(e.eth().transfers.is_empty());
        } else {
            e.handle(Event::Generate(0, 0)).unwrap();
            e.now = (1, 0, 0, 1);
            e.sequence = u64::MAX;
            assert!(e.handle(Event::SourceReady(0)).is_err());
            assert_eq!(e.eth().frames[0].ready_ps, None);
            assert!(e.eth().transfers.is_empty());
            e.sequence = 10;
            e.handle(Event::SourceReady(0)).unwrap();
            e.now = (1, 0, 1, 2);
            e.sequence = u64::MAX;
            assert!(e.handle(Event::Offer(0)).is_err());
            assert_eq!(e.eth().frames[0].ready_ps, None);
            assert!(e.eth().transfers.is_empty());
        }
    }
    let p = vlan_fixture();
    let mut e = initialize(&p).unwrap();
    e.now = (0, 0, 1, 0);
    e.handle(Event::Generate(0, 0)).unwrap();
    let d = e.direction_indices[0];
    e.now = (0, 0, 2, 1);
    e.handle(Event::Start(d)).unwrap();
    e.now = (576000, 0, 0, 2);
    e.handle(Event::Eof(0)).unwrap();
    e.now = (577000, 0, 1, 3);
    e.handle(Event::Arrival(0)).unwrap();
    let b = e
        .model
        .directions
        .iter()
        .position(|d| d.from_port == "Main.sw.tx_b")
        .unwrap();
    e.queues[b].bytes[2] = u64::MAX;
    let points = e.snapshot.common.points.len();
    e.now = (579000, 0, 1, 4);
    assert!(e.handle(Event::Complete(0)).is_err());
    assert_eq!(e.eth().receptions[0].status, "processing");
    assert_eq!(e.eth().receptions[0].ready_ps, None);
    assert!(e.eth().receptions[0].egress_transfer_ids.is_empty());
    assert_eq!(e.eth().transfers.len(), 1);
    assert_eq!(e.snapshot.common.points.len(), points);
}
