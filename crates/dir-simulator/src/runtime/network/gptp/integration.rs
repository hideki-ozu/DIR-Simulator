use super::super::*;
use super::codec::{self, Announce, Body, FollowUpInfo, Message, PortIdentity, Timestamp};
use super::math::{self, Rat};
use super::state::{self, CaptureKind, CauseKind, Role, Timer};

pub(super) fn gerr(s: impl Into<String>) -> Diagnostic {
    error(format!("gPTP: {}", s.into()))
}
pub(super) fn from_wire_correction(value: i64) -> Result<Rat> {
    Rat::new(
        i128::from(value)
            .checked_mul(1000)
            .ok_or_else(|| gerr("correction overflow"))?,
        65536,
    )
    .map_err(gerr)
}
pub(super) fn to_wire_correction(value: Rat) -> Result<i64> {
    let scaled = value
        .num
        .checked_mul(65536)
        .ok_or_else(|| gerr("correction overflow"))?
        .div_euclid(
            value
                .den
                .checked_mul(1000)
                .ok_or_else(|| gerr("correction overflow"))?,
        );
    let raw = i64::try_from(scaled).map_err(|_| gerr("correctionField overflow"))?;
    if raw == i64::MAX {
        return Err(gerr("unrepresentable correctionField"));
    }
    Ok(raw)
}
pub(super) fn timestamp_part(value: Rat) -> Result<(Timestamp, Rat)> {
    if value.num < 0 {
        return Err(gerr("negative raw oscillator timestamp"));
    }
    let ns = value.div(Rat::new(1000, 1).unwrap()).map_err(gerr)?.floor();
    let seconds = u64::try_from(ns.div_euclid(1_000_000_000))
        .map_err(|_| gerr("timestamp seconds overflow"))?;
    let nanos = u32::try_from(ns.rem_euclid(1_000_000_000))
        .map_err(|_| gerr("timestamp nanoseconds overflow"))?;
    let stamp = Timestamp::new(seconds, nanos).map_err(gerr)?;
    let rest = value
        .sub(Rat::new(stamp.floor_ps().map_err(gerr)?, 1).unwrap())
        .map_err(gerr)?;
    Ok((stamp, rest))
}
pub(super) fn stamp_with_correction(stamp: Timestamp, correction: i64) -> Result<Rat> {
    Rat::new(stamp.floor_ps().map_err(gerr)?, 1)
        .unwrap()
        .add(from_wire_correction(correction)?)
        .map_err(gerr)
}
pub(super) fn application_target(target: Rat) -> Result<clock::LocalTime> {
    if let Ok(den) = u64::try_from(target.den) {
        clock::LocalTime::fraction(target.num, den).map_err(|e| gerr(e.to_string()))
    } else {
        // Application clocks have a u64 denominator. Keep packet arithmetic exact
        // in the journal; quantize only the interface value to integer picoseconds.
        clock::LocalTime::fraction(target.floor(), 1).map_err(|e| gerr(e.to_string()))
    }
}
pub(super) fn application_rate(rate: Rat) -> Result<(u64, u64)> {
    if let (Ok(num), Ok(den)) = (u64::try_from(rate.num), u64::try_from(rate.den)) {
        return Ok((num, den));
    }
    let den = 1_000_000_000_000u64;
    let scaled = rate
        .num
        .checked_mul(i128::from(den))
        .ok_or_else(|| gerr("clock rate quantization overflow"))?
        .checked_add(rate.den / 2)
        .ok_or_else(|| gerr("clock rate quantization overflow"))?
        / rate.den;
    let num = u64::try_from(scaled).map_err(|_| gerr("disciplined rate exceeds clock range"))?;
    if num == 0 {
        return Err(gerr("disciplined rate rounds to zero"));
    }
    Ok((num, den))
}
pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    encoded
}
fn mac(bytes: [u8; 6]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
pub(super) fn quality(n: &crate::types::network::gptp::GptpNode) -> [u8; 14] {
    let mut v = [0; 14];
    v[0] = n.priority1;
    v[1] = n.clock_class;
    v[2] = n.clock_accuracy;
    v[3..5].copy_from_slice(&n.offset_scaled_log_variance.to_be_bytes());
    v[5] = n.priority2;
    v[6..14].copy_from_slice(&n.clock_identity);
    v
}
pub(in crate::runtime::network) fn reference_duration(bitrate: u64) -> Result<u64> {
    if bitrate == 0 {
        return Err(gerr("zero bitrate at timestamp point"));
    }
    let bits = 64u128 * 1_000_000_000_000u128;
    u64::try_from(bits / u128::from(bitrate) + u128::from(bits % u128::from(bitrate) != 0))
        .map_err(|_| gerr("timestamp point overflow"))
}

impl NetworkState {
    pub(super) fn gptp_source_evidence(
        &self,
        source: state::MacSource,
        d: &NetworkDelta,
    ) -> Result<state::ExpressSourceEvidence> {
        let transfer = match source {
            state::MacSource::MergeExpress { transfer, .. }
            | state::MacSource::AbstractExpress { transfer, .. } => transfer,
            state::MacSource::OrdinaryAttempt(binding) => binding.transfer,
        };
        if self.transfers.get(transfer).is_none()
            && !d.transfers.iter().any(|(t, _)| *t == transfer)
        {
            return Err(gerr("retained MAC source transfer missing"));
        }
        match source {
            state::MacSource::MergeExpress {
                transfer,
                packet_id,
                epoch,
                ..
            } => self.gptp_merge_source(transfer, packet_id, epoch, d),
            state::MacSource::AbstractExpress { transfer, .. } => {
                self.gptp_abstract_source(transfer, d)
            }
            state::MacSource::OrdinaryAttempt(_) => {
                Err(gerr("ordinary source requires actual attempt helper"))
            }
        }
    }
    /// Checked exact timeline shared by capture, private checkpoint and FES checks.
    pub(super) fn gptp_source_proof(
        &self,
        source: state::MacSource,
        kind: CaptureKind,
        d: &NetworkDelta,
    ) -> Result<state::ConfiguredCaptureProof> {
        if let state::MacSource::OrdinaryAttempt(binding) = source {
            let proof = self.configured_gptp_proof(self.media_state(d), binding, kind)?;
            let transfer = d
                .transfers
                .iter()
                .find(|(t, _)| *t == binding.transfer)
                .map(|(_, t)| t)
                .or_else(|| self.transfers.get(binding.transfer))
                .ok_or_else(|| gerr("attempt source transfer missing"))?;
            let (eof, release, _) = l2::timing(
                proof.sof_ps,
                transfer.row.wire.mac_bytes,
                proof.bitrate_bps,
                0,
            )?;
            if transfer.direction != binding.direction
                || transfer.row.sof_ps != Some(proof.sof_ps)
                || eof != proof.eof_planned_ps
                || release != self.media_state(d).attempts[binding.index].release
            {
                return Err(gerr("configured attempt frame timeline differs"));
            }
            return Ok(proof);
        }
        let evidence = self.gptp_source_evidence(source, d)?;
        if evidence.source != source {
            return Err(gerr("retained serializer source differs"));
        }
        let (transfer, direction) = match source {
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
            _ => unreachable!(),
        };
        let transfer_state = d
            .transfers
            .iter()
            .find(|(t, _)| *t == transfer)
            .map(|(_, t)| t)
            .or_else(|| self.transfers.get(transfer))
            .ok_or_else(|| gerr("MAC source transfer missing"))?;
        let row = &transfer_state.row;
        if transfer_state.direction != direction
            || row.sof_ps != Some(evidence.sof_ps)
            || row.wire.mac_hex != evidence.mac_hex
            || row.planned_eof_ps != Some(evidence.eof_planned_ps)
            || row.planned_arrival_ps != Some(evidence.arrival_planned_ps)
        {
            return Err(gerr(
                "retained serializer timeline or bytes differ from transfer",
            ));
        }
        let dir = &self.prepared.ethernet.directions[direction];
        let (eof, release, _) =
            l2::timing(evidence.sof_ps, row.wire.mac_bytes, dir.bitrate_bps, 0)?;
        if eof != evidence.eof_planned_ps || release != evidence.release_planned_ps {
            return Err(gerr("express source is not a complete ordinary MAC frame"));
        }
        let endpoint = self.configured_endpoint(direction);
        let (tx_latency, rx_latency) = if let Some((domain, side)) = endpoint {
            let cfg = &self
                .prepared
                .ethernet
                .media
                .as_ref()
                .unwrap()
                .physical_links[domain];
            if cfg.duplex != "full" || cfg.directions[side] != direction {
                return Err(gerr("configured express source physical owner differs"));
            }
            let (tx, rx) = if side == 0 {
                (&cfg.a_phy, &cfg.b_phy)
            } else {
                (&cfg.b_phy, &cfg.a_phy)
            };
            (tx.tx_latency_ps, rx.rx_latency_ps)
        } else {
            (0, 0)
        };
        let mdi = add(evidence.sof_ps, tx_latency)?;
        let peer = add(mdi, dir.delay_ps)?;
        let arrival = add(add(add(eof, tx_latency)?, dir.delay_ps)?, rx_latency)?;
        if evidence.arrival_planned_ps != arrival {
            return Err(gerr(
                "serializer arrival differs from physical configuration",
            ));
        }
        let reference = add(
            if kind == CaptureKind::Tx { mdi } else { peer },
            reference_duration(dir.bitrate_bps)?,
        )?;
        Ok(state::ConfiguredCaptureProof {
            source,
            endpoint,
            plane: if endpoint.is_some() {
                state::ReferencePlane::MdiMacSfd
            } else {
                state::ReferencePlane::IdealMacSfd
            },
            reference_ps: reference,
            observed_at_ps: reference,
            sof_ps: evidence.sof_ps,
            mdi_sof_ps: mdi,
            peer_sof_ps: peer,
            eof_planned_ps: eof,
            arrival_planned_ps: arrival,
            bitrate_bps: dir.bitrate_bps,
            propagation_ps: dir.delay_ps,
            tx_latency_ps: tx_latency,
            peer_rx_latency_ps: rx_latency,
        })
    }
    pub(super) fn gptp_cause(
        &self,
        kind: CauseKind,
        d: &mut NetworkDelta,
        c: &Context<'_>,
    ) -> Result<()> {
        let cfg = self
            .gcfg()
            .ok_or_else(|| gerr("cause without prepared instance"))?;
        let limit = cfg
            .max_journal_entries
            .checked_mul(32)
            .ok_or_else(|| gerr("causal history limit overflow"))?;
        let state = self.gm(d)?;
        if state.causes.len() >= limit {
            return Err(gerr("complete causal history limit exceeded"));
        }
        if state
            .causes
            .last()
            .is_some_and(|previous| previous.at_ps > c.now())
        {
            return Err(gerr("causal history order reversed"));
        }
        state.causes.push(state::Cause {
            at_ps: c.now(),
            kind,
        });
        Ok(())
    }
    pub(super) fn gcfg(&self) -> Option<&crate::types::network::gptp::PreparedGptp> {
        self.prepared.gptp.as_ref()
    }
    pub(super) fn gs<'a>(&'a self, d: &'a NetworkDelta) -> Option<&'a state::State> {
        d.gptp.as_ref().or(self.gptp.as_ref())
    }
    pub(super) fn gm<'a>(&self, d: &'a mut NetworkDelta) -> Result<&'a mut state::State> {
        if d.gptp.is_none() {
            d.gptp = self.gptp.clone();
        }
        d.gptp.as_mut().ok_or_else(|| gerr("missing packet owner"))
    }
    pub(in crate::runtime::network) fn gptp_packet(&self, d: &NetworkDelta, t: usize) -> bool {
        self.gs(d).is_some_and(|s| s.packets.contains_key(&t))
    }
    pub(super) fn gptp_port_identity(&self, direction: usize) -> PortIdentity {
        let cfg = self.gcfg().unwrap();
        let p = cfg.ports[direction].as_ref().unwrap();
        PortIdentity {
            clock: cfg.nodes[p.node].clock_identity,
            port: p.port_number,
        }
    }
    pub(super) fn gptp_raw(&self, node: usize, at: u64) -> Result<Rat> {
        self.gptp_raw_reference(node, Rat::new(i128::from(at), 1).map_err(gerr)?)
    }
    fn gptp_raw_reference(&self, node: usize, at: Rat) -> Result<Rat> {
        let cfg = self
            .gcfg()
            .ok_or_else(|| gerr("missing prepared instance"))?;
        let initial = &self
            .prepared
            .clocks
            .as_ref()
            .ok_or_else(|| gerr("missing initial clock"))?
            .initial[cfg.nodes[node].device];
        Rat::new(initial.offset_ps, 1)
            .map_err(gerr)?
            .add(
                at.mul(
                    Rat::new(i128::from(initial.rate_num), i128::from(initial.rate_den))
                        .map_err(gerr)?,
                )
                .map_err(gerr)?,
            )
            .map_err(gerr)
    }
    /// Return the first global event-grid instant at which the immutable local
    /// oscillator reaches a protocol deadline. NodeClock servo steps do not enter.
    pub(super) fn gptp_global_deadline(&self, node: usize, local: Rat) -> Result<u64> {
        let cfg = self
            .gcfg()
            .ok_or_else(|| gerr("missing prepared instance"))?;
        let initial = &self.prepared.clocks.as_ref().unwrap().initial[cfg.nodes[node].device];
        let elapsed = local
            .sub(Rat::new(initial.offset_ps, 1).map_err(gerr)?)
            .map_err(gerr)?
            .mul(
                Rat::new(i128::from(initial.rate_den), i128::from(initial.rate_num))
                    .map_err(gerr)?,
            )
            .map_err(gerr)?;
        let at = elapsed.ceil().map_err(gerr)?.max(0);
        u64::try_from(at).map_err(|_| gerr("raw-local protocol deadline exceeds global grid"))
    }
    pub(super) fn gptp_local_deadline(
        &self,
        node: usize,
        now: u64,
        interval_ps: u64,
    ) -> Result<(Rat, u64)> {
        let local = self
            .gptp_raw(node, now)?
            .add(Rat::new(i128::from(interval_ps), 1).map_err(gerr)?)
            .map_err(gerr)?;
        Ok((local, self.gptp_global_deadline(node, local)?))
    }
    pub(super) fn gptp_record(
        &self,
        kind: &str,
        t: Option<usize>,
        reason: Option<&str>,
        data: Value,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let id = format!("gptp:{}:{}", kind, d.record_sequence);
        let mut fields = json!({"kind":kind,"global_ps":c.now().to_string(),"reason":reason,
            "transfer_id":t.map(|t|self.transfer(d,t).row.transfer_id.clone()),
            "frame_id":t.map(|t|self.transfer(d,t).row.frame_id.clone()),"details":data});
        if let Some(t) = t {
            if let Some(packet) = self.gs(d).and_then(|s| s.packets.get(&t)) {
                fields["packet_id"] = json!(packet.id.to_string());
                fields["message_type"] = json!(packet.message.body.message_type());
                fields["sequence"] = json!(packet.message.sequence.to_string());
                fields["source_clock_identity"] = json!(hex(&packet.message.source.clock));
                fields["source_port_number"] = json!(packet.message.source.port.to_string());
                fields["tx_reference_ps"] = json!(packet.tx.as_ref().map(|c| c.at_ps.to_string()));
                fields["rx_reference_ps"] = json!(packet.rx.as_ref().map(|c| c.at_ps.to_string()));
            }
        }
        Self::record(c, d, "dir.network.gptp", id, fields)
    }
    pub(in crate::runtime::network) fn initialize_gptp(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(cfg) = self.gcfg() else {
            return Ok(());
        };
        self.sync_gptp_links(d, c)?;
        for i in 0..cfg.ports.len() {
            if cfg.ports[i].is_none() {
                continue;
            }
            let node = cfg.ports[i].as_ref().unwrap().node;
            let due = self.gptp_raw(node, c.now())?;
            let port = self.gm(d)?.ports[i].as_mut().unwrap();
            port.pdelay_due_raw = Some(due);
            port.announce_due_raw = Some(due);
            port.sync_due_raw = Some(due);
            for timer in [
                Timer::Pdelay {
                    direction: i,
                    generation: 0,
                },
                Timer::Announce {
                    direction: i,
                    generation: 0,
                },
                Timer::Sync {
                    direction: i,
                    generation: 0,
                },
            ] {
                Self::schedule(c, d, c.now(), Event::Gptp(state::Event::Timer(timer)))?;
            }
        }
        self.gptp_reselect(d, c)?;
        Ok(())
    }
    fn gptp_ethernet_wire(&self, direction: usize, payload: &[u8]) -> Result<EthernetWireFrame> {
        let cfg = self.gcfg().unwrap();
        let port = cfg.ports[direction].as_ref().unwrap();
        let mut bytes = Vec::with_capacity(64.max(payload.len() + 18));
        bytes.extend_from_slice(&codec::MULTICAST);
        bytes.extend_from_slice(&port.source_mac);
        bytes.extend_from_slice(&codec::ETHERTYPE.to_be_bytes());
        bytes.extend_from_slice(payload);
        let pad = 46usize.saturating_sub(payload.len());
        bytes.resize(bytes.len() + pad, 0);
        let fcs = crate::runtime::ethernet::crc32(&bytes).to_le_bytes();
        bytes.extend_from_slice(&fcs);
        Ok(EthernetWireFrame {
            src_mac: mac(port.source_mac),
            dst_mac: mac(codec::MULTICAST),
            ether_type: codec::ETHERTYPE,
            data_hex: hex(payload),
            tag: None,
            tags: vec![],
            pad_bytes: pad as u64,
            mac_bytes: bytes.len() as u64,
            fcs_hex: hex(&fcs),
            mac_hex: hex(&bytes),
        })
    }
    pub(super) fn gptp_send(
        &self,
        direction: usize,
        message: Message,
        relay: Option<state::Relay>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<Option<usize>> {
        let cfg = self.gcfg().ok_or_else(|| gerr("missing instance"))?;
        let port = cfg.ports[direction]
            .as_ref()
            .ok_or_else(|| gerr("unconfigured output"))?;
        let link = &self.prepared.ethernet.directions[direction];
        let body = message.encode().map_err(gerr)?;
        let wire = self.gptp_ethernet_wire(direction, &body)?;
        let p = usize::from(cfg.priority);
        let q = self.queue(d, direction);
        let qcfg = self
            .prepared
            .ethernet
            .outputs
            .iter()
            .find(|o| o.port == link.from_port)
            .and_then(|o| o.queues.iter().find(|q| q.priority == cfg.priority));
        let up = self.policy(d).is_none_or(|policy| {
            policy
                .link_up
                .get(&link.from_port)
                .copied()
                .unwrap_or(false)
        });
        let allowed = up
            && l2::admission(
                q.waiting.iter().map(|v| v.len() as u64).sum(),
                self.prepared.ethernet.devices[link.source].queue_capacity,
                q.waiting[p].len() as u64,
                q.bytes[p],
                qcfg,
                wire.mac_bytes,
            )?;
        let state = self.gs(d).unwrap();
        let id = state.next_packet;
        let f = self.frames.len()
            + d.frames
                .iter()
                .filter(|(i, _)| *i >= self.frames.len())
                .count();
        let t = self.transfers.len()
            + d.transfers
                .iter()
                .filter(|(i, _)| *i >= self.transfers.len())
                .count();
        let node = &cfg.nodes[port.node];
        let frame_id = format!(
            "gptp:{}:{id}",
            self.prepared.ethernet.devices[node.device].id
        );
        let transfer_id = format!("{frame_id}@{}", link.from_port);
        let row = EthernetFrameRecord {
            frame_id: frame_id.clone(),
            time_ps: c.now(),
            source: self.prepared.ethernet.devices[node.device].id.clone(),
            source_vlan_id: self
                .prepared
                .ethernet
                .port_policies
                .iter()
                .find(|o| o.port == link.from_port)
                .map(|o| o.pvid),
            wire: wire.clone(),
            flow_id: None,
            priority: cfg.priority,
            deadline_ps: None,
            generated_ps: c.now(),
            ready_ps: Some(c.now()),
        };
        d.frames.push((
            f,
            Frame {
                generator: None,
                row,
                ip: None,
                visits: 0,
                lineage: None,
                branch_id: None,
                gateway_injection: None,
                terminal_indications: Vec::new(),
            },
        ));
        d.transfers.push((
            t,
            Transfer {
                circuit: None,
                fragments: self.preemption_direction(direction).then(Default::default),
                row: EthernetTransferRecord {
                    transfer_id: transfer_id.clone(),
                    time_ps: c.now(),
                    frame_id,
                    parent_transfer_id: None,
                    queue_id: Some(format!("{}.queue.{}", link.from_port, cfg.priority)),
                    priority: cfg.priority,
                    wire,
                    vlan_id: None,
                    from_port: link.from_port.clone(),
                    to_port: link.to_port.clone(),
                    queued_ps: c.now(),
                    sof_ps: None,
                    eof_ps: None,
                    release_ps: None,
                    arrival_ps: None,
                    planned_eof_ps: None,
                    planned_release_ps: None,
                    planned_arrival_ps: None,
                    status: if allowed { "queued" } else { "dropped" }.into(),
                    drop_reason: (!allowed)
                        .then(|| if up { "queue_full" } else { "link_down" }.into()),
                    media: self.configured_endpoint(direction).map(|(i, _)| {
                        EthernetMediaTransfer {
                            physical_link: self
                                .prepared
                                .ethernet
                                .media
                                .as_ref()
                                .unwrap()
                                .physical_links[i]
                                .id
                                .clone(),
                            ..Default::default()
                        }
                    }),
                },
                frame: f,
                direction,
                visit: 0,
                offer_epoch: self.epoch_after(d),
                sof_epoch: None,
            },
        ));
        let link_epoch = self.gs(d).unwrap().ports[direction]
            .as_ref()
            .unwrap()
            .link_epoch;
        let state = self.gm(d)?;
        state.next_packet = add(id, 1)?;
        state.packets.insert(
            t,
            state::Packet {
                id,
                transfer: t,
                frame: f,
                direction,
                link_epoch,
                mac_packet: None,
                physical_epoch: None,
                mac_source: None,
                tx_completed_at_ps: None,
                admitted: allowed,
                message,
                received_mac_hex: None,
                received_message: None,
                relay,
                tx: None,
                rx: None,
                rx_validated: false,
                tx_eof: false,
            },
        );
        self.gptp_cause(CauseKind::Sent { transfer: t }, d, c)?;
        if allowed {
            let mac_bytes = self.transfer(d, t).row.wire.mac_bytes;
            let q = self.queue_mut(d, direction);
            q.waiting[p].push_back(t);
            q.bytes[p] = add(q.bytes[p], mac_bytes)?;
            c.request_arbitration("network").map_err(model)?;
        }
        self.gptp_record(
            "packet_queued",
            Some(t),
            (!allowed).then_some(if up { "queue_full" } else { "link_down" }),
            json!({"port":link.from_port,"wire_hex":self.transfer(d,t).row.wire.mac_hex}),
            d,
            c,
        )?;
        Ok(allowed.then_some(t))
    }
    pub(in crate::runtime::network) fn gptp_tx_start(
        &self,
        t: usize,
        mac_packet: Option<u64>,
        physical_epoch: u64,
        sof: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(packet) = self.gs(d).and_then(|s| s.packets.get(&t)).cloned() else {
            return Ok(());
        };
        let epoch = packet.link_epoch;
        let state = self.gs(d).unwrap();
        if state.ports[packet.direction].as_ref().unwrap().link_epoch != epoch {
            return Ok(());
        }
        let stored = self.gm(d)?.packets.get_mut(&t).unwrap();
        stored.mac_packet = mac_packet;
        stored.physical_epoch = Some(physical_epoch);
        // A sampled retry starts a new physical attempt. Its failed provisional
        // SFD must never become the successful attempt's FollowUp timestamp.
        if self.phy_attachment_for(packet.direction).is_some() {
            stored.tx = None;
            stored.rx = None;
            stored.tx_eof = false;
            stored.tx_completed_at_ps = None;
        }
        let source = if self.phy_attachment_for(packet.direction).is_some() {
            None
        } else if let Some(packet_id) = mac_packet {
            Some(state::MacSource::MergeExpress {
                transfer: t,
                direction: packet.direction,
                packet_id,
                epoch: physical_epoch,
            })
        } else if self.preemption_direction(packet.direction) {
            Some(self.gptp_abstract_source(t, d)?.source)
        } else if let Some((domain, side)) = self.configured_endpoint(packet.direction) {
            let index = self.media_state(d).pairs[domain].macs[side]
                .emitting
                .ok_or_else(|| gerr("configured SOF lacks actual emitting attempt"))?;
            let a = &self.media_state(d).attempts[index];
            Some(state::MacSource::OrdinaryAttempt(
                state::ConfiguredAttempt {
                    index,
                    transfer: t,
                    number: a.number,
                    direction: packet.direction,
                    physical_link: domain,
                    side,
                },
            ))
        } else {
            None
        };
        if let Some(source) = source {
            let proof = self.gptp_source_proof(source, CaptureKind::Tx, d)?;
            if proof.sof_ps != sof {
                return Err(gerr("serializer actual SOF differs"));
            }
            self.gm(d)?.packets.get_mut(&t).unwrap().mac_source = Some(source);
        }
        self.gptp_cause(CauseKind::Sof { transfer: t }, d, c)?;
        if self.phy_attachment_for(packet.direction).is_some() {
            return self.gptp_record(
                "physical_sof",
                Some(t),
                None,
                json!({"sof_ps":sof.to_string(),"physical_attempt":physical_epoch.to_string()}),
                d,
                c,
            );
        }
        let link = &self.prepared.ethernet.directions[packet.direction];
        let binding = self.gs(d).unwrap().packets[&t].mac_source;
        let (reference, rx) = if let Some(binding) = binding {
            (
                self.gptp_source_proof(binding, CaptureKind::Tx, d)?
                    .reference_ps,
                self.gptp_source_proof(binding, CaptureKind::Rx, d)?
                    .reference_ps,
            )
        } else {
            let tx = add(sof, reference_duration(link.bitrate_bps)?)?;
            (tx, add(tx, link.delay_ps)?)
        };
        Self::schedule(
            c,
            d,
            reference,
            Event::Gptp(state::Event::Capture {
                transfer: t,
                packet_id: packet.id,
                link_epoch: epoch,
                kind: CaptureKind::Tx,
            }),
        )?;
        Self::schedule(
            c,
            d,
            rx,
            Event::Gptp(state::Event::Capture {
                transfer: t,
                packet_id: packet.id,
                link_epoch: epoch,
                kind: CaptureKind::Rx,
            }),
        )?;
        self.gptp_record(
            "physical_sof",
            Some(t),
            None,
            json!({"sof_ps":sof.to_string(),"tx_reference_planned_ps":reference.to_string(),
            "rx_reference_planned_ps":rx.to_string()}),
            d,
            c,
        )
    }
    // Keep the actual producer identity, reference plane and observation
    // context explicit at this narrow boundary instead of inventing defaults.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::runtime::network) fn gptp_sampled_capture(
        &self,
        t: usize,
        kind: CaptureKind,
        attachment: usize,
        reference_sample: u64,
        physical_attempt: u32,
        packet_generation: Option<u64>,
        plane: state::ReferencePlane,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(packet) = self.gs(d).and_then(|s| s.packets.get(&t)).cloned() else {
            return Ok(());
        };
        if self.phy_attachment_for(packet.direction) != Some(attachment) {
            return Err(gerr("sampled capture crossed physical owner"));
        }
        let cfg = &self.prepared.phy_attachments[attachment].config;
        let expected_plane = if cfg.medium.gigabit_raw_merge {
            state::ReferencePlane::MdiMergeDelimiter
        } else {
            state::ReferencePlane::MdiMacSfd
        };
        if !cfg.medium.exact_mdi_timestamp_mapping_supported() || plane != expected_plane {
            return Err(gerr(
                "sampled capture reference plane differs from physical owner",
            ));
        }
        let exact = cfg
            .medium
            .channel
            .grid
            .time(reference_sample)
            .map_err(gerr)?;
        let reference = Rat::new(
            i128::try_from(exact.numerator_ps).map_err(|_| gerr("sample reference overflow"))?,
            i128::try_from(exact.denominator).map_err(|_| gerr("sample denominator overflow"))?,
        )
        .map_err(gerr)?
        .add(Rat::new(i128::from(cfg.origin_ps), 1).map_err(gerr)?)
        .map_err(gerr)?;
        let at_ps = u64::try_from(reference.ceil().map_err(gerr)?)
            .map_err(|_| gerr("sample reference exceeds event grid"))?;
        if at_ps > c.now() || packet.physical_epoch.is_none() {
            return Err(gerr(
                "sampled capture precedes physical observation or actual SOF",
            ));
        }
        if !cfg.medium.gigabit_raw_merge
            && packet.physical_epoch != Some(u64::from(physical_attempt))
        {
            return Err(gerr("sampled capture physical attempt differs"));
        }
        let port = self.gs(d).unwrap().ports[packet.direction]
            .as_ref()
            .unwrap();
        if port.link_epoch != packet.link_epoch || !port.link_up {
            return Ok(());
        }
        let gcfg = self.gcfg().unwrap();
        let direction = if kind == CaptureKind::Tx {
            packet.direction
        } else {
            gcfg.reverse[packet.direction]
        };
        let node = gcfg.ports[direction].as_ref().unwrap().node;
        let raw = self.gptp_raw_reference(node, reference)?;
        let provenance = state::SampledCapture {
            attachment,
            reference_sample,
            reference,
            observed_at_ps: c.now(),
            physical_attempt,
            packet_generation,
            plane,
        };
        let capture = state::Capture {
            at_ps,
            raw,
            packet_id: packet.id,
            link_epoch: packet.link_epoch,
            sampled: Some(provenance),
            configured: None,
            express: None,
        };
        let saved = self.gm(d)?.packets.get_mut(&t).unwrap();
        let slot = if kind == CaptureKind::Tx {
            &mut saved.tx
        } else {
            &mut saved.rx
        };
        if slot.is_some() {
            return Err(gerr("duplicate sampled timestamp capture"));
        }
        *slot = Some(capture);
        self.gptp_cause(
            CauseKind::SampledCapture {
                transfer: t,
                kind,
                provenance,
            },
            d,
            c,
        )?;
        self.gptp_record(
            if kind == CaptureKind::Tx {
                "tx_capture"
            } else {
                "rx_capture"
            },
            Some(t),
            None,
            json!({"raw_numerator":raw.num.to_string(),"raw_denominator":raw.den.to_string(),
                "reference_ps":at_ps.to_string(),"link_epoch":packet.link_epoch.to_string(),
                "sampled":provenance}),
            d,
            c,
        )?;
        self.gptp_try_complete_tx(t, d, c)
    }
    pub(in crate::runtime::network) fn gptp_tx_eof(
        &self,
        t: usize,
        mac_packet: Option<u64>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(packet) = self.gs(d).and_then(|s| s.packets.get(&t)).cloned() else {
            return Ok(());
        };
        if packet.mac_packet != mac_packet {
            return Err(gerr("EOF MAC packet identity differs"));
        }
        if let Some(source) = packet.mac_source {
            self.gptp_source_proof(source, CaptureKind::Tx, d)?;
            match source {
                state::MacSource::OrdinaryAttempt(binding) => {
                    let a = &self.media_state(d).attempts[binding.index];
                    if a.status != "serialized" || a.eof != c.now() {
                        return Err(gerr("EOF lacks actual serialized attempt"));
                    }
                }
                _ => {
                    let evidence = self.gptp_source_evidence(source, d)?;
                    if evidence.source != source || evidence.eof_actual_ps != Some(c.now()) {
                        return Err(gerr("EOF lacks actual serializer source completion"));
                    }
                }
            }
        }
        if packet.tx_eof {
            return Err(gerr("duplicate gPTP EOF"));
        }
        self.gm(d)?.packets.get_mut(&t).unwrap().tx_eof = true;
        self.gptp_cause(CauseKind::Eof { transfer: t }, d, c)?;
        self.gptp_record(
            "physical_eof",
            Some(t),
            None,
            json!({"eof_ps":c.now().to_string()}),
            d,
            c,
        )?;
        self.gptp_try_complete_tx(t, d, c)
    }
    fn gptp_try_complete_tx(
        &self,
        t: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let packet = self.gs(d).unwrap().packets[&t].clone();
        if !packet.tx_eof || packet.tx.is_none() || packet.tx_completed_at_ps.is_some() {
            return Ok(());
        }
        let capture = packet.tx.as_ref().unwrap();
        if capture.link_epoch != packet.link_epoch {
            return Err(gerr("TX capture link epoch mismatch"));
        }
        self.gm(d)?.packets.get_mut(&t).unwrap().tx_completed_at_ps = Some(c.now());
        self.gptp_cause(CauseKind::TxComplete { transfer: t }, d, c)?;
        let port = self.gs(d).unwrap().ports[packet.direction]
            .as_ref()
            .unwrap();
        if port.link_epoch != packet.link_epoch || !port.link_up {
            return Ok(());
        }
        match packet.message.body {
            Body::PdelayResp { requester, .. } => {
                let (stamp, fraction) = timestamp_part(capture.raw)?;
                let fup = Message {
                    source: packet.message.source.clone(),
                    sequence: packet.message.sequence,
                    correction: to_wire_correction(fraction)?,
                    log_interval: 127,
                    body: Body::PdelayRespFollowUp {
                        response_origin: stamp,
                        requester,
                    },
                };
                self.gptp_send(packet.direction, fup, None, d, c)?;
                crate::allocation::reservation_checkpoint("network_gptp_followup")?;
            }
            Body::Sync => {
                let cfg = self.gcfg().unwrap();
                let node = cfg.ports[packet.direction].as_ref().unwrap().node;
                if self.gs(d).unwrap().ports[packet.direction]
                    .as_ref()
                    .unwrap()
                    .role
                    != Role::Master
                {
                    return Ok(());
                }
                let (origin, correction, ratio) = if let Some(relay) = &packet.relay {
                    if relay.epoch != self.gs(d).unwrap().nodes[node].epoch {
                        return Ok(());
                    }
                    (
                        relay.origin,
                        math::relay_correction(
                            relay.correction,
                            relay.gm_local_ratio,
                            capture.raw,
                            relay.upstream_raw,
                        )
                        .map_err(gerr)?,
                        relay.gm_local_ratio,
                    )
                } else {
                    let (stamp, fraction) = timestamp_part(capture.raw)?;
                    (stamp, fraction, Rat::ONE)
                };
                let scaled = codec::rate_to_scaled(ratio.num, ratio.den).map_err(gerr)?;
                let fup = Message {
                    source: packet.message.source.clone(),
                    sequence: packet.message.sequence,
                    correction: to_wire_correction(correction)?,
                    log_interval: cfg.log_sync_interval,
                    body: Body::FollowUp {
                        precise_origin: origin,
                        info: FollowUpInfo {
                            cumulative_scaled_rate_offset: scaled,
                            gm_time_base_indicator: 0,
                            last_gm_phase_change: [0; 12],
                            scaled_last_gm_freq_change: 0,
                        },
                    },
                };
                self.gptp_send(packet.direction, fup, None, d, c)?;
                crate::allocation::reservation_checkpoint("network_gptp_followup")?;
            }
            _ => {}
        }
        Ok(())
    }
}

