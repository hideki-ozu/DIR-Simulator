//! Independent sampled-channel oracle; no IEEE codec conformance is asserted.
use dir_simulator::runtime::ethernet::phy;
use phy::{channel::*, receiver::*, *};
fn grid(samples_per_ui: u32) -> SampleGrid {
    SampleGrid {
        symbol_rate_hz: 750_000_000,
        samples_per_ui,
    }
}
fn tx_config(samples_per_ui: u32) -> TransmitterConfig {
    TransmitterConfig {
        grid: grid(samples_per_ui),
        peak_uv: 1_000_000,
        transition_ui_ppm: 250_000,
        clock_ppm: 0,
        filter_taps_q30: vec![Q30],
    }
}
fn rx_config(samples_per_ui: u32) -> ReceiverConfig {
    ReceiverConfig {
        grid: grid(samples_per_ui),
        peak_uv: 1_000_000,
        frontend_taps_q30: vec![Q30],
        adc_bits: 16,
        adc_full_scale_uv: 3_000_000,
        ffe_taps: 3,
        echo_taps: 3,
        adaptation_q30: Q30 / 4,
        minimum_training_symbols: 128,
        lock_error_ppm: 80_000,
        clock_ppm: 0,
        max_tracking_ppm: 0,
        max_no_transition_symbols: 128,
        loss_error_ppm: 500_000,
    }
}
fn config(samples_per_ui: u32) -> ChannelConfig {
    ChannelConfig {
        grid: grid(samples_per_ui),
        endpoints: vec!["remote".into(), "receiver".into()],
        paths: vec![
            vec![None, None],
            vec![
                Some(Path {
                    delay_samples: 0,
                    taps_q30: vec![Q30],
                }),
                None,
            ],
        ],
        noise_rms_uv: vec![0, 0],
        noise_seed: 0x1234,
        clip_uv: 10_000_000,
    }
}
fn symbols(n: usize, seed: u64) -> Vec<i8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((x >> 32) % 3) as i8 - 1
        })
        .collect()
}
fn render(config: TransmitterConfig, symbols: &[i8], chunk: usize) -> Vec<i64> {
    let mut tx = Transmitter::new(config).unwrap();
    tx.push_symbols(symbols).unwrap();
    let mut out: Vec<i64> = vec![];
    loop {
        let part = tx.render(chunk).unwrap();
        if part.is_empty() {
            break;
        }
        out.extend(part);
    }
    out
}
#[test]
fn rational_time_keeps_subpicosecond_samples_and_absolute_phase() {
    let g = grid(32);
    let a = g.time(1).unwrap();
    let b = g.time(1_000_001).unwrap();
    assert_eq!(a.numerator_ps, 1_000_000_000_000);
    assert_eq!(a.denominator, 24_000_000_000);
    assert_eq!(b.numerator_ps, a.numerator_ps * 1_000_001);
    assert_eq!(b.denominator, a.denominator);
    assert!(
        SampleGrid {
            symbol_rate_hz: 0,
            samples_per_ui: 32
        }
        .time(1)
        .is_err()
    );
}
#[test]
fn matrix_propagation_echo_and_clipping_match_independent_full_convolution() {
    let mut c = config(32);
    c.paths[1][0] = Some(Path {
        delay_samples: 5,
        taps_q30: vec![Q30 / 2, Q30 / 4, -Q30 / 8],
    });
    c.paths[1][1] = Some(Path {
        delay_samples: 2,
        taps_q30: vec![Q30 / 3, Q30 / 5],
    });
    c.clip_uv = 700_000;
    let input = SampleBlock {
        grid: c.grid,
        start_index: 0,
        voltages: vec![
            (0..200)
                .map(|i| if i % 3 == 0 { 1_000_000 } else { -500_000 })
                .collect(),
            (0..200)
                .map(|i| if i % 5 == 0 { -800_000 } else { 900_000 })
                .collect(),
        ],
    };
    let mut channel = MatrixChannel::new(c.clone()).unwrap();
    let result = channel.process(&input).unwrap();
    let mut clipped = 0;
    for index in 0..input.len() {
        let mut independent = 0f64;
        for tx in 0..2 {
            if let Some(path) = &c.paths[1][tx] {
                for (tap, &gain) in path.taps_q30.iter().enumerate() {
                    if let Some(source) = index.checked_sub(path.delay_samples + tap) {
                        independent += input.voltages[tx][source] as f64 * gain as f64 / Q30 as f64;
                    }
                }
            }
        }
        let rounded = independent.round() as i64;
        clipped += u64::from(rounded.abs() > c.clip_uv);
        assert_eq!(
            result.voltages[1][index],
            rounded.clamp(-c.clip_uv, c.clip_uv)
        );
    }
    assert_eq!(channel.clipped_samples()[1], clipped);
    assert_eq!(channel.next_index(), 200);
    assert_eq!(channel.config(), &c);
    // A delayed impulse checks causal onset, DC sum and the literal tap shape.
    let mut impulse = vec![0; 40];
    impulse[0] = 1_000_000;
    let mut channel = MatrixChannel::new(c.clone()).unwrap();
    let wave = channel
        .process(&SampleBlock {
            grid: c.grid,
            start_index: 0,
            voltages: vec![impulse, vec![0; 40]],
        })
        .unwrap();
    assert!(wave.voltages[1][..5].iter().all(|&v| v == 0));
    assert_eq!(&wave.voltages[1][5..8], &[500000, 250000, -125000]);
}
#[test]
fn endpoint_permutation_cannot_change_summed_voltage_clipping_or_noise() {
    let mut c = config(32);
    c.paths[0][0] = Some(Path {
        delay_samples: 0,
        taps_q30: vec![Q30 / 2],
    });
    c.paths[0][1] = Some(Path {
        delay_samples: 1,
        taps_q30: vec![Q30 / 3],
    });
    c.noise_rms_uv = vec![100_000, 200_000];
    let input = SampleBlock {
        grid: c.grid,
        start_index: 0,
        voltages: vec![vec![700000; 400], vec![-500000; 400]],
    };
    let result = MatrixChannel::new(c.clone())
        .unwrap()
        .process(&input)
        .unwrap();
    let mut swapped = c.clone();
    swapped.endpoints.reverse();
    swapped.paths.reverse();
    for row in &mut swapped.paths {
        row.reverse();
    }
    swapped.noise_rms_uv.reverse();
    let mut wave = input.clone();
    wave.voltages.reverse();
    let mut permuted = MatrixChannel::new(swapped).unwrap().process(&wave).unwrap();
    permuted.voltages.reverse();
    assert_eq!(result.voltages, permuted.voltages);
}
#[test]
fn streamed_noise_tx_channel_and_receiver_restart_are_exact_across_arbitrary_chunks() {
    let symbols = symbols(320, 17);
    let txc = tx_config(32);
    let mut tx = Transmitter::new(txc.clone()).unwrap();
    tx.push_symbols(&symbols).unwrap();
    let first = tx.render(117).unwrap();
    let bytes = tx.checkpoint().unwrap();
    let mut restored = Transmitter::restore(&txc, &bytes).unwrap();
    assert_eq!(tx.next_index(), 117);
    assert_eq!(tx.render(1000).unwrap(), restored.render(1000).unwrap());
    let full = render(txc, &symbols, 16384);
    assert_eq!(first, &full[..117]);
    let mut c = config(32);
    c.noise_rms_uv[1] = 10000;
    let mut channel = MatrixChannel::new(c.clone()).unwrap();
    let expected = channel
        .process(&SampleBlock {
            grid: c.grid,
            start_index: 0,
            voltages: vec![full.clone(), vec![0; full.len()]],
        })
        .unwrap();
    let mut channel = MatrixChannel::new(c.clone()).unwrap();
    let rxc = rx_config(32);
    let mut receiver = Receiver::new(rxc.clone()).unwrap();
    receiver.queue_training(&symbols[..192]).unwrap();
    let mut result: Vec<i64> = vec![];
    let mut decisions = vec![];
    let mut at = 0;
    for chunk in [1usize, 31, 59, 7, 1024, 319, 89].into_iter().cycle() {
        if at == full.len() {
            break;
        }
        let end = (at + chunk).min(full.len());
        let block = channel
            .process(&SampleBlock {
                grid: c.grid,
                start_index: at as u64,
                voltages: vec![full[at..end].to_vec(), vec![0; end - at]],
            })
            .unwrap();
        result.extend(&block.voltages[1]);
        decisions.extend(
            receiver
                .process(at as u64, &block.voltages[1], &vec![0; end - at])
                .unwrap(),
        );
        at = end;
        let saved = channel.checkpoint().unwrap();
        channel = MatrixChannel::restore(&c, &saved).unwrap();
        let saved = receiver.checkpoint().unwrap();
        receiver = Receiver::restore(&rxc, &saved).unwrap();
    }
    assert_eq!(result, expected.voltages[1]);
    let mut direct = Receiver::new(rxc).unwrap();
    direct.queue_training(&symbols[..192]).unwrap();
    assert_eq!(
        decisions,
        direct
            .process(0, &expected.voltages[1], &vec![0; full.len()])
            .unwrap()
    );
    assert_eq!(receiver.next_index(), full.len() as u64);
}
#[test]
fn actual_training_timing_equalization_and_local_echo_determine_decoded_symbols() {
    let remote = symbols(512, 2);
    let local = symbols(512, 987);
    let remote_wave = render(tx_config(32), &remote, 101);
    let local_wave = render(tx_config(32), &local, 333);
    let mut c = config(32);
    c.paths[1][0] = Some(Path {
        delay_samples: 3,
        taps_q30: vec![Q30 * 3 / 4],
    });
    c.paths[1][1] = Some(Path {
        delay_samples: 0,
        taps_q30: vec![Q30 / 2],
    });
    let channel = MatrixChannel::new(c.clone())
        .unwrap()
        .process(&SampleBlock {
            grid: c.grid,
            start_index: 0,
            voltages: vec![remote_wave, local_wave.clone()],
        })
        .unwrap();
    let mut rx = Receiver::new(rx_config(32)).unwrap();
    rx.queue_training(&remote[..384]).unwrap();
    let decisions = rx.process(0, &channel.voltages[1], &local_wave).unwrap();
    assert_eq!(rx.acquisition(), Acquisition::Locked);
    assert!(rx.selected_phase() > 3);
    assert!(rx.coefficients().1.iter().any(|&tap| tap.abs() > Q30 / 10));
    assert!(
        decisions[384..]
            .iter()
            .zip(&remote[384..])
            .all(|(actual, &expected)| actual.symbol == expected)
    );
    let mut broken = Receiver::new(rx_config(32)).unwrap();
    broken.queue_training(&remote[..384]).unwrap();
    broken
        .process(0, &vec![0; remote.len() * 32], &local_wave)
        .unwrap();
    assert_eq!(
        broken.acquisition(),
        Acquisition::Training,
        "elapsed training alone cannot grant lock"
    );
}
#[test]
fn noiseless_32_and_64_ui_sampling_agree_on_waveforms_decisions_and_lock_transition() {
    let symbols = symbols(400, 51);
    let mut waveforms = vec![];
    let mut decisions = vec![];
    for rate in [32, 64] {
        let wave = render(tx_config(rate), &symbols, 997);
        let mut c = config(rate);
        c.paths[1][0] = Some(Path {
            delay_samples: rate as usize / 8,
            taps_q30: vec![Q30],
        });
        let output = MatrixChannel::new(c.clone())
            .unwrap()
            .process(&SampleBlock {
                grid: c.grid,
                start_index: 0,
                voltages: vec![wave.clone(), vec![0; wave.len()]],
            });
        // Production batches remain bounded even for long traces.
        let actual = if let Ok(block) = output {
            block.voltages[1].clone()
        } else {
            let mut channel = MatrixChannel::new(c).unwrap();
            let mut out: Vec<i64> = vec![];
            for (block_index, part) in wave.chunks(4096).enumerate() {
                out.extend(
                    channel
                        .process(&SampleBlock {
                            grid: grid(rate),
                            start_index: (block_index * 4096) as u64,
                            voltages: vec![part.to_vec(), vec![0; part.len()]],
                        })
                        .unwrap()
                        .voltages[1]
                        .iter(),
                );
            }
            out
        };
        let mut rxc = rx_config(rate);
        rxc.max_tracking_ppm = 5000;
        let mut rx = Receiver::new(rxc).unwrap();
        rx.queue_training(&symbols[..192]).unwrap();
        let mut decoded = vec![];
        for (block_index, part) in actual.chunks(4096).enumerate() {
            decoded.extend(
                rx.process((block_index * 4096) as u64, part, &vec![0; part.len()])
                    .unwrap(),
            );
        }
        waveforms.push(actual);
        decisions.push(decoded);
    }
    let rms = (waveforms[0]
        .iter()
        .zip(waveforms[1].iter().step_by(2))
        .map(|(&a, &b)| (a - b) as f64 * ((a - b) as f64))
        .sum::<f64>()
        / waveforms[0].len() as f64)
        .sqrt();
    assert!(rms <= 10_000.0, "RMS={rms}µV");
    assert_eq!(
        decisions[0].iter().map(|d| d.symbol).collect::<Vec<_>>(),
        decisions[1].iter().map(|d| d.symbol).collect::<Vec<_>>()
    );
    let lock32 = decisions[0]
        .iter()
        .find(|d| d.acquisition == Acquisition::Locked)
        .unwrap();
    let lock64 = decisions[1]
        .iter()
        .find(|d| d.acquisition == Acquisition::Locked)
        .unwrap();
    assert_eq!(lock32.ordinal, lock64.ordinal);
    assert!(lock32.sample_index.abs_diff(lock64.sample_index / 2) <= 1);
}

