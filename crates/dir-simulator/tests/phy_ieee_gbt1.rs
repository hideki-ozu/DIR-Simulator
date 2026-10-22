use dir_simulator::runtime::ethernet::{
    crc32,
    phy::{
        Q30, SampleGrid,
        attachment::sample_boundary_ps,
        channel::*,
        ieee::{an_hsm, t1_gigabit::Role, t1_info, t1_link, t1_stream},
        medium::*,
        receiver::*,
    },
};

fn forced_pair() -> MediumConfig {
    let grid = SampleGrid {
        symbol_rate_hz: 750_000_000,
        samples_per_ui: 16,
    };
    let endpoints = (0..2)
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
                max_no_transition_symbols: 4096,
                loss_error_ppm: 500_000,
            },
        })
        .collect::<Vec<_>>();
    MediumConfig {
        execution_contract: t1_link::CONTRACT.into(),
        standard_edition: "IEEE Std 802.3-2022".into(),
        physical_mode: PhysicalMode::Ieee1000BaseT1PointToPoint2022,
        plca: None,
        repeater: None,
        gigabit_t1: Some(t1_link::Config {
            stations: (0..2)
                .map(|i| t1_link::StationConfig {
                    endpoint: format!("p{i}"),
                    autoneg: false,
                    preferred_master: i == 0,
                    force_role: true,
                    nonce_low4: i as u8 + 1,
                    an_random_seed: i as u64 + 1,
                    training_seed: if i == 0 { 0x1a2b_3c4d5 } else { 0x1234_56789 },
                    data_seed: if i == 0 { 0x2255 } else { 0x315a },
                })
                .collect(),
            ideal_reference_echo_cancellation: true,
        }),
        gigabit_raw_merge: false,
        channel: ChannelConfig {
            grid,
            endpoints: endpoints.iter().map(|e| e.id.clone()).collect(),
            paths: vec![
                vec![
                    None,
                    Some(Path {
                        delay_samples: 0,
                        taps_q30: vec![Q30],
                    }),
                ],
                vec![
                    Some(Path {
                        delay_samples: 0,
                        taps_q30: vec![Q30],
                    }),
                    None,
                ],
            ],
            noise_rms_uv: vec![0; 2],
            noise_seed: 1,
            clip_uv: 4_000_000,
        },
        endpoints,
        half_duplex: false,
        carrier_threshold_uv: 100_000,
        carrier_hold_samples: 32,
        collision_threshold_uv: 200_000,
        ifg_bits: 96,
        jam_bits: 32,
        slot_bits: 512,
        retry_limit: 16,
        seed: 11,
        maximum_mac_bytes: 1518,
        maximum_samples: 100_000_000,
        trace_samples: false,
    }
}
fn mac() -> Vec<u8> {
    let mut bytes = vec![0xa5; 60];
    bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    bytes[12..14].copy_from_slice(&[8, 0]);
    bytes.extend(crc32(&bytes).to_le_bytes());
    bytes
}
#[test]
fn decoded_clause97_capture_cuts_keep_exact_rational_time_at_16_and_32_ui() {
    let mut times = vec![];
    for ui in [16, 32] {
        let grid = SampleGrid {
            symbol_rate_hz: 750_000_000,
            samples_per_ui: ui,
        };
        let mut tx = t1_stream::TransmitStream::new(Role::Master, 0x2255).unwrap();
        let mut rx = t1_stream::ReceiveStream::new(Role::Master, 0x2255, 1518).unwrap();
        let range = tx.enqueue_mac(&mac(), 1518).unwrap();
        let mut delivered = vec![];
        for _ in 0..2700 {
            delivered.extend(rx.push_symbol(tx.next_symbol().unwrap()).unwrap());
        }
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].sfd_end_physical_symbol, 44);
        assert_eq!(delivered[0].fcs_end_physical_symbol, 394);
        assert_eq!(delivered[0].sfd_end_physical_symbol, range.sfd_end_symbol());
        assert_eq!(delivered[0].fcs_end_physical_symbol, range.fcs_end_symbol());
        let sfd = sample_boundary_ps(grid, range.sfd_end_symbol() * ui as u64).unwrap();
        let fcs = sample_boundary_ps(grid, range.fcs_end_symbol() * ui as u64).unwrap();
        times.push((sfd, fcs));
    }
    assert_eq!(times[0], times[1]);
    assert_eq!(times[0], (58_667, 525_334));
}

#[test]
fn forced_pair_keeps_exact_sample_and_checkpoint_during_startup() {
    for ui in [16, 32] {
        let mut config = forced_pair();
        config.channel.grid.samples_per_ui = ui;
        config.carrier_hold_samples = ui * 2;
        for endpoint in &mut config.endpoints {
            endpoint.transmitter.grid.samples_per_ui = ui;
            endpoint.receiver.grid.samples_per_ui = ui;
        }
        config.validate().unwrap();
        let mut medium = SampledMedium::new(config.clone()).unwrap();
        let bytes = mac();
        medium
            .admit(MacTx {
                frame_id: "g0".into(),
                source_endpoint: "p0".into(),
                at_ps: 0,
                mac_bytes: bytes,
            })
            .unwrap();
        assert!(!medium.tx_ready("p0").unwrap());
        assert!(medium.advance_samples(1000).unwrap().is_empty());
        let saved = medium.checkpoint().unwrap();
        let mut restored = SampledMedium::restore(&config, &saved).unwrap();
        assert_eq!(
            medium.advance_samples(4000).unwrap(),
            restored.advance_samples(4000).unwrap()
        );
        assert_eq!(medium.next_sample_index(), 5000);
    }
}

#[test]
#[ignore = "runs Clause 98 HSM page exchange on two actual sample streams"]
fn autoneg_partner_pages_resolve_two_roles_through_channel() {
    let mut config = forced_pair();
    let gigabit = config.gigabit_t1.as_mut().unwrap();
    for station in &mut gigabit.stations {
        station.autoneg = true;
    }
    gigabit.stations[0].an_random_seed = 11;
    gigabit.stations[1].an_random_seed = 22;
    config.channel.paths[0][1].as_mut().unwrap().delay_samples = 16;
    config.channel.paths[1][0].as_mut().unwrap().delay_samples = 16;
    let mut medium = SampledMedium::new(config).unwrap();
    let mut completed = std::collections::BTreeSet::new();
    while medium.next_sample_index() < 2_000_000 && completed.len() < 2 {
        for effect in medium.advance_samples(65_536).unwrap() {
            if let PhyEffect::PcsControl {
                endpoint, command, ..
            } = effect
            {
                if command == "autoneg_complete" {
                    completed.insert(endpoint);
                }
            }
        }
    }
    assert_eq!(completed, ["p0".to_string(), "p1".to_string()].into());
}

