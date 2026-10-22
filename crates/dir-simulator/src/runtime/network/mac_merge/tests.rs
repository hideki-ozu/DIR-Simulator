use super::*;
fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn mac(body: Vec<u8>) -> Vec<u8> {
    let mut b = body;
    let f = crate::runtime::ethernet::crc32(&b).to_le_bytes();
    b.extend(f);
    b
}
fn rtag() -> Vec<u8> {
    let mut b = unhex("020000000002020000000001f1c1000001230800");
    b.extend(0u8..160);
    // Literal independent expected original FCS from normative design.
    b.extend(unhex("3c1bdbe9"));
    b
}
fn external(bytes: Vec<u8>) -> PhysicalPacket {
    PhysicalPacket {
        bytes,
        complete: true,
        provenance: RxProvenance::ExternalInjection {
            evidence_id: "physical-1".into(),
        },
    }
}
fn literal_rtag_fragments() -> Vec<Vec<u8>> {
    let b = rtag();
    [
        ("55555555555555e6", 0, 60, "96cadf02"),
        ("55555555555561e6", 60, 120, "37dc867d"),
        ("555555555555614c", 120, 180, "3c1bdbe9"),
    ]
    .into_iter()
    .map(|(header, from, to, crc)| {
        let mut out = unhex(header);
        out.extend(&b[from..to]);
        out.extend(unhex(crc));
        out
    })
    .collect()
}
fn capability(epoch: u64, add: u8) -> PeerCapabilitiesObserved {
    let mut b = unhex("0180c200000e02000000000188cc");
    // Actual received extension with support bit set, peer enabled/active false.
    b.extend([0xfe, 6, 0, 0x12, 0x0f, 7, 0, 1 | (add << 3), 0, 0]);
    b.resize(60, 0);
    let ingress = ValidatedMacIngress {
        epoch,
        received_at: 123,
        bytes: mac(b),
        provenance: RxProvenance::ExternalInjection {
            evidence_id: "lldp-packet".into(),
        },
    };
    PeerCapabilitiesObserved::from_received_lldp(2, &ingress, "lldp-evidence".into()).unwrap()
}
#[test]
fn mac_merge_independent_crc_and_control_vectors() {
    let original = mac(vec![0; 120]);
    for (frame, smd_s, smd_c) in [
        (0, 0xe6, 0x61),
        (1, 0x4c, 0x52),
        (2, 0x7f, 0x9e),
        (3, 0xb3, 0x2a),
    ] {
        let start = encode_fragment(&original, 0, 60, frame, None, true, 0).unwrap();
        assert_eq!(start[7], smd_s);
        for (frag, encoded) in [(0, 0xe6), (1, 0x4c), (2, 0x7f), (3, 0xb3)] {
            let continuation =
                encode_fragment(&original, 60, 120, frame, Some(frag), true, 0).unwrap();
            assert_eq!(
                &continuation[..8],
                &[0x55, 0x55, 0x55, 0x55, 0x55, 0x55, smd_c, encoded]
            );
            assert_eq!(
                decode_packet(&continuation).unwrap().kind,
                PacketKind::Continuation {
                    frame_count: frame,
                    frag_count: frag
                }
            );
        }
    }
    for (body, fcs, crc) in [
        (vec![0; 60], "08891204", "f7761204"),
        ((0u8..60).collect(), "ee7fecb0", "1180ecb0"),
        ((0u8..120).collect(), "6e5e4523", "91a14523"),
        ((0u8..180).collect(), "446e75a1", "bb9175a1"),
    ] {
        assert_eq!(
            crate::runtime::ethernet::crc32(&body)
                .to_le_bytes()
                .to_vec(),
            unhex(fcs)
        );
        assert_eq!(mcrc(&body).to_vec(), unhex(crc));
    }
    let mut expected = vec![0x55; 7];
    expected.push(7);
    expected.extend([0; 60]);
    expected.extend(unhex("f7761204"));
    assert_eq!(encode_control(PacketKind::Verify).unwrap(), expected);
    expected[7] = 0x19;
    assert_eq!(encode_control(PacketKind::Respond).unwrap(), expected);
    assert_eq!(bit_duration(576, 1_000_000_000).unwrap(), 576000);
    assert_eq!(bit_duration(672, 100_000_000).unwrap(), 6720000);
}
#[test]
fn mac_merge_literal_rtag_reassembly_with_express_interleave() {
    let original = rtag();
    let fs = literal_rtag_fragments();
    let mut rx = MergeRxState::new(7, 2048).unwrap();
    for (i, f) in fs.iter().enumerate() {
        assert_eq!(f.len(), 72);
        assert_eq!(
            encode_fragment(
                &original,
                i * 60,
                (i + 1) * 60,
                0,
                if i == 0 { None } else { Some((i - 1) as u8) },
                true,
                0
            )
            .unwrap(),
            *f
        );
        let out = rx.receive(7, i as u64, external(f.clone()));
        assert!(out.errors.is_empty());
        if i < 2 {
            assert!(out.ingress.is_none());
            assert!(out.suspended);
            rx.validate_checkpoint().unwrap();
            let express = rx.receive(
                7,
                10 + i as u64,
                external(encode_express(&mac(vec![0; 60])).unwrap()),
            );
            assert_eq!(express.ingress.unwrap().bytes, mac(vec![0; 60]));
            assert!(rx.assembly.is_some());
        } else {
            assert_eq!(out.ingress.unwrap().bytes, original);
            assert!(!out.suspended);
        }
    }
}
#[test]
fn mac_merge_minimum_splits_all_add_frag_sizes_and_remaining_boundaries() {
    for add in 0..4 {
        let minimum = 64 * (1 + usize::from(add)) - 4;
        for size in [minimum + 63, minimum + 64, minimum + 65] {
            let m = mac(vec![0; size - 4]);
            let mut tx = MergeTxState::new(1);
            tx.select_pmac(m, true, add).unwrap();
            let p = tx.plan_packet(0, Some(0), 1_000_000_000).unwrap();
            assert_eq!(p.final_packet, size < minimum + 64);
            if !p.final_packet {
                assert_eq!(p.end_offset, minimum);
                assert_eq!(p.bytes.len(), minimum + 12);
            }
        }
    }
    assert!(encode_fragment(&mac(vec![0; 119]), 0, 60, 0, None, true, 0).is_err()); // remaining63
    assert!(encode_fragment(&mac(vec![0; 120]), 0, 60, 0, None, true, 0).is_ok()); // remaining64
}
#[test]
fn mac_merge_tx_demand_octet_boundary_latch_and_no_fixed_slicing() {
    let mut tx = MergeTxState::new(0);
    tx.select_pmac(mac(vec![0; 296]), true, 0).unwrap();
    assert!(tx.plan_packet(0, None, 1_000_000_000).unwrap().final_packet);
    let p = tx
        .plan_packet(0, Some(64000 + 60 * 8000 + 1), 1_000_000_000)
        .unwrap();
    assert_eq!(p.end_offset, 61);
    // Replacing a tentative packet invalidates its old EOF token.
    let newer = tx.plan_packet(0, Some(0), 1_000_000_000).unwrap();
    assert_eq!(
        tx.complete_packet(&p, p.eof_ps),
        Err(MergeError::InvalidState)
    );
    tx.complete_packet(&newer, newer.eof_ps).unwrap();
    assert_eq!(tx.tx_frame, 0);
    assert_eq!(tx.tx_frag, 0);
    assert!(
        tx.plan_packet(newer.release_ps - 1, Some(0), 1_000_000_000)
            .is_err()
    );
    let mut inactive = MergeTxState::new(0);
    inactive.select_pmac(rtag(), false, 0).unwrap();
    let p = inactive.plan_packet(0, Some(0), 1_000_000_000).unwrap();
    assert!(p.final_packet);
    assert_eq!(p.bytes[7], 0xd5);
    inactive.complete_packet(&p, p.eof_ps).unwrap();
    assert_eq!(inactive.tx_frame, 1);
}
#[test]
fn mac_merge_tx_three_fragment_timing_and_modulo_counts() {
    for rate in [100_000_000, 1_000_000_000] {
        let scale = 1_000_000_000 / rate;
        let mut tx = MergeTxState::new(0);
        tx.select_pmac(rtag(), true, 0).unwrap();
        let expected = literal_rtag_fragments();
        for (i, (start, eof, release)) in [
            (0, 576000, 672000),
            (1344000, 1920000, 2016000),
            (2688000, 3264000, 3360000),
        ]
        .into_iter()
        .enumerate()
        {
            let p = tx
                .plan_packet(
                    start * scale,
                    if i < 2 { Some(start * scale) } else { None },
                    rate,
                )
                .unwrap();
            assert_eq!(p.bytes, expected[i]);
            assert_eq!((p.eof_ps, p.release_ps), (eof * scale, release * scale));
            tx.complete_packet(&p, p.eof_ps).unwrap();
            tx.validate_checkpoint().unwrap();
        }
        assert_eq!(tx.tx_frame, 1);
        assert!(tx.selected.is_none());
    }
    let mut tx = MergeTxState::new(0);
    for i in 0..9 {
        tx.select_pmac(mac(vec![0; 60]), true, 0).unwrap();
        let p = tx.plan_packet(tx.ipg_until, None, 1_000_000_000).unwrap();
        assert_eq!(p.bytes[7], SMD_S[i % 4]);
        tx.complete_packet(&p, p.eof_ps).unwrap();
    }
    assert_eq!(tx.tx_frame, 1);
    assert_eq!(
        select_service(false, false, true, true, true, true),
        IdleSelection::Express
    );
    assert_eq!(
        select_service(false, false, false, true, true, true),
        IdleSelection::Respond
    );
    assert_eq!(
        select_service(true, false, false, true, true, true),
        IdleSelection::Preemptable
    );
    assert_eq!(
        select_service(true, true, false, true, true, true),
        IdleSelection::None
    );
}
#[test]
fn mac_merge_rx_errors_never_synthesize_original_delivery() {
    let fs = literal_rtag_fragments();
    for (index, byte) in [(0, 7), (0, 20), (0, 71), (1, 6), (1, 7), (1, 70), (2, 71)] {
        let mut rx = MergeRxState::new(0, 2048).unwrap();
        let mut delivered = false;
        for (i, f) in fs.iter().enumerate() {
            let mut f = f.clone();
            if i == index {
                f[byte] ^= 0x80;
            }
            delivered |= rx.receive(0, i as u64, external(f)).ingress.is_some();
        }
        assert!(!delivered, "corruption at {index}/{byte}");
    }
    let mut rx = MergeRxState::new(0, 2048).unwrap();
    assert!(rx.receive(0, 0, external(fs[0].clone())).suspended);
    // Missing final has no timer-driven successful completion.
    assert!(rx.receive(0, 1, external(fs[1].clone())).ingress.is_none());
    rx.validate_checkpoint().unwrap();
    assert_eq!(rx.assembly.as_ref().unwrap().bytes.len(), 120);
    let replacement = rx.receive(0, 2, external(fs[0].clone()));
    assert_eq!(replacement.errors, vec![MergeError::FrameCheckError]);
    assert!(replacement.suspended);
    assert_eq!(
        rx.receive(0, 3, external(fs[2].clone())).errors,
        vec![MergeError::CounterMismatch]
    );
    assert!(!rx.receive(0, 4, external(fs[1].clone())).errors.is_empty());
    rx.receive(0, 5, external(fs[0].clone()));
    assert!(rx.reset(1));
    assert_eq!(
        rx.receive(0, 6, external(fs[2].clone())).errors,
        vec![MergeError::WrongEpoch]
    );
    assert!(rx.assembly.is_none());
    let mut bounded = MergeRxState::new(0, 100).unwrap();
    bounded.receive(0, 0, external(fs[0].clone()));
    assert_eq!(
        bounded.receive(0, 1, external(fs[1].clone())).errors,
        vec![MergeError::ReceiveLimit]
    );
}
#[test]
fn mac_merge_rx_continuation_modulo_wrap_and_checkpoint_crc_tamper() {
    let original = mac((0..480).map(|n| n as u8).collect());
    let mut tx = MergeTxState::new(4);
    tx.select_pmac(original.clone(), true, 0).unwrap();
    let mut rx = MergeRxState::new(4, 2048).unwrap();
    let mut at = 0;
    for i in 0..8 {
        let p = tx.plan_packet(at, Some(at), 1_000_000_000).unwrap();
        if i > 0 {
            assert_eq!(p.bytes[7], SMD_S[(i - 1) % 4]);
        }
        let out = rx.receive(4, p.eof_ps, external(p.bytes.clone()));
        tx.complete_packet(&p, p.eof_ps).unwrap();
        at = p.release_ps;
        if i < 7 {
            assert!(out.suspended);
        } else {
            assert_eq!(out.ingress.unwrap().bytes, original);
        }
    }
    rx.receive(4, at, external(literal_rtag_fragments()[0].clone()));
    rx.assembly.as_mut().unwrap().bytes[0] ^= 1;
    assert_eq!(rx.validate_checkpoint(), Err(MergeError::InvalidCheckpoint));
}
#[test]
fn mac_merge_actual_capability_prerequisite_and_timer_after_real_eof() {
    let mut v = VerificationState::new(3, true, 10).unwrap();
    v.request_if_enabled();
    assert!(!v.p_enable());
    assert!(!v.send_v);
    assert!(!v.p_active());
    v.observe_capabilities(capability(3, 3)).unwrap();
    assert!(v.p_enable());
    assert!(v.send_v);
    assert!(v.timer.is_none());
    assert_eq!(v.attempts, 0);
    assert!(!v.peer.as_ref().unwrap().enabled);
    assert!(!v.peer.as_ref().unwrap().active);
    let timer = v
        .tx_complete(3, PacketKind::Verify, 9_000_000)
        .unwrap()
        .unwrap();
    assert_eq!(timer.due_ps, 10_009_000_000);
    v.validate_checkpoint().unwrap();
    v.receive_control(3, PacketKind::Respond).unwrap();
    assert!(v.p_active());
    assert!(!v.timeout(timer, timer.due_ps).unwrap());
    v.validate_checkpoint().unwrap();
    v.link_reset(4, true).unwrap();
    assert!(!v.p_active());
    assert!(v.peer.is_none());
    assert_eq!(
        v.receive_control(3, PacketKind::Respond),
        Err(MergeError::WrongEpoch)
    );
    assert!(!v.p_active());
}
#[test]
fn mac_merge_three_attempts_sticky_failure_and_independent_responder() {
    let mut v = VerificationState::new(0, true, 1).unwrap();
    v.observe_capabilities(capability(0, 0)).unwrap();
    for attempt in 1..=3 {
        let timer = v
            .tx_complete(0, PacketKind::Verify, attempt * 2_000_000_000)
            .unwrap()
            .unwrap();
        assert_eq!(v.attempts, u8::try_from(attempt).unwrap());
        v.timeout(timer, timer.due_ps).unwrap();
        v.validate_checkpoint().unwrap();
    }
    assert!(v.verify_fail);
    assert!(!v.send_v);
    assert!(v.timer.is_none());
    v.receive_control(0, PacketKind::Respond).unwrap();
    assert!(!v.p_active());
    assert!(v.verify_fail);
    v.receive_control(0, PacketKind::Verify).unwrap();
    assert!(v.send_r);
    v.tx_complete(0, PacketKind::Respond, 9_000_000_000)
        .unwrap();
    assert!(!v.send_r);
    v.set_admin_enabled(false).unwrap();
    v.set_admin_enabled(true).unwrap();
    assert!(v.send_v);
    assert!(!v.verify_fail);
    assert_eq!(v.attempts, 0);
    let timer = v
        .tx_complete(0, PacketKind::Verify, 10_000_000_000)
        .unwrap()
        .unwrap();
    v.receive_control(0, PacketKind::Respond).unwrap();
    v.timeout(timer, timer.due_ps).unwrap();
    assert!(v.p_active());
}
#[test]
fn mac_merge_corrupt_verify_and_respond_do_not_set_flags() {
    for kind in [PacketKind::Verify, PacketKind::Respond] {
        let mut rx = MergeRxState::new(1, 2048).unwrap();
        let mut b = encode_control(kind).unwrap();
        b[71] ^= 1;
        let out = rx.receive(1, 0, external(b));
        assert!(out.control.is_none());
        assert!(!out.errors.is_empty());
        assert_eq!(
            rx.receive(1, 1, external(encode_control(kind).unwrap()))
                .control,
            Some(kind)
        );
    }
    let mut v = VerificationState::new(1, false, 10).unwrap();
    v.receive_control(1, PacketKind::Verify).unwrap();
    assert!(v.send_r);
    assert!(!v.p_enable());
    v.tx_complete(1, PacketKind::Respond, 10).unwrap();
}

