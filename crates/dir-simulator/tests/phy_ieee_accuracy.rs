//! Bounded post-training 1000BASE-T1 data-phase numerical envelope.
//! The full sampled Clause 97/98 startup is exercised separately in
//! `phy_ieee_gbt1`; this gate starts with an actual labelled acquisition
//! waveform and then uses the production PCS, PMA, channel, and ADC paths.
use dir_simulator::runtime::ethernet::{
    crc32,
    phy::{
        Q30, SampleBlock, SampleGrid,
        channel::{ChannelConfig, MatrixChannel, Path, Transmitter, TransmitterConfig},
        ieee::{t1_gigabit::Role, t1_stream},
        receiver::{Acquisition, Receiver, ReceiverConfig},
    },
};

const TRAINING_SYMBOLS: usize = 128;
const PEAK_UV: i64 = 1_000_000;

struct Observation {
    ui: usize,
    tx: Vec<i64>,
    rx: Vec<i64>,
    decision_symbols: Vec<i8>,
    data_decision_samples: Vec<u64>,
    received: Vec<t1_stream::ReceivedMac>,
    range: t1_stream::SourceRange,
    clipped: u64,
}

fn mac() -> Vec<u8> {
    let mut bytes = vec![0xff; 6];
    bytes.extend([0x02, 0, 0, 0, 0, 1]);
    bytes.extend([0x08, 0x00]);
    bytes.extend((0..46).map(|n| (n * 37) as u8));
    bytes.extend(crc32(&bytes).to_le_bytes());
    bytes
}

fn run(ui: usize, attenuation_q30: i64, noise_uv: u32) -> Observation {
    let grid = SampleGrid {
        symbol_rate_hz: 750_000_000,
        samples_per_ui: ui as u32,
    };
    let tx_config = TransmitterConfig {
        grid,
        peak_uv: PEAK_UV,
        transition_ui_ppm: 250_000,
        clock_ppm: 0,
        // The declared cross-grid TX envelope uses a UI-scaled transition
        // and a unity output tap.  The sample-spaced analogue filter is in
        // the receiver front-end below; arbitrary TX FIR taps are outside
        // this cross-grid equivalence claim.
        filter_taps_q30: vec![Q30],
    };
    let rx_config = ReceiverConfig {
        grid,
        peak_uv: PEAK_UV,
        frontend_taps_q30: vec![3 * Q30 / 4, Q30 / 4],
        adc_bits: 12,
        adc_full_scale_uv: 2_000_000,
        ffe_taps: 1,
        echo_taps: 0,
        adaptation_q30: 0,
        minimum_training_symbols: 64,
        lock_error_ppm: 300_000,
        clock_ppm: 0,
        max_tracking_ppm: 0,
        max_no_transition_symbols: 4096,
        loss_error_ppm: 500_000,
    };
    let channel_config = ChannelConfig {
        grid,
        endpoints: vec!["source".into(), "receiver".into()],
        paths: vec![
            vec![None, None],
            vec![
                Some(Path {
                    delay_samples: 0,
                    taps_q30: vec![attenuation_q30],
                }),
                None,
            ],
        ],
        noise_rms_uv: vec![0, noise_uv],
        noise_seed: 0x1973_2022,
        clip_uv: 2_000_000,
    };
    let mut tx = Transmitter::new(tx_config).unwrap();
    let mut channel = MatrixChannel::new(channel_config).unwrap();
    let mut rx = Receiver::new(rx_config).unwrap();
    let mut pcs_tx = t1_stream::TransmitStream::new(Role::Master, 0x2255).unwrap();
    let mut pcs_rx = t1_stream::ReceiveStream::new(Role::Master, 0x2255, 1518).unwrap();
    let frame = mac();
    let range = pcs_tx.enqueue_mac(&frame, 1518).unwrap();
    let mut training = Vec::with_capacity(TRAINING_SYMBOLS);
    let mut state = 0x1a2b_3c4du32;
    for _ in 0..TRAINING_SYMBOLS {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        training.push(if state & 1 == 0 { -1 } else { 1 });
    }
    rx.queue_training(&training).unwrap();
    let mut symbols = training;
    symbols.extend((0..2700).map(|_| pcs_tx.next_symbol().unwrap()));
    let mut observation = Observation {
        ui,
        tx: Vec::with_capacity(symbols.len() * ui),
        rx: Vec::with_capacity(symbols.len() * ui),
        decision_symbols: vec![],
        data_decision_samples: vec![],
        received: vec![],
        range,
        clipped: 0,
    };
    for segment in symbols.chunks(128) {
        tx.push_symbols(segment).unwrap();
        let rendered = tx.render(segment.len() * ui).unwrap();
        assert_eq!(rendered.len(), segment.len() * ui);
        let start_index = channel.next_index();
        let block = channel
            .process(&SampleBlock {
                grid,
                start_index,
                voltages: vec![rendered.clone(), vec![0; rendered.len()]],
            })
            .unwrap();
        let received_wave = &block.voltages[1];
        let decisions = rx
            .process(start_index, received_wave, &vec![0; received_wave.len()])
            .unwrap();
        observation.tx.extend(rendered);
        observation.rx.extend(received_wave.iter().copied());
        for decision in decisions {
            if decision.ordinal < TRAINING_SYMBOLS as u64 {
                continue;
            }
            assert_eq!(decision.acquisition, Acquisition::Locked);
            observation.decision_symbols.push(decision.symbol);
            observation
                .data_decision_samples
                .push(decision.sample_index);
            observation
                .received
                .extend(pcs_rx.push_symbol(decision.symbol).unwrap());
        }
    }
    observation.clipped = rx.clipped_samples();
    assert_eq!(observation.decision_symbols.len(), 2700);
    assert_eq!(observation.received.len(), 1);
    assert_eq!(observation.received[0].bytes, frame);
    assert_eq!(
        observation.received[0].sfd_end_physical_symbol,
        range.sfd_end_physical_symbol
    );
    assert_eq!(
        observation.received[0].fcs_end_physical_symbol,
        range.fcs_end_physical_symbol
    );
    observation
}

