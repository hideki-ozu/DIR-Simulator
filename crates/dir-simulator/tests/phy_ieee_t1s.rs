use dir_simulator::runtime::ethernet::{
    crc32,
    phy::{attachment::*, channel::*, ieee::plca, medium::*, receiver::*, *},
};

fn medium() -> MediumConfig {
    // 25 Mchip/s DME: each chip lasts 40 ns. Sixteen samples per chip
    // provide 32 samples per Clause 147 80 ns coded-bit interval.
    let grid = SampleGrid {
        symbol_rate_hz: 25_000_000,
        samples_per_ui: 16,
    };
    let endpoints: Vec<_> = (0..3)
        .map(|index| EndpointConfig {
            id: format!("p{index}"),
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
        execution_contract: "ieee8023.10base_t1s.multidrop.2022".into(),
        standard_edition: "IEEE Std 802.3-2022".into(),
        physical_mode: PhysicalMode::Ieee10BaseT1sMultidrop2022,
        plca: None,
        repeater: None,
        gigabit_t1: None,
        gigabit_raw_merge: false,
        channel: ChannelConfig {
            grid,
            endpoints: endpoints.iter().map(|e| e.id.clone()).collect(),
            paths: (0..3)
                .map(|rx| {
                    (0..3)
                        .map(|tx| {
                            Some(Path {
                                delay_samples: if rx == tx { 0 } else { 16 },
                                taps_q30: vec![Q30],
                            })
                        })
                        .collect()
                })
                .collect(),
            noise_rms_uv: vec![0; 3],
            noise_seed: 51,
            clip_uv: 4_000_000,
        },
        endpoints,
        half_duplex: true,
        carrier_threshold_uv: 100_000,
        carrier_hold_samples: 32,
        collision_threshold_uv: 200_000,
        ifg_bits: 96,
        jam_bits: 32,
        slot_bits: 512,
        retry_limit: 16,
        seed: 17,
        maximum_mac_bytes: 1518,
        maximum_samples: 300_000,
        trace_samples: false,
    }
}
fn plca_medium(node_ids: [u8; 3], node_count: u8) -> MediumConfig {
    let mut config = medium();
    config.plca = Some(plca::DomainConfig {
        stations: (0..3)
            .map(|i| plca::StationConfig {
                endpoint: format!("p{i}"),
                node_id: node_ids[i],
            })
            .collect(),
        node_count,
        to_timer_bits: 32,
        burst_timer_bits: 128,
        max_burst_count: 0,
        fault_duplicate_ids: false,
    });
    config
}
fn request() -> MacTx {
    let mut bytes = vec![0x5a; 60];
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
fn arbitrary_admission_marks_first_real_chip_and_exact_mac_boundaries() {
    for (samples_per_chip, sample_ps) in [(16, 2_500), (32, 1_250)] {
        let mut config = medium();
        config.channel.grid.samples_per_ui = samples_per_chip;
        config.carrier_hold_samples = samples_per_chip * 2;
        for endpoint in &mut config.endpoints {
            endpoint.transmitter.grid = config.channel.grid;
            endpoint.receiver.grid = config.channel.grid;
        }
        let mut medium = SampledMedium::new(config).unwrap();
        let mut frame = request();
        frame.mac_bytes.truncate(60);
        frame.mac_bytes.extend([0x10, 0x11, 0x12, 0x13]);
        frame
            .mac_bytes
            .extend(crc32(&frame.mac_bytes).to_le_bytes());
        assert_eq!(frame.mac_bytes.len(), 68);
        frame.at_ps = 611_000;
        medium.admit(frame).unwrap();
        let mut start = None;
        let mut sfd = None;
        let mut fcs = None;
        while medium.has_pending() {
            for effect in medium.advance_one_sample().unwrap() {
                match effect {
                    PhyEffect::TxMacPreamble { sample_index, .. } => start = Some(sample_index),
                    PhyEffect::TxMacSfd { sample_index, .. } => sfd = Some(sample_index),
                    PhyEffect::TxMacFcsEnd { sample_index, .. } => fcs = Some(sample_index),
                    _ => {}
                }
            }
        }
        let (start, sfd, fcs) = (start.unwrap(), sfd.unwrap(), fcs.unwrap());
        assert_eq!((sfd - start) * sample_ps, 6_400_000);
        assert_eq!((fcs - sfd) * sample_ps, 54_400_000);
        assert!(start * sample_ps >= 611_000);
    }
}
#[test]
fn t1s_32_and_64_samples_per_coded_bit_match_aligned_analog_waveforms() {
    fn trace(samples_per_chip: u32, count: usize) -> (Vec<i64>, Vec<i64>) {
        let mut config = medium();
        config.channel.grid.samples_per_ui = samples_per_chip;
        config.carrier_hold_samples = samples_per_chip * 2;
        config.trace_samples = true;
        for (rx, row) in config.channel.paths.iter_mut().enumerate() {
            for (tx, path) in row.iter_mut().enumerate() {
                if rx != tx {
                    path.as_mut().unwrap().delay_samples = samples_per_chip as usize;
                }
            }
        }
        for endpoint in &mut config.endpoints {
            endpoint.transmitter.grid = config.channel.grid;
            endpoint.receiver.grid = config.channel.grid;
        }
        let mut medium = SampledMedium::new(config).unwrap();
        medium.admit(request()).unwrap();
        let mut tx = Vec::with_capacity(count);
        let mut rx = Vec::with_capacity(count);
        for _ in 0..count {
            for effect in medium.advance_one_sample().unwrap() {
                if let PhyEffect::Samples {
                    transmitted_uv,
                    received_uv,
                    ..
                } = effect
                {
                    tx.push(transmitted_uv[0]);
                    rx.push(received_uv[1]);
                }
            }
        }
        (tx, rx)
    }
    let low = trace(16, 8_000);
    let high = trace(32, 16_000);
    let mut squared_error = 0f64;
    for (low, high) in [(&low.0, &high.0), (&low.1, &high.1)] {
        for (coarse, fine) in low.iter().zip(high.iter().step_by(2)) {
            squared_error += (*coarse as f64 - *fine as f64).powi(2);
        }
    }
    let rms = (squared_error / 16_000f64).sqrt();
    assert!(
        rms <= 10_000f64,
        "aligned waveform RMS={rms} µV exceeds 1% TX peak"
    );
}
#[test]
fn named_t1s_rejects_wrong_edition_grid_and_ambiguous_ports() {
    let mut wrong = medium();
    wrong.standard_edition = "IEEE Std 802.3cg-2019".into();
    assert!(SampledMedium::new(wrong).is_err());
    let mut wrong = medium();
    wrong.channel.grid.symbol_rate_hz = 20_000_000;
    for endpoint in &mut wrong.endpoints {
        endpoint.transmitter.grid = wrong.channel.grid;
        endpoint.receiver.grid = wrong.channel.grid;
    }
    assert!(SampledMedium::new(wrong).is_err());
    let mut wrong = AttachmentConfig {
        origin_ps: 0,
        medium: medium(),
        bindings: vec![],
        physical_ports: (0..3)
            .map(|i| PhysicalPort {
                endpoint: format!("p{i}"),
                tx_port: format!("tx{i}"),
                rx_port: format!("rx{i}"),
            })
            .collect(),
        lookahead_samples: None,
    };
    let duplicate = wrong.physical_ports[0].rx_port.clone();
    wrong.physical_ports[1].rx_port = duplicate;
    assert!(AttachedMedium::new(wrong).is_err());
}
#[test]
fn plca_beacon_is_physical_and_received_before_noncoordinator_transmit() {
    let config = plca_medium([0, 1, 2], 3);
    let grid = config.channel.grid;
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    let mut saw_beacon_tx = false;
    let mut saw_beacon_at_p1 = false;
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            if let PhyEffect::PcsControl {
                endpoint, command, ..
            } = effect
            {
                if endpoint == "p0" && command == "beacon_tx" {
                    saw_beacon_tx = true;
                }
                if endpoint == "p1" && command == "beacon" {
                    saw_beacon_at_p1 = true;
                }
            }
        }
        if saw_beacon_at_p1 {
            break;
        }
    }
    assert!(saw_beacon_tx && saw_beacon_at_p1);
    let mut request = request();
    request.source_endpoint = "p1".into();
    request.at_ps = sample_boundary_ps(grid, medium.next_sample_index()).unwrap();
    medium.admit(request.clone()).unwrap();
    let mut commits = vec![];
    let mut frames = vec![];
    let mut starts = vec![];
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::PcsControl {
                    endpoint, command, ..
                } if command == "commit" => commits.push(endpoint),
                PhyEffect::TxStarted { endpoint, .. } => starts.push(endpoint),
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => frames.push((receiver_endpoint, mac_bytes)),
                _ => {}
            }
        }
    }
    assert_eq!(starts, ["p1"]);
    assert!(commits.contains(&"p0".into()) && commits.contains(&"p2".into()));
    frames.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        frames,
        [
            ("p0".into(), request.mac_bytes.clone()),
            ("p2".into(), request.mac_bytes)
        ]
    );
}
#[test]
fn plca_late_noncoordinator_admission_waits_for_next_beacon_cycle() {
    let config = plca_medium([0, 1, 2], 3);
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    let mut last_beacon = 0;
    let mut yielded = false;
    while !yielded {
        for effect in medium.advance_one_sample().unwrap() {
            if let PhyEffect::PcsControl {
                endpoint,
                command,
                sample_index,
            } = effect
            {
                yielded |= endpoint == "p1" && command == "plca_control:Yield";
                if endpoint == "p0" && command == "beacon_tx" {
                    last_beacon = sample_index;
                }
            }
        }
    }
    let mut frame = request();
    frame.source_endpoint = "p1".into();
    frame.at_ps = sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap();
    medium.admit(frame.clone()).unwrap();
    let mut next_beacon = None;
    let mut started = None;
    let mut collisions = 0;
    let mut delivered = Vec::new();
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint == "p0" && command == "beacon_tx" && sample_index > last_beacon => {
                    next_beacon.get_or_insert(sample_index);
                }
                PhyEffect::TxStarted {
                    endpoint,
                    sample_index,
                    ..
                } if endpoint == "p1" => {
                    started.get_or_insert(sample_index);
                }
                PhyEffect::Collision { .. } => collisions += 1,
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => delivered.push((receiver_endpoint, mac_bytes)),
                _ => {}
            }
        }
        if !medium.has_pending() && started.is_some() {
            break;
        }
    }
    assert!(next_beacon.unwrap() < started.unwrap());
    assert_eq!(collisions, 0);
    delivered.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        delivered,
        [
            ("p0".into(), frame.mac_bytes.clone()),
            ("p2".into(), frame.mac_bytes),
        ]
    );
}
#[test]
fn plca_sparse_ids_wait_for_their_received_opportunity() {
    let config = plca_medium([0, 2, 4], 5);
    let grid = config.channel.grid;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut beacon_at_p2 = false;
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            if matches!(effect, PhyEffect::PcsControl { endpoint, command, .. }
                if endpoint == "p2" && command == "beacon")
            {
                beacon_at_p2 = true;
            }
        }
        if beacon_at_p2 {
            break;
        }
    }
    assert!(beacon_at_p2);
    let mut request = request();
    request.source_endpoint = "p2".into();
    request.at_ps = sample_boundary_ps(grid, medium.next_sample_index()).unwrap();
    medium.admit(request.clone()).unwrap();
    let mut frames = vec![];
    let mut starts = vec![];
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::TxStarted { endpoint, .. } => starts.push(endpoint),
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => frames.push((receiver_endpoint, mac_bytes)),
                _ => {}
            }
        }
    }
    assert_eq!(starts, ["p2"]);
    frames.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        frames,
        [
            ("p0".into(), request.mac_bytes.clone()),
            ("p1".into(), request.mac_bytes)
        ]
    );
}
#[test]
fn plca_without_coordinator_falls_back_to_csma_cd() {
    let config = plca_medium([1, 2, 3], 4);
    let mut medium = SampledMedium::new(config).unwrap();
    let request = request();
    medium.admit(request.clone()).unwrap();
    let mut frames = vec![];
    let mut beacons = 0;
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => frames.push((receiver_endpoint, mac_bytes)),
                PhyEffect::PcsControl { command, .. } if command == "beacon_tx" => beacons += 1,
                _ => {}
            }
        }
    }
    assert_eq!(beacons, 0);
    frames.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        frames,
        [
            ("p1".into(), request.mac_bytes.clone()),
            ("p2".into(), request.mac_bytes)
        ]
    );
}
#[test]
fn plca_node_id_255_uses_csma_cd_while_other_nodes_are_configured() {
    let config = plca_medium([0, 1, 255], 3);
    let mut medium = SampledMedium::new(config).unwrap();
    let mut frame = request();
    frame.source_endpoint = "p2".into();
    medium.admit(frame.clone()).unwrap();
    let mut starts = vec![];
    let mut frames = vec![];
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::TxStarted { endpoint, .. } => starts.push(endpoint),
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => frames.push((receiver_endpoint, mac_bytes)),
                _ => {}
            }
        }
    }
    assert_eq!(starts, ["p2"]);
    frames.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        frames,
        [
            ("p0".into(), frame.mac_bytes.clone()),
            ("p1".into(), frame.mac_bytes)
        ]
    );
}
#[test]
fn explicit_duplicate_plca_ids_collide_after_shared_beacon() {
    let mut config = plca_medium([0, 1, 1], 3);
    config.plca.as_mut().unwrap().fault_duplicate_ids = true;
    let grid = config.channel.grid;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut beacons = std::collections::BTreeSet::new();
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            if let PhyEffect::PcsControl {
                endpoint, command, ..
            } = effect
            {
                if command == "beacon" {
                    beacons.insert(endpoint);
                }
            }
        }
        if beacons.contains("p1") && beacons.contains("p2") {
            break;
        }
    }
    assert!(beacons.contains("p1") && beacons.contains("p2"));
    let at_ps = sample_boundary_ps(grid, medium.next_sample_index()).unwrap();
    let mut a = request();
    a.frame_id = "a".into();
    a.source_endpoint = "p1".into();
    a.at_ps = at_ps;
    let mut b = request();
    b.frame_id = "b".into();
    b.source_endpoint = "p2".into();
    b.at_ps = at_ps;
    medium.admit(a).unwrap();
    medium.admit(b).unwrap();
    let mut starts = std::collections::BTreeMap::new();
    let mut collisions = std::collections::BTreeSet::new();
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::TxStarted {
                    endpoint,
                    sample_index,
                    ..
                } => {
                    starts.entry(endpoint).or_insert(sample_index);
                }
                PhyEffect::Collision { endpoint, .. } => {
                    collisions.insert(endpoint);
                }
                _ => {}
            }
        }
        if collisions.contains("p1") && collisions.contains("p2") {
            break;
        }
    }
    assert_eq!(starts["p1"], starts["p2"]);
    assert!(collisions.contains("p1") && collisions.contains("p2"));
}
#[test]
fn plca_burst_holds_carrier_with_commit_between_two_mac_frames() {
    let mut config = plca_medium([0, 1, 2], 3);
    config.plca.as_mut().unwrap().max_burst_count = 1;
    let grid = config.channel.grid;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut beacon_at_p1 = false;
    for _ in 0..100_000 {
        for effect in medium.advance_one_sample().unwrap() {
            if matches!(effect, PhyEffect::PcsControl { endpoint, command, .. }
                if endpoint == "p1" && command == "beacon")
            {
                beacon_at_p1 = true;
            }
        }
        if beacon_at_p1 {
            break;
        }
    }
    assert!(beacon_at_p1);
    let at_ps = sample_boundary_ps(grid, medium.next_sample_index()).unwrap();
    let mut first = request();
    first.source_endpoint = "p1".into();
    first.at_ps = at_ps;
    let mut second = first.clone();
    second.frame_id = "burst-2".into();
    second.mac_bytes[15] ^= 0x37;
    second.mac_bytes.truncate(60);
    second
        .mac_bytes
        .extend(crc32(&second.mac_bytes).to_le_bytes());
    medium.admit(first.clone()).unwrap();
    medium.admit(second.clone()).unwrap();
    let mut starts = vec![];
    let mut frames = vec![];
    let mut beacons_before_second = 0;
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::TxStarted { origin, .. } => starts.push(origin.frame_id),
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    ..
                } => frames.push((receiver_endpoint, mac_bytes)),
                PhyEffect::PcsControl { command, .. }
                    if command == "beacon_tx" && starts.len() < 2 =>
                {
                    beacons_before_second += 1
                }
                _ => {}
            }
        }
    }
    assert_eq!(starts, ["f", "burst-2"]);
    assert_eq!(beacons_before_second, 0);
    for endpoint in ["p0", "p2"] {
        let bytes: Vec<_> = frames
            .iter()
            .filter(|(rx, _)| rx == endpoint)
            .map(|(_, bytes)| bytes.as_slice())
            .collect();
        assert_eq!(
            bytes,
            [first.mac_bytes.as_slice(), second.mac_bytes.as_slice()]
        );
    }
}
#[test]
fn plca_mid_cycle_checkpoint_preserves_received_opportunity_and_frame() {
    let config = plca_medium([0, 1, 2], 3);
    let grid = config.channel.grid;
    let mut live = SampledMedium::new(config.clone()).unwrap();
    for _ in 0..100_000 {
        let observed = live
            .advance_one_sample()
            .unwrap()
            .into_iter()
            .any(|effect| {
                matches!(effect, PhyEffect::PcsControl { endpoint, command, .. }
                if endpoint == "p1" && command == "beacon")
            });
        if observed {
            break;
        }
    }
    let mut frame = request();
    frame.source_endpoint = "p1".into();
    frame.at_ps = sample_boundary_ps(grid, live.next_sample_index()).unwrap();
    live.admit(frame.clone()).unwrap();
    for _ in 0..4_000 {
        live.advance_one_sample().unwrap();
    }
    let saved = live.checkpoint().unwrap();
    let mut restored = SampledMedium::restore(&config, &saved).unwrap();
    let mut received = vec![];
    while live.has_pending() {
        let original = live.advance_one_sample().unwrap();
        let replay = restored.advance_one_sample().unwrap();
        assert_eq!(original, replay);
        received.extend(original.into_iter().filter_map(|effect| match effect {
            PhyEffect::RxFrame {
                receiver_endpoint,
                mac_bytes,
                ..
            } => Some((receiver_endpoint, mac_bytes)),
            _ => None,
        }));
    }
    assert_eq!(live.checkpoint().unwrap(), restored.checkpoint().unwrap());
    received.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        received,
        [
            ("p0".into(), frame.mac_bytes.clone()),
            ("p2".into(), frame.mac_bytes)
        ]
    );
}
#[test]
fn three_port_t1s_signal_to_decoded_mac_and_checkpoint() {
    let config = medium();
    let request = request();
    let mut a = SampledMedium::new(config.clone()).unwrap();
    a.admit(request.clone()).unwrap();
    for _ in 0..5_000 {
        a.advance_one_sample().unwrap();
    }
    let saved = a.checkpoint().unwrap();
    let mut b = SampledMedium::restore(&config, &saved).unwrap();
    let mut effects = vec![];
    while a.has_pending() {
        let ea = a.advance_one_sample().unwrap();
        let eb = b.advance_one_sample().unwrap();
        assert_eq!(ea, eb);
        effects.extend(ea);
    }
    assert_eq!(a.checkpoint().unwrap(), b.checkpoint().unwrap());
    let receivers: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            PhyEffect::RxFrame {
                receiver_endpoint,
                mac_bytes,
                ..
            } => Some((receiver_endpoint.as_str(), mac_bytes.as_slice())),
            _ => None,
        })
        .collect();
    assert_eq!(
        receivers,
        [
            ("p1", request.mac_bytes.as_slice()),
            ("p2", request.mac_bytes.as_slice())
        ]
    );
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, PhyEffect::TxMacSfd { .. }))
            .count(),
        0,
        "SFD occurred before the checkpoint cut"
    );
    assert_eq!(
        effects
            .iter()
            .filter(|e| matches!(e, PhyEffect::TxMacFcsEnd { .. }))
            .count(),
        1
    );
}
#[test]
fn attached_t1s_one_attempt_has_two_decoded_ingresses() {
    let config = AttachmentConfig {
        origin_ps: 0,
        medium: medium(),
        bindings: vec![],
        physical_ports: (0..3)
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
    let mut a = AttachedMedium::new(config.clone()).unwrap();
    assert!(a.tx_ready("tx0").unwrap());
    assert!(a.tx_ready("p0").is_err());
    a.admit("tx0", "physical-attempt-1", 0, &wire).unwrap();
    for _ in 0..5_000 {
        a.advance_one_sample().unwrap();
    }
    let bytes = a.checkpoint().unwrap();
    let mut b = AttachedMedium::restore(&config, &bytes).unwrap();
    let mut ingresses = vec![];
    while a.has_pending() {
        let left = a.advance_one_sample().unwrap();
        let right = b.advance_one_sample().unwrap();
        assert_eq!(
            serde_json::to_value(&left).unwrap(),
            serde_json::to_value(&right).unwrap()
        );
        ingresses.extend(left.into_iter().filter_map(|effect| match effect {
            AttachmentEffect::PhysicalIngress {
                admitted_transfer_id,
                physical_attempt,
                source_endpoint,
                receiver_endpoint,
                receiver_tx_port,
                receiver_rx_port,
                frame,
                rx_sfd_sample,
                rx_fcs_sample,
                ..
            } => Some((
                admitted_transfer_id,
                physical_attempt,
                source_endpoint,
                receiver_endpoint,
                receiver_tx_port,
                receiver_rx_port,
                frame.wire.mac_hex,
                rx_sfd_sample,
                rx_fcs_sample,
            )),
            _ => None,
        }));
    }
    assert_eq!(a.checkpoint().unwrap(), b.checkpoint().unwrap());
    assert_eq!(ingresses.len(), 2);
    assert_eq!(a.physical_ingress_history().len(), 2);
    a.verify_physical_ingress_replay().unwrap();
    let mut tampered: serde_json::Value = serde_json::from_slice(&a.checkpoint().unwrap()).unwrap();
    let proof = &mut tampered["physical_ingress_history"][0]["rx_fcs_sample"];
    *proof = serde_json::json!(proof.as_u64().unwrap() - 1);
    let restored =
        AttachedMedium::restore(&config, &serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert!(restored.verify_physical_ingress_replay().is_err());
    for (i, ingress) in ingresses.iter().enumerate() {
        assert_eq!(ingress.0, "physical-attempt-1");
        assert_eq!(ingress.1.frame_id, "physical-attempt-1");
        assert_eq!(ingress.2, "p0");
        assert_eq!(ingress.3, format!("p{}", i + 1));
        assert_eq!(ingress.4, format!("tx{}", i + 1));
        assert_eq!(ingress.5, format!("rx{}", i + 1));
        assert_eq!(ingress.6, wire.mac_hex);
        assert!(ingress.7 < ingress.8);
    }
}

#[test]
fn simultaneous_t1s_mac_attempts_jam_then_retry_with_beb() {
    let mut config = medium();
    config.maximum_samples = 900_000;
    let mut a = SampledMedium::new(config).unwrap();
    let first = request();
    let mut second = first.clone();
    second.frame_id = "g".into();
    second.source_endpoint = "p1".into();
    a.admit(first.clone()).unwrap();
    a.admit(second.clone()).unwrap();
    let mut collisions = vec![];
    let mut retries = vec![];
    let mut received = vec![];
    while a.has_pending() {
        for effect in a.advance_one_sample().unwrap() {
            match effect {
                PhyEffect::Collision {
                    endpoint,
                    origin,
                    late,
                    ..
                } => {
                    collisions.push((endpoint, origin, late));
                }
                PhyEffect::Retry {
                    endpoint,
                    origin,
                    slots,
                    ..
                } => {
                    retries.push((endpoint, origin, slots));
                }
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    origins,
                    ..
                } => {
                    received.push((receiver_endpoint, mac_bytes, origins));
                }
                _ => {}
            }
        }
    }
    assert_eq!(collisions.len(), 2);
    assert!(collisions.iter().all(|(_, _, late)| !late));
    assert_eq!(retries.len(), 2);
    assert!(
        retries
            .iter()
            .all(|(_, origin, slots)| origin.attempt == 0 && *slots < 2)
    );
    assert!(
        received.iter().any(|(ep, bytes, origins)| ep == "p2"
            && bytes == &first.mac_bytes
            && origins.len() == 1)
    );
}

#[test]
fn t1s_back_to_back_mac_requests_preserve_pcs_resynchronization() {
    let mut medium = SampledMedium::new(medium()).unwrap();
    let first = request();
    let mut second = first.clone();
    second.frame_id = "following".into();
    second.mac_bytes[15] ^= 0x33;
    let last = second.mac_bytes.len() - 4;
    second.mac_bytes.truncate(last);
    second
        .mac_bytes
        .extend(crc32(&second.mac_bytes).to_le_bytes());
    medium.admit(first.clone()).unwrap();
    medium.admit(second.clone()).unwrap();
    let mut received = vec![];
    while medium.has_pending() {
        for effect in medium.advance_one_sample().unwrap() {
            if let PhyEffect::RxFrame {
                receiver_endpoint,
                mac_bytes,
                ..
            } = effect
            {
                received.push((receiver_endpoint, mac_bytes));
            }
        }
    }
    assert_eq!(received.len(), 4);
    for endpoint in ["p1", "p2"] {
        let frames: Vec<_> = received
            .iter()
            .filter(|(rx, _)| rx == endpoint)
            .map(|(_, bytes)| bytes.as_slice())
            .collect();
        assert_eq!(
            frames,
            [first.mac_bytes.as_slice(), second.mac_bytes.as_slice()]
        );
    }
}

#[test]
fn t1s_decoding_and_mac_timing_agree_at_32_and_64_samples_per_coded_bit() {
    let mut observations = vec![];
    for samples_per_ui in [16, 32] {
        let mut config = medium();
        config.channel.grid.samples_per_ui = samples_per_ui;
        config.carrier_hold_samples = samples_per_ui * 2;
        for endpoint in &mut config.endpoints {
            endpoint.transmitter.grid = config.channel.grid;
            endpoint.receiver.grid = config.channel.grid;
        }
        let grid = config.channel.grid;
        let mut medium = SampledMedium::new(config).unwrap();
        let request = request();
        medium.admit(request.clone()).unwrap();
        let mut receiver_frames = vec![];
        let mut source_sfd = None;
        let mut source_fcs = None;
        while medium.has_pending() {
            for effect in medium.advance_one_sample().unwrap() {
                match effect {
                    PhyEffect::TxMacSfd { sample_index, .. } => {
                        source_sfd = Some(sample_boundary_ps(grid, sample_index).unwrap());
                    }
                    PhyEffect::TxMacFcsEnd { sample_index, .. } => {
                        source_fcs = Some(sample_boundary_ps(grid, sample_index).unwrap());
                    }
                    PhyEffect::RxFrame {
                        receiver_endpoint,
                        mac_bytes,
                        rx_fcs_sample,
                        ..
                    } => receiver_frames.push((
                        receiver_endpoint,
                        mac_bytes,
                        sample_boundary_ps(grid, rx_fcs_sample).unwrap(),
                    )),
                    _ => {}
                }
            }
        }
        receiver_frames.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(receiver_frames.len(), 2);
        assert!(
            receiver_frames
                .iter()
                .all(|(_, bytes, _)| bytes == &request.mac_bytes)
        );
        observations.push((source_sfd.unwrap(), source_fcs.unwrap(), receiver_frames));
    }
    // Sixteen Clause 147 groups precede the first MAC byte: 16 * 5 * 80 ns.
    assert_eq!(observations[0].0, 6_400_000);
    assert_eq!(observations[0].0, observations[1].0);
    assert_eq!(observations[0].1, 57_600_000);
    assert_eq!(observations[0].1, observations[1].1);
    for ((left_id, left_bytes, left_ps), (right_id, right_bytes, right_ps)) in
        observations[0].2.iter().zip(&observations[1].2)
    {
        assert_eq!(left_id, right_id);
        assert_eq!(left_bytes, right_bytes);
        assert!(left_ps.abs_diff(*right_ps) <= 80_000);
    }
}

#[test]
fn named_three_mac_transmitters_generate_collision_jam_beb_and_fresh_bytes() {
    let mut config = medium();
    config.maximum_samples = 4_000_000;
    let mut sampled = SampledMedium::new(config).unwrap();
    for i in 0..3 {
        let mut tx = request();
        tx.frame_id = format!("three-{i}");
        tx.source_endpoint = format!("p{i}");
        sampled.admit(tx).unwrap();
    }
    let mut collisions = std::collections::BTreeSet::new();
    let mut retries = std::collections::BTreeSet::new();
    let mut ends = std::collections::BTreeSet::new();
    let mut delivered = std::collections::BTreeSet::new();
    while sampled.has_pending() {
        for effect in sampled.advance_samples(512).unwrap() {
            match effect {
                PhyEffect::Collision { origin, late, .. } => {
                    assert!(!late);
                    collisions.insert(origin.frame_id);
                }
                PhyEffect::Retry { origin, slots, .. } => {
                    assert!(slots < (1 << (origin.attempt + 1).min(10)));
                    retries.insert(origin.frame_id);
                }
                PhyEffect::TxCodedEnd { origin, .. } => {
                    ends.insert(origin.frame_id);
                }
                PhyEffect::RxFrame {
                    mac_bytes,
                    origins,
                    receiver_endpoint,
                    ..
                } => {
                    assert_eq!(mac_bytes, request().mac_bytes);
                    assert_eq!(origins.len(), 1);
                    delivered.insert((origins[0].frame_id.clone(), receiver_endpoint));
                }
                _ => {}
            }
        }
    }
    assert_eq!(collisions.len(), 3);
    assert_eq!(retries.len(), 3);
    assert_eq!(ends.len(), 3);
    assert_eq!(delivered.len(), 6);
}

#[test]
fn plca_burst_accepts_arrival_during_frame_and_inside_postframe_window() {
    for postframe in [false, true] {
        let mut config = plca_medium([0, 1, 2], 3);
        config.maximum_samples = 1_000_000;
        config.plca.as_mut().unwrap().max_burst_count = 1;
        config.plca.as_mut().unwrap().burst_timer_bits = 255;
        let mut sampled = SampledMedium::new(config.clone()).unwrap();
        let mut synchronized = false;
        while !synchronized {
            synchronized = sampled.advance_samples(1).unwrap().iter().any(|e| {
                matches!(e, PhyEffect::PcsControl {endpoint,command,..}
                    if endpoint == "p1" && command == "beacon")
            });
        }
        let mut first = request();
        first.source_endpoint = "p1".into();
        first.at_ps = sample_boundary_ps(config.channel.grid, sampled.next_sample_index()).unwrap();
        sampled.admit(first.clone()).unwrap();
        let mut all = vec![];
        loop {
            let effects = sampled.advance_samples(1).unwrap();
            let cut = effects.iter().any(|effect| {
                if postframe {
                    matches!(effect, PhyEffect::TxFinished {origin,..} if origin.frame_id == "f")
                } else {
                    matches!(effect, PhyEffect::TxStarted {origin,..} if origin.frame_id == "f")
                }
            });
            all.extend(effects);
            if cut {
                break;
            }
        }
        // The packet did not exist at the first frame's start. The second
        // case waits past one real IPG, inside the configured 255-bit window.
        let delay = if postframe { 100 * 40 } else { 200 };
        all.extend(sampled.advance_samples(delay).unwrap());
        let mut second = first.clone();
        second.frame_id = "late-burst".into();
        second.at_ps =
            sample_boundary_ps(config.channel.grid, sampled.next_sample_index()).unwrap();
        sampled.admit(second).unwrap();
        while sampled.has_pending() {
            all.extend(sampled.advance_samples(128).unwrap());
        }
        let starts = all
            .iter()
            .filter_map(|e| match e {
                PhyEffect::TxStarted {
                    origin,
                    sample_index,
                    ..
                } => Some((&origin.frame_id, *sample_index)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 2, "postframe={postframe}");
        assert!(
            all.iter()
                .filter(|e| matches!(e, PhyEffect::RxFrame {mac_bytes,..}
            if mac_bytes == &first.mac_bytes))
                .count()
                == 4
        );
        assert!(
            !all.iter().any(|e| matches!(e,
            PhyEffect::PcsControl {command,sample_index,..}
                if command == "beacon_tx" && *sample_index < starts[1].1)),
            "postframe={postframe}"
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
        assert!(
            !saved["state"]["ports"][1]["plca_commit_active"]
                .as_bool()
                .unwrap()
        );
    }
}

#[test]
fn plca_actual_hold_overflow_jams_delays_pending_commits_and_retries_with_checkpoint() {
    let mut config = plca_medium([0, 1, 7], 8);
    config.maximum_samples = 4_000_000;
    config.plca.as_mut().unwrap().to_timer_bits = 255;
    let mut sampled = SampledMedium::new(config.clone()).unwrap();
    loop {
        if sampled.advance_samples(1).unwrap().iter().any(|e| {
            matches!(e,PhyEffect::PcsControl{endpoint,command,..}
                if endpoint=="p2"&&command=="beacon")
        }) {
            break;
        }
    }
    let mut tx = request();
    tx.source_endpoint = "p2".into();
    tx.at_ps = sample_boundary_ps(config.channel.grid, sampled.next_sample_index()).unwrap();
    sampled.admit(tx.clone()).unwrap();
    let mut transitions = vec![];
    let mut collisions = 0;
    let mut collision_sample = None;
    let mut retries = 0;
    let mut frames = 0;
    let mut resumed: Option<SampledMedium> = None;
    while sampled.has_pending() {
        let effects = sampled.advance_samples(1).unwrap();
        if let Some(restored) = &mut resumed {
            assert_eq!(effects, restored.advance_samples(1).unwrap());
        }
        for effect in effects {
            match effect {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint == "p2" && command.starts_with("plca_data:") => {
                    transitions.push((command.clone(), sample_index));
                    if command == "plca_data:Flush" {
                        let state: serde_json::Value =
                            serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
                        let program = &state["state"]["ports"][2]["program"];
                        assert!(
                            program.is_object(),
                            "FLUSH must drive the actual retained PHY tail"
                        );
                        assert!(program["mac_input_end_sample"].as_u64().unwrap() <= sample_index);
                    }
                    if command == "plca_data:DelayPending" && resumed.is_none() {
                        let checkpoint = sampled.checkpoint().unwrap();
                        resumed = Some(SampledMedium::restore(&config, &checkpoint).unwrap());
                    }
                }
                PhyEffect::Collision {
                    endpoint,
                    late,
                    sample_index,
                    ..
                } if endpoint == "p2" => {
                    assert!(!late);
                    collisions += 1;
                    collision_sample.get_or_insert(sample_index);
                }
                PhyEffect::Retry { endpoint, .. } if endpoint == "p2" => {
                    retries += 1;
                }
                PhyEffect::RxFrame { mac_bytes, .. } => {
                    assert_eq!(mac_bytes, tx.mac_bytes);
                    frames += 1;
                }
                _ => {}
            }
        }
    }
    assert!(collisions > 0 && retries > 0, "{transitions:?}");
    let first_hold = transitions
        .iter()
        .find(|(s, _)| s == "plca_data:Hold")
        .unwrap()
        .1;
    assert_eq!(
        collision_sample.unwrap().checked_sub(first_hold),
        Some(396 * 40),
        "the collision must exhaust the real bounded HOLD line: {transitions:?}"
    );
    assert_eq!(frames, 2, "{transitions:?}");
    for state in [
        "Hold",
        "Collide",
        "DelayPending",
        "Pending",
        "WaitMac",
        "Transmit",
        "WaitIdle",
    ] {
        assert!(
            transitions
                .iter()
                .any(|(s, _)| s == &format!("plca_data:{state}")),
            "missing {state}: {transitions:?}"
        );
    }
    let delay = transitions
        .iter()
        .position(|(s, _)| s == "plca_data:DelayPending")
        .unwrap();
    assert_eq!(transitions[delay + 1].0, "plca_data:Pending");
    assert_eq!(transitions[delay + 1].1 - transitions[delay].1, 512 * 40);
    assert_eq!(
        sampled.checkpoint().unwrap(),
        resumed.unwrap().checkpoint().unwrap()
    );
}

#[test]
fn named_slot_boundary_uses_detection_time_and_late_fault_has_no_retry() {
    // An explicitly asymmetric receive-open fault prevents p1 sensing p0.
    // This exercises late-fault behavior; it is not a conforming mixing
    // segment topology or a claim of a legal propagation envelope.
    for (detection_sample, expected_late) in [(512 * 40, false), (512 * 40 + 1, true)] {
        let mut config = medium();
        config.maximum_samples = 500_000;
        config.channel.paths[1][0] = None;
        config.channel.paths[0][1].as_mut().unwrap().delay_samples =
            if expected_late { 16 } else { 15 };
        let mut long = request();
        long.mac_bytes.truncate(60);
        long.mac_bytes.resize(1000, 0x5a);
        long.mac_bytes.extend(crc32(&long.mac_bytes).to_le_bytes());
        let mut interferer = request();
        interferer.frame_id = "late-interferer".into();
        interferer.source_endpoint = "p1".into();
        // p1 starts at a real chip boundary. Delay chooses the last slot
        // sample or the first sample outside it at p0's detector.
        interferer.at_ps = (512 * 40 - 16) * 2500;
        let mut sampled = SampledMedium::new(config).unwrap();
        sampled.admit(long).unwrap();
        sampled.admit(interferer).unwrap();
        let mut collided = false;
        let mut retired = false;
        while sampled.next_sample_index() < 100_000 && !retired {
            for effect in sampled.advance_samples(1).unwrap() {
                match effect {
                    PhyEffect::Collision {
                        endpoint,
                        late,
                        sample_index,
                        ..
                    } if endpoint == "p0" => {
                        assert_eq!(sample_index, detection_sample);
                        assert_eq!(late, expected_late);
                        collided = true;
                    }
                    PhyEffect::Retry { endpoint, .. } if endpoint == "p0" => {
                        assert!(!expected_late);
                        retired = true;
                    }
                    PhyEffect::RetryExhausted { endpoint, .. } if endpoint == "p0" => {
                        assert!(expected_late);
                        retired = true;
                    }
                    _ => {}
                }
            }
        }
        assert!(collided && retired);
    }
}

#[test]
fn plca_actual_mac_wait_timeout_reaches_wait_idle_and_control_aborts_commit() {
    let mut config = plca_medium([0, 1, 7], 8);
    config.maximum_samples = 1_000_000;
    config.ifg_bits = 512;
    config.plca.as_mut().unwrap().to_timer_bits = 255;
    let mut sampled = SampledMedium::new(config.clone()).unwrap();
    loop {
        if sampled.advance_samples(1).unwrap().iter().any(|e| {
            matches!(e,
        PhyEffect::PcsControl{endpoint,command,..}if endpoint=="p2"&&command=="beacon")
        }) {
            break;
        }
    }
    let mut tx = request();
    tx.source_endpoint = "p2".into();
    tx.at_ps = sample_boundary_ps(config.channel.grid, sampled.next_sample_index()).unwrap();
    sampled.admit(tx).unwrap();
    let mut wait = None;
    let mut timeout = None;
    let mut abort = None;
    while sampled.next_sample_index() < 900_000 && abort.is_none() {
        for e in sampled.advance_samples(1).unwrap() {
            if let PhyEffect::PcsControl {
                endpoint,
                command,
                sample_index,
            } = e
            {
                if endpoint == "p2" && command == "plca_data:WaitMac" {
                    wait = Some(sample_index);
                }
                if endpoint == "p2" && command == "plca_data:WaitIdle" && wait.is_some() {
                    timeout = Some(sample_index);
                }
                if endpoint == "p2" && command == "plca_control:Abort" {
                    abort = Some(sample_index);
                }
            }
        }
    }
    assert_eq!(timeout.unwrap() - wait.unwrap(), 288 * 40);
    assert!(abort.unwrap() > timeout.unwrap());
    let saved: serde_json::Value = serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
    let p = &saved["state"]["ports"][2];
    assert_eq!(p["plca_commit_active"], false);
    assert_eq!(p["plca_node"]["committed"], false);
    assert_eq!(p["plca_control_chips"].as_array().unwrap().len(), 2);
    assert!(p["program"].is_null());
    // WAIT_MAC still expires after exactly 288 bits above. Only the group
    // sampled before command removal survives, through the following MCD.
    sampled.advance_samples(160).unwrap();
    let drained: serde_json::Value =
        serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
    let p = &drained["state"]["ports"][2];
    assert!(p["plca_control_chips"].as_array().unwrap().is_empty());
    assert!(p["program"].is_null());
}

#[test]
#[ignore = "release gate: real 130090-bit PLCA status hysteresis after coordinator disable"]
fn plca_coordinator_disappears_status_falls_back_then_actual_beacon_recovers() {
    let mut config = plca_medium([0, 1, 2], 3);
    config.maximum_samples = 7_000_000;
    let mut sampled = SampledMedium::new(config.clone()).unwrap();
    loop {
        if sampled.advance_samples(1).unwrap().iter().any(|e| {
            matches!(e,
        PhyEffect::PcsControl{endpoint,command,..}if endpoint=="p1"&&command=="beacon")
        }) {
            break;
        }
    }
    sampled.set_plca_enabled("p0", false).unwrap();
    sampled.advance_samples(10_000).unwrap();
    let mut lost = false;
    while sampled.next_sample_index() < 6_000_000 {
        sampled.advance_samples(65_536).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
        lost = (1..3).all(|i| saved["state"]["ports"][i]["plca_node"]["status"] == "inactive");
        if lost {
            break;
        }
    }
    assert!(lost);
    sampled.set_plca_enabled("p0", true).unwrap();
    let mut beacon = false;
    while sampled.next_sample_index() < 6_500_000 && !beacon {
        beacon = sampled.advance_samples(128).unwrap().iter().any(|e| {
            matches!(e,
            PhyEffect::PcsControl{endpoint,command,..}if endpoint=="p1"&&command=="beacon")
        });
    }
    assert!(beacon);
    let mut tx = request();
    tx.source_endpoint = "p1".into();
    tx.at_ps = sample_boundary_ps(config.channel.grid, sampled.next_sample_index()).unwrap();
    sampled.admit(tx.clone()).unwrap();
    let mut frames = 0;
    while sampled.has_pending() {
        for e in sampled.advance_samples(128).unwrap() {
            if let PhyEffect::RxFrame { mac_bytes, .. } = e {
                assert_eq!(mac_bytes, tx.mac_bytes);
                frames += 1;
            }
        }
    }
    assert_eq!(frames, 2);
}

#[test]
fn actual_control_acquisition_all_sample_offsets_polarities_fir_and_seeded_noise() {
    for n in [16, 32] {
        for inverted in [false, true] {
            for offset in 0..n {
                let mut cfg = medium();
                cfg.channel.grid.samples_per_ui = n;
                cfg.carrier_hold_samples = n * 2;
                cfg.maximum_samples = 180_000;
                for e in &mut cfg.endpoints {
                    e.transmitter.grid.samples_per_ui = n;
                    e.receiver.grid.samples_per_ui = n;
                    e.receiver.frontend_taps_q30 = vec![Q30 * 95 / 100, Q30 - Q30 * 95 / 100];
                }
                for (i, paths) in cfg.channel.paths.iter_mut().enumerate() {
                    for (j, path) in paths.iter_mut().enumerate() {
                        let path = path.as_mut().unwrap();
                        path.delay_samples = if i == j {
                            0
                        } else {
                            n as usize + offset as usize
                        };
                        path.taps_q30 = vec![if inverted { -Q30 } else { Q30 }];
                    }
                }
                cfg.channel.noise_rms_uv = vec![1000; 3];
                let mut sampled = SampledMedium::new(cfg).unwrap();
                sampled.admit(request()).unwrap();
                let mut frames = 0;
                while sampled.has_pending() {
                    for e in sampled.advance_samples(256).unwrap() {
                        if let PhyEffect::RxFrame { mac_bytes, .. } = e {
                            assert_eq!(mac_bytes, request().mac_bytes);
                            frames += 1;
                        }
                    }
                }
                assert_eq!(frames, 2, "samples={n} inverted={inverted} offset={offset}");
            }
        }
    }
}

#[test]
fn sampled_invalid_beacon_times_out_while_actual_carrier_stays_on() {
    // Explicit hidden-transmitter receive-open fault corrupts the remainder
    // of a real BEACON after its valid first pair; no conforming topology claim.
    let mut config = plca_medium([0, 1, 255], 3);
    config.channel.paths[2][0] = None;
    // Dominating amplitude avoids a cancellation-induced carrier gap: the
    // remaining BEACON is invalid while the physical carrier remains present.
    config.endpoints[2].transmitter.peak_uv = 3_000_000;
    config.maximum_samples = 100_000;
    let mut sampled = SampledMedium::new(config.clone()).unwrap();
    let mut beacon_start = None;
    while beacon_start.is_none() {
        for e in sampled.advance_samples(1).unwrap() {
            if let PhyEffect::PcsControl {
                endpoint,
                command,
                sample_index,
            } = e
            {
                if endpoint == "p0" && command == "beacon_tx" {
                    beacon_start = Some(sample_index);
                }
            }
        }
    }
    let mut interferer = request();
    interferer.source_endpoint = "p2".into();
    interferer.at_ps =
        sample_boundary_ps(config.channel.grid, beacon_start.unwrap() + 400).unwrap();
    sampled.admit(interferer).unwrap();
    let mut synchronized = false;
    let mut invalid = None;
    let mut resync = None;
    let mut carrier = true;
    while sampled.next_sample_index() < 20_000 && resync.is_none() {
        for e in sampled.advance_samples(1).unwrap() {
            match e {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint == "p1" => {
                    if command == "plca_control:Syncing" {
                        synchronized = true;
                    }
                    if command == "plca_control:Resync" && synchronized {
                        resync = Some(sample_index);
                        assert!(carrier);
                    }
                }
                PhyEffect::RxAbort {
                    endpoint,
                    sample_index,
                    ..
                } if endpoint == "p1" && synchronized => {
                    invalid.get_or_insert(sample_index);
                }
                PhyEffect::CarrierChanged {
                    endpoint, present, ..
                } if endpoint == "p1" => {
                    carrier = present;
                }
                PhyEffect::RxFrame {
                    receiver_endpoint, ..
                } if receiver_endpoint == "p1" => panic!("invalid beacon fabricated a frame"),
                _ => {}
            }
        }
    }
    assert_eq!(resync.unwrap() - invalid.unwrap(), 40 * 40);
}

#[test]
fn actual_mid_frame_interference_discards_receiver_assembly_then_fresh_retry_decodes() {
    let mut cfg = medium();
    cfg.maximum_samples = 1_000_000;
    cfg.channel.paths[1][0] = None; // Explicit hidden-transmitter receive-open fault.
    let mut sampled = SampledMedium::new(cfg).unwrap();
    sampled.admit(request()).unwrap();
    let mut interferer = request();
    interferer.frame_id = "hidden-interference".into();
    interferer.source_endpoint = "p1".into();
    interferer.at_ps = 10_000_000;
    sampled.admit(interferer).unwrap();
    let mut abort = false;
    let mut fresh = 0;
    while sampled.has_pending() {
        for e in sampled.advance_samples(128).unwrap() {
            match e {
                PhyEffect::RxAbort { endpoint, .. } if endpoint == "p2" => abort = true,
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    origins,
                    ..
                } => {
                    assert!(!origins.iter().any(|o| o.frame_id == "f" && o.attempt == 0));
                    if receiver_endpoint == "p2"
                        && origins.iter().any(|o| o.frame_id == "f" && o.attempt > 0)
                    {
                        assert_eq!(mac_bytes, request().mac_bytes);
                        fresh += 1;
                    }
                }
                _ => {}
            }
        }
    }
    assert!(abort);
    assert_eq!(fresh, 1);
}

#[test]
fn forced_mac_jam_does_not_drive_mdi_or_interrupt_coordinator_beacon() {
    let mut cfg = plca_medium([0, 1, 7], 8);
    cfg.plca.as_mut().unwrap().to_timer_bits = 255;
    cfg.maximum_samples = 500_000;
    let mut timing = SampledMedium::new(cfg.clone()).unwrap();
    let mut beacons = vec![];
    while beacons.len() < 2 {
        for e in timing.advance_samples(1024).unwrap() {
            if let PhyEffect::PcsControl {
                endpoint,
                command,
                sample_index,
            } = e
            {
                if endpoint == "p0" && command == "beacon_tx" {
                    beacons.push(sample_index);
                }
            }
        }
    }
    let admit_at = beacons[1] - 396 * 40;
    cfg.trace_samples = true;
    let mut actual = SampledMedium::new(cfg.clone()).unwrap();
    while actual.next_sample_index() < admit_at {
        actual
            .advance_samples((admit_at - actual.next_sample_index()).min(4096))
            .unwrap();
    }
    let mut baseline = actual.clone();
    let mut frame = request();
    frame.at_ps = sample_boundary_ps(cfg.channel.grid, admit_at).unwrap();
    actual.admit(frame).unwrap();
    let mut jam_start = None;
    let mut jam_end = None;
    let mut resumed: Option<SampledMedium> = None;
    while actual.next_sample_index() < beacons[1] + 32 * 40 + 64 {
        let expected = baseline.advance_samples(1).unwrap();
        let events = actual.advance_samples(1).unwrap();
        if let Some(saved) = &mut resumed {
            assert_eq!(events, saved.advance_samples(1).unwrap());
        }
        let sample = |events: &Vec<PhyEffect>| {
            events
                .iter()
                .find_map(|e| match e {
                    PhyEffect::Samples {
                        transmitted_uv,
                        received_uv,
                        ..
                    } => Some((transmitted_uv.clone(), received_uv.clone())),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(
            sample(&events),
            sample(&expected),
            "MAC-only JAM must leave actual control MDI unchanged"
        );
        for e in events {
            match e {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint == "p0" => {
                    if command == "plca_mac_jam_start" {
                        jam_start = Some(sample_index);
                        let saved = actual.checkpoint().unwrap();
                        let state: serde_json::Value = serde_json::from_slice(&saved).unwrap();
                        assert_eq!(state["state"]["ports"][0]["plca_beacon_in_flight"], true);
                        resumed = Some(SampledMedium::restore(&cfg, &saved).unwrap());
                    }
                    if command == "plca_mac_jam_end" {
                        jam_end = Some(sample_index);
                    }
                }
                PhyEffect::TxStarted { .. }
                | PhyEffect::TxMacPreamble { .. }
                | PhyEffect::TxMacSfd { .. }
                | PhyEffect::TxMacFcsEnd { .. }
                | PhyEffect::TxCodedEnd { .. }
                | PhyEffect::RxFrame { .. } => {
                    panic!("suppressed MAC JAM fabricated a physical frame marker")
                }
                _ => {}
            }
        }
    }
    assert_eq!(jam_end.unwrap() - jam_start.unwrap(), 32 * 40);
    let a: serde_json::Value = serde_json::from_slice(&actual.checkpoint().unwrap()).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&baseline.checkpoint().unwrap()).unwrap();
    assert_eq!(a["state"]["origin_history"], b["state"]["origin_history"]);
    assert_eq!(
        actual.checkpoint().unwrap(),
        resumed.unwrap().checkpoint().unwrap()
    );
}

#[test]
fn three_named_plca_transmitters_keep_physical_jam_in_transmit_until_mac_off_mcd() {
    use std::collections::{BTreeMap, BTreeSet};
    let mut cfg = medium();
    cfg.maximum_samples = 500_000;
    cfg.trace_samples = true;
    let mut fourth = cfg.endpoints[2].clone();
    fourth.id = "p3".into();
    cfg.endpoints.push(fourth);
    cfg.channel.endpoints.push("p3".into());
    cfg.channel.noise_rms_uv.push(0);
    for row in &mut cfg.channel.paths {
        row.push(Some(Path {
            delay_samples: 16,
            taps_q30: vec![Q30],
        }));
    }
    cfg.channel.paths.push(
        (0..4)
            .map(|i| {
                Some(Path {
                    delay_samples: if i == 3 { 0 } else { 16 },
                    taps_q30: vec![Q30],
                })
            })
            .collect(),
    );
    cfg.plca = Some(plca::DomainConfig {
        stations: (0..4)
            .map(|i| plca::StationConfig {
                endpoint: format!("p{i}"),
                node_id: if i == 0 { 0 } else { 1 },
            })
            .collect(),
        node_count: 2,
        to_timer_bits: 32,
        burst_timer_bits: 128,
        max_burst_count: 0,
        fault_duplicate_ids: true,
    });
    let mut sampled = SampledMedium::new(cfg.clone()).unwrap();
    let mut beacons = BTreeSet::new();
    while beacons.len() < 3 {
        for e in sampled.advance_samples(1).unwrap() {
            if let PhyEffect::PcsControl {
                endpoint, command, ..
            } = e
            {
                if endpoint != "p0" && command == "beacon" {
                    beacons.insert(endpoint);
                }
            }
        }
    }
    for i in 1..4 {
        let mut tx = request();
        tx.frame_id = format!("physical-col-{i}");
        tx.source_endpoint = format!("p{i}");
        tx.at_ps = sample_boundary_ps(cfg.channel.grid, sampled.next_sample_index()).unwrap();
        sampled.admit(tx).unwrap();
    }
    let mut collisions = BTreeMap::new();
    let mut ends = BTreeMap::new();
    let mut wait = BTreeMap::new();
    let mut coded = BTreeMap::new();
    let mut retries = BTreeSet::new();
    let mut clear_during_jam = BTreeSet::new();
    let mut driven: BTreeMap<String, u64> = BTreeMap::new();
    while sampled.next_sample_index() < 100_000 && retries.len() < 3 {
        for e in sampled.advance_samples(1).unwrap() {
            match e {
                PhyEffect::Collision {
                    endpoint,
                    origin,
                    late,
                    sample_index,
                    ..
                } => {
                    assert_eq!(origin.attempt, 0);
                    assert!(!late);
                    collisions.insert(endpoint, sample_index);
                }
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint != "p0" => {
                    assert!(
                        ![
                            "plca_data:Collide",
                            "plca_data:DelayPending",
                            "plca_data:Pending"
                        ]
                        .contains(&command.as_str()),
                        "physical COL must not enter forced collision recovery: {command}"
                    );
                    if [
                        "plca_signal_error_on",
                        "plca_signal_error_off",
                        "plca_data:WaitIdle",
                    ]
                    .contains(&command.as_str())
                    {
                        let state: serde_json::Value =
                            serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
                        let i = endpoint[1..].parse::<usize>().unwrap();
                        let p = &state["state"]["ports"][i];
                        if command == "plca_signal_error_on" {
                            assert_eq!(p["plca_node"]["data"], "transmit");
                            assert_eq!(p["plca_node"]["hold_samples"], 0);
                            assert!(p["program"]["mac_jam_samples_left"].is_null());
                            let end = p["program"]["mac_input_end_sample"].as_u64().unwrap();
                            ends.entry(endpoint.clone()).or_insert(end);
                            assert_eq!(end - collisions[&endpoint], 32 * 40);
                        }
                        if command == "plca_signal_error_off"
                            && sample_index < *ends.get(&endpoint).unwrap_or(&0)
                        {
                            assert_eq!(p["plca_node"]["data"], "transmit");
                            assert_eq!(p["plca_node"]["hold_samples"], 0);
                            assert_eq!(p["program"]["jam"], true);
                            clear_during_jam.insert(endpoint.clone());
                        }
                        if command == "plca_data:WaitIdle" {
                            wait.insert(endpoint.clone(), sample_index);
                            assert_eq!(sample_index, ends[&endpoint].div_ceil(160) * 160 + 1);
                            assert_eq!(p["plca_node"]["data_timer_samples"], 0);
                        }
                    }
                }
                PhyEffect::TxCodedEnd {
                    endpoint,
                    sample_index,
                    ..
                } => {
                    coded.insert(endpoint, sample_index);
                }
                PhyEffect::Retry {
                    endpoint,
                    origin,
                    slots,
                    ..
                } => {
                    assert_eq!(origin.attempt, 0);
                    assert!(slots < 2);
                    retries.insert(endpoint);
                }
                PhyEffect::Samples {
                    sample_index,
                    transmitted_uv,
                    ..
                } => {
                    for (endpoint, end) in &ends {
                        if sample_index >= collisions[endpoint] && sample_index < *end {
                            let i = endpoint[1..].parse::<usize>().unwrap();
                            assert_eq!(transmitted_uv[i].abs(), 1_000_000);
                            *driven.entry(endpoint.clone()).or_default() += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }
    assert_eq!(collisions.len(), 3);
    assert_eq!(retries.len(), 3);
    assert_eq!(wait.len(), 3);
    assert!(
        !clear_during_jam.is_empty(),
        "actual COL must clear independently of ongoing MAC JAM"
    );
    for endpoint in collisions.keys() {
        assert!(wait[endpoint] < coded[endpoint]);
        // The first COL sample precedes its signal-status observation by one sample.
        assert!(driven[endpoint] >= 32 * 40 - 1);
    }
}

#[test]
fn actual_hold_cancellation_aborts_without_wire_frame_then_fresh_attempt_flushes() {
    let mut config = plca_medium([0, 1, 2], 3);
    config.maximum_samples = 1_000_000;
    let mut sampled = SampledMedium::new(config.clone()).unwrap();
    loop {
        if sampled.advance_samples(1).unwrap().iter().any(|e| {
            matches!(e,
            PhyEffect::PcsControl {endpoint,command,..} if endpoint=="p1" && command=="beacon")
        }) {
            break;
        }
    }
    let mut tx = request();
    tx.source_endpoint = "p1".into();
    tx.at_ps = sample_boundary_ps(config.channel.grid, sampled.next_sample_index()).unwrap();
    sampled.admit(tx.clone()).unwrap();
    loop {
        let effects = sampled.advance_samples(1).unwrap();
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, PhyEffect::TxStarted { .. }))
        );
        if effects.iter().any(|e| {
            matches!(e,PhyEffect::PcsControl{endpoint,command,..}
            if endpoint=="p1" && command=="plca_data:Hold")
        }) {
            break;
        }
    }
    sampled.set_mac_start_permission("p1", None).unwrap();
    let mut abort = false;
    let mut wait_idle = false;
    for _ in 0..320 {
        for e in sampled.advance_samples(1).unwrap() {
            match e {
                PhyEffect::PcsControl {
                    endpoint, command, ..
                } if endpoint == "p1" => {
                    abort |= command == "plca_data:Abort";
                    wait_idle |= command == "plca_data:WaitIdle";
                }
                PhyEffect::TxStarted { .. }
                | PhyEffect::TxFinished { .. }
                | PhyEffect::RxFrame { .. } => {
                    panic!("cancelled HOLD must not create a physical frame")
                }
                _ => {}
            }
        }
    }
    assert!(abort && wait_idle);
    sampled
        .set_mac_start_permission("p1", Some(u64::MAX))
        .unwrap();
    let mut flush = None;
    let mut finish = None;
    let mut received = 0;
    while sampled.has_pending() {
        for e in sampled.advance_samples(1).unwrap() {
            match e {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint == "p1" && command == "plca_data:Flush" => {
                    flush = Some(sample_index);
                    let state: serde_json::Value =
                        serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
                    let port = &state["state"]["ports"][1];
                    assert!(port["plca_node"]["hold_samples"].as_u64().unwrap() > 0);
                    assert!(port["program"].is_object());
                    assert!(
                        port["program"]["mac_input_end_sample"].as_u64().unwrap() <= sample_index
                    );
                }
                PhyEffect::TxFinished { sample_index, .. } => finish = Some(sample_index),
                PhyEffect::RxFrame { mac_bytes, .. } => {
                    assert_eq!(mac_bytes, tx.mac_bytes);
                    received += 1;
                }
                _ => {}
            }
        }
    }
    assert!(flush.unwrap() < finish.unwrap());
    assert_eq!(received, 2);
}

#[test]
fn actual_plca_pmcd_control_mcd_first_chips_and_exact_retained_tail() {
    let mut cfg = plca_medium([0, 1, 2], 3);
    cfg.trace_samples = true;
    let mut sampled = SampledMedium::new(cfg.clone()).unwrap();
    let mut beacon_command = None;
    let mut beacon_drive = None;
    let mut beacon_sync = None;
    let mut beacon_release = None;
    let mut received = false;
    let mut dme = dir_simulator::runtime::ethernet::phy::ieee::t1s::DmeReceiver::default();
    let mut groups = vec![];
    loop {
        for e in sampled.advance_samples(1).unwrap() {
            match e {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } => {
                    if endpoint == "p0" && command == "beacon_tx" {
                        beacon_command.get_or_insert(sample_index);
                    }
                    if endpoint == "p0" && command == "plca_control:Syncing" {
                        beacon_sync.get_or_insert(sample_index);
                    }
                    received |= endpoint == "p1" && command == "beacon";
                }
                PhyEffect::Samples {
                    sample_index,
                    transmitted_uv,
                    ..
                } => {
                    if transmitted_uv[0] != 0 {
                        let first = *beacon_drive.get_or_insert(sample_index);
                        if (sample_index - first) % 16 == 8 {
                            groups.extend(dme.push_chip(transmitted_uv[0].signum() as i8).unwrap());
                        }
                    } else if beacon_drive.is_some() {
                        beacon_release.get_or_insert(sample_index);
                    }
                }
                _ => {}
            }
        }
        if received && beacon_sync.is_some() && beacon_release.is_some() {
            break;
        }
    }
    assert_eq!(
        (beacon_command.unwrap() - 1) % 160,
        120,
        "PMCD leads MCD by one MAC bit"
    );
    assert_eq!(beacon_drive.unwrap(), beacon_command.unwrap() + 39);
    assert_eq!(beacon_drive.unwrap() % 160, 0);
    assert_eq!(beacon_sync.unwrap() - beacon_command.unwrap(), 20 * 40);
    assert_eq!(
        beacon_release.unwrap() - beacon_drive.unwrap(),
        20 * 40 + 2 * 16
    );
    assert_eq!(
        groups,
        vec![dir_simulator::runtime::ethernet::phy::ieee::t1s::BEACON; 5]
    );
    sampled.advance_samples(13).unwrap();
    let mut tx = request();
    tx.source_endpoint = "p1".into();
    tx.at_ps = sample_boundary_ps(cfg.channel.grid, sampled.next_sample_index()).unwrap();
    sampled.admit(tx.clone()).unwrap();
    let mut hold = None;
    let mut flush = None;
    let mut wait = None;
    let mut finish = None;
    let mut delay = None;
    let mut frames = 0;
    while sampled.has_pending() {
        for e in sampled.advance_samples(1).unwrap() {
            match e {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if endpoint == "p1" => {
                    if command == "plca_data:Hold" {
                        hold.get_or_insert(sample_index);
                    }
                    if command == "plca_data:Flush" {
                        flush = Some(sample_index);
                        let state: serde_json::Value =
                            serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
                        let p = &state["state"]["ports"][1];
                        let a = p["plca_node"]["hold_samples"].as_u64().unwrap();
                        delay = Some(a);
                        assert!(a > 0);
                        assert_eq!(a % 160, 0);
                        assert_eq!(
                            sample_index,
                            p["program"]["mac_input_end_sample"].as_u64().unwrap() + 1
                        );
                    }
                    if command == "plca_data:WaitIdle" && flush.is_some() {
                        wait.get_or_insert(sample_index);
                    }
                }
                PhyEffect::TxStarted { sample_index, .. } => assert_eq!(sample_index % 160, 0),
                PhyEffect::TxFinished { sample_index, .. } => finish = Some(sample_index),
                PhyEffect::RxFrame { mac_bytes, .. } => {
                    assert_eq!(mac_bytes, tx.mac_bytes);
                    frames += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!((hold.unwrap() - 1) % 160, 0, "source MAC is sampled at MCD");
    assert_eq!(wait.unwrap() - flush.unwrap(), delay.unwrap());
    assert!(
        wait.unwrap() < finish.unwrap(),
        "PCS coded end remains independent of MAC output-off"
    );
    assert_eq!(frames, 2);
}

#[test]
fn actual_positive_and_negative_ppm_plca_joins_and_flushes_on_oscillator_edges() {
    let mut rounding_witnesses = 0;
    for ppm in [-100i32, 100] {
        let rate = (1_000_000i64 + ppm as i64) as u128;
        let edge = |k: u128| (k * 160_000_000).div_ceil(rate) as u64;
        let cycle = |n: u64| n as u128 * rate / 160_000_000;
        let mut adversarial_length = 64usize;
        for (case, offset) in [1, 321, 961, 1].into_iter().enumerate() {
            let mut cfg = plca_medium([0, 1, 2], 3);
            cfg.maximum_samples = 500_000;
            for ep in &mut cfg.endpoints {
                ep.transmitter.clock_ppm = ppm;
            }
            let mut sampled = SampledMedium::new(cfg.clone()).unwrap();
            let mut command = None;
            let mut sync = None;
            loop {
                let mut beacon = false;
                for e in sampled.advance_samples(1).unwrap() {
                    if let PhyEffect::PcsControl {
                        endpoint,
                        command: c,
                        sample_index,
                    } = e
                    {
                        if endpoint == "p0" && c == "beacon_tx" {
                            command.get_or_insert(sample_index);
                        }
                        if endpoint == "p0" && c == "plca_control:Syncing" {
                            sync.get_or_insert(sample_index);
                        }
                        beacon |= endpoint == "p1" && c == "beacon";
                    }
                }
                if beacon && sync.is_some() {
                    break;
                }
            }
            let start = command.unwrap() - 1;
            let pmcd_cycle = (start as u128 * rate + 40_000_000) / 160_000_000;
            assert_eq!(
                start,
                ((pmcd_cycle * 160_000_000 - 40_000_000).div_ceil(rate)) as u64
            );
            assert_eq!(
                sync.unwrap() - 1,
                (((pmcd_cycle + 5) * 160_000_000 - 40_000_000).div_ceil(rate)) as u64
            );
            sampled.advance_samples(offset).unwrap();
            let mut tx = request();
            tx.source_endpoint = "p1".into();
            if case == 3 {
                tx.mac_bytes.truncate(60);
                tx.mac_bytes.resize(adversarial_length - 4, 0x5a);
                tx.mac_bytes.extend(crc32(&tx.mac_bytes).to_le_bytes());
            }
            tx.at_ps = sample_boundary_ps(cfg.channel.grid, sampled.next_sample_index()).unwrap();
            sampled.admit(tx.clone()).unwrap();
            let mut hold = None;
            let mut phy_start = None;
            let mut expected_end = None;
            let mut input_end = None;
            let mut wait = None;
            let mut coded = None;
            let mut frames = 0;
            let mut resumed: Option<SampledMedium> = None;
            while sampled.has_pending() {
                let effects = sampled.advance_samples(1).unwrap();
                if let Some(restored) = &mut resumed {
                    assert_eq!(effects, restored.advance_samples(1).unwrap());
                }
                for e in effects {
                    match e {
                        PhyEffect::PcsControl {
                            endpoint,
                            command,
                            sample_index,
                        } if endpoint == "p1" => {
                            if command == "plca_data:Hold" {
                                hold.get_or_insert(sample_index - 1);
                            }
                            if command == "plca_data:Flush" {
                                let v: serde_json::Value =
                                    serde_json::from_slice(&sampled.checkpoint().unwrap()).unwrap();
                                if resumed.is_none() {
                                    resumed = Some(
                                        SampledMedium::restore(
                                            &cfg,
                                            &sampled.checkpoint().unwrap(),
                                        )
                                        .unwrap(),
                                    );
                                }
                                input_end = Some(
                                    v["state"]["ports"][1]["program"]["mac_input_end_sample"]
                                        .as_u64()
                                        .unwrap(),
                                );
                                assert_eq!(sample_index, input_end.unwrap() + 1);
                            }
                            if command == "plca_data:WaitIdle" && input_end.is_some() {
                                wait.get_or_insert(sample_index - 1);
                            }
                        }
                        PhyEffect::TxStarted { sample_index, .. } => {
                            assert_eq!(edge(cycle(sample_index)), sample_index);
                            phy_start = Some(sample_index);
                            expected_end = Some(edge(
                                cycle(sample_index) + (8 + tx.mac_bytes.len() as u128) * 2,
                            ));
                        }
                        PhyEffect::TxFinished { sample_index, .. } => coded = Some(sample_index),
                        PhyEffect::RxFrame { mac_bytes, .. } => {
                            assert_eq!(mac_bytes, tx.mac_bytes);
                            frames += 1;
                        }
                        PhyEffect::Collision { .. } => {
                            panic!("legal clocked opportunity unexpectedly collided ppm={ppm}")
                        }
                        _ => {}
                    }
                }
            }
            assert_eq!(frames, 2, "ppm={ppm} offset={offset}");
            assert_eq!(
                sampled.checkpoint().unwrap(),
                resumed.unwrap().checkpoint().unwrap()
            );
            assert_eq!(edge(cycle(hold.unwrap())), hold.unwrap());
            assert_eq!(
                input_end.unwrap(),
                edge(cycle(hold.unwrap()) + (8 + tx.mac_bytes.len() as u128) * 2)
            );
            assert_eq!(wait.unwrap(), expected_end.unwrap());
            assert!(wait.unwrap() < coded.unwrap());
            if case == 0 {
                let k0 = cycle(hold.unwrap());
                let k1 = cycle(phy_start.unwrap());
                adversarial_length = (64..=128usize)
                    .find(|len| {
                        edge(k1 + (8 + *len as u128) * 2) - edge(k0 + (8 + *len as u128) * 2)
                            != phy_start.unwrap() - hold.unwrap()
                    })
                    .expect(
                        "independent phase arithmetic must find a short rounding counterexample",
                    );
            }
            if wait.unwrap() - input_end.unwrap() != phy_start.unwrap() - hold.unwrap() {
                rounding_witnesses += 1;
                eprintln!(
                    "actual ppm={ppm} bytes={} source={} output={} input_off={} output_off={} retained_start_samples={} retained_tail_samples={}",
                    tx.mac_bytes.len(),
                    hold.unwrap(),
                    phy_start.unwrap(),
                    input_end.unwrap(),
                    wait.unwrap(),
                    phy_start.unwrap() - hold.unwrap(),
                    wait.unwrap() - input_end.unwrap()
                );
            }
        }
    }
    assert!(
        rounding_witnesses > 0,
        "must exercise the one-sample endpoint-rounding trap"
    );
}

#[test]
fn actual_commit_timeout_drains_sampled_group_and_checkpoint_at_zero_and_nonzero_ppm() {
    for ppm in [0i32, -100, 100] {
        // Independent rational chip/nibble edges, and a level-pair DME oracle;
        // neither uses the production clock helper or DME decoder.
        let rate = (1_000_000i64 + ppm as i64) as u128;
        let chip_edge = |j: u128| (j * 16_000_000).div_ceil(rate) as u64;
        let mut cfg = plca_medium([0, 1, 2], 3);
        cfg.trace_samples = true;
        cfg.plca.as_mut().unwrap().max_burst_count = 1;
        for ep in &mut cfg.endpoints {
            ep.transmitter.clock_ppm = ppm;
        }
        let mut sampled = SampledMedium::new(cfg.clone()).unwrap();
        loop {
            if sampled.advance_samples(1).unwrap().iter().any(|e| {
                matches!(e, PhyEffect::PcsControl { endpoint, command, .. }
                    if endpoint == "p1" && command == "beacon")
            }) {
                break;
            }
        }
        let mut tx = request();
        tx.source_endpoint = "p1".into();
        tx.at_ps = sample_boundary_ps(cfg.channel.grid, sampled.next_sample_index()).unwrap();
        sampled.admit(tx.clone()).unwrap(); // Deliberately NO next packet.
        let first_sample = sampled.next_sample_index();
        let mut waveform = vec![];
        let mut burst = None;
        let mut abort = None;
        let mut cut = None;
        let mut resumed: Option<SampledMedium> = None;
        let mut frames = 0;
        let mut starts = 0;
        let mut end = cfg.maximum_samples;
        while sampled.next_sample_index() < end {
            let effects = sampled.advance_samples(1).unwrap();
            if let Some(restored) = &mut resumed {
                assert_eq!(effects, restored.advance_samples(1).unwrap());
            }
            for e in effects {
                match e {
                    PhyEffect::Samples {
                        sample_index,
                        transmitted_uv,
                        ..
                    } => {
                        assert_eq!(sample_index, first_sample + waveform.len() as u64);
                        waveform.push(transmitted_uv[1]);
                    }
                    PhyEffect::PcsControl {
                        endpoint,
                        command,
                        sample_index,
                    } if endpoint == "p1" => {
                        if command == "plca_control:Burst" {
                            burst = Some(sample_index - 1);
                        }
                        if command == "plca_control:Abort" && burst.is_some() && abort.is_none() {
                            let removed = sample_index - 1;
                            abort = Some(removed);
                            let group = removed as u128 * rate / 160_000_000;
                            end = chip_edge((group + 3) * 10);
                            // Save immediately inside the actual residual drain.
                            // Keep the old-code validation error until after its
                            // waveform failure, so the baseline proves F1 itself.
                            let saved = sampled.checkpoint();
                            if let Ok(bytes) = &saved {
                                resumed = Some(SampledMedium::restore(&cfg, bytes).unwrap());
                            }
                            cut = Some(saved);
                        }
                    }
                    PhyEffect::TxStarted { .. } => starts += 1,
                    PhyEffect::RxFrame { mac_bytes, .. } => {
                        assert_eq!(mac_bytes, tx.mac_bytes);
                        frames += 1;
                    }
                    PhyEffect::Collision { .. } => panic!("unexpected collision ppm={ppm}"),
                    _ => {}
                }
            }
        }
        let removed = abort.expect("actual burst timeout");
        assert_eq!(
            removed - burst.unwrap(),
            (128 * 40_000_000u128).div_ceil(rate) as u64
        );
        assert_eq!((starts, frames), (1, 2));
        let group = removed as u128 * rate / 160_000_000;
        let group_start = chip_edge(group * 10);
        let release = chip_edge((group + 1) * 10);
        assert!(group_start < removed && removed < release);
        let level = |n: u64| waveform[(n - first_sample) as usize];
        for g in [group - 1, group] {
            let mut code = 0u8;
            let mut previous = None;
            for j in 0..10u128 {
                let begin = chip_edge(g * 10 + j);
                let next = chip_edge(g * 10 + j + 1);
                let v = level(begin);
                assert_eq!(
                    v.abs(),
                    1_000_000,
                    "started COMMIT group truncated: ppm={ppm} removed={removed} chip={j} sample={begin}"
                );
                for n in begin..next {
                    assert_eq!(level(n), v, "irregular chip cadence ppm={ppm} sample={n}");
                }
                if let Some(prior) = previous {
                    if j % 2 == 0 {
                        assert_ne!(v, prior, "missing DME bit-boundary transition");
                    } else if v != prior {
                        code |= 1 << (j / 2);
                    }
                }
                previous = Some(v);
            }
            assert_eq!(code, 0b11000, "independent SYNC/COMMIT decode ppm={ppm}");
        }
        for n in release..end {
            assert_eq!(
                level(n),
                0,
                "extra group after command removal ppm={ppm} sample={n}"
            );
        }
        let bytes = cut.unwrap().expect("legal residual COMMIT checkpoint");
        let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let port = &state["state"]["ports"][1];
        assert_eq!(port["plca_commit_active"], false);
        assert!(port["program"].is_null());
        assert_eq!(port["plca_control_chips"].as_array().unwrap().len(), 9);
        for bad in [vec![1; 10], vec![1; 8], vec![0; 9], vec![2; 9], vec![-2; 9]] {
            let mut malformed = state.clone();
            malformed["state"]["ports"][1]["plca_control_chips"] = serde_json::json!(bad);
            assert!(
                SampledMedium::restore(&cfg, &serde_json::to_vec(&malformed).unwrap()).is_err()
            );
        }
        assert_eq!(
            sampled.checkpoint().unwrap(),
            resumed.unwrap().checkpoint().unwrap()
        );
        if let Some(root) = std::env::var_os("DIR_COMMIT_DRAIN_EVIDENCE") {
            use std::fmt::Write as _;
            let root = std::path::PathBuf::from(root);
            std::fs::write(root.join(format!("drain-{ppm}.json")), &bytes).unwrap();
            let rows = (group_start..end).fold(String::new(), |mut rows, n| {
                writeln!(&mut rows, "{n},{}", level(n)).unwrap();
                rows
            });
            std::fs::write(root.join(format!("waveform-{ppm}.csv")), rows).unwrap();
        }
        eprintln!(
            "actual COMMIT ppm={ppm} burst={} removal={removed} final_group={group_start} release={release} residual=9 decoded=11000 starts={starts} receptions={frames}",
            burst.unwrap()
        );
    }
}

#[test]
fn actual_beacon_at_extreme_accepted_clock_rates_has_five_groups_and_continuous_tail() {
    use dir_simulator::runtime::ethernet::phy::ieee::t1s::{BEACON, DmeReceiver};
    for ppm in [-10_000i32, 10_000] {
        let rate = (1_000_000i64 + ppm as i64) as u128;
        let edge = |k: u128| (k * 160_000_000).div_ceil(rate) as u64;
        let mut cfg = plca_medium([0, 1, 2], 3);
        cfg.trace_samples = true;
        for ep in &mut cfg.endpoints {
            ep.transmitter.clock_ppm = ppm;
        }
        let mut sampled = SampledMedium::new(cfg).unwrap();
        // Exercise the management re-enable guard on this same TX clock.
        sampled.set_plca_enabled("p0", false).unwrap();
        sampled.advance_samples(7).unwrap();
        sampled.set_plca_enabled("p0", true).unwrap();
        let mut request = None;
        let mut command_off = None;
        let mut first = None;
        let mut release = None;
        let mut dme = DmeReceiver::default();
        let mut groups = vec![];
        while release.is_none() {
            for e in sampled.advance_samples(1).unwrap() {
                match e {
                    PhyEffect::PcsControl {
                        endpoint,
                        command,
                        sample_index,
                    } if endpoint == "p0" => {
                        if command == "beacon_tx" {
                            request.get_or_insert(sample_index - 1);
                        }
                        if command == "plca_control:Syncing" {
                            command_off.get_or_insert(sample_index - 1);
                        }
                    }
                    PhyEffect::Samples {
                        sample_index,
                        transmitted_uv,
                        ..
                    } => {
                        if transmitted_uv[0] != 0 {
                            first.get_or_insert(sample_index);
                            let chip = sample_index as u128 * rate / 16_000_000;
                            if sample_index
                                == ((chip * 16_000_000 + 8_000_000).div_ceil(rate)) as u64
                            {
                                groups.extend(
                                    dme.push_chip(transmitted_uv[0].signum() as i8).unwrap(),
                                );
                            }
                        } else if first.is_some() {
                            release = Some(sample_index);
                        }
                    }
                    _ => {}
                }
            }
        }
        assert!(
            request.unwrap() >= 7 + (4_640_000_000u128.div_ceil(rate)) as u64,
            "full local-clock cycle guard must survive management re-enable"
        );
        let k = (request.unwrap() as u128 * rate + 40_000_000) / 160_000_000;
        assert_eq!(
            request.unwrap(),
            ((k * 160_000_000 - 40_000_000).div_ceil(rate)) as u64
        );
        assert_eq!(first.unwrap(), edge(k));
        assert_eq!(
            command_off.unwrap(),
            (((k + 5) * 160_000_000 - 40_000_000).div_ceil(rate)) as u64
        );
        assert_eq!(
            release.unwrap(),
            (((10 * (k + 5) + 2) * 16_000_000).div_ceil(rate)) as u64
        );
        assert_eq!(groups, vec![BEACON; 5], "ppm={ppm}");
        eprintln!(
            "actual beacon ppm={ppm} command={} first_mdi={} command_off={} mdi_release={} groups={}",
            request.unwrap(),
            first.unwrap(),
            command_off.unwrap(),
            release.unwrap(),
            groups.len()
        );
    }
}
