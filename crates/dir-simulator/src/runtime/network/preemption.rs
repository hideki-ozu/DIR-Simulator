//! Fragment boundary scheduler. Owns no queue and schedules only through NetworkDelta.
use super::replication::identifier;
use super::*;
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TransferFragments {
    pub completed: u64,
    #[serde(default)]
    pub partial_mac_bytes: u64,
    #[serde(default)]
    pub reset_ps: Option<u64>,
    #[serde(default)]
    pub arrival_retired_ps: Option<u64>,
    pub occupied: u64,
    pub fragments: Vec<String>,
    pub gaps: Vec<String>,
    pub active_start: Option<u64>,
    pub active_end: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub(super) enum Work {
    Data(usize),
    Wire(String),
}
impl Work {
    pub(super) fn data(&self) -> Option<usize> {
        if let Self::Data(t) = self {
            Some(*t)
        } else {
            None
        }
    }
}
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Port {
    pub active: Option<Work>,
    pub active_end: Option<u64>,
    pub suspended: Option<usize>,
    pub after_express: bool,
    pub tx_epoch: u64,
    pub gap_epoch: u64,
    pub wake_epoch: u64,
    pub gap_due: Option<u64>,
    pub wake_due: Option<u64>,
    pub gap_id: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreemptionState {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub express_sources: BTreeMap<usize, gptp::state::ExpressSourceEvidence>,
    pub ports: Vec<Port>,
    pub fragments: BTreeMap<String, Value>,
    pub gaps: BTreeMap<String, Value>,
    pub deliveries: BTreeMap<String, Value>,
    #[serde(deserialize_with = "super::checkpoint::records::deserialize")]
    pub records: Vec<(&'static str, String, Value)>,
}
impl PreemptionState {
    pub fn new(ports: usize) -> Self {
        Self {
            ports: vec![Port::default(); ports],
            fragments: BTreeMap::new(),
            gaps: BTreeMap::new(),
            deliveries: BTreeMap::new(),
            records: vec![],
            express_sources: BTreeMap::new(),
        }
    }
}
pub(super) fn duration(payload: u64, overhead: u64, bitrate: u64) -> Result<u64> {
    if payload == 0 || bitrate == 0 {
        return Err(error("fragment length and bitrate must be positive"));
    }
    let n = u128::from(payload)
        .checked_add(u128::from(overhead))
        .and_then(|n| n.checked_mul(8))
        .and_then(|n| n.checked_mul(1_000_000_000_000))
        .ok_or_else(|| error("fragment duration overflow"))?;
    u64::try_from(n.div_ceil(u128::from(bitrate)))
        .map_err(|_| error("fragment duration exceeds u64"))
}
impl NetworkState {
    pub(super) fn preemption_direction(&self, direction: usize) -> bool {
        self.prepared
            .extensions
            .as_ref()
            .and_then(|e| e.preemption.as_ref())
            .is_some_and(|p| p.participating.get(direction) == Some(&true))
    }
    pub(super) fn preemption_mut<'a>(
        &self,
        d: &'a mut NetworkDelta,
    ) -> Result<&'a mut PreemptionState> {
        if d.preemption.is_none() {
            d.preemption = self.preemption.clone();
        }
        d.preemption
            .as_mut()
            .ok_or_else(|| error("missing preemption state"))
    }
    pub(super) fn preemption_port<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Port {
        &d.preemption
            .as_ref()
            .or(self.preemption.as_ref())
            .unwrap()
            .ports[i]
    }
    pub(super) fn preemptable(&self, t: &Transfer) -> bool {
        self.prepared
            .extensions
            .as_ref()
            .and_then(|e| e.preemption.as_ref())
            .is_some_and(|p| {
                p.participating[t.direction]
                    && p.outputs[t.direction].preemptable_mask & (1 << t.row.priority) != 0
            })
    }
    pub(super) fn preemption_created(&self, f: usize, d: &mut NetworkDelta) -> Result<()> {
        if self.preemption.is_none() {
            return Ok(());
        }
        let frame = &self.frame(d, f).row;
        let id = frame.frame_id.clone();
        let data = json!({"frame_id":id,"source_transfer_id":null,"first_sof_ps":null,"target_count":"0","targets":[],"completed_count":"0"});
        let state = self.preemption_mut(d)?;
        state.deliveries.insert(id.clone(), data.clone());
        state.records.push(("dir.preemption.delivery", id, data));
        Ok(())
    }
    pub(super) fn preemption_sof(&self, t: usize, d: &mut NetworkDelta, now: u64) -> Result<()> {
        let transfer = self.transfer(d, t).clone();
        if self.preemption.is_none() || transfer.row.parent_transfer_id.is_some() {
            return Ok(());
        }
        let frame = self.frame(d, transfer.frame).clone();
        let source = self.prepared.ethernet.directions[transfer.direction].source;
        let targets = self.ethernet_targets(
            source,
            frame.row.source_vlan_id.unwrap(),
            &transfer.row.wire,
        );
        let state = self.preemption_mut(d)?;
        let row = state
            .deliveries
            .get_mut(&frame.row.frame_id)
            .ok_or_else(|| error("missing delivery origin"))?;
        if row["first_sof_ps"].is_null() {
            row["first_sof_ps"] = json!(now.to_string());
            row["source_transfer_id"] = json!(transfer.row.transfer_id);
            row["target_count"] = json!(targets.len().to_string());
            row["targets"]=json!(targets.into_iter().map(|id|json!({"terminal_id":id,"completed_ps":null,"completion_reception_id":null})).collect::<Vec<_>>());
            state
                .records
                .push(("dir.preemption.delivery", frame.row.frame_id, row.clone()));
        }
        Ok(())
    }
    pub(super) fn preemption_received(
        &self,
        r: &Reception,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        if self.preemption.is_none() {
            return Ok(());
        }
        let state = self.preemption_mut(d)?;
        let row = state
            .deliveries
            .get_mut(&r.row.frame_id)
            .ok_or_else(|| error("unknown delivery frame"))?;
        let mut completed = false;
        for target in row["targets"].as_array_mut().unwrap() {
            if target["terminal_id"] == r.row.device && target["completed_ps"].is_null() {
                target["completed_ps"] = json!(now.to_string());
                target["completion_reception_id"] = json!(r.row.reception_id);
                completed = true;
            }
        }
        if completed {
            let count = row["targets"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|t| t["completed_ps"].is_string())
                .count();
            row["completed_count"] = json!(count.to_string());
            state.records.push((
                "dir.preemption.delivery",
                r.row.frame_id.clone(),
                row.clone(),
            ));
        }
        Ok(())
    }
    pub(super) fn preemption_input(&self, direction: usize, d: &NetworkDelta) -> Result<PortInput> {
        let q = self.queue(d, direction);
        let port = self.preemption_port(d, direction);
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .preemption
            .as_ref()
            .unwrap()
            .outputs[direction];
        let link = &self.prepared.ethernet.directions[direction];
        let mut input = PortInput {
            policy_epoch: self.epoch_after(d),
            ..PortInput::default()
        };
        for p in 0..8 {
            input.backlog[p] = !q.waiting[p].is_empty();
            let candidate = if config.preemptable_mask & (1 << p) != 0 {
                if let Some(t) = port.suspended {
                    let t = self.transfer(d, t);
                    if t.row.priority as usize == p {
                        Some(t)
                    } else {
                        None
                    }
                } else {
                    q.waiting[p].front().map(|&t| self.transfer(d, t))
                }
            } else {
                q.waiting[p].front().map(|&t| self.transfer(d, t))
            };
            if let Some(t) = candidate {
                input.backlog[p] = true;
                let occupancy = if self.preemptable(t) {
                    let completed = t.fragments.as_ref().unwrap().completed;
                    duration(
                        config.fragment_payload_bytes.min(
                            t.row
                                .wire
                                .mac_bytes
                                .checked_sub(completed)
                                .ok_or_else(|| error("fragment offset beyond MAC"))?,
                        ),
                        config.fragment_overhead_bytes,
                        link.bitrate_bps,
                    )?
                } else {
                    l2::timing(0, t.row.wire.mac_bytes, link.bitrate_bps, 0)?.1
                };
                if port.gap_due.is_none() {
                    input.heads[p] = Some(Head {
                        occupancy_ps: occupancy,
                    });
                }
            }
        }
        if let (Some(t), Some(end)) = (port.active.as_ref().and_then(Work::data), port.active_end) {
            input.sending = Some(Sending {
                priority: self.transfer(d, t).row.priority,
                release_ps: end,
            });
        }
        if let (Some(Work::Wire(id)), Some(end)) = (&port.active, port.active_end) {
            if !self.wire_host(d).unwrap().tx[id].is_mac_control() {
                input.sending = Some(Sending {
                    priority: self.wire_priority(direction, id, d),
                    release_ps: end,
                });
            }
        }
        Ok(input)
    }
    /// Explicit source-event retirement, including down/up at the same cut.
    /// The caller invokes this for each down control before final policy projection.
    pub(super) fn preemption_link_down(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if !self.preemption_direction(direction) {
            return Ok(());
        }
        let port = self.preemption_port(d, direction).clone();
        crate::allocation::reservation_checkpoint("network_preemption_link_reset")?;
        let name = &self.prepared.ethernet.directions[direction].from_port;
        // Settle the old occupation through the actual reset cut before removing it.
        let input = self.port_input(direction, d)?;
        let delta = self.tsn.as_ref().unwrap().plan_port_clock_in(
            std::mem::take(&mut d.tsn),
            name,
            c.now(),
            &input,
            self.clock_for_port(d, name),
        )?;
        self.stage_tsn(d, delta, &[name.clone()])?;
        if let Some(Work::Wire(id)) = &port.active {
            self.wire_owner_reset(direction, id, "link_epoch_reset", d, c)?;
        }
        let mut victims = std::collections::BTreeSet::new();
        victims.extend(port.active.as_ref().and_then(Work::data));
        victims.extend(port.suspended);
        // Already released frames can still have a pending receiver callback.
        for (i, transfer) in self
            .transfers
            .iter()
            .enumerate()
            .map(|(i, _)| (i, self.transfer(d, i)))
            .chain(
                d.transfers
                    .iter()
                    .filter(|(i, _)| *i >= self.transfers.len())
                    .map(|(i, t)| (*i, t)),
            )
        {
            if transfer.direction == direction
                && transfer.fragments.is_some()
                && transfer.row.eof_ps.is_some()
                && transfer.row.arrival_ps.is_none()
                && transfer.row.status != "dropped"
            {
                victims.insert(i);
            }
        }
        for t in &victims {
            let mut transfer = self.transfer(d, *t).clone();
            let fragmented = self.preemptable(&transfer);
            let f = transfer.fragments.as_mut().unwrap();
            if let Some(start) = f.active_start.take() {
                let elapsed = c
                    .now()
                    .checked_sub(start)
                    .ok_or_else(|| error("reset before serializer start"))?;
                f.occupied = add(f.occupied, elapsed)?;
                if transfer.row.eof_ps.is_none() {
                    let octets = u128::from(elapsed)
                        * u128::from(self.prepared.ethernet.directions[direction].bitrate_bps)
                        / 8_000_000_000_000u128;
                    let overhead = if fragmented {
                        self.prepared
                            .extensions
                            .as_ref()
                            .unwrap()
                            .preemption
                            .as_ref()
                            .unwrap()
                            .outputs[direction]
                            .fragment_overhead_bytes
                    } else {
                        8
                    }; // Whole-frame serializer preamble/SFD precedes MAC bytes.
                    f.partial_mac_bytes =
                        u64::try_from(octets.saturating_sub(u128::from(overhead)))
                            .unwrap_or(u64::MAX)
                            .min(transfer.row.wire.mac_bytes.saturating_sub(f.completed));
                    if fragmented {
                        if let Some(id) = f.fragments.last() {
                            let state = self.preemption_mut(d)?;
                            let row = state
                                .fragments
                                .get_mut(id)
                                .ok_or_else(|| error("reset fragment proof missing"))?;
                            let payload = row["payload_bytes"]
                                .as_str()
                                .unwrap()
                                .parse::<u64>()
                                .map_err(|_| error("fragment payload"))?;
                            f.partial_mac_bytes = f.partial_mac_bytes.min(payload);
                            row["end_ps"] = json!(c.now().to_string());
                            row["status"] = json!("aborted");
                            row["emitted_payload_bytes"] = json!(f.partial_mac_bytes.to_string());
                            state.records.push((
                                "dir.preemption.fragment",
                                id.clone(),
                                row.clone(),
                            ));
                        }
                    }
                }
            }
            f.active_end = None;
            f.reset_ps = Some(c.now());
            transfer.row.time_ps = c.now();
            if transfer.row.arrival_ps.is_none() {
                transfer.row.status = "dropped".into();
                transfer.row.drop_reason = Some("link_epoch_reset".into());
            }
            put(&mut d.transfers, *t, transfer);
        }
        if let Some(id) = &port.gap_id {
            let state = self.preemption_mut(d)?;
            let row = state
                .gaps
                .get_mut(id)
                .ok_or_else(|| error("reset gap proof missing"))?;
            row["end_ps"] = json!(c.now().to_string());
            row["status"] = json!("aborted");
            state
                .records
                .push(("dir.preemption.gap", id.clone(), row.clone()));
        }
        self.due_mut(d).retain(|due| match &due.kind {
            Completion::Eof(t) | Completion::Release(t) => !victims.contains(t),
            Completion::Fragment { direction: i, .. } | Completion::Gap { direction: i, .. } => {
                *i != direction
            }
            Completion::Wire(
                wire::HostEvent::Eof { direction: i, id }
                | wire::HostEvent::Release { direction: i, id },
            ) => !(*i == direction && port.active.as_ref() == Some(&Work::Wire(id.clone()))),
            _ => true,
        });
        let p = &mut self.preemption_mut(d)?.ports[direction];
        p.active = None;
        p.active_end = None;
        p.suspended = None;
        p.after_express = false;
        p.tx_epoch = add(p.tx_epoch, 1)?;
        p.gap_epoch = add(p.gap_epoch, 1)?;
        p.wake_epoch = add(p.wake_epoch, 1)?;
        p.gap_due = None;
        p.gap_id = None;
        p.wake_due = None;
        // Retire the matching common TSN reservation too. The normal post-control
        // refresh may choose the same timestamp, but must install it as a new wake.
        if let Some(i) = self
            .prepared
            .tsn
            .as_ref()
            .and_then(|tsn| tsn.port_index.get(name))
        {
            d.tsn_wakes.insert(*i, None);
        }
        self.queue_mut(d, direction).active = None;
        crate::allocation::reservation_checkpoint("network_preemption_reset_retirement")?;
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn sync_preemption_wire_links(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        if self.preemption.is_none() {
            return Ok(());
        }
        for i in 0..self.prepared.ethernet.directions.len() {
            let p = self.preemption_port(d, i);
            if (p.active.is_some() || p.suspended.is_some() || p.gap_due.is_some())
                && !self
                    .policy(d)
                    .and_then(|p| {
                        p.link_up
                            .get(&self.prepared.ethernet.directions[i].from_port)
                            .copied()
                    })
                    .unwrap_or(true)
            {
                self.preemption_link_down(i, d, c)?;
            }
        }
        Ok(())
    }
    pub(super) fn preemption_arbitrate(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let link = &self.prepared.ethernet.directions[direction];
        if !self
            .policy(d)
            .and_then(|p| p.link_up.get(&link.from_port).copied())
            .unwrap_or(true)
        {
            self.wire_prune_candidates(direction, d, c)?;
            return self.preemption_decision(direction, d, c);
        }
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .preemption
            .as_ref()
            .unwrap()
            .outputs[direction];
        let input = self.port_input(direction, d)?;
        let tsn = self
            .tsn
            .as_ref()
            .ok_or_else(|| error("preemption requires TSN"))?;
        let clock = self.clock_for_port(d, &link.from_port).cloned();
        let delta = tsn.plan_port_clock_in(
            std::mem::take(&mut d.tsn),
            &link.from_port,
            c.now(),
            &input,
            clock.as_ref(),
        )?;
        let ready = delta.ready.get(&link.from_port).copied().unwrap_or(0);
        self.stage_tsn(d, delta, &[link.from_port.clone()])?;
        let port = self.preemption_port(d, direction).clone();
        if port.active.is_some() || port.gap_due.is_some() {
            return self.preemption_decision(direction, d, c);
        }
        self.wire_prune_candidates(direction, d, c)?;
        if let Some(id) = self.wire_candidate(direction, d, c.now())? {
            self.preemption_wire_start(direction, &id, d, c)?;
            return self.preemption_decision(direction, d, c);
        }
        let allowed = self.output_request_allowed(direction, false, d, c.now());
        let mut express = None;
        for p in (0..8)
            .rev()
            .filter(|_| allowed)
            .filter(|p| ready & config.express_mask & (1 << p) != 0)
        {
            if let Some(&t) = self.queue(d, direction).waiting[p].front() {
                let occupancy = l2::timing(
                    0,
                    self.transfer(d, t).row.wire.mac_bytes,
                    link.bitrate_bps,
                    0,
                )?
                .1;
                if tsn
                    .retained_start_permission_clock_in(
                        &d.tsn,
                        &link.from_port,
                        p as u8,
                        c.now(),
                        occupancy,
                        clock.as_ref(),
                    )?
                    .is_some()
                {
                    express = Some(p);
                    break;
                }
            }
        }
        if express.is_none() && port.suspended.is_some() && port.after_express {
            let t = port.suspended.unwrap();
            let mut row = self.transfer(d, t).clone();
            let f = row.fragments.as_mut().unwrap();
            let id = identifier(&[&row.row.transfer_id, "gap", &f.gaps.len().to_string()]);
            let end = add(c.now(), config.resume_gap_ps)?;
            f.gaps.push(id.clone());
            put(&mut d.transfers, t, row.clone());
            let state = self.preemption_mut(d)?;
            let p = &mut state.ports[direction];
            p.gap_epoch = add(p.gap_epoch, 1)?;
            p.after_express = false;
            let data = json!({"gap_id":id,"transfer_id":row.row.transfer_id,"port":link.from_port,"start_ps":c.now().to_string(),"planned_end_ps":end.to_string(),"end_ps":if end==c.now(){opt(Some(end))}else{Value::Null},"gap_epoch":p.gap_epoch.to_string(),"status":if end==c.now(){"completed"}else{"pending"}});
            state.gaps.insert(id.clone(), data.clone());
            state.records.push(("dir.preemption.gap", id.clone(), data));
            if end > c.now() {
                p.gap_due = Some(end);
                p.gap_id = Some(id.clone());
                let epoch = p.gap_epoch;
                self.due_mut(d).push(Due {
                    at: end,
                    kind: Completion::Gap {
                        direction,
                        epoch,
                        id,
                    },
                });
                return self.preemption_decision(direction, d, c);
            }
        }
        let port = self.preemption_port(d, direction).clone();
        let selected = if let Some(p) = express {
            self.queue(d, direction).waiting[p].front().copied()
        } else if let Some(t) = port.suspended {
            (ready & (1 << self.transfer(d, t).row.priority) != 0).then_some(t)
        } else if allowed {
            (0..8)
                .rev()
                .find(|p| ready & config.preemptable_mask & (1 << p) != 0)
                .and_then(|p| self.queue(d, direction).waiting[p].front().copied())
        } else {
            None
        };
        if let Some(t) = selected {
            self.preemption_start(t, d, c)?;
        }
        self.preemption_decision(direction, d, c)
    }
    fn preemption_wire_start(
        &self,
        direction: usize,
        id: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let link = &self.prepared.ethernet.directions[direction];
        let tx = &self.wire_host(d).unwrap().tx[id];
        let mac_control = tx.is_mac_control();
        let priority = self.wire_priority(direction, id, d);
        let (eof, release, arrival) = l2::timing(
            c.now(),
            tx.intent.bytes.len() as u64,
            link.bitrate_bps,
            self.physical_receive_delay(direction)?,
        )?;
        self.wire_claim(direction, id, eof, release, Some(arrival), d, c)?;
        let p = &mut self.preemption_mut(d)?.ports[direction];
        p.tx_epoch = add(p.tx_epoch, 1)?;
        p.active = Some(Work::Wire(id.to_owned()));
        p.active_end = Some(release);
        if p.suspended.is_some() {
            p.after_express = true
        }
        self.due_mut(d).extend([
            Due {
                at: eof,
                kind: Completion::Wire(wire::HostEvent::Eof {
                    direction,
                    id: id.to_owned(),
                }),
            },
            Due {
                at: release,
                kind: Completion::Wire(wire::HostEvent::Release {
                    direction,
                    id: id.to_owned(),
                }),
            },
        ]);
        let post = self.port_input(direction, d)?;
        let tsn = self.tsn.as_ref().unwrap();
        let clock = self.clock_for_port(d, &link.from_port).cloned();
        let delta = if mac_control {
            tsn.plan_port_clock_in(
                std::mem::take(&mut d.tsn),
                &link.from_port,
                c.now(),
                &post,
                clock.as_ref(),
            )?
        } else {
            tsn.plan_start_duration_clock_in(
                std::mem::take(&mut d.tsn),
                &link.from_port,
                priority,
                c.now(),
                eof - c.now(),
                release - c.now(),
                &post,
                clock.as_ref(),
            )?
            .0
        };
        self.stage_tsn(d, delta, &[link.from_port.clone()])
    }
    pub(super) fn preemption_wire_release(
        &self,
        direction: usize,
        id: &str,
        d: &mut NetworkDelta,
    ) -> Result<()> {
        let p = &mut self.preemption_mut(d)?.ports[direction];
        if p.active.as_ref() != Some(&Work::Wire(id.to_owned())) {
            return Err(error("abstract Wire release owner differs"));
        }
        p.active = None;
        p.active_end = None;
        p.tx_epoch = add(p.tx_epoch, 1)?;
        Ok(())
    }
    fn preemption_start(&self, t: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut transfer = self.transfer(d, t).clone();
        if self.gptp_packet(d, t) {
            transfer
                .fragments
                .get_or_insert_with(TransferFragments::default);
        }
        let direction = transfer.direction;
        let link = &self.prepared.ethernet.directions[direction];
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .preemption
            .as_ref()
            .unwrap()
            .outputs[direction];
        let preemptable = self.preemptable(&transfer);
        let first = transfer.row.sof_ps.is_none();
        let p = transfer.row.priority as usize;
        if first {
            let q = self.queue_mut(d, direction);
            if q.waiting[p].pop_front() != Some(t) {
                return Err(error("fragment dequeue not FIFO"));
            }
            q.bytes[p] = q.bytes[p]
                .checked_sub(transfer.row.wire.mac_bytes)
                .ok_or_else(|| error("queue bytes underflow"))?;
            transfer.row.sof_ps = Some(c.now());
            transfer.sof_epoch = Some(self.epoch_after(d));
        }
        let (eof, release, arrival) = if preemptable {
            let fragments = transfer.fragments.as_mut().unwrap();
            let remaining = transfer
                .row
                .wire
                .mac_bytes
                .checked_sub(fragments.completed)
                .ok_or_else(|| error("fragment bytes underflow"))?;
            let len = remaining.min(config.fragment_payload_bytes);
            let end = add(
                c.now(),
                duration(len, config.fragment_overhead_bytes, link.bitrate_bps)?,
            )?;
            let id = identifier(&[
                &transfer.row.transfer_id,
                "fragment",
                &fragments.fragments.len().to_string(),
            ]);
            let state = self.preemption_mut(d)?;
            let port = &mut state.ports[direction];
            port.tx_epoch = add(port.tx_epoch, 1)?;
            port.active = Some(Work::Data(t));
            port.active_end = Some(end);
            port.suspended = None;
            port.after_express = false;
            let epoch = port.tx_epoch;
            let data = json!({"fragment_id":id,"transfer_id":transfer.row.transfer_id,"frame_id":transfer.row.frame_id,"port":link.from_port,"ordinal":fragments.fragments.len().to_string(),"offset_bytes":fragments.completed.to_string(),"payload_bytes":len.to_string(),"overhead_bytes":config.fragment_overhead_bytes.to_string(),"bitrate_bps":link.bitrate_bps.to_string(),"start_ps":c.now().to_string(),"planned_end_ps":end.to_string(),"end_ps":null,"tx_epoch":epoch.to_string(),"status":"in_flight"});
            fragments.fragments.push(id.clone());
            fragments.active_start = Some(c.now());
            fragments.active_end = Some(end);
            state.fragments.insert(id.clone(), data.clone());
            state
                .records
                .push(("dir.preemption.fragment", id.clone(), data));
            self.due_mut(d).push(Due {
                at: end,
                kind: Completion::Fragment {
                    direction,
                    transfer: t,
                    epoch,
                    id,
                    payload: len,
                },
            });
            if len == remaining {
                transfer.row.planned_eof_ps = Some(end);
                transfer.row.planned_release_ps = Some(end);
                transfer.row.planned_arrival_ps =
                    Some(add(end, self.physical_receive_delay(direction)?)?);
            }
            (end, end, add(end, self.physical_receive_delay(direction)?)?)
        } else {
            let (eof, release, arrival) = l2::timing(
                c.now(),
                transfer.row.wire.mac_bytes,
                link.bitrate_bps,
                self.physical_receive_delay(direction)?,
            )?;
            transfer.row.planned_eof_ps = Some(eof);
            transfer.row.planned_release_ps = Some(release);
            transfer.row.planned_arrival_ps = Some(arrival);
            let fragments = transfer.fragments.as_mut().unwrap();
            fragments.active_start = Some(c.now());
            fragments.active_end = Some(release);
            let state = self.preemption_mut(d)?;
            let port = &mut state.ports[direction];
            port.tx_epoch = add(port.tx_epoch, 1)?;
            port.active = Some(Work::Data(t));
            port.active_end = Some(release);
            if port.suspended.is_some() {
                port.after_express = true;
            }
            self.due_mut(d).extend([
                Due {
                    at: eof,
                    kind: Completion::Eof(t),
                },
                Due {
                    at: release,
                    kind: Completion::Release(t),
                },
            ]);
            (eof, release, arrival)
        };
        let _ = arrival;
        transfer.row.time_ps = c.now();
        transfer.row.status = "transmitting".into();
        put(&mut d.transfers, t, transfer);
        self.queue_mut(d, direction).active = Some(t);
        if self.gptp_packet(d, t) {
            if preemptable {
                return Err(error("gPTP requires abstract EXPRESS class"));
            }
            crate::allocation::reservation_checkpoint("preemption_gptp_source")?;
            let epoch = self.preemption_port(d, direction).tx_epoch;
            if self.preemption_mut(d)?.express_sources.len()
                >= self.prepared.gptp.as_ref().unwrap().max_journal_entries
            {
                return Err(error("abstract gPTP retained source capacity"));
            }
            let source = gptp::state::ExpressSourceEvidence {
                source: gptp::state::MacSource::AbstractExpress {
                    transfer: t,
                    direction,
                    epoch,
                },
                sof_ps: c.now(),
                eof_planned_ps: eof,
                release_planned_ps: release,
                arrival_planned_ps: arrival,
                eof_actual_ps: None,
                arrival_actual_ps: None,
                mac_hex: self.transfer(d, t).row.wire.mac_hex.clone(),
            };
            if self
                .preemption_mut(d)?
                .express_sources
                .insert(t, source)
                .is_some()
            {
                return Err(error("duplicate abstract gPTP source"));
            }
            self.gptp_tx_start(t, None, epoch, c.now(), d, c)?;
        } else {
            self.preemption_sof(t, d, c.now())?;
        }
        let post = self.port_input(direction, d)?;
        let tsn = self.tsn.as_ref().unwrap();
        let clock = self.clock_for_port(d, &link.from_port).cloned();
        let (delta, _, _) = tsn.plan_start_duration_clock_in(
            std::mem::take(&mut d.tsn),
            &link.from_port,
            p as u8,
            c.now(),
            eof - c.now(),
            release - c.now(),
            &post,
            clock.as_ref(),
        )?;
        self.stage_tsn(d, delta, &[link.from_port.clone()])?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Carries the complete typed fragment token into one atomic callback.
    pub(super) fn fragment_complete(
        &self,
        direction: usize,
        t: usize,
        epoch: u64,
        id: &str,
        payload: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let port = self.preemption_port(d, direction).clone();
        if port.tx_epoch != epoch || port.active != Some(Work::Data(t)) {
            return Ok(());
        }
        let mut transfer = self.transfer(d, t).clone();
        let f = transfer.fragments.as_mut().unwrap();
        let start = f
            .active_start
            .ok_or_else(|| error("missing active fragment start"))?;
        f.completed = add(f.completed, payload)?;
        f.occupied = add(
            f.occupied,
            c.now()
                .checked_sub(start)
                .ok_or_else(|| error("fragment end before start"))?,
        )?;
        f.active_start = None;
        f.active_end = None;
        let final_fragment = f.completed == transfer.row.wire.mac_bytes;
        let state = self.preemption_mut(d)?;
        let row = state
            .fragments
            .get_mut(id)
            .ok_or_else(|| error("missing fragment record"))?;
        row["end_ps"] = json!(c.now().to_string());
        row["status"] = json!("completed");
        state
            .records
            .push(("dir.preemption.fragment", id.into(), row.clone()));
        let port = &mut state.ports[direction];
        port.active = None;
        port.active_end = None;
        port.tx_epoch = add(port.tx_epoch, 1)?;
        transfer.row.time_ps = c.now();
        if final_fragment {
            transfer.row.eof_ps = Some(c.now());
            transfer.row.release_ps = Some(c.now());
            transfer.row.status = "serialized".into();
            let arrival = transfer
                .row
                .planned_arrival_ps
                .ok_or_else(|| error("missing final arrival"))?;
            crate::allocation::reservation_checkpoint("network_final_fragment_arrival")?;
            Self::schedule(c, d, arrival, Event::Arrival(t))?;
        } else {
            port.suspended = Some(t);
        }
        self.queue_mut(d, direction).active = None;
        put(&mut d.transfers, t, transfer);
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn gap_complete(
        &self,
        direction: usize,
        epoch: u64,
        id: &str,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let p = self.preemption_port(d, direction);
        if p.gap_epoch != epoch || p.gap_id.as_deref() != Some(id) {
            return Ok(());
        }
        let state = self.preemption_mut(d)?;
        let p = &mut state.ports[direction];
        p.gap_due = None;
        p.gap_id = None;
        p.gap_epoch = add(p.gap_epoch, 1)?;
        let row = state
            .gaps
            .get_mut(id)
            .ok_or_else(|| error("missing gap record"))?;
        row["end_ps"] = json!(c.now().to_string());
        row["status"] = json!("completed");
        state
            .records
            .push(("dir.preemption.gap", id.into(), row.clone()));
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn preemption_eof(&self, t: &mut Transfer) {
        if let Some(f) = t.fragments.as_mut() {
            f.completed = t.row.wire.mac_bytes;
        }
    }
    pub(super) fn preemption_release(
        &self,
        t: &mut Transfer,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        if let Some(f) = t.fragments.as_mut() {
            let start = f
                .active_start
                .take()
                .ok_or_else(|| error("missing express start"))?;
            f.occupied = add(
                f.occupied,
                now.checked_sub(start)
                    .ok_or_else(|| error("release before start"))?,
            )?;
            f.active_end = None;
            let state = self.preemption_mut(d)?;
            let p = &mut state.ports[t.direction];
            p.active = None;
            p.active_end = None;
            p.tx_epoch = add(p.tx_epoch, 1)?;
        }
        Ok(())
    }
    fn preemption_decision(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &Context<'_>,
    ) -> Result<()> {
        let p = self.preemption_port(d, direction).clone();
        let link = &self.prepared.ethernet.directions[direction];
        let active = p
            .active
            .as_ref()
            .and_then(Work::data)
            .map(|t| self.transfer(d, t));
        let suspended = p.suspended.map(|t| self.transfer(d, t));
        let state = if let Some(active) = active {
            if self.preemptable(active) {
                "PreemptableFragment"
            } else {
                "Express"
            }
        } else if matches!(p.active, Some(Work::Wire(_))) {
            "Express"
        } else if suspended.is_some() {
            "Suspended"
        } else {
            "Idle"
        };
        let remaining = suspended
            .or(active)
            .map(|t| {
                t.row
                    .wire
                    .mac_bytes
                    .saturating_sub(t.fragments.as_ref().unwrap().completed)
            })
            .unwrap_or(0);
        let offset = suspended
            .or(active)
            .map_or(0, |t| t.fragments.as_ref().unwrap().completed);
        let wake = self
            .prepared
            .tsn
            .as_ref()
            .and_then(|t| t.port_index.get(&link.from_port))
            .and_then(|i| d.tsn_wakes.get(i).unwrap_or(&self.tsn_wakes[*i]).as_ref())
            .map(|w| w.time_ps);
        let candidate = suspended.map(|t| t.row.priority).or_else(|| {
            (0..8)
                .rev()
                .find(|&p| !self.queue(d, direction).waiting[p].is_empty())
                .map(|p| p as u8)
        });
        let reason = if p.gap_due.is_some() {
            Some("gap")
        } else if p.active.is_some() {
            None
        } else {
            let observed = d.tsn.records.iter().rev().find_map(|r| {
                if let crate::snapshot::ethernet::tsn::TsnRecord::Decision(r) = r {
                    (r.port == link.from_port && r.priority == candidate)
                        .then_some(r.reason.as_str())
                } else {
                    None
                }
            });
            Some(match observed {
                Some("gate_closed" | "no_active_schedule") => "gate",
                Some("guard_blocked") => "guard",
                Some("credit_negative") => "credit",
                Some("never_eligible") => "never_eligible",
                _ => "empty",
            })
        };
        let data = json!({"port":link.from_port,"state":state,"wait_reason":reason,"wire_control_id":p.active.as_ref().and_then(|w|if let Work::Wire(id)=w{Some(id)}else{None}),"active_transfer_index":p.active.as_ref().and_then(Work::data).map(|t|t.to_string()),"active_transfer_id":active.map(|t|&t.row.transfer_id),"suspended_transfer_id":suspended.map(|t|&t.row.transfer_id),"offset_bytes":offset.to_string(),"remaining_bytes":remaining.to_string(),"next_wake_ps":opt(p.gap_due.or(wake)),"tx_epoch":p.tx_epoch.to_string(),"gap_epoch":p.gap_epoch.to_string(),"wake_epoch":p.wake_epoch.to_string()});
        let id = identifier(&[
            &link.from_port,
            &c.now().to_string(),
            &d.record_sequence.to_string(),
        ]);
        self.preemption_mut(d)?
            .records
            .push(("dir.preemption.decision", id, data));
        Ok(())
    }
    pub(super) fn emit_preemption(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if let Some(state) = d.preemption.as_mut() {
            for (schema, id, data) in std::mem::take(&mut state.records) {
                Self::record(c, d, schema, id, data)?;
            }
        }
        if let Some(state) = d.preemption.as_mut() {
            state.fragments.retain(|_, r| r["end_ps"].is_null());
            state.gaps.retain(|_, r| r["end_ps"].is_null());
        }
        Ok(())
    }
    pub(super) fn preemption_transfer(&self, t: &Transfer, h: u64) -> Value {
        let row = &t.row;
        let f = t.fragments.as_ref().unwrap();
        let prefix = f
            .active_start
            .zip(f.active_end)
            .map_or(0, |(start, end)| h.min(end).saturating_sub(start));
        json!({"transfer_id":row.transfer_id,"frame_id":row.frame_id,"parent_transfer_id":row.parent_transfer_id,"queue_id":row.queue_id,"priority":row.priority.to_string(),"wire":wire(&row.wire),"vlan_id":opt(row.vlan_id.map(u64::from)),"from_port":row.from_port,"to_port":row.to_port,"queued_ps":row.queued_ps.to_string(),"sof_ps":opt(row.sof_ps),"eof_ps":opt(row.eof_ps),"release_ps":opt(row.release_ps),"arrival_ps":opt(row.arrival_ps),"planned_eof_ps":opt(row.planned_eof_ps),"planned_release_ps":opt(row.planned_release_ps),"planned_arrival_ps":opt(row.planned_arrival_ps),"status":if row.status=="queued"{"queued"}else if row.status=="dropped"{"dropped"}else if row.arrival_ps.is_some(){"completed"}else{"in_flight"},"drop_reason":row.drop_reason,"kind":if self.preemptable(t){"preemptable"}else{"express"},"visit_id":t.visit.to_string(),"copy_id":row.transfer_id,"policy_epoch_offer":t.offer_epoch.to_string(),"policy_epoch_sof":opt(t.sof_epoch),"mac_bytes":row.wire.mac_bytes.to_string(),"completed_mac_bytes":f.completed.to_string(),"partial_mac_bytes":f.partial_mac_bytes.to_string(),"reset_ps":opt(f.reset_ps),"fragment_ids":f.fragments,"gap_ids":f.gaps,"occupied_ps":(u128::from(f.occupied)+u128::from(prefix)).to_string()})
    }
    pub(super) fn preemption_snapshot(&self) -> Option<Value> {
        self.preemption.as_ref().map(|state|json!({"ports":state.ports.iter().enumerate().map(|(i,p)|{
            let transfer=p.active.as_ref().and_then(Work::data).or(p.suspended).map(|i|&self.transfers[i]);
            let completed=transfer.map_or(0,|t|t.fragments.as_ref().unwrap().completed);
            json!({"port":self.prepared.ethernet.directions[i].from_port,"state":if let Some(t)=p.active.as_ref().and_then(Work::data) {if self.preemptable(&self.transfers[t]){"PreemptableFragment"}else{"Express"}}else if matches!(p.active,Some(Work::Wire(_))){"Express"}else if p.suspended.is_some(){"Suspended"}else{"Idle"},"active":p.active.as_ref().map(|w|match w{Work::Data(i)=>self.transfers[*i].row.transfer_id.clone(),Work::Wire(id)=>id.clone()}),"active_end":opt(p.active_end),"suspended":p.suspended.map(|i|&self.transfers[i].row.transfer_id),"frame":transfer.map(|t|&t.row.frame_id),"next_offset":completed.to_string(),"remaining":transfer.map_or(0,|t|t.row.wire.mac_bytes.saturating_sub(completed)).to_string(),"after_express":p.after_express,"tx_epoch":p.tx_epoch.to_string(),"gap_epoch":p.gap_epoch.to_string(),"wake_epoch":p.wake_epoch.to_string(),"gap_due":opt(p.gap_due),"wake_due":opt(p.wake_due),"fragment_refs":transfer.map(|t|&t.fragments.as_ref().unwrap().fragments),"gap_refs":transfer.map(|t|&t.fragments.as_ref().unwrap().gaps)})
        }).collect::<Vec<_>>()}))
    }
    /// Read-only prefix projection. Core must apply this after collecting model records.
    pub(crate) fn project_prefix_records(
        &self,
        h: u64,
        records: &mut BTreeMap<String, ModelRecord>,
    ) {
        if self.preemption.is_none() {
            return;
        }
        for t in &self.transfers {
            if let Some(record) =
                records.get_mut(&format!("dir.preemption.transfer:{}", t.row.transfer_id))
            {
                let seq = record.data["effect_seq"].clone();
                record.data = self.preemption_transfer(t, h);
                record.data["effect_seq"] = seq;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_fragment_arithmetic() {
        assert_eq!(duration(40, 4, 1_000_000_000).unwrap(), 352000);
        assert_eq!(duration(20, 4, 1_000_000_000).unwrap(), 192000);
        assert_eq!(duration(1, 0, 3).unwrap(), 2_666_666_666_667);
        assert!(duration(0, 0, 1).is_err());
        assert!(duration(u64::MAX, u64::MAX, 1).is_err());
    }
}
#[cfg(test)]
mod atomic_tests {
    #[test]
    fn final_fragment_reservation_failure_preserves_uncompleted_prefix() {
        let mut p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-extensions/preemption.ini"),
        )
        .unwrap();
        p.common.time_limit_ps = 1_576_000;
        let baseline = crate::runtime::simulate(&p).unwrap();
        p.common.time_limit_ps = 2_000_000;
        crate::allocation::fail_next_reservation("network_final_fragment_arrival");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        assert_eq!(failed.common.end_ps, 1_384_000);
        assert_eq!(failed.common.diagnostics[0].time_ps, Some(1_576_000));
        let b = &baseline.registered.as_ref().unwrap().model_records;
        let f = &failed.registered.as_ref().unwrap().model_records;
        for (key, row) in b
            .iter()
            .filter(|(_, r)| r.schema.name.starts_with("dir.preemption."))
        {
            let mut expected = row.data.clone();
            let mut actual = f[key].data.clone();
            if row.schema.name == "dir.preemption.transfer" {
                expected.as_object_mut().unwrap().remove("occupied_ps");
                actual.as_object_mut().unwrap().remove("occupied_ps");
            }
            assert_eq!(actual, expected, "{key}");
        }
        assert_eq!(
            failed.network.as_ref().unwrap()["preemption"],
            baseline.network.as_ref().unwrap()["preemption"]
        );
    }
}
#[cfg(test)]
mod epoch_tests {
    use super::*;
    #[test]
    fn obsolete_fragment_and_gap_tokens_do_not_touch_state_records_or_metrics() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-extensions/preemption.ini"),
        )
        .unwrap();
        let registered = p.registered.as_ref().unwrap();
        let mut state = NetworkState::new(registered.network.clone().unwrap()).unwrap();
        state.preemption.as_mut().unwrap().ports[0].tx_epoch = 8;
        state.preemption.as_mut().unwrap().ports[0].gap_epoch = 4;
        state.preemption.as_mut().unwrap().ports[0].wake_epoch = 13;
        let mut d = state.delta();
        let mut batch = crate::registry::EffectBatch::default();
        let mut next = 0;
        let channels = BTreeMap::new();
        let live = std::collections::BTreeSet::new();
        let mut c = Context {
            now: 100,
            config: &registered.models[0],
            registry: &registered.registry,
            profile: registered.registry.profile(&p.common.profile).unwrap(),
            channels: &channels,
            channel_configs: &[],
            live_timers: &live,
            executing_timer: None,
            next_id: &mut next,
            batch: &mut batch,
        };
        state
            .fragment_complete(0, usize::MAX, 7, "invalid-old-id", 40, &mut d, &mut c)
            .unwrap();
        state
            .gap_complete(0, 3, "invalid-old-id", &mut d, &mut c)
            .unwrap();
        assert!(d.preemption.is_none());
        assert!(d.transfers.is_empty());
        assert!(d.points.is_empty());
        assert!(c.batch.effects.is_empty());
        assert_eq!(d.record_sequence, state.record_sequence);
    }
}

#[cfg(test)]
mod reset_atomic_tests {
    use super::*;
    #[test]
    fn partial_reset_failure_preserves_committed_serializer_credit_and_fes_then_retries() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/wire-serializers/abstract.ini"),
        )
        .unwrap();
        let mut engine = crate::runtime::registered::Engine::new(Arc::new(p.clone())).unwrap();
        engine.initialize().unwrap();
        let mut checked = false;
        while !checked {
            assert!(engine.step());
            let now = engine.snapshot.common.last_event_time_ps.unwrap_or(0);
            let keys = engine
                .checkpoint_work()
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>();
            let live = engine.live.clone();
            let sequence = engine.sequence;
            checked = engine
                .with_network_state_for_test(|state| {
                    let i = state
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|p| p.from_port == "Net.s1.tx_b")
                        .unwrap();
                    let port = &state.preemption.as_ref().unwrap().ports[i];
                    let Some(Work::Data(t)) = port.active.as_ref() else {
                        return false;
                    };
                    if port.active_end.is_none_or(|end| end <= now + 1) {
                        return false;
                    }
                    let t = *t;
                    let before = state.checkpoint();
                    let mut batch = crate::registry::EffectBatch::default();
                    let mut next = 0;
                    let channels = BTreeMap::new();
                    let mut c = Context {
                        now: now + 1,
                        config: &engine.registered.models[0],
                        registry: &engine.registered.registry,
                        profile: engine
                            .registered
                            .registry
                            .profile(&p.common.profile)
                            .unwrap(),
                        channels: &channels,
                        channel_configs: &[],
                        live_timers: &engine.live,
                        executing_timer: None,
                        next_id: &mut next,
                        batch: &mut batch,
                    };
                    let mut d = state.delta();
                    crate::allocation::fail_next_reservation("network_preemption_reset_retirement");
                    let error = state.preemption_link_down(i, &mut d, &mut c).unwrap_err();
                    assert_eq!(error.reason, "allocation_failed");
                    assert_eq!(state.transfer(&d, t).row.status, "dropped");
                    assert!(!d.tsn.records.is_empty());
                    assert_eq!(state.checkpoint(), before);
                    let mut retry = state.delta();
                    state.preemption_link_down(i, &mut retry, &mut c).unwrap();
                    assert_eq!(state.transfer(&retry, t).row.status, "dropped");
                    assert!(state.preemption_port(&retry, i).active.is_none());
                    assert_eq!(state.checkpoint(), before);
                    true
                })
                .unwrap_or(false);
            assert_eq!(engine.sequence, sequence);
            assert_eq!(engine.live, live);
            assert_eq!(
                engine
                    .checkpoint_work()
                    .iter()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>(),
                keys
            );
        }
        while engine.step() {}
        assert_eq!(engine.snapshot.common.termination, "time_limit");
    }
}

#[cfg(test)]
mod reset_native_tests {
    use super::*;
    #[test]
    fn configured_delay_abstract_owns_native_reservations_before_and_after_reset() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let src = repo.join("examples/network-composite/wire-serializers");
        for (cut, express) in [
            (5_670_000u64, false),
            (7_914_001, false),
            (7_914_001, true),
            (8_240_001, true),
        ] {
            let root = std::env::temp_dir().join(format!(
                "wire-native-reset-{}-{}-{cut}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let mut m: Value =
                serde_json::from_slice(&std::fs::read(src.join("abstract-model.json")).unwrap())
                    .unwrap();
            let mut w: Value =
                serde_json::from_slice(&std::fs::read(src.join("workload.json")).unwrap()).unwrap();
            if express {
                w["generators"][0]["priority"] = json!(7);
                w["generators"][0]["frame"]["tag"]["pcp"] = json!(7);
            }
            m["configured_media"] = json!({"seed":"7","physical_links":[{"id":"delayed_rx","a":"Net.b.tx","b":"Net.s1.tx_b","phy_mode":"1000base-t1","duplex":"full","a_phy":{"role":"master","tx_latency_ps":"0","rx_latency_ps":"500000"},"b_phy":{"role":"slave","tx_latency_ps":"0","rx_latency_ps":"0"}}]});
            w["controls"] = json!([{"id":"down","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":false},{"id":"up","at_ps":cut.to_string(),"kind":"link_set","link":"B","up":true}]);
            std::fs::write(root.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
            std::fs::write(root.join("workload.json"), serde_json::to_vec(&w).unwrap()).unwrap();
            std::fs::write(root.join("model.ini"),format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 20000000ps\n",src.join("models").display())).unwrap();
            let p = crate::prepare(&root.join("model.ini")).unwrap();
            let mut engine = crate::runtime::registered::Engine::new(Arc::new(p.clone())).unwrap();
            engine.initialize().unwrap();
            let mut observations = std::collections::BTreeSet::new();
            while engine.step() {
                let now = engine.snapshot.common.last_event_time_ps.unwrap_or(0);
                let work = engine.checkpoint_work();
                engine.with_network_state_for_test(|state| {
                    let i = state
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|p| p.from_port == "Net.s1.tx_b")
                        .unwrap();
                    let port = &state.preemption.as_ref().unwrap().ports[i];
                    let reset = state
                        .transfers
                        .iter()
                        .any(|t| t.fragments.as_ref().is_some_and(|f| f.reset_ps.is_some()));
                    let tag = match &port.active {
                        Some(Work::Data(_)) => "active_data",
                        Some(Work::Wire(_)) => "active_wire",
                        None if reset => "retired",
                        _ => return,
                    };
                    if !observations.insert(tag) {
                        return;
                    }
                    let restored = NetworkState::restore_checkpoint(
                        state.prepared.clone(),
                        &state.checkpoint(),
                        now,
                    )
                    .unwrap_or_else(|e| panic!("cut={cut} now={now} {tag}: {e:?}"));
                    restored
                        .validate_checkpoint_reservations(
                            &work,
                            &engine.live,
                            p.common.time_limit_ps,
                        )
                        .unwrap_or_else(|e| {
                            panic!("cut={cut} now={now} {tag} reservations: {e:?}")
                        });
                });
            }
            assert_eq!(engine.snapshot.common.termination, "time_limit");
            assert_eq!(
                observations,
                std::collections::BTreeSet::from(["active_data", "active_wire", "retired"])
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(test)]
mod same_future_wake_tests {
    use super::*;

    #[test]
    fn source_down_up_replaces_same_10us_wake_and_starts_queued_frame() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/network-composite/wire-serializers");
        let root = std::env::temp_dir().join(format!(
            "abstract-same-wake-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut m: Value =
            serde_json::from_slice(&std::fs::read(src.join("abstract-model.json")).unwrap())
                .unwrap();
        let mut w: Value =
            serde_json::from_slice(&std::fs::read(src.join("workload.json")).unwrap()).unwrap();
        m["base"]["config"]["tsn"]["outputs"][2]["tas"] = json!({
            "id":"closed_until_10us","base_time_ps":"0","cycle_time_ps":"20000000",
            "entries":[{"duration_ps":"10000000","open_priorities":[]},
                       {"duration_ps":"10000000","open_priorities":[0,1,2,3,4,5,6,7]}]
        });
        w["generators"][0]["node"] = json!("Net.b");
        w["generators"][0]["times_ps"] = json!(["0"]);
        w["generators"][0]["frame"]["dst_mac"] = json!("02:00:00:00:00:01");
        w["generators"][0]["frame"]["data"] = json!("");
        w["controls"] = json!([
            {"id":"down","at_ps":"1000000","kind":"link_set","link":"B","up":false},
            {"id":"up","at_ps":"1000000","kind":"link_set","link":"B","up":true}
        ]);
        std::fs::write(root.join("model.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        std::fs::write(root.join("workload.json"), serde_json::to_vec(&w).unwrap()).unwrap();
        std::fs::write(root.join("model.ini"), format!("[General]\nnetwork = ethdynamic.Net\nned-path = \"{}\"\nmodel-profile = \"ethernet.network.composite.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 20000000ps\n", src.join("models").display())).unwrap();
        let p = crate::prepare(&root.join("model.ini")).unwrap();
        let mut engine = crate::runtime::registered::Engine::new(Arc::new(p.clone())).unwrap();
        engine.initialize().unwrap();
        let mut old = None;
        let mut replacement = None;
        let mut started = false;
        let mut fes_reserved = false;
        while engine.step() {
            let now = engine.snapshot.common.last_event_time_ps.unwrap_or(0);
            let work = engine.checkpoint_work();
            engine.with_network_state_for_test(|state| {
                let direction = state.prepared.ethernet.directions.iter().position(|d| d.from_port == "Net.b.tx").unwrap();
                let tsn_index = state.prepared.tsn.as_ref().unwrap().port_index["Net.b.tx"];
                let port = &state.preemption.as_ref().unwrap().ports[direction];
                if now < 1_000_000 && !state.queues[direction].waiting[0].is_empty() {
                    let token = state.tsn_wakes[tsn_index].clone().unwrap();
                    assert_eq!(token.time_ps, 10_000_000);
                    assert_eq!(port.wake_due, Some(10_000_000));
                    old = Some((token, port.wake_epoch));
                }
                if now == 1_000_000 && replacement.is_none() {
                    let (old_token, old_epoch) = old.as_ref().expect("frame must queue before source down/up");
                    let token = state.tsn_wakes[tsn_index].clone().expect("replacement common wake missing");
                    assert_eq!(token.time_ps, old_token.time_ps);
                    assert_eq!(port.wake_due, Some(10_000_000));
                    assert!(port.wake_epoch > *old_epoch);
                    assert_ne!(token, *old_token);
                    assert!(!state.tsn.as_ref().unwrap().wake_valid(old_token));
                    assert!(state.tsn.as_ref().unwrap().wake_valid(&token));
                    assert!(!state.queues[direction].waiting[0].is_empty());
                    let before = state.checkpoint();
                    let mut restored = NetworkState::restore_checkpoint(state.prepared.clone(), &before, now).unwrap();
                    restored.validate_checkpoint_reservations(&work, &engine.live, p.common.time_limit_ps).unwrap();
                    let stale = restored.tsn.as_ref().unwrap().plan_wake(old_token, &restored.port_input(direction, &restored.delta()).unwrap()).unwrap();
                    restored.tsn.as_mut().unwrap().apply(stale);
                    assert_eq!(restored.checkpoint(), before, "old actual token must be inert");
                    // Logical wake is carried by the one shared, source-bound Control FES timer.
                    let (at, held) = state.control.as_ref().unwrap();
                    assert!(*at <= 10_000_000);
                    assert!(engine.live.contains(held));
                    eprintln!("actual same-T replacement at {now}: old={old_token:?} epoch={old_epoch}; new={token:?} epoch={}; current FES held={held:?} at={at}", port.wake_epoch);
                    replacement = Some(token);
                }
                if let Some((_, held)) = state.control.as_ref().filter(|(at, _)|
                    (1_000_000..10_000_000).contains(&now) && *at == 10_000_000)
                {
                    assert!(engine.live.contains(held));
                    let carried = state.tsn_wakes[tsn_index].as_ref().unwrap();
                    assert_eq!(carried.time_ps, 10_000_000);
                    assert!(carried.wake_generation >= replacement.as_ref().unwrap().wake_generation);
                    assert!(state.tsn.as_ref().unwrap().wake_valid(carried));
                    assert_eq!(work.iter().filter(|(key, work)| {
                        if let crate::runtime::registered::Work::Delivery { envelope, token } = work {
                            key.0 == 10_000_000 && token.as_ref() == Some(held)
                                && matches!(serde_json::from_slice::<Event>(&envelope.payload).unwrap(), Event::Control(g) if g == state.control_generation)
                        } else { false }
                    }).count(), 1);
                    if !fes_reserved {
                        eprintln!("actual replacement wake carried by live FES at 10000000: now={now} token={carried:?} held={held:?}");
                    }
                    fes_reserved = true;
                }
                if let Some(transfer) = state.transfers.iter().find(|t| t.direction == direction && t.row.sof_ps.is_some()) {
                    assert_eq!(transfer.row.sof_ps, Some(10_000_000));
                    assert!(replacement.is_some());
                    if !started {
                        eprintln!("actual queued source frame SOF={:?} at now={now}", transfer.row.sof_ps);
                    }
                    started = true;
                }
            });
        }
        assert!(old.is_some() && replacement.is_some() && fes_reserved && started);
        assert_eq!(engine.snapshot.common.termination, "time_limit");
        std::fs::remove_dir_all(root).unwrap();
    }
}
