//! Deterministic CAN FD callbacks with completion, generation and arbitration phases.
use crate::snapshot::canfd::{CanFdReception, CanFdRequest, CanFdSnapshot};
use crate::snapshot::{CanSnapshot, CommonSnapshot, GatewaySnapshot, Snapshot};
use crate::types::canfd::PreparedCanFd;
use crate::types::{Diagnostic, Frame, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
pub mod protocol;

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
    }
}
fn add(a: u64, b: u64) -> Result<u64> {
    u64::try_from(a as u128 + b as u128).map_err(|_| limit("CAN FD timestamp overflow"))
}
struct Engine<'a> {
    prepared: &'a PreparedSimulation,
    model: &'a PreparedCanFd,
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
        self.snapshot.canfd.as_ref().unwrap()
    }
    fn state_mut(&mut self) -> &mut CanFdSnapshot {
        self.snapshot.canfd.as_mut().unwrap()
    }
    /// Validate the entire callback's event reservation before changing journal/state.
    fn reserve(
        &self,
        events: Vec<(u64, u8, u64, usize, u64, Event)>,
    ) -> Result<Vec<Reverse<(Key, Event)>>> {
        self.sequence
            .checked_add(events.len() as u64)
            .ok_or_else(|| limit("CAN FD event sequence overflow"))?;
        events
            .into_iter()
            .enumerate()
            .map(|(offset, (time, phase, generated, g, ordinal, event))| {
                if time < self.now {
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
                        self.sequence + offset as u64,
                    ),
                    event,
                )))
            })
            .collect()
    }
    fn publish(&mut self, events: Vec<Reverse<(Key, Event)>>) {
        self.sequence += events.len() as u64;
        self.heap.extend(events);
    }
    fn start_reservation(&self) -> Vec<(u64, u8, u64, usize, u64, Event)> {
        if self.starts.contains(&self.now) {
            Vec::new()
        } else {
            vec![(self.now, 2, 0, 0, 0, Event::Start)]
        }
    }
    fn mark_start(&mut self) {
        self.starts.insert(self.now);
    }
    fn ready_commit(&mut self, r: usize) {
        let g = self.state().requests[r].generator;
        let source = self.model.generators[g].source;
        let full =
            self.queues[source].len() as u64 >= self.model.controllers[source].queue_capacity;
        let now = self.now;
        let row = &mut self.state_mut().requests[r];
        row.time_ps = now;
        row.ready_ps = Some(now);
        row.state = if full { "dropped" } else { "queued" }.into();
        row.drop_reason = full.then(|| "queue_full".into());
        if !full {
            self.queues[source].push(r);
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
                if generator.times_ps.get(ordinal as usize) != Some(&self.now) {
                    return Err(Diagnostic::execution("CAN FD generator cursor invariant"));
                }
                let ready = add(
                    self.now,
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
                if ready == self.now {
                    if (self.queues[generator.source].len() as u64)
                        < self.model.controllers[generator.source].queue_capacity
                    {
                        events.extend(self.start_reservation());
                    }
                } else {
                    events.push((ready, 1, self.now, g, ordinal, Event::Ready(r, notice)));
                }
                let reserved = self.reserve(events)?;
                let row = CanFdRequest {
                    request_id: format!("{}:{ordinal}", generator.id),
                    generator: g,
                    ordinal,
                    time_ps: self.now,
                    generated_ps: self.now,
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
                if ready == self.now {
                    self.ready_commit(r);
                }
            }
            Event::Ready(r, body) => {
                let row = self.state().requests.get(r).ok_or_else(|| {
                    Diagnostic::execution("CAN FD ready request reference invariant")
                })?;
                if row.state != "pending" || row.planned_ready_ps != self.now {
                    return Err(Diagnostic::execution("CAN FD ready notification invariant"));
                }
                if protocol::Notification::decode(&body, protocol::TX_SCHEMA)?
                    != self.notification(row.generator, row.ordinal, self.now, None)
                {
                    return Err(Diagnostic::execution(
                        "CAN FD ready source/generation invariant",
                    ));
                }
                let source = self.model.generators[row.generator].source;
                let events = if (self.queues[source].len() as u64)
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
                if self.active.is_some() {
                    self.starts.remove(&self.now);
                    return Ok(());
                }
                let winner = self
                    .queues
                    .iter()
                    .flat_map(|q| q.iter().copied())
                    .min_by(|&a, &b| self.request_order(a).cmp(&self.request_order(b)));
                let Some(r) = winner else {
                    self.starts.remove(&self.now);
                    return Ok(());
                };
                let generator = &self.model.generators[self.state().requests[r].generator];
                let source = generator.source;
                let eof = add(self.now, generator.frame.duration_ps)?;
                let release = add(self.now, generator.frame.occupancy_ps)?;
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
                self.starts.remove(&self.now);
                self.queues[source].retain(|&index| index != r);
                self.active = Some(r);
                let now = self.now;
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
                if self.active != Some(r)
                    || row.state != "transmitting"
                    || row.planned_eof_ps != Some(self.now)
                {
                    return Err(Diagnostic::execution("CAN FD EOF ownership invariant"));
                }
                let plan = self.reception_plan(r, self.now)?;
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
                let now = self.now;
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
                if self.active != Some(r)
                    || self.state().requests[r].planned_release_ps != Some(self.now)
                {
                    return Err(Diagnostic::execution("CAN FD release ownership invariant"));
                }
                let reserved = self.reserve(self.start_reservation())?;
                let now = self.now;
                let row = &mut self.state_mut().requests[r];
                row.time_ps = now;
                row.release_ps = Some(now);
                self.active = None;
                self.mark_start();
                self.publish(reserved);
            }
            Event::Arrival(index, body) => {
                let row = &self.state().receptions[index];
                if row.state != "pending"
                    || row.planned_arrival_ps != self.now
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
                        self.now,
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
                let events = if accepts && completed > self.now {
                    vec![(completed, 0, 0, 0, 0, Event::Completed(index))]
                } else {
                    Vec::new()
                };
                let reserved = self.reserve(events)?;
                let now = self.now;
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
                if row.state != "processing" || row.planned_completed_ps != self.now {
                    return Err(Diagnostic::execution("CAN FD completion invariant"));
                }
                let now = self.now;
                let row = &mut self.state_mut().receptions[index];
                row.time_ps = now;
                row.completed_ps = Some(now);
                row.state = "completed".into();
            }
        }
        Ok(())
    }
    fn run(mut self) -> Snapshot {
        while let Some(Reverse((key, event))) = self.heap.peek().cloned() {
            if key.0 >= self.prepared.common.time_limit_ps {
                break;
            }
            self.now = key.0;
            self.heap.pop();
            let failed_event = event.clone();
            let result = if self.snapshot.common.committed_events >= self.prepared.common.max_events
            {
                Err(limit("CAN FD max-events exceeded"))
            } else {
                self.handle(event)
            };
            if let Err(d) = result {
                self.heap.push(Reverse((key, failed_event)));
                self.snapshot.common.partial = true;
                self.snapshot.common.termination = "execution_failed".into();
                self.snapshot.common.end_ps = key.0;
                self.snapshot.common.diagnostics.push(d);
                break;
            }
            self.snapshot.common.committed_events += 1;
            self.snapshot.common.last_event_time_ps = Some(key.0);
        }
        self.snapshot.common.pending_events = self.heap.len() as u64;
        if !self.snapshot.common.partial {
            self.snapshot.common.termination =
                if self.prepared.common.time_limit_ps == 0 || !self.heap.is_empty() {
                    "time_limit"
                } else {
                    "events_exhausted"
                }
                .into();
        }
        self.snapshot
    }
}
pub(super) fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot> {
    let model = prepared
        .canfd
        .as_ref()
        .ok_or_else(|| Diagnostic::execution("missing CAN FD prepared model"))?;
    let mut engine = Engine {
        prepared,
        model,
        heap: BinaryHeap::new(),
        sequence: 0,
        now: 0,
        active: None,
        starts: BTreeSet::new(),
        queues: vec![Vec::new(); model.controllers.len()],
        snapshot: Snapshot {
            axi: None,
            soc: None,
            memory_ipc: None,
            common: CommonSnapshot {
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
                bus_state: "idle".into(),
                bus_states: Vec::new(),
                requests: Vec::new(),
                receivers: Vec::new(),
            },
            gateway: GatewaySnapshot::default(),
            ethernet: None,
            canfd: Some(CanFdSnapshot::default()),
        },
    };
    let events = model
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
    let reserved = engine.reserve(events)?;
    engine.publish(reserved);
    Ok(engine.run())
}
