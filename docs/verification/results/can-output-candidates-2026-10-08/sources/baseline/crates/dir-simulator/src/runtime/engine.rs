//! Engine state ownership, dispatch and committed termination.
use super::can::{QueueEntry, protocol::WireFrame};
use super::can_archive::{Active, ArchivedRequest, CanArchive};
use super::scheduler::{Event, Key, limit_error};
use crate::snapshot::{Point, Request, RequestLineage, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

pub(super) struct Engine<'a> {
    pub(super) prepared: &'a PreparedSimulation,
    pub(super) wires: Vec<WireFrame>,
    pub(super) heap: BinaryHeap<Reverse<(Key, Event)>>,
    pub(super) next_sequence: u64,
    pub(super) now: Key,
    pub(super) dirty: BTreeSet<(u64, u64, usize)>,
    pub(super) cursors: Vec<u64>,
    pub(super) future_generation: bool,
    pub(super) queues: Vec<BTreeSet<QueueEntry>>,
    pub(super) requests: Active<Request>,
    pub(super) receivers: Active<crate::snapshot::Receiver>,
    pub(super) forwards: Active<crate::snapshot::ForwardRecord>,
    pub(super) rx_buffers: Active<crate::snapshot::RxBufferRecord>,
    pub(super) request_generators: Active<usize>,
    pub(super) active: Vec<Option<usize>>,
    pub(super) request_sources: Active<usize>,
    pub(super) request_forwards: Active<Option<usize>>,
    pub(super) rx_used: Vec<u64>,
    pub(super) tx_waiting: Vec<BTreeSet<(u64, usize)>>,
    pub(super) routed: BTreeSet<(usize, usize)>,
    pub(super) request_events: BTreeMap<usize, usize>,
    pub(super) request_receivers: BTreeMap<usize, Vec<usize>>,
    pub(super) retire_candidates: BTreeSet<usize>,
    pub(super) archive: Option<CanArchive>,
    pub(super) snapshot: Snapshot,
}

