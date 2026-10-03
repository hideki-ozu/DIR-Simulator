//! Engine state ownership, dispatch and committed termination.
use super::can::{QueueEntry, protocol::WireFrame};
use super::scheduler::{Event, Key, limit_error};
use crate::snapshot::{Point, Request, RequestLineage, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

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
    pub(super) request_generators: Vec<usize>,
    pub(super) active: Vec<Option<usize>>,
    pub(super) request_sources: Vec<usize>,
    pub(super) request_forwards: Vec<Option<usize>>,
    pub(super) rx_used: Vec<u64>,
    pub(super) tx_waiting: Vec<BTreeSet<(u64, usize)>>,
    pub(super) routed: BTreeSet<(usize, usize)>,
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
        self.snapshot
            .gateway
            .request_lineage
            .insert(request.request_id.clone(), lineage);
        self.snapshot.can.requests.push(request);
        self.request_generators.push(generator);
        self.request_sources.push(source);
        self.request_forwards.push(None);
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
            request_id: Some(self.snapshot.can.requests[request].request_id.clone()),
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
        loop {
            // Seal the complete arbitration batch after phase 0/1 has drained,
            // before any arbitration callback can reserve further events.
            if let Some(&(time, delta, _)) = self.dirty.first() {
                let before_next = self
                    .heap
                    .peek()
                    .is_none_or(|Reverse((key, _))| (time, delta, 2) <= (key.0, key.1, key.2));
                if before_next {
                    let dirty: Vec<_> = self
                        .dirty
                        .range((time, delta, 0)..=(time, delta, usize::MAX))
                        .copied()
                        .collect();
                    let events: Vec<_> = dirty
                        .iter()
                        .map(|&(time, _, bus)| (time, Event::Arbitrate(bus)))
                        .collect();
                    if let Err(d) = self.preflight(&events) {
                        self.fail(d, time);
                        break;
                    }
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
            if let Err(d) = self.handle(event) {
                self.fail(d, key.0);
                break;
            }
            let popped = self.heap.pop().expect("current event retained");
            debug_assert_eq!(popped.0.0, key);
            self.snapshot.common.committed_events += 1;
            self.snapshot.common.last_event_time_ps = Some(key.0);
        }
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
        self.snapshot
    }
    pub(super) fn fail(&mut self, diagnostic: Diagnostic, at: u64) {
        self.snapshot.common.termination = "execution_failed".into();
        self.snapshot.common.partial = true;
        self.snapshot.common.end_ps = at;
        self.snapshot.common.diagnostics.push(diagnostic);
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
        request_generators: Vec::new(),
        active: vec![None; prepared.can.buses.len()],
        request_sources: Vec::new(),
        request_forwards: Vec::new(),
        rx_used: vec![0; prepared.can.controllers.len()],
        tx_waiting: vec![BTreeSet::new(); prepared.can.controllers.len()],
        routed: BTreeSet::new(),
        snapshot: Snapshot {
            common: crate::snapshot::CommonSnapshot {
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
        if time < prepared.common.time_limit_ps as u128 {
            let events = vec![(time as u64, Event::Dispatch)];
            engine.preflight(&events)?;
            engine.publish(events);
        } else {
            engine.future_generation = true;
        }
    }
    Ok(engine.finish())
}
