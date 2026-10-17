//! Fragment boundary scheduler. Owns no queue and schedules only through NetworkDelta.
use super::replication::identifier;
use super::*;
#[derive(Debug, Clone, Default)]
pub(super) struct TransferFragments {
    pub completed: u64,
    pub occupied: u64,
    pub fragments: Vec<String>,
    pub gaps: Vec<String>,
    pub active_start: Option<u64>,
    pub active_end: Option<u64>,
}
#[derive(Debug, Clone, Default)]
pub(super) struct Port {
    pub active: Option<usize>,
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
#[derive(Debug, Clone)]
pub(super) struct PreemptionState {
    pub ports: Vec<Port>,
    pub fragments: BTreeMap<String, Value>,
    pub gaps: BTreeMap<String, Value>,
    pub deliveries: BTreeMap<String, Value>,
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
    fn preemption_port<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Port {
        &d.preemption
            .as_ref()
            .or(self.preemption.as_ref())
            .unwrap()
            .ports[i]
    }
    fn preemptable(&self, t: &Transfer) -> bool {
        self.prepared
            .extensions
            .as_ref()
            .and_then(|e| e.preemption.as_ref())
            .is_some_and(|p| p.outputs[t.direction].preemptable_mask & (1 << t.row.priority) != 0)
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
    fn preemption_sof(&self, t: usize, d: &mut NetworkDelta, now: u64) -> Result<()> {
        let transfer = self.transfer(d, t).clone();
        if transfer.row.parent_transfer_id.is_some() {
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
        if let (Some(t), Some(end)) = (port.active, port.active_end) {
            input.sending = Some(Sending {
                priority: self.transfer(d, t).row.priority,
                release_ps: end,
            });
        }
        Ok(input)
    }
    pub(super) fn preemption_arbitrate(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
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
        let input = self.preemption_input(direction, d)?;
        let tsn = self
            .tsn
            .as_ref()
            .ok_or_else(|| error("preemption requires TSN"))?;
        let delta =
            tsn.plan_port_in(std::mem::take(&mut d.tsn), &link.from_port, c.now(), &input)?;
        let ready = delta.ready.get(&link.from_port).copied().unwrap_or(0);
        self.stage_tsn(d, delta, &[link.from_port.clone()])?;
        let port = self.preemption_port(d, direction).clone();
        if port.active.is_some() || port.gap_due.is_some() {
            return self.preemption_decision(direction, d, c);
        }
        let express = (0..8)
            .rev()
            .find(|p| ready & config.express_mask & (1 << p) != 0);
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
        } else {
            (0..8)
                .rev()
                .find(|p| ready & config.preemptable_mask & (1 << p) != 0)
                .and_then(|p| self.queue(d, direction).waiting[p].front().copied())
        };
        if let Some(t) = selected {
            self.preemption_start(t, d, c)?;
        }
        self.preemption_decision(direction, d, c)
    }
    fn preemption_start(&self, t: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut transfer = self.transfer(d, t).clone();
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
            port.active = Some(t);
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
                transfer.row.planned_arrival_ps = Some(add(end, link.delay_ps)?);
            }
            (end, end, add(end, link.delay_ps)?)
        } else {
            let (eof, release, arrival) = l2::timing(
                c.now(),
                transfer.row.wire.mac_bytes,
                link.bitrate_bps,
                link.delay_ps,
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
            port.active = Some(t);
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
        self.preemption_sof(t, d, c.now())?;
        let post = self.preemption_input(direction, d)?;
        let tsn = self.tsn.as_ref().unwrap();
        let (delta, _, _) = tsn.plan_start_duration_in(
            std::mem::take(&mut d.tsn),
            &link.from_port,
            p as u8,
            c.now(),
            eof - c.now(),
            release - c.now(),
            &post,
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
        if port.tx_epoch != epoch || port.active != Some(t) {
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
        let active = p.active.map(|t| self.transfer(d, t));
        let suspended = p.suspended.map(|t| self.transfer(d, t));
        let state = if let Some(active) = active {
            if self.preemptable(active) {
                "PreemptableFragment"
            } else {
                "Express"
            }
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
        } else if active.is_some() {
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
        let data = json!({"port":link.from_port,"state":state,"wait_reason":reason,"active_transfer_id":active.map(|t|&t.row.transfer_id),"suspended_transfer_id":suspended.map(|t|&t.row.transfer_id),"offset_bytes":offset.to_string(),"remaining_bytes":remaining.to_string(),"next_wake_ps":opt(p.gap_due.or(wake)),"tx_epoch":p.tx_epoch.to_string(),"gap_epoch":p.gap_epoch.to_string(),"wake_epoch":p.wake_epoch.to_string()});
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
        json!({"transfer_id":row.transfer_id,"frame_id":row.frame_id,"parent_transfer_id":row.parent_transfer_id,"queue_id":row.queue_id,"priority":row.priority.to_string(),"wire":wire(&row.wire),"vlan_id":opt(row.vlan_id.map(u64::from)),"from_port":row.from_port,"to_port":row.to_port,"queued_ps":row.queued_ps.to_string(),"sof_ps":opt(row.sof_ps),"eof_ps":opt(row.eof_ps),"release_ps":opt(row.release_ps),"arrival_ps":opt(row.arrival_ps),"planned_eof_ps":opt(row.planned_eof_ps),"planned_release_ps":opt(row.planned_release_ps),"planned_arrival_ps":opt(row.planned_arrival_ps),"status":if row.status=="queued"{"queued"}else if row.status=="dropped"{"dropped"}else if row.arrival_ps.is_some(){"completed"}else{"in_flight"},"drop_reason":row.drop_reason,"kind":if self.preemptable(t){"preemptable"}else{"express"},"visit_id":t.visit.to_string(),"copy_id":row.transfer_id,"policy_epoch_offer":t.offer_epoch.to_string(),"policy_epoch_sof":opt(t.sof_epoch),"mac_bytes":row.wire.mac_bytes.to_string(),"completed_mac_bytes":f.completed.to_string(),"fragment_ids":f.fragments,"gap_ids":f.gaps,"occupied_ps":(u128::from(f.occupied)+u128::from(prefix)).to_string()})
    }
    pub(super) fn preemption_snapshot(&self) -> Option<Value> {
        self.preemption.as_ref().map(|state|json!({"ports":state.ports.iter().enumerate().map(|(i,p)|{
            let transfer=p.active.or(p.suspended).map(|i|&self.transfers[i]);
            let completed=transfer.map_or(0,|t|t.fragments.as_ref().unwrap().completed);
            json!({"port":self.prepared.ethernet.directions[i].from_port,"state":if let Some(t)=p.active {if self.preemptable(&self.transfers[t]){"PreemptableFragment"}else{"Express"}}else if p.suspended.is_some(){"Suspended"}else{"Idle"},"active":p.active.map(|i|&self.transfers[i].row.transfer_id),"active_end":opt(p.active_end),"suspended":p.suspended.map(|i|&self.transfers[i].row.transfer_id),"frame":transfer.map(|t|&t.row.frame_id),"next_offset":completed.to_string(),"remaining":transfer.map_or(0,|t|t.row.wire.mac_bytes.saturating_sub(completed)).to_string(),"after_express":p.after_express,"tx_epoch":p.tx_epoch.to_string(),"gap_epoch":p.gap_epoch.to_string(),"wake_epoch":p.wake_epoch.to_string(),"gap_due":opt(p.gap_due),"wake_due":opt(p.wake_due),"fragment_refs":transfer.map(|t|&t.fragments.as_ref().unwrap().fragments),"gap_refs":transfer.map(|t|&t.fragments.as_ref().unwrap().gaps)})
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