#[test]
fn mac_merge_pallow_latches_at_delimiter_and_stays_latched() {
    let mut tx = MergeTxState::new(0);
    tx.select_pmac(rtag(), false, 0).unwrap();
    let before = tx.plan_packet(0, Some(0), 1_000_000_000).unwrap();
    assert!(before.final_packet);
    assert!(tx.smd_sent(true).unwrap());
    let active = tx.plan_packet(0, Some(0), 1_000_000_000).unwrap();
    assert!(!active.final_packet);
    assert_eq!(active.bytes[7], 0xe6);
    assert!(tx.complete_packet(&before, before.eof_ps).is_err());
    assert!(tx.smd_sent(false).is_err());
    tx.complete_packet(&active, active.eof_ps).unwrap();
    assert!(tx.smd_sent(false).is_err());
    let mut inactive = MergeTxState::new(0);
    inactive.select_pmac(rtag(), false, 0).unwrap();
    inactive.smd_sent(false).unwrap();
    assert!(inactive.smd_sent(true).is_err());
    assert!(
        inactive
            .plan_packet(0, Some(100000), 1_000_000_000)
            .unwrap()
            .final_packet
    );
}

#[test]
fn mac_merge_suspended_assembly_survives_unrelated_bad_control_and_smd() {
    let fs = literal_rtag_fragments();
    let mut rx = MergeRxState::new(0, 2048).unwrap();
    rx.receive(0, 0, external(fs[0].clone()));
    let mut bad_control = encode_control(PacketKind::Verify).unwrap();
    bad_control[71] ^= 1;
    assert!(!rx.receive(0, 1, external(bad_control)).errors.is_empty());
    assert!(rx.assembly.is_some());
    let mut unknown = encode_control(PacketKind::Verify).unwrap();
    unknown[7] = 0x33;
    assert_eq!(
        rx.receive(0, 2, external(unknown)).errors,
        vec![MergeError::InvalidDelimiter]
    );
    assert!(rx.assembly.is_some());
    assert!(rx.receive(0, 3, external(fs[1].clone())).suspended);
    assert_eq!(
        rx.receive(0, 4, external(fs[2].clone()))
            .ingress
            .unwrap()
            .bytes,
        rtag()
    );
}

