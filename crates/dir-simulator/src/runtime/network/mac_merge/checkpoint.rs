//! Exact causal ledger for this opt-in owner. Generic timers and other owners
//! retain their existing validators.
use super::super::*;
use super::integration::{MergeEvent, Origin};
use super::*;
use std::collections::BTreeSet;
fn bad(s: &str) -> Diagnostic {
    error(format!("MAC Merge checkpoint: {s}"))
}
fn key(at: u64, event: &MergeEvent) -> Result<String> {
    Ok(format!(
        "{at}:{}",
        serde_json::to_string(event).map_err(|e| bad(&e.to_string()))?
    ))
}
impl NetworkState {
    pub(in crate::runtime::network) fn validate_mac_merge(&self, horizon: u64) -> Result<()> {
        let Some(cfg) = self
            .prepared
            .extensions
            .as_ref()
            .and_then(|e| e.mac_merge.as_ref())
        else {
            if self.mac_merge.is_some() {
                return Err(bad("unexpected owner"));
            }
            return Ok(());
        };
        let state = self
            .mac_merge
            .as_ref()
            .ok_or_else(|| bad("missing owner"))?;
        if &state.identity != cfg
            || state.ports.len() != cfg.ports.len()
            || !state.records.is_empty()
            || state.packets.len() > cfg.max_pending_packets
        {
            return Err(bad("configuration/shape/pending capacity"));
        }
        let mut retained = BTreeSet::new();
        for (i, p) in state.ports.iter().enumerate() {
            if p.permission
                .as_ref()
                .is_some_and(|proof| proof.at > horizon || proof.epoch > p.permission_epoch)
            {
                return Err(bad("future fractional permission proof"));
            }

            self.validate_merge_journal(i, horizon)?;
            if p.tx.hold != p.applied_tas.is_some_and(|c| c.hold) {
                return Err(bad("MM HOLD differs from applied GCL control"));
            }
            if let Some(applied) = p.applied_tas {
                self.merge_validate_tas_proof(i, applied, horizon)?;
                if applied.generation > p.tas_generation {
                    return Err(bad("future applied MM control generation"));
                }
            }
            if p.tas_planned_at > horizon {
                return Err(bad("future TAS control plan"));
            }
            if p.link_up && p.tas_generation > 0 {
                let expected = self
                    .merge_next_tas(i, &self.delta(), p.tas_planned_at)?
                    .map(|mut c| {
                        c.generation = p.tas_generation;
                        c
                    });
                if p.tas_control != expected || p.tas_control.is_some_and(|c| c.at < horizon) {
                    return Err(bad("TAS advance token differs from GCL"));
                }
            } else if p.tas_control.is_some() {
                return Err(bad("TAS control without active schedule owner"));
            }
            if p.accounting_at > horizon {
                return Err(bad("future accounting segment"));
            }
            if let Some(view) = p.accounting {
                if view != self.merge_accounting_view(i, &self.delta(), p.accounting_at)? {
                    return Err(bad("accounting segment differs from actual packet"));
                }
                if let Some(tsn) = &self.tsn {
                    tsn.validate_merge_view(
                        &self.prepared.ethernet.directions[i].from_port,
                        &self.port_input(i, &self.delta())?,
                    )?;
                }
            }
            p.tx.validate_checkpoint().map_err(|_| bad("TX reducer"))?;
            p.rx.validate_checkpoint().map_err(|_| bad("RX reducer"))?;
            p.verify
                .validate_checkpoint()
                .map_err(|_| bad("verification reducer"))?;
            if p.rx.epoch != p.tx.epoch
                || p.verify.epoch != p.tx.epoch
                || p.rx.max_mac_octets != cfg.max_rx_mac_octets
                || p.verify.link_up != p.link_up
                || p.verify.admin_enabled != cfg.ports[i].as_ref().is_some_and(|p| p.admin_enabled)
                || p.verify.verify_time_ms != cfg.ports[i].as_ref().map_or(10, |p| p.verify_time_ms)
            {
                return Err(bad("port epoch/configuration"));
            }
            if p.verify.peer.is_some() != p.capability_packet.is_some()
                || p.verify.rcv_r != p.last_response_packet.is_some()
            {
                return Err(bad(
                    "capability/response flags without actual received physical evidence",
                ));
            }
            if let Some(packet) = &p.capability_packet {
                self.validate_merge_receive_evidence(i, packet, PacketKind::Express, horizon)?;
                let received = packet
                    .received
                    .as_ref()
                    .ok_or_else(|| bad("capability packet dropped"))?;
                let ingress = ValidatedMacIngress {
                    epoch: packet.epoch,
                    received_at: packet.arrival,
                    bytes: received[8..].to_vec(),
                    provenance: RxProvenance::ExternalInjection {
                        evidence_id: format!("mac-merge-packet:{}", packet.id),
                    },
                };
                let observed =
                    lldp::receive(i, &ingress, format!("mac-merge-packet:{}", packet.id))
                        .map_err(|_| bad("actual LLDP evidence"))?;
                if observed.capability.as_ref() != p.verify.peer.as_ref()
                    || observed.ttl == 0
                    || packet
                        .arrival
                        .checked_add(u64::from(observed.ttl) * 1_000_000_000_000)
                        != p.capability_expires
                {
                    return Err(bad(
                        "peer capability or expiry not derived from actual LLDP packet",
                    ));
                }
            }
            if let Some(packet) = &p.last_response_packet {
                self.validate_merge_receive_evidence(i, packet, PacketKind::Respond, horizon)?;
            }
            if let Some(packet) = &p.last_verify_packet {
                self.validate_merge_receive_evidence(i, packet, PacketKind::Verify, horizon)?;
            }
            if (p.verify.send_r || p.verify.rcv_v) && p.last_verify_packet.is_none() {
                return Err(bad(
                    "responder request lacks actual Verify receive evidence",
                ));
            }
            if p.verify.peer.is_some() != p.capability_expires.is_some()
                || p.capability_expires.is_some_and(|at| at < horizon)
            {
                return Err(bad("capability lifetime/evidence"));
            }
            if cfg.ports[i].is_some() != p.next_advertisement.is_some()
                || p.next_advertisement.is_some_and(|at| at < horizon)
            {
                return Err(bad("advertisement ledger"));
            }
            if let Some(id) = p.busy {
                let packet = state
                    .packets
                    .get(&id)
                    .ok_or_else(|| bad("occupied serializer without packet"))?;
                if packet.direction != i || packet.release_done || packet.release < horizon {
                    return Err(bad("occupied serializer differs"));
                }
            }
            let expected_active =
                p.busy
                    .and_then(|id| state.packets.get(&id))
                    .and_then(|p| match p.origin {
                        Origin::Pmac(t) | Origin::Express(t) => Some(t),
                        _ => None,
                    });
            if cfg.participating[i] && self.queues[i].active != expected_active {
                return Err(bad("queue/physical owner differs"));
            }
            for (which, t) in [(false, p.selected_pmac), (true, p.selected_emac)] {
                if let Some(t) = t {
                    let transfer = self
                        .transfers
                        .get(t)
                        .ok_or_else(|| bad("selected transfer missing"))?;
                    if transfer.direction != i
                        || !retained.insert(t)
                        || self.queues[i].waiting.iter().any(|q| q.contains(&t))
                    {
                        return Err(bad("selected transfer queue/uniqueness"));
                    }
                    let mask = cfg.ports[i]
                        .as_ref()
                        .map_or(if which { 255 } else { 0 }, |p| {
                            if which {
                                p.express_mask
                            } else {
                                p.preemptable_mask
                            }
                        });
                    if mask & (1 << transfer.row.priority) == 0 {
                        return Err(bad("selected service/class differs"));
                    }
                }
            }
            if p.tx.selected.is_some() != p.selected_pmac.is_some() {
                let final_ipg = p.selected_pmac.zip(p.busy).is_some_and(|(t, id)| {
                    state.packets.get(&id).is_some_and(|p| {
                        p.origin == Origin::Pmac(t)
                            && p.eof_done
                            && p.plan.as_ref().is_some_and(|p| p.final_packet)
                    })
                });
                if !final_ipg {
                    return Err(bad("selected pMAC differs"));
                }
            }
            if let Some(selected) = &p.tx.selected {
                let t = p
                    .selected_pmac
                    .ok_or_else(|| bad("TX bytes without transfer"))?;
                if ingress::bytes(&self.transfers[t].row.wire.mac_hex)
                    .map_err(|_| bad("selected MAC hex"))?
                    != selected.mac
                {
                    return Err(bad("selected original MAC bytes differ"));
                }
            }
            if p.rx.assembly.as_ref().map_or(0, |a| a.fragments.len()) != p.assembly_packets.len() {
                return Err(bad("RX fragment physical proof count"));
            }
            if let Some(a) = &p.rx.assembly {
                let mut ids = BTreeSet::new();
                for (f, packet) in a.fragments.iter().zip(&p.assembly_packets) {
                    let kind = decode_packet(&f.packet.bytes)
                        .map_err(|_| bad("RX fragment framing"))?
                        .kind;
                    self.validate_merge_receive_evidence(i, packet, kind, horizon)?;
                    let provenance = match packet.origin {
                        Origin::Pmac(t) => RxProvenance::LocalTransfer(t),
                        _ => return Err(bad("RX fragment source kind")),
                    };
                    if !ids.insert(packet.id)
                        || packet.received.as_ref() != Some(&f.packet.bytes)
                        || packet.arrival != f.received_at
                        || !f.packet.complete
                        || f.packet.provenance != provenance
                    {
                        return Err(bad("RX fragment not bound to physical reception"));
                    }
                }
                if a.started_at > horizon || a.last_fragment_at > horizon {
                    return Err(bad("future RX prefix"));
                }
                if let RxProvenance::LocalTransfer(t) = a.provenance {
                    if self
                        .transfers
                        .get(t)
                        .is_none_or(|t| t.direction != cfg.reverse[i])
                    {
                        return Err(bad("RX provenance on another physical ingress"));
                    }
                }
            }
        }
        let mut occupied = BTreeSet::new();
        for (id, p) in &state.packets {
            let link = self
                .prepared
                .ethernet
                .directions
                .get(p.direction)
                .ok_or_else(|| bad("packet direction"))?;
            let port = &state.ports[p.direction];
            if p.aborted_ps.is_some()
                || p.invalidated_ps.is_some()
                || *id != p.id
                || *id >= state.next_packet
                || p.epoch != port.tx.epoch
                || p.ordinal >= port.ordinal
                || p.sof > horizon
                || p.eof <= p.sof
                || p.release <= p.eof
                || p.arrival
                    != p.eof
                        .checked_add(self.physical_receive_delay(p.direction)?)
                        .ok_or_else(|| bad("arrival overflow"))?
                || p.eof_done && p.eof > horizon
                || !p.eof_done && p.eof < horizon
                || p.release_done && !p.eof_done
                || p.release_done && p.release > horizon
                || !p.release_done && p.release < horizon
                || p.arrival_done && !p.eof_done
            {
                return Err(bad("packet physical history/timing"));
            }
            if !p.release_done && (!occupied.insert(p.direction) || port.busy != Some(*id)) {
                return Err(bad("multiple physical serializer owners"));
            }
            let bits = u64::try_from(p.bytes.len())
                .ok()
                .and_then(|n| n.checked_mul(8))
                .ok_or_else(|| bad("packet length"))?;
            if p.eof
                != p.sof
                    .checked_add(bit_duration(bits, link.bitrate_bps).map_err(|_| bad("rate"))?)
                    .ok_or_else(|| bad("EOF overflow"))?
                || p.release
                    != p.eof
                        .checked_add(bit_duration(96, link.bitrate_bps).map_err(|_| bad("rate"))?)
                        .ok_or_else(|| bad("IPG overflow"))?
            {
                return Err(bad("byte duration differs"));
            }
            if (p.origin == Origin::WireControl) != p.wire_control_id.is_some() {
                return Err(bad("Wire origin identity pairing"));
            }
            if let Some(raw) = &p.raw {
                if let Some(sample) = raw.start_sample {
                    let medium = self
                        .phy
                        .get(raw.attachment)
                        .ok_or_else(|| bad("raw attachment"))?;
                    if medium.sample_time_ps(sample).map_err(error)? != p.sof
                        || sample > medium.next_sample_index()
                    {
                        return Err(bad("raw admission sample/time"));
                    }
                }
            }
            match p.origin {
                Origin::WireControl => {
                    self.validate_wire_merge_source(p, horizon)?;
                    let id = p.wire_control_id.as_ref().unwrap();
                    let host = self.wire.as_ref().unwrap();
                    let tx = &host.tx[id];
                    if (!p.release_done
                        && p.invalidated_ps.is_none()
                        && !tx.dropped
                        && host.active[p.direction].as_ref() != Some(id))
                        || (p.release_done && host.active[p.direction].as_ref() == Some(id))
                    {
                        return Err(bad("Wire active/released claim differs"));
                    }
                }
                Origin::Pmac(t) => {
                    let row = self
                        .transfers
                        .get(t)
                        .filter(|t| t.direction == p.direction)
                        .ok_or_else(|| bad("pMAC packet transfer"))?;
                    let plan = p.plan.as_ref().ok_or_else(|| bad("pMAC missing plan"))?;
                    let decoded = decode_packet(&p.bytes).map_err(|_| bad("pMAC encoding"))?;
                    let (frame, frag, active) = match decoded.kind {
                        PacketKind::Start { frame_count } => (frame_count, None, true),
                        PacketKind::Continuation {
                            frame_count,
                            frag_count,
                        } => (frame_count, Some(frag_count), true),
                        PacketKind::Express => (0, None, false),
                        _ => return Err(bad("pMAC control kind")),
                    };
                    let mac = ingress::bytes(&row.row.wire.mac_hex)
                        .map_err(|_| bad("original MAC hex"))?;
                    let add = port
                        .tx
                        .selected
                        .as_ref()
                        .map_or(0, |s| s.peer_add_frag_size);
                    if plan.bytes != p.bytes
                        || plan.bitrate != link.bitrate_bps
                        || plan.epoch != p.epoch
                        || plan.start_ps != p.sof
                        || plan.eof_ps != p.eof
                        || plan.release_ps != p.release
                        || plan.final_packet != (plan.end_offset == mac.len() - 4)
                        || (!p.eof_done && plan.generation != port.tx.generation)
                        || encode_fragment(
                            &mac,
                            plan.start_offset,
                            plan.end_offset,
                            frame,
                            frag,
                            active,
                            add,
                        )
                        .map_err(|_| bad("pMAC legal cut"))?
                            != p.bytes
                    {
                        return Err(bad("pMAC plan/source/counter/CRC differs"));
                    }
                }
                Origin::Express(t) => {
                    let row = self
                        .transfers
                        .get(t)
                        .filter(|t| t.direction == p.direction)
                        .ok_or_else(|| bad("express packet transfer"))?;
                    if p.plan.is_some()
                        || p.token.is_some()
                        || encode_express(
                            &ingress::bytes(&row.row.wire.mac_hex)
                                .map_err(|_| bad("express MAC hex"))?,
                        )
                        .map_err(|_| bad("express FCS"))?
                            != p.bytes
                    {
                        return Err(bad("express original bytes differ"));
                    }
                }
                Origin::Control(kind) => {
                    let token = p.token.ok_or_else(|| bad("control packet token"))?;
                    if p.plan.is_some()
                        || token.epoch != p.epoch
                        || token.kind != kind
                        || encode_control(kind).map_err(|_| bad("control kind"))? != p.bytes
                        || (!p.eof_done && token.generation != port.verify.control_generation)
                    {
                        return Err(bad("control request/bytes differ"));
                    }
                }
                Origin::Lldp => {
                    let decoded =
                        decode_packet(&p.bytes).map_err(|_| bad("LLDP packet framing"))?;
                    let packet = ValidatedMacIngress {
                        epoch: p.epoch,
                        received_at: p.arrival,
                        bytes: p.bytes[8..].to_vec(),
                        provenance: RxProvenance::ExternalInjection {
                            evidence_id: format!("mac-merge-packet:{id}"),
                        },
                    };
                    let observation =
                        lldp::receive(p.direction, &packet, "checkpoint-check".into())
                            .map_err(|_| bad("LLDP packet subset"))?;
                    let configured = cfg.ports[p.direction]
                        .as_ref()
                        .ok_or_else(|| bad("legacy peer advertised capability"))?;
                    if decoded.kind != PacketKind::Express
                        || observation.capability.as_ref().is_none_or(|peer| {
                            peer.source_mac != configured.source_mac
                                || peer.add_frag_size != configured.advertised_add_frag_size
                        })
                        || observation.ttl != cfg.lldp_ttl_seconds
                    {
                        return Err(bad("LLDP configured bytes differ"));
                    }
                }
            }
            let mut expected = p.clone();
            self.merge_fault(cfg, &mut expected)?;
            if expected.received != p.received || expected.complete != p.complete {
                return Err(bad("physical fault outcome differs"));
            }
        }
        for (t, b) in &state.validated {
            let row = self
                .transfers
                .get(*t)
                .ok_or_else(|| bad("validated transfer missing"))?;
            if !cfg.participating[row.direction]
                || !valid_mac(b)
                || ingress::bytes(&row.row.wire.mac_hex).map_err(|_| bad("received MAC hex"))? != *b
                || row.row.arrival_ps.is_none()
                || self.receptions.iter().filter(|r| r.transfer == *t).count() != 1
            {
                return Err(bad("validated ingress/delivery provenance differs"));
            }
        }
        for (t, row) in self.transfers.iter().enumerate() {
            if cfg.participating[row.direction]
                && row.row.arrival_ps.is_some()
                && !state.validated.contains_key(&t)
            {
                return Err(bad("ordinary arrival without final validated ingress"));
            }
        }
        Ok(())
    }
    fn validate_merge_receive_evidence(
        &self,
        i: usize,
        p: &super::integration::Packet,
        kind: PacketKind,
        horizon: u64,
    ) -> Result<()> {
        let state = self
            .mac_merge
            .as_ref()
            .ok_or_else(|| bad("receive evidence without owner"))?;
        let cfg = &state.identity;
        if p.aborted_ps.is_some()
            || p.invalidated_ps.is_some()
            || p.direction != cfg.reverse[i]
            || p.epoch != state.ports[i].tx.epoch
            || p.id >= state.next_packet
            || p.ordinal >= state.ports[p.direction].ordinal
            || !p.eof_done
            || !p.arrival_done
            || !p.complete
            || p.sof > p.eof
            || p.arrival > horizon
        {
            return Err(bad("receive evidence physical identity/history"));
        }
        let link = &self.prepared.ethernet.directions[p.direction];
        let bits = u64::try_from(p.bytes.len())
            .ok()
            .and_then(|n| n.checked_mul(8))
            .ok_or_else(|| bad("receive evidence length"))?;
        if p.sof
            .checked_add(bit_duration(bits, link.bitrate_bps).map_err(|_| bad("evidence rate"))?)
            != Some(p.eof)
            || p.eof.checked_add(self.physical_receive_delay(p.direction)?) != Some(p.arrival)
        {
            return Err(bad("receive evidence physical timing"));
        }
        let mut expected = p.clone();
        self.merge_fault(cfg, &mut expected)?;
        if expected.received != p.received || expected.complete != p.complete {
            return Err(bad("receive evidence fault replay"));
        }
        let bytes = p
            .received
            .as_ref()
            .ok_or_else(|| bad("receive evidence missing bytes"))?;
        if decode_packet(bytes)
            .map_err(|_| bad("receive evidence CRC/delimiter"))?
            .kind
            != kind
        {
            return Err(bad("receive evidence kind"));
        }
        match p.origin {
            Origin::Pmac(t) => {
                let row = self
                    .transfers
                    .get(t)
                    .filter(|row| row.direction == p.direction)
                    .ok_or_else(|| bad("fragment source transfer"))?;
                let plan = p.plan.as_ref().ok_or_else(|| bad("fragment source plan"))?;
                let (frame, frag) = match kind {
                    PacketKind::Start { frame_count } => (frame_count, None),
                    PacketKind::Continuation {
                        frame_count,
                        frag_count,
                    } => (frame_count, Some(frag_count)),
                    _ => return Err(bad("fragment evidence delimiter")),
                };
                let mac = ingress::bytes(&row.row.wire.mac_hex)
                    .map_err(|_| bad("fragment source MAC"))?;
                if plan.bytes != p.bytes
                    || plan.epoch != p.epoch
                    || plan.start_ps != p.sof
                    || plan.eof_ps != p.eof
                    || plan.bitrate != link.bitrate_bps
                    || plan.final_packet
                    || encode_fragment(
                        &mac,
                        plan.start_offset,
                        plan.end_offset,
                        frame,
                        frag,
                        true,
                        0,
                    )
                    .map_err(|_| bad("fragment source encoding"))?
                        != p.bytes
                {
                    return Err(bad("fragment source bytes/counters differ"));
                }
            }
            Origin::Control(control) => {
                if encode_control(control).map_err(|_| bad("evidence source control"))? != p.bytes {
                    return Err(bad("evidence original control bytes"));
                }
            }
            Origin::Lldp => {
                if !valid_mac(&p.bytes[8..]) {
                    return Err(bad("evidence original LLDP FCS"));
                }
            }
            _ => return Err(bad("capability/control evidence has an unrelated source")),
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn validate_mac_merge_pending(
        &self,
        events: &[(u64, Event)],
    ) -> Result<()> {
        let Some(state) = &self.mac_merge else {
            if !events.is_empty() {
                return Err(bad("unexpected direct events"));
            }
            return Ok(());
        };
        let mut expected = Vec::new();
        for p in state.packets.values() {
            if !p.eof_done {
                expected.push(key(p.eof, &MergeEvent::Eof(p.id))?);
            }
            if !p.release_done {
                expected.push(key(p.release, &MergeEvent::Release(p.id))?);
            }
            if let Some(plan) = &p.plan {
                let rate = self.prepared.ethernet.directions[p.direction].bitrate_bps;
                let mut boundaries = vec![];
                if plan.start_offset > 0 {
                    boundaries.push(add(
                        p.sof,
                        bit_duration(64, rate).map_err(|_| bad("accounting rate"))?,
                    )?);
                }
                if !plan.final_packet {
                    boundaries
                        .push(p.eof - bit_duration(32, rate).map_err(|_| bad("accounting rate"))?);
                }
                for at in boundaries {
                    if at > state.ports[p.direction].accounting_at {
                        expected.push(key(at, &MergeEvent::Accounting(p.id))?);
                    }
                }
            }
            if p.smd_pending {
                expected.push(key(
                    add(
                        p.sof,
                        bit_duration(
                            56,
                            self.prepared.ethernet.directions[p.direction].bitrate_bps,
                        )
                        .map_err(|_| bad("SMD rate"))?,
                    )?,
                    &MergeEvent::Smd(p.id),
                )?);
            }
        }
        for (i, p) in state.ports.iter().enumerate() {
            if let Some(control) = p.tas_control {
                expected.push(key(
                    control.at,
                    &MergeEvent::Tas {
                        direction: i,
                        control,
                    },
                )?);
            }
            if let Some(at) = p.next_advertisement {
                expected.push(key(at, &MergeEvent::Advertise(i))?);
            }
            if let Some(at) = p.capability_expires {
                expected.push(key(
                    at,
                    &MergeEvent::Expiry {
                        direction: i,
                        generation: p.capability_generation,
                    },
                )?);
            }
        }
        let mut actual = self
            .due
            .iter()
            .filter_map(|d| match &d.kind {
                Completion::MacMerge(e) => Some(key(d.at, e)),
                _ => None,
            })
            .collect::<Result<Vec<_>>>()?;
        actual.sort();
        expected.sort();
        if expected != actual {
            return Err(bad(
                "orphan/missing/duplicate/wrong-generation physical or lifetime due event",
            ));
        }
        let mut direct = BTreeSet::new();
        let mut arrivals = BTreeSet::new();
        let mut timers = BTreeSet::new();
        for (at, event) in events {
            let unique = format!(
                "{at}:{}",
                serde_json::to_string(event).map_err(|_| bad("event encoding"))?
            );
            if !direct.insert(unique) {
                return Err(bad("duplicate direct event"));
            }
            match event {
                Event::MacMergeArrival {
                    direction,
                    epoch,
                    packet,
                } => {
                    let p = state
                        .ports
                        .get(*direction)
                        .ok_or_else(|| bad("arrival direction"))?;
                    if *epoch < p.tx.epoch && *packet < state.next_packet {
                        continue;
                    }
                    self.validate_merge_arrival(*direction, *epoch, *packet, *at)?;
                    arrivals.insert(*packet);
                }
                Event::MacMergeTimer {
                    direction,
                    token,
                    commit,
                } => {
                    let p = state
                        .ports
                        .get(*direction)
                        .ok_or_else(|| bad("timer direction"))?;
                    if token.due_ps != *at
                        || token.epoch > p.verify.epoch
                        || token.generation > p.verify.generation
                    {
                        return Err(bad("timer epoch/generation/deadline"));
                    }
                    if p.verify.timer == Some(*token) {
                        if p.verify_timer_commit != *commit {
                            return Err(bad("timer probe/commit stage differs"));
                        }
                        timers.insert(*direction);
                    } else if token.epoch == p.verify.epoch
                        && token.generation == p.verify.generation
                        && !(p.verify.phase == VerificationPhase::Verified
                            && p.verify.last_verify_eof.and_then(|e| {
                                e.checked_add(u64::from(p.verify.verify_time_ms) * 1_000_000_000)
                            }) == Some(*at))
                    {
                        return Err(bad("unowned timer is not a proven stale no-op"));
                    }
                }
                _ => return Err(bad("unrelated direct event")),
            }
        }
        for p in state.packets.values() {
            if (p.eof_done && !p.arrival_done && p.received.is_some()) != arrivals.contains(&p.id) {
                return Err(bad("arrival owner/reservation mismatch"));
            }
        }
        for (i, p) in state.ports.iter().enumerate() {
            if p.verify.timer.is_some() != timers.contains(&i) {
                return Err(bad("verify timer owner/reservation mismatch"));
            }
        }
        Ok(())
    }
}