impl NetworkState {
    pub(in crate::runtime::network) fn gptp_event(
        &self,
        event: state::Event,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        match event {
            state::Event::Timer(timer) => {
                self.gptp_cause(CauseKind::Timer(timer), d, c)?;
                self.gptp_timer(timer, d, c)
            }
            state::Event::Capture {
                transfer,
                packet_id,
                link_epoch,
                kind,
            } => {
                let Some(packet) = self.gs(d).and_then(|s| s.packets.get(&transfer)).cloned()
                else {
                    return Err(gerr("timestamp capture packet missing"));
                };
                if packet.id != packet_id || packet.link_epoch != link_epoch {
                    return Err(gerr("timestamp capture ownership mismatch"));
                }
                if self.phy_attachment_for(packet.direction).is_some() {
                    return Err(gerr(
                        "sampled PHY capture requires an actual marker observation",
                    ));
                }
                let port = self.gs(d).unwrap().ports[packet.direction]
                    .as_ref()
                    .unwrap();
                if port.link_epoch != link_epoch || !port.link_up {
                    return Ok(());
                }
                self.gptp_cause(CauseKind::Capture { transfer, kind }, d, c)?;
                let cfg = self.gcfg().unwrap();
                let receiving_direction = cfg.reverse[packet.direction];
                let node = if kind == CaptureKind::Tx {
                    cfg.ports[packet.direction].as_ref().unwrap().node
                } else {
                    cfg.ports[receiving_direction].as_ref().unwrap().node
                };
                let sof = self
                    .transfer(d, transfer)
                    .row
                    .sof_ps
                    .ok_or_else(|| gerr("capture before actual SOF"))?;
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
                let proof = packet
                    .mac_source
                    .map(|source| self.gptp_source_proof(source, kind, d))
                    .transpose()?;
                let configured = proof.filter(|p| p.endpoint.is_some());
                let express = proof.filter(|p| p.endpoint.is_none());
                let expected = proof.map_or(expected, |proof| proof.reference_ps);
                if self.configured_endpoint(packet.direction).is_some() != configured.is_some() {
                    return Err(gerr("configured capture lacks bound source"));
                }
                if expected != c.now() {
                    return Err(gerr("SFD reference point timing mismatch"));
                }
                let raw = self.gptp_raw(node, c.now())?;
                let capture = state::Capture {
                    at_ps: c.now(),
                    raw,
                    packet_id,
                    link_epoch,
                    sampled: None,
                    configured,
                    express,
                };
                let saved = self.gm(d)?.packets.get_mut(&transfer).unwrap();
                let slot = if kind == CaptureKind::Tx {
                    &mut saved.tx
                } else {
                    &mut saved.rx
                };
                if slot.replace(capture).is_some() {
                    return Err(gerr("duplicate physical timestamp capture"));
                }
                let mut details = json!({"raw_numerator":raw.num.to_string(),"raw_denominator":raw.den.to_string(),
                    "reference_ps":c.now().to_string(),"link_epoch":link_epoch.to_string()});
                if let Some(proof) = configured {
                    details["configured"] = decimal(json!(proof));
                }
                if let Some(proof) = express {
                    details["express"] = decimal(json!(proof));
                }
                self.gptp_record(
                    if kind == CaptureKind::Tx {
                        "tx_capture"
                    } else {
                        "rx_capture"
                    },
                    Some(transfer),
                    None,
                    details,
                    d,
                    c,
                )?;
                crate::allocation::reservation_checkpoint("network_gptp_capture")?;
                self.gptp_try_complete_tx(transfer, d, c)
            }
        }
    }
    /// The caller stages every effect at this physical cut first. Busy MACs do
    /// not withdraw the link; only the PHY's bilateral trained state does.
    pub(in crate::runtime::network) fn gptp_phy_readiness_changed(
        &self,
        index: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(cfg) = self.gcfg() else {
            return Ok(());
        };
        let attachment = &self.prepared.phy_attachments[index];
        if !attachment
            .directions
            .iter()
            .any(|&direction| cfg.ports[direction].is_some())
        {
            return Ok(());
        }
        let ready = d.phy.as_ref().unwrap_or(&self.phy)[index]
            .gigabit_bilateral_link_ready()
            .map_err(gerr)?;
        let previous = self.gs(d).unwrap().physical_links.get(&index).copied();
        if previous.is_none() && !ready || previous.is_some_and(|proof| proof.ready == ready) {
            return Ok(());
        }
        let epoch = previous.map_or(Ok(0), |proof| add(proof.epoch, 1))?;
        self.gm(d)?
            .physical_links
            .insert(index, state::PhysicalReadiness { ready, epoch });
        self.gptp_cause(
            CauseKind::PhysicalReadiness {
                attachment: index,
                ready,
                epoch,
            },
            d,
            c,
        )?;
        self.gptp_record("physical_readiness",None,(!ready).then_some("actual_phy_readiness_withdrawn"),
            json!({"attachment":index.to_string(),"ready":ready,"physical_epoch":epoch.to_string()}),d,c)?;
        self.sync_gptp_links(d, c)?;
        c.request_arbitration("network").map_err(model)
    }
    fn gptp_retire_waiting_epoch(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let queue = self.queue(d, direction);
        let retired: Vec<_> = queue
            .waiting
            .iter()
            .enumerate()
            .flat_map(|(class, waiting)| {
                waiting
                    .iter()
                    .copied()
                    .filter(|&t| self.gptp_packet(d, t))
                    .map(move |t| (class, t))
            })
            .collect();
        for (class, t) in retired {
            let mut transfer = self.transfer(d, t).clone();
            let bytes = transfer.row.wire.mac_bytes;
            let queue = self.queue_mut(d, direction);
            queue.waiting[class].retain(|&queued| queued != t);
            queue.bytes[class] = queue.bytes[class]
                .checked_sub(bytes)
                .ok_or_else(|| gerr("retired protocol queue byte underflow"))?;
            transfer.row.status = "dropped".into();
            transfer.row.drop_reason = Some("gptp_physical_epoch_withdrawn".into());
            transfer.row.time_ps = c.now();
            put(&mut d.transfers, t, transfer);
            self.gptp_record(
                "packet_retired",
                Some(t),
                Some("physical_link_epoch_withdrawn"),
                json!({}),
                d,
                c,
            )?;
        }
        Ok(())
    }
    fn gptp_change_link(
        &self,
        i: usize,
        up: bool,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<bool> {
        let Some(port) = self
            .gs(d)
            .and_then(|s| s.ports.get(i))
            .and_then(Option::as_ref)
        else {
            return Ok(false);
        };
        if port.link_up == up {
            return Ok(false);
        }
        let p = self.gm(d)?.ports[i].as_mut().unwrap();
        p.link_up = up;
        p.link_epoch = add(p.link_epoch, 1)?;
        p.pending = None;
        p.last_sample = None;
        p.last_sample_peer = None;
        p.last_sample_index = None;
        p.rate = None;
        p.delay = None;
        p.as_capable = false;
        p.lost_responses = 0;
        p.candidate = None;
        p.peer = None;
        p.role = Role::Disabled;
        if !up && self.phy_attachment_for(i).is_some() {
            self.gptp_retire_waiting_epoch(i, d, c)?;
        }
        self.gptp_cause(CauseKind::Link { direction: i, up }, d, c)?;
        Ok(true)
    }
    /// Observe each ordered administrative source operation, including a down/up
    /// pair whose final projected policy is unchanged. Physical readiness remains
    /// a separate prerequisite for raising the protocol link.
    pub(in crate::runtime::network) fn gptp_source_link(
        &self,
        i: usize,
        administrative_up: bool,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self
            .gs(d)
            .and_then(|s| s.ports.get(i))
            .and_then(Option::as_ref)
            .is_none()
        {
            return Ok(());
        }
        let physical_up = if let Some(index) = self.phy_attachment_for(i) {
            match self.gs(d).unwrap().physical_links.get(&index) {
                None => true,
                Some(proof) => {
                    proof.ready
                        && d.phy.as_ref().unwrap_or(&self.phy)[index]
                            .gigabit_bilateral_link_ready()
                            .map_err(gerr)?
                }
            }
        } else {
            true
        };
        let up = administrative_up && physical_up;
        if !self.gptp_change_link(i, up, d, c)? {
            return Ok(());
        }
        if !up && self.phy_attachment_for(i).is_none() {
            self.gptp_retire_waiting_epoch(i, d, c)?;
        }
        self.gptp_record(
            "link_transition",
            None,
            None,
            json!({
                "port":self.prepared.ethernet.directions[i].from_port,
                "up":up,
                "link_epoch":self.gs(d).unwrap().ports[i].as_ref().unwrap().link_epoch.to_string()
            }),
            d,
            c,
        )?;
        self.gptp_reselect(d, c)
    }
    pub(in crate::runtime::network) fn sync_gptp_links(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(cfg) = self.gcfg() else {
            return Ok(());
        };
        let policy = self.policy(d);
        let mut changed = Vec::new();
        for i in 0..cfg.ports.len() {
            if cfg.ports[i].is_none() {
                continue;
            }
            let name = &self.prepared.ethernet.directions[i].from_port;
            let physical_up = if let Some(index) = self.phy_attachment_for(i) {
                match self.gs(d).unwrap().physical_links.get(&index) {
                    None => true,
                    Some(proof) => {
                        proof.ready
                            && d.phy.as_ref().unwrap_or(&self.phy)[index]
                                .gigabit_bilateral_link_ready()
                                .map_err(gerr)?
                    }
                }
            } else {
                true
            };
            let up = physical_up
                && policy
                    .as_ref()
                    .is_none_or(|p| p.link_up.get(name).copied().unwrap_or(false));
            if self.gptp_change_link(i, up, d, c)? {
                changed.push(i);
            }
        }
        for i in changed.iter().copied() {
            self.gptp_record("link_transition",None,None,json!({"port":self.prepared.ethernet.directions[i].from_port,
                "up":self.gs(d).unwrap().ports[i].as_ref().unwrap().link_up,
                "link_epoch":self.gs(d).unwrap().ports[i].as_ref().unwrap().link_epoch.to_string()}),d,c)?;
        }
        if !changed.is_empty() {
            self.gptp_reselect(d, c)?
        }
        Ok(())
    }
    pub(super) fn gptp_reselect(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let Some(cfg) = self.gcfg() else {
            return Ok(());
        };
        let changes = super::selection::apply(cfg, self.gm(d)?, c.now())?;
        for change in changes {
            self.gptp_record("selection", None, None, json!({
                "node": self.prepared.ethernet.devices[cfg.nodes[change.node].device].id,
                "old_parent": change.old_parent.map(|i| self.prepared.ethernet.directions[i].from_port.clone()),
                "parent": change.parent.map(|i| self.prepared.ethernet.directions[i].from_port.clone()),
                "grandmaster": hex(&change.grandmaster),
                "epoch": change.epoch.to_string(),
                "synchronized": change.parent.is_none() && cfg.nodes[change.node].priority1 != 255
            }), d, c)?;
        }
        Ok(())
    }
    fn gptp_timer(&self, timer: Timer, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let cfg = self.gcfg().ok_or_else(|| gerr("timer without instance"))?;
        match timer {
            Timer::Pdelay {
                direction,
                generation,
            } => {
                let p = self.gs(d).unwrap().ports[direction]
                    .as_ref()
                    .ok_or_else(|| gerr("Pdelay port missing"))?
                    .clone();
                if p.pdelay_generation != generation {
                    return Ok(());
                }
                let node = cfg.ports[direction].as_ref().unwrap().node;
                let due = p
                    .pdelay_due_raw
                    .ok_or_else(|| gerr("Pdelay raw deadline absent"))?;
                if self.gptp_global_deadline(node, due)? != c.now() {
                    return Err(gerr("Pdelay timer differs from raw LocalClock deadline"));
                }
                let next_due = due
                    .add(Rat::new(i128::from(cfg.pdelay_interval_ps), 1).map_err(gerr)?)
                    .map_err(gerr)?;
                let next_at = self.gptp_global_deadline(node, next_due)?;
                if next_at <= c.now() {
                    return Err(gerr(
                        "Pdelay interval is below global event-grid resolution",
                    ));
                }
                if p.pending.is_some() {
                    self.gptp_pdelay_timeout(
                        direction,
                        p.pending.as_ref().unwrap().generation,
                        d,
                        c,
                    )?;
                }
                let p = self.gs(d).unwrap().ports[direction]
                    .as_ref()
                    .unwrap()
                    .clone();
                let next = add(generation, 1)?;
                self.gm(d)?.ports[direction]
                    .as_mut()
                    .unwrap()
                    .pdelay_generation = next;
                self.gm(d)?.ports[direction]
                    .as_mut()
                    .unwrap()
                    .pdelay_due_raw = Some(next_due);
                if p.link_up {
                    if self.gs(d).unwrap().pending_exchange_count() >= cfg.max_pending_exchanges {
                        return Err(gerr("gPTP pending exchange limit exceeded"));
                    }
                    let seq = p.pdelay_seq;
                    let msg = Message {
                        source: self.gptp_port_identity(direction),
                        sequence: seq,
                        correction: 0,
                        log_interval: cfg.log_pdelay_req_interval,
                        body: Body::PdelayReq,
                    };
                    if let Some(t) = self.gptp_send(direction, msg, None, d, c)? {
                        let port = self.gm(d)?.ports[direction].as_mut().unwrap();
                        port.pdelay_seq = port.pdelay_seq.wrapping_add(1);
                        port.pending = Some(state::PdelayPending {
                            generation: next,
                            sequence: seq,
                            request_transfer: t,
                            t1: None,
                            t2: None,
                            t3: None,
                            t4: None,
                            peer: None,
                            response_transfer: None,
                            followup_transfer: None,
                            deadline_raw: next_due,
                            deadline_ps: next_at,
                        });
                        Self::schedule(
                            c,
                            d,
                            next_at,
                            Event::Gptp(state::Event::Timer(Timer::PdelayTimeout {
                                direction,
                                generation: next,
                            })),
                        )?;
                    }
                }
                Self::schedule(
                    c,
                    d,
                    next_at,
                    Event::Gptp(state::Event::Timer(Timer::Pdelay {
                        direction,
                        generation: next,
                    })),
                )?;
            }
            Timer::PdelayTimeout {
                direction,
                generation,
            } => self.gptp_pdelay_timeout(direction, generation, d, c)?,
            Timer::Announce {
                direction,
                generation,
            } => {
                let p = self.gs(d).unwrap().ports[direction]
                    .as_ref()
                    .ok_or_else(|| gerr("Announce port missing"))?
                    .clone();
                if p.announce_generation != generation {
                    return Ok(());
                }
                let node = cfg.ports[direction].as_ref().unwrap().node;
                let due = p
                    .announce_due_raw
                    .ok_or_else(|| gerr("Announce raw deadline absent"))?;
                if self.gptp_global_deadline(node, due)? != c.now() {
                    return Err(gerr("Announce timer differs from raw LocalClock deadline"));
                }
                let next_due = due
                    .add(Rat::new(i128::from(cfg.announce_interval_ps), 1).map_err(gerr)?)
                    .map_err(gerr)?;
                let next_at = self.gptp_global_deadline(node, next_due)?;
                if next_at <= c.now() {
                    return Err(gerr(
                        "Announce interval is below global event-grid resolution",
                    ));
                }
                self.gm(d)?.ports[direction]
                    .as_mut()
                    .unwrap()
                    .announce_generation = add(generation, 1)?;
                self.gm(d)?.ports[direction]
                    .as_mut()
                    .unwrap()
                    .announce_due_raw = Some(next_due);
                if p.role == Role::Master && p.link_up && p.as_capable {
                    let selected = &self.gs(d).unwrap().nodes[node];
                    let (gm, steps, path, quality_bytes, source) =
                        if let Some(parent) = selected.parent {
                            let a = self.gs(d).unwrap().ports[parent]
                                .as_ref()
                                .unwrap()
                                .candidate
                                .as_ref()
                                .unwrap();
                            let mut path = a.path.clone();
                            path.push(cfg.nodes[node].clock_identity);
                            (
                                a.grandmaster,
                                a.steps_removed
                                    .checked_add(1)
                                    .ok_or_else(|| gerr("stepsRemoved overflow"))?,
                                path,
                                a.identity,
                                a.time_source,
                            )
                        } else {
                            let n = &cfg.nodes[node];
                            (
                                n.clock_identity,
                                0,
                                vec![n.clock_identity],
                                quality(n),
                                n.time_source,
                            )
                        };
                    let ann = Announce {
                        current_utc_offset: 0,
                        priority1: quality_bytes[0],
                        clock_class: quality_bytes[1],
                        clock_accuracy: quality_bytes[2],
                        offset_scaled_log_variance: u16::from_be_bytes([
                            quality_bytes[3],
                            quality_bytes[4],
                        ]),
                        priority2: quality_bytes[5],
                        grandmaster_identity: gm,
                        steps_removed: steps,
                        time_source: source,
                        path_trace: path,
                    };
                    let seq = p.announce_seq;
                    self.gm(d)?.ports[direction].as_mut().unwrap().announce_seq =
                        seq.wrapping_add(1);
                    self.gptp_send(
                        direction,
                        Message {
                            source: self.gptp_port_identity(direction),
                            sequence: seq,
                            correction: 0,
                            log_interval: cfg.log_announce_interval,
                            body: Body::Announce(ann),
                        },
                        None,
                        d,
                        c,
                    )?;
                }
                Self::schedule(
                    c,
                    d,
                    next_at,
                    Event::Gptp(state::Event::Timer(Timer::Announce {
                        direction,
                        generation: add(generation, 1)?,
                    })),
                )?;
            }
            Timer::AnnounceTimeout {
                direction,
                generation,
            } => {
                let current = self.gs(d).unwrap().ports[direction]
                    .as_ref()
                    .unwrap()
                    .candidate
                    .as_ref()
                    .is_some_and(|a| a.generation == generation && a.deadline_ps <= c.now());
                if current {
                    self.gm(d)?.ports[direction].as_mut().unwrap().candidate = None;
                    self.gptp_record(
                        "announce_timeout",
                        None,
                        Some("announce_receipt_timeout"),
                        json!({"port":self.prepared.ethernet.directions[direction].from_port}),
                        d,
                        c,
                    )?;
                    self.gptp_reselect(d, c)?;
                }
            }
            Timer::Sync {
                direction,
                generation,
            } => {
                let p = self.gs(d).unwrap().ports[direction]
                    .as_ref()
                    .ok_or_else(|| gerr("Sync port missing"))?
                    .clone();
                if p.sync_generation != generation {
                    return Ok(());
                }
                let node = cfg.ports[direction].as_ref().unwrap().node;
                let due = p
                    .sync_due_raw
                    .ok_or_else(|| gerr("Sync raw deadline absent"))?;
                if self.gptp_global_deadline(node, due)? != c.now() {
                    return Err(gerr("Sync timer differs from raw LocalClock deadline"));
                }
                let next_due = due
                    .add(Rat::new(i128::from(cfg.sync_interval_ps), 1).map_err(gerr)?)
                    .map_err(gerr)?;
                let next_at = self.gptp_global_deadline(node, next_due)?;
                if next_at <= c.now() {
                    return Err(gerr("Sync interval is below global event-grid resolution"));
                }
                self.gm(d)?.ports[direction]
                    .as_mut()
                    .unwrap()
                    .sync_generation = add(generation, 1)?;
                self.gm(d)?.ports[direction].as_mut().unwrap().sync_due_raw = Some(next_due);
                if p.role == Role::Master && p.link_up && p.as_capable {
                    let n = self.gs(d).unwrap().nodes[node].clone();
                    if n.synchronized && (n.parent.is_none() || n.relay.is_some()) {
                        let seq = p.sync_seq;
                        self.gm(d)?.ports[direction].as_mut().unwrap().sync_seq =
                            seq.wrapping_add(1);
                        self.gptp_send(
                            direction,
                            Message {
                                source: self.gptp_port_identity(direction),
                                sequence: seq,
                                correction: 0,
                                log_interval: cfg.log_sync_interval,
                                body: Body::Sync,
                            },
                            n.relay,
                            d,
                            c,
                        )?;
                    }
                }
                Self::schedule(
                    c,
                    d,
                    next_at,
                    Event::Gptp(state::Event::Timer(Timer::Sync {
                        direction,
                        generation: add(generation, 1)?,
                    })),
                )?;
            }
            Timer::SyncTimeout { node, generation } => {
                let n = self.gs(d).unwrap().nodes[node].clone();
                if n.sync_timeout_generation == generation
                    && n.sync_deadline_ps.is_some_and(|at| at <= c.now())
                {
                    let changed = &mut self.gm(d)?.nodes[node];
                    changed.synchronized = false;
                    changed.relay = None;
                    if let Some(parent) = n.parent {
                        self.gm(d)?.ports[parent].as_mut().unwrap().candidate = None;
                    }
                    self.gptp_record(
                        "holdover",
                        None,
                        Some("sync_receipt_timeout"),
                        json!({"node":self.prepared.ethernet.devices[cfg.nodes[node].device].id,
                        "grandmaster":hex(&n.grandmaster),"epoch":n.epoch.to_string()}),
                        d,
                        c,
                    )?;
                    self.gptp_reselect(d, c)?;
                }
            }
        }
        Ok(())
    }
    fn gptp_pdelay_timeout(
        &self,
        direction: usize,
        generation: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(pending) = self
            .gs(d)
            .and_then(|s| s.ports[direction].as_ref())
            .and_then(|p| p.pending.as_ref())
            .cloned()
        else {
            return Ok(());
        };
        if pending.generation != generation || pending.deadline_ps > c.now() {
            return Ok(());
        }
        self.gptp_pdelay_reset(direction, generation, "lost_response", d, c)
    }
    pub(super) fn gptp_pdelay_reset(
        &self,
        direction: usize,
        generation: u64,
        reason: &'static str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(pending) = self
            .gs(d)
            .and_then(|s| s.ports[direction].as_ref())
            .and_then(|p| p.pending.as_ref())
            .cloned()
        else {
            return Ok(());
        };
        if pending.generation != generation {
            return Ok(());
        }
        let cfg = self.gcfg().unwrap();
        let p = self.gm(d)?.ports[direction].as_mut().unwrap();
        p.reset_after_lost_response(cfg.allowed_lost_responses);
        self.gptp_record(if reason == "lost_response"{"pdelay_timeout"}else{"pdelay_reset"},Some(pending.request_transfer),Some(reason),
            json!({"port":self.prepared.ethernet.directions[direction].from_port,
                "lost_responses":self.gs(d).unwrap().ports[direction].as_ref().unwrap().lost_responses}),d,c)?;
        self.gptp_reselect(d, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, sync::Arc};
    #[test]
    fn raw_local_periods_advance_on_opposite_drift_and_ignore_servo() {
        let simulation = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gptp/two-node.ini"),
        )
        .unwrap();
        let prepared = simulation
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap();
        let cfg = prepared.gptp.as_ref().unwrap();
        let interval = cfg.pdelay_interval_ps;
        let faster = NetworkState::new(prepared.clone()).unwrap();
        let a = cfg
            .nodes
            .iter()
            .position(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 1])
            .unwrap();
        let b = cfg
            .nodes
            .iter()
            .position(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 2])
            .unwrap();
        let a_deadline = faster
            .gptp_raw(a, 0)
            .unwrap()
            .add(Rat::new(i128::from(interval), 1).unwrap())
            .unwrap();
        let b_deadline = faster
            .gptp_raw(b, 0)
            .unwrap()
            .add(Rat::new(i128::from(interval), 1).unwrap())
            .unwrap();
        assert_eq!(
            faster.gptp_global_deadline(a, a_deadline).unwrap(),
            interval
        );
        let fast_global = faster.gptp_global_deadline(b, b_deadline).unwrap();
        assert!(fast_global < interval);
        assert!(
            faster.gptp_raw(b, fast_global).unwrap().num * b_deadline.den
                >= b_deadline.num * faster.gptp_raw(b, fast_global).unwrap().den
        );
        assert!(
            faster.gptp_raw(b, fast_global - 1).unwrap().num * b_deadline.den
                < b_deadline.num * faster.gptp_raw(b, fast_global - 1).unwrap().den
        );
        let mut slow_prepared = (**prepared).clone();
        let device = cfg.nodes[b].device;
        slow_prepared.clocks.as_mut().unwrap().initial[device].rate_num = 9999;
        slow_prepared.clocks.as_mut().unwrap().initial[device].rate_den = 10000;
        let mut slower = NetworkState::new(Arc::new(slow_prepared)).unwrap();
        let slow_deadline = slower
            .gptp_raw(b, 0)
            .unwrap()
            .add(Rat::new(i128::from(interval), 1).unwrap())
            .unwrap();
        let slow_global = slower.gptp_global_deadline(b, slow_deadline).unwrap();
        assert!(slow_global > interval);
        let before = slower.gptp_global_deadline(b, slow_deadline).unwrap();
        slower.clocks.as_mut().unwrap()[device]
            .reanchor_and_rate(1000, clock::LocalTime::fraction(9000000, 1).unwrap(), 7, 4)
            .unwrap();
        assert_eq!(
            slower.gptp_global_deadline(b, slow_deadline).unwrap(),
            before
        );
    }
}