#[test]
fn mac_merge_checkpoint_roundtrip_and_reject_timers_counters_and_crcs() {
    let mut tx = MergeTxState::new(3);
    tx.select_pmac(rtag(), true, 0).unwrap();
    tx.smd_sent(true).unwrap();
    let plan = tx.plan_packet(0, Some(0), 1_000_000_000).unwrap();
    tx.complete_packet(&plan, plan.eof_ps).unwrap();
    let saved = serde_json::to_value(&tx).unwrap();
    let restored: MergeTxState = serde_json::from_value(saved.clone()).unwrap();
    restored.validate_checkpoint().unwrap();
    assert_eq!(restored, tx);
    let mut bad = saved;
    bad["tx_frame"] = serde_json::json!(4);
    let bad: MergeTxState = serde_json::from_value(bad).unwrap();
    assert!(bad.validate_checkpoint().is_err());
    let mut v = VerificationState::new(3, true, 10).unwrap();
    v.observe_capabilities(capability(3, 0)).unwrap();
    v.tx_complete(3, PacketKind::Verify, 123).unwrap();
    let saved = serde_json::to_value(&v).unwrap();
    let restored: VerificationState = serde_json::from_value(saved.clone()).unwrap();
    restored.validate_checkpoint().unwrap();
    assert_eq!(restored, v);
    for field in ["timer", "last_verify_eof"] {
        let mut bad = saved.clone();
        bad[field] = serde_json::Value::Null;
        let bad: VerificationState = serde_json::from_value(bad).unwrap();
        assert!(bad.validate_checkpoint().is_err());
    }
    let mut bad = saved;
    bad["timer"]["due_ps"] = serde_json::json!(1);
    let bad: VerificationState = serde_json::from_value(bad).unwrap();
    assert!(bad.validate_checkpoint().is_err());
}

