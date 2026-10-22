//! Engine state ownership, dispatch and committed termination.
use super::can::{QueueEntry, protocol::WireFrame};
use super::can_archive::{Active, ArchivedRequest, CanArchive};
use super::scheduler::{Event, Key, limit_error};
use crate::snapshot::{Point, Request, RequestLineage, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

pub(super) struct EngineState {
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
pub(super) struct Engine<'a> {
    pub(super) prepared: &'a PreparedSimulation,
    pub(super) state: EngineState,
}
impl std::ops::Deref for Engine<'_> {
    type Target = EngineState;
    fn deref(&self) -> &EngineState {
        &self.state
    }
}
impl std::ops::DerefMut for Engine<'_> {
    fn deref_mut(&mut self) -> &mut EngineState {
        &mut self.state
    }
}

impl Engine<'_> {
    pub(super) fn append_request(
        &mut self,
        request: Request,
        lineage: RequestLineage,
        generator: usize,
        source: usize,
    ) {
        self.state
            .request_receivers
            .insert(self.state.requests.len(), Vec::new());
        self.state
            .snapshot
            .gateway
            .request_lineage
            .insert(request.request_id.clone(), lineage);
        self.state.requests.push(request);
        self.state.request_generators.push(generator);
        self.state.request_sources.push(source);
        self.state.request_forwards.push(None);
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
            Event::Forward(f) | Event::ForwardDue(f) => Some(self.state.forwards[f].parent_request),
            _ => None,
        }
    }
    pub(super) fn retain_event(&mut self, event: &Event) {
        if self.state.archive.is_some() {
            if let Some(r) = self.event_request(event) {
                *self.state.request_events.entry(r).or_default() += 1;
            }
        }
    }
    fn archive_row(&self, r: usize) -> (ArchivedRequest, Vec<usize>, Vec<usize>) {
        let request = self.state.requests[r].clone();
        let receivers = self
            .state
            .request_receivers
            .get(&r)
            .into_iter()
            .flatten()
            .map(|&row| self.state.receivers[row].clone())
            .collect();
        let forward_ids: Vec<_> = self
            .state
            .forwards
            .values
            .iter()
            .filter(|(_, f)| f.parent_request == r)
            .map(|(&i, _)| i)
            .collect();
        let buffer_ids: Vec<_> = self
            .state
            .rx_buffers
            .values
            .iter()
            .filter(|(_, b)| b.parent_request == r)
            .map(|(&i, _)| i)
            .collect();
        let forwards = forward_ids
            .iter()
            .map(|&i| {
                let mut f = self.state.forwards[i].clone();
                f.parent_request = 0;
                f
            })
            .collect();
        let rx_buffers = buffer_ids
            .iter()
            .map(|&i| {
                let mut b = self.state.rx_buffers[i].clone();
                b.parent_request = 0;
                b
            })
            .collect();
        let lineage = self.state.snapshot.gateway.request_lineage[&request.request_id].clone();
        (
            ArchivedRequest {
                request,
                lineage,
                receivers,
                forwards,
                rx_buffers,
            },
            forward_ids,
            buffer_ids,
        )
    }
    fn archive_request(&mut self, r: usize) -> Result<(), Diagnostic> {
        let (row, forward_ids, buffer_ids) = self.archive_row(r);
        self.state.archive.as_mut().unwrap().append(&row)?;
        let request = self.state.requests.remove(r).unwrap();
        self.state
            .snapshot
            .gateway
            .request_lineage
            .remove(&request.request_id);
        for row in self.state.request_receivers.remove(&r).unwrap_or_default() {
            self.state.receivers.remove(row);
        }
        for i in forward_ids {
            self.state.forwards.remove(i);
        }
        for i in buffer_ids {
            self.state.rx_buffers.remove(i);
        }
        self.state.request_generators.remove(r);
        self.state.request_sources.remove(r);
        self.state.request_forwards.remove(r);
        self.state.request_events.remove(&r);
        self.state.routed.retain(|&(parent, _)| parent != r);
        Ok(())
    }
    fn retire_completed(&mut self) -> Result<(), Diagnostic> {
        if self.state.archive.is_none() {
            return Ok(());
        }
        let candidates = std::mem::take(&mut self.state.retire_candidates);
        for r in candidates {
            let Some(request) = self.state.requests.get(r) else {
                continue;
            };
            if !matches!(request.status.as_str(), "success" | "dropped")
                || self.state.request_events.get(&r).copied().unwrap_or(0) != 0
            {
                continue;
            }
            // A Gateway ingress remains occupied until every child reaches its TX queue.
            if self
                .state
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
            .state
            .snapshot
            .common
            .points
            .iter()
            .rev()
            .take_while(|p| p.event_seq == Some(self.state.now.3))
            .count() as u64;
        self.state.snapshot.common.points.push(Point {
            event_seq: Some(self.state.now.3),
            effect_seq: Some(effect_seq),
            time_ps: self.state.now.0,
            target,
            metric: metric.into(),
            value,
            request_id: Some(self.state.requests[request].request_id.clone()),
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
    fn step(&mut self) -> super::legacy::StepOutcome {
        // Seal the complete arbitration batch after phase 0/1 has drained,
        // before any arbitration callback can reserve further events.
        if let Some(&(time, delta, _)) = self.state.dirty.first() {
            let before_next = self
                .state
                .heap
                .peek()
                .is_none_or(|Reverse((key, _))| (time, delta, 2) <= (key.0, key.1, key.2));
            if before_next {
                let (dirty, events) = match self.seal_dirty_preflight(time, delta) {
                    Ok(batch) => batch,
                    Err(d) => {
                        self.fail(d.with_detail("operation", "seal_arbitration"), time);
                        self.state
                            .snapshot
                            .common
                            .diagnostics
                            .last_mut()
                            .unwrap()
                            .event_seq = None;
                        return super::legacy::StepOutcome::Failed;
                    }
                };
                self.publish(events);
                for key in dirty {
                    self.state.dirty.remove(&key);
                }
            }
        }
        let Some(Reverse((key, event))) = self.state.heap.peek().cloned() else {
            return super::legacy::StepOutcome::Exhausted;
        };
        if key.0 >= self.prepared.common.time_limit_ps {
            return super::legacy::StepOutcome::Exhausted;
        }
        if self.state.snapshot.common.committed_events >= self.prepared.common.max_events {
            self.fail(limit_error("max-events exceeded"), key.0);
            return super::legacy::StepOutcome::Failed;
        }
        if key.1 >= self.prepared.common.max_delta_cycles {
            self.fail(limit_error("max-delta-cycles exceeded"), key.0);
            return super::legacy::StepOutcome::Failed;
        }
        self.state.now = key;
        // Keep the candidate in the heap until the complete callback succeeds.
        // New reservations sort after it, so the candidate remains the minimum.
        let request = self.event_request(&event);
        #[cfg(test)]
        super::test_probe::can_event_callback();
        if let Err(d) = self.handle(event) {
            self.fail(d, key.0);
            return super::legacy::StepOutcome::Failed;
        }
        let popped = self.state.heap.pop().expect("current event retained");
        debug_assert_eq!(popped.0.0, key);
        self.state.snapshot.common.committed_events += 1;
        self.state.snapshot.common.last_event_time_ps = Some(key.0);
        if self.state.archive.is_some() {
            self.state.archive.as_mut().unwrap().observe_live(
                self.state.requests.values.len(),
                self.state.receivers.values.len(),
            );
            if let Some(r) = request {
                *self
                    .state
                    .request_events
                    .get_mut(&r)
                    .expect("retained event") -= 1;
                self.state.retire_candidates.insert(r);
            }
            if let Err(d) = self.retire_completed() {
                self.state.snapshot.common.spool_error = Some(d);
                return super::legacy::StepOutcome::Failed;
            }
        }

        super::legacy::StepOutcome::Committed
    }
    fn finish_once(&mut self) {
        self.state.snapshot.common.pending_events = self.state.heap.len() as u64
            + self.state.dirty.len() as u64
            + u64::from(self.state.future_generation);
        if !self.state.snapshot.common.partial {
            self.state.snapshot.common.end_ps = self.prepared.common.time_limit_ps;
            self.state.snapshot.common.termination = if self.prepared.common.time_limit_ps == 0
                || self.state.snapshot.common.pending_events > 0
            {
                "time_limit"
            } else {
                "events_exhausted"
            }
            .into();
        }
        if self.state.archive.is_some() {
            let remaining: Vec<_> = self.state.requests.values.keys().copied().collect();
            for r in remaining {
                if let Err(d) = self.archive_request(r) {
                    self.state.snapshot.common.spool_error = Some(d);
                    break;
                }
            }
            let mut archive = self.state.archive.take().unwrap();
            if let Err(d) = archive.finish() {
                self.state.snapshot.common.spool_error = Some(d);
            }
            self.state.snapshot.can.archive = Some(std::sync::Arc::new(archive));
        } else {
            self.state.snapshot.can.requests = std::mem::take(&mut self.state.requests).into_vec();
            self.state.snapshot.can.receivers =
                std::mem::take(&mut self.state.receivers).into_vec();
            self.state.snapshot.gateway.forwards =
                std::mem::take(&mut self.state.forwards).into_vec();
            self.state.snapshot.gateway.rx_buffers =
                std::mem::take(&mut self.state.rx_buffers).into_vec();
        }
    }
    pub(super) fn finish(mut self) -> Snapshot {
        let event_loop = super::timing::event_loop();
        loop {
            if !matches!(self.step(), super::legacy::StepOutcome::Committed)
                || !self.state.snapshot.common.checkpoint()
            {
                break;
            }
        }
        drop(event_loop);
        self.finish_once();
        self.state.snapshot
    }
    pub(super) fn fail(&mut self, diagnostic: Diagnostic, at: u64) {
        self.state.snapshot.common.termination = "execution_failed".into();
        self.state.snapshot.common.partial = true;
        self.state.snapshot.common.end_ps = at;
        let event_seq = self
            .state
            .heap
            .peek()
            .filter(|entry| entry.0.0.0 == at)
            .map(|entry| entry.0.0.3);
        self.state
            .snapshot
            .common
            .diagnostics
            .push(diagnostic.with_runtime("run", Some(at), event_seq, None));
    }
}

/// Execute a prepared CAN model with independent buses and optional Gateways.
/// All input is snapshotted before this call.
fn create_engine(prepared: &PreparedSimulation) -> Result<Engine<'_>, Diagnostic> {
    #[cfg(test)]
    super::test_probe::can_engine();
    let wires = super::can::wires(prepared)?;
    let engine = Engine {
        prepared,
        state: EngineState {
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
        },
    };
    Ok(engine)
}

impl Engine<'_> {
    fn initialize(&mut self) -> Result<(), Diagnostic> {
        self.initialize_can_points();
        if let Some(time) = self.next_generation(&self.state.cursors) {
            if time <= u64::MAX as u128 {
                let events = vec![(time as u64, Event::Dispatch)];
                self.preflight(&events)?;
                self.publish(events);
            } else {
                self.state.future_generation = true;
            }
        }
        Ok(())
    }
}

pub fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot, Diagnostic> {
    let mut engine = create_engine(prepared)?;
    engine.initialize()?;
    Ok(engine.finish())
}

/// Owned continuation; each advance executes one original event callback.
pub(crate) struct Stepper {
    prepared: std::sync::Arc<PreparedSimulation>,
    state: Option<EngineState>,
    initialized: bool,
    finished: bool,
}
impl Stepper {
    pub(crate) fn new(prepared: std::sync::Arc<PreparedSimulation>) -> Result<Self, Diagnostic> {
        let state = create_engine(&prepared)?.state;
        Ok(Self {
            prepared,
            state: Some(state),
            initialized: false,
            finished: false,
        })
    }
    fn with_engine<T>(&mut self, f: impl FnOnce(&mut Engine<'_>) -> T) -> T {
        let prepared = &*self.prepared;
        let mut engine = Engine {
            prepared,
            state: self.state.take().expect("owned continuation"),
        };
        let result = f(&mut engine);
        self.state = Some(engine.state);
        result
    }
    pub(crate) fn initialize(&mut self) -> Result<(), Diagnostic> {
        if !self.initialized {
            self.with_engine(|engine| engine.initialize())?;
            self.initialized = true;
        }
        Ok(())
    }
    pub(crate) fn step(&mut self) -> super::legacy::StepOutcome {
        debug_assert!(self.initialized && !self.finished);
        self.with_engine(|engine| engine.step())
    }
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.state.as_ref().expect("owned continuation").snapshot
    }
    pub(crate) fn snapshot_mut(&mut self) -> &mut Snapshot {
        &mut self.state.as_mut().expect("owned continuation").snapshot
    }
    pub(crate) fn refresh_prefix(&mut self, horizon: u64) -> Result<(), Diagnostic> {
        if self.finished {
            self.snapshot_mut().common.end_ps = horizon;
            return Ok(());
        }
        self.with_engine(|engine| {
            engine.state.snapshot.common.end_ps = horizon;
            engine.state.snapshot.common.pending_events = engine.state.heap.len() as u64
                + engine.state.dirty.len() as u64
                + u64::from(engine.state.future_generation);
            if let Some(archive) = engine.state.archive.as_mut() {
                archive.flush_prefix()?;
                let mut projection = archive.empty_projection()?;
                for row in archive.iter()? {
                    projection.append(&row?)?;
                }
                for &r in engine.state.requests.values.keys() {
                    projection.append(&engine.archive_row(r).0)?;
                }
                projection.flush_prefix()?;
                engine.state.snapshot.can.archive = Some(std::sync::Arc::new(projection));
                engine.state.snapshot.can.requests.clear();
                engine.state.snapshot.can.receivers.clear();
                engine.state.snapshot.gateway.forwards.clear();
                engine.state.snapshot.gateway.rx_buffers.clear();
            } else {
                engine.state.snapshot.can.requests =
                    engine.state.requests.values.values().cloned().collect();
                engine.state.snapshot.can.receivers =
                    engine.state.receivers.values.values().cloned().collect();
                engine.state.snapshot.gateway.forwards =
                    engine.state.forwards.values.values().cloned().collect();
                engine.state.snapshot.gateway.rx_buffers =
                    engine.state.rx_buffers.values.values().cloned().collect();
            }
            Ok(())
        })
    }
    pub(crate) fn finish_once(&mut self) {
        if !self.finished {
            self.with_engine(|engine| engine.finish_once());
            self.finished = true;
        }
    }
}

#[cfg(test)]
mod cooperative_tests {
    use super::super::legacy::StepOutcome;
    use super::*;
    use std::sync::Arc;

    fn fixture(relative: &str) -> PreparedSimulation {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(relative),
        )
        .unwrap()
    }
    fn rows(snapshot: &Snapshot) -> serde_json::Value {
        if let Some(archive) = &snapshot.can.archive {
            serde_json::to_value(
                archive
                    .iter()
                    .unwrap()
                    .map(Result::unwrap)
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        } else {
            serde_json::json!([
                snapshot.can.requests,
                snapshot.can.receivers,
                snapshot.gateway.forwards,
                snapshot.gateway.rx_buffers,
                snapshot.gateway.request_lineage
            ])
        }
    }
    fn run_case(relative: &str, archive: bool) {
        let prepared = Arc::new(fixture(relative));
        let directory = std::env::temp_dir().join(format!(
            "dir-can-cooperative-{}-{}",
            std::process::id(),
            if archive { "archive" } else { "memory" }
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let (expected, mut stepper) = if archive {
            super::super::spool::with_spooling(&directory, || {
                (
                    simulate(&prepared).unwrap(),
                    Stepper::new(prepared.clone()).unwrap(),
                )
            })
        } else {
            (
                simulate(&prepared).unwrap(),
                Stepper::new(prepared.clone()).unwrap(),
            )
        };
        assert_eq!(stepper.snapshot().common.committed_events, 0);
        assert!(stepper.snapshot().common.points.is_empty());
        stepper.refresh_prefix(0).unwrap();
        stepper.initialize().unwrap();
        assert_eq!(stepper.snapshot().common.committed_events, 0);
        let mut observed_inflight = false;
        loop {
            let before = stepper.snapshot().common.committed_events;
            match stepper.step() {
                StepOutcome::Committed => {
                    assert_eq!(stepper.snapshot().common.committed_events, before + 1)
                }
                StepOutcome::Exhausted => break,
                StepOutcome::Failed => panic!("{:?}", stepper.snapshot().common.diagnostics),
            }
            if before % 7 == 0 {
                let horizon = stepper.snapshot().common.last_event_time_ps.unwrap_or(0);
                let pending = stepper.state.as_ref().unwrap().heap.len();
                let live = stepper.state.as_ref().unwrap().requests.values.len();
                for _ in 0..2 {
                    stepper.refresh_prefix(horizon).unwrap();
                }
                assert_eq!(stepper.snapshot().common.committed_events, before + 1);
                assert_eq!(stepper.state.as_ref().unwrap().heap.len(), pending);
                assert_eq!(stepper.state.as_ref().unwrap().requests.values.len(), live);
                if archive {
                    let state = stepper.state.as_ref().unwrap();
                    assert_eq!(
                        state.snapshot.can.archive.as_ref().unwrap().len(),
                        state.archive.as_ref().unwrap().len() + live
                    );
                    if before == 7 {
                        // A projection I/O failure must leave the continuation usable.
                        let displaced = directory.with_extension("displaced");
                        std::fs::rename(&directory, &displaced).unwrap();
                        assert!(stepper.refresh_prefix(horizon).is_err());
                        std::fs::rename(&displaced, &directory).unwrap();
                        assert_eq!(stepper.snapshot().common.committed_events, before + 1);
                        assert!(stepper.snapshot().common.spool_error.is_none());
                    }
                }
                for request in stepper.state.as_ref().unwrap().requests.values.values() {
                    if request.sof_ps.is_some() && request.eof_ps.is_none() {
                        observed_inflight = true;
                    }
                    assert!(request.eof_ps.is_none_or(|t| t <= horizon));
                }
            }
        }
        stepper.finish_once();
        let final_rows = rows(stepper.snapshot());
        stepper.finish_once();
        assert_eq!(rows(stepper.snapshot()), final_rows);
        assert_eq!(final_rows, rows(&expected));
        assert_eq!(
            format!("{:?}", stepper.snapshot().common),
            format!("{:?}", expected.common)
        );
        assert_eq!(stepper.snapshot().can.bus_states, expected.can.bus_states);
        assert!(observed_inflight);
        drop(stepper);
        drop(expected);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn pause_projection_does_not_consume_can_or_gateway_state() {
        for archive in [false, true] {
            for relative in ["examples/can/contention.ini", "examples/gateway/fanout.ini"] {
                run_case(relative, archive);
            }
        }
    }
}
