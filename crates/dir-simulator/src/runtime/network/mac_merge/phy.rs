//! Clause 99 binds to the causal raw GMII producer, never canonical MacTx.
use super::super::*;
use super::integration::{MergeEvent, Origin, Packet};
use crate::runtime::ethernet::phy::{
    attachment::AttachmentEffect,
    medium::{PhyEffect, RawMPacketTx},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBinding {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sample: Option<u64>,
    pub attachment: usize,
    pub generation: u64,
    pub start_slot: u64,
    pub source_endpoint: String,
    pub origin_frame_id: String,
    pub coded_end: bool,
    #[serde(default)]
    pub tx_delimiter_sample: Option<u64>,
    #[serde(default)]
    pub receive_cut_sample: Option<u64>,
    #[serde(default)]
    pub decoded_at_sample: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter_slot: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_mdi_delimiter_sample: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_mdi_delimiter_sample: Option<u64>,
}
/// Source-prefix-derived proof captured before the first mutation at notification N.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PermissionProof {
    pub at: u64,
    pub epoch: u64,
    pub identity: serde_json::Value,
    pub floor_credit: Option<u8>,
}
impl NetworkState {
    pub(in crate::runtime::network) fn merge_permission_changed(
        &self,
        i: usize,
        d: &mut NetworkDelta,
    ) -> Result<()> {
        if !self.merge_raw_transport(i) {
            return Ok(());
        }
        let p = &mut self.merge_mut(d)?.ports[i];
        p.permission_epoch = add(p.permission_epoch, 1)?;
        Ok(())
    }
    fn merge_permission_identity(
        &self,
        i: usize,
        d: &NetworkDelta,
        at: u64,
    ) -> Result<serde_json::Value> {
        let p = &self.merge_state(d).unwrap().ports[i];
        let name = &self.prepared.ethernet.directions[i].from_port;
        let schedule = self
            .tsn
            .as_ref()
            .map(|s| s.merge_schedule(&d.tsn, name).map(|s| s.0))
            .transpose()?;
        let pause = self.pause_port(i, d).map(|p| {
            json!([
                p.generation,
                p.epoch,
                p.rx_enabled,
                p.tx_enabled,
                p.transmit_enabled,
                p.link_up,
                p.timer
                    .as_ref()
                    .map(|t| (t.generation, t.loaded_ps, t.expires_ps))
            ])
        });
        let policy=self.policy(d).map(|p|json!({"role":p.roles.get(name),"link":p.link_up.get(name),"vlans":p.effective_vlans.get(name),"trees":p.tree_roles.get(name)}));
        let view = self.merge_accounting_view(i, d, at)?;
        let index = self.merge_phy_attachment(i).unwrap();
        let medium = &d.phy.as_ref().unwrap_or(&self.phy)[index];
        Ok(
            json!({"policy":policy,"clock":self.clock_for_port(d,name).map(clock::NodeClock::generation),"schedule":schedule,"pause":pause,"epoch":p.tx.epoch,"busy":p.busy,"hold":p.tx.hold,"tas":p.applied_tas,"verify":[p.verify.control_generation],"active":p.verify.p_active(),"send_v":p.verify.send_v,"send_r":p.verify.send_r,"lldp":p.lldp_pending,"segment":view.segment,"class":view.transmit_class,"link":p.link_up,"bilateral":medium.gigabit_bilateral_link_ready().map_err(error)?}),
        )
    }
    pub(in crate::runtime::network) fn merge_capture_fractional_permissions(
        &self,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        let Some(state) = self.merge_state(d) else {
            return Ok(());
        };
        let directions = (0..state.ports.len())
            .filter(|i| {
                self.merge_raw_transport(*i)
                    && state.ports[*i]
                        .permission
                        .as_ref()
                        .is_none_or(|p| p.at != now)
            })
            .collect::<Vec<_>>();
        for i in directions {
            let name = &self.prepared.ethernet.directions[i].from_port;
            let mask = match (now.checked_sub(1), &self.tsn) {
                (Some(f), Some(tsn)) => tsn.credit_nonnegative_forward_clock_in(
                    &d.tsn,
                    name,
                    f,
                    self.clock_for_port(d, name),
                )?,
                (Some(_), None) => Some(255),
                _ => None,
            };
            let proof = PermissionProof {
                at: now,
                epoch: self.merge_state(d).unwrap().ports[i].permission_epoch,
                identity: self.merge_permission_identity(i, d, now.saturating_sub(1))?,
                floor_credit: mask,
            };
            self.merge_mut(d)?.ports[i].permission = Some(proof);
        }
        Ok(())
    }
    pub(super) fn merge_fractional_permission(
        &self,
        i: usize,
        origin: Origin,
        wire: Option<&str>,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        let Some(index) = self.merge_phy_attachment(i) else {
            return Ok(true);
        };
        let medium = &d.phy.as_ref().unwrap_or(&self.phy)[index];
        let sample = medium.next_sample_index();
        let time = medium
            .config()
            .medium
            .channel
            .grid
            .time(sample)
            .map_err(error)?;
        let numerator =
            u128::from(medium.config().origin_ps) * time.denominator + time.numerator_ps;
        if u128::from(now) * time.denominator == numerator {
            return Ok(true);
        }
        if u128::from(now) != numerator.div_ceil(time.denominator) {
            return Ok(false);
        }
        let Some(f) = now.checked_sub(1) else {
            return Ok(false);
        };
        let p = &self.merge_state(d).unwrap().ports[i];
        let Some(proof) = &p.permission else {
            return Ok(false);
        };
        // Check semantic generations BEFORE projecting a possibly reanchored clock.
        if proof.at != now || proof.epoch != p.permission_epoch {
            return Ok(false);
        }
        if proof.identity != self.merge_permission_identity(i, d, now)? {
            return Ok(false);
        }
        let (availability, class, occupancy, credit) = match origin {
            Origin::WireControl => {
                let Some(id) = wire else { return Ok(false) };
                let tx = &self.wire_host(d).unwrap().tx[id];
                let Some(at) = tx.queued_ps else {
                    return Ok(false);
                };
                (
                    at,
                    (!tx.is_mac_control()).then(|| self.wire_priority(i, id, d)),
                    super::bit_duration(
                        (tx.intent.bytes.len() as u64 + 20) * 8,
                        self.prepared.ethernet.directions[i].bitrate_bps,
                    )
                    .map_err(super::integration::err)?,
                    !tx.is_mac_control(),
                )
            }
            Origin::Express(t) | Origin::Pmac(t) => {
                let row = self.transfer(d, t);
                let continuation = matches!(origin, Origin::Pmac(_)) && row.row.sof_ps.is_some();
                (
                    row.row.queued_ps,
                    Some(row.row.priority),
                    self.merge_retained_head(t, d)?.occupancy_ps,
                    !continuation,
                )
            }
            Origin::Lldp => (
                0,
                Some(self.merge_lldp_priority(i)),
                self.merge_lldp_head(i, d)?.occupancy_ps,
                true,
            ),
            Origin::Control(_) => (0, None, 0, false),
        };
        if u128::from(availability) * time.denominator > numerator {
            return Ok(false);
        }
        let mac_control = wire.is_some_and(|id| self.wire_host(d).unwrap().tx[id].is_mac_control());
        let retained_request =
            matches!(origin,Origin::Pmac(t) if self.transfer(d,t).row.sof_ps.is_some());
        let internal_control = matches!(origin, Origin::Control(_));
        if !retained_request
            && !internal_control
            && (!self.output_request_allowed(i, mac_control, d, f)
                || !self.output_request_allowed(i, mac_control, d, now))
        {
            return Ok(false);
        }
        if let (Some(class), Some(tsn)) = (class, &self.tsn) {
            let name = &self.prepared.ethernet.directions[i].from_port;
            if credit
                && (proof.floor_credit.is_none_or(|m| m & (1 << class) == 0)
                    || tsn
                        .retained_start_permission_clock_in(
                            &d.tsn,
                            name,
                            class,
                            now,
                            occupancy,
                            self.clock_for_port(d, name),
                        )?
                        .is_none())
            {
                return Ok(false);
            }
            // Legacy started pMAC retains its shipped continuation rule.
            let continuation =
                matches!(origin,Origin::Pmac(t) if self.transfer(d,t).row.sof_ps.is_some());
            let managed = self.prepared.bindings.as_ref().is_some_and(|b| {
                b.ports[i].schedule_writer
                    == Some(crate::types::network::bindings::ScheduleWriter::Qcc)
            });
            if (!continuation || managed)
                && (!tsn.retained_gate_fit_clock_in(
                    &d.tsn,
                    name,
                    class,
                    f,
                    occupancy,
                    self.clock_for_port(d, name),
                )? || !tsn.retained_gate_fit_clock_in(
                    &d.tsn,
                    name,
                    class,
                    now,
                    occupancy,
                    self.clock_for_port(d, name),
                )?)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(in crate::runtime::network) fn merge_raw_transport(&self, direction: usize) -> bool {
        self.merge_direction(direction) && self.merge_phy_attachment(direction).is_some()
    }
    pub(super) fn merge_phy_attachment(&self, direction: usize) -> Option<usize> {
        self.phy_attachment_for(direction).filter(|index| {
            self.prepared.phy_attachments[*index]
                .config
                .medium
                .gigabit_raw_merge
        })
    }
    pub(super) fn merge_phy_ready(
        &self,
        direction: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        let Some(index) = self.merge_phy_attachment(direction) else {
            return Ok(true);
        };
        let medium = &d.phy.as_ref().unwrap_or(&self.phy)[index];
        let status = medium
            .raw_ingress_status(&self.prepared.ethernet.directions[direction].from_port)
            .map_err(error)?;
        let sample = medium.next_sample_index();
        let time = medium
            .config()
            .medium
            .channel
            .grid
            .time(sample)
            .map_err(error)?;
        let local = now.checked_sub(medium.config().origin_ps);
        Ok(medium.gigabit_bilateral_link_ready().map_err(error)?
            && status.ready
            && status.next_cut_sample == sample
            && local
                .is_some_and(|ps| u128::from(ps) == time.numerator_ps.div_ceil(time.denominator)))
    }
    /// Forecast only the next real GMII admission cut when actual demand exists.
    pub(in crate::runtime::network) fn merge_phy_next_cut(
        &self,
        index: usize,
        d: &NetworkDelta,
    ) -> Result<Option<u64>> {
        let Some(state) = self.merge_state(d) else {
            return Ok(None);
        };
        if !self.prepared.phy_attachments[index]
            .config
            .medium
            .gigabit_raw_merge
        {
            return Ok(None);
        }
        let medium = &d.phy.as_ref().unwrap_or(&self.phy)[index];
        if !medium.gigabit_bilateral_link_ready().map_err(error)? {
            return Ok(None);
        }
        let mut next = None;
        for &i in &self.prepared.phy_attachments[index].directions {
            let p = &state.ports[i];
            if !state.identity.participating[i] || !p.link_up || p.busy.is_some() {
                continue;
            }
            let ready = self.tsn.as_ref().map_or(255, |_| {
                d.tsn
                    .ready
                    .get(&self.prepared.ethernet.directions[i].from_port)
                    .copied()
                    .unwrap_or(0)
            });
            let permission_at = self
                .tsn
                .as_ref()
                .map(|tsn| {
                    tsn.merge_staged_time(&d.tsn, &self.prepared.ethernet.directions[i].from_port)
                })
                .transpose()?
                .unwrap_or(p.accounting_at);
            let allowed = self.output_request_allowed(i, false, d, permission_at);
            let express_mask = state.identity.ports[i]
                .as_ref()
                .map_or(255, |p| p.express_mask);
            let preemptable_mask = state.identity.ports[i]
                .as_ref()
                .map_or(0, |p| p.preemptable_mask);
            let mut queued = false;
            if allowed {
                for (class, q) in self.queue(d, i).waiting.iter().enumerate() {
                    let eligible = ready & (1 << class) != 0
                        && ((express_mask & (1 << class) != 0 && p.selected_emac.is_none())
                            || (preemptable_mask & (1 << class) != 0
                                && p.selected_pmac.is_none()
                                && !p.tx.hold));
                    if let Some(t) = q.front().filter(|_| eligible) {
                        if self.merge_fresh_permission(i, *t, d, permission_at)? {
                            queued = true;
                            break;
                        }
                    }
                }
            }
            let retained = if let Some(t) = p.selected_pmac {
                !p.tx.hold && self.merge_resume_gate(i, t, d, permission_at)?
            } else {
                false
            };
            let express = if let Some(t) = p.selected_emac {
                allowed && self.merge_fresh_permission(i, t, d, permission_at)?
            } else {
                false
            };
            let wire = self.wire_candidate(i, d, permission_at)?.is_some();
            let lldp = p.lldp_pending
                && state.identity.ports[i].is_some()
                && self.merge_lldp_permission(i, d, permission_at)?;
            if !(wire
                || queued
                || retained
                || express
                || lldp
                || p.verify.send_r
                || p.verify.send_v)
            {
                continue;
            }
            let status = medium
                .raw_ingress_status(&self.prepared.ethernet.directions[i].from_port)
                .map_err(error)?;
            if status.next_cut_sample != u64::MAX {
                // A callback may occur after an unconsumed physical cut but
                // before the next sampled boundary. Advance one real sample
                // before forecasting its following GMII cut; never admit late.
                let cut = status
                    .next_cut_sample
                    .max(add(medium.next_sample_index(), 1)?);
                next = Some(next.map_or(cut, |old: u64| old.min(cut)));
            }
        }
        Ok(next)
    }
    pub(super) fn merge_phy_admit(
        &self,
        p: &mut Packet,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let index = self
            .merge_phy_attachment(p.direction)
            .ok_or_else(|| error("raw Merge physical owner missing"))?;
        let cfg = &self.prepared.phy_attachments[index].config;
        let from = self.prepared.ethernet.directions[p.direction]
            .from_port
            .clone();
        let endpoint = cfg
            .physical_ports
            .iter()
            .find(|port| port.tx_port == from)
            .ok_or_else(|| error("raw Merge endpoint missing"))?
            .endpoint
            .clone();
        let status = d.phy.as_ref().unwrap_or(&self.phy)[index]
            .raw_ingress_status(&from)
            .map_err(error)?;
        let origin_frame_id = match p.origin {
            Origin::Express(t) | Origin::Pmac(t) => self.transfer(d, t).row.transfer_id.clone(),
            Origin::WireControl => p
                .wire_control_id
                .clone()
                .ok_or_else(|| error("raw Wire source missing"))?,
            _ => format!("merge:{}:{}", p.direction, p.id),
        };
        let generation = 0;
        let start_sample = d.phy.as_ref().unwrap_or(&self.phy)[index].next_sample_index();
        let effect = self
            .phy_mut(d, index)?
            .admit_raw_mpacket(
                &from,
                RawMPacketTx {
                    reference_sample: Some(start_sample),
                    packet_id: p.id,
                    generation,
                    source_endpoint: endpoint.clone(),
                    at_ps: c.now(),
                    xmii_bytes: p.bytes.clone(),
                    origin_frame_id: origin_frame_id.clone(),
                },
            )
            .map_err(error)?;
        if !matches!(effect, AttachmentEffect::Physical {effect:PhyEffect::XmiStarted {packet_id,sample_index,generation:actual_generation,endpoint:ref actual_endpoint}} if packet_id == p.id && sample_index == start_sample && actual_generation == generation && actual_endpoint == &endpoint)
        {
            return Err(error(
                "raw Merge admission did not produce actual xMII start",
            ));
        }
        if let AttachmentEffect::Physical { effect } = effect {
            Self::record(
                c,
                d,
                "dir.network.phy",
                format!("merge-phy:admit:{}", p.id),
                decimal(serde_json::to_value(effect).map_err(|e| error(e.to_string()))?),
            )?;
        }
        p.raw = Some(RawBinding {
            start_sample: Some(start_sample),
            attachment: index,
            generation,
            start_slot: status.consumed_gmii_slots,
            source_endpoint: endpoint,
            origin_frame_id,
            coded_end: false,
            tx_delimiter_sample: None,
            receive_cut_sample: None,
            decoded_at_sample: None,
            delimiter_slot: None,
            delimiter_generation: None,
            tx_mdi_delimiter_sample: None,
            rx_mdi_delimiter_sample: None,
        });
        p.received = None;
        self.phy_plan_boundary(index, d, c)?;
        Ok(())
    }
    pub(super) fn merge_phy_replace(
        &self,
        p: &mut Packet,
        d: &mut NetworkDelta,
        _now: u64,
    ) -> Result<()> {
        if self.merge_state(d).unwrap().packets[&p.id].bytes == p.bytes {
            // A demand with no legal remaining cut does not rewrite a packet
            // whose last GMII octet may already have been consumed.
            return Ok(());
        }
        let raw = p
            .raw
            .as_mut()
            .ok_or_else(|| error("raw Merge replacement missing binding"))?;
        let from = self.prepared.ethernet.directions[p.direction]
            .from_port
            .clone();
        let old = &self.merge_state(d).unwrap().packets[&p.id].bytes;
        let prefix = old.iter().zip(&p.bytes).take_while(|(a, b)| a == b).count();
        let next = add(raw.generation, 1)?;
        self.phy_mut(d, raw.attachment)?
            .replace_unconsumed_raw_suffix(
                &from,
                p.id,
                raw.generation,
                prefix,
                &p.bytes[prefix..],
                next,
            )
            .map_err(error)?;
        raw.generation = next;
        p.received = None;
        Ok(())
    }
    fn merge_phy_fault(
        &self,
        index: usize,
        endpoint: &str,
        reason: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self
            .merge_state(d)
            .is_none_or(|s| s.phy_faulted.contains(&index))
        {
            return Ok(());
        }
        let Some(port) = self.prepared.phy_attachments[index]
            .config
            .physical_ports
            .iter()
            .find(|p| p.endpoint == endpoint)
        else {
            return Err(error("raw fault endpoint not bound"));
        };
        let direction = self
            .prepared
            .ethernet
            .directions
            .iter()
            .position(|d| d.from_port == port.tx_port)
            .ok_or_else(|| error("raw fault direction missing"))?;
        if !self.merge_direction(direction) {
            return Ok(());
        }
        self.merge_mut(d)?.phy_fault_at.insert(index, c.now());
        self.merge_mut(d)?.phy_faulted.insert(index);
        self.merge_mut(d)?.phy_ready.remove(&index);
        let reverse = self.merge_state(d).unwrap().identity.reverse[direction];
        let directions = [direction, reverse];
        let epoch = add(
            directions
                .iter()
                .map(|i| self.merge_state(d).unwrap().ports[*i].tx.epoch)
                .max()
                .unwrap(),
            1,
        )?;
        // A failed point-to-point PHY epoch invalidates both MAC ends;
        // otherwise the peer's next control packet would carry a stale
        // Merge epoch and recovery could never verify the link again.
        for i in directions {
            let old = self.merge_state(d).unwrap().ports[i].clone();
            let ids = self
                .merge_state(d)
                .unwrap()
                .packets
                .values()
                .filter(|p| p.direction == i)
                .map(|p| p.id)
                .collect::<std::collections::BTreeSet<_>>();
            let mut affected = old
                .selected_pmac
                .into_iter()
                .chain(old.selected_emac)
                .collect::<std::collections::BTreeSet<_>>();
            for id in &ids {
                if let Some(wire_id) = self.merge_state(d).unwrap().packets[id]
                    .wire_control_id
                    .clone()
                {
                    self.wire_owner_reset(i, &wire_id, reason, d, c)?;
                }
                let packet = self.merge_mut(d)?.packets.get_mut(id).unwrap();
                packet.invalidated_ps = Some(c.now());
                if !packet.eof_done {
                    packet.aborted_ps = Some(c.now());
                }
                if let Origin::Express(t) | Origin::Pmac(t) = packet.origin {
                    affected.insert(t);
                }
                self.merge_packet_record(d, *id, "actual_phy_link_reset")?;
            }
            self.merge_mut(d)?.packets.retain(|_, p| p.direction != i);
            let configured = self.merge_state(d).unwrap().identity.ports[i].is_some();
            let port = &mut self.merge_mut(d)?.ports[i];
            port.tx.reset(epoch);
            port.rx.reset(epoch);
            port.assembly_packets.clear();
            port.verify
                .link_reset(epoch, old.link_up)
                .map_err(super::integration::err)?;
            port.selected_pmac = None;
            port.selected_emac = None;
            port.busy = None;
            port.capability_expires = None;
            port.capability_packet = None;
            port.last_response_packet = None;
            port.last_verify_packet = None;
            port.verify_timer_commit = false;
            port.lldp_pending = old.link_up && configured;
            port.tas_control = None;
            port.applied_tas = None;
            port.tas_generation = 0;
            port.tas_planned_at = c.now();
            port.accounting = None;
            self.queue_mut(d, i).active = None;
            self.due_mut(d).retain(|due|!matches!(&due.kind,Completion::MacMerge(MergeEvent::Eof(id)|MergeEvent::Release(id)|MergeEvent::Smd(id)|MergeEvent::Accounting(id)) if ids.contains(id)) && !matches!(&due.kind,Completion::MacMerge(MergeEvent::Verify{direction,..}|MergeEvent::Expiry{direction,..}|MergeEvent::Tas{direction,..}) if *direction==i));
            for t in affected {
                self.merge_drop_transfer(t, d, c.now(), reason)?;
            }
            self.qcc_protocol_changed(i, d, c)?;
            self.merge_shaper_transition(i, d, c.now())?;
            self.merge_port_record(d, i, c.now(), "actual_phy_link_reset")?;
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn merge_phy_effect(
        &self,
        index: usize,
        effect: &PhyEffect,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self.prepared.phy_attachments[index]
            .config
            .medium
            .gigabit_raw_merge
        {
            if let PhyEffect::TxReady {
                endpoint, ready, ..
            } = effect
            {
                if *ready {
                    let expected = self.prepared.phy_attachments[index]
                        .config
                        .physical_ports
                        .len();
                    let state = self.merge_mut(d)?;
                    let ports = state.phy_ready.entry(index).or_default();
                    ports.insert(endpoint.clone());
                    if ports.len() == expected {
                        state.phy_faulted.remove(&index);
                    }
                    c.request_arbitration("network").map_err(model)?;
                } else {
                    return self.merge_phy_fault(
                        index,
                        endpoint,
                        "physical_readiness_withdrawn",
                        d,
                        c,
                    );
                }
            }
            if let PhyEffect::PcsControl {
                endpoint, command, ..
            } = effect
            {
                if let Some(reason) = command.strip_prefix("link_fault:") {
                    return self.merge_phy_fault(index, endpoint, reason, d, c);
                }
                if command == "link_retraining" || command.starts_with("link_retraining:") {
                    return self.merge_phy_fault(index, endpoint, command, d, c);
                }
            }
        }
        let id = match effect {
            PhyEffect::XmiStarted { packet_id, .. }
            | PhyEffect::XmiDelimiter { packet_id, .. }
            | PhyEffect::XmiDelimiterConsumed { packet_id, .. }
            | PhyEffect::MdiDelimiterEnd { packet_id, .. }
            | PhyEffect::XmiEnd { packet_id, .. }
            | PhyEffect::XmiReleased { packet_id, .. }
            | PhyEffect::XmiAborted { packet_id, .. }
            | PhyEffect::MdiCodedEnd { packet_id, .. }
            | PhyEffect::RawMPacketIngress { packet_id, .. } => Some(*packet_id),
            PhyEffect::RawPacketRejected {
                packet_ids, reason, ..
            } => {
                for id in packet_ids {
                    if let Some(p) = self.merge_state(d).and_then(|s| s.packets.get(id)).cloned() {
                        if p.raw.as_ref().is_none_or(|raw| raw.attachment != index) {
                            return Err(error("raw rejection crossed physical owner"));
                        }
                        self.merge_mut(d)?.packets.get_mut(id).unwrap().arrival_done = true;
                        let reverse = self.merge_state(d).unwrap().identity.reverse[p.direction];
                        let receiver = &mut self.merge_mut(d)?.ports[reverse];
                        receiver.rx.reset(p.epoch);
                        receiver.assembly_packets.clear();
                        if let Origin::Express(t) | Origin::Pmac(t) = p.origin {
                            self.merge_drop_transfer(t, d, c.now(), reason)?;
                        }
                        if let Some(wire_id) = &p.wire_control_id {
                            self.wire_owner_fail(p.direction, wire_id, reason, d, c)?;
                        }
                        self.merge_packet_record(d, *id, "actual_phy_reject")?;
                    }
                }
                return Ok(());
            }
            _ => None,
        };
        let Some(id) = id else {
            return Ok(());
        };
        let Some(p) = self
            .merge_state(d)
            .and_then(|s| s.packets.get(&id))
            .cloned()
        else {
            // A link epoch reset retires old packets while emitted coded tails drain.
            return Ok(());
        };
        let raw = p
            .raw
            .as_ref()
            .ok_or_else(|| error("raw PHY event has ideal Merge packet"))?;
        if raw.attachment != index {
            return Err(error("raw PHY event crossed attachment"));
        }
        match effect {
            PhyEffect::XmiStarted { .. } => {}
            PhyEffect::XmiDelimiter {
                endpoint,
                generation,
                ..
            }
            | PhyEffect::XmiEnd {
                endpoint,
                generation,
                ..
            }
            | PhyEffect::XmiReleased {
                endpoint,
                generation,
                ..
            }
            | PhyEffect::MdiCodedEnd {
                endpoint,
                generation,
                ..
            }
            | PhyEffect::XmiAborted {
                endpoint,
                generation,
                ..
            } if endpoint != &raw.source_endpoint || *generation != raw.generation => {
                return Err(error("raw PHY event identity/generation differs"));
            }
            PhyEffect::XmiDelimiterConsumed {
                endpoint,
                generation,
                octet_index,
                value,
                consumed_cut_sample,
                ..
            } => {
                if endpoint != &raw.source_endpoint
                    || *generation != raw.generation
                    || p.bytes.get(*octet_index) != Some(value)
                    || raw.tx_delimiter_sample.is_some()
                {
                    return Err(error(
                        "actual consumed raw delimiter differs from source packet",
                    ));
                }
                let stored = self
                    .merge_mut(d)?
                    .packets
                    .get_mut(&id)
                    .unwrap()
                    .raw
                    .as_mut()
                    .unwrap();
                stored.tx_delimiter_sample = Some(*consumed_cut_sample);
                stored.delimiter_slot = Some(add(raw.start_slot, *octet_index as u64)?);
                stored.delimiter_generation = Some(*generation);
                self.merge_packet_record(d, id, "actual_xmii_delimiter_consumed")?;
            }
            PhyEffect::MdiDelimiterEnd {
                source_endpoint,
                generation,
                delimiter_slot,
                sample_index,
                ..
            } => {
                if source_endpoint != &raw.source_endpoint
                    || raw.delimiter_generation != Some(*generation)
                    || raw.delimiter_slot != Some(*delimiter_slot)
                    || raw.tx_mdi_delimiter_sample.is_some()
                {
                    return Err(error(
                        "mapped MDI delimiter differs from consumed source prefix",
                    ));
                }
                self.merge_mut(d)?
                    .packets
                    .get_mut(&id)
                    .unwrap()
                    .raw
                    .as_mut()
                    .unwrap()
                    .tx_mdi_delimiter_sample = Some(*sample_index);
                if let Origin::Express(t) = p.origin {
                    self.gptp_sampled_capture(
                        t,
                        gptp::state::CaptureKind::Tx,
                        index,
                        *sample_index,
                        0,
                        Some(*generation),
                        gptp::state::ReferencePlane::MdiMergeDelimiter,
                        d,
                        c,
                    )?;
                }
                self.merge_packet_record(d, id, "actual_mdi_delimiter_end")?;
            }
            PhyEffect::XmiDelimiter { .. } if p.smd_pending => {
                self.merge_event(MergeEvent::Smd(id), d, c)?
            }
            PhyEffect::XmiEnd { .. } => self.merge_event(MergeEvent::Eof(id), d, c)?,
            PhyEffect::XmiReleased { .. } => self.merge_event(MergeEvent::Release(id), d, c)?,
            PhyEffect::MdiCodedEnd { .. } => {
                self.merge_mut(d)?
                    .packets
                    .get_mut(&id)
                    .unwrap()
                    .raw
                    .as_mut()
                    .unwrap()
                    .coded_end = true;
                self.merge_packet_record(d, id, "actual_mdi_coded_end")?;
            }
            PhyEffect::RawMPacketIngress {
                receiver_endpoint,
                xmii_bytes,
                origins,
                receive_cut_sample,
                decoded_at_sample,
                delimiter_sample,
                generation,
                final_generation,
                ..
            } => {
                let reverse = self.merge_state(d).unwrap().identity.reverse[p.direction];
                let receiver = self.prepared.phy_attachments[index]
                    .config
                    .physical_ports
                    .iter()
                    .find(|port| {
                        port.tx_port == self.prepared.ethernet.directions[reverse].from_port
                    })
                    .ok_or_else(|| error("raw receiver not bound"))?;
                if receiver.endpoint != *receiver_endpoint
                    || origins.len() != 1
                    || origins[0].frame_id != raw.origin_frame_id
                    || origins[0].source_endpoint != raw.source_endpoint
                    || origins[0].attempt != 0
                    || receive_cut_sample > decoded_at_sample
                    || raw.delimiter_generation != Some(*generation)
                    || raw.generation != *final_generation
                    || delimiter_sample.is_some_and(|sample| sample > *decoded_at_sample)
                    || !p.eof_done
                    || p.arrival_done
                {
                    return Err(error(
                        "raw Merge decoded ingress lineage or causal order differs",
                    ));
                }
                let packet = self.merge_mut(d)?.packets.get_mut(&id).unwrap();
                let binding = packet.raw.as_mut().unwrap();
                binding.receive_cut_sample = Some(*receive_cut_sample);
                binding.decoded_at_sample = Some(*decoded_at_sample);
                binding.rx_mdi_delimiter_sample = *delimiter_sample;
                packet.received = Some(xmii_bytes.clone());
                packet.arrival = c.now();
                packet.complete = true;
                self.merge_arrival(p.direction, p.epoch, id, d, c)?;
            }
            PhyEffect::XmiAborted { reason, .. } => {
                self.merge_phy_fault(index, &raw.source_endpoint, reason, d, c)?
            }
            _ => {}
        }
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
}