#[test]
fn mac_merge_eof_plan_timing_and_overflow_rejection_are_atomic() {
    let mut tx = MergeTxState::new(0);
    tx.select_pmac(rtag(), true, 0).unwrap();
    let mut p = tx.plan_packet(0, Some(0), 1_000_000_000).unwrap();
    let before = tx.clone();
    p.release_ps += 1;
    assert!(tx.complete_packet(&p, p.eof_ps).is_err());
    assert_eq!(tx, before);
    let mut v = VerificationState::new(0, true, 10).unwrap();
    v.observe_capabilities(capability(0, 0)).unwrap();
    let before = v.clone();
    assert_eq!(
        v.tx_complete(0, PacketKind::Verify, u64::MAX),
        Err(MergeError::Overflow)
    );
    assert_eq!(v, before);
}

#[test]
fn mac_merge_responder_retains_a_verify_received_while_response_pending() {
    let mut v = VerificationState::new(1, false, 10).unwrap();
    v.receive_control(1, PacketKind::Verify).unwrap();
    assert!(v.send_r);
    assert!(!v.rcv_v);
    v.receive_control(1, PacketKind::Verify).unwrap();
    assert!(v.rcv_v);
    v.tx_complete(1, PacketKind::Respond, 10).unwrap();
    assert!(v.send_r);
    assert!(!v.rcv_v);
    v.validate_checkpoint().unwrap();
    v.tx_complete(1, PacketKind::Respond, 20).unwrap();
    assert!(!v.send_r);
    v.validate_checkpoint().unwrap();
}

