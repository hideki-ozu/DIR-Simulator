//! Deterministic event loop. The event ordering key contains no CAN priority fields.
use crate::can::{self, WireFrame};
use crate::snapshot::{Point, Receiver, Request, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Event {
    Dispatch,
    Generate(usize, u64),
    TxProcessed(usize),
    Ready(usize),
    Arbitrate,
    Eof(usize),
    Release(usize),
    Observe(usize, usize, usize), // request, controller, receiver row
    RxProcessed(usize, usize, usize),
    Received(usize, usize, usize),
}
impl Event {
    fn phase(&self) -> u8 {
        match self {
            Self::TxProcessed(_) | Self::Eof(_) | Self::Release(_) | Self::RxProcessed(..) => 0,
            Self::Arbitrate => 2,
            _ => 1,
        }
    }
}
type Key = (u64, u64, u8, u64);
type QueueEntry = (Vec<u8>, u64, String, u64, usize);

struct Engine<'a> {
    prepared: &'a PreparedSimulation,
    wires: Vec<WireFrame>,
    heap: BinaryHeap<Reverse<(Key, Event)>>,
    next_sequence: u64,
    now: Key,
    dirty: Option<(u64, u64)>,
    cursors: Vec<u64>,
    future_generation: bool,
    queues: Vec<BTreeSet<QueueEntry>>,
    request_generators: Vec<usize>,
    active: Option<usize>,
    snapshot: Snapshot,
}

fn add(a: u64, b: u64) -> Result<u64, Diagnostic> {
    a.checked_add(b)
        .ok_or_else(|| Diagnostic::execution("time arithmetic overflow"))
}
fn limit_error(message: &str) -> Diagnostic {
    let mut d = Diagnostic::execution(message);
    d.code = "E-0004".into();
    d
}