impl Engine<'_> {
    pub(super) fn append_request(
        &mut self,
        request: Request,
        lineage: RequestLineage,
        generator: usize,
        source: usize,
    ) {
        self.request_receivers
            .insert(self.requests.len(), Vec::new());
        self.snapshot
            .gateway
            .request_lineage
            .insert(request.request_id.clone(), lineage);
        self.requests.push(request);
        self.request_generators.push(generator);
        self.request_sources.push(source);
        self.request_forwards.push(None);
    }
    pub(super) fn event_request(&self, event: &Event) -> Option<usize> {
        match *event {
            Event::TxProcessed(r)
            | Event::Ready(r)
            | Event::Eof(r)
            | Event::Release(r)
            | Event::Observe(r, _, _)
            | Event::RxProcessed(r, _, _)
            | Event::Received(r, _, _)
            | Event::RoutingInput(r, _, _) => Some(r),
            Event::Forward(f) | Event::ForwardDue(f) => Some(self.forwards[f].parent_request),
            _ => None,
        }
    }
    pub(super) fn retain_event(&mut self, event: &Event) {
        if self.archive.is_some() {
            if let Some(r) = self.event_request(event) {
                *self.request_events.entry(r).or_default() += 1;
            }
        }
    }
    fn archive_request(&mut self, r: usize) -> Result<(), Diagnostic> {
        let request = self.requests[r].clone();
        let receivers = self
            .request_receivers
            .get(&r)
            .into_iter()
            .flatten()
            .map(|&row| self.receivers[row].clone())
            .collect();
        let forward_ids: Vec<_> = self
            .forwards
            .values
            .iter()
            .filter(|(_, f)| f.parent_request == r)
            .map(|(&i, _)| i)
            .collect();
        let buffer_ids: Vec<_> = self
            .rx_buffers
            .values
            .iter()
            .filter(|(_, b)| b.parent_request == r)
            .map(|(&i, _)| i)
            .collect();
        let forwards = forward_ids
            .iter()
            .map(|&i| {
                let mut f = self.forwards[i].clone();
                f.parent_request = 0;
                f
            })
            .collect();
        let rx_buffers = buffer_ids
            .iter()
            .map(|&i| {
                let mut b = self.rx_buffers[i].clone();
                b.parent_request = 0;
                b
            })
            .collect();
        let lineage = self.snapshot.gateway.request_lineage[&request.request_id].clone();
        self.archive.as_mut().unwrap().append(&ArchivedRequest {
            request,
            lineage,
            receivers,
            forwards,
            rx_buffers,
        })?;
        let request = self.requests.remove(r).unwrap();
        self.snapshot
            .gateway
            .request_lineage
            .remove(&request.request_id);
        for row in self.request_receivers.remove(&r).unwrap_or_default() {
            self.receivers.remove(row);
        }
        for i in forward_ids {
            self.forwards.remove(i);
        }
        for i in buffer_ids {
            self.rx_buffers.remove(i);
        }
        self.request_generators.remove(r);
        self.request_sources.remove(r);
        self.request_forwards.remove(r);
        self.request_events.remove(&r);
        self.routed.retain(|&(parent, _)| parent != r);
        Ok(())
    }
    fn retire_completed(&mut self) -> Result<(), Diagnostic> {
        if self.archive.is_none() {
            return Ok(());
        }
        let candidates = std::mem::take(&mut self.retire_candidates);
        for r in candidates {
            let Some(request) = self.requests.get(r) else {
                continue;
            };
            if !matches!(request.status.as_str(), "success" | "dropped")
                || self.request_events.get(&r).copied().unwrap_or(0) != 0
            {
                continue;
            }
            // A Gateway ingress remains occupied until every child reaches its TX queue.
            if self
                .rx_buffers
                .values
                .values()
                .any(|b| b.parent_request == r && b.status == "holding")
            {
                continue;
            }
            self.archive_request(r)?;
        }
        Ok(())
    }
    pub(super) fn point(
        &mut self,
        target: String,
        metric: &str,
        value: u64,
        request: usize,
        receiver: Option<String>,
    ) {
        let effect_seq = self
            .snapshot
            .common
            .points
            .iter()
            .rev()
            .take_while(|p| p.event_seq == Some(self.now.3))
            .count() as u64;
        self.snapshot.common.points.push(Point {
            event_seq: Some(self.now.3),
            effect_seq: Some(effect_seq),
            time_ps: self.now.0,
            target,
            metric: metric.into(),
            value,
            request_id: Some(self.requests[request].request_id.clone()),
            receiver,
            reason: None,
        });
    }
    fn handle(&mut self, event: Event) -> Result<(), Diagnostic> {
        match event {
            Event::Dispatch => self.dispatch(),
            Event::Generate(g, ordinal) => self.generate(g, ordinal),
            Event::TxProcessed(r) => self.tx_processed(r),
            Event::Ready(r) => self.ready(r),
            Event::Arbitrate(bus) => self.arbitrate(bus),
            Event::Eof(r) => self.eof(r),
            Event::Release(r) => self.release(r),
            Event::Observe(r, c, row) => self.observe(r, c, row),
            Event::RxProcessed(r, c, row) => self.rx_processed(r, c, row),
            Event::Received(r, c, row) => self.received(r, c, row),
            Event::ForwardDue(f) => self.forward_due(f),
            Event::RoutingInput(r, c, row) => self.route(r, c, row),
            Event::Forward(f) => self.forward(f),
            Event::GwTxSpace(source) => {
                self.drain_gateway_tx(source);
                Ok(())
            }
        }
    }
    pub(super) fn finish(mut self) -> Snapshot {
        let event_loop = super::timing::event_loop();
        loop {
            // Seal the complete arbitration batch after phase 0/1 has drained,
            // before any arbitration callback can reserve further events.
            if let Some(&(time, delta, _)) = self.dirty.first() {
                let before_next = self
                    .heap
                    .peek()
                    .is_none_or(|Reverse((key, _))| (time, delta, 2) <= (key.0, key.1, key.2));
                if before_next {
                    let (dirty, events) = match self.seal_dirty_preflight(time, delta) {
                        Ok(batch) => batch,
                        Err(d) => {
                            self.fail(d.with_detail("operation", "seal_arbitration"), time);
                            self.snapshot
                                .common
                                .diagnostics
                                .last_mut()
                                .unwrap()
                                .event_seq = None;
                            break;
                        }
                    };
                    self.publish(events);
                    for key in dirty {
                        self.dirty.remove(&key);
                    }
                }
            }
            let Some(Reverse((key, event))) = self.heap.peek().cloned() else {
                break;
            };
            if key.0 >= self.prepared.common.time_limit_ps {
                break;
            }
            if self.snapshot.common.committed_events >= self.prepared.common.max_events {
                self.fail(limit_error("max-events exceeded"), key.0);
                break;
            }
            if key.1 >= self.prepared.common.max_delta_cycles {
                self.fail(limit_error("max-delta-cycles exceeded"), key.0);
                break;
            }
            self.now = key;
            // Keep the candidate in the heap until the complete callback succeeds.
            // New reservations sort after it, so the candidate remains the minimum.
            let request = self.event_request(&event);
            if let Err(d) = self.handle(event) {
                self.fail(d, key.0);
                break;
            }
            let popped = self.heap.pop().expect("current event retained");
            debug_assert_eq!(popped.0.0, key);
            self.snapshot.common.committed_events += 1;
            self.snapshot.common.last_event_time_ps = Some(key.0);
            if self.archive.is_some() {
                self.archive
                    .as_mut()
                    .unwrap()
                    .observe_live(self.requests.values.len(), self.receivers.values.len());
                if let Some(r) = request {
                    *self.request_events.get_mut(&r).expect("retained event") -= 1;
                    self.retire_candidates.insert(r);
                }
                if let Err(d) = self.retire_completed() {
                    self.snapshot.common.spool_error = Some(d);
                    break;
                }
            }
            if !self.snapshot.common.checkpoint() {
                break;
            }
        }
        drop(event_loop);
        self.snapshot.common.pending_events =
            self.heap.len() as u64 + self.dirty.len() as u64 + u64::from(self.future_generation);
        if !self.snapshot.common.partial {
            self.snapshot.common.termination = if self.prepared.common.time_limit_ps == 0
                || self.snapshot.common.pending_events > 0
            {
                "time_limit"
            } else {
                "events_exhausted"
            }
            .into();
        }
        if self.archive.is_some() {
            let remaining: Vec<_> = self.requests.values.keys().copied().collect();
            for r in remaining {
                if let Err(d) = self.archive_request(r) {
                    self.snapshot.common.spool_error = Some(d);
                    break;
                }
            }
            let mut archive = self.archive.take().unwrap();
            if let Err(d) = archive.finish() {
                self.snapshot.common.spool_error = Some(d);
            }
            self.snapshot.can.archive = Some(std::sync::Arc::new(archive));
        } else {
            self.snapshot.can.requests = self.requests.into_vec();
            self.snapshot.can.receivers = self.receivers.into_vec();
            self.snapshot.gateway.forwards = self.forwards.into_vec();
            self.snapshot.gateway.rx_buffers = self.rx_buffers.into_vec();
        }
        self.snapshot
    }
    pub(super) fn fail(&mut self, diagnostic: Diagnostic, at: u64) {
        self.snapshot.common.termination = "execution_failed".into();
        self.snapshot.common.partial = true;
        self.snapshot.common.end_ps = at;
        let event_seq = self
            .heap
            .peek()
            .filter(|entry| entry.0.0.0 == at)
            .map(|entry| entry.0.0.3);
        self.snapshot
            .common
            .diagnostics
            .push(diagnostic.with_runtime("run", Some(at), event_seq, None));
    }
}