#[test]
fn mac_merge_control_completion_tokens_reject_same_epoch_reset_and_replay() {
    let mut v = VerificationState::new(1, true, 10).unwrap();
    v.observe_capabilities(capability(1, 0)).unwrap();
    let old = v.control_tx_token(PacketKind::Verify).unwrap();
    v.set_admin_enabled(false).unwrap();
    v.set_admin_enabled(true).unwrap();
    let before = v.clone();
    assert!(v.tx_complete_token(old, 100).is_err());
    assert_eq!(v, before);
    let current = v.control_tx_token(PacketKind::Verify).unwrap();
    v.tx_complete_token(current, 200).unwrap();
    assert!(v.tx_complete_token(current, 200).is_err());
    v.receive_control(1, PacketKind::Verify).unwrap();
    v.receive_control(1, PacketKind::Verify).unwrap();
    let first = v.control_tx_token(PacketKind::Respond).unwrap();
    v.tx_complete_token(first, 300).unwrap();
    assert!(v.send_r);
    assert!(v.tx_complete_token(first, 300).is_err());
    let second = v.control_tx_token(PacketKind::Respond).unwrap();
    v.tx_complete_token(second, 400).unwrap();
    assert!(!v.send_r);
}

#[test]
fn mac_merge_network_reservation_failures_preserve_exact_committed_prefix() {
    for point in [
        "mac_merge_cut",
        "mac_merge_response_packet",
        "mac_merge_verify_timer",
        "mac_merge_packet_arrival",
        "mac_merge_validated_ingress",
        "mac_merge_accounting_journal",
        "mac_merge_accounting_packet_proof",
        "mac_merge_accounting_queue_cause",
    ] {
        let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/mac-merge/model.ini");
        let mut prepared = crate::prepare(&input).unwrap();
        if point.starts_with("mac_merge_accounting_") {
            let network = std::sync::Arc::make_mut(
                prepared
                    .registered
                    .as_mut()
                    .unwrap()
                    .network
                    .as_mut()
                    .unwrap(),
            );
            network.tsn.as_mut().unwrap().outputs[0].cbs[0] =
                Some(crate::types::ethernet::tsn::CbsConfig {
                    priority: 0,
                    idle_bps: 250_000_000,
                    link_bps: 1_000_000_000,
                    hi: 1_000_000_000_000_000_000,
                    lo: 1_000_000_000_000_000_000,
                });
        }
        let (mut failing, _) =
            crate::RunSession::new(prepared.clone(), crate::RunOptions::default()).unwrap();
        if point.starts_with("mac_merge_accounting_") {
            while failing.snapshot().common.last_event_time_ps.unwrap_or(0) < 2_016_000 {
                assert_eq!(failing.advance_one().unwrap(), crate::RunState::Running);
            }
        }
        crate::allocation::fail_next_reservation(point);
        assert_eq!(
            failing.run_until_yield().unwrap(),
            crate::RunState::Failed,
            "{point}"
        );
        let failed = failing.observe().unwrap();
        assert_eq!(failed.common.termination, "execution_failed", "{point}");
        let (mut baseline, _) =
            crate::RunSession::new(prepared, crate::RunOptions::default()).unwrap();
        while baseline.snapshot().common.committed_events < failed.common.committed_events {
            assert_eq!(baseline.advance_one().unwrap(), crate::RunState::Running);
        }
        let expected = baseline.observe().unwrap();
        assert_eq!(
            expected.common.committed_events, failed.common.committed_events,
            "{point}"
        );
        assert_eq!(
            expected.network.as_ref().unwrap()["mac_merge"],
            failed.network.as_ref().unwrap()["mac_merge"],
            "{point}"
        );
        assert_eq!(
            expected.network.as_ref().unwrap()["tsn"],
            failed.network.as_ref().unwrap()["tsn"],
            "{point}: credit must preserve the exact committed prefix"
        );
        let a = &expected.registered.as_ref().unwrap().model_records;
        let b = &failed.registered.as_ref().unwrap().model_records;
        for (id, row) in a
            .iter()
            .filter(|(_, r)| r.schema.name.starts_with("dir.mac_merge."))
        {
            assert_eq!(row.data, b[id].data, "{point}: {id}");
        }
        assert_eq!(
            a.values()
                .filter(|r| r.schema.name.starts_with("dir.mac_merge."))
                .count(),
            b.values()
                .filter(|r| r.schema.name.starts_with("dir.mac_merge."))
                .count(),
            "{point}"
        );
    }
}

