//! Transactional protocol coordinator. The network host owns FES, media and commit.
#[path = "wire/boundary.rs"]
pub(crate) mod boundary;
#[path = "wire/codec.rs"]
pub(crate) mod codec;
#[path = "wire/component_envelope.rs"]
pub(crate) mod component_envelope;
#[path = "wire/guards.rs"]
pub(crate) mod guards;
#[path = "wire/membership.rs"]
pub(crate) mod membership;
#[path = "wire/mstp.rs"]
pub(crate) mod mstp;
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
fn c_bridge_mac(id: &u64) -> [u8; 6] {
    id.to_be_bytes()[2..].try_into().unwrap()
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
    /// MAC-service capability from the physical owner. Missing preserves legacy point-to-point behavior.
    pub oper_point_to_point_mac: BTreeMap<String, bool>,
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    pub tree_policies: BTreeMap<(String, u16, String), WirePortPolicy>,
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
    mstp: BTreeMap<String, mstp::MstBridge>,
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
    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }
    pub(crate) fn new(prepared: Arc<PreparedWire>) -> Self {
        let mut bridges = BTreeMap::new();
        let mut mst_bridges = BTreeMap::new();
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
                    let times = rstp::Times {
                        age: 0,
                        max_age: c.max_age,
                        hello: 512,
                        forward_delay: c.forward_delay,
                    };
                    let bridge = if let Some(binding) = &a.component_binding {
                        if binding.domain == TreeDomain::Customer {
                            let cep = binding
                                .ports
                                .iter()
                                .find_map(|(name, attachment)| {
                                    matches!(attachment, PortAttachment::Physical { .. })
                                        .then(|| name.clone())
                                })
                                .expect("prepared C component CEP");
                            rstp::Bridge::new_cep(
                                c.bridge_id,
                                times,
                                c.tx_hold_count,
                                ports,
                                cep,
                                binding.service_cardinality.clone(),
                            )
                            .expect("prepared enhanced CEP RSTP")
                        } else {
                            rstp::Bridge::new(c.bridge_id, times, c.tx_hold_count, ports)
                        }
                    } else {
                        rstp::Bridge::new(c.bridge_id, times, c.tx_hold_count, ports)
                    };
                    bridges.insert(a.id.clone(), bridge);
                }
                WireOptions::Mvrp(_) => {
                    mrp.insert(a.id.clone(), mvrp_state::State::new(a));
                }
                WireOptions::Mstp(c) => {
                    let vlan_map: &[u16; 4096] = c
                        .vlan_map
                        .as_slice()
                        .try_into()
                        .expect("prepared MSTP VLAN map length");
                    let region =
                        mstp::RegionId::from_vlan_map(&c.region_name, c.region_revision, vlan_map)
                            .expect("prepared MSTP region");
                    let instances = c
                        .instances
                        .iter()
                        .map(|i| mstp::InstanceConfig {
                            mstid: mstp::Mstid(i.mstid),
                            bridge_priority: i.bridge_priority,
                        })
                        .collect();
                    let ports = c
                        .ports
                        .iter()
                        .map(|p| {
                            (
                                p.port.clone(),
                                mstp::MstPortConfig {
                                    id: p.port_id,
                                    external_cost: p.external_path_cost,
                                    internal_cost: p.internal_path_cost,
                                    enabled: true,
                                    oper_point_to_point_mac: true,
                                    admin_edge: p.admin_edge,
                                },
                            )
                        })
                        .collect();
                    let mut bridge = mstp::MstBridge::new(
                        c.bridge_id,
                        region,
                        c.vlan_map.clone(),
                        instances,
                        ports,
                        rstp::Times {
                            age: 0,
                            max_age: c.max_age,
                            hello: 512,
                            forward_delay: c.forward_delay,
                        },
                        c.tx_hold_count,
                    )
                    .expect("prepared MSTP bridge");
                    bridge.max_hops = c.max_hops;
                    for tree in bridge.trees.values_mut() {
                        for port in tree.ports.values_mut() {
                            port.remaining_hops = c.max_hops;
                        }
                    }
                    mst_bridges.insert(a.id.clone(), bridge);
                }
                _ => {}
            }
        }
        Self {
            prepared,
            rstp: bridges,
            mstp: mst_bridges,
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
        json!({"initialized":self.initialized,"callback":self.callback.to_string(),"record_sequence":self.record_sequence.to_string(),"rstp":self.rstp,"mstp":self.mstp,"mvrp":mvrp,"leases":self.snooper.leases.iter().map(|((key,filter),lease)|json!({"key":key,"filter":filter,"deadline_ps":lease.deadline.to_string(),"generation":lease.generation.to_string()})).collect::<Vec<_>>(),"routers":self.snooper.routers.iter().map(|(key,lease)|json!({"key":key,"deadline_ps":lease.deadline.to_string(),"generation":lease.generation.to_string()})).collect::<Vec<_>>(),"lease_generations":self.snooper.generations.iter().map(|((key,filter),generation)|json!({"key":key,"filter":filter,"generation":generation.to_string()})).collect::<Vec<_>>(),"router_generations":self.snooper.router_generations.iter().map(|(key,generation)|json!({"key":key,"generation":generation.to_string()})).collect::<Vec<_>>(),"compatibility_flood":self.snooper.flood,"timer_ledger":self.timers.iter().map(|((adapter,key),(generation,deadline))|json!({"adapter_id":adapter,"key":key,"generation":generation.to_string(),"deadline_ps":deadline.map(|n|n.to_string())})).collect::<Vec<_>>(),"generations":self.generation,"tx_ordinals":self.tx_ordinals,"registration_generations":self.registration_generations.iter().map(|((port,vid),generation)|json!({"port":port,"vid":vid,"generation":generation.to_string()})).collect::<Vec<_>>()})
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
            .chain(self.mstp.values().flat_map(|b| {
                b.trees[&mstp::Mstid(0)].ports.iter().map(|(n, p)| {
                    (
                        n.clone(),
                        WirePortPolicy {
                            role: format!("{:?}", p.role),
                            learning: p.learning,
                            forwarding: p.forwarding,
                            link_up: b.ports[n].config.enabled,
                        },
                    )
                })
            }))
            .collect()
    }
    pub(crate) fn tree_policies(&self) -> BTreeMap<(String, u16, String), WirePortPolicy> {
        self.mstp
            .iter()
            .flat_map(|(adapter, b)| {
                b.trees.iter().flat_map(move |(id, tree)| {
                    tree.ports.iter().map(move |(name, p)| {
                        (
                            (adapter.clone(), id.0, name.clone()),
                            WirePortPolicy {
                                role: format!("{:?}", p.role),
                                learning: p.learning,
                                forwarding: p.forwarding,
                                link_up: b.ports[name].config.enabled,
                            },
                        )
                    })
                })
            })
            .collect()
    }
    pub(crate) fn tree_policy(&self, port: &str, vid: u16) -> Option<WirePortPolicy> {
        let (_, b) = self.mstp.iter().find(|(_, b)| b.ports.contains_key(port))?;
        let mstid = b.mstid_for_vid(vid)?;
        let state = b.trees.get(&mstid)?.ports.get(port)?;
        Some(WirePortPolicy {
            role: format!("{:?}", state.role),
            learning: state.learning,
            forwarding: state.forwarding,
            link_up: b.ports[port].config.enabled,
        })
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
            tree_policies: BTreeMap::new(),
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
                            p.oper_point_to_point_mac = a
                                .component_binding
                                .as_ref()
                                .and_then(|binding| binding.service_cardinality.get(n))
                                .map(|count| *count <= 2)
                                .unwrap_or_else(|| {
                                    view.oper_point_to_point_mac.get(n).copied().unwrap_or(true)
                                });
                        }
                        d.rstp(a, now, rstp::Input::Initialize, "initialize")?;
                    } else if a.protocol == WireProtocol::Mstp {
                        for (n, p) in &mut d.next.mstp.get_mut(&a.id).unwrap().ports {
                            p.config.enabled = view.link_up.get(n).copied().unwrap_or(true);
                            p.config.oper_point_to_point_mac =
                                view.oper_point_to_point_mac.get(n).copied().unwrap_or(true);
                        }
                        d.mstp(a, now, mstp::MstInput::Initialize, "initialize")?;
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
                        **p == ingress_port
                            || d.next
                                .prepared
                                .ports
                                .get(*p)
                                .is_some_and(|physical| physical.ingress == ingress_port)
                    })
                    .unwrap()
                    .clone();
                let reason = if !view.link_up.get(&port).copied().unwrap_or(true) {
                    Some("link_down".to_string())
                } else {
                    match &a.options {
                        WireOptions::Rstp(c) => {
                            let pc = c.ports.iter().find(|p| p.port == port).unwrap();
                            let decoded = if let Some(binding) = &a.component_binding {
                                component_envelope::decode(&bytes, a.max_pdu_bytes).and_then(
                                    |env| {
                                        if env.domain != binding.domain || !env.tags.is_empty() {
                                            return Err(codec::bad("protocol_malformed"));
                                        }
                                        rstp::decode_llc(env.llc, pc.port_id)
                                    },
                                )
                            } else {
                                rstp::decode(&bytes, pc.port_id, a.max_pdu_bytes)
                            };
                            match decoded {
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
                        WireOptions::Mstp(c) => {
                            let pc = c.ports.iter().find(|p| p.port == port).unwrap();
                            let decoded = if let Some(binding) = &a.component_binding {
                                component_envelope::decode(&bytes, a.max_pdu_bytes).and_then(
                                    |env| {
                                        if env.domain != binding.domain || !env.tags.is_empty() {
                                            return Err(codec::bad("protocol_malformed"));
                                        }
                                        mstp::decode_llc(env.llc, pc.port_id)
                                    },
                                )
                            } else {
                                mstp::decode(&bytes, pc.port_id, a.max_pdu_bytes)
                            };
                            match decoded {
                                Err(e) => Some(match e.0.as_str() {
                                    "frame_bounds" | "fcs" => e.0,
                                    _ => "protocol_malformed".into(),
                                }),
                                Ok(m) => {
                                    d.mstp(
                                        &a,
                                        now,
                                        mstp::MstInput::Receive(port.clone(), m),
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
                        WireProtocol::Mstp => d.mstp(&a, now, mstp::MstInput::Tick, "tick")?,
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
                let paired_rstp = adapters
                    .iter()
                    .any(|a| matches!(a.protocol, WireProtocol::Rstp | WireProtocol::Mstp));
                for a in adapters {
                    match a.protocol {
                        WireProtocol::Rstp => {
                            d.rstp(&a, now, rstp::Input::Link(port.clone(), up), "link")?
                        }
                        WireProtocol::Mstp => {
                            d.mstp(&a, now, mstp::MstInput::Link(port.clone(), up), "link")?
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
        d.tree_policies = d.next.tree_policies();
        d.registered = d.next.registered();
        d.memberships = d.next.snooper.projection();
        d.router_ports = d.next.snooper.routers.keys().cloned().collect();
        d.compatibility_flood = d.next.snooper.flood.clone();
        d.topology_changed =
            d.policies != self.policies() || d.tree_policies != self.tree_policies();
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
                    WireProtocol::Mstp => self.mstp.get(&a.id) != d.next.mstp.get(&a.id),
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
                if matches!(a.protocol, WireProtocol::Rstp | WireProtocol::Mstp)
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
            } else if intent.reason == "mstp_topology_change" {
                WireProtocol::Mstp
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
            for ((adapter_id, mstid, port), policy) in &self.tree_policies {
                let bridge = &self.next.mstp[adapter_id];
                let tree = &bridge.trees[&mstp::Mstid(*mstid)];
                let region_name = String::from_utf8_lossy(&bridge.region.name)
                    .trim_end_matches('\0')
                    .to_string();
                let mapped_vids: Vec<_> = (1..=4094)
                    .filter(|vid| bridge.vlan_map[*vid] == *mstid)
                    .collect();
                self.records.push(WireRecord {
                    kind: "ethernet.control.wire.tree_policy".into(),
                    data: json!({"schema_version":1,"time_ps":now.to_string(),
                        "policy_epoch":policy_epoch.to_string(),"component_id":adapter_id,
                        "bridge":self.next.prepared.adapter(adapter_id).unwrap().bridge,
                        "mstid":mstid,"mapped_vids":mapped_vids,"port":port,
                        "policy":policy,"boundary":bridge.ports[port].boundary,
                        "region_name":region_name,"region_revision":bridge.region.revision,
                        "region_digest_hex":codec::hex(&bridge.region.digest),
                        "root_port":tree.root_port}),
                });
            }
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
            let bytes = if let Some(binding) = &a.component_binding {
                let sa = if binding.domain == TreeDomain::Customer {
                    d.state
                        .cep_source_mac()
                        .ok_or_else(|| fail("CEP MAC identity missing"))?
                } else if matches!(
                    binding.ports.get(&port),
                    Some(PortAttachment::Physical { .. })
                ) {
                    self.next
                        .prepared
                        .ports
                        .get(&port)
                        .and_then(|p| p.source_mac)
                        .ok_or_else(|| fail("provider physical TX identity missing"))?
                } else {
                    c_bridge_mac(&d.state.id)
                };
                component_envelope::encode_local(binding.domain, sa, &rstp::encode_llc(&m))
                    .map_err(|e| fail(format!("component RSTP envelope: {}", e.0)))?
            } else {
                let sa = self.next.prepared.ports[&port]
                    .source_mac
                    .ok_or_else(|| fail("RSTP TX identity missing"))?;
                rstp::encode(sa, &m)
            };
            self.tx(a, port, bytes, cause)?;
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
    fn mstp(
        &mut self,
        a: &PreparedWireAdapter,
        now: u64,
        input: mstp::MstInput,
        cause: &str,
    ) -> Result<()> {
        let before = self.next.mstp[&a.id].clone();
        let d = before.plan(input, a.max_control_steps).map_err(fail)?;
        #[cfg(test)]
        self.integration_stages.push("mstp_settled".into());
        for (mstid, port) in &d.flush {
            let vids = (1..=4094)
                .filter(|vid| d.state.vlan_map[*vid] == mstid.0)
                .map(|vid| vid as u16)
                .collect();
            self.flush.push(WireFlushIntent {
                reason: "mstp_topology_change".into(),
                port: port.clone(),
                vids,
            });
        }
        for (port, packet) in d.tx {
            let sa = self
                .next
                .prepared
                .ports
                .get(&port)
                .and_then(|p| p.source_mac)
                .or_else(|| {
                    a.component_binding
                        .as_ref()
                        .map(|_| c_bridge_mac(&d.state.id))
                })
                .ok_or_else(|| fail("MSTP TX identity missing"))?;
            let bytes = if let Some(binding) = &a.component_binding {
                let llc = match packet {
                    mstp::MstTx::Legacy(m) => rstp::encode_llc(&m),
                    mstp::MstTx::Mst(m) => mstp::encode_llc(&m).map_err(fail)?,
                };
                component_envelope::encode_local(binding.domain, sa, &llc)
                    .map_err(|e| fail(format!("component MSTP envelope: {}", e.0)))?
            } else {
                match packet {
                    mstp::MstTx::Legacy(m) => rstp::encode(sa, &m),
                    mstp::MstTx::Mst(m) => mstp::encode(sa, &m).map_err(fail)?,
                }
            };
            self.tx(a, port, bytes, cause)?;
        }
        let after = d.state;
        for (id, tree) in &after.trees {
            for (name, p) in &tree.ports {
                let old = &before.trees[id].ports[name];
                if old == p {
                    continue;
                }
                let mut changes = BTreeMap::new();
                changes.insert("mstid".into(), json!(id.0));
                changes.insert("boundary".into(), json!(after.ports[name].boundary));
                changes.insert(
                    "region_digest".into(),
                    json!(codec::hex(&after.region.digest)),
                );
                changes.insert(
                    "learning".into(),
                    json!({"before":old.learning,"after":p.learning}),
                );
                changes.insert(
                    "forwarding".into(),
                    json!({"before":old.forwarding,"after":p.forwarding}),
                );
                changes.insert(
                    "remaining_hops".into(),
                    json!({"before":old.remaining_hops,"after":p.remaining_hops}),
                );
                let ordinal = self.next_transition_ordinal()?;
                let record = WireTransitionRecord {
                    schema_version: 1,
                    adapter_id: a.id.clone(),
                    bridge: a.bridge.clone(),
                    port: Some(name.clone()),
                    protocol: "mstp".into(),
                    time_ps: now.to_string(),
                    callback_ordinal: self.next.callback.to_string(),
                    transition_ordinal: ordinal.to_string(),
                    machine: "MSTI_PORT".into(),
                    from_state: format!("{:?}", old.role),
                    to_state: format!("{:?}", p.role),
                    trigger: cause.into(),
                    changes,
                };
                self.records.push(WireRecord {
                    kind: "ethernet.control.wire.transition".into(),
                    data: serde_json::to_value(record).map_err(|e| fail(e.to_string()))?,
                });
            }
        }
        self.next.mstp.insert(a.id.clone(), after.clone());
        if let Some(m) = self
            .next
            .prepared
            .adapters
            .iter()
            .find(|m| m.protocol == WireProtocol::Mvrp && m.bridge == a.bridge)
            .cloned()
        {
            if cause != "initialize" {
                let changes = after.trees[&mstp::Mstid(0)]
                    .ports
                    .iter()
                    .filter_map(|(name, p)| {
                        let old = &before.trees[&mstp::Mstid(0)].ports[name];
                        (old.role != p.role
                            || old.forwarding != p.forwarding
                            || before.ports[name].config.enabled
                                != after.ports[name].config.enabled)
                            .then(|| mvrp_state::TreeChange {
                                port: name.clone(),
                                operational: after.ports[name].config.enabled,
                                forwarding: p.forwarding,
                                tc_detected: u32::from(p.topology_change) * 3,
                                flush: old.forwarding && !p.forwarding,
                                redeclare: old.role == mstp::Role::Designated && p.role != old.role,
                            })
                    })
                    .collect();
                self.mvrp(&m, now, mvrp_state::Input::TreeBatch(changes), cause)?;
            }
        }
        let next = now
            .checked_div(1_000_000_000_000)
            .and_then(|s| s.checked_add(1))
            .and_then(|s| s.checked_mul(1_000_000_000_000))
            .ok_or_else(|| fail("MSTP tick overflow"))?;
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
#[path = "wire/mstp_adapter_tests.rs"]
mod mstp_adapter_tests;
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

crate::runtime::network::checkpoint::fields!(WireState, value; rstp, mstp, mvrp, snooper, generation, tx_ordinals, callback, record_sequence, initialized; timers, registration_generations);
impl WireState {
    pub(crate) fn checkpoint_prepared(&self) -> &PreparedWire {
        &self.prepared
    }
    pub(crate) fn checkpoint_adapter(&self, id: &str) -> Option<&PreparedWireAdapter> {
        self.prepared.adapter(id)
    }
    pub(crate) fn checkpoint_timer(
        &self,
        adapter: &str,
        key: &WireTimerKey,
        generation: u64,
        deadline: u64,
    ) -> bool {
        self.timers.get(&(adapter.into(), key.clone())) == Some(&(generation, Some(deadline)))
    }
    pub(crate) fn validate_checkpoint(&self, horizon: u64) -> Result<()> {
        let initial = Self::new(self.prepared.clone());
        if self.rstp.keys().ne(initial.rstp.keys())
            || self.mstp.keys().ne(initial.mstp.keys())
            || self.mvrp.keys().ne(initial.mvrp.keys())
            || self
                .generation
                .keys()
                .any(|id| self.prepared.adapter(id).is_none())
        {
            return Err(fail("wire checkpoint adapter catalog mismatch"));
        }
        for (id, bridge) in &self.rstp {
            if bridge.ports.keys().ne(initial.rstp[id].ports.keys())
                || bridge.id != initial.rstp[id].id
                || bridge
                    .ports
                    .iter()
                    .any(|(name, p)| p.config != initial.rstp[id].ports[name].config)
            {
                return Err(fail("wire checkpoint RSTP port catalog mismatch"));
            }
        }
        for (id, bridge) in &self.mstp {
            let original = &initial.mstp[id];
            if bridge.id != original.id
                || bridge.region != original.region
                || bridge.vlan_map != original.vlan_map
                || bridge.times != original.times
                || bridge.max_hops != original.max_hops
                || bridge.tx_hold != original.tx_hold
                || bridge.root_hops == 0
                || bridge.root_times.hello != bridge.times.hello
                || bridge.trees.keys().ne(original.trees.keys())
                || bridge.ports.keys().ne(original.ports.keys())
                || bridge.trees.iter().any(|(mstid, tree)| {
                    tree.bridge_id != original.trees[mstid].bridge_id
                        || tree.ports.keys().ne(original.ports.keys())
                        || tree
                            .root_port
                            .as_ref()
                            .is_some_and(|p| !bridge.ports.contains_key(p))
                })
                || bridge.ports.iter().any(|(name, p)| {
                    p.config.id != original.ports[name].config.id
                        || p.config.external_cost != original.ports[name].config.external_cost
                        || p.config.internal_cost != original.ports[name].config.internal_cost
                        || p.config.admin_edge != original.ports[name].config.admin_edge
                        || p.migration_seconds > 3
                        || p.migration_phase == mstp::MigrationPhase::SelectingStp && p.send_mstp
                        || p.neighbors.iter().any(|(key, n)| {
                            *key != (n.cist.designated_bridge, n.cist.designated_port)
                                || n.info_seconds == 0
                                || n.info_seconds
                                    > 3 * ((bridge.times.hello as u32 + 255) / 256).max(1)
                                || n.age_seconds
                                    >= 3 * ((bridge.times.hello as u32 + 255) / 256).max(1)
                                || if n.region.as_ref() == Some(&bridge.region) {
                                    n.remaining_hops <= 1
                                } else {
                                    ((n.times.age as u32 + 384) / 256) * 256
                                        > n.times.max_age as u32
                                }
                                || n.msti.iter().any(|(mstid, msg)| {
                                    mstid != &msg.mstid
                                        || !bridge.trees.contains_key(mstid)
                                        || msg.remaining_hops <= 1
                                })
                        })
                })
            {
                return Err(fail("wire checkpoint MSTP catalog mismatch"));
            }
        }
        for (id, mvrp) in &self.mvrp {
            if mvrp.ports.keys().ne(initial.mvrp[id].ports.keys())
                || mvrp.ports.values().any(|p| {
                    p.attributes.keys().any(|vid| *vid == 0 || *vid > 4094)
                        || p.last_opportunity.is_some_and(|t| t > horizon)
                })
            {
                return Err(fail("wire checkpoint MVRP port catalog mismatch"));
            }
        }
        for ((adapter, key), (_, deadline)) in &self.timers {
            let config = self
                .prepared
                .adapter(adapter)
                .ok_or_else(|| fail("wire checkpoint timer adapter"))?;
            if (key.port.is_empty() && !matches!(key.slot.as_str(), "tick" | "membership"))
                || (!key.port.is_empty() && !config.ports.contains(&key.port))
                || key.bridge != config.bridge
                || key.protocol != protocol(config.protocol)
                || key.attribute.is_some_and(|v| v == 0 || v > 4094)
                || deadline.is_some_and(|d| d < horizon)
            {
                return Err(fail("wire checkpoint timer reference/deadline"));
            }
        }
        Ok(())
    }
}
impl WireState {
    pub(crate) fn checkpoint_timers(&self) -> BTreeMap<(String, WireTimerKey), (u64, u64)> {
        self.timers
            .iter()
            .filter_map(|(key, (generation, deadline))| {
                deadline.map(|d| (key.clone(), (*generation, d)))
            })
            .collect()
    }
}
