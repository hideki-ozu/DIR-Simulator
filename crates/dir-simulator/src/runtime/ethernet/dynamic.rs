//! Private, staged policy adapter. No event queue or run loop is owned here.
use crate::{
    snapshot::ethernet::dynamic::{DynamicControlRecord, DynamicPolicyRecord},
    types::{
        Diagnostic,
        ethernet::{EthernetWireFrame, PreparedEthernet, dynamic::*},
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::execution(message).with_reason("dynamic_policy")
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(bound(
    serialize = "K: serde::Serialize, T: serde::Serialize",
    deserialize = "K: serde::Deserialize<'de> + Ord, T: serde::Deserialize<'de>"
))]
#[serde(deny_unknown_fields)]
struct Partition<K: Ord, T> {
    #[serde(with = "crate::runtime::network::checkpoint::map")]
    entries: BTreeMap<K, Lease<T>>,
    #[serde(with = "crate::runtime::network::checkpoint::map")]
    generations: BTreeMap<K, u64>,
    deadlines: BTreeMap<u64, BTreeSet<K>>,
}
impl<K: Ord, T> Default for Partition<K, T> {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            generations: BTreeMap::new(),
            deadlines: BTreeMap::new(),
        }
    }
}
impl<K: Ord + Clone + Serialize, T: Clone + Serialize> Partition<K, T> {
    fn set(
        &mut self,
        key: K,
        value: T,
        expires_at: u64,
        table: &str,
        changes: &mut Vec<Value>,
    ) -> Result<u64> {
        let generation = self
            .generations
            .get(&key)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| error("timer generation overflow"))?;
        self.generations.insert(key.clone(), generation);
        let after = Lease {
            expires_at,
            generation,
            value,
        };
        let before = self.entries.insert(key.clone(), after.clone());
        if let Some(old) = &before {
            self.cancel_deadline(&key, old.expires_at);
        }
        self.deadlines
            .entry(expires_at)
            .or_default()
            .insert(key.clone());
        changes.push(json!({"table":table,"key":key,"before":before,"after":after}));
        Ok(generation)
    }
    fn cancel_deadline(&mut self, key: &K, at: u64) {
        if let Some(bucket) = self.deadlines.get_mut(&at) {
            bucket.remove(key);
            if bucket.is_empty() {
                self.deadlines.remove(&at);
            }
        }
    }
    fn due(&self, now: u64) -> Vec<K> {
        self.deadlines
            .range(..=now)
            .flat_map(|(_, keys)| keys.iter().cloned())
            .collect()
    }
    fn remove(&mut self, key: &K, table: &str, changes: &mut Vec<Value>) -> Option<Lease<T>> {
        let before = self.entries.remove(key);
        if let Some(ref value) = before {
            self.cancel_deadline(key, value.expires_at);
            changes.push(json!({"table":table,"key":key,"before":value,"after":null}));
        }
        before
    }
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Topology {
    link_up: BTreeMap<String, bool>,
    roles: BTreeMap<String, PortRole>,
    generation: u64,
    converge_at: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum WireProjectionKey {
    Membership(crate::runtime::wire::membership::Key),
    Router(crate::runtime::wire::membership::Key),
    Compatibility(String, u16, bool),
}
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WireProjection {
    policies: BTreeMap<String, crate::snapshot::wire::WirePortPolicy>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    tree_policies: BTreeMap<String, BTreeMap<u16, crate::snapshot::wire::WirePortPolicy>>,
    registered: BTreeSet<(String, u16)>,
    #[serde(with = "crate::runtime::network::checkpoint::map")]
    memberships:
        BTreeMap<crate::runtime::wire::membership::Key, crate::runtime::wire::membership::Filter>,
    routers: BTreeSet<crate::runtime::wire::membership::Key>,
    flood: BTreeSet<(String, u16, bool)>,
    #[serde(with = "crate::runtime::network::checkpoint::map")]
    generations: BTreeMap<WireProjectionKey, u64>,
}
#[derive(Debug)]
struct MstpPortBinding {
    adapter: String,
    vlan_map: Arc<Vec<u16>>,
    instances: BTreeSet<u16>,
}
#[derive(Debug)]
pub(crate) struct DynamicState {
    prepared: Arc<PreparedDynamicEthernet>,
    base: Arc<PreparedEthernet>,
    mac: Partition<MacKey, String>,
    membership: Partition<MembershipKey, SourceFilter>,
    routers: Partition<RouterKey, bool>,
    registrations: Partition<RegistrationKey, bool>,
    topology: Topology,
    epoch: u64,
    control_cursor: usize,
    audit_cursor: u64,
    wire_enabled: bool,
    wire_tree_bridges: BTreeSet<String>,
    wire_registration_ports: BTreeSet<String>,
    // Rebound from immutable prepared inputs, never restored from a checkpoint.
    wire_mstp_ports: BTreeMap<String, MstpPortBinding>,
    wire: WireProjection,
}
/// Changed partitions are fully built before commit and swapped without allocation.
#[derive(Debug)]
pub(crate) struct DynamicDelta {
    wire: Option<WireProjection>,
    mac: Option<Partition<MacKey, String>>,
    membership: Option<Partition<MembershipKey, SourceFilter>>,
    routers: Option<Partition<RouterKey, bool>>,
    registrations: Option<Partition<RegistrationKey, bool>>,
    topology: Option<Topology>,
    epoch: u64,
    control_cursor: usize,
    audit_cursor: u64,
    pub control_records: Vec<DynamicControlRecord>,
    pub policy_record: Option<DynamicPolicyRecord>,
    pub next_deadline: Option<u64>,
    pub dirty_ports: BTreeSet<String>,
}
impl DynamicState {
    pub(crate) fn new(prepared: &PreparedDynamicEthernet, base: &PreparedEthernet) -> Result<Self> {
        Self::new_with_wire(prepared, base, None)
    }
    pub(crate) fn new_with_wire(
        prepared: &PreparedDynamicEthernet,
        base: &PreparedEthernet,
        wire: Option<&crate::types::wire::PreparedWire>,
    ) -> Result<Self> {
        use crate::types::wire::{WireOptions, WireProtocol};
        let wire_tree_bridges: BTreeSet<_> = wire
            .into_iter()
            .flat_map(|w| &w.adapters)
            .filter(|a| matches!(a.protocol, WireProtocol::Rstp | WireProtocol::Mstp))
            .map(|a| a.bridge.clone())
            .collect();
        let wire_registration_ports = wire
            .into_iter()
            .flat_map(|w| &w.adapters)
            .filter(|a| a.protocol == WireProtocol::Mvrp)
            .flat_map(|a| a.ports.clone())
            .collect();
        let mut projection = WireProjection::default();
        let mut wire_mstp_ports = BTreeMap::new();
        for adapter in wire
            .into_iter()
            .flat_map(|w| &w.adapters)
            .filter(|a| matches!(a.protocol, WireProtocol::Rstp | WireProtocol::Mstp))
        {
            if let WireOptions::Mstp(config) = &adapter.options {
                let vlan_map = Arc::new(config.vlan_map.clone());
                let instances = std::iter::once(0)
                    .chain(config.instances.iter().map(|i| i.mstid))
                    .collect::<BTreeSet<_>>();
                for port in &adapter.ports {
                    wire_mstp_ports.insert(
                        port.clone(),
                        MstpPortBinding {
                            adapter: adapter.id.clone(),
                            vlan_map: vlan_map.clone(),
                            instances: instances.clone(),
                        },
                    );
                }
            }
            for port in &adapter.ports {
                projection.policies.insert(
                    port.clone(),
                    crate::snapshot::wire::WirePortPolicy {
                        role: "disabled".into(),
                        learning: false,
                        forwarding: false,
                        link_up: true,
                    },
                );
            }
        }
        let up = prepared
            .links
            .iter()
            .map(|l| (l.id.clone(), l.up))
            .collect();
        let mut abstract_prepared = prepared.clone();
        abstract_prepared
            .bridges
            .retain(|bridge, _| !wire_tree_bridges.contains(bridge));
        let roles = tree(&abstract_prepared, base, &up)?;
        Ok(Self {
            prepared: Arc::new(prepared.clone()),
            base: Arc::new(base.clone()),
            mac: Partition::default(),
            membership: Partition::default(),
            routers: Partition::default(),
            registrations: Partition::default(),
            topology: Topology {
                link_up: up,
                roles,
                generation: 0,
                converge_at: None,
            },
            epoch: 0,
            control_cursor: 0,
            audit_cursor: 0,
            wire_enabled: wire.is_some(),
            wire_tree_bridges,
            wire_registration_ports,
            wire_mstp_ports,
            wire: projection,
        })
    }
    fn delta(&self) -> DynamicDelta {
        DynamicDelta {
            wire: None,
            mac: None,
            membership: None,
            routers: None,
            registrations: None,
            topology: None,
            epoch: self.epoch,
            control_cursor: self.control_cursor,
            audit_cursor: self.audit_cursor,
            control_records: Vec::new(),
            policy_record: None,
            next_deadline: None,
            dirty_ports: BTreeSet::new(),
        }
    }
    pub(crate) fn policy_epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn next_deadline(&self) -> Option<u64> {
        self.deadline(&self.delta())
    }
    fn deadline(&self, d: &DynamicDelta) -> Option<u64> {
        let mut times = Vec::new();
        if let Some(c) = self.prepared.controls.get(d.control_cursor) {
            times.push(c.at_ps);
        }
        if let Some(t) = d.topology.as_ref().unwrap_or(&self.topology).converge_at {
            times.push(t);
        }
        times.extend(
            d.mac
                .as_ref()
                .unwrap_or(&self.mac)
                .deadlines
                .first_key_value()
                .map(|(at, _)| *at),
        );
        times.extend(
            d.membership
                .as_ref()
                .unwrap_or(&self.membership)
                .deadlines
                .first_key_value()
                .map(|(at, _)| *at),
        );
        times.extend(
            d.routers
                .as_ref()
                .unwrap_or(&self.routers)
                .deadlines
                .first_key_value()
                .map(|(at, _)| *at),
        );
        times.extend(
            d.registrations
                .as_ref()
                .unwrap_or(&self.registrations)
                .deadlines
                .first_key_value()
                .map(|(at, _)| *at),
        );
        times.into_iter().min()
    }
    fn policy_for(&self, d: &DynamicDelta) -> PolicySnapshot {
        let topology = d.topology.as_ref().unwrap_or(&self.topology);
        let mut link_up = BTreeMap::new();
        for l in &self.prepared.links {
            for p in &l.ports {
                link_up.insert(p.clone(), topology.link_up[&l.id]);
            }
        }
        let mut effective_vlans: BTreeMap<_, _> = self
            .base
            .port_policies
            .iter()
            .map(|p| (p.port.clone(), p.vlans.clone()))
            .collect();
        let wire = d.wire.as_ref().unwrap_or(&self.wire);
        for (port, vid) in &wire.registered {
            effective_vlans
                .entry(port.clone())
                .or_default()
                .insert(*vid, true);
        }
        for key in d
            .registrations
            .as_ref()
            .unwrap_or(&self.registrations)
            .entries
            .keys()
        {
            if self.wire_registration_ports.contains(&key.port) {
                continue;
            }
            effective_vlans
                .entry(key.port.clone())
                .or_default()
                .insert(key.vid, true);
        }
        let mut roles = topology.roles.clone();
        for (port, policy) in &wire.policies {
            let role = if !policy.link_up {
                PortRole::Disabled
            } else if !policy.forwarding {
                PortRole::Alternate
            } else if policy.role.eq_ignore_ascii_case("root") {
                PortRole::Root
            } else {
                PortRole::Designated
            };
            roles.insert(port.clone(), role);
        }
        PolicySnapshot {
            policy_epoch: d.epoch,
            topology_generation: topology.generation,
            roles,
            link_up,
            tree_roles: self
                .wire_mstp_ports
                .iter()
                .map(|(port, binding)| {
                    let roles = effective_vlans
                        .get(port)
                        .into_iter()
                        .flat_map(|v| v.keys())
                        .map(|vid| {
                            let policy = wire
                                .tree_policies
                                .get(port)
                                .and_then(|p| {
                                    binding.vlan_map.get(*vid as usize).and_then(|id| p.get(id))
                                })
                                .or_else(|| wire.policies.get(port));
                            let role = match policy {
                                Some(p) if p.link_up && p.forwarding => {
                                    if p.role.eq_ignore_ascii_case("root") {
                                        PortRole::Root
                                    } else {
                                        PortRole::Designated
                                    }
                                }
                                Some(p) if p.link_up => PortRole::Alternate,
                                _ => PortRole::Disabled,
                            };
                            (*vid, role)
                        })
                        .collect();
                    (port.clone(), roles)
                })
                .collect(),
            effective_vlans,
        }
    }
    pub(crate) fn snapshot_policy_after(&self, d: &DynamicDelta) -> PolicySnapshot {
        self.policy_for(d)
    }
    pub(crate) fn snapshot_policy(&self) -> PolicySnapshot {
        self.policy_for(&self.delta())
    }
    pub(crate) fn eligible_egress(
        &self,
        port: &str,
        vid: u16,
        snapshot: &PolicySnapshot,
    ) -> std::result::Result<(), Ineligible> {
        eligible_egress(port, vid, snapshot)
    }
    pub(crate) fn initial_record(&self) -> DynamicPolicyRecord {
        DynamicPolicyRecord {
            time_ps: 0,
            policy_epoch: 0,
            topology_generation: 0,
            initial: true,
            changes: vec![
                json!({"table":"policy","key":null,"before":null,"after":self.snapshot_policy()}),
            ],
        }
    }
    pub(crate) fn snapshot_tables(&self) -> Value {
        let mut snapshot = json!({"policy":self.snapshot_policy(),"mac":rows(&self.mac),"membership":rows(&self.membership),"router":rows(&self.routers),"registration":rows(&self.registrations),"next_deadline":self.next_deadline(),"converge_at":self.topology.converge_at});
        if self.wire_enabled {
            snapshot["wire_membership"]=json!(self.wire.memberships.iter().map(|(key,filter)|json!({"key":key,"filter":{"mode":if filter.exclude{"exclude"}else{"include"},"sources":filter.sources}})).collect::<Vec<_>>());
            snapshot["wire_router"] = json!(self.wire.routers);
            snapshot["wire_compatibility_flood"] = json!(self.wire.flood);
        }
        snapshot
    }

    fn finish(
        &self,
        mut d: DynamicDelta,
        now: u64,
        mut changes: Vec<Value>,
    ) -> Result<DynamicDelta> {
        let timer_count = d
            .mac
            .as_ref()
            .unwrap_or(&self.mac)
            .entries
            .len()
            .checked_add(
                d.membership
                    .as_ref()
                    .unwrap_or(&self.membership)
                    .entries
                    .len(),
            )
            .and_then(|n| n.checked_add(d.routers.as_ref().unwrap_or(&self.routers).entries.len()))
            .and_then(|n| {
                n.checked_add(
                    d.registrations
                        .as_ref()
                        .unwrap_or(&self.registrations)
                        .entries
                        .len(),
                )
            })
            .and_then(|n| {
                n.checked_add(usize::from(
                    d.topology
                        .as_ref()
                        .unwrap_or(&self.topology)
                        .converge_at
                        .is_some(),
                ))
            })
            .ok_or_else(|| error("timer capacity arithmetic overflow"))?;
        if timer_count > self.prepared.limits.pending_timers {
            return Err(error("pending_timers capacity exceeded"));
        }
        if !changes.is_empty() {
            d.epoch = self
                .epoch
                .checked_add(1)
                .ok_or_else(|| error("policy epoch overflow"))?;
            if d.topology.is_some() || d.registrations.is_some() {
                changes.push(json!({"table":"policy","key":null,"before":self.snapshot_policy(),"after":self.policy_for(&d)}));
            }
            d.policy_record = Some(DynamicPolicyRecord {
                time_ps: now,
                policy_epoch: d.epoch,
                topology_generation: d.topology.as_ref().unwrap_or(&self.topology).generation,
                initial: false,
                changes,
            });
        }
        for (i, r) in d.control_records.iter_mut().enumerate() {
            if r.synthetic {
                r.control_id = format!("@{}/{}", r.kind, d.audit_cursor);
                d.audit_cursor = d
                    .audit_cursor
                    .checked_add(1)
                    .ok_or_else(|| error("audit ID overflow"))?;
            }
            r.batch_ordinal = i as u64;
            r.epoch_before = self.epoch;
            r.epoch_after = d.epoch;
        }
        d.next_deadline = self.deadline(&d);
        Ok(d)
    }
    /// A whole same-time batch is staged, including controls followed by indexed lease expiry.
    pub(crate) fn plan_controls(&self, now: u64) -> Result<DynamicDelta> {
        let mut d = self.delta();
        let mut changes = Vec::new();
        let mut topology_changed = false;
        while let Some(c) = self.prepared.controls.get(d.control_cursor) {
            if c.at_ps != now {
                break;
            }
            d.control_cursor += 1;
            let before: Value;
            let mut after = Value::Null;
            let mut generation = None;
            let mut changed = false;
            let key: Value;
            match &c.op {
                ControlOp::LinkSet { link, up } => {
                    key = json!({"link":link});
                    let t = d.topology.get_or_insert_with(|| self.topology.clone());
                    before = json!(t.link_up[link]);
                    if t.link_up[link] != *up {
                        t.link_up.insert(link.clone(), *up);
                        changed = true;
                        topology_changed = true;
                        changes.push(json!({"table":"link","key":key,"before":before,"after":up}));
                    }
                    after = json!(up);
                }
                ControlOp::MembershipSet {
                    key: k,
                    mode,
                    sources,
                    expires_at,
                } => {
                    key = json!(k);
                    let t = d.membership.get_or_insert_with(|| self.membership.clone());
                    before = json!(t.entries.get(k));
                    if !t.entries.contains_key(k)
                        && t.entries.len()
                            + d.routers.as_ref().unwrap_or(&self.routers).entries.len()
                            >= self.prepared.limits.membership_entries
                    {
                        return Err(error("membership_entries capacity exceeded"));
                    }
                    generation = Some(t.set(
                        k.clone(),
                        SourceFilter {
                            mode: mode.clone(),
                            sources: sources.clone(),
                        },
                        *expires_at,
                        "membership",
                        &mut changes,
                    )?);
                    after = json!(t.entries.get(k));
                    changed = true;
                }
                ControlOp::MembershipLeave { key: k } => {
                    key = json!(k);
                    let t = d.membership.get_or_insert_with(|| self.membership.clone());
                    before = json!(t.entries.get(k));
                    generation = t.entries.get(k).map(|v| v.generation);
                    changed = t.remove(k, "membership", &mut changes).is_some();
                }
                ControlOp::RouterSet { key: k, expires_at } => {
                    key = json!(k);
                    let t = d.routers.get_or_insert_with(|| self.routers.clone());
                    before = json!(t.entries.get(k));
                    if !t.entries.contains_key(k)
                        && t.entries.len()
                            + d.membership
                                .as_ref()
                                .unwrap_or(&self.membership)
                                .entries
                                .len()
                            >= self.prepared.limits.membership_entries
                    {
                        return Err(error("membership_entries capacity exceeded"));
                    }
                    generation =
                        Some(t.set(k.clone(), true, *expires_at, "router", &mut changes)?);
                    after = json!(t.entries.get(k));
                    changed = true;
                }
                ControlOp::RouterLeave { key: k } => {
                    key = json!(k);
                    let t = d.routers.get_or_insert_with(|| self.routers.clone());
                    before = json!(t.entries.get(k));
                    generation = t.entries.get(k).map(|v| v.generation);
                    changed = t.remove(k, "router", &mut changes).is_some();
                }
                ControlOp::VlanRegister { key: k, expires_at } => {
                    key = json!(k);
                    let t = d
                        .registrations
                        .get_or_insert_with(|| self.registrations.clone());
                    before = json!(t.entries.get(k));
                    if !t.entries.contains_key(k)
                        && t.entries.len() >= self.prepared.limits.registrations
                    {
                        return Err(error("registrations capacity exceeded"));
                    }
                    generation =
                        Some(t.set(k.clone(), true, *expires_at, "registration", &mut changes)?);
                    after = json!(t.entries.get(k));
                    changed = true;
                    d.dirty_ports.insert(k.port.clone());
                }
                ControlOp::VlanUnregister { key: k } => {
                    key = json!(k);
                    let t = d
                        .registrations
                        .get_or_insert_with(|| self.registrations.clone());
                    before = json!(t.entries.get(k));
                    generation = t.entries.get(k).map(|v| v.generation);
                    changed = t.remove(k, "registration", &mut changes).is_some();
                    if changed {
                        d.dirty_ports.insert(k.port.clone());
                    }
                }
            }
            d.control_records.push(DynamicControlRecord {
                synthetic: false,
                control_id: c.id.clone(),
                kind: c.op.kind().into(),
                scheduled_ps: c.at_ps,
                applied_ps: Some(now),
                batch_ordinal: 0,
                epoch_before: 0,
                epoch_after: 0,
                generation,
                outcome: if changed { "changed" } else { "no_op" }.into(),
                key,
                before,
                after,
            });
        }
        if topology_changed {
            let t = d.topology.as_mut().unwrap();
            t.generation = t
                .generation
                .checked_add(1)
                .ok_or_else(|| error("topology generation overflow"))?;
            let abstract_changed = d.control_records.iter().any(|record| {
                if record.kind != "link_set" || record.outcome != "changed" {
                    return false;
                }
                record
                    .key
                    .get("link")
                    .and_then(Value::as_str)
                    .and_then(|id| self.prepared.links.iter().find(|l| l.id == id))
                    .is_some_and(|link| {
                        link.ports
                            .iter()
                            .all(|p| !self.wire_tree_bridges.contains(owner(&self.base, p)))
                    })
            });
            t.converge_at = abstract_changed
                .then(|| {
                    now.checked_add(self.prepared.convergence_ps)
                        .ok_or_else(|| error("convergence overflow"))
                })
                .transpose()?;
            t.roles = closed_roles(&self.prepared, &self.base, &t.link_up);
            let m = d.mac.get_or_insert_with(|| self.mac.clone());
            let keys: Vec<_> = m
                .entries
                .keys()
                .filter(|key| !self.wire_tree_bridges.contains(&key.switch))
                .cloned()
                .collect();
            for key in keys {
                let old = m.remove(&key, "mac", &mut changes);
                audit(
                    &mut d.control_records,
                    now,
                    "mac_flush",
                    json!(key),
                    old,
                    Option::<Lease<String>>::None,
                );
            }
            d.dirty_ports
                .extend(self.base.port_policies.iter().map(|p| p.port.clone()));
            changes.push(json!({"table":"topology","key":null,"before":self.topology.generation,"after":t.generation}));
        }
        // Updated entries carry their new expiry, so superseded expiries never enter this batch.
        expire(
            &self.mac,
            &mut d.mac,
            now,
            "mac",
            &mut changes,
            &mut d.control_records,
        );
        expire(
            &self.membership,
            &mut d.membership,
            now,
            "membership",
            &mut changes,
            &mut d.control_records,
        );
        expire(
            &self.routers,
            &mut d.routers,
            now,
            "router",
            &mut changes,
            &mut d.control_records,
        );
        let expiring: Vec<_> = d
            .registrations
            .as_ref()
            .unwrap_or(&self.registrations)
            .entries
            .iter()
            .filter(|(_, v)| v.expires_at <= now)
            .map(|(k, _)| k.port.clone())
            .collect();
        d.dirty_ports.extend(expiring);
        expire(
            &self.registrations,
            &mut d.registrations,
            now,
            "registration",
            &mut changes,
            &mut d.control_records,
        );
        if d.topology
            .as_ref()
            .unwrap_or(&self.topology)
            .converge_at
            .is_some_and(|at| at <= now)
        {
            let t = d.topology.get_or_insert_with(|| self.topology.clone());
            let before = json!(t.roles);
            let mut prepared = (*self.prepared).clone();
            prepared
                .bridges
                .retain(|bridge, _| !self.wire_tree_bridges.contains(bridge));
            t.roles = tree(&prepared, &self.base, &t.link_up)?;
            t.converge_at = None;
            changes.push(json!({"table":"roles","key":null,"before":before,"after":t.roles}));
            audit(
                &mut d.control_records,
                now,
                "tree_publish",
                Value::Null,
                Some(t.generation),
                Some(t.generation),
            );
            d.dirty_ports
                .extend(self.base.port_policies.iter().map(|p| p.port.clone()));
        }
        self.finish(d, now, changes)
    }
    pub(crate) fn plan_learning(
        &self,
        ingress: &str,
        vid: u16,
        src_mac: &str,
        now: u64,
    ) -> Result<DynamicDelta> {
        let mut d = self.delta();
        let mut changes = Vec::new();
        let p = self
            .base
            .port_policies
            .iter()
            .find(|p| p.ingress == ingress || p.port == ingress)
            .ok_or_else(|| error("unknown learning ingress"))?;
        let device = &self.base.devices[p.device];
        if self
            .wire_policy_for_vid(&self.wire, &p.port, vid)
            .is_some_and(|policy| !policy.learning || !policy.link_up)
        {
            return self.finish(d, now, changes);
        }
        if device.kind != "switch" {
            return self.finish(d, now, changes);
        }
        let key = MacKey {
            switch: device.id.clone(),
            vid,
            mac: src_mac.into(),
        };
        let mut reason = None;
        if device.vlan_fdb.contains_key(&(vid, src_mac.into())) || device.fdb.contains_key(src_mac)
        {
            reason = Some("static_shadowed");
        } else if !self.prepared.mac_keys.contains(&key) {
            return Err(error("source MAC is outside prepared generation ledger"));
        } else if !self.mac.entries.contains_key(&key)
            && self.mac.entries.len() >= self.prepared.limits.mac_entries
        {
            reason = Some("mac_capacity");
        }
        if let Some(reason) = reason {
            audit(
                &mut d.control_records,
                now,
                reason,
                json!(key),
                Option::<Lease<String>>::None,
                Option::<Lease<String>>::None,
            );
        } else {
            let expires_at = now
                .checked_add(self.prepared.mac_age_ps)
                .ok_or_else(|| error("MAC age overflow"))?;
            let m = d.mac.get_or_insert_with(|| self.mac.clone());
            let before = m.entries.get(&key).cloned();
            let kind = match &before {
                None => "mac_learn",
                Some(v) if v.value == p.port => "mac_refresh",
                _ => "mac_move",
            };
            m.set(key.clone(), p.port.clone(), expires_at, "mac", &mut changes)?;
            audit(
                &mut d.control_records,
                now,
                kind,
                json!(key.clone()),
                before,
                m.entries.get(&key).cloned(),
            );
        }
        self.finish(d, now, changes)
    }
    pub(crate) fn apply(&mut self, d: DynamicDelta) {
        if let Some(v) = d.wire {
            self.wire = v;
        }
        if let Some(v) = d.mac {
            self.mac = v;
        }
        if let Some(v) = d.membership {
            self.membership = v;
        }
        if let Some(v) = d.routers {
            self.routers = v;
        }
        if let Some(v) = d.registrations {
            self.registrations = v;
        }
        if let Some(v) = d.topology {
            self.topology = v;
        }
        self.epoch = d.epoch;
        self.control_cursor = d.control_cursor;
        self.audit_cursor = d.audit_cursor;
    }
    /// Semantic reservations, including coalesced deadlines, rather than coordinator token count.
    pub(crate) fn semantic_pending_after(&self, delta: Option<&DynamicDelta>) -> usize {
        let current = self.delta();
        let d = delta.unwrap_or(&current);
        self.prepared
            .controls
            .len()
            .saturating_sub(d.control_cursor)
            .saturating_add(d.mac.as_ref().unwrap_or(&self.mac).entries.len())
            .saturating_add(
                d.membership
                    .as_ref()
                    .unwrap_or(&self.membership)
                    .entries
                    .len(),
            )
            .saturating_add(d.routers.as_ref().unwrap_or(&self.routers).entries.len())
            .saturating_add(
                d.registrations
                    .as_ref()
                    .unwrap_or(&self.registrations)
                    .entries
                    .len(),
            )
            .saturating_add(usize::from(
                d.topology
                    .as_ref()
                    .unwrap_or(&self.topology)
                    .converge_at
                    .is_some(),
            ))
    }
    pub(crate) fn wire_ingress_state(
        &self,
        ingress: &str,
        delta: Option<&DynamicDelta>,
    ) -> Option<(bool, bool)> {
        let port = self
            .base
            .port_policies
            .iter()
            .find(|p| p.port == ingress || p.ingress == ingress)?;
        let wire = delta.and_then(|d| d.wire.as_ref()).unwrap_or(&self.wire);
        wire.policies
            .get(&port.port)
            .map(|p| (p.learning && p.link_up, p.forwarding && p.link_up))
    }
    fn wire_policy_for_vid<'a>(
        &self,
        wire: &'a WireProjection,
        port: &str,
        vid: u16,
    ) -> Option<&'a crate::snapshot::wire::WirePortPolicy> {
        self.wire_mstp_ports
            .get(port)
            .and_then(|binding| binding.vlan_map.get(vid as usize))
            .and_then(|id| {
                wire.tree_policies
                    .get(port)
                    .and_then(|policies| policies.get(id))
            })
            .or_else(|| wire.policies.get(port))
    }
    pub(crate) fn wire_ingress_state_for_vid(
        &self,
        ingress: &str,
        vid: u16,
        delta: Option<&DynamicDelta>,
    ) -> Option<(bool, bool)> {
        let port = self
            .base
            .port_policies
            .iter()
            .find(|p| p.port == ingress || p.ingress == ingress)?;
        let wire = delta.and_then(|d| d.wire.as_ref()).unwrap_or(&self.wire);
        self.wire_policy_for_vid(wire, &port.port, vid)
            .map(|p| (p.learning && p.link_up, p.forwarding && p.link_up))
    }
    fn project_wire_trees(
        &self,
        policies: &BTreeMap<(String, u16, String), crate::snapshot::wire::WirePortPolicy>,
    ) -> Result<BTreeMap<String, BTreeMap<u16, crate::snapshot::wire::WirePortPolicy>>> {
        if policies.len()
            != self
                .wire_mstp_ports
                .values()
                .map(|p| p.instances.len())
                .sum::<usize>()
        {
            return Err(error("MSTP tree projection catalog mismatch"));
        }
        self.wire_mstp_ports
            .iter()
            .map(|(port, binding)| {
                let trees = binding
                    .instances
                    .iter()
                    .map(|id| {
                        let key = (binding.adapter.clone(), *id, port.clone());
                        let policy = policies
                            .get(&key)
                            .ok_or_else(|| error("MSTP tree projection owner/instance mismatch"))?;
                        if policy.forwarding && !policy.learning {
                            return Err(error("MSTP forwarding without learning"));
                        }
                        Ok((*id, policy.clone()))
                    })
                    .collect::<Result<_>>()?;
                Ok((port.clone(), trees))
            })
            .collect()
    }
    /// Preserve one policy per configured instance, selecting it by the immutable VID map.
    pub(crate) fn plan_wire_trees_in(
        &self,
        mut d: DynamicDelta,
        now: u64,
        policies: &BTreeMap<(String, u16, String), crate::snapshot::wire::WirePortPolicy>,
    ) -> Result<DynamicDelta> {
        let trees = self.project_wire_trees(policies)?;
        let projection = d.wire.get_or_insert_with(|| self.wire.clone());
        if projection.tree_policies != trees {
            let before = std::mem::replace(&mut projection.tree_policies, trees);
            d.dirty_ports.extend(
                before
                    .keys()
                    .chain(projection.tree_policies.keys())
                    .cloned(),
            );
            // plan_wire_in already advances the shared epoch when the wire reducer changed.
            // A direct staged projection still advances once, never once per MSTI.
            if d.epoch == self.epoch {
                d.epoch = self
                    .epoch
                    .checked_add(1)
                    .ok_or_else(|| error("policy epoch overflow"))?;
            }
            let record = d.policy_record.get_or_insert_with(|| DynamicPolicyRecord {
                time_ps: now,
                policy_epoch: d.epoch,
                topology_generation: d.topology.as_ref().unwrap_or(&self.topology).generation,
                initial: false,
                changes: Vec::new(),
            });
            record.policy_epoch = d.epoch;
            record
                .changes
                .push(json!({"table":"wire_tree_projection","key":null,
                "before":before,"after":projection.tree_policies}));
            for record in &mut d.control_records {
                record.epoch_after = d.epoch;
            }
        }
        Ok(d)
    }
    pub(crate) fn validate_wire_tree_projection(
        &self,
        policies: &BTreeMap<(String, u16, String), crate::snapshot::wire::WirePortPolicy>,
        initialized: bool,
    ) -> Result<()> {
        if !initialized {
            if !self.wire.tree_policies.is_empty() {
                return Err(error("MSTP tree projection before initialization"));
            }
        } else if self.wire.tree_policies != self.project_wire_trees(policies)? {
            return Err(error("MSTP tree projection differs from protocol state"));
        }
        Ok(())
    }
    /// Merge protocol projections and scoped removals into the same host callback.
    /// FDB generation ledgers survive removals; matching stale expiry cannot remove relearned entries.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub(crate) fn plan_wire_in(
        &self,
        base: Option<DynamicDelta>,
        now: u64,
        policies: &BTreeMap<String, crate::snapshot::wire::WirePortPolicy>,
        registered: &BTreeSet<(String, u16)>,
        flush: &[crate::runtime::wire::WireFlushIntent],
        policy_changed: bool,
        topology_changed: bool,
    ) -> Result<(
        DynamicDelta,
        Vec<Vec<crate::snapshot::wire::WireMacKey>>,
        Vec<crate::snapshot::wire::WireMacEntry>,
    )> {
        use crate::snapshot::wire::{WireMacEntry, WireMacKey};
        let mut d = base.unwrap_or_else(|| self.delta());
        let mut changes = Vec::new();
        let mut projection = d.wire.as_ref().unwrap_or(&self.wire).clone();
        projection.policies = policies.clone();
        projection.registered = registered.clone();
        let previous = d.wire.as_ref().unwrap_or(&self.wire);
        let changed = previous.policies != projection.policies
            || previous.registered != projection.registered;
        if changed {
            d.dirty_ports.extend(
                previous
                    .policies
                    .keys()
                    .chain(projection.policies.keys())
                    .cloned(),
            );
            d.dirty_ports.extend(
                previous
                    .registered
                    .symmetric_difference(&projection.registered)
                    .map(|(port, _)| port.clone()),
            );
            changes.push(json!({"table":"wire_projection","key":null,"before":{"ports":previous.policies,"registered":previous.registered},"after":{"ports":projection.policies,"registered":projection.registered}}));
        }
        d.wire = Some(projection);
        if topology_changed {
            let topology = d.topology.get_or_insert_with(|| self.topology.clone());
            if topology.generation == self.topology.generation {
                topology.generation = topology
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| error("topology generation overflow"))?;
            }
        }
        let mut flushed = Vec::with_capacity(flush.len());
        for intent in flush {
            let table = d.mac.get_or_insert_with(|| self.mac.clone());
            let keys: Vec<_> = table
                .entries
                .iter()
                .filter(|(key, lease)| lease.value == intent.port && intent.vids.contains(&key.vid))
                .map(|(key, _)| key.clone())
                .collect();
            let mut completed = Vec::with_capacity(keys.len());
            for key in keys {
                table.remove(&key, "mac", &mut changes);
                completed.push(WireMacKey {
                    switch: key.switch,
                    vid: key.vid,
                    mac: key.mac,
                });
            }
            flushed.push(completed);
        }
        if changed || policy_changed || topology_changed || !changes.is_empty() {
            d.epoch = self
                .epoch
                .checked_add(1)
                .ok_or_else(|| error("policy epoch overflow"))?;
            let mut record = d
                .policy_record
                .take()
                .unwrap_or_else(|| DynamicPolicyRecord {
                    time_ps: now,
                    policy_epoch: d.epoch,
                    topology_generation: 0,
                    initial: false,
                    changes: Vec::new(),
                });
            record.policy_epoch = d.epoch;
            record.topology_generation = d.topology.as_ref().unwrap_or(&self.topology).generation;
            record.changes.extend(changes);
            d.policy_record = Some(record);
            for record in &mut d.control_records {
                record.epoch_after = d.epoch;
            }
        }
        d.next_deadline = self.deadline(&d);
        let mac = d
            .mac
            .as_ref()
            .unwrap_or(&self.mac)
            .entries
            .iter()
            .map(|(key, lease)| WireMacEntry {
                switch: key.switch.clone(),
                vid: key.vid,
                mac: key.mac.clone(),
                port: lease.value.clone(),
                expires_at_ps: lease.expires_at.to_string(),
                generation: lease.generation.to_string(),
            })
            .collect();
        Ok((d, flushed, mac))
    }
    /// Store only effective source-filter projections. WireState retains every lease and expiry.
    pub(crate) fn plan_wire_membership_in(
        &self,
        mut d: DynamicDelta,
        now: u64,
        memberships: &BTreeMap<
            crate::runtime::wire::membership::Key,
            crate::runtime::wire::membership::Filter,
        >,
        routers: &BTreeSet<crate::runtime::wire::membership::Key>,
        flood: &BTreeSet<(String, u16, bool)>,
    ) -> Result<DynamicDelta> {
        use crate::runtime::wire::membership::{Filter, Key};
        struct Change {
            ledger: WireProjectionKey,
            kind: &'static str,
            key: Value,
            before: Value,
            after: Value,
            order: (bool, u16, Option<std::net::IpAddr>, String, String),
        }
        let old = d.wire.as_ref().unwrap_or(&self.wire);
        let key_json = |key: &Key, router: bool| json!({"switch":key.bridge,"vid":key.vid,"family":if key.group.is_ipv6(){"ipv6"}else{"ipv4"},"group":if router{None}else{Some(key.group.to_string())},"port":key.port});
        let filter_json = |filter: Option<&Filter>| {
            filter.map_or(Value::Null, |filter| {
                json!(SourceFilter {
                    mode: if filter.exclude {
                        FilterMode::Exclude
                    } else {
                        FilterMode::Include
                    },
                    sources: filter.sources.clone()
                })
            })
        };
        let mut changes = Vec::new();
        let keys: BTreeSet<_> = old
            .memberships
            .keys()
            .chain(memberships.keys())
            .cloned()
            .collect();
        for key in keys {
            let before = old.memberships.get(&key);
            let after = memberships.get(&key);
            if before == after {
                continue;
            }
            changes.push(Change {
                ledger: WireProjectionKey::Membership(key.clone()),
                kind: if after.is_some() {
                    "wire_membership_set"
                } else {
                    "wire_membership_leave"
                },
                key: key_json(&key, false),
                before: filter_json(before),
                after: filter_json(after),
                order: (
                    key.group.is_ipv6(),
                    key.vid,
                    Some(key.group),
                    key.port,
                    key.bridge,
                ),
            });
        }
        for key in old.routers.union(routers) {
            let before = old.routers.contains(key);
            let after = routers.contains(key);
            if before == after {
                continue;
            }
            changes.push(Change {
                ledger: WireProjectionKey::Router(key.clone()),
                kind: if after {
                    "wire_router_set"
                } else {
                    "wire_router_leave"
                },
                key: key_json(key, true),
                before: if before { json!(true) } else { Value::Null },
                after: if after { json!(true) } else { Value::Null },
                order: (
                    key.group.is_ipv6(),
                    key.vid,
                    None,
                    key.port.clone(),
                    key.bridge.clone(),
                ),
            });
        }
        for key in old.flood.union(flood) {
            let before = old.flood.contains(key);
            let after = flood.contains(key);
            if before == after {
                continue;
            }
            changes.push(Change {
                ledger: WireProjectionKey::Compatibility(key.0.clone(), key.1, key.2),
                kind: "wire_compatibility_flood",
                key: json!({"switch":key.0,"vid":key.1,"family":if key.2{"ipv6"}else{"ipv4"}}),
                before: json!(before),
                after: json!(after),
                order: (key.2, key.1, None, String::new(), key.0.clone()),
            });
        }
        if changes.is_empty() {
            return Ok(d);
        }
        changes.sort_by(|a, b| a.order.cmp(&b.order));
        let mut projection = old.clone();
        projection.memberships = memberships.clone();
        projection.routers = routers.clone();
        projection.flood = flood.clone();
        d.epoch = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| error("policy epoch overflow"))?;
        let mut policy_changes = Vec::new();
        for change in changes {
            let generation = projection
                .generations
                .get(&change.ledger)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| error("wire projection generation overflow"))?;
            projection.generations.insert(change.ledger, generation);
            let id = format!("@{}/{}", change.kind, d.audit_cursor);
            d.audit_cursor = d
                .audit_cursor
                .checked_add(1)
                .ok_or_else(|| error("audit ID overflow"))?;
            policy_changes.push(json!({"table":change.kind,"key":change.key,"before":change.before,"after":change.after}));
            d.control_records.push(DynamicControlRecord {
                synthetic: true,
                control_id: id,
                kind: change.kind.into(),
                scheduled_ps: now,
                applied_ps: Some(now),
                batch_ordinal: d.control_records.len() as u64,
                epoch_before: self.epoch,
                epoch_after: d.epoch,
                generation: Some(generation),
                outcome: "changed".into(),
                key: change.key,
                before: change.before,
                after: change.after,
            });
        }
        d.wire = Some(projection);
        for record in &mut d.control_records {
            record.epoch_after = d.epoch;
        }
        let mut record = d
            .policy_record
            .take()
            .unwrap_or_else(|| DynamicPolicyRecord {
                time_ps: now,
                policy_epoch: d.epoch,
                topology_generation: 0,
                initial: false,
                changes: Vec::new(),
            });
        record.policy_epoch = d.epoch;
        record.topology_generation = d.topology.as_ref().unwrap_or(&self.topology).generation;
        record.changes.extend(policy_changes);
        d.policy_record = Some(record);
        Ok(d)
    }
    pub(crate) fn select_egress(
        &self,
        switch: &str,
        ingress: &str,
        vid: u16,
        wire: &EthernetWireFrame,
        ip: Option<&IpMulticast>,
    ) -> EgressSelection {
        self.select_egress_after(&self.delta(), switch, ingress, vid, wire, ip)
    }
    pub(crate) fn select_egress_after(
        &self,
        d: &DynamicDelta,
        switch: &str,
        ingress: &str,
        vid: u16,
        wire: &EthernetWireFrame,
        ip: Option<&IpMulticast>,
    ) -> EgressSelection {
        let Some(device) = self.base.devices.iter().find(|d| d.id == switch) else {
            return EgressSelection {
                ports: vec![],
                reason: Some("unknown_switch".into()),
            };
        };
        let all: Vec<_> = self
            .base
            .port_policies
            .iter()
            .filter(|p| {
                self.base.devices[p.device].id == switch
                    && p.port != ingress
                    && p.ingress != ingress
            })
            .map(|p| p.port.clone())
            .collect();
        let policy = self.snapshot_policy_after(d);
        let multicast = u8::from_str_radix(&wire.dst_mac[0..2], 16).is_ok_and(|v| v & 1 == 1);
        let broadcast = wire.dst_mac == "ff:ff:ff:ff:ff:ff";
        let mut known = false;
        let mut selected = BTreeSet::new();
        if !multicast {
            let dynamic = MacKey {
                switch: switch.into(),
                vid,
                mac: wire.dst_mac.clone(),
            };
            if let Some(port) = device
                .vlan_fdb
                .get(&(vid, wire.dst_mac.clone()))
                .or_else(|| device.fdb.get(&wire.dst_mac))
                .or_else(|| {
                    d.mac
                        .as_ref()
                        .unwrap_or(&self.mac)
                        .entries
                        .get(&dynamic)
                        .map(|v| &v.value)
                })
            {
                known = true;
                selected.insert(port.clone());
            }
        } else if !broadcast {
            if let Some(ports) = device.multicast.get(&(vid, wire.dst_mac.clone())) {
                known = true;
                selected.extend(ports.iter().cloned());
            }
            if let Some(ip) = ip {
                for (k, v) in &d.membership.as_ref().unwrap_or(&self.membership).entries {
                    if k.switch == switch
                        && k.vid == vid
                        && k.family == ip.family
                        && k.group == ip.group
                    {
                        known = true;
                        let has = v.value.sources.contains(&ip.source);
                        if matches!(v.value.mode, FilterMode::Include) && has
                            || matches!(v.value.mode, FilterMode::Exclude) && !has
                        {
                            selected.insert(k.port.clone());
                        }
                    }
                }
                for k in d.routers.as_ref().unwrap_or(&self.routers).entries.keys() {
                    if k.switch == switch && k.vid == vid && k.family == ip.family {
                        known = true;
                        selected.insert(k.port.clone());
                    }
                }
            }
        }
        if !known {
            if multicast && !broadcast && device.unknown_multicast == "drop" {
                return EgressSelection {
                    ports: vec![],
                    reason: Some("unknown_multicast".into()),
                };
            }
            selected.extend(all.iter().cloned());
        }
        let selected: Vec<_> = selected
            .into_iter()
            .filter(|p| all.contains(p) && eligible_egress(p, vid, &policy).is_ok())
            .collect();
        let reason = if known && selected.is_empty() {
            Some(
                if multicast {
                    "multicast_filtered"
                } else {
                    "known_egress_ineligible"
                }
                .into(),
            )
        } else {
            None
        };
        EgressSelection {
            ports: selected,
            reason,
        }
    }
}
pub(crate) fn eligible_egress(
    port: &str,
    vid: u16,
    s: &PolicySnapshot,
) -> std::result::Result<(), Ineligible> {
    if !s.link_up.get(port).copied().unwrap_or(false) {
        Err(Ineligible::LinkDown)
    } else if !s
        .tree_roles
        .get(port)
        .and_then(|v| v.get(&vid))
        .or_else(|| s.roles.get(port))
        .copied()
        .is_some_and(PortRole::forwarding)
    {
        Err(Ineligible::StpDiscarding)
    } else if !s
        .effective_vlans
        .get(port)
        .is_some_and(|v| v.contains_key(&vid))
    {
        Err(Ineligible::VlanUnregistered)
    } else {
        Ok(())
    }
}
fn rows<K: Serialize + Ord, T: Serialize>(p: &Partition<K, T>) -> Vec<Value> {
    p.entries
        .iter()
        .map(|(k, v)| json!({"key":k,"lease":v}))
        .collect()
}
fn audit<T: Serialize>(
    records: &mut Vec<DynamicControlRecord>,
    now: u64,
    kind: &str,
    key: Value,
    before: Option<T>,
    after: Option<T>,
) {
    let before_value = json!(before);
    let after_value = json!(after);
    let generation = after_value
        .get("generation")
        .or_else(|| before_value.get("generation"))
        .and_then(Value::as_u64);
    records.push(DynamicControlRecord {
        synthetic: true,
        control_id: format!("@{kind}/{}", records.len()),
        kind: kind.into(),
        scheduled_ps: now,
        applied_ps: Some(now),
        batch_ordinal: 0,
        epoch_before: 0,
        epoch_after: 0,
        generation,
        outcome: if before.is_some() || after.is_some() {
            "changed"
        } else {
            "no_op"
        }
        .into(),
        key,
        before: before_value,
        after: after_value,
    });
}
fn expire<K: Ord + Clone + Serialize, T: Clone + Serialize>(
    current: &Partition<K, T>,
    staged: &mut Option<Partition<K, T>>,
    now: u64,
    table: &str,
    changes: &mut Vec<Value>,
    records: &mut Vec<DynamicControlRecord>,
) {
    let keys = staged.as_ref().unwrap_or(current).due(now);
    if !keys.is_empty() {
        let p = staged.get_or_insert_with(|| current.clone());
        for key in keys {
            let old = p.remove(&key, table, changes);
            audit(
                records,
                now,
                &format!("{table}_expire"),
                json!(key),
                old,
                Option::<Lease<T>>::None,
            );
        }
    }
}
fn owner<'a>(base: &'a PreparedEthernet, port: &str) -> &'a str {
    let p = base.port_policies.iter().find(|p| p.port == port).unwrap();
    &base.devices[p.device].id
}
fn closed_roles(
    prepared: &PreparedDynamicEthernet,
    base: &PreparedEthernet,
    up: &BTreeMap<String, bool>,
) -> BTreeMap<String, PortRole> {
    let mut roles = BTreeMap::new();
    for l in &prepared.links {
        let inter = l
            .ports
            .iter()
            .all(|p| prepared.bridges.contains_key(owner(base, p)));
        for p in &l.ports {
            roles.insert(
                p.clone(),
                if !up[&l.id] {
                    PortRole::Disabled
                } else if inter {
                    PortRole::Converging
                } else {
                    PortRole::Designated
                },
            );
        }
    }
    roles
}
/// Deterministic minimum-cost common tree, computed independently in each component.
fn tree(
    prepared: &PreparedDynamicEthernet,
    base: &PreparedEthernet,
    up: &BTreeMap<String, bool>,
) -> Result<BTreeMap<String, PortRole>> {
    let mut roles = closed_roles(prepared, base, up);
    let mut graph: BTreeMap<String, Vec<(String, String, String, u64)>> = prepared
        .bridges
        .keys()
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    for l in &prepared.links {
        let a = owner(base, &l.ports[0]);
        let b = owner(base, &l.ports[1]);
        if up[&l.id] && graph.contains_key(a) && graph.contains_key(b) {
            graph.get_mut(a).unwrap().push((
                b.into(),
                l.ports[1].clone(),
                l.ports[0].clone(),
                l.cost,
            ));
            graph.get_mut(b).unwrap().push((
                a.into(),
                l.ports[0].clone(),
                l.ports[1].clone(),
                l.cost,
            ));
        }
    }
    let mut remaining: BTreeSet<_> = graph.keys().cloned().collect();
    while let Some(start) = remaining.first().cloned() {
        let mut component = BTreeSet::from([start.clone()]);
        let mut todo = vec![start];
        while let Some(v) = todo.pop() {
            for (n, _, _, _) in &graph[&v] {
                if component.insert(n.clone()) {
                    todo.push(n.clone());
                }
            }
        }
        for v in &component {
            remaining.remove(v);
        }
        let root = component
            .iter()
            .min_by_key(|id| prepared.bridges[*id])
            .unwrap();
        let mut dist = BTreeMap::from([(root.clone(), 0u64)]);
        let mut unsettled = component.clone();
        while let Some((vertex, cost)) = unsettled
            .iter()
            .filter_map(|v| dist.get(v).map(|d| (v.clone(), *d)))
            .min_by_key(|(v, d)| (*d, prepared.bridges[v]))
        {
            unsettled.remove(&vertex);
            for (n, _, _, c) in &graph[&vertex] {
                if !unsettled.contains(n) {
                    continue;
                }
                let candidate = cost
                    .checked_add(*c)
                    .ok_or_else(|| error("tree distance overflow"))?;
                if dist.get(n).is_none_or(|old| candidate < *old) {
                    dist.insert(n.clone(), candidate);
                }
            }
        }
        let mut root_ports = BTreeSet::new();
        for vertex in &component {
            if vertex == root {
                continue;
            }
            let candidate = graph[vertex]
                .iter()
                .filter(|(n, _, _, c)| dist[n].checked_add(*c) == Some(dist[vertex]))
                .min_by_key(|(n, np, lp, _)| (prepared.bridges[n], np, lp))
                .ok_or_else(|| error("tree root port missing"))?;
            root_ports.insert(candidate.2.clone());
        }
        for l in &prepared.links {
            let a = owner(base, &l.ports[0]);
            let b = owner(base, &l.ports[1]);
            if !up[&l.id] || !component.contains(a) || !component.contains(b) {
                continue;
            }
            let winner = if (dist[a], prepared.bridges[a], &l.ports[0])
                < (dist[b], prepared.bridges[b], &l.ports[1])
            {
                0
            } else {
                1
            };
            for (i, p) in l.ports.iter().enumerate() {
                roles.insert(
                    p.clone(),
                    if root_ports.contains(p) {
                        PortRole::Root
                    } else if i == winner {
                        PortRole::Designated
                    } else {
                        PortRole::Alternate
                    },
                );
            }
        }
    }
    Ok(roles)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::types::ethernet::{EthernetDevice, EthernetDirection, EthernetPortPolicy};
    fn fixture() -> (PreparedEthernet, PreparedDynamicEthernet) {
        let mut devices = Vec::new();
        for (id, kind, mac) in [
            ("Net.a", "endpoint", Some("02:00:00:00:00:01")),
            ("Net.b", "endpoint", Some("02:00:00:00:00:02")),
            ("Net.c", "endpoint", Some("02:00:00:00:00:03")),
            ("Net.s1", "switch", None),
            ("Net.s2", "switch", None),
            ("Net.s3", "switch", None),
        ] {
            devices.push(EthernetDevice {
                id: id.into(),
                kind: kind.into(),
                mac: mac.map(str::to_owned),
                queue_capacity: 64,
                tx_processing_delay_ps: 0,
                rx_processing_delay_ps: 0,
                forward_delay_ps: 2000,
                fdb: BTreeMap::new(),
                vlan_fdb: BTreeMap::new(),
                multicast: BTreeMap::new(),
                subscriptions: BTreeSet::new(),
                unknown_multicast: "flood".into(),
            });
        }
        let pairs = [
            ("A", "Net.a.tx", "Net.s1.tx_a"),
            ("B", "Net.b.tx", "Net.s2.tx_a"),
            ("C", "Net.c.tx", "Net.s3.tx_a"),
            ("L12", "Net.s1.tx_b", "Net.s2.tx_b"),
            ("L13", "Net.s1.tx_c", "Net.s3.tx_b"),
            ("L23", "Net.s2.tx_c", "Net.s3.tx_c"),
        ];
        let mut policies = Vec::new();
        let mut directions = Vec::new();
        let mut links = Vec::new();
        for (id, a, b) in pairs {
            let device = |port: &str| {
                devices
                    .iter()
                    .position(|d| port.starts_with(&format!("{}.", d.id)))
                    .unwrap()
            };
            for p in [a, b] {
                policies.push(EthernetPortPolicy {
                    port: p.into(),
                    ingress: p.replace(".tx", ".rx"),
                    device: device(p),
                    pvid: 10,
                    admit: "all".into(),
                    default_priority: 0,
                    vlans: BTreeMap::from([(10, true)]),
                });
            }
            for (from, to) in [(a, b), (b, a)] {
                directions.push(EthernetDirection {
                    channel_id: format!("{id}:{from}"),
                    from_port: from.into(),
                    to_port: to.replace(".tx", ".rx"),
                    source: device(from),
                    destination: device(to),
                    bitrate_bps: 1_000_000_000,
                    delay_ps: 1000,
                });
            }
            links.push(DynamicLink {
                id: id.into(),
                ports: [a.into(), b.into()],
                up: true,
                cost: 10,
            });
        }
        let base = PreparedEthernet {
            devices,
            directions,
            generators: vec![],
            outputs: vec![],
            port_policies: policies,
            media: None,
        };
        let mut mac_keys = BTreeSet::new();
        for switch in ["Net.s1", "Net.s2", "Net.s3"] {
            for mac in [
                "02:00:00:00:00:01",
                "02:00:00:00:00:02",
                "02:00:00:00:00:03",
            ] {
                mac_keys.insert(MacKey {
                    switch: switch.into(),
                    vid: 10,
                    mac: mac.into(),
                });
            }
        }
        let prepared = PreparedDynamicEthernet {
            mac_age_ps: 1000,
            convergence_ps: 100,
            bridges: BTreeMap::from([
                ("Net.s1".into(), 1),
                ("Net.s2".into(), 2),
                ("Net.s3".into(), 3),
            ]),
            links,
            registrable: BTreeSet::from([RegistrationKey {
                port: "Net.s1.tx_c".into(),
                vid: 20,
            }]),
            limits: DynamicLimits {
                mac_entries: 16,
                membership_entries: 16,
                sources_per_entry: 8,
                registrations: 8,
                control_events: 100,
                pending_timers: 100,
                visits_per_frame: 8,
            },
            controls: vec![],
            generator_ip: BTreeMap::new(),
            mac_keys,
        };
        (base, prepared)
    }
    fn control(id: &str, at: u64, op: ControlOp) -> DynamicControl {
        DynamicControl {
            id: id.into(),
            at_ps: at,
            input_index: 0,
            op,
        }
    }
    fn membership(port: &str) -> MembershipKey {
        MembershipKey {
            switch: "Net.s1".into(),
            vid: 10,
            family: IpFamily::Ipv4,
            group: "239.1.1.1".parse().unwrap(),
            port: port.into(),
        }
    }
    fn wire(dst: &str) -> EthernetWireFrame {
        EthernetWireFrame {
            src_mac: "02:00:00:00:00:01".into(),
            dst_mac: dst.into(),
            ether_type: 2048,
            data_hex: String::new(),
            tag: None,
            tags: vec![],
            pad_bytes: 0,
            mac_bytes: 68,
            fcs_hex: String::new(),
            mac_hex: String::new(),
        }
    }
    #[test]
    fn tree_triangle_deterministic_and_break_before_make() {
        let (base, mut p) = fixture();
        p.controls.push(control(
            "down",
            100,
            ControlOp::LinkSet {
                link: "L13".into(),
                up: false,
            },
        ));
        let mut s = DynamicState::new(&p, &base).unwrap();
        assert_eq!(s.snapshot_tables().as_object().unwrap().len(), 7);
        assert_eq!(
            s.snapshot_policy().roles["Net.s3.tx_c"],
            PortRole::Alternate
        );
        s.apply(s.plan_controls(100).unwrap());
        assert_eq!(s.policy_epoch(), 1);
        assert_eq!(
            s.snapshot_policy().roles["Net.s2.tx_b"],
            PortRole::Converging
        );
        assert_eq!(s.next_deadline(), Some(200));
        s.apply(s.plan_controls(200).unwrap());
        assert_eq!(s.snapshot_policy().roles["Net.s3.tx_c"], PortRole::Root);
        assert_eq!(
            s.snapshot_policy().roles["Net.s2.tx_c"],
            PortRole::Designated
        );
        p.links.reverse();
        let reversed = DynamicState::new(&p, &base).unwrap();
        let original = DynamicState::new(&fixture().1, &base).unwrap();
        assert_eq!(
            reversed.snapshot_policy().roles,
            original.snapshot_policy().roles
        );
    }
    #[test]
    fn learning_refresh_move_capacity_and_expiry() {
        let (base, mut p) = fixture();
        p.limits.mac_entries = 1;
        let mut s = DynamicState::new(&p, &base).unwrap();
        let mac = "02:00:00:00:00:01";
        s.apply(s.plan_learning("Net.s1.rx_a", 10, mac, 100).unwrap());
        s.apply(s.plan_learning("Net.s1.rx_a", 10, mac, 500).unwrap());
        s.apply(s.plan_learning("Net.s1.rx_b", 10, mac, 700).unwrap());
        assert_eq!(s.next_deadline(), Some(1700));
        let entry = s.mac.entries.values().next().unwrap();
        assert_eq!(
            (entry.expires_at, entry.generation, entry.value.as_str()),
            (1700, 3, "Net.s1.tx_b")
        );
        let delta = s
            .plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:02", 800)
            .unwrap();
        assert_eq!(delta.control_records[0].kind, "mac_capacity");
        s.apply(delta);
        s.apply(s.plan_controls(1100).unwrap());
        assert_eq!(s.mac.entries.len(), 1);
        s.apply(s.plan_controls(1700).unwrap());
        assert!(s.mac.entries.is_empty());
        s.apply(s.plan_learning("Net.s1.rx_a", 10, mac, 1700).unwrap());
        assert_eq!(s.mac.entries.values().next().unwrap().generation, 4);
    }
    #[test]
    fn atomic_capacity_failure_retains_state_and_epoch() {
        let (base, mut p) = fixture();
        p.limits.membership_entries = 1;
        p.controls = vec![
            control(
                "a",
                100,
                ControlOp::MembershipSet {
                    key: membership("Net.s1.tx_b"),
                    mode: FilterMode::Include,
                    sources: BTreeSet::new(),
                    expires_at: 1000,
                },
            ),
            control(
                "b",
                100,
                ControlOp::MembershipSet {
                    key: membership("Net.s1.tx_c"),
                    mode: FilterMode::Exclude,
                    sources: BTreeSet::new(),
                    expires_at: 1000,
                },
            ),
        ];
        let s = DynamicState::new(&p, &base).unwrap();
        let before = s.snapshot_tables();
        assert!(s.plan_controls(100).is_err());
        assert_eq!(s.snapshot_tables(), before);
        assert_eq!(s.policy_epoch(), 0);
    }
    #[test]
    fn source_filter_static_union_router_and_ip_collision() {
        let (mut base, mut p) = fixture();
        let x = "192.0.2.1".parse().unwrap();
        p.controls = vec![
            control(
                "b",
                100,
                ControlOp::MembershipSet {
                    key: membership("Net.s1.tx_b"),
                    mode: FilterMode::Include,
                    sources: BTreeSet::from([x]),
                    expires_at: 1000,
                },
            ),
            control(
                "c",
                100,
                ControlOp::MembershipSet {
                    key: membership("Net.s1.tx_c"),
                    mode: FilterMode::Exclude,
                    sources: BTreeSet::from([x]),
                    expires_at: 1000,
                },
            ),
        ];
        let mut s = DynamicState::new(&p, &base).unwrap();
        s.apply(s.plan_controls(100).unwrap());
        let ip = IpMulticast {
            family: IpFamily::Ipv4,
            source: x,
            group: "239.1.1.1".parse().unwrap(),
        };
        let w = wire("01:00:5e:01:01:01");
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, Some(&ip))
                .ports,
            vec!["Net.s1.tx_b"]
        );
        let ip_y = IpMulticast {
            source: "192.0.2.2".parse().unwrap(),
            ..ip.clone()
        };
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, Some(&ip_y))
                .ports,
            vec!["Net.s1.tx_c"]
        );
        let collision = IpMulticast {
            group: "239.129.1.1".parse().unwrap(),
            ..ip.clone()
        };
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, Some(&collision))
                .ports
                .len(),
            2
        );
        base.devices[3]
            .multicast
            .insert((10, w.dst_mac.clone()), vec!["Net.s1.tx_b".into()]);
        let mut s = DynamicState::new(&p, &base).unwrap();
        s.apply(s.plan_controls(100).unwrap());
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, Some(&ip_y))
                .ports
                .len(),
            2
        );
    }
    #[test]
    fn control_refresh_at_expiry_and_registration_eligibility() {
        let (base, mut p) = fixture();
        let k = RegistrationKey {
            port: "Net.s1.tx_c".into(),
            vid: 20,
        };
        p.controls = vec![
            control(
                "a",
                100,
                ControlOp::VlanRegister {
                    key: k.clone(),
                    expires_at: 1100,
                },
            ),
            control(
                "b",
                1100,
                ControlOp::VlanRegister {
                    key: k.clone(),
                    expires_at: 2100,
                },
            ),
        ];
        let mut s = DynamicState::new(&p, &base).unwrap();
        s.apply(s.plan_controls(100).unwrap());
        s.apply(s.plan_controls(1100).unwrap());
        assert_eq!(s.registrations.entries[&k].generation, 2);
        assert_eq!(s.eligible_egress(&k.port, 20, &s.snapshot_policy()), Ok(()));
        s.apply(s.plan_controls(2100).unwrap());
        assert_eq!(
            s.eligible_egress(&k.port, 20, &s.snapshot_policy()),
            Err(Ineligible::VlanUnregistered)
        );
        assert!(s.snapshot_policy().effective_vlans[&k.port].contains_key(&10));
    }
    #[test]
    fn stale_convergence_replaced_and_noop_epoch() {
        let (base, mut p) = fixture();
        p.controls = vec![
            control(
                "a",
                100,
                ControlOp::LinkSet {
                    link: "L13".into(),
                    up: false,
                },
            ),
            control(
                "b",
                200,
                ControlOp::LinkSet {
                    link: "L13".into(),
                    up: true,
                },
            ),
            control(
                "c",
                400,
                ControlOp::LinkSet {
                    link: "L13".into(),
                    up: true,
                },
            ),
        ];
        let mut s = DynamicState::new(&p, &base).unwrap();
        s.apply(s.plan_controls(100).unwrap());
        s.apply(s.plan_controls(200).unwrap());
        assert_eq!(s.topology.converge_at, Some(300));
        assert_eq!(
            s.snapshot_policy().roles["Net.s3.tx_b"],
            PortRole::Converging
        );
        s.apply(s.plan_controls(300).unwrap());
        let epoch = s.policy_epoch();
        let d = s.plan_controls(400).unwrap();
        assert!(d.policy_record.is_none());
        assert_eq!(d.control_records[0].outcome, "no_op");
        s.apply(d);
        assert_eq!(s.policy_epoch(), epoch);
    }
    #[test]
    fn static_shadow_and_known_ineligible_do_not_flood() {
        let (mut base, p) = fixture();
        base.devices[3]
            .vlan_fdb
            .insert((10, "02:00:00:00:00:02".into()), "Net.s1.tx_b".into());
        let mut s = DynamicState::new(&p, &base).unwrap();
        let d = s
            .plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:02", 10)
            .unwrap();
        assert_eq!(d.control_records[0].kind, "static_shadowed");
        s.apply(d);
        assert!(s.mac.entries.is_empty());
        s.topology.link_up.insert("L12".into(), false);
        let selection = s.select_egress(
            "Net.s1",
            "Net.s1.rx_a",
            10,
            &wire("02:00:00:00:00:02"),
            None,
        );
        assert!(selection.ports.is_empty());
        assert_eq!(selection.reason.as_deref(), Some("known_egress_ineligible"));
    }
    #[test]
    fn timer_refresh_bounded_and_generation_overflow_atomic() {
        let (base, mut p) = fixture();
        p.limits.pending_timers = 1;
        let mut s = DynamicState::new(&p, &base).unwrap();
        for t in 0..1000 {
            s.apply(
                s.plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:01", t)
                    .unwrap(),
            );
        }
        assert_eq!(s.mac.entries.len(), 1);
        assert_eq!(s.mac.deadlines.len(), 1);
        assert_eq!(s.next_deadline(), Some(1999));
        let key = s.mac.entries.keys().next().unwrap().clone();
        s.mac.generations.insert(key, u64::MAX);
        let before = s.snapshot_tables();
        assert!(
            s.plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:01", 1000)
                .is_err()
        );
        assert_eq!(s.snapshot_tables(), before);
    }
    #[test]
    fn isolated_components() {
        let (base, mut p) = fixture();
        for l in &mut p.links {
            if l.id.starts_with('L') {
                l.up = false;
            }
        }
        let s = DynamicState::new(&p, &base).unwrap();
        assert_eq!(s.snapshot_policy().roles["Net.s3.tx_c"], PortRole::Disabled);
        assert_eq!(
            s.snapshot_policy().roles["Net.s3.tx_a"],
            PortRole::Designated
        );
    }
    #[test]
    fn strict_input_validates_future_controls_and_ip() {
        let (base, p) = fixture();
        let links: Vec<_> = p
            .links
            .iter()
            .map(|l| json!({"id":l.id,"ports":l.ports,"up":l.up,"cost":l.cost.to_string()}))
            .collect();
        let mut config = json!({"mac_age_ps":"1000","convergence_ps":"100","bridges":[{"instance":"Net.s1","bridge_id":"1"},{"instance":"Net.s2","bridge_id":"2"},{"instance":"Net.s3","bridge_id":"3"}],"links":links,"registrable":[{"port":"Net.s1.tx_c","vid":20,"tagged":true}],"limits":{"mac_entries":10,"membership_entries":10,"sources_per_entry":8,"registrations":8,"control_events":10,"pending_timers":20,"visits_per_frame":8}});
        let mut workload = json!({"schema_version":4,"generators":[],"controls":[]});
        let prepare = crate::input::ethernet::dynamic::prepare;
        assert!(prepare(&config, &workload, &base).is_ok());
        workload["controls"] = json!([{"id":"future","at_ps":"9999999","kind":"vlan_register","port":"Net.s1.tx_c","vid":10,"lifetime_ps":"1"}]);
        assert!(prepare(&config, &workload, &base).is_err());
        workload["controls"] = json!([{"id":"future","at_ps":"18446744073709551615","kind":"link_set","link":"L13","up":false}]);
        assert!(prepare(&config, &workload, &base).is_err());
        workload["controls"] = json!([]);
        config["mac_age_ps"] = json!(true);
        assert!(prepare(&config, &workload, &base).is_err());
    }
    #[test]
    fn parallel_same_cost_chooses_lexical_ports() {
        let (base, mut p) = fixture();
        p.links.retain(|l| l.id != "L23");
        p.links.iter_mut().find(|l| l.id == "L13").unwrap().ports[1] = "Net.s2.tx_c".into();
        let s = DynamicState::new(&p, &base).unwrap();
        assert_eq!(s.snapshot_policy().roles["Net.s2.tx_b"], PortRole::Root);
        assert_eq!(
            s.snapshot_policy().roles["Net.s2.tx_c"],
            PortRole::Alternate
        );
        p.links.reverse();
        assert_eq!(
            s.snapshot_policy().roles,
            DynamicState::new(&p, &base)
                .unwrap()
                .snapshot_policy()
                .roles
        );
    }
    #[test]
    fn known_source_mismatch_does_not_flood_and_router_unions() {
        let (base, mut p) = fixture();
        p.controls = vec![
            control(
                "a",
                0,
                ControlOp::MembershipSet {
                    key: membership("Net.s1.tx_b"),
                    mode: FilterMode::Include,
                    sources: BTreeSet::new(),
                    expires_at: 100,
                },
            ),
            control(
                "b",
                1,
                ControlOp::RouterSet {
                    key: RouterKey {
                        switch: "Net.s1".into(),
                        vid: 10,
                        family: IpFamily::Ipv4,
                        port: "Net.s1.tx_c".into(),
                    },
                    expires_at: 100,
                },
            ),
        ];
        let mut s = DynamicState::new(&p, &base).unwrap();
        s.apply(s.plan_controls(0).unwrap());
        let ip = IpMulticast {
            family: IpFamily::Ipv4,
            source: "192.0.2.1".parse().unwrap(),
            group: "239.1.1.1".parse().unwrap(),
        };
        let w = wire("01:00:5e:01:01:01");
        assert!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, Some(&ip))
                .ports
                .is_empty()
        );
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, None)
                .ports
                .len(),
            2
        );
        s.apply(s.plan_controls(1).unwrap());
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, Some(&ip))
                .ports,
            vec!["Net.s1.tx_c"]
        );
    }
    #[test]
    fn staged_learning_lookup_observes_delta_before_commit() {
        let (base, p) = fixture();
        let s = DynamicState::new(&p, &base).unwrap();
        let w = wire("02:00:00:00:00:01");
        assert_eq!(
            s.select_egress("Net.s1", "Net.s1.rx_a", 10, &w, None)
                .ports
                .len(),
            2
        );
        let d = s.plan_learning("Net.s1.rx_a", 10, &w.src_mac, 100).unwrap();
        assert!(
            s.select_egress_after(&d, "Net.s1", "Net.s1.rx_a", 10, &w, None)
                .ports
                .is_empty()
        );
        assert_eq!(s.snapshot_policy_after(&d).policy_epoch, 1);
        assert_eq!(s.policy_epoch(), 0);
        assert!(s.mac.entries.is_empty());
    }
    fn wire_config(base: &PreparedEthernet) -> crate::types::wire::PreparedWire {
        use crate::types::wire::*;
        let adapters = base
            .devices
            .iter()
            .filter(|device| device.kind == "switch")
            .map(|device| PreparedWireAdapter {
                id: format!("rstp-{}", device.id),
                protocol: WireProtocol::Rstp,
                bridge: device.id.clone(),
                component_binding: None,
                ports: base
                    .port_policies
                    .iter()
                    .filter(|p| base.devices[p.device].id == device.id)
                    .map(|p| p.port.clone())
                    .collect(),
                max_pdu_bytes: 1500,
                max_state_entries: 100,
                max_pending_timers: 100,
                max_pending_tx: 100,
                max_control_steps: 10000,
                options: WireOptions::Rstp(RstpConfig {
                    bridge_id: 1,
                    max_age: 5120,
                    forward_delay: 3840,
                    tx_hold_count: 6,
                    ports: vec![],
                }),
            })
            .collect();
        PreparedWire {
            adapters,
            ports: BTreeMap::new(),
            packets: vec![],
            svre_exists: BTreeSet::new(),
            registrar_modes: BTreeMap::new(),
        }
    }
    fn wire_policy(learning: bool, forwarding: bool) -> crate::snapshot::wire::WirePortPolicy {
        crate::snapshot::wire::WirePortPolicy {
            role: "designated".into(),
            learning,
            forwarding,
            link_up: true,
        }
    }
    #[test]
    fn mstp_vid_policy_controls_learning_flooding_and_checkpoint_projection() {
        use crate::types::wire::{MstpConfig, MstpInstanceConfig, WireOptions, WireProtocol};
        let (mut base, prepared) = fixture();
        for port in &mut base.port_policies {
            port.vlans.insert(20, true);
        }
        let mut prepared_wire = wire_config(&base);
        prepared_wire.adapters.truncate(1);
        let adapter = &mut prepared_wire.adapters[0];
        adapter.protocol = WireProtocol::Mstp;
        let mut vlan_map = vec![0; 4096];
        vlan_map[10] = 1;
        adapter.options = WireOptions::Mstp(MstpConfig {
            bridge_id: 1,
            max_age: 5120,
            forward_delay: 3840,
            tx_hold_count: 6,
            max_hops: 20,
            region_name: "region".into(),
            region_revision: 0,
            vlan_map,
            instances: vec![MstpInstanceConfig {
                mstid: 1,
                bridge_priority: 32768,
            }],
            ports: vec![],
        });
        let adapter = prepared_wire.adapters[0].clone();
        let mut state =
            DynamicState::new_with_wire(&prepared, &base, Some(&prepared_wire)).unwrap();
        let cist: BTreeMap<_, _> = adapter
            .ports
            .iter()
            .map(|port| {
                (
                    port.clone(),
                    wire_policy(port.ends_with("tx_a"), port.ends_with("tx_a")),
                )
            })
            .collect();
        let trees: BTreeMap<_, _> = adapter
            .ports
            .iter()
            .flat_map(|port| {
                let cist = cist[port].clone();
                [
                    ((adapter.id.clone(), 0, port.clone()), cist),
                    (
                        (adapter.id.clone(), 1, port.clone()),
                        wire_policy(true, true),
                    ),
                ]
            })
            .collect();
        let (delta, _, _) = state
            .plan_wire_in(None, 0, &cist, &BTreeSet::new(), &[], true, true)
            .unwrap();
        let delta = state.plan_wire_trees_in(delta, 0, &trees).unwrap();
        // The staged MSTI permits VLAN10 where the same physical port's CIST blocks VLAN20.
        assert_eq!(
            state.wire_ingress_state_for_vid("Net.s1.rx_b", 10, Some(&delta)),
            Some((true, true))
        );
        assert_eq!(
            state.wire_ingress_state_for_vid("Net.s1.rx_b", 20, Some(&delta)),
            Some((false, false))
        );
        let staged = state.snapshot_policy_after(&delta);
        assert_eq!(state.eligible_egress("Net.s1.tx_b", 10, &staged), Ok(()));
        assert_eq!(
            state.eligible_egress("Net.s1.tx_b", 20, &staged),
            Err(Ineligible::StpDiscarding)
        );
        state.apply(delta);
        state.apply(
            state
                .plan_learning("Net.s1.rx_b", 10, "02:00:00:00:00:01", 1)
                .unwrap(),
        );
        state.apply(
            state
                .plan_learning("Net.s1.rx_b", 20, "02:00:00:00:00:01", 1)
                .unwrap(),
        );
        assert_eq!(state.mac.entries.len(), 1);
        assert_eq!(state.mac.entries.keys().next().unwrap().vid, 10);
        for vid in [10, 20] {
            let selected = state.select_egress(
                "Net.s1",
                "Net.s1.rx_a",
                vid,
                &wire("ff:ff:ff:ff:ff:ff"),
                None,
            );
            assert_eq!(selected.ports.contains(&"Net.s1.tx_b".into()), vid == 10);
        }
        state.validate_wire_tree_projection(&trees, true).unwrap();
        let saved = state.checkpoint_fields();
        let restored = DynamicState::restore_fields(
            &saved,
            DynamicState::new_with_wire(&prepared, &base, Some(&prepared_wire)).unwrap(),
        )
        .unwrap();
        restored
            .validate_wire_tree_projection(&trees, true)
            .unwrap();
        let mut mutant = saved;
        mutant["wire"]["tree_policies"]["Net.s1.tx_b"]["1"]["forwarding"] = json!(false);
        let restored = DynamicState::restore_fields(
            &mutant,
            DynamicState::new_with_wire(&prepared, &base, Some(&prepared_wire)).unwrap(),
        )
        .unwrap();
        assert!(
            restored
                .validate_wire_tree_projection(&trees, true)
                .is_err()
        );
    }
    #[test]
    fn wire_learning_scoped_flush_is_staged_and_preserves_relearn_generation() {
        let (mut base, mut prepared) = fixture();
        base.devices[3]
            .vlan_fdb
            .insert((10, "02:00:00:00:00:03".into()), "Net.s1.tx_a".into());
        prepared.mac_keys.insert(MacKey {
            switch: "Net.s1".into(),
            vid: 20,
            mac: "02:00:00:00:00:01".into(),
        });
        let mut state =
            DynamicState::new_with_wire(&prepared, &base, Some(&wire_config(&base))).unwrap();
        assert_eq!(
            state.wire_ingress_state("Net.s1.rx_a", None),
            Some((false, false))
        );
        state.apply(
            state
                .plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:01", 0)
                .unwrap(),
        );
        assert!(state.mac.entries.is_empty());
        let policies = BTreeMap::from([
            ("Net.s1.tx_a".into(), wire_policy(true, false)),
            ("Net.s1.tx_b".into(), wire_policy(true, true)),
            ("Net.s1.tx_c".into(), wire_policy(false, false)),
        ]);
        let (delta, _, _) = state
            .plan_wire_in(None, 0, &policies, &BTreeSet::new(), &[], true, true)
            .unwrap();
        assert_eq!(
            state.wire_ingress_state("Net.s1.rx_a", Some(&delta)),
            Some((true, false))
        );
        assert!(
            state
                .eligible_egress("Net.s1.tx_a", 10, &state.snapshot_policy_after(&delta))
                .is_err()
        );
        assert_eq!(state.policy_epoch(), 0);
        state.apply(delta);
        for (port, vid, mac, time) in [
            ("Net.s1.rx_a", 10, "02:00:00:00:00:01", 0),
            ("Net.s1.rx_a", 20, "02:00:00:00:00:01", 1),
            ("Net.s1.rx_b", 10, "02:00:00:00:00:02", 2),
        ] {
            state.apply(state.plan_learning(port, vid, mac, time).unwrap());
        }
        assert_eq!(state.mac.entries.len(), 3);
        let flush = [crate::runtime::wire::WireFlushIntent {
            reason: "rstp_topology_change".into(),
            port: "Net.s1.tx_a".into(),
            vids: vec![10],
        }];
        let (delta, removed, mac) = state
            .plan_wire_in(None, 10, &policies, &BTreeSet::new(), &flush, false, false)
            .unwrap();
        assert_eq!(removed[0].len(), 1);
        assert_eq!(removed[0][0].vid, 10);
        assert_eq!(mac.len(), 2);
        assert_eq!(state.mac.entries.len(), 3);
        state.apply(delta);
        assert_eq!(state.mac.entries.len(), 2);
        assert_eq!(base.devices[3].vlan_fdb.len(), 1);
        state.apply(
            state
                .plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:01", 20)
                .unwrap(),
        );
        state.apply(state.plan_controls(1000).unwrap());
        let entry = &state.mac.entries[&MacKey {
            switch: "Net.s1".into(),
            vid: 10,
            mac: "02:00:00:00:00:01".into(),
        }];
        assert_eq!((entry.generation, entry.expires_at), (2, 1020));
    }
    #[test]
    fn wire_link_change_does_not_run_abstract_tree_or_flush_all_mac() {
        let (base, mut prepared) = fixture();
        prepared.controls.push(control(
            "down",
            100,
            ControlOp::LinkSet {
                link: "L13".into(),
                up: false,
            },
        ));
        let mut state =
            DynamicState::new_with_wire(&prepared, &base, Some(&wire_config(&base))).unwrap();
        let policies = base
            .port_policies
            .iter()
            .filter(|p| base.devices[p.device].kind == "switch")
            .map(|p| (p.port.clone(), wire_policy(true, true)))
            .collect();
        let (delta, _, _) = state
            .plan_wire_in(None, 0, &policies, &BTreeSet::new(), &[], true, false)
            .unwrap();
        state.apply(delta);
        state.apply(
            state
                .plan_learning("Net.s1.rx_a", 10, "02:00:00:00:00:01", 0)
                .unwrap(),
        );
        let delta = state.plan_controls(100).unwrap();
        assert!(delta.mac.as_ref().unwrap_or(&state.mac).entries.len() == 1);
        assert!(delta.topology.as_ref().unwrap().converge_at.is_none());
        assert_eq!(state.semantic_pending_after(Some(&delta)), 1);
        assert_eq!(state.mac.entries.len(), 1);
    }
    #[test]
    fn passive_wire_projection_audits_are_atomic_without_duplicate_expiry_ownership() {
        use crate::runtime::wire::membership::{Filter, Key};
        let (base, prepared) = fixture();
        let mut state = DynamicState::new(&prepared, &base).unwrap();
        let key = Key {
            bridge: "Net.s1".into(),
            vid: 10,
            group: "239.1.1.1".parse().unwrap(),
            port: "Net.s1.tx_a".into(),
        };
        let memberships = BTreeMap::from([(
            key.clone(),
            Filter {
                exclude: false,
                sources: BTreeSet::from(["192.0.2.1".parse().unwrap()]),
            },
        )]);
        let delta = state
            .plan_wire_membership_in(
                state.delta(),
                10,
                &memberships,
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .unwrap();
        assert!(state.wire.memberships.is_empty());
        assert!(delta.next_deadline.is_none());
        assert_eq!(delta.control_records.len(), 1);
        assert_eq!(delta.control_records[0].kind, "wire_membership_set");
        assert_eq!(delta.control_records[0].after["mode"], "include");
        assert_eq!(delta.control_records[0].generation, Some(1));
        assert_eq!(state.semantic_pending_after(Some(&delta)), 0);
        state.apply(delta);
        let noop = state
            .plan_wire_membership_in(
                state.delta(),
                11,
                &memberships,
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .unwrap();
        assert!(noop.control_records.is_empty());
        assert_eq!(noop.epoch, 1);
        let delta = state
            .plan_wire_membership_in(
                state.delta(),
                20,
                &BTreeMap::new(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(delta.control_records.len(), 1);
        assert_eq!(delta.control_records[0].kind, "wire_membership_leave");
        assert_eq!(delta.control_records[0].generation, Some(2));
        assert!(delta.control_records[0].after.is_null());
        state.apply(delta);
        let delta = state
            .plan_wire_membership_in(
                state.delta(),
                30,
                &memberships,
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(delta.control_records[0].generation, Some(3));
        assert_eq!(delta.control_records[0].epoch_before, 2);
        assert_eq!(delta.control_records[0].epoch_after, 3);
    }
    include!("dynamic_vector_tests.rs");
}

crate::runtime::network::checkpoint::fields!(DynamicState, value; mac, membership, routers, registrations, topology, epoch, control_cursor, audit_cursor, wire_enabled, wire_tree_bridges, wire_registration_ports, wire; );
impl<K: Ord + Clone, T> Partition<K, T> {
    fn validate_checkpoint(&self, horizon: u64) -> Result<()> {
        let mut deadlines = BTreeMap::<u64, BTreeSet<K>>::new();
        for (key, lease) in &self.entries {
            if lease.expires_at < horizon
                || self.generations.get(key) != Some(&lease.generation)
                || lease.generation == 0
            {
                return Err(error("invalid dynamic lease checkpoint"));
            }
            deadlines
                .entry(lease.expires_at)
                .or_default()
                .insert(key.clone());
        }
        if deadlines != self.deadlines {
            return Err(error("dynamic deadline checkpoint mismatch"));
        }
        Ok(())
    }
}
impl DynamicState {
    pub(crate) fn validate_checkpoint(&self, horizon: u64) -> Result<()> {
        if self.control_cursor > self.prepared.controls.len() {
            return Err(error("dynamic control checkpoint cursor"));
        }
        if self.prepared.controls[..self.control_cursor]
            .iter()
            .any(|c| c.at_ps > horizon)
            || self.prepared.controls[self.control_cursor..]
                .iter()
                .any(|c| c.at_ps < horizon)
        {
            return Err(error("dynamic control checkpoint time"));
        }
        self.mac.validate_checkpoint(horizon)?;
        self.membership.validate_checkpoint(horizon)?;
        self.routers.validate_checkpoint(horizon)?;
        self.registrations.validate_checkpoint(horizon)?;
        let initial = Self::new_with_wire(&self.prepared, &self.base, None)?;
        if self
            .topology
            .link_up
            .keys()
            .ne(initial.topology.link_up.keys())
            || self.topology.roles.keys().ne(initial.topology.roles.keys())
        {
            return Err(error("dynamic topology checkpoint catalog mismatch"));
        }
        if self.mac.entries.len() > self.prepared.limits.mac_entries
            || self.membership.entries.len() > self.prepared.limits.membership_entries
        {
            return Err(error("dynamic checkpoint table limit"));
        }
        Ok(())
    }
}
