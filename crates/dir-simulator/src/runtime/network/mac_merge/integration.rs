//! One physical owner: these methods only stage NetworkDelta and common FES work.
use super::super::*;
use super::*;
use crate::types::network::mac_merge::{MergeFaultAction, PreparedMacMerge};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Port {
    #[serde(default)]
    pub permission_epoch: u64,
    #[serde(default)]
    pub permission: Option<super::phy::PermissionProof>,
    pub tx: MergeTxState,
    pub rx: MergeRxState,
    pub assembly_packets: Vec<Packet>,
    pub verify: VerificationState,
    pub selected_pmac: Option<usize>,
    pub selected_emac: Option<usize>,
    pub busy: Option<u64>,
    pub lldp_pending: bool,
    pub next_advertisement: Option<u64>,
    pub capability_expires: Option<u64>,
    pub capability_generation: u64,
    pub verify_timer_commit: bool,
    pub capability_packet: Option<Packet>,
    pub last_response_packet: Option<Packet>,
    pub last_verify_packet: Option<Packet>,
    pub accounting_journal: Option<super::journal::Journal>,
    pub applied_tas: Option<super::tas::TasControl>,
    pub overrun_packet: Option<u64>,
    pub tas_control: Option<super::tas::TasControl>,
    pub tas_generation: u64,
    pub tas_planned_at: u64,
    pub accounting_at: u64,
    pub accounting: Option<crate::runtime::ethernet::tsn::merge::MergeShaperView>,
    pub ordinal: u64,
    pub link_up: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Packet {
    pub id: u64,
    pub direction: usize,
    pub epoch: u64,
    pub ordinal: u64,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_control_id: Option<String>,
    pub bytes: Vec<u8>,
    pub received: Option<Vec<u8>>,
    /// A raw packet is retained until both its actual receive outcome and coded tail.
    #[serde(default)]
    pub raw: Option<super::phy::RawBinding>,
    pub complete: bool,
    pub plan: Option<TxPacketPlan>,
    pub token: Option<ControlTxToken>,
    pub demand_ps: Option<u64>,
    pub smd_pending: bool,
    pub sof: u64,
    pub eof: u64,
    pub release: u64,
    pub arrival: u64,
    pub eof_done: bool,
    pub release_done: bool,
    pub arrival_done: bool,
    pub aborted_ps: Option<u64>,
    pub invalidated_ps: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Origin {
    Pmac(usize),
    Express(usize),
    Lldp,
    WireControl,
    Control(PacketKind),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MergeEvent {
    Eof(u64),
    Accounting(u64),
    Tas {
        direction: usize,
        control: super::tas::TasControl,
    },
    Release(u64),
    Smd(u64),
    Advertise(usize),
    Expiry {
        direction: usize,
        generation: u64,
    },
    Verify {
        direction: usize,
        token: VerifyTimer,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MergeState {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(in crate::runtime::network) express_sources:
        BTreeMap<usize, gptp::state::ExpressSourceEvidence>,
    pub identity: PreparedMacMerge,
    pub ports: Vec<Port>,
    pub packets: BTreeMap<u64, Packet>,
    pub next_packet: u64,
    #[serde(default)]
    pub phy_fault_at: BTreeMap<usize, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub phy_ready: BTreeMap<usize, std::collections::BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub phy_faulted: std::collections::BTreeSet<usize>,
    pub validated: BTreeMap<usize, Vec<u8>>,
    #[serde(deserialize_with = "super::super::checkpoint::records::deserialize")]
    pub records: Vec<(&'static str, String, Value)>,
}
pub(super) fn err(e: MergeError) -> Diagnostic {
    error(format!("MAC Merge: {e:?}"))
}
impl MergeState {
    pub fn new(p: &PreparedMacMerge) -> Result<Self> {
        Ok(Self {
            identity: p.clone(),
            ports: p
                .ports
                .iter()
                .map(|cfg| {
                    Ok(Port {
                        permission_epoch: 0,
                        permission: None,
                        tx: MergeTxState::new(0),
                        rx: MergeRxState::new(0, p.max_rx_mac_octets).map_err(err)?,
                        assembly_packets: vec![],
                        verify: VerificationState::new(
                            0,
                            cfg.as_ref().is_some_and(|c| c.admin_enabled),
                            cfg.as_ref().map_or(10, |c| c.verify_time_ms),
                        )
                        .map_err(err)?,
                        selected_pmac: None,
                        selected_emac: None,
                        busy: None,
                        lldp_pending: cfg.is_some(),
                        next_advertisement: None,
                        capability_expires: None,
                        capability_generation: 0,
                        verify_timer_commit: false,
                        capability_packet: None,
                        last_response_packet: None,
                        last_verify_packet: None,
                        accounting_journal: None,
                        applied_tas: None,
                        overrun_packet: None,
                        tas_control: None,
                        tas_generation: 0,
                        tas_planned_at: 0,
                        accounting_at: 0,
                        accounting: None,
                        ordinal: 0,
                        link_up: true,
                    })
                })
                .collect::<Result<_>>()?,
            packets: BTreeMap::new(),
            next_packet: 0,
            phy_fault_at: BTreeMap::new(),
            phy_ready: BTreeMap::new(),
            phy_faulted: std::collections::BTreeSet::new(),
            validated: BTreeMap::new(),
            records: vec![],
            express_sources: BTreeMap::new(),
        })
    }
}
impl NetworkState {
    pub(super) fn merge_state<'a>(&'a self, d: &'a NetworkDelta) -> Option<&'a MergeState> {
        d.mac_merge.as_ref().or(self.mac_merge.as_ref())
    }
    pub(super) fn merge_mut<'a>(&self, d: &'a mut NetworkDelta) -> Result<&'a mut MergeState> {
        if d.mac_merge.is_none() {
            d.mac_merge = self.mac_merge.clone();
        }
        d.mac_merge
            .as_mut()
            .ok_or_else(|| error("missing MAC Merge owner"))
    }
    pub(in crate::runtime::network) fn mac_merge_snapshot(&self) -> Option<Value> {
        let state = self.mac_merge.as_ref()?;
        Some(
            json!({"standard":state.identity.standard,"capability_adapter":"dir_lldp_capability_v1","validated_ingresses":state.validated.len().to_string(),"ports":state.ports.iter().enumerate().filter(|(i,_)|state.identity.participating[*i]).map(|(i,p)|json!({"port":self.prepared.ethernet.directions[i].from_port,"epoch":p.tx.epoch.to_string(),"p_enable":p.verify.p_enable(),"p_active":p.verify.p_active(),"hold":p.tx.hold,"last_mm_ctl":p.applied_tas.map(|c|if c.hold{"hold"}else{"release"}),"verify_phase":format!("{:?}",p.verify.phase),"rx_incomplete":p.rx.assembly.is_some(),"rx_octets":p.rx.assembly.as_ref().map_or(0,|a|a.bytes.len()).to_string(),"physical_owner":p.busy.map(|id|id.to_string()),"selected_pmac":p.selected_pmac.map(|t|&self.transfers[t].row.transfer_id)})).collect::<Vec<_>>()}),
        )
    }
    pub(in crate::runtime::network) fn merge_physical_release(
        &self,
        i: usize,
        d: &NetworkDelta,
    ) -> Option<u64> {
        let state = self.merge_state(d)?;
        let id = state.ports.get(i)?.busy?;
        state.packets.get(&id).map(|p| p.release)
    }
    pub(in crate::runtime::network) fn merge_direction(&self, i: usize) -> bool {
        if let Some(bindings) = &self.prepared.bindings {
            return bindings.ports.get(i).is_some_and(|port| {
                port.mac == crate::types::network::bindings::MacOwner::Clause99
            });
        }
        self.prepared
            .extensions
            .as_ref()
            .and_then(|e| e.mac_merge.as_ref())
            .is_some_and(|p| p.participating[i])
    }
    pub(super) fn merge_due(&self, d: &mut NetworkDelta, at: u64, event: MergeEvent) {
        self.due_mut(d).push(Due {
            at,
            kind: Completion::MacMerge(event),
        });
    }
    pub(in crate::runtime::network) fn initialize_mac_merge(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(state) = self.merge_state(d) else {
            return Ok(());
        };
        let cfg = state.identity.clone();
        for i in 0..cfg.ports.len() {
            if cfg.ports[i].is_some() {
                let next = add(c.now(), cfg.lldp_interval_ps)?;
                self.merge_mut(d)?.ports[i].next_advertisement = Some(next);
                self.merge_due(d, next, MergeEvent::Advertise(i));
            }
        }
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn merge_record(
        &self,
        d: &mut NetworkDelta,
        schema: &'static str,
        id: String,
        data: Value,
    ) -> Result<()> {
        self.merge_mut(d)?.records.push((schema, id, data));
        Ok(())
    }
    pub(super) fn merge_port_record(
        &self,
        d: &mut NetworkDelta,
        i: usize,
        now: u64,
        cause: &str,
    ) -> Result<()> {
        let p = &self.merge_state(d).unwrap().ports[i];
        let data = json!({"port":self.prepared.ethernet.directions[i].from_port,"epoch":p.tx.epoch.to_string(),"p_enable":p.verify.p_enable(),"p_active":p.verify.p_active(),"hold":p.tx.hold,"last_mm_ctl":p.applied_tas.map(|c|if c.hold{"hold"}else{"release"}),"phase":format!("{:?}",p.verify.phase),"attempts":p.verify.attempts.to_string(),"send_v":p.verify.send_v,"send_r":p.verify.send_r,"peer_evidence":p.verify.peer.as_ref().map(|p|&p.lldp_evidence_id),"capability_expires_ps":opt(p.capability_expires),"cause":cause});
        self.merge_record(
            d,
            "dir.mac_merge.verification",
            format!("{i}:{now}:{}", d.record_sequence),
            data,
        )
    }
    pub(super) fn merge_packet_record(
        &self,
        d: &mut NetworkDelta,
        id: u64,
        cause: &str,
    ) -> Result<()> {
        let p = &self.merge_state(d).unwrap().packets[&id];
        let kind = decode_packet(&p.bytes).map_err(err)?.kind;
        let accounting = self.merge_state(d).unwrap().ports[p.direction].accounting;
        let data = json!({"packet_id":id.to_string(),"port":self.prepared.ethernet.directions[p.direction].from_port,"epoch":p.epoch.to_string(),"packet_ordinal":p.ordinal.to_string(),"kind":format!("{kind:?}"),"wire_control_id":p.wire_control_id,"wire_priority":p.wire_control_id.as_ref().map(|id|self.wire_priority(p.direction,id,d)),"priority":self.wire_packet_class(p,d),"transfer_index":match p.origin{Origin::Pmac(t)|Origin::Express(t)=>Some(t.to_string()),_=>None},"transfer":match p.origin{Origin::Pmac(t)|Origin::Express(t)=>Some(self.transfer(d,t).row.transfer_id.clone()),_=>None},"bitrate_bps":self.prepared.ethernet.directions[p.direction].bitrate_bps.to_string(),"smd":format!("{:02x}",if matches!(kind,PacketKind::Continuation{..}){p.bytes[6]}else{p.bytes[7]}),"frame_count":match kind{PacketKind::Start{frame_count}|PacketKind::Continuation{frame_count,..}=>Some(frame_count.to_string()),_=>None},"frag_count":match kind{PacketKind::Continuation{frag_count,..}=>Some(frag_count.to_string()),_=>None},"crc_kind":if matches!(kind,PacketKind::Verify|PacketKind::Respond)||p.plan.as_ref().is_some_and(|p|!p.final_packet){"mcrc"}else{"fcs"},"wire_hex":ingress::hex(&p.bytes),"received_hex":p.received.as_ref().map(|b|ingress::hex(b)),"sof_ps":p.sof.to_string(),"planned_eof_ps":p.eof.to_string(),"planned_release_ps":p.release.to_string(),"planned_arrival_ps":if p.raw.is_some(){None}else{Some(p.arrival.to_string())},"raw_phy":p.raw,"timing_plane":if p.raw.is_some(){"actual_xmii"}else if self.configured_endpoint(p.direction).is_some(){"configured_media"}else{"ideal_mac"},"eof_ps":opt(p.eof_done.then_some(p.eof)),"release_ps":opt(p.release_done.then_some(p.release)),"arrival_ps":opt((p.arrival_done && p.received.is_some()).then_some(p.arrival)),"fault_outcome":if p.raw.is_some() && !p.arrival_done {"pending"}else if p.received.is_none(){"dropped"}else if !p.complete{"truncated"}else if p.received.as_ref()!=Some(&p.bytes){"corrupted"}else{"none"},"aborted_ps":opt(p.aborted_ps),"invalidated_ps":opt(p.invalidated_ps),"complete":p.complete,"source_start":p.plan.as_ref().map(|p|p.start_offset.to_string()),"source_end":p.plan.as_ref().map(|p|p.end_offset.to_string()),"final_packet":p.plan.as_ref().map(|p|p.final_packet),"accounting_segment":accounting.map(|a|format!("{:?}",a.segment)),"cbs_transmit_class":accounting.and_then(|a|a.transmit_class),"cause":cause});
        self.merge_record(d, "dir.mac_merge.packet", id.to_string(), data)
    }
    /// A retained selection remains the FIFO head of its class. Its remaining
    /// original octets plus Merge preamble/header and IPG define admission fit.
    pub(in crate::runtime::network) fn merge_retained_head(
        &self,
        t: usize,
        d: &NetworkDelta,
    ) -> Result<Head> {
        let transfer = self.transfer(d, t);
        let i = transfer.direction;
        let port = &self
            .merge_state(d)
            .ok_or_else(|| error("retained Merge head without owner"))?
            .ports[i];
        let octets = if port.selected_pmac == Some(t) {
            port.tx
                .selected
                .as_ref()
                .map_or(transfer.row.wire.mac_bytes, |s| {
                    (s.mac.len() - s.offset) as u64
                })
        } else {
            transfer.row.wire.mac_bytes
        };
        Ok(Head {
            occupancy_ps: bit_duration(
                (octets + 8 + 12) * 8,
                self.prepared.ethernet.directions[i].bitrate_bps,
            )
            .map_err(err)?,
        })
    }
    pub(super) fn merge_resume_gate(
        &self,
        i: usize,
        t: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        let managed = self
            .prepared
            .bindings
            .as_ref()
            .and_then(|b| b.ports.get(i))
            .is_some_and(|p| {
                p.schedule_writer == Some(crate::types::network::bindings::ScheduleWriter::Qcc)
            });
        if !managed {
            // Legacy Clause99 continuation belongs to the already selected
            // frame; HOLD governs it and CBS credit may remain negative.
            return Ok(true);
        }
        let Some(tsn) = &self.tsn else {
            return Ok(true);
        };
        let name = &self.prepared.ethernet.directions[i].from_port;
        tsn.retained_gate_fit_clock_in(
            &d.tsn,
            name,
            self.transfer(d, t).row.priority,
            now,
            self.merge_retained_head(t, d)?.occupancy_ps,
            self.clock_for_port(d, name),
        )
    }
    /// The capability adapter emits an untagged ordinary MAC frame. Its class
    /// is the same ingress default priority used for other untagged control.
    pub(in crate::runtime::network) fn merge_lldp_priority(&self, i: usize) -> u8 {
        let name = &self.prepared.ethernet.directions[i].from_port;
        self.prepared
            .ethernet
            .port_policies
            .iter()
            .find(|p| p.port == *name)
            .map_or(0, |p| p.default_priority)
    }
    pub(in crate::runtime::network) fn merge_lldp_head(
        &self,
        i: usize,
        d: &NetworkDelta,
    ) -> Result<Head> {
        let state = self.merge_state(d).unwrap();
        let p = &state.ports[i];
        let mac = lldp::advertise(
            &self.prepared.ethernet.directions[i].from_port,
            state.identity.ports[i].as_ref().unwrap(),
            p.verify.p_enable(),
            p.verify.p_active(),
            state.identity.lldp_ttl_seconds,
        );
        Ok(Head {
            occupancy_ps: bit_duration(
                (mac.len() as u64 + 20) * 8,
                self.prepared.ethernet.directions[i].bitrate_bps,
            )
            .map_err(err)?,
        })
    }
    pub(super) fn merge_lldp_permission(
        &self,
        i: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        if !self.output_request_allowed(i, false, d, now) {
            return Ok(false);
        }
        let Some(tsn) = &self.tsn else {
            return Ok(true);
        };
        let name = &self.prepared.ethernet.directions[i].from_port;
        Ok(tsn
            .retained_start_permission_clock_in(
                &d.tsn,
                name,
                self.merge_lldp_priority(i),
                now,
                self.merge_lldp_head(i, d)?.occupancy_ps,
                self.clock_for_port(d, name),
            )?
            .is_some())
    }
    pub(super) fn merge_fresh_permission(
        &self,
        i: usize,
        t: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        let Some(tsn) = &self.tsn else {
            return Ok(true);
        };
        let name = &self.prepared.ethernet.directions[i].from_port;
        Ok(tsn
            .retained_start_permission_clock_in(
                &d.tsn,
                name,
                self.transfer(d, t).row.priority,
                now,
                self.merge_retained_head(t, d)?.occupancy_ps,
                self.clock_for_port(d, name),
            )?
            .is_some())
    }
    fn merge_select(&self, i: usize, mask: u8, d: &mut NetworkDelta) -> Result<Option<usize>> {
        let ready = self.tsn.as_ref().map_or(255, |_| {
            d.tsn
                .ready
                .get(&self.prepared.ethernet.directions[i].from_port)
                .copied()
                .unwrap_or(0)
        });
        let now = self.merge_state(d).unwrap().ports[i].accounting_at;
        let mut selected = None;
        for p in (0..8).rev().filter(|p| mask & ready & (1 << p) != 0) {
            if let Some(&t) = self.queue(d, i).waiting[p].front() {
                if self.merge_fresh_permission(i, t, d, now)? {
                    selected = Some((p, t));
                    break;
                }
            }
        }
        let Some((priority, t)) = selected else {
            return Ok(None);
        };
        let bytes = self.transfer(d, t).row.wire.mac_bytes;
        let q = self.queue_mut(d, i);
        q.waiting[priority].pop_front();
        q.bytes[priority] = q.bytes[priority]
            .checked_sub(bytes)
            .ok_or_else(|| error("Merge queue byte underflow"))?;
        Ok(Some(t))
    }
    pub(super) fn merge_fault(&self, cfg: &PreparedMacMerge, p: &mut Packet) -> Result<()> {
        p.received = Some(p.bytes.clone());
        p.complete = true;
        for f in cfg
            .faults
            .iter()
            .filter(|f| f.direction == p.direction && f.packet_ordinal == p.ordinal)
        {
            match f.action {
                MergeFaultAction::Drop => p.received = None,
                MergeFaultAction::Corrupt { octet, xor } => {
                    let b = p
                        .received
                        .as_mut()
                        .and_then(|b| b.get_mut(octet))
                        .ok_or_else(|| {
                            error("MAC Merge corrupt fault octet outside actual packet")
                        })?;
                    *b ^= xor;
                }
                MergeFaultAction::Truncate { octets } => {
                    let b = p
                        .received
                        .as_mut()
                        .ok_or_else(|| error("MAC Merge conflicting packet faults"))?;
                    if octets >= b.len() {
                        return Err(error("MAC Merge truncate must remove actual octets"));
                    }
                    b.truncate(octets);
                    p.complete = false;
                }
            }
        }
        Ok(())
    }
    fn merge_replan(&self, id: u64, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let now = c.now();
        crate::allocation::reservation_checkpoint("mac_merge_cut")?;
        let mut p = self.merge_state(d).unwrap().packets[&id].clone();
        if p.eof_done || now >= p.eof {
            return Ok(());
        }
        let cfg = self.merge_state(d).unwrap().identity.clone();
        let rate = self.prepared.ethernet.directions[p.direction].bitrate_bps;
        let plan = self.merge_mut(d)?.ports[p.direction]
            .tx
            .plan_packet(p.sof, p.demand_ps, rate)
            .map_err(err)?;
        p.bytes = plan.bytes.clone();
        p.eof = plan.eof_ps;
        p.release = plan.release_ps;
        p.arrival = add(p.eof, self.physical_receive_delay(p.direction)?)?;
        p.plan = Some(plan.clone());
        if p.raw.is_some() {
            self.merge_phy_replace(&mut p, d, now)?;
        } else {
            self.merge_fault(&cfg, &mut p)?;
        }
        self.due_mut(d).retain(|v|!matches!(&v.kind,Completion::MacMerge(MergeEvent::Eof(x)|MergeEvent::Release(x)) if *x==id));
        if p.raw.is_none() {
            self.merge_due(d, p.eof, MergeEvent::Eof(id));
            self.merge_due(d, p.release, MergeEvent::Release(id));
        }
        if let Origin::Pmac(t) = p.origin {
            let mut row = self.transfer(d, t).clone();
            row.row.planned_eof_ps = plan.final_packet.then_some(p.eof);
            row.row.planned_release_ps = plan.final_packet.then_some(p.release);
            row.row.planned_arrival_ps =
                (plan.final_packet && p.raw.is_none()).then_some(p.arrival);
            put(&mut d.transfers, t, row);
        }
        let raw_attachment = p.raw.as_ref().map(|r| r.attachment);
        self.merge_mut(d)?.packets.insert(id, p);
        if let Some(index) = raw_attachment {
            self.phy_plan_boundary(index, d, c)?;
        }
        self.merge_accounting_schedule(id, d, now)?;
        self.merge_shaper_transition(self.merge_state(d).unwrap().packets[&id].direction, d, now)?;
        self.merge_packet_record(d, id, "cut_replanned")
    }
    fn merge_start(
        &self,
        i: usize,
        origin: Origin,
        wire_control_id: Option<String>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let now = c.now();
        let cfg = self.merge_state(d).unwrap().identity.clone();
        let rate = self.prepared.ethernet.directions[i].bitrate_bps;
        let (bytes, plan, token) = match origin {
            Origin::Pmac(t) => {
                if self.merge_state(d).unwrap().ports[i].tx.selected.is_none() {
                    let m = ingress::bytes(&self.transfer(d, t).row.wire.mac_hex).map_err(err)?;
                    let p = &self.merge_state(d).unwrap().ports[i];
                    let active = p.verify.p_active();
                    let add = p.verify.peer.as_ref().map_or(0, |p| p.add_frag_size);
                    self.merge_mut(d)?.ports[i]
                        .tx
                        .select_pmac(m, active, add)
                        .map_err(err)?;
                }
                let plan = self.merge_mut(d)?.ports[i]
                    .tx
                    .plan_packet(now, None, rate)
                    .map_err(err)?;
                (plan.bytes.clone(), Some(plan), None)
            }
            Origin::Express(t) => (
                encode_express(
                    &ingress::bytes(&self.transfer(d, t).row.wire.mac_hex).map_err(err)?,
                )
                .map_err(err)?,
                None,
                None,
            ),
            Origin::WireControl => {
                let id = wire_control_id
                    .as_ref()
                    .ok_or_else(|| error("Wire Merge source missing"))?;
                (
                    encode_express(&self.wire_host(d).unwrap().tx[id].intent.bytes).map_err(err)?,
                    None,
                    None,
                )
            }
            Origin::Control(kind) => {
                if kind == PacketKind::Respond {
                    crate::allocation::reservation_checkpoint("mac_merge_response_packet")?;
                }
                let token = self.merge_state(d).unwrap().ports[i]
                    .verify
                    .control_tx_token(kind)
                    .map_err(err)?;
                (encode_control(kind).map_err(err)?, None, Some(token))
            }
            Origin::Lldp => {
                let p = &self.merge_state(d).unwrap().ports[i];
                let m = lldp::advertise(
                    &self.prepared.ethernet.directions[i].from_port,
                    cfg.ports[i].as_ref().unwrap(),
                    p.verify.p_enable(),
                    p.verify.p_active(),
                    cfg.lldp_ttl_seconds,
                );
                (encode_express(&m).map_err(err)?, None, None)
            }
        };
        let eof = add(
            now,
            bit_duration(bytes.len() as u64 * 8, rate).map_err(err)?,
        )?;
        let release = add(eof, bit_duration(96, rate).map_err(err)?)?;
        let arrival = add(eof, self.physical_receive_delay(i)?)?;
        let state = self.merge_mut(d)?;
        let id = state.next_packet;
        state.next_packet = add(id, 1)?;
        let port = &mut state.ports[i];
        let ordinal = port.ordinal;
        port.ordinal = add(ordinal, 1)?;
        port.busy = Some(id);
        if origin == Origin::Lldp {
            port.lldp_pending = false;
        }
        let smd_pending =
            matches!(origin, Origin::Pmac(_)) && plan.as_ref().is_some_and(|p| p.start_offset == 0);
        let mut p = Packet {
            id,
            direction: i,
            epoch: port.tx.epoch,
            ordinal,
            origin,
            wire_control_id: wire_control_id.clone(),
            bytes,
            received: None,
            raw: None,
            complete: true,
            plan,
            token,
            demand_ps: None,
            smd_pending,
            sof: now,
            eof,
            release,
            arrival,
            eof_done: false,
            release_done: false,
            arrival_done: false,
            aborted_ps: None,
            invalidated_ps: None,
        };
        let raw = self.merge_phy_attachment(i).is_some();
        if raw {
            self.merge_phy_admit(&mut p, d, c)?;
        } else {
            self.merge_fault(&cfg, &mut p)?;
        }
        if let Some(wire_id) = &wire_control_id {
            self.wire_claim(i, wire_id, eof, release, (!raw).then_some(arrival), d, c)?;
        }
        self.merge_mut(d)?.packets.insert(id, p);
        if let Origin::Pmac(t) | Origin::Express(t) = origin {
            let first = self.transfer(d, t).row.sof_ps.is_none();
            let mut row = self.transfer(d, t).clone();
            row.row.time_ps = now;
            row.row.status = "transmitting".into();
            if first {
                row.row.sof_ps = Some(now);
                row.sof_epoch = Some(self.epoch_after(d));
            }
            let final_packet = self.merge_state(d).unwrap().packets[&id]
                .plan
                .as_ref()
                .is_none_or(|p| p.final_packet);
            row.row.planned_eof_ps = final_packet.then_some(eof);
            row.row.planned_release_ps = final_packet.then_some(release);
            row.row.planned_arrival_ps = (final_packet && !raw).then_some(arrival);
            put(&mut d.transfers, t, row);
            self.queue_mut(d, i).active = Some(t);
            if first {
                self.replication_sof(t, d, now)?;
                self.qcc_sof(t, d, c)?;
                if self.preemption.is_some() && !self.gptp_packet(d, t) {
                    self.preemption_sof(t, d, now)?;
                }
            }
        }
        if !raw {
            self.merge_due(d, eof, MergeEvent::Eof(id));
            self.merge_due(d, release, MergeEvent::Release(id));
        }
        if smd_pending && !raw {
            self.merge_due(
                d,
                add(now, bit_duration(56, rate).map_err(err)?)?,
                MergeEvent::Smd(id),
            );
        }
        self.merge_accounting_schedule(id, d, now)?;
        self.merge_shaper_transition(i, d, now)?;
        if let Origin::Express(t) = origin {
            if self.gptp_packet(d, t) {
                crate::allocation::reservation_checkpoint("mac_merge_gptp_source")?;
                if self.merge_state(d).unwrap().express_sources.len()
                    >= self.prepared.gptp.as_ref().unwrap().max_journal_entries
                {
                    return Err(error("Merge gPTP retained source capacity"));
                }
                let source = gptp::state::ExpressSourceEvidence {
                    source: gptp::state::MacSource::MergeExpress {
                        transfer: t,
                        direction: i,
                        packet_id: id,
                        epoch: self.merge_state(d).unwrap().packets[&id].epoch,
                    },
                    sof_ps: now,
                    eof_planned_ps: eof,
                    release_planned_ps: release,
                    arrival_planned_ps: arrival,
                    eof_actual_ps: None,
                    arrival_actual_ps: None,
                    mac_hex: self.transfer(d, t).row.wire.mac_hex.clone(),
                };
                if self
                    .merge_mut(d)?
                    .express_sources
                    .insert(t, source)
                    .is_some()
                {
                    return Err(error("duplicate Merge gPTP source"));
                }
            }
            self.gptp_tx_start(
                t,
                Some(id),
                self.merge_state(d).unwrap().packets[&id].epoch,
                now,
                d,
                c,
            )?;
        }
        self.merge_packet_record(d, id, "sof")
    }
    pub(in crate::runtime::network) fn merge_arbitrate(
        &self,
        i: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let cfg = self.merge_state(d).unwrap().identity.clone();
        if !cfg.participating[i] {
            return Ok(());
        }
        self.merge_shaper_transition(i, d, c.now())?;
        if !self.merge_state(d).unwrap().ports[i].link_up {
            self.wire_prune_candidates(i, d, c)?;
            return Ok(());
        }
        self.merge_plan_tas(i, d, c.now())?;
        self.wire_prune_candidates(i, d, c)?;
        let wire = self.wire_candidate(i, d, c.now())?;
        let new_allowed = self.output_request_allowed(i, false, d, c.now());
        let mask = cfg.ports[i].as_ref().map_or(255, |p| p.express_mask);
        if new_allowed
            && self.merge_state(d).unwrap().ports[i]
                .selected_emac
                .is_none()
        {
            let t = self.merge_select(i, mask, d)?;
            self.merge_mut(d)?.ports[i].selected_emac = t;
        }
        let port = &self.merge_state(d).unwrap().ports[i];
        let lldp = port.lldp_pending
            && cfg.ports[i].is_some()
            && self.merge_lldp_permission(i, d, c.now())?;
        // A sampled owner may hold an express selection across real GMII
        // admission cuts. Re-evaluate its current eligibility before use.
        let express = if let Some(t) = port.selected_emac {
            ((new_allowed || self.transfer(d, t).row.sof_ps.is_some())
                && self.merge_fresh_permission(i, t, d, c.now())?)
            .then_some(t)
        } else {
            None
        };
        if let Some(id) = port.busy {
            let p = &self.merge_state(d).unwrap().packets[&id];
            if matches!(p.origin, Origin::Pmac(_))
                && !p.eof_done
                && c.now() < p.eof
                && (wire.is_some() || express.is_some() || port.tx.hold || lldp)
                && p.demand_ps.is_none()
            {
                self.merge_mut(d)?.packets.get_mut(&id).unwrap().demand_ps = Some(c.now());
                self.merge_replan(id, d, c)?;
            }
            return Ok(());
        }
        if !self.merge_phy_ready(i, d, c.now())? {
            return Ok(());
        }
        let pmac = self.merge_state(d).unwrap().ports[i].selected_pmac;
        let resume_gate = pmac
            .map(|t| self.merge_resume_gate(i, t, d, c.now()))
            .transpose()?
            .unwrap_or(false);
        let port = &self.merge_state(d).unwrap().ports[i];
        let origin = if wire.is_some() {
            Some(Origin::WireControl)
        } else if let Some(t) = express {
            Some(Origin::Express(t))
        } else if lldp {
            Some(Origin::Lldp)
        } else if let Some(t) = port.selected_pmac.filter(|_| !port.tx.hold && resume_gate) {
            Some(Origin::Pmac(t))
        } else if port.verify.send_r {
            Some(Origin::Control(PacketKind::Respond))
        } else if port.verify.send_v {
            Some(Origin::Control(PacketKind::Verify))
        } else if new_allowed && !port.tx.hold && port.selected_pmac.is_none() {
            let mask = cfg.ports[i].as_ref().map_or(0, |p| p.preemptable_mask);
            let t = self.merge_select(i, mask, d)?;
            self.merge_mut(d)?.ports[i].selected_pmac = t;
            t.map(Origin::Pmac)
        } else {
            None
        };
        if let Some(origin) = origin {
            if !self.merge_fractional_permission(i, origin, wire.as_deref(), d, c.now())? {
                return Ok(());
            }
            self.merge_start(i, origin, wire, d, c)?;
        }
        Ok(())
    }
    /// The phase1 probe defers commit behind all phase1 RX events already
    /// reserved by phase0 physical EOF, preserving phase2 arbitration.
    pub(in crate::runtime::network) fn merge_timer(
        &self,
        i: usize,
        token: VerifyTimer,
        commit: bool,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let state = self
            .merge_state(d)
            .ok_or_else(|| error("Merge timer without owner"))?;
        let p = state
            .ports
            .get(i)
            .ok_or_else(|| error("Merge timer direction"))?;
        if p.verify.timer != Some(token) {
            return Ok(());
        }
        if p.verify_timer_commit != commit || c.now() != token.due_ps {
            return Err(error("Merge timer stage/deadline mismatch"));
        }
        if !commit {
            self.merge_mut(d)?.ports[i].verify_timer_commit = true;
            Self::schedule(
                c,
                d,
                c.now(),
                Event::MacMergeTimer {
                    direction: i,
                    token,
                    commit: true,
                },
            )?;
        } else {
            self.merge_mut(d)?.ports[i].verify_timer_commit = false;
            self.merge_event(
                MergeEvent::Verify {
                    direction: i,
                    token,
                },
                d,
                c,
            )?;
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn merge_event(
        &self,
        event: MergeEvent,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let now = c.now();
        match event {
            MergeEvent::Tas { direction, control } => {
                self.merge_tas_event(direction, control, d, now)?;
            }
            MergeEvent::Accounting(id) => {
                if let Some(packet) = self.merge_state(d).unwrap().packets.get(&id) {
                    let i = packet.direction;
                    self.merge_shaper_transition(i, d, now)?;
                    self.merge_packet_record(d, id, "accounting_boundary")?;
                }
            }
            MergeEvent::Advertise(i) => {
                let cfg = self.merge_state(d).unwrap().identity.clone();
                let next = add(now, cfg.lldp_interval_ps)?;
                let p = &mut self.merge_mut(d)?.ports[i];
                p.next_advertisement = Some(next);
                p.lldp_pending = p.link_up;
                self.merge_due(d, next, MergeEvent::Advertise(i));
                self.merge_shaper_transition(i, d, now)?;
            }
            MergeEvent::Expiry {
                direction: i,
                generation,
            } => {
                let p = &mut self.merge_mut(d)?.ports[i];
                if p.capability_generation == generation && p.capability_expires == Some(now) {
                    p.capability_expires = None;
                    p.capability_packet = None;
                    p.last_response_packet = None;
                    p.verify.withdraw_capabilities().map_err(err)?;
                    self.due_mut(d).retain(|d|!matches!(&d.kind,Completion::MacMerge(MergeEvent::Verify{direction,..}) if *direction==i));
                    self.qcc_protocol_changed(i, d, c)?;
                    self.merge_port_record(d, i, now, "capability_expired")?;
                }
            }
            MergeEvent::Verify {
                direction: i,
                token,
            } => {
                self.merge_mut(d)?.ports[i]
                    .verify
                    .timeout(token, now)
                    .map_err(err)?;
                self.merge_port_record(d, i, now, "verify_timeout")?;
            }
            MergeEvent::Smd(id) => {
                let Some(p) = self.merge_state(d).unwrap().packets.get(&id).cloned() else {
                    return Ok(());
                };
                if p.epoch != self.merge_state(d).unwrap().ports[p.direction].tx.epoch {
                    return Ok(());
                }
                let active = self.merge_state(d).unwrap().ports[p.direction]
                    .verify
                    .p_active();
                let changed = self.merge_mut(d)?.ports[p.direction]
                    .tx
                    .smd_sent(active)
                    .map_err(err)?;
                self.merge_mut(d)?.packets.get_mut(&id).unwrap().smd_pending = false;
                if changed {
                    self.merge_replan(id, d, c)?;
                }
            }
            MergeEvent::Eof(id) => {
                let Some(p) = self.merge_state(d).unwrap().packets.get(&id).cloned() else {
                    return Ok(());
                };
                let i = p.direction;
                if p.epoch != self.merge_state(d).unwrap().ports[i].tx.epoch {
                    return Ok(());
                }
                if now != p.eof || p.eof_done {
                    return Err(error("MAC Merge EOF owner mismatch"));
                }
                if let Some(plan) = &p.plan {
                    self.merge_mut(d)?.ports[i]
                        .tx
                        .complete_packet(plan, now)
                        .map_err(err)?;
                }
                if let Some(token) = p.token {
                    if self.merge_state(d).unwrap().ports[i]
                        .verify
                        .control_generation
                        == token.generation
                    {
                        if let Some(token) = self.merge_mut(d)?.ports[i]
                            .verify
                            .tx_complete_token(token, now)
                            .map_err(err)?
                        {
                            crate::allocation::reservation_checkpoint("mac_merge_verify_timer")?;
                            self.merge_mut(d)?.ports[i].verify_timer_commit = false;
                            Self::schedule(
                                c,
                                d,
                                token.due_ps,
                                Event::MacMergeTimer {
                                    direction: i,
                                    token,
                                    commit: false,
                                },
                            )?;
                        }
                        self.merge_port_record(d, i, now, "actual_control_eof")?;
                    }
                }
                if let Origin::Pmac(t) | Origin::Express(t) = p.origin {
                    let final_packet = p.plan.as_ref().is_none_or(|p| p.final_packet);
                    if final_packet {
                        let mut row = self.transfer(d, t).clone();
                        row.row.time_ps = now;
                        row.row.eof_ps = Some(now);
                        row.row.status = if row.row.drop_reason.is_some() {
                            "dropped"
                        } else {
                            "serialized"
                        }
                        .into();
                        put(&mut d.transfers, t, row);
                        if !self.pause_request_in_progress(i, d) {
                            self.pause_mac_completed(i, d, c)?;
                        }
                    }
                }
                if let Some(wire_id) = &p.wire_control_id {
                    self.wire_owner_eof(i, wire_id, d, c)?;
                }
                self.merge_mut(d)?.packets.get_mut(&id).unwrap().eof_done = true;
                if let Origin::Express(t) = p.origin {
                    if self.gptp_packet(d, t) {
                        self.merge_mut(d)?
                            .express_sources
                            .get_mut(&t)
                            .ok_or_else(|| error("Merge gPTP EOF source missing"))?
                            .eof_actual_ps = Some(now);
                    }
                    self.gptp_tx_eof(t, Some(id), d, c)?;
                }
                if p.raw.is_some() {
                    // Only actual decoded raw ingress can reserve a receive event.
                } else if p.received.is_some() {
                    crate::allocation::reservation_checkpoint("mac_merge_packet_arrival")?;
                    Self::schedule(
                        c,
                        d,
                        p.arrival,
                        Event::MacMergeArrival {
                            direction: i,
                            epoch: p.epoch,
                            packet: id,
                        },
                    )?;
                } else {
                    self.merge_mut(d)?
                        .packets
                        .get_mut(&id)
                        .unwrap()
                        .arrival_done = true;
                    if let Origin::Pmac(t) | Origin::Express(t) = p.origin {
                        self.merge_drop_transfer(t, d, now, "mac_merge_packet_lost")?;
                    }
                    if let Some(wire_id) = &p.wire_control_id {
                        self.wire_owner_fail(i, wire_id, "mac_merge_packet_lost", d, c)?;
                    }
                }
                self.merge_shaper_transition(i, d, now)?;
                self.merge_packet_record(d, id, "actual_eof")?;
            }
            MergeEvent::Release(id) => {
                let Some(p) = self.merge_state(d).unwrap().packets.get(&id).cloned() else {
                    return Ok(());
                };
                let i = p.direction;
                if p.epoch != self.merge_state(d).unwrap().ports[i].tx.epoch {
                    return Ok(());
                }
                if !p.eof_done
                    || now != p.release
                    || self.merge_state(d).unwrap().ports[i].busy != Some(id)
                {
                    return Err(error("MAC Merge release owner mismatch"));
                }
                let port = &mut self.merge_mut(d)?.ports[i];
                port.busy = None;
                match p.origin {
                    Origin::Express(_) => port.selected_emac = None,
                    Origin::Pmac(_) if p.plan.as_ref().unwrap().final_packet => {
                        port.selected_pmac = None
                    }
                    _ => {}
                }
                if let Origin::Pmac(t) | Origin::Express(t) = p.origin {
                    self.queue_mut(d, i).active = None;
                    if p.plan.as_ref().is_none_or(|p| p.final_packet) {
                        let mut row = self.transfer(d, t).clone();
                        row.row.time_ps = now;
                        row.row.release_ps = Some(now);
                        put(&mut d.transfers, t, row);
                    }
                }
                if let Some(wire_id) = &p.wire_control_id {
                    self.wire_owner_release(i, wire_id, d, c)?;
                }
                self.merge_mut(d)?
                    .packets
                    .get_mut(&id)
                    .unwrap()
                    .release_done = true;
                self.merge_shaper_transition(i, d, now)?;
                self.merge_packet_record(d, id, "actual_release")?;
            }
        }
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn merge_drop_transfer(
        &self,
        t: usize,
        d: &mut NetworkDelta,
        now: u64,
        reason: &str,
    ) -> Result<()> {
        let mut row = self.transfer(d, t).clone();
        if row.row.arrival_ps.is_none() {
            row.row.time_ps = now;
            row.row.status = "dropped".into();
            row.row.drop_reason = Some(reason.into());
            put(&mut d.transfers, t, row);
            self.replication_drop(t, d, now, reason)?;
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn merge_arrival(
        &self,
        i: usize,
        epoch: u64,
        id: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        // Epoch first: invalidated events must not dereference freed packets.
        let state = self
            .merge_state(d)
            .ok_or_else(|| error("Merge arrival without owner"))?;
        if state.ports.get(i).is_none_or(|p| p.tx.epoch != epoch) {
            return Ok(());
        }
        let cfg = state.identity.clone();
        let reverse = cfg.reverse[i];
        let packet = state
            .packets
            .get(&id)
            .ok_or_else(|| error("MAC Merge arrival packet missing"))?
            .clone();
        if packet.direction != i
            || !packet.eof_done
            || packet.arrival_done
            || packet.arrival != c.now()
        {
            return Err(error("MAC Merge arrival causal owner differs"));
        }
        let provenance = match packet.origin {
            Origin::Pmac(t) | Origin::Express(t) => RxProvenance::LocalTransfer(t),
            _ => RxProvenance::ExternalInjection {
                evidence_id: format!("mac-merge-packet:{id}"),
            },
        };
        let out = self.merge_mut(d)?.ports[reverse].rx.receive(
            epoch,
            c.now(),
            PhysicalPacket {
                bytes: packet
                    .received
                    .clone()
                    .ok_or_else(|| error("dropped packet has arrival"))?,
                provenance,
                complete: packet.complete,
            },
        );
        {
            let port = &mut self.merge_mut(d)?.ports[reverse];
            if let Some(a) = &port.rx.assembly {
                let n = a.fragments.len();
                if a.fragments.last().is_some_and(|f| {
                    f.received_at == c.now() && packet.received.as_ref() == Some(&f.packet.bytes)
                }) {
                    if n == 1 {
                        port.assembly_packets.clear();
                    }
                    let mut proof = packet.clone();
                    proof.arrival_done = true;
                    port.assembly_packets.push(proof);
                }
            } else {
                port.assembly_packets.clear();
            }
        }
        self.merge_mut(d)?
            .packets
            .get_mut(&id)
            .unwrap()
            .arrival_done = true;
        self.merge_packet_record(d, id, "actual_receive")?;
        if let Some(kind) = out.control {
            if cfg.ports[reverse].is_some() {
                let mut evidence = packet.clone();
                evidence.arrival_done = true;
                let port = &mut self.merge_mut(d)?.ports[reverse];
                if kind == PacketKind::Respond {
                    port.last_response_packet = Some(evidence);
                } else {
                    port.last_verify_packet = Some(evidence);
                }
                self.merge_mut(d)?.ports[reverse]
                    .verify
                    .receive_control(epoch, kind)
                    .map_err(err)?;
                if self.merge_state(d).unwrap().ports[reverse]
                    .verify
                    .timer
                    .is_none()
                {
                    self.due_mut(d).retain(|d|!matches!(&d.kind,Completion::MacMerge(MergeEvent::Verify{direction,..}) if *direction==reverse));
                }
                self.merge_port_record(d, reverse, c.now(), "actual_control_receive")?;
            }
        }
        if !out.errors.is_empty() {
            if let Some(wire_id) = &packet.wire_control_id {
                self.wire_owner_fail(i, wire_id, "mac_merge_receive_error", d, c)?;
            }
            if let Origin::Pmac(t) | Origin::Express(t) = packet.origin {
                self.merge_drop_transfer(t, d, c.now(), "mac_merge_receive_error")?;
            }
        }
        if let Some(validated) = out.ingress {
            if matches!(packet.origin, Origin::Lldp) {
                if cfg.ports[reverse].is_some() {
                    match lldp::receive(reverse, &validated, format!("mac-merge-packet:{id}")) {
                        Ok(observation) => {
                            let matching = self.merge_state(d).unwrap().ports[reverse]
                                .verify
                                .peer
                                .as_ref()
                                .is_some_and(|p| observation.matches(p));
                            // A shutdown or refresh without capability only affects its own MSAP.
                            if (observation.ttl == 0 || observation.capability.is_none())
                                && !matching
                            {
                                return Ok(());
                            }
                            let had_peer = self.merge_state(d).unwrap().ports[reverse]
                                .verify
                                .peer
                                .is_some();
                            let ttl = observation.ttl;
                            let mut evidence = packet.clone();
                            evidence.arrival_done = true;
                            self.due_mut(d).retain(|d|!matches!(&d.kind,Completion::MacMerge(MergeEvent::Expiry{direction,..}) if *direction==reverse));
                            let p = &mut self.merge_mut(d)?.ports[reverse];
                            p.capability_packet = Some(evidence);
                            p.capability_generation = add(p.capability_generation, 1)?;
                            if ttl == 0 || observation.capability.is_none() {
                                p.capability_packet = None;
                                p.last_response_packet = None;
                                p.capability_expires = None;
                                p.verify.withdraw_capabilities().map_err(err)?;
                                self.due_mut(d).retain(|d|!matches!(&d.kind,Completion::MacMerge(MergeEvent::Verify{direction,..}) if *direction==reverse));
                            } else {
                                let expires = add(c.now(), u64::from(ttl) * 1_000_000_000_000)?;
                                p.capability_expires = Some(expires);
                                p.verify
                                    .observe_capabilities(observation.capability.unwrap())
                                    .map_err(err)?;
                                if !p.verify.rcv_r {
                                    p.last_response_packet = None;
                                }
                                let generation = p.capability_generation;
                                self.merge_due(
                                    d,
                                    expires,
                                    MergeEvent::Expiry {
                                        direction: reverse,
                                        generation,
                                    },
                                );
                            }
                            let has_peer = self.merge_state(d).unwrap().ports[reverse]
                                .verify
                                .peer
                                .is_some();
                            if had_peer != has_peer {
                                self.qcc_protocol_changed(reverse, d, c)?;
                            }
                            self.merge_port_record(
                                d,
                                reverse,
                                c.now(),
                                "received_lldp_capability",
                            )?;
                        }
                        Err(_) => {
                            self.merge_record(d,"dir.mac_merge.capability",format!("invalid:{id}"),json!({"packet_id":id.to_string(),"accepted":false,"reason":"invalid_lldp_capability_adapter_packet"}))?;
                        }
                    }
                }
            } else if let Some(wire_id) = &packet.wire_control_id {
                self.wire_owner_receive(i, wire_id, &validated.bytes, d, c)?;
            } else if let RxProvenance::LocalTransfer(t) = validated.provenance {
                if let Origin::Express(t) = packet.origin {
                    if self.gptp_packet(d, t) {
                        if let Some(raw) = &packet.raw {
                            self.gptp_sampled_capture(
                                t,
                                gptp::state::CaptureKind::Rx,
                                raw.attachment,
                                raw.rx_mdi_delimiter_sample.ok_or_else(|| {
                                    error("validated raw gPTP ingress lacks mapped MDI delimiter")
                                })?,
                                0,
                                Some(raw.delimiter_generation.ok_or_else(|| {
                                    error("raw gPTP delimiter lacks source generation")
                                })?),
                                gptp::state::ReferencePlane::MdiMergeDelimiter,
                                d,
                                c,
                            )?;
                        }
                        self.merge_mut(d)?
                            .express_sources
                            .get_mut(&t)
                            .ok_or_else(|| error("Merge gPTP RX source missing"))?
                            .arrival_actual_ps = Some(c.now());
                        self.gptp_rx_validated(t, id, packet.epoch, &validated.bytes, d, c)?;
                    }
                }
                let decoded = ingress::decode(&validated.bytes).map_err(err)?;
                if self.merge_state(d).unwrap().validated.contains_key(&t) {
                    return Err(error("MAC Merge duplicate validated delivery"));
                }
                crate::allocation::reservation_checkpoint("mac_merge_validated_ingress")?;
                self.merge_mut(d)?.validated.insert(t, validated.bytes);
                let mut row = self.transfer(d, t).clone();
                row.row.wire = decoded;
                put(&mut d.transfers, t, row);
                if !self.gptp_packet(d, t) {
                    self.arrive(t, None, d, c)?;
                }
            }
        }
        let a = self.merge_state(d).unwrap().ports[reverse]
            .rx
            .assembly
            .as_ref();
        let data = json!({"packet_id":id.to_string(),"port":self.prepared.ethernet.directions[reverse].from_port,"epoch":epoch.to_string(),"errors":out.errors.iter().map(|e|format!("{e:?}")).collect::<Vec<_>>(),"incomplete":a.is_some(),"assembled_octets":a.map(|a|a.bytes.len().to_string()),"expected_fragment":a.map(|a|a.next_frag_count.to_string())});
        self.merge_record(d, "dir.mac_merge.assembly", format!("{reverse}:{id}"), data)?;
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(in crate::runtime::network) fn sync_mac_merge_links(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(state) = self.merge_state(d) else {
            return Ok(());
        };
        let cfg = state.identity.clone();
        let policy = self.policy(d);
        for i in 0..cfg.ports.len() {
            if !cfg.participating[i] {
                continue;
            }
            let up = policy
                .as_ref()
                .and_then(|p| {
                    p.link_up
                        .get(&self.prepared.ethernet.directions[i].from_port)
                })
                .copied()
                .unwrap_or(true);
            if up == self.merge_state(d).unwrap().ports[i].link_up {
                continue;
            }
            let old = self.merge_state(d).unwrap().ports[i].clone();
            let epoch = add(old.tx.epoch, 1)?;
            let mut affected = old
                .selected_pmac
                .into_iter()
                .chain(old.selected_emac)
                .collect::<std::collections::BTreeSet<_>>();
            self.queue_mut(d, i).active = None;
            let p = &mut self.merge_mut(d)?.ports[i];
            p.tx.reset(epoch);
            p.rx.reset(epoch);
            p.assembly_packets.clear();
            p.tas_control = None;
            p.applied_tas = None;
            p.tas_generation = 0;
            p.tas_planned_at = c.now();
            p.accounting = None;
            p.verify.link_reset(epoch, up).map_err(err)?;
            p.verify_timer_commit = false;
            p.link_up = up;
            p.busy = None;
            p.selected_pmac = None;
            p.selected_emac = None;
            p.capability_expires = None;
            p.capability_packet = None;
            p.last_response_packet = None;
            p.last_verify_packet = None;
            p.lldp_pending = up && cfg.ports[i].is_some();
            let ids = self
                .merge_state(d)
                .unwrap()
                .packets
                .values()
                .filter(|p| p.direction == i)
                .map(|p| p.id)
                .collect::<std::collections::BTreeSet<_>>();
            for id in &ids {
                if let Some(wire_id) = self.merge_state(d).unwrap().packets[id]
                    .wire_control_id
                    .clone()
                {
                    self.wire_owner_reset(i, &wire_id, "link_epoch_reset", d, c)?;
                }
                let packet = self.merge_mut(d)?.packets.get_mut(id).unwrap();
                packet.invalidated_ps = Some(c.now());
                if !packet.eof_done {
                    packet.aborted_ps = Some(c.now());
                }
                if let Origin::Pmac(t) | Origin::Express(t) = packet.origin {
                    affected.insert(t);
                }
                self.merge_packet_record(d, *id, "link_epoch_reset")?;
            }
            for t in affected {
                self.merge_drop_transfer(t, d, c.now(), "mac_merge_link_epoch_reset")?;
            }
            self.merge_mut(d)?.packets.retain(|_, p| p.direction != i);
            self.due_mut(d).retain(|d|!matches!(&d.kind,Completion::MacMerge(MergeEvent::Eof(id)|MergeEvent::Release(id)|MergeEvent::Smd(id)|MergeEvent::Accounting(id)) if ids.contains(id))&&!matches!(&d.kind,Completion::MacMerge(MergeEvent::Verify{direction,..}|MergeEvent::Expiry{direction,..}|MergeEvent::Tas{direction,..}) if *direction==i));
            self.qcc_protocol_changed(i, d, c)?;
            self.merge_shaper_transition(i, d, c.now())?;
            self.merge_port_record(d, i, c.now(), "link_epoch_reset")?;
            c.request_arbitration("network").map_err(model)?;
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn emit_mac_merge(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if let Some(state) = d.mac_merge.as_mut() {
            let records = std::mem::take(&mut state.records);
            state.packets.retain(|_, p| {
                !p.release_done || !p.arrival_done || p.raw.as_ref().is_some_and(|r| !r.coded_end)
            });
            if state.packets.len() > state.identity.max_pending_packets {
                return Err(error("MAC Merge pending packet limit exceeded"));
            }
            for (schema, id, data) in records {
                Self::record(c, d, schema, id, data)?;
            }
        }
        Ok(())
    }
}
impl NetworkState {
    pub(in crate::runtime::network) fn validate_merge_arrival(
        &self,
        i: usize,
        epoch: u64,
        id: u64,
        at: u64,
    ) -> Result<()> {
        let state = self
            .mac_merge
            .as_ref()
            .ok_or_else(|| error("MAC Merge event without owner"))?;
        let port = state
            .ports
            .get(i)
            .ok_or_else(|| error("MAC Merge event direction"))?;
        if port.tx.epoch != epoch {
            return Ok(());
        }
        let p = state
            .packets
            .get(&id)
            .ok_or_else(|| error("MAC Merge pending arrival packet absent"))?;
        if p.direction != i || p.epoch != epoch || p.arrival != at || !p.eof_done || p.arrival_done
        {
            return Err(error("MAC Merge pending arrival differs"));
        }
        Ok(())
    }
}
