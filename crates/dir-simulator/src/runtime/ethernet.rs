//! Deterministic full-duplex Ethernet v1 engine. Every callback preflights its effects.
use crate::snapshot::ethernet::*;
use crate::snapshot::{CanSnapshot, CommonSnapshot, GatewaySnapshot, Point, Snapshot};
use crate::types::ethernet::*;
use crate::types::{Diagnostic, PreparedSimulation};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};

type Result<T> = std::result::Result<T, Diagnostic>;
fn failure(s: &str) -> Diagnostic {
    Diagnostic::execution(s)
}
fn overflow(s: &str) -> Diagnostic {
    let mut d = failure(s);
    d.code = "E-0004".into();
    d
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| overflow("Ethernet time overflow"))
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}
fn decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Diagnostic::prepare("invalid Ethernet payload range"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| Diagnostic::prepare("invalid hex"))
        })
        .collect()
}
fn mac(s: &str, broadcast: bool) -> Result<Vec<u8>> {
    let parts: Vec<_> = s.split(':').collect();
    if parts.len() != 6 || parts.iter().any(|p| p.len() != 2) {
        return Err(Diagnostic::prepare("invalid MAC"));
    }
    let b = decode(&parts.concat())?;
    if b.iter().all(|b| *b == 0) || (b[0] & 1 != 0 && !(broadcast && b.iter().all(|b| *b == 255))) {
        return Err(Diagnostic::prepare("invalid MAC range"));
    }
    Ok(b)
}
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc ^ 0xffff_ffff
}
/// Serialize an untagged Ethernet II frame, including minimum padding and wire-order FCS.
pub fn serialize_frame(
    src_mac: &str,
    dst_mac: &str,
    ether_type: u16,
    data: &str,
) -> Result<EthernetWireFrame> {
    if ether_type < 1536 || [0x8100, 0x88a8].contains(&ether_type) || data.len() > 3000 {
        return Err(Diagnostic::prepare("invalid Ethernet frame range"));
    }
    let source = mac(src_mac, false)?;
    let destination = mac(dst_mac, true)?;
    let payload = decode(data)?;
    let pad_bytes = 46usize.saturating_sub(payload.len());
    let mut bytes = destination;
    bytes.extend(source);
    bytes.extend(ether_type.to_be_bytes());
    bytes.extend(payload);
    bytes.resize(bytes.len() + pad_bytes, 0);
    let fcs = crc32(&bytes).to_le_bytes();
    bytes.extend(fcs);
    Ok(EthernetWireFrame {
        src_mac: src_mac.to_ascii_lowercase(),
        dst_mac: dst_mac.to_ascii_lowercase(),
        ether_type,
        data_hex: data.to_ascii_lowercase(),
        pad_bytes: pad_bytes as u64,
        mac_bytes: bytes.len() as u64,
        fcs_hex: hex(&fcs),
        mac_hex: hex(&bytes),
    })
}
fn duration(bits: u64, rate: u64) -> Result<u64> {
    let n = bits
        .checked_mul(1_000_000_000_000)
        .ok_or_else(|| overflow("Ethernet serialization overflow"))?;
    if rate == 0 {
        return Err(failure("zero Ethernet bitrate"));
    }
    Ok(n / rate + u64::from(n % rate != 0))
}
type Key = (u64, u64, u8, u64);
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Event {
    Dispatch,
    Generate(usize, u64),
    SourceReady(usize),
    Offer(usize),
    Start(usize),
    Eof(usize),
    Release(usize),
    Arrival(usize),
    Processed(usize),
    Complete(usize),
}
#[derive(Clone)]
struct Queue {
    waiting: Vec<VecDeque<usize>>,
    bytes: Vec<u64>,
    active: Option<usize>,
}
struct Engine<'a> {
    prepared: &'a PreparedSimulation,
    model: &'a PreparedEthernet,
    heap: BinaryHeap<Reverse<(Key, Event)>>,
    sequence: u64,
    cursors: Vec<u64>,
    future_generation: bool,
    now: Key,
    dirty: BTreeSet<(u64, u64, usize)>,
    queues: Vec<Queue>,
    frame_indices: Vec<usize>,
    direction_indices: Vec<usize>,
    snapshot: Snapshot,
}
impl Engine<'_> {
    fn eth(&self) -> &EthernetSnapshot {
        self.snapshot.ethernet.as_ref().unwrap()
    }
    fn eth_mut(&mut self) -> &mut EthernetSnapshot {
        self.snapshot.ethernet.as_mut().unwrap()
    }
    // Reservation calculation never mutates engine state. Publication happens after all checks.
    fn reserve(&self, events: &[(u64, u8, Event)]) -> Result<Vec<(Key, Event)>> {
        self.sequence
            .checked_add(events.len() as u64)
            .and_then(|n| {
                n.checked_add(
                    self.dirty.len().saturating_sub(
                        events
                            .iter()
                            .filter(|(_, _, event)| matches!(event, Event::Start(_)))
                            .count(),
                    ) as u64,
                )
            })
            .ok_or_else(|| overflow("Ethernet sequence overflow"))?;
        let mut seq = self.sequence;
        events
            .iter()
            .map(|(t, phase, event)| {
                if *t < self.now.0 {
                    return Err(failure("Ethernet reservation precedes current time"));
                }
                let delta = if *t == self.now.0 {
                    self.now
                        .1
                        .checked_add(u64::from(*phase < self.now.2))
                        .ok_or_else(|| overflow("Ethernet delta overflow"))?
                } else {
                    0
                };
                let key = (*t, delta, *phase, seq);
                seq = seq
                    .checked_add(1)
                    .ok_or_else(|| overflow("Ethernet sequence overflow"))?;
                Ok((key, event.clone()))
            })
            .collect()
    }
    fn publish(&mut self, events: Vec<(Key, Event)>) {
        for (key, event) in events {
            self.sequence = key.3 + 1;
            self.heap.push(Reverse((key, event)));
        }
    }
    fn point(
        &mut self,
        target: String,
        metric: &str,
        value: u64,
        frame: Option<usize>,
        receiver: Option<String>,
    ) {
        let request_id = frame.map(|f| self.eth().frames[f].frame_id.clone());
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
            request_id,
            receiver,
            reason: None,
        });
    }
    fn qos(&self) -> bool {
        self.prepared.common.profile == "ethernet.l2.qos.v1"
    }
    fn output_config(&self, d: usize) -> Option<&EthernetOutputConfig> {
        self.model
            .outputs
            .iter()
            .find(|o| o.port == self.model.directions[d].from_port)
    }
    fn waiting(&self, d: usize) -> u64 {
        self.queues[d].waiting.iter().map(|q| q.len() as u64).sum()
    }
    fn admit(&self, d: usize, priority: u8, mac_bytes: u64) -> Result<bool> {
        let output = &self.model.directions[d];
        if self.waiting(d) >= self.model.devices[output.source].queue_capacity {
            return Ok(false);
        }
        if let Some(config) = self.output_config(d) {
            let queue = &config.queues[priority as usize];
            if self.queues[d].waiting[priority as usize].len() as u64 >= queue.capacity_frames {
                return Ok(false);
            }
            let bytes = self.queues[d].bytes[priority as usize]
                .checked_add(mac_bytes)
                .ok_or_else(|| overflow("Ethernet queue byte overflow"))?;
            if queue
                .capacity_bytes
                .is_some_and(|capacity| bytes > capacity)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn queue_point(&mut self, d: usize, f: usize) {
        let priority = self.eth().frames[f].priority as usize;
        self.point(
            format!("{}.queue", self.model.directions[d].from_port),
            "queue_length",
            self.waiting(d),
            Some(f),
            None,
        );
        if self.qos() {
            let queue_id = format!("{}.queue.{priority}", self.model.directions[d].from_port);
            self.point(
                queue_id.clone(),
                "queue_length",
                self.queues[d].waiting[priority].len() as u64,
                Some(f),
                None,
            );
            self.point(
                queue_id,
                "queue_bytes",
                self.queues[d].bytes[priority],
                Some(f),
                None,
            );
        }
    }
    fn next_waiting(&self, d: usize) -> Option<(usize, usize)> {
        if self
            .output_config(d)
            .is_some_and(|config| config.scheduler == "strict_priority")
        {
            self.queues[d]
                .waiting
                .iter()
                .enumerate()
                .rev()
                .find_map(|(priority, q)| q.front().map(|t| (priority, *t)))
        } else {
            self.queues[d]
                .waiting
                .iter()
                .enumerate()
                .filter_map(|(priority, q)| q.front().map(|t| (priority, *t)))
                .min_by_key(|(_, t)| *t)
        }
    }
    fn offer(&mut self, f: usize, parent: Option<usize>, directions: &[usize]) -> Vec<String> {
        let frame_id = self.eth().frames[f].frame_id.clone();
        let priority = self.eth().frames[f].priority;
        let mac_bytes = self.eth().frames[f].wire.mac_bytes;
        let parent_transfer_id = parent.map(|p| self.eth().transfers[p].transfer_id.clone());
        let mut ids = Vec::new();
        for &d in directions {
            let direction = &self.model.directions[d];
            let transfer_id = format!("{frame_id}@{}", direction.from_port);
            ids.push(transfer_id.clone());
            let full = !self
                .admit(d, priority, mac_bytes)
                .expect("offer capacity preflighted");
            let row = EthernetTransferRecord {
                transfer_id,
                time_ps: self.now.0,
                frame_id: frame_id.clone(),
                parent_transfer_id: parent_transfer_id.clone(),
                queue_id: self
                    .qos()
                    .then(|| format!("{}.queue.{priority}", direction.from_port)),
                priority,
                from_port: direction.from_port.clone(),
                to_port: direction.to_port.clone(),
                queued_ps: self.now.0,
                sof_ps: None,
                eof_ps: None,
                release_ps: None,
                arrival_ps: None,
                planned_eof_ps: None,
                planned_release_ps: None,
                planned_arrival_ps: None,
                status: if full { "dropped" } else { "queued" }.into(),
                drop_reason: full.then(|| "queue_full".into()),
            };
            let index = self.eth().transfers.len();
            self.eth_mut().transfers.push(row);
            self.frame_indices.push(f);
            self.direction_indices.push(d);
            if !full {
                self.queues[d].waiting[priority as usize].push_back(index);
                self.queues[d].bytes[priority as usize] += mac_bytes;
                self.dirty.insert((self.now.0, self.now.1, d));
            }
            self.queue_point(d, f);
        }
        ids
    }
    fn next_generation(&self, cursors: &[u64]) -> Option<u128> {
        self.model
            .generators
            .iter()
            .enumerate()
            .filter_map(|(g, generator)| generator.time(cursors[g]))
            .min()
    }
    fn preflight_offer(
        &self,
        frame_id: &str,
        priority: u8,
        mac_bytes: u64,
        directions: &[usize],
    ) -> Result<()> {
        let mut dirty = self.dirty.clone();
        for &d in directions {
            let direction = &self.model.directions[d];
            let transfer_id = format!("{frame_id}@{}", direction.from_port);
            if self
                .eth()
                .transfers
                .iter()
                .any(|t| t.transfer_id == transfer_id)
            {
                return Err(failure("duplicate Ethernet transfer ID"));
            }
            if self.admit(d, priority, mac_bytes)? {
                dirty.insert((self.now.0, self.now.1, d));
            }
        }
        self.sequence
            .checked_add(dirty.len() as u64)
            .ok_or_else(|| overflow("Ethernet sequence overflow"))?;
        Ok(())
    }
    fn handle(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Dispatch => {
                let mut cursors = self.cursors.clone();
                let g = self
                    .model
                    .generators
                    .iter()
                    .enumerate()
                    .filter(|(g, generator)| {
                        generator.time(cursors[*g]) == Some(self.now.0 as u128)
                    })
                    .min_by(|(_, a), (_, b)| a.id.cmp(&b.id))
                    .map(|(g, _)| g)
                    .ok_or_else(|| failure("Ethernet generation cursor invariant"))?;
                let ordinal = cursors[g];
                cursors[g] = ordinal
                    .checked_add(1)
                    .ok_or_else(|| overflow("Ethernet ordinal overflow"))?;
                let mut events = vec![(self.now.0, 1, Event::Generate(g, ordinal))];
                let next = self.next_generation(&cursors);
                if let Some(time) =
                    next.filter(|time| *time < self.prepared.common.time_limit_ps as u128)
                {
                    events.push((time as u64, 1, Event::Dispatch));
                }
                let reserved = self.reserve(&events)?;
                self.cursors = cursors;
                self.future_generation =
                    next.is_some_and(|time| time >= self.prepared.common.time_limit_ps as u128);
                self.publish(reserved);
            }
            Event::Generate(g, ordinal) => {
                let generator = &self.model.generators[g];
                let device = &self.model.devices[generator.source];
                let f = self.eth().frames.len();
                let ready = add(self.now.0, device.tx_processing_delay_ps)?;
                let events = if ready > self.now.0 {
                    self.reserve(&[(ready, 0, Event::SourceReady(f))])?
                } else {
                    Vec::new()
                };
                if ready == self.now.0 {
                    let directions: Vec<_> = self
                        .model
                        .directions
                        .iter()
                        .enumerate()
                        .filter(|(_, d)| d.source == generator.source)
                        .map(|(i, _)| i)
                        .collect();
                    self.preflight_offer(
                        &format!("{}:{ordinal}", generator.id),
                        generator.priority,
                        generator.frame.mac_bytes,
                        &directions,
                    )?;
                }
                let now = self.now.0;
                self.eth_mut().frames.push(EthernetFrameRecord {
                    frame_id: format!("{}:{ordinal}", generator.id),
                    time_ps: now,
                    source: device.id.clone(),
                    wire: generator.frame.clone(),
                    flow_id: generator.flow_id.clone(),
                    priority: generator.priority,
                    deadline_ps: generator.deadline_ps,
                    generated_ps: now,
                    ready_ps: None,
                });
                self.publish(events);
                if ready == self.now.0 {
                    self.source_offer(f);
                }
            }
            Event::SourceReady(f) => {
                let events = self.reserve(&[(self.now.0, 1, Event::Offer(f))])?;
                self.publish(events);
            }
            Event::Offer(f) => {
                let source = &self.eth().frames[f].source;
                let directions: Vec<_> = self
                    .model
                    .directions
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| &self.model.devices[d.source].id == source)
                    .map(|(i, _)| i)
                    .collect();
                self.preflight_offer(
                    &self.eth().frames[f].frame_id,
                    self.eth().frames[f].priority,
                    self.eth().frames[f].wire.mac_bytes,
                    &directions,
                )?;
                self.source_offer(f);
            }
            Event::Start(d) => {
                if self.queues[d].active.is_some() || self.next_waiting(d).is_none() {
                    return Ok(());
                }
                let (priority, t) = self.next_waiting(d).unwrap();
                let f = self.frame_indices[t];
                let wire = &self.eth().frames[f].wire;
                let link = &self.model.directions[d];
                let wire_bits = wire
                    .mac_bytes
                    .checked_add(8)
                    .and_then(|n| n.checked_mul(8))
                    .ok_or_else(|| overflow("Ethernet wire length overflow"))?;
                let occupied_bits = wire
                    .mac_bytes
                    .checked_add(20)
                    .and_then(|n| n.checked_mul(8))
                    .ok_or_else(|| overflow("Ethernet occupied length overflow"))?;
                let eof = add(self.now.0, duration(wire_bits, link.bitrate_bps)?)?;
                let release = add(self.now.0, duration(occupied_bits, link.bitrate_bps)?)?;
                let arrival = add(eof, link.delay_ps)?;
                let events =
                    self.reserve(&[(eof, 0, Event::Eof(t)), (release, 0, Event::Release(t))])?;
                let remaining_bytes = self.queues[d].bytes[priority]
                    .checked_sub(wire.mac_bytes)
                    .ok_or_else(|| failure("Ethernet queue byte invariant"))?;
                self.queues[d].waiting[priority].pop_front();
                self.queues[d].bytes[priority] = remaining_bytes;
                self.queues[d].active = Some(t);
                self.queue_point(d, f);
                let now = self.now.0;
                let row = &mut self.eth_mut().transfers[t];
                row.time_ps = now;
                row.status = "transmitting".into();
                row.sof_ps = Some(now);
                row.planned_eof_ps = Some(eof);
                row.planned_release_ps = Some(release);
                row.planned_arrival_ps = Some(arrival);
                self.publish(events);
            }
            Event::Eof(t) => {
                let arrival = self.eth().transfers[t]
                    .planned_arrival_ps
                    .ok_or_else(|| failure("Ethernet missing planned arrival"))?;
                let events = self.reserve(&[(arrival, 1, Event::Arrival(t))])?;
                let now = self.now.0;
                let row = &mut self.eth_mut().transfers[t];
                row.eof_ps = Some(now);
                row.time_ps = now;
                row.status = "serialized".into();
                self.publish(events);
            }
            Event::Release(t) => {
                let d = self.direction_indices[t];
                let now = self.now.0;
                let row = &mut self.eth_mut().transfers[t];
                row.release_ps = Some(now);
                row.time_ps = now;
                self.queues[d].active = None;
                self.dirty.insert((self.now.0, self.now.1, d));
            }
            Event::Arrival(t) => {
                let f = self.frame_indices[t];
                let direction = &self.model.directions[self.direction_indices[t]];
                let device = &self.model.devices[direction.destination];
                let frame = &self.eth().frames[f];
                let bytes = decode(&frame.wire.mac_hex)
                    .map_err(|_| failure("Ethernet corrupted frame bytes"))?;
                if bytes.len() < 4
                    || crc32(&bytes[..bytes.len() - 4]).to_le_bytes() != bytes[bytes.len() - 4..]
                {
                    return Err(failure("Ethernet FCS invariant failed"));
                }
                let mismatch = device.kind == "endpoint"
                    && device.mac.as_deref() != Some(&frame.wire.dst_mac)
                    && frame.wire.dst_mac != "ff:ff:ff:ff:ff:ff";
                let ready = if mismatch {
                    None
                } else {
                    Some(add(
                        self.now.0,
                        if device.kind == "endpoint" {
                            device.rx_processing_delay_ps
                        } else {
                            device.forward_delay_ps
                        },
                    )?)
                };
                let r = self.eth().receptions.len();
                let events = if ready.is_some_and(|ready| ready > self.now.0) {
                    self.reserve(&[(ready.unwrap(), 0, Event::Processed(r))])?
                } else {
                    Vec::new()
                };
                let reception = EthernetReceptionRecord {
                    reception_id: format!("{}@rx", self.eth().transfers[t].transfer_id),
                    time_ps: self.now.0,
                    frame_id: frame.frame_id.clone(),
                    transfer_id: self.eth().transfers[t].transfer_id.clone(),
                    device: device.id.clone(),
                    ingress: direction.to_port.clone(),
                    observed_ps: self.now.0,
                    ready_ps: mismatch.then_some(self.now.0),
                    planned_ready_ps: ready,
                    status: if mismatch { "filtered" } else { "processing" }.into(),
                    reason: mismatch.then(|| "destination_mismatch".into()),
                    egress_transfer_ids: Vec::new(),
                };
                // Zero-delay processing is checked before committing the arrival callback.
                let outputs = if ready == Some(self.now.0) {
                    Some(self.outputs(t))
                } else {
                    None
                };
                if let Some(outputs) = &outputs {
                    self.preflight_offer(
                        &frame.frame_id,
                        frame.priority,
                        frame.wire.mac_bytes,
                        outputs,
                    )?;
                }
                let now = self.now.0;
                let transfer = &mut self.eth_mut().transfers[t];
                transfer.time_ps = now;
                transfer.arrival_ps = Some(now);
                self.eth_mut().receptions.push(reception);
                self.publish(events);
                if let Some(outputs) = outputs {
                    self.complete(r, t, outputs);
                }
            }
            Event::Processed(r) => {
                let events = self.reserve(&[(self.now.0, 1, Event::Complete(r))])?;
                self.publish(events);
            }
            Event::Complete(r) => {
                let t = self
                    .eth()
                    .transfers
                    .iter()
                    .position(|t| t.transfer_id == self.eth().receptions[r].transfer_id)
                    .ok_or_else(|| failure("Ethernet reception reference invariant"))?;
                let outputs = self.outputs(t);
                self.preflight_offer(
                    &self.eth().receptions[r].frame_id,
                    self.eth().frames[self.frame_indices[t]].priority,
                    self.eth().frames[self.frame_indices[t]].wire.mac_bytes,
                    &outputs,
                )?;
                self.complete(r, t, outputs);
            }
        }
        Ok(())
    }
    fn source_offer(&mut self, f: usize) {
        let source = &self.eth().frames[f].source;
        let directions: Vec<_> = self
            .model
            .directions
            .iter()
            .enumerate()
            .filter(|(_, d)| &self.model.devices[d.source].id == source)
            .map(|(i, _)| i)
            .collect();
        let now = self.now.0;
        self.eth_mut().frames[f].ready_ps = Some(now);
        self.eth_mut().frames[f].time_ps = now;
        self.offer(f, None, &directions);
    }
    fn outputs(&self, t: usize) -> Vec<usize> {
        let d = &self.model.directions[self.direction_indices[t]];
        let device = &self.model.devices[d.destination];
        if device.kind == "endpoint" {
            return Vec::new();
        }
        let frame = &self.eth().frames[self.frame_indices[t]].wire;
        if let Some(output) = device.fdb.get(&frame.dst_mac) {
            return self
                .model
                .directions
                .iter()
                .enumerate()
                .filter(|(_, e)| &e.from_port == output && e.to_port != crate_pair(&d.from_port))
                .map(|(i, _)| i)
                .collect();
        }
        self.model
            .directions
            .iter()
            .enumerate()
            .filter(|(_, e)| e.source == d.destination && e.to_port != crate_pair(&d.from_port))
            .map(|(i, _)| i)
            .collect()
    }
    fn complete(&mut self, r: usize, t: usize, outputs: Vec<usize>) {
        let f = self.frame_indices[t];
        let device =
            &self.model.devices[self.model.directions[self.direction_indices[t]].destination];
        let status = if device.kind == "endpoint" {
            "received"
        } else if outputs.is_empty() {
            "filtered"
        } else {
            "forwarded"
        };
        let ids = self.offer(f, Some(t), &outputs);
        if status == "received" {
            self.point(
                device.id.clone(),
                "ethernet.delivery_ps",
                self.now.0 - self.eth().frames[f].generated_ps,
                Some(f),
                Some(device.id.clone()),
            );
        }
        let now = self.now.0;
        let row = &mut self.eth_mut().receptions[r];
        row.time_ps = now;
        row.ready_ps = Some(now);
        row.status = status.into();
        row.reason = (status == "filtered").then(|| "same_ingress".into());
        row.egress_transfer_ids = ids;
    }
    fn run(mut self) -> Snapshot {
        loop {
            if let Some(&(time, delta, _)) = self.dirty.first() {
                if self
                    .heap
                    .peek()
                    .is_none_or(|Reverse((key, _))| (time, delta, 2) <= (key.0, key.1, key.2))
                {
                    let dirty: Vec<_> = self
                        .dirty
                        .range((time, delta, 0)..=(time, delta, usize::MAX))
                        .copied()
                        .collect();
                    let events: Vec<_> = dirty
                        .iter()
                        .map(|&(_, _, d)| (time, 2, Event::Start(d)))
                        .collect();
                    // Dirty batches retain their originating delta even after unrelated callbacks.
                    let previous = self.now;
                    self.now = (time, delta, 1, previous.3);
                    let reserved = self.reserve(&events);
                    self.now = previous;
                    match reserved {
                        Ok(events) => {
                            self.publish(events);
                            for key in dirty {
                                self.dirty.remove(&key);
                            }
                        }
                        Err(error) => {
                            self.fail(error, time);
                            break;
                        }
                    }
                }
            }
            let Some(Reverse((key, event))) = self.heap.peek().cloned() else {
                break;
            };
            if key.0 >= self.prepared.common.time_limit_ps {
                break;
            }
            if self.snapshot.common.committed_events >= self.prepared.common.max_events
                || key.1 >= self.prepared.common.max_delta_cycles
            {
                self.fail(
                    Diagnostic {
                        schema_version: 1,
                        code: "E-0004".into(),
                        stage: "run".into(),
                        message: "Ethernet execution limit exceeded".into(),
                    },
                    key.0,
                );
                break;
            }
            self.now = key;
            if let Err(error) = self.handle(event) {
                self.fail(error, key.0);
                break;
            }
            let popped = self.heap.pop().unwrap();
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
    fn fail(&mut self, error: Diagnostic, time: u64) {
        self.snapshot.common.partial = true;
        self.snapshot.common.termination = "execution_failed".into();
        self.snapshot.common.end_ps = time;
        self.snapshot.common.diagnostics.push(error);
    }
}
fn crate_pair(port: &str) -> String {
    let (owner, gate) = port.rsplit_once('.').unwrap();
    format!(
        "{owner}.{}",
        if gate == "tx" {
            "rx".into()
        } else {
            format!("rx_{}", gate.strip_prefix("tx_").unwrap())
        }
    )
}
fn initialize(prepared: &PreparedSimulation) -> Result<Engine<'_>> {
    let model = prepared
        .ethernet
        .as_ref()
        .ok_or_else(|| failure("missing prepared Ethernet model"))?;
    let mut engine = Engine {
        prepared,
        model,
        heap: BinaryHeap::new(),
        sequence: 0,
        cursors: vec![0; model.generators.len()],
        future_generation: false,
        now: (0, 0, 0, 0),
        dirty: BTreeSet::new(),
        queues: vec![
            Queue {
                waiting: vec![VecDeque::new(); 8],
                bytes: vec![0; 8],
                active: None
            };
            model.directions.len()
        ],
        frame_indices: Vec::new(),
        direction_indices: Vec::new(),
        snapshot: Snapshot {
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
            ethernet: Some(EthernetSnapshot::default()),
        },
    };
    for direction in &model.directions {
        engine.snapshot.common.points.push(Point {
            event_seq: None,
            effect_seq: None,
            time_ps: 0,
            target: format!("{}.queue", direction.from_port),
            metric: "queue_length".into(),
            value: 0,
            request_id: None,
            receiver: None,
            reason: None,
        });
    }
    if engine.qos() {
        for direction in &model.directions {
            for priority in 0..8 {
                for metric in ["queue_length", "queue_bytes"] {
                    engine.snapshot.common.points.push(Point {
                        event_seq: None,
                        effect_seq: None,
                        time_ps: 0,
                        target: format!("{}.queue.{priority}", direction.from_port),
                        metric: metric.into(),
                        value: 0,
                        request_id: None,
                        receiver: None,
                        reason: None,
                    });
                }
            }
        }
    }
    if let Some(time) = engine.next_generation(&engine.cursors) {
        if time < prepared.common.time_limit_ps as u128 {
            let reserved = engine.reserve(&[(time as u64, 1, Event::Dispatch)])?;
            engine.publish(reserved);
        } else {
            engine.future_generation = true;
        }
    }
    Ok(engine)
}
pub(super) fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot> {
    Ok(initialize(prepared)?.run())
}

#[cfg(test)]
mod tests;
