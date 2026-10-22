use super::super::*;
use super::codec::{self, Body, Message};
use super::integration::{
    application_rate, application_target, from_wire_correction, gerr, reference_duration,
    stamp_with_correction, timestamp_part, to_wire_correction,
};
use super::math::{self, Rat};
use super::state::{self, CaptureKind, Timer};
use std::collections::{BTreeMap, BTreeSet};

fn received(packet: &state::Packet) -> Result<&Message> {
    packet
        .received_message
        .as_ref()
        .ok_or_else(|| gerr("validated gPTP packet lacks received body"))
}

/// Reconstruct the receive preconditions at the actual callback, rather than
/// applying the checkpoint's later link state to a historical arrival. Byte
/// decoding and protocol release are separate: stale bytes remain evidence.
fn receive_permissions(state: &state::State) -> Result<BTreeMap<usize, bool>> {
    let mut epochs = vec![0u64; state.ports.len()];
    let mut up = vec![true; state.ports.len()];
    let mut eof = BTreeSet::new();
    let mut rx = BTreeSet::new();
    let mut permissions = BTreeMap::new();
    for cause in &state.causes {
        match cause.kind {
            state::CauseKind::Link {
                direction,
                up: value,
            } => {
                if state.ports.get(direction).is_none_or(Option::is_none) || up[direction] == value
                {
                    return Err(gerr("receive history has invalid link transition"));
                }
                epochs[direction] = add(epochs[direction], 1)?;
                up[direction] = value;
            }
            state::CauseKind::Eof { transfer } => {
                eof.insert(transfer);
            }
            state::CauseKind::Capture {
                transfer,
                kind: CaptureKind::Rx,
            }
            | state::CauseKind::SampledCapture {
                transfer,
                kind: CaptureKind::Rx,
                ..
            } => {
                rx.insert(transfer);
            }
            state::CauseKind::Receive { transfer } => {
                let packet = state
                    .packets
                    .get(&transfer)
                    .ok_or_else(|| gerr("receive history packet missing"))?;
                let direction = packet.direction;
                let permitted = epochs.get(direction).copied() == Some(packet.link_epoch)
                    && up.get(direction).copied() == Some(true)
                    && eof.contains(&transfer)
                    && rx.contains(&transfer);
                if permissions.insert(transfer, permitted).is_some() {
                    return Err(gerr("duplicate actual receive cause"));
                }
            }
            _ => {}
        }
    }
    Ok(permissions)
}

impl NetworkState {
    fn gptp_periodic_deadline(
        &self,
        node: usize,
        generation: u64,
        interval_ps: u64,
    ) -> Result<(Rat, u64)> {
        let initial = self.gptp_raw(node, 0)?;
        let elapsed = Rat::new(i128::from(generation), 1)
            .map_err(gerr)?
            .mul(Rat::new(i128::from(interval_ps), 1).map_err(gerr)?)
            .map_err(gerr)?;
        let raw = initial.add(elapsed).map_err(gerr)?;
        Ok((raw, self.gptp_global_deadline(node, raw)?))
    }
}