impl Engine<'_> {
    /// Check all reservations before publishing any state or events.
    fn preflight(&self, events: &[(u64, Event)]) -> Result<(), Diagnostic> {
        self.next_sequence
            .checked_add(events.len() as u64)
            .ok_or_else(|| limit_error("event sequence overflow"))?;
        for (time, event) in events {
            if *time < self.now.0 {
                return Err(Diagnostic::execution("attempt to schedule in the past"));
            }
            if *time == self.now.0 && event.phase() < self.now.2 {
                self.now
                    .1
                    .checked_add(1)
                    .ok_or_else(|| limit_error("delta overflow"))?;
            }
        }
        Ok(())
    }
    fn publish(&mut self, events: Vec<(u64, Event)>) {
        for (time, event) in events {
            let phase = event.phase();
            let delta = if time == self.now.0 {
                self.now.1 + u64::from(phase < self.now.2)
            } else {
                0
            };
            self.heap
                .push(Reverse(((time, delta, phase, self.next_sequence), event)));
            self.next_sequence += 1;
        }
    }
    fn dirty(&mut self) {
        self.dirty = Some((self.now.0, self.now.1 + u64::from(self.now.2 == 2)));
    }
    fn point(
        &mut self,
        target: String,
        metric: &str,
        value: u64,
        request: usize,
        receiver: Option<String>,
    ) {
        let effect_seq = self
            .snapshot
            .points
            .iter()
            .rev()
            .take_while(|p| p.event_seq == Some(self.now.3))
            .count() as u64;
        self.snapshot.points.push(Point {
            event_seq: Some(self.now.3),
            effect_seq: Some(effect_seq),
            time_ps: self.now.0,
            target,
            metric: metric.into(),
            value,
            request_id: Some(self.snapshot.requests[request].request_id.clone()),
            receiver,
        });
    }
    fn queue_point(&mut self, source: usize, request: usize) {
        self.point(
            format!("{}.txQueue", self.prepared.controllers[source].id),
            "queue_length",
            self.queues[source].len() as u64,
            request,
            None,
        );
    }
    fn next_generation(&self, cursors: &[u64]) -> Option<u128> {
        self.prepared
            .generators
            .iter()
            .zip(cursors)
            .filter_map(|(g, &ordinal)| g.schedule.time(ordinal))
            .min()
    }
    fn dispatch(&mut self) -> Result<(), Diagnostic> {
        let mut cursors = self.cursors.clone();
        let mut events = Vec::new();
        for (g, generator) in self.prepared.generators.iter().enumerate() {
            while generator.schedule.time(cursors[g]) == Some(self.now.0 as u128) {
                events.push((self.now.0, Event::Generate(g, cursors[g])));
                cursors[g] = cursors[g]
                    .checked_add(1)
                    .ok_or_else(|| limit_error("generator ordinal overflow"))?;
            }
        }
        let future = self.next_generation(&cursors);
        if let Some(time) = future.filter(|&t| t < self.prepared.time_limit_ps as u128) {
            events.push((time as u64, Event::Dispatch));
        }
        self.preflight(&events)?;
        self.cursors = cursors;
        self.future_generation = future.is_some_and(|t| t >= self.prepared.time_limit_ps as u128);
        self.publish(events);
        Ok(())
    }
    fn handle(&mut self, event: Event) -> Result<(), Diagnostic> {
        let time = self.now.0;
        match event {
            Event::Dispatch => self.dispatch()?,
            Event::Generate(g, ordinal) => {
                let generator = &self.prepared.generators[g];
                let controller = &self.prepared.controllers[generator.source];
                let ready = add(time, controller.tx_processing_ps)?;
                let index = self.snapshot.requests.len();
                let events = vec![(
                    ready,
                    if ready == time {
                        Event::Ready(index)
                    } else {
                        Event::TxProcessed(index)
                    },
                )];
                self.preflight(&events)?;
                let wire = &self.wires[g];
                self.snapshot.requests.push(Request {
                    request_id: format!("{}:{ordinal}", generator.id),
                    source: controller.id.clone(),
                    bus: self.prepared.bus_id.clone(),
                    status: "processing".into(),
                    generated_ps: time,
                    ready_ps: None,
                    sof_ps: None,
                    eof_ps: None,
                    planned_eof_ps: None,
                    planned_release_ps: None,
                    release_ps: None,
                    payload_bits: generator.frame.data.len() as u64 * 4,
                    frame_bits: wire.frame.len() as u64,
                    crc15: wire.crc15,
                    stuff_bits: wire.stuff_positions.len() as u64,
                    bitrate_bps: self.prepared.bitrate,
                });
                self.request_generators.push(g);
                self.publish(events);
            }
            Event::TxProcessed(r) => {
                let events = vec![(time, Event::Ready(r))];
                self.preflight(&events)?;
                self.publish(events);
            }
            Event::Ready(r) => {
                let g = self.request_generators[r];
                let source = self.prepared.generators[g].source;
                let request = &mut self.snapshot.requests[r];
                request.ready_ps = Some(time);
                if self.queues[source].len() as u64
                    >= self.prepared.controllers[source].queue_capacity
                {
                    request.status = "dropped".into();
                } else {
                    request.status = "pending".into();
                    let ordinal = request
                        .request_id
                        .rsplit(':')
                        .next()
                        .unwrap()
                        .parse()
                        .unwrap();
                    self.queues[source].insert((
                        self.wires[g].arbitration.clone(),
                        request.generated_ps,
                        self.prepared.generators[g].id.clone(),
                        ordinal,
                        r,
                    ));
                    self.dirty();
                }
                self.queue_point(source, r);
            }
            Event::Arbitrate => {
                if self.active.is_none() {
                    let candidate = self
                        .queues
                        .iter()
                        .enumerate()
                        .filter_map(|(s, q)| q.first().map(|e| (s, e)))
                        .min_by(|a, b| a.1.cmp(b.1))
                        .map(|(s, e)| (s, e.clone()));
                    if let Some((source, entry)) = candidate {
                        let r = entry.4;
                        let eof = can::end_time(
                            time,
                            self.snapshot.requests[r].frame_bits,
                            self.prepared.bitrate,
                        )?;
                        let release = can::end_time(
                            time,
                            self.snapshot.requests[r].frame_bits + 3,
                            self.prepared.bitrate,
                        )?;
                        let events = vec![(eof, Event::Eof(r)), (release, Event::Release(r))];
                        self.preflight(&events)?;
                        self.queues[source].remove(&entry);
                        self.active = Some(r);
                        self.snapshot.bus_state = "transmitting".into();
                        let request = &mut self.snapshot.requests[r];
                        request.status = "in_flight".into();
                        request.sof_ps = Some(time);
                        request.planned_eof_ps = Some(eof);
                        request.planned_release_ps = Some(release);
                        let tx_wait = time - request.generated_ps;
                        let arb_wait = time - request.ready_ps.unwrap();
                        let target = request.source.clone();
                        self.queue_point(source, r);
                        self.point(target.clone(), "tx_wait_ps", tx_wait, r, None);
                        self.point(target, "arbitration_wait_ps", arb_wait, r, None);
                        self.publish(events);
                    }
                }
            }
            Event::Eof(r) => {
                let g = self.request_generators[r];
                let source = self.prepared.generators[g].source;
                let mut events = Vec::new();
                let mut rows = Vec::new();
                for (c, controller) in self.prepared.controllers.iter().enumerate() {
                    if c == source {
                        continue;
                    }
                    let observed = add(
                        add(time, self.prepared.controllers[source].tx_channel_ps)?,
                        controller.rx_channel_ps,
                    )?;
                    let row = self.snapshot.receivers.len() + rows.len();
                    events.push((observed, Event::Observe(r, c, row)));
                    rows.push(Receiver {
                        request_id: self.snapshot.requests[r].request_id.clone(),
                        receiver: controller.id.clone(),
                        status: "pending".into(),
                        observed_ps: None,
                        received_ps: None,
                    });
                }
                self.preflight(&events)?;
                self.snapshot.requests[r].status = "success".into();
                self.snapshot.requests[r].eof_ps = Some(time);
                self.snapshot.bus_state = "intermission".into();
                self.snapshot.receivers.extend(rows);
                self.point(
                    self.prepared.bus_id.clone(),
                    "transfer_ps",
                    time - self.snapshot.requests[r].sof_ps.unwrap(),
                    r,
                    None,
                );
                self.publish(events);
            }
            Event::Release(r) => {
                self.snapshot.requests[r].release_ps = Some(time);
                self.active = None;
                self.snapshot.bus_state = "idle".into();
                self.dirty();
            }
            Event::Observe(r, c, row) => {
                let controller = &self.prepared.controllers[c];
                let g = self.request_generators[r];
                let accepted =
                    can::accepts(&controller.rx_filter, &self.prepared.generators[g].frame);
                let events = if accepted {
                    let received = add(time, controller.rx_processing_ps)?;
                    vec![(
                        received,
                        if received == time {
                            Event::Received(r, c, row)
                        } else {
                            Event::RxProcessed(r, c, row)
                        },
                    )]
                } else {
                    Vec::new()
                };
                self.preflight(&events)?;
                self.snapshot.receivers[row].observed_ps = Some(time);
                if !accepted {
                    self.snapshot.receivers[row].status = "filtered".into();
                }
                self.publish(events);
            }
            Event::RxProcessed(r, c, row) => {
                let events = vec![(time, Event::Received(r, c, row))];
                self.preflight(&events)?;
                self.publish(events);
            }
            Event::Received(r, c, row) => {
                self.snapshot.receivers[row].status = "received".into();
                self.snapshot.receivers[row].received_ps = Some(time);
                let receiver = self.prepared.controllers[c].id.clone();
                self.point(
                    receiver.clone(),
                    "delivery_ps",
                    time - self.snapshot.requests[r].generated_ps,
                    r,
                    Some(receiver),
                );
            }
        }
        Ok(())
    }
    fn finish(mut self) -> Snapshot {
        loop {
            // Assign arbitration sequence only after phase 0/1 has drained.
            if let Some((time, delta)) = self.dirty {
                let before_next = self
                    .heap
                    .peek()
                    .is_none_or(|Reverse((key, _))| (time, delta, 2) <= (key.0, key.1, key.2));
                if before_next {
                    let events = vec![(time, Event::Arbitrate)];
                    if let Err(d) = self.preflight(&events) {
                        self.fail(d, time);
                        break;
                    }
                    self.publish(events);
                    self.dirty = None;
                }
            }
            let Some(Reverse((key, event))) = self.heap.peek().cloned() else {
                break;
            };
            if key.0 >= self.prepared.time_limit_ps {
                break;
            }
            if self.snapshot.committed_events >= self.prepared.max_events {
                self.fail(limit_error("max-events exceeded"), key.0);
                break;
            }
            if key.1 >= self.prepared.max_delta_cycles {
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
            self.snapshot.committed_events += 1;
            self.snapshot.last_event_time_ps = Some(key.0);
        }
        self.snapshot.pending_events = self.heap.len() as u64
            + u64::from(self.dirty.is_some())
            + u64::from(self.future_generation);
        if !self.snapshot.partial {
            self.snapshot.termination =
                if self.prepared.time_limit_ps == 0 || self.snapshot.pending_events > 0 {
                    "time_limit"
                } else {
                    "events_exhausted"
                }
                .into();
        }
        self.snapshot
    }
    fn fail(&mut self, diagnostic: Diagnostic, at: u64) {
        self.snapshot.termination = "execution_failed".into();
        self.snapshot.partial = true;
        self.snapshot.end_ps = at;
        self.snapshot.diagnostics.push(diagnostic);
    }
}

