//! Sparse edge scheduling with atomic callback preflight and committed journal.
use crate::snapshot::axi::{AxiHandshake, AxiRequest, AxiSnapshot};
use crate::snapshot::{Point, Snapshot};
use crate::types::axi::PreparedAxi;
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
pub mod protocol;
type Result<T> = std::result::Result<T, Diagnostic>;
type Key = (u64, u8, usize, u64, u64);
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Event {
    Generate(usize, u64),
    Wake,
    Arbitrate,
    Handshake(usize, String, u64, u64, String),
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
fn failed(message: &str) -> Diagnostic {
    let mut diagnostic = Diagnostic::execution(message);
    diagnostic.details = Some(json!({"reason":"model_failed"}));
    diagnostic
}
fn add(a: u64, b: u64) -> Result<u64> {
    u64::try_from(a as u128 + b as u128).map_err(|_| limit("AXI timestamp overflow"))
}
fn multiply(a: u64, b: u64) -> Result<u64> {
    u64::try_from(a as u128 * b as u128).map_err(|_| limit("AXI edge timestamp overflow"))
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 15) as usize] as char);
    }
    result
}
struct Engine<'a> {
    prepared: &'a PreparedSimulation,
    model: &'a PreparedAxi,
    heap: BinaryHeap<Reverse<(Key, Event, String)>>,
    sequence: u64,
    now: u64,
    cursor: usize,
    active: Option<usize>,
    stage: Option<(String, u64, u64, u64)>,
    queues: Vec<VecDeque<usize>>,
    outstanding: Vec<u64>,
    wake: Option<u64>,
    arbitration: Option<u64>,
    effect: u64,
    event_sequence: u64,
    snapshot: Snapshot,
}
impl Engine<'_> {
    fn internal(&self, event: &Event) -> Option<protocol::Internal> {
        match event {
            Event::Generate(g, ordinal) => Some(protocol::Internal::Generate {
                generator_id: self.model.generators[*g].id.clone(),
                ordinal: *ordinal,
            }),
            Event::Wake => Some(protocol::Internal::Wake {
                interconnect: self.model.interconnect.clone(),
            }),
            Event::Handshake(r, channel, beat, _, _) => Some(protocol::Internal::Handshake {
                request_id: self.state().requests[*r].request_id.clone(),
                channel: channel.clone(),
                beat: matches!(channel.as_str(), "W" | "R").then_some(*beat),
            }),
            Event::Arbitrate => None,
        }
    }
    fn state(&self) -> &AxiSnapshot {
        self.snapshot.axi.as_ref().unwrap()
    }
    fn state_mut(&mut self) -> &mut AxiSnapshot {
        self.snapshot.axi.as_mut().unwrap()
    }
    fn reserve(
        &self,
        events: Vec<(u64, u8, usize, u64, Event)>,
    ) -> Result<Vec<Reverse<(Key, Event, String)>>> {
        self.sequence
            .checked_add(events.len() as u64)
            .ok_or_else(|| limit("AXI event sequence overflow"))?;
        events
            .into_iter()
            .enumerate()
            .map(|(offset, (time, phase, g, ordinal, event))| {
                if time < self.now {
                    return Err(failed("AXI timer before callback"));
                }
                Ok(Reverse((
                    (time, phase, g, ordinal, self.sequence + offset as u64),
                    event.clone(),
                    self.internal(&event)
                        .map(|payload| payload.encode())
                        .transpose()?
                        .unwrap_or_default(),
                )))
            })
            .collect()
    }
    fn publish(&mut self, events: Vec<Reverse<(Key, Event, String)>>) {
        self.sequence += events.len() as u64;
        self.heap.extend(events);
    }
    fn point(
        &mut self,
        source: usize,
        metric: &str,
        value: u64,
        request: Option<String>,
        reason: Option<String>,
    ) {
        self.snapshot.common.points.push(Point {
            event_seq: Some(self.event_sequence),
            effect_seq: Some(self.effect),
            time_ps: self.now,
            target: if metric == "axi.channel_stall_cycles" {
                self.model.interconnect.clone()
            } else {
                self.model.managers[source].id.clone()
            },
            metric: metric.into(),
            value,
            request_id: request,
            receiver: None,
            reason,
        });
        self.effect += 1;
    }
    fn gauges(&mut self, source: usize, request: String) {
        self.point(
            source,
            "axi.queue_length",
            self.queues[source].len() as u64,
            Some(request.clone()),
            None,
        );
        self.point(
            source,
            "axi.outstanding",
            self.outstanding[source],
            Some(request),
            None,
        );
    }
    fn schedule_arbitration(&self) -> Vec<(u64, u8, usize, u64, Event)> {
        if self.arbitration == Some(self.now) {
            vec![]
        } else {
            vec![(self.now, 2, 0, 0, Event::Arbitrate)]
        }
    }
    fn handshake(
        &self,
        r: usize,
        channel: &str,
        beat: u64,
        valid_edge: u64,
        search_edge: u64,
    ) -> Result<(u64, u8, usize, u64, Event)> {
        let row = &self.state().requests[r];
        let source = self.model.generators[row.generator].source;
        let ram = &self.model.ram;
        let manager = &self.model.managers[source];
        let pattern = match channel {
            "AW" => &ram.aw_ready,
            "W" => &ram.w_ready,
            "AR" => &ram.ar_ready,
            "B" => &manager.b_ready,
            "R" => &manager.r_ready,
            _ => return Err(failed("invalid AXI stage")),
        };
        let mut h = search_edge;
        for j in 0..pattern.len() as u64 {
            let edge = add(search_edge, j)?;
            if pattern.as_bytes()[(edge % pattern.len() as u64) as usize] == b'1' {
                h = edge;
                break;
            }
        }
        let time = multiply(h, self.model.clock_period_ps)?;
        let valid = multiply(valid_edge, self.model.clock_period_ps)?;
        Ok((
            time,
            0,
            0,
            0,
            Event::Handshake(
                r,
                channel.into(),
                beat,
                valid,
                protocol::encode(&self.payload(r, channel, beat), &Self::schema(channel))?,
            ),
        ))
    }
    fn schema(channel: &str) -> String {
        format!(
            "axi4.transaction.v1.{}",
            match channel {
                "AW" => "Aw",
                "AR" => "Ar",
                c => c,
            }
        )
    }
    fn payload(&self, r: usize, channel: &str, beat: u64) -> Value {
        let row = &self.state().requests[r];
        let generator = &self.model.generators[row.generator];
        let t = &generator.transaction;
        match channel {
            "AW" | "AR" => {
                json!({"request_id":row.request_id,"manager":self.model.managers[generator.source].id,"target":self.model.ram.id,"address":t.address.to_string(),"len":(t.beats-1).to_string(),"size":2,"burst":1,"id":0})
            }
            "W" => {
                json!({"request_id":row.request_id,"beat":beat.to_string(),"data_hex":t.write_data[beat as usize],"strb":t.write_strobes[beat as usize].to_string(),"last":beat+1==t.beats})
            }
            "B" => json!({"request_id":row.request_id,"id":0,"resp":self.response(r)}),
            "R" => {
                let response = self.response(r);
                let data = if response == "OKAY" {
                    let start = (t.address + 4 * beat - self.model.ram.base) as usize;
                    hex(&self.state().memory[start..start + 4])
                } else {
                    "00000000".into()
                };
                json!({"request_id":row.request_id,"id":0,"beat":beat.to_string(),"data_hex":data,"resp":response,"last":beat+1==t.beats})
            }
            _ => Value::Null,
        }
    }
    fn response(&self, r: usize) -> String {
        let t = &self.model.generators[self.state().requests[r].generator].transaction;
        let ram = &self.model.ram;
        let end = t.address + 4 * t.beats;
        if t.address < ram.base || end > ram.base + ram.size {
            "DECERR"
        } else if ram.error_ranges.iter().any(|e| {
            t.address < e.end && end > e.start && (e.access == "both" || e.access == t.operation)
        }) {
            "SLVERR"
        } else {
            "OKAY"
        }
        .into()
    }
    fn handle(&mut self, event: Event, body: &str) -> Result<()> {
        if let Some(expected) = self.internal(&event) {
            if protocol::Internal::decode(body, expected.schema())? != expected {
                return Err(failed("AXI internal event does not match context"));
            }
        }
        self.effect = 0;
        match event {
            Event::Generate(g, ordinal) => {
                let generator = &self.model.generators[g];
                if generator.times_ps.get(ordinal as usize) != Some(&self.now) {
                    return Err(failed("AXI generator invariant"));
                }
                let p = self.model.clock_period_ps;
                let edge = self.now / p + u64::from(self.now % p != 0);
                let eligible = multiply(edge, p)?;
                let source = generator.source;
                let accepted =
                    self.outstanding[source] < self.model.managers[source].max_outstanding;
                let r = self.state().requests.len();
                let mut events = vec![];
                if let Some(&time) = generator.times_ps.get(ordinal as usize + 1) {
                    if time < self.prepared.common.time_limit_ps {
                        events.push((time, 1, g, ordinal + 1, Event::Generate(g, ordinal + 1)));
                    }
                }
                let mut new_wake = None;
                if accepted && self.active.is_none() {
                    if eligible == self.now {
                        events.extend(self.schedule_arbitration());
                    } else if self.wake.is_none_or(|old| eligible < old) {
                        events.push((eligible, 0, 0, 0, Event::Wake));
                        new_wake = Some(eligible);
                    }
                }
                let reserved = self.reserve(events)?;
                let request_id = format!("{}:{ordinal}", generator.id);
                let now = self.now;
                self.state_mut().requests.push(AxiRequest {
                    request_id: request_id.clone(),
                    generator: g,
                    ordinal,
                    time_ps: now,
                    generated_ps: now,
                    eligible_ps: eligible,
                    status: if accepted { "pending" } else { "dropped" }.into(),
                    grant_ps: None,
                    completed_ps: None,
                    response: None,
                    read_data: vec![],
                    drop_reason: (!accepted).then(|| "outstanding_full".into()),
                });
                if accepted {
                    self.queues[source].push_back(r);
                    self.outstanding[source] += 1;
                    if self.active.is_none() && eligible == self.now {
                        self.arbitration = Some(self.now);
                    }
                }
                if let Some(time) = new_wake {
                    if let Some(old) = self.wake {
                        self.heap
                            .retain(|Reverse((key, e, _))| !(key.0 == old && *e == Event::Wake));
                    }
                    self.wake = Some(time);
                }
                self.gauges(source, request_id);
                self.publish(reserved);
            }
            Event::Wake => {
                if self.wake != Some(self.now) {
                    return Err(failed("AXI Wake token invariant"));
                }
                let reserved = self.reserve(self.schedule_arbitration())?;
                self.wake = None;
                self.arbitration = Some(self.now);
                self.publish(reserved);
            }
            Event::Arbitrate => {
                if self.arbitration != Some(self.now) {
                    return Err(failed("AXI arbitration invariant"));
                }
                if self.active.is_some() {
                    self.arbitration = None;
                    return Ok(());
                }
                let winner = (0..self.queues.len())
                    .map(|offset| (self.cursor + offset) % self.queues.len())
                    .find_map(|source| {
                        self.queues[source]
                            .front()
                            .copied()
                            .filter(|&r| self.state().requests[r].eligible_ps <= self.now)
                            .map(|r| (source, r))
                    });
                let Some((source, r)) = winner else {
                    let earliest = self
                        .queues
                        .iter()
                        .filter_map(|q| q.front())
                        .map(|&r| self.state().requests[r].eligible_ps)
                        .min();
                    let events = if let Some(time) = earliest {
                        if self.wake != Some(time) {
                            vec![(time, 0, 0, 0, Event::Wake)]
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };
                    let reserved = self.reserve(events)?;
                    self.arbitration = None;
                    if let Some(time) = earliest {
                        self.wake = Some(time);
                    }
                    self.publish(reserved);
                    return Ok(());
                };
                let channel = if self.model.generators[self.state().requests[r].generator]
                    .transaction
                    .operation
                    == "write"
                {
                    "AW"
                } else {
                    "AR"
                };
                let edge = add(self.now / self.model.clock_period_ps, 1)?;
                let event = self.handshake(r, channel, 0, edge, edge)?;
                let stage = (
                    channel.into(),
                    0,
                    event.0,
                    multiply(edge, self.model.clock_period_ps)?,
                );
                let reserved = self.reserve(vec![event])?;
                self.arbitration = None;
                if let Some(time) = self.wake.take() {
                    self.heap
                        .retain(|Reverse((key, e, _))| !(key.0 == time && *e == Event::Wake));
                }
                self.queues[source].pop_front();
                self.cursor = (source + 1) % self.queues.len();
                self.active = Some(r);
                self.stage = Some(stage);
                let now = self.now;
                let row = &mut self.state_mut().requests[r];
                row.status = "active".into();
                row.grant_ps = Some(now);
                row.time_ps = now;
                let id = row.request_id.clone();
                let wait = now - row.generated_ps;
                self.gauges(source, id.clone());
                self.point(source, "axi.wait_ps", wait, Some(id), None);
                self.publish(reserved);
            }
            Event::Handshake(r, channel, beat, valid, body) => {
                if self.active != Some(r)
                    || self.stage != Some((channel.clone(), beat, self.now, valid))
                {
                    return Err(failed("AXI handshake ownership/stage invariant"));
                }
                if protocol::decode(&body, &Self::schema(&channel))?
                    != self.payload(r, &channel, beat)
                {
                    return Err(failed("AXI payload does not match committed state"));
                }
                let row = &self.state().requests[r];
                let generator = &self.model.generators[row.generator];
                let source = generator.source;
                let t = &generator.transaction;
                let edge = self.now / self.model.clock_period_ps;
                let address = t.address + 4 * beat;
                let response = if matches!(channel.as_str(), "AW" | "AR") {
                    self.response(r)
                } else {
                    row.response
                        .clone()
                        .ok_or_else(|| failed("AXI missing decoded response"))?
                };
                let mut handshake = AxiHandshake {
                    request: r,
                    channel: channel.clone(),
                    beat: None,
                    time_ps: self.now,
                    valid_since_ps: valid,
                    address: None,
                    data_hex: None,
                    wstrb: None,
                    last: None,
                    response: None,
                };
                let mut delta = vec![];
                let mut read = None;
                let mut next = None;
                let mut done = false;
                match channel.as_str() {
                    "AW" => {
                        handshake.address = Some(t.address);
                        let valid_edge =
                            add(row.grant_ps.unwrap() / self.model.clock_period_ps, 1)?;
                        next = Some(self.handshake(r, "W", 0, valid_edge, add(edge, 1)?)?);
                    }
                    "AR" => {
                        handshake.address = Some(t.address);
                        let e = add(edge, self.model.ram.read_latency_cycles)?;
                        next = Some(self.handshake(r, "R", 0, e, e)?);
                    }
                    "W" => {
                        let data = &t.write_data[beat as usize];
                        let strobe = t.write_strobes[beat as usize];
                        handshake.beat = Some(beat);
                        handshake.data_hex = Some(data.clone());
                        handshake.wstrb = Some(strobe);
                        handshake.last = Some(beat + 1 == t.beats);
                        if response == "OKAY" {
                            for j in 0..4 {
                                if strobe & (1 << j) != 0 {
                                    delta.push((
                                        (address - self.model.ram.base + j) as usize,
                                        u8::from_str_radix(
                                            &data[(j * 2) as usize..(j * 2 + 2) as usize],
                                            16,
                                        )
                                        .map_err(|_| failed("AXI immutable data invariant"))?,
                                    ));
                                }
                            }
                        }
                        if beat + 1 == t.beats {
                            let e = add(edge, self.model.ram.write_response_cycles)?;
                            next = Some(self.handshake(r, "B", 0, e, e)?);
                        } else {
                            let e = add(edge, 1)?;
                            next = Some(self.handshake(r, "W", beat + 1, e, e)?);
                        }
                    }
                    "B" => {
                        handshake.response = Some(response.clone());
                        done = true;
                    }
                    "R" => {
                        let data = if response == "OKAY" {
                            hex(
                                &self.state().memory[(address - self.model.ram.base) as usize
                                    ..(address - self.model.ram.base + 4) as usize],
                            )
                        } else {
                            "00000000".into()
                        };
                        handshake.beat = Some(beat);
                        handshake.data_hex = Some(data.clone());
                        handshake.last = Some(beat + 1 == t.beats);
                        handshake.response = Some(response.clone());
                        read = Some(data);
                        if beat + 1 == t.beats {
                            done = true;
                        } else {
                            let e = add(edge, 1)?;
                            next = Some(self.handshake(r, "R", beat + 1, e, e)?);
                        }
                    }
                    _ => return Err(failed("AXI invalid channel")),
                }
                let mut events = vec![];
                let next_stage = next.as_ref().and_then(|(time, _, _, _, event)| {
                    if let Event::Handshake(_, c, b, v, _) = event {
                        Some((c.clone(), *b, *time, *v))
                    } else {
                        None
                    }
                });
                if let Some(event) = next {
                    events.push(event);
                }
                if done {
                    events.extend(self.schedule_arbitration());
                }
                let reserved = self.reserve(events)?;
                let now = self.now;
                let id = self.state().requests[r].request_id.clone();
                self.point(
                    source,
                    "axi.channel_stall_cycles",
                    (now - valid) / self.model.clock_period_ps,
                    Some(id.clone()),
                    Some(channel.clone()),
                );
                if !delta.is_empty() {
                    let state = self.state_mut();
                    for (index, value) in delta {
                        state.memory[index] = value;
                    }
                    state.memory_time_ps = now;
                }
                let row = &mut self.state_mut().requests[r];
                row.time_ps = now;
                row.response = Some(response);
                if let Some(data) = read {
                    row.read_data.push(data);
                }
                if done {
                    row.status = "completed".into();
                    row.completed_ps = Some(now);
                }
                self.state_mut().handshakes.push(handshake);
                self.stage = next_stage;
                if done {
                    self.active = None;
                    self.outstanding[source] -= 1;
                    self.arbitration = Some(now);
                    self.gauges(source, id.clone());
                    let latency = now - self.state().requests[r].generated_ps;
                    self.point(source, "axi.latency_ps", latency, Some(id), None);
                }
                self.publish(reserved);
            }
        }
        Ok(())
    }
    fn run(mut self) -> Snapshot {
        let event_loop = super::timing::event_loop();
        while let Some(Reverse((key, event, body))) = self.heap.peek().cloned() {
            if key.0 >= self.prepared.common.time_limit_ps {
                break;
            }
            self.now = key.0;
            self.heap.pop();
            self.event_sequence = key.4;
            let failed = event.clone();
            let result = if self.snapshot.common.committed_events >= self.prepared.common.max_events
            {
                Err(limit("AXI max-events exceeded"))
            } else {
                self.handle(event, &body)
            };
            if let Err(d) = result {
                self.heap.push(Reverse((key, failed, body)));
                self.snapshot.common.partial = true;
                self.snapshot.common.termination = "execution_failed".into();
                self.snapshot.common.end_ps = key.0;
                self.snapshot.common.diagnostics.push(d.with_runtime(
                    "run",
                    Some(key.0),
                    Some(key.4),
                    None,
                ));
                break;
            }
            self.snapshot.common.committed_events += 1;
            self.snapshot.common.last_event_time_ps = Some(key.0);
            if !self.snapshot.common.checkpoint() {
                break;
            }
        }
        drop(event_loop);
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
fn initialize(prepared: &PreparedSimulation) -> Result<Engine<'_>> {
    let model = prepared
        .axi
        .as_ref()
        .ok_or_else(|| failed("missing AXI prepared model"))?;
    let mut snapshot = Snapshot::empty(prepared);
    snapshot.axi = Some(AxiSnapshot {
        memory: model.ram.initial.clone(),
        ..AxiSnapshot::default()
    });
    if prepared.common.time_limit_ps > 0 {
        for m in &model.managers {
            for metric in ["axi.queue_length", "axi.outstanding"] {
                snapshot.common.points.push(Point {
                    event_seq: None,
                    effect_seq: None,
                    time_ps: 0,
                    target: m.id.clone(),
                    metric: metric.into(),
                    value: 0,
                    request_id: None,
                    receiver: None,
                    reason: None,
                });
            }
        }
    }
    let mut engine = Engine {
        prepared,
        model,
        heap: BinaryHeap::new(),
        sequence: 0,
        now: 0,
        cursor: 0,
        active: None,
        stage: None,
        queues: vec![VecDeque::new(); model.managers.len()],
        outstanding: vec![0; model.managers.len()],
        wake: None,
        arbitration: None,
        effect: 0,
        event_sequence: 0,
        snapshot,
    };
    let events = model
        .generators
        .iter()
        .enumerate()
        .filter_map(|(g, generator)| {
            generator
                .times_ps
                .first()
                .copied()
                .filter(|time| *time < prepared.common.time_limit_ps)
                .map(|time| (time, 1, g, 0, Event::Generate(g, 0)))
        })
        .collect();
    let reserved = engine.reserve(events)?;
    engine.publish(reserved);
    Ok(engine)
}

pub(super) fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot> {
    Ok(initialize(prepared)?.run())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn malformed_or_mismatched_timer_preserves_initial_committed_prefix() {
        let prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/verification/fixtures/axi/read-write.ini"),
        )
        .unwrap();
        for (body, reason) in [
            ("{}", "invalid_event"),
            (
                r#"{"generator_id":"another","ordinal":"0"}"#,
                "model_failed",
            ),
        ] {
            let mut engine = initialize(&prepared).unwrap();
            let Reverse((key, event, _)) = engine.heap.pop().unwrap();
            engine.heap.push(Reverse((key, event, body.into())));
            let snapshot = engine.run();
            assert!(snapshot.common.partial);
            assert_eq!(snapshot.common.committed_events, 0);
            assert_eq!(snapshot.common.pending_events, 2);
            assert_eq!(snapshot.common.diagnostics[0].code, "E-0002");
            assert_eq!(
                snapshot.common.diagnostics[0].details.as_ref().unwrap()["reason"],
                reason
            );
            assert!(snapshot.axi.as_ref().unwrap().requests.is_empty());
            assert_eq!(
                snapshot.axi.as_ref().unwrap().memory,
                prepared.axi.as_ref().unwrap().ram.initial
            );
        }
    }
}
