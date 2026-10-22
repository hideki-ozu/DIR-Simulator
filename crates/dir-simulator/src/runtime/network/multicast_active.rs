//! Active IGMPv3 and MLDv2 actors. All deadlines are global FES times;
//! transmission is an intent consumed by the network's real control FIFO.
use crate::runtime::wire::{codec, membership};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Filter {
    pub exclude: bool,
    pub sources: BTreeSet<IpAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Interface {
    pub id: String,
    pub output_port: String,
    pub ingress_port: String,
    pub vid: u16,
    pub address: IpAddr,
    pub mac: [u8; 6],
    pub router: bool,
    pub router_version: u8,
    pub robustness: u8,
    pub query_interval_ps: u64,
    pub query_response_ps: u64,
    pub last_member_interval_ps: u64,
    pub unsolicited_interval_ps: u64,
    pub max_groups: usize,
    pub max_sources: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Change {
    pub at_ps: u64,
    pub interface: String,
    pub socket: String,
    pub group: IpAddr,
    pub filter: Option<Filter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    pub interfaces: Vec<Interface>,
    pub bridge_ports: Vec<BridgePort>,
    pub changes: Vec<Change>,
    pub random_seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BridgePort {
    pub bridge: String,
    pub output_port: String,
    pub ingress_port: String,
    pub vid: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Event {
    Start,
    Change(usize),
    Receive {
        ingress_port: String,
        vid: u16,
        bytes: Vec<u8>,
    },
    Timer,
}

#[derive(Debug, Clone)]
pub(crate) struct Transmission {
    pub interface: String,
    pub output_port: String,
    pub vid: u16,
    pub source_mac: [u8; 6],
    pub destination_mac: [u8; 6],
    pub ip_bytes: Vec<u8>,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Effects {
    pub tx: Vec<Transmission>,
    pub next_deadline: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pending {
    records: Vec<membership::Record>,
    remaining: u8,
    deadline: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Response {
    deadline: u64,
    // None means a General or Group Specific Query; Some means source specific.
    sources: Option<BTreeSet<IpAddr>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Host {
    #[serde(with = "super::checkpoint::map")]
    sockets: BTreeMap<(String, IpAddr), Filter>,
    filters: BTreeMap<IpAddr, Filter>,
    pending: BTreeMap<IpAddr, Pending>,
    responses: BTreeMap<IpAddr, Response>,
    older_v1_until: Option<u64>,
    older_v2_until: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RouterGroup {
    exclude_until: Option<u64>,
    sources: BTreeMap<IpAddr, u64>,
    blocked: BTreeSet<IpAddr>,
    older_v1_until: Option<u64>,
    older_v2_until: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Router {
    groups: BTreeMap<IpAddr, RouterGroup>,
    #[serde(default)]
    learned_robustness: Option<u8>,
    #[serde(default)]
    learned_query_interval_ps: Option<u64>,
    other_querier_until: Option<u64>,
    other_querier: Option<IpAddr>,
    next_query: Option<u64>,
    startup_left: u8,
    // (group, sources) to query again, remaining transmissions, deadline.
    pending_queries: Vec<(IpAddr, BTreeSet<IpAddr>, u8, u64)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SnoopedGroup {
    known_until: Option<u64>,
    exclude_until: Option<u64>,
    sources: BTreeMap<IpAddr, u64>,
    blocked: BTreeSet<IpAddr>,
    older_until: Option<u64>,
    #[serde(default)]
    older_v1_until: Option<u64>,
}

#[derive(Default)]
struct GroupQueries {
    sources: BTreeSet<IpAddr>,
    group: bool,
}

// RFC 9776, Sections 6.4.1 and 6.4.2, Tables 8 and 9. X is the set of
// positive source timers and Y is the zero-timer blocked set. The same
// transition governs router interfaces and each downstream snooping port.
fn apply_group_record(
    exclude_until: &mut Option<u64>,
    sources: &mut BTreeMap<IpAddr, u64>,
    blocked: &mut BTreeSet<IpAddr>,
    record: &membership::Record,
    gmi_deadline: u64,
) -> GroupQueries {
    let a = &record.sources;
    let x: BTreeSet<_> = sources.keys().copied().collect();
    let y = blocked.clone();
    let exclude = exclude_until.is_some();
    let mut queries = GroupQueries::default();
    match (exclude, record.kind) {
        // IS_IN and ALLOW refresh the reported sources.
        (false, 1 | 5) | (true, 1 | 5) => {
            for source in a {
                sources.insert(*source, gmi_deadline);
                blocked.remove(source);
            }
        }
        // INCLUDE(A) + IS_EX/TO_EX(B) -> EXCLUDE(A*B, B-A).
        (false, 2 | 4) => {
            if record.kind == 4 {
                queries.sources = x.intersection(a).copied().collect();
            }
            sources.retain(|source, _| a.contains(source));
            *blocked = a.difference(&x).copied().collect();
            *exclude_until = Some(gmi_deadline);
        }
        // EXCLUDE(X,Y) + IS_EX/TO_EX(A) -> EXCLUDE(A-Y, Y*A).
        (true, 2 | 4) => {
            if record.kind == 4 {
                queries.sources = a.difference(&y).copied().collect();
            }
            sources.retain(|source, _| a.contains(source) && !y.contains(source));
            for source in a.difference(&y) {
                let deadline = if record.kind == 2 {
                    gmi_deadline
                } else {
                    exclude_until.unwrap()
                };
                sources.entry(*source).or_insert(deadline);
            }
            *blocked = y.intersection(a).copied().collect();
            *exclude_until = Some(gmi_deadline);
        }
        // TO_IN retains prior interest during the group/source queries.
        (false, 3) => {
            queries.sources = x.difference(a).copied().collect();
            for source in a {
                sources.insert(*source, gmi_deadline);
            }
        }
        (true, 3) => {
            queries.sources = x.difference(a).copied().collect();
            queries.group = true;
            for source in a {
                sources.insert(*source, gmi_deadline);
                blocked.remove(source);
            }
        }
        (false, 6) => {
            queries.sources = x.intersection(a).copied().collect();
        }
        (true, 6) => {
            queries.sources = a.difference(&y).copied().collect();
            for source in a.difference(&y) {
                sources.entry(*source).or_insert(exclude_until.unwrap());
            }
        }
        _ => {}
    }
    queries
}

fn expire_group(
    exclude_until: &mut Option<u64>,
    sources: &mut BTreeMap<IpAddr, u64>,
    blocked: &mut BTreeSet<IpAddr>,
    now: u64,
) {
    let expired: Vec<_> = sources
        .iter()
        .filter(|(_, deadline)| **deadline <= now)
        .map(|(source, _)| *source)
        .collect();
    for source in expired {
        sources.remove(&source);
        if exclude_until.is_some_and(|deadline| deadline > now) {
            blocked.insert(source);
        }
    }
    if exclude_until.is_some_and(|deadline| deadline <= now) {
        *exclude_until = None;
        blocked.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Actor {
    hosts: Vec<Host>,
    routers: Vec<Router>,
    #[serde(with = "super::checkpoint::map")]
    snooped: BTreeMap<(String, u16, IpAddr, String), SnoopedGroup>,
    #[serde(with = "super::checkpoint::map")]
    router_ports: BTreeMap<(String, u16, bool, String), u64>,
    #[serde(with = "super::checkpoint::map")]
    older_flood: BTreeMap<(String, u16, bool), u64>,
    #[serde(default, with = "super::checkpoint::map")]
    snoop_query_parameters: BTreeMap<(String, u16, bool), (u8, u64)>,
    rng: u64,
    started: bool,
}

impl Actor {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            hosts: vec![Host::default(); config.interfaces.len()],
            routers: vec![Router::default(); config.interfaces.len()],
            snooped: BTreeMap::new(),
            router_ports: BTreeMap::new(),
            older_flood: BTreeMap::new(),
            snoop_query_parameters: BTreeMap::new(),
            rng: config.random_seed.max(1),
            started: false,
        }
    }

    pub(crate) fn validate(&self, config: &Config, now: u64) -> Result<(), String> {
        if self.hosts.len() != config.interfaces.len()
            || self.routers.len() != config.interfaces.len()
            || self.rng == 0
        {
            return Err("active multicast checkpoint dimensions/random state".into());
        }
        for (i, iface) in config.interfaces.iter().enumerate() {
            let host = &self.hosts[i];
            for ((_, group), filter) in &host.sockets {
                if !group.is_multicast()
                    || group.is_ipv6() != iface.address.is_ipv6()
                    || !filter_valid(filter, iface.address.is_ipv6())
                {
                    return Err("active multicast checkpoint socket filter".into());
                }
            }
            for (group, filter) in &host.filters {
                if !group.is_multicast()
                    || group.is_ipv6() != iface.address.is_ipv6()
                    || !filter_valid(filter, iface.address.is_ipv6())
                    || *filter
                        != aggregate(
                            host.sockets
                                .iter()
                                .filter(|((_, g), _)| g == group)
                                .map(|(_, f)| f),
                        )
                {
                    return Err("active multicast checkpoint interface filter".into());
                }
            }
            if host.filters.len() > iface.max_groups
                || host
                    .filters
                    .values()
                    .map(|f| f.sources.len())
                    .sum::<usize>()
                    > iface.max_sources
            {
                return Err("active multicast checkpoint host capacity".into());
            }
            for (group, pending) in &host.pending {
                if group.is_ipv6() != iface.address.is_ipv6()
                    || pending.deadline < now
                    || pending.remaining == 0
                    || pending.remaining >= iface.robustness
                    || pending.records.iter().any(|r| {
                        r.group != *group
                            || r.sources.iter().any(|s| s.is_ipv6() != group.is_ipv6())
                    })
                {
                    return Err("active multicast checkpoint retransmission".into());
                }
            }
            for (group, response) in &host.responses {
                if !host.filters.contains_key(group)
                    || response.deadline < now
                    || response.sources.as_ref().is_some_and(|sources| {
                        sources.iter().any(|s| s.is_ipv6() != group.is_ipv6())
                    })
                {
                    return Err("active multicast checkpoint query response".into());
                }
            }
            let router = &self.routers[i];
            if !iface.router
                && (router.next_query.is_some()
                    || router.other_querier_until.is_some()
                    || router.learned_robustness.is_some()
                    || router.learned_query_interval_ps.is_some()
                    || !router.groups.is_empty()
                    || !router.pending_queries.is_empty())
            {
                return Err("active multicast checkpoint non-router state".into());
            }
            if router.other_querier_until.is_some_and(|t| t < now)
                || router.next_query.is_some_and(|t| t < now)
                || router.learned_robustness.is_some_and(|v| v == 0 || v > 7)
                || router.learned_query_interval_ps.is_some_and(|v| v == 0)
                || router
                    .other_querier
                    .is_some_and(|a| a.is_ipv6() != iface.address.is_ipv6())
                || router
                    .pending_queries
                    .iter()
                    .any(|(group, sources, remaining, deadline)| {
                        *deadline < now
                            || *remaining == 0
                            || *remaining >= iface.robustness
                            || group.is_ipv6() != iface.address.is_ipv6()
                            || sources.iter().any(|s| s.is_ipv6() != group.is_ipv6())
                    })
            {
                return Err("active multicast checkpoint querier timers".into());
            }
            for (group, state) in &router.groups {
                if !group.is_multicast()
                    || group.is_ipv6() != iface.address.is_ipv6()
                    || state.exclude_until.is_some_and(|t| t < now)
                    || state.sources.iter().any(|(source, deadline)| {
                        source.is_ipv6() != group.is_ipv6() || *deadline < now
                    })
                {
                    return Err("active multicast checkpoint router group".into());
                }
            }
        }
        for ((bridge, vid, group, port), state) in &self.snooped {
            if !config
                .bridge_ports
                .iter()
                .any(|p| p.bridge == *bridge && p.vid == *vid && p.output_port == *port)
                || !group.is_multicast()
                || state.known_until.is_some_and(|t| t < now)
                || state.exclude_until.is_some_and(|t| t < now)
                || state.older_v1_until.is_some_and(|t| t < now)
                || state.sources.iter().any(|(source, deadline)| {
                    source.is_ipv6() != group.is_ipv6() || *deadline < now
                })
            {
                return Err("active multicast checkpoint snooper".into());
            }
        }
        for ((bridge, vid, family, port), deadline) in &self.router_ports {
            if *deadline < now
                || !config
                    .bridge_ports
                    .iter()
                    .any(|p| p.bridge == *bridge && p.vid == *vid && p.output_port == *port)
                || !config
                    .interfaces
                    .iter()
                    .any(|f| f.address.is_ipv6() == *family && f.vid == *vid)
            {
                return Err("active multicast checkpoint router port".into());
            }
        }
        for ((bridge, vid, family), deadline) in &self.older_flood {
            if *deadline < now
                || !config
                    .bridge_ports
                    .iter()
                    .any(|p| p.bridge == *bridge && p.vid == *vid)
                || !config
                    .interfaces
                    .iter()
                    .any(|f| f.address.is_ipv6() == *family && f.vid == *vid)
            {
                return Err("active multicast checkpoint older flood".into());
            }
        }
        for ((bridge, vid, family), (robustness, interval)) in &self.snoop_query_parameters {
            if *robustness == 0
                || *robustness > 7
                || *interval == 0
                || !config
                    .bridge_ports
                    .iter()
                    .any(|p| p.bridge == *bridge && p.vid == *vid)
                || !config
                    .interfaces
                    .iter()
                    .any(|f| f.address.is_ipv6() == *family && f.vid == *vid)
            {
                return Err("active multicast checkpoint snoop query parameters".into());
            }
        }
        Ok(())
    }

    pub(crate) fn validate_change_prefix(
        &self,
        config: &Config,
        cursor: usize,
    ) -> Result<(), String> {
        if cursor > config.changes.len() {
            return Err("active multicast change prefix cursor".into());
        }
        let mut sockets = vec![BTreeMap::new(); config.interfaces.len()];
        for change in &config.changes[..cursor] {
            let i = config
                .interfaces
                .iter()
                .position(|f| f.id == change.interface)
                .ok_or("multicast prefix interface")?;
            let key = (change.socket.clone(), change.group);
            if let Some(filter) = &change.filter {
                sockets[i].insert(key, filter.clone());
            } else {
                sockets[i].remove(&key);
            }
        }
        for (i, expected) in sockets.iter().enumerate() {
            if self.hosts[i].sockets != *expected {
                return Err("active multicast sockets differ from applied change prefix".into());
            }
            let groups: BTreeSet<_> = expected.keys().map(|(_, group)| *group).collect();
            let filters: BTreeMap<_, _> = groups
                .into_iter()
                .filter_map(|group| {
                    let filter = aggregate(
                        expected
                            .iter()
                            .filter(|((_, g), _)| *g == group)
                            .map(|(_, filter)| filter),
                    );
                    (filter != empty_filter()).then_some((group, filter))
                })
                .collect();
            if self.hosts[i].filters != filters {
                return Err("active multicast filters differ from applied change prefix".into());
            }
        }
        Ok(())
    }
    fn random_delay(rng: &mut u64, limit: u64) -> u64 {
        // xorshift64*: deterministic and checkpointed; zero is never returned
        // for an unsolicited retransmission.
        *rng ^= *rng >> 12;
        *rng ^= *rng << 25;
        *rng ^= *rng >> 27;
        let x = rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        if limit <= 1 { 1 } else { 1 + x % (limit - 1) }
    }

    pub(crate) fn step(
        &mut self,
        config: &Config,
        now: u64,
        event: Event,
    ) -> Result<Effects, String> {
        let mut out = Effects::default();
        match event {
            Event::Start => {
                if self.started {
                    return Err("active multicast started twice".into());
                }
                self.started = true;
                for (i, iface) in config.interfaces.iter().enumerate() {
                    if iface.router {
                        self.routers[i].startup_left = iface.robustness;
                        self.routers[i].next_query = Some(now);
                    }
                }
            }
            Event::Change(index) => self.change(config, now, index, &mut out)?,
            Event::Receive {
                ingress_port,
                vid,
                bytes,
            } => {
                self.snoop_receive(config, now, &ingress_port, vid, &bytes)?;
                for (i, iface) in config.interfaces.iter().enumerate() {
                    if iface.ingress_port == ingress_port
                        && iface.vid == vid
                        && iface.address.is_ipv6() == bytes.first().is_some_and(|b| *b >> 4 == 6)
                    {
                        let message =
                            membership::decode(&bytes, iface.address.is_ipv6(), iface.max_sources)
                                .map_err(|e| e.0)?;
                        let src = packet_source(&bytes, iface.address.is_ipv6())?;
                        if message.is_query {
                            self.query_received(config, i, now, src, &bytes, &message)?;
                        } else if iface.router {
                            self.report_received(config, i, now, &bytes, &message, &mut out)?;
                        }
                    }
                }
            }
            Event::Timer => {}
        }
        self.expire_and_emit(config, now, &mut out)?;
        out.next_deadline = self.next_deadline(config, now);
        Ok(out)
    }

    fn snoop_receive(
        &mut self,
        config: &Config,
        now: u64,
        ingress: &str,
        vid: u16,
        bytes: &[u8],
    ) -> Result<(), String> {
        let Some(port) = config
            .bridge_ports
            .iter()
            .find(|p| p.ingress_port == ingress && p.vid == vid)
        else {
            return Ok(());
        };
        let v6 = bytes.first().is_some_and(|b| *b >> 4 == 6);
        let iface = config
            .interfaces
            .iter()
            .find(|f| f.address.is_ipv6() == v6 && f.vid == vid)
            .ok_or("multicast bridge family has no interface")?;
        let message = membership::decode(bytes, v6, iface.max_sources).map_err(|e| e.0)?;
        let parameters = self
            .snoop_query_parameters
            .get(&(port.bridge.clone(), vid, v6))
            .copied()
            .unwrap_or((iface.robustness, iface.query_interval_ps));
        let group_interval = checked_add(
            parameters.0 as u64 * parameters.1,
            2 * iface.query_response_ps,
        )?;
        let older_interval = checked_add(
            iface.robustness as u64 * iface.query_interval_ps,
            iface.query_response_ps,
        )?;
        if message.legacy {
            self.older_flood.insert(
                (port.bridge.clone(), vid, v6),
                checked_add(now, older_interval)?,
            );
        }
        if message.is_query {
            let parameters = if let Some(query) = &message.query {
                let parameters = (
                    if query.qrv == 0 {
                        iface.robustness
                    } else {
                        query.qrv
                    },
                    query.interval_ps,
                );
                self.snoop_query_parameters
                    .insert((port.bridge.clone(), vid, v6), parameters);
                parameters
            } else {
                parameters
            };
            let interval = checked_add(
                parameters.0 as u64 * parameters.1,
                iface.query_response_ps / 2,
            )?;
            self.router_ports.insert(
                (port.bridge.clone(), vid, v6, port.output_port.clone()),
                checked_add(now, interval)?,
            );
            if let Some(query) = &message.query {
                if !query.suppress && !query.group.is_unspecified() {
                    let count = if query.qrv == 0 {
                        iface.robustness
                    } else {
                        query.qrv
                    };
                    let last = checked_add(now, count as u64 * iface.last_member_interval_ps)?;
                    let requested = query_sources(bytes, v6)?;
                    for ((bridge, group_vid, group, _), state) in &mut self.snooped {
                        if bridge != &port.bridge || *group_vid != vid || *group != query.group {
                            continue;
                        }
                        if let Some(sources) = &requested {
                            for source in sources {
                                if let Some(deadline) = state.sources.get_mut(source) {
                                    *deadline = (*deadline).min(last);
                                }
                            }
                        } else if let Some(deadline) = &mut state.exclude_until {
                            *deadline = (*deadline).min(last);
                        }
                    }
                }
            }
            return Ok(());
        }
        if message.legacy {
            let group = packet_group(bytes, v6)?;
            let leave = is_legacy_leave(bytes, v6)?;
            let entry = self
                .snooped
                .entry((port.bridge.clone(), vid, group, port.output_port.clone()))
                .or_default();
            if !leave {
                let older_deadline = checked_add(now, older_interval)?;
                entry.older_until = Some(older_deadline);
                if !v6 && report_version(bytes, false)? == 1 {
                    entry.older_v1_until = Some(older_deadline);
                }
                entry.known_until = Some(checked_add(now, group_interval)?);
                entry.exclude_until = Some(checked_add(now, group_interval)?);
                entry.blocked.clear();
            }
            return Ok(());
        }
        for record in &message.records {
            let entry = self
                .snooped
                .entry((
                    port.bridge.clone(),
                    vid,
                    record.group,
                    port.output_port.clone(),
                ))
                .or_default();
            let old = entry.older_until.is_some_and(|t| t > now);
            let old_v1 = entry.older_v1_until.is_some_and(|t| t > now);
            if old && (record.kind == 6 || old_v1 && record.kind == 3) {
                continue;
            }
            let mut effective = record.clone();
            if old && effective.kind == 4 {
                effective.sources.clear();
            }
            let gmi = checked_add(now, group_interval)?;
            entry.known_until = Some(gmi);
            let _queries = apply_group_record(
                &mut entry.exclude_until,
                &mut entry.sources,
                &mut entry.blocked,
                &effective,
                gmi,
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn candidates(
        &self,
        bridge: &str,
        vid: u16,
        source: IpAddr,
        group: IpAddr,
        eligible: &BTreeSet<String>,
        ingress: &str,
        static_members: &BTreeSet<String>,
        static_known: bool,
        now: u64,
    ) -> BTreeSet<String> {
        let v6 = group.is_ipv6();
        let link_local = match group {
            IpAddr::V4(a) => a.octets()[..3] == [224, 0, 0],
            IpAddr::V6(a) => a.segments()[0] == 0xff02,
        };
        let known = static_known
            || self.snooped.iter().any(|((b, v, g, _), state)| {
                b == bridge
                    && *v == vid
                    && *g == group
                    && state.known_until.is_some_and(|t| t > now)
            });
        let flood = self
            .older_flood
            .get(&(bridge.into(), vid, v6))
            .is_some_and(|t| *t > now);
        eligible
            .iter()
            .filter(|port| {
                port.as_str() != ingress
                    && (link_local
                        || !known
                        || flood
                        || static_members.contains(*port)
                        || self
                            .router_ports
                            .get(&(bridge.into(), vid, v6, (*port).clone()))
                            .is_some_and(|t| *t > now)
                        || self
                            .snooped
                            .get(&(bridge.into(), vid, group, (*port).clone()))
                            .is_some_and(|state| {
                                state.sources.get(&source).is_some_and(|t| *t > now)
                                    || state.exclude_until.is_some_and(|t| {
                                        t > now && !state.blocked.contains(&source)
                                    })
                            }))
            })
            .cloned()
            .collect()
    }

    fn change(
        &mut self,
        config: &Config,
        now: u64,
        index: usize,
        out: &mut Effects,
    ) -> Result<(), String> {
        let change = config.changes.get(index).ok_or("multicast change index")?;
        if change.at_ps != now {
            return Err("multicast change time".into());
        }
        let i = config
            .interfaces
            .iter()
            .position(|f| f.id == change.interface)
            .ok_or("multicast interface")?;
        let iface = &config.interfaces[i];
        let host = &mut self.hosts[i];
        let key = (change.socket.clone(), change.group);
        if let Some(filter) = &change.filter {
            host.sockets.insert(key, filter.clone());
        } else {
            host.sockets.remove(&key);
        }
        let previous = host
            .filters
            .get(&change.group)
            .cloned()
            .unwrap_or_else(empty_filter);
        let next = aggregate(
            host.sockets
                .iter()
                .filter(|((_, group), _)| *group == change.group)
                .map(|(_, filter)| filter),
        );
        if previous == next {
            return Ok(());
        }
        if next == empty_filter() {
            host.filters.remove(&change.group);
            host.responses.remove(&change.group);
        } else {
            host.filters.insert(change.group, next.clone());
        }
        if host.filters.len() > iface.max_groups
            || host
                .filters
                .values()
                .map(|f| f.sources.len())
                .sum::<usize>()
                > iface.max_sources
        {
            return Err("multicast host capacity".into());
        }
        let change_records = difference(change.group, &previous, &next);
        if change_records.is_empty() {
            return Ok(());
        }
        let version = compatibility(host, now, iface.address.is_ipv6());
        if version < 3 {
            // IGMPv1 has no Leave, while MLDv1 has a Done message.
            if next != empty_filter() || version == 2 || iface.address.is_ipv6() {
                out.tx.push(Self::legacy_report(
                    iface,
                    change.group,
                    next == empty_filter(),
                    version,
                )?);
            }
        } else {
            // A later change must not erase sources whose earlier State-Change
            // Report was lost. Rebuild the outstanding report for the current
            // filter, carrying its retransmission list into this transmission.
            let records = merge_pending_records(
                change.group,
                &next,
                host.pending.get(&change.group),
                &change_records,
            );
            out.tx.push(transmit(
                iface,
                encode_report(iface.address, &records)?,
                "state_change",
            ));
            if iface.robustness > 1 {
                let delay = Self::random_delay(&mut self.rng, iface.unsolicited_interval_ps);
                self.hosts[i].pending.insert(
                    change.group,
                    Pending {
                        records,
                        remaining: iface.robustness - 1,
                        deadline: checked_add(now, delay)?,
                    },
                );
            }
        }
        Ok(())
    }

    fn legacy_report(
        iface: &Interface,
        group: IpAddr,
        leave: bool,
        version: u8,
    ) -> Result<Transmission, String> {
        let bytes = encode_legacy_report(iface.address, group, leave, version)?;
        Ok(transmit(
            iface,
            bytes,
            if leave { "leave" } else { "legacy_report" },
        ))
    }

    fn query_received(
        &mut self,
        config: &Config,
        i: usize,
        now: u64,
        source: IpAddr,
        bytes: &[u8],
        message: &membership::Message,
    ) -> Result<(), String> {
        let iface = &config.interfaces[i];
        if source == iface.address {
            return Ok(());
        }
        let version = query_version(bytes, iface.address.is_ipv6())?;
        let host = &mut self.hosts[i];
        if version < 3 {
            let interval = checked_add(
                iface.robustness as u64 * iface.query_interval_ps,
                10 * iface.query_response_ps,
            )?;
            let until = checked_add(now, interval)?;
            if version == 1 {
                host.older_v1_until = Some(until);
            } else {
                host.older_v2_until = Some(until);
            }
            host.pending.clear();
            host.responses.clear();
        }
        let group = if let Some(query) = &message.query {
            query.group
        } else {
            packet_group(bytes, iface.address.is_ipv6())?
        };
        let response = if let Some(query) = &message.query {
            query.response_ps
        } else if iface.address.is_ipv6() {
            let payload = membership_payload(bytes, true)?;
            u16::from_be_bytes([payload[4], payload[5]]) as u64 * 1_000_000_000
        } else if version == 1 {
            10_000_000_000_000
        } else {
            membership_payload(bytes, false)?[1] as u64 * 100_000_000_000
        };
        let requested = query_sources(bytes, iface.address.is_ipv6())?;
        if iface.router
            && message.query.as_ref().is_some_and(|query| !query.suppress)
            && !group.is_unspecified()
        {
            let count = message
                .query
                .as_ref()
                .map(|q| q.qrv)
                .filter(|q| *q != 0)
                .unwrap_or(iface.robustness);
            let until = checked_add(now, count as u64 * iface.last_member_interval_ps)?;
            if let Some(state) = self.routers[i].groups.get_mut(&group) {
                if let Some(sources) = &requested {
                    for source in sources {
                        if let Some(deadline) = state.sources.get_mut(source) {
                            *deadline = (*deadline).min(until);
                        }
                    }
                } else if let Some(deadline) = &mut state.exclude_until {
                    *deadline = (*deadline).min(until);
                }
            }
        }
        for target in self.hosts[i].filters.keys().copied().collect::<Vec<_>>() {
            if !group.is_unspecified() && target != group {
                continue;
            }
            let deadline = checked_add(now, Self::random_delay(&mut self.rng, response.max(2)))?;
            let entry = self.hosts[i]
                .responses
                .entry(target)
                .or_insert_with(|| Response {
                    deadline,
                    sources: requested.clone(),
                });
            entry.deadline = entry.deadline.min(deadline);
            entry.sources = match (entry.sources.take(), &requested) {
                (None, _) | (_, None) => None,
                (Some(mut old), Some(new)) => {
                    old.extend(new);
                    Some(old)
                }
            };
        }
        if iface.router && version >= 3 && group.is_unspecified() {
            let router = &mut self.routers[i];
            if let Some(query) = &message.query {
                router.learned_robustness = Some(if query.qrv == 0 {
                    iface.robustness
                } else {
                    query.qrv
                });
                router.learned_query_interval_ps = Some(query.interval_ps);
            }
        }
        if iface.router && group.is_unspecified() && source < iface.address {
            let router = &mut self.routers[i];
            let interval = router.learned_robustness.unwrap_or(iface.robustness) as u64
                * router
                    .learned_query_interval_ps
                    .unwrap_or(iface.query_interval_ps)
                + iface.query_response_ps / 2;
            router.other_querier = Some(source);
            router.other_querier_until = Some(checked_add(now, interval)?);
            router.next_query = None;
            router.pending_queries.clear();
        }
        Ok(())
    }

    fn report_received(
        &mut self,
        config: &Config,
        i: usize,
        now: u64,
        bytes: &[u8],
        message: &membership::Message,
        out: &mut Effects,
    ) -> Result<(), String> {
        let iface = &config.interfaces[i];
        let router = &self.routers[i];
        let gmi = checked_add(
            router.learned_robustness.unwrap_or(iface.robustness) as u64
                * router
                    .learned_query_interval_ps
                    .unwrap_or(iface.query_interval_ps),
            2 * iface.query_response_ps,
        )?;
        let older_interval = checked_add(
            iface.robustness as u64 * iface.query_interval_ps,
            iface.query_response_ps,
        )?;
        if message.legacy {
            let version = report_version(bytes, iface.address.is_ipv6())?;
            let group = packet_group(bytes, iface.address.is_ipv6())?;
            let leave = is_legacy_leave(bytes, iface.address.is_ipv6())?;
            let entry = self.routers[i].groups.entry(group).or_default();
            if leave {
                // MLDv1 Done and IGMPv2 Leave are TO_IN({}) events. Only
                // IGMPv2 Leave is ignored while an IGMPv1 host is present.
                if iface.address.is_ipv6() || entry.older_v1_until.is_none_or(|t| t <= now) {
                    self.last_member_query(config, i, now, group, BTreeSet::new(), out)?;
                }
            } else {
                if version == 1 {
                    entry.older_v1_until = Some(checked_add(now, older_interval)?);
                } else {
                    entry.older_v2_until = Some(checked_add(now, older_interval)?);
                }
                // RFC 9776 Table 13 / RFC 9777 Table 10: Report is
                // IS_EX({}), so any prior source-specific block is gone.
                entry.sources.clear();
                entry.blocked.clear();
                entry.exclude_until = Some(checked_add(now, gmi)?);
            }
            return Ok(());
        }
        for record in &message.records {
            let entry = self.routers[i].groups.entry(record.group).or_default();
            let old_v1 = entry.older_v1_until.is_some_and(|t| t > now);
            let old = old_v1 || entry.older_v2_until.is_some_and(|t| t > now);
            // RFC 9776 7.3.2 / RFC 9777 8.3.2: old members suppress
            // BLOCK, strip TO_EX source lists, and IGMPv1 also suppresses
            // TO_IN. ALLOW and the other current records remain meaningful.
            if old && (record.kind == 6 || !iface.address.is_ipv6() && old_v1 && record.kind == 3) {
                continue;
            }
            let mut effective = record.clone();
            if old && effective.kind == 4 {
                effective.sources.clear();
            }
            let queries = apply_group_record(
                &mut entry.exclude_until,
                &mut entry.sources,
                &mut entry.blocked,
                &effective,
                checked_add(now, gmi)?,
            );
            self.prune_pending_queries(i, now, record.group);
            if !queries.sources.is_empty() {
                self.last_member_query(config, i, now, record.group, queries.sources, out)?;
            }
            if queries.group {
                self.last_member_query(config, i, now, record.group, BTreeSet::new(), out)?;
            }
        }
        Ok(())
    }

    fn prune_pending_queries(&mut self, i: usize, now: u64, group: IpAddr) {
        let router = &mut self.routers[i];
        let Some(state) = router.groups.get(&group) else {
            return;
        };
        router
            .pending_queries
            .retain_mut(|(pending_group, sources, _, _)| {
                if *pending_group != group {
                    return true;
                }
                if sources.is_empty() {
                    return state.exclude_until.is_some_and(|t| t > now);
                }
                // A refreshed source keeps its retransmission state. Its next
                // query carries S=1 while the source timer exceeds LMQT.
                sources.retain(|source| state.sources.get(source).is_some_and(|t| *t > now));
                !sources.is_empty()
            });
    }

    fn last_member_query(
        &mut self,
        config: &Config,
        i: usize,
        now: u64,
        group: IpAddr,
        sources: BTreeSet<IpAddr>,
        out: &mut Effects,
    ) -> Result<(), String> {
        let iface = &config.interfaces[i];
        if self.routers[i].other_querier_until.is_some_and(|t| t > now) {
            return Ok(());
        }
        let until = checked_add(now, iface.robustness as u64 * iface.last_member_interval_ps)?;
        if let Some(entry) = self.routers[i].groups.get_mut(&group) {
            if sources.is_empty() {
                entry.exclude_until = entry.exclude_until.map(|t| t.min(until));
            } else {
                for source in &sources {
                    if let Some(t) = entry.sources.get_mut(source) {
                        *t = (*t).min(until);
                    }
                }
            }
        }
        if self.routers[i].other_querier_until.is_none_or(|t| t <= now) {
            out.tx.push(transmit(
                iface,
                encode_query(iface.address, group, &sources, iface)?,
                "last_member_query",
            ));
            if iface.robustness > 1 {
                let deadline = checked_add(now, iface.last_member_interval_ps)?;
                if let Some((_, pending_sources, remaining, pending_deadline)) = self.routers[i]
                    .pending_queries
                    .iter_mut()
                    .find(|(g, s, _, _)| *g == group && s.is_empty() == sources.is_empty())
                {
                    pending_sources.extend(sources);
                    *remaining = (*remaining).max(iface.robustness - 1);
                    *pending_deadline = (*pending_deadline).min(deadline);
                } else {
                    self.routers[i].pending_queries.push((
                        group,
                        sources,
                        iface.robustness - 1,
                        deadline,
                    ));
                }
            }
        }
        Ok(())
    }

    fn expire_and_emit(
        &mut self,
        config: &Config,
        now: u64,
        out: &mut Effects,
    ) -> Result<(), String> {
        self.router_ports.retain(|_, deadline| *deadline > now);
        self.older_flood.retain(|_, deadline| *deadline > now);
        for group in self.snooped.values_mut() {
            expire_group(
                &mut group.exclude_until,
                &mut group.sources,
                &mut group.blocked,
                now,
            );
            if group.older_until.is_some_and(|t| t <= now) {
                group.older_until = None;
            }
            if group.older_v1_until.is_some_and(|t| t <= now) {
                group.older_v1_until = None;
            }
        }
        self.snooped.retain(|_, group| {
            group.known_until.is_some_and(|t| t > now)
                || group.exclude_until.is_some()
                || !group.sources.is_empty()
        });
        for (i, iface) in config.interfaces.iter().enumerate() {
            let host = &mut self.hosts[i];
            if host.older_v1_until.is_some_and(|t| t <= now) {
                host.older_v1_until = None;
            }
            if host.older_v2_until.is_some_and(|t| t <= now) {
                host.older_v2_until = None;
            }
            let due = host
                .pending
                .iter()
                .filter(|(_, p)| p.deadline <= now)
                .map(|(g, _)| *g)
                .collect::<Vec<_>>();
            for group in due {
                let p = host.pending.get(&group).unwrap().clone();
                out.tx.push(transmit(
                    iface,
                    encode_report(iface.address, &p.records)?,
                    "retransmit",
                ));
                if p.remaining <= 1 {
                    host.pending.remove(&group);
                } else {
                    let delay = Self::random_delay(&mut self.rng, iface.unsolicited_interval_ps);
                    host.pending.get_mut(&group).unwrap().remaining -= 1;
                    host.pending.get_mut(&group).unwrap().deadline = checked_add(now, delay)?;
                }
            }
            let due = host
                .responses
                .iter()
                .filter(|(_, t)| t.deadline <= now)
                .map(|(g, _)| *g)
                .collect::<Vec<_>>();
            for group in due {
                let filter = host.filters[&group].clone();
                let response = host.responses.remove(&group).unwrap();
                if compatibility(host, now, iface.address.is_ipv6()) < 3 {
                    out.tx.push(Self::legacy_report(
                        iface,
                        group,
                        false,
                        compatibility(host, now, iface.address.is_ipv6()),
                    )?);
                } else {
                    let record = if let Some(requested) = response.sources {
                        membership::Record {
                            kind: 1,
                            group,
                            sources: if filter.exclude {
                                requested.difference(&filter.sources).copied().collect()
                            } else {
                                requested.intersection(&filter.sources).copied().collect()
                            },
                        }
                    } else {
                        membership::Record {
                            kind: if filter.exclude { 2 } else { 1 },
                            group,
                            sources: filter.sources,
                        }
                    };
                    out.tx.push(transmit(
                        iface,
                        encode_report(iface.address, &[record])?,
                        "query_response",
                    ));
                }
            }
            let router = &mut self.routers[i];
            for entry in router.groups.values_mut() {
                expire_group(
                    &mut entry.exclude_until,
                    &mut entry.sources,
                    &mut entry.blocked,
                    now,
                );
                if entry.older_v1_until.is_some_and(|t| t <= now) {
                    entry.older_v1_until = None;
                }
                if entry.older_v2_until.is_some_and(|t| t <= now) {
                    entry.older_v2_until = None;
                }
            }
            router
                .groups
                .retain(|_, g| g.exclude_until.is_some() || !g.sources.is_empty());
            if router.other_querier_until.is_some_and(|t| t <= now) {
                router.other_querier_until = None;
                router.other_querier = None;
                router.learned_robustness = None;
                router.learned_query_interval_ps = None;
                router.next_query = Some(now);
                router.startup_left = iface.robustness;
            }
            if iface.router
                && router.other_querier_until.is_none()
                && router.next_query.is_some_and(|t| t <= now)
            {
                out.tx.push(transmit(
                    iface,
                    encode_query(
                        iface.address,
                        unspecified(iface.address),
                        &BTreeSet::new(),
                        iface,
                    )?,
                    "general_query",
                ));
                let interval = if router.startup_left > 0 {
                    router.startup_left -= 1;
                    if router.startup_left > 0 {
                        iface.query_interval_ps / 4
                    } else {
                        iface.query_interval_ps
                    }
                } else {
                    iface.query_interval_ps
                };
                router.next_query = Some(checked_add(now, interval.max(1))?);
            }
            let mut next = Vec::new();
            for (group, sources, remaining, deadline) in std::mem::take(&mut router.pending_queries)
            {
                if deadline <= now && remaining > 0 {
                    let Some(state) = router.groups.get(&group) else {
                        continue;
                    };
                    let lmt =
                        checked_add(now, iface.robustness as u64 * iface.last_member_interval_ps)?;
                    let active = if sources.is_empty() {
                        if state.exclude_until.is_some_and(|t| t > now) {
                            let suppress = state.exclude_until.is_some_and(|t| t > lmt);
                            out.tx.push(transmit(
                                iface,
                                encode_query_with_suppress(
                                    iface.address,
                                    group,
                                    &sources,
                                    iface,
                                    suppress,
                                )?,
                                "last_member_retransmit",
                            ));
                            Some(sources)
                        } else {
                            None
                        }
                    } else {
                        let mut suppressed = BTreeSet::new();
                        let mut unsuppressed = BTreeSet::new();
                        for source in sources {
                            if let Some(timer) = state.sources.get(&source).filter(|t| **t > now) {
                                if *timer > lmt {
                                    suppressed.insert(source);
                                } else {
                                    unsuppressed.insert(source);
                                }
                            }
                        }
                        let active: BTreeSet<IpAddr> =
                            suppressed.union(&unsuppressed).copied().collect();
                        if iface.router_version >= if iface.address.is_ipv6() { 2 } else { 3 } {
                            for (set, suppress) in [(&suppressed, true), (&unsuppressed, false)] {
                                if !set.is_empty() {
                                    out.tx.push(transmit(
                                        iface,
                                        encode_query_with_suppress(
                                            iface.address,
                                            group,
                                            set,
                                            iface,
                                            suppress,
                                        )?,
                                        "last_member_retransmit",
                                    ));
                                }
                            }
                        } else if !suppressed.is_empty() || !unsuppressed.is_empty() {
                            out.tx.push(transmit(
                                iface,
                                encode_query(iface.address, group, &BTreeSet::new(), iface)?,
                                "last_member_retransmit",
                            ));
                        }
                        (!active.is_empty()).then_some(active)
                    };
                    if remaining > 1 {
                        if let Some(active) = active {
                            next.push((
                                group,
                                active,
                                remaining - 1,
                                checked_add(now, iface.last_member_interval_ps)?,
                            ));
                        }
                    }
                } else {
                    next.push((group, sources, remaining, deadline));
                }
            }
            router.pending_queries = next;
        }
        Ok(())
    }

    fn timer_times(&self) -> Vec<u64> {
        let mut times = Vec::new();
        times.extend(self.router_ports.values().copied());
        times.extend(self.older_flood.values().copied());
        for group in self.snooped.values() {
            times.extend(
                [
                    group.known_until,
                    group.exclude_until,
                    group.older_until,
                    group.older_v1_until,
                ]
                .into_iter()
                .flatten(),
            );
            times.extend(group.sources.values().copied());
        }
        for (host, router) in self.hosts.iter().zip(&self.routers) {
            times.extend(host.pending.values().map(|p| p.deadline));
            times.extend(host.responses.values().map(|r| r.deadline));
            times.extend(
                [
                    host.older_v1_until,
                    host.older_v2_until,
                    router.other_querier_until,
                    router.next_query,
                ]
                .into_iter()
                .flatten(),
            );
            times.extend(router.pending_queries.iter().map(|p| p.3));
            for group in router.groups.values() {
                times.extend(
                    [
                        group.exclude_until,
                        group.older_v1_until,
                        group.older_v2_until,
                    ]
                    .into_iter()
                    .flatten(),
                );
                times.extend(group.sources.values().copied());
            }
        }
        times
    }
    pub(crate) fn next_deadline(&self, config: &Config, now: u64) -> Option<u64> {
        self.timer_times()
            .into_iter()
            .chain(config.changes.iter().map(|change| change.at_ps))
            .filter(|t| *t > now)
            .min()
    }
    pub(crate) fn checkpoint_deadline(
        &self,
        config: &Config,
        now: u64,
        cursor: usize,
    ) -> Option<u64> {
        self.timer_times()
            .into_iter()
            .chain(
                config
                    .changes
                    .iter()
                    .skip(cursor)
                    .map(|change| change.at_ps),
            )
            .filter(|t| *t >= now)
            .min()
    }

    pub(crate) fn interested(
        &self,
        config: &Config,
        ingress_port: &str,
        vid: u16,
        source: IpAddr,
        group: IpAddr,
        now: u64,
    ) -> bool {
        config.interfaces.iter().enumerate().any(|(i, f)| {
            f.ingress_port == ingress_port
                && f.vid == vid
                && (self.hosts[i].filters.get(&group).is_some_and(|state| {
                    if state.exclude {
                        !state.sources.contains(&source)
                    } else {
                        state.sources.contains(&source)
                    }
                }) || self.routers[i].groups.get(&group).is_some_and(|state| {
                    state.sources.get(&source).is_some_and(|t| *t > now)
                        || state
                            .exclude_until
                            .is_some_and(|t| t > now && !state.blocked.contains(&source))
                }))
        })
    }
}

fn checked_add(a: u64, b: u64) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| "multicast time overflow".into())
}
fn unspecified(a: IpAddr) -> IpAddr {
    if a.is_ipv6() {
        IpAddr::V6(Ipv6Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    }
}
fn empty_filter() -> Filter {
    Filter {
        exclude: false,
        sources: BTreeSet::new(),
    }
}
fn filter_valid(filter: &Filter, v6: bool) -> bool {
    (filter.exclude || !filter.sources.is_empty())
        && filter.sources.iter().all(|source| {
            source.is_ipv6() == v6
                && !source.is_multicast()
                && !source.is_unspecified()
                && !source.is_loopback()
                && !matches!(source, IpAddr::V4(v) if *v == Ipv4Addr::BROADCAST)
        })
}
fn compatibility(host: &Host, now: u64, v6: bool) -> u8 {
    if host.older_v1_until.is_some_and(|t| t > now) {
        1
    } else if !v6 && host.older_v2_until.is_some_and(|t| t > now) {
        2
    } else {
        3
    }
}
fn aggregate<'a>(filters: impl Iterator<Item = &'a Filter>) -> Filter {
    let mut include = BTreeSet::new();
    let mut exclude: Option<BTreeSet<IpAddr>> = None;
    for f in filters {
        if f.exclude {
            exclude = Some(match exclude {
                None => f.sources.clone(),
                Some(e) => e.intersection(&f.sources).copied().collect(),
            });
        } else {
            include.extend(&f.sources);
        }
    }
    match exclude {
        None => Filter {
            exclude: false,
            sources: include,
        },
        Some(e) => Filter {
            exclude: true,
            sources: e.difference(&include).copied().collect(),
        },
    }
}
fn difference(group: IpAddr, old: &Filter, new: &Filter) -> Vec<membership::Record> {
    let mut out = Vec::new();
    let mut push = |kind, sources: BTreeSet<IpAddr>| {
        if !sources.is_empty() || kind == 3 || kind == 4 {
            out.push(membership::Record {
                kind,
                group,
                sources,
            });
        }
    };
    match (old.exclude, new.exclude) {
        (false, false) => {
            push(5, new.sources.difference(&old.sources).copied().collect());
            push(6, old.sources.difference(&new.sources).copied().collect());
        }
        (true, true) => {
            push(5, old.sources.difference(&new.sources).copied().collect());
            push(6, new.sources.difference(&old.sources).copied().collect());
        }
        (false, true) => push(4, new.sources.clone()),
        (true, false) => push(3, new.sources.clone()),
    }
    out
}

fn merge_pending_records(
    group: IpAddr,
    current: &Filter,
    pending: Option<&Pending>,
    change: &[membership::Record],
) -> Vec<membership::Record> {
    let mut sources = BTreeSet::new();
    let mut mode_change = false;
    for record in pending
        .into_iter()
        .flat_map(|pending| pending.records.iter())
        .chain(change.iter())
    {
        mode_change |= matches!(record.kind, 3 | 4);
        sources.extend(&record.sources);
    }
    if mode_change {
        return vec![membership::Record {
            kind: if current.exclude { 4 } else { 3 },
            group,
            sources: current.sources.clone(),
        }];
    }
    let forwarded: BTreeSet<_> = sources
        .iter()
        .filter(|source| current.exclude != current.sources.contains(source))
        .copied()
        .collect();
    let blocked: BTreeSet<_> = sources.difference(&forwarded).copied().collect();
    let mut records = Vec::new();
    for (kind, sources) in [(5, forwarded), (6, blocked)] {
        if !sources.is_empty() {
            records.push(membership::Record {
                kind,
                group,
                sources,
            });
        }
    }
    records
}

fn packet_source(bytes: &[u8], v6: bool) -> Result<IpAddr, String> {
    if v6 {
        Ok(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(bytes.get(8..24).ok_or("IPv6 source")?)
                .map_err(|_| "IPv6 source")?,
        )))
    } else {
        Ok(IpAddr::V4(Ipv4Addr::from(
            <[u8; 4]>::try_from(bytes.get(12..16).ok_or("IPv4 source")?)
                .map_err(|_| "IPv4 source")?,
        )))
    }
}
fn packet_group(bytes: &[u8], v6: bool) -> Result<IpAddr, String> {
    let message = membership_payload(bytes, v6)?;
    if v6 {
        Ok(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(message.get(8..24).ok_or("MLD group")?)
                .map_err(|_| "MLD group")?,
        )))
    } else {
        Ok(IpAddr::V4(Ipv4Addr::from(
            <[u8; 4]>::try_from(message.get(4..8).ok_or("IGMP group")?)
                .map_err(|_| "IGMP group")?,
        )))
    }
}
fn membership_payload(bytes: &[u8], v6: bool) -> Result<&[u8], String> {
    // Decode has already validated the envelope. Still use checked indexing so
    // direct helpers and future callers cannot panic on a short packet.
    let offset = if v6 {
        40 + 8 * (bytes.get(41).ok_or("MLD header")?.to_owned() as usize + 1)
    } else {
        (bytes.first().ok_or("IGMP header")? & 15) as usize * 4
    };
    bytes.get(offset..).ok_or("membership payload".into())
}
fn query_sources(bytes: &[u8], v6: bool) -> Result<Option<BTreeSet<IpAddr>>, String> {
    let message = membership_payload(bytes, v6)?;
    let base = if v6 { 28 } else { 12 };
    let size = if v6 { 16 } else { 4 };
    if message.len() < base {
        return Ok(None);
    }
    let n = u16::from_be_bytes([message[base - 2], message[base - 1]]) as usize;
    let tail = message
        .get(base..base + n * size)
        .ok_or("query source length")?;
    let mut sources = BTreeSet::new();
    for bytes in tail.chunks_exact(size) {
        sources.insert(if v6 {
            IpAddr::V6(Ipv6Addr::from(
                <[u8; 16]>::try_from(bytes).map_err(|_| "query source")?,
            ))
        } else {
            IpAddr::V4(Ipv4Addr::from(
                <[u8; 4]>::try_from(bytes).map_err(|_| "query source")?,
            ))
        });
    }
    Ok((!sources.is_empty()).then_some(sources))
}
fn query_version(bytes: &[u8], v6: bool) -> Result<u8, String> {
    let message = membership_payload(bytes, v6)?;
    if v6 {
        Ok(if message.len() == 24 { 1 } else { 3 })
    } else if message.len() == 8 {
        Ok(if message[1] == 0 { 1 } else { 2 })
    } else {
        Ok(3)
    }
}
fn report_version(bytes: &[u8], v6: bool) -> Result<u8, String> {
    if v6 || membership_payload(bytes, false)?.first() == Some(&0x12) {
        Ok(1)
    } else {
        Ok(2)
    }
}
fn is_legacy_leave(bytes: &[u8], v6: bool) -> Result<bool, String> {
    if v6 {
        Ok(membership_payload(bytes, true)?.first() == Some(&132))
    } else {
        Ok(membership_payload(bytes, false)?.first() == Some(&0x17))
    }
}

fn ip_address_bytes(a: IpAddr, v6: bool) -> Result<Vec<u8>, String> {
    match (a, v6) {
        (IpAddr::V4(a), false) => Ok(a.octets().to_vec()),
        (IpAddr::V6(a), true) => Ok(a.octets().to_vec()),
        _ => Err("multicast IP family mismatch".into()),
    }
}
fn envelope(source: IpAddr, destination: IpAddr, mut message: Vec<u8>) -> Result<Vec<u8>, String> {
    let v6 = source.is_ipv6();
    if source.is_ipv6() != destination.is_ipv6() {
        return Err("multicast envelope family".into());
    }
    if v6 {
        let len = message.len() as u32;
        let mut pseudo = Vec::with_capacity(40 + message.len());
        pseudo.extend(ip_address_bytes(source, true)?);
        pseudo.extend(ip_address_bytes(destination, true)?);
        pseudo.extend(len.to_be_bytes());
        pseudo.extend([0, 0, 0, 58]);
        pseudo.extend(&message);
        let csum = codec::checksum(&pseudo);
        message[2..4].copy_from_slice(&csum.to_be_bytes());
        let payload_len = u16::try_from(message.len() + 8).map_err(|_| "MLD payload too long")?;
        let mut out = vec![0x60, 0, 0, 0];
        out.extend(payload_len.to_be_bytes());
        out.extend([0, 1]); // Hop-by-Hop; Hop Limit 1.
        out.extend(ip_address_bytes(source, true)?);
        out.extend(ip_address_bytes(destination, true)?);
        out.extend([58, 0, 5, 2, 0, 0, 1, 0]); // Router Alert, PadN.
        out.extend(message);
        Ok(out)
    } else {
        let csum = codec::checksum(&message);
        message[2..4].copy_from_slice(&csum.to_be_bytes());
        let total = u16::try_from(message.len() + 24).map_err(|_| "IGMP packet too long")?;
        let mut out = vec![0x46, 0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 0];
        out[2..4].copy_from_slice(&total.to_be_bytes());
        out.extend(ip_address_bytes(source, false)?);
        out.extend(ip_address_bytes(destination, false)?);
        out.extend([148, 4, 0, 0]); // IPv4 Router Alert.
        let csum = codec::checksum(&out);
        out[10..12].copy_from_slice(&csum.to_be_bytes());
        out.extend(message);
        Ok(out)
    }
}
fn transmit(iface: &Interface, ip_bytes: Vec<u8>, reason: &'static str) -> Transmission {
    let destination = if iface.address.is_ipv6() {
        IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(&ip_bytes[24..40]).unwrap(),
        ))
    } else {
        IpAddr::V4(Ipv4Addr::from(
            <[u8; 4]>::try_from(&ip_bytes[16..20]).unwrap(),
        ))
    };
    Transmission {
        interface: iface.id.clone(),
        output_port: iface.output_port.clone(),
        vid: iface.vid,
        source_mac: iface.mac,
        destination_mac: membership::destination_mac(destination),
        ip_bytes,
        reason,
    }
}
fn encode_report(source: IpAddr, records: &[membership::Record]) -> Result<Vec<u8>, String> {
    let v6 = source.is_ipv6();
    let destination = if v6 {
        "ff02::16".parse().unwrap()
    } else {
        "224.0.0.22".parse().unwrap()
    };
    let mut m = vec![if v6 { 143 } else { 34 }, 0, 0, 0, 0, 0];
    m.extend(
        u16::try_from(records.len())
            .map_err(|_| "too many group records")?
            .to_be_bytes(),
    );
    for record in records {
        if record.sources.len() > u16::MAX as usize {
            return Err("too many sources".into());
        }
        m.extend([record.kind, 0]);
        m.extend((record.sources.len() as u16).to_be_bytes());
        m.extend(ip_address_bytes(record.group, v6)?);
        for source in &record.sources {
            m.extend(ip_address_bytes(*source, v6)?);
        }
    }
    envelope(source, destination, m)
}
/// Exact representability keeps advertised and local timer intervals equal.
pub(crate) fn encode_float8(value: u64) -> Option<u8> {
    if value < 128 {
        return Some(value as u8);
    }
    for exponent in 0..8 {
        let scale = 1u64 << (exponent + 3);
        if value % scale == 0 {
            let mantissa = value / scale;
            if (16..32).contains(&mantissa) {
                return Some(0x80 | (exponent << 4) as u8 | (mantissa as u8 - 16));
            }
        }
    }
    None
}
pub(crate) fn encode_float16(value: u64) -> Option<u16> {
    if value < 32768 {
        return Some(value as u16);
    }
    for exponent in 0..8 {
        let scale = 1u64 << (exponent + 3);
        if value % scale == 0 {
            let mantissa = value / scale;
            if (4096..8192).contains(&mantissa) {
                return Some(0x8000 | (exponent << 12) as u16 | (mantissa as u16 - 4096));
            }
        }
    }
    None
}
fn encode_query(
    source: IpAddr,
    group: IpAddr,
    sources: &BTreeSet<IpAddr>,
    iface: &Interface,
) -> Result<Vec<u8>, String> {
    encode_query_with_suppress(source, group, sources, iface, false)
}
fn encode_query_with_suppress(
    source: IpAddr,
    group: IpAddr,
    sources: &BTreeSet<IpAddr>,
    iface: &Interface,
    suppress: bool,
) -> Result<Vec<u8>, String> {
    let v6 = source.is_ipv6();
    let destination = if group.is_unspecified() {
        if v6 {
            "ff02::1".parse().unwrap()
        } else {
            "224.0.0.1".parse().unwrap()
        }
    } else {
        group
    };
    let response = if group.is_unspecified() {
        iface.query_response_ps
    } else {
        iface.last_member_interval_ps
    };
    let code = if v6 {
        let milliseconds = response / 1_000_000_000;
        if iface.router_version >= 2 {
            encode_float16(milliseconds).ok_or("unrepresentable MLD response code")?
        } else {
            u16::try_from(milliseconds).map_err(|_| "MLD response code")?
        }
    } else if iface.router_version >= 3 {
        encode_float8(response / 100_000_000_000).ok_or("unrepresentable IGMP response code")?
            as u16
    } else {
        u16::try_from(response / 100_000_000_000).map_err(|_| "IGMP response code")?
    };
    let mut m = vec![
        if v6 { 130 } else { 17 },
        if v6 {
            0
        } else {
            u8::try_from(code).map_err(|_| "IGMP response code")?
        },
        0,
        0,
    ];
    if v6 {
        m.extend(code.to_be_bytes());
        m.extend([0, 0]);
    }
    m.extend(ip_address_bytes(group, v6)?);
    if iface.router_version >= if v6 { 2 } else { 3 } {
        m.extend([
            (iface.robustness & 7) | if suppress { 0x08 } else { 0 },
            encode_float8(iface.query_interval_ps / 1_000_000_000_000)
                .ok_or("unrepresentable query interval code")?,
        ]);
        m.extend(
            u16::try_from(sources.len())
                .map_err(|_| "too many query sources")?
                .to_be_bytes(),
        );
        for s in sources {
            m.extend(ip_address_bytes(*s, v6)?);
        }
    } else if !v6 && iface.router_version == 1 {
        m[1] = 0;
    }
    envelope(source, destination, m)
}
fn encode_legacy_report(
    source: IpAddr,
    group: IpAddr,
    leave: bool,
    version: u8,
) -> Result<Vec<u8>, String> {
    let v6 = source.is_ipv6();
    let destination = if leave {
        if v6 {
            "ff02::2".parse().unwrap()
        } else {
            "224.0.0.2".parse().unwrap()
        }
    } else {
        group
    };
    let mut m = vec![
        if v6 {
            if leave { 132 } else { 131 }
        } else if leave {
            23
        } else if version == 1 {
            18
        } else {
            22
        },
        0,
        0,
        0,
    ];
    if v6 {
        m.extend([0, 0, 0, 0]);
    }
    m.extend(ip_address_bytes(group, v6)?);
    envelope(source, destination, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn iface(id: &str, addr: &str, router: bool) -> Interface {
        Interface {
            id: id.into(),
            output_port: format!("{id}/out"),
            ingress_port: format!("{id}/in"),
            vid: 7,
            address: addr.parse().unwrap(),
            mac: [2, 0, 0, 0, 0, if router { 1 } else { 2 }],
            router,
            router_version: if addr.contains(':') { 2 } else { 3 },
            robustness: 2,
            query_interval_ps: 4_000_000_000_000,
            query_response_ps: 1_000_000_000_000,
            last_member_interval_ps: 100_000_000_000,
            unsolicited_interval_ps: 100_000_000_000,
            max_groups: 16,
            max_sources: 16,
        }
    }
    #[test]
    fn current_report_round_trip() {
        for (source, group, member) in [
            ("192.0.2.10", "239.1.2.3", "192.0.2.11"),
            ("fe80::10", "ff3e::1234", "2001:db8::11"),
        ] {
            let source: IpAddr = source.parse().unwrap();
            let group: IpAddr = group.parse().unwrap();
            let member: IpAddr = member.parse().unwrap();
            let bytes = encode_report(
                source,
                &[membership::Record {
                    kind: 1,
                    group,
                    sources: BTreeSet::from([member]),
                }],
            )
            .unwrap();
            let decoded = membership::decode(&bytes, source.is_ipv6(), 16).unwrap();
            assert_eq!(decoded.records[0].sources, BTreeSet::from([member]));
            let bad = {
                let mut bad = bytes.clone();
                let last = bad.len() - 1;
                bad[last] ^= 1;
                bad
            };
            assert!(membership::decode(&bad, source.is_ipv6(), 16).is_err());
        }
    }
    #[test]
    fn last_socket_leave_cancels_pending_query_response_for_both_families() {
        for (address, querier, group, source) in [
            ("192.0.2.10", "192.0.2.1", "239.1.2.3", "192.0.2.11"),
            ("fe80::10", "fe80::1", "ff3e::1234", "2001:db8::11"),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("host", address, false)],
                bridge_ports: vec![],
                changes: vec![
                    Change {
                        at_ps: 1,
                        interface: "host".into(),
                        socket: "only".into(),
                        group,
                        filter: Some(Filter {
                            exclude: false,
                            sources: BTreeSet::from([source]),
                        }),
                    },
                    Change {
                        at_ps: 2,
                        interface: "host".into(),
                        socket: "only".into(),
                        group,
                        filter: None,
                    },
                ],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            actor.step(&config, 1, Event::Change(0)).unwrap();
            let mut query_iface = config.interfaces[0].clone();
            query_iface.address = querier.parse().unwrap();
            let query =
                encode_query(query_iface.address, group, &BTreeSet::new(), &query_iface).unwrap();
            actor
                .step(
                    &config,
                    2,
                    Event::Receive {
                        ingress_port: "host/in".into(),
                        vid: 7,
                        bytes: query,
                    },
                )
                .unwrap();
            let response_due = actor.hosts[0].responses[&group].deadline;
            actor.step(&config, 2, Event::Change(1)).unwrap();
            assert!(!actor.hosts[0].responses.contains_key(&group));
            let due = actor.step(&config, response_due, Event::Timer).unwrap();
            assert!(!due.tx.iter().any(|tx| tx.reason == "query_response"));
            actor.validate(&config, response_due).unwrap();
        }
    }

    #[test]
    fn legacy_igmp_reads_payload_after_router_alert_and_extra_options() {
        let config = Config {
            interfaces: vec![iface("router", "192.0.2.10", true)],
            bridge_ports: vec![],
            changes: vec![],
            random_seed: 3,
        };
        let group: IpAddr = "239.1.2.3".parse().unwrap();
        let reporter: IpAddr = "192.0.2.11".parse().unwrap();
        let mut old = config.interfaces[0].clone();
        old.address = "192.0.2.20".parse().unwrap();
        old.router_version = 2;
        let mut query = encode_query(
            old.address,
            unspecified(old.address),
            &BTreeSet::new(),
            &old,
        )
        .unwrap();
        assert_eq!(query_version(&query, false).unwrap(), 2);
        // Keep Router Alert and add four valid NOP options before IGMP.
        query.splice(24..24, [1, 1, 1, 1]);
        query[0] = 0x47;
        let total_len = query.len() as u16;
        query[2..4].copy_from_slice(&total_len.to_be_bytes());
        query[10..12].fill(0);
        let checksum = codec::checksum(&query[..28]);
        query[10..12].copy_from_slice(&checksum.to_be_bytes());
        membership::decode(&query, false, 16).unwrap();
        assert_eq!(query_version(&query, false).unwrap(), 2);
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        actor
            .step(
                &config,
                1,
                Event::Receive {
                    ingress_port: "router/in".into(),
                    vid: 7,
                    bytes: query,
                },
            )
            .unwrap();
        assert!(actor.hosts[0].older_v2_until.is_some());
        for (at, leave) in [(2, false), (3, true)] {
            let bytes = encode_legacy_report(reporter, group, leave, 2).unwrap();
            membership::decode(&bytes, false, 16).unwrap();
            assert_eq!(packet_group(&bytes, false).unwrap(), group);
            assert_eq!(is_legacy_leave(&bytes, false).unwrap(), leave);
            let effects = actor
                .step(
                    &config,
                    at,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes,
                    },
                )
                .unwrap();
            assert!(actor.routers[0].groups.contains_key(&group));
            if leave {
                assert!(effects.tx.iter().any(|tx| tx.reason == "last_member_query"));
            }
        }
    }

    #[test]
    fn legacy_group_specific_query_does_not_elect_or_query_other_groups() {
        for (router, host, lower, group, other_group, source, old_version) in [
            (
                "192.0.2.10",
                "192.0.2.20",
                "192.0.2.1",
                "239.1.2.3",
                "239.1.2.4",
                "192.0.2.11",
                2,
            ),
            (
                "fe80::10",
                "fe80::20",
                "fe80::1",
                "ff3e::1234",
                "ff3e::1235",
                "2001:db8::11",
                1,
            ),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let other_group: IpAddr = other_group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("router", router, true), iface("host", host, false)],
                bridge_ports: vec![],
                changes: [group, other_group]
                    .into_iter()
                    .enumerate()
                    .map(|(index, group)| Change {
                        at_ps: 1,
                        interface: "host".into(),
                        socket: format!("socket-{index}"),
                        group,
                        filter: Some(Filter {
                            exclude: false,
                            sources: BTreeSet::from([source]),
                        }),
                    })
                    .collect(),
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            actor.step(&config, 1, Event::Change(0)).unwrap();
            actor.step(&config, 1, Event::Change(1)).unwrap();
            let mut sender = config.interfaces[0].clone();
            sender.address = lower.parse().unwrap();
            sender.router_version = old_version;
            let query = encode_query(sender.address, group, &BTreeSet::new(), &sender).unwrap();
            for ingress in ["router/in", "host/in"] {
                actor
                    .step(
                        &config,
                        2,
                        Event::Receive {
                            ingress_port: ingress.into(),
                            vid: 7,
                            bytes: query.clone(),
                        },
                    )
                    .unwrap();
            }
            assert!(actor.routers[0].other_querier_until.is_none());
            assert!(actor.routers[0].next_query.is_some());
            assert!(actor.hosts[1].responses.contains_key(&group));
            assert!(!actor.hosts[1].responses.contains_key(&other_group));
        }
    }

    #[test]
    fn legacy_report_removes_source_block_for_both_families() {
        for (router, reporter, group, source, version) in [
            ("192.0.2.10", "192.0.2.11", "239.1.2.3", "192.0.2.2", 2),
            ("fe80::10", "fe80::11", "ff3e::1234", "2001:db8::2", 1),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let reporter: IpAddr = reporter.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("router", router, true)],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            for (at, bytes) in [
                (
                    1,
                    encode_report(
                        reporter,
                        &[membership::Record {
                            kind: 2,
                            group,
                            sources: BTreeSet::from([source]),
                        }],
                    )
                    .unwrap(),
                ),
                (
                    2,
                    encode_legacy_report(reporter, group, false, version).unwrap(),
                ),
            ] {
                actor
                    .step(
                        &config,
                        at,
                        Event::Receive {
                            ingress_port: "router/in".into(),
                            vid: 7,
                            bytes,
                        },
                    )
                    .unwrap();
                if at == 1 {
                    assert!(actor.routers[0].groups[&group].blocked.contains(&source));
                    assert!(!actor.interested(&config, "router/in", 7, source, group, at));
                }
            }
            assert!(actor.routers[0].groups[&group].blocked.is_empty());
            assert!(actor.routers[0].groups[&group].sources.is_empty());
            assert!(actor.interested(&config, "router/in", 7, source, group, 2));
        }
    }

    #[test]
    fn mldv1_done_starts_last_listener_query() {
        let config = Config {
            interfaces: vec![iface("router", "fe80::10", true)],
            bridge_ports: vec![],
            changes: vec![],
            random_seed: 3,
        };
        let reporter: IpAddr = "fe80::11".parse().unwrap();
        let group: IpAddr = "ff3e::1234".parse().unwrap();
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        actor
            .step(
                &config,
                1,
                Event::Receive {
                    ingress_port: "router/in".into(),
                    vid: 7,
                    bytes: encode_legacy_report(reporter, group, false, 1).unwrap(),
                },
            )
            .unwrap();
        assert!(actor.routers[0].groups[&group].older_v1_until.is_some());
        let done = actor
            .step(
                &config,
                2,
                Event::Receive {
                    ingress_port: "router/in".into(),
                    vid: 7,
                    bytes: encode_legacy_report(reporter, group, true, 1).unwrap(),
                },
            )
            .unwrap();
        let query = done
            .tx
            .iter()
            .find(|tx| tx.reason == "last_member_query")
            .unwrap();
        assert_eq!(
            membership::decode(&query.ip_bytes, true, 16)
                .unwrap()
                .query
                .unwrap()
                .group,
            group
        );
        assert_eq!(actor.routers[0].pending_queries.len(), 1);
    }

    #[test]
    fn mldv1_host_sends_done_but_igmpv1_host_has_no_leave() {
        for (host, old_querier, group, source, expect_done) in [
            ("192.0.2.10", "192.0.2.1", "239.1.2.3", "192.0.2.11", false),
            ("fe80::10", "fe80::1", "ff3e::1234", "2001:db8::11", true),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("host", host, false)],
                bridge_ports: vec![],
                changes: vec![
                    Change {
                        at_ps: 1,
                        interface: "host".into(),
                        socket: "only".into(),
                        group,
                        filter: Some(Filter {
                            exclude: false,
                            sources: BTreeSet::from([source]),
                        }),
                    },
                    Change {
                        at_ps: 3,
                        interface: "host".into(),
                        socket: "only".into(),
                        group,
                        filter: None,
                    },
                ],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            actor.step(&config, 1, Event::Change(0)).unwrap();
            let mut querier = config.interfaces[0].clone();
            querier.address = old_querier.parse().unwrap();
            querier.router_version = 1;
            actor
                .step(
                    &config,
                    2,
                    Event::Receive {
                        ingress_port: "host/in".into(),
                        vid: 7,
                        bytes: encode_query(
                            querier.address,
                            unspecified(querier.address),
                            &BTreeSet::new(),
                            &querier,
                        )
                        .unwrap(),
                    },
                )
                .unwrap();
            let leave = actor.step(&config, 3, Event::Change(1)).unwrap();
            assert_eq!(leave.tx.iter().any(|tx| tx.reason == "leave"), expect_done);
            if expect_done {
                let done = leave.tx.iter().find(|tx| tx.reason == "leave").unwrap();
                let decoded = membership::decode(&done.ip_bytes, true, 16).unwrap();
                assert!(decoded.legacy && !decoded.is_query);
                assert!(is_legacy_leave(&done.ip_bytes, true).unwrap());
            }
        }
    }
    #[test]
    fn joins_retransmits_and_answers_source_queries_for_both_families() {
        for (host_addr, router_addr, group, source, other) in [
            (
                "192.0.2.10",
                "192.0.2.1",
                "239.1.2.3",
                "192.0.2.11",
                "192.0.2.12",
            ),
            (
                "fe80::10",
                "fe80::1",
                "ff3e::1234",
                "2001:db8::11",
                "2001:db8::12",
            ),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let other: IpAddr = other.parse().unwrap();
            let config = Config {
                interfaces: vec![
                    iface("host", host_addr, false),
                    iface("router", router_addr, true),
                ],
                bridge_ports: vec![],
                changes: vec![Change {
                    at_ps: 1,
                    interface: "host".into(),
                    socket: "a".into(),
                    group,
                    filter: Some(Filter {
                        exclude: false,
                        sources: BTreeSet::from([source]),
                    }),
                }],
                random_seed: 13,
            };
            let mut actor = Actor::new(&config);
            let start = actor.step(&config, 0, Event::Start).unwrap();
            assert_eq!(start.tx.len(), 1);
            let join = actor.step(&config, 1, Event::Change(0)).unwrap();
            assert_eq!(join.tx.len(), 1);
            let decoded = membership::decode(&join.tx[0].ip_bytes, source.is_ipv6(), 16).unwrap();
            assert_eq!(decoded.records[0].kind, 5);
            assert_eq!(decoded.records[0].sources, BTreeSet::from([source]));
            let due = join.next_deadline.unwrap();
            let retry = actor.step(&config, due, Event::Timer).unwrap();
            assert!(retry.tx.iter().any(|t| t.reason == "retransmit"));
            let query = encode_query(
                config.interfaces[1].address,
                group,
                &BTreeSet::from([source, other]),
                &config.interfaces[1],
            )
            .unwrap();
            let response = actor
                .step(
                    &config,
                    due + 1,
                    Event::Receive {
                        ingress_port: "host/in".into(),
                        vid: 7,
                        bytes: query,
                    },
                )
                .unwrap();
            let answer = actor
                .step(&config, response.next_deadline.unwrap(), Event::Timer)
                .unwrap();
            let report = answer
                .tx
                .iter()
                .find(|t| t.reason == "query_response")
                .unwrap();
            let decoded = membership::decode(&report.ip_bytes, source.is_ipv6(), 16).unwrap();
            assert_eq!(decoded.records[0].kind, 1);
            assert_eq!(decoded.records[0].sources, BTreeSet::from([source]));
        }
    }
    #[test]
    fn lower_querier_suppresses_and_timeout_restarts_queries() {
        let config = Config {
            interfaces: vec![iface("router", "192.0.2.10", true)],
            bridge_ports: vec![],
            changes: vec![],
            random_seed: 3,
        };
        let mut actor = Actor::new(&config);
        assert_eq!(actor.step(&config, 0, Event::Start).unwrap().tx.len(), 1);
        let other = encode_query(
            "192.0.2.1".parse().unwrap(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            &BTreeSet::new(),
            &config.interfaces[0],
        )
        .unwrap();
        let received = actor
            .step(
                &config,
                1,
                Event::Receive {
                    ingress_port: "router/in".into(),
                    vid: 7,
                    bytes: other,
                },
            )
            .unwrap();
        let deadline = received.next_deadline.unwrap();
        assert!(
            actor
                .step(&config, deadline - 1, Event::Timer)
                .unwrap()
                .tx
                .is_empty()
        );
        assert!(
            actor
                .step(&config, deadline, Event::Timer)
                .unwrap()
                .tx
                .iter()
                .any(|t| t.reason == "general_query")
        );
    }
    #[test]
    fn floating_query_codes_preserve_exact_advertised_timers() {
        assert_eq!(encode_float8(127), Some(127));
        assert_eq!(encode_float8(128), Some(0x80));
        assert_eq!(encode_float8(200), Some(0x89));
        assert_eq!(encode_float8(255), None);
        assert_eq!(encode_float16(32767), Some(32767));
        assert_eq!(encode_float16(32768), Some(0x8000));
        assert_eq!(encode_float16(40000), Some(0x8388));
        assert_eq!(encode_float16(40001), None);
        for address in ["192.0.2.1", "fe80::1"] {
            let mut interface = iface("router", address, true);
            interface.query_interval_ps = 200_000_000_000_000;
            if interface.address.is_ipv6() {
                interface.query_response_ps = 40_000_000_000_000;
            }
            let bytes = encode_query(
                interface.address,
                unspecified(interface.address),
                &BTreeSet::new(),
                &interface,
            )
            .unwrap();
            let decoded = membership::decode(&bytes, interface.address.is_ipv6(), 16)
                .unwrap()
                .query
                .unwrap();
            assert_eq!(decoded.interval_ps, interface.query_interval_ps);
            assert_eq!(decoded.response_ps, interface.query_response_ps);
        }
    }
    #[test]
    fn lower_querier_cancels_pending_last_member_retransmission() {
        for address in ["192.0.2.10", "fe80::10"] {
            let interface = iface("router", address, true);
            let config = Config {
                interfaces: vec![interface.clone()],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            let group = if interface.address.is_ipv6() {
                "ff3e::1234"
            } else {
                "239.1.2.3"
            }
            .parse()
            .unwrap();
            let mut out = Effects::default();
            actor
                .last_member_query(&config, 0, 1, group, BTreeSet::new(), &mut out)
                .unwrap();
            assert!(!actor.routers[0].pending_queries.is_empty());
            let lower = if interface.address.is_ipv6() {
                "fe80::1"
            } else {
                "192.0.2.1"
            }
            .parse()
            .unwrap();
            let bytes =
                encode_query(lower, unspecified(lower), &BTreeSet::new(), &interface).unwrap();
            actor
                .step(
                    &config,
                    2,
                    Event::Receive {
                        ingress_port: interface.ingress_port.clone(),
                        vid: interface.vid,
                        bytes,
                    },
                )
                .unwrap();
            assert!(actor.routers[0].pending_queries.is_empty());
            let out = actor
                .step(&config, interface.last_member_interval_ps + 1, Event::Timer)
                .unwrap();
            assert!(!out.tx.iter().any(|t| t.reason == "last_member_retransmit"));
        }
    }
    #[test]
    fn checkpoint_socket_filters_are_bound_to_exact_applied_change_prefix() {
        let interface = iface("host", "192.0.2.10", false);
        let group = "239.1.2.3".parse().unwrap();
        let config = Config {
            interfaces: vec![interface],
            bridge_ports: vec![],
            changes: vec![Change {
                at_ps: 1,
                interface: "host".into(),
                socket: "s".into(),
                group,
                filter: Some(Filter {
                    exclude: true,
                    sources: BTreeSet::new(),
                }),
            }],
            random_seed: 3,
        };
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        actor.validate_change_prefix(&config, 0).unwrap();
        assert!(actor.validate_change_prefix(&config, 1).is_err());
        actor.step(&config, 1, Event::Change(0)).unwrap();
        actor.validate_change_prefix(&config, 1).unwrap();
        assert!(actor.validate_change_prefix(&config, 0).is_err());
        actor.hosts[0].sockets.clear();
        actor.hosts[0].filters.clear();
        assert!(actor.validate_change_prefix(&config, 1).is_err());
    }
    #[test]
    fn shared_dual_stack_ingress_dispatches_only_matching_family() {
        let mut v4 = iface("router4", "192.0.2.10", true);
        let mut v6 = iface("router6", "fe80::10", true);
        v4.ingress_port = "shared/in".into();
        v6.ingress_port = "shared/in".into();
        let config = Config {
            interfaces: vec![v4, v6],
            bridge_ports: vec![],
            changes: vec![],
            random_seed: 7,
        };
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        for i in 0..2 {
            let iface = &config.interfaces[i];
            let bytes = encode_query(
                iface.address,
                unspecified(iface.address),
                &BTreeSet::new(),
                iface,
            )
            .unwrap();
            actor
                .step(
                    &config,
                    1 + i as u64,
                    Event::Receive {
                        ingress_port: "shared/in".into(),
                        vid: 7,
                        bytes,
                    },
                )
                .unwrap();
        }
    }
    #[test]
    fn mld_query_rejects_hop_limit_router_alert_source_and_checksum_damage() {
        let iface = iface("router", "fe80::1", true);
        let good = encode_query(
            iface.address,
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            &BTreeSet::new(),
            &iface,
        )
        .unwrap();
        assert!(membership::decode(&good, true, 16).is_ok());
        for offset in [7, 42, 8, good.len() - 1] {
            let mut bad = good.clone();
            bad[offset] ^= 1;
            assert!(
                membership::decode(&bad, true, 16).is_err(),
                "damage at {offset} accepted"
            );
        }
    }
    #[test]
    fn bridge_snooping_restores_source_filter_after_older_querier_timeout() {
        let group: IpAddr = "239.1.2.3".parse().unwrap();
        let a: IpAddr = "192.0.2.11".parse().unwrap();
        let b: IpAddr = "192.0.2.12".parse().unwrap();
        let mut config = Config {
            interfaces: vec![iface("host", "192.0.2.10", false)],
            bridge_ports: ["one", "two"]
                .into_iter()
                .map(|id| BridgePort {
                    bridge: "s".into(),
                    output_port: id.into(),
                    ingress_port: format!("{id}/in"),
                    vid: 7,
                })
                .collect(),
            changes: vec![Change {
                at_ps: 1,
                interface: "host".into(),
                socket: "a".into(),
                group,
                filter: Some(Filter {
                    exclude: false,
                    sources: BTreeSet::from([a]),
                }),
            }],
            random_seed: 11,
        };
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        let report = actor
            .step(&config, 1, Event::Change(0))
            .unwrap()
            .tx
            .remove(0)
            .ip_bytes;
        actor
            .step(
                &config,
                2,
                Event::Receive {
                    ingress_port: "one/in".into(),
                    vid: 7,
                    bytes: report,
                },
            )
            .unwrap();
        let eligible = BTreeSet::from(["one".into(), "two".into()]);
        assert_eq!(
            actor.candidates(
                "s",
                7,
                a,
                group,
                &eligible,
                "source",
                &BTreeSet::new(),
                false,
                3
            ),
            BTreeSet::from(["one".into()])
        );
        assert!(
            actor
                .candidates(
                    "s",
                    7,
                    b,
                    group,
                    &eligible,
                    "source",
                    &BTreeSet::new(),
                    false,
                    3
                )
                .is_empty()
        );
        let mut old = config.interfaces[0].clone();
        old.address = "192.0.2.2".parse().unwrap();
        old.router_version = 2;
        let query = encode_query(
            old.address,
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            &BTreeSet::new(),
            &old,
        )
        .unwrap();
        actor
            .step(
                &config,
                4,
                Event::Receive {
                    ingress_port: "two/in".into(),
                    vid: 7,
                    bytes: query,
                },
            )
            .unwrap();
        assert_eq!(
            actor.candidates(
                "s",
                7,
                b,
                group,
                &eligible,
                "source",
                &BTreeSet::new(),
                false,
                5
            ),
            eligible
        );
        let older_expiry = 4 + 2 * old.query_interval_ps + old.query_response_ps;
        actor.step(&config, older_expiry, Event::Timer).unwrap();
        assert!(
            actor
                .candidates(
                    "s",
                    7,
                    b,
                    group,
                    &eligible,
                    "source",
                    &BTreeSet::new(),
                    false,
                    older_expiry
                )
                .is_empty()
        );
        assert_eq!(
            actor.candidates(
                "s",
                7,
                a,
                group,
                &eligible,
                "source",
                &BTreeSet::new(),
                false,
                older_expiry
            ),
            BTreeSet::from(["one".into()])
        );
        config.changes.clear();
    }

    #[test]
    fn rfc9776_table8_and_table9_group_timer_vectors() {
        fn report(
            state: &mut RouterGroup,
            group: IpAddr,
            kind: u8,
            members: &[IpAddr],
            deadline: u64,
        ) -> GroupQueries {
            apply_group_record(
                &mut state.exclude_until,
                &mut state.sources,
                &mut state.blocked,
                &membership::Record {
                    kind,
                    group,
                    sources: members.iter().copied().collect(),
                },
                deadline,
            )
        }
        for (group, a, b, c) in [
            ("239.9.9.9", "192.0.2.1", "192.0.2.2", "192.0.2.3"),
            ("ff3e::9", "2001:db8::1", "2001:db8::2", "2001:db8::3"),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let [a, b, c] = [a, b, c].map(|s| s.parse::<IpAddr>().unwrap());
            let mut include = RouterGroup::default();
            report(&mut include, group, 1, &[a, c], 90);
            report(&mut include, group, 2, &[a, b], 100);
            assert_eq!(include.sources, BTreeMap::from([(a, 90)]));
            assert_eq!(include.blocked, BTreeSet::from([b]));
            assert_eq!(include.exclude_until, Some(100));
            let mut state = RouterGroup::default();
            report(&mut state, group, 2, &[a], 100);
            assert_eq!(state.blocked, BTreeSet::from([a]));
            report(&mut state, group, 2, &[b], 110);
            assert!(state.blocked.is_empty());
            assert_eq!(state.sources.get(&b), Some(&110));
            // Both A and B remain forwarded for two disjoint EXCLUDE listeners.
            assert!(!state.blocked.contains(&a));
            report(&mut state, group, 1, &[c], 120);
            assert_eq!(state.sources.get(&c), Some(&120));
            let queries = report(&mut state, group, 4, &[a, b], 130);
            assert_eq!(queries.sources, BTreeSet::from([a, b]));
            assert_eq!(state.sources.get(&b), Some(&110));
            assert_eq!(state.sources.get(&a), Some(&110));
            assert!(!state.sources.contains_key(&c));
            let queries = report(&mut state, group, 6, &[c], 140);
            assert_eq!(queries.sources, BTreeSet::from([c]));
            assert_eq!(state.sources.get(&c), Some(&130));
            let queries = report(&mut state, group, 3, &[a], 150);
            assert!(queries.group);
            assert_eq!(queries.sources, BTreeSet::from([b, c]));
            assert_eq!(state.sources.get(&a), Some(&150));
            expire_group(
                &mut state.exclude_until,
                &mut state.sources,
                &mut state.blocked,
                115,
            );
            assert_eq!(state.blocked, BTreeSet::from([b]));
            expire_group(
                &mut state.exclude_until,
                &mut state.sources,
                &mut state.blocked,
                130,
            );
            assert!(state.blocked.is_empty());
            assert_eq!(state.sources.get(&a), Some(&150));
            expire_group(
                &mut state.exclude_until,
                &mut state.sources,
                &mut state.blocked,
                150,
            );
            assert!(state.blocked.is_empty());
            assert!(state.sources.is_empty());
            assert_eq!(state.exclude_until, None);
        }
    }

    #[test]
    fn received_query_parameters_and_report_driven_query_merge() {
        let interface = iface("router", "192.0.2.10", true);
        let config = Config {
            interfaces: vec![interface.clone()],
            bridge_ports: vec![],
            changes: vec![],
            random_seed: 3,
        };
        let group: IpAddr = "239.9.9.9".parse().unwrap();
        let [a, b, c] =
            ["192.0.2.1", "192.0.2.2", "192.0.2.3"].map(|s| s.parse::<IpAddr>().unwrap());
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        let mut other = interface.clone();
        other.address = "192.0.2.9".parse().unwrap();
        other.robustness = 3;
        other.query_interval_ps = 8_000_000_000_000;
        let query = encode_query(
            other.address,
            unspecified(other.address),
            &BTreeSet::new(),
            &other,
        )
        .unwrap();
        actor
            .step(
                &config,
                1,
                Event::Receive {
                    ingress_port: "router/in".into(),
                    vid: 7,
                    bytes: query,
                },
            )
            .unwrap();
        assert_eq!(actor.routers[0].learned_robustness, Some(3));
        assert_eq!(
            actor.routers[0].learned_query_interval_ps,
            Some(8_000_000_000_000)
        );
        assert_eq!(
            actor.routers[0].other_querier_until,
            Some(24_500_000_000_001)
        );
        let send = |actor: &mut Actor, now, kind, members: &[IpAddr]| {
            let bytes = encode_report(
                "192.0.2.11".parse().unwrap(),
                &[membership::Record {
                    kind,
                    group,
                    sources: members.iter().copied().collect(),
                }],
            )
            .unwrap();
            actor
                .step(
                    &config,
                    now,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes,
                    },
                )
                .unwrap()
        };
        send(&mut actor, 2, 1, &[a, b]);
        assert_eq!(
            actor.routers[0].groups[&group].sources[&a],
            26_000_000_000_002
        );
        // The lower querier prevents this router from originating probes.
        assert!(send(&mut actor, 3, 6, &[a]).tx.is_empty());
        assert!(actor.routers[0].pending_queries.is_empty());
        actor
            .step(&config, 24_500_000_000_001, Event::Timer)
            .unwrap();
        assert_eq!(actor.routers[0].learned_query_interval_ps, None);
        assert_eq!(actor.routers[0].learned_robustness, None);
        // Refresh after the lower querier's source probe window has elapsed.
        send(&mut actor, 24_500_000_000_001, 1, &[a, b]);
        // Distinct BLOCKs merge into one source query.
        assert_eq!(send(&mut actor, 24_500_000_000_002, 6, &[a]).tx.len(), 1);
        assert_eq!(send(&mut actor, 24_500_000_000_003, 6, &[b]).tx.len(), 1);
        assert_eq!(actor.routers[0].pending_queries.len(), 1);
        assert_eq!(
            actor.routers[0].pending_queries[0].1,
            BTreeSet::from([a, b])
        );
        send(&mut actor, 24_500_000_000_004, 5, &[a]);
        assert_eq!(
            actor.routers[0].pending_queries[0].1,
            BTreeSet::from([a, b])
        );
        assert!(send(&mut actor, 24_500_000_000_005, 6, &[c]).tx.is_empty());
        assert_eq!(actor.routers[0].pending_queries.len(), 1);
    }

    #[test]
    fn refreshed_source_retransmits_with_s_flag_without_suppressing_other_probe() {
        for (address, reporter, group, a, b) in [
            (
                "192.0.2.10",
                "192.0.2.11",
                "239.1.2.3",
                "192.0.2.1",
                "192.0.2.2",
            ),
            (
                "fe80::10",
                "fe80::11",
                "ff3e::1234",
                "2001:db8::1",
                "2001:db8::2",
            ),
        ] {
            let interface = iface("router", address, true);
            let config = Config {
                interfaces: vec![interface],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let group: IpAddr = group.parse().unwrap();
            let a: IpAddr = a.parse().unwrap();
            let b: IpAddr = b.parse().unwrap();
            let reporter: IpAddr = reporter.parse().unwrap();
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            for (at, kind, sources) in [
                (1, 1, BTreeSet::from([a, b])),
                (2, 6, BTreeSet::from([a, b])),
                (3, 5, BTreeSet::from([a])),
            ] {
                let bytes = encode_report(
                    reporter,
                    &[membership::Record {
                        kind,
                        group,
                        sources,
                    }],
                )
                .unwrap();
                actor
                    .step(
                        &config,
                        at,
                        Event::Receive {
                            ingress_port: "router/in".into(),
                            vid: 7,
                            bytes,
                        },
                    )
                    .unwrap();
            }
            assert_eq!(
                actor.routers[0].pending_queries[0].1,
                BTreeSet::from([a, b])
            );
            let deadline = actor.routers[0].pending_queries[0].3;
            let retry = actor.step(&config, deadline, Event::Timer).unwrap();
            let queries: Vec<_> = retry
                .tx
                .iter()
                .filter(|tx| tx.reason == "last_member_retransmit")
                .map(|tx| {
                    (
                        membership::decode(&tx.ip_bytes, reporter.is_ipv6(), 16)
                            .unwrap()
                            .query
                            .unwrap(),
                        query_sources(&tx.ip_bytes, reporter.is_ipv6())
                            .unwrap()
                            .unwrap(),
                    )
                })
                .collect();
            assert_eq!(queries.len(), 2);
            assert!(
                queries
                    .iter()
                    .any(|(query, sources)| query.suppress && *sources == BTreeSet::from([a]))
            );
            assert!(
                queries
                    .iter()
                    .any(|(query, sources)| !query.suppress && *sources == BTreeSet::from([b]))
            );
        }
    }

    #[test]
    fn lower_group_specific_query_does_not_elect_another_querier() {
        for (address, lower, reporter, group, source) in [
            (
                "192.0.2.10",
                "192.0.2.1",
                "192.0.2.11",
                "239.1.2.3",
                "192.0.2.2",
            ),
            (
                "fe80::10",
                "fe80::1",
                "fe80::11",
                "ff3e::1234",
                "2001:db8::2",
            ),
        ] {
            let config = Config {
                interfaces: vec![iface("router", address, true)],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let reporter: IpAddr = reporter.parse().unwrap();
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            for (at, kind) in [(1, 1), (2, 6)] {
                actor
                    .step(
                        &config,
                        at,
                        Event::Receive {
                            ingress_port: "router/in".into(),
                            vid: 7,
                            bytes: encode_report(
                                reporter,
                                &[membership::Record {
                                    kind,
                                    group,
                                    sources: BTreeSet::from([source]),
                                }],
                            )
                            .unwrap(),
                        },
                    )
                    .unwrap();
            }
            assert_eq!(actor.routers[0].pending_queries.len(), 1);
            let mut lower_iface = config.interfaces[0].clone();
            lower_iface.address = lower.parse().unwrap();
            for (at, queried_group) in [(3, group), (4, unspecified(group))] {
                actor
                    .step(
                        &config,
                        at,
                        Event::Receive {
                            ingress_port: "router/in".into(),
                            vid: 7,
                            bytes: encode_query(
                                lower_iface.address,
                                queried_group,
                                &if queried_group == group {
                                    BTreeSet::from([source])
                                } else {
                                    BTreeSet::new()
                                },
                                &lower_iface,
                            )
                            .unwrap(),
                        },
                    )
                    .unwrap();
                if queried_group == group {
                    assert!(actor.routers[0].other_querier_until.is_none());
                    assert_eq!(actor.routers[0].pending_queries.len(), 1);
                } else {
                    assert!(actor.routers[0].other_querier_until.is_some());
                    assert!(actor.routers[0].pending_queries.is_empty());
                }
            }
        }
    }

    #[test]
    fn unanswered_source_probe_retransmits_then_expires_in_both_families() {
        for (address, reporter, group, source) in [
            ("192.0.2.10", "192.0.2.11", "239.9.9.9", "192.0.2.1"),
            ("fe80::10", "fe80::11", "ff3e::9", "2001:db8::1"),
        ] {
            let interface = iface("router", address, true);
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let config = Config {
                interfaces: vec![interface],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            for (now, kind) in [(1, 1), (2, 6)] {
                let bytes = encode_report(
                    reporter.parse().unwrap(),
                    &[membership::Record {
                        kind,
                        group,
                        sources: BTreeSet::from([source]),
                    }],
                )
                .unwrap();
                let effects = actor
                    .step(
                        &config,
                        now,
                        Event::Receive {
                            ingress_port: "router/in".into(),
                            vid: 7,
                            bytes,
                        },
                    )
                    .unwrap();
                if kind == 6 {
                    assert_eq!(effects.tx.len(), 1);
                    assert_eq!(effects.tx[0].reason, "last_member_query");
                    assert!(actor.interested(&config, "router/in", 7, source, group, now));
                }
            }
            let retry = actor.step(&config, 100_000_000_002, Event::Timer).unwrap();
            assert!(
                retry
                    .tx
                    .iter()
                    .any(|tx| tx.reason == "last_member_retransmit")
            );
            assert!(actor.interested(&config, "router/in", 7, source, group, 100_000_000_002));
            actor.step(&config, 200_000_000_002, Event::Timer).unwrap();
            assert!(!actor.interested(&config, "router/in", 7, source, group, 200_000_000_002));
        }
    }

    #[test]
    fn successive_host_changes_retransmit_all_unconfirmed_sources() {
        for (address, group, a, b) in [
            ("192.0.2.10", "239.1.2.3", "192.0.2.1", "192.0.2.2"),
            ("fe80::10", "ff3e::1234", "2001:db8::1", "2001:db8::2"),
        ] {
            let group: IpAddr = group.parse().unwrap();
            let a: IpAddr = a.parse().unwrap();
            let b: IpAddr = b.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("host", address, false)],
                bridge_ports: vec![],
                changes: [BTreeSet::from([a]), BTreeSet::from([a, b])]
                    .into_iter()
                    .enumerate()
                    .map(|(index, sources)| Change {
                        at_ps: index as u64 + 1,
                        interface: "host".into(),
                        socket: "socket".into(),
                        group,
                        filter: Some(Filter {
                            exclude: false,
                            sources,
                        }),
                    })
                    .collect(),
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            actor.step(&config, 1, Event::Change(0)).unwrap();
            let changed = actor.step(&config, 2, Event::Change(1)).unwrap();
            let immediate = membership::decode(&changed.tx[0].ip_bytes, group.is_ipv6(), 16)
                .unwrap()
                .records;
            assert!(
                immediate
                    .iter()
                    .any(|record| { record.kind == 5 && record.sources == BTreeSet::from([a, b]) })
            );
            let deadline = actor.hosts[0].pending[&group].deadline;
            let retry = actor.step(&config, deadline, Event::Timer).unwrap();
            let repeated = membership::decode(&retry.tx[0].ip_bytes, group.is_ipv6(), 16)
                .unwrap()
                .records;
            assert_eq!(repeated, immediate);
        }
    }

    #[test]
    fn mldv1_response_uses_received_maximum_response_delay() {
        let group: IpAddr = "ff3e::1234".parse().unwrap();
        let source: IpAddr = "2001:db8::1".parse().unwrap();
        let config = Config {
            interfaces: vec![iface("host", "fe80::10", false)],
            bridge_ports: vec![],
            changes: vec![Change {
                at_ps: 1,
                interface: "host".into(),
                socket: "socket".into(),
                group,
                filter: Some(Filter {
                    exclude: false,
                    sources: BTreeSet::from([source]),
                }),
            }],
            random_seed: 3,
        };
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        actor.step(&config, 1, Event::Change(0)).unwrap();
        let mut querier = config.interfaces[0].clone();
        querier.address = "fe80::1".parse().unwrap();
        querier.router_version = 1;
        actor.rng = 1;
        actor
            .step(
                &config,
                2,
                Event::Receive {
                    ingress_port: "host/in".into(),
                    vid: 7,
                    bytes: encode_query(querier.address, group, &BTreeSet::new(), &querier)
                        .unwrap(),
                },
            )
            .unwrap();
        assert!(actor.hosts[0].responses[&group].deadline <= 2 + querier.last_member_interval_ps);
    }

    #[test]
    fn older_listener_keeps_current_allow_and_strips_to_ex_sources() {
        for (router, reporter, group, source, version) in [
            ("192.0.2.10", "192.0.2.11", "239.1.2.3", "192.0.2.2", 2),
            ("fe80::10", "fe80::11", "ff3e::1234", "2001:db8::2", 1),
        ] {
            let reporter: IpAddr = reporter.parse().unwrap();
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("router", router, true)],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            actor
                .step(
                    &config,
                    1,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes: encode_legacy_report(reporter, group, false, version).unwrap(),
                    },
                )
                .unwrap();
            let original_expiry = actor.routers[0].groups[&group].exclude_until.unwrap();
            actor
                .step(
                    &config,
                    2,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes: encode_report(
                            reporter,
                            &[membership::Record {
                                kind: 5,
                                group,
                                sources: BTreeSet::from([source]),
                            }],
                        )
                        .unwrap(),
                    },
                )
                .unwrap();
            actor.step(&config, original_expiry, Event::Timer).unwrap();
            assert!(actor.interested(&config, "router/in", 7, source, group, original_expiry));

            actor
                .step(
                    &config,
                    original_expiry + 1,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes: encode_legacy_report(reporter, group, false, version).unwrap(),
                    },
                )
                .unwrap();
            let refreshed_expiry = actor.routers[0].groups[&group].exclude_until.unwrap();
            actor
                .step(
                    &config,
                    original_expiry + 2,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes: encode_report(
                            reporter,
                            &[membership::Record {
                                kind: 4,
                                group,
                                sources: BTreeSet::from([source]),
                            }],
                        )
                        .unwrap(),
                    },
                )
                .unwrap();
            assert!(actor.routers[0].groups[&group].blocked.is_empty());
            assert!(
                !actor.routers[0].groups[&group]
                    .sources
                    .contains_key(&source)
            );
            assert!(actor.routers[0].groups[&group].exclude_until.unwrap() > refreshed_expiry);
        }
    }

    #[test]
    fn to_in_with_older_members_respects_igmp_v1_v2_and_mld_v1() {
        for (router, reporter, group, source, version, expected) in [
            (
                "192.0.2.10",
                "192.0.2.11",
                "239.1.2.3",
                "192.0.2.2",
                1,
                false,
            ),
            (
                "192.0.2.10",
                "192.0.2.11",
                "239.1.2.3",
                "192.0.2.2",
                2,
                true,
            ),
            ("fe80::10", "fe80::11", "ff3e::1234", "2001:db8::2", 1, true),
        ] {
            let reporter: IpAddr = reporter.parse().unwrap();
            let group: IpAddr = group.parse().unwrap();
            let source: IpAddr = source.parse().unwrap();
            let config = Config {
                interfaces: vec![iface("router", router, true)],
                bridge_ports: vec![],
                changes: vec![],
                random_seed: 3,
            };
            let mut actor = Actor::new(&config);
            actor.step(&config, 0, Event::Start).unwrap();
            actor
                .step(
                    &config,
                    1,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes: encode_legacy_report(reporter, group, false, version).unwrap(),
                    },
                )
                .unwrap();
            let original_expiry = actor.routers[0].groups[&group].exclude_until.unwrap();
            actor
                .step(
                    &config,
                    2,
                    Event::Receive {
                        ingress_port: "router/in".into(),
                        vid: 7,
                        bytes: encode_report(
                            reporter,
                            &[membership::Record {
                                kind: 3,
                                group,
                                sources: BTreeSet::from([source]),
                            }],
                        )
                        .unwrap(),
                    },
                )
                .unwrap();
            actor.step(&config, original_expiry, Event::Timer).unwrap();
            assert_eq!(
                actor.interested(&config, "router/in", 7, source, group, original_expiry),
                expected,
                "version {version} for {group}"
            );
        }
    }

    #[test]
    fn snooper_igmp_v2_to_in_retains_source_after_legacy_group_lease() {
        let group: IpAddr = "239.1.2.3".parse().unwrap();
        let source: IpAddr = "192.0.2.2".parse().unwrap();
        let reporter: IpAddr = "192.0.2.11".parse().unwrap();
        let config = Config {
            interfaces: vec![iface("router", "192.0.2.10", true)],
            bridge_ports: vec![BridgePort {
                bridge: "bridge".into(),
                output_port: "bridge/out".into(),
                ingress_port: "bridge/in".into(),
                vid: 7,
            }],
            changes: vec![],
            random_seed: 3,
        };
        let mut actor = Actor::new(&config);
        actor.step(&config, 0, Event::Start).unwrap();
        actor
            .step(
                &config,
                1,
                Event::Receive {
                    ingress_port: "bridge/in".into(),
                    vid: 7,
                    bytes: encode_legacy_report(reporter, group, false, 2).unwrap(),
                },
            )
            .unwrap();
        let key = ("bridge".into(), 7, group, "bridge/out".into());
        let original_expiry = actor.snooped[&key].exclude_until.unwrap();
        actor
            .step(
                &config,
                2,
                Event::Receive {
                    ingress_port: "bridge/in".into(),
                    vid: 7,
                    bytes: encode_report(
                        reporter,
                        &[membership::Record {
                            kind: 3,
                            group,
                            sources: BTreeSet::from([source]),
                        }],
                    )
                    .unwrap(),
                },
            )
            .unwrap();
        actor.step(&config, original_expiry, Event::Timer).unwrap();
        assert!(
            actor.snooped[&key]
                .sources
                .get(&source)
                .is_some_and(|t| *t > original_expiry)
        );
    }
}
