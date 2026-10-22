use dir_simulator::runtime::ethernet::phy::ieee::repeater;
use dir_simulator::runtime::ethernet::{
    crc32,
    phy::{attachment::*, channel::*, medium::*, receiver::*, *},
};

fn pair(samples_per_chip: u32) -> MediumConfig {
    let grid = SampleGrid {
        symbol_rate_hz: 20_000_000,
        samples_per_ui: samples_per_chip,
    };
    let endpoints: Vec<_> = (0..2)
        .map(|i| EndpointConfig {
            id: format!("p{i}"),
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
                adaptation_q30: 0,
                minimum_training_symbols: 32,
                lock_error_ppm: 100_000,
                clock_ppm: 0,
                max_tracking_ppm: 0,
                max_no_transition_symbols: 128,
                loss_error_ppm: 500_000,
            },
        })
        .collect();
    MediumConfig {
        execution_contract: "ieee8023.10base_t.half.2022".into(),
        standard_edition: "IEEE Std 802.3-2022".into(),
        physical_mode: PhysicalMode::Ieee10BaseT2022,
        plca: None,
        repeater: None,
        gigabit_t1: None,
        gigabit_raw_merge: false,
        channel: ChannelConfig {
            grid,
            endpoints: endpoints.iter().map(|e| e.id.clone()).collect(),
            paths: (0..2)
                .map(|rx| {
                    (0..2)
                        .map(|tx| {
                            Some(Path {
                                delay_samples: if rx == tx {
                                    0
                                } else {
                                    samples_per_chip as usize
                                },
                                taps_q30: vec![Q30],
                            })
                        })
                        .collect()
                })
                .collect(),
            noise_rms_uv: vec![0; 2],
            noise_seed: 1,
            clip_uv: 4_000_000,
        },
        endpoints,
        half_duplex: true,
        carrier_threshold_uv: 100_000,
        carrier_hold_samples: samples_per_chip * 2,
        collision_threshold_uv: 200_000,
        ifg_bits: 96,
        jam_bits: 32,
        slot_bits: 512,
        retry_limit: 16,
        seed: 11,
        maximum_mac_bytes: 1518,
        maximum_samples: 200_000,
        trace_samples: false,
    }
}
fn request() -> MacTx {
    let mut bytes = vec![0xa5; 60];
    bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    bytes[12..14].copy_from_slice(&[8, 0]);
    bytes.extend(crc32(&bytes).to_le_bytes());
    MacTx {
        frame_id: "f".into(),
        source_endpoint: "p0".into(),
        at_ps: 0,
        mac_bytes: bytes,
    }
}
#[test]
fn ieee_8023_llc_length_and_padding_roundtrip_without_accepting_reserved_gap() {
    let mut bytes = vec![0x01, 0x80, 0xc2, 0, 0, 0, 0x02, 0, 0, 0, 0, 1];
    let llc = [0x42, 0x42, 0x03, 0, 0, 0, 0];
    bytes.extend((llc.len() as u16).to_be_bytes());
    bytes.extend(llc);
    bytes.resize(60, 0);
    bytes.extend(crc32(&bytes).to_le_bytes());
    let decoded = decode_mac(&bytes, 1518).unwrap();
    assert_eq!(decoded.wire.ether_type, llc.len() as u16);
    assert_eq!(decoded.wire.data_hex, "42420300000000");
    assert_eq!(decoded.wire.pad_bytes, 39);
    assert_eq!(canonical_bytes(&decoded.wire, 1518).unwrap(), bytes);
    let mut inconsistent = decoded.wire.clone();
    inconsistent.pad_bytes -= 1;
    assert!(canonical_bytes(&inconsistent, 1518).is_err());
    let mut bad = bytes.clone();
    for length in [47u16, 1501, 1535] {
        bad[12..14].copy_from_slice(&length.to_be_bytes());
        bad.truncate(60);
        bad.extend(crc32(&bad).to_le_bytes());
        assert!(decode_mac(&bad, 1518).is_err(), "length={length}");
    }
}
#[test]
fn tenbase_t_prior_contract_spelling_remains_an_explicit_alias() {
    let mut config = pair(16);
    config.execution_contract = "ieee8023.10base_t.2022".into();
    config.validate().unwrap();
    assert_eq!(config.physical_mode, PhysicalMode::Ieee10BaseT2022);
    config.half_duplex = false;
    assert!(config.validate().is_err());
}
#[test]
fn mac_permission_blocks_new_attempt_until_exact_sample_cut() {
    let mut medium = SampledMedium::new(pair(16)).unwrap();
    medium.set_mac_start_permission("p0", None).unwrap();
    medium.admit(request()).unwrap();
    assert!(medium.tx_ready("p0").unwrap());
    assert!(
        !medium
            .advance_samples(1_000)
            .unwrap()
            .iter()
            .any(|e| matches!(e, PhyEffect::TxStarted { .. }))
    );
    let cut = 1_008;
    let deadline = sample_boundary_ps(medium.config().channel.grid, cut).unwrap();
    medium
        .set_mac_start_permission("p0", Some(deadline))
        .unwrap();
    assert_eq!(medium.mac_start_permission("p0").unwrap(), Some(deadline));
    let started = medium
        .advance_samples(32)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            PhyEffect::TxStarted {
                endpoint,
                sample_index,
                ..
            } if endpoint == "p0" => Some(sample_index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(started, vec![cut]);
}
#[test]
#[ignore = "requires 5.12 million exact physical samples to reach the normative 16 ms NLP interval"]
fn tenbase_t_full_medium_pulse_traverses_channel_without_mac_carrier() {
    let mut config = pair(16);
    config.trace_samples = true;
    config.maximum_samples = 5_121_000;
    config.channel.paths[0][0] = None;
    config.channel.paths[1][1] = None;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut tx_positive = Vec::new();
    let mut rx_positive = Vec::new();
    let mut carrier_or_frame = 0;
    while medium.next_sample_index() < 5_120_160 {
        let count = (5_120_160 - medium.next_sample_index()).min(4096);
        for effect in medium.advance_samples(count).unwrap() {
            match effect {
                PhyEffect::Samples {
                    sample_index,
                    transmitted_uv,
                    received_uv,
                } if sample_index >= 5_120_000 => {
                    if transmitted_uv[0] > 0 {
                        tx_positive.push(sample_index);
                    }
                    if received_uv[1] > 0 {
                        rx_positive.push(sample_index);
                    }
                }
                PhyEffect::CarrierChanged { .. } | PhyEffect::RxFrame { .. } => {
                    carrier_or_frame += 1
                }
                _ => {}
            }
        }
    }
    assert_eq!(tx_positive, (5_120_000..5_120_032).collect::<Vec<_>>());
    assert_eq!(rx_positive, (5_120_016..5_120_048).collect::<Vec<_>>());
    assert_eq!(carrier_or_frame, 0);
}
fn three_station_repeater() -> MediumConfig {
    let mut config = pair(16);
    let template = config.endpoints[0].clone();
    config.endpoints = (0..3)
        .flat_map(|i| [format!("m{i}"), format!("r{i}")])
        .map(|id| EndpointConfig {
            id,
            ..template.clone()
        })
        .collect();
    config.channel.endpoints = config.endpoints.iter().map(|e| e.id.clone()).collect();
    config.channel.paths = (0..6)
        .map(|rx| {
            (0..6)
                .map(|tx| {
                    if rx / 2 == tx / 2 && rx != tx {
                        Some(Path {
                            delay_samples: 16,
                            taps_q30: vec![Q30],
                        })
                    } else {
                        None
                    }
                })
                .collect()
        })
        .collect();
    config.channel.noise_rms_uv = vec![0; 6];
    config.repeater = Some(repeater::Config {
        execution_contract: repeater::CONTRACT.into(),
        pairs: (0..3)
            .map(|i| repeater::Pair {
                mac_endpoint: format!("m{i}"),
                mau_endpoint: format!("r{i}"),
            })
            .collect(),
        retime_chips: 4,
        consecutive_collision_limit: 31,
        reconnect_bits: 500,
        excessive_collision_bits: 1000,
    });
    config
}
#[test]
fn three_station_repeater_forwards_actual_decoded_frame_from_two_mau_outputs() {
    let config = three_station_repeater();
    config.validate().unwrap();
    let mut medium = SampledMedium::new(config).unwrap();
    let mut frame = request();
    frame.source_endpoint = "m0".into();
    medium.admit(frame.clone()).unwrap();
    let mut got = vec![];
    let mut first_repeated_sample = None;
    let mut source_finished = None;
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    origins,
                    ..
                } => {
                    assert_eq!(mac_bytes, frame.mac_bytes);
                    assert_eq!(origins.len(), 1);
                    assert_eq!(origins[0].source_endpoint, "m0");
                    got.push(receiver_endpoint);
                }
                PhyEffect::Repeater {
                    event: repeater::Event::Forwarding { .. },
                    sample_index,
                } => {
                    first_repeated_sample.get_or_insert(sample_index);
                }
                PhyEffect::TxFinished {
                    endpoint,
                    sample_index,
                    ..
                } if endpoint == "m0" => {
                    source_finished = Some(sample_index);
                }
                _ => {}
            }
        }
        if !medium.has_pending() {
            break;
        }
    }
    got.sort();
    assert_eq!(got, ["m1", "m2"]);
    assert!(first_repeated_sample.unwrap() < source_finished.unwrap());
    assert!(!medium.has_pending());
}
#[test]
fn repeater_attachment_exposes_only_mac_stations_and_rejects_mau_admission() {
    let config = three_station_repeater();
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    assert!(!medium.tx_ready("r0").unwrap());
    let mut mau_frame = request();
    mau_frame.source_endpoint = "r0".into();
    assert!(medium.admit(mau_frame).is_err());
    let ports: Vec<_> = (0..3)
        .map(|i| PhysicalPort {
            endpoint: format!("m{i}"),
            tx_port: format!("tx{i}"),
            rx_port: format!("rx{i}"),
        })
        .collect();
    let attachment = AttachmentConfig {
        origin_ps: 0,
        medium: config,
        bindings: vec![],
        physical_ports: ports,
        lookahead_samples: None,
    };
    attachment.validate().unwrap();
    let mut invalid = attachment.clone();
    invalid.physical_ports[2].endpoint = "r2".into();
    assert!(invalid.validate().is_err());
    let mut bounded = attachment;
    bounded.lookahead_samples = Some(65_536);
    bounded.validate().unwrap();
    bounded.lookahead_samples = Some(0);
    assert!(bounded.validate().is_err());
    bounded.lookahead_samples = Some(65_537);
    assert!(bounded.validate().is_err());
}
#[test]
fn repeater_rejects_cross_link_bypass_and_slot_budget_overrun() {
    let mut bypass = three_station_repeater();
    bypass.channel.paths[2][0] = Some(Path {
        delay_samples: 16,
        taps_q30: vec![Q30],
    });
    assert!(bypass.validate().is_err());
    let mut excessive = three_station_repeater();
    for (rx, tx) in [(0, 1), (1, 0), (2, 3), (3, 2)] {
        excessive.channel.paths[rx][tx]
            .as_mut()
            .unwrap()
            .delay_samples = 4090;
    }
    assert!(excessive.validate().is_err());
}
#[test]
fn repeater_checkpoint_preserves_live_retime_and_decoded_delivery() {
    let config = three_station_repeater();
    let mut uninterrupted = SampledMedium::new(config.clone()).unwrap();
    let mut frame = request();
    frame.source_endpoint = "m0".into();
    uninterrupted.admit(frame).unwrap();
    for _ in 0..3000 {
        uninterrupted.advance_one_sample().unwrap();
    }
    let saved = uninterrupted.checkpoint().unwrap();
    let mut resumed = SampledMedium::restore(&config, &saved).unwrap();
    let mut delivered = 0;
    while uninterrupted.has_pending() {
        let expected = uninterrupted.advance_one_sample().unwrap();
        let actual = resumed.advance_one_sample().unwrap();
        assert_eq!(actual, expected);
        delivered += expected
            .iter()
            .filter(|e| matches!(e, PhyEffect::RxFrame { .. }))
            .count();
    }
    assert_eq!(delivered, 2);
    assert!(!resumed.has_pending());
    assert_eq!(
        uninterrupted.checkpoint().unwrap(),
        resumed.checkpoint().unwrap()
    );
}
#[test]
fn repeater_batch_boundaries_equal_scalar_samples_and_checkpoint() {
    let config = three_station_repeater();
    let mut scalar = SampledMedium::new(config.clone()).unwrap();
    let mut batched = SampledMedium::new(config).unwrap();
    let mut frame = request();
    frame.source_endpoint = "m0".into();
    scalar.admit(frame.clone()).unwrap();
    batched.admit(frame).unwrap();
    for count in [1, 15, 16, 17, 257, 4096, 8999] {
        let expected: Vec<_> = (0..count)
            .flat_map(|_| scalar.advance_one_sample().unwrap())
            .collect();
        let actual = batched.advance_samples(count).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(batched.checkpoint().unwrap(), scalar.checkpoint().unwrap());
    }
}
#[test]
fn repeater_collision_is_observed_by_mac_and_generates_retries() {
    let mut medium = SampledMedium::new(three_station_repeater()).unwrap();
    let mut a = request();
    a.source_endpoint = "m0".into();
    let mut b = request();
    b.frame_id = "g".into();
    b.source_endpoint = "m1".into();
    medium.admit(a).unwrap();
    medium.admit(b).unwrap();
    let mut repeater_collision = false;
    let mut mac_collisions = std::collections::BTreeSet::new();
    let mut retries = std::collections::BTreeSet::new();
    for _ in 0..50_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::Repeater {
                    event: repeater::Event::Collision,
                    ..
                } => repeater_collision = true,
                PhyEffect::Collision { endpoint, .. } => {
                    mac_collisions.insert(endpoint);
                }
                PhyEffect::Retry { endpoint, .. } => {
                    retries.insert(endpoint);
                }
                _ => {}
            }
        }
        if repeater_collision && retries.len() == 2 {
            break;
        }
    }
    assert!(repeater_collision);
    assert_eq!(mac_collisions, ["m0".to_owned(), "m1".to_owned()].into());
    assert_eq!(retries, mac_collisions);
}
#[test]
fn repeater_requires_both_station_and_output_segments_for_delivery() {
    for disconnected in [(1, 0), (2, 3)] {
        let mut config = three_station_repeater();
        config.channel.paths[disconnected.0][disconnected.1] = None;
        config.validate().unwrap();
        let mut medium = SampledMedium::new(config).unwrap();
        let mut frame = request();
        frame.source_endpoint = "m0".into();
        medium.admit(frame).unwrap();
        let mut receivers = vec![];
        for _ in 0..50_000 {
            for effect in medium.advance_one_sample().unwrap() {
                if let PhyEffect::RxFrame {
                    receiver_endpoint, ..
                } = effect
                {
                    receivers.push(receiver_endpoint);
                }
            }
            if !medium.has_pending() {
                break;
            }
        }
        receivers.sort();
        assert_eq!(
            receivers,
            if disconnected == (1, 0) {
                vec![]
            } else {
                vec!["m2".to_owned()]
            }
        );
    }
}
#[test]
fn repeater_integrated_jam_is_alternating_and_future_driven() {
    let mut config = three_station_repeater();
    config.trace_samples = true;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut a = request();
    a.source_endpoint = "m0".into();
    let mut b = request();
    b.frame_id = "g".into();
    b.source_endpoint = "m1".into();
    medium.admit(a).unwrap();
    medium.admit(b).unwrap();
    let mut collision_sample = None;
    let mut first_chip_sample = None;
    let mut jam = vec![];
    for _ in 0..3_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::Repeater {
                    event: repeater::Event::Collision,
                    sample_index,
                } => {
                    collision_sample.get_or_insert(sample_index);
                }
                PhyEffect::Samples {
                    sample_index,
                    transmitted_uv,
                    ..
                } if collision_sample.is_some() && sample_index % 16 == 0 => {
                    let level = transmitted_uv[5].signum() as i8;
                    if level != 0 || first_chip_sample.is_some() {
                        first_chip_sample.get_or_insert(sample_index);
                        jam.push(level);
                    }
                }
                _ => {}
            }
        }
        if jam.len() >= 124 {
            break;
        }
    }
    assert!(first_chip_sample.unwrap() >= collision_sample.unwrap());
    assert_eq!(jam.len(), 124);
    for (bit, pair) in jam.chunks_exact(2).enumerate() {
        assert_eq!(pair, if bit % 2 == 0 { &[1, -1] } else { &[-1, 1] });
    }
}
#[test]
fn repeater_station_order_does_not_change_sampled_deliveries() {
    type ReceivedFrame = (String, Vec<u8>, Vec<Origin>, u64, u64);

    fn run(config: MediumConfig) -> Vec<ReceivedFrame> {
        let mut medium = SampledMedium::new(config).unwrap();
        let mut frame = request();
        frame.source_endpoint = "m0".into();
        medium.admit(frame).unwrap();
        let mut received = vec![];
        while medium.has_pending() {
            for effect in medium.advance_one_sample().unwrap() {
                if let PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    origins,
                    rx_sfd_sample,
                    rx_fcs_sample,
                    ..
                } = effect
                {
                    received.push((
                        receiver_endpoint,
                        mac_bytes,
                        origins,
                        rx_sfd_sample,
                        rx_fcs_sample,
                    ));
                }
            }
        }
        received.sort();
        received
    }
    let original = three_station_repeater();
    let expected = run(original.clone());
    let order = [4, 5, 0, 1, 2, 3];
    let mut permuted = original.clone();
    permuted.endpoints = order.map(|i| original.endpoints[i].clone()).to_vec();
    permuted.channel.endpoints = permuted.endpoints.iter().map(|p| p.id.clone()).collect();
    permuted.channel.paths = order
        .iter()
        .map(|&rx| {
            order
                .iter()
                .map(|&tx| original.channel.paths[rx][tx].clone())
                .collect()
        })
        .collect();
    permuted.channel.noise_rms_uv = order.map(|i| original.channel.noise_rms_uv[i]).to_vec();
    assert_eq!(run(permuted), expected);
}
#[test]
fn repeater_asymmetric_station_starts_still_collide_causally() {
    let mut medium = SampledMedium::new(three_station_repeater()).unwrap();
    let mut a = request();
    a.source_endpoint = "m0".into();
    let mut b = request();
    b.frame_id = "g".into();
    b.source_endpoint = "m1".into();
    b.at_ps = 350_000;
    medium.admit(a).unwrap();
    medium.admit(b).unwrap();
    let mut starts = std::collections::BTreeMap::new();
    let mut repeater_collision = None;
    let mut mac_collisions = std::collections::BTreeSet::new();
    for _ in 0..10_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::TxStarted {
                    endpoint,
                    sample_index,
                    ..
                } => {
                    starts.entry(endpoint).or_insert(sample_index);
                }
                PhyEffect::Repeater {
                    event: repeater::Event::Collision,
                    sample_index,
                } => {
                    repeater_collision.get_or_insert(sample_index);
                }
                PhyEffect::Collision { endpoint, .. } => {
                    mac_collisions.insert(endpoint);
                }
                _ => {}
            }
        }
        if mac_collisions.len() == 2 {
            break;
        }
    }
    assert!(starts["m0"] < starts["m1"]);
    assert!(starts["m1"] < repeater_collision.unwrap());
    assert_eq!(mac_collisions, ["m0".to_owned(), "m1".to_owned()].into());
}
#[test]
#[ignore = "runs a frame longer than Clause 9 Tw6 and then Tw5 at the physical sample clock"]
fn repeater_full_medium_tw6_partition_and_tw5_reconnect() {
    let mut config = three_station_repeater();
    // Fault injection: the two station MACs fail to heed the repeater's
    // collision signal, leaving CollIn active for the normative Tw6 interval.
    config.collision_threshold_uv = 3_000_000;
    config.maximum_samples = 800_000;
    config.trace_samples = true;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut a = request();
    a.source_endpoint = "m0".into();
    // 170-byte MAC frame plus preamble exceeds Tw6=1000 bit times, but
    // leaves fewer than Tw5=500 bit times of carrier after partition.
    a.mac_bytes = vec![0x5a; 166];
    a.mac_bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    a.mac_bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    a.mac_bytes[12..14].copy_from_slice(&[8, 0]);
    a.mac_bytes.extend(crc32(&a.mac_bytes).to_le_bytes());
    let mut b = a.clone();
    b.frame_id = "g".into();
    b.source_endpoint = "m1".into();
    medium.admit(a).unwrap();
    medium.admit(b).unwrap();
    let mut partitioned = std::collections::BTreeSet::new();
    let mut egress_after_partition = false;
    for _ in 0..110 {
        for effect in medium.advance_samples(4096).unwrap() {
            match effect {
                PhyEffect::Repeater {
                    event: repeater::Event::Partitioned { port },
                    ..
                } => {
                    partitioned.insert(port);
                }
                PhyEffect::Samples { transmitted_uv, .. }
                    if partitioned.contains("r0") && partitioned.contains("r1") =>
                {
                    egress_after_partition |= transmitted_uv[5] != 0;
                }
                _ => {}
            }
        }
        if !medium.has_pending() && partitioned.len() == 2 {
            break;
        }
    }
    assert_eq!(partitioned, ["r0".to_owned(), "r1".to_owned()].into());
    assert!(egress_after_partition);
    let mut recovery = request();
    recovery.frame_id = "recovery".into();
    recovery.source_endpoint = "m0".into();
    recovery.at_ps =
        sample_boundary_ps(medium.config().channel.grid, medium.next_sample_index()).unwrap();
    medium.admit(recovery).unwrap();
    let mut reconnected = false;
    for _ in 0..8 {
        for effect in medium.advance_samples(4096).unwrap() {
            if matches!(effect, PhyEffect::Repeater { event: repeater::Event::Reconnected { port }, .. } if port == "r0")
            {
                reconnected = true;
            }
        }
        if reconnected {
            break;
        }
    }
    if !reconnected {
        let checkpoint: serde_json::Value =
            serde_json::from_slice(&medium.checkpoint().unwrap()).unwrap();
        eprintln!(
            "recovery snapshot sample={} partitioned={} reconnect_chips={} active_before={} collision_before={} m0_queue_len={} m0_program_active={}",
            medium.next_sample_index(),
            checkpoint["state"]["repeater"]["partitioned"],
            checkpoint["state"]["repeater"]["reconnect_chips"],
            checkpoint["state"]["repeater"]["active_before"],
            checkpoint["state"]["repeater"]["collision_before"],
            checkpoint["state"]["ports"][0]["queue"]
                .as_array()
                .unwrap()
                .len(),
            !checkpoint["state"]["ports"][0]["program"].is_null()
        );
    }
    assert!(reconnected);
}
#[test]
fn tenbase_t_arbitrary_admission_has_exact_sfd_and_fcs_end() {
    for (ui, sample_ps) in [(16, 3_125), (32, 1_562)] {
        let mut medium = SampledMedium::new(pair(ui)).unwrap();
        let mut frame = request();
        frame.mac_bytes.truncate(60);
        frame.mac_bytes.extend([1, 2, 3, 4]);
        frame
            .mac_bytes
            .extend(crc32(&frame.mac_bytes).to_le_bytes());
        assert_eq!(frame.mac_bytes.len(), 68);
        frame.at_ps = 611_000;
        medium.admit(frame).unwrap();
        let mut markers = [Vec::new(), Vec::new(), Vec::new()];
        while medium.has_pending() {
            for effect in medium.advance_one_sample().unwrap() {
                match effect {
                    PhyEffect::TxMacPreamble { sample_index, .. } => markers[0].push(sample_index),
                    PhyEffect::TxMacSfd { sample_index, .. } => markers[1].push(sample_index),
                    PhyEffect::TxMacFcsEnd { sample_index, .. } => markers[2].push(sample_index),
                    _ => {}
                }
            }
        }
        assert!(markers.iter().all(|v| v.len() == 1), "markers={markers:?}");
        let [start, sfd, fcs] = markers.map(|v| v[0]);
        let ps = |index: u64| sample_boundary_ps(pair(ui).channel.grid, index).unwrap();
        assert_eq!(
            ps(sfd) - ps(start),
            6_400_000,
            "ui={ui}, sample_ps={sample_ps}"
        );
        assert_eq!(
            ps(fcs) - ps(sfd),
            54_400_000,
            "ui={ui}, sample_ps={sample_ps}"
        );
    }
}
#[test]
fn tenbase_t_transmits_and_recovers_real_manchester_without_project_pilot() {
    for ui in [16, 32] {
        let config = pair(ui);
        let frame = request();
        let mut medium = SampledMedium::new(config.clone()).unwrap();
        medium.admit(frame.clone()).unwrap();
        let mut got = vec![];
        while medium.has_pending() {
            for effect in medium.advance_one_sample().unwrap() {
                if let PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    rx_sfd_sample,
                    rx_fcs_sample,
                    ..
                } = effect
                {
                    got.push((receiver_endpoint, mac_bytes, rx_sfd_sample, rx_fcs_sample));
                }
            }
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "p1");
        assert_eq!(got[0].1, frame.mac_bytes);
        assert!(got[0].3 > got[0].2);
    }
}
#[test]
fn attached_tenbase_t_delivers_only_decoded_other_port_ingress() {
    let config = AttachmentConfig {
        origin_ps: 0,
        medium: pair(16),
        bindings: vec![],
        physical_ports: (0..2)
            .map(|i| PhysicalPort {
                endpoint: format!("p{i}"),
                tx_port: format!("tx{i}"),
                rx_port: format!("rx{i}"),
            })
            .collect(),
        lookahead_samples: None,
    };
    let wire = dir_simulator::runtime::ethernet::serialize_frame(
        "02:00:00:00:00:01",
        "02:00:00:00:00:02",
        0x0800,
        "deadbeef",
    )
    .unwrap();
    let mut attached = AttachedMedium::new(config).unwrap();
    attached.admit("tx0", "f", 0, &wire).unwrap();
    let mut ingresses = vec![];
    while attached.has_pending() {
        ingresses.extend(
            attached
                .advance_one_sample()
                .unwrap()
                .into_iter()
                .filter_map(|e| match e {
                    AttachmentEffect::PhysicalIngress {
                        source_endpoint,
                        receiver_endpoint,
                        receiver_rx_port,
                        frame,
                        ..
                    } => Some((source_endpoint, receiver_endpoint, receiver_rx_port, frame)),
                    _ => None,
                }),
        );
    }
    assert_eq!(ingresses.len(), 1);
    assert_eq!(ingresses[0].0, "p0");
    assert_eq!(ingresses[0].1, "p1");
    assert_eq!(ingresses[0].2, "rx1");
    assert_eq!(ingresses[0].3.wire.mac_hex, wire.mac_hex);
}
#[test]
fn attached_mac_permission_expiry_does_not_hold_physical_release() {
    let config = AttachmentConfig {
        origin_ps: 0,
        medium: pair(16),
        bindings: vec![],
        physical_ports: (0..2)
            .map(|i| PhysicalPort {
                endpoint: format!("p{i}"),
                tx_port: format!("tx{i}"),
                rx_port: format!("rx{i}"),
            })
            .collect(),
        lookahead_samples: None,
    };
    let wire = dir_simulator::runtime::ethernet::serialize_frame(
        "02:00:00:00:00:01",
        "02:00:00:00:00:02",
        0x0800,
        "deadbeef",
    )
    .unwrap();
    let mut attached = AttachedMedium::new(config).unwrap();
    let deadline_ps = 20_000_000;
    attached
        .set_mac_start_permission("tx0", Some(deadline_ps))
        .unwrap();
    attached.admit("tx0", "f", 0, &wire).unwrap();
    let mut started = None;
    let mut serialized = None;
    let mut released = None;
    for _ in 0..100_000 {
        let effects = attached.advance_one_sample().unwrap();
        let sample = attached.next_sample_index();
        for effect in effects {
            match effect {
                AttachmentEffect::Physical {
                    effect: PhyEffect::TxStarted { endpoint, .. },
                } if endpoint == "p0" => started = Some(sample),
                AttachmentEffect::TxSerialized { transfer_id, .. } if transfer_id == "f" => {
                    serialized = Some(sample)
                }
                AttachmentEffect::TxReleased { transfer_id, .. } if transfer_id == "f" => {
                    released = Some(sample)
                }
                _ => {}
            }
        }
        if released.is_some() {
            break;
        }
    }
    let start = started.expect("physical frame started while permission was active");
    let serialized = serialized.expect("physical serialization completed");
    let released = released.expect("physical IFG retired after permission expired");
    assert!(
        sample_boundary_ps(attached.config().medium.channel.grid, start).unwrap() < deadline_ps
    );
    assert!(
        sample_boundary_ps(attached.config().medium.channel.grid, serialized).unwrap()
            > deadline_ps
    );
    assert!(released > serialized);
    assert!(attached.tx_ready("tx0").unwrap());
    assert_eq!(
        attached.mac_start_permission("tx0").unwrap(),
        Some(deadline_ps)
    );
}
#[test]
fn attached_prediction_and_batch_match_scalar_effects_and_checkpoint() {
    let config = AttachmentConfig {
        origin_ps: 0,
        medium: pair(16),
        bindings: vec![],
        physical_ports: (0..2)
            .map(|i| PhysicalPort {
                endpoint: format!("p{i}"),
                tx_port: format!("tx{i}"),
                rx_port: format!("rx{i}"),
            })
            .collect(),
        lookahead_samples: None,
    };
    let wire = dir_simulator::runtime::ethernet::serialize_frame(
        "02:00:00:00:00:01",
        "02:00:00:00:00:02",
        0x0800,
        "deadbeef",
    )
    .unwrap();
    let mut batch = AttachedMedium::new(config).unwrap();
    batch.admit("tx0", "f", 0, &wire).unwrap();
    let before = batch.checkpoint().unwrap();
    let prediction = batch.predict_until_first_effect(4096).unwrap();
    let forecast = batch.forecast_until_first_effect(4096).unwrap();
    assert_eq!(forecast.samples, prediction.samples);
    assert_eq!(forecast.end_sample, prediction.end_sample);
    assert!(prediction.effects_present);
    assert_eq!(
        prediction.end_sample,
        batch.next_sample_index() + prediction.samples
    );
    assert_eq!(batch.checkpoint().unwrap(), before);
    let mut scalar = batch.clone();
    for _ in 1..prediction.samples {
        assert!(scalar.advance_one_sample().unwrap().is_empty());
    }
    let cut_effects = scalar.advance_one_sample().unwrap();
    assert!(!cut_effects.is_empty());
    assert_eq!(
        serde_json::to_value(&forecast.effects).unwrap(),
        serde_json::to_value(&cut_effects).unwrap()
    );
    assert_eq!(
        forecast.end_state.checkpoint().unwrap(),
        scalar.checkpoint().unwrap()
    );
    let batched_effects = batch.advance_samples(4096).unwrap();
    let mut scalar = AttachedMedium::restore(batch.config(), &before).unwrap();
    let mut scalar_effects = vec![];
    for _ in 0..4096 {
        scalar_effects.extend(scalar.advance_one_sample().unwrap());
    }
    assert_eq!(
        serde_json::to_value(&batched_effects).unwrap(),
        serde_json::to_value(&scalar_effects).unwrap()
    );
    assert_eq!(batch.checkpoint().unwrap(), scalar.checkpoint().unwrap());
    let idle = AttachedMedium::new(batch.config().clone()).unwrap();
    let quiet = idle.forecast_until_first_effect(16).unwrap();
    assert_eq!(quiet.samples, 16);
    assert!(quiet.effects.is_empty());
    assert_eq!(quiet.end_sample, 16);
    let mut executed = idle.clone();
    assert!(executed.advance_samples(16).unwrap().is_empty());
    assert_eq!(
        quiet.end_state.checkpoint().unwrap(),
        executed.checkpoint().unwrap()
    );
}
#[test]
fn simultaneous_tenbase_t_mac_attempts_collide_and_retry_on_wall_clock() {
    let mut medium = SampledMedium::new(pair(16)).unwrap();
    let first = request();
    let mut second = first.clone();
    second.frame_id = "g".into();
    second.source_endpoint = "p1".into();
    second.mac_bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    second.mac_bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    second.mac_bytes.truncate(60);
    second
        .mac_bytes
        .extend(crc32(&second.mac_bytes).to_le_bytes());
    medium.admit(first.clone()).unwrap();
    medium.admit(second.clone()).unwrap();
    let mut collisions = vec![];
    let mut retries = vec![];
    let mut frames = vec![];
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::Collision { endpoint, .. } => collisions.push(endpoint),
                PhyEffect::Retry { endpoint, .. } => retries.push(endpoint),
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => frames.push((receiver_endpoint, mac_bytes)),
                _ => {}
            }
        }
    }
    assert!(collisions.contains(&"p0".into()) && collisions.contains(&"p1".into()));
    assert!(!retries.is_empty());
    assert!(frames.contains(&("p0".into(), second.mac_bytes)));
    assert!(frames.contains(&("p1".into(), first.mac_bytes)));
}
