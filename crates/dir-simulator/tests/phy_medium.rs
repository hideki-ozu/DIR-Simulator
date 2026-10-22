use dir_simulator::runtime::ethernet::{
    crc32,
    phy::{self, channel::*, medium::*, receiver::*, *},
};
fn config(n: usize) -> MediumConfig {
    let grid = SampleGrid {
        symbol_rate_hz: 20_000_000,
        samples_per_ui: 8,
    };
    let endpoints: Vec<_> = (0..n)
        .map(|i| EndpointConfig {
            id: format!("port{i}"),
            transmitter: TransmitterConfig {
                grid,
                peak_uv: 1_000_000,
                transition_ui_ppm: 0,
                clock_ppm: 0,
                filter_taps_q30: vec![Q30],
            },
            receiver: ReceiverConfig {
                grid,
                peak_uv: 1_000_000,
                frontend_taps_q30: vec![Q30],
                adc_bits: 16,
                adc_full_scale_uv: 4_000_000,
                ffe_taps: 1,
                echo_taps: 0,
                adaptation_q30: Q30 / 4,
                minimum_training_symbols: 128,
                lock_error_ppm: 80_000,
                clock_ppm: 0,
                max_tracking_ppm: 0,
                max_no_transition_symbols: 128,
                loss_error_ppm: 500_000,
            },
        })
        .collect();
    MediumConfig {
        execution_contract: phy::wire::CONTRACT.into(),
        standard_edition: "project.numerical.v1".into(),
        physical_mode: PhysicalMode::ProjectNumerical,
        plca: None,
        repeater: None,
        gigabit_t1: None,
        gigabit_raw_merge: false,
        channel: ChannelConfig {
            grid,
            endpoints: endpoints.iter().map(|e| e.id.clone()).collect(),
            paths: (0..n)
                .map(|rx| {
                    (0..n)
                        .map(|tx| {
                            if rx == tx {
                                None
                            } else {
                                Some(Path {
                                    delay_samples: 8 + (rx + tx) * 2,
                                    taps_q30: vec![Q30],
                                })
                            }
                        })
                        .collect()
                })
                .collect(),
            noise_rms_uv: vec![0; n],
            noise_seed: 1,
            clip_uv: 10_000_000,
        },
        endpoints,
        half_duplex: true,
        carrier_threshold_uv: 100_000,
        carrier_hold_samples: 24,
        collision_threshold_uv: 200_000,
        ifg_bits: 96,
        jam_bits: 32,
        slot_bits: 512,
        retry_limit: 4,
        seed: 17,
        maximum_mac_bytes: 1518,
        maximum_samples: 500_000,
        trace_samples: false,
    }
}
fn frame(id: &str, source: &str, at_ps: u64) -> MacTx {
    let mut bytes = vec![0x55; 60];
    bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    bytes[12..14].copy_from_slice(&[8, 0]);
    bytes.extend(crc32(&bytes).to_le_bytes());
    MacTx {
        frame_id: id.into(),
        source_endpoint: source.into(),
        at_ps,
        mac_bytes: bytes,
    }
}
fn run(m: &mut SampledMedium) -> Vec<PhyEffect> {
    let mut out = vec![];
    while m.has_pending() {
        out.extend(m.advance_one_sample().unwrap());
    }
    out
}
#[test]
fn prior_single_origin_checkpoint_shape_restores() {
    let c = config(2);
    let mut medium = SampledMedium::new(c.clone()).unwrap();
    medium.admit(frame("f", "port0", 0)).unwrap();
    for _ in 0..200 {
        medium.advance_one_sample().unwrap();
    }
    let mut saved: serde_json::Value =
        serde_json::from_slice(&medium.checkpoint().unwrap()).unwrap();
    let histories = saved["state"]["origin_history"].as_array_mut().unwrap();
    for history in histories {
        for entry in history.as_array_mut().unwrap() {
            let origins = entry.as_array().unwrap();
            *entry = origins.first().cloned().unwrap_or(serde_json::Value::Null);
        }
    }
    saved["state"].as_object_mut().unwrap().remove("repeater");
    saved["config"].as_object_mut().unwrap().remove("repeater");
    let restored = SampledMedium::restore(&c, &serde_json::to_vec(&saved).unwrap()).unwrap();
    assert_eq!(restored.next_sample_index(), medium.next_sample_index());
    assert_eq!(run(&mut medium.clone()), run(&mut restored.clone()));
}
#[test]
fn manchester_independent_vector_and_real_crc_failure() {
    assert_eq!(
        phy::wire::encode_bytes(&[0xa6]),
        vec![-1, 1, 1, -1, 1, -1, -1, 1, -1, 1, 1, -1, -1, 1, 1, -1]
    );
    let f = frame("f", "port0", 0);
    let chips = phy::wire::encode_mac(&f.mac_bytes, 1518).unwrap();
    let mut d = phy::wire::Decoder::new(1518);
    let mut starts = 0;
    for chip in chips {
        if d.push(chip, true).unwrap() == Some(phy::wire::DecodeEffect::Start) {
            starts += 1;
        }
    }
    assert_eq!(starts, 1);
    assert_eq!(d.finish().unwrap(), f.mac_bytes);
    let mut bad = f.mac_bytes;
    bad[20] ^= 1;
    assert!(phy::wire::encode_mac(&bad, 1518).is_err());
    let mut chips = phy::wire::pilot();
    chips.extend(phy::wire::encode_bytes(&[
        0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5,
    ]));
    chips.extend(phy::wire::encode_bytes(&bad));
    let mut d = phy::wire::Decoder::new(1518);
    for chip in chips {
        d.push(chip, true).unwrap();
    }
    assert!(d.finish().unwrap_err().contains("FCS"));
}
#[test]
fn actual_bytes_cross_samples_and_arriving_carrier() {
    let c = config(2);
    let mut m = SampledMedium::new(c).unwrap();
    let f = frame("f", "port0", 0);
    m.admit(f.clone()).unwrap();
    let out = run(&mut m);
    let rx: Vec<_> = out
        .iter()
        .filter_map(|e| {
            if let PhyEffect::RxFrame {
                receiver_endpoint,
                mac_bytes,
                origins,
                sample_index,
                ..
            } = e
            {
                Some((receiver_endpoint, mac_bytes, origins, *sample_index))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(rx.len(), 1);
    assert_eq!(rx[0].0, "port1");
    assert_eq!(rx[0].1, &f.mac_bytes);
    assert_eq!(rx[0].2[0].frame_id, "f");
    let arrival = out
        .iter()
        .find_map(|e| {
            if let PhyEffect::CarrierChanged {
                endpoint,
                present: true,
                sample_index,
            } = e
            {
                (endpoint == "port1").then_some(*sample_index)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(arrival, 11);
}
#[test]
fn checkpoint_at_physical_sample_is_exact() {
    let c = config(2);
    let mut a = SampledMedium::new(c.clone()).unwrap();
    a.admit(frame("f", "port0", 0)).unwrap();
    for _ in 0..2501 {
        a.advance_one_sample().unwrap();
    }
    let saved = a.checkpoint().unwrap();
    let mut b = SampledMedium::restore(&c, &saved).unwrap();
    assert_eq!(run(&mut a), run(&mut b));
    assert_eq!(a.checkpoint().unwrap(), b.checkpoint().unwrap());
}
#[test]
fn three_local_starts_collide_then_seeded_mac_retries() {
    let mut c = config(4);
    c.retry_limit = 8;
    let mut m = SampledMedium::new(c).unwrap();
    for i in 0..3 {
        m.admit(frame(&format!("f{i}"), &format!("port{i}"), 0))
            .unwrap();
    }
    let effects = run(&mut m);
    let initial: Vec<_> = effects
        .iter()
        .filter_map(|e| {
            if let PhyEffect::TxStarted {
                origin,
                sample_index,
                ..
            } = e
            {
                (origin.attempt == 0).then_some(*sample_index)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(initial, vec![1, 1, 1]);
    let collisions: std::collections::BTreeSet<_> = effects
        .iter()
        .filter_map(|e| {
            if let PhyEffect::Collision { endpoint, .. } = e {
                Some(endpoint.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(collisions.len(), 3);
    assert!(effects.iter().any(|e| matches!(e, PhyEffect::Retry { .. })));
    assert!(effects.iter().any(
        |e| matches!(e,PhyEffect::RxFrame{receiver_endpoint,..}if receiver_endpoint=="port3")
    ));
}
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(c: MediumConfig, requests: Vec<MacTx>) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dir-phy-medium-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/phy")).unwrap();
        std::fs::write(root.join("ned/phy/model.ned"),"package phy; simple Medium {parameters: @class(\"ethernet.SampledPhy\");} network Demo{submodules: medium: phy.Medium;}").unwrap();
        std::fs::write(
            root.join("model.json"),
            serde_json::to_vec(&phy::execution::Configuration {
                schema: "dir.ethernet-phy.model".into(),
                version: 1,
                medium: c,
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("workload.json"),
            serde_json::to_vec(&phy::execution::Workload {
                schema: "dir.ethernet-phy.workload".into(),
                version: 1,
                requests,
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("scenario.ini"),"[General]\nnetwork=phy.Demo\nned-path=\"ned\"\nmodel-profile=\"ethernet.phy.v1\"\nmodel-config=\"model.json\"\nworkload=\"workload.json\"\nsim-time-limit=10000000000ps\nmetrics-window=10000000ps\n").unwrap();
        Self(root)
    }
    fn prepared(&self) -> dir_simulator::PreparedSimulation {
        dir_simulator::prepare_with_registry(&self.0.join("scenario.ini"), registry()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn registry() -> dir_simulator::registry::Registry {
    let mut r = dir_simulator::registry::Registry::empty();
    phy::execution::register(&mut r).unwrap();
    r
}
#[test]
fn registered_public_session_one_sample_pause_export_checkpoint_resume() {
    use dir_simulator::runtime::session::*;
    let f = Fixture::new(config(2), vec![frame("f", "port0", 0)]);
    let full = dir_simulator::runtime::simulate(&f.prepared()).unwrap();
    assert!(
        full.registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .any(|r| r.data["kind"] == "RxFrame")
    );
    let (mut session, controller) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .is_empty()
    );
    for _ in 0..4001 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
    }
    let before = session.snapshot().common.committed_events;
    controller.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    let rows = serde_json::to_value(
        &session
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records,
    )
    .unwrap();
    assert!(
        !rows
            .as_object()
            .unwrap()
            .values()
            .any(|r| r["data"]["kind"] == "RxFrame")
    );
    assert_eq!(
        session.export(&f.0.join("prefix")).unwrap().status,
        ControlStatus::Applied
    );
    assert_eq!(session.snapshot().common.committed_events, before);
    assert_eq!(
        serde_json::to_value(
            &session
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
        )
        .unwrap(),
        rows
    );
    let path = f.0.join("checkpoint");
    let ack = session
        .checkpoint(&path, CheckpointLimits::default())
        .unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    let (mut restored, token) =
        restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
    assert_eq!(restored.snapshot().common.committed_events, before);
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(
        serde_json::to_value(
            &restored
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
        )
        .unwrap(),
        serde_json::to_value(&full.registered.as_ref().unwrap().model_records).unwrap()
    );
}
#[test]
fn zero_channel_never_invents_link_or_frame_and_budget_failure_is_atomic() {
    let mut c = config(2);
    c.channel.paths = vec![vec![None; 2]; 2];
    let mut m = SampledMedium::new(c.clone()).unwrap();
    m.admit(frame("f", "port0", 0)).unwrap();
    let effects = run(&mut m);
    assert!(!effects.iter().any(|e| matches!(
        e,
        PhyEffect::RxFrame { .. }
            | PhyEffect::Receiver {
                event: ReceiverEvent::LockAcquired { .. },
                ..
            }
    )));
    c.maximum_samples = 20;
    let mut m = SampledMedium::new(c).unwrap();
    m.admit(frame("f", "port0", 0)).unwrap();
    for _ in 0..20 {
        m.advance_one_sample().unwrap();
    }
    let bytes = m.checkpoint().unwrap();
    assert!(m.advance_one_sample().unwrap_err().contains("budget"));
    assert_eq!(m.checkpoint().unwrap(), bytes);
}
#[test]
fn checkpoint_rejects_removed_pending_admission_changed_payload_and_physical_cursor() {
    let c = config(2);
    let mut m = SampledMedium::new(c.clone()).unwrap();
    m.admit(frame("f", "port0", 0)).unwrap();
    for _ in 0..4097 {
        m.advance_one_sample().unwrap();
    }
    let saved = m.checkpoint().unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    v["state"]["ports"][0]["queue"] = serde_json::json!([]);
    assert!(SampledMedium::restore(&c, &serde_json::to_vec(&v).unwrap()).is_err());
    let mut v: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    v["state"]["index"] = serde_json::json!(4098);
    assert!(SampledMedium::restore(&c, &serde_json::to_vec(&v).unwrap()).is_err());
    let mut v: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    v["state"]["ports"][0]["queue"][0]["mac_bytes"][20] = serde_json::json!(7);
    assert!(SampledMedium::restore(&c, &serde_json::to_vec(&v).unwrap()).is_err());
}
#[test]
fn frames_and_lock_transitions_agree_at_32_and_64_samples_per_chip() {
    let mut results = vec![];
    for samples in [32, 64] {
        let mut c = config(2);
        let scale = samples / 8;
        c.channel.grid.samples_per_ui = samples;
        c.carrier_hold_samples *= scale;
        for e in &mut c.endpoints {
            e.transmitter.grid = c.channel.grid;
            e.receiver.grid = c.channel.grid;
        }
        for p in c.channel.paths.iter_mut().flatten().flatten() {
            p.delay_samples *= scale as usize;
        }
        let mut m = SampledMedium::new(c.clone()).unwrap();
        m.admit(frame("f", "port0", 0)).unwrap();
        let effects = run(&mut m);
        let frames: Vec<_> = effects
            .iter()
            .filter_map(|e| {
                if let PhyEffect::RxFrame { mac_bytes, .. } = e {
                    Some(mac_bytes.clone())
                } else {
                    None
                }
            })
            .collect();
        let lock = effects
            .iter()
            .find_map(|e| {
                if let PhyEffect::Receiver {
                    event: ReceiverEvent::LockAcquired { sample_index, .. },
                    ..
                } = e
                {
                    Some(*sample_index)
                } else {
                    None
                }
            })
            .unwrap();
        results.push((frames, c.channel.grid.time(lock).unwrap()));
    }
    assert_eq!(results[0].0, results[1].0);
    assert_eq!(results[0].0.len(), 1);
    let a = results[0].1;
    let b = results[1].1;
    let diff = (a.numerator_ps * b.denominator).abs_diff(b.numerator_ps * a.denominator);
    let one_tick = SampleGrid {
        symbol_rate_hz: 20_000_000,
        samples_per_ui: 32,
    }
    .time(1)
    .unwrap();
    assert!(diff * one_tick.denominator <= one_tick.numerator_ps * a.denominator * b.denominator);
}
#[test]
fn unknown_transmitter_drift_uses_sampled_training_and_decodes_mac() {
    for ppm in [-1000, 1000] {
        let mut c = config(2);
        c.channel.grid.samples_per_ui = 32;
        c.carrier_hold_samples = 96;
        for e in &mut c.endpoints {
            e.transmitter.grid = c.channel.grid;
            e.receiver.grid = c.channel.grid;
            e.receiver.max_tracking_ppm = 3000;
        }
        for p in c.channel.paths.iter_mut().flatten().flatten() {
            p.delay_samples *= 4;
        }
        c.endpoints[0].transmitter.clock_ppm = ppm;
        let mut m = SampledMedium::new(c).unwrap();
        let tx = frame("f", "port0", 0);
        m.admit(tx.clone()).unwrap();
        let effects = run(&mut m);
        let frames: Vec<_> = effects
            .iter()
            .filter_map(|e| {
                if let PhyEffect::RxFrame { mac_bytes, .. } = e {
                    Some(mac_bytes)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            frames,
            vec![&tx.mac_bytes],
            "ppm={ppm}; aborts={:?}",
            effects
                .iter()
                .filter(|e| matches!(
                    e,
                    PhyEffect::RxAbort { .. }
                        | PhyEffect::Receiver {
                            event: ReceiverEvent::LockLost { .. },
                            ..
                        }
                ))
                .collect::<Vec<_>>()
        );
    }
}
#[test]
fn state_events_never_precede_observation_and_unsupported_editions_reject() {
    let c = config(2);
    let mut m = SampledMedium::new(c.clone()).unwrap();
    m.admit(frame("f", "port0", 0)).unwrap();
    while m.has_pending() {
        let boundary = m.next_sample_index() + 1;
        for effect in m.advance_one_sample().unwrap() {
            if let PhyEffect::Receiver { event, .. } = effect {
                let at = match event {
                    ReceiverEvent::TrainingStarted { sample_index, .. }
                    | ReceiverEvent::LockAcquired { sample_index, .. }
                    | ReceiverEvent::LockLost { sample_index, .. }
                    | ReceiverEvent::FrameAborted { sample_index, .. }
                    | ReceiverEvent::FrameCompleted { sample_index, .. } => sample_index,
                };
                assert_eq!(at, boundary);
            }
        }
    }
    for edition in ["IEEE802.3-2022", "IEEE802.3cg-2019"] {
        let mut c = c.clone();
        c.standard_edition = edition.into();
        assert!(
            SampledMedium::new(c)
                .unwrap_err()
                .contains("contract/edition mismatch")
        );
    }
}
#[test]
fn independent_duplex_both_endpoints_transmit_and_decode_concurrently() {
    let mut c = config(2);
    c.half_duplex = false;
    let mut a = frame("a", "port0", 0);
    let mut b = frame("b", "port1", 0);
    b.mac_bytes[20] = 0x39;
    let end = b.mac_bytes.len() - 4;
    let crc = crc32(&b.mac_bytes[..end]).to_le_bytes();
    b.mac_bytes[end..].copy_from_slice(&crc);
    a.mac_bytes[21] = 0x7a;
    let end = a.mac_bytes.len() - 4;
    let crc = crc32(&a.mac_bytes[..end]).to_le_bytes();
    a.mac_bytes[end..].copy_from_slice(&crc);
    let mut m = SampledMedium::new(c).unwrap();
    m.admit(a.clone()).unwrap();
    m.admit(b.clone()).unwrap();
    let effects = run(&mut m);
    for (endpoint, expected) in [("port0", &b.mac_bytes), ("port1", &a.mac_bytes)] {
        let actual: Vec<_> = effects
            .iter()
            .filter_map(|e| {
                if let PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } = e
                {
                    (receiver_endpoint == endpoint).then_some(mac_bytes)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(actual, vec![expected]);
    }
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, PhyEffect::Collision { .. }))
    );
}
#[test]
fn hidden_transmitter_midframe_interference_aborts_actual_receive() {
    let mut c = config(3);
    c.retry_limit = 0;
    c.channel.paths[1][0] = None;
    let mut m = SampledMedium::new(c.clone()).unwrap();
    m.admit(frame("first", "port0", 0)).unwrap();
    let at = c.channel.grid.time(6000).unwrap();
    m.admit(frame(
        "hidden",
        "port1",
        (at.numerator_ps / at.denominator) as u64,
    ))
    .unwrap();
    let effects = run(&mut m);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e,PhyEffect::RxStarted{endpoint,..}if endpoint=="port2"))
    );
    assert!(effects.iter().any(|e|matches!(e,PhyEffect::RxAbort{endpoint,..}if endpoint=="port2")||matches!(e,PhyEffect::Receiver{endpoint,event:ReceiverEvent::FrameAborted{..}}if endpoint=="port2")));
    assert!(!effects.iter().any(
        |e| matches!(e,PhyEffect::RxFrame{receiver_endpoint,..}if receiver_endpoint=="port2")
    ));
}
fn attachment_config() -> phy::attachment::AttachmentConfig {
    phy::attachment::AttachmentConfig {
        origin_ps: 123,
        medium: config(2),
        bindings: vec![phy::attachment::PortBinding {
            direction_id: "link-forward".into(),
            from_port: "port0".into(),
            to_port: "port1".into(),
        }],
        physical_ports: vec![],
        lookahead_samples: None,
    }
}
#[test]
fn attachment_releases_at_ifg_then_delivers_actual_decoded_metadata() {
    use phy::attachment::*;
    let c = attachment_config();
    let source = dir_simulator::runtime::ethernet::serialize_frame(
        "02:00:00:00:00:01",
        "02:00:00:00:00:02",
        0x0800,
        "deadbeef",
    )
    .unwrap();
    let identity = AttachedMedium::identity(
        "external-transfer",
        "link-forward",
        &source,
        c.medium.maximum_mac_bytes,
    )
    .unwrap();
    let mut a = AttachedMedium::new(c.clone()).unwrap();
    assert!(a.tx_ready("port0").unwrap());
    a.admit("link-forward", "external-transfer", c.origin_ps, &source)
        .unwrap();
    let mut effects = vec![];
    for _ in 0..4099 {
        effects.extend(a.advance_one_sample().unwrap());
    }
    let bytes = a.checkpoint().unwrap();
    let mut b = AttachedMedium::restore(&c, &bytes).unwrap();
    b.validate_external_admissions(&[identity]).unwrap();
    let mut tail_a = vec![];
    let mut tail_b = vec![];
    while a.has_pending() {
        tail_a.extend(a.advance_one_sample().unwrap());
        tail_b.extend(b.advance_one_sample().unwrap());
    }
    assert_eq!(
        serde_json::to_value(&tail_a).unwrap(),
        serde_json::to_value(&tail_b).unwrap()
    );
    effects.extend(tail_a);
    let serialized = effects
        .iter()
        .position(|e| matches!(e, AttachmentEffect::TxSerialized { success: true, .. }))
        .unwrap();
    let release = effects
        .iter()
        .position(|e| matches!(e, AttachmentEffect::TxReleased { success: true, .. }))
        .unwrap();
    assert!(release > serialized);
    let mut index = 0;
    let mut serialized_index = 0;
    let mut released_index = 0;
    let mut frames = vec![];
    for effect in effects {
        match effect {
            AttachmentEffect::Physical {
                effect: PhyEffect::TxFinished { sample_index, .. },
            } => serialized_index = sample_index,
            AttachmentEffect::Physical {
                effect:
                    PhyEffect::TxReady {
                        endpoint,
                        ready: true,
                        sample_index,
                    },
            } if endpoint == "port0" => released_index = sample_index,
            AttachmentEffect::Received { frame, to_port, .. } => {
                assert_eq!(to_port, "port1");
                frames.push(frame);
            }
            _ => {}
        }
        index += 1;
    }
    assert!(index > 0);
    assert!(
        released_index
            >= serialized_index
                + c.medium.ifg_bits as u64 * 2 * c.medium.channel.grid.samples_per_ui as u64
                - 1
    );
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].wire.mac_hex, source.mac_hex);
    assert_eq!(frames[0].wire.pad_bytes, 0);
    assert_eq!(frames[0].wire.data_hex.len(), 92);
    assert!(frames[0].wire.data_hex.starts_with("deadbeef"));
    assert!(a.tx_ready("port0").unwrap());
    assert_eq!(a.checkpoint().unwrap(), b.checkpoint().unwrap());
    assert!(a.validate_external_admissions(&[]).is_err());
}
#[test]
fn attachment_derives_c_vlan_rtag_and_rejects_metadata_payload_forgery() {
    use phy::attachment::*;
    let mut source = dir_simulator::runtime::ethernet::serialize_vlan_frame(
        "02:00:00:00:00:01",
        "01:00:5e:00:00:01",
        0x0800,
        "010203",
        Some(dir_simulator::types::ethernet::EthernetVlanTag {
            vid: 7,
            pcp: 3,
            dei: 1,
        }),
    )
    .unwrap();
    let mut bytes = canonical_bytes(&source, 1518).unwrap();
    bytes.truncate(bytes.len() - 4);
    bytes.splice(16..16, [0xf1, 0xc1, 0, 0, 0x12, 0x34]);
    let fcs = crc32(&bytes).to_le_bytes();
    bytes.extend(fcs);
    source.mac_hex = hex_bytes(&bytes);
    source.mac_bytes = bytes.len() as u64;
    source.fcs_hex = hex_bytes(&fcs);
    assert_eq!(canonical_bytes(&source, 1518).unwrap(), bytes);
    let parsed = decode_mac(&bytes, 1518).unwrap();
    assert_eq!(parsed.rtag.unwrap().sequence, 0x1234);
    assert_eq!(parsed.wire.tag.unwrap().vid, 7);
    assert_eq!(parsed.wire.ether_type, 0x0800);
    assert!(parsed.wire.data_hex.starts_with("010203"));
    source.data_hex = "990203".into();
    assert!(canonical_bytes(&source, 1518).is_err());
}
#[test]
fn attachment_drain_reports_failed_receive_after_actual_source_release() {
    use phy::attachment::*;
    let mut c = attachment_config();
    c.medium.channel.paths[1][0].as_mut().unwrap().taps_q30 = vec![0];
    let wire = dir_simulator::runtime::ethernet::serialize_frame(
        "02:00:00:00:00:01",
        "02:00:00:00:00:02",
        0x0800,
        "01",
    )
    .unwrap();
    let mut a = AttachedMedium::new(c.clone()).unwrap();
    a.admit("link-forward", "missing-rx", c.origin_ps, &wire)
        .unwrap();
    let mut all = vec![];
    while a.has_pending() {
        all.extend(a.advance_one_sample().unwrap());
    }
    let release = all
        .iter()
        .position(|e| matches!(e, AttachmentEffect::TxReleased { .. }))
        .unwrap();
    let failed = all
        .iter()
        .position(
            |e| matches!(e,AttachmentEffect::Failed{reason,..}if reason=="no_valid_received_frame"),
        )
        .unwrap();
    assert!(failed > release);
    assert!(
        !all.iter()
            .any(|e| matches!(e, AttachmentEffect::Received { .. }))
    );
}
#[test]
fn independent_duplex_ifg_readiness_does_not_wait_for_long_incoming_frame() {
    let mut c = config(2);
    c.half_duplex = false;
    let mut long = frame("long", "port1", 0);
    long.mac_bytes.truncate(60);
    long.mac_bytes.extend((0..176).map(|i| i as u8));
    let crc = crc32(&long.mac_bytes).to_le_bytes();
    long.mac_bytes.extend(crc);
    let mut m = SampledMedium::new(c).unwrap();
    m.admit(frame("short0", "port0", 0)).unwrap();
    m.admit(frame("short1", "port0", 0)).unwrap();
    m.admit(long).unwrap();
    let effects = run(&mut m);
    let second = effects
        .iter()
        .find_map(|e| {
            if let PhyEffect::TxStarted {
                origin,
                sample_index,
                ..
            } = e
            {
                (origin.frame_id == "short1").then_some(*sample_index)
            } else {
                None
            }
        })
        .unwrap();
    let incoming_end = effects
        .iter()
        .find_map(|e| {
            if let PhyEffect::RxFrame {
                receiver_endpoint,
                sample_index,
                ..
            } = e
            {
                (receiver_endpoint == "port0").then_some(*sample_index)
            } else {
                None
            }
        })
        .unwrap();
    assert!(second < incoming_end);
    assert_eq!(effects.iter().filter(|e|matches!(e,PhyEffect::RxFrame{receiver_endpoint,..}if receiver_endpoint=="port1")).count(),2);
}
#[test]
fn attachment_rejects_skipped_sample_clock_and_accepts_causal_idle_advance() {
    use phy::attachment::*;
    let c = attachment_config();
    let source = dir_simulator::runtime::ethernet::serialize_frame(
        "02:00:00:00:00:01",
        "02:00:00:00:00:02",
        0x0800,
        "01",
    )
    .unwrap();
    let mut a = AttachedMedium::new(c.clone()).unwrap();
    let saved = a.checkpoint().unwrap();
    assert!(
        a.admit("link-forward", "late", c.origin_ps + 1000000, &source)
            .unwrap_err()
            .contains("clock")
    );
    assert_eq!(saved, a.checkpoint().unwrap());
    for _ in 0..160 {
        a.advance_one_sample().unwrap();
    }
    let now = a.sample_time_ps(a.next_sample_index()).unwrap();
    a.admit("link-forward", "actual", now, &source).unwrap();
    let mut received = 0;
    while a.has_pending() {
        received += a
            .advance_one_sample()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AttachmentEffect::Received { .. }))
            .count();
    }
    assert_eq!(received, 1);
}

fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    for byte in bytes {
        write!(result, "{byte:02x}").unwrap();
    }
    result
}