#[test]
fn seeded_noisy_bit_trials_match_independent_high_resolution_reference_confidence() {
    // Binary data uses the -1/+1 subset of this project PAM3 numerical contract.
    // A zero symbol is decoded as bit0. Reference threshold is thus +0.5peak.
    let bits: Vec<_> = symbols(5128, 20261021)
        .iter()
        .map(|&s| if s < 0 { -1 } else { 1 })
        .collect();
    for noise_uv in [100_000u32, 200_000, 300_000] {
        let mut estimates = vec![];
        for rate in [32, 64] {
            let mut txc = tx_config(rate);
            txc.transition_ui_ppm = 0;
            let wave = render(txc, &bits, 997);
            let mut c = config(rate);
            c.noise_seed = 0x4512 + noise_uv as u64;
            c.noise_rms_uv[1] = noise_uv;
            let mut channel = MatrixChannel::new(c).unwrap();
            let mut rxc = rx_config(rate);
            rxc.ffe_taps = 1;
            rxc.echo_taps = 0;
            rxc.adaptation_q30 = 0;
            rxc.lock_error_ppm = 500_000;
            let mut receiver = Receiver::new(rxc).unwrap();
            receiver.queue_training(&bits[..128]).unwrap();
            let mut decoded = vec![];
            let mut reference = vec![];
            for (block, part) in wave.chunks(4096).enumerate() {
                let start = block * 4096;
                let actual = channel
                    .process(&SampleBlock {
                        grid: grid(rate),
                        start_index: start as u64,
                        voltages: vec![part.to_vec(), vec![0; part.len()]],
                    })
                    .unwrap();
                for (offset, &sample) in actual.voltages[1].iter().enumerate() {
                    let absolute = start + offset;
                    if absolute % (rate as usize) == rate as usize / 2 {
                        reference.push(sample > 500_000);
                    }
                }
                decoded.extend(
                    receiver
                        .process(start as u64, &actual.voltages[1], &vec![0; part.len()])
                        .unwrap(),
                );
            }
            assert_eq!(receiver.acquisition(), Acquisition::Locked);
            assert_eq!(receiver.clipped_samples(), 0);
            assert_eq!(receiver.trained_symbols(), 128);
            let errors = if rate == 32 {
                decoded[128..]
                    .iter()
                    .zip(&bits[128..])
                    .filter(|(d, b)| (d.symbol > 0) != (**b > 0))
                    .count()
            } else {
                reference[128..]
                    .iter()
                    .zip(&bits[128..])
                    .filter(|(d, b)| **d != (**b > 0))
                    .count()
            };
            let interval = ber_confidence(errors as u64, 5000).unwrap();
            estimates.push(interval);
            println!(
                "noise_rms_uv={noise_uv} samples_per_ui={rate} actual_errors={errors} trials=5000 wilson95={interval:?}"
            );
        }
        // Independent64-sample threshold receiver and production32-sample
        // trained receiver must have confidence-compatible measured BER.
        assert!(
            estimates[0].0 <= estimates[1].1 && estimates[1].0 <= estimates[0].1,
            "noise={noise_uv}: {estimates:?}"
        );
    }
    let zero = ber_confidence(0, 5000).unwrap();
    assert!(zero.1 > 0.0);
    assert!(ber_confidence(1, 0).is_err());
}
#[test]
fn inconsistent_noncausal_unbounded_and_tampered_saved_inputs_are_rejected_atomically() {
    let mut c = config(32);
    c.paths[1][0].as_mut().unwrap().delay_samples = 262145;
    assert!(MatrixChannel::new(c).is_err());
    let mut c = config(32);
    c.paths[1][0].as_mut().unwrap().taps_q30 = vec![17 * Q30];
    assert!(MatrixChannel::new(c).is_err());
    let c = config(32);
    let mut channel = MatrixChannel::new(c.clone()).unwrap();
    let saved = channel.checkpoint().unwrap();
    let block = SampleBlock {
        grid: c.grid,
        start_index: 1,
        voltages: vec![vec![0], vec![0]],
    };
    assert!(!block.is_empty());
    assert!(channel.process(&block).is_err());
    assert_eq!(channel.checkpoint().unwrap(), saved);
    let mut raw: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    raw["history"][0] = serde_json::json!([1]);
    assert!(MatrixChannel::restore(&c, &serde_json::to_vec(&raw).unwrap()).is_err());
    let txc = tx_config(32);
    let mut tx = Transmitter::new(txc.clone()).unwrap();
    tx.push_symbols(&[1, -1]).unwrap();
    tx.render(1).unwrap();
    let saved = tx.checkpoint().unwrap();
    let mut raw: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    raw["phase"] = serde_json::json!(32_000_000);
    assert!(Transmitter::restore(&txc, &serde_json::to_vec(&raw).unwrap()).is_err());
    let rxc = rx_config(32);
    let rx = Receiver::new(rxc.clone()).unwrap();
    let saved = rx.checkpoint().unwrap();
    let mut raw: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    raw["selected_phase"] = serde_json::json!(32);
    assert!(Receiver::restore(&rxc, &serde_json::to_vec(&raw).unwrap()).is_err());
    let mut changed = rxc;
    changed.adc_bits = 12;
    assert!(Receiver::restore(&changed, &saved).is_err());
}

