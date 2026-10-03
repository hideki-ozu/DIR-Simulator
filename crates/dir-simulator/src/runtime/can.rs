//! Classical CAN initialization, generation, arbitration and delivery callbacks.
use super::engine::Engine;
use super::scheduler::{Event, add, limit_error};
use crate::snapshot::{Point, Receiver, Request, RequestLineage};
use crate::types::{Diagnostic, PreparedSimulation};

pub mod protocol;

pub(super) type QueueEntry = (Vec<u8>, u64, String, u64, usize);

pub(super) fn wires(prepared: &PreparedSimulation) -> Result<Vec<protocol::WireFrame>, Diagnostic> {
    prepared
        .can
        .generators
        .iter()
        .map(|g| protocol::serialize(&g.frame))
        .collect()
}

impl Engine<'_> {
    pub(super) fn initialize_can_points(&mut self) {
        if self.prepared.common.time_limit_ps > 0 {
            for controller in &self.prepared.can.controllers {
                self.snapshot.common.points.push(Point {
                    event_seq: None,
                    effect_seq: None,
                    time_ps: 0,
                    target: format!("{}.txQueue", controller.id),
                    metric: "queue_length".into(),
                    value: 0,
                    request_id: None,
                    receiver: None,
                    reason: None,
                });
            }
            for gateway in &self.prepared.gateway.gateways {
                for &port in &gateway.ports {
                    self.snapshot.common.points.push(Point {
                        event_seq: None,
                        effect_seq: None,
                        time_ps: 0,
                        target: format!("{}.rxQueue", self.prepared.can.controllers[port].id),
                        metric: "gw_rx_queue_length".into(),
                        value: 0,
                        request_id: None,
                        receiver: None,
                        reason: None,
                    });
                }
            }
        }
    }
    pub(super) fn bitrate(&self, bus: usize) -> u64 {
        if self.prepared.common.profile == "can.cc.ideal.v1" {
            self.prepared.can.bitrate
        } else {
            self.prepared.can.buses[bus].bitrate
        }
    }
    pub(super) fn bus_state(&mut self, bus: usize, state: &str) {
        self.snapshot.can.bus_states[bus] = state.into();
        if bus == 0 {
            self.snapshot.can.bus_state = state.into();
        }
    }
    pub(super) fn queue_point(&mut self, source: usize, request: usize) {
        self.point(
            format!("{}.txQueue", self.prepared.can.controllers[source].id),
            "queue_length",
            self.queues[source].len() as u64,
            request,
            None,
        );
    }
    pub(super) fn next_generation(&self, cursors: &[u64]) -> Option<u128> {
        self.prepared
            .can
            .generators
            .iter()
            .zip(cursors)
            .filter_map(|(g, &ordinal)| g.schedule.time(ordinal))
            .min()
    }
    pub(super) fn dispatch(&mut self) -> Result<(), Diagnostic> {
        let mut cursors = self.cursors.clone();
        let mut events = Vec::new();
        for (g, generator) in self.prepared.can.generators.iter().enumerate() {
            while generator.schedule.time(cursors[g]) == Some(self.now.0 as u128) {
                events.push((self.now.0, Event::Generate(g, cursors[g])));
                cursors[g] = cursors[g]
                    .checked_add(1)
                    .ok_or_else(|| limit_error("generator ordinal overflow"))?;
            }
        }
        let future = self.next_generation(&cursors);
        if let Some(time) = future.filter(|&t| t < self.prepared.common.time_limit_ps as u128) {
            events.push((time as u64, Event::Dispatch));
        }
        self.preflight(&events)?;
        self.cursors = cursors;
        self.future_generation =
            future.is_some_and(|t| t >= self.prepared.common.time_limit_ps as u128);
        self.publish(events);
        Ok(())
    }
    pub(super) fn generate(&mut self, g: usize, ordinal: u64) -> Result<(), Diagnostic> {
        let time = self.now.0;
        let generator = &self.prepared.can.generators[g];
        let controller = &self.prepared.can.controllers[generator.source];
        let ready = add(time, controller.tx_processing_ps)?;
        let index = self.snapshot.can.requests.len();
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
        let request = Request {
            request_id: format!("{}:{ordinal}", generator.id),
            source: controller.id.clone(),
            bus: self.prepared.can.buses[self.prepared.can.controller_buses[generator.source]]
                .id
                .clone(),
            status: "processing".into(),
            generated_ps: time,
            ready_ps: None,
            tx_enqueued_ps: None,
            sof_ps: None,
            eof_ps: None,
            planned_eof_ps: None,
            planned_release_ps: None,
            release_ps: None,
            payload_bits: generator.frame.data.len() as u64 * 4,
            frame_bits: wire.frame.len() as u64,
            crc15: wire.crc15,
            stuff_bits: wire.stuff_positions.len() as u64,
            bitrate_bps: self.bitrate(self.prepared.can.controller_buses[generator.source]),
        };
        let lineage = RequestLineage {
            origin_request_id: request.request_id.clone(),
            parent_request_id: None,
            gw_hops: 0,
        };
        self.append_request(request, lineage, g, generator.source);
        self.publish(events);
        Ok(())
    }
    pub(super) fn tx_processed(&mut self, r: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        let events = vec![(time, Event::Ready(r))];
        self.preflight(&events)?;
        self.publish(events);
        Ok(())
    }
    pub(super) fn ready(&mut self, r: usize) -> Result<(), Diagnostic> {
        let source = self.request_sources[r];
        let capacity = self.prepared.can.controllers[source].queue_capacity;
        self.snapshot.can.requests[r].ready_ps = Some(self.now.0);
        if self.request_forwards[r].is_some() && capacity > 0 {
            self.snapshot.can.requests[r].status = "waiting_tx".into();
            self.tx_waiting[source].insert((self.now.3, r));
            self.drain_gateway_tx(source);
        } else if self.queues[source].len() as u64 >= capacity {
            self.snapshot.can.requests[r].status = "dropped".into();
            self.queue_point(source, r);
            self.finish_gateway_egress(r);
        } else {
            self.enqueue(r, self.now.3);
        }
        Ok(())
    }
    /// Commit TX queue admission separately from processing completion.
    pub(super) fn enqueue(&mut self, r: usize, order: u64) {
        let source = self.request_sources[r];
        let g = self.request_generators[r];
        let request = &mut self.snapshot.can.requests[r];
        request.status = "pending".into();
        request.tx_enqueued_ps = Some(self.now.0);
        let (generated, generator_id, ordinal) = if self.snapshot.gateway.request_lineage
            [&request.request_id]
            .parent_request_id
            .is_some()
        {
            (order, String::new(), 0)
        } else {
            (
                request.generated_ps,
                self.prepared.can.generators[g].id.clone(),
                request
                    .request_id
                    .rsplit(':')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap(),
            )
        };
        self.queues[source].insert((
            self.wires[g].arbitration.clone(),
            generated,
            generator_id,
            ordinal,
            r,
        ));
        self.dirty(self.prepared.can.controller_buses[source]);
        self.queue_point(source, r);
        self.finish_gateway_egress(r);
    }
    pub(super) fn arbitrate(&mut self, bus: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        if self.active[bus].is_none() {
            let candidate = self
                .queues
                .iter()
                .enumerate()
                .filter(|(s, _)| self.prepared.can.controller_buses[*s] == bus)
                .filter_map(|(s, q)| q.first().map(|e| (s, e)))
                .min_by(|a, b| a.1.cmp(b.1))
                .map(|(s, e)| (s, e.clone()));
            if let Some((source, entry)) = candidate {
                let r = entry.4;
                let eof = protocol::end_time(
                    time,
                    self.snapshot.can.requests[r].frame_bits,
                    self.bitrate(bus),
                )?;
                let release = protocol::end_time(
                    time,
                    self.snapshot.can.requests[r].frame_bits + 3,
                    self.bitrate(bus),
                )?;
                let mut events = vec![(eof, Event::Eof(r)), (release, Event::Release(r))];
                if !self.tx_waiting[source].is_empty() {
                    events.push((time, Event::GwTxSpace(source)));
                }
                self.preflight(&events)?;
                self.queues[source].remove(&entry);
                self.active[bus] = Some(r);
                self.bus_state(bus, "transmitting");
                let request = &mut self.snapshot.can.requests[r];
                request.status = "in_flight".into();
                request.sof_ps = Some(time);
                request.planned_eof_ps = Some(eof);
                request.planned_release_ps = Some(release);
                let tx_wait = time - request.generated_ps;
                let arb_wait = time - request.tx_enqueued_ps.unwrap();
                let target = request.source.clone();
                self.queue_point(source, r);
                self.point(target.clone(), "tx_wait_ps", tx_wait, r, None);
                self.point(target, "arbitration_wait_ps", arb_wait, r, None);
                self.publish(events);
            }
        }
        Ok(())
    }
    pub(super) fn eof(&mut self, r: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        let source = self.request_sources[r];
        let bus = self.prepared.can.controller_buses[source];
        let mut events = Vec::new();
        let mut rows = Vec::new();
        for (c, controller) in self.prepared.can.controllers.iter().enumerate() {
            if c == source || self.prepared.can.controller_buses[c] != bus {
                continue;
            }
            let observed = add(
                add(time, self.prepared.can.controllers[source].tx_channel_ps)?,
                controller.rx_channel_ps,
            )?;
            let row = self.snapshot.can.receivers.len() + rows.len();
            events.push((observed, Event::Observe(r, c, row)));
            rows.push(Receiver {
                request_id: self.snapshot.can.requests[r].request_id.clone(),
                receiver: controller.id.clone(),
                status: "pending".into(),
                observed_ps: None,
                received_ps: None,
            });
        }
        self.preflight(&events)?;
        self.snapshot.can.requests[r].status = "success".into();
        self.snapshot.can.requests[r].eof_ps = Some(time);
        self.bus_state(bus, "intermission");
        self.snapshot.can.receivers.extend(rows);
        self.point(
            self.prepared.can.buses[bus].id.clone(),
            "transfer_ps",
            time - self.snapshot.can.requests[r].sof_ps.unwrap(),
            r,
            None,
        );
        self.publish(events);
        Ok(())
    }
    pub(super) fn release(&mut self, r: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        self.snapshot.can.requests[r].release_ps = Some(time);
        let bus = self.prepared.can.controller_buses[self.request_sources[r]];
        self.active[bus] = None;
        self.bus_state(bus, "idle");
        self.dirty(bus);
        Ok(())
    }
    pub(super) fn observe(&mut self, r: usize, c: usize, row: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        let controller = &self.prepared.can.controllers[c];
        let g = self.request_generators[r];
        let accepted = protocol::accepts(
            &controller.rx_filter,
            &self.prepared.can.generators[g].frame,
        );
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
        self.snapshot.can.receivers[row].observed_ps = Some(time);
        if !accepted {
            self.snapshot.can.receivers[row].status = "filtered".into();
        }
        self.publish(events);
        Ok(())
    }
    pub(super) fn rx_processed(
        &mut self,
        r: usize,
        c: usize,
        row: usize,
    ) -> Result<(), Diagnostic> {
        let time = self.now.0;
        let events = vec![(time, Event::Received(r, c, row))];
        self.preflight(&events)?;
        self.publish(events);
        Ok(())
    }
    pub(super) fn received(&mut self, r: usize, c: usize, row: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        let events = if self.prepared.gateway.controller_gateways[c].is_some() {
            vec![(time, Event::RoutingInput(r, c, row))]
        } else {
            Vec::new()
        };
        self.preflight(&events)?;
        self.snapshot.can.receivers[row].status = "received".into();
        self.snapshot.can.receivers[row].received_ps = Some(time);
        let receiver = self.prepared.can.controllers[c].id.clone();
        self.point(
            receiver.clone(),
            "delivery_ps",
            time - self.snapshot.can.requests[r].generated_ps,
            r,
            Some(receiver),
        );
        self.publish(events);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