#[test]
fn conflicting_forced_an_roles_reject_queued_mac_without_phy_transmission() {
    let mut config = forced_pair();
    let gigabit = config.gigabit_t1.as_mut().unwrap();
    for station in &mut gigabit.stations {
        station.autoneg = true;
    }
    gigabit.stations[1].preferred_master = true;
    gigabit.stations[0].an_random_seed = 11;
    gigabit.stations[1].an_random_seed = 22;
    config.channel.paths[0][1].as_mut().unwrap().delay_samples = 16;
    config.channel.paths[1][0].as_mut().unwrap().delay_samples = 16;
    let mut medium = SampledMedium::new(config).unwrap();
    medium
        .admit(MacTx {
            frame_id: "conflict".into(),
            source_endpoint: "p0".into(),
            at_ps: 0,
            mac_bytes: mac(),
        })
        .unwrap();
    let mut failed = 0;
    let mut started = 0;
    let mut controls = Vec::new();
    while medium.next_sample_index() < 2_000_000 && failed == 0 {
        for effect in medium.advance_samples(65_536).unwrap() {
            match effect {
                PhyEffect::RetryExhausted {
                    endpoint, origin, ..
                } if endpoint == "p0" => {
                    assert_eq!(origin.frame_id, "conflict");
                    failed += 1;
                }
                PhyEffect::TxStarted { endpoint, .. } if endpoint == "p0" => started += 1,
                PhyEffect::PcsControl {
                    endpoint, command, ..
                } => {
                    controls.push((endpoint, command));
                }
                _ => {}
            }
        }
    }
    assert_eq!((failed, started), (1, 0), "controls={controls:?}");
}

