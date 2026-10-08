//! Private immutable planning for composed Ethernet profiles. The registered engine owns the FES.
use super::can_ethernet::{
    BridgeAction, BridgeDelta, BridgeEvent, BridgeState, BridgeTimer, BridgeTxView, TxQueueView,
};
use super::ethernet::{
    dynamic::{DynamicDelta, DynamicState},
    l2,
    tsn::{Head, PortInput, Sending, TsnDelta, TsnState, WakeToken},
};
use crate::registry::{Context, Effect, Envelope, ModelError, ModelRecord, Schema, TimerToken};
use crate::snapshot::Point;
use crate::snapshot::ethernet::*;
use crate::types::can_ethernet::BridgeLineage;
use crate::types::{Diagnostic, ethernet::dynamic::*, ethernet::*, network::PreparedNetwork};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(s: impl Into<String>) -> Diagnostic {
    Diagnostic::execution(s)
}
fn model(e: ModelError) -> Diagnostic {
    error(e.0)
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| {
        let mut e = error("network time/ID overflow");
        e.code = "E-0004".into();
        e
    })
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Event {
    Dispatch,
    Generate(usize, u64),
    Ready(usize),
    Arrival(usize),
    Complete(usize),
    Bridge(BridgeTimer),
    Control(u64),
}
#[derive(Debug, Clone)]
struct Frame {
    row: EthernetFrameRecord,
    ip: Option<IpMulticast>,
    visits: u64,
    lineage: Option<BridgeLineage>,
    branch_id: Option<String>,
}
#[derive(Debug, Clone)]
struct Transfer {
    row: EthernetTransferRecord,
    frame: usize,
    direction: usize,
    visit: u64,
    offer_epoch: u64,
    sof_epoch: Option<u64>,
}
#[derive(Debug, Clone)]
struct Reception {
    row: EthernetReceptionRecord,
    transfer: usize,
    visit: u64,
    epoch: u64,
}
struct CopyOrigin {
    frame: usize,
    parent: Option<usize>,
    visit: u64,
}
#[derive(Debug, Clone, Default)]
struct Queue {
    waiting: [VecDeque<usize>; 8],
    bytes: [u64; 8],
    active: Option<usize>,
}
#[derive(Debug, Clone)]
enum Completion {
    Eof(usize),
    Release(usize),
    Ready(usize),
    Processed(usize),
    Bridge(BridgeTimer),
}
#[derive(Debug, Clone)]
struct Due {
    at: u64,
    kind: Completion,
}
pub(crate) struct NetworkState {
    prepared: Arc<PreparedNetwork>,
    cursors: Vec<u64>,
    frames: Vec<Frame>,
    transfers: Vec<Transfer>,
    receptions: Vec<Reception>,
    queues: Vec<Queue>,
    dynamic: Option<DynamicState>,
    bridge: Option<BridgeState>,
    tsn: Option<TsnState>,
    tsn_wakes: Vec<Option<WakeToken>>,
    due: Vec<Due>,
    control: Option<(u64, TimerToken)>,
    control_generation: u64,
    record_sequence: u64,
    next_visit: u64,
    future_generation: bool,
}
pub(crate) struct NetworkDelta {
    pub(crate) points: Vec<Point>,
    cursors: Option<Vec<u64>>,
    frames: Vec<(usize, Frame)>,
    transfers: Vec<(usize, Transfer)>,
    receptions: Vec<(usize, Reception)>,
    queues: BTreeMap<usize, Queue>,
    dynamic: Option<DynamicDelta>,
    bridge: Option<BridgeDelta>,
    tsn: TsnDelta,
    tsn_wakes: BTreeMap<usize, Option<WakeToken>>,
    due: Option<Vec<Due>>,
    control: Option<Option<(u64, TimerToken)>>,
    control_generation: u64,
    record_sequence: u64,
    next_visit: u64,
    future_generation: bool,
}
impl NetworkState {
    pub(crate) fn new(p: Arc<PreparedNetwork>) -> Result<Self> {
        let dynamic = p
            .dynamic
            .as_ref()
            .map(|d| DynamicState::new(d, &p.ethernet))
            .transpose()?;
        let tsn = p.tsn.as_ref().map(TsnState::new);
        let tsn_wakes = vec![None; p.tsn.as_ref().map_or(0, |t| t.outputs.len())];
        let bridge = p.bridge.as_ref().map(BridgeState::new).transpose()?;
        Ok(Self {
            cursors: vec![0; p.ethernet.generators.len()],
            bridge,
            tsn,
            tsn_wakes,
            queues: vec![Queue::default(); p.ethernet.directions.len()],
            prepared: p,
            frames: vec![],
            transfers: vec![],
            receptions: vec![],
            dynamic,
            due: vec![],
            control: None,
            control_generation: 0,
            record_sequence: 0,
            next_visit: 1,
            future_generation: false,
        })
    }
    fn delta(&self) -> NetworkDelta {
        NetworkDelta {
            points: vec![],
            cursors: None,
            frames: vec![],
            transfers: vec![],
            receptions: vec![],
            queues: BTreeMap::new(),
            dynamic: None,
            bridge: self.bridge.as_ref().map(BridgeState::empty_delta),
            tsn: TsnDelta::default(),
            tsn_wakes: BTreeMap::new(),
            due: None,
            control: None,
            control_generation: self.control_generation,
            record_sequence: self.record_sequence,
            next_visit: self.next_visit,
            future_generation: self.future_generation,
        }
    }
    fn frame<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Frame {
        d.frames
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.frames[i])
    }
    fn transfer<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Transfer {
        d.transfers
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.transfers[i])
    }
    fn reception<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Reception {
        d.receptions
            .iter()
            .rev()
            .find(|(n, _)| *n == i)
            .map(|(_, v)| v)
            .unwrap_or_else(|| &self.receptions[i])
    }
    fn queue<'a>(&'a self, d: &'a NetworkDelta, i: usize) -> &'a Queue {
        d.queues.get(&i).unwrap_or(&self.queues[i])
    }
    fn queue_mut<'a>(&self, d: &'a mut NetworkDelta, i: usize) -> &'a mut Queue {
        d.queues.entry(i).or_insert_with(|| self.queues[i].clone())
    }
    fn due_mut<'a>(&self, d: &'a mut NetworkDelta) -> &'a mut Vec<Due> {
        d.due.get_or_insert_with(|| self.due.clone())
    }
    fn policy(&self, d: &NetworkDelta) -> Option<PolicySnapshot> {
        self.dynamic.as_ref().map(|s| {
            d.dynamic.as_ref().map_or_else(
                || s.snapshot_policy(),
                |delta| s.snapshot_policy_after(delta),
            )
        })
    }
    fn epoch_after(&self, d: &NetworkDelta) -> u64 {
        self.policy(d).map_or(0, |p| p.policy_epoch)
    }
    fn epoch(&self) -> u64 {
        self.dynamic.as_ref().map_or(0, DynamicState::policy_epoch)
    }
    fn schedule(c: &mut Context<'_>, at: u64, event: Event) -> Result<TimerToken> {
        let phase = matches!(event, Event::Control(_));
        let bytes = serde_json::to_vec(&event).map_err(|e| error(e.to_string()))?;
        c.schedule_at(
            at,
            Schema::new(
                if phase {
                    "dir.network.control"
                } else {
                    "dir.network.event"
                },
                1,
            ),
            bytes,
        )
        .map_err(model)
    }
    fn record(
        c: &mut Context<'_>,
        d: &mut NetworkDelta,
        schema: &str,
        id: String,
        mut data: Value,
    ) -> Result<()> {
        let seq = d.record_sequence;
        d.record_sequence = add(seq, 1)?;
        if !matches!(
            schema,
            "can.request"
                | "can.receiver"
                | "ethernet.frame"
                | "ethernet.transfer"
                | "ethernet.reception"
        ) {
            data["effect_seq"] = json!(seq.to_string());
        }
        c.upsert_model_record(ModelRecord {
            id,
            subject: c.subject().into(),
            time_ps: c.now(),
            schema: Schema::new(
                schema,
                match schema {
                    "ethernet.frame" | "ethernet.transfer" => 3,
                    "ethernet.reception" => 2,
                    _ => 1,
                },
            ),
            data,
        })
        .map_err(model)
    }
    fn emit_dynamic(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if let Some(delta) = d.dynamic.as_ref() {
            let mut records = Vec::new();
            for r in &delta.control_records {
                let mut v = decimal(serde_json::to_value(r).map_err(|e| error(e.to_string()))?);
                let mut id = if r.synthetic {
                    format!(
                        "{}:{}",
                        r.control_id,
                        add(d.record_sequence, records.len() as u64)?
                    )
                } else {
                    r.control_id.clone()
                };
                if r.synthetic {
                    while self
                        .prepared
                        .dynamic
                        .as_ref()
                        .unwrap()
                        .controls
                        .iter()
                        .any(|control| control.id == id)
                    {
                        id.push(':');
                    }
                }
                v["control_id"] = json!(id);
                records.push(("ethernet.dynamic.control", id, v));
            }
            if let Some(r) = &delta.policy_record {
                records.push((
                    "ethernet.dynamic.policy",
                    format!("policy:{}", r.policy_epoch),
                    decimal(serde_json::to_value(r).map_err(|e| error(e.to_string()))?),
                ));
            }
            for (s, i, v) in records {
                Self::record(c, d, s, i, v)?;
            }
        }
        Ok(())
    }
    fn arm_control(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let due = d
            .due
            .as_ref()
            .unwrap_or(&self.due)
            .iter()
            .map(|v| v.at)
            .min();
        let policy = d
            .dynamic
            .as_ref()
            .and_then(|v| v.next_deadline)
            .or_else(|| {
                if d.dynamic.is_none() {
                    self.dynamic.as_ref().and_then(DynamicState::next_deadline)
                } else {
                    None
                }
            });
        let tsn_control = self.tsn.as_ref().and_then(|s| s.next_control_after(&d.tsn));
        let wake = (0..self.tsn_wakes.len())
            .filter_map(|i| {
                d.tsn_wakes
                    .get(&i)
                    .unwrap_or(&self.tsn_wakes[i])
                    .as_ref()
                    .map(|w| w.time_ps)
            })
            .min();
        let at = due
            .into_iter()
            .chain(policy)
            .chain(tsn_control)
            .chain(wake)
            .min();
        let old = d.control.as_ref().unwrap_or(&self.control);
        if old.as_ref().map(|v| v.0) == at {
            return Ok(());
        }
        if let Some((_, token)) = old {
            c.batch.effects.push(Effect::Cancel(token.clone()));
        }
        d.control_generation = add(d.control_generation, 1)?;
        d.control = Some(if let Some(at) = at {
            Some((
                at,
                Self::schedule(c, at, Event::Control(d.control_generation))?,
            ))
        } else {
            None
        });
        Ok(())
    }
    pub(crate) fn initialize(&self, c: &mut Context<'_>, limit: u64) -> Result<NetworkDelta> {
        let mut d = self.delta();
        for i in 0..self.queues.len() {
            d.queues.insert(i, Queue::default());
        }
        self.queue_points(&mut d, c.now());
        if let Some(state) = &self.dynamic {
            Self::record(
                c,
                &mut d,
                "ethernet.dynamic.policy",
                "policy:0".into(),
                decimal(
                    serde_json::to_value(state.initial_record())
                        .map_err(|e| error(e.to_string()))?,
                ),
            )?;
        }
        if let Some(state) = &self.bridge {
            d.bridge = Some(state.initialize(limit)?);
            self.bridge_actions(&mut d, c, limit)?;
        }
        if let Some(time) = self.next_generation(&self.cursors) {
            if let Ok(time) = u64::try_from(time) {
                // Reserving the next cursor event is independent of the run
                // horizon, so earlier callbacks retain identical FES sequences.
                Self::schedule(c, time, Event::Dispatch)?;
            } else {
                d.future_generation = true;
            }
        }
        if let Some(tsn) = &self.tsn {
            if tsn.next_control() != Some(0) {
                self.refresh_tsn(&mut d, c, true)?;
            }
        }
        self.emit_tsn(&mut d, c)?;
        self.arm_control(&mut d, c)?;
        Ok(d)
    }
    pub(crate) fn event(
        &self,
        envelope: &Envelope,
        c: &mut Context<'_>,
        limit: u64,
    ) -> Result<NetworkDelta> {
        let event: Event =
            serde_json::from_slice(&envelope.payload).map_err(|e| error(e.to_string()))?;
        let mut d = self.delta();
        let now = c.now();
        let control_event = matches!(event, Event::Control(_));
        match event {
            Event::Dispatch => {
                let mut cursors = self.cursors.clone();
                let g = self
                    .prepared
                    .ethernet
                    .generators
                    .iter()
                    .enumerate()
                    .filter(|(i, g)| g.time(cursors[*i]) == Some(u128::from(now)))
                    .min_by(|(_, a), (_, b)| a.id.cmp(&b.id))
                    .map(|(i, _)| i)
                    .ok_or_else(|| error("network generation cursor invariant"))?;
                let ordinal = cursors[g];
                cursors[g] = add(ordinal, 1)?;
                Self::schedule(c, now, Event::Generate(g, ordinal))?;
                if let Some(time) = self.next_generation(&cursors) {
                    if let Ok(time) = u64::try_from(time) {
                        Self::schedule(c, time, Event::Dispatch)?;
                    } else {
                        d.future_generation = true;
                    }
                }
                d.cursors = Some(cursors);
            }
            Event::Generate(g, ordinal) => {
                let generator = self
                    .prepared
                    .ethernet
                    .generators
                    .get(g)
                    .ok_or_else(|| error("unknown generator"))?;
                let device = &self.prepared.ethernet.devices[generator.source];
                let ready = add(now, device.tx_processing_delay_ps)?;
                let f = self.frames.len();
                let ip = self
                    .prepared
                    .dynamic
                    .as_ref()
                    .and_then(|p| p.generator_ip.get(&generator.id))
                    .cloned()
                    .flatten();
                let row = EthernetFrameRecord {
                    frame_id: format!("{}:{ordinal}", generator.id),
                    time_ps: now,
                    source: device.id.clone(),
                    source_vlan_id: generator.source_vlan_id,
                    wire: generator.frame.clone(),
                    flow_id: generator.flow_id.clone(),
                    priority: generator.priority,
                    deadline_ps: generator.deadline_ps,
                    generated_ps: now,
                    ready_ps: None,
                };
                let lineage = self
                    .bridge
                    .as_ref()
                    .map(|_| BridgeLineage::native("ethernet", &row.frame_id, now));
                d.frames.push((
                    f,
                    Frame {
                        row,
                        ip,
                        visits: 0,
                        lineage,
                        branch_id: None,
                    },
                ));
                if ready == now {
                    self.source_offer(f, &mut d, c)?;
                } else {
                    self.due_mut(&mut d).push(Due {
                        at: ready,
                        kind: Completion::Ready(f),
                    });
                }
            }
            Event::Bridge(timer) => {
                self.bridge_event(&BridgeEvent::Timer(timer), &mut d, c, limit)?
            }
            Event::Ready(f) => self.source_offer(f, &mut d, c)?,
            Event::Arrival(t) => self.arrive(t, &mut d, c)?,
            Event::Complete(r) => self.complete(r, &mut d, c)?,
            Event::Control(generation) => {
                if generation != self.control_generation {
                    return Ok(d);
                }
                d.control = Some(None);
                let mut due = self.due.clone();
                let mut current = Vec::new();
                due.retain(|v| {
                    if v.at == now {
                        current.push(v.kind.clone());
                        false
                    } else {
                        true
                    }
                });
                d.due = Some(due);
                for kind in current {
                    match kind {
                        Completion::Bridge(timer @ BridgeTimer::ConversionReady { .. }) => {
                            // Positive-delay conversion completion notifies phase 1.
                            // Admission (and RX release) must follow already-reserved
                            // arrivals, just like Ethernet processing completions.
                            Self::schedule(c, now, Event::Bridge(timer))?;
                        }
                        Completion::Bridge(timer) => {
                            self.bridge_event(&BridgeEvent::Timer(timer), &mut d, c, limit)?
                        }
                        Completion::Eof(t) => {
                            let mut row = self.transfer(&d, t).clone();
                            row.row.time_ps = now;
                            row.row.eof_ps = Some(now);
                            row.row.status = "serialized".into();
                            let arrival = row
                                .row
                                .planned_arrival_ps
                                .ok_or_else(|| error("missing arrival"))?;
                            put(&mut d.transfers, t, row);
                            Self::schedule(c, arrival, Event::Arrival(t))?;
                        }
                        Completion::Release(t) => {
                            let mut row = self.transfer(&d, t).clone();
                            row.row.time_ps = now;
                            row.row.release_ps = Some(now);
                            self.queue_mut(&mut d, row.direction).active = None;
                            put(&mut d.transfers, t, row);
                            c.request_arbitration("network").map_err(model)?;
                        }
                        Completion::Ready(f) => {
                            Self::schedule(c, now, Event::Ready(f))?;
                        }
                        Completion::Processed(r) => {
                            Self::schedule(c, now, Event::Complete(r))?;
                        }
                    }
                }
                if let Some(state) = &self.dynamic {
                    if state.next_deadline().is_some_and(|t| t <= now) {
                        let delta = state.plan_controls(now)?;
                        if !delta.dirty_ports.is_empty() {
                            c.request_arbitration("network").map_err(model)?;
                        }
                        d.dynamic = Some(delta);
                    }
                }
                self.refresh_tsn(&mut d, c, true)?;
            }
        }
        if !control_event {
            self.refresh_tsn(&mut d, c, false)?;
        }
        if control_event {
            // Publish the same phase-0 order used to build the staged state:
            // wire completions, dynamic policy, then GCL/credit observations.
            self.emit_rows(&mut d, c)?;
            self.emit_dynamic(&mut d, c)?;
            self.emit_tsn(&mut d, c)?;
        } else {
            self.emit_tsn(&mut d, c)?;
            self.emit_dynamic(&mut d, c)?;
            self.emit_rows(&mut d, c)?;
        }
        self.arm_control(&mut d, c)?;
        self.queue_points(&mut d, c.now());
        Ok(d)
    }
    fn source_offer(&self, f: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut frame = self.frame(d, f).clone();
        frame.row.time_ps = c.now();
        frame.row.ready_ps = Some(c.now());
        let source = self
            .prepared
            .ethernet
            .devices
            .iter()
            .position(|v| v.id == frame.row.source)
            .ok_or_else(|| error("unknown source"))?;
        let class = l2::Classification {
            vid: frame
                .row
                .source_vlan_id
                .ok_or_else(|| error("missing source VID"))?,
            priority: frame.row.priority,
            dei: frame.row.wire.tag.map_or(0, |t| t.dei),
        };
        put(&mut d.frames, f, frame.clone());
        let dirs: Vec<_> = self
            .prepared
            .ethernet
            .directions
            .iter()
            .enumerate()
            .filter(|(_, v)| v.source == source)
            .map(|(i, _)| i)
            .collect();
        self.offer(
            CopyOrigin {
                frame: f,
                parent: None,
                visit: 0,
            },
            &frame.row.wire,
            class,
            &dirs,
            d,
            c,
        )?;
        Ok(())
    }
    fn offer(
        &self,
        origin: CopyOrigin,
        wire: &EthernetWireFrame,
        class: l2::Classification,
        dirs: &[usize],
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<Vec<String>> {
        let CopyOrigin {
            frame: f,
            parent,
            visit,
        } = origin;
        let mut ids = Vec::new();
        for &direction in dirs {
            let link = &self.prepared.ethernet.directions[direction];
            let port = &link.from_port;
            let policy = self
                .prepared
                .ethernet
                .port_policies
                .iter()
                .find(|p| &p.port == port)
                .ok_or_else(|| error("missing output policy"))?;
            let ps = self.policy(d);
            let tagged = ps
                .as_ref()
                .and_then(|p| p.effective_vlans.get(port))
                .and_then(|v| v.get(&class.vid))
                .copied()
                .or_else(|| {
                    self.prepared.dynamic.as_ref().and_then(|p| {
                        p.registrable
                            .contains(&RegistrationKey {
                                port: port.clone(),
                                vid: class.vid,
                            })
                            .then_some(true)
                    })
                });
            let outgoing = l2::rewrite(policy, wire, class, tagged)?;
            let t = self.transfers.len()
                + d.transfers
                    .iter()
                    .filter(|(i, _)| *i >= self.transfers.len())
                    .count();
            let frame = self.frame(d, f);
            let id = if self.bridge.is_some() {
                format!("{}@{port}", frame.row.frame_id)
            } else {
                format!("{}/v{visit}/c{t}", frame.row.frame_id)
            };
            let q = self.queue(d, direction);
            let p = class.priority as usize;
            let config = self
                .prepared
                .ethernet
                .outputs
                .iter()
                .find(|o| o.port == *port)
                .and_then(|o| o.queues.iter().find(|q| q.priority == class.priority));
            let policy_reason = self
                .dynamic
                .as_ref()
                .zip(ps.as_ref())
                .and_then(|(s, p)| s.eligible_egress(port, class.vid, p).err())
                .map(|r| r.reason());
            let admitted = policy_reason.is_none()
                && l2::admission(
                    q.waiting.iter().map(|v| v.len() as u64).sum(),
                    self.prepared.ethernet.devices[link.source].queue_capacity,
                    q.waiting[p].len() as u64,
                    q.bytes[p],
                    config,
                    outgoing.mac_bytes,
                )?;
            let row = EthernetTransferRecord {
                transfer_id: id.clone(),
                time_ps: c.now(),
                frame_id: frame.row.frame_id.clone(),
                parent_transfer_id: parent.map(|t| self.transfer(d, t).row.transfer_id.clone()),
                queue_id: Some(format!("{port}.queue.{}", class.priority)),
                priority: class.priority,
                wire: outgoing.clone(),
                vlan_id: Some(class.vid),
                from_port: port.clone(),
                to_port: link.to_port.clone(),
                queued_ps: c.now(),
                sof_ps: None,
                eof_ps: None,
                release_ps: None,
                arrival_ps: None,
                planned_eof_ps: None,
                planned_release_ps: None,
                planned_arrival_ps: None,
                status: if admitted { "queued" } else { "dropped" }.into(),
                drop_reason: (!admitted).then(|| policy_reason.unwrap_or("queue_full").into()),
                media: None,
            };
            if admitted {
                let q = self.queue_mut(d, direction);
                q.bytes[p] = add(q.bytes[p], outgoing.mac_bytes)?;
                q.waiting[p].push_back(t);
                c.request_arbitration("network").map_err(model)?;
            }
            d.transfers.push((
                t,
                Transfer {
                    row,
                    frame: f,
                    direction,
                    visit,
                    offer_epoch: self.epoch_after(d),
                    sof_epoch: None,
                },
            ));
            ids.push(id);
        }
        Ok(ids)
    }
    fn arrive(&self, t: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut transfer = self.transfer(d, t).clone();
        let link = &self.prepared.ethernet.directions[transfer.direction];
        let device = &self.prepared.ethernet.devices[link.destination];
        let policy = self
            .prepared
            .ethernet
            .port_policies
            .iter()
            .find(|p| p.ingress == link.to_port)
            .ok_or_else(|| error("missing ingress policy"))?;
        let class = l2::classify(policy, &transfer.row.wire);
        let ps = self.policy(d);
        let mut reason = self
            .dynamic
            .as_ref()
            .zip(ps.as_ref())
            .and_then(|(s, p)| s.eligible_egress(&policy.port, class.vid, p).err())
            .map(|r| r.reason().to_string());
        if reason.is_none() && self.dynamic.is_none() && !policy.vlans.contains_key(&class.vid) {
            reason = Some("ingress_vlan_membership".into());
        }
        if reason.is_none()
            && ((policy.admit == "tagged_only" && transfer.row.wire.tag.is_none())
                || (policy.admit == "untagged_only" && transfer.row.wire.tag.is_some()))
        {
            reason = Some("ingress_frame_type".into());
        }
        if reason.is_none()
            && device.kind == "endpoint"
            && device.mac.as_ref() != Some(&transfer.row.wire.dst_mac)
            && transfer.row.wire.dst_mac != "ff:ff:ff:ff:ff:ff"
        {
            let group =
                u8::from_str_radix(&transfer.row.wire.dst_mac[..2], 16).unwrap_or(0) & 1 != 0;
            if !group {
                reason = Some("destination_mismatch".into());
            } else if !device
                .subscriptions
                .contains(&(class.vid, transfer.row.wire.dst_mac.clone()))
            {
                reason = Some("multicast_not_subscribed".into());
            }
        }
        if reason.is_none() {
            if let Some(tsn) = &self.tsn {
                let delta = tsn.plan_arrival_in(
                    std::mem::take(&mut d.tsn),
                    &link.to_port,
                    &transfer.row.wire.dst_mac,
                    class.vid,
                    class.priority,
                    transfer.row.wire.mac_bytes,
                    &format!("{}@rx", transfer.row.transfer_id),
                    c.now(),
                )?;
                if let Some(p) = &delta.psfp {
                    if p.verdict == "drop" {
                        reason = Some(p.reason.clone());
                    }
                }
                d.tsn = delta;
            }
        }
        let mut frame = self.frame(d, transfer.frame).clone();
        let visit = d.next_visit;
        d.next_visit = add(d.next_visit, 1)?;
        if reason.is_none()
            && self
                .prepared
                .dynamic
                .as_ref()
                .is_some_and(|p| frame.visits >= p.limits.visits_per_frame)
        {
            reason = Some("visit_limit".into());
        }
        if reason.is_none() {
            frame.visits = add(frame.visits, 1)?;
            put(&mut d.frames, transfer.frame, frame);
        }
        let ready = if reason.is_none() {
            Some(add(
                c.now(),
                if device.kind == "endpoint" {
                    device.rx_processing_delay_ps
                } else {
                    device.forward_delay_ps
                },
            )?)
        } else {
            None
        };
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
            device: device.id.clone(),
            ingress: link.to_port.clone(),
            vlan_id: Some(class.vid),
            priority: class.priority,
            observed_ps: c.now(),
            ready_ps: reason.as_ref().map(|_| c.now()),
            planned_ready_ps: ready,
            status: if reason.is_some() {
                "filtered"
            } else {
                "processing"
            }
            .into(),
            reason,
            egress_transfer_ids: vec![],
        };
        transfer.row.time_ps = c.now();
        transfer.row.arrival_ps = Some(c.now());
        put(&mut d.transfers, t, transfer.clone());
        d.receptions.push((
            r,
            Reception {
                row,
                transfer: t,
                visit,
                epoch: self.epoch(),
            },
        ));
        if ready.is_some() {
            if let Some(state) = &self.dynamic {
                d.dynamic = Some(state.plan_learning(
                    &link.to_port,
                    class.vid,
                    &transfer.row.wire.src_mac,
                    c.now(),
                )?);
            }
        }
        if ready.is_some() && self.bridge.is_some() {
            let frame = self.frame(d, transfer.frame);
            let event = BridgeEvent::EthernetIngress {
                endpoint: link.destination,
                ingress_record: format!("{}@rx", transfer.row.transfer_id),
                origin_id: frame
                    .lineage
                    .as_ref()
                    .map_or_else(|| frame.row.frame_id.clone(), |l| l.origin_id.clone()),
                generated_ps: frame
                    .lineage
                    .as_ref()
                    .map_or(frame.row.generated_ps, |l| l.generated_ps),
                wire: transfer.row.wire.clone(),
                vid: class.vid,
                priority: class.priority,
                lineage: frame.lineage.clone(),
            };
            self.bridge_event(&event, d, c, u64::MAX)?;
        }

        if ready == Some(c.now()) {
            self.complete(r, d, c)?;
        } else if let Some(ready) = ready {
            self.due_mut(d).push(Due {
                at: ready,
                kind: Completion::Processed(r),
            });
        }
        Ok(())
    }
    fn complete(&self, r: usize, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut reception = self.reception(d, r).clone();
        let transfer = self.transfer(d, reception.transfer).clone();
        let link = &self.prepared.ethernet.directions[transfer.direction];
        let device = &self.prepared.ethernet.devices[link.destination];
        reception.row.time_ps = c.now();
        reception.row.ready_ps = Some(c.now());
        if device.kind == "endpoint" {
            reception.row.status = "received".into();
            let frame = self.frame(d, transfer.frame);
            d.points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: c.now(),
                target: device.id.clone(),
                metric: "ethernet.delivery_ps".into(),
                value: c
                    .now()
                    .checked_sub(frame.row.generated_ps)
                    .ok_or_else(|| error("delivery before generation"))?,
                request_id: Some(frame.row.frame_id.clone()),
                receiver: Some(device.id.clone()),
                reason: None,
            });
            if let Some(lineage) = self.frame(d, transfer.frame).lineage.clone() {
                self.bridge_event(
                    &BridgeEvent::TerminalReceived {
                        lineage,
                        terminal_id: device.id.clone(),
                        reception_id: reception.row.reception_id.clone(),
                    },
                    d,
                    c,
                    u64::MAX,
                )?;
            }
        } else {
            let vid = reception.row.vlan_id.unwrap();
            let frame = self.frame(d, transfer.frame);
            let selected = if let Some(state) = &self.dynamic {
                d.dynamic.as_ref().map_or_else(
                    || {
                        state.select_egress(
                            &device.id,
                            &link.to_port,
                            vid,
                            &transfer.row.wire,
                            frame.ip.as_ref(),
                        )
                    },
                    |delta| {
                        state.select_egress_after(
                            delta,
                            &device.id,
                            &link.to_port,
                            vid,
                            &transfer.row.wire,
                            frame.ip.as_ref(),
                        )
                    },
                )
            } else {
                {
                    let (directions, reason) = l2::static_egress(
                        &self.prepared.ethernet,
                        transfer.direction,
                        &transfer.row.wire,
                        Some(vid),
                    )?;
                    EgressSelection {
                        ports: directions
                            .into_iter()
                            .map(|i| self.prepared.ethernet.directions[i].from_port.clone())
                            .collect(),
                        reason: reason.map(str::to_string),
                    }
                }
            };
            let dirs: Vec<_> = selected
                .ports
                .iter()
                .filter_map(|p| {
                    self.prepared
                        .ethernet
                        .directions
                        .iter()
                        .position(|v| v.from_port == *p)
                })
                .collect();
            let class = l2::Classification {
                vid,
                priority: reception.row.priority,
                dei: transfer.row.wire.tag.map_or(0, |v| v.dei),
            };
            reception.row.egress_transfer_ids = self.offer(
                CopyOrigin {
                    frame: transfer.frame,
                    parent: Some(reception.transfer),
                    visit: reception.visit,
                },
                &transfer.row.wire,
                class,
                &dirs,
                d,
                c,
            )?;
            reception.row.status = if selected.reason.is_some() {
                "filtered"
            } else {
                "forwarded"
            }
            .into();
            reception.row.reason = selected.reason;
        }
        put(&mut d.receptions, r, reception);
        Ok(())
    }
    pub(crate) fn arbitrate(&self, c: &mut Context<'_>) -> Result<NetworkDelta> {
        let mut d = self.delta();
        let policy = self.policy(&d);
        for direction in 0..self.queues.len() {
            let link = &self.prepared.ethernet.directions[direction];
            // Eligibility applies to every queued copy, even when its gate would remain closed.
            for priority in 0..8 {
                let pending: Vec<_> = self.queue(&d, direction).waiting[priority]
                    .iter()
                    .copied()
                    .collect();
                for t in pending {
                    let old = self.transfer(&d, t);
                    let reason = self
                        .dynamic
                        .as_ref()
                        .zip(policy.as_ref())
                        .and_then(|(s, p)| {
                            s.eligible_egress(&link.from_port, old.row.vlan_id.unwrap(), p)
                                .err()
                        });
                    if let Some(reason) = reason {
                        let mut row = old.clone();
                        row.row.time_ps = c.now();
                        row.row.status = "dropped".into();
                        row.row.drop_reason = Some(reason.reason().into());
                        let q = self.queue_mut(&mut d, direction);
                        q.waiting[priority].retain(|v| *v != t);
                        q.bytes[priority] = q.bytes[priority]
                            .checked_sub(row.row.wire.mac_bytes)
                            .ok_or_else(|| error("queue byte underflow"))?;
                        put(&mut d.transfers, t, row);
                    }
                }
            }
            let input = self.port_input(direction, &d)?;
            let tsn_selected = if let Some(tsn) = &self.tsn {
                let next =
                    tsn.plan_port_in(std::mem::take(&mut d.tsn), &link.from_port, c.now(), &input)?;
                let selected = next.selected.get(&link.from_port).copied();
                self.stage_tsn(&mut d, next, &[link.from_port.clone()]);
                selected
            } else {
                None
            };
            if self.queue(&d, direction).active.is_some() {
                continue;
            }
            let selected = if self.tsn.is_some() {
                tsn_selected.and_then(|p| {
                    self.queue(&d, direction).waiting[p as usize]
                        .front()
                        .map(|t| (p as usize, *t))
                })
            } else {
                let strict = self
                    .prepared
                    .ethernet
                    .outputs
                    .iter()
                    .find(|o| o.port == link.from_port)
                    .is_some_and(|o| o.scheduler == "strict_priority");
                if strict {
                    (0..8).rev().find_map(|p| {
                        self.queue(&d, direction).waiting[p]
                            .front()
                            .map(|t| (p, *t))
                    })
                } else {
                    (0..8)
                        .filter_map(|p| {
                            self.queue(&d, direction).waiting[p]
                                .front()
                                .map(|t| (p, *t))
                        })
                        .min_by_key(|(_, t)| *t)
                }
            };
            let Some((p, t)) = selected else { continue };
            let mut row = self.transfer(&d, t).clone();
            let (eof, release, arrival) = l2::timing(
                c.now(),
                row.row.wire.mac_bytes,
                link.bitrate_bps,
                link.delay_ps,
            )?;
            let q = self.queue_mut(&mut d, direction);
            q.waiting[p].pop_front();
            q.bytes[p] = q.bytes[p]
                .checked_sub(row.row.wire.mac_bytes)
                .ok_or_else(|| error("queue byte underflow"))?;
            q.active = Some(t);
            row.row.time_ps = c.now();
            row.row.sof_ps = Some(c.now());
            row.row.planned_eof_ps = Some(eof);
            row.row.planned_release_ps = Some(release);
            row.row.planned_arrival_ps = Some(arrival);
            row.row.status = "transmitting".into();
            row.sof_epoch = Some(self.epoch());
            put(&mut d.transfers, t, row);
            self.due_mut(&mut d).extend([
                Due {
                    at: eof,
                    kind: Completion::Eof(t),
                },
                Due {
                    at: release,
                    kind: Completion::Release(t),
                },
            ]);
            if self.bridge.is_some() {
                let frame = self.frame(&d, self.transfer(&d, t).frame);
                let source_sof = self.transfer(&d, t).row.parent_transfer_id.is_none();
                if source_sof {
                    if let Some(lineage) = frame.lineage.clone() {
                        let event = BridgeEvent::EthernetSof {
                            branch_id: frame.branch_id.clone(),
                            lineage,
                            source_record_id: frame.row.frame_id.clone(),
                            targets: self.ethernet_targets(
                                link.source,
                                frame.row.source_vlan_id.ok_or_else(|| {
                                    error("missing source VLAN for terminal targets")
                                })?,
                                &self.transfer(&d, t).row.wire,
                            ),
                        };
                        self.bridge_event(&event, &mut d, c, u64::MAX)?;
                    }
                }
                // Retry is a phase1 notification; phase2 naturally sends it to the next delta.
                Self::schedule(
                    c,
                    c.now(),
                    Event::Bridge(BridgeTimer::RetryEthernet {
                        source: link.source,
                    }),
                )?;
            }

            if let Some(tsn) = &self.tsn {
                let input = self.port_input(direction, &d)?;
                let mac_bytes = self.transfer(&d, t).row.wire.mac_bytes;
                let (next, checked_eof, checked_release) = tsn.plan_start_in(
                    std::mem::take(&mut d.tsn),
                    &link.from_port,
                    p as u8,
                    c.now(),
                    mac_bytes,
                    &input,
                )?;
                if checked_eof != eof || checked_release != release {
                    return Err(error("TSN/shared wire timing mismatch"));
                }
                self.stage_tsn(&mut d, next, &[link.from_port.clone()]);
            }
        }
        if let Some(state) = &self.bridge {
            for bus in 0..self.prepared.bridge.as_ref().unwrap().can.buses.len() {
                let tx = self.bridge_tx(&d);
                state.plan_can_arbitration_in_delta(
                    d.bridge.as_mut().unwrap(),
                    bus,
                    c.now(),
                    &tx,
                )?;
                self.bridge_actions(&mut d, c, u64::MAX)?;
            }
        }
        self.emit_tsn(&mut d, c)?;
        self.arm_control(&mut d, c)?;
        self.emit_rows(&mut d, c)?;
        self.queue_points(&mut d, c.now());
        Ok(d)
    }
    fn bridge_tx(&self, d: &NetworkDelta) -> BridgeTxView {
        let mut view = BridgeTxView::default();
        for (i, link) in self.prepared.ethernet.directions.iter().enumerate() {
            let q = self.queue(d, i);
            let port_capacity = self.prepared.ethernet.devices[link.source].queue_capacity;
            let total = q.waiting.iter().map(|v| v.len() as u64).sum();
            for p in 0..8 {
                let config = self
                    .prepared
                    .ethernet
                    .outputs
                    .iter()
                    .find(|v| v.port == link.from_port)
                    .and_then(|o| o.queues.iter().find(|q| q.priority == p as u8));
                view.ethernet.insert(
                    (link.source, p as u8),
                    TxQueueView {
                        capacity_frames: config.map_or(port_capacity, |q| q.capacity_frames),
                        capacity_bytes: config.and_then(|q| q.capacity_bytes),
                        queued_frames: q.waiting[p].len() as u64,
                        queued_bytes: q.bytes[p],
                        port_capacity_frames: port_capacity,
                        port_queued_frames: total,
                    },
                );
            }
        }
        view
    }
    fn bridge_event(
        &self,
        event: &BridgeEvent,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        limit: u64,
    ) -> Result<()> {
        let Some(state) = &self.bridge else {
            return Ok(());
        };
        let tx = self.bridge_tx(d);
        state.plan_in_delta(d.bridge.as_mut().unwrap(), event, c.now(), &tx, limit)?;
        self.bridge_actions(d, c, limit)
    }
    fn bridge_actions(&self, d: &mut NetworkDelta, c: &mut Context<'_>, limit: u64) -> Result<()> {
        let actions = std::mem::take(&mut d.bridge.as_mut().unwrap().actions);
        for action in actions {
            match action {
                BridgeAction::Schedule { at, phase, event } => {
                    if phase == 0 {
                        if at == c.now() {
                            self.bridge_event(&BridgeEvent::Timer(event), d, c, limit)?;
                        } else {
                            self.due_mut(d).push(Due {
                                at,
                                kind: Completion::Bridge(event),
                            });
                        }
                    } else {
                        Self::schedule(c, at, Event::Bridge(event))?;
                    }
                }
                BridgeAction::DirtyCan { bus } => {
                    if bus >= self.prepared.bridge.as_ref().unwrap().can.buses.len() {
                        return Err(error("unknown CAN arbitration resource"));
                    }
                    c.request_arbitration("network").map_err(model)?;
                }
                BridgeAction::InjectEthernet {
                    source,
                    wire,
                    vid,
                    priority,
                    origin_id,
                    parent_id,
                    visited_gateways,
                    branch_lineage,
                    segment_id,
                    generated_ps,
                    branch_id,
                    child_id,
                } => {
                    let f = self.frames.len()
                        + d.frames
                            .iter()
                            .filter(|(i, _)| *i >= self.frames.len())
                            .count();
                    let device = &self.prepared.ethernet.devices[source];
                    let row = EthernetFrameRecord {
                        frame_id: child_id.clone(),
                        time_ps: c.now(),
                        source: device.id.clone(),
                        source_vlan_id: Some(vid),
                        wire: wire.clone(),
                        flow_id: Some(child_id.clone()),
                        priority,
                        deadline_ps: None,
                        generated_ps: c.now(),
                        ready_ps: Some(c.now()),
                    };
                    d.frames.push((
                        f,
                        Frame {
                            row,
                            ip: None,
                            visits: 0,
                            lineage: Some(BridgeLineage {
                                origin_id,
                                parent_id,
                                visited_gateways,
                                branch_lineage,
                                segment_id,
                                generated_ps,
                            }),
                            branch_id: Some(branch_id),
                        },
                    ));
                    let dirs = self
                        .prepared
                        .ethernet
                        .directions
                        .iter()
                        .enumerate()
                        .filter(|(_, v)| v.source == source)
                        .map(|(i, _)| i)
                        .collect::<Vec<_>>();
                    let prior = d.transfers.len();
                    self.offer(
                        CopyOrigin {
                            frame: f,
                            parent: None,
                            visit: 0,
                        },
                        &wire,
                        l2::Classification {
                            vid,
                            priority,
                            dei: wire.tag.map_or(0, |t| t.dei),
                        },
                        &dirs,
                        d,
                        c,
                    )?;
                    if d.transfers[prior..]
                        .iter()
                        .any(|(_, t)| t.row.status != "queued")
                    {
                        return Err(error(
                            "bridge reserved Ethernet admission disagrees with media queue",
                        ));
                    }
                }
                BridgeAction::Record {
                    schema,
                    record_id,
                    time_ps,
                    data,
                } => {
                    if time_ps != c.now() {
                        return Err(error("bridge record time mismatch"));
                    }
                    let data = if schema == "can.request" {
                        data
                    } else {
                        decimal(data)
                    };
                    Self::record(c, d, schema, record_id, data)?;
                }
            }
        }
        d.future_generation |= d.bridge.as_ref().unwrap().future_generation;
        d.bridge.as_mut().unwrap().clear_ethernet_reservations();
        Ok(())
    }
    fn ethernet_targets(&self, source: usize, vid: u16, wire: &EthernetWireFrame) -> Vec<String> {
        let base = &self.prepared.ethernet;
        // SOF freezes intended recipients. Admission and forwarding failures leave
        // those opportunities incomplete rather than removing the denominator.
        let mut targets: Vec<_> = base
            .devices
            .iter()
            .enumerate()
            .filter_map(|(index, device)| {
                let member = base
                    .port_policies
                    .iter()
                    .any(|policy| policy.device == index && policy.vlans.contains_key(&vid));
                (index != source
                    && device.kind == "endpoint"
                    && member
                    && (wire.dst_mac == "ff:ff:ff:ff:ff:ff"
                        || device.mac.as_ref() == Some(&wire.dst_mac)
                        || device.subscriptions.contains(&(vid, wire.dst_mac.clone()))))
                .then(|| device.id.clone())
            })
            .collect();
        targets.sort();
        targets
    }
    fn port_input(&self, direction: usize, d: &NetworkDelta) -> Result<PortInput> {
        let q = self.queue(d, direction);
        let link = &self.prepared.ethernet.directions[direction];
        let mut input = PortInput {
            policy_epoch: self.epoch_after(d),
            ..PortInput::default()
        };
        for p in 0..8 {
            input.backlog[p] = !q.waiting[p].is_empty();
            if let Some(&t) = q.waiting[p].front() {
                let row = self.transfer(d, t);
                let (_, release, _) = l2::timing(0, row.row.wire.mac_bytes, link.bitrate_bps, 0)?;
                input.heads[p] = Some(Head {
                    occupancy_ps: release,
                });
            }
        }
        if let Some(t) = q.active {
            let row = self.transfer(d, t);
            input.sending = Some(Sending {
                priority: row.row.priority,
                release_ps: row
                    .row
                    .planned_release_ps
                    .ok_or_else(|| error("missing sending release"))?,
            });
        }
        Ok(input)
    }
    fn stage_tsn(&self, d: &mut NetworkDelta, delta: TsnDelta, ports: &[String]) {
        if let Some(prepared) = &self.prepared.tsn {
            for port in ports {
                if let Some(&i) = prepared.port_index.get(port) {
                    d.tsn_wakes.insert(
                        i,
                        delta.wakes.iter().rev().find(|w| &w.port == port).cloned(),
                    );
                }
            }
        }
        d.tsn = delta;
    }
    fn refresh_tsn(&self, d: &mut NetworkDelta, c: &mut Context<'_>, control: bool) -> Result<()> {
        let Some(state) = &self.tsn else {
            return Ok(());
        };
        let prepared = self.prepared.tsn.as_ref().unwrap();
        let mut inputs = BTreeMap::new();
        for (i, o) in prepared.outputs.iter().enumerate() {
            let direction = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|v| v.from_port == o.port)
                .ok_or_else(|| error("unknown TSN direction"))?;
            let due = self.tsn_wakes[i]
                .as_ref()
                .is_some_and(|w| w.time_ps == c.now() && state.wake_valid(w));
            let update = prepared
                .updates
                .iter()
                .any(|u| u.port == o.port && u.effective_at_ps == c.now());
            let policy = d
                .dynamic
                .as_ref()
                .is_some_and(|v| v.dirty_ports.contains(&o.port));
            if d.queues.contains_key(&direction)
                || due
                || update
                || policy
                || (control && c.now() == 0)
            {
                inputs.insert(o.port.clone(), self.port_input(direction, d)?);
            }
        }
        if inputs.is_empty() {
            return Ok(());
        }
        let ports = inputs.keys().cloned().collect::<Vec<_>>();
        if control {
            let delta = state.plan_control(c.now(), &inputs)?;
            if !delta.selected.is_empty() {
                c.request_arbitration("network").map_err(model)?;
            }
            self.stage_tsn(d, delta, &ports);
        } else {
            for (port, input) in inputs {
                let delta =
                    state.plan_port_in(std::mem::take(&mut d.tsn), &port, c.now(), &input)?;
                if !delta.selected.is_empty() {
                    c.request_arbitration("network").map_err(model)?;
                }
                self.stage_tsn(d, delta, &[port]);
            }
        }
        Ok(())
    }
    fn emit_tsn(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let records = std::mem::take(&mut d.tsn.records);
        for record in records {
            let schema = record
                .schema()
                .strip_prefix("dir.")
                .unwrap_or(record.schema());
            let id = format!("{}:{}", schema, d.record_sequence);
            let value = decimal(record.data(d.record_sequence));
            Self::record(c, d, schema, id, value)?;
        }
        Ok(())
    }
    fn queue_points(&self, d: &mut NetworkDelta, now: u64) {
        if let (Some(state), Some(delta)) = (&self.bridge, &d.bridge) {
            for (target, value) in state.can_queue_occupancy_after(delta) {
                d.points.push(Point {
                    event_seq: None,
                    effect_seq: None,
                    time_ps: now,
                    target,
                    metric: "queue_length".into(),
                    value,
                    request_id: None,
                    receiver: None,
                    reason: None,
                });
            }
        }

        for (&i, q) in &d.queues {
            let port = &self.prepared.ethernet.directions[i].from_port;
            d.points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: now,
                target: format!("{port}.queue"),
                metric: "queue_length".into(),
                value: q.waiting.iter().map(|v| v.len() as u64).sum(),
                request_id: None,
                receiver: None,
                reason: None,
            });
            for p in 0..8 {
                for (metric, value) in [
                    ("queue_length", q.waiting[p].len() as u64),
                    ("queue_bytes", q.bytes[p]),
                ] {
                    d.points.push(Point {
                        event_seq: None,
                        effect_seq: None,
                        time_ps: now,
                        target: format!("{port}.queue.{p}"),
                        metric: metric.into(),
                        value,
                        request_id: None,
                        receiver: None,
                        reason: None,
                    });
                }
            }
        }
    }
    fn emit_rows(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let mut records = Vec::new();
        for (index, f) in &d.frames {
            if self.frames.get(*index).is_some_and(|old| {
                old.row.time_ps == f.row.time_ps && old.row.ready_ps == f.row.ready_ps
            }) {
                continue;
            }
            let row = &f.row;
            let mut v = wire(&row.wire);
            v["source"] = json!(row.source);
            v["generated_ps"] = json!(row.generated_ps.to_string());
            v["ready_ps"] = opt(row.ready_ps);
            v["flow_id"] = json!(row.flow_id);
            v["priority"] = json!(row.priority.to_string());
            v["deadline_ps"] = opt(row.deadline_ps);
            v["source_vlan_id"] = opt(row.source_vlan_id.map(u64::from));
            v["ip_multicast"] = decimal(json!(f.ip));
            if self.bridge.is_some() {
                v.as_object_mut().unwrap().remove("ip_multicast");
            }
            records.push((
                if self.bridge.is_some() {
                    "ethernet.frame"
                } else {
                    "ethernet.dynamic.frame"
                },
                row.frame_id.clone(),
                v,
            ));
        }
        for (_, t) in &d.transfers {
            let row = &t.row;
            let mut v = json!({"frame_id":row.frame_id,"parent_transfer_id":row.parent_transfer_id,"from_port":row.from_port,"to_port":row.to_port,"queued_ps":row.queued_ps.to_string(),"sof_ps":opt(row.sof_ps),"eof_ps":opt(row.eof_ps),"release_ps":opt(row.release_ps),"arrival_ps":opt(row.arrival_ps),"planned_eof_ps":opt(row.planned_eof_ps),"planned_release_ps":opt(row.planned_release_ps),"planned_arrival_ps":opt(row.planned_arrival_ps),"status":row.status,"drop_reason":row.drop_reason,"queue_id":row.queue_id,"priority":row.priority.to_string(),"vlan_id":opt(row.vlan_id.map(u64::from)),"wire":wire(&row.wire),"visit_id":t.visit.to_string(),"copy_id":row.transfer_id,"policy_epoch_offer":t.offer_epoch.to_string(),"policy_epoch_sof":opt(t.sof_epoch)});
            if self.bridge.is_some() {
                for k in [
                    "visit_id",
                    "copy_id",
                    "policy_epoch_offer",
                    "policy_epoch_sof",
                ] {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            records.push((
                if self.bridge.is_some() {
                    "ethernet.transfer"
                } else {
                    "ethernet.dynamic.transfer"
                },
                row.transfer_id.clone(),
                v,
            ));
        }
        for (_, r) in &d.receptions {
            let row = &r.row;
            let mut v = json!({"frame_id":row.frame_id,"transfer_id":row.transfer_id,"ingress":row.ingress,"observed_ps":row.observed_ps.to_string(),"ready_ps":opt(row.ready_ps),"planned_ready_ps":opt(row.planned_ready_ps),"status":row.status,"reason":row.reason,"egress_transfer_ids":row.egress_transfer_ids,"vlan_id":opt(row.vlan_id.map(u64::from)),"priority":row.priority.to_string(),"visit_id":r.visit.to_string(),"policy_epoch_ingress":r.epoch.to_string()});
            if self.bridge.is_some() {
                for k in ["visit_id", "policy_epoch_ingress"] {
                    v.as_object_mut().unwrap().remove(k);
                }
            }
            records.push((
                if self.bridge.is_some() {
                    "ethernet.reception"
                } else {
                    "ethernet.dynamic.reception"
                },
                row.reception_id.clone(),
                v,
            ));
        }
        for (s, id, v) in records {
            Self::record(c, d, s, id, v)?;
        }
        Ok(())
    }
    pub(crate) fn reserve(&mut self, d: &NetworkDelta) -> Result<()> {
        if let (Some(state), Some(delta)) = (&mut self.bridge, &d.bridge) {
            state.reserve(delta)?;
        }
        for (result, label) in [
            (
                self.frames.try_reserve(
                    d.frames
                        .iter()
                        .filter(|(i, _)| *i >= self.frames.len())
                        .count(),
                ),
                "frames",
            ),
            (
                self.transfers.try_reserve(
                    d.transfers
                        .iter()
                        .filter(|(i, _)| *i >= self.transfers.len())
                        .count(),
                ),
                "transfers",
            ),
            (
                self.receptions.try_reserve(
                    d.receptions
                        .iter()
                        .filter(|(i, _)| *i >= self.receptions.len())
                        .count(),
                ),
                "receptions",
            ),
        ] {
            result.map_err(|_| error(format!("network allocation failed: {label}")))?;
        }
        Ok(())
    }
    pub(crate) fn apply(&mut self, d: NetworkDelta) {
        if let Some(cursors) = d.cursors {
            self.cursors = cursors;
        }
        if let (Some(state), Some(delta)) = (&mut self.bridge, d.bridge) {
            state.apply(delta);
        }
        apply(&mut self.frames, d.frames);
        apply(&mut self.transfers, d.transfers);
        apply(&mut self.receptions, d.receptions);
        if let Some(tsn) = &mut self.tsn {
            tsn.apply(d.tsn);
        }
        for (i, wake) in d.tsn_wakes {
            self.tsn_wakes[i] = wake;
        }
        for (i, q) in d.queues {
            self.queues[i] = q;
        }
        if let Some(delta) = d.dynamic {
            self.dynamic.as_mut().unwrap().apply(delta);
        }
        if let Some(due) = d.due {
            self.due = due;
        }
        if let Some(control) = d.control {
            self.control = control;
        }
        self.control_generation = d.control_generation;
        self.record_sequence = d.record_sequence;
        self.next_visit = d.next_visit;
        self.future_generation = d.future_generation;
    }
    pub(crate) fn snapshot(&self) -> EthernetSnapshot {
        EthernetSnapshot {
            frames: self.frames.iter().map(|f| f.row.clone()).collect(),
            transfers: self.transfers.iter().map(|t| t.row.clone()).collect(),
            receptions: self.receptions.iter().map(|r| r.row.clone()).collect(),
            attempts: vec![],
        }
    }
    pub(crate) fn runtime_snapshot(&self, h: u64) -> Result<Value> {
        let tsn=self.tsn.as_ref().map(|s|s.snapshot(h)).transpose()?.map(|s|json!({"ports":s.ports.into_iter().map(|p|json!({"port":p.port,"schedule_id":p.schedule_id,"schedule_generation":p.schedule_generation.to_string(),"wake_generation":p.wake_generation.to_string(),"next_wake_ps":p.next_wake_ps.map(|v|v.to_string()),"credits":p.credits.into_iter().map(|c|json!({"priority":c.priority.to_string(),"credit":decimal(json!(c.credit)),"last_ps":c.last_ps.to_string(),"mode":c.mode,"backlog":c.backlog,"sending":c.sending})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"meters":s.meters.into_iter().map(|m|json!({"stream_id":m.stream_id,"committed":m.committed.to_string(),"peak":m.peak.to_string(),"last_evaluated_ps":m.last_evaluated_ps.to_string()})).collect::<Vec<_>>(),"update_cursor":s.update_cursor.to_string()}));
        Ok(
            json!({"dynamic":self.dynamic.as_ref().map(|s|decimal(s.snapshot_tables())),"tsn":tsn,"bridge":self.bridge.as_ref().map(|b|decimal(b.snapshot()))}),
        )
    }
    pub(crate) fn can_snapshot(&self) -> Option<crate::snapshot::CanSnapshot> {
        self.bridge.as_ref().map(BridgeState::can_snapshot)
    }
    fn next_generation(&self, cursors: &[u64]) -> Option<u128> {
        self.prepared
            .ethernet
            .generators
            .iter()
            .zip(cursors)
            .filter_map(|(g, &ordinal)| g.time(ordinal))
            .min()
    }
    pub(crate) fn future_generation(&self) -> bool {
        self.future_generation
    }
}
fn put<T>(rows: &mut Vec<(usize, T)>, i: usize, value: T) {
    if let Some((_, v)) = rows.iter_mut().find(|(n, _)| *n == i) {
        *v = value
    } else {
        rows.push((i, value));
    }
}
fn apply<T>(target: &mut Vec<T>, rows: Vec<(usize, T)>) {
    for (i, row) in rows {
        if i == target.len() {
            target.push(row)
        } else {
            target[i] = row
        }
    }
}
fn opt(v: Option<u64>) -> Value {
    v.map_or(Value::Null, |n| json!(n.to_string()))
}
fn wire(w: &EthernetWireFrame) -> Value {
    decimal(json!(w))
}
pub(crate) fn decimal(v: Value) -> Value {
    match v {
        Value::Number(n) => Value::String(n.to_string()),
        Value::Array(a) => Value::Array(a.into_iter().map(decimal).collect()),
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, decimal(v))).collect()),
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared(example: &str) -> crate::types::PreparedSimulation {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(example),
        )
        .unwrap()
    }
    #[test]
    fn intended_terminal_survives_ingress_rejection_without_completion() {
        let mut p = prepared("examples/can-ethernet/r01.ini");
        let network = Arc::make_mut(p.registered.as_mut().unwrap().network.as_mut().unwrap());
        let sink = network
            .ethernet
            .devices
            .iter()
            .position(|device| device.id == "R01.sink")
            .unwrap();
        let policy = network
            .ethernet
            .port_policies
            .iter_mut()
            .find(|policy| policy.device == sink)
            .unwrap();
        policy.admit = "tagged_only".into();
        let snapshot = super::super::registered::simulate(&p).unwrap();
        assert_eq!(snapshot.common.termination, "events_exhausted");
        let records = &snapshot.registered.as_ref().unwrap().model_records;
        let segment = records
            .values()
            .find(|row| {
                row.schema.name == "dir.can_ethernet.segment"
                    && row.data["source_record_id"].as_str().is_some_and(|id| {
                        snapshot
                            .ethernet
                            .as_ref()
                            .unwrap()
                            .frames
                            .iter()
                            .any(|frame| frame.frame_id == id)
                    })
            })
            .unwrap();
        assert_eq!(segment.data["target_count"], "1");
        assert_eq!(segment.data["targets"][0]["terminal_id"], "R01.sink");
        assert_eq!(segment.data["targets"][0]["completed_ps"], Value::Null);
        assert_eq!(
            segment.data["targets"][0]["completion_reception_id"],
            Value::Null
        );
        assert!(
            snapshot
                .ethernet
                .as_ref()
                .unwrap()
                .receptions
                .iter()
                .any(|row| row.device == "R01.sink"
                    && row.status == "filtered"
                    && row.reason.as_deref() == Some("ingress_frame_type"))
        );
    }
    #[test]
    fn intended_terminal_set_uses_source_vid_and_ignores_static_misrouting() {
        let p = prepared("examples/ethernet/dynamic/unicast.ini");
        let mut network = (**p.registered.as_ref().unwrap().network.as_ref().unwrap()).clone();
        let source = network.ethernet.generators[0].source;
        let wire = network.ethernet.generators[0].frame.clone();
        let sink = network
            .ethernet
            .devices
            .iter()
            .position(|device| device.mac.as_ref() == Some(&wire.dst_mac))
            .unwrap();
        let expected = network.ethernet.devices[sink].id.clone();
        let incoming = network
            .ethernet
            .directions
            .iter()
            .position(|direction| direction.source == source)
            .unwrap();
        let switch = network.ethernet.directions[incoming].destination;
        let ingress_output = network
            .ethernet
            .port_policies
            .iter()
            .find(|policy| policy.ingress == network.ethernet.directions[incoming].to_port)
            .unwrap()
            .port
            .clone();
        network.ethernet.devices[switch]
            .vlan_fdb
            .insert((10, wire.dst_mac.clone()), ingress_output);
        assert!(
            l2::static_egress(&network.ethernet, incoming, &wire, Some(10))
                .unwrap()
                .0
                .is_empty()
        );
        let state = NetworkState::new(Arc::new(network.clone())).unwrap();
        assert_eq!(
            state.ethernet_targets(source, 10, &wire),
            vec![expected.clone()]
        );
        // Neither tag acceptance nor receiving PVID changes source-VID membership.
        let policy = network
            .ethernet
            .port_policies
            .iter_mut()
            .find(|policy| policy.device == sink)
            .unwrap();
        policy.pvid = 20;
        policy.admit = "untagged_only".into();
        let state = NetworkState::new(Arc::new(network.clone())).unwrap();
        assert_eq!(state.ethernet_targets(source, 10, &wire), vec![expected]);
        assert!(state.ethernet_targets(source, 20, &wire).is_empty());
        let mut broadcast = wire.clone();
        broadcast.dst_mac = "ff:ff:ff:ff:ff:ff".into();
        let targets = state.ethernet_targets(source, 10, &broadcast);
        assert!(!targets.contains(&state.prepared.ethernet.devices[source].id));
        assert_eq!(targets.len(), 2);
    }
}