#[cfg(test)]
mod configured_atomic_tests {
    use super::*;
    use std::path::Path;
    fn fes(engine: &crate::runtime::registered::Engine) -> Value {
        use crate::runtime::registered::Work;
        let mut work = engine
            .checkpoint_work()
            .iter()
            .map(|(key, work)| {
                (
                    *key,
                    match work {
                        Work::Delivery { envelope, token } => {
                            json!({"key":key,"event_id":envelope.event_id,
                "at":envelope.time_ps,"payload":envelope.payload,"token":token})
                        }
                        Work::Arbitration { owner, resource } => {
                            json!({"key":key,"owner":owner,"resource":resource})
                        }
                    },
                )
            })
            .collect::<Vec<_>>();
        let mut dirty = engine.checkpoint_dirty();
        let mut sequence = engine.sequence;
        // Engine seals one already committed dirty request before entering the
        // callback transaction. Compare the exact logical FES on both sides of
        // that representation change; no callback event or ordering key is dropped.
        if let Some((time, delta, owner, resource)) = dirty.first().cloned() {
            if work
                .first()
                .is_none_or(|(k, _)| (k.0, k.1, k.2) >= (time, delta, 2))
            {
                let key = (time, delta, 2, sequence);
                work.push((key, json!({"key":key,"owner":owner,"resource":resource})));
                work.sort_by_key(|(key, _)| *key);
                dirty.remove(0);
                sequence += 1;
            }
        }
        json!({"work":work.into_iter().map(|(_, work)|work).collect::<Vec<_>>(),"dirty":dirty,"sequence":sequence})
    }
    #[test]
    fn mac_source_capture_and_followup_reservation_failures_are_atomic() {
        for (fixture, admission_fault) in [
            ("model.ini", "network_media_pause_reserve"),
            ("merge.ini", "mac_merge_gptp_source"),
            ("abstract.ini", "preemption_gptp_source"),
        ] {
            for fault in [
                admission_fault,
                "network_gptp_capture",
                "network_gptp_followup",
            ] {
                let p = crate::prepare(
                    &Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../examples/network-composite/gptp-configured")
                        .join(fixture),
                )
                .unwrap();
                let mut engine =
                    crate::runtime::registered::Engine::new(std::sync::Arc::new(p)).unwrap();
                engine.initialize().unwrap();
                crate::allocation::fail_next_reservation(fault);
                let mut failed = false;
                for _ in 0..5000 {
                    let before = engine.checkpoint_network().unwrap();
                    let work = fes(&engine);
                    let committed = engine.snapshot.common.committed_events;
                    engine.step();
                    if engine.snapshot.common.partial {
                        assert_eq!(
                            engine.checkpoint_network().unwrap(),
                            before,
                            "{fault}: state, records and causes"
                        );
                        assert_eq!(fes(&engine), work, "{fault}: FES");
                        assert_eq!(engine.snapshot.common.committed_events, committed);
                        assert!(
                            crate::allocation::reservation_checkpoint(fault).is_ok(),
                            "seam not reached"
                        );
                        failed = true;
                        break;
                    }
                }
                assert!(failed, "no failure {fixture}/{fault}");
            }
        }
    }
}
