//! Actual control media and protocol host, inside the single network owner.
use super::*;
use crate::runtime::wire::boundary::{BoundaryState, Event as BoundaryEvent};
use crate::runtime::wire::{WireEvent, WireState, WireTxIntent, WireView};
use crate::snapshot::wire::{WireEventRecord, WireTimerKey};
use crate::types::wire::{PortAttachment, PreparedWire, WireProtocol};

type IpControlMetadata = (Option<[u8; 6]>, Option<[u8; 6]>, WireProtocol);
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
    Logical(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ControlTransport {
    MacControl {
        epoch: u64,
    },
    LocalProtocol {
        component: String,
        port: String,
    },
    RelayedService {
        component: String,
        ingress: String,
        egress: String,
        vid: u16,
        tree: u16,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LogicalWork {
    pub at_ps: u64,
    pub source_component: String,
    pub source_port: String,
    pub peer_component: String,
    pub peer_port: String,
    pub adapter_id: String,
    pub packet_id: String,
    pub bytes: Vec<u8>,
    pub cause_id: String,
    pub relayed: bool,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlTx {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_ps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrival_retired_ps: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<ControlTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phy_transfer: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub phy_receivers: BTreeMap<String, u64>,
}
impl ControlTx {
    pub(super) fn is_mac_control(&self) -> bool {
        matches!(self.transport, Some(ControlTransport::MacControl { .. }))
            && self.intent.protocol == WireProtocol::MacControl
    }
    fn validate_transport(&self, prepared: &PreparedWire) -> Result<()> {
        if self.is_mac_control() {
            return if self.intent.adapter_id == "@mac-control"
                && self.intent.bytes == self.payload
                && self.intent.bytes.len() == 64
                && self.intent.bytes[12..16] == [0x88, 8, 0, 1]
            {
                Ok(())
            } else {
                Err(error("invalid MAC Control transport"))
            };
        }
        use crate::types::wire::{TreeDomain, WireOptions};
        let adapter = prepared
            .adapter(&self.intent.adapter_id)
            .ok_or_else(|| error("wire checkpoint transport owner missing"))?;
        let Some(binding) = &adapter.component_binding else {
            return if self.transport.is_none() {
                Ok(())
            } else {
                Err(error("legacy wire TX has component transport"))
            };
        };
        let envelope =
            crate::runtime::wire::component_envelope::decode(&self.payload, adapter.max_pdu_bytes)
                .map_err(|e| error(e.0))?;
        if self.payload != self.intent.bytes
            || self.source.is_some()
            || self.destination.is_some()
            || !matches!(
                binding.ports.get(&self.intent.port),
                Some(PortAttachment::Physical { .. })
            )
        {
            return Err(error(
                "wire checkpoint component transport bytes/physical port",
            ));
        }
        let valid = match &self.transport {
            Some(ControlTransport::LocalProtocol { component, port }) => {
                component == &binding.component
                    && port == &self.intent.port
                    && envelope.domain == binding.domain
                    && envelope.tags.is_empty()
                    && self.vid.is_none()
                    && prepared
                        .ports
                        .get(&self.intent.port)
                        .and_then(|port| port.source_mac)
                        == Some(envelope.source)
            }
            Some(ControlTransport::RelayedService {
                component,
                ingress,
                egress,
                vid,
                tree,
            }) => {
                let expected_tree = match &adapter.options {
                    WireOptions::Mstp(m) => m.vlan_map.get(*vid as usize).copied(),
                    _ => Some(0),
                };
                binding.domain == TreeDomain::Provider
                    && envelope.domain == TreeDomain::Customer
                    && component == &binding.component
                    && egress == &self.intent.port
                    && ingress != egress
                    && binding.ports.contains_key(ingress)
                    && (1..=4094).contains(vid)
                    && prepared
                        .registrar_modes
                        .contains_key(&(ingress.clone(), *vid))
                    && prepared
                        .registrar_modes
                        .contains_key(&(egress.clone(), *vid))
                    && expected_tree == Some(*tree)
                    && self.vid == Some(*vid)
                    && envelope.tags.first().is_some_and(|tag| {
                        tag.tpid == crate::types::ethernet::EthernetTag::S_TPID
                            && tag.tci.vid == *vid
                    })
            }
            Some(ControlTransport::MacControl { .. }) | None => false,
        };
        if valid {
            Ok(())
        } else {
            Err(error(
                "wire checkpoint component transport/domain/tree mismatch",
            ))
        }
    }
}
#[derive(Debug, Clone)]
pub(super) struct WireHost {
    pub protocol: WireState,
    pub boundaries: Vec<BoundaryState>,
    pub queues: Vec<VecDeque<String>>,
    pub active: Vec<Option<String>>,
    pub tx: BTreeMap<String, ControlTx>,
    pub logical: BTreeMap<String, LogicalWork>,
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
            logical: BTreeMap::new(),
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
    pub(super) fn mstp_tree_policy(
        &self,
        d: &NetworkDelta,
        port: &str,
        vid: u16,
    ) -> Option<crate::snapshot::wire::WirePortPolicy> {
        self.wire_host(d)?.protocol.tree_policy(port, vid)
    }
    pub(super) fn data_egress_reason(
        &self,
        d: &NetworkDelta,
        port: &str,
        vid: u16,
    ) -> Option<&'static str> {
        let base = self
            .dynamic
            .as_ref()
            .zip(self.policy(d).as_ref())
            .and_then(|(state, policy)| state.eligible_egress(port, vid, policy).err());
        let Some(tree) = self.mstp_tree_policy(d, port, vid) else {
            return base.map(|reason| self.data_reason(reason, port, d));
        };
        if !tree.link_up {
            return Some("link_down");
        }
        if let Some(reason) = base.filter(|reason| reason.reason() != "stp_discarding") {
            return Some(reason.reason());
        }
        if !tree.forwarding {
            Some(if tree.learning {
                "stp_learning"
            } else {
                "stp_discarding"
            })
        } else {
            None
        }
    }
    pub(super) fn data_ingress_reason(
        &self,
        d: &NetworkDelta,
        port: &str,
        vid: u16,
    ) -> Option<&'static str> {
        let base = self
            .dynamic
            .as_ref()
            .zip(self.policy(d).as_ref())
            .and_then(|(state, policy)| state.eligible_egress(port, vid, policy).err());
        let Some(tree) = self.mstp_tree_policy(d, port, vid) else {
            return base.map(|reason| reason.reason());
        };
        if !tree.link_up {
            return Some("link_down");
        }
        if let Some(reason) = base.filter(|reason| reason.reason() != "stp_discarding") {
            return Some(reason.reason());
        }
        if !tree.learning {
            Some("stp_discarding")
        } else {
            None
        }
    }
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
    pub(super) fn wire_mut<'a>(&self, d: &'a mut NetworkDelta) -> Result<&'a mut WireHost> {
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
                    let base =
                        crate::runtime::ethernet::dynamic::eligible_egress(port, *vid, &policy);
                    let eligible = match self.mstp_tree_policy(d, port, *vid) {
                        Some(tree) => {
                            tree.link_up
                                && tree.forwarding
                                && base
                                    .as_ref()
                                    .err()
                                    .is_none_or(|reason| reason.reason() == "stp_discarding")
                        }
                        None => base.is_ok(),
                    };
                    view.ingress_eligible.insert((port.clone(), *vid), eligible);
                }
            }
        }
        for segment in &self.prepared.shared_lans {
            for &direction in &segment.members {
                view.oper_point_to_point_mac.insert(
                    self.prepared.ethernet.directions[direction]
                        .from_port
                        .clone(),
                    false,
                );
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
        let active_receive = match &event {
            WireEvent::Receive {
                protocol: WireProtocol::Igmp | WireProtocol::Mld,
                ingress_port,
                packet_id,
                bytes,
                vid: Some(vid),
                ..
            } => Some((ingress_port.clone(), *vid, bytes.clone(), packet_id.clone())),
            _ => None,
        };
        let view = self.wire_view(d);
        let mut protocol = self.wire_mut(d)?.protocol.plan(c.now(), event, &view)?;
        // Invalidate only actual egress policy transitions, rather than coarse
        // dynamic dirty-port sets that also contain unaffected refresh targets.
        let host = self.wire_host(d).unwrap();
        let old_policies = host.protocol.policies();
        let old_trees = host.protocol.tree_policies();
        let old_registered = host.protocol.registered();
        let affected = self
            .prepared
            .ethernet
            .directions
            .iter()
            .enumerate()
            .filter(|(i, link)| {
                self.merge_raw_transport(*i)
                    && (old_policies.get(&link.from_port) != protocol.policies.get(&link.from_port)
                        || old_trees
                            .iter()
                            .filter(|((_, _, port), _)| port == &link.from_port)
                            .collect::<Vec<_>>()
                            != protocol
                                .tree_policies
                                .iter()
                                .filter(|((_, _, port), _)| port == &link.from_port)
                                .collect::<Vec<_>>()
                        || old_registered
                            .iter()
                            .filter(|(port, _)| port == &link.from_port)
                            .collect::<Vec<_>>()
                            != protocol
                                .registered
                                .iter()
                                .filter(|(port, _)| port == &link.from_port)
                                .collect::<Vec<_>>())
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        for i in affected {
            self.merge_permission_changed(i, d)?;
        }

        if receiving {
            crate::allocation::reservation_checkpoint("network_wire_receive")?;
        }
        let (dynamic, mut flush, mac) = self
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
        if let Some(topology) = &self.prepared.provider_l2 {
            let fdb = d
                .provider_fdb
                .get_or_insert_with(|| self.provider_fdb.clone().expect("prepared provider FDB"));
            for (intent, completed) in protocol.flush.iter().zip(&mut flush) {
                if let Some(port) = topology.ports.get(&intent.port) {
                    let vids = intent.vids.iter().copied().collect();
                    completed.extend(
                        fdb.flush_port(&port.component, &intent.port, &vids)
                            .into_iter()
                            .map(|(component, vid, mac)| crate::snapshot::wire::WireMacKey {
                                switch: component,
                                vid,
                                mac,
                            }),
                    );
                    completed.sort();
                }
            }
        }
        let dynamic = self.dynamic.as_ref().unwrap().plan_wire_trees_in(
            dynamic,
            c.now(),
            &protocol.tree_policies,
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
        if let Some((ingress, vid, bytes, packet_id)) = active_receive {
            // The passive wire owner audits malformed input. Its TX intent can
            // also contain a complete MAC frame rather than an IP packet.
            // Only a decoded IP control payload enters the active actor.
            let v6 = bytes.first().is_some_and(|b| *b >> 4 == 6);
            if crate::runtime::wire::membership::decode(&bytes, v6, 4096).is_ok() {
                self.multicast_receive(&ingress, vid, bytes, packet_id, d, c)?;
            }
        }
        for intent in tx {
            let payload = intent.bytes.clone();
            if let Some(binding) = self
                .prepared
                .wire
                .as_ref()
                .unwrap()
                .adapter(&intent.adapter_id)
                .and_then(|a| a.component_binding.as_ref())
            {
                if let Some(PortAttachment::Internal { peer, .. }) = binding.ports.get(&intent.port)
                {
                    self.logical_enqueue(
                        LogicalWork {
                            at_ps: c.now(),
                            source_component: binding.component.clone(),
                            source_port: intent.port.clone(),
                            peer_component: peer.component.clone(),
                            peer_port: peer.port.clone(),
                            adapter_id: intent.adapter_id.clone(),
                            packet_id: intent.packet_id.clone(),
                            bytes: payload,
                            cause_id: intent.cause_id.clone(),
                            relayed: false,
                        },
                        d,
                        c,
                    )?;
                    continue;
                }
            }
            let transport = self
                .prepared
                .wire
                .as_ref()
                .unwrap()
                .adapter(&intent.adapter_id)
                .and_then(|a| a.component_binding.as_ref())
                .map(|binding| ControlTransport::LocalProtocol {
                    component: binding.component.clone(),
                    port: intent.port.clone(),
                });
            self.wire_enqueue(
                ControlTx {
                    queued_ps: None,
                    arrival_retired_ps: None,
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
                    phy_transfer: None,
                    phy_receivers: BTreeMap::new(),
                    transport,
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
                let output_adapter = self
                    .prepared
                    .wire
                    .as_ref()
                    .unwrap()
                    .owner(meta.2, &port)
                    .ok_or_else(|| error("forwarded control output has no protocol owner"))?;
                let intent = WireTxIntent {
                    adapter_id: output_adapter.id.clone(),
                    packet_id: id,
                    port,
                    protocol: meta.2,
                    ordinal: d.record_sequence,
                    bytes,
                    cause_id: forward.packet_id.clone(),
                };
                self.wire_enqueue(
                    ControlTx {
                        queued_ps: None,
                        arrival_retired_ps: None,
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
                        phy_transfer: None,
                        phy_receivers: BTreeMap::new(),
                        transport: None,
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
            ));
        }
        let tx = self
            .wire_host(d)
            .unwrap()
            .tx
            .get(id)
            .ok_or_else(|| error("forwarded control lineage missing"))?;
        Ok((tx.source, tx.destination, tx.intent.protocol))
    }
    fn logical_enqueue(
        &self,
        work: LogicalWork,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let wire = self
            .prepared
            .wire
            .as_ref()
            .ok_or_else(|| error("logical ISS lacks wire topology"))?;
        let source = wire
            .adapter(&work.adapter_id)
            .and_then(|a| a.component_binding.as_ref())
            .ok_or_else(|| error("logical ISS source component missing"))?;
        let target = wire
            .adapters
            .iter()
            .filter_map(|a| a.component_binding.as_ref())
            .find(|b| b.component == work.peer_component)
            .ok_or_else(|| error("logical ISS peer component missing"))?;
        let source_key = (&work.source_component, &work.source_port);
        let peer_key = (&work.peer_component, &work.peer_port);
        if source.component != *source_key.0
            || !matches!(source.ports.get(source_key.1), Some(PortAttachment::Internal { peer, .. })
                if &peer.component == peer_key.0 && &peer.port == peer_key.1)
            || !matches!(target.ports.get(peer_key.1), Some(PortAttachment::Internal { peer, .. })
                if &peer.component == source_key.0 && &peer.port == source_key.1)
        {
            return Err(error("logical ISS attachment mismatch"));
        }
        let host = self.wire_mut(d)?;
        if host.logical.contains_key(&work.packet_id)
            || host.logical.len()
                >= self
                    .prepared
                    .extensions
                    .as_ref()
                    .ok_or_else(|| error("logical ISS limits missing"))?
                    .limits
                    .max_pending_branches as usize
        {
            return Err(error("logical ISS pending capacity/identity"));
        }
        crate::allocation::reservation_checkpoint("network_logical_iss")?;
        let id = work.packet_id.clone();
        host.logical.insert(id.clone(), work);
        self.due_mut(d).push(Due {
            at: c.now(),
            kind: Completion::Wire(HostEvent::Logical(id)),
        });
        Ok(())
    }
    pub(super) fn provider_tree_policy(
        &self,
        port: &str,
        vid: u16,
        d: &NetworkDelta,
    ) -> Option<crate::snapshot::wire::WirePortPolicy> {
        let host = self.wire_host(d)?;
        host.protocol
            .tree_policy(port, vid)
            .or_else(|| host.protocol.policies().get(port).cloned())
    }
    /// Enter a provider component from an actual physical MAC receive or an
    /// internal ISS indication. DA and FCS are decoded from the bytes before
    /// component dispatch; sender adapter metadata never selects the receiver.
    pub(super) fn provider_bpdu_ingress(
        &self,
        ingress: &str,
        packet_id: &str,
        bytes: &[u8],
        external: bool,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        use crate::runtime::network::provider_l2::{ControlDisposition, PortRole};
        let topology = self
            .prepared
            .provider_l2
            .as_ref()
            .ok_or_else(|| error("provider BPDU ingress lacks topology"))?;
        let port = topology
            .ports
            .get(ingress)
            .ok_or_else(|| error("provider BPDU ingress component port unknown"))?;
        let envelope = match crate::runtime::wire::component_envelope::decode(bytes, 1500) {
            Ok(v) => v,
            Err(reason) => {
                self.provider_record(
                    "filtered",
                    &port.component,
                    port.pvid,
                    ingress,
                    packet_id,
                    None,
                    Some(&reason.0),
                    d,
                    c,
                )?;
                return Ok(());
            }
        };
        let domain = envelope.domain;
        let frame = envelope.into_wire_frame();
        let class = match topology.classify_ingress(ingress, &frame) {
            Ok(v) => v,
            Err(reason) => {
                self.provider_record(
                    "filtered",
                    &port.component,
                    port.pvid,
                    ingress,
                    packet_id,
                    None,
                    Some(&reason.message),
                    d,
                    c,
                )?;
                return Ok(());
            }
        };
        let disposition =
            crate::runtime::network::provider_l2::control_disposition(port.role, &frame.dst_mac);
        self.provider_record(
            "control_ingress",
            &class.component,
            class.vid,
            ingress,
            packet_id,
            None,
            None,
            d,
            c,
        )?;
        match disposition {
            ControlDisposition::Drop | ControlDisposition::Ordinary => {
                self.provider_record(
                    "filtered",
                    &class.component,
                    class.vid,
                    ingress,
                    packet_id,
                    None,
                    Some("component_reserved_address"),
                    d,
                    c,
                )?;
            }
            ControlDisposition::Consume => {
                let adapter = self
                    .prepared
                    .wire
                    .as_ref()
                    .unwrap()
                    .adapters
                    .iter()
                    .find(|a| {
                        a.component_binding
                            .as_ref()
                            .is_some_and(|b| b.component == class.component)
                    })
                    .ok_or_else(|| error("provider control recipient adapter missing"))?;
                let expected_domain = adapter.component_binding.as_ref().unwrap().domain;
                if domain != expected_domain {
                    return Err(error("provider control destination/component mismatch"));
                }
                self.provider_record(
                    "control_consumed",
                    &class.component,
                    class.vid,
                    ingress,
                    packet_id,
                    Some(&adapter.id),
                    None,
                    d,
                    c,
                )?;
                self.wire_protocol(
                    WireEvent::Receive {
                        protocol: adapter.protocol,
                        ingress_port: ingress.to_owned(),
                        packet_id: packet_id.to_owned(),
                        bytes: bytes.to_vec(),
                        vid: Some(class.vid),
                        destination_mac: None,
                        external_injection: external,
                    },
                    d,
                    c,
                )?;
            }
            ControlDisposition::Forward => {
                if !matches!(
                    port.role,
                    PortRole::CustomerNetwork | PortRole::ProviderNetwork
                ) || self
                    .provider_tree_policy(ingress, class.vid, d)
                    .is_none_or(|policy| !policy.learning || !policy.forwarding || !policy.link_up)
                {
                    self.provider_record(
                        "filtered",
                        &class.component,
                        class.vid,
                        ingress,
                        packet_id,
                        None,
                        Some("service_ingress_tree"),
                        d,
                        c,
                    )?;
                    return Ok(());
                }
                let targets: Vec<_> = topology
                    .ports
                    .iter()
                    .filter(|(name, p)| {
                        *name != ingress
                            && p.component == class.component
                            && p.member_vids.contains(&class.vid)
                            && matches!(
                                p.role,
                                PortRole::CustomerNetwork | PortRole::ProviderNetwork
                            )
                    })
                    .map(|(name, p)| (name.clone(), p.role))
                    .collect();
                for (egress, role) in targets {
                    if self
                        .provider_tree_policy(&egress, class.vid, d)
                        .is_none_or(|policy| !policy.forwarding || !policy.link_up)
                    {
                        continue;
                    }
                    let adapter = self
                        .prepared
                        .wire
                        .as_ref()
                        .unwrap()
                        .adapters
                        .iter()
                        .find(|a| {
                            a.component_binding
                                .as_ref()
                                .is_some_and(|b| b.component == class.component)
                        })
                        .ok_or_else(|| error("provider relay adapter missing"))?;
                    if role == PortRole::CustomerNetwork {
                        for target in topology.handoff_all(&egress, &class).map_err(|e| {
                            error(format!("provider control handoff: {}", e.message))
                        })? {
                            let id = replication::identifier(&[
                                packet_id,
                                &egress,
                                &target.ingress_port,
                                &d.record_sequence.to_string(),
                            ]);
                            let decoded = crate::runtime::wire::codec::unhex(&target.wire.mac_hex)
                                .map_err(|e| error(e.0))?;
                            self.logical_enqueue(
                                LogicalWork {
                                    at_ps: c.now(),
                                    source_component: class.component.clone(),
                                    source_port: egress.clone(),
                                    peer_component: target.component.clone(),
                                    peer_port: target.ingress_port.clone(),
                                    adapter_id: adapter.id.clone(),
                                    packet_id: id.clone(),
                                    bytes: decoded,
                                    cause_id: packet_id.to_owned(),
                                    relayed: true,
                                },
                                d,
                                c,
                            )?;
                            self.provider_record(
                                "control_handoff",
                                &class.component,
                                class.vid,
                                &egress,
                                &id,
                                Some(&target.component),
                                None,
                                d,
                                c,
                            )?;
                        }
                    } else {
                        let wire = topology
                            .encode_egress(&egress, &class)
                            .map_err(|e| error(format!("provider BPDU egress: {}", e.message)))?;
                        let encoded = crate::runtime::wire::codec::unhex(&wire.mac_hex)
                            .map_err(|e| error(e.0))?;
                        let id = replication::identifier(&[
                            packet_id,
                            &egress,
                            &d.record_sequence.to_string(),
                        ]);
                        let tree = match &adapter.options {
                            crate::types::wire::WireOptions::Mstp(m) => {
                                m.vlan_map[class.vid as usize]
                            }
                            _ => 0,
                        };
                        let intent = WireTxIntent {
                            adapter_id: adapter.id.clone(),
                            packet_id: id.clone(),
                            port: egress.clone(),
                            protocol: adapter.protocol,
                            ordinal: d.record_sequence,
                            bytes: encoded.clone(),
                            cause_id: packet_id.to_owned(),
                        };
                        self.wire_enqueue(
                            ControlTx {
                                queued_ps: None,
                                arrival_retired_ps: None,
                                intent,
                                payload: encoded,
                                vid: Some(class.vid),
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
                                phy_transfer: None,
                                phy_receivers: BTreeMap::new(),
                                transport: Some(ControlTransport::RelayedService {
                                    component: class.component.clone(),
                                    ingress: ingress.to_owned(),
                                    egress: egress.clone(),
                                    vid: class.vid,
                                    tree,
                                }),
                            },
                            d,
                            c,
                        )?;
                        self.provider_record(
                            "control_relay",
                            &class.component,
                            class.vid,
                            &egress,
                            &id,
                            None,
                            None,
                            d,
                            c,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn wire_enqueue(
        &self,
        mut tx: ControlTx,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
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
        let max_pending = if tx.is_mac_control() {
            self.prepared
                .extensions
                .as_ref()
                .map_or(4096, |e| e.limits.max_pending_branches as usize)
        } else {
            p.adapter(&tx.intent.adapter_id)
                .ok_or_else(|| error("wire TX adapter unknown"))?
                .max_pending_tx
        };
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
        if pending >= max_pending {
            return Err(error("wire control FIFO capacity exceeded"));
        }
        crate::allocation::reservation_checkpoint("network_wire_fifo")?;
        let id = tx.intent.packet_id.clone();
        host.queues[direction].push_back(id.clone());
        tx.queued_ps = Some(c.now());
        host.tx.insert(id.clone(), tx);
        self.wire_tx_record(direction, &id, "tx_queued", None, d, c)?;
        c.request_arbitration("network").map_err(model)?;
        Ok(())
    }
    pub(super) fn active_wire_tx(
        &self,
        row: multicast_active::Transmission,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let protocol = if row.ip_bytes.first().is_some_and(|b| *b >> 4 == 6) {
            WireProtocol::Mld
        } else {
            WireProtocol::Igmp
        };
        let prepared = self
            .prepared
            .wire
            .as_ref()
            .ok_or_else(|| error("active multicast wire missing"))?;
        let adapter = prepared
            .adapters
            .iter()
            .find(|a| a.protocol == protocol && a.ports.contains(&row.output_port))
            .ok_or_else(|| error("active multicast output adapter missing"))?;
        let bytes = tagged_ip(
            row.source_mac,
            row.destination_mac,
            row.vid,
            protocol,
            &row.ip_bytes,
        );
        let id = format!(
            "@active/{}/{}/{}",
            c.now(),
            row.interface,
            self.wire_host(d)
                .ok_or_else(|| error("active multicast wire host missing"))?
                .tx
                .len()
        );
        let intent = WireTxIntent {
            adapter_id: adapter.id.clone(),
            packet_id: id,
            port: row.output_port,
            protocol,
            ordinal: d.record_sequence,
            bytes,
            cause_id: row.reason.to_owned(),
        };
        self.wire_enqueue(
            ControlTx {
                queued_ps: None,
                arrival_retired_ps: None,
                intent,
                payload: row.ip_bytes,
                vid: Some(row.vid),
                destination: Some(row.destination_mac),
                source: Some(row.source_mac),
                sof: None,
                eof: None,
                release: None,
                arrival: None,
                planned_eof: None,
                planned_release: None,
                planned_arrival: None,
                dropped: false,
                phy_transfer: None,
                phy_receivers: BTreeMap::new(),
                transport: None,
            },
            d,
            c,
        )
    }
    pub(super) fn wire_event(
        &self,
        event: HostEvent,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        self.wire_mut(d)?.phase = if matches!(
            event,
            HostEvent::Packet(_) | HostEvent::Arrival { .. } | HostEvent::Logical(_)
        ) {
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
                let adapter = self
                    .prepared
                    .wire
                    .as_ref()
                    .unwrap()
                    .adapter(&packet.adapter_id)
                    .unwrap();
                let protocol = adapter.protocol;
                if packet.boundary.is_some() {
                    self.wire_boundary(i, BoundaryEvent::Arrival, d, c)?;
                }
                let rx = self
                    .prepared
                    .wire
                    .as_ref()
                    .unwrap()
                    .ports
                    .values()
                    .find(|p| p.ingress == packet.ingress_port || p.output == packet.ingress_port)
                    .map(|p| p.direction);
                if let Some(rx) = rx {
                    if self.pause_receive(rx, &packet.bytes, &packet.id, d, c)? {
                        return Ok(());
                    }
                }
                if adapter.component_binding.is_some() {
                    let port = self
                        .prepared
                        .wire
                        .as_ref()
                        .unwrap()
                        .ports
                        .values()
                        .find(|p| {
                            p.ingress == packet.ingress_port || p.output == packet.ingress_port
                        })
                        .ok_or_else(|| error("provider boundary ingress port unknown"))?;
                    self.provider_bpdu_ingress(
                        &port.output,
                        &packet.id,
                        &packet.bytes,
                        true,
                        d,
                        c,
                    )?;
                } else {
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
                if tx.dropped || tx.eof.is_some() {
                    return Ok(());
                }
                tx.eof = Some(c.now());
                let arrival = tx
                    .planned_arrival
                    .ok_or_else(|| error("wire arrival not planned"))?;
                self.pause_mac_completed(direction, d, c)?;
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
                if self.preemption_direction(direction) {
                    self.preemption_wire_release(direction, &id, d)?;
                }
                self.segment_release(direction, SegmentOwner::Control(id), d, c)?;
                c.request_arbitration("network").map_err(model)?;
            }
            HostEvent::Arrival { direction, id } => {
                let tx = self.wire_host(d).unwrap().tx[&id].clone();
                if tx.dropped {
                    self.wire_mut(d)?
                        .tx
                        .get_mut(&id)
                        .unwrap()
                        .arrival_retired_ps = Some(c.now());
                    return Ok(());
                }
                self.wire_mut(d)?.tx.get_mut(&id).unwrap().arrival = Some(c.now());
                let recipients = if self.segment_for(direction).is_some() {
                    self.segment_receivers(direction)
                        .into_iter()
                        .map(|receiver| {
                            self.effective_link(direction, Some(receiver))
                                .map(|link| (Some(receiver), link.to_port))
                        })
                        .collect::<Result<Vec<_>>>()?
                } else {
                    vec![(
                        None,
                        self.prepared.ethernet.directions[direction].to_port.clone(),
                    )]
                };
                self.wire_deliver(direction, &id, &tx, recipients, d, c)?;
            }
            HostEvent::Logical(id) => {
                let work = self
                    .wire_mut(d)?
                    .logical
                    .remove(&id)
                    .ok_or_else(|| error("logical ISS event missing pending work"))?;
                if work.at_ps != c.now() {
                    return Err(error("logical ISS event time mismatch"));
                }
                if work.relayed {
                    let topology = self
                        .prepared
                        .provider_l2
                        .as_ref()
                        .ok_or_else(|| error("logical service topology missing"))?;
                    let source = topology
                        .ports
                        .get(&work.source_port)
                        .ok_or_else(|| error("logical service source missing"))?;
                    if self
                        .provider_tree_policy(&work.source_port, source.pvid, d)
                        .is_none_or(|policy| !policy.link_up || !policy.forwarding)
                    {
                        self.provider_record(
                            "filtered",
                            &work.source_component,
                            source.pvid,
                            &work.source_port,
                            &work.packet_id,
                            Some(&work.peer_component),
                            Some("service_egress_tree"),
                            d,
                            c,
                        )?;
                        return Ok(());
                    }
                }
                self.provider_record(
                    "logical_iss",
                    &work.source_component,
                    0,
                    &work.source_port,
                    &work.packet_id,
                    Some(&work.peer_component),
                    Some(if work.relayed {
                        "relayed_service"
                    } else {
                        "local_protocol"
                    }),
                    d,
                    c,
                )?;
                self.provider_bpdu_ingress(
                    &work.peer_port,
                    &work.packet_id,
                    &work.bytes,
                    false,
                    d,
                    c,
                )?;
            }
        }
        Ok(())
    }
    pub(super) fn wire_deliver(
        &self,
        direction: usize,
        id: &str,
        tx: &ControlTx,
        recipients: Vec<(Option<usize>, String)>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let prepared = self.prepared.wire.as_ref().unwrap();
        if let Some(ControlTransport::MacControl { epoch }) = tx.transport {
            if self
                .pause_port(direction, d)
                .is_none_or(|p| p.epoch != epoch || !p.link_up)
            {
                self.wire_tx_record(direction, id, "receive", Some("stale_link_epoch"), d, c)?;
                return Ok(());
            }
        }
        for (receiver, ingress_port) in recipients {
            let rx = receiver.or_else(|| {
                self.prepared
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.ingress == ingress_port)
                    .and_then(|p| {
                        self.prepared
                            .ethernet
                            .directions
                            .iter()
                            .position(|d| d.from_port == p.port)
                    })
            });
            if let Some(rx) = rx {
                if self.pause_receive(rx, &tx.intent.bytes, id, d, c)? {
                    continue;
                }
            }
            if let Some(segment) = self.segment_for(direction) {
                self.segment_record(
                    segment,
                    &SegmentOwner::Control(id.to_owned()),
                    "receive",
                    direction,
                    receiver,
                    d,
                    c,
                )?;
            }
            if let Some(topology) = &self.prepared.provider_l2 {
                let physical = prepared
                    .ports
                    .values()
                    .find(|p| p.ingress == ingress_port || p.output == ingress_port)
                    .filter(|p| topology.ports.contains_key(&p.output))
                    .map(|p| p.output.clone());
                if let Some(physical) = physical {
                    let packet_id = if receiver.is_some() {
                        format!("{id}@{ingress_port}")
                    } else {
                        id.to_owned()
                    };
                    self.provider_bpdu_ingress(&physical, &packet_id, &tx.payload, false, d, c)?;
                    continue;
                }
            }
            // RSTP and MSTP share BPDU transport. Dispatch to the receiver's
            // tree adapter so its compatibility state can decode the neighbor.
            let recipient = if matches!(tx.intent.protocol, WireProtocol::Rstp | WireProtocol::Mstp)
            {
                prepared
                    .owner(WireProtocol::Mstp, &ingress_port)
                    .or_else(|| prepared.owner(WireProtocol::Rstp, &ingress_port))
            } else {
                prepared.owner(tx.intent.protocol, &ingress_port)
            };
            let Some(recipient) = recipient else {
                if receiver.is_none() {
                    self.wire_tx_record(direction, id, "receive", Some("unsupported_kind"), d, c)?;
                }
                continue;
            };
            self.wire_protocol(
                WireEvent::Receive {
                    protocol: recipient.protocol,
                    ingress_port: ingress_port.clone(),
                    packet_id: if receiver.is_some() {
                        format!("{id}@{ingress_port}")
                    } else {
                        id.to_owned()
                    },
                    bytes: tx.payload.clone(),
                    vid: tx.vid,
                    destination_mac: tx.destination,
                    external_injection: false,
                },
                d,
                c,
            )?;
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
    pub(super) fn wire_tx_record(
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
        if tx.is_mac_control() {
            return self.pause_wire_record(direction, id, kind, reason, d, c);
        }
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
            adapter_id: if matches!(kind, "receive" | "rx_failed") {
                format!(
                    "@wire/unowned/{}",
                    self.prepared.ethernet.devices[link.destination].id
                )
            } else {
                adapter.id.clone()
            },
            protocol: format!("{:?}", tx.intent.protocol).to_lowercase(),
            protocol_revision: tx.intent.protocol.revision().into(),
            bridge: if matches!(kind, "receive" | "rx_failed") {
                self.prepared.ethernet.devices[link.destination].id.clone()
            } else {
                adapter.bridge.clone()
            },
            port: if matches!(kind, "receive" | "rx_failed") {
                link.to_port.clone()
            } else {
                tx.intent.port.clone()
            },
            event_id: format!("@wire/host/{}", d.record_sequence),
            cause_id: Some(tx.intent.cause_id.clone()),
            event_kind: kind.into(),
            time_ps: c.now().to_string(),
            phase: if tx.phy_transfer.is_some() && kind != "tx_queued" {
                1
            } else if kind == "tx_started" {
                2
            } else if matches!(kind, "receive" | "rx_failed") {
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
    pub(super) fn wire_priority(&self, direction: usize, id: &str, d: &NetworkDelta) -> u8 {
        let bytes = &self.wire_host(d).unwrap().tx[id].intent.bytes;
        if bytes.len() >= 16 && matches!(&bytes[12..14], [0x81, 0] | [0x88, 0xa8]) {
            return bytes[14] >> 5;
        }
        let port = &self.prepared.ethernet.directions[direction].from_port;
        self.prepared
            .ethernet
            .port_policies
            .iter()
            .find(|p| p.port == *port)
            .map_or(0, |p| p.default_priority)
    }
    pub(super) fn wire_start_allowed(
        &self,
        direction: usize,
        id: &str,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        let tx = &self.wire_host(d).unwrap().tx[id];
        if !self.output_request_allowed(direction, tx.is_mac_control(), d, now) {
            return Ok(false);
        }
        if tx.is_mac_control() {
            return Ok(true);
        }
        if let Some(tsn) = &self.tsn {
            let link = &self.prepared.ethernet.directions[direction];
            let occupancy = if self.phy_attachment_for(direction).is_some()
                && !self.merge_raw_transport(direction)
            {
                self.phy_occupancy_ps(direction, tx.intent.bytes.len() as u64)?
            } else {
                l2::timing(0, tx.intent.bytes.len() as u64, link.bitrate_bps, 0)?.1
            };
            return tsn
                .retained_start_permission_clock_in(
                    &d.tsn,
                    &link.from_port,
                    self.wire_priority(direction, id, d),
                    now,
                    occupancy,
                    self.clock_for_port(d, &link.from_port),
                )
                .map(|p| p.is_some());
        }
        Ok(true)
    }
    pub(super) fn wire_arbitrate(
        &self,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<bool> {
        if self.merge_direction(direction) || self.preemption_direction(direction) {
            return Ok(false);
        }
        let Some(host) = self.wire_host(d) else {
            return Ok(false);
        };
        if self.segment_busy(direction, d) {
            return Ok(true);
        }
        if host.active[direction].is_some() {
            if self.configured_endpoint(direction).is_some() {
                self.media_try_start(direction, d, c)?;
            }
            return Ok(true);
        }
        if self.queue(d, direction).active.is_some() {
            return Ok(false);
        }
        self.wire_prune_candidates(direction, d, c)?;
        let Some(id) = self.wire_candidate(direction, d, c.now())? else {
            return Ok(false);
        };
        let link = &self.prepared.ethernet.directions[direction];
        self.wire_mut(d)?.queues[direction].retain(|candidate| candidate != &id);
        if self.configured_endpoint(direction).is_some() {
            self.wire_mut(d)?.active[direction] = Some(id.clone());
            self.media_claim(direction, configured_media::Work::Control(id), d, c)?;
            return Ok(true);
        }
        if self.phy_attachment_for(direction).is_some() {
            self.wire_phy_admit(direction, &id, d, c)?;
            return Ok(true);
        }
        let bytes = self.wire_host(d).unwrap().tx[&id].intent.bytes.len() as u64;
        let (eof, release, arrival) = l2::timing(c.now(), bytes, link.bitrate_bps, link.delay_ps)?;
        self.segment_reserve(
            direction,
            SegmentOwner::Control(id.clone()),
            eof,
            release,
            d,
            c,
        )?;
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
        Ok(true)
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
    // Existing ingress projection keeps bridge identity, packet and delta/time explicit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn wire_multicast(
        &self,
        bridge: &str,
        ingress: &str,
        vid: u16,
        ip: Option<&IpMulticast>,
        wire: &EthernetWireFrame,
        d: &NetworkDelta,
        now: u64,
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
        let selected = if let Some(active) = d
            .multicast_active
            .as_ref()
            .or(self.multicast_active.as_ref())
        {
            active.candidates(
                bridge,
                vid,
                ip.source,
                ip.group,
                &eligible,
                &ingress_output,
                &static_view.members,
                static_view.known,
                now,
            )
        } else {
            host.protocol.multicast_candidates_with_static(
                bridge,
                vid,
                ip.source,
                ip.group,
                &eligible,
                &ingress_output,
                &static_view,
            )
        };
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
        let tx=host.tx.values().map(|t|json!({"packet_id":t.intent.packet_id,"port":t.intent.port,"mac_bytes":t.intent.bytes.len().to_string(),"sof_ps":opt(t.sof),"eof_ps":opt(t.eof),"release_ps":opt(t.release),"arrival_ps":opt(t.arrival),"planned_eof_ps":opt(t.planned_eof),"planned_release_ps":opt(t.planned_release),"planned_arrival_ps":opt(t.planned_arrival),"busy_ps":t.sof.map_or(0,|s|h.min(t.release.or(t.planned_release).unwrap_or(h)).saturating_sub(s)).to_string(),"dropped":t.dropped})).collect::<Vec<_>>();
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
                gateway_injection: None,
                terminal_indications: Vec::new(),
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
            state.arrive(0, None, &mut d, &mut c).unwrap();
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

crate::runtime::network::checkpoint::fields!(WireHost, value; boundaries, queues, active, tx, generations, pending_records, phase; timers);
impl WireHost {
    pub(super) fn checkpoint(&self, component_wire: bool) -> Value {
        let mut host = self.checkpoint_fields();
        if component_wire {
            host["logical"] = json!(self.logical);
        }
        json!({"host":host,"protocol":self.protocol.checkpoint_fields()})
    }
    pub(super) fn restore_checkpoint(
        value: &Value,
        state: Self,
        horizon: u64,
        component_wire: bool,
    ) -> Result<Self> {
        let protocol = crate::runtime::wire::WireState::restore_fields(
            &value["protocol"],
            state.protocol.clone(),
        )?;
        let mut host_value = value["host"].clone();
        let logical = if component_wire {
            let object = host_value
                .as_object_mut()
                .ok_or_else(|| error("wire checkpoint host object missing"))?;
            let saved = object
                .remove("logical")
                .ok_or_else(|| error("wire checkpoint logical ledger missing"))?;
            serde_json::from_value(saved)
                .map_err(|e| error(format!("wire checkpoint logical ledger: {e}")))?
        } else {
            BTreeMap::new()
        };
        let mut state = Self::restore_fields(&host_value, state)?;
        state.protocol = protocol;
        state.logical = logical;
        state.protocol.validate_checkpoint(horizon)?;
        if state.phase > 1 || !state.pending_records.is_empty() {
            return Err(error("nonquiescent wire checkpoint"));
        }
        if state.queues.len() != state.active.len() {
            return Err(error("wire queue dimensions mismatch"));
        }
        for (i, queue) in state.queues.iter().enumerate() {
            for id in queue.iter().chain(state.active[i].iter()) {
                let tx = state
                    .tx
                    .get(id)
                    .ok_or_else(|| error("unknown queued wire transmission"))?;
                if !tx.is_mac_control()
                    && state
                        .protocol
                        .checkpoint_adapter(&tx.intent.adapter_id)
                        .is_none()
                {
                    return Err(error("wire queue owner mismatch"));
                }
            }
        }
        for (id, tx) in &state.tx {
            if tx.is_mac_control() {
                tx.validate_transport(state.protocol.checkpoint_prepared())?;
                if tx.intent.packet_id != *id {
                    return Err(error("MAC Control checkpoint identity"));
                }
                continue;
            }
            let config = state
                .protocol
                .checkpoint_adapter(&tx.intent.adapter_id)
                .ok_or_else(|| error("wire checkpoint TX adapter"))?;
            if tx.intent.packet_id != *id
                || tx.intent.protocol != config.protocol
                || !config.ports.contains(&tx.intent.port)
                || tx.sof.is_some_and(|t| t > horizon)
                || tx.eof.is_some_and(|t| t > horizon)
                || tx.release.is_some_and(|t| t > horizon)
                || tx.arrival.is_some_and(|t| t > horizon)
                || tx.arrival_retired_ps.is_some_and(|t| {
                    t > horizon || !tx.dropped || tx.eof.is_none() || tx.planned_arrival != Some(t)
                })
            {
                return Err(error("wire checkpoint TX identity/owner/time"));
            }
            tx.validate_transport(state.protocol.checkpoint_prepared())?;
        }
        for (id, work) in &state.logical {
            let config = state
                .protocol
                .checkpoint_adapter(&work.adapter_id)
                .ok_or_else(|| error("wire checkpoint logical owner missing"))?;
            let binding = config
                .component_binding
                .as_ref()
                .ok_or_else(|| error("wire checkpoint logical component missing"))?;
            if id != &work.packet_id
                || work.at_ps != horizon
                || binding.component != work.source_component
                || !matches!(binding.ports.get(&work.source_port),
                    Some(PortAttachment::Internal { peer, .. })
                        if peer.component == work.peer_component && peer.port == work.peer_port)
            {
                return Err(error("wire checkpoint logical identity/attachment"));
            }
            let envelope =
                crate::runtime::wire::component_envelope::decode(&work.bytes, config.max_pdu_bytes)
                    .map_err(|e| error(e.0))?;
            let relay = binding.domain == crate::types::wire::TreeDomain::Provider
                && envelope.domain == crate::types::wire::TreeDomain::Customer;
            if work.relayed != relay
                || relay
                    && envelope
                        .tags
                        .iter()
                        .any(|tag| tag.tpid == crate::types::ethernet::EthernetTag::S_TPID)
                || !relay && (envelope.domain != binding.domain || !envelope.tags.is_empty())
            {
                return Err(error("wire checkpoint logical transport/domain mismatch"));
            }
        }
        for ((adapter, key), (generation, deadline)) in &state.timers {
            if *deadline < horizon
                || !state
                    .protocol
                    .checkpoint_timer(adapter, key, *generation, *deadline)
            {
                return Err(error("wire host/protocol timer mismatch"));
            }
        }
        Ok(state)
    }
}
