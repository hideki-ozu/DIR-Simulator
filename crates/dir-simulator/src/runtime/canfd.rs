//! Deterministic CAN FD callbacks with completion, generation and arbitration phases.
use super::legacy::StepOutcome;
use crate::snapshot::canfd::{CanFdReception, CanFdRequest, CanFdSnapshot};
use crate::snapshot::{CanSnapshot, CommonSnapshot, GatewaySnapshot, Snapshot};
use crate::types::canfd::PreparedCanFd;
use crate::types::{Diagnostic, Frame, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::sync::Arc;
pub mod codec;
pub mod protocol;
pub mod receiver;

type Result<T> = std::result::Result<T, Diagnostic>;
type Key = (u64, u8, u64, usize, u64, u64);
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Event {
    Generate(usize, u64),
    Ready(usize, String),
    Start,
    Eof(usize),
    Release(usize),
    Arrival(usize, String),
    Completed(usize),
}
fn limit(message: &str) -> Diagnostic {
    Diagnostic {
        schema_version: 1,
        code: "E-0004".into(),
        stage: "run".into(),
        message: message.into(),
        details: None,
        ..Diagnostic::execution("").with_reason("event_limit")
    }
}
fn add(a: u64, b: u64) -> Result<u64> {
    u64::try_from(a as u128 + b as u128).map_err(|_| limit("CAN FD timestamp overflow"))
}
struct Engine<'a> {
    prepared: &'a PreparedSimulation,
    model: &'a PreparedCanFd,
    owned: EngineState,
}
struct EngineState {
    heap: BinaryHeap<Reverse<(Key, Event)>>,
    sequence: u64,
    now: u64,
    active: Option<usize>,
    starts: BTreeSet<u64>,
    queues: Vec<Vec<usize>>,
    snapshot: Snapshot,
}
impl Engine<'_> {
    fn notification(
        &self,
        g: usize,
        ordinal: u64,
        time: u64,
        receiver: Option<usize>,
    ) -> protocol::Notification {
        let generator = &self.model.generators[g];
        protocol::Notification {
            kind: if receiver.is_some() {
                "arrival"
            } else {
                "ready"
            }
            .into(),
            request_id: format!("{}:{ordinal}", generator.id),
            source: self.model.controllers[generator.source].id.clone(),
            bus: self.model.bus_id.clone(),
            receiver: receiver.map(|index| self.model.controllers[index].id.clone()),
            frame_id: generator.id.clone(),
            time_ps: time,
            generation: ordinal,
        }
    }
    fn state(&self) -> &CanFdSnapshot {
        self.owned.snapshot.canfd.as_ref().unwrap()
    }
    fn state_mut(&mut self) -> &mut CanFdSnapshot {
        self.owned.snapshot.canfd.as_mut().unwrap()
    }
    /// Validate the entire callback's event reservation before changing journal/state.
    fn reserve(
        &self,
        events: Vec<(u64, u8, u64, usize, u64, Event)>,
    ) -> Result<Vec<Reverse<(Key, Event)>>> {
        self.owned
            .sequence
            .checked_add(events.len() as u64)
            .ok_or_else(|| limit("CAN FD event sequence overflow"))?;
        events
            .into_iter()
            .enumerate()
            .map(|(offset, (time, phase, generated, g, ordinal, event))| {
                if time < self.owned.now {
                    return Err(Diagnostic::execution(
                        "CAN FD reservation before callback time",
                    ));
                }
                Ok(Reverse((
                    (
                        time,
                        phase,
                        generated,
                        g,
                        ordinal,
                        self.owned.sequence + offset as u64,
                    ),
                    event,
                )))
            })
            .collect()
    }
    fn publish(&mut self, events: Vec<Reverse<(Key, Event)>>) {
        self.owned.sequence += events.len() as u64;
        self.owned.heap.extend(events);
    }
    fn start_reservation(&self) -> Vec<(u64, u8, u64, usize, u64, Event)> {
        if self.owned.starts.contains(&self.owned.now) {
            Vec::new()
        } else {
            vec![(self.owned.now, 2, 0, 0, 0, Event::Start)]
        }
    }
    fn mark_start(&mut self) {
        self.owned.starts.insert(self.owned.now);
    }
    fn ready_commit(&mut self, r: usize) {
        let g = self.state().requests[r].generator;
        let source = self.model.generators[g].source;
        let full =
            self.owned.queues[source].len() as u64 >= self.model.controllers[source].queue_capacity;
        let now = self.owned.now;
        let row = &mut self.state_mut().requests[r];
        row.time_ps = now;
        row.ready_ps = Some(now);
        row.state = if full { "dropped" } else { "queued" }.into();
        row.drop_reason = full.then(|| "queue_full".into());
        if !full {
            self.owned.queues[source].push(r);
            self.mark_start();
        }
    }
    fn request_order(&self, r: usize) -> (&Vec<u8>, u64, &str, u64) {
        let row = &self.state().requests[r];
        let generator = &self.model.generators[row.generator];
        (
            &generator.frame.arbitration,
            row.generated_ps,
            &generator.id,
            row.ordinal,
        )
    }
    fn reception_plan(&self, r: usize, eof: u64) -> Result<Vec<(usize, u64, u64)>> {
        let generator = &self.model.generators[self.state().requests[r].generator];
        let source = &self.model.controllers[generator.source];
        self.model
            .controllers
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != generator.source)
            .map(|(index, receiver)| {
                let arrival = add(add(eof, source.tx_channel_ps)?, receiver.rx_channel_ps)?;
                Ok((index, arrival, add(arrival, receiver.rx_processing_ps)?))
            })
            .collect()
    }
    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Generate(g, ordinal) => {
                let generator = &self.model.generators[g];
                if generator.times_ps.get(ordinal as usize) != Some(&self.owned.now) {
                    return Err(Diagnostic::execution("CAN FD generator cursor invariant"));
                }
                let ready = add(
                    self.owned.now,
                    self.model.controllers[generator.source].tx_processing_ps,
                )?;
                let r = self.state().requests.len();
                let notice = self.notification(g, ordinal, ready, None).encode()?;
                let mut events = Vec::new();
                if let Some(&next) = generator.times_ps.get(ordinal as usize + 1) {
                    events.push((
                        next,
                        1,
                        next,
                        g,
                        ordinal + 1,
                        Event::Generate(g, ordinal + 1),
                    ));
                }
                if ready == self.owned.now {
                    if (self.owned.queues[generator.source].len() as u64)
                        < self.model.controllers[generator.source].queue_capacity
                    {
                        events.extend(self.start_reservation());
                    }
                } else {
                    events.push((
                        ready,
                        1,
                        self.owned.now,
                        g,
                        ordinal,
                        Event::Ready(r, notice),
                    ));
                }
                let reserved = self.reserve(events)?;
                let row = CanFdRequest {
                    request_id: format!("{}:{ordinal}", generator.id),
                    generator: g,
                    ordinal,
                    time_ps: self.owned.now,
                    generated_ps: self.owned.now,
                    planned_ready_ps: ready,
                    ready_ps: None,
                    sof_ps: None,
                    eof_ps: None,
                    release_ps: None,
                    planned_eof_ps: None,
                    planned_release_ps: None,
                    state: "pending".into(),
                    drop_reason: None,
                };
                self.state_mut().requests.push(row);
                self.publish(reserved);
                if ready == self.owned.now {
                    self.ready_commit(r);
                }
            }
            Event::Ready(r, body) => {
                let row = self.state().requests.get(r).ok_or_else(|| {
                    Diagnostic::execution("CAN FD ready request reference invariant")
                })?;
                if row.state != "pending" || row.planned_ready_ps != self.owned.now {
                    return Err(Diagnostic::execution("CAN FD ready notification invariant"));
                }
                if protocol::Notification::decode(&body, protocol::TX_SCHEMA)?
                    != self.notification(row.generator, row.ordinal, self.owned.now, None)
                {
                    return Err(Diagnostic::execution(
                        "CAN FD ready source/generation invariant",
                    ));
                }
                let source = self.model.generators[row.generator].source;
                let events = if (self.owned.queues[source].len() as u64)
                    < self.model.controllers[source].queue_capacity
                {
                    self.start_reservation()
                } else {
                    Vec::new()
                };
                let reserved = self.reserve(events)?;
                self.ready_commit(r);
                self.publish(reserved);
            }
            Event::Start => {
                if self.owned.active.is_some() {
                    self.owned.starts.remove(&self.owned.now);
                    return Ok(());
                }
                let winner = self
                    .owned
                    .queues
                    .iter()
                    .flat_map(|q| q.iter().copied())
                    .min_by(|&a, &b| self.request_order(a).cmp(&self.request_order(b)));
                let Some(r) = winner else {
                    self.owned.starts.remove(&self.owned.now);
                    return Ok(());
                };
                let generator = &self.model.generators[self.state().requests[r].generator];
                let source = generator.source;
                let eof = add(self.owned.now, generator.frame.duration_ps)?;
                let release = add(self.owned.now, generator.frame.occupancy_ps)?;
                // Preflight all later reception reservations before committing SOF.
                for (receiver, arrival, _) in self.reception_plan(r, eof)? {
                    self.notification(
                        self.state().requests[r].generator,
                        self.state().requests[r].ordinal,
                        arrival,
                        Some(receiver),
                    )
                    .encode()?;
                }
                let reserved = self.reserve(vec![
                    (eof, 0, 0, 0, 0, Event::Eof(r)),
                    (release, 0, 0, 0, 0, Event::Release(r)),
                ])?;
                self.owned.starts.remove(&self.owned.now);
                self.owned.queues[source].retain(|&index| index != r);
                self.owned.active = Some(r);
                let now = self.owned.now;
                let row = &mut self.state_mut().requests[r];
                row.time_ps = now;
                row.sof_ps = Some(now);
                row.planned_eof_ps = Some(eof);
                row.planned_release_ps = Some(release);
                row.state = "transmitting".into();
                self.publish(reserved);
            }
            Event::Eof(r) => {
                let row = &self.state().requests[r];
                if self.owned.active != Some(r)
                    || row.state != "transmitting"
                    || row.planned_eof_ps != Some(self.owned.now)
                {
                    return Err(Diagnostic::execution("CAN FD EOF ownership invariant"));
                }
                let plan = self.reception_plan(r, self.owned.now)?;
                let first = self.state().receptions.len();
                let events = plan
                    .iter()
                    .enumerate()
                    .map(|(offset, (receiver, time, _))| {
                        Ok((
                            *time,
                            0,
                            0,
                            0,
                            0,
                            Event::Arrival(
                                first + offset,
                                self.notification(
                                    row.generator,
                                    row.ordinal,
                                    *time,
                                    Some(*receiver),
                                )
                                .encode()?,
                            ),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let reserved = self.reserve(events)?;
                let now = self.owned.now;
                let row = &mut self.state_mut().requests[r];
                row.time_ps = now;
                row.eof_ps = Some(now);
                row.state = "serialized".into();
                self.state_mut().receptions.extend(plan.into_iter().map(
                    |(receiver, arrival, completed)| CanFdReception {
                        request: r,
                        receiver,
                        time_ps: now,
                        planned_arrival_ps: arrival,
                        planned_completed_ps: completed,
                        arrival_ps: None,
                        completed_ps: None,
                        state: "pending".into(),
                    },
                ));
                self.publish(reserved);
            }
            Event::Release(r) => {
                if self.owned.active != Some(r)
                    || self.state().requests[r].planned_release_ps != Some(self.owned.now)
                {
                    return Err(Diagnostic::execution("CAN FD release ownership invariant"));
                }
                let reserved = self.reserve(self.start_reservation())?;
                let now = self.owned.now;
                let row = &mut self.state_mut().requests[r];
                row.time_ps = now;
                row.release_ps = Some(now);
                self.owned.active = None;
                self.mark_start();
                self.publish(reserved);
            }
            Event::Arrival(index, body) => {
                let row = &self.state().receptions[index];
                if row.state != "pending"
                    || row.planned_arrival_ps != self.owned.now
                    || self.state().requests[row.request].eof_ps.is_none()
                {
                    return Err(Diagnostic::execution(
                        "CAN FD arrival notification invariant",
                    ));
                }
                let request = &self.state().requests[row.request];
                if protocol::Notification::decode(&body, protocol::RX_SCHEMA)?
                    != self.notification(
                        request.generator,
                        request.ordinal,
                        self.owned.now,
                        Some(row.receiver),
                    )
                {
                    return Err(Diagnostic::execution(
                        "CAN FD arrival receiver/generation invariant",
                    ));
                }
                let generator =
                    &self.model.generators[self.state().requests[row.request].generator];
                let accepts = super::can::protocol::accepts(
                    &self.model.controllers[row.receiver].rx_filter,
                    &Frame {
                        format: generator.frame.format.clone(),
                        id: generator.frame.id,
                        data: generator.frame.data.clone(),
                    },
                );
                let completed = row.planned_completed_ps;
                let events = if accepts && completed > self.owned.now {
                    vec![(completed, 0, 0, 0, 0, Event::Completed(index))]
                } else {
                    Vec::new()
                };
                let reserved = self.reserve(events)?;
                let now = self.owned.now;
                let row = &mut self.state_mut().receptions[index];
                row.time_ps = now;
                row.arrival_ps = Some(now);
                row.state = if !accepts {
                    "filtered"
                } else if completed == now {
                    row.completed_ps = Some(now);
                    "completed"
                } else {
                    "processing"
                }
                .into();
                self.publish(reserved);
            }
            Event::Completed(index) => {
                let row = &self.state().receptions[index];
                if row.state != "processing" || row.planned_completed_ps != self.owned.now {
                    return Err(Diagnostic::execution("CAN FD completion invariant"));
                }
                let now = self.owned.now;
                let row = &mut self.state_mut().receptions[index];
                row.time_ps = now;
                row.completed_ps = Some(now);
                row.state = "completed".into();
            }
        }
        Ok(())
    }
    fn initialize(&mut self) -> Result<()> {
        let events = self
            .model
            .generators
            .iter()
            .enumerate()
            .filter_map(|(g, generator)| {
                generator
                    .times_ps
                    .first()
                    .map(|&time| (time, 1, time, g, 0, Event::Generate(g, 0)))
            })
            .collect();
        let reserved = self.reserve(events)?;
        self.publish(reserved);
        self.owned.snapshot.common.pending_events = self.owned.heap.len() as u64;
        Ok(())
    }
    fn refresh_prefix(&mut self, horizon: u64) {
        self.owned.snapshot.common.end_ps = horizon;
        self.owned.snapshot.common.pending_events = self.owned.heap.len() as u64;
    }
    fn terminal(&mut self) {
        if !self.owned.snapshot.common.partial {
            self.owned.snapshot.common.termination =
                if self.prepared.common.time_limit_ps == 0 || !self.owned.heap.is_empty() {
                    "time_limit"
                } else {
                    "events_exhausted"
                }
                .into();
            self.owned.snapshot.common.end_ps = self.prepared.common.time_limit_ps;
        }
        self.owned.snapshot.common.pending_events = self.owned.heap.len() as u64;
    }
    fn step(&mut self) -> StepOutcome {
        if self.owned.snapshot.common.partial || self.owned.snapshot.common.spool_error.is_some() {
            return StepOutcome::Failed;
        }
        let Some(Reverse((key, event))) = self.owned.heap.peek().cloned() else {
            self.terminal();
            return StepOutcome::Exhausted;
        };
        if key.0 >= self.prepared.common.time_limit_ps {
            self.terminal();
            return StepOutcome::Exhausted;
        }
        let _event_loop = super::timing::event_loop();
        self.owned.now = key.0;
        self.owned.heap.pop();
        let failed_event = event.clone();
        let result =
            if self.owned.snapshot.common.committed_events >= self.prepared.common.max_events {
                Err(limit("CAN FD max-events exceeded"))
            } else {
                self.handle(event)
            };
        if let Err(d) = result {
            self.owned.heap.push(Reverse((key, failed_event)));
            self.owned.snapshot.common.partial = true;
            self.owned.snapshot.common.termination = "execution_failed".into();
            self.owned.snapshot.common.end_ps = key.0;
            self.owned.snapshot.common.diagnostics.push(d.with_runtime(
                "run",
                Some(key.0),
                Some(key.5),
                None,
            ));
            self.owned.snapshot.common.pending_events = self.owned.heap.len() as u64;
            return StepOutcome::Failed;
        }
        self.owned.snapshot.common.committed_events += 1;
        self.owned.snapshot.common.last_event_time_ps = Some(key.0);
        self.owned.snapshot.common.pending_events = self.owned.heap.len() as u64;
        if !self.owned.snapshot.common.checkpoint() {
            return StepOutcome::Failed;
        }
        StepOutcome::Committed
    }
}

fn initial_state(prepared: &PreparedSimulation) -> Result<EngineState> {
    let model = prepared
        .canfd
        .as_ref()
        .ok_or_else(|| Diagnostic::execution("missing CAN FD prepared model"))?;
    Ok(EngineState {
        heap: BinaryHeap::new(),
        sequence: 0,
        now: 0,
        active: None,
        starts: BTreeSet::new(),
        queues: vec![Vec::new(); model.controllers.len()],
        snapshot: Snapshot {
            registered: None,
            network: None,
            axi: None,
            soc: None,
            memory_ipc: None,
            common: CommonSnapshot {
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
            can: CanSnapshot {
                archive: None,
                bus_state: "idle".into(),
                bus_states: Vec::new(),
                requests: Vec::new(),
                receivers: Vec::new(),
            },
            gateway: GatewaySnapshot::default(),
            ethernet: None,
            canfd: Some(CanFdSnapshot::default()),
        },
    })
}

pub(crate) struct Stepper {
    prepared: Arc<PreparedSimulation>,
    owned: Option<EngineState>,
    initialized: bool,
}
impl Stepper {
    pub(crate) fn new(prepared: Arc<PreparedSimulation>) -> Result<Self> {
        let mut owned = initial_state(&prepared)?;
        owned.snapshot.common.end_ps = 0;
        Ok(Self {
            prepared,
            owned: Some(owned),
            initialized: false,
        })
    }
    fn with_engine<T>(&mut self, operation: impl FnOnce(&mut Engine<'_>) -> T) -> T {
        let prepared = Arc::clone(&self.prepared);
        let mut engine = Engine {
            prepared: &prepared,
            model: prepared.canfd.as_ref().expect("validated FD model"),
            owned: self.owned.take().expect("owned FD state"),
        };
        let result = operation(&mut engine);
        self.owned = Some(engine.owned);
        result
    }
    pub(crate) fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Err(Diagnostic::execution("CAN FD initialized twice"));
        }
        self.with_engine(|engine| engine.initialize())?;
        self.initialized = true;
        Ok(())
    }
    pub(crate) fn step(&mut self) -> StepOutcome {
        self.with_engine(|engine| engine.step())
    }
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.owned.as_ref().expect("owned FD state").snapshot
    }
    pub(crate) fn snapshot_mut(&mut self) -> &mut Snapshot {
        &mut self.owned.as_mut().expect("owned FD state").snapshot
    }
    pub(crate) fn refresh_prefix(&mut self, horizon: u64) -> Result<()> {
        self.with_engine(|engine| engine.refresh_prefix(horizon));
        Ok(())
    }
    pub(crate) fn finish_once(&mut self) {}
}

pub(super) fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot> {
    let owned = initial_state(prepared)?;
    let model = prepared.canfd.as_ref().expect("validated FD model");
    let mut engine = Engine {
        prepared,
        model,
        owned,
    };
    engine.initialize()?;
    let _event_loop = super::timing::event_loop();
    while engine.step() == StepOutcome::Committed {}
    Ok(engine.owned.snapshot)
}