#[test]
fn mac_merge_selected_lldp_adapter_requires_actual_mandatory_tlvs_and_ttl() {
    let cfg = crate::types::network::mac_merge::MergePort {
        source_mac: [2, 0, 0, 0, 0, 1],
        express_mask: 128,
        preemptable_mask: 127,
        advertised_add_frag_size: 3,
        verify_time_ms: 10,
        admin_enabled: true,
    };
    let actual = super::lldp::advertise("p", &cfg, false, false, 17);
    let ingress = ValidatedMacIngress {
        epoch: 8,
        received_at: 100,
        bytes: actual.clone(),
        provenance: RxProvenance::ExternalInjection {
            evidence_id: "actual-lldp".into(),
        },
    };
    let observation = super::lldp::receive(4, &ingress, "actual-lldp".into()).unwrap();
    assert_eq!(observation.ttl, 17);
    let cap = observation.capability.unwrap();
    assert_eq!(cap.add_frag_size, 3);
    assert!(cap.support);
    assert!(!cap.enabled);
    let mut malformed = actual;
    malformed[14] = 0;
    malformed[15] = 0;
    let n = malformed.len();
    let crc = crate::runtime::ethernet::crc32(&malformed[..n - 4]).to_le_bytes();
    malformed[n - 4..].copy_from_slice(&crc);
    let bad = ValidatedMacIngress {
        bytes: malformed,
        ..ingress
    };
    assert!(super::lldp::receive(4, &bad, "actual-lldp".into()).is_err());
}