#[test]
fn integer_clock_offsets_preserve_fractional_phase_across_batches_and_training() {
    let symbols = symbols(2000, 1789);
    for ppm in [-1000, 37, 1000] {
        let mut txc = tx_config(32);
        txc.clock_ppm = ppm;
        let expected_len =
            (symbols.len() as u64 * 32 * 1_000_000).div_ceil((1_000_000i64 + ppm as i64) as u64);
        let wave = render(txc.clone(), &symbols, 313);
        assert_eq!(wave.len() as u64, expected_len);
        let mut tx = Transmitter::new(txc.clone()).unwrap();
        tx.push_symbols(&symbols).unwrap();
        let mut resumed = vec![];
        loop {
            let part = tx.render(79).unwrap();
            if part.is_empty() {
                break;
            }
            resumed.extend(part);
            tx = Transmitter::restore(&txc, &tx.checkpoint().unwrap()).unwrap();
        }
        assert_eq!(resumed, wave);
        let mut rxc = rx_config(32);
        rxc.clock_ppm = ppm;
        let mut rx = Receiver::new(rxc.clone()).unwrap();
        rx.queue_training(&symbols[..384]).unwrap();
        let mut decoded = vec![];
        for (block, part) in wave.chunks(997).enumerate() {
            decoded.extend(
                rx.process((block * 997) as u64, part, &vec![0; part.len()])
                    .unwrap(),
            );
            rx = Receiver::restore(&rxc, &rx.checkpoint().unwrap()).unwrap();
        }
        assert_eq!(rx.acquisition(), Acquisition::Locked);
        assert_eq!(decoded.len(), symbols.len());
        assert!(
            decoded[384..]
                .iter()
                .zip(&symbols[384..])
                .all(|(actual, &expected)| actual.symbol == expected)
        );
    }
}

