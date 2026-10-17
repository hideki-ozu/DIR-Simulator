//! Transactional protocol coordinator. The network host owns FES, media and commit.
#[path = "wire/boundary.rs"]
pub(crate) mod boundary;
#[path = "wire/codec.rs"]
pub(crate) mod codec;
#[path = "wire/guards.rs"]
pub(crate) mod guards;
#[path = "wire/membership.rs"]
pub(crate) mod membership;
#[path = "wire/mvrp.rs"]
pub(crate) mod mvrp;
#[path = "wire/mvrp_state.rs"]
pub(crate) mod mvrp_state;
#[path = "wire/records.rs"]
mod records;
#[path = "wire/rstp.rs"]
pub(crate) mod rstp;
#[path = "wire/transition.rs"]
pub(crate) mod transition;
#[cfg(test)]
#[path = "wire/vector_manifest.rs"]
pub(crate) mod vector_manifest;
use crate::snapshot::wire::{
    WireEventRecord, WirePortPolicy, WireTimerKey, WireTimerRecord, WireTransitionRecord,
};
use crate::types::{Diagnostic, wire::*};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
type Result<T> = std::result::Result<T, Diagnostic>;
fn fail(s: impl Into<String>) -> Diagnostic {
    Diagnostic::execution(s)
}
fn protocol(p: WireProtocol) -> String {
    format!("{p:?}").to_lowercase()
}
#[derive(Debug, Clone)]
pub(crate) enum WireEvent {
    Initialize,
    Receive {
        protocol: WireProtocol,
        ingress_port: String,
        packet_id: String,
        bytes: Vec<u8>,
        vid: Option<u16>,
        destination_mac: Option<[u8; 6]>,
        external_injection: bool,
    },
    Timer {
        adapter_id: String,
        key: WireTimerKey,
        generation: u64,
        deadline_ps: u64,
    },
    Link {
        port: String,
        up: bool,
    },
}
#[derive(Debug, Clone, Default)]
pub(crate) struct WireStaticMulticast {
    pub known: bool,
    pub members: BTreeSet<String>,
    pub routers: BTreeSet<String>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct WireView {
    /// Current physical links, keyed by output port. Missing means link up.
    pub link_up: BTreeMap<String, bool>,
    /// Current data ingress admission, checked after boundary RX completion.
    pub ingress_eligible: BTreeMap<(String, u16), bool>,
    pub policy_epoch: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireTimerIntent {
    pub adapter_id: String,
    pub key: WireTimerKey,
    pub generation: u64,
    pub deadline_ps: Option<u64>,
}
#[derive(Debug, Clone)]
pub(crate) struct WireTxIntent {
    pub adapter_id: String,
    pub packet_id: String,
    pub port: String,
    pub protocol: WireProtocol,
    pub ordinal: u64,
    pub bytes: Vec<u8>,
    pub cause_id: String,
}
#[derive(Debug, Clone)]
pub(crate) struct WireFlushIntent {
    pub reason: String,
    pub port: String,
    pub vids: Vec<u16>,
}
#[derive(Debug, Clone)]
pub(crate) struct WireForwardIntent {
    pub packet_id: String,
    pub ports: Vec<String>,
    pub bytes: Vec<u8>,
    pub vid: u16,
}
#[derive(Debug, Clone)]
pub(crate) struct WireRecord {
    pub kind: String,
    pub data: Value,
}
#[derive(Debug, Clone)]
pub(crate) struct ProtocolDelta {
    next: WireState,
    transition_ordinal: u64,
    #[cfg(test)]
    pub integration_stages: Vec<String>,
    pub timers: Vec<WireTimerIntent>,
    pub tx: Vec<WireTxIntent>,
    pub flush: Vec<WireFlushIntent>,
    pub forwards: Vec<WireForwardIntent>,
    pub records: Vec<WireRecord>,
    pub policies: BTreeMap<String, WirePortPolicy>,
    pub registered: BTreeSet<(String, u16)>,
    pub registration_generations: BTreeMap<(String, u16), u64>,
    pub memberships: BTreeMap<membership::Key, membership::Filter>,
    pub router_ports: BTreeSet<membership::Key>,
    pub compatibility_flood: BTreeSet<(String, u16, bool)>,
    pub policy_changed: bool,
    pub topology_changed: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct WireState {
    prepared: Arc<PreparedWire>,
    rstp: BTreeMap<String, rstp::Bridge>,
    mvrp: BTreeMap<String, mvrp_state::State>,
    snooper: membership::Snooper,
    generation: BTreeMap<String, u64>,
    timers: BTreeMap<(String, WireTimerKey), (u64, Option<u64>)>,
    tx_ordinals: BTreeMap<String, u64>,
    registration_generations: BTreeMap<(String, u16), u64>,
    callback: u64,
    record_sequence: u64,
    initialized: bool,
}
impl WireState {
    pub(crate) fn new(prepared: Arc<PreparedWire>) -> Self {
        let mut bridges = BTreeMap::new();
        let mut mrp = BTreeMap::new();
        for a in &prepared.adapters {
            match &a.options {
                WireOptions::Rstp(c) => {
                    let ports = c
                        .ports
                        .iter()
                        .map(|p| {
                            (
                                p.port.clone(),
                                (
                                    rstp::PortConfig {
                                        id: p.port_id,
                                        cost: p.external_path_cost,
                                        admin_edge: p.admin_edge,
                                        auto_edge: p.auto_edge,
                                        auto_isolate: p.auto_isolate,
                                        restricted_role: p.restricted_role,
                                        restricted_tcn: p.restricted_tcn,
                                    },
                                    true,
                                ),
                            )
                        })
                        .collect();
                    bridges.insert(
                        a.id.clone(),
                        rstp::Bridge::new(
                            c.bridge_id,
                            rstp::Times {
                                age: 0,
                                max_age: c.max_age,
                                hello: 512,
                                forward_delay: c.forward_delay,
                            },
                            c.tx_hold_count,
                            ports,
                        ),
                    );
                }
                WireOptions::Mvrp(_) => {
                    mrp.insert(a.id.clone(), mvrp_state::State::new(a));
                }
                _ => {}
            }
        }
        Self {
            prepared,
            rstp: bridges,
            mvrp: mrp,
            snooper: membership::Snooper::default(),
            generation: BTreeMap::new(),
            timers: BTreeMap::new(),
            tx_ordinals: BTreeMap::new(),
            registration_generations: BTreeMap::new(),
            callback: 0,
            record_sequence: 0,
            initialized: false,
        }
    }
    pub(crate) fn pending_timers(&self) -> usize {
        self.timers
            .values()
            .filter(|(_, deadline)| deadline.is_some())
            .count()
    }
    pub(crate) fn runtime_snapshot(&self) -> Value {
        let mvrp:Vec<_>=self.mvrp.iter().map(|(id,state)|json!({"adapter_id":id,"ports":state.ports.iter().map(|(name,p)|json!({"port":name,"operational":p.operational,"forwarding":p.forwarding,"tc_detected":p.tc_detected.to_string(),"leaveall_active":p.leaveall_active,"periodic_active":p.periodic_active,"random_counter":p.random_counter.to_string(),"last_opportunity":p.last_opportunity.map(|n|n.to_string()),"attribute_domain":[1,4094],"attribute_default":{"applicant":"VO","registrar":"MT","registered":false,"demand":false,"request_tx":false},"attributes":p.attributes.iter().filter(|(_,a)|a.applicant!=mvrp::Applicant::VO||a.registrar!=mvrp::Registrar::MT||a.registered||a.demand||a.request_tx).map(|(vid,a)|json!({"vid":vid,"applicant":a.applicant,"registrar":a.registrar,"registered":a.registered,"demand":a.demand,"request_tx":a.request_tx})).collect::<Vec<_>>(),"timers":p.timers.iter().map(|((slot,vid),t)|json!({"slot":slot,"vid":vid,"generation":t.generation.to_string(),"deadline_ps":t.deadline.map(|n|n.to_string())})).collect::<Vec<_>>() })).collect::<Vec<_>>() })).collect();
        json!({"initialized":self.initialized,"callback":self.callback.to_string(),"record_sequence":self.record_sequence.to_string(),"rstp":self.rstp,"mvrp":mvrp,"leases":self.snooper.leases.iter().map(|((key,filter),lease)|json!({"key":key,"filter":filter,"deadline_ps":lease.deadline.to_string(),"generation":lease.generation.to_string()})).collect::<Vec<_>>(),"routers":self.snooper.routers.iter().map(|(key,lease)|json!({"key":key,"deadline_ps":lease.deadline.to_string(),"generation":lease.generation.to_string()})).collect::<Vec<_>>(),"lease_generations":self.snooper.generations.iter().map(|((key,filter),generation)|json!({"key":key,"filter":filter,"generation":generation.to_string()})).collect::<Vec<_>>(),"router_generations":self.snooper.router_generations.iter().map(|(key,generation)|json!({"key":key,"generation":generation.to_string()})).collect::<Vec<_>>(),"compatibility_flood":self.snooper.flood,"timer_ledger":self.timers.iter().map(|((adapter,key),(generation,deadline))|json!({"adapter_id":adapter,"key":key,"generation":generation.to_string(),"deadline_ps":deadline.map(|n|n.to_string())})).collect::<Vec<_>>(),"generations":self.generation,"tx_ordinals":self.tx_ordinals,"registration_generations":self.registration_generations.iter().map(|((port,vid),generation)|json!({"port":port,"vid":vid,"generation":generation.to_string()})).collect::<Vec<_>>()})
    }
    pub(crate) fn policies(&self) -> BTreeMap<String, WirePortPolicy> {
        self.rstp
            .values()
            .flat_map(|b| {
                b.ports.iter().map(|(n, p)| {
                    (
                        n.clone(),
                        WirePortPolicy {
                            role: format!("{:?}", p.role),
                            learning: p.learning,
                            forwarding: p.forwarding,
                            link_up: p.enabled,
                        },
                    )
                })
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn multicast_candidates(
        &self,
        bridge: &str,
        vid: u16,
        source: std::net::IpAddr,
        group: std::net::IpAddr,
        eligible: &BTreeSet<String>,
        ingress: &str,
    ) -> BTreeSet<String> {
        records::candidates(
            &self.snooper,
            bridge,
            vid,
            source,
            group,
            eligible,
            ingress,
            &WireStaticMulticast::default(),
        )
    }
    #[allow(clippy::too_many_arguments)] // Preserve the host selector interface.
    pub(crate) fn multicast_candidates_with_static(
        &self,
        bridge: &str,
        vid: u16,
        source: std::net::IpAddr,
        group: std::net::IpAddr,
        eligible: &BTreeSet<String>,
        ingress: &str,
        static_view: &WireStaticMulticast,
    ) -> BTreeSet<String> {
        records::candidates(
            &self.snooper,
            bridge,
            vid,
            source,
            group,
            eligible,
            ingress,
            static_view,
        )
    }
    pub(crate) fn registered(&self) -> BTreeSet<(String, u16)> {
        self.mvrp
            .values()
            .flat_map(|s| {
                s.ports.iter().flat_map(|(name, p)| {
                    p.attributes
                        .iter()
                        .filter(|(_, a)| a.registered)
                        .map(|(vid, _)| (name.clone(), *vid))
                })
            })
            .collect()
    }
    pub(crate) fn apply(&mut self, delta: ProtocolDelta) {
        *self = delta.next;
    }
    pub(crate) fn plan(
        &self,
        now: u64,
        event: WireEvent,
        view: &WireView,
    ) -> Result<ProtocolDelta> {
        if !self.initialized && !matches!(event, WireEvent::Initialize) {
            return Err(fail("wire callback before initialize"));
        }
        if self.initialized && matches!(event, WireEvent::Initialize) {
            return Err(fail("wire duplicate initialize"));
        }
        let mut d = ProtocolDelta {
            next: self.clone(),
            transition_ordinal: 0,
            #[cfg(test)]
            integration_stages: vec![],
            timers: vec![],
            tx: vec![],
            flush: vec![],
            forwards: vec![],
            records: vec![],
            policies: BTreeMap::new(),
            registered: BTreeSet::new(),
            registration_generations: BTreeMap::new(),
            memberships: BTreeMap::new(),
            router_ports: BTreeSet::new(),
            compatibility_flood: BTreeSet::new(),
            policy_changed: false,
            topology_changed: false,
        };
        d.next.callback = d
            .next
            .callback
            .checked_add(1)
            .ok_or_else(|| fail("wire callback overflow"))?;
        let audit_event = event.clone();
        match event {
            WireEvent::Initialize => {
                d.next.initialized = true;
                let adapters = d.next.prepared.adapters.clone();
                for a in &adapters {
                    if a.protocol == WireProtocol::Rstp {
                        for (n, p) in &mut d.next.rstp.get_mut(&a.id).unwrap().ports {
                            p.enabled = view.link_up.get(n).copied().unwrap_or(true);
                        }
                        d.rstp(a, now, rstp::Input::Initialize, "initialize")?;
                    } else if a.protocol == WireProtocol::Mvrp {
                        let policies = d.next.policies();
                        for (n, p) in &mut d.next.mvrp.get_mut(&a.id).unwrap().ports {
                            p.operational = view.link_up.get(n).copied().unwrap_or(true);
                            p.forwarding = policies.get(n).is_none_or(|v| v.forwarding);
                        }
                        d.mvrp(a, now, mvrp_state::Input::Initialize, "initialize")?;
                    }
                    d.event(
                        a,
                        now,
                        "initialize",
                        None,
                        None,
                        None,
                        false,
                        None,
                        view.policy_epoch,
                    )?;
                }
            }
            WireEvent::Receive {
                protocol: kind,
                ingress_port,
                packet_id,
                bytes,
                vid,
                destination_mac,
                external_injection,
            } => {
                let a = d
                    .next
                    .prepared
                    .owner(kind, &ingress_port)
                    .cloned()
                    .ok_or_else(|| fail("wire receive owner not found"))?;
                let port = a
                    .ports
                    .iter()
                    .find(|p| {
                        **p == ingress_port || d.next.prepared.ports[*p].ingress == ingress_port
                    })
                    .unwrap()
                    .clone();
                let reason = if !view.link_up.get(&port).copied().unwrap_or(true) {
                    Some("link_down".to_string())
                } else {
                    match &a.options {
                        WireOptions::Rstp(c) => {
                            let pc = c.ports.iter().find(|p| p.port == port).unwrap();
                            match rstp::decode(&bytes, pc.port_id, a.max_pdu_bytes) {
                                Err(e) => Some(match e.0.as_str() {
                                    "frame_bounds" | "fcs" => e.0,
                                    _ => "protocol_malformed".into(),
                                }),
                                Ok(m) => {
                                    d.rstp(
                                        &a,
                                        now,
                                        rstp::Input::Receive(port.clone(), m),
                                        &packet_id,
                                    )?;
                                    None
                                }
                            }
                        }
                        WireOptions::Mvrp(c) => match mvrp::decode(
                            &bytes,
                            d.next.prepared.ports[&port].pvid,
                            a.max_pdu_bytes,
                            c.max_attributes_per_pdu,
                        ) {
                            Err(e) => Some(match e.0.as_str() {
                                "frame_bounds" | "fcs" => e.0,
                                _ => "protocol_malformed".into(),
                            }),
                            Ok(m) => {
                                d.mvrp(
                                    &a,
                                    now,
                                    mvrp_state::Input::Receive(port.clone(), m),
                                    &packet_id,
                                )?;
                                None
                            }
                        },
                        WireOptions::Membership(c) => {
                            let vid = vid.ok_or_else(|| fail("membership receive requires VID"))?;
                            if !view
                                .ingress_eligible
                                .get(&(port.clone(), vid))
                                .copied()
                                .unwrap_or(true)
                            {
                                Some("ingress_ineligible".into())
                            } else {
                                match membership::decode(
                                    &bytes,
                                    kind == WireProtocol::Mld,
                                    c.max_sources_per_record,
                                ) {
                                    Err(e) => Some(e.0),
                                    Ok(m) => {
                                        if destination_mac
                                            != Some(membership::destination_mac(m.destination))
                                        {
                                            Some("destination_mac".into())
                                        } else {
                                            let candidate = d
                                                .next
                                                .snooper
                                                .receive(
                                                    &a.bridge,
                                                    &port,
                                                    vid,
                                                    kind == WireProtocol::Mld,
                                                    now,
                                                    &m,
                                                    c.membership_interval_ps,
                                                    c.router_interval_ps,
                                                )
                                                .map_err(fail)?;
                                            let count = candidate
                                                .leases
                                                .iter()
                                                .filter(|((k, _), _)| {
                                                    k.bridge == a.bridge
                                                        && k.group.is_ipv6()
                                                            == (kind == WireProtocol::Mld)
                                                })
                                                .count();
                                            let groups: BTreeSet<_> = candidate
                                                .leases
                                                .keys()
                                                .filter(|(k, _)| {
                                                    k.bridge == a.bridge
                                                        && k.group.is_ipv6()
                                                            == (kind == WireProtocol::Mld)
                                                })
                                                .map(|(k, _)| k.clone())
                                                .collect();
                                            let sources: usize = candidate
                                                .leases
                                                .keys()
                                                .filter(|(k, _)| {
                                                    k.bridge == a.bridge
                                                        && k.group.is_ipv6()
                                                            == (kind == WireProtocol::Mld)
                                                })
                                                .map(|(_, f)| f.sources.len())
                                                .sum();
                                            if count > c.max_filter_leases
                                                || groups.len() > c.max_groups
                                                || sources > c.max_sources_total
                                            {
                                                return Err(fail("wire membership capacity"));
                                            }
                                            d.next.snooper = candidate;
                                            let routers: BTreeSet<_> = d
                                                .next
                                                .snooper
                                                .routers
                                                .keys()
                                                .filter(|k| {
                                                    k.bridge == a.bridge
                                                        && k.vid == vid
                                                        && k.group.is_ipv6()
                                                            == (kind == WireProtocol::Mld)
                                                })
                                                .map(|k| k.port.clone())
                                                .collect();
                                            let forwarding = d
                                                .next
                                                .prepared
                                                .ports
                                                .values()
                                                .filter(|p| {
                                                    p.bridge == a.bridge
                                                        && p.output != port
                                                        && view
                                                            .link_up
                                                            .get(&p.output)
                                                            .copied()
                                                            .unwrap_or(true)
                                                        && view
                                                            .ingress_eligible
                                                            .get(&(p.output.clone(), vid))
                                                            .copied()
                                                            .unwrap_or(true)
                                                        && (m.is_query
                                                            || m.legacy
                                                            || routers.is_empty()
                                                            || routers.contains(&p.output))
                                                })
                                                .map(|p| p.output.clone())
                                                .collect();
                                            d.forwards.push(WireForwardIntent {
                                                packet_id: packet_id.clone(),
                                                ports: forwarding,
                                                bytes: bytes.clone(),
                                                vid,
                                            });
                                            d.membership_wake(&a, now)?;
                                            None
                                        }
                                    }
                                }
                            }
                        }
                    }
                };
                d.event(
                    &a,
                    now,
                    "receive",
                    Some(&port),
                    Some((&packet_id, &bytes)),
                    Some(&packet_id),
                    external_injection,
                    reason,
                    view.policy_epoch,
                )?;
            }
            WireEvent::Timer {
                adapter_id,
                key,
                generation,
                deadline_ps,
            } => {
                let a = d
                    .next
                    .prepared
                    .adapter(&adapter_id)
                    .cloned()
                    .ok_or_else(|| fail("unknown wire timer adapter"))?;
                let valid = d.next.timers.get(&(adapter_id.clone(), key.clone()))
                    == Some(&(generation, Some(deadline_ps)))
                    && deadline_ps == now;
                if !valid {
                    d.event(
                        &a,
                        now,
                        "timer",
                        Some(&key.port),
                        None,
                        None,
                        false,
                        Some("timer_token".into()),
                        view.policy_epoch,
                    )?;
                } else {
                    d.next.timers.get_mut(&(adapter_id, key.clone())).unwrap().1 = None;
                    match a.protocol {
                        WireProtocol::Rstp => d.rstp(&a, now, rstp::Input::Tick, "tick")?,
                        WireProtocol::Mvrp => d.mvrp(
                            &a,
                            now,
                            mvrp_state::Input::Timer {
                                port: key.port.clone(),
                                slot: key.slot.clone(),
                                vid: key.attribute,
                                generation,
                            },
                            "timer",
                        )?,
                        _ => {
                            d.next.snooper = d.next.snooper.expire(&a.bridge, now).map_err(fail)?;
                            d.membership_wake(&a, now)?;
                        }
                    }
                    d.event(
                        &a,
                        now,
                        "timer",
                        Some(&key.port),
                        None,
                        None,
                        false,
                        None,
                        view.policy_epoch,
                    )?;
                }
            }
            WireEvent::Link { port, up } => {
                let adapters: Vec<_> = d
                    .next
                    .prepared
                    .adapters
                    .iter()
                    .filter(|a| a.ports.contains(&port))
                    .cloned()
                    .collect();
                let paired_rstp = adapters.iter().any(|a| a.protocol == WireProtocol::Rstp);
                for a in adapters {
                    match a.protocol {
                        WireProtocol::Rstp => {
                            d.rstp(&a, now, rstp::Input::Link(port.clone(), up), "link")?
                        }
                        WireProtocol::Mvrp if !paired_rstp => {
                            let policy = d.next.policies();
                            d.mvrp(
                                &a,
                                now,
                                mvrp_state::Input::Tree {
                                    port: port.clone(),
                                    operational: up,
                                    forwarding: policy.get(&port).map_or(up, |p| p.forwarding),
                                    tc_detected: 0,
                                    flush: false,
                                    redeclare: false,
                                },
                                "link",
                            )?;
                        }
                        _ => {}
                    }
                    d.event(
                        &a,
                        now,
                        "link",
                        Some(&port),
                        None,
                        None,
                        false,
                        None,
                        view.policy_epoch,
                    )?;
                }
            }
        }
        d.policies = d.next.policies();
        d.registered = d.next.registered();
        d.memberships = d.next.snooper.projection();
        d.router_ports = d.next.snooper.routers.keys().cloned().collect();
        d.compatibility_flood = d.next.snooper.flood.clone();
        d.topology_changed = d.policies != self.policies();
        let old = self.registered();
        for key in old.symmetric_difference(&d.registered) {
            let g = d
                .next
                .registration_generations
                .entry(key.clone())
                .or_insert(0);
            *g = g
                .checked_add(1)
                .ok_or_else(|| fail("wire registration generation overflow"))?;
        }
        d.registration_generations = d.next.registration_generations.clone();
        d.policy_changed =
            d.topology_changed || old != d.registered || self.snooper != d.next.snooper;
        let epoch = if d.policy_changed {
            view.policy_epoch
                .checked_add(1)
                .ok_or_else(|| fail("wire policy epoch overflow"))?
        } else {
            view.policy_epoch
        };
        // Adapter generations advance once per callback, irrespective of internal worklist steps.
        for a in &self.prepared.adapters {
            let changed =
                match a.protocol {
                    WireProtocol::Rstp => self.rstp.get(&a.id) != d.next.rstp.get(&a.id),
                    WireProtocol::Mvrp => self.mvrp.get(&a.id) != d.next.mvrp.get(&a.id),
                    _ => {
                        let v6 = a.protocol == WireProtocol::Mld;
                        !self
                            .snooper
                            .leases
                            .iter()
                            .filter(|((k, _), _)| k.bridge == a.bridge && k.group.is_ipv6() == v6)
                            .eq(d.next.snooper.leases.iter().filter(|((k, _), _)| {
                                k.bridge == a.bridge && k.group.is_ipv6() == v6
                            }))
                            || !self
                                .snooper
                                .routers
                                .iter()
                                .filter(|(k, _)| k.bridge == a.bridge && k.group.is_ipv6() == v6)
                                .eq(d.next.snooper.routers.iter().filter(|(k, _)| {
                                    k.bridge == a.bridge && k.group.is_ipv6() == v6
                                }))
                            || !self
                                .snooper
                                .flood
                                .iter()
                                .filter(|(b, _, f)| b == &a.bridge && *f == v6)
                                .eq(d
                                    .next
                                    .snooper
                                    .flood
                                    .iter()
                                    .filter(|(b, _, f)| b == &a.bridge && *f == v6))
                    }
                };
            if changed
                && !d.records.iter().any(|r| {
                    r.kind == "ethernet.control.wire.event" && r.data["adapter_id"] == a.id
                })
            {
                d.event(
                    a,
                    now,
                    "local",
                    None,
                    None,
                    None,
                    false,
                    None,
                    view.policy_epoch,
                )?;
            }
            let before = *self.generation.get(&a.id).unwrap_or(&0);
            let after = if changed {
                before
                    .checked_add(1)
                    .ok_or_else(|| fail("wire protocol generation overflow"))?
            } else {
                before
            };
            d.next.generation.insert(a.id.clone(), after);
            for r in &mut d.records {
                if r.kind == "ethernet.control.wire.event" && r.data["adapter_id"] == a.id {
                    r.data["policy_epoch_after"] = json!(epoch.to_string());
                    r.data["protocol_generation_before"] = json!(before.to_string());
                    r.data["protocol_generation_after"] = json!(after.to_string());
                    if r.data["reason"].is_null() {
                        r.data["outcome"] = json!(if changed { "changed" } else { "no_op" });
                    }
                }
            }
            let pending = d
                .next
                .timers
                .iter()
                .filter(|((id, _), (_, deadline))| id == &a.id && deadline.is_some())
                .count();
            if a.protocol.active() && pending > a.max_pending_timers {
                return Err(fail("wire timer capacity"));
            }
        }
        // Effects describe intent acceptance, never media completion. Host supplies actual FDB keys.
        for r in &mut d.records {
            if r.kind != "ethernet.control.wire.event" || !r.data["reason"].is_null() {
                continue;
            }
            let Some(a) = self
                .prepared
                .adapter(r.data["adapter_id"].as_str().unwrap())
            else {
                continue;
            };
            let mut effects = vec![];
            for (port, after) in &d.policies {
                if a.protocol == WireProtocol::Rstp
                    && a.ports.contains(port)
                    && self.policies().get(port) != Some(after)
                {
                    effects.push(json!({"kind":"port_policy","port":port,"before":self.policies().get(port),"after":after}));
                }
            }
            if a.protocol == WireProtocol::Mvrp {
                for key in old.symmetric_difference(&d.registered) {
                    if a.ports.contains(&key.0) {
                        effects.push(json!({"kind":"registration","port":key.0,"vid":key.1,"before":old.contains(key),"after":d.registered.contains(key),"generation":d.registration_generations[key].to_string()}));
                    }
                }
            }
            for t in d.timers.iter().filter(|t| t.adapter_id == a.id) {
                effects.push(json!({"kind":if t.deadline_ps.is_some(){"timer_arm"}else{"timer_cancel"},"key":t.key,"token":t.generation.to_string(),"deadline_ps":t.deadline_ps.map(|n|n.to_string())}));
            }
            for t in d.tx.iter().filter(|t| t.adapter_id == a.id) {
                effects.push(json!({"kind":"tx","packet_id":t.packet_id,"port":t.port,"ordinal":t.ordinal.to_string()}));
            }
            r.data["effects"] = json!(effects);
        }
        records::append(self, &mut d, &audit_event, now, view.policy_epoch, epoch)?;
        for r in &mut d.records {
            if r.kind == "ethernet.control.wire.event" {
                r.data["phase"] = json!(if matches!(audit_event, WireEvent::Receive { .. }) {
                    1
                } else {
                    0
                });
            }
            d.next.record_sequence = d
                .next
                .record_sequence
                .checked_add(1)
                .ok_or_else(|| fail("wire audit sequence overflow"))?;
            if r.data.get("effect_seq").is_some() {
                r.data["effect_seq"] = json!(d.next.record_sequence.to_string());
            }
        }
        Ok(d)
    }
}
impl ProtocolDelta {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)] // Preserve the host selector interface.
    pub(crate) fn multicast_candidates_with_static(
        &self,
        bridge: &str,
        vid: u16,
        source: std::net::IpAddr,
        group: std::net::IpAddr,
        eligible: &BTreeSet<String>,
        ingress: &str,
        static_view: &WireStaticMulticast,
    ) -> BTreeSet<String> {
        self.next.multicast_candidates_with_static(
            bridge,
            vid,
            source,
            group,
            eligible,
            ingress,
            static_view,
        )
    }
    #[cfg(test)]
    pub(crate) fn pending_timers(&self) -> usize {
        self.next.pending_timers()
    }
    #[cfg(test)]
    pub(crate) fn runtime_snapshot(&self) -> Value {
        self.next.runtime_snapshot()
    }
    /// Call after staged MAC effects are resolved, before host record/FES reservation.
    /// Each flush_keys row corresponds exactly to self.flush in planner order.
    pub(crate) fn complete_host_records(
        &mut self,
        now: u64,
        policy_epoch: u64,
        topology_generation: u64,
        flush_keys: Vec<Vec<crate::snapshot::wire::WireMacKey>>,
        mac: Vec<crate::snapshot::wire::WireMacEntry>,
    ) -> Result<()> {
        if flush_keys.len() != self.flush.len() {
            return Err(fail("wire flush completion count mismatch"));
        }
        for (intent, keys) in self.flush.iter().zip(flush_keys) {
            if keys.windows(2).any(|w| w[0] >= w[1])
                || keys.iter().any(|k| !intent.vids.contains(&k.vid))
            {
                return Err(fail("wire flush keys must be sorted unique and scoped"));
            }
            let protocol = if intent.reason == "mvrp_new" {
                WireProtocol::Mvrp
            } else {
                WireProtocol::Rstp
            };
            let owner = self
                .next
                .prepared
                .owner(protocol, &intent.port)
                .ok_or_else(|| fail("wire flush owner missing"))?;
            let record = self
                .records
                .iter_mut()
                .rev()
                .find(|r| {
                    r.kind == "ethernet.control.wire.event"
                        && r.data["adapter_id"] == owner.id
                        && r.data["reason"].is_null()
                })
                .ok_or_else(|| fail("wire flush lacks successful event"))?;
            record.data["effects"].as_array_mut().ok_or_else(||fail("wire event effects malformed"))?.push(json!({"kind":"mac_flush","reason":intent.reason,"port":intent.port,"vids":intent.vids,"keys":keys,"completed":true}));
        }
        if self.policy_changed || self.next.callback == 1 {
            let registered:Vec<_>=self.registered.iter().map(|key|json!({"port":key.0,"vid":key.1,"generation":self.registration_generations[key].to_string()})).collect();
            self.records.push(WireRecord{kind:"ethernet.control.wire.policy".into(),data:json!({"schema_version":1,"time_ps":now.to_string(),"policy_epoch":policy_epoch.to_string(),"topology_generation":topology_generation.to_string(),"ports":self.policies,"registered":registered,"mac":mac})});
        }
        Ok(())
    }
    fn next_transition_ordinal(&mut self) -> Result<u64> {
        let ordinal = self.transition_ordinal;
        self.transition_ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| fail("wire transition ordinal overflow"))?;
        Ok(ordinal)
    }
    fn timer(
        &mut self,
        a: &PreparedWireAdapter,
        port: &str,
        slot: &str,
        attribute: Option<u16>,
        deadline: Option<u64>,
    ) -> Result<()> {
        let key = WireTimerKey {
            protocol: protocol(a.protocol),
            bridge: a.bridge.clone(),
            port: port.into(),
            slot: slot.into(),
            attribute,
        };
        let t = self
            .next
            .timers
            .entry((a.id.clone(), key.clone()))
            .or_insert((0, None));
        if t.1 == deadline {
            return Ok(());
        }
        t.0 =
            t.0.checked_add(1)
                .ok_or_else(|| fail("wire timer generation overflow"))?;
        t.1 = deadline;
        self.timers.push(WireTimerIntent {
            adapter_id: a.id.clone(),
            key,
            generation: t.0,
            deadline_ps: deadline,
        });
        Ok(())
    }
    fn membership_wake(&mut self, a: &PreparedWireAdapter, _now: u64) -> Result<()> {
        let next = self.next.snooper.next_deadline(&a.bridge);
        let owner = self
            .next
            .prepared
            .adapters
            .iter()
            .find(|x| x.bridge == a.bridge && !x.protocol.active())
            .cloned()
            .ok_or_else(|| fail("membership owner missing"))?;
        self.timer(&owner, "", "membership", None, next)
    }
    fn tx(
        &mut self,
        a: &PreparedWireAdapter,
        port: String,
        bytes: Vec<u8>,
        cause: &str,
    ) -> Result<()> {
        let ordinal = self.next.tx_ordinals.entry(a.id.clone()).or_insert(0);
        *ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| fail("wire TX ordinal overflow"))?;
        while self
            .next
            .prepared
            .packets
            .iter()
            .any(|p| p.id == format!("@wire/tx/{}/{}", a.id, ordinal))
        {
            *ordinal = ordinal
                .checked_add(1)
                .ok_or_else(|| fail("wire TX ordinal overflow"))?;
        }
        self.tx.push(WireTxIntent {
            adapter_id: a.id.clone(),
            packet_id: format!("@wire/tx/{}/{}", a.id, ordinal),
            port,
            protocol: a.protocol,
            ordinal: *ordinal,
            bytes,
            cause_id: cause.into(),
        });
        if self.tx.iter().filter(|t| t.adapter_id == a.id).count() > a.max_pending_tx {
            return Err(fail("wire pending TX capacity"));
        }
        Ok(())
    }
    fn rstp(
        &mut self,
        a: &PreparedWireAdapter,
        now: u64,
        input: rstp::Input,
        cause: &str,
    ) -> Result<()> {
        let before = self.next.rstp[&a.id].clone();
        let d = before.plan(input, a.max_control_steps).map_err(fail)?;
        #[cfg(test)]
        self.integration_stages.push("rstp_settled".into());

        let mask: BTreeSet<u16> = d
            .state
            .ports
            .iter()
            .filter(|(_, p)| !p.oper_edge)
            .flat_map(|(name, _)| {
                self.next
                    .prepared
                    .registrar_modes
                    .iter()
                    .filter(move |((n, _), m)| {
                        n == name
                            && matches!(
                                m,
                                WireRegistrarMode::FixedIgnoreNew
                                    | WireRegistrarMode::FixedPropagateNew
                            )
                    })
                    .map(|((_, v), _)| *v)
            })
            .collect();
        for port in d.flush {
            self.flush.push(WireFlushIntent {
                reason: "rstp_topology_change".into(),
                vids: if d.state.ports[&port].oper_edge {
                    vec![]
                } else {
                    mask.iter().copied().collect()
                },
                port,
            });
        }
        for (port, m) in d.tx {
            let sa = self.next.prepared.ports[&port]
                .source_mac
                .ok_or_else(|| fail("RSTP TX identity missing"))?;
            self.tx(a, port, rstp::encode(sa, &m), cause)?;
        }
        for entry in d.transitions {
            let mut changes = transition::project_rstp_changes(
                &entry.machine,
                entry.port.as_deref(),
                &entry.before,
                &entry.after,
            )
            .map_err(fail)?;
            if let (Some(before), Some(after)) = (&entry.tree_before, &entry.tree_after) {
                changes.extend(
                    transition::project_rstp_tree_observations(&entry.machine, before, after)
                        .map_err(fail)?,
                );
            }
            let ordinal = self.next_transition_ordinal()?;
            let r = WireTransitionRecord {
                schema_version: 1,
                adapter_id: a.id.clone(),
                bridge: a.bridge.clone(),
                port: entry.port,
                protocol: "rstp".into(),
                time_ps: now.to_string(),
                callback_ordinal: self.next.callback.to_string(),
                transition_ordinal: ordinal.to_string(),
                machine: entry.machine,
                from_state: entry.from_state,
                to_state: entry.to_state,
                trigger: entry.guard,
                changes,
            };
            self.records.push(WireRecord {
                kind: "ethernet.control.wire.transition".into(),
                data: serde_json::to_value(r).map_err(|e| fail(e.to_string()))?,
            });
        }
        let after = d.state;
        self.next.rstp.insert(a.id.clone(), after.clone());
        let mrp = self
            .next
            .prepared
            .adapters
            .iter()
            .find(|m| m.protocol == WireProtocol::Mvrp && m.bridge == a.bridge)
            .cloned();
        if let Some(m) = mrp {
            if cause != "initialize" {
                let changes: Vec<_> = after
                    .ports
                    .iter()
                    .filter_map(|(name, p)| {
                        let old = &before.ports[name];
                        (old.role != p.role
                            || old.forwarding != p.forwarding
                            || old.enabled != p.enabled
                            || old.counters.detected != p.counters.detected)
                            .then(|| mvrp_state::TreeChange {
                                port: name.clone(),
                                operational: p.enabled,
                                forwarding: p.forwarding,
                                tc_detected: p.counters.detected,
                                flush: matches!(old.role, rstp::Role::Root | rstp::Role::Alternate)
                                    && p.role == rstp::Role::Designated,
                                redeclare: old.role == rstp::Role::Designated
                                    && matches!(p.role, rstp::Role::Root | rstp::Role::Alternate),
                            })
                    })
                    .collect();
                if !changes.is_empty() {
                    self.mvrp(&m, now, mvrp_state::Input::TreeBatch(changes), cause)?;
                }
            }
        }
        let next = now
            .checked_div(1_000_000_000_000)
            .and_then(|s| s.checked_add(1))
            .and_then(|s| s.checked_mul(1_000_000_000_000))
            .ok_or_else(|| fail("RSTP tick overflow"))?;
        self.timer(a, "", "tick", None, Some(next))
    }
    fn mvrp(
        &mut self,
        a: &PreparedWireAdapter,
        now: u64,
        input: mvrp_state::Input,
        cause: &str,
    ) -> Result<()> {
        let before = self.next.mvrp[&a.id].clone();
        let d = before
            .plan(&self.next.prepared, a, now, input)
            .map_err(fail)?;
        for (port, vid) in d.flush {
            self.flush.push(WireFlushIntent {
                reason: "mvrp_new".into(),
                port,
                vids: vec![vid],
            });
        }
        for (port, bytes) in d.tx {
            self.tx(a, port, bytes, cause)?;
        }
        #[cfg(test)]
        self.integration_stages.extend(d.stages);
        for tr in d.trace {
            let machines = if let Some(machine) = tr.machine.as_deref() {
                vec![(machine, "state")]
            } else {
                vec![("Applicant", "applicant"), ("Registrar", "registrar")]
            };
            for (machine, field) in machines {
                // Retain true self-loop indications; omit the other machine's no-op.
                let relevant = if tr.machine.is_some() {
                    true
                } else if machine == "Applicant" {
                    tr.before["applicant"] != tr.after["applicant"]
                        || tr.before["request_tx"] != tr.after["request_tx"]
                        || tr.before["demand"] != tr.after["demand"]
                } else {
                    tr.before["registrar"] != tr.after["registrar"]
                        || tr.before["registered"] != tr.after["registered"]
                        || tr.event == "RNew"
                };
                if !relevant {
                    continue;
                }
                let changes =
                    transition::project_mvrp_changes(machine, &tr.before, &tr.after, tr.vid)
                        .map_err(fail)?;
                let WireOptions::Mvrp(config) = &a.options else {
                    unreachable!()
                };
                let port_config = config
                    .ports
                    .iter()
                    .find(|p| p.port == tr.port)
                    .ok_or_else(|| fail("MRP trace configuration missing"))?;
                let mode = serde_json::to_value(port_config.applicant_control)
                    .map_err(|e| fail(e.to_string()))?;
                let registrar = serde_json::to_value(
                    self.next
                        .prepared
                        .registrar_modes
                        .get(&(tr.port.clone(), tr.vid.unwrap_or(0)))
                        .copied()
                        .unwrap_or(WireRegistrarMode::Normal),
                )
                .map_err(|e| fail(e.to_string()))?;
                let trigger = guards::mvrp(
                    machine,
                    &tr.event,
                    &tr.before,
                    &tr.after,
                    mode.as_str().unwrap(),
                    registrar.as_str().unwrap(),
                    true,
                    tr.room,
                );
                let ordinal = self.next_transition_ordinal()?;
                let r = json!({"schema_version":1,"adapter_id":a.id,"bridge":a.bridge,"port":tr.port,"protocol":"mvrp","time_ps":now.to_string(),"callback_ordinal":self.next.callback.to_string(),"transition_ordinal":ordinal.to_string(),"machine":machine,"from_state":tr.before[field],"to_state":tr.after[field],"trigger":trigger,"changes":changes});
                self.records.push(WireRecord {
                    kind: "ethernet.control.wire.transition".into(),
                    data: r,
                });
            }
        }
        for (name, p) in &d.state.ports {
            for ((slot, vid), timer) in &p.timers {
                let old = before.ports[name].timers.get(&(slot.clone(), *vid));
                if old != Some(timer) {
                    let key = WireTimerKey {
                        protocol: "mvrp".into(),
                        bridge: a.bridge.clone(),
                        port: name.clone(),
                        slot: slot.clone(),
                        attribute: *vid,
                    };
                    self.next.timers.insert(
                        (a.id.clone(), key.clone()),
                        (timer.generation, timer.deadline),
                    );
                    self.timers.push(WireTimerIntent {
                        adapter_id: a.id.clone(),
                        key,
                        generation: timer.generation,
                        deadline_ps: timer.deadline,
                    });
                }
            }
        }
        self.next.mvrp.insert(a.id.clone(), d.state);
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // Frozen common wire event builder.
    fn event(
        &mut self,
        a: &PreparedWireAdapter,
        now: u64,
        kind: &str,
        port: Option<&str>,
        packet: Option<(&str, &[u8])>,
        cause: Option<&str>,
        external: bool,
        reason: Option<String>,
        epoch: u64,
    ) -> Result<()> {
        let generation = *self.next.generation.get(&a.id).unwrap_or(&0);
        let stale = reason.as_deref() == Some("timer_token");
        let record = WireEventRecord {
            schema_version: 1,
            adapter_id: a.id.clone(),
            protocol: protocol(a.protocol),
            protocol_revision: a.protocol.revision().into(),
            bridge: a.bridge.clone(),
            port: port.unwrap_or("").into(),
            event_id: format!("@wire/event/{}/{}", self.next.callback, self.records.len()),
            cause_id: cause.map(str::to_owned),
            event_kind: kind.into(),
            time_ps: now.to_string(),
            phase: if kind == "receive" { 1 } else { 0 },
            batch_ordinal: self.records.len().to_string(),
            packet_id: packet.map(|p| p.0.into()),
            bytes_hex: packet.map(|p| codec::hex(p.1)),
            mac_bytes: packet.map(|p| p.1.len() as u32),
            outcome: if stale {
                "stale"
            } else if reason.is_some() {
                "control_drop"
            } else {
                "changed"
            }
            .into(),
            reason,
            policy_epoch_before: epoch.to_string(),
            policy_epoch_after: epoch.to_string(),
            protocol_generation_before: generation.saturating_sub(1).to_string(),
            protocol_generation_after: generation.to_string(),
            timer: None::<WireTimerRecord>,
            effects: vec![],
            external_injection: external,
        };
        self.records.push(WireRecord {
            kind: "ethernet.control.wire.event".into(),
            data: serde_json::to_value(record).map_err(|e| fail(e.to_string()))?,
        });
        Ok(())
    }
}
#[cfg(test)]
#[path = "wire/tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
impl WireState {
    pub(crate) fn vector_seed_order_case(&mut self, adapter: &str, target: &str) {
        let bridge = self
            .prepared
            .adapters
            .iter()
            .find(|a| a.id == adapter)
            .unwrap()
            .bridge
            .clone();
        let rstp = self.rstp.get_mut(adapter).unwrap();
        rstp.prs = rstp::Prs::ROLE_SELECTION;
        for (name, p) in &mut rstp.ports {
            p.enabled = true;
            p.info = rstp::Info::Mine;
            p.pim = rstp::Pim::CURRENT;
            p.prt = rstp::Prt::DESIGNATED_PORT;
            p.role = rstp::Role::Designated;
            p.selected_role = rstp::Role::Designated;
            p.selected = true;
            p.reselect = false;
            p.update = false;
            p.ppm = rstp::Ppm::SENSING;
            p.bdm = rstp::Bdm::NOT_EDGE;
            p.oper_edge = false;
            p.config.auto_edge = false;
            p.config.admin_edge = false;
            p.sync = false;
            p.synced = true;
            p.reroot = false;
            p.learn = true;
            p.forward = true;
            p.learning = true;
            p.forwarding = true;
            p.agree = true;
            p.agreed = true;
            p.proposing = true;
            p.proposed = false;
            p.disputed = false;
            p.tcm = rstp::Tcm::ACTIVE;
            p.counters.edge = 3;
            p.counters.hello = 2;
            p.counters.rr = 0;
            p.counters.fd = 0;
            p.counters.detected = 3;
            p.fdb_flush = false;
            if name == target {
                p.info = rstp::Info::Aged;
                p.pim = rstp::Pim::AGED;
                p.role = rstp::Role::Root;
                p.selected_role = rstp::Role::Root;
                p.prt = rstp::Prt::ROOT_PORT;
                p.reselect = true;
                p.selected = false;
                p.learn = false;
                p.forward = false;
                p.learning = false;
                p.forwarding = false;
                p.tcm = rstp::Tcm::LEARNING;
                p.counters.rr = 15;
            }
        }
        for a in self
            .prepared
            .adapters
            .iter()
            .filter(|a| a.protocol == WireProtocol::Mvrp && a.bridge == bridge)
        {
            let mrp = self.mvrp.get_mut(&a.id).unwrap();
            let source = a
                .ports
                .iter()
                .find(|p| p.as_str() != target)
                .unwrap()
                .clone();
            for (name, p) in &mut mrp.ports {
                if name == &source {
                    let x = p.attributes.get_mut(&100).unwrap();
                    x.registered = true;
                    x.registrar = mvrp::Registrar::IN;
                    self.registration_generations.insert((name.clone(), 100), 1);
                }
                p.operational = true;
                p.forwarding = name != target;
                p.tc_detected = 3;
                if name == target {
                    for x in p.attributes.values_mut() {
                        x.demand = false;
                        x.applicant = mvrp::Applicant::VO;
                        x.request_tx = false;
                    }
                }
            }
        }
    }
}