#[test]
fn mac_merge_lldp_mandatory_shutdown_and_refresh_replace_matching_msap() {
    // Independent literal from review: mandatory-only TTL-zero shutdown, valid FCS.
    let shutdown = unhex(
        "0180c200000e02000000000188cc0207040200000000010402077006020000000000000000000000000000000000000000000000000000000000000029ddec4a",
    );
    let packet = ValidatedMacIngress {
        epoch: 0,
        received_at: 100,
        bytes: shutdown,
        provenance: RxProvenance::ExternalInjection {
            evidence_id: "shutdown".into(),
        },
    };
    let shutdown = lldp::receive(0, &packet, "shutdown".into()).unwrap();
    assert_eq!(shutdown.ttl, 0);
    assert!(shutdown.capability.is_none());
    let cfg = crate::types::network::mac_merge::MergePort {
        source_mac: [2, 0, 0, 0, 0, 1],
        express_mask: 128,
        preemptable_mask: 127,
        advertised_add_frag_size: 0,
        verify_time_ms: 1,
        admin_enabled: true,
    };
    let original = ValidatedMacIngress {
        bytes: lldp::advertise("p", &cfg, true, true, 3),
        ..packet.clone()
    };
    let original = lldp::receive(0, &original, "original".into())
        .unwrap()
        .capability
        .unwrap();
    assert!(shutdown.matches(&original));
    let mut refresh = packet.clone();
    refresh.bytes[29] = 0;
    refresh.bytes[30] = 3;
    let n = refresh.bytes.len();
    let crc = crate::runtime::ethernet::crc32(&refresh.bytes[..n - 4]).to_le_bytes();
    refresh.bytes[n - 4..].copy_from_slice(&crc);
    let refresh = lldp::receive(0, &refresh, "refresh".into()).unwrap();
    assert_eq!(refresh.ttl, 3);
    assert!(refresh.capability.is_none());
    assert!(refresh.matches(&original));
    let mut other = original;
    other.port_id = vec![7, b'q'];
    assert!(!shutdown.matches(&other));
}

#[test]
fn prepared_merge_port_rejects_invalid_physical_profile() {
    let mut p = PreparedMergePort {
        bitrate: 1_000_000_000,
        full_duplex: true,
        advertised_add_frag_size: 0,
        verify_time_ms: 10,
        admin_enabled: true,
        max_rx_mac_octets: 1518,
    };
    assert_eq!(p.validate(), Ok(()));
    p.full_duplex = false;
    assert_eq!(p.validate(), Err(MergeError::InvalidConfiguration));
}
