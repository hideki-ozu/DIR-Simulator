//! Staged sampled-PHY attachment to the network owner's single FES.
use super::*;
use crate::runtime::ethernet::phy::{
    attachment::{AttachedMedium, AttachmentEffect},
    medium::PhyEffect,
};

impl NetworkState {
    pub(super) fn phy_receiver_port(
        &self,
        direction: usize,
        receiver: usize,
    ) -> Option<&crate::runtime::ethernet::phy::attachment::PhysicalPort> {
        let index = self.phy_attachment_for(direction)?;
        let attachment = &self.prepared.phy_attachments[index];
        if direction == receiver || !attachment.directions.contains(&receiver) {
            return None;
        }
        let port = &self.prepared.ethernet.directions.get(receiver)?.from_port;
        attachment
            .config
            .physical_ports
            .iter()
            .find(|p| &p.tx_port == port)
    }

    /// Reproduce the actual electrical prefix once when restoring a named
    /// attachment, then bind every decoded observation to its logical ingress.
    pub(super) fn validate_phy_ingress_checkpoint(&self, horizon: u64) -> Result<()> {
        let mut explained = std::collections::BTreeSet::new();
        for (owner, medium) in self.phy.iter().enumerate() {
            if medium.config().physical_ports.is_empty() {
                continue;
            }
            medium.verify_physical_ingress_replay().map_err(error)?;
            for proof in medium.physical_ingress_history() {
                if let Some((_, tx)) = self.wire.as_ref().and_then(|h| {
                    h.tx.iter().find(|(_, tx)| {
                        tx.phy_transfer.as_deref() == Some(proof.admitted_transfer_id.as_str())
                    })
                }) {
                    let direction = self
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|d| d.from_port == tx.intent.port)
                        .ok_or_else(|| error("physical control proof source MAC"))?;
                    let receiver = self
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|d| d.from_port == proof.receiver_tx_port)
                        .ok_or_else(|| error("physical control proof receiver MAC"))?;
                    let port = self
                        .phy_receiver_port(direction, receiver)
                        .ok_or_else(|| error("physical control proof crossed domain"))?;
                    let at = medium
                        .sample_time_ps(proof.completed_sample)
                        .map_err(error)?;
                    if self.phy_attachment_for(direction) != Some(owner)
                        || port.endpoint != proof.receiver_endpoint
                        || port.rx_port != proof.receiver_rx_port
                        || at > horizon
                        || proof.rx_sfd_sample > proof.rx_fcs_sample
                        || proof.rx_fcs_sample > proof.completed_sample
                        || proof.mac_hex != crate::runtime::wire::codec::hex(&tx.intent.bytes)
                        || tx.phy_receivers.get(&proof.receiver_rx_port) != Some(&at)
                    {
                        return Err(error(
                            "physical control ingress differs from decoded bytes/time/owner",
                        ));
                    }
                    continue;
                }
                let t = self.transfer_by_id(&proof.admitted_transfer_id)?;
                let transfer = &self.transfers[t];
                if self.phy_admitted.get(&t) != Some(&owner) {
                    return Err(error(
                        "physical ingress proof belongs to another attachment",
                    ));
                }
                let receiver = self
                    .prepared
                    .ethernet
                    .directions
                    .iter()
                    .position(|d| d.from_port == proof.receiver_tx_port)
                    .ok_or_else(|| error("physical ingress proof receiver is not an output"))?;
                let port = self
                    .phy_receiver_port(transfer.direction, receiver)
                    .ok_or_else(|| error("physical ingress proof crossed an electrical domain"))?;
                if port.endpoint != proof.receiver_endpoint
                    || port.rx_port != proof.receiver_rx_port
                    || proof.rx_sfd_sample > proof.rx_fcs_sample
                    || proof.rx_fcs_sample > proof.completed_sample
                {
                    return Err(error("physical ingress proof port or sample order differs"));
                }
                let at = medium
                    .sample_time_ps(proof.completed_sample)
                    .map_err(error)?;
                if at > horizon {
                    return Err(error("physical ingress proof is in the future"));
                }
                // A valid CRC on unexpected bytes is retained as an explicit
                // unmatched physical observation, not delivered under a known
                // application's lineage.
                if proof.mac_hex != transfer.row.wire.mac_hex {
                    continue;
                }
                // MAC Control is consumed below L2, including frames supplied by a
                // raw MAC client rather than the generated-control FIFO.
                if proof.mac_hex.get(24..28) == Some("8808") {
                    if transfer.row.arrival_ps.is_none_or(|observed| observed > at) {
                        return Err(error("consumed MAC Control lacks actual physical receive"));
                    }
                    continue;
                }
                let id = format!(
                    "{}@phy{}@{}",
                    proof.admitted_transfer_id, proof.reception_index, proof.receiver_rx_port
                );
                let matches = self
                    .receptions
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.transfer == t && r.row.reception_id == id)
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return Err(error(
                        "physical proof and network ingress are not bijective",
                    ));
                }
                let (index, reception) = matches[0];
                if !explained.insert(index)
                    || reception.segment_receiver != Some(receiver)
                    || reception.row.ingress != proof.receiver_rx_port
                    || reception.row.observed_ps != at
                    || transfer.row.wire.mac_hex != proof.mac_hex
                {
                    return Err(error(
                        "network ingress differs from actual decoded PHY evidence",
                    ));
                }
            }
        }
        for (index, reception) in self.receptions.iter().enumerate() {
            let t = &self.transfers[reception.transfer];
            if self.phy_attachment_for(t.direction).is_some_and(|owner| {
                !self.prepared.phy_attachments[owner]
                    .config
                    .physical_ports
                    .is_empty()
            }) && !explained.contains(&index)
            {
                return Err(error(
                    "named PHY network reception has no physical observation",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn phy_attachment_for(&self, direction: usize) -> Option<usize> {
        if let Some(bindings) = &self.prepared.bindings {
            let port = bindings.ports.get(direction)?;
            return match bindings.domains.get(port.physical_domain)?.owner {
                crate::types::network::bindings::PhysicalOwner::Sampled { attachment } => {
                    Some(attachment)
                }
                _ => None,
            };
        }
        self.prepared
            .phy_attachments
            .iter()
            .position(|a| a.directions.contains(&direction))
    }

    pub(super) fn phy_mut<'a>(
        &self,
        d: &'a mut NetworkDelta,
        index: usize,
    ) -> Result<&'a mut AttachedMedium> {
        d.phy_boundaries
            .get_or_insert_with(|| self.phy_boundaries.clone())
            .get_mut(index)
            .ok_or_else(|| error("unknown sampled PHY boundary"))?
            .invalidate_forecast();
        if d.phy.is_none() {
            d.phy = Some(self.phy.clone());
        }
        d.phy
            .as_mut()
            .and_then(|states| states.get_mut(index))
            .ok_or_else(|| error("unknown sampled PHY attachment"))
    }

    pub(super) fn sync_phy_links(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        use crate::runtime::ethernet::phy::medium::PhysicalMode;
        let after = self.policy(d);
        let before = self
            .dynamic
            .as_ref()
            .map(|dynamic| dynamic.snapshot_policy());
        for (index, attachment) in self.prepared.phy_attachments.iter().enumerate() {
            if attachment.config.medium.physical_mode
                != PhysicalMode::Ieee1000BaseT1PointToPoint2022
            {
                continue;
            }
            let up = |policy: Option<&PolicySnapshot>| {
                attachment.directions.iter().all(|&direction| {
                    policy.is_none_or(|policy| {
                        policy
                            .link_up
                            .get(&self.prepared.ethernet.directions[direction].from_port)
                            .copied()
                            .unwrap_or(false)
                    })
                })
            };
            let current = up(after.as_ref());
            if current != up(before.as_ref()) {
                self.phy_mut(d, index)?
                    .set_gigabit_mdi_disconnected(!current)
                    .map_err(error)?;
                self.phy_plan_boundary(index, d, c)?;
            }
        }
        Ok(())
    }

    pub(super) fn initialize_phy(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        _limit: u64,
    ) -> Result<()> {
        if self.phy.is_empty() {
            return Ok(());
        }
        let mut wakes = self.phy_wakes.clone();
        for (index, medium) in self.phy.iter().enumerate() {
            if medium.config().medium.physical_mode
                == crate::runtime::ethernet::phy::medium::PhysicalMode::Ieee1000BaseT1PointToPoint2022
                && self.policy(d).is_some_and(|policy| self.prepared.phy_attachments[index].directions.iter()
                    .any(|&direction| !policy.link_up.get(&self.prepared.ethernet.directions[direction].from_port)
                        .copied().unwrap_or(false))) {
                self.phy_mut(d, index)?.set_gigabit_mdi_disconnected(true).map_err(error)?;
            }
            if medium.config().medium.physical_mode.is_named() {
                self.phy_plan_boundary(index, d, c)?;
                continue;
            }
            let at = medium.next_event_ps().map_err(error)?;
            let sample_index = medium.next_sample_index();
            Self::schedule(
                c,
                d,
                at,
                Event::PhySample {
                    attachment: index,
                    sample_index,
                },
            )?;
            wakes[index] = Some(sample_index);
        }
        d.phy_wakes = Some(wakes);
        Ok(())
    }

    pub(super) fn phy_admit(
        &self,
        direction: usize,
        transfer: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let index = self
            .phy_attachment_for(direction)
            .ok_or_else(|| error("PHY direction not attached"))?;
        let row = self.transfer(d, transfer).row.clone();
        let from_port = self.prepared.ethernet.directions[direction]
            .from_port
            .clone();
        self.phy_mut(d, index)?
            .admit(&from_port, &row.transfer_id, c.now(), &row.wire)
            .map_err(error)?;
        let admitted = d
            .phy_admitted
            .get_or_insert_with(|| self.phy_admitted.clone());
        if admitted.insert(transfer, index).is_some() {
            return Err(error("duplicate sampled PHY admission"));
        }
        self.phy_service_claim(direction, transfer, d, c)?;
        Self::record(
            c,
            d,
            "dir.network.phy",
            format!("phy:admit:{}", row.transfer_id),
            json!({"kind":"admitted","attachment":index.to_string(),"direction":from_port,
                "transfer_id":row.transfer_id,"mac_bytes":row.wire.mac_bytes.to_string()}),
        )?;
        if self.prepared.phy_attachments[index]
            .config
            .medium
            .physical_mode
            .is_named()
        {
            self.phy_plan_boundary(index, d, c)?;
        }
        Ok(())
    }

    fn transfer_by_id(&self, id: &str) -> Result<usize> {
        self.transfers
            .iter()
            .position(|t| t.row.transfer_id == id)
            .ok_or_else(|| error(format!("sampled PHY references unknown transfer: {id}")))
    }

    pub(super) fn phy_sample(
        &self,
        index: usize,
        sample_index: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        _limit: u64,
    ) -> Result<()> {
        if self.phy_wakes.get(index) != Some(&Some(sample_index))
            || self.phy.get(index).is_none_or(|a| {
                a.next_sample_index() != sample_index || a.next_event_ps().ok() != Some(c.now())
            })
        {
            return Err(error("stale or mismatched sampled PHY wake"));
        }
        let effects = self
            .phy_mut(d, index)?
            .advance_one_sample()
            .map_err(error)?;
        let medium = d.phy.as_ref().unwrap().get(index).unwrap();
        let next = medium.next_sample_index();
        let at = medium.next_event_ps().map_err(error)?;
        let mut wakes = self.phy_wakes.clone();
        Self::schedule(
            c,
            d,
            at,
            Event::PhySample {
                attachment: index,
                sample_index: next,
            },
        )?;
        wakes[index] = Some(next);
        d.phy_wakes = Some(wakes);
        self.phy_effects(index, sample_index, effects, d, c)
    }

    pub(super) fn phy_effects(
        &self,
        index: usize,
        sample_index: u64,
        effects: Vec<AttachmentEffect>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        for (ordinal, effect) in effects.into_iter().enumerate() {
            if self.wire_phy_effect(index, &effect, d, c)? {
                if let AttachmentEffect::Physical { effect } = effect {
                    Self::record(
                        c,
                        d,
                        "dir.network.phy",
                        format!("phy:{index}:{sample_index}:{ordinal}"),
                        decimal(serde_json::to_value(effect).map_err(|e| error(e.to_string()))?),
                    )?;
                }
                continue;
            }
            match effect {
                AttachmentEffect::Physical { effect } => {
                    self.merge_phy_effect(index, &effect, d, c)?;
                    self.phy_service_effect(&effect, d, c)?;
                    if let PhyEffect::TxStarted {
                        origin,
                        sample_index,
                        ..
                    } = &effect
                    {
                        let t = self.transfer_by_id(&origin.frame_id)?;
                        let reference = d.phy.as_ref().unwrap_or(&self.phy)[index]
                            .sample_time_ps(*sample_index)
                            .map_err(error)?;
                        self.gptp_tx_start(t, None, u64::from(origin.attempt), reference, d, c)?;
                        let mut transfer = self.transfer(d, t).clone();
                        if transfer.row.sof_ps.is_none() {
                            let observed = if self.prepared.phy_attachments[index]
                                .config
                                .physical_ports
                                .is_empty()
                            {
                                c.now()
                            } else {
                                d.phy.as_ref().unwrap()[index]
                                    .sample_time_ps(*sample_index)
                                    .map_err(error)?
                            };
                            transfer.row.sof_ps = Some(observed);
                            transfer.row.status = "transmitting".into();
                            transfer.row.time_ps = c.now();
                            put(&mut d.transfers, t, transfer);
                            if self.preemption.is_some() && !self.gptp_packet(d, t) {
                                self.preemption_sof(t, d, observed)?;
                            }
                            self.replication_sof(t, d, observed)?;
                            self.qcc_sof(t, d, c)?;
                        }
                    }
                    if let PhyEffect::TxMacSfd {
                        origin,
                        sample_index,
                        ..
                    } = &effect
                    {
                        let t = self.transfer_by_id(&origin.frame_id)?;
                        self.gptp_sampled_capture(
                            t,
                            gptp::state::CaptureKind::Tx,
                            index,
                            *sample_index,
                            origin.attempt,
                            None,
                            gptp::state::ReferencePlane::MdiMacSfd,
                            d,
                            c,
                        )?;
                    }
                    if let PhyEffect::TxMacFcsEnd {
                        origin,
                        sample_index,
                        ..
                    } = &effect
                    {
                        if !self.prepared.phy_attachments[index]
                            .config
                            .physical_ports
                            .is_empty()
                        {
                            let t = self.transfer_by_id(&origin.frame_id)?;
                            let mut transfer = self.transfer(d, t).clone();
                            // MAC EOF is the actual end of FCS. The PCS ESD,
                            // electrical silence and MAC release occur later.
                            transfer.row.eof_ps = Some(
                                d.phy.as_ref().unwrap()[index]
                                    .sample_time_ps(*sample_index)
                                    .map_err(error)?,
                            );
                            transfer.row.time_ps = c.now();
                            put(&mut d.transfers, t, transfer);
                            self.gptp_tx_eof(t, None, d, c)?;
                        }
                    }
                    Self::record(
                        c,
                        d,
                        "dir.network.phy",
                        format!("phy:{index}:{sample_index}:{ordinal}"),
                        decimal(serde_json::to_value(effect).map_err(|e| error(e.to_string()))?),
                    )?;
                }
                AttachmentEffect::TxSerialized {
                    transfer_id,
                    success,
                    reason,
                    ..
                } => {
                    let t = self.transfer_by_id(&transfer_id)?;
                    self.pause_mac_completed(self.transfer(d, t).direction, d, c)?;
                    let mut transfer = self.transfer(d, t).clone();
                    if self.prepared.phy_attachments[index]
                        .config
                        .physical_ports
                        .is_empty()
                    {
                        transfer.row.eof_ps = success.then_some(c.now());
                    } else if !success {
                        transfer.row.eof_ps = None;
                    } else if transfer.row.eof_ps.is_none() {
                        return Err(error(
                            "named PHY retired a successful frame without actual MAC FCS end",
                        ));
                    }
                    transfer.row.status = if success { "serialized" } else { "dropped" }.into();
                    transfer.row.drop_reason = reason;
                    transfer.row.time_ps = c.now();
                    put(&mut d.transfers, t, transfer);
                }
                AttachmentEffect::TxReleased { transfer_id, .. } => {
                    let t = self.transfer_by_id(&transfer_id)?;
                    let mut transfer = self.transfer(d, t).clone();
                    transfer.row.release_ps = Some(c.now());
                    transfer.row.time_ps = c.now();
                    let queue = self.queue_mut(d, transfer.direction);
                    if queue.active != Some(t) {
                        return Err(error("sampled PHY release lacks source reservation"));
                    }
                    queue.active = None;
                    self.phy_service_release(transfer.direction, d, c)?;
                    put(&mut d.transfers, t, transfer);
                    c.request_arbitration("network").map_err(model)?;
                }
                AttachmentEffect::Received {
                    transfer_id,
                    direction_id,
                    to_port,
                    frame,
                    ..
                } => {
                    let t = self.transfer_by_id(&transfer_id)?;
                    let mut transfer = self.transfer(d, t).clone();
                    let direction = &self.prepared.ethernet.directions[transfer.direction];
                    if direction.from_port != direction_id
                        || direction.to_port != to_port
                        || transfer.row.wire.mac_hex != frame.wire.mac_hex
                    {
                        transfer.row.status = "dropped".into();
                        transfer.row.drop_reason = Some("phy:lineage_or_mac_byte_mismatch".into());
                        transfer.row.time_ps = c.now();
                        put(&mut d.transfers, t, transfer);
                        self.replication_drop(t, d, c.now(), "phy_byte_mismatch")?;
                        Self::record(
                            c,
                            d,
                            "dir.network.phy",
                            format!("phy:reject:{transfer_id}"),
                            json!({"kind":"rejected_receive","transfer_id":transfer_id,
                                "reason":"lineage_or_mac_byte_mismatch"}),
                        )?;
                        continue;
                    }
                    transfer.row.wire = frame.wire;
                    put(&mut d.transfers, t, transfer);
                    let arrivals = d
                        .phy_arrivals
                        .get_or_insert_with(|| self.phy_arrivals.clone());
                    if arrivals.insert(t, c.now()).is_some() {
                        return Err(error("duplicate sampled PHY arrival"));
                    }
                    Self::schedule(c, d, c.now(), Event::Arrival(t))?;
                }
                AttachmentEffect::PhysicalIngress {
                    admitted_transfer_id,
                    physical_attempt,
                    source_endpoint,
                    receiver_endpoint,
                    receiver_tx_port,
                    receiver_rx_port,
                    reception_index,
                    frame,
                    origins,
                    rx_sfd_sample,
                    rx_fcs_sample,
                    rx_mapped_sfd_sample,
                    rx_mapped_fcs_sample: _,
                } => {
                    let t = self.transfer_by_id(&admitted_transfer_id)?;
                    let transfer = self.transfer(d, t).clone();
                    let receiver = self
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|direction| direction.from_port == receiver_tx_port)
                        .ok_or_else(|| {
                            error("physical ingress receiver has no owned MAC output")
                        })?;
                    let physical = self
                        .phy_receiver_port(transfer.direction, receiver)
                        .ok_or_else(|| error("physical ingress crossed an electrical domain"))?;
                    let cfg = &self.prepared.phy_attachments[index].config;
                    let source = cfg
                        .physical_ports
                        .iter()
                        .find(|p| p.endpoint == source_endpoint)
                        .ok_or_else(|| error("physical ingress source endpoint is not bound"))?;
                    if self.phy_admitted.get(&t) != Some(&index)
                        || physical.endpoint != receiver_endpoint
                        || physical.rx_port != receiver_rx_port
                        || source.tx_port
                            != self.prepared.ethernet.directions[transfer.direction].from_port
                        || physical_attempt.frame_id != admitted_transfer_id
                        || physical_attempt.source_endpoint != source_endpoint
                        || origins.as_slice() != std::slice::from_ref(&physical_attempt)
                        || rx_sfd_sample > rx_fcs_sample
                    {
                        return Err(error(
                            "physical ingress identity, sample order or attempt provenance differs",
                        ));
                    }
                    // Actual decoded bytes are the ingress, including padding.
                    // The source identity never substitutes for FCS decoding.
                    if frame.wire.mac_hex != transfer.row.wire.mac_hex {
                        Self::record(
                            c,
                            d,
                            "dir.network.phy",
                            format!("phy:unmatched:{t}:{reception_index}"),
                            json!({"kind":"unmatched_decoded_ingress","transfer_id":admitted_transfer_id,
                                "receiver":receiver_rx_port,"mac_hex":frame.wire.mac_hex}),
                        )?;
                        continue;
                    }
                    let mut received = transfer;
                    received.row.wire = frame.wire;
                    put(&mut d.transfers, t, received);
                    Self::record(
                        c,
                        d,
                        "dir.network.phy",
                        format!("phy:ingress:{t}:{reception_index}"),
                        json!({"kind":"decoded_ingress","attachment":index.to_string(),
                            "transfer_id":admitted_transfer_id,"physical_attempt":physical_attempt,
                            "receiver_endpoint":receiver_endpoint,"receiver_port":receiver_rx_port,
                            "rx_sfd_sample":rx_sfd_sample.to_string(),"rx_fcs_sample":rx_fcs_sample.to_string()}),
                    )?;
                    if self.gptp_packet(d, t) {
                        self.gptp_sampled_capture(
                            t,
                            gptp::state::CaptureKind::Rx,
                            index,
                            rx_mapped_sfd_sample.ok_or_else(|| {
                                error("gPTP RX lacks a source-certified MDI marker")
                            })?,
                            physical_attempt.attempt,
                            None,
                            gptp::state::ReferencePlane::MdiMacSfd,
                            d,
                            c,
                        )?;
                        let wire = &self.transfer(d, t).row.wire;
                        let bytes = crate::runtime::ethernet::phy::attachment::canonical_bytes(
                            wire,
                            self.prepared.phy_attachments[index]
                                .config
                                .medium
                                .maximum_mac_bytes,
                        )
                        .map_err(error)?;
                        self.gptp_phy_rx_validated(t, physical_attempt.attempt, &bytes, d, c)?;
                    } else {
                        let id = format!(
                            "{admitted_transfer_id}@phy{reception_index}@{receiver_rx_port}"
                        );
                        self.arrive_with_id(t, Some(receiver), Some(id), d, c)?;
                    }
                }
                AttachmentEffect::Failed {
                    transfer_id,
                    reason,
                    ..
                } => {
                    let t = self.transfer_by_id(&transfer_id)?;
                    let mut transfer = self.transfer(d, t).clone();
                    if transfer.row.arrival_ps.is_none() {
                        transfer.row.status = "dropped".into();
                        transfer.row.drop_reason = Some(format!("phy:{reason}"));
                        transfer.row.time_ps = c.now();
                        put(&mut d.transfers, t, transfer);
                        self.replication_drop(t, d, c.now(), &reason)?;
                    }
                }
                AttachmentEffect::UnattributedReceive {
                    to_port, reason, ..
                } => {
                    Self::record(
                        c,
                        d,
                        "dir.network.phy",
                        format!("phy:unattributed:{index}:{sample_index}:{ordinal}"),
                        json!({"kind":"unattributed_receive","to_port":to_port,"reason":reason}),
                    )?;
                }
                AttachmentEffect::DecodeRejected {
                    to_port, reason, ..
                } => {
                    Self::record(
                        c,
                        d,
                        "dir.network.phy",
                        format!("phy:decode-rejected:{index}:{sample_index}:{ordinal}"),
                        json!({"kind":"decode_rejected","to_port":to_port,"reason":reason}),
                    )?;
                }
            }
        }
        self.gptp_phy_readiness_changed(index, d, c)?;
        self.qcc_refresh_raw_sources(d, c)?;
        Ok(())
    }
}
