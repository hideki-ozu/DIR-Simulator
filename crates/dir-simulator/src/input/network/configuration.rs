//! Validated scope of the fully centralized local management adapter.
use crate::types::{
    Diagnostic,
    network::{PreparedNetwork, configuration::*},
};
use serde_json::Value;
use std::collections::BTreeSet;
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(message).with_target("/qcc")
}
pub(crate) fn prepare(value: &Value, n: &PreparedNetwork) -> Result<PreparedQcc> {
    let c: QccConfig =
        serde_json::from_value(value.clone()).map_err(|e| error(format!("Qcc config: {e}")))?;
    if c.architecture != "fully_centralized"
        || c.transport != "dir.cuc-cnc.local.v1"
        || c.standard_edition != "IEEE802.1Q-2022"
        || c.cycle_ps.0 == 0
        || c.management_latency_ps.0 == 0
        || c.search_budget == 0
        || c.search_budget > 1_000_000
        || c.max_streams == 0
        || c.max_streams > 256
        || c.requirements.len() > c.max_streams as usize
        || c.operations.len() > 10000
    {
        return Err(error(
            "unsupported Qcc architecture/transport/edition or finite bounds",
        ));
    }
    if n.tsn.is_none() {
        return Err(error("Qcc requires a TSN coordinator"));
    }
    let bindings = n
        .bindings
        .as_ref()
        .ok_or_else(|| error("Qcc requires final physical resource bindings"))?;
    let ports: Vec<ManagedPort> = n
        .ethernet
        .directions
        .iter()
        .enumerate()
        .map(|(i, d)| {
            Ok(ManagedPort {
                port: d.from_port.clone(),
                direction: i,
                source: d.source,
                destination: d.destination,
                bitrate_bps: d.bitrate_bps,
                propagation_ps: d.delay_ps,
                source_processing_ps: n.ethernet.devices[d.source].tx_processing_delay_ps,
                capacity_frames: n.ethernet.devices[d.source].queue_capacity,
                class_capacity_frames: std::array::from_fn(|class| {
                    n.ethernet
                        .outputs
                        .iter()
                        .find(|o| o.port == d.from_port)
                        .and_then(|o| o.queues.iter().find(|q| q.priority == class as u8))
                        .map_or(n.ethernet.devices[d.source].queue_capacity, |q| {
                            q.capacity_frames
                        })
                }),
                class_capacity_bytes: std::array::from_fn(|class| {
                    n.ethernet
                        .outputs
                        .iter()
                        .find(|o| o.port == d.from_port)
                        .and_then(|o| o.queues.iter().find(|q| q.priority == class as u8))
                        .and_then(|q| q.capacity_bytes)
                }),
                calendar_domain: if bindings.domains[bindings.ports[i].physical_domain].duplex
                    == crate::types::network::bindings::Duplex::Half
                {
                    bindings.ports[i].physical_domain
                } else {
                    bindings.domains.len() + i
                },
                writable_schedule: n
                    .tsn
                    .as_ref()
                    .unwrap()
                    .outputs
                    .iter()
                    .any(|o| o.port == d.from_port)
                    && !n
                        .tsn
                        .as_ref()
                        .unwrap()
                        .outputs
                        .iter()
                        .any(|o| o.port == d.from_port && o.tas.is_some())
                    && !n
                        .tsn
                        .as_ref()
                        .unwrap()
                        .updates
                        .iter()
                        .any(|u| u.port == d.from_port),
                clock_offset_ps: n
                    .clocks
                    .as_ref()
                    .map_or(0, |c| c.initial[d.source].offset_ps),
                clock_rate_num: n
                    .clocks
                    .as_ref()
                    .map_or(1, |c| c.initial[d.source].rate_num),
                clock_fractional: false,
                clock_rate_den: n
                    .clocks
                    .as_ref()
                    .map_or(1, |c| c.initial[d.source].rate_den),
                destination_processing_ps: n.ethernet.devices[d.destination]
                    .rx_processing_delay_ps
                    .checked_add(n.ethernet.devices[d.destination].forward_delay_ps)
                    .ok_or_else(|| error("Qcc processing delay overflow"))?,
            })
        })
        .collect::<Result<_>>()?;
    let mut streams = Vec::new();
    let mut ids = BTreeSet::new();
    let mut generators = BTreeSet::new();
    let mut priorities = 0;
    for r in &c.requirements {
        if r.id.is_empty()
            || !ids.insert(r.id.clone())
            || !generators.insert(r.generator.clone())
            || r.listeners.is_empty()
            || r.listeners.len() > 64
            || r.max_frames_per_interval == 0
            || r.max_frames_per_interval > 4096
            || r.max_frame_size == 0
            || r.interval.ps() != Some(c.cycle_ps.0)
            || r.earliest_transmit_offset_ps.0 > r.latest_transmit_offset_ps.0
            || r.latest_transmit_offset_ps.0 >= c.cycle_ps.0
            || r.jitter_ps.0 >= c.cycle_ps.0
            || r.num_seamless_trees > 8
        {
            return Err(error(
                "invalid Qcc stream or unsupported interval (this cyclic planner requires Interval=cycle)",
            ));
        }
        let Some(generator) = n.ethernet.generators.iter().find(|g| g.id == r.generator) else {
            // Gateway and other non-enumerated sources need a declared producer
            // envelope. Their graph can execute while this guarantee is rejected.
            let paths = r
                .paths
                .iter()
                .map(|path| {
                    path.ports
                        .iter()
                        .map(|name| {
                            ports
                                .iter()
                                .find(|p| p.port == *name)
                                .map(|p| p.direction)
                                .ok_or_else(|| error("unknown Qcc path port"))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?;
            let source = paths
                .first()
                .and_then(|p| p.first())
                .map_or(0, |d| ports[*d].source);
            streams.push(PreparedStream {
                requirement: r.clone(),
                priority: 0,
                mac_bytes: 22 + u64::from(r.max_frame_size).max(46),
                source,
                paths,
                demands: Vec::new(),
                unproven: Some(UnprovenBound::ProducerEnvelope),
            });
            continue;
        };
        let data = generator.frame.data_hex.len() / 2;
        if data > r.max_frame_size as usize {
            return Err(error(
                "actual client payload exceeds TrafficSpecification.MaxFrameSize",
            ));
        }
        let tsn = n.tsn.as_ref().unwrap();
        let mut unproven = None;
        let mut paths = Vec::new();
        let mut receivers = std::collections::BTreeMap::<usize, BTreeSet<usize>>::new();
        let mut listener_counts = std::collections::BTreeMap::new();
        for route in &r.paths {
            if !r.listeners.contains(&route.listener)
                || route.ports.is_empty()
                || route.ports.len() > 256
            {
                return Err(error("invalid Qcc listener path"));
            }
            let mut at = generator.source;
            let mut seen = BTreeSet::new();
            let mut path = Vec::new();
            for (hop, key) in route.ports.iter().enumerate() {
                let port = ports
                    .iter()
                    .find(|p| p.port == *key)
                    .ok_or_else(|| error("unknown Qcc path port"))?;
                if port.source != at || !seen.insert(port.source) {
                    return Err(error("discontinuous or looping Qcc path"));
                }
                if !port.writable_schedule {
                    unproven = Some(UnprovenBound::ScheduleOwnership);
                }
                path.push(port.direction);
                at = if let Some(segment) = n
                    .shared_lans
                    .iter()
                    .find(|s| s.members.contains(&port.direction))
                {
                    let receiver = if let Some(next) = route.ports.get(hop + 1) {
                        ports.iter().find(|p| p.port == *next).map(|p| p.source)
                    } else {
                        n.ethernet
                            .devices
                            .iter()
                            .position(|d| d.id == route.listener)
                    }
                    .ok_or_else(|| error("Qcc shared path intended receiver absent"))?;
                    if !segment.members.iter().any(|d| ports[*d].source == receiver) {
                        return Err(error(
                            "Qcc shared path intended receiver is outside physical domain",
                        ));
                    }
                    receiver
                } else {
                    port.destination
                };
                receivers.entry(port.direction).or_default().insert(at);
                if seen.contains(&at)
                    || hop + 1 < route.ports.len() && n.ethernet.devices[at].kind != "switch"
                    || hop + 1 == route.ports.len() && n.ethernet.devices[at].kind != "endpoint"
                {
                    return Err(error(
                        "Qcc route requires loop-free switch relays and endpoint listener",
                    ));
                }
            }
            if n.ethernet.devices[at].id != route.listener {
                return Err(error("Qcc path does not end at declared listener"));
            }
            *listener_counts
                .entry(route.listener.clone())
                .or_insert(0usize) += 1;
            paths.push(path);
        }
        if r.listeners.iter().collect::<BTreeSet<_>>().len() != r.listeners.len()
            || r.listeners
                .iter()
                .any(|l| listener_counts.get(l) != Some(&(r.num_seamless_trees.max(1) as usize)))
            || listener_counts.len() != r.listeners.len()
        {
            return Err(error("Qcc paths do not cover every listener/tree"));
        }
        let replication = n
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .and_then(|rep| {
                rep.streams
                    .iter()
                    .find(|s| s.source_generator == r.generator)
            });
        if r.num_seamless_trees.max(1) > 1 && replication.is_none() {
            unproven = Some(UnprovenBound::ReplicationRoute);
        }
        if r.listeners.len() > 1
            && replication.is_none()
            && !generator.frame.dst_mac.starts_with("01:")
            && !generator.frame.dst_mac.starts_with("33:33:")
        {
            unproven = Some(UnprovenBound::ReplicationRoute);
        }
        if let Some(rep) = replication {
            let actual: BTreeSet<Vec<usize>> = match &rep.placement {
                crate::types::network::ReplicationPlacement::LegacyEndpoint => {
                    rep.paths.iter().map(|p| p.directions.clone()).collect()
                }
                crate::types::network::ReplicationPlacement::BridgePair(pair) => rep
                    .paths
                    .iter()
                    .map(|p| {
                        pair.prefix
                            .directions
                            .iter()
                            .chain(&p.directions)
                            .chain(&pair.suffix.directions)
                            .copied()
                            .collect()
                    })
                    .collect(),
            };
            if actual != paths.iter().cloned().collect() {
                unproven = Some(UnprovenBound::ReplicationRoute);
            }
        }
        let vid = generator
            .source_vlan_id
            .ok_or_else(|| error("Qcc generator lacks explicit VID"))?;
        let listener = paths[0]
            .last()
            .map(|i| ports[*i].destination)
            .ok_or_else(|| error("empty Qcc path"))?;
        if r.listeners.len() == 1
            && n.shared_lans.is_empty()
            && n.ethernet.devices[listener].mac.as_ref() != Some(&generator.frame.dst_mac)
        {
            unproven = Some(UnprovenBound::Classification);
        }
        for i in paths.iter().flatten() {
            let link = &n.ethernet.directions[*i];
            if n.provider_l2.as_ref().is_some_and(|p| {
                p.ports.contains_key(&link.from_port)
                    || p.ports.values().any(|p| p.port == link.to_port)
            }) {
                continue;
            }
            let output = n
                .ethernet
                .port_policies
                .iter()
                .find(|p| p.port == link.from_port)
                .ok_or_else(|| error("Qcc egress VLAN policy absent"))?;
            let ingress = n
                .ethernet
                .port_policies
                .iter()
                .find(|p| p.ingress == link.to_port)
                .ok_or_else(|| error("Qcc ingress VLAN policy absent"))?;
            let tagged = *output
                .vlans
                .get(&vid)
                .ok_or_else(|| error("Qcc egress VLAN membership absent"))?;
            if !ingress.vlans.contains_key(&vid)
                || tagged && ingress.admit == "untagged_only"
                || !tagged && ingress.admit == "tagged_only"
                || !tagged
                    && (ingress.default_priority != generator.priority || ingress.pvid != vid)
            {
                unproven = Some(UnprovenBound::Classification);
            }
        }
        let classes = match hop_classes(
            n,
            &paths,
            &receivers,
            &generator.frame,
            vid,
            generator.priority,
        ) {
            Ok((classes, ingress)) => {
                let max_wire = 14
                    + if n.provider_l2.is_some() { 8 } else { 4 }
                    + u64::from(r.max_frame_size).max(46)
                    + 4
                    + if replication.is_some_and(|rep| {
                        rep.wire_format
                            == crate::types::network::ReplicationWireFormat::Rtag8021cb2017
                    }) {
                        6
                    } else {
                        0
                    };
                if tsn.streams.iter().any(|s| {
                    ingress.contains(&s.key)
                        && (s.gate.is_some() || s.meter.is_some() || s.max_sdu_bytes < max_wire)
                }) {
                    unproven = Some(UnprovenBound::IngressServiceEnvelope);
                }
                classes
            }
            Err(reason) => {
                unproven = Some(reason);
                std::collections::BTreeMap::new()
            }
        };
        let rtag = replication.is_some_and(|rep| {
            rep.wire_format == crate::types::network::ReplicationWireFormat::Rtag8021cb2017
        });
        let mut demands = Vec::new();
        for direction in paths.iter().flatten().copied().collect::<BTreeSet<_>>() {
            let (priority, demand_vid) = classes
                .get(&direction)
                .copied()
                .unwrap_or((generator.priority, vid));
            let binding = &bindings.ports[direction];
            if binding
                .protocol_adapters
                .iter()
                .any(|id| id != "mac_merge_lldp")
            {
                // Protocol callbacks may submit future control work independently
                // of a CUC stream. No declared per-cycle bypass-service envelope
                // means this output cannot promise the requested lossless delay.
                unproven = Some(UnprovenBound::ControlServiceEnvelope);
            }
            if pause_service_unproven(n, direction) {
                unproven = Some(UnprovenBound::ControlServiceEnvelope);
            }
            let port = &ports[direction];
            let joint: Vec<_> = ports
                .iter()
                .filter(|p| p.calendar_domain == port.calendar_domain)
                .collect();
            if joint.iter().any(|p| !p.writable_schedule) {
                unproven = Some(UnprovenBound::ScheduleOwnership);
            }
            if joint.iter().any(|p| {
                bindings.ports[p.direction].discipline
                    == crate::types::network::bindings::ClockDiscipline::Gptp
            }) {
                unproven = Some(UnprovenBound::ClockDisciplineEnvelope);
            }
            let output = tsn.outputs.iter().find(|o| o.port == port.port);
            // Provider components support at most one C-tag inside one S-tag.
            // Reserving this declared maximum on ordinary portions is conservative.
            let tag_octets = if n.provider_l2.is_some() { 8 } else { 4 };
            let mac_bytes =
                14 + tag_octets + (r.max_frame_size as u64).max(46) + 4 + if rtag { 6 } else { 0 };
            let mut occupied =
                crate::runtime::network::configuration::occupied_ps(mac_bytes, port.bitrate_bps)
                    .map_err(|_| error("Qcc wire occupancy overflow"))?;
            let mut propagation = port.propagation_ps;
            match physical_bound(n, direction, mac_bytes, occupied) {
                Ok((wire, delay)) => {
                    occupied = wire;
                    propagation = delay;
                }
                Err(reason) => {
                    unproven = Some(reason);
                }
            }
            if let Some(preemption) = n.extensions.as_ref().and_then(|e| e.preemption.as_ref()) {
                if let Some(o) = preemption
                    .outputs
                    .get(direction)
                    .filter(|o| o.preemptable_mask & (1 << priority) != 0)
                {
                    let fragments = mac_bytes.div_ceil(o.fragment_payload_bytes);
                    let extra = fragments.saturating_sub(1);
                    occupied = occupied
                        .checked_add(
                            crate::runtime::network::configuration::occupied_ps(
                                extra
                                    .checked_mul(o.fragment_overhead_bytes)
                                    .ok_or_else(|| error("Qcc fragment bound overflow"))?,
                                port.bitrate_bps,
                            )
                            .map_err(|_| error("Qcc fragment occupancy overflow"))?,
                        )
                        .and_then(|v| v.checked_add(extra.checked_mul(o.resume_gap_ps)?))
                        .ok_or_else(|| error("Qcc fragment bound overflow"))?;
                }
            }
            // Managed windows close express/background queues, preventing further
            // interruptions. Bound every minimum-size Clause99 fragment overhead.
            if binding.mac == crate::types::network::bindings::MacOwner::Clause99 {
                let fragments = mac_bytes.div_ceil(60);
                occupied = occupied
                    .checked_add(
                        crate::runtime::network::configuration::occupied_ps(
                            fragments
                                .checked_mul(24)
                                .ok_or_else(|| error("Qcc Merge bound overflow"))?,
                            port.bitrate_bps,
                        )
                        .map_err(|_| error("Qcc Merge occupancy overflow"))?,
                    )
                    .ok_or_else(|| error("Qcc Merge occupancy overflow"))?;
            }
            if let Some(cbs) = output.and_then(|o| o.cbs[priority as usize].as_ref()) {
                // Reserve physical send plus complete worst negative-credit recovery.
                // Slopes remain physical; gate is open for the entire reservation.
                let recovery = (occupied as u128) * (cbs.link_bps - cbs.idle_bps) as u128;
                occupied = occupied
                    .checked_add(
                        u64::try_from(recovery.div_ceil(cbs.idle_bps as u128))
                            .map_err(|_| error("Qcc CBS recovery overflow"))?,
                    )
                    .ok_or_else(|| error("Qcc CBS occupancy overflow"))?;
            }
            if binding.discipline == crate::types::network::bindings::ClockDiscipline::Gptp {
                unproven = Some(UnprovenBound::ClockDisciplineEnvelope);
            }
            let destination_processing_ps = receivers[&direction]
                .iter()
                .map(|receiver| {
                    n.ethernet.devices[*receiver]
                        .rx_processing_delay_ps
                        .checked_add(n.ethernet.devices[*receiver].forward_delay_ps)
                        .ok_or_else(|| error("Qcc receiver processing overflow"))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .max()
                .unwrap_or(0);
            demands.push(TransmissionDemand {
                direction,
                priority,
                vid: demand_vid,
                mac_bytes,
                occupied_ps: occupied,
                propagation_ps: propagation,
                destination_processing_ps,
                start_jitter_ps: n
                    .phy_attachments
                    .iter()
                    .find(|a| a.directions.contains(&direction))
                    .map_or(0, |a| {
                        let grid = a.config.medium.channel.grid;
                        u64::try_from(2_000_000_000_000u128.div_ceil(
                            u128::from(grid.symbol_rate_hz) * u128::from(grid.samples_per_ui),
                        ))
                        .unwrap_or(u64::MAX)
                    }),
                merge_control: merge_control_bound(n, direction)
                    .map_err(|reason| {
                        unproven = Some(reason);
                    })
                    .ok()
                    .flatten(),
            });
        }
        let mac_bytes = demands.iter().map(|d| d.mac_bytes).max().unwrap_or(0);
        priorities |= 1 << generator.priority;
        for demand in &demands {
            priorities |= 1 << demand.priority;
        }
        streams.push(PreparedStream {
            requirement: r.clone(),
            priority: generator.priority,
            mac_bytes,
            source: generator.source,
            paths,
            demands,
            unproven,
        });
    }
    let mut opids = BTreeSet::new();
    let mut previous = 0;
    for (i, op) in c.operations.iter().enumerate() {
        if op.id().is_empty() || !opids.insert(op.id()) || i > 0 && op.at() < previous {
            return Err(error(
                "Qcc operations must have unique IDs and be time ordered",
            ));
        }
        previous = op.at();
        match op {
            ManagementOperation::Prepare {
                add,
                remove,
                activate_at_ps,
                timeout_at_ps,
                ..
            } => {
                if add.iter().chain(remove).any(|id| !ids.contains(id))
                    || add.iter().any(|id| remove.contains(id))
                    || add.iter().collect::<BTreeSet<_>>().len() != add.len()
                    || remove.iter().collect::<BTreeSet<_>>().len() != remove.len()
                {
                    return Err(error("invalid Qcc transaction stream set"));
                }
                validate_times(&c, op.at(), activate_at_ps.0, timeout_at_ps.0)?;
            }
            ManagementOperation::Rollback {
                activate_at_ps,
                timeout_at_ps,
                ..
            } => validate_times(&c, op.at(), activate_at_ps.0, timeout_at_ps.0)?,
            ManagementOperation::Cancel { transaction, .. } if transaction.is_empty() => {
                return Err(error("empty Qcc cancel identity"));
            }
            ManagementOperation::Participant { port, .. }
                if !ports.iter().any(|p| p.port == *port) =>
            {
                return Err(error("unknown Qcc participant port"));
            }
            _ => {}
        }
    }
    let stream_index = streams
        .iter()
        .enumerate()
        .map(|(i, s)| (s.requirement.id.clone(), i))
        .collect();
    let generator_index = streams
        .iter()
        .enumerate()
        .map(|(i, s)| (s.requirement.generator.clone(), i))
        .collect();
    Ok(PreparedQcc {
        config: c,
        ports,
        streams,
        stream_index,
        generator_index,
        reserved_priorities: priorities,
    })
}
fn validate_times(c: &QccConfig, now: u64, activate: u64, timeout: u64) -> Result<()> {
    if activate <= now
        || activate % c.cycle_ps.0 != 0
        || timeout <= now
        || timeout >= activate
        || now.checked_add(c.management_latency_ps.0).is_none()
    {
        return Err(error("invalid Qcc activation/timeout/deadline"));
    }
    Ok(())
}

/// A source-level envelope for the rectangular, noiseless numerical channel.
/// More general numerical channels still execute; they do not gain a lossless
/// management guarantee merely because one sampled frame happened to decode.
fn physical_bound(
    n: &PreparedNetwork,
    direction: usize,
    mac: u64,
    ordinary: u64,
) -> std::result::Result<(u64, u64), UnprovenBound> {
    use crate::runtime::ethernet::phy::{Q30, medium::PhysicalMode};
    use crate::types::network::bindings::PhysicalOwner;
    let bindings = n.bindings.as_ref().unwrap();
    let domain = &bindings.domains[bindings.ports[direction].physical_domain];
    let port = &n.ethernet.directions[direction];
    match domain.owner {
        PhysicalOwner::IdealPointToPoint => Ok((ordinary, port.delay_ps)),
        PhysicalOwner::IdealShared { segment } => Ok((ordinary, n.shared_lans[segment].delay_ps)),
        PhysicalOwner::ConfiguredMedia { link } if !domain.retry_owned_by_medium => {
            let physical = &n.ethernet.media.as_ref().unwrap().physical_links[link];
            let (tx, rx) = if physical.directions[0] == direction {
                (physical.a_phy.tx_latency_ps, physical.b_phy.rx_latency_ps)
            } else {
                (physical.b_phy.tx_latency_ps, physical.a_phy.rx_latency_ps)
            };
            let delay = port
                .delay_ps
                .checked_add(tx)
                .and_then(|v| v.checked_add(rx))
                .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
            Ok((ordinary, delay))
        }
        PhysicalOwner::ConfiguredMedia { .. } => Err(UnprovenBound::PhysicalNoiseOrRetry),
        PhysicalOwner::Sampled { attachment } => {
            let a = &n.phy_attachments[attachment];
            let m = &a.config.medium;
            if m.half_duplex || m.channel.noise_rms_uv.iter().any(|v| *v != 0) {
                return Err(UnprovenBound::PhysicalNoiseOrRetry);
            }
            if m.gigabit_raw_merge {
                if mac > m.maximum_mac_bytes as u64 {
                    return Err(UnprovenBound::PhysicalSignalEnvelope);
                }
                return raw_transport_bound(n, direction).map(|(_, delay)| (ordinary, delay));
            }
            if m.physical_mode != PhysicalMode::ProjectNumerical {
                return Err(UnprovenBound::PhysicalStartup);
            }
            let binding = a
                .config
                .bindings
                .iter()
                .find(|b| b.from_port == port.from_port)
                .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
            let from = m
                .endpoints
                .iter()
                .position(|e| e.id == binding.from_port)
                .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
            let to = m
                .endpoints
                .iter()
                .position(|e| e.id == binding.to_port)
                .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
            let tx = &m.endpoints[from].transmitter;
            let rx = &m.endpoints[to].receiver;
            let path = m.channel.paths[to][from]
                .as_ref()
                .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
            if tx.filter_taps_q30 != [Q30]
                || tx.transition_ui_ppm != 0
                || tx.clock_ppm != 0
                || rx.frontend_taps_q30 != [Q30]
                || rx.clock_ppm != 0
                || rx.max_tracking_ppm != 0
                || rx.ffe_taps != 1
                || rx.echo_taps != 0
                || rx.adaptation_q30 != 0
                || rx.minimum_training_symbols > 256
                || rx.max_no_transition_symbols < 2
                || tx.peak_uv != rx.peak_uv
                || m.carrier_threshold_uv >= tx.peak_uv
                || m.channel.paths[to]
                    .iter()
                    .enumerate()
                    .any(|(source, path)| source != from && path.is_some())
                || m.channel.clip_uv < tx.peak_uv
                || path.taps_q30 != [Q30]
                || mac > m.maximum_mac_bytes as u64
            {
                return Err(UnprovenBound::PhysicalSignalEnvelope);
            }
            let levels = (1i128 << (rx.adc_bits - 1)) - 1;
            let code = (i128::from(tx.peak_uv) * levels + i128::from(rx.adc_full_scale_uv) / 2)
                / i128::from(rx.adc_full_scale_uv);
            let quantized = code * i128::from(rx.adc_full_scale_uv) / levels;
            let error = (quantized - i128::from(tx.peak_uv)).unsigned_abs();
            if error * 1_000_000 > tx.peak_uv as u128 * rx.lock_error_ppm as u128 {
                return Err(UnprovenBound::PhysicalSignalEnvelope);
            }
            // Pilot (16 B), preamble/SFD (8 B), actual MAC and IFG; two
            // Manchester symbols per bit. Two UI cover admission/receive phase.
            let symbols = (mac as u128 + 24) * 16 + m.ifg_bits as u128 * 2 + 2;
            let ps = (symbols * 1_000_000_000_000).div_ceil(m.channel.grid.symbol_rate_hz as u128);
            let delay = ((path.delay_samples as u128
                + m.carrier_hold_samples as u128
                + m.channel.grid.samples_per_ui as u128 * 2)
                * 1_000_000_000_000)
                .div_ceil(
                    m.channel.grid.symbol_rate_hz as u128 * m.channel.grid.samples_per_ui as u128,
                );
            Ok((
                u64::try_from(ps).map_err(|_| UnprovenBound::PhysicalSignalEnvelope)?,
                u64::try_from(delay).map_err(|_| UnprovenBound::PhysicalSignalEnvelope)?,
            ))
        }
    }
}

type HopClasses = (
    std::collections::BTreeMap<usize, (u8, u16)>,
    BTreeSet<crate::types::ethernet::tsn::StreamKey>,
);

/// Apply the same declared VLAN/provider codecs as runtime to derive the class
/// and service VID on every physical transmission, including internal C/S ISS.
pub(super) fn hop_classes(
    n: &PreparedNetwork,
    paths: &[Vec<usize>],
    receivers: &std::collections::BTreeMap<usize, BTreeSet<usize>>,
    source: &crate::types::ethernet::EthernetWireFrame,
    source_vid: u16,
    source_priority: u8,
) -> std::result::Result<HopClasses, UnprovenBound> {
    use crate::runtime::{ethernet::retag_wire_frame, network::provider_l2::PortRole};
    use crate::types::ethernet::{EthernetTag, EthernetVlanTag};
    let mut result = std::collections::BTreeMap::new();
    let mut ingress_keys = BTreeSet::new();
    for path in paths {
        let mut wire = source.clone();
        let mut vid = source_vid;
        let mut priority = source_priority;
        let mut previous = None;
        for &direction in path {
            let link = &n.ethernet.directions[direction];
            let mut provider_class = None;
            if let Some(incoming) = previous {
                let ingress_port = super::extensions::logical_receiving_port(
                    n,
                    incoming,
                    link.source,
                    "/qcc/streams",
                )
                .map_err(|_| UnprovenBound::Classification)?;
                let ingress = n
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.ingress == ingress_port)
                    .ok_or(UnprovenBound::Classification)?;
                if let Some(topology) = n
                    .provider_l2
                    .as_ref()
                    .filter(|p| p.ports.contains_key(&ingress.port))
                {
                    let class = topology
                        .classify_ingress(&ingress.port, &wire)
                        .map_err(|_| UnprovenBound::Classification)?;
                    vid = class.vid;
                    priority = class.priority;
                    provider_class = Some(class);
                } else {
                    let tags = wire
                        .canonical_tags()
                        .map_err(|_| UnprovenBound::Classification)?;
                    let tag = tags.first();
                    vid = tag.map_or(ingress.pvid, |t| {
                        if t.tci.vid == 0 {
                            ingress.pvid
                        } else {
                            t.tci.vid
                        }
                    });
                    priority = tag.map_or(ingress.default_priority, |t| t.tci.pcp);
                }
                ingress_keys.insert(crate::types::ethernet::tsn::StreamKey {
                    ingress: ingress.ingress.clone(),
                    dst_mac: wire.dst_mac.clone(),
                    vid,
                    priority,
                });
            }
            if let Some(topology) = n
                .provider_l2
                .as_ref()
                .filter(|p| p.ports.contains_key(&link.from_port))
            {
                let target = &topology.ports[&link.from_port];
                let initial = provider_class.ok_or(UnprovenBound::Classification)?;
                let mut pending = vec![initial];
                let mut seen = BTreeSet::new();
                let mut candidates = Vec::new();
                while let Some(class) = pending.pop() {
                    if !seen.insert((
                        class.component.clone(),
                        class.ingress_port.clone(),
                        class.vid,
                    )) {
                        continue;
                    }
                    if class.component == target.component
                        && target.member_vids.contains(&class.vid)
                    {
                        candidates.push(class.clone());
                    }
                    for handoff in topology.ports.values().filter(|p| {
                        p.component == class.component
                            && matches!(p.role, PortRole::ProviderEdge | PortRole::CustomerNetwork)
                            && p.member_vids.contains(&class.vid)
                    }) {
                        if let Ok(next) = topology.handoff_all(&handoff.port, &class) {
                            pending.extend(next);
                        }
                    }
                }
                let class = candidates.first().ok_or(UnprovenBound::Classification)?;
                if candidates.iter().any(|other| {
                    other.vid != class.vid
                        || other.priority != class.priority
                        || other.wire.canonical_tags().ok() != class.wire.canonical_tags().ok()
                }) {
                    return Err(UnprovenBound::Classification);
                }
                vid = class.vid;
                priority = class.priority;
                wire = topology
                    .encode_egress(&link.from_port, class)
                    .map_err(|_| UnprovenBound::Classification)?;
            } else {
                let output = n
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.port == link.from_port)
                    .ok_or(UnprovenBound::Classification)?;
                let tagged = *output
                    .vlans
                    .get(&vid)
                    .ok_or(UnprovenBound::Classification)?;
                let tags = if tagged {
                    vec![EthernetTag {
                        tpid: EthernetTag::C_TPID,
                        tci: EthernetVlanTag {
                            vid,
                            pcp: priority,
                            dei: 0,
                        },
                    }]
                } else {
                    Vec::new()
                };
                wire = retag_wire_frame(&wire, &tags).map_err(|_| UnprovenBound::Classification)?;
            }
            if result
                .insert(direction, (priority, vid))
                .is_some_and(|old| old != (priority, vid))
            {
                return Err(UnprovenBound::Classification);
            }
            for receiver in &receivers[&direction] {
                let ingress_port = super::extensions::logical_receiving_port(
                    n,
                    direction,
                    *receiver,
                    "/qcc/streams",
                )
                .map_err(|_| UnprovenBound::Classification)?;
                let ingress = n
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|p| p.ingress == ingress_port)
                    .ok_or(UnprovenBound::Classification)?;
                let (received_vid, received_priority) = if let Some(topology) = n
                    .provider_l2
                    .as_ref()
                    .filter(|p| p.ports.contains_key(&ingress.port))
                {
                    let received = topology
                        .classify_ingress(&ingress.port, &wire)
                        .map_err(|_| UnprovenBound::Classification)?;
                    (received.vid, received.priority)
                } else {
                    let tags = wire
                        .canonical_tags()
                        .map_err(|_| UnprovenBound::Classification)?;
                    (
                        tags.first().map_or(ingress.pvid, |tag| {
                            if tag.tci.vid == 0 {
                                ingress.pvid
                            } else {
                                tag.tci.vid
                            }
                        }),
                        tags.first()
                            .map_or(ingress.default_priority, |tag| tag.tci.pcp),
                    )
                };
                ingress_keys.insert(crate::types::ethernet::tsn::StreamKey {
                    ingress: ingress.ingress.clone(),
                    dst_mac: wire.dst_mac.clone(),
                    vid: received_vid,
                    priority: received_priority,
                });
            }
            previous = Some(direction);
        }
    }
    Ok((result, ingress_keys))
}

/// Typed Merge-origin protocol arrivals for a stable ideal p2p epoch. Real
/// encoder wire sizes are used; arbitrary protocol adapters need their own proof.
fn merge_control_bound(
    n: &PreparedNetwork,
    direction: usize,
) -> std::result::Result<Option<MergeControlEnvelope>, UnprovenBound> {
    use crate::types::network::bindings::{MacOwner, PhysicalOwner};
    let bindings = n.bindings.as_ref().unwrap();
    if bindings.ports[direction].mac != MacOwner::Clause99 {
        return Ok(None);
    }
    let merge = n
        .extensions
        .as_ref()
        .and_then(|e| e.mac_merge.as_ref())
        .ok_or(UnprovenBound::ControlServiceEnvelope)?;
    let peer = merge.reverse[direction];
    if [direction, peer].iter().any(|d| {
        bindings.ports[*d]
            .protocol_adapters
            .iter()
            .any(|id| id != "mac_merge_lldp")
    }) {
        return Err(UnprovenBound::ControlServiceEnvelope);
    }
    let transport = |d: usize| match bindings.domains[bindings.ports[d].physical_domain].owner {
        PhysicalOwner::IdealPointToPoint => Ok((None, 0)),
        PhysicalOwner::Sampled { .. } => raw_transport_bound(n, d)
            .map(|(a, delay)| (Some(a), delay))
            .map_err(|_| UnprovenBound::ControlServiceEnvelope),
        _ => Err(UnprovenBound::ControlServiceEnvelope),
    };
    let (raw_attachment, local_jitter) = transport(direction)?;
    let (peer_attachment, peer_jitter) = transport(peer)?;
    if raw_attachment != peer_attachment {
        return Err(UnprovenBound::ControlServiceEnvelope);
    }
    let alignment = if raw_attachment.is_some() { 8_000 } else { 0 };
    let link = &n.ethernet.directions[direction];
    let lldp_mac = 64u64.max(
        44 + u64::try_from(link.from_port.len())
            .map_err(|_| UnprovenBound::ControlServiceEnvelope)?,
    );
    let wire_duration = |octets: u64| -> std::result::Result<u64, UnprovenBound> {
        u64::try_from(
            ((u128::from(octets) + 12) * 8 * 1_000_000_000_000)
                .div_ceil(u128::from(link.bitrate_bps)),
        )
        .map_err(|_| UnprovenBound::ControlServiceEnvelope)
    };
    let verify = crate::runtime::network::mac_merge::encode_control(
        crate::runtime::network::mac_merge::PacketKind::Verify,
    )
    .map_err(|_| UnprovenBound::ControlServiceEnvelope)?;
    let respond = crate::runtime::network::mac_merge::encode_control(
        crate::runtime::network::mac_merge::PacketKind::Respond,
    )
    .map_err(|_| UnprovenBound::ControlServiceEnvelope)?;
    let largest_generator = n
        .ethernet
        .generators
        .iter()
        .map(|g| (g.frame.data_hex.len() as u64 / 2).max(46) + 32)
        .max()
        .unwrap_or(64);
    let carry = u64::try_from(merge.max_rx_mac_octets)
        .ok()
        .and_then(|n| n.max(largest_generator).checked_add(32))
        .ok_or(UnprovenBound::ControlServiceEnvelope)?;
    Ok(Some(MergeControlEnvelope {
        peer_direction: peer,
        raw_attachment,
        admission_alignment_ps: alignment,
        local_lldp_interval_ps: merge.lldp_interval_ps,
        peer_lldp_interval_ps: merge.lldp_interval_ps,
        local_lldp_jitter_ps: local_jitter,
        peer_lldp_jitter_ps: peer_jitter,
        peer_verify_jitter_ps: peer_jitter,
        lldp_occupied_ps: wire_duration(lldp_mac + 8)?,
        verify_occupied_ps: wire_duration(verify.len() as u64)?,
        respond_occupied_ps: wire_duration(respond.len() as u64)?,
        carry_in_ps: wire_duration(carry)?,
    }))
}

/// Terminator to actual decode: one 450-slot codeword plus its 60-octet
/// renderer tail (4080ns), source-certified cadence displacement and FES ceil.
fn raw_transport_bound(
    n: &PreparedNetwork,
    direction: usize,
) -> std::result::Result<(usize, u64), UnprovenBound> {
    use crate::types::network::bindings::PhysicalOwner;
    let bindings = n.bindings.as_ref().unwrap();
    let PhysicalOwner::Sampled { attachment } =
        bindings.domains[bindings.ports[direction].physical_domain].owner
    else {
        return Err(UnprovenBound::PhysicalSignalEnvelope);
    };
    let a = &n.phy_attachments[attachment];
    let envelope = a
        .config
        .medium
        .raw_timing_envelope()
        .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
    let from = &n.ethernet.directions[direction].from_port;
    let endpoint = a
        .config
        .physical_ports
        .iter()
        .find(|p| &p.tx_port == from)
        .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
    let bound = envelope
        .directions
        .iter()
        .find(|d| d.source_endpoint == endpoint.endpoint)
        .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
    let samples = bound
        .pmax_samples_upper_bound
        .checked_add(bound.callback_rounding_samples)
        .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
    let phase_ps = (u128::from(samples) * 1_000_000_000_000).div_ceil(
        u128::from(envelope.grid.symbol_rate_hz) * u128::from(envelope.grid.samples_per_ui),
    );
    let delay = u64::try_from(phase_ps)
        .ok()
        .and_then(|p| p.checked_add(4_080_001))
        .ok_or(UnprovenBound::PhysicalSignalEnvelope)?;
    Ok((attachment, delay))
}

/// PAUSE RX and administrative inhibit have no finite service envelope yet.
/// Scope this to affected directions, including scheduled future enables.
fn pause_service_unproven(n: &PreparedNetwork, direction: usize) -> bool {
    use crate::types::network::mac_control::Action;
    n.mac_control.as_ref().is_some_and(|control| {
        control
            .ports
            .get(direction)
            .and_then(Option::as_ref)
            .is_some_and(|port| port.rx_enabled || port.tx_enabled)
            || control.controls.iter().any(|operation| {
                operation.direction == direction
                    && matches!(
                        operation.action,
                        Action::RxEnable(true)
                            | Action::TxEnable(true)
                            | Action::PauseRequest(_)
                            | Action::TransmitEnable(false)
                    )
            })
    })
}

#[cfg(test)]
mod raw_bound_tests {
    use super::*;
    fn prepared() -> PreparedNetwork {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/qcc-raw/model.ini"),
        )
        .unwrap()
        .registered
        .as_ref()
        .unwrap()
        .network
        .as_ref()
        .unwrap()
        .as_ref()
        .clone()
    }
    #[test]
    fn pause_current_future_and_disjoint_requests_remain_scoped() {
        use crate::types::network::mac_control::{Action, Control, PausePort, PreparedMacControl};
        let mut n = prepared();
        let port = PausePort {
            station_mac: [2, 0, 0, 0, 0, 2],
            rx_enabled: true,
            tx_enabled: false,
            experimental_merge: true,
            experimental_project_generation: false,
        };
        n.mac_control = Some(PreparedMacControl {
            ports: vec![None, Some(port)],
            controls: vec![],
        });
        assert!(
            !pause_service_unproven(&n, 0),
            "disjoint PAUSE must not invalidate this request"
        );
        assert_eq!(
            prepare(&n.config["qcc"], &n).unwrap().streams[0].unproven,
            None
        );
        for action in [
            Action::RxEnable(true),
            Action::TxEnable(true),
            Action::PauseRequest(1),
            Action::TransmitEnable(false),
        ] {
            n.mac_control.as_mut().unwrap().controls = vec![Control {
                at_ps: 9_000_000_000,
                direction: 0,
                action,
            }];
            assert!(pause_service_unproven(&n, 0));
            assert_eq!(
                prepare(&n.config["qcc"], &n).unwrap().streams[0].unproven,
                Some(UnprovenBound::ControlServiceEnvelope)
            );
        }
    }
    #[test]
    fn raw_bound_is_static_directional_and_rejects_unknown_control_producers() {
        let mut n = prepared();
        assert_eq!(n.qcc.as_ref().unwrap().streams[0].unproven, None);
        for delay in [0, 4, 15, 16, 17] {
            n.phy_attachments[0].config.medium.channel.paths[1][0]
                .as_mut()
                .unwrap()
                .delay_samples = delay;
            // At12GS/s, upper phase is delay+15 samples. Always round up,
            // then reserve one more ps for the enclosing FES ceiling.
            let expected = 4_080_001 + ((delay as u64 + 15) * 1000).div_ceil(12);
            assert_eq!(raw_transport_bound(&n, 0), Ok((0, expected)));
        }
        let control = merge_control_bound(&n, 0).unwrap().unwrap();
        assert_eq!(control.admission_alignment_ps, 8000);
        assert_eq!(control.raw_attachment, Some(0));
        assert_eq!(control.verify_occupied_ps, 672_000);
        assert_eq!(control.respond_occupied_ps, 672_000);
        for direction in [0, 1] {
            let mut extra = n.clone();
            extra.bindings.as_mut().unwrap().ports[direction]
                .protocol_adapters
                .push("gptp_unbounded".into());
            assert_eq!(
                merge_control_bound(&extra, 0),
                Err(UnprovenBound::ControlServiceEnvelope)
            );
        }
        n.phy_attachments[0].config.medium.endpoints[0]
            .receiver
            .max_tracking_ppm = 1;
        assert_eq!(
            raw_transport_bound(&n, 0),
            Err(UnprovenBound::PhysicalSignalEnvelope)
        );
    }
}