/// Execute a prepared single-bus CAN model. All input is snapshotted before this call.
pub fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot, Diagnostic> {
    let wires = prepared
        .generators
        .iter()
        .map(|g| can::serialize(&g.frame))
        .collect::<Result<Vec<_>, _>>()?;
    let mut engine = Engine {
        prepared,
        wires,
        heap: BinaryHeap::new(),
        next_sequence: 0,
        now: (0, 0, 0, 0),
        dirty: None,
        cursors: vec![0; prepared.generators.len()],
        future_generation: false,
        queues: vec![BTreeSet::new(); prepared.controllers.len()],
        request_generators: Vec::new(),
        active: None,
        snapshot: Snapshot {
            termination: "events_exhausted".into(),
            partial: false,
            end_ps: prepared.time_limit_ps,
            last_event_time_ps: None,
            committed_events: 0,
            pending_events: 0,
            bus_state: "idle".into(),
            requests: Vec::new(),
            receivers: Vec::new(),
            points: Vec::new(),
            diagnostics: Vec::new(),
        },
    };
    if prepared.time_limit_ps > 0 {
        for controller in &prepared.controllers {
            engine.snapshot.points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: 0,
                target: format!("{}.txQueue", controller.id),
                metric: "queue_length".into(),
                value: 0,
                request_id: None,
                receiver: None,
            });
        }
    }
    if let Some(time) = engine.next_generation(&engine.cursors) {
        if time < prepared.time_limit_ps as u128 {
            let events = vec![(time as u64, Event::Dispatch)];
            engine.preflight(&events)?;
            engine.publish(events);
        } else {
            engine.future_generation = true;
        }
    }
    Ok(engine.finish())
}