impl NetworkState {
    pub(in crate::runtime::network) fn validate_gptp_checkpoint(
        &self,
        horizon: u64,
        initial: &[clock::NodeClock],
    ) -> Result<()> {
        let cfg = self
            .prepared
            .gptp
            .as_ref()
            .ok_or_else(|| gerr("checkpoint has no prepared gPTP"))?;
        let state = self
            .gptp
            .as_ref()
            .ok_or_else(|| gerr("checkpoint gPTP owner absent"))?;
        if state.ports.len() != cfg.ports.len()
            || state.nodes.len() != cfg.nodes.len()
            || state
                .ports
                .iter()
                .zip(&cfg.ports)
                .any(|(s, p)| s.is_some() != p.is_some())
        {
            return Err(gerr("checkpoint gPTP dimensions differ"));
        }
        let current_policy = self.policy(&self.delta());
        for (direction, port) in state.ports.iter().enumerate() {
            let Some(port) = port else { continue };
            let name = &self.prepared.ethernet.directions[direction].from_port;
            let actual_up = current_policy
                .as_ref()
                .is_none_or(|policy| policy.link_up.get(name).copied().unwrap_or(false));
            if port.link_up != actual_up {
                return Err(gerr(
                    "checkpoint gPTP link state differs from physical policy",
                ));
            }
        }
        if state.packets.len() > self.transfers.len()
            || state.journal.len() > cfg.max_journal_entries
            || state.selections.len() > cfg.max_journal_entries
            || state.samples.len() > cfg.max_journal_entries
            || state.pending_exchange_count() > cfg.max_pending_exchanges
        {
            return Err(gerr("checkpoint gPTP bounded state exceeded"));
        }
        let receive_permissions = receive_permissions(state)?;
        let mut ids = BTreeSet::new();
        for (&t, packet) in &state.packets {
            if packet.transfer != t
                || !ids.insert(packet.id)
                || packet.id >= state.next_packet
                || cfg.ports.get(packet.direction).is_none_or(Option::is_none)
                || self
                    .transfers
                    .get(t)
                    .is_none_or(|v| v.direction != packet.direction || v.frame != packet.frame)
                || self
                    .frames
                    .get(packet.frame)
                    .is_none_or(|v| v.row.frame_id != self.transfers[t].row.frame_id)
            {
                return Err(gerr("checkpoint packet/transfer ownership differs"));
            }
            let row = &self.transfers[t].row;
            if !packet.admitted
                && (row.status != "dropped"
                    || !matches!(row.drop_reason.as_deref(), Some("queue_full" | "link_down"))
                    || row.sof_ps.is_some()
                    || packet.tx.is_some()
                    || packet.tx_eof)
            {
                return Err(gerr(
                    "rejected gPTP queue admission has physical transmission",
                ));
            }
            if row.wire.ether_type != codec::ETHERTYPE
                || row.wire.dst_mac != "01:80:c2:00:00:0e"
                || row.wire.tag.is_some()
                || !row.wire.tags.is_empty()
            {
                return Err(gerr("checkpoint gPTP Ethernet binding differs"));
            }
            let bytes = (0..row.wire.mac_hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&row.wire.mac_hex[i..i + 2], 16))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| gerr("checkpoint packet MAC hex invalid"))?;
            if bytes.len() != row.wire.mac_bytes as usize
                || bytes.len() < 64
                || bytes[bytes.len() - 4..]
                    != crate::runtime::ethernet::crc32(&bytes[..bytes.len() - 4]).to_le_bytes()
                || bytes[..6] != codec::MULTICAST
                || bytes[12..14] != codec::ETHERTYPE.to_be_bytes()
            {
                return Err(gerr("checkpoint gPTP FCS or MAC bytes differ"));
            }
            if Message::decode(&bytes[14..bytes.len() - 4]).map_err(gerr)? != packet.message {
                return Err(gerr(
                    "checkpoint decoded packet differs from transfer bytes",
                ));
            }
            if let Some(actual_hex) = &packet.received_mac_hex {
                let actual = super::receive::bytes(actual_hex)?;
                if packet.mac_packet.is_some() {
                    if self
                        .mac_merge
                        .as_ref()
                        .and_then(|merge| merge.validated.get(&t))
                        .is_none_or(|validated| validated != &actual)
                    {
                        return Err(gerr("received gPTP bytes differ from MAC Merge proof"));
                    }
                } else if actual != bytes {
                    return Err(gerr("received gPTP bytes differ from physical transfer"));
                }
                let decoded = super::receive::validate_mac(&actual)
                    .ok()
                    .and_then(|payload| Message::decode(payload).ok());
                let permitted = receive_permissions
                    .get(&t)
                    .copied()
                    .ok_or_else(|| gerr("received MAC bytes lack actual ingress cause"))?;
                let decoded = decoded.filter(|_| permitted);
                if decoded != packet.received_message {
                    return Err(gerr("received gPTP body differs from actual MAC bytes"));
                }
            } else if packet.received_message.is_some()
                || packet.rx_validated
                || receive_permissions.contains_key(&t)
            {
                return Err(gerr("received gPTP body has no MAC byte proof"));
            }
            for (kind, capture) in [
                (CaptureKind::Tx, packet.tx.as_ref()),
                (CaptureKind::Rx, packet.rx.as_ref()),
            ] {
                let Some(capture) = capture else { continue };
                let sof = row
                    .sof_ps
                    .ok_or_else(|| gerr("capture before committed SOF"))?;
                let expected = add(
                    sof,
                    reference_duration(
                        self.prepared.ethernet.directions[packet.direction].bitrate_bps,
                    )?,
                )?;
                let expected = if kind == CaptureKind::Rx {
                    add(
                        expected,
                        self.prepared.ethernet.directions[packet.direction].delay_ps,
                    )?
                } else {
                    expected
                };
                let configured = packet
                    .mac_source
                    .map(|binding| self.gptp_source_proof(binding, kind, &self.delta()))
                    .transpose()?;
                if capture.configured != configured.filter(|p| p.endpoint.is_some())
                    || capture.express != configured.filter(|p| p.endpoint.is_none())
                    || capture.sampled.is_some()
                    || self.configured_endpoint(packet.direction).is_some()
                        != configured.is_some_and(|p| p.endpoint.is_some())
                {
                    return Err(gerr("checkpoint configured capture proof differs"));
                }
                let expected = configured.map_or(expected, |proof| proof.reference_ps);
                let node = if kind == CaptureKind::Tx {
                    cfg.ports[packet.direction].as_ref().unwrap().node
                } else {
                    cfg.ports[cfg.reverse[packet.direction]]
                        .as_ref()
                        .unwrap()
                        .node
                };
                let init = &self.prepared.clocks.as_ref().unwrap().initial[cfg.nodes[node].device];
                let raw = math::raw(init.offset_ps, init.rate_num, init.rate_den, expected)
                    .map_err(gerr)?;
                if capture.at_ps != expected
                    || capture.at_ps > horizon
                    || capture.raw != raw
                    || capture.packet_id != packet.id
                    || capture.link_epoch != packet.link_epoch
                {
                    return Err(gerr(
                        "checkpoint SFD capture disagrees with physical packet",
                    ));
                }
            }
            if packet.rx_validated
                && (packet.tx.is_none()
                    || packet.rx.is_none()
                    || !packet.tx_eof
                    || row.arrival_ps.is_none()
                    || self.receptions.iter().filter(|r| r.transfer == t).count() != 1
                    || packet.received_message.as_ref().is_none_or(|m| {
                        m.source != packet.message.source
                            || m.sequence != packet.message.sequence
                            || m.body.message_type() != packet.message.body.message_type()
                    }))
            {
                return Err(gerr(
                    "checkpoint accepted gPTP packet lacks physical ingress proof",
                ));
            }
            let needs_source = self.configured_endpoint(packet.direction).is_some()
                || packet.mac_packet.is_some()
                || self.preemption_direction(packet.direction);
            if row.sof_ps.is_some() && needs_source {
                let source = packet
                    .mac_source
                    .ok_or_else(|| gerr("packet lacks actual MAC source"))?;
                let proof = self.gptp_source_proof(source, CaptureKind::Tx, &self.delta())?;
                match source {
                    state::MacSource::MergeExpress {
                        packet_id, epoch, ..
                    } => {
                        if packet.mac_packet != Some(packet_id)
                            || packet.physical_epoch != Some(epoch)
                        {
                            return Err(gerr("checkpoint Merge packet/token differs from source"));
                        }
                    }
                    state::MacSource::AbstractExpress { epoch, .. } => {
                        if packet.mac_packet.is_some() || packet.physical_epoch != Some(epoch) {
                            return Err(gerr("checkpoint abstract SOF token differs from source"));
                        }
                    }
                    _ => {}
                }
                let (transfer, direction) = match source {
                    state::MacSource::OrdinaryAttempt(binding) => {
                        (binding.transfer, binding.direction)
                    }
                    state::MacSource::MergeExpress {
                        transfer,
                        direction,
                        ..
                    }
                    | state::MacSource::AbstractExpress {
                        transfer,
                        direction,
                        ..
                    } => (transfer, direction),
                };
                if transfer != t
                    || direction != packet.direction
                    || row.sof_ps != Some(proof.sof_ps)
                    || row.planned_eof_ps != Some(proof.eof_planned_ps)
                    || row.planned_arrival_ps != Some(proof.arrival_planned_ps)
                {
                    return Err(gerr("checkpoint actual MAC source timeline differs"));
                }
                if !matches!(source, state::MacSource::OrdinaryAttempt(_)) {
                    let evidence = self.gptp_source_evidence(source, &self.delta())?;
                    if evidence.eof_actual_ps != row.eof_ps
                        || evidence.arrival_actual_ps != row.arrival_ps
                    {
                        return Err(gerr("checkpoint serializer actual EOF/arrival differs"));
                    }
                }
            } else if packet.mac_source.is_some() {
                return Err(gerr("unexpected MAC source"));
            }
            let completed = match (&packet.tx, row.eof_ps) {
                (Some(capture), Some(eof)) => {
                    Some(eof.max(capture.sampled.map_or(capture.at_ps, |p| p.observed_at_ps)))
                }
                _ => None,
            };
            if packet.tx_completed_at_ps != completed {
                return Err(gerr("checkpoint TX completion join differs"));
            }
            if packet.tx_eof != row.eof_ps.is_some() {
                return Err(gerr("checkpoint gPTP EOF differs"));
            }
        }
        for (i, p) in state.ports.iter().enumerate() {
            let Some(p) = p else { continue };
            let node = cfg.ports[i].as_ref().unwrap().node;
            for (actual, generation, interval, label) in [
                (
                    p.pdelay_due_raw,
                    p.pdelay_generation,
                    cfg.pdelay_interval_ps,
                    "Pdelay",
                ),
                (
                    p.announce_due_raw,
                    p.announce_generation,
                    cfg.announce_interval_ps,
                    "Announce",
                ),
                (
                    p.sync_due_raw,
                    p.sync_generation,
                    cfg.sync_interval_ps,
                    "Sync",
                ),
            ] {
                if actual != Some(self.gptp_periodic_deadline(node, generation, interval)?.0) {
                    return Err(gerr(format!("{label} raw LocalClock phase differs")));
                }
            }
            if p.as_capable
                && (!p.link_up
                    || p.rate.is_none()
                    || p.delay.is_none()
                    || p.last_sample.is_none()
                    || p.peer.is_none())
            {
                return Err(gerr("checkpoint asCapable lacks qualified Pdelay evidence"));
            }
            if let Some(pending) = &p.pending {
                if pending.request_transfer >= self.transfers.len()
                    || pending.deadline_ps < horizon
                    || state
                        .packets
                        .get(&pending.request_transfer)
                        .is_none_or(|v| {
                            v.direction != i || !matches!(v.message.body, Body::PdelayReq)
                        })
                {
                    return Err(gerr("checkpoint outstanding Pdelay owner differs"));
                }
                let req = &state.packets[&pending.request_transfer];
                let (deadline_raw, deadline_ps) =
                    self.gptp_periodic_deadline(node, pending.generation, cfg.pdelay_interval_ps)?;
                if pending.generation != p.pdelay_generation
                    || pending.sequence != req.message.sequence
                    || req.link_epoch != p.link_epoch
                    || pending.deadline_raw != deadline_raw
                    || pending.deadline_ps != deadline_ps
                    || pending.t3.is_some()
                    || pending.followup_transfer.is_some()
                {
                    return Err(gerr(
                        "checkpoint outstanding Pdelay deadline/sequence differs",
                    ));
                }
                if let Some(response) = pending.response_transfer {
                    let resp = state
                        .packets
                        .get(&response)
                        .ok_or_else(|| gerr("pending Pdelay response owner absent"))?;
                    let observed = received(resp)?;
                    let Body::PdelayResp {
                        request_receipt,
                        ref requester,
                    } = observed.body
                    else {
                        return Err(gerr("pending Pdelay response type differs"));
                    };
                    if !resp.rx_validated
                        || observed.sequence != pending.sequence
                        || cfg.reverse[resp.direction] != i
                        || req.tx.as_ref().is_none_or(|v| Some(v.raw) != pending.t1)
                        || resp.rx.as_ref().is_none_or(|v| Some(v.raw) != pending.t4)
                        || pending.t2
                            != Some(stamp_with_correction(request_receipt, observed.correction)?)
                        || pending.peer.as_ref() != Some(&observed.source)
                        || requester != &req.message.source
                    {
                        return Err(gerr(
                            "pending Pdelay timestamps differ from received response",
                        ));
                    }
                } else if pending.t1.is_some()
                    || pending.t2.is_some()
                    || pending.t4.is_some()
                    || pending.peer.is_some()
                {
                    return Err(gerr("pending Pdelay has timestamps before response"));
                }
            }
            if let Some(a) = &p.candidate {
                if !p.as_capable
                    || a.deadline_ps < horizon
                    || !state
                        .packets
                        .get(&a.transfer)
                        .is_some_and(|v| v.rx_validated)
                    || a.path.last() != Some(&a.source.clock)
                    || a.path.first() != Some(&a.grandmaster)
                {
                    return Err(gerr("checkpoint Announce candidate lacks validated bytes"));
                }
                let packet = &state.packets[&a.transfer];
                let observed = received(packet)?;
                let Body::Announce(ref announce) = observed.body else {
                    return Err(gerr("candidate is not an Announce packet"));
                };
                let identity = [
                    announce.priority1,
                    announce.clock_class,
                    announce.clock_accuracy,
                    announce.offset_scaled_log_variance.to_be_bytes()[0],
                    announce.offset_scaled_log_variance.to_be_bytes()[1],
                    announce.priority2,
                    announce.grandmaster_identity[0],
                    announce.grandmaster_identity[1],
                    announce.grandmaster_identity[2],
                    announce.grandmaster_identity[3],
                    announce.grandmaster_identity[4],
                    announce.grandmaster_identity[5],
                    announce.grandmaster_identity[6],
                    announce.grandmaster_identity[7],
                ];
                let window = cfg
                    .announce_interval_ps
                    .checked_mul(u64::from(cfg.announce_receipt_timeout))
                    .ok_or_else(|| gerr("Announce deadline overflow"))?;
                let (expected_raw, expected_deadline) =
                    self.gptp_local_deadline(node, a.receipt_ps, window)?;
                if cfg.reverse[packet.direction] != i
                    || observed.source != a.source
                    || observed.sequence != a.sequence
                    || announce.grandmaster_identity != a.grandmaster
                    || announce.steps_removed != a.steps_removed
                    || announce.path_trace != a.path
                    || announce.time_source != a.time_source
                    || identity != a.identity
                    || self.transfers[a.transfer].row.arrival_ps != Some(a.receipt_ps)
                    || a.deadline_raw != expected_raw
                    || a.deadline_ps != expected_deadline
                {
                    return Err(gerr(
                        "candidate differs from decoded Announce or receipt time",
                    ));
                }
            }
        }
        for (i, n) in state.nodes.iter().enumerate() {
            if let Some(parent) = n.parent {
                if cfg
                    .ports
                    .get(parent)
                    .and_then(Option::as_ref)
                    .is_none_or(|p| p.node != i)
                    || state.ports[parent].as_ref().is_none_or(|p| {
                        p.candidate
                            .as_ref()
                            .is_none_or(|a| a.grandmaster != n.grandmaster)
                    })
                {
                    return Err(gerr("checkpoint selected parent disagrees with candidate"));
                }
            }
            if let Some(pair) = &n.sync_pair {
                let window = cfg
                    .sync_interval_ps
                    .checked_mul(u64::from(cfg.sync_receipt_timeout))
                    .ok_or_else(|| gerr("Sync pairing deadline overflow"))?;
                let sync_arrival = self
                    .transfers
                    .get(pair.transfer)
                    .and_then(|t| t.row.arrival_ps)
                    .ok_or_else(|| gerr("pending Sync lacks physical arrival"))?;
                let (expected_raw, expected_ps) =
                    self.gptp_local_deadline(i, sync_arrival, window)?;
                if n.parent != Some(pair.parent)
                    || n.epoch != pair.epoch
                    || pair.deadline_ps < horizon
                    || pair.deadline_raw != expected_raw
                    || pair.deadline_ps != expected_ps
                    || !state.packets.get(&pair.transfer).is_some_and(|p| {
                        p.rx_validated
                            && p.received_message.as_ref().is_some_and(|m| {
                                matches!(m.body, Body::Sync)
                                    && m.sequence == pair.sequence
                                    && m.source == pair.source
                            })
                            && p.rx.as_ref().is_some_and(|rx| rx.raw == pair.rx)
                    })
                {
                    return Err(gerr("checkpoint pending Sync pair lacks causal packet"));
                }
            }
            if let Some(last) = n.last_sync_ps {
                let window = cfg
                    .sync_interval_ps
                    .checked_mul(u64::from(cfg.sync_receipt_timeout))
                    .ok_or_else(|| gerr("Sync receipt deadline overflow"))?;
                let (expected_raw, expected_ps) = self.gptp_local_deadline(i, last, window)?;
                if n.sync_deadline_raw != Some(expected_raw)
                    || n.sync_deadline_ps != Some(expected_ps)
                {
                    return Err(gerr("Sync timeout differs from raw LocalClock receipt"));
                }
            } else if n.sync_deadline_raw.is_some() || n.sync_deadline_ps.is_some() {
                return Err(gerr("Sync timeout without accepted packet"));
            }
            if let Some(relay) = &n.relay {
                if !n.synchronized
                    || n.epoch != relay.epoch
                    || n.grandmaster != relay.gm
                    || !state
                        .packets
                        .get(&relay.sync_transfer)
                        .is_some_and(|p| p.rx_validated)
                    || !state
                        .packets
                        .get(&relay.followup_transfer)
                        .is_some_and(|p| p.rx_validated)
                {
                    return Err(gerr(
                        "checkpoint relay tuple lacks current validated parent",
                    ));
                }
                let correction = state
                    .journal
                    .iter()
                    .rev()
                    .find(|j| j.node == i)
                    .ok_or_else(|| gerr("checkpoint relay tuple has no correction journal"))?;
                let ratio = math::gm_ratio(
                    correction.cumulative_scaled_rate_offset,
                    correction.neighbor_rate,
                )
                .map_err(gerr)?;
                let (upstream, _) = math::sync_target(
                    Rat::new(correction.origin.floor_ps().map_err(gerr)?, 1).unwrap(),
                    correction.correction,
                    correction.delay_neighbor,
                    correction.neighbor_rate,
                    ratio,
                    correction.captured_rx,
                    correction.captured_rx,
                )
                .map_err(gerr)?;
                if correction.epoch != relay.epoch
                    || n.parent != Some(correction.parent)
                    || correction.sync_transfer != relay.sync_transfer
                    || correction.followup_transfer != relay.followup_transfer
                    || correction.origin != relay.origin
                    || correction.correction != relay.correction
                    || relay.upstream_raw != upstream
                    || relay.gm_local_ratio != ratio
                    || n.last_sync_ps != Some(correction.at_ps)
                {
                    return Err(gerr(
                        "checkpoint relay tuple differs from latest decoded correction",
                    ));
                }
            }
        }
        for (i, sample) in state.samples.iter().enumerate() {
            let prior = sample
                .previous
                .map(|j| {
                    state
                        .samples
                        .get(j)
                        .ok_or_else(|| gerr("Pdelay predecessor missing"))
                })
                .transpose()?;
            if sample.direction >= cfg.ports.len()
                || cfg.ports[sample.direction].is_none()
                || sample.previous.is_some_and(|j| j >= i)
                || prior.is_some_and(|v| {
                    v.direction != sample.direction
                        || v.link_epoch != sample.link_epoch
                        || state
                            .packets
                            .get(&v.response_transfer)
                            .and_then(|p| p.received_message.as_ref())
                            .map(|m| &m.source)
                            != state
                                .packets
                                .get(&sample.response_transfer)
                                .and_then(|p| p.received_message.as_ref())
                                .map(|m| &m.source)
                })
            {
                return Err(gerr("Pdelay evidence predecessor or owner differs"));
            }
            let req = state
                .packets
                .get(&sample.request_transfer)
                .ok_or_else(|| gerr("Pdelay request missing"))?;
            let resp = state
                .packets
                .get(&sample.response_transfer)
                .ok_or_else(|| gerr("Pdelay response missing"))?;
            let fup = state
                .packets
                .get(&sample.followup_transfer)
                .ok_or_else(|| gerr("Pdelay followup missing"))?;
            let req_rx = received(req)?;
            let resp_rx = received(resp)?;
            let fup_rx = received(fup)?;
            let ingress = sample.direction;
            if req.direction != ingress
                || resp.direction != cfg.reverse[ingress]
                || fup.direction != cfg.reverse[ingress]
                || req.link_epoch != sample.link_epoch
                || !req.rx_validated
                || !resp.rx_validated
                || !fup.rx_validated
                || !matches!(req_rx.body, Body::PdelayReq)
                || req_rx.sequence != resp_rx.sequence
                || req_rx.sequence != fup_rx.sequence
                || req.message.source != self.gptp_port_identity(ingress)
                || resp_rx.source != self.gptp_port_identity(cfg.reverse[ingress])
                || resp_rx.source != fup_rx.source
                || resp.tx.is_none()
                || fup.tx.is_none()
            {
                return Err(gerr("Pdelay evidence packet identity differs"));
            }
            let Body::PdelayResp {
                request_receipt,
                ref requester,
            } = resp_rx.body
            else {
                return Err(gerr("Pdelay response evidence type differs"));
            };
            let Body::PdelayRespFollowUp {
                response_origin,
                requester: ref fup_requester,
            } = fup_rx.body
            else {
                return Err(gerr("Pdelay followup evidence type differs"));
            };
            let (expected_t2, frac_t2) = timestamp_part(req.rx.as_ref().unwrap().raw)?;
            let (expected_t3, frac_t3) = timestamp_part(resp.tx.as_ref().unwrap().raw)?;
            if requester != &req.message.source
                || fup_requester != &req.message.source
                || req.tx.as_ref().is_none_or(|v| v.raw != sample.t1)
                || request_receipt != expected_t2
                || to_wire_correction(frac_t2)? != resp_rx.correction
                || response_origin != expected_t3
                || to_wire_correction(frac_t3)? != fup_rx.correction
                || resp.rx.as_ref().is_none_or(|v| v.raw != sample.t4)
                || stamp_with_correction(request_receipt, resp_rx.correction)? != sample.t2
                || stamp_with_correction(response_origin, fup_rx.correction)? != sample.t3
            {
                return Err(gerr(
                    "Pdelay evidence timestamp differs from decoded packets",
                ));
            }
            let old = prior.map(|p| (p.t3, p.t4));
            let (calculated, quantization) = if self.phy_attachment_for(sample.direction).is_some()
            {
                match math::pdelay_wire_quantized(old, sample.t1, sample.t2, sample.t3, sample.t4)
                    .map_err(gerr)?
                {
                    Some((rate, delay, proof)) => (Some((rate, delay)), Some(proof)),
                    None => (None, None),
                }
            } else {
                (
                    math::pdelay(old, sample.t1, sample.t2, sample.t3, sample.t4).map_err(gerr)?,
                    None,
                )
            };
            let accepted = calculated.filter(|(_, delay)| {
                delay.within_integer_threshold(cfg.mean_link_delay_threshold_ps)
            });
            if sample.rate != accepted.map(|v| v.0)
                || sample.delay != accepted.map(|v| v.1)
                || sample.quantization != quantization
            {
                return Err(gerr(
                    "Pdelay rate/delay differ from physical packet evidence",
                ));
            }
        }
        for (i, p) in state.ports.iter().enumerate() {
            let Some(p) = p else { continue };
            if let Some(index) = p.last_sample_index {
                let s = state
                    .samples
                    .get(index)
                    .ok_or_else(|| gerr("current Pdelay evidence missing"))?;
                if s.direction != i
                    || s.link_epoch != p.link_epoch
                    || p.last_sample != Some((s.t3, s.t4))
                    || p.last_sample_peer.as_ref()
                        != state
                            .packets
                            .get(&s.response_transfer)
                            .and_then(|p| p.received_message.as_ref())
                            .map(|m| &m.source)
                    || p.rate != s.rate
                    || p.delay != s.delay
                    || p.as_capable != s.rate.is_some()
                {
                    return Err(gerr("current Pdelay state differs from packet evidence"));
                }
            } else if p.last_sample.is_some()
                || p.last_sample_peer.is_some()
                || p.rate.is_some()
                || p.delay.is_some()
                || p.as_capable
            {
                return Err(gerr("current Pdelay state lacks evidence"));
            }
        }
        let mut selection_at = 0;
        let mut selected = cfg
            .nodes
            .iter()
            .map(|n| (0u64, None, n.clock_identity))
            .collect::<Vec<_>>();
        for selection in &state.selections {
            if selection.at_ps < selection_at
                || selection.at_ps > horizon
                || selection.node >= cfg.nodes.len()
            {
                return Err(gerr("selection journal time/node invalid"));
            }
            selection_at = selection.at_ps;
            let previous = selected[selection.node];
            if selection.epoch != add(previous.0, 1)?
                || previous.1 == selection.parent && previous.2 == selection.grandmaster
            {
                return Err(gerr(
                    "selection epoch does not follow an actual parent/GM change",
                ));
            }
            if let Some(parent) = selection.parent {
                let transfer = selection
                    .candidate_transfer
                    .ok_or_else(|| gerr("selection missing Announce owner"))?;
                let packet = state
                    .packets
                    .get(&transfer)
                    .ok_or_else(|| gerr("selection Announce packet absent"))?;
                let observed = received(packet)?;
                let Body::Announce(ref announce) = observed.body else {
                    return Err(gerr("selection owner is not Announce"));
                };
                let received = self
                    .transfers
                    .get(transfer)
                    .and_then(|t| t.row.arrival_ps)
                    .ok_or_else(|| gerr("selection Announce has no arrival"))?;
                let (_, deadline) = self.gptp_local_deadline(
                    selection.node,
                    received,
                    cfg.announce_interval_ps
                        .checked_mul(u64::from(cfg.announce_receipt_timeout))
                        .ok_or_else(|| gerr("selection timeout overflow"))?,
                )?;
                if cfg
                    .ports
                    .get(parent)
                    .and_then(Option::as_ref)
                    .is_none_or(|p| p.node != selection.node)
                    || cfg.reverse[packet.direction] != parent
                    || !packet.rx_validated
                    || received > selection.at_ps
                    || deadline <= selection.at_ps
                    || packet.link_epoch != selection.candidate_link_epoch.unwrap_or(u64::MAX)
                    || announce.grandmaster_identity != selection.grandmaster
                    || announce.path_trace.first() != Some(&selection.grandmaster)
                    || announce.path_trace.last() != Some(&observed.source.clock)
                {
                    return Err(gerr(
                        "selection differs from received Announce/link generation",
                    ));
                }
            } else if selection.candidate_transfer.is_some()
                || selection.candidate_link_epoch.is_some()
                || selection.grandmaster != cfg.nodes[selection.node].clock_identity
            {
                return Err(gerr("self selection has foreign packet/GM"));
            }
            selected[selection.node] = (selection.epoch, selection.parent, selection.grandmaster);
        }
        for (node, current) in state.nodes.iter().enumerate() {
            if selected[node] != (current.epoch, current.parent, current.grandmaster) {
                return Err(gerr("current selection differs from chronological journal"));
            }
        }
        let mut replay = initial.to_vec();
        let mut last = 0;
        let mut used = BTreeSet::new();
        for correction in &state.journal {
            if correction.at_ps < last
                || correction.at_ps > horizon
                || !used.insert(correction.followup_transfer)
                || cfg.nodes.get(correction.node).is_none()
                || correction.parent >= cfg.ports.len()
            {
                return Err(gerr("checkpoint correction journal order/identity"));
            }
            last = correction.at_ps;
            let sync = state
                .packets
                .get(&correction.sync_transfer)
                .ok_or_else(|| gerr("journal Sync packet missing"))?;
            let fup = state
                .packets
                .get(&correction.followup_transfer)
                .ok_or_else(|| gerr("journal FUP packet missing"))?;
            let sync_rx = received(sync)?;
            let fup_rx = received(fup)?;
            if !sync.rx_validated
                || !fup.rx_validated
                || !matches!(sync_rx.body, Body::Sync)
                || sync_rx.sequence != fup_rx.sequence
                || sync_rx.source != fup_rx.source
                || cfg.reverse[sync.direction] != correction.parent
                || cfg.reverse[fup.direction] != correction.parent
                || cfg.ports[correction.parent]
                    .as_ref()
                    .is_none_or(|port| port.node != correction.node)
                || self.transfers[correction.followup_transfer].row.arrival_ps
                    != Some(correction.at_ps)
                || self.transfers[correction.sync_transfer]
                    .row
                    .arrival_ps
                    .is_none_or(|at| at > correction.at_ps)
            {
                return Err(gerr("journal packet identity or FCS validation differs"));
            }
            let node = &state.nodes[correction.node];
            if correction.epoch == 0
                || correction.epoch > node.epoch
                || (correction.epoch == node.epoch && node.parent != Some(correction.parent))
            {
                return Err(gerr("journal parent epoch differs from selected state"));
            }
            let selected_at_correction = state
                .selections
                .iter()
                .rev()
                .find(|s| s.node == correction.node && s.at_ps <= correction.at_ps)
                .ok_or_else(|| gerr("journal has no accepted parent selection"))?;
            if selected_at_correction.epoch != correction.epoch
                || selected_at_correction.parent != Some(correction.parent)
                || selected_at_correction
                    .candidate_transfer
                    .and_then(|t| state.packets.get(&t))
                    .is_none_or(|announce| {
                        announce
                            .received_message
                            .as_ref()
                            .is_none_or(|m| m.source != sync_rx.source)
                    })
            {
                return Err(gerr(
                    "journal epoch/parent differs from accepted Announce prefix",
                ));
            }
            let Body::FollowUp {
                precise_origin,
                ref info,
            } = fup_rx.body
            else {
                return Err(gerr("journal FUP body differs"));
            };
            if precise_origin != correction.origin
                || info.cumulative_scaled_rate_offset != correction.cumulative_scaled_rate_offset
                || sync
                    .rx
                    .as_ref()
                    .is_none_or(|v| v.raw != correction.captured_rx)
            {
                return Err(gerr("journal wire fields or RX capture differ"));
            }
            let expected_correction = from_wire_correction(sync_rx.correction)?
                .add(from_wire_correction(fup_rx.correction)?)
                .map_err(gerr)?;
            let sample = state
                .samples
                .get(correction.pdelay_sample)
                .ok_or_else(|| gerr("journal Pdelay evidence missing"))?;
            if sample.direction != correction.parent
                || sample.rate != Some(correction.neighbor_rate)
                || sample.delay != Some(correction.delay_neighbor)
                || self.transfers[sample.followup_transfer]
                    .row
                    .arrival_ps
                    .is_none_or(|at| at > correction.at_ps)
            {
                return Err(gerr(
                    "journal neighbor rate/delay differ from Pdelay packet evidence",
                ));
            }
            let ratio =
                math::gm_ratio(info.cumulative_scaled_rate_offset, correction.neighbor_rate)
                    .map_err(gerr)?;
            let init =
                &self.prepared.clocks.as_ref().unwrap().initial[cfg.nodes[correction.node].device];
            let now_raw = math::raw(
                init.offset_ps,
                init.rate_num,
                init.rate_den,
                correction.at_ps,
            )
            .map_err(gerr)?;
            let (_, target) = math::sync_target(
                Rat::new(precise_origin.floor_ps().map_err(gerr)?, 1).unwrap(),
                expected_correction,
                correction.delay_neighbor,
                correction.neighbor_rate,
                ratio,
                correction.captured_rx,
                now_raw,
            )
            .map_err(gerr)?;
            let rate = Rat::new(i128::from(init.rate_num), i128::from(init.rate_den))
                .unwrap()
                .mul(ratio)
                .map_err(gerr)?;
            if expected_correction != correction.correction
                || target != correction.target
                || rate != correction.rate
            {
                return Err(gerr("journal target/rate differs from packet arithmetic"));
            }
            let (rate_num, rate_den) = application_rate(rate)?;
            replay[cfg.nodes[correction.node].device]
                .reanchor_and_rate(
                    correction.at_ps,
                    application_target(target)?,
                    rate_num,
                    rate_den,
                )
                .map_err(|e| gerr(e.to_string()))?;
            if replay[cfg.nodes[correction.node].device].generation() != correction.clock_generation
            {
                return Err(gerr("journal generation differs"));
            }
        }
        if self.clocks.as_deref() != Some(replay.as_slice()) {
            return Err(gerr("application clocks differ from packet journal replay"));
        }
        self.replay_gptp_checkpoint(horizon, initial)?;
        Ok(())
    }
    pub(in crate::runtime::network) fn validate_gptp_event(
        &self,
        event: &state::Event,
        at: u64,
    ) -> Result<()> {
        let state = self
            .gptp
            .as_ref()
            .ok_or_else(|| gerr("gPTP event without owner"))?;
        match event {
            state::Event::Capture {
                transfer,
                packet_id,
                link_epoch,
                kind,
            } => {
                let p = state
                    .packets
                    .get(transfer)
                    .ok_or_else(|| gerr("capture packet missing"))?;
                let row = &self.transfers[*transfer].row;
                let sof = row.sof_ps.ok_or_else(|| gerr("capture before SOF"))?;
                let expected = add(
                    sof,
                    reference_duration(self.prepared.ethernet.directions[p.direction].bitrate_bps)?,
                )?;
                let expected = if *kind == CaptureKind::Rx {
                    add(
                        expected,
                        self.prepared.ethernet.directions[p.direction].delay_ps,
                    )?
                } else {
                    expected
                };
                let configured = p
                    .mac_source
                    .map(|binding| self.gptp_source_proof(binding, *kind, &self.delta()))
                    .transpose()?;
                if self.configured_endpoint(p.direction).is_some()
                    != configured.is_some_and(|p| p.endpoint.is_some())
                {
                    return Err(gerr("pending configured capture lacks attempt"));
                }
                let expected = configured.map_or(expected, |proof| proof.reference_ps);
                if p.id != *packet_id
                    || p.link_epoch != *link_epoch
                    || expected != at
                    || if *kind == CaptureKind::Tx {
                        p.tx.is_some()
                    } else {
                        p.rx.is_some()
                    }
                {
                    return Err(gerr("pending capture differs from physical owner"));
                }
            }
            state::Event::Timer(timer) => match timer {
                Timer::Pdelay { direction, .. }
                | Timer::PdelayTimeout { direction, .. }
                | Timer::Announce { direction, .. }
                | Timer::AnnounceTimeout { direction, .. }
                | Timer::Sync { direction, .. } => {
                    if state.ports.get(*direction).is_none_or(Option::is_none) {
                        return Err(gerr("timer port missing"));
                    }
                }
                Timer::SyncTimeout { node, .. } => {
                    if *node >= state.nodes.len() {
                        return Err(gerr("timer node missing"));
                    }
                }
            },
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn validate_gptp_reservations(
        &self,
        events: &[(state::Event, u64)],
    ) -> Result<()> {
        let Some(state) = &self.gptp else {
            if !events.is_empty() {
                return Err(gerr("unexpected gPTP event"));
            }
            return Ok(());
        };
        let cfg = self.gcfg().unwrap();
        for (i, p) in state.ports.iter().enumerate() {
            let Some(p) = p else { continue };
            let node = cfg.ports[i].as_ref().unwrap().node;
            for (expected, raw, interval, generation) in [
                (
                    Timer::Pdelay {
                        direction: i,
                        generation: p.pdelay_generation,
                    },
                    p.pdelay_due_raw,
                    cfg.pdelay_interval_ps,
                    p.pdelay_generation,
                ),
                (
                    Timer::Announce {
                        direction: i,
                        generation: p.announce_generation,
                    },
                    p.announce_due_raw,
                    cfg.announce_interval_ps,
                    p.announce_generation,
                ),
                (
                    Timer::Sync {
                        direction: i,
                        generation: p.sync_generation,
                    },
                    p.sync_due_raw,
                    cfg.sync_interval_ps,
                    p.sync_generation,
                ),
            ] {
                let (expected_raw, expected_at) =
                    self.gptp_periodic_deadline(node, generation, interval)?;
                if raw != Some(expected_raw) {
                    return Err(gerr("periodic raw LocalClock deadline differs"));
                }
                if events
                    .iter()
                    .filter(|(e, at)| *e == state::Event::Timer(expected) && *at == expected_at)
                    .count()
                    != 1
                {
                    return Err(gerr(
                        "periodic gPTP timer reservation missing or duplicated",
                    ));
                }
            }
            if let Some(exchange) = &p.pending {
                if self.gptp_global_deadline(node, exchange.deadline_raw)? != exchange.deadline_ps {
                    return Err(gerr("Pdelay reservation raw deadline differs"));
                }
                if events
                    .iter()
                    .filter(|(e, at)| {
                        *e == state::Event::Timer(Timer::PdelayTimeout {
                            direction: i,
                            generation: exchange.generation,
                        }) && *at == exchange.deadline_ps
                    })
                    .count()
                    != 1
                {
                    return Err(gerr("Pdelay timeout reservation missing or duplicated"));
                }
            }
            if let Some(a) = &p.candidate {
                if self.gptp_global_deadline(node, a.deadline_raw)? != a.deadline_ps {
                    return Err(gerr("Announce reservation raw deadline differs"));
                }
                if events
                    .iter()
                    .filter(|(e, at)| {
                        *e == state::Event::Timer(Timer::AnnounceTimeout {
                            direction: i,
                            generation: a.generation,
                        }) && *at == a.deadline_ps
                    })
                    .count()
                    != 1
                {
                    return Err(gerr("Announce timeout reservation missing or duplicated"));
                }
            }
        }
        for (node, state_node) in state.nodes.iter().enumerate() {
            if let (Some(raw), Some(at)) =
                (state_node.sync_deadline_raw, state_node.sync_deadline_ps)
            {
                if self.gptp_global_deadline(node, raw)? != at
                    || events
                        .iter()
                        .filter(|(e, event_at)| {
                            *e == state::Event::Timer(Timer::SyncTimeout {
                                node,
                                generation: state_node.sync_timeout_generation,
                            }) && *event_at == at
                        })
                        .count()
                        != 1
                {
                    return Err(gerr("Sync timeout reservation differs from raw deadline"));
                }
            }
        }
        for (&t, p) in &state.packets {
            let Some(sof) = self.transfers[t].row.sof_ps else {
                continue;
            };
            let tx = add(
                sof,
                reference_duration(self.prepared.ethernet.directions[p.direction].bitrate_bps)?,
            )?;
            let rx = add(tx, self.prepared.ethernet.directions[p.direction].delay_ps)?;
            let (tx, rx) = if let Some(binding) = p.mac_source {
                (
                    self.gptp_source_proof(binding, CaptureKind::Tx, &self.delta())?
                        .reference_ps,
                    self.gptp_source_proof(binding, CaptureKind::Rx, &self.delta())?
                        .reference_ps,
                )
            } else {
                (tx, rx)
            };
            for (kind, captured, at) in [
                (CaptureKind::Tx, p.tx.is_some(), tx),
                (CaptureKind::Rx, p.rx.is_some(), rx),
            ] {
                if !captured
                    && p.link_epoch == state.ports[p.direction].as_ref().unwrap().link_epoch
                    && events
                        .iter()
                        .filter(|(e, time)| {
                            *time == at
                                && *e
                                    == state::Event::Capture {
                                        transfer: t,
                                        packet_id: p.id,
                                        link_epoch: p.link_epoch,
                                        kind,
                                    }
                        })
                        .count()
                        != 1
                {
                    return Err(gerr("physical capture reservation missing or duplicated"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod configured_fault_checkpoint_tests {
    use super::*;
    #[test]
    fn private_configured_checkpoint_accepts_actual_stale_ingress_after_fault_and_restart() {
        use std::{
            fs,
            path::Path,
            sync::Arc,
            time::{SystemTime, UNIX_EPOCH},
        };
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .canonicalize()
            .unwrap();
        for (down_at, up_at) in [
            (800_000u64, 20_000_000u64),
            (8_000_000, 20_000_000),
            (800_000, 800_000),
            (8_000_000, 8_000_000),
        ] {
            let path = std::env::temp_dir().join(format!(
                "configured-native-fault-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            let mut m: Value = serde_json::from_slice(
                &fs::read(root.join("network-composite/gptp-configured/model.json")).unwrap(),
            )
            .unwrap();
            let l = &mut m["configured_media"]["physical_links"][0];
            l["a_phy"]["tx_latency_ps"] = json!("2000000");
            l["b_phy"]["tx_latency_ps"] = json!("3000000");
            l["a_phy"]["rx_latency_ps"] = json!("4000000");
            l["b_phy"]["rx_latency_ps"] = json!("5000000");
            fs::write(path.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
            fs::write(path.join("workload.json"),serde_json::to_vec(&json!({"schema_version":4,"generators":[],"controls":[{"id":"down","at_ps":down_at.to_string(),"kind":"link_set","link":"a-b","up":false},{"id":"up","at_ps":up_at.to_string(),"kind":"link_set","link":"a-b","up":true}]})).unwrap()).unwrap();
            fs::write(path.join("model.ini"),format!("[General]\nnetwork = ethtsn.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 30000000ps\n",root.join("gptp/models/two").display())).unwrap();
            let prepared = crate::prepare(&path.join("model.ini")).unwrap();
            let mut engine =
                crate::runtime::registered::Engine::new(Arc::new(prepared.clone())).unwrap();
            engine.initialize().unwrap();
            while engine.step() {}
            assert!(
                !engine.snapshot.common.partial,
                "{:?}",
                engine.snapshot.common.diagnostics
            );
            let backend = engine.checkpoint_network().unwrap();
            let horizon = engine.snapshot.common.last_event_time_ps.unwrap();
            let restored = NetworkState::restore_checkpoint(
                prepared
                    .registered
                    .as_ref()
                    .unwrap()
                    .network
                    .as_ref()
                    .unwrap()
                    .clone(),
                &backend["state"],
                horizon,
            );
            assert!(
                restored.is_ok(),
                "legitimate native stale ingress rejected: {:?}",
                restored.err()
            );
            let mut forged_body = backend["state"].clone();
            let packets = forged_body["gptp"]["packets"].as_object_mut().unwrap();
            let packet = packets
                .values_mut()
                .find(|p| p["received_mac_hex"].is_string() && p["received_message"].is_null())
                .unwrap();
            packet["received_message"] = packet["message"].clone();
            assert!(
                NetworkState::restore_checkpoint(
                    prepared
                        .registered
                        .as_ref()
                        .unwrap()
                        .network
                        .as_ref()
                        .unwrap()
                        .clone(),
                    &forged_body,
                    horizon
                )
                .is_err(),
                "stale receive acquired a decoded protocol body"
            );
            assert_eq!(
                engine.checkpoint_network().unwrap(),
                backend,
                "failed private validation changed live state"
            );
            if down_at == 8_000_000 {
                assert!(
                    backend["state"]["gptp"]["packets"]
                        .as_object()
                        .unwrap()
                        .values()
                        .any(|p| p["rx_validated"] == true && p["received_message"].is_object()),
                    "no accepted historical body before later fault"
                );
            }
            fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn private_configured_checkpoint_rejects_missing_accepted_body_and_forged_staleness() {
        use std::{path::Path, sync::Arc};
        let p = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/gptp-configured/model.ini"),
        )
        .unwrap();
        let cfg = p
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .clone();
        let mut engine = crate::runtime::registered::Engine::new(Arc::new(p)).unwrap();
        engine.initialize().unwrap();
        let backend = loop {
            assert!(engine.step());
            let backend = engine.checkpoint_network().unwrap();
            if backend["state"]["gptp"]["packets"]
                .as_object()
                .unwrap()
                .values()
                .any(|p| p["rx_validated"] == true)
            {
                break backend;
            }
        };
        let horizon = engine.snapshot.common.last_event_time_ps.unwrap();
        NetworkState::restore_checkpoint(cfg.clone(), &backend["state"], horizon).unwrap();
        let (key, packet) = backend["state"]["gptp"]["packets"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, p)| p["rx_validated"] == true)
            .unwrap();
        let transfer = packet["transfer"].as_u64().unwrap();
        let direction = packet["direction"].as_u64().unwrap() as usize;
        let mut missing = backend["state"].clone();
        missing["gptp"]["packets"][key]["received_message"] = Value::Null;
        assert!(
            NetworkState::restore_checkpoint(cfg.clone(), &missing, horizon).is_err(),
            "valid accepted body was silently removed"
        );
        let mut forged = missing;
        forged["gptp"]["packets"][key]["rx_validated"] = json!(false);
        let causes = forged["gptp"]["causes"].as_array_mut().unwrap();
        let index = causes
            .iter()
            .position(|c| c["kind"]["Receive"]["transfer"].as_u64() == Some(transfer))
            .unwrap();
        let at = causes[index]["at_ps"].clone();
        causes.insert(
            index,
            json!({"at_ps":at,"kind":{"Link":{"direction":direction,"up":false}}}),
        );
        causes.insert(
            index + 2,
            json!({"at_ps":at,"kind":{"Link":{"direction":direction,"up":true}}}),
        );
        let error = NetworkState::restore_checkpoint(cfg, &forged, horizon)
            .err()
            .unwrap();
        assert!(
            error.message.contains("causal link"),
            "forged receive disposition escaped independent link-source replay: {error:?}"
        );
        assert_eq!(
            engine.checkpoint_network().unwrap(),
            backend,
            "failed validation changed installed state"
        );
    }
}
