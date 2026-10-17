//! Actual control media and protocol host, inside the single network owner.
use super::*;
use crate::runtime::wire::boundary::{BoundaryState, Event as BoundaryEvent};
use crate::runtime::wire::{WireEvent, WireState, WireTxIntent, WireView};
use crate::snapshot::wire::{WireEventRecord, WireTimerKey};
use crate::types::wire::{PreparedWire, WireProtocol};

type IpControlMetadata = (Option<[u8; 6]>, Option<[u8; 6]>, WireProtocol, String);
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) enum HostEvent {
    Packet(usize),
    BoundaryStart(usize),
    BoundaryRelease(usize),
    Timer {
        adapter: String,
        key: WireTimerKey,
        generation: u64,
        deadline: u64,
    },
    Eof {
        direction: usize,
        id: String,
    },
    Release {
        direction: usize,
        id: String,
    },
    Arrival {
        direction: usize,
        id: String,
    },
}
#[derive(Debug, Clone)]
pub(super) struct ControlTx {
    pub intent: WireTxIntent,
    pub payload: Vec<u8>,
    pub vid: Option<u16>,
    pub destination: Option<[u8; 6]>,
    pub source: Option<[u8; 6]>,
    pub sof: Option<u64>,
    pub eof: Option<u64>,
    pub release: Option<u64>,
    pub arrival: Option<u64>,
    pub planned_eof: Option<u64>,
    pub planned_release: Option<u64>,
    pub planned_arrival: Option<u64>,
    pub dropped: bool,
}
#[derive(Debug, Clone)]
pub(super) struct WireHost {
    pub protocol: WireState,
    pub boundaries: Vec<BoundaryState>,
    pub queues: Vec<VecDeque<String>>,
    pub active: Vec<Option<String>>,
    pub tx: BTreeMap<String, ControlTx>,
    pub timers: BTreeMap<(String, WireTimerKey), (u64, u64)>,
    pub generations: BTreeMap<String, u64>,
    pub pending_records: Vec<(String, String, Value)>,
    pub phase: u8,
    #[cfg(test)]
    pub integration_stages: Vec<String>,
    #[cfg(test)]
    pub pending_policy_commit: bool,
}
impl WireHost {
    pub fn new(p: &PreparedWire, directions: usize) -> Self {
        Self {
            protocol: WireState::new(Arc::new(p.clone())),
            boundaries: vec![BoundaryState::default(); p.packets.len()],
            queues: vec![VecDeque::new(); directions],
            active: vec![None; directions],
            tx: BTreeMap::new(),
            timers: BTreeMap::new(),
            generations: BTreeMap::new(),
            pending_records: vec![],
            phase: 0,
            #[cfg(test)]
            integration_stages: vec![],
            #[cfg(test)]
            pending_policy_commit: false,
        }
    }
}
impl NetworkState {
    #[cfg(test)]
    pub(crate) fn wire_vector_committed(&mut self) {
        if let Some(host) = &mut self.wire {
            if host.pending_policy_commit {
                host.integration_stages.push("policy_commit".into());
                host.pending_policy_commit = false;
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn wire_vector_stages(&self) -> Vec<String> {
        self.wire
            .as_ref()
            .map(|host| host.integration_stages.clone())
            .unwrap_or_default()
    }
    fn wire_mut<'a>(&self, d: &'a mut NetworkDelta) -> Result<&'a mut WireHost> {
        if d.wire.is_none() {
            d.wire = self.wire.clone();
        }
        d.wire.as_mut().ok_or_else(|| error("wire host missing"))
    }
    pub(super) fn wire_host<'a>(&'a self, d: &'a NetworkDelta) -> Option<&'a WireHost> {
        d.wire.as_ref().or(self.wire.as_ref())
    }
    fn wire_view(&self, d: &NetworkDelta) -> WireView {
        let policy = self.policy(d);
        let mut view = WireView::default();
        if let Some(policy) = policy {
            view.link_up = policy.link_up.clone();
            view.policy_epoch = policy.policy_epoch;
            for (port, vlans) in &policy.effective_vlans {
                for vid in vlans.keys() {
                    view.ingress_eligible.insert(
                        (port.clone(), *vid),
                        crate::runtime::ethernet::dynamic::eligible_egress(port, *vid, &policy)
                            .is_ok(),
                    );
                }
            }
        }
        view
    }
    pub(super) fn initialize_wire(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let Some(prepared) = self.prepared.wire.as_ref() else {
            return Ok(());
        };
        self.wire_protocol(WireEvent::Initialize, d, c)?;
        for (i, packet) in prepared.packets.iter().enumerate() {
            if let Some(boundary) = &packet.boundary {
                if boundary.start_ps == c.now() {
                    self.wire_event(HostEvent::BoundaryStart(i), d, c)?;
                } else {
                    self.due_mut(d).push(Due {
                        at: boundary.start_ps,
                        kind: Completion::Wire(HostEvent::BoundaryStart(i)),
                    });
                }
                self.due_mut(d).push(Due {
                    at: boundary.release_ps,
                    kind: Completion::Wire(HostEvent::BoundaryRelease(i)),
                });
            }
            Self::schedule(c, d, packet.arrival_ps, Event::Wire(HostEvent::Packet(i)))?;
        }
        Ok(())
    }
    fn wire_protocol(
        &self,
        event: WireEvent,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let receiving = matches!(event, WireEvent::Receive { .. });
        let view = self.wire_view(d);
        let mut protocol = self.wire_mut(d)?.protocol.plan(c.now(), event, &view)?;
        if receiving {
            crate::allocation::reservation_checkpoint("network_wire_receive")?;
        }
        let (dynamic, flush, mac) = self
            .dynamic
            .as_ref()
            .ok_or_else(|| error("wire requires dynamic state"))?
            .plan_wire_in(
                d.dynamic.take(),
                c.now(),
                &protocol.policies,
                &protocol.registered,
                &protocol.flush,
                protocol.policy_changed,
                protocol.topology_changed,
            )?;
        let dynamic = self.dynamic.as_ref().unwrap().plan_wire_membership_in(
            dynamic,
            c.now(),
            &protocol.memberships,
            &protocol.router_ports,
            &protocol.compatibility_flood,
        )?;
        d.dynamic = Some(dynamic);
        let policy = self.policy(d).ok_or_else(|| error("wire policy missing"))?;
        protocol.complete_host_records(
            c.now(),
            policy.policy_epoch,
            policy.topology_generation,
            flush,
            mac,
        )?;
        #[cfg(test)]
        {
            let host = self.wire_mut(d)?;
            host.integration_stages
                .extend(protocol.integration_stages.clone());
            if protocol.flush.iter().any(|f| f.reason == "mvrp_new") {
                host.integration_stages.push("new_fdb_flush".into());
            }
            host.pending_policy_commit |= protocol.policy_changed;
        }
        // The host owns the single policy epoch for this whole atomic batch.
        // Multiple logical due items can share that epoch after coalescing.
        for row in &mut protocol.records {
            for field in ["policy_epoch_after", "epoch_after"] {
                if row.data.get(field).is_some() {
                    row.data[field] = json!(policy.policy_epoch.to_string());
                }
            }
        }
        let mut timers = protocol.timers.clone();
        timers.sort_by(|a, b| (&a.adapter_id, &a.key).cmp(&(&b.adapter_id, &b.key)));
        for timer in timers {
            self.due_mut(d).retain(|due|!matches!(&due.kind,Completion::Wire(HostEvent::Timer{adapter,key,..})if adapter==&timer.adapter_id&&key==&timer.key));
            let id = (timer.adapter_id.clone(), timer.key.clone());
            if let Some(deadline) = timer.deadline_ps {
                if deadline <= c.now() {
                    return Err(error("wire timer must advance time"));
                }
                self.wire_mut(d)?
                    .timers
                    .insert(id, (timer.generation, deadline));
                self.due_mut(d).push(Due {
                    at: deadline,
                    kind: Completion::Wire(HostEvent::Timer {
                        adapter: timer.adapter_id,
                        key: timer.key,
                        generation: timer.generation,
                        deadline,
                    }),
                });
            } else {
                self.wire_mut(d)?.timers.remove(&id);
            }
        }
        let tx = protocol.tx.clone();
        let forwards = protocol.forwards.clone();
        for record in std::mem::take(&mut protocol.records) {
            if record.kind == "ethernet.control.wire.event" {
                if let (Some(adapter), Some(generation)) = (
                    record.data["adapter_id"].as_str(),
                    record.data["protocol_generation_after"]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok()),
                ) {
                    self.wire_mut(d)?
                        .generations
                        .insert(adapter.into(), generation);
                }
            }
            let schema = record.kind.strip_suffix("/1").unwrap_or(&record.kind);
            let id = record
                .data
                .get("event_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("@wire/record/{schema}/{}", d.record_sequence));
            Self::record(c, d, schema, id, record.data)?;
        }
        let dirty = protocol.policy_changed || protocol.topology_changed;
        self.wire_mut(d)?.protocol.apply(protocol);
        for intent in tx {
            let payload = intent.bytes.clone();
            self.wire_enqueue(
                ControlTx {
                    intent,
                    payload,
                    vid: None,
                    destination: None,
                    source: None,
                    sof: None,
                    eof: None,
                    release: None,
                    arrival: None,
                    planned_eof: None,
                    planned_release: None,
                    planned_arrival: None,
                    dropped: false,
                },
                d,
                c,
            )?;
        }
        for forward in forwards {
            let meta = self.wire_metadata(&forward.packet_id, d)?;
            for port in forward.ports {
                let source = meta
                    .0
                    .ok_or_else(|| error("forwarded IP missing source MAC"))?;
                let destination = meta
                    .1
                    .ok_or_else(|| error("forwarded IP missing destination MAC"))?;
                let bytes = tagged_ip(source, destination, forward.vid, meta.2, &forward.bytes);
                let id = replication::identifier(&[
                    &forward.packet_id,
                    &port,
                    &d.record_sequence.to_string(),
                    &self.wire_host(d).unwrap().tx.len().to_string(),
                ]);
                let intent = WireTxIntent {
                    adapter_id: meta.3.clone(),
                    packet_id: id,
                    port,
                    protocol: meta.2,
                    ordinal: d.record_sequence,
                    bytes,
                    cause_id: forward.packet_id.clone(),
                };
                self.wire_enqueue(
                    ControlTx {
                        intent,
                        payload: forward.bytes.clone(),
                        vid: Some(forward.vid),
                        destination: Some(destination),
                        source: Some(source),
                        sof: None,
                        eof: None,
                        release: None,
                        arrival: None,
                        planned_eof: None,
                        planned_release: None,
                        planned_arrival: None,
                        dropped: false,
                    },
                    d,
                    c,
                )?;
            }
        }
        if dirty {
            c.request_arbitration("network").map_err(model)?;
        }
        Ok(())
    }
    fn wire_metadata(&self, id: &str, d: &NetworkDelta) -> Result<IpControlMetadata> {
        let prepared = self.prepared.wire.as_ref().unwrap();
        if let Some(packet) = prepared.packets.iter().find(|p| p.id == id) {
            return Ok((
                packet.source_mac,
                packet.destination_mac,
                prepared.adapter(&packet.adapter_id).unwrap().protocol,
                packet.adapter_id.clone(),
            ));
        }
        let tx = self
            .wire_host(d)
            .unwrap()
            .tx
            .get(id)
            .ok_or_else(|| error("forwarded control lineage missing"))?;
        Ok((
            tx.source,
            tx.destination,
            tx.intent.protocol,
            tx.intent.adapter_id.clone(),
        ))
    }
    fn wire_enqueue(&self, tx: ControlTx, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        let p = self.prepared.wire.as_ref().unwrap();
        let direction = self
            .prepared
            .ethernet
            .directions
            .iter()
            .position(|l| l.from_port == tx.intent.port)
            .ok_or_else(|| error("wire TX output unknown"))?;
        if tx.intent.bytes.len() < 64 || tx.intent.bytes.len() > 1522 {
            return Err(error("wire TX MAC bounds"));
        }
        let bytes = &tx.intent.bytes;
        let end = bytes.len() - 4;
        if crate::runtime::wire::codec::crc32(&bytes[..end]).to_le_bytes() != bytes[end..] {
            return Err(error("wire TX FCS invariant"));
        }
        let adapter = p
            .adapter(&tx.intent.adapter_id)
            .ok_or_else(|| error("wire TX adapter unknown"))?;
        let host = self.wire_mut(d)?;
        if host.tx.contains_key(&tx.intent.packet_id) {
            return Err(error("duplicate wire TX ID"));
        }
        let pending = host
            .queues
            .iter()
            .flat_map(|q| q.iter())
            .chain(host.active.iter().filter_map(Option::as_ref))
            .filter(|id| host.tx[*id].intent.adapter_id == tx.intent.adapter_id)
            .count();
        if pending >= adapter.max_pending_tx {
            return Err(error("wire control FIFO capacity exceeded"));
        }
        crate::allocation::reservation_checkpoint("network_wire_fifo")?;
        let id = tx.intent.packet_id.clone();
        host.queues[direction].push_back(id.clone());
        host.tx.insert(id.clone(), tx);
        self.wire_tx_record(direction, &id, "tx_queued", None, d, c)?;
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn wire_event(
        &self,
        event: HostEvent,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.wire_mut(d)?.phase =
            if matches!(event, HostEvent::Packet(_) | HostEvent::Arrival { .. }) {
                1
            } else {
                0
            };
        match event {
            HostEvent::BoundaryStart(i) | HostEvent::BoundaryRelease(i) => {
                let event = if matches!(event, HostEvent::BoundaryStart(_)) {
                    BoundaryEvent::Start
                } else {
                    BoundaryEvent::Release
                };
                self.wire_boundary(i, event, d, c)?;
            }
            HostEvent::Packet(i) => {
                let packet = self.prepared.wire.as_ref().unwrap().packets[i].clone();
                let protocol = self
                    .prepared
                    .wire
                    .as_ref()
                    .unwrap()
                    .adapter(&packet.adapter_id)
                    .unwrap()
                    .protocol;
                if packet.boundary.is_some() {
                    self.wire_boundary(i, BoundaryEvent::Arrival, d, c)?;
                }
                self.wire_protocol(
                    WireEvent::Receive {
                        protocol,
                        ingress_port: packet.ingress_port,
                        packet_id: packet.id,
                        bytes: packet.bytes,
                        vid: packet.vid,
                        destination_mac: packet.destination_mac,
                        external_injection: true,
                    },
                    d,
                    c,
                )?;
            }
            HostEvent::Timer {
                adapter,
                key,
                generation,
                deadline,
            } => {
                let id = (adapter.clone(), key.clone());
                if self.wire_host(d).unwrap().timers.get(&id) == Some(&(generation, deadline)) {
                    self.wire_mut(d)?.timers.remove(&id);
                }
                // Protocol owns stale-token audit; keep any newer host reservation intact.
                self.wire_protocol(
                    WireEvent::Timer {
                        adapter_id: adapter,
                        key,
                        generation,
                        deadline_ps: deadline,
                    },
                    d,
                    c,
                )?;
            }
            HostEvent::Eof { direction, id } => {
                let tx = self
                    .wire_mut(d)?
                    .tx
                    .get_mut(&id)
                    .ok_or_else(|| error("wire EOF missing transfer"))?;
                if tx.eof.is_some() {
                    return Ok(());
                }
                tx.eof = Some(c.now());
                let arrival = tx
                    .planned_arrival
                    .ok_or_else(|| error("wire arrival not planned"))?;
                self.wire_tx_record(direction, &id, "tx_completed", None, d, c)?;
                Self::schedule(
                    c,
                    d,
                    arrival,
                    Event::Wire(HostEvent::Arrival { direction, id }),
                )?;
            }
            HostEvent::Release { direction, id } => {
                let host = self.wire_mut(d)?;
                if host.active[direction].as_deref() != Some(&id) {
                    return Ok(());
                }
                host.tx.get_mut(&id).unwrap().release = Some(c.now());
                host.active[direction] = None;
                c.request_arbitration("network").map_err(model)?;
            }
            HostEvent::Arrival { direction, id } => {
                let link = &self.prepared.ethernet.directions[direction];
                let tx = self.wire_host(d).unwrap().tx[&id].clone();
                self.wire_mut(d)?.tx.get_mut(&id).unwrap().arrival = Some(c.now());
                let prepared = self.prepared.wire.as_ref().unwrap();
                if prepared.owner(tx.intent.protocol, &link.to_port).is_none() {
                    self.wire_tx_record(direction, &id, "receive", Some("unsupported_kind"), d, c)?;
                    return Ok(());
                }
                self.wire_protocol(
                    WireEvent::Receive {
                        protocol: tx.intent.protocol,
                        ingress_port: link.to_port.clone(),
                        packet_id: id,
                        bytes: tx.payload,
                        vid: tx.vid,
                        destination_mac: tx.destination,
                        external_injection: false,
                    },
                    d,
                    c,
                )?;
            }
        }
        Ok(())
    }
    fn wire_boundary(
        &self,
        i: usize,
        event: BoundaryEvent,
        d: &mut NetworkDelta,
        c: &Context<'_>,
    ) -> Result<()> {
        let packet = &self.prepared.wire.as_ref().unwrap().packets[i];
        let plan = packet
            .boundary
            .as_ref()
            .ok_or_else(|| error("missing boundary plan"))?;
        let state = self.wire_host(d).unwrap().boundaries[i].clone();
        let (next, row) = state
            .plan(
                plan,
                &packet.id,
                &packet.ingress_port,
                event,
                c.now(),
                d.record_sequence,
            )
            .map_err(error)?;
        self.wire_mut(d)?.boundaries[i] = next;
        // Records are published by the same callback as protocol processing.
        let id = replication::identifier(&[&packet.id, &format!("{event:?}")]);
        let data = serde_json::to_value(row).map_err(|e| error(e.to_string()))?;
        self.wire_mut(d)?
            .pending_records
            .push(("ethernet.wire.boundary".into(), id, data));
        Ok(())
    }
    fn wire_tx_record(
        &self,
        direction: usize,
        id: &str,
        kind: &str,
        reason: Option<&str>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let host = self.wire_host(d).unwrap();
        let tx = &host.tx[id];
        let adapter = self
            .prepared
            .wire
            .as_ref()
            .unwrap()
            .adapter(&tx.intent.adapter_id)
            .unwrap();
        let generation = *host.generations.get(&adapter.id).unwrap_or(&0);
        let epoch = self.epoch_after(d);
        let link = &self.prepared.ethernet.directions[direction];
        let row = WireEventRecord {
            schema_version: 1,
            adapter_id: if kind == "receive" {
                format!(
                    "@wire/unowned/{}",
                    self.prepared.ethernet.devices[link.destination].id
                )
            } else {
                adapter.id.clone()
            },
            protocol: format!("{:?}", tx.intent.protocol).to_lowercase(),
            protocol_revision: tx.intent.protocol.revision().into(),
            bridge: if kind == "receive" {
                self.prepared.ethernet.devices[link.destination].id.clone()
            } else {
                adapter.bridge.clone()
            },
            port: if kind == "receive" {
                link.to_port.clone()
            } else {
                tx.intent.port.clone()
            },
            event_id: format!("@wire/host/{}", d.record_sequence),
            cause_id: Some(tx.intent.cause_id.clone()),
            event_kind: kind.into(),
            time_ps: c.now().to_string(),
            phase: if kind == "tx_started" {
                2
            } else if kind == "receive" {
                1
            } else if kind == "tx_queued" {
                host.phase
            } else {
                0
            },
            batch_ordinal: "0".into(),
            packet_id: Some(id.into()),
            bytes_hex: Some(crate::runtime::wire::codec::hex(&tx.intent.bytes)),
            mac_bytes: Some(tx.intent.bytes.len() as u32),
            outcome: if reason.is_some() {
                "control_drop"
            } else {
                "changed"
            }
            .into(),
            reason: reason.map(str::to_owned),
            policy_epoch_before: epoch.to_string(),
            policy_epoch_after: epoch.to_string(),
            protocol_generation_before: generation.to_string(),
            protocol_generation_after: generation.to_string(),
            timer: None,
            effects: if reason.is_some() {
                vec![]
            } else {
                vec![
                    json!({"kind":"tx","packet_id":id,"port":tx.intent.port,"ordinal":tx.intent.ordinal.to_string()}),
                ]
            },
            external_injection: false,
        };
        let id = row.event_id.clone();
        Self::record(
            c,
            d,
            "ethernet.control.wire.event",
            id,
            serde_json::to_value(row).map_err(|e| error(e.to_string()))?,
        )
    }
    pub(super) fn wire_arbitrate(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<bool> {
        let Some(host) = self.wire_host(d) else {
            return Ok(false);
        };
        if host.active[direction].is_some() {
            return Ok(true);
        }
        if self.queue(d, direction).active.is_some() {
            return Ok(false);
        }
        while let Some(id) = self.wire_host(d).unwrap().queues[direction]
            .front()
            .cloned()
        {
            let link = &self.prepared.ethernet.directions[direction];
            let up = self
                .policy(d)
                .and_then(|p| p.link_up.get(&link.from_port).copied())
                .unwrap_or(true);
            self.wire_mut(d)?.queues[direction].pop_front();
            if !up {
                self.wire_mut(d)?.tx.get_mut(&id).unwrap().dropped = true;
                self.wire_tx_record(direction, &id, "tx_completed", Some("link_down"), d, c)?;
                continue;
            }
            let bytes = self.wire_host(d).unwrap().tx[&id].intent.bytes.len() as u64;
            let (eof, release, arrival) =
                l2::timing(c.now(), bytes, link.bitrate_bps, link.delay_ps)?;
            let host = self.wire_mut(d)?;
            let tx = host.tx.get_mut(&id).unwrap();
            tx.sof = Some(c.now());
            tx.planned_eof = Some(eof);
            tx.planned_release = Some(release);
            tx.planned_arrival = Some(arrival);
            host.active[direction] = Some(id.clone());
            self.due_mut(d).extend([
                Due {
                    at: eof,
                    kind: Completion::Wire(HostEvent::Eof {
                        direction,
                        id: id.clone(),
                    }),
                },
                Due {
                    at: release,
                    kind: Completion::Wire(HostEvent::Release {
                        direction,
                        id: id.clone(),
                    }),
                },
            ]);
            self.wire_tx_record(direction, &id, "tx_started", None, d, c)?;
            return Ok(true);
        }
        Ok(false)
    }
    pub(super) fn data_reason(
        &self,
        reason: Ineligible,
        port: &str,
        d: &NetworkDelta,
    ) -> &'static str {
        if reason == Ineligible::StpDiscarding
            && self
                .dynamic
                .as_ref()
                .and_then(|s| s.wire_ingress_state(port, d.dynamic.as_ref()))
                == Some((true, false))
        {
            "stp_learning"
        } else {
            reason.reason()
        }
    }
    pub(super) fn wire_links(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if self.wire.is_none() || d.dynamic.is_none() {
            return Ok(());
        }
        let before = self.dynamic.as_ref().unwrap().snapshot_policy();
        let after = self.policy(d).unwrap();
        let ports = before
            .link_up
            .keys()
            .chain(after.link_up.keys())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        for port in ports {
            if before.link_up.get(&port) != after.link_up.get(&port) {
                self.wire_protocol(
                    WireEvent::Link {
                        port: port.clone(),
                        up: after.link_up.get(&port).copied().unwrap_or(false),
                    },
                    d,
                    c,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn wire_multicast(
        &self,
        bridge: &str,
        ingress: &str,
        vid: u16,
        ip: Option<&IpMulticast>,
        wire: &EthernetWireFrame,
        d: &NetworkDelta,
    ) -> Option<EgressSelection> {
        let ip = ip?;
        let host = self.wire_host(d)?;
        let prepared = self.prepared.wire.as_ref()?;
        let protocol = if matches!(ip.family, IpFamily::Ipv6) {
            WireProtocol::Mld
        } else {
            WireProtocol::Igmp
        };
        if !prepared
            .adapters
            .iter()
            .any(|a| a.bridge == bridge && a.protocol == protocol)
        {
            return None;
        }
        let policy = self.policy(d)?;
        let ingress_output = self
            .prepared
            .ethernet
            .port_policies
            .iter()
            .find(|p| p.ingress == ingress)?
            .port
            .clone();
        let eligible = self
            .prepared
            .ethernet
            .port_policies
            .iter()
            .filter(|p| {
                self.prepared.ethernet.devices[p.device].id == bridge
                    && p.port != ingress_output
                    && crate::runtime::ethernet::dynamic::eligible_egress(&p.port, vid, &policy)
                        .is_ok()
            })
            .map(|p| p.port.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let static_ports = self
            .prepared
            .ethernet
            .devices
            .iter()
            .find(|d| d.id == bridge)
            .and_then(|d| d.multicast.get(&(vid, wire.dst_mac.clone())));
        let static_view = crate::runtime::wire::WireStaticMulticast {
            known: static_ports.is_some(),
            members: static_ports
                .map(|ports| ports.iter().cloned().collect())
                .unwrap_or_default(),
            routers: Default::default(),
        };
        let selected = host.protocol.multicast_candidates_with_static(
            bridge,
            vid,
            ip.source,
            ip.group,
            &eligible,
            &ingress_output,
            &static_view,
        );
        Some(EgressSelection {
            reason: selected.is_empty().then(|| "multicast_filtered".into()),
            ports: selected.into_iter().collect(),
        })
    }
    pub(super) fn emit_wire(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if let Some(host) = d.wire.as_mut() {
            for (schema, id, data) in std::mem::take(&mut host.pending_records) {
                Self::record(c, d, &schema, id, data)?;
            }
        }
        Ok(())
    }
    pub(super) fn wire_snapshot(&self, h: u64) -> Result<Option<Value>> {
        let Some(host) = &self.wire else {
            return Ok(None);
        };
        let p = self.prepared.wire.as_ref().unwrap();
        let mut boundaries = Vec::new();
        for (packet, state) in p.packets.iter().zip(&host.boundaries) {
            if let Some(plan) = &packet.boundary {
                let m = state.metrics(plan, h).map_err(error)?;
                boundaries.push(json!({"packet_id":packet.id,"ingress_port":packet.ingress_port,"rx_wire_ps":m.rx_wire_ps.to_string(),"rx_ifg_ps":m.rx_ifg_ps.to_string(),"completed_frames":m.completed_frames.to_string(),"completed_mac_bytes":m.completed_mac_bytes.to_string(),"completed_bits":m.completed_bits.to_string(),"actual_start_ps":opt(state.actual_start_ps),"actual_arrival_ps":opt(state.actual_arrival_ps),"actual_release_ps":opt(state.actual_release_ps)}));
            }
        }
        let tx=host.tx.values().map(|t|json!({"packet_id":t.intent.packet_id,"port":t.intent.port,"mac_bytes":t.intent.bytes.len().to_string(),"sof_ps":opt(t.sof),"eof_ps":opt(t.eof),"release_ps":opt(t.release),"arrival_ps":opt(t.arrival),"planned_eof_ps":opt(t.planned_eof),"planned_release_ps":opt(t.planned_release),"planned_arrival_ps":opt(t.planned_arrival),"busy_ps":t.sof.map_or(0,|s|h.min(t.planned_release.unwrap_or(h)).saturating_sub(s)).to_string(),"dropped":t.dropped})).collect::<Vec<_>>();
        Ok(Some(
            json!({"boundaries":boundaries,"tx":tx,"protocol":crate::runtime::wire::transition::public_state_projection(&host.protocol.runtime_snapshot()).map_err(error)?,"timers":host.timers.iter().map(|((adapter,key),(generation,deadline))|json!({"adapter_id":adapter,"key":key,"generation":generation.to_string(),"deadline_ps":deadline.to_string()})).collect::<Vec<_>>()}),
        ))
    }
}
fn tagged_ip(
    source: [u8; 6],
    destination: [u8; 6],
    vid: u16,
    protocol: WireProtocol,
    payload: &[u8],
) -> Vec<u8> {
    let mut client = Vec::new();
    client.extend(vid.to_be_bytes());
    client.extend(
        if protocol == WireProtocol::Igmp {
            0x0800u16
        } else {
            0x86ddu16
        }
        .to_be_bytes(),
    );
    client.extend(payload);
    client.resize(4 + payload.len().max(46), 0);
    crate::runtime::wire::codec::frame(destination, source, 0x8100, &client)
}
#[cfg(test)]
mod atomic_tests {
    #[test]
    fn boundary_receive_failure_preserves_started_only_prefix() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/ethernet-wire/passive.ini"),
        )
        .unwrap();
        crate::allocation::fail_next_reservation("network_wire_receive");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        let records = &failed.registered.as_ref().unwrap().model_records;
        assert!(
            !records
                .values()
                .any(|r| r.schema.name == "ethernet.wire.membership")
        );
        assert!(
            !records
                .values()
                .any(|r| r.schema.name == "ethernet.wire.boundary" && r.data["event"] == "arrival")
        );
        assert!(
            records
                .values()
                .any(|r| r.schema.name == "ethernet.wire.boundary" && r.data["event"] == "start")
        );
        let host = &failed.network.as_ref().unwrap()["wire"];
        assert!(host["tx"].as_array().unwrap().is_empty());
        assert!(
            host["boundaries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["actual_arrival_ps"].is_null())
        );
    }
    #[test]
    fn control_fifo_reservation_failure_rolls_back_protocol_initialization() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/ethernet-wire/active.ini"),
        )
        .unwrap();
        crate::allocation::fail_next_reservation("network_wire_fifo");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "prep_failed");
        assert!(failed.registered.as_ref().unwrap().model_records.is_empty());
        assert!(failed.registered.as_ref().unwrap().deliveries.is_empty());
    }
}
#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::snapshot::wire::WirePortPolicy;
    fn prepared() -> crate::PreparedSimulation {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/ethernet-wire/active.ini"),
        )
        .unwrap()
    }
    #[test]
    fn stale_host_timer_audits_without_displacing_current_reservation() {
        let p = prepared();
        let registered = p.registered.as_ref().unwrap();
        let mut network = registered.network.as_ref().unwrap().as_ref().clone();
        network
            .wire
            .as_mut()
            .unwrap()
            .adapters
            .retain(|a| a.protocol == WireProtocol::Rstp);
        let mut state = NetworkState::new(Arc::new(network)).unwrap();
        let mut batch = crate::registry::EffectBatch::default();
        let mut next = 0;
        let channels = BTreeMap::new();
        let live = std::collections::BTreeSet::new();
        let mut c = Context {
            now: 0,
            config: &registered.models[0],
            registry: &registered.registry,
            profile: registered.registry.profile(&p.common.profile).unwrap(),
            channels: &channels,
            channel_configs: &[],
            live_timers: &live,
            executing_timer: None,
            next_id: &mut next,
            batch: &mut batch,
        };
        let d = state.initialize(&mut c, p.common.time_limit_ps).unwrap();
        state.reserve(&d).unwrap();
        state.apply(d);
        c.batch.effects.clear();
        let ((adapter, key), (generation, deadline)) = state
            .wire
            .as_ref()
            .unwrap()
            .timers
            .iter()
            .next()
            .map(|(k, v)| (k.clone(), *v))
            .unwrap();
        let before = state.wire.as_ref().unwrap().protocol.runtime_snapshot();
        c.now = deadline;
        let mut d = state.delta();
        state
            .wire_event(
                HostEvent::Timer {
                    adapter: adapter.clone(),
                    key: key.clone(),
                    generation: generation - 1,
                    deadline,
                },
                &mut d,
                &mut c,
            )
            .unwrap();
        assert_eq!(
            d.wire.as_ref().unwrap().timers.get(&(adapter, key)),
            Some(&(generation, deadline))
        );
        let mut after = d.wire.as_ref().unwrap().protocol.runtime_snapshot();
        let mut before = before;
        for field in ["callback", "record_sequence"] {
            before.as_object_mut().unwrap().remove(field);
            after.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(before, after);
        assert!(
            c.batch
                .effects
                .iter()
                .all(|e| matches!(e, Effect::Record(_)))
        );
        assert!(c.batch.effects.iter().any(|e|matches!(e,Effect::Record(r)if r.schema.name=="ethernet.control.wire.event"&&r.data["outcome"]=="stale")));
    }
    #[test]
    fn ordinary_data_learns_in_learning_state_without_relay_and_discard_does_not_learn() {
        let p = prepared();
        let registered = p.registered.as_ref().unwrap();
        for learning in [true, false] {
            let network = registered.network.clone().unwrap();
            let mut state = NetworkState::new(network).unwrap();
            let mut policies = BTreeMap::new();
            policies.insert(
                "Net.s1.tx_a".into(),
                WirePortPolicy {
                    role: "Designated".into(),
                    learning,
                    forwarding: false,
                    link_up: true,
                },
            );
            let (dynamic, _, _) = state
                .dynamic
                .as_ref()
                .unwrap()
                .plan_wire_in(None, 0, &policies, &Default::default(), &[], true, true)
                .unwrap();
            state.dynamic.as_mut().unwrap().apply(dynamic);
            let source = state
                .prepared
                .ethernet
                .devices
                .iter()
                .position(|d| d.id == "Net.a")
                .unwrap();
            let wire = crate::runtime::ethernet::serialize_vlan_frame(
                "02:00:00:00:00:01",
                "02:00:00:00:00:02",
                2048,
                "00",
                Some(EthernetVlanTag {
                    vid: 10,
                    pcp: 0,
                    dei: 0,
                }),
            )
            .unwrap();
            state.frames.push(Frame {
                generator: None,
                row: EthernetFrameRecord {
                    frame_id: "opaque_data:0".into(),
                    time_ps: 0,
                    source: "Net.a".into(),
                    source_vlan_id: Some(10),
                    wire,
                    flow_id: None,
                    priority: 0,
                    deadline_ps: None,
                    generated_ps: 0,
                    ready_ps: None,
                },
                ip: None,
                visits: 0,
                lineage: None,
                branch_id: None,
            });
            let mut batch = crate::registry::EffectBatch::default();
            let mut next = 0;
            let channels = BTreeMap::new();
            let live = std::collections::BTreeSet::new();
            let mut c = Context {
                now: 0,
                config: &registered.models[0],
                registry: &registered.registry,
                profile: registered.registry.profile(&p.common.profile).unwrap(),
                channels: &channels,
                channel_configs: &[],
                live_timers: &live,
                executing_timer: None,
                next_id: &mut next,
                batch: &mut batch,
            };
            let mut d = state.delta();
            state.source_offer(0, &mut d, &mut c).unwrap();
            state.reserve(&d).unwrap();
            state.apply(d);
            let d = state.arbitrate(&mut c).unwrap();
            state.reserve(&d).unwrap();
            state.apply(d);
            assert_eq!(
                state.prepared.ethernet.directions[state.transfers[0].direction].source,
                source
            );
            let arrival = state.transfers[0].row.planned_arrival_ps.unwrap();
            c.now = arrival;
            let mut d = state.delta();
            state.arrive(0, &mut d, &mut c).unwrap();
            state.reserve(&d).unwrap();
            state.apply(d);
            let mac = state.dynamic.as_ref().unwrap().snapshot_tables()["mac"]
                .as_array()
                .unwrap()
                .len();
            assert_eq!(mac, usize::from(learning));
            if learning {
                c.now = state.receptions[0].row.planned_ready_ps.unwrap();
                let mut d = state.delta();
                state.complete(0, &mut d, &mut c).unwrap();
                state.reserve(&d).unwrap();
                state.apply(d);
                assert_eq!(
                    state.receptions[0].row.reason.as_deref(),
                    Some("stp_learning")
                );
            } else {
                assert_eq!(
                    state.receptions[0].row.reason.as_deref(),
                    Some("stp_discarding")
                );
            }
            assert_eq!(state.transfers.len(), 1);
        }
    }
    crate::runtime::ethernet::dynamic::tests::vector_host_private_cases!();
}

#[cfg(test)]
#[path = "../wire/order_host_tests.rs"]
mod wire_order_tests;
