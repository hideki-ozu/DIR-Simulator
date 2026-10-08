//! Sparse transaction coordinator with phase cohorts and shared memory service.
use crate::snapshot::{Snapshot, memory_ipc::*};
use crate::types::{Diagnostic, PreparedSimulation, memory_ipc::*};
use std::collections::{BTreeMap, BTreeSet};
mod protocol;
#[derive(Clone)]
struct Queued {
    event: Event,
    body: Option<Vec<u8>>,
}
type Result<T> = std::result::Result<T, Diagnostic>;
type Key = (u64, u64, u8, u8, usize, u64, u8, u64, u8, u64);
#[derive(Debug, Clone)]
enum Event {
    Offer(usize, u64),
    Child(usize, bool),
    Admit,
    Dispatch(usize),
    Complete(usize, usize),
    RefreshDue(usize),
    RefreshEnd(usize),
    Setup(usize),
    Response(usize),
    DmaNotify(usize),
    MailNotify(usize),
}
fn limit(s: &str) -> Diagnostic {
    Diagnostic {
        schema_version: 1,
        code: "E-0004".into(),
        stage: "run".into(),
        message: s.into(),
        details: None,
        ..Diagnostic::execution("").with_reason("event_limit")
    }
}
fn add(a: u64, b: u64) -> Result<u64> {
    u64::try_from(a as u128 + b as u128).map_err(|_| limit("memory/IPC timestamp overflow"))
}
fn sum(a: u64, b: u64) -> Result<u64> {
    add(a, b)
}
struct Engine<'a> {
    p: &'a PreparedSimulation,
    m: &'a PreparedMemoryIpc,
    s: Snapshot,
    current_event_sequence: u64,
    effect: u64,
    events: BTreeMap<Key, Queued>,
    scheduled: BTreeSet<(u64, u64, u8, u8, usize)>,
    sequence: u64,
    now: u64,
    delta: u64,
    phase: u8,
    intents: Vec<usize>,
}
impl Engine<'_> {
    fn st(&self) -> &MemoryIpcSnapshot {
        self.s.memory_ipc.as_ref().unwrap()
    }
    fn st_mut(&mut self) -> &mut MemoryIpcSnapshot {
        self.s.memory_ipc.as_mut().unwrap()
    }
    fn plan(&self, time: u64, phase: u8) -> Result<u64> {
        if time < self.now {
            return Err(Diagnostic::execution(
                "memory/IPC event before current time",
            ));
        }
        let delta = if time == self.now {
            if self.phase == 2 && phase == 1 {
                self.delta
                    .checked_add(1)
                    .ok_or_else(|| limit("memory/IPC delta overflow"))?
            } else {
                self.delta
            }
        } else {
            0
        };
        if delta >= self.p.common.max_delta_cycles {
            return Err(limit("memory/IPC max-delta-cycles exceeded"));
        }
        self.sequence
            .checked_add(1)
            .ok_or_else(|| limit("memory/IPC sequence overflow"))?;
        Ok(delta)
    }
    fn codec(&self, event: &Event, generation: u64, _time: u64) -> Option<protocol::Body> {
        let (kind, node, request) = match *event {
            Event::Admit | Event::Dispatch(_) => return None,
            Event::Offer(g, ordinal) => {
                let g = &self.m.generators[g];
                ("Offer", g.node, Some(format!("{}:{ordinal}", g.id)))
            }
            Event::Child(i, write) => {
                let q = &self.st().requests[i];
                let chunk = self.st().resources[q.node].chunk_index;
                (
                    "Offer",
                    if write {
                        q.spec.dst.unwrap()
                    } else {
                        q.spec.src.unwrap()
                    },
                    Some(format!(
                        "{}/{}/{chunk}",
                        q.id,
                        if write { "w" } else { "r" }
                    )),
                )
            }
            Event::Complete(n, i) => ("Complete", n, Some(self.st().requests[i].id.clone())),
            Event::RefreshDue(n) => ("RefreshDue", n, None),
            Event::RefreshEnd(n) => ("RefreshEnd", n, None),
            Event::Setup(i) => (
                "DmaSetup",
                self.st().requests[i].node,
                Some(self.st().requests[i].id.clone()),
            ),
            Event::Response(i) => {
                let q = &self.st().requests[i];
                (
                    "DmaResponse",
                    self.st().requests[q.origin.unwrap()].node,
                    Some(q.id.clone()),
                )
            }
            Event::DmaNotify(i) => (
                "DmaNotify",
                self.st().requests[i].node,
                Some(self.st().requests[i].id.clone()),
            ),
            Event::MailNotify(i) => {
                let n = &self.st().notifications[i];
                ("MailNotify", n.node, Some(n.id.clone()))
            }
        };
        Some(protocol::Body {
            generation: generation.to_string(),
            kind: kind.into(),
            node: self.m.resources[node].node.clone(),
            request_id: request,
        })
    }
    fn validate_codec(&self, key: Key, queued: &Queued) -> Result<()> {
        let expected = self.codec(&queued.event, key.9, key.0);
        match (expected, queued.body.as_ref()) {
            (None, None) => return Ok(()),
            (Some(expected), Some(bytes)) => {
                let body = protocol::Body::decode(
                    bytes,
                    &format!("dir.memory-ipc.transaction.{}", expected.kind),
                )?;
                if body == expected {
                    let valid = match queued.event {
                        Event::Complete(n, i) => {
                            let q = &self.st().requests[i];
                            let st = &self.st().resources[n];
                            q.status == "active"
                                && q.planned == Some(key.0)
                                && (st.active == Some(i) || st.ports.contains(&Some(i)))
                        }
                        Event::Setup(i) => {
                            let q = &self.st().requests[i];
                            q.status == "setup"
                                && q.planned == Some(key.0)
                                && self.st().resources[q.node].active == Some(i)
                        }
                        Event::Response(i) => {
                            let q = &self.st().requests[i];
                            let p = q.origin.unwrap();
                            matches!(q.status.as_str(), "completed" | "rejected")
                                && !q.response_consumed
                                && self.st().resources[self.st().requests[p].node].active == Some(p)
                        }
                        Event::DmaNotify(i) => {
                            let q = &self.st().requests[i];
                            q.status == "notifying"
                                && q.planned_notify == Some(key.0)
                                && self.st().resources[q.node].active == Some(i)
                        }
                        Event::MailNotify(i) => {
                            let v = &self.st().notifications[i];
                            v.planned == key.0 && v.delivered.is_none()
                        }
                        Event::RefreshEnd(n) => {
                            let r = &self.st().resources[n];
                            r.refresh_planned_end == Some(key.0) && r.refresh_ended.is_none()
                        }
                        Event::Offer(g, ordinal) => {
                            self.m.generators[g].times.get(ordinal as usize) == Some(&key.0)
                        }
                        Event::Child(i, _) => {
                            let q = &self.st().requests[i];
                            matches!(q.status.as_str(), "setup" | "reading" | "writing")
                                && self.st().resources[q.node].active == Some(i)
                        }
                        _ => true,
                    };
                    if valid {
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
        let mut d = Diagnostic::execution("memory/IPC transaction event/owner/generation mismatch");
        d.details = Some(serde_json::json!({"kind":"model_failed"}));
        Err(d)
    }
    fn schedule(
        &mut self,
        time: u64,
        phase: u8,
        rank: u8,
        node: usize,
        event: Event,
    ) -> Result<()> {
        let delta = self.plan(time, phase)?;
        let generation = self.sequence + 1;
        let body = self
            .codec(&event, generation, time)
            .map(|b| b.encode())
            .transpose()?;
        self.sequence = generation;
        let (order_node, ordinal, source_rank, chunk, operation) = match event {
            Event::Offer(g, ordinal) => (g, ordinal, 0, 0, 0),
            Event::Child(i, write) => {
                let p = &self.st().requests[i];
                (
                    p.generator,
                    p.ordinal,
                    1,
                    self.st().resources[p.node].chunk_index,
                    u8::from(write),
                )
            }
            _ => (node, 0, 0, 0, 0),
        };
        self.events.insert(
            (
                time,
                delta,
                phase,
                rank,
                order_node,
                ordinal,
                source_rank,
                chunk,
                operation,
                self.sequence,
            ),
            Queued { event, body },
        );
        Ok(())
    }
    fn unique(&mut self, time: u64, phase: u8, rank: u8, node: usize, event: Event) -> Result<()> {
        let delta = self.plan(time, phase)?;
        if self.scheduled.insert((time, delta, phase, rank, node)) {
            self.schedule(time, phase, rank, node, event)?;
        }
        Ok(())
    }
    fn dirty(&mut self, node: usize) -> Result<()> {
        self.unique(self.now, 2, 1, node, Event::Dispatch(node))
    }
    fn point(
        &mut self,
        node: usize,
        metric: &str,
        value: u64,
        request: Option<String>,
        initial: bool,
    ) {
        let event_seq = (!initial).then_some(self.current_event_sequence);
        let effect_seq = (!initial).then_some(self.effect);
        if !initial {
            self.effect += 1;
        }
        self.s.common.points.push(crate::snapshot::Point {
            event_seq,
            effect_seq,
            time_ps: self.now,
            target: self.m.resources[node].node.clone(),
            metric: metric.into(),
            value,
            request_id: request,
            receiver: None,
            reason: None,
        });
    }
    fn queue_point(&mut self, node: usize, request: Option<String>) {
        let time = self.now;
        let value = self.st().resources[node].queue.len();
        let event = self.current_event_sequence;
        self.point(
            node,
            "memory_ipc.queue_length",
            value as u64,
            request.clone(),
            request.is_none(),
        );
        self.st_mut().queues.push(QueuePoint {
            node,
            time,
            value,
            request,
            event,
        });
    }
    #[allow(clippy::too_many_arguments)]
    fn new_row(
        &self,
        id: String,
        node: usize,
        g: usize,
        ordinal: u64,
        origin: Option<usize>,
        chunk: u64,
        spec: Request,
    ) -> RequestRow {
        RequestRow {
            id,
            node,
            generator: g,
            ordinal,
            origin,
            chunk,
            spec,
            time: self.now,
            generated: self.now,
            started: None,
            planned: None,
            completed: None,
            status: "awaiting_admission".into(),
            reason: None,
            output: None,
            slot: None,
            port: None,
            dispatch: None,
            bank: None,
            row: None,
            row_hit: None,
            message: None,
            committed: 0,
            data_done: None,
            planned_notify: None,
            notified: None,
            response_consumed: false,
        }
    }
    fn offer(&mut self, row: RequestRow) -> Result<()> {
        self.plan(self.now, 2)?;
        let i = self.st().requests.len();
        let n = row.node;
        self.st_mut().requests.push(row);
        self.intents.push(i);
        self.st_mut().resources[n].time = self.now;
        self.unique(self.now, 2, 0, 0, Event::Admit)
    }
    fn in_range(&self, n: usize, a: u64, l: u64, row: bool) -> bool {
        let r = &self.m.resources[n];
        a as u128 + l as u128 <= r.size as u128
            && (!row || r.kind != Kind::Ddr || a % r.row_bytes + l <= r.row_bytes)
    }
    fn rejection(&self, i: usize) -> Option<&'static str> {
        let q = &self.st().requests[i];
        let r = &self.m.resources[q.node];
        let s = &q.spec;
        match r.kind {
            Kind::Ddr | Kind::Sram => {
                if !self.in_range(q.node, s.address.unwrap(), s.length.unwrap(), true) {
                    Some("address_error")
                } else {
                    None
                }
            }
            Kind::Shared | Kind::Mailbox => {
                let set = if matches!(s.op.as_str(), "publish" | "send") {
                    &r.producers
                } else {
                    &r.consumers
                };
                if !set.contains(s.actor.as_ref().unwrap()) {
                    Some("access_denied")
                } else {
                    None
                }
            }
            Kind::Dma => {
                let src = s.src.unwrap();
                let dst = s.dst.unwrap();
                let sa = s.src_address.unwrap();
                let da = s.dst_address.unwrap();
                let l = s.length.unwrap();
                if !self.in_range(src, sa, l, false)
                    || !self.in_range(dst, da, l, false)
                    || src == dst
                        && (sa as u128) < da as u128 + l as u128
                        && (da as u128) < sa as u128 + l as u128
                {
                    Some("address_error")
                } else {
                    None
                }
            }
        }
    }
    fn finish(&mut self, i: usize, status: &str, reason: &str) {
        let now = self.now;
        let q = &mut self.st_mut().requests[i];
        q.time = now;
        q.status = status.into();
        q.reason = Some(reason.into());
        q.completed = Some(now);
        let node = q.node;
        let id = q.id.clone();
        let latency = now - q.generated;
        self.point(node, "memory_ipc.latency_ps", latency, Some(id), false);
    }
    fn respond(&mut self, i: usize) -> Result<()> {
        if self.st().requests[i].origin.is_some() {
            self.schedule(self.now, 1, 0, 0, Event::Response(i))?;
        }
        Ok(())
    }
    fn admit(&mut self) -> Result<()> {
        let mut intents = self.intents.clone();
        intents.sort_by_key(|&i| {
            let q = &self.st().requests[i];
            (
                self.m.generators[q.generator].id.clone(),
                q.ordinal,
                u8::from(q.origin.is_some()),
                q.chunk,
                u8::from(q.spec.op == "write"),
            )
        });
        let mut lengths = self
            .st()
            .resources
            .iter()
            .map(|r| r.queue.len())
            .collect::<Vec<_>>();
        for &i in &intents {
            let q = &self.st().requests[i];
            let n = q.node;
            let rejected =
                self.rejection(i).is_some() || lengths[n] >= self.m.resources[n].queue_capacity;
            if rejected {
                if q.origin.is_some() {
                    self.plan(self.now, 1)?;
                }
            } else {
                lengths[n] += 1;
            }
        }
        self.intents.clear();
        for i in intents {
            let n = self.st().requests[i].node;
            let reason = self.rejection(i).or_else(|| {
                (self.st().resources[n].queue.len() >= self.m.resources[n].queue_capacity)
                    .then_some("queue_full")
            });
            if let Some(reason) = reason {
                self.finish(i, "rejected", reason);
                self.respond(i)?;
            } else {
                self.st_mut().requests[i].status = "queued".into();
                self.st_mut().resources[n].queue.push_back(i);
            }
            self.st_mut().resources[n].time = self.now;
            self.queue_point(n, Some(self.st().requests[i].id.clone()));
            self.dirty(n)?;
        }
        Ok(())
    }
    fn fault(&self, i: usize) -> bool {
        let q = &self.st().requests[i];
        let a = q.spec.address.unwrap();
        let l = q.spec.length.unwrap();
        self.m.resources[q.node]
            .faults
            .iter()
            .any(|&(at, len)| a < at + len && at < a + l)
    }
    fn refresh(&mut self, n: usize) -> Result<bool> {
        let r = &self.m.resources[n];
        let s = &self.st().resources[n];
        if !s.refresh_pending || s.ports[0].is_some() {
            return Ok(false);
        }
        let end = add(self.now, r.times["refresh_ps"])?;
        let due = add(self.now, r.times["refresh_interval_ps"])?;
        self.plan(end, 0)?;
        self.plan(due, 0)?;
        let now = self.now;
        let s = &mut self.st_mut().resources[n];
        s.time = now;
        s.refresh_pending = false;
        s.open_rows.fill(None);
        s.refresh_started = Some(now);
        s.refresh_planned_end = Some(end);
        s.refresh_ended = None;
        s.refreshes += 1;
        self.schedule(end, 0, 0, n, Event::RefreshEnd(n))?;
        self.schedule(due, 0, 0, n, Event::RefreshDue(n))?;
        Ok(true)
    }
    fn dispatch(&mut self, n: usize) -> Result<()> {
        let kind = self.m.resources[n].kind;
        if kind == Kind::Ddr {
            if self.refresh(n)? {
                return Ok(());
            }
            let s = &self.st().resources[n];
            if s.refresh_pending || s.refresh_planned_end.is_some() && s.refresh_ended.is_none() {
                return Ok(());
            }
        }
        {
            let s = &self.st().resources[n];
            let port = if matches!(kind, Kind::Ddr | Kind::Sram) {
                s.ports.iter().position(Option::is_none)
            } else {
                s.active.is_none().then_some(0)
            };
            let Some(port) = port else {
                return Ok(());
            };
            let Some(&i) = s.queue.front() else {
                return Ok(());
            };
            let spec = self.st().requests[i].spec.clone();
            let immediate = if matches!(kind, Kind::Ddr | Kind::Sram) && self.fault(i) {
                Some("memory_fault")
            } else if kind == Kind::Shared {
                if spec.op == "publish" && s.slots.iter().all(|s| s.state != "free") {
                    Some("full")
                } else if spec.op == "consume" && s.ready.is_empty() {
                    Some("empty")
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(reason) = immediate {
                if self.st().requests[i].origin.is_some() {
                    self.plan(self.now, 1)?;
                }
                self.st_mut().resources[n].queue.pop_front();
                self.st_mut().resources[n].time = self.now;
                self.st_mut().requests[i].started = Some(self.now);
                self.finish(i, "rejected", reason);
                self.respond(i)?;
                self.queue_point(n, Some(self.st().requests[i].id.clone()));
                return self.dirty(n);
            }
            let r = &self.m.resources[n];
            let mut bank = None;
            let mut row = None;
            let mut hit = None;
            let slot = if kind == Kind::Shared {
                Some(if spec.op == "publish" {
                    s.slots.iter().position(|s| s.state == "free").unwrap()
                } else {
                    *s.ready.front().unwrap()
                })
            } else {
                None
            };
            let latency = match kind {
                Kind::Ddr => {
                    let a = spec.address.unwrap();
                    let b = (a / r.row_bytes) % r.banks as u64;
                    let rr = a / (r.row_bytes * r.banks as u64);
                    let old = s.open_rows[b as usize];
                    bank = Some(b as usize);
                    row = Some(rr);
                    hit = Some(old == Some(rr));
                    let setup = if old == Some(rr) {
                        0
                    } else if old.is_none() {
                        r.times["open_ps"]
                    } else {
                        sum(r.times["close_ps"], r.times["open_ps"])?
                    };
                    let beats = spec.length.unwrap().div_ceil(r.width_bytes) as u128;
                    u64::try_from(
                        setup as u128
                            + r.times["column_ps"] as u128
                            + beats * r.times["beat_ps"] as u128,
                    )
                    .map_err(|_| limit("DDR service duration overflow"))?
                }
                Kind::Sram => {
                    r.times[if spec.op == "read" {
                        "read_ps"
                    } else {
                        "write_ps"
                    }]
                }
                Kind::Shared => {
                    r.times[if spec.op == "publish" {
                        "publish_ps"
                    } else {
                        "consume_ps"
                    }]
                }
                Kind::Dma => r.times["setup_ps"],
                Kind::Mailbox => r.times["service_ps"],
            };
            let end = add(self.now, latency)?;
            self.plan(end, 0)?;
            let ordinal = s
                .next_dispatch
                .checked_add(1)
                .ok_or_else(|| limit("dispatch ordinal overflow"))?;
            let now = self.now;
            let request_id = self.st().requests[i].id.clone();
            {
                let s = &mut self.st_mut().resources[n];
                s.queue.pop_front();
                s.time = now;
                s.next_dispatch = ordinal;
                if matches!(kind, Kind::Ddr | Kind::Sram) {
                    s.ports[port] = Some(i);
                } else {
                    s.active = Some(i);
                }
                if let (Some(b), Some(row)) = (bank, row) {
                    s.open_rows[b] = Some(row);
                }
                if let Some(slot) = slot {
                    let sl = &mut s.slots[slot];
                    sl.owner = spec.actor.clone();
                    if spec.op == "publish" {
                        sl.state = "publishing".into();
                        sl.message = Some(request_id.clone());
                    } else {
                        sl.state = "consuming".into();
                        s.ready.pop_front();
                    }
                }
            }
            {
                let q = &mut self.st_mut().requests[i];
                q.time = now;
                q.started = Some(now);
                q.planned = Some(end);
                q.status = if kind == Kind::Dma { "setup" } else { "active" }.into();
                q.dispatch = Some(ordinal - 1);
                q.port = matches!(kind, Kind::Sram).then_some(port);
                q.bank = bank;
                q.row = row;
                q.row_hit = hit;
                q.slot = slot;
            }
            self.queue_point(n, Some(self.st().requests[i].id.clone()));
            if kind == Kind::Dma {
                self.schedule(end, 0, 0, n, Event::Setup(i))?;
            } else {
                self.unique(end, 0, 1, n, Event::Complete(n, i))?;
            }
            self.dirty(n)
        }
    }
    fn complete(&mut self, n: usize) -> Result<()> {
        let kind = self.m.resources[n].kind;
        let mut owners = if matches!(kind, Kind::Ddr | Kind::Sram) {
            self.st().resources[n]
                .ports
                .iter()
                .flatten()
                .copied()
                .filter(|&i| self.st().requests[i].planned == Some(self.now))
                .collect::<Vec<_>>()
        } else {
            self.st().resources[n].active.into_iter().collect()
        };
        owners.sort_by_key(|&i| self.st().requests[i].dispatch);
        // Every follow-up reservation is checked before the cohort publishes any bytes.
        for &i in &owners {
            let q = &self.st().requests[i];
            if q.origin.is_some() {
                self.plan(self.now, 1)?;
                if q.spec.op == "write" {
                    let parent = &self.st().requests[q.origin.unwrap()];
                    if parent.committed + q.spec.length.unwrap() == parent.spec.length.unwrap() {
                        add(self.now, self.m.resources[parent.node].times["notify_ps"])?;
                    }
                }
            }
            if kind == Kind::Mailbox
                && q.spec.op == "send"
                && self.st().resources[n].messages.len() < self.m.resources[n].capacity
            {
                let planned = add(self.now, self.m.resources[n].times["notify_ps"])?;
                self.plan(planned, 1)?;
            }
        }
        for i in owners {
            let spec = self.st().requests[i].spec.clone();
            let now = self.now;
            match kind {
                Kind::Ddr | Kind::Sram => {
                    let a = spec.address.unwrap() as usize;
                    let l = spec.length.unwrap() as usize;
                    if spec.op == "write" {
                        self.st_mut().resources[n].bytes[a..a + l]
                            .copy_from_slice(spec.bytes.as_ref().unwrap());
                    } else {
                        let b = self.st().resources[n].bytes[a..a + l].to_vec();
                        self.st_mut().requests[i].output = Some(b);
                    }
                    if let Some(parent) = self.st().requests[i].origin {
                        let pn = self.st().requests[parent].node;
                        self.st_mut().resources[pn].time = now;
                        if spec.op == "write" {
                            let notify = if self.st().requests[parent].committed + l as u64
                                == self.st().requests[parent].spec.length.unwrap()
                            {
                                Some(add(now, self.m.resources[pn].times["notify_ps"])?)
                            } else {
                                None
                            };
                            let q = &mut self.st_mut().requests[parent];
                            q.time = now;
                            q.committed += l as u64;
                            if q.committed == q.spec.length.unwrap() {
                                q.data_done = Some(now);
                                q.planned_notify = notify;
                                q.status = "notifying".into();
                            }
                        } else {
                            self.st_mut().resources[pn].chunk_hex =
                                self.st().requests[i].output.clone();
                        }
                    }
                    let port = self.st().resources[n]
                        .ports
                        .iter()
                        .position(|p| *p == Some(i))
                        .unwrap();
                    self.st_mut().resources[n].ports[port] = None;
                    self.finish(i, "completed", "ok");
                    self.respond(i)?;
                }
                Kind::Shared => {
                    let slot = self.st().requests[i].slot.unwrap();
                    if spec.op == "publish" {
                        let s = &mut self.st_mut().resources[n];
                        s.slots[slot].bytes = spec.bytes.unwrap();
                        s.slots[slot].state = "ready".into();
                        s.slots[slot].owner = None;
                        s.ready.push_back(slot);
                    } else {
                        self.st_mut().requests[i].output =
                            Some(self.st().resources[n].slots[slot].bytes.clone());
                        let sl = &mut self.st_mut().resources[n].slots[slot];
                        sl.state = "free".into();
                        sl.owner = None;
                        sl.message = None;
                        sl.bytes.clear();
                    }
                    self.finish(i, "completed", "ok");
                    self.st_mut().resources[n].active = None;
                }
                Kind::Mailbox => {
                    if spec.op == "send" {
                        if self.st().resources[n].messages.len() < self.m.resources[n].capacity {
                            let id = self.st().requests[i].id.clone();
                            let planned = add(now, self.m.resources[n].times["notify_ps"])?;
                            self.st_mut().resources[n].messages.push_back(Message {
                                id: id.clone(),
                                bytes: spec.bytes.unwrap(),
                                enqueued: now,
                            });
                            self.st_mut().requests[i].message = Some(id.clone());
                            let index = self.st().notifications.len();
                            self.st_mut().notifications.push(Notification {
                                node: n,
                                id,
                                enqueued: now,
                                planned,
                                delivered: None,
                            });
                            self.schedule(planned, 1, 0, n, Event::MailNotify(index))?;
                            self.finish(i, "completed", "ok");
                        } else {
                            self.finish(i, "completed", "full");
                        }
                    } else if let Some(m) = self.st_mut().resources[n].messages.pop_front() {
                        self.st_mut().requests[i].output = Some(m.bytes);
                        self.st_mut().requests[i].message = Some(m.id);
                        self.finish(i, "completed", "ok");
                    } else {
                        self.finish(i, "completed", "empty");
                    }
                    self.st_mut().resources[n].active = None;
                }
                Kind::Dma => unreachable!(),
            }
            self.st_mut().resources[n].time = now;
        }
        self.dirty(n)
    }
    fn child(&mut self, parent: usize, write: bool) -> Result<()> {
        let p = &self.st().requests[parent];
        let n = p.node;
        let spec = &p.spec;
        let offset = p.committed;
        let mut l = self.m.resources[n]
            .chunk_bytes
            .min(spec.length.unwrap() - offset);
        for (mem, a) in [
            (spec.src.unwrap(), spec.src_address.unwrap() + offset),
            (spec.dst.unwrap(), spec.dst_address.unwrap() + offset),
        ] {
            let r = &self.m.resources[mem];
            if r.kind == Kind::Ddr {
                l = l.min(r.row_bytes - a % r.row_bytes);
            }
        }
        let target = if write {
            spec.dst.unwrap()
        } else {
            spec.src.unwrap()
        };
        let request = Request {
            op: if write { "write" } else { "read" }.into(),
            address: Some(
                if write {
                    spec.dst_address.unwrap()
                } else {
                    spec.src_address.unwrap()
                } + offset,
            ),
            length: Some(l),
            bytes: write.then(|| self.st().resources[n].chunk_hex.clone().unwrap()),
            ..Default::default()
        };
        let chunk = self.st().resources[n].chunk_index;
        let id = format!("{}/{}/{chunk}", p.id, if write { "w" } else { "r" });
        let row = self.new_row(
            id,
            target,
            p.generator,
            p.ordinal,
            Some(parent),
            chunk,
            request,
        );
        let index = self.st().requests.len();
        self.offer(row)?;
        self.st_mut().resources[n].child = Some(index);
        self.st_mut().resources[n].time = self.now;
        self.st_mut().requests[parent].status = if write { "writing" } else { "reading" }.into();
        self.st_mut().requests[parent].time = self.now;
        Ok(())
    }
    fn response(&mut self, i: usize) -> Result<()> {
        let child = &self.st().requests[i];
        if child.response_consumed {
            return Err(Diagnostic::execution("DMA response consumed twice"));
        }
        let parent = child
            .origin
            .ok_or_else(|| Diagnostic::execution("DMA response missing parent"))?;
        let n = self.st().requests[parent].node;
        let reason = child.reason.clone().unwrap();
        let write = child.spec.op == "write";
        if reason == "ok" {
            if write && self.st().requests[parent].data_done.is_some() {
                self.plan(self.st().requests[parent].planned_notify.unwrap(), 1)?;
            } else {
                self.plan(self.now, 1)?;
            }
        }
        self.st_mut().requests[i].response_consumed = true;
        self.st_mut().resources[n].time = self.now;
        if reason != "ok" {
            self.finish(parent, "failed", &format!("child_{reason}"));
            let now = self.now;
            let s = &mut self.st_mut().resources[n];
            s.active = None;
            s.child = None;
            s.chunk_index = 0;
            s.chunk_hex = None;
            s.time = now;
            self.dirty(n)?;
        } else if write {
            if let Some(time) = self.st().requests[parent].planned_notify {
                self.schedule(time, 1, 0, n, Event::DmaNotify(parent))?;
            } else {
                self.st_mut().resources[n].chunk_index += 1;
                self.st_mut().resources[n].chunk_hex = None;
                self.schedule(self.now, 1, 0, n, Event::Child(parent, false))?;
            }
        } else {
            self.schedule(self.now, 1, 0, n, Event::Child(parent, true))?;
        }
        Ok(())
    }
    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Offer(g, ordinal) => {
                let generator = &self.m.generators[g];
                if let Some(&next) = generator.times.get(ordinal as usize + 1) {
                    self.plan(next, 1)?;
                }
                let row = self.new_row(
                    format!("{}:{ordinal}", generator.id),
                    generator.node,
                    g,
                    ordinal,
                    None,
                    0,
                    generator.request.clone(),
                );
                self.offer(row)?;
                if let Some(&next) = self.m.generators[g].times.get(ordinal as usize + 1) {
                    self.schedule(next, 1, 0, 0, Event::Offer(g, ordinal + 1))?;
                }
            }
            Event::Child(p, w) => self.child(p, w)?,
            Event::Admit => self.admit()?,
            Event::Dispatch(n) => self.dispatch(n)?,
            Event::Complete(n, _) => self.complete(n)?,
            Event::RefreshDue(n) => {
                self.st_mut().resources[n].refresh_pending = true;
                self.st_mut().resources[n].time = self.now;
                self.dirty(n)?;
            }
            Event::RefreshEnd(n) => {
                let now = self.now;
                self.st_mut().resources[n].refresh_ended = Some(now);
                self.st_mut().resources[n].time = now;
                self.dirty(n)?;
            }
            Event::Setup(i) => {
                self.plan(self.now, 1)?;
                self.schedule(self.now, 1, 0, 0, Event::Child(i, false))?;
            }
            Event::Response(i) => self.response(i)?,
            Event::DmaNotify(i) => {
                let n = self.st().requests[i].node;
                let now = self.now;
                self.finish(i, "completed", "ok");
                self.st_mut().requests[i].notified = Some(now);
                let s = &mut self.st_mut().resources[n];
                s.time = now;
                s.active = None;
                s.child = None;
                s.chunk_index = 0;
                s.chunk_hex = None;
                self.dirty(n)?;
            }
            Event::MailNotify(i) => {
                if self.st().notifications[i].delivered.is_some() {
                    return Err(Diagnostic::execution(
                        "mailbox notification delivered twice",
                    ));
                }
                self.st_mut().notifications[i].delivered = Some(self.now);
            }
        }
        Ok(())
    }
    fn run(mut self) -> Snapshot {
        let event_loop = super::timing::event_loop();
        while let Some((&key, _)) = self.events.first_key_value() {
            if key.0 >= self.p.common.time_limit_ps {
                break;
            }
            self.now = key.0;
            self.delta = key.1;
            self.phase = key.2;
            self.current_event_sequence = key.9;
            self.effect = 0;
            let queued = self.events.remove(&key).unwrap();
            self.scheduled.remove(&(key.0, key.1, key.2, key.3, key.4));
            let result = if self.s.common.committed_events >= self.p.common.max_events {
                Err(limit("memory/IPC max-events exceeded"))
            } else {
                self.validate_codec(key, &queued)
                    .and_then(|()| self.handle(queued.event.clone()))
            };
            if let Err(d) = result {
                self.events.insert(key, queued);
                self.s.common.partial = true;
                self.s.common.termination = "execution_failed".into();
                self.s.common.end_ps = self.now;
                self.s.common.diagnostics.push(d.with_runtime(
                    "run",
                    Some(self.now),
                    Some(key.9),
                    None,
                ));
                break;
            }
            self.s.common.committed_events += 1;
            self.s.common.last_event_time_ps = Some(self.now);
            if !self.s.common.checkpoint() {
                break;
            }
        }
        drop(event_loop);
        self.s.common.pending_events = self.events.len() as u64;
        if !self.s.common.partial {
            self.s.common.termination =
                if self.events.is_empty() && self.p.common.time_limit_ps != 0 {
                    "events_exhausted"
                } else {
                    "time_limit"
                }
                .into();
        }
        self.s
    }
}
pub(super) fn simulate(p: &PreparedSimulation) -> Result<Snapshot> {
    let m = p
        .memory_ipc
        .as_ref()
        .ok_or_else(|| Diagnostic::execution("missing memory/IPC prepared state"))?;
    let mut s = Snapshot::empty(p);
    let resources = m
        .resources
        .iter()
        .map(|r| ResourceState {
            bytes: r.initial.clone(),
            open_rows: vec![None; r.banks],
            ports: if matches!(r.kind, Kind::Ddr | Kind::Sram) {
                vec![None; r.ports]
            } else {
                Vec::new()
            },
            slots: (0..r.slots)
                .map(|_| Slot {
                    state: "free".into(),
                    owner: None,
                    message: None,
                    bytes: Vec::new(),
                })
                .collect(),
            ..Default::default()
        })
        .collect();
    s.memory_ipc = Some(MemoryIpcSnapshot {
        resources,
        ..Default::default()
    });
    let mut e = Engine {
        p,
        m,
        s,
        current_event_sequence: 0,
        effect: 0,
        events: BTreeMap::new(),
        scheduled: BTreeSet::new(),
        sequence: 0,
        now: 0,
        delta: 0,
        phase: 0,
        intents: Vec::new(),
    };
    for n in 0..m.resources.len() {
        e.queue_point(n, None);
        if m.resources[n].kind == Kind::Ddr {
            e.schedule(
                m.resources[n].times["refresh_interval_ps"],
                0,
                0,
                n,
                Event::RefreshDue(n),
            )?;
        }
    }
    for (g, generator) in m.generators.iter().enumerate() {
        if let Some(&time) = generator.times.first() {
            e.schedule(time, 1, 0, 0, Event::Offer(g, 0))?;
        }
    }
    Ok(e.run())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn prepared() -> PreparedSimulation {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/verification/fixtures/memory-ipc/sram-ports.ini"),
        )
        .unwrap()
    }
    #[test]
    fn codec_binds_kind_node_request_generation_and_live_owner() {
        let p = prepared();
        let m = p.memory_ipc.as_ref().unwrap();
        let s = Snapshot::empty(&p);
        let mut e = Engine {
            p: &p,
            m,
            s,
            current_event_sequence: 0,
            effect: 0,
            events: BTreeMap::new(),
            scheduled: BTreeSet::new(),
            sequence: 0,
            now: 0,
            delta: 0,
            phase: 0,
            intents: Vec::new(),
        };
        e.s.memory_ipc = Some(MemoryIpcSnapshot {
            resources: m
                .resources
                .iter()
                .map(|r| ResourceState {
                    ports: vec![None; r.ports],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        e.schedule(0, 1, 0, 0, Event::Offer(0, 0)).unwrap();
        let (&key, original) = e.events.first_key_value().unwrap();
        let original = original.clone();
        assert!(e.validate_codec(key, &original).is_ok());
        for (field, value) in [
            (
                "node",
                serde_json::json!(
                    m.resources
                        .iter()
                        .find(|r| r.node != m.resources[m.generators[0].node].node)
                        .unwrap()
                        .node
                ),
            ),
            ("kind", serde_json::json!("Complete")),
            ("generation", serde_json::json!("999")),
            ("request_id", serde_json::json!("other:0")),
        ] {
            let mut q = original.clone();
            let mut b: serde_json::Value =
                serde_json::from_slice(q.body.as_ref().unwrap()).unwrap();
            b[field] = value;
            q.body = Some(serde_json::to_vec(&b).unwrap());
            assert!(e.validate_codec(key, &q).is_err(), "{field}");
        }
        let n = m
            .resources
            .iter()
            .position(|r| r.kind == Kind::Sram)
            .unwrap();
        for i in 0..2 {
            let mut q = e.new_row(
                format!("q{i}"),
                n,
                0,
                0,
                None,
                0,
                m.generators[0].request.clone(),
            );
            q.status = "active".into();
            q.planned = Some(4);
            e.st_mut().requests.push(q);
        }
        e.st_mut().resources[n].ports[0] = Some(0);
        e.st_mut().resources[n].ports[1] = Some(1);
        e.schedule(4, 0, 1, n, Event::Complete(n, 0)).unwrap();
        let (&key, queued) = e.events.last_key_value().unwrap();
        let mut queued = queued.clone();
        assert!(e.validate_codec(key, &queued).is_ok());
        let mut b: serde_json::Value =
            serde_json::from_slice(queued.body.as_ref().unwrap()).unwrap();
        b["request_id"] = serde_json::json!("q1");
        queued.body = Some(serde_json::to_vec(&b).unwrap());
        assert!(e.validate_codec(key, &queued).is_err());
        let queued = e.events[&key].clone();
        e.st_mut().resources[n].ports[0] = None;
        assert!(e.validate_codec(key, &queued).is_err());
    }
}