#[test]
fn unknown_partner_clock_drift_is_recovered_from_waveform_without_tx_clock_configuration() {
    let source = symbols(9000, 412);
    for offset in [-2000, -500, 500, 2000] {
        let mut txc = tx_config(32);
        txc.clock_ppm = offset;
        let waveform = render(txc, &source, 997);
        let mut rxc = rx_config(32);
        rxc.max_tracking_ppm = 5000;
        // RX knows only its own nominal oscillator, never the TX offset.
        assert_eq!(rxc.clock_ppm, 0);
        let mut receiver = Receiver::new(rxc.clone()).unwrap();
        receiver.queue_training(&source[..512]).unwrap();
        let mut actual = vec![];
        for (block, part) in waveform.chunks(997).enumerate() {
            actual.extend(
                receiver
                    .process((block * 997) as u64, part, &vec![0; part.len()])
                    .unwrap(),
            );
            receiver = Receiver::restore(&rxc, &receiver.checkpoint().unwrap()).unwrap();
        }
        let debug: serde_json::Value =
            serde_json::from_slice(&receiver.checkpoint().unwrap()).unwrap();
        let errors = actual
            .iter()
            .filter(|decision| decision.ordinal >= 512 && decision.ordinal < 8704)
            .filter(|decision| decision.symbol != source[decision.ordinal as usize])
            .count();
        println!(
            "offset={offset} estimate={} phase={} selected={} trained={} ordinal={} errors={errors}",
            receiver.estimated_partner_offset_ppm(),
            debug["phase"],
            debug["selected_phase"],
            debug["trained"],
            debug["ordinal"]
        );
        assert_eq!(
            receiver.acquisition(),
            Acquisition::Locked,
            "offset={offset}, estimated={}",
            receiver.estimated_partner_offset_ppm()
        );
        let errors = actual
            .iter()
            .filter(|decision| decision.ordinal >= 512 && decision.ordinal < 8704)
            .filter(|decision| decision.symbol != source[decision.ordinal as usize])
            .count();
        assert_eq!(
            errors,
            0,
            "offset={offset}, estimated={}",
            receiver.estimated_partner_offset_ppm()
        );
        assert!(
            receiver
                .estimated_partner_offset_ppm()
                .abs_diff(offset as i64)
                < 500,
            "offset={offset}, estimate={}",
            receiver.estimated_partner_offset_ppm()
        );
    }
}
#[test]
fn observable_loss_aborts_current_frame_and_retraining_cannot_complete_a_stale_epoch() {
    let source = symbols(512, 351);
    let wave = render(tx_config(32), &source, 997);
    let mut c = rx_config(32);
    c.max_tracking_ppm = 5000;
    c.max_no_transition_symbols = 64;
    let mut receiver = Receiver::new(c.clone()).unwrap();
    receiver.queue_training(&source[..384]).unwrap();
    receiver.process(0, &wave, &vec![0; wave.len()]).unwrap();
    assert_eq!(receiver.acquisition(), Acquisition::Locked);
    let token = receiver.begin_frame("frame-before-loss").unwrap();
    let start = receiver.next_index();
    receiver
        .process(start, &vec![0; 4096], &vec![0; 4096])
        .unwrap();
    assert!(matches!(
        receiver.acquisition(),
        Acquisition::Lost(LossReason::TransitionTimeout)
    ));
    assert!(receiver.finish_frame(&token).is_err());
    let events = receiver.take_events();
    assert!(events.iter().any(
        |event| matches!(event,ReceiverEvent::FrameAborted{token:aborted,..}if aborted==&token)
    ));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ReceiverEvent::FrameCompleted { .. }))
    );
    receiver = Receiver::restore(&c, &receiver.checkpoint().unwrap()).unwrap();
    receiver.start_training(&source[..384]).unwrap();
    let begin = receiver.next_index();
    receiver
        .process(begin, &wave, &vec![0; wave.len()])
        .unwrap();
    assert_eq!(receiver.acquisition(), Acquisition::Locked);
    assert!(receiver.finish_frame(&token).is_err());
    let current = receiver.begin_frame("frame-after-retraining").unwrap();
    assert!(current.epoch > token.epoch);
    receiver.finish_frame(&current).unwrap();
    let events = receiver.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ReceiverEvent::TrainingStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event,ReceiverEvent::FrameCompleted{token,..}if token==&current))
    );
}
#[test]
fn unsupported_drift_range_is_observed_as_loss_and_decision_boundaries_split_batches() {
    let source = symbols(2048, 54);
    let mut txc = tx_config(32);
    txc.clock_ppm = 10000;
    let waveform = render(txc, &source, 997);
    let mut c = rx_config(32);
    c.max_tracking_ppm = 1000;
    let mut receiver = Receiver::new(c).unwrap();
    receiver.queue_training(&source[..1024]).unwrap();
    let mut at = 0;
    while at < waveform.len() {
        let end = (at + receiver.samples_until_decision()).min(waveform.len());
        receiver
            .process(at as u64, &waveform[at..end], &vec![0; end - at])
            .unwrap();
        at = end;
        if matches!(receiver.acquisition(), Acquisition::Lost(_)) {
            break;
        }
    }
    assert!(
        matches!(
            receiver.acquisition(),
            Acquisition::Lost(LossReason::TimingRange | LossReason::DecisionError)
        ),
        "{:?}, estimated={}",
        receiver.acquisition(),
        receiver.estimated_partner_offset_ppm()
    );
    assert!(receiver.begin_frame("out-of-range").is_err());
}