/// Execute a prepared CAN model with independent buses and optional Gateways.
/// All input is snapshotted before this call.
pub fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot, Diagnostic> {
    let wires = super::can::wires(prepared)?;
    let mut engine = Engine {
        prepared,
        wires,
        heap: BinaryHeap::new(),
        next_sequence: 0,
        now: (0, 0, 0, 0),
        dirty: BTreeSet::new(),
        cursors: vec![0; prepared.can.generators.len()],
        future_generation: false,
        queues: vec![BTreeSet::new(); prepared.can.controllers.len()],
        requests: Active::default(),
        receivers: Active::default(),
        forwards: Active::default(),
        rx_buffers: Active::default(),
        request_generators: Active::default(),
        active: vec![None; prepared.can.buses.len()],
        request_sources: Active::default(),
        request_forwards: Active::default(),
        rx_used: vec![0; prepared.can.controllers.len()],
        tx_waiting: vec![BTreeSet::new(); prepared.can.controllers.len()],
        routed: BTreeSet::new(),
        request_events: BTreeMap::new(),
        request_receivers: BTreeMap::new(),
        retire_candidates: BTreeSet::new(),
        archive: CanArchive::create()?,
        snapshot: Snapshot {
            registered: None,
            network: None,
            ethernet: None,
            canfd: None,
            axi: None,
            soc: None,
            memory_ipc: None,
            common: crate::snapshot::CommonSnapshot {
                point_spool: None,
                spool_error: None,
                termination: "events_exhausted".into(),
                partial: false,
                end_ps: prepared.common.time_limit_ps,
                last_event_time_ps: None,
                committed_events: 0,
                pending_events: 0,
                points: Vec::new(),
                diagnostics: Vec::new(),
            },
            can: crate::snapshot::CanSnapshot {
                archive: None,
                bus_state: "idle".into(),
                bus_states: vec!["idle".into(); prepared.can.buses.len()],
                requests: Vec::new(),
                receivers: Vec::new(),
            },
            gateway: crate::snapshot::GatewaySnapshot {
                forwards: Vec::new(),
                rx_buffers: Vec::new(),
                request_lineage: Default::default(),
            },
        },
    };
    engine.initialize_can_points();
    if let Some(time) = engine.next_generation(&engine.cursors) {
        if time <= u64::MAX as u128 {
            let events = vec![(time as u64, Event::Dispatch)];
            engine.preflight(&events)?;
            engine.publish(events);
        } else {
            engine.future_generation = true;
        }
    }
    Ok(engine.finish())
}