#[test]
fn gbt1_post_training_analog_accuracy_matrix_16_32_64_ui() {
    // Detect an actual channel-voltage transition close to each MAC cut.
    // Receiver decision samples are interior acquisition phases, not edge
    // timestamps; comparing those phases would misstate analog edge error.
    let edge_offset = |run: &Observation, cut: u64| -> (usize, i64) {
        let j = (cut as usize - 8..cut as usize + 8)
            .find(|&j| j > 0 && run.decision_symbols[j - 1] != run.decision_symbols[j])
            .expect("no PAM3 transition near MAC cut");
        let left = run.decision_symbols[j - 1] as i64;
        let right = run.decision_symbols[j] as i64;
        let threshold = (left + right) * (85 * PEAK_UV / 100) / 2;
        let boundary = (TRAINING_SYMBOLS + j) * run.ui;
        let crossing = (boundary..boundary + run.ui)
            .find(|&sample| {
                if right > left {
                    run.rx[sample] >= threshold
                } else {
                    run.rx[sample] <= threshold
                }
            })
            .expect("channel did not resolve PAM3 edge");
        (j, crossing as i64 - boundary as i64)
    };
    let runs = [16, 32, 64].map(|ui| run(ui, 85 * Q30 / 100, 1_000));
    let repeat32 = run(32, 85 * Q30 / 100, 1_000);
    assert_eq!(runs[1].rx, repeat32.rx, "seeded channel must replay");
    assert_eq!(
        runs[1].data_decision_samples,
        repeat32.data_decision_samples
    );
    for run in &runs {
        assert_eq!(run.clipped, 0);
        assert_eq!(run.received[0].decoded_at_symbol, 2700);
        assert_eq!(run.range.sfd_end_physical_symbol, 44);
        assert_eq!(run.range.fcs_end_physical_symbol, 394);
    }
    for (coarse, ratio) in [(&runs[0], 4usize), (&runs[1], 2usize)] {
        let fine = &runs[2];
        assert_eq!(coarse.tx.len() * ratio, fine.tx.len());
        let peak_sq = (PEAK_UV as f64).powi(2);
        let normalized_tx_rms = (coarse
            .tx
            .iter()
            .zip(fine.tx.iter().step_by(ratio))
            .map(|(&a, &b)| ((a - b) as f64).powi(2))
            .sum::<f64>()
            / coarse.tx.len() as f64
            / peak_sq)
            .sqrt();
        let normalized_rx_rms = (coarse
            .rx
            .iter()
            .zip(fine.rx.iter().step_by(ratio))
            .map(|(&a, &b)| ((a - b) as f64).powi(2))
            .sum::<f64>()
            / coarse.rx.len() as f64
            / peak_sq)
            .sqrt();
        assert!(normalized_tx_rms <= 0.01, "TX RMS={normalized_tx_rms}");
        assert!(normalized_rx_rms <= 0.01, "RX RMS={normalized_rx_rms}");
        assert_eq!(coarse.decision_symbols, fine.decision_symbols);
        assert_eq!(coarse.received, fine.received);
        let mut edge_deltas_64 = vec![];
        for cut in [
            coarse.range.sfd_end_physical_symbol,
            coarse.range.fcs_end_physical_symbol,
        ] {
            let (coarse_symbol, coarse_offset) = edge_offset(coarse, cut);
            let (fine_symbol, fine_offset) = edge_offset(fine, cut);
            assert_eq!(coarse_symbol, fine_symbol);
            let coarse_64 = coarse_offset * ratio as i64;
            let fine_64 = fine_offset;
            edge_deltas_64.push((coarse_64 - fine_64).abs());
            assert!(
                (coarse_64 - fine_64).abs() <= 4,
                "edge timing discrepancy exceeds one 16-UI sample: coarse={coarse_64} fine={fine_64}"
            );
        }
        eprintln!(
            "1000BASE-T1 post-training ui={} tx_rms_fraction={normalized_tx_rms:.6} rx_rms_fraction={normalized_rx_rms:.6} edge_delta_64_samples={edge_deltas_64:?} decoded=1 sfd_symbol={} fcs_symbol={} clipped={}",
            coarse.ui,
            coarse.range.sfd_end_physical_symbol,
            coarse.range.fcs_end_physical_symbol,
            coarse.clipped
        );
    }
}
