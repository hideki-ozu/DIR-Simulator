use super::super::*;
use super::codec::{self, Body, Message};
use super::integration::{
    application_rate, application_target, from_wire_correction, gerr, hex, stamp_with_correction,
    timestamp_part,
};
use super::math::{self, Rat};
use super::state::{self, CauseKind};

pub(super) fn bytes(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return Err(gerr("odd MAC hex length"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| gerr("invalid MAC hex")))
        .collect()
}
pub(super) fn validate_mac(data: &[u8]) -> std::result::Result<&[u8], &'static str> {
    if data.len() < 64 {
        return Err("truncated_mac");
    }
    if data[..6] != codec::MULTICAST || data[12..14] != codec::ETHERTYPE.to_be_bytes() {
        return Err("wrong_mac_destination_or_ethertype");
    }
    if data[data.len() - 4..]
        != crate::runtime::ethernet::crc32(&data[..data.len() - 4]).to_le_bytes()
    {
        return Err("bad_fcs");
    }
    Ok(&data[14..data.len() - 4])
}

impl NetworkState {
    pub(in crate::runtime::network) fn gptp_arrival(
        &self,
        t: usize,
        _receiver: Option<usize>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let wire = self.transfer(d, t).row.wire.mac_hex.clone();
        let data = bytes(&wire)?;
        self.gptp_receive(t, None, None, &data, d, c)
    }
    pub(in crate::runtime::network) fn gptp_rx_validated(
        &self,
        t: usize,
        id: u64,
        epoch: u64,
        data: &[u8],
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.gptp_receive(t, Some(id), Some(epoch), data, d, c)
    }
    pub(in crate::runtime::network) fn gptp_phy_rx_validated(
        &self,
        t: usize,
        attempt: u32,
        data: &[u8],
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.gptp_receive(t, None, Some(u64::from(attempt)), data, d, c)
    }
    fn gptp_reject(
        &self,
        t: usize,
        reason: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.gptp_record("packet_rejected", Some(t), Some(reason), json!({}), d, c)
    }
    fn gptp_receive(
        &self,
        t: usize,
        mac_id: Option<u64>,
        epoch: Option<u64>,
        data: &[u8],
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let packet = self
            .gs(d)
            .and_then(|s| s.packets.get(&t))
            .ok_or_else(|| gerr("validated ingress without gPTP packet"))?
            .clone();
        if packet.mac_packet != mac_id || epoch.is_some_and(|e| packet.physical_epoch != Some(e)) {
            return Err(gerr("validated ingress MAC packet owner differs"));
        }
        if let Some(source) = packet.mac_source {
            let proof = self.gptp_source_proof(source, state::CaptureKind::Rx, d)?;
            match source {
                state::MacSource::OrdinaryAttempt(binding) => {
                    let a = &self.media_state(d).attempts[binding.index];
                    if a.status != "serialized" {
                        return Err(gerr("configured receive lacks actual serialized attempt"));
                    }
                }
                _ => {
                    let evidence = self.gptp_source_evidence(source, d)?;
                    if evidence.source != source
                        || evidence.eof_actual_ps != Some(proof.eof_planned_ps)
                        || evidence.arrival_actual_ps != Some(c.now())
                    {
                        return Err(gerr("express receive lacks actual serializer ingress"));
                    }
                }
            }
            let capture = packet.rx.as_ref();
            if c.now() != proof.arrival_planned_ps
                || !packet.tx_eof
                || capture.is_some_and(|c| c.configured.or(c.express) != Some(proof))
            {
                return Err(gerr(
                    "MAC source receive differs from actual serialized timeline",
                ));
            }
        } else if self.configured_endpoint(packet.direction).is_some() {
            return Err(gerr("configured receive lacks actual source"));
        }
        let cfg = self.gcfg().unwrap();
        let reverse = cfg.reverse[packet.direction];
        let link = &self.prepared.ethernet.directions[packet.direction];
        let current = self.gs(d).unwrap().ports[packet.direction]
            .as_ref()
            .unwrap();
        let cause = if current.link_epoch != packet.link_epoch || !current.link_up {
            Some("stale_link_epoch")
        } else if !packet.tx_eof {
            Some("missing_actual_eof")
        } else if packet.rx.is_none() {
            Some("missing_rx_reference_capture")
        } else {
            None
        };
        let (message, reason) = if let Some(reason) = cause {
            (None, Some(reason))
        } else {
            match validate_mac(data) {
                Ok(payload) => match Message::decode(payload) {
                    Ok(m) => (Some(m), None),
                    Err(e) => (None, Some(e)),
                },
                Err(e) => (None, Some(e)),
            }
        };
        {
            let saved = self.gm(d)?.packets.get_mut(&t).unwrap();
            saved.received_mac_hex = Some(hex(data));
            saved.received_message = message.clone();
        }
        let mut transfer = self.transfer(d, t).clone();
        transfer.row.time_ps = c.now();
        transfer.row.arrival_ps = (mac_id.is_some() || reason.is_none()).then_some(c.now());
        if reason.is_some() {
            transfer.row.status = "dropped".into();
            transfer.row.drop_reason = reason.map(str::to_string);
        }
        put(&mut d.transfers, t, transfer.clone());
        let r = self.receptions.len()
            + d.receptions
                .iter()
                .filter(|(i, _)| *i >= self.receptions.len())
                .count();
        let row = EthernetReceptionRecord {
            reception_id: format!("{}@rx", transfer.row.transfer_id),
            time_ps: c.now(),
            frame_id: transfer.row.frame_id.clone(),
            transfer_id: transfer.row.transfer_id.clone(),
            device: self.prepared.ethernet.devices[link.destination].id.clone(),
            ingress: link.to_port.clone(),
            vlan_id: None,
            priority: cfg.priority,
            observed_ps: c.now(),
            ready_ps: Some(c.now()),
            planned_ready_ps: Some(c.now()),
            status: if reason.is_none() {
                "delivered"
            } else {
                "filtered"
            }
            .into(),
            reason: reason.map(str::to_string),
            egress_transfer_ids: vec![],
        };
        d.receptions.push((
            r,
            Reception {
                row,
                transfer: t,
                visit: 0,
                epoch: self.epoch(),
                ingress_no_relay_reason: None,
                segment_receiver: None,
            },
        ));
        self.gptp_cause(CauseKind::Receive { transfer: t }, d, c)?;
        if let Some(reason) = reason {
            self.gptp_reject(t, reason, d, c)?;
            return Ok(());
        }
        let message = message.unwrap();
        if message.source != packet.message.source
            || message.sequence != packet.message.sequence
            || message.body.message_type() != packet.message.body.message_type()
        {
            self.gptp_reject(t, "wire_header_does_not_match_physical_owner", d, c)?;
            return Ok(());
        }
        self.gm(d)?.packets.get_mut(&t).unwrap().rx_validated = true;
        let capture = packet.rx.as_ref().unwrap();
        self.gptp_record(
            "packet_received",
            Some(t),
            None,
            json!({"ingress":link.to_port,
            "rx_reference_ps":capture.at_ps.to_string(),"raw_numerator":capture.raw.num.to_string(),
            "raw_denominator":capture.raw.den.to_string(),"received_hex":hex(data)}),
            d,
            c,
        )?;
        if let Some(reason) = self.gptp_process(reverse, t, message, capture.raw, d, c)? {
            self.gptp_reject(t, reason, d, c)?;
        }
        Ok(())
    }
    fn gptp_process(
        &self,
        ingress: usize,
        t: usize,
        m: Message,
        rx_raw: Rat,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<Option<&'static str>> {
        let cfg = self.gcfg().unwrap();
        let port = cfg.ports[ingress].as_ref().unwrap();
        let node = port.node;
        let own = self.gptp_port_identity(ingress);
        if m.source.clock == own.clock {
            if matches!(
                m.body,
                Body::PdelayResp { .. } | Body::PdelayRespFollowUp { .. }
            ) {
                if let Some(generation) = self.gs(d).unwrap().ports[ingress]
                    .as_ref()
                    .unwrap()
                    .pending
                    .as_ref()
                    .map(|p| p.generation)
                {
                    self.gptp_pdelay_reset(ingress, generation, "self_source_identity", d, c)?;
                }
            }
            return Ok(Some("self_source_identity"));
        }
        match m.body {
            Body::PdelayReq => {
                let (stamp, fraction) = timestamp_part(rx_raw)?;
                let response = Message {
                    source: own,
                    sequence: m.sequence,
                    correction: super::integration::to_wire_correction(fraction)?,
                    log_interval: 127,
                    body: Body::PdelayResp {
                        request_receipt: stamp,
                        requester: m.source,
                    },
                };
                self.gptp_send(ingress, response, None, d, c)?;
            }
            Body::PdelayResp {
                request_receipt,
                requester,
            } => {
                if requester != own {
                    if let Some(generation) = self.gs(d).unwrap().ports[ingress]
                        .as_ref()
                        .unwrap()
                        .pending
                        .as_ref()
                        .map(|p| p.generation)
                    {
                        self.gptp_pdelay_reset(
                            ingress,
                            generation,
                            "wrong_requesting_port_identity",
                            d,
                            c,
                        )?;
                    }
                    return Ok(Some("wrong_requesting_port_identity"));
                }
                let current = self.gs(d).unwrap().ports[ingress].as_ref().unwrap();
                let Some(pending) = current.pending.as_ref().cloned() else {
                    return Ok(Some("stale_pdelay_response"));
                };
                if pending.sequence != m.sequence || pending.deadline_ps <= c.now() {
                    if pending.sequence != m.sequence {
                        self.gptp_pdelay_reset(
                            ingress,
                            pending.generation,
                            "wrong_pdelay_sequence",
                            d,
                            c,
                        )?;
                    }
                    return Ok(Some("wrong_pdelay_sequence_or_expired"));
                }
                if pending.response_transfer.is_some() {
                    self.gptp_pdelay_reset(
                        ingress,
                        pending.generation,
                        "duplicate_pdelay_response",
                        d,
                        c,
                    )?;
                    return Ok(Some("duplicate_pdelay_response"));
                }
                let req = self
                    .gs(d)
                    .unwrap()
                    .packets
                    .get(&pending.request_transfer)
                    .unwrap();
                let Some(tx_raw) = req.tx.as_ref().map(|v| v.raw) else {
                    return Ok(Some("request_tx_capture_missing"));
                };
                let t2 = stamp_with_correction(request_receipt, m.correction)?;
                let changed_peer = current
                    .last_sample_peer
                    .as_ref()
                    .is_some_and(|old| old != &m.source);
                if changed_peer {
                    let port = self.gm(d)?.ports[ingress].as_mut().unwrap();
                    port.last_sample = None;
                    port.last_sample_peer = None;
                    port.last_sample_index = None;
                    port.rate = None;
                    port.delay = None;
                    port.as_capable = false;
                    port.candidate = None;
                }
                let p = self.gm(d)?.ports[ingress]
                    .as_mut()
                    .unwrap()
                    .pending
                    .as_mut()
                    .unwrap();
                p.t1 = Some(tx_raw);
                p.t2 = Some(t2);
                p.t4 = Some(rx_raw);
                p.peer = Some(m.source);
                p.response_transfer = Some(t);
                self.gptp_record("pdelay_response",Some(t),None,json!({"t1_num":tx_raw.num.to_string(),"t2_num":t2.num.to_string(),
                    "t4_num":rx_raw.num.to_string(),"request_transfer_id":self.transfer(d,pending.request_transfer).row.transfer_id}),d,c)?;
                if changed_peer {
                    self.gptp_reselect(d, c)?;
                }
            }
            Body::PdelayRespFollowUp {
                response_origin,
                requester,
            } => {
                if requester != own {
                    if let Some(generation) = self.gs(d).unwrap().ports[ingress]
                        .as_ref()
                        .unwrap()
                        .pending
                        .as_ref()
                        .map(|p| p.generation)
                    {
                        self.gptp_pdelay_reset(
                            ingress,
                            generation,
                            "wrong_requesting_port_identity",
                            d,
                            c,
                        )?;
                    }
                    return Ok(Some("wrong_requesting_port_identity"));
                }
                let p = self.gs(d).unwrap().ports[ingress].as_ref().unwrap();
                let Some(exchange) = p.pending.as_ref() else {
                    return Ok(Some("stale_pdelay_followup"));
                };
                if exchange.sequence != m.sequence || exchange.deadline_ps <= c.now() {
                    if exchange.sequence != m.sequence {
                        self.gptp_pdelay_reset(
                            ingress,
                            exchange.generation,
                            "wrong_pdelay_sequence",
                            d,
                            c,
                        )?;
                    }
                    return Ok(Some("wrong_pdelay_sequence_or_expired"));
                }
                if exchange.response_transfer.is_none() {
                    return Ok(Some("followup_before_response"));
                }
                if exchange.followup_transfer.is_some() {
                    return Ok(Some("duplicate_pdelay_followup"));
                }
                if exchange.peer.as_ref() != Some(&m.source) {
                    self.gptp_pdelay_reset(
                        ingress,
                        exchange.generation,
                        "wrong_pdelay_responder",
                        d,
                        c,
                    )?;
                    return Ok(Some("wrong_pdelay_responder"));
                }
                let t1 = exchange.t1.unwrap();
                let t2 = exchange.t2.unwrap();
                let t4 = exchange.t4.unwrap();
                let t3 = stamp_with_correction(response_origin, m.correction)?;
                let same_peer = p.last_sample_peer.as_ref() == Some(&m.source);
                let old = same_peer.then_some(p.last_sample).flatten();
                let previous = same_peer.then_some(p.last_sample_index).flatten();
                let link_epoch = p.link_epoch;
                let request_transfer = exchange.request_transfer;
                let response_transfer = exchange.response_transfer.unwrap();
                let calculated = if let Some(attachment) = self.phy_attachment_for(ingress) {
                    // Only captures from the prepared certified sampled owner
                    // select the floored-wire interpretation. Remote exact raw
                    // values never enter this protocol calculation.
                    for capture in [
                        self.gs(d).unwrap().packets[&request_transfer].tx.as_ref(),
                        self.gs(d).unwrap().packets[&response_transfer].rx.as_ref(),
                        self.gs(d).unwrap().packets[&t].rx.as_ref(),
                    ] {
                        if capture
                            .and_then(|capture| capture.sampled)
                            .is_none_or(|provenance| provenance.attachment != attachment)
                        {
                            return Err(gerr("sampled Pdelay lacks physical timestamp provenance"));
                        }
                    }
                    math::pdelay_wire_quantized(old, t1, t2, t3, t4).map(|value| match value {
                        Some((rate, delay, proof)) => (Some((rate, delay)), Some(proof)),
                        None => (None, None),
                    })
                } else {
                    math::pdelay(old, t1, t2, t3, t4).map(|value| (value, None))
                };
                let (next, quantization) = match calculated {
                    Ok(v) => v,
                    Err(reason) => {
                        self.gptp_record("pdelay_invalid", Some(t), Some(reason), json!({}), d, c)?;
                        let port = self.gm(d)?.ports[ingress].as_mut().unwrap();
                        port.pending = None;
                        port.last_sample = None;
                        port.last_sample_peer = None;
                        port.last_sample_index = None;
                        port.rate = None;
                        port.delay = None;
                        port.as_capable = false;
                        port.candidate = None;
                        self.gptp_reselect(d, c)?;
                        return Ok(None);
                    }
                };
                let state = self.gm(d)?;
                if state.samples.len() >= cfg.max_journal_entries {
                    return Err(gerr("Pdelay evidence journal limit exceeded"));
                }
                let sample_index = state.samples.len();
                let accepted = next.filter(|(_, delay)| {
                    delay.within_integer_threshold(cfg.mean_link_delay_threshold_ps)
                });
                state.samples.push(state::PdelaySample {
                    direction: ingress,
                    link_epoch,
                    previous,
                    request_transfer,
                    response_transfer,
                    followup_transfer: t,
                    t1,
                    t2,
                    t3,
                    t4,
                    rate: accepted.map(|v| v.0),
                    delay: accepted.map(|v| v.1),
                    quantization,
                });
                let p = state.ports[ingress].as_mut().unwrap();
                p.pending = None;
                p.last_sample = Some((t3, t4));
                p.last_sample_peer = Some(m.source.clone());
                p.last_sample_index = Some(sample_index);
                p.lost_responses = 0;
                p.peer = Some(m.source.clone());
                if let Some((r, delay)) = next {
                    if !delay.within_integer_threshold(cfg.mean_link_delay_threshold_ps) {
                        p.as_capable = false;
                        p.rate = None;
                        p.delay = None;
                        p.candidate = None;
                    } else {
                        p.rate = Some(r);
                        p.delay = Some(delay);
                        p.as_capable = true;
                    }
                }
                let p = self.gs(d).unwrap().ports[ingress].as_ref().unwrap();
                let mut details = json!({"port":self.prepared.ethernet.directions[ingress].from_port,
                    "t3_numerator":t3.num.to_string(),"t3_denominator":t3.den.to_string(),
                    "as_capable":p.as_capable,"neighbor_rate":p.rate.map(|r|json!({"num":r.num.to_string(),"den":r.den.to_string()})),
                    "mean_link_delay":p.delay.map(|r|json!({"num":r.num.to_string(),"den":r.den.to_string()}))});
                if let Some(proof) = quantization {
                    details["wire_quantization"] = json!(proof);
                }
                self.gptp_record("pdelay_complete", Some(t), None, details, d, c)?;
                self.gptp_reselect(d, c)?;
            }
            Body::Announce(a) => {
                let p = self.gs(d).unwrap().ports[ingress].as_ref().unwrap();
                if !p.as_capable {
                    return Ok(Some("announce_before_as_capable"));
                }
                if p.peer.as_ref() != Some(&m.source) {
                    return Ok(Some("announce_peer_identity_mismatch"));
                }
                if a.path_trace.first() != Some(&a.grandmaster_identity)
                    || a.path_trace.last() != Some(&m.source.clock)
                    || a.path_trace.contains(&own.clock)
                    || a.steps_removed >= 255
                {
                    return Ok(Some("announce_path_trace_invalid_or_loop"));
                }
                let identity = [
                    a.priority1,
                    a.clock_class,
                    a.clock_accuracy,
                    a.offset_scaled_log_variance.to_be_bytes()[0],
                    a.offset_scaled_log_variance.to_be_bytes()[1],
                    a.priority2,
                    a.grandmaster_identity[0],
                    a.grandmaster_identity[1],
                    a.grandmaster_identity[2],
                    a.grandmaster_identity[3],
                    a.grandmaster_identity[4],
                    a.grandmaster_identity[5],
                    a.grandmaster_identity[6],
                    a.grandmaster_identity[7],
                ];
                let generation = p
                    .candidate
                    .as_ref()
                    .map_or(1, |v| v.generation.saturating_add(1));
                let window = cfg
                    .announce_interval_ps
                    .checked_mul(u64::from(cfg.announce_receipt_timeout))
                    .ok_or_else(|| gerr("Announce timeout overflow"))?;
                let (deadline_raw, deadline) = self.gptp_local_deadline(node, c.now(), window)?;
                self.gm(d)?.ports[ingress].as_mut().unwrap().candidate = Some(state::Candidate {
                    source: m.source.clone(),
                    grandmaster: a.grandmaster_identity,
                    identity,
                    steps_removed: a.steps_removed,
                    path: a.path_trace,
                    sequence: m.sequence,
                    transfer: t,
                    receipt_ps: c.now(),
                    deadline_raw,
                    deadline_ps: deadline,
                    generation,
                    time_source: a.time_source,
                });
                Self::schedule(
                    c,
                    d,
                    deadline,
                    Event::Gptp(state::Event::Timer(state::Timer::AnnounceTimeout {
                        direction: ingress,
                        generation,
                    })),
                )?;
                self.gptp_record("announce_qualified",Some(t),None,json!({"grandmaster":hex(&a.grandmaster_identity),
                    "steps_removed":a.steps_removed.to_string(),"deadline_ps":deadline.to_string()}),d,c)?;
                self.gptp_reselect(d, c)?;
            }
            Body::Sync => {
                let n = self.gs(d).unwrap().nodes[node].clone();
                if n.parent != Some(ingress) {
                    return Ok(Some("sync_not_selected_parent"));
                }
                let p = self.gs(d).unwrap().ports[ingress].as_ref().unwrap();
                if !p.as_capable || p.peer.as_ref() != Some(&m.source) {
                    return Ok(Some("sync_peer_not_qualified"));
                }
                if p.candidate.as_ref().is_none_or(|v| v.source != m.source) {
                    return Ok(Some("sync_source_not_selected_announce"));
                }
                if n.sync_pair.is_none()
                    && self.gs(d).unwrap().pending_exchange_count() >= cfg.max_pending_exchanges
                {
                    return Ok(Some("pending_exchange_limit"));
                }
                let window = cfg
                    .sync_interval_ps
                    .checked_mul(u64::from(cfg.sync_receipt_timeout))
                    .ok_or_else(|| gerr("Sync pairing timeout overflow"))?;
                let (deadline_raw, deadline) = self.gptp_local_deadline(node, c.now(), window)?;
                self.gm(d)?.nodes[node].sync_pair = Some(state::SyncPair {
                    parent: ingress,
                    source: m.source,
                    sequence: m.sequence,
                    transfer: t,
                    rx: rx_raw,
                    epoch: n.epoch,
                    deadline_raw,
                    deadline_ps: deadline,
                });
                self.gptp_record(
                    "sync_rx_waiting_followup",
                    Some(t),
                    None,
                    json!({"deadline_ps":deadline.to_string(),"epoch":n.epoch.to_string()}),
                    d,
                    c,
                )?;
            }
            Body::FollowUp {
                precise_origin,
                info,
            } => {
                let n = self.gs(d).unwrap().nodes[node].clone();
                if n.parent != Some(ingress) {
                    return Ok(Some("followup_not_selected_parent"));
                }
                let Some(pair) = n.sync_pair else {
                    return Ok(Some("followup_without_sync"));
                };
                if pair.epoch != n.epoch
                    || pair.parent != ingress
                    || pair.sequence != m.sequence
                    || pair.source != m.source
                {
                    return Ok(Some("followup_pair_identity_or_sequence_mismatch"));
                }
                if pair.deadline_ps <= c.now() {
                    return Ok(Some("followup_after_pair_expiry"));
                }
                let p = self.gs(d).unwrap().ports[ingress].as_ref().unwrap();
                let (Some(r), Some(delay), Some(sample_index)) =
                    (p.rate, p.delay, p.last_sample_index)
                else {
                    return Ok(Some("followup_without_peer_delay"));
                };
                let ratio = math::gm_ratio(info.cumulative_scaled_rate_offset, r).map_err(gerr)?;
                let sync = self
                    .gs(d)
                    .unwrap()
                    .packets
                    .get(&pair.transfer)
                    .ok_or_else(|| gerr("paired Sync packet missing"))?;
                let sync_message = sync
                    .received_message
                    .as_ref()
                    .ok_or_else(|| gerr("paired Sync has no validated received body"))?;
                let correction = from_wire_correction(sync_message.correction)?
                    .add(from_wire_correction(m.correction)?)
                    .map_err(gerr)?;
                let origin = Rat::new(precise_origin.floor_ps().map_err(gerr)?, 1).unwrap();
                let now_raw = self.gptp_raw(node, c.now())?;
                let (upstream, target) =
                    math::sync_target(origin, correction, delay, r, ratio, pair.rx, now_raw)
                        .map_err(gerr)?;
                let initial =
                    &self.prepared.clocks.as_ref().unwrap().initial[cfg.nodes[node].device];
                let own_rate =
                    Rat::new(i128::from(initial.rate_num), i128::from(initial.rate_den)).unwrap();
                let rate = own_rate.mul(ratio).map_err(gerr)?;
                let app_target = application_target(target)?;
                let (rate_num, rate_den) = application_rate(rate)?;
                self.before_clock_change(cfg.nodes[node].device, d, c)?;
                let clocks = d
                    .clocks
                    .get_or_insert_with(|| self.clocks.as_ref().unwrap().clone());
                let clock = &mut clocks[cfg.nodes[node].device];
                clock
                    .reanchor_and_rate(c.now(), app_target, rate_num, rate_den)
                    .map_err(|e| gerr(e.to_string()))?;
                let generation = clock.generation();
                let changed = clock.clone();
                let window = cfg
                    .sync_interval_ps
                    .checked_mul(u64::from(cfg.sync_receipt_timeout))
                    .ok_or_else(|| gerr("Sync timeout overflow"))?;
                let (deadline_raw, deadline) = self.gptp_local_deadline(node, c.now(), window)?;
                let (timeout_generation, gm_identity, sync_epoch) = {
                    let state = self.gm(d)?;
                    if state.journal.len() >= cfg.max_journal_entries {
                        return Err(gerr("clock correction journal limit exceeded"));
                    }
                    let n = &mut state.nodes[node];
                    n.synchronized = true;
                    n.sync_pair = None;
                    n.last_sync_ps = Some(c.now());
                    n.relay = Some(state::Relay {
                        origin: precise_origin,
                        correction,
                        upstream_raw: upstream,
                        gm_local_ratio: ratio,
                        epoch: n.epoch,
                        gm: n.grandmaster,
                        sync_transfer: pair.transfer,
                        followup_transfer: t,
                    });
                    n.sync_timeout_generation = add(n.sync_timeout_generation, 1)?;
                    n.sync_deadline_ps = Some(deadline);
                    n.sync_deadline_raw = Some(deadline_raw);
                    let result = (n.sync_timeout_generation, n.grandmaster, n.epoch);
                    state.journal.push(state::Correction {
                        node,
                        at_ps: c.now(),
                        target,
                        rate,
                        sync_transfer: pair.transfer,
                        followup_transfer: t,
                        parent: ingress,
                        epoch: result.2,
                        captured_rx: pair.rx,
                        delay_neighbor: delay,
                        neighbor_rate: r,
                        pdelay_sample: sample_index,
                        origin: precise_origin,
                        correction,
                        cumulative_scaled_rate_offset: info.cumulative_scaled_rate_offset,
                        clock_generation: generation,
                    });
                    result
                };
                Self::schedule(
                    c,
                    d,
                    deadline,
                    Event::Gptp(state::Event::Timer(state::Timer::SyncTimeout {
                        node,
                        generation: timeout_generation,
                    })),
                )?;
                self.record_clock(cfg.nodes[node].device, &changed, "gptp_sync", d, c)?;
                self.gptp_record("clock_corrected",Some(t),None,json!({"sync_transfer_id":self.transfer(d,pair.transfer).row.transfer_id,
                    "node":self.prepared.ethernet.devices[cfg.nodes[node].device].id,"grandmaster":hex(&gm_identity),
                    "epoch":sync_epoch.to_string(),"target_numerator":target.num.to_string(),"target_denominator":target.den.to_string(),
                    "rate_numerator":rate.num.to_string(),"rate_denominator":rate.den.to_string(),
                    "neighbor_delay_num":delay.num.to_string(),"neighbor_rate_num":r.num.to_string(),
                    "clock_generation":generation.to_string()}),d,c)?;
                self.after_clock_change(cfg.nodes[node].device, d, c)?;
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Context, EffectBatch};
    use crate::{
        CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::Path,
    };

    #[test]
    fn real_sync_owner_accepts_changed_correction_only_with_recomputed_fcs() {
        let prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gptp/two-node.ini"),
        )
        .unwrap();
        let (mut session, controller) = RunSession::new(
            prepared.clone(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut selected = false;
        let mut pending_sync = None;
        for _ in 0..5000 {
            if session.advance_one().unwrap() != RunState::Running {
                break;
            }
            let snapshot = session.snapshot();
            let rows = &snapshot.registered.as_ref().unwrap().model_records;
            selected |= rows.values().any(|r| {
                r.schema.name == "dir.network.gptp"
                    && r.data["kind"] == "selection"
                    && r.data["details"]["node"] == "Net.b"
            });
            if selected {
                pending_sync = rows
                    .values()
                    .find(|r| {
                        r.schema.name == "dir.network.gptp"
                            && r.data["kind"] == "physical_eof"
                            && r.data["message_type"] == 0
                            && r.data["transfer_id"]
                                .as_str()
                                .is_some_and(|v| v.contains("Net.a.tx"))
                            && !rows.values().any(|rx| {
                                rx.schema.name == "dir.network.gptp"
                                    && rx.data["kind"] == "packet_received"
                                    && rx.data["transfer_id"] == r.data["transfer_id"]
                            })
                    })
                    .map(|r| r.data["transfer_id"].as_str().unwrap().to_owned());
                if pending_sync.is_some() {
                    break;
                }
            }
        }
        let transfer_id = pending_sync.expect("physical Sync EOF before selected ingress");
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let root = std::env::temp_dir().join(format!("gptp-rx-body-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            session
                .checkpoint(&root, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let saved: Value =
            serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        let horizon = session.snapshot().common.last_event_time_ps.unwrap();
        let network_prepared = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .clone();
        let network = NetworkState::restore_checkpoint(
            network_prepared.clone(),
            &saved["network_backend"]["state"],
            horizon,
        )
        .unwrap();
        let transfer = network
            .transfers
            .iter()
            .position(|v| v.row.transfer_id == transfer_id)
            .unwrap();
        let packet = network
            .gptp
            .as_ref()
            .unwrap()
            .packets
            .get(&transfer)
            .unwrap();
        assert!(matches!(packet.message.body, Body::Sync));
        assert!(packet.tx_eof && packet.rx.is_some() && !packet.rx_validated);
        let arrival = network.transfers[transfer].row.planned_arrival_ps.unwrap();
        let mut changed = bytes(&network.transfers[transfer].row.wire.mac_hex).unwrap();
        let mut received = packet.message.clone();
        received.correction += 65_536;
        let payload = received.encode().unwrap();
        changed[14..14 + payload.len()].copy_from_slice(&payload);
        let len = changed.len();
        let fcs = crate::runtime::ethernet::crc32(&changed[..len - 4]).to_le_bytes();
        changed[len - 4..].copy_from_slice(&fcs);
        assert_eq!(
            Message::decode(validate_mac(&changed).unwrap())
                .unwrap()
                .correction,
            received.correction
        );
        let reg = prepared.registered.as_ref().unwrap();
        let mut next = 0;
        let channels = BTreeMap::new();
        let live = BTreeSet::new();
        let mut batch = EffectBatch::default();
        let mut context = Context {
            now: arrival,
            config: reg.models.iter().find(|m| m.id == "@network").unwrap(),
            registry: &reg.registry,
            profile: reg.registry.profile(&prepared.common.profile).unwrap(),
            channels: &channels,
            channel_configs: &[],
            live_timers: &live,
            executing_timer: None,
            next_id: &mut next,
            batch: &mut batch,
        };
        let mut delta = network.delta();
        network
            .gptp_receive(transfer, None, None, &changed, &mut delta, &mut context)
            .unwrap();
        let accepted = delta.gptp.as_ref().unwrap().packets.get(&transfer).unwrap();
        assert!(accepted.rx_validated);
        assert_eq!(
            accepted.received_message.as_ref().unwrap().correction,
            received.correction
        );
        let ingress = network_prepared.gptp.as_ref().unwrap().reverse[packet.direction];
        let node = network_prepared.gptp.as_ref().unwrap().ports[ingress]
            .as_ref()
            .unwrap()
            .node;
        assert!(delta.gptp.as_ref().unwrap().nodes[node].sync_pair.is_some());

        // The later Follow_Up transition must use the received Sync body.
        // The Follow_Up is already locally generated at the Sync TX capture;
        // this invokes the private decoded boundary before its physical RX.
        let (followup_transfer, followup) = network
            .gptp
            .as_ref()
            .unwrap()
            .packets
            .iter()
            .find(|(_, p)| {
                matches!(p.message.body, Body::FollowUp { .. })
                    && p.message.source == received.source
                    && p.message.sequence == received.sequence
            })
            .map(|(&t, p)| (t, p.message.clone()))
            .expect("paired physical Follow_Up owner");
        let mut baseline = network.delta();
        let original_mac = bytes(&network.transfers[transfer].row.wire.mac_hex).unwrap();
        network
            .gptp_receive(
                transfer,
                None,
                None,
                &original_mac,
                &mut baseline,
                &mut context,
            )
            .unwrap();
        context.now = arrival + 1;
        let followup_raw = network.gptp_raw(node, context.now).unwrap();
        assert!(
            network
                .gptp_process(
                    ingress,
                    followup_transfer,
                    followup.clone(),
                    followup_raw,
                    &mut baseline,
                    &mut context,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            network
                .gptp_process(
                    ingress,
                    followup_transfer,
                    followup,
                    followup_raw,
                    &mut delta,
                    &mut context,
                )
                .unwrap()
                .is_none()
        );
        let original_target = baseline
            .gptp
            .as_ref()
            .unwrap()
            .journal
            .last()
            .unwrap()
            .target;
        let changed_target = delta.gptp.as_ref().unwrap().journal.last().unwrap().target;
        assert_eq!(
            changed_target.sub(original_target).unwrap(),
            Rat::new(1_000, 1).unwrap()
        );

        // The same changed body with the old FCS is stopped at MAC validation.
        changed[len - 4] ^= 1;
        let network = NetworkState::restore_checkpoint(
            network_prepared,
            &saved["network_backend"]["state"],
            horizon,
        )
        .unwrap();
        let mut delta = network.delta();
        context.now = arrival;
        network
            .gptp_receive(transfer, None, None, &changed, &mut delta, &mut context)
            .unwrap();
        let rejected = delta.gptp.as_ref().unwrap().packets.get(&transfer).unwrap();
        assert!(!rejected.rx_validated);
        assert!(rejected.received_message.is_none());
        assert!(delta.gptp.as_ref().unwrap().nodes[node].sync_pair.is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pdelay_response_identity_guards_reset_pending_before_followup() {
        // These foreign identity variants cannot be emitted by the local
        // two-node generator. Exercise the production decoded-receive reducer
        // against a real pending request/response captured from that generator.
        let prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gptp/two-node.ini"),
        )
        .unwrap();
        let (mut session, controller) = RunSession::new(
            prepared.clone(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut response_id = None;
        for _ in 0..5000 {
            if session.advance_one().unwrap() != RunState::Running {
                break;
            }
            response_id = session
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .find(|r| {
                    r.schema.name == "dir.network.gptp" && r.data["kind"] == "pdelay_response"
                })
                .and_then(|r| r.data["transfer_id"].as_str())
                .map(str::to_owned);
            if response_id.is_some() {
                break;
            }
        }
        let response_id = response_id.expect("a real pending Pdelay response");
        controller.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let root = std::env::temp_dir().join(format!("gptp-pdelay-guard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        session
            .checkpoint(&root, CheckpointLimits::default())
            .unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        let horizon = session.snapshot().common.last_event_time_ps.unwrap();
        let network_prepared = prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .clone();
        let registry = prepared.registered.as_ref().unwrap();
        for variant in [
            "duplicate",
            "wrong_requester",
            "wrong_sequence",
            "self_source",
        ] {
            let network = NetworkState::restore_checkpoint(
                network_prepared.clone(),
                &saved["network_backend"]["state"],
                horizon,
            )
            .unwrap();
            let transfer = network
                .transfers
                .iter()
                .position(|t| t.row.transfer_id == response_id)
                .unwrap();
            let packet = network
                .gptp
                .as_ref()
                .unwrap()
                .packets
                .get(&transfer)
                .unwrap();
            let ingress = network_prepared.gptp.as_ref().unwrap().reverse[packet.direction];
            let before = network.gptp.as_ref().unwrap();
            let pending = before.ports[ingress]
                .as_ref()
                .unwrap()
                .pending
                .as_ref()
                .unwrap();
            assert_eq!(pending.response_transfer, Some(transfer));
            let samples = before.samples.len();
            let corrections = before.journal.len();
            let clocks = network.clocks.clone();
            let mut response = packet.received_message.clone().unwrap();
            match variant {
                "duplicate" => {}
                "wrong_requester" => {
                    if let Body::PdelayResp { requester, .. } = &mut response.body {
                        requester.port = requester.port.checked_add(1).unwrap();
                    } else {
                        panic!("response body");
                    }
                }
                "wrong_sequence" => response.sequence = response.sequence.wrapping_add(1),
                "self_source" => response.source = network.gptp_port_identity(ingress),
                _ => unreachable!(),
            }
            let mut next = 0;
            let channels = BTreeMap::new();
            let live = BTreeSet::new();
            let mut batch = EffectBatch::default();
            let mut context = Context {
                now: horizon,
                config: registry.models.iter().find(|m| m.id == "@network").unwrap(),
                registry: &registry.registry,
                profile: registry.registry.profile(&prepared.common.profile).unwrap(),
                channels: &channels,
                channel_configs: &[],
                live_timers: &live,
                executing_timer: None,
                next_id: &mut next,
                batch: &mut batch,
            };
            let mut delta = network.delta();
            let reason = network
                .gptp_process(
                    ingress,
                    transfer,
                    response.clone(),
                    packet.rx.as_ref().unwrap().raw,
                    &mut delta,
                    &mut context,
                )
                .unwrap();
            assert!(reason.is_some(), "{variant}");
            assert!(
                delta.gptp.as_ref().unwrap().ports[ingress]
                    .as_ref()
                    .unwrap()
                    .pending
                    .is_none(),
                "{variant}"
            );
            let mut followup = response;
            followup.body = Body::PdelayRespFollowUp {
                response_origin: super::super::codec::Timestamp::new(0, 0).unwrap(),
                requester: network.gptp_port_identity(ingress),
            };
            let reason = network
                .gptp_process(
                    ingress,
                    transfer,
                    followup,
                    packet.rx.as_ref().unwrap().raw,
                    &mut delta,
                    &mut context,
                )
                .unwrap();
            assert!(reason.is_some(), "{variant}");
            let after = delta.gptp.as_ref().unwrap();
            assert_eq!(after.samples.len(), samples, "{variant}");
            assert_eq!(after.journal.len(), corrections, "{variant}");
            assert_eq!(network.clocks, clocks, "{variant}");
        }
        let _ = fs::remove_dir_all(&root);
    }
}