fn decode_real_mac_after_startup(config: MediumConfig) {
    let mut medium = SampledMedium::new(config).unwrap();
    let bytes = mac();
    medium
        .admit(MacTx {
            frame_id: "g0".into(),
            source_endpoint: "p0".into(),
            at_ps: 0,
            mac_bytes: bytes.clone(),
        })
        .unwrap();
    let mut started = 0;
    let mut sfd = 0;
    let mut fcs = 0;
    let mut coded = 0;
    let mut received = 0;
    let mut tx_sfd_sample = None;
    let mut tx_fcs_sample = None;
    let mut rx_capture = None;
    while medium.next_sample_index() < 80_000_000 && received == 0 {
        if medium.next_sample_index() % 5_000_000 < 65_536 {
            eprintln!(
                "sampled 1000BASE-T1 progress: {} samples",
                medium.next_sample_index()
            );
        }
        for effect in medium.advance_samples(65_536).unwrap() {
            match effect {
                PhyEffect::TxStarted { endpoint, .. } if endpoint == "p0" => started += 1,
                PhyEffect::TxMacSfd {
                    endpoint,
                    sample_index,
                    ..
                } if endpoint == "p0" => {
                    sfd += 1;
                    tx_sfd_sample = Some(sample_index);
                }
                PhyEffect::TxMacFcsEnd {
                    endpoint,
                    sample_index,
                    ..
                } if endpoint == "p0" => {
                    fcs += 1;
                    tx_fcs_sample = Some(sample_index);
                }
                PhyEffect::TxCodedEnd { endpoint, .. } if endpoint == "p0" => coded += 1,
                PhyEffect::RxFrame {
                    receiver_endpoint,
                    mac_bytes,
                    origins,
                    rx_sfd_sample,
                    rx_fcs_sample,
                    ..
                } if receiver_endpoint == "p1" => {
                    assert_eq!(mac_bytes, bytes);
                    assert_eq!(origins.len(), 1);
                    assert_eq!(origins[0].frame_id, "g0");
                    assert!(rx_sfd_sample < rx_fcs_sample);
                    rx_capture = Some((rx_sfd_sample, rx_fcs_sample));
                    received += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!((started, sfd, fcs, coded, received), (1, 1, 1, 1, 1));
    assert_eq!(
        rx_capture,
        Some((tx_sfd_sample.unwrap(), tx_fcs_sample.unwrap()))
    );
}

#[test]
#[ignore = "requires tens of millions of actual 750 MBd samples for Clause 97 training"]
fn forced_pair_decodes_real_mac_after_training_and_fec() {
    decode_real_mac_after_startup(forced_pair());
}

#[test]
#[ignore = "runs HSM AN, Clause 97 training, then actual coded MAC over sampled MDI"]
fn autoneg_pair_decodes_real_mac_after_training_and_fec() {
    let mut config = forced_pair();
    let gigabit = config.gigabit_t1.as_mut().unwrap();
    for station in &mut gigabit.stations {
        station.autoneg = true;
    }
    gigabit.stations[0].an_random_seed = 11;
    gigabit.stations[1].an_random_seed = 22;
    decode_real_mac_after_startup(config);
}

#[test]
#[ignore = "runs two complete sampled Clause 97 startups around a bilateral MDI outage"]
fn sampled_mdi_outage_aborts_active_mac_and_retrains_with_checkpoint_equivalence() {
    sampled_outage(false);
}
#[test]
#[ignore = "release gate: actual Clause98 exchange, active outage, new Clause98 exchange and fresh decode"]
fn autoneg_active_outage_restarts_actual_clause98_and_delivers_fresh_mac() {
    sampled_outage(true);
}
fn sampled_outage(autoneg: bool) {
    let mut config = forced_pair();
    if autoneg {
        for (i, station) in config
            .gigabit_t1
            .as_mut()
            .unwrap()
            .stations
            .iter_mut()
            .enumerate()
        {
            station.autoneg = true;
            station.an_random_seed = if i == 0 { 11 } else { 22 };
        }
    }
    for endpoint in &mut config.endpoints {
        endpoint.receiver.max_no_transition_symbols = 128;
    }
    config.maximum_samples = 150_000_000;
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    assert!(!medium.gigabit_bilateral_link_ready().unwrap());
    while medium.next_sample_index() < 80_000_000
        && !(medium.tx_ready("p0").unwrap() && medium.tx_ready("p1").unwrap())
    {
        medium.advance_samples(65_536).unwrap();
    }
    assert!(medium.tx_ready("p0").unwrap() && medium.tx_ready("p1").unwrap());
    medium
        .admit(MacTx {
            frame_id: "outage".into(),
            source_endpoint: "p0".into(),
            at_ps: sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap(),
            mac_bytes: mac(),
        })
        .unwrap();
    let mut started = false;
    for _ in 0..10_000 {
        started |= medium.advance_samples(16).unwrap().iter().any(
            |effect| matches!(effect, PhyEffect::TxStarted { endpoint, .. } if endpoint == "p0"),
        );
        if started {
            break;
        }
    }
    assert!(
        started,
        "outage must begin during actual coded transmission"
    );
    assert!(!medium.tx_ready("p0").unwrap());
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
    medium.set_gigabit_mdi_disconnected(true).unwrap();
    assert!(!medium.gigabit_bilateral_link_ready().unwrap());
    medium.advance_samples(500).unwrap();
    let checkpoint = medium.checkpoint().unwrap();
    let mut resumed = SampledMedium::restore(&config, &checkpoint).unwrap();
    let expected = medium.advance_samples(10_000).unwrap();
    let actual = resumed.advance_samples(10_000).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(resumed.checkpoint().unwrap(), medium.checkpoint().unwrap());
    assert!(actual.iter().any(|effect| matches!(
        effect,
        PhyEffect::PcsControl { command, .. } if command == "link_retraining"
    )));
    assert!(actual.iter().any(|effect| matches!(
        effect,
        PhyEffect::RetryExhausted { origin, .. } if origin.frame_id == "outage"
    )));
    assert!(
        !actual
            .iter()
            .any(|effect| matches!(effect, PhyEffect::RxFrame { .. }))
    );
    medium.set_gigabit_mdi_disconnected(false).unwrap();
    let mut renegotiated = std::collections::BTreeSet::new();
    while medium.next_sample_index() < 145_000_000
        && !(medium.tx_ready("p0").unwrap() && medium.tx_ready("p1").unwrap())
    {
        for effect in medium.advance_samples(65_536).unwrap() {
            if let PhyEffect::PcsControl {
                endpoint, command, ..
            } = effect
            {
                if command == "autoneg_complete" {
                    renegotiated.insert(endpoint);
                }
            }
        }
    }
    if autoneg {
        assert_eq!(renegotiated.len(), 2);
    }
    assert!(medium.tx_ready("p0").unwrap() && medium.tx_ready("p1").unwrap());
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
    let fresh = mac();
    medium
        .admit(MacTx {
            frame_id: "fresh".into(),
            source_endpoint: "p0".into(),
            at_ps: sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap(),
            mac_bytes: fresh.clone(),
        })
        .unwrap();
    let mut received = vec![];
    while medium.next_sample_index() < 146_000_000 && received.is_empty() {
        received.extend(
            medium
                .advance_samples(65_536)
                .unwrap()
                .into_iter()
                .filter_map(|effect| match effect {
                    PhyEffect::RxFrame {
                        mac_bytes, origins, ..
                    } => Some((mac_bytes, origins)),
                    _ => None,
                }),
        );
    }
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, fresh);
    assert_eq!(received[0].1[0].frame_id, "fresh");
}

#[test]
#[ignore = "runs complete sampled Clause 97 startup before two raw mPackets share one RS codeword"]
fn raw_merge_packets_share_codeword_with_distinct_lineage_and_real_xmii_release() {
    let mut config = forced_pair();
    config.gigabit_raw_merge = true;
    let grid = config.channel.grid;
    let mut medium = SampledMedium::new(config).unwrap();
    while medium.next_sample_index() < 80_000_000 && !medium.tx_ready("p0").unwrap() {
        medium.advance_samples(65_536).unwrap();
    }
    assert!(medium.tx_ready("p0").unwrap());
    for _ in 0..500 {
        let status = medium.raw_ingress_status("p0").unwrap();
        assert_ne!(status.next_cut_sample, u64::MAX);
        medium
            .advance_samples(status.next_cut_sample - medium.next_sample_index())
            .unwrap();
        let status = medium.raw_ingress_status("p0").unwrap();
        if status.consumed_gmii_slots % 450 == 0 {
            break;
        }
        medium
            .advance_samples(6 * grid.samples_per_ui as u64)
            .unwrap();
    }
    let first = medium.raw_ingress_status("p0").unwrap();
    assert!(first.ready);
    assert_eq!(first.consumed_gmii_slots % 450, 0);
    let mut a = [vec![0x55; 7], vec![0xe6], mac()].concat();
    let mut b_mac = mac();
    b_mac[15] ^= 1;
    let n = b_mac.len();
    let fcs = crc32(&b_mac[..n - 4]).to_le_bytes();
    b_mac[n - 4..].copy_from_slice(&fcs);
    let b = [vec![0x55; 6], vec![0x61, 0xe6], b_mac].concat();
    let at = sample_boundary_ps(grid, medium.next_sample_index()).unwrap();
    assert!(matches!(
        medium
            .admit_raw_mpacket(RawMPacketTx {
                reference_sample: None,
                packet_id: 1,
                generation: 0,
                source_endpoint: "p0".into(),
                at_ps: at,
                xmii_bytes: a.clone(),
                origin_frame_id: "a".into()
            })
            .unwrap(),
        PhyEffect::XmiStarted { packet_id: 1, .. }
    ));
    a[40] ^= 0x11;
    let fcs = crc32(&a[8..a.len() - 4]).to_le_bytes();
    let fcs_start = a.len() - 4;
    a[fcs_start..].copy_from_slice(&fcs);
    assert!(
        medium
            .replace_unconsumed_raw_suffix("p0", 1, 7, 40, &a[40..], 8)
            .is_err()
    );
    medium
        .replace_unconsumed_raw_suffix("p0", 1, 0, 40, &a[40..], 1)
        .unwrap();
    let before_consumption = medium.checkpoint().unwrap();
    medium
        .advance_samples(6 * grid.samples_per_ui as u64)
        .unwrap();
    assert!(
        medium
            .replace_unconsumed_raw_suffix("p0", 1, 1, 0, &a, 2)
            .is_err()
    );
    assert_ne!(medium.checkpoint().unwrap(), before_consumption);
    medium
        .replace_unconsumed_raw_suffix("p0", 1, 1, 1, &a[1..], 2)
        .unwrap();
    let mut released = false;
    while !released {
        for effect in medium.advance_samples(96).unwrap() {
            if matches!(
                effect,
                PhyEffect::XmiReleased {
                    packet_id: 1,
                    generation: 2,
                    ..
                }
            ) {
                released = true;
            }
        }
    }
    assert!(medium.raw_ingress_status("p0").unwrap().ready);
    let at = sample_boundary_ps(grid, medium.next_sample_index()).unwrap();
    medium
        .admit_raw_mpacket(RawMPacketTx {
            reference_sample: None,
            packet_id: 2,
            generation: 0,
            source_endpoint: "p0".into(),
            at_ps: at,
            xmii_bytes: b.clone(),
            origin_frame_id: "b".into(),
        })
        .unwrap();
    let mut received = Vec::new();
    let mut coded = Vec::new();
    while medium.next_sample_index() < 81_000_000 && received.len() < 2 {
        for effect in medium.advance_samples(65_536).unwrap() {
            match effect {
                PhyEffect::RawMPacketIngress {
                    packet_id,
                    xmii_bytes,
                    origins,
                    decoded_at_sample,
                    ..
                } => {
                    assert_eq!(origins.len(), 1);
                    received.push((
                        packet_id,
                        xmii_bytes,
                        origins[0].frame_id.clone(),
                        decoded_at_sample,
                    ));
                }
                PhyEffect::MdiCodedEnd { packet_id, .. } => coded.push(packet_id),
                _ => {}
            }
        }
    }
    received.sort_by_key(|r| r.0);
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].1, a);
    assert_eq!(received[1].1, b);
    assert_eq!(received[0].2, "a");
    assert_eq!(received[1].2, "b");
    assert_eq!(received[0].3, received[1].3);
    assert_eq!(coded, vec![1, 2]);
}

#[test]
fn clause98_hsm_page_has_exact_transition_timing_and_rejects_corrupted_payload() {
    let grid = SampleGrid {
        symbol_rate_hz: 750_000_000,
        samples_per_ui: 32,
    };
    assert_eq!(
        sample_boundary_ps(grid, an_hsm::SAMPLES_PER_POSITION_750M_32 as u64).unwrap(),
        30_000
    );
    assert_eq!(
        sample_boundary_ps(
            grid,
            ((an_hsm::DME_POSITIONS - 1) * an_hsm::SAMPLES_PER_POSITION_750M_32) as u64
        )
        .unwrap(),
        4_680_000
    );
    let page = an_hsm::BasePage::new_1000base_t1(0x13, true);
    let bits = page.to_bits().unwrap();
    assert_eq!(bits[23], 1); // Clause 98B A2.
    let levels = an_hsm::encode_page(&bits, true).unwrap();
    assert_eq!(an_hsm::decode_page(&levels).unwrap(), bits);
    let mut corrupted = levels;
    for level in &mut corrupted[27..156] {
        *level = -*level;
    }
    assert_eq!(
        an_hsm::decode_page(&corrupted).unwrap_err(),
        "Clause 98 HSM DME page CRC16 failed"
    );
}

#[test]
fn clause97_infofield_has_wire_order_crc_and_partner_capability_decode() {
    let local = t1_info::InfoField {
        role: Role::Master,
        pfc24: t1_info::INITIAL_PFC24,
        state: t1_info::PmaState::Training,
        local_receiver_ok: true,
        enable_or_timing_lock: true,
        data_scrambler_seed: 0x4e35,
        eee_capable: false,
        oam_capable: false,
        user_config: 0,
        data_switch_pfc24: 0,
    };
    let wire = local.to_bytes().unwrap();
    assert_eq!(&wire[..3], &[0xbb, 0xa7, 0x00]);
    assert_eq!(&wire[3..6], &[14, 0, 0]);
    assert_eq!(wire[6], 0x30); // TRAINING, receive OK, slave TX allowed.
    assert_eq!(
        t1_info::InfoField::from_wire_bits(&local.to_wire_bits().unwrap(), Role::Master).unwrap(),
        local
    );
    let mut corrupted = wire;
    corrupted[9] ^= 1;
    assert_eq!(
        t1_info::InfoField::from_bytes(&corrupted, Role::Master).unwrap_err(),
        "Clause 97 InfoField CRC16 failed"
    );
    let mut countdown = local;
    countdown.pfc24 = 899;
    countdown.state = t1_info::PmaState::Countdown;
    countdown.data_switch_pfc24 = 915;
    let decoded =
        t1_info::InfoField::from_bytes(&countdown.to_bytes().unwrap(), Role::Master).unwrap();
    assert_eq!(decoded.data_switch_pfc24, 915);
    assert_eq!(decoded.state, t1_info::PmaState::Countdown);
}

#[test]
#[ignore = "runs actual sampled Clause97 startup for mapped raw delimiter golden coordinates"]
fn raw_mdi_delimiter_actual_renderer_and_recovered_references_both_directions() {
    for (samples_per_ui, delay_samples) in [(16, 0), (32, 4)] {
        let mut config = forced_pair();
        config.gigabit_raw_merge = true;
        config.channel.grid.samples_per_ui = samples_per_ui;
        config.carrier_hold_samples = samples_per_ui * 2;
        config.maximum_samples = 200_000_000;
        for endpoint in &mut config.endpoints {
            endpoint.transmitter.grid = config.channel.grid;
            endpoint.receiver.grid = config.channel.grid;
        }
        for path in config.channel.paths.iter_mut().flatten().flatten() {
            path.delay_samples = delay_samples;
        }
        assert!(config.exact_mdi_timestamp_mapping_supported());
        let envelope = config.raw_timing_envelope().unwrap();
        for direction in &envelope.directions {
            assert_eq!(
                direction.pmax_samples_upper_bound,
                delay_samples as u64 + samples_per_ui as u64 - 1
            );
        }
        let mut unsupported = config.clone();
        unsupported.channel.paths[0][1]
            .as_mut()
            .unwrap()
            .taps_q30
            .push(1);
        assert!(unsupported.raw_timing_envelope().is_none());
        let grid = config.channel.grid;
        let ui = samples_per_ui as u64;
        let mut base = SampledMedium::new(config.clone()).unwrap();
        while base.next_sample_index() < 160_000_000
            && !(base.tx_ready("p0").unwrap() && base.tx_ready("p1").unwrap())
        {
            base.advance_samples(65_536).unwrap();
        }
        assert!(base.tx_ready("p0").unwrap() && base.tx_ready("p1").unwrap());
        let certificate = base
            .raw_timing_certificate()
            .unwrap()
            .expect("paired initial raw codeword alignment");
        assert!(certificate.bilateral_ready && certificate.initial_idle_verified);
        assert_eq!(certificate.directions.len(), 2);
        for direction in &certificate.directions {
            assert_eq!(
                direction.source_initial_idle_end_sample,
                direction.source_data_epoch_sample + 2700 * ui
            );
            assert_eq!(
                direction.received_initial_idle_end_sample,
                direction.reconstructed_rx_epoch_sample + 2700 * ui
            );
            assert_eq!(
                direction.pmax_samples,
                direction
                    .reconstructed_rx_epoch_sample
                    .saturating_sub(direction.source_data_epoch_sample)
            );
            assert_eq!(direction.callback_rounding_samples, 0);
        }
        let mut outage = base.clone();
        outage.set_gigabit_mdi_disconnected(true).unwrap();
        assert!(outage.raw_timing_certificate().unwrap().is_none());
        let packet = [vec![0x55; 7], vec![0xe6], mac()].concat();
        for source in 0..2 {
            let mut medium = base.clone();
            let endpoint = format!("p{source}");
            // SEND_DATA follows mandatory DataIdle MINWAIT. Admission therefore
            // uses a real public cut at an arbitrary subsequent450-slot boundary.
            let slot = loop {
                let status = medium.raw_ingress_status(&endpoint).unwrap();
                medium
                    .advance_samples(status.next_cut_sample - medium.next_sample_index())
                    .unwrap();
                let status = medium.raw_ingress_status(&endpoint).unwrap();
                if status.ready && status.consumed_gmii_slots % 450 == 0 {
                    break status.consumed_gmii_slots;
                }
                medium.advance_samples(6 * ui).unwrap();
            };
            let admission = medium.next_sample_index();
            let packet_id = source as u64 + 101;
            medium
                .admit_raw_mpacket(RawMPacketTx {
                    reference_sample: None,
                    packet_id,
                    generation: 0,
                    source_endpoint: endpoint.clone(),
                    at_ps: sample_boundary_ps(grid, admission).unwrap(),
                    xmii_bytes: packet.clone(),
                    origin_frame_id: format!("golden-{source}"),
                })
                .unwrap();
            let marker_cut = admission + 404 * ui;
            let mut effects = medium.advance_samples(marker_cut - 1 - admission).unwrap();
            let saved = medium.checkpoint().unwrap();
            let mut restored = SampledMedium::restore(&config, &saved).unwrap();
            assert_eq!(
                restored.raw_timing_certificate().unwrap(),
                Some(certificate.clone())
            );
            // SMD was consumed atgeneration0. The final four bytes remain
            // unconsumed; a permitted suffix edit must not rewrite that capture.
            medium
                .replace_unconsumed_raw_suffix(&endpoint, packet_id, 0, 68, &packet[68..], 1)
                .unwrap();
            restored
                .replace_unconsumed_raw_suffix(&endpoint, packet_id, 0, 68, &packet[68..], 1)
                .unwrap();
            while medium.next_sample_index() < admission + 7000 * ui {
                let expected = medium.advance_samples(256).unwrap();
                assert_eq!(restored.advance_samples(256).unwrap(), expected);
                effects.extend(expected);
            }
            assert_eq!(restored.checkpoint().unwrap(), medium.checkpoint().unwrap());
            let tx = effects
                .iter()
                .find_map(|effect| match effect {
                    PhyEffect::MdiDelimiterEnd {
                        source_endpoint,
                        packet_id: id,
                        generation,
                        delimiter_slot,
                        sample_index,
                    } if *id == packet_id => {
                        assert_eq!(source_endpoint, &endpoint);
                        assert_eq!(*generation, 0);
                        assert_eq!(*delimiter_slot, slot + 7);
                        Some(*sample_index)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(tx, marker_cut);
            let (rx, available) = effects
                .iter()
                .find_map(|effect| match effect {
                    PhyEffect::RawMPacketIngress {
                        packet_id: id,
                        generation,
                        final_generation,
                        delimiter_sample,
                        receive_cut_sample,
                        decoded_at_sample,
                        ..
                    } if *id == packet_id => {
                        assert_eq!((*generation, *final_generation), (0, 1));
                        assert_eq!(receive_cut_sample, decoded_at_sample);
                        Some((delimiter_sample.unwrap(), *decoded_at_sample))
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                rx,
                tx + delay_samples as u64,
                "source {source} exact channel-mapped path"
            );
            assert!(available > rx);
            let epoch = admission - (2340 + slot * 6) * ui;
            assert_eq!(tx, epoch + (2700 + slot / 450 * 2700 + 44) * ui);
            let elapsed = grid.time(tx - admission).unwrap();
            assert_eq!(elapsed.numerator_ps * 3, 1_616_000 * elapsed.denominator);
            eprintln!(
                "raw mapped source={source} U={ui} path={delay_samples} slot={slot} tx={tx} rx={rx} availability={available}"
            );
        }
    }
}

#[test]
#[ignore = "runs sampled startup and adjacent canonical packets over a sparse FIR tail"]
fn sparse_fir_tail_after_decode_does_not_contaminate_next_mac_origin() {
    let mut config = forced_pair();
    for path in config.channel.paths.iter_mut().flatten().flatten() {
        path.taps_q30 = vec![Q30, 0, 0, 0, Q30 / 32];
    }
    assert!(!config.exact_mdi_timestamp_mapping_supported());
    let mut medium = SampledMedium::new(config).unwrap();
    let bytes = mac();
    medium
        .admit(MacTx {
            frame_id: "fir-first".into(),
            source_endpoint: "p0".into(),
            at_ps: 0,
            mac_bytes: bytes.clone(),
        })
        .unwrap();
    let mut received = Vec::new();
    while medium.next_sample_index() < 80_000_000 && received.is_empty() {
        for effect in medium.advance_samples(65_536).unwrap() {
            if let PhyEffect::RxFrame {
                receiver_endpoint,
                origins,
                mac_bytes,
                rx_mapped_sfd_sample,
                ..
            } = effect
            {
                if receiver_endpoint == "p1" {
                    assert_eq!(mac_bytes, bytes);
                    assert!(rx_mapped_sfd_sample.is_none());
                    received.push(origins);
                }
            }
        }
    }
    assert_eq!(received.len(), 1);
    // The delayed nonzero tap remains physically observable after decode clears
    // the previous frame. Its source extent must end before the following SFD.
    medium.advance_samples(4096).unwrap();
    let at_ps =
        sample_boundary_ps(medium.config().channel.grid, medium.next_sample_index()).unwrap();
    medium
        .admit(MacTx {
            frame_id: "fir-second".into(),
            source_endpoint: "p0".into(),
            at_ps,
            mac_bytes: bytes.clone(),
        })
        .unwrap();
    while medium.next_sample_index() < 81_000_000 && received.len() < 2 {
        for effect in medium.advance_samples(65_536).unwrap() {
            if let PhyEffect::RxFrame {
                receiver_endpoint,
                origins,
                mac_bytes,
                rx_mapped_sfd_sample,
                ..
            } = effect
            {
                if receiver_endpoint == "p1" {
                    assert_eq!(mac_bytes, bytes);
                    assert!(rx_mapped_sfd_sample.is_none());
                    received.push(origins);
                }
            }
        }
    }
    assert_eq!(received.len(), 2);
    for (origins, expected) in received.iter().zip(["fir-first", "fir-second"]) {
        assert_eq!(origins.len(), 1);
        assert_eq!(origins[0].frame_id, expected);
    }
}

#[test]
#[ignore = "actual sampled startup matrix for fractional raw admission references"]
fn raw_sample_reference_actual_delay_matrix_and_rejects_stale_future_late() {
    use dir_simulator::runtime::ethernet::phy::attachment::{
        AttachedMedium, AttachmentConfig, AttachmentEffect, PhysicalPort,
    };
    let mut phases = std::collections::BTreeSet::new();
    for (samples_per_ui, delay) in [(16, 0), (16, 4), (16, 16), (16, 32)] {
        let mut config = forced_pair();
        config.gigabit_raw_merge = true;
        config.maximum_samples = 200_000_000;
        config.channel.grid.samples_per_ui = samples_per_ui;
        config.carrier_hold_samples = samples_per_ui * 2;
        for e in &mut config.endpoints {
            e.transmitter.grid = config.channel.grid;
            e.receiver.grid = config.channel.grid;
        }
        for path in config.channel.paths.iter_mut().flatten().flatten() {
            path.delay_samples = delay
        }
        let grid = config.channel.grid;
        let origin = 12345;
        let mut base = AttachedMedium::new(AttachmentConfig {
            origin_ps: origin,
            medium: config,
            bindings: vec![],
            physical_ports: (0..2)
                .map(|i| PhysicalPort {
                    endpoint: format!("p{i}"),
                    tx_port: format!("Net.p{i}.tx"),
                    rx_port: format!("Net.p{i}.rx"),
                })
                .collect(),
            lookahead_samples: Some(65536),
        })
        .unwrap();
        while base.next_sample_index() < 160_000_000
            && !base.gigabit_bilateral_link_ready().unwrap()
        {
            base.advance_samples(65536).unwrap();
        }
        assert!(base.gigabit_bilateral_link_ready().unwrap());
        for source in 0..2 {
            let mut medium = base.clone();
            let port = format!("Net.p{source}.tx");
            loop {
                let status = medium.raw_ingress_status(&port).unwrap();
                medium
                    .advance_samples(status.next_cut_sample - medium.next_sample_index())
                    .unwrap();
                if medium.raw_ingress_status(&port).unwrap().ready {
                    break;
                }
                medium.advance_samples(1).unwrap();
            }
            let sample = medium.next_sample_index();
            let exact = grid.time(sample).unwrap();
            let at = origin + sample_boundary_ps(grid, sample).unwrap();
            let remainder = exact.numerator_ps % exact.denominator;
            phases.insert(remainder * 3 / exact.denominator);
            eprintln!(
                "delay={delay} source={source} sample={sample} phase={}/3 observed={at}",
                remainder * 3 / exact.denominator
            );
            let bytes = [vec![0x55; 7], vec![0xe6], mac()].concat();
            let request = RawMPacketTx {
                reference_sample: Some(sample),
                packet_id: 101 + source,
                generation: 0,
                source_endpoint: format!("p{source}"),
                at_ps: at,
                xmii_bytes: bytes.clone(),
                origin_frame_id: format!("sample-{delay}-{source}"),
            };
            for (reference, observed) in [(sample - 1, at), (sample + 1, at), (sample, at + 55)] {
                let before = medium.checkpoint().unwrap();
                let mut bad = request.clone();
                bad.reference_sample = Some(reference);
                bad.at_ps = observed;
                assert!(medium.admit_raw_mpacket(&port, bad).is_err());
                assert_eq!(before, medium.checkpoint().unwrap());
            }
            if remainder != 0 {
                let mut legacy = medium.clone();
                let mut request = request.clone();
                request.reference_sample = None;
                assert!(legacy.admit_raw_mpacket(&port, request).is_err());
            }
            assert!(
                matches!(medium.admit_raw_mpacket(&port,request).unwrap(),AttachmentEffect::Physical{effect:PhyEffect::XmiStarted{sample_index,..}} if sample_index==sample)
            );
            let mut effects = Vec::new();
            while medium.next_sample_index() < sample + 120_000 {
                effects.extend(medium.advance_samples(4096).unwrap());
            }
            let release = effects
                .iter()
                .find_map(|e| match e {
                    AttachmentEffect::Physical {
                        effect: PhyEffect::XmiReleased { sample_index, .. },
                    } => Some(*sample_index),
                    _ => None,
                })
                .unwrap();
            let ingress = effects
                .iter()
                .find_map(|e| match e {
                    AttachmentEffect::Physical {
                        effect:
                            PhyEffect::RawMPacketIngress {
                                xmii_bytes,
                                decoded_at_sample,
                                ..
                            },
                    } => Some((xmii_bytes, *decoded_at_sample)),
                    _ => None,
                })
                .unwrap();
            assert_eq!(ingress.0, &bytes);
            assert!(
                ingress.1 > release,
                "actual PCS decode must survive release"
            );
            let n = u128::from(at - origin) * exact.denominator;
            assert!(n >= exact.numerator_ps && n - exact.numerator_ps < exact.denominator);
        }
    }
    assert!(
        phases.contains(&1) && phases.contains(&2),
        "both fractional phases required: {phases:?}"
    );
}

#[test]
#[ignore = "release gate: consumes every sample of the real 97.5ms initial maxwait"]
fn initial_quiet_maxwait_restarts_then_restored_partner_delivers_fresh_mac() {
    let mut config = forced_pair();
    config.maximum_samples = 1_400_000_000;
    let timeout = t1_link::MAXWAIT_NS * 12;
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    medium.set_gigabit_mdi_disconnected(true).unwrap();
    let mut restarts = std::collections::BTreeSet::new();
    while medium.next_sample_index() <= timeout {
        let count = (timeout + 1 - medium.next_sample_index()).min(1_000_000);
        for effect in medium.advance_samples(count).unwrap() {
            match effect {
                PhyEffect::PcsControl {
                    endpoint,
                    command,
                    sample_index,
                } if command.contains("maxwait_timer") => {
                    assert_eq!(sample_index, timeout + 1);
                    assert!(command.starts_with("startup_restarting:"));
                    restarts.insert(endpoint);
                }
                PhyEffect::TxStarted { .. } | PhyEffect::RxFrame { .. } => {
                    panic!("data during initial quiet")
                }
                _ => {}
            }
        }
    }
    assert_eq!(restarts.len(), 2);
    assert!(!medium.gigabit_bilateral_link_ready().unwrap());
    let saved = medium.checkpoint().unwrap();
    let mut continued = SampledMedium::restore(&config, &saved).unwrap();
    assert_eq!(
        medium.advance_samples(4096).unwrap(),
        continued.advance_samples(4096).unwrap()
    );
    medium.set_gigabit_mdi_disconnected(false).unwrap();
    let request = MacTx {
        frame_id: "fresh-after-maxwait".into(),
        source_endpoint: "p0".into(),
        at_ps: sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap(),
        mac_bytes: mac(),
    };
    medium.admit(request.clone()).unwrap();
    let mut received = vec![];
    while received.is_empty() && medium.next_sample_index() < 1_399_000_000 {
        for effect in medium.advance_samples(65_536).unwrap() {
            if let PhyEffect::RxFrame {
                mac_bytes, origins, ..
            } = effect
            {
                received.push((mac_bytes, origins));
            }
        }
    }
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, request.mac_bytes);
    assert_eq!(received[0].1[0].frame_id, request.frame_id);
    // RX can decode the master's new frame while the slave is still in its
    // final SEND_IDLE minwait. Wait for that independent TX readiness cut.
    while !medium.gigabit_bilateral_link_ready().unwrap()
        && medium.next_sample_index() < 1_399_000_000
    {
        assert!(
            !medium
                .advance_samples(65_536)
                .unwrap()
                .iter()
                .any(|e| matches!(e, PhyEffect::RxFrame { .. }))
        );
    }
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
}

#[test]
fn terminal_an_rejection_releases_exact_attachment_admission_without_ready_or_eof() {
    use dir_simulator::runtime::ethernet::phy::attachment::*;
    let mut config = forced_pair();
    let stations = &mut config.gigabit_t1.as_mut().unwrap().stations;
    for station in stations.iter_mut() {
        station.autoneg = true;
        station.preferred_master = true;
    }
    stations[0].an_random_seed = 11;
    stations[1].an_random_seed = 22;
    let attachment = AttachmentConfig {
        origin_ps: 0,
        medium: config,
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
    let mut medium = AttachedMedium::new(attachment).unwrap();
    let frame = decode_mac(&mac(), 1518).unwrap().wire;
    medium.admit("tx0", "terminal", 0, &frame).unwrap();
    let mut serialized = 0;
    let mut released = 0;
    for _ in 0..2_000_000 {
        for effect in medium.advance_one_sample().unwrap() {
            match effect {
                AttachmentEffect::TxSerialized {
                    transfer_id,
                    success,
                    ..
                } => {
                    assert_eq!(transfer_id, "terminal");
                    assert!(!success);
                    serialized += 1;
                }
                AttachmentEffect::TxReleased {
                    transfer_id,
                    success,
                    ..
                } => {
                    assert_eq!(transfer_id, "terminal");
                    assert!(!success);
                    released += 1;
                }
                AttachmentEffect::Received { .. } => {
                    panic!("terminal rejection fabricated reception")
                }
                AttachmentEffect::Physical {
                    effect: PhyEffect::TxFinished { .. } | PhyEffect::TxMacFcsEnd { .. },
                } => panic!("terminal rejection fabricated EOF"),
                _ => {}
            }
        }
        if released > 0 {
            break;
        }
    }
    assert_eq!((serialized, released), (1, 1));
    assert!(!medium.tx_ready("tx0").unwrap());
    let checkpoint = medium.checkpoint().unwrap();
    let at = medium.next_event_ps().unwrap();
    assert!(
        medium
            .admit("tx0", "new-after-terminal", at, &frame)
            .is_err()
    );
    assert_eq!(medium.checkpoint().unwrap(), checkpoint);
}

#[test]
#[ignore = "release gate: real sampled training interruption and complete fresh retraining"]
fn training_decode_failure_restarts_acquisition_and_eventually_delivers_fresh_frame() {
    let mut config = forced_pair();
    config.maximum_samples = 200_000_000;
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    let mut both_training = false;
    while medium.next_sample_index() < 60_000_000 {
        medium.advance_samples(16_384).unwrap();
        let cp: serde_json::Value = serde_json::from_slice(&medium.checkpoint().unwrap()).unwrap();
        both_training = (0..2).all(|i| {
            cp["state"]["ports"][i]["gigabit_link"]["phase"] == "training"
                && !cp["state"]["ports"][i]["gigabit_link"]["training_rx"].is_null()
                && !cp["state"]["ports"][i]["gigabit_link"]["training_tx"].is_null()
        });
        if both_training {
            break;
        }
    }
    assert!(both_training);
    medium.set_gigabit_mdi_disconnected(true).unwrap();
    let mut restarts = medium.advance_samples(128).unwrap();
    medium.set_gigabit_mdi_disconnected(false).unwrap();
    while medium.next_sample_index() < 190_000_000
        && !medium.gigabit_bilateral_link_ready().unwrap()
    {
        restarts.extend(medium.advance_samples(65_536).unwrap());
    }
    assert!(
        restarts
            .iter()
            .any(|e| matches!(e,PhyEffect::PcsControl{command,..}
        if command.starts_with("startup_restarting:")&&!command.contains("maxwait")))
    );
    assert!(
        !restarts
            .iter()
            .any(|e| matches!(e, PhyEffect::RxFrame { .. }))
    );
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
    medium
        .admit(MacTx {
            frame_id: "fresh-after-training-failure".into(),
            source_endpoint: "p0".into(),
            at_ps: sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap(),
            mac_bytes: mac(),
        })
        .unwrap();
    let mut frames = vec![];
    while medium.next_sample_index() < 199_000_000 && frames.is_empty() {
        frames.extend(
            medium
                .advance_samples(65_536)
                .unwrap()
                .into_iter()
                .filter_map(|e| match e {
                    PhyEffect::RxFrame {
                        mac_bytes, origins, ..
                    } => Some((mac_bytes, origins)),
                    _ => None,
                }),
        );
    }
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].0, mac());
    assert_eq!(frames[0].1[0].frame_id, "fresh-after-training-failure");
}

#[test]
#[ignore = "release gate: active sampled PCS corruption versus persistent physical loss"]
fn short_separated_mdi_bursts_reject_active_codeword_without_signal_loss_or_fabricated_rx() {
    let mut config = forced_pair();
    config.maximum_samples = 100_000_000;
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    while !medium.gigabit_bilateral_link_ready().unwrap() && medium.next_sample_index() < 80_000_000
    {
        medium.advance_samples(65_536).unwrap();
    }
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
    medium
        .admit(MacTx {
            frame_id: "decoder-reject".into(),
            source_endpoint: "p0".into(),
            at_ps: sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap(),
            mac_bytes: mac(),
        })
        .unwrap();
    let mut effects = vec![];
    loop {
        let next = medium.advance_samples(16).unwrap();
        let started = next
            .iter()
            .any(|e| matches!(e, PhyEffect::TxStarted { .. }));
        effects.extend(next);
        if started {
            break;
        }
    }
    for _ in 0..10 {
        medium.set_gigabit_mdi_disconnected(true).unwrap();
        effects.extend(medium.advance_samples(50 * 16).unwrap());
        medium.set_gigabit_mdi_disconnected(false).unwrap();
        effects.extend(medium.advance_samples(16 * 16).unwrap());
    }
    effects.extend(medium.advance_samples(100_000).unwrap());
    assert!(
        effects
            .iter()
            .any(|e| matches!(e,PhyEffect::RxAbort{reason,..}
        if reason == "invalid 1000BASE-T1 PAM3 symbol pair")),
        "{effects:?}"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e,PhyEffect::PcsControl{command,..}
        if command=="link_retraining"))
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, PhyEffect::RxFrame { .. }))
    );
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
}

