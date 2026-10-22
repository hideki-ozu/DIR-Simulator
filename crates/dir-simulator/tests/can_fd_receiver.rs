use dir_simulator::runtime::canfd::{
    codec,
    receiver::{DecodedFrame, RateAction, Receiver, Sample},
};
fn feed(bits: &[u8], capable: bool) -> Vec<dir_simulator::runtime::canfd::receiver::Observation> {
    let mut receiver = Receiver::new(capable);
    let mut observations = vec![];
    for (index, &bit) in bits.iter().enumerate() {
        let observation = receiver
            .sample(Sample {
                level: bit,
                local_index: index,
                exact_num: index as u128,
                exact_den: 1,
            })
            .unwrap();
        let stop = observation.detection.is_some();
        observations.push(observation);
        receiver.validate().unwrap();
        if stop {
            break;
        }
    }
    observations
}
#[test]
fn every_fd_length_format_and_rate_is_received_from_actual_samples_with_real_ack_intent() {
    for &length in &codec::DATA_LENGTHS {
        for extended in [false, true] {
            for brs in [false, true] {
                let frame = codec::Frame {
                    id: if extended { 0x1abcde } else { 0x123 },
                    extended,
                    brs,
                    esi: false,
                    data: (0..length).map(|i| i as u8).collect(),
                };
                let wire = codec::encode(&frame).unwrap();
                let mut physical = wire.frame.clone();
                physical[wire.crc_delimiter_index + 1] = 1; // receivers do not assume the codec's acknowledged slot
                let observations = feed(&physical, true);
                assert!(
                    observations.iter().all(|o| o.detection.is_none()),
                    "{length}/{extended}/{brs}"
                );
                let acknowledgements: Vec<_> =
                    observations.iter().filter(|o| o.ack_intent).collect();
                assert_eq!(acknowledgements.len(), 1);
                assert_eq!(acknowledgements[0].sample_index, wire.crc_delimiter_index);
                let complete: Vec<_> = observations
                    .iter()
                    .filter_map(|o| o.completed_frame.as_ref())
                    .collect();
                assert_eq!(
                    complete,
                    vec![&DecodedFrame::IsoFd {
                        id: frame.id,
                        extended,
                        brs,
                        esi: false,
                        data: frame.data
                    }]
                );
                assert_eq!(
                    observations
                        .iter()
                        .find(|o| o.completed_frame.is_some())
                        .unwrap()
                        .sample_index,
                    wire.frame.len() - 2
                );
                assert!(observations.last().unwrap().frame_end);
                assert_eq!(
                    observations
                        .iter()
                        .filter(|o| o.rate_action == RateAction::EnterData)
                        .count(),
                    usize::from(brs)
                );
                assert_eq!(
                    observations
                        .iter()
                        .filter(|o| o.rate_action == RateAction::ReturnNominal)
                        .count(),
                    usize::from(brs)
                );
            }
        }
    }
}
#[test]
fn partial_prefix_is_pending_and_stuff_errors_are_detected_at_the_actual_bit() {
    let frame = codec::Frame {
        id: 0,
        extended: false,
        brs: true,
        esi: false,
        data: vec![0; 64],
    };
    let wire = codec::encode(&frame).unwrap();
    for prefix in 0..wire.frame.len() - 1 {
        let observations = feed(&wire.frame[..prefix], true);
        assert!(
            observations
                .iter()
                .all(|o| o.detection.is_none() && o.completed_frame.is_none())
        );
    }
    for (position, kind) in [
        (wire.dynamic_stuff_positions[0], "stuff"),
        (wire.fixed_stuff_positions[0], "form"),
    ] {
        let mut bits = wire.frame.clone();
        bits[position] ^= 1;
        let observations = feed(&bits, true);
        let error = observations.last().unwrap().detection.as_ref().unwrap();
        assert_eq!(error.local_index, position);
        assert_eq!(error.kind, kind);
        assert!(!error.delayed);
    }
}
#[test]
fn crc_and_count_invalidity_withhold_ack_and_signal_only_after_ack_delimiter() {
    let frame = codec::Frame {
        id: 0x123,
        extended: true,
        brs: true,
        esi: false,
        data: vec![0x5a; 12],
    };
    let wire = codec::encode(&frame).unwrap();
    for count in [false, true] {
        let mut bits = wire.frame.clone();
        if count {
            bits[wire.fixed_stuff_positions[0] + 4] ^= 1;
            for &position in &wire.fixed_stuff_positions {
                bits[position] = 1 - bits[position - 1];
            }
        } else {
            bits[wire.crc_delimiter_index - 1] ^= 1;
        }
        let observations = feed(&bits, true);
        assert!(!observations.iter().any(|o| o.ack_intent));
        let error = observations.last().unwrap().detection.as_ref().unwrap();
        assert_eq!(error.local_index, wire.crc_delimiter_index + 2);
        assert!(error.delayed);
        assert!(
            error
                .causes
                .iter()
                .any(|cause| cause == if count { "stuff_count" } else { "crc" })
        );
        bits[wire.crc_delimiter_index + 2] = 0;
        let observations = feed(&bits, true);
        let error = observations.last().unwrap().detection.as_ref().unwrap();
        assert_eq!(error.kind, "form");
        assert!(error.causes.len() >= 2);
        assert!(!error.delayed);
    }
}
#[test]
fn classical_receiver_uses_its_own_reserved_bit_and_crc_rules() {
    use dir_simulator::runtime::can_protocol::{codec as cc, types::Frame};
    for remote in [false, true] {
        for extended in [false, true] {
            let frame = Frame {
                id: if extended { 0x1abcde } else { 0x123 },
                extended,
                remote,
                dlc: 2,
                data: if remote { vec![] } else { vec![0x12, 0x34] },
            };
            let wire = cc::encode(&frame).unwrap();
            for capable in [false, true] {
                let observations = feed(&wire.bits, capable);
                assert!(observations.iter().all(|o| o.detection.is_none()));
                assert!(
                    observations
                        .iter()
                        .any(|o| matches!(o.completed_frame, Some(DecodedFrame::Cc { .. })))
                );
            }
        }
    }
    let fd = codec::encode(&codec::Frame {
        id: 0x123,
        extended: false,
        brs: false,
        esi: false,
        data: vec![0; 2],
    })
    .unwrap();
    let observations = feed(&fd.frame, false);
    let fdf = observations
        .iter()
        .find(|o| o.field == "fdf_reserved")
        .unwrap();
    assert!(
        fdf.detection.is_none(),
        "recessive CC reserved bit is not an immediate form error"
    );
    assert!(observations.iter().any(|o| o.detection.is_some()));
    assert!(
        !observations
            .iter()
            .any(|o| o.rate_action == RateAction::EnterData)
    );
}
