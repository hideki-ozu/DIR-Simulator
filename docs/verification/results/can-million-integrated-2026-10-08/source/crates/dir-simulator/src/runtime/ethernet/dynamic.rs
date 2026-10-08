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
#[derive(Debug, Clone)]
struct Partition<K, T> {
    entries: BTreeMap<K, Lease<T>>,
    generations: BTreeMap<K, u64>,
    deadlines: BTreeMap<u64, BTreeSet<K>>,
}
impl<K, T> Default for Partition<K, T> {
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
#[derive(Debug, Clone)]
struct Topology {
    link_up: BTreeMap<String, bool>,
    roles: BTreeMap<String, PortRole>,
    generation: u64,
    converge_at: Option<u64>,
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
}
/// Changed partitions are fully built before commit and swapped without allocation.
#[derive(Debug)]
pub(crate) struct DynamicDelta {
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
        let up = prepared
            .links
            .iter()
            .map(|l| (l.id.clone(), l.up))
            .collect();
        let roles = tree(prepared, base, &up)?;
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
        })
    }
    fn delta(&self) -> DynamicDelta {
        DynamicDelta {
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
        for key in d
            .registrations
            .as_ref()
            .unwrap_or(&self.registrations)
            .entries
            .keys()
        {
            effective_vlans
                .entry(key.port.clone())
                .or_default()
                .insert(key.vid, true);
        }
        PolicySnapshot {
            policy_epoch: d.epoch,
            topology_generation: topology.generation,
            roles: topology.roles.clone(),
            link_up,
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
        json!({"policy":self.snapshot_policy(),"mac":rows(&self.mac),"membership":rows(&self.membership),"router":rows(&self.routers),"registration":rows(&self.registrations),"next_deadline":self.next_deadline(),"converge_at":self.topology.converge_at})
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
            t.converge_at = Some(
                now.checked_add(self.prepared.convergence_ps)
                    .ok_or_else(|| error("convergence overflow"))?,
            );
            t.roles = closed_roles(&self.prepared, &self.base, &t.link_up);
            let m = d.mac.get_or_insert_with(|| self.mac.clone());
            let keys: Vec<_> = m.entries.keys().cloned().collect();
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
            t.roles = tree(&self.prepared, &self.base, &t.link_up)?;
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
    } else if !s.roles.get(port).copied().is_some_and(PortRole::forwarding) {
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
mod tests {
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
}
