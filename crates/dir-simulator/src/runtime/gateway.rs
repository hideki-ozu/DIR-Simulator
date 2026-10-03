//! CAN-to-CAN routing and fanout, committed after reservation preflight.
use super::engine::Engine;
use super::scheduler::{Event, add};
use crate::snapshot::{ForwardRecord, Request, RequestLineage, RxBufferRecord};
use crate::types::Diagnostic;

impl Engine<'_> {
    fn rx_queue_point(&mut self, ingress: usize, parent: usize) {
        self.point(
            format!("{}.rxQueue", self.prepared.can.controllers[ingress].id),
            "gw_rx_queue_length",
            self.rx_used[ingress],
            parent,
            Some(self.prepared.can.controllers[ingress].id.clone()),
        );
    }
    fn release_rx(&mut self, buffer: usize) {
        let record = &mut self.snapshot.gateway.rx_buffers[buffer];
        if record.status != "holding" || !record.remaining_egress.is_empty() {
            return;
        }
        record.status = "released".into();
        record.released_ps = Some(self.now.0);
        let ingress = record.ingress;
        let parent = record.parent_request;
        self.rx_used[ingress] -= 1;
        self.rx_queue_point(ingress, parent);
    }
    fn finish_rx_egress(&mut self, buffer: usize, egress: usize) {
        let removed = self.snapshot.gateway.rx_buffers[buffer]
            .remaining_egress
            .remove(&egress);
        debug_assert!(removed, "Gateway egress completed more than once");
        self.release_rx(buffer);
    }
    pub(super) fn finish_gateway_egress(&mut self, request: usize) {
        if let Some(forward) = self.request_forwards[request] {
            let f = &self.snapshot.gateway.forwards[forward];
            let buffer = f.rx_buffer;
            let source = f.egress.unwrap();
            if let Some(enqueued) = self.snapshot.can.requests[request].tx_enqueued_ps {
                self.point(
                    self.prepared.can.controllers[source].id.clone(),
                    "gw_tx_buffer_wait_ps",
                    enqueued - self.snapshot.can.requests[request].ready_ps.unwrap(),
                    request,
                    None,
                );
            }
            self.finish_rx_egress(buffer, source);
        }
    }
    /// A queue slot, not bus-idle status, permits a buffered copy to advance.
    pub(super) fn drain_gateway_tx(&mut self, source: usize) {
        let capacity = self.prepared.can.controllers[source].queue_capacity;
        while (self.queues[source].len() as u64) < capacity {
            let Some((order, request)) = self.tx_waiting[source].pop_first() else {
                break;
            };
            self.enqueue(request, order);
        }
    }
    pub(super) fn gw_point(
        &mut self,
        gateway: usize,
        metric: &str,
        parent: usize,
        receiver: usize,
        reason: Option<&str>,
    ) {
        self.point(
            self.prepared.gateway.gateways[gateway].node.clone(),
            metric,
            1,
            parent,
            Some(self.prepared.can.controllers[receiver].id.clone()),
        );
        self.snapshot.common.points.last_mut().unwrap().reason = reason.map(str::to_owned);
    }
    pub(super) fn route(
        &mut self,
        parent: usize,
        ingress: usize,
        row: usize,
    ) -> Result<(), Diagnostic> {
        let gateway = self.prepared.gateway.controller_gateways[ingress]
            .ok_or_else(|| Diagnostic::execution("routing to a non-Gateway port"))?;
        let received = self
            .snapshot
            .can
            .receivers
            .get(row)
            .ok_or_else(|| Diagnostic::execution("unknown routing receiver"))?;
        let request = self
            .snapshot
            .can
            .requests
            .get(parent)
            .ok_or_else(|| Diagnostic::execution("unknown routing parent"))?;
        if received.request_id != request.request_id
            || received.receiver != self.prepared.can.controllers[ingress].id
            || received.received_ps != Some(self.now.0)
            || received.status != "received"
            || self.routed.contains(&(parent, ingress))
        {
            return Err(Diagnostic::execution(
                "duplicate or inconsistent Gateway routing input",
            ));
        }
        let frame = &self.prepared.can.generators[self.request_generators[parent]].frame;
        let gw = &self.prepared.gateway.gateways[gateway];
        let route = gw.routes.iter().find(|r| {
            r.ingress == ingress
                && r.format == frame.format
                && r.id_min <= frame.id
                && frame.id <= r.id_max
        });
        let buffer = self.snapshot.gateway.rx_buffers.len();
        let rejected = self.rx_used[ingress] >= gw.rx_queue_capacity;
        let rx_record = RxBufferRecord {
            buffer_id: format!(
                "rx:{}/{}/{}",
                request.request_id, gw.node, self.prepared.can.controllers[ingress].id
            ),
            parent_request: parent,
            gateway,
            ingress,
            capacity: gw.rx_queue_capacity,
            received_ps: self.now.0,
            released_ps: None,
            status: if rejected { "dropped" } else { "holding" }.into(),
            reason: rejected.then(|| "rx_queue_full".into()),
            egress: route.map(|r| r.egress.clone()).unwrap_or_default(),
            remaining_egress: if rejected {
                Default::default()
            } else {
                route
                    .map(|r| r.egress.iter().copied().collect())
                    .unwrap_or_default()
            },
        };
        if rejected {
            self.routed.insert((parent, ingress));
            self.snapshot.gateway.rx_buffers.push(rx_record);
            self.gw_point(
                gateway,
                "gw_rx_dropped",
                parent,
                ingress,
                Some("rx_queue_full"),
            );
            self.rx_queue_point(ingress, parent);
            return Ok(());
        }
        let mut rows = Vec::new();
        let mut events = Vec::new();
        if let Some(route) = route {
            let due = add(self.now.0, gw.processing_delay_ps)?;
            let hops = self.snapshot.gateway.request_lineage[&request.request_id]
                .gw_hops
                .checked_add(1)
                .filter(|&n| n <= 65536)
                .ok_or_else(|| Diagnostic::execution("Gateway hop arithmetic overflow"))?;
            for &egress in &route.egress {
                let index = self.snapshot.gateway.forwards.len() + rows.len();
                rows.push(ForwardRecord {
                    rx_buffer: buffer,
                    forward_id: format!(
                        "gw:{}/{}/{}/{}",
                        request.request_id,
                        gw.node,
                        route.id,
                        self.prepared.can.controllers[egress].id
                    ),
                    parent_request: parent,
                    gateway,
                    ingress,
                    egress: Some(egress),
                    route_id: Some(route.id.clone()),
                    gw_hops: hops,
                    received_ps: self.now.0,
                    planned_forward_ps: Some(due),
                    forwarded_ps: None,
                    child_request_id: None,
                    status: "processing".into(),
                    reason: None,
                });
                events.push((
                    due,
                    if due == self.now.0 {
                        Event::Forward(index)
                    } else {
                        Event::ForwardDue(index)
                    },
                ));
            }
        } else {
            rows.push(ForwardRecord {
                rx_buffer: buffer,
                forward_id: format!(
                    "filtered:{}/{}/{}",
                    request.request_id, gw.node, self.prepared.can.controllers[ingress].id
                ),
                parent_request: parent,
                gateway,
                ingress,
                egress: None,
                route_id: None,
                gw_hops: self.snapshot.gateway.request_lineage[&request.request_id].gw_hops,
                received_ps: self.now.0,
                planned_forward_ps: None,
                forwarded_ps: None,
                child_request_id: None,
                status: "filtered".into(),
                reason: Some("no_route".into()),
            });
        }
        self.preflight(&events)?;
        self.routed.insert((parent, ingress));
        self.snapshot.gateway.rx_buffers.push(rx_record);
        self.rx_used[ingress] += 1;
        self.rx_queue_point(ingress, parent);
        for row in &rows {
            if let Some(egress) = row.egress {
                self.gw_point(gateway, "gw_copy_created", parent, egress, None);
            } else {
                self.gw_point(
                    gateway,
                    "gw_route_filtered",
                    parent,
                    ingress,
                    Some("no_route"),
                );
            }
        }
        self.snapshot.gateway.forwards.extend(rows);
        self.release_rx(buffer);
        self.publish(events);
        Ok(())
    }
    pub(super) fn validate_forward(&self, f: usize) -> Result<(), Diagnostic> {
        let row = self
            .snapshot
            .gateway
            .forwards
            .get(f)
            .ok_or_else(|| Diagnostic::execution("unknown Gateway forward id"))?;
        if row.status != "processing"
            || row.planned_forward_ps != Some(self.now.0)
            || row.egress.is_none()
            || row.child_request_id.is_some()
        {
            return Err(Diagnostic::execution(
                "inconsistent Gateway forward state or time",
            ));
        }
        Ok(())
    }
    pub(super) fn forward(&mut self, f: usize) -> Result<(), Diagnostic> {
        self.validate_forward(f)?;
        let row = self.snapshot.gateway.forwards[f].clone();
        let source = row.egress.unwrap();
        if row.gw_hops > self.prepared.gateway.gateways[row.gateway].hop_limit {
            self.snapshot.gateway.forwards[f].status = "dropped".into();
            self.snapshot.gateway.forwards[f].reason = Some("dropped_hop_limit".into());
            self.snapshot.gateway.forwards[f].forwarded_ps = Some(self.now.0);
            self.gw_point(
                row.gateway,
                "gw_hop_dropped",
                row.parent_request,
                source,
                Some("dropped_hop_limit"),
            );
            self.finish_rx_egress(row.rx_buffer, source);
        } else {
            let ready = add(
                self.now.0,
                self.prepared.can.controllers[source].tx_processing_ps,
            )?;
            let index = self.snapshot.can.requests.len();
            let events = vec![(
                ready,
                if ready == self.now.0 {
                    Event::Ready(index)
                } else {
                    Event::TxProcessed(index)
                },
            )];
            self.preflight(&events)?;
            let parent = &self.snapshot.can.requests[row.parent_request];
            let request = Request {
                request_id: row.forward_id.clone(),
                source: self.prepared.can.controllers[source].id.clone(),
                bus: self.prepared.can.buses[self.prepared.can.controller_buses[source]]
                    .id
                    .clone(),
                status: "processing".into(),
                generated_ps: self.now.0,
                ready_ps: None,
                tx_enqueued_ps: None,
                sof_ps: None,
                eof_ps: None,
                release_ps: None,
                planned_eof_ps: None,
                planned_release_ps: None,
                payload_bits: parent.payload_bits,
                frame_bits: parent.frame_bits,
                crc15: parent.crc15,
                stuff_bits: parent.stuff_bits,
                bitrate_bps: self.bitrate(self.prepared.can.controller_buses[source]),
            };
            let lineage = RequestLineage {
                origin_request_id: self.snapshot.gateway.request_lineage[&parent.request_id]
                    .origin_request_id
                    .clone(),
                parent_request_id: Some(parent.request_id.clone()),
                gw_hops: row.gw_hops,
            };
            self.append_request(
                request,
                lineage,
                self.request_generators[row.parent_request],
                source,
            );
            self.request_forwards[index] = Some(f);
            self.snapshot.gateway.forwards[f].status = "submitted".into();
            self.snapshot.gateway.forwards[f].forwarded_ps = Some(self.now.0);
            self.snapshot.gateway.forwards[f].child_request_id = Some(row.forward_id);
            self.gw_point(
                row.gateway,
                "gw_copy_submitted",
                row.parent_request,
                source,
                None,
            );
            self.publish(events);
        }
        Ok(())
    }
    pub(super) fn forward_due(&mut self, f: usize) -> Result<(), Diagnostic> {
        let time = self.now.0;
        self.validate_forward(f)?;
        let events = vec![(time, Event::Forward(f))];
        self.preflight(&events)?;
        self.publish(events);
        Ok(())
    }
}
