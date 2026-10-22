//! Stable-epoch Merge protocol arrival envelopes, including peer-driven Respond.
use super::*;
fn lldp(t: u64, interval: u64) -> Result<u64, Rejection> {
    if interval == 0 {
        return Err(Rejection::InvalidState);
    }
    add(3, t.div_ceil(interval))
}
pub fn merge_interference(e: &MergeControlEnvelope, t: u64) -> Result<u64, Rejection> {
    let local = lldp(t, e.local_lldp_interval_ps)?;
    let verify = add(
        3,
        multiply(
            3,
            lldp(add(t, e.peer_lldp_jitter_ps)?, e.peer_lldp_interval_ps)?,
        )?,
    )?;
    let peer_verify = add(
        3,
        multiply(
            3,
            lldp(
                add(add(t, e.peer_verify_jitter_ps)?, e.local_lldp_jitter_ps)?,
                e.local_lldp_interval_ps,
            )?,
        )?,
    )?;
    let respond = add(2, peer_verify)?;
    add(
        add(
            multiply(local, add(e.lldp_occupied_ps, e.admission_alignment_ps)?)?,
            multiply(verify, add(e.verify_occupied_ps, e.admission_alignment_ps)?)?,
        )?,
        multiply(
            respond,
            add(e.respond_occupied_ps, e.admission_alignment_ps)?,
        )?,
    )
}
pub fn burst_service(
    demand: Option<&TransmissionDemand>,
    ordinary: u64,
    count: u64,
    cycle: u64,
    budget: &mut u32,
    limit: u32,
) -> Result<(u64, u64), Rejection> {
    let data = multiply(ordinary, count)?;
    let Some(control) = demand.and_then(|d| d.merge_control.as_ref()) else {
        return Ok((data, 0));
    };
    let initial = add(data, control.carry_in_ps)?;
    let mut x = initial;
    loop {
        *budget = budget.checked_add(1).ok_or(Rejection::SearchLimit)?;
        if *budget > limit {
            return Err(Rejection::SearchLimit);
        }
        let next = add(initial, merge_interference(control, x)?)?;
        if next > cycle {
            return Err(Rejection::InfeasibleFrameCapacity);
        }
        if next <= x {
            return Ok((x, x - data));
        }
        x = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_actual_start_counts_charge_each_alignment_and_both_peer_delays() {
        let e = MergeControlEnvelope {
            peer_direction: 1,
            raw_attachment: Some(0),
            admission_alignment_ps: 8_000,
            local_lldp_interval_ps: 1_000_000,
            peer_lldp_interval_ps: 2_000_000,
            local_lldp_jitter_ps: 10,
            peer_lldp_jitter_ps: 10,
            peer_verify_jitter_ps: 20,
            lldp_occupied_ps: 672_000,
            verify_occupied_ps: 672_000,
            respond_occupied_ps: 672_000,
            carry_in_ps: 16_736_000,
        };
        // Independent counts in [a,a+2us): L=5, V=18, R=23.
        // 46 separate starts each reserve a whole8ns admission allowance.
        assert_eq!(merge_interference(&e, 2_000_000), Ok(46 * 680_000));
        let mut overflow = e;
        overflow.admission_alignment_ps = u64::MAX;
        assert_eq!(
            merge_interference(&overflow, 1),
            Err(Rejection::ArithmeticOverflow)
        );
    }
    #[test]
    fn delayed_verifier_admission_has_three_attempts_and_two_response_carry_ins() {
        use crate::runtime::network::mac_merge::{
            PacketKind, PeerCapabilitiesObserved, VerificationState,
        };
        let mut verifier = VerificationState::new(7, true, 1).unwrap();
        verifier
            .observe_capabilities(PeerCapabilitiesObserved {
                ingress: 0,
                link_epoch: 7,
                source_mac: [2, 0, 0, 0, 0, 2],
                port_id: vec![7, 2],
                received_at: 0,
                support: true,
                enabled: true,
                active: false,
                add_frag_size: 0,
                lldp_evidence_id: "actual-decoded-lldp".into(),
            })
            .unwrap();
        let mut starts = Vec::new();
        for n in 0..3u64 {
            // Arbitrary serializer backlog delays a start by many LLDP periods;
            // no retry timer exists until its actual EOF.
            assert!(verifier.send_v);
            assert!(verifier.timer.is_none());
            let start = 100_000_000_000 + n * 70_000_000_000;
            starts.push(start);
            let token = verifier
                .tx_complete(7, PacketKind::Verify, start + 576_000)
                .unwrap()
                .unwrap();
            assert_eq!(token.due_ps, start + 576_000 + 1_000_000_000);
            assert!(verifier.timeout(token, token.due_ps).unwrap());
        }
        assert_eq!(starts.len(), 3);
        assert!(!verifier.send_v);
        assert!(verifier.verify_fail);
        for _ in 0..20 {
            verifier.receive_control(7, PacketKind::Verify).unwrap();
        }
        assert!(verifier.send_r && verifier.rcv_v);
        verifier
            .tx_complete(7, PacketKind::Respond, 400_000_000_000)
            .unwrap();
        assert!(verifier.send_r && !verifier.rcv_v);
        verifier
            .tx_complete(7, PacketKind::Respond, 500_000_000_000)
            .unwrap();
        assert!(!verifier.send_r && !verifier.rcv_v);
        // These are supplemental reducer tests; production sampled proof is
        // exercised by qcc_raw_certified_startup_installs_and_delivers_actual_sampled_stream.
    }
}

#[cfg(test)]
mod raw_transport_boundary_tests {
    #[test]
    fn actual_raw_codec_terminators_449_450_and_next_frame_obey_4080ns() {
        use crate::runtime::ethernet::phy::ieee::{
            t1_gigabit::{Control, Gmii, Role},
            t1_raw::{RawReceiveStream, RawTransmitStream},
        };
        for terminator in [449u64, 450, 900] {
            let bytes = [vec![0x55; 7], vec![0xe6], vec![0x37; 64]].concat();
            let start = terminator - bytes.len() as u64;
            let mut tx = RawTransmitStream::new(Role::Master, 0x2255).unwrap();
            let mut rx = RawReceiveStream::new(Role::Master, 0x2255, 16_400).unwrap();
            let mut packets = Vec::new();
            for _ in 0..10_800 {
                if tx.ingress_due() {
                    let slot = tx.consumed_slots();
                    let value = if slot >= start && slot < terminator {
                        Gmii::Data(bytes[(slot - start) as usize])
                    } else {
                        Gmii::Control(Control::InterFrameReady)
                    };
                    tx.push_gmii(value).unwrap();
                }
                packets.extend(rx.push_symbol(tx.next_symbol().unwrap()).unwrap());
            }
            assert!(rx.initial_idle_verified());
            assert_eq!(packets.len(), 1);
            assert_eq!(packets[0].bytes, bytes);
            assert_eq!(packets[0].terminator_slot, terminator);
            // xMII first idle slot starts at2340+6*terminator UIs.
            // Integer symbol bound3060 *4/3ns =4080ns exactly.
            let displacement = packets[0].decoded_at_symbol - (2340 + 6 * terminator);
            assert!(displacement <= 3060);
            if terminator % 450 == 0 {
                assert_eq!(displacement, 3060);
            }
        }
    }
}

#[cfg(test)]
mod raw_audit_codec_tests {
    use super::*;
    #[test]
    fn internally_tagged_raw_source_audit_roundtrips_nonempty_numeric_attachment_map() {
        let physical = crate::runtime::ethernet::phy::medium::RawTimingCertificate {
            medium_config_sha256: "0".repeat(64),
            attachment_config_sha256: Some("1".repeat(64)),
            attachment_origin_ps: 0,
            endpoints: vec!["p0".into(), "p1".into()],
            physical_epoch_start_samples: vec![0, 0],
            grid: crate::runtime::ethernet::phy::SampleGrid {
                symbol_rate_hz: 750_000_000,
                samples_per_ui: 16,
            },
            bilateral_ready: false,
            initial_idle_verified: false,
            directions: vec![],
        };
        let step = AuditStep::RawSources {
            at_ps: 123,
            certificates: BTreeMap::from([(
                0,
                RawSourceCertificate {
                    attachment: 0,
                    physical,
                    merge: vec![],
                },
            )]),
        };
        let value = serde_json::to_value(&step).unwrap();
        assert_eq!(serde_json::from_value::<AuditStep>(value).unwrap(), step);
    }
}