#[test]
fn tracking_requires_even_grid_and_all_state_events_commit_at_consumed_sample_boundaries() {
    let mut odd = rx_config(9);
    odd.max_tracking_ppm = 1000;
    assert!(Receiver::new(odd).is_err());
    let source = symbols(512, 119);
    let waveform = render(tx_config(32), &source, 997);
    let mut config = rx_config(32);
    config.max_tracking_ppm = 5000;
    config.max_no_transition_symbols = 16;
    let mut rx = Receiver::new(config).unwrap();
    rx.queue_training(&source[..384]).unwrap();
    let mut at = 0;
    while at < waveform.len() {
        let end = (at + rx.samples_until_decision()).min(waveform.len());
        rx.process(at as u64, &waveform[at..end], &vec![0; end - at])
            .unwrap();
        at = end;
        for event in rx.take_events() {
            if let ReceiverEvent::LockAcquired { sample_index, .. } = event {
                assert_eq!(sample_index, rx.next_index());
            }
        }
    }
    assert_eq!(rx.acquisition(), Acquisition::Locked);
    rx.begin_frame("timed-abort").unwrap();
    let mut loss = false;
    for _ in 0..100 {
        let count = rx.samples_until_decision();
        rx.process(rx.next_index(), &vec![0; count], &vec![0; count])
            .unwrap();
        for event in rx.take_events() {
            match event {
                ReceiverEvent::FrameAborted { sample_index, .. }
                | ReceiverEvent::LockLost { sample_index, .. } => {
                    assert_eq!(sample_index, rx.next_index());
                    loss = true;
                }
                _ => {}
            }
        }
        if loss {
            break;
        }
    }
    assert!(loss);
}