#[test]
#[ignore = "release gate: actual partial Clause98 DME page interruption, rejection and reacquisition"]
fn autoneg_partial_dme_page_interruption_rejects_then_reacquires_partner() {
    let mut config = forced_pair();
    config.maximum_samples = 100_000_000;
    for (i, s) in config
        .gigabit_t1
        .as_mut()
        .unwrap()
        .stations
        .iter_mut()
        .enumerate()
    {
        s.autoneg = true;
        s.an_random_seed = if i == 0 { 11 } else { 22 };
    }
    let mut medium = SampledMedium::new(config.clone()).unwrap();
    let mut receiving = false;
    while medium.next_sample_index() < 1_000_000 {
        medium.advance_samples(1024).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&medium.checkpoint().unwrap()).unwrap();
        receiving =
            (0..2).any(|i| saved["state"]["ports"][i]["gigabit_link"]["an"]["phase"] == "receive");
        if receiving {
            break;
        }
    }
    assert!(receiving);
    medium.set_gigabit_mdi_disconnected(true).unwrap();
    let cp = medium.checkpoint().unwrap();
    let mut resumed = SampledMedium::restore(&config, &cp).unwrap();
    let mut effects = medium.advance_samples(4096).unwrap();
    assert_eq!(effects, resumed.advance_samples(4096).unwrap());
    effects.extend(medium.advance_samples(100_000).unwrap());
    medium.set_gigabit_mdi_disconnected(false).unwrap();
    while !medium.gigabit_bilateral_link_ready().unwrap() && medium.next_sample_index() < 85_000_000
    {
        effects.extend(medium.advance_samples(65_536).unwrap());
    }
    assert!(
        effects
            .iter()
            .any(|e| matches!(e,PhyEffect::PcsControl{command,..}
        if command=="autoneg_page_rejected"))
    );
    let completed = effects
        .iter()
        .filter_map(|e| match e {
            PhyEffect::PcsControl {
                endpoint, command, ..
            } if command == "autoneg_complete" => Some(endpoint.clone()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(completed.len(), 2);
    assert!(medium.gigabit_bilateral_link_ready().unwrap());
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, PhyEffect::RxFrame { .. }))
    );
    medium
        .admit(MacTx {
            frame_id: "fresh-after-an-interruption".into(),
            source_endpoint: "p0".into(),
            at_ps: sample_boundary_ps(config.channel.grid, medium.next_sample_index()).unwrap(),
            mac_bytes: mac(),
        })
        .unwrap();
    let mut received = vec![];
    while received.is_empty() && medium.next_sample_index() < 90_000_000 {
        received.extend(
            medium
                .advance_samples(65_536)
                .unwrap()
                .into_iter()
                .filter_map(|e| match e {
                    PhyEffect::RxFrame {
                        mac_bytes, origins, ..
                    } => Some((mac_bytes, origins)),
                    _ => None,
                }),
        );
    }
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].0, mac());
    assert_eq!(received[0].1[0].frame_id, "fresh-after-an-interruption");
}