#[test]
fn repeated_observed_timing_saturation_has_its_own_loss_reason() {
    let source: Vec<_> = (0..1024).map(|i| if i % 2 == 0 { -1 } else { 1 }).collect();
    let mut txc = tx_config(32);
    txc.clock_ppm = 2000;
    let wave = render(txc, &source, 997);
    let mut config = rx_config(32);
    config.max_tracking_ppm = 100;
    config.lock_error_ppm = 500_000;
    config.minimum_training_symbols = 16;
    let mut rx = Receiver::new(config).unwrap();
    rx.queue_training(&source).unwrap();
    let mut at = 0;
    while at < wave.len() {
        let end = (at + rx.samples_until_decision()).min(wave.len());
        rx.process(at as u64, &wave[at..end], &vec![0; end - at])
            .unwrap();
        at = end;
        if matches!(rx.acquisition(), Acquisition::Lost(_)) {
            break;
        }
    }
    assert!(
        matches!(rx.acquisition(), Acquisition::Lost(LossReason::TimingRange)),
        "{:?}, estimated={}",
        rx.acquisition(),
        rx.estimated_partner_offset_ppm()
    );
}
#[test]
fn unknown_drift_tracking_survives_causal_fir_reflection_echo_and_seeded_noise() {
    let remote = symbols(9248, 441);
    let local = symbols(9400, 991);
    for ppm in [-1000, 1000] {
        let mut txc = tx_config(32);
        txc.clock_ppm = ppm;
        let remote_wave = render(txc, &remote, 997);
        let mut local_wave = render(tx_config(32), &local, 997);
        local_wave.truncate(remote_wave.len());
        let mut channel_config = config(32);
        let mut taps = vec![0; 33];
        taps[0] = Q30 * 4 / 5;
        taps[32] = Q30 / 10;
        channel_config.paths[1][0] = Some(Path {
            delay_samples: 2,
            taps_q30: taps,
        });
        channel_config.paths[1][1] = Some(Path {
            delay_samples: 3,
            taps_q30: vec![Q30 / 4],
        });
        channel_config.noise_rms_uv[1] = 20_000;
        let mut channel = MatrixChannel::new(channel_config.clone()).unwrap();
        let mut rx_config = rx_config(32);
        rx_config.max_tracking_ppm = 5000;
        let mut rx = Receiver::new(rx_config.clone()).unwrap();
        rx.queue_training(&remote[..1024]).unwrap();
        let mut decoded = vec![];
        for start in (0..remote_wave.len()).step_by(997) {
            let end = (start + 997).min(remote_wave.len());
            let wave = channel
                .process(&SampleBlock {
                    grid: grid(32),
                    start_index: start as u64,
                    voltages: vec![
                        remote_wave[start..end].to_vec(),
                        local_wave[start..end].to_vec(),
                    ],
                })
                .unwrap();
            decoded.extend(
                rx.process(start as u64, &wave.voltages[1], &local_wave[start..end])
                    .unwrap(),
            );
            rx = Receiver::restore(&rx_config, &rx.checkpoint().unwrap()).unwrap();
            channel =
                MatrixChannel::restore(&channel_config, &channel.checkpoint().unwrap()).unwrap();
        }
        let state: serde_json::Value = serde_json::from_slice(&rx.checkpoint().unwrap()).unwrap();
        let selected = state["selected_phase"].as_u64().unwrap() as usize;
        let squared: Vec<_> = state["candidates"][selected]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as f64)
            .collect();
        println!(
            "combinedppm={ppm}, trained={}, ordinal={}, RMS={}, events={:?}",
            rx.trained_symbols(),
            state["ordinal"],
            (squared.iter().sum::<f64>() / squared.len() as f64).sqrt(),
            rx.take_events()
        );
        assert_eq!(
            rx.acquisition(),
            Acquisition::Locked,
            "ppm={ppm}, estimate={}",
            rx.estimated_partner_offset_ppm()
        );
        let errors = decoded
            .iter()
            .filter(|d| d.ordinal >= 1024 && d.ordinal < 9216)
            .filter(|d| d.symbol != remote[d.ordinal as usize])
            .count();
        assert_eq!(
            errors,
            0,
            "ppm={ppm}, estimate={}",
            rx.estimated_partner_offset_ppm()
        );
    }
}
