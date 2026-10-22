//! Bounded cyclic CUC/CNC service. Its owner stages it on the common network FES.
//! No packets, links, byte storage or private simulation scheduler live here.
use crate::types::network::configuration::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[path = "configuration/envelope.rs"]
mod envelope;
#[path = "configuration/projection.rs"]
mod projection;
#[cfg(test)]
use envelope::merge_interference;
pub use projection::{compile_certificate, compiled_schedule, rounding_quantum};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    UnknownStream,
    NoProvenBound(UnprovenBound),
    InactiveStream,
    Busy,
    NoPreviousVersion,
    StaleTopology,
    ParticipantRejected,
    Timeout,
    NotAcknowledged,
    TransitionNotDrained,
    Cancelled,
    UnknownTransaction,
    InfeasibleFrameCapacity,
    NoScheduleInSupportedModel,
    SearchLimit,
    UnsupportedDependencyCycle,
    UnsupportedSharedClass,
    BufferCapacity,
    OffsetRange,
    LatencyBound,
    UnavailablePath,
    ExcessBurst,
    Oversize,
    LateSource,
    ArithmeticOverflow,
    InvalidDeadline,
    InvalidState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QccEffect {
    Prepared {
        transaction: String,
        generation: u64,
        activate_at_ps: u64,
        participants: Vec<usize>,
    },
    ParticipantReply {
        transaction: String,
        generation: u64,
        direction: usize,
        accepted: bool,
    },
    Rejected {
        transaction: String,
        reason: Rejection,
    },
    Installed {
        transaction: String,
        version: u64,
        previous_version: u64,
        plan: ConfigurationPlan,
        rollback: bool,
    },
    Suspended {
        version: u64,
        topology_epoch: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Vote {
    direction: usize,
    mode: ParticipantMode,
    replied: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    operation: usize,
    id: String,
    generation: u64,
    base_version: u64,
    topology_epoch: u64,
    ack_at: u64,
    timeout_at: u64,
    activate_at: u64,
    votes: Vec<Vote>,
    plan: ConfigurationPlan,
    rollback: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceCounter {
    interval: u64,
    count: u16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QccState {
    pub version: u64,
    pub topology_epoch: u64,
    pub active: ConfigurationPlan,
    pub active_valid: bool,
    previous: Option<ConfigurationPlan>,
    pending: Option<Pending>,
    cursor: usize,
    generation: u64,
    available: Vec<bool>,
    participant_modes: BTreeMap<usize, ParticipantMode>,
    counters: BTreeMap<usize, SourceCounter>,
    dependency_clocks: BTreeMap<usize, super::clock::NodeClock>,
    dependency_raw: BTreeMap<usize, RawSourceCertificate>,
    last_ps: Option<u64>,
    audit: Vec<AuditStep>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum AuditStep {
    RawSources {
        at_ps: u64,
        #[serde(with = "super::checkpoint::map")]
        certificates: BTreeMap<usize, RawSourceCertificate>,
    },
    Advance {
        at_ps: u64,
        quiescent: bool,
    },
    Topology {
        at_ps: u64,
        available: Vec<bool>,
    },
    Protocol {
        at_ps: u64,
        direction: usize,
    },
    Clock {
        at_ps: u64,
        node: usize,
        clock: super::clock::NodeClock,
    },
    Source {
        generator: String,
        ready_ps: u64,
        payload_bytes: usize,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAdmission {
    pub stream: usize,
    pub version: u64,
    pub interval_origin_ps: u64,
    pub path: Vec<usize>,
}
fn add(a: u64, b: u64) -> Result<u64, Rejection> {
    a.checked_add(b).ok_or(Rejection::ArithmeticOverflow)
}
fn multiply(a: u64, b: u64) -> Result<u64, Rejection> {
    a.checked_mul(b).ok_or(Rejection::ArithmeticOverflow)
}
pub fn occupied_ps(mac: u64, bps: u64) -> Result<u64, Rejection> {
    if bps == 0 {
        return Err(Rejection::UnavailablePath);
    }
    let n = (mac as u128 + 20) * 8 * 1_000_000_000_000;
    u64::try_from(n.div_ceil(bps as u128)).map_err(|_| Rejection::ArithmeticOverflow)
}
/// Conservative first-fit certificate. Failure is scoped to this supported model.
pub fn solve(
    p: &PreparedQcc,
    streams: &BTreeSet<usize>,
    epoch: u64,
    available: &[bool],
) -> Result<ConfigurationPlan, Rejection> {
    if available.len() != p.ports.len() {
        return Err(Rejection::UnavailablePath);
    }
    // One managed stream on each (direction,class) gives an exact Talker first-SOF
    // offset without assuming an order between different Talker burst arrivals.
    let mut vertices: BTreeMap<(usize, u8), (usize, u64)> = BTreeMap::new();
    let mut source_interference = BTreeMap::new();
    let mut predecessors: BTreeMap<(usize, u8), BTreeSet<(usize, u8)>> = BTreeMap::new();
    let mut successors: BTreeMap<(usize, u8), BTreeSet<(usize, u8)>> = BTreeMap::new();
    let mut frames = BTreeMap::<usize, [u64; 8]>::new();
    let mut bytes = BTreeMap::<usize, [u64; 8]>::new();
    let cycle = p.config.cycle_ps.0;
    let mut budget = 0u32;
    for &i in streams {
        let s = p.streams.get(i).ok_or(Rejection::UnknownStream)?;
        if let Some(reason) = &s.unproven {
            return Err(Rejection::NoProvenBound(reason.clone()));
        }
        let mut seen_demands = BTreeSet::new();
        for path in &s.paths {
            let mut previous = None;
            for &direction in path {
                budget = budget.checked_add(1).ok_or(Rejection::SearchLimit)?;
                if budget > p.config.search_budget {
                    return Err(Rejection::SearchLimit);
                }
                let port = p.ports.get(direction).ok_or(Rejection::UnavailablePath)?;
                if !available[direction] {
                    return Err(Rejection::UnavailablePath);
                }
                let priority = s
                    .demands
                    .iter()
                    .find(|d| d.direction == direction)
                    .map_or(s.priority, |d| d.priority);
                let key = (direction, priority);
                if vertices.get(&key).is_some_and(|(owner, _)| *owner != i) {
                    return Err(Rejection::UnsupportedSharedClass);
                }
                let demand = s.demands.iter().find(|d| d.direction == direction);
                let mac_bytes = demand.map_or(s.mac_bytes, |d| d.mac_bytes);
                let occupied = demand.map_or_else(
                    || occupied_ps(mac_bytes, port.bitrate_bps),
                    |d| Ok(d.occupied_ps),
                )?;
                if occupied > cycle {
                    return Err(Rejection::InfeasibleFrameCapacity);
                }
                let count = s.requirement.max_frames_per_interval as u64;
                let margin = multiply(
                    add(s.requirement.jitter_ps.0, p.config.clock_error_budget_ps.0)?,
                    2,
                )?;
                let quantization = multiply(rounding_quantum(port), 2)?;
                let (service, interference) = envelope::burst_service(
                    demand,
                    occupied,
                    count,
                    cycle,
                    &mut budget,
                    p.config.search_budget,
                )?;
                source_interference.insert(key, interference);
                let width = add(add(service, margin)?, quantization)?;
                vertices.insert(key, (i, width));
                predecessors.entry(key).or_default();
                successors.entry(key).or_default();
                if seen_demands.insert(direction) {
                    let counts = frames.entry(direction).or_default();
                    counts[priority as usize] = add(counts[priority as usize], count)?;
                    let sizes = bytes.entry(direction).or_default();
                    sizes[priority as usize] =
                        add(sizes[priority as usize], multiply(mac_bytes, count)?)?;
                }
                if let Some(prev) = previous {
                    predecessors.entry(key).or_default().insert(prev);
                    successors.entry(prev).or_default().insert(key);
                }
                previous = Some(key);
            }
        }
    }
    let mut reservations = Vec::new();
    for (&direction, counts) in &frames {
        let port = &p.ports[direction];
        let sizes = bytes[&direction];
        if counts.iter().sum::<u64>() > port.capacity_frames
            || (0..8).any(|c| {
                counts[c] > port.class_capacity_frames[c]
                    || port.class_capacity_bytes[c].is_some_and(|cap| sizes[c] > cap)
            })
        {
            return Err(Rejection::BufferCapacity);
        }
        reservations.push(BufferReservation {
            direction,
            frames: *counts,
            bytes: sizes,
        });
    }
    let mut ready: BTreeSet<_> = predecessors
        .iter()
        .filter(|(_, v)| v.is_empty())
        .map(|(key, _)| *key)
        .collect();
    let mut slots = BTreeMap::<usize, Vec<GateSlot>>::new();
    let mut domain_slots = BTreeMap::<usize, Vec<GateSlot>>::new();
    let mut finished = BTreeMap::<(usize, u8), u64>::new();
    let mut offsets = BTreeMap::new();
    let mut latest_offsets = BTreeMap::new();
    while !ready.is_empty() {
        let key = *ready
            .iter()
            .min_by_key(|key| {
                let s = &p.streams[vertices[key].0];
                (
                    std::cmp::Reverse(s.requirement.rank),
                    &s.requirement.id,
                    **key,
                )
            })
            .unwrap();
        ready.remove(&key);
        budget = budget.checked_add(1).ok_or(Rejection::SearchLimit)?;
        if budget > p.config.search_budget {
            return Err(Rejection::SearchLimit);
        }
        let (stream, width) = vertices[&key];
        let s = &p.streams[stream];
        let port = &p.ports[key.0];
        let mut start = 0;
        for &prev in &predecessors[&key] {
            let previous = &p.ports[prev.0];
            start = start.max(add(
                finished[&prev],
                add(
                    s.demands
                        .iter()
                        .find(|d| d.direction == prev.0)
                        .map_or(previous.propagation_ps, |d| d.propagation_ps),
                    s.demands
                        .iter()
                        .find(|d| d.direction == prev.0)
                        .map_or(previous.destination_processing_ps, |d| {
                            d.destination_processing_ps
                        }),
                )?,
            )?);
        }
        let source = s.paths.iter().any(|path| path.first() == Some(&key.0));
        if source {
            let margin = multiply(
                add(s.requirement.jitter_ps.0, p.config.clock_error_budget_ps.0)?,
                2,
            )?;
            start = start
                .max(add(
                    s.requirement.earliest_transmit_offset_ps.0,
                    rounding_quantum(port),
                )?)
                .max(add(port.source_processing_ps, margin)?);
        }
        let calendar = domain_slots.entry(port.calendar_domain).or_default();
        loop {
            let end = add(start, width)?;
            let conflict = calendar
                .iter()
                .find(|slot| start < slot.end_ps && slot.start_ps < end);
            if let Some(slot) = conflict {
                start = slot.end_ps;
                budget = budget.checked_add(1).ok_or(Rejection::SearchLimit)?;
                if budget > p.config.search_budget {
                    return Err(Rejection::SearchLimit);
                }
            } else {
                break;
            }
        }
        let end = add(start, width)?;
        if end > cycle {
            return Err(Rejection::NoScheduleInSupportedModel);
        }
        if source {
            let quantization = rounding_quantum(port);
            let start_jitter = s
                .demands
                .iter()
                .find(|d| d.direction == key.0)
                .map_or(0, |d| d.start_jitter_ps);
            let latest = add(
                add(add(start, quantization)?, source_interference[&key])?,
                start_jitter,
            )?;
            if latest > s.requirement.latest_transmit_offset_ps.0 {
                return Err(Rejection::OffsetRange);
            }
            offsets.insert(s.requirement.id.clone(), start);
            latest_offsets.insert(s.requirement.id.clone(), latest);
        }
        let port_slots = slots.entry(key.0).or_default();
        port_slots.push(GateSlot {
            start_ps: start,
            end_ps: end,
            priority: key.1,
        });
        calendar.push(port_slots.last().unwrap().clone());
        calendar.sort_by_key(|s| s.start_ps);
        port_slots.sort_by_key(|s| s.start_ps);
        finished.insert(key, end);
        for &next in &successors[&key] {
            if predecessors[&next]
                .iter()
                .all(|prev| finished.contains_key(prev))
            {
                ready.insert(next);
            }
        }
    }
    if finished.len() != vertices.len() {
        return Err(Rejection::UnsupportedDependencyCycle);
    }
    let mut bounds = BTreeMap::new();
    for &i in streams {
        let s = &p.streams[i];
        let mut bound = 0;
        for path in &s.paths {
            let direction = *path.last().ok_or(Rejection::UnavailablePath)?;
            let port = &p.ports[direction];
            bound = bound.max(add(
                finished[&(
                    direction,
                    s.demands
                        .iter()
                        .find(|d| d.direction == direction)
                        .map_or(s.priority, |d| d.priority),
                )],
                add(
                    s.demands
                        .iter()
                        .find(|d| d.direction == direction)
                        .map_or(port.propagation_ps, |d| d.propagation_ps),
                    s.demands
                        .iter()
                        .find(|d| d.direction == direction)
                        .map_or(port.destination_processing_ps, |d| {
                            d.destination_processing_ps
                        }),
                )?,
            )?);
        }
        if bound > cycle
            || s.requirement.max_latency_ps.0 != 0 && bound > s.requirement.max_latency_ps.0
        {
            return Err(Rejection::LatencyBound);
        }
        bounds.insert(s.requirement.id.clone(), bound);
    }
    // Every managed port is included even after removal, retaining closed reserved
    // classes instead of accidentally restoring always-open unconfigured traffic.
    let mut directions: BTreeSet<_> = p
        .streams
        .iter()
        .flat_map(|s| s.paths.iter().flatten().copied())
        .filter(|d| p.ports[*d].writable_schedule)
        .collect();
    let domains: BTreeSet<_> = directions
        .iter()
        .map(|d| p.ports[*d].calendar_domain)
        .collect();
    directions.extend(
        p.ports
            .iter()
            .filter(|p| p.writable_schedule && domains.contains(&p.calendar_domain))
            .map(|p| p.direction),
    );
    Ok(ConfigurationPlan {
        streams: streams.iter().copied().collect(),
        ports: directions
            .into_iter()
            .map(|direction| PortPlan {
                direction,
                closed_slots: domain_slots
                    .get(&p.ports[direction].calendar_domain)
                    .into_iter()
                    .flatten()
                    .filter(|s| !slots.get(&direction).is_some_and(|own| own.contains(s)))
                    .cloned()
                    .collect(),
                slots: slots.remove(&direction).unwrap_or_default(),
            })
            .collect(),
        latency_bounds_ps: bounds,
        talker_offsets_ps: offsets,
        talker_latest_offsets_ps: latest_offsets,
        topology_epoch: epoch,
        reservations,
        schedules: Vec::new(),
        raw_certificates: BTreeMap::new(),
    })
}
impl QccState {
    pub fn new(p: &PreparedQcc) -> Result<Self, Rejection> {
        let available = vec![true; p.ports.len()];
        let active = solve(p, &BTreeSet::new(), 0, &available)?;
        Ok(Self {
            version: 0,
            topology_epoch: 0,
            active,
            active_valid: true,
            previous: None,
            pending: None,
            cursor: 0,
            generation: 0,
            available,
            participant_modes: BTreeMap::new(),
            counters: BTreeMap::new(),
            dependency_clocks: BTreeMap::new(),
            dependency_raw: BTreeMap::new(),
            last_ps: None,
            audit: Vec::new(),
        })
    }
    pub fn next_deadline(&self, p: &PreparedQcc) -> Option<u64> {
        let operation = p
            .config
            .operations
            .get(self.cursor)
            .map(ManagementOperation::at);
        let ack = self
            .pending
            .as_ref()
            .filter(|t| {
                t.votes
                    .iter()
                    .any(|v| !v.replied && v.mode != ParticipantMode::Silent)
            })
            .map(|t| t.ack_at);
        [
            operation,
            ack,
            self.pending
                .as_ref()
                .filter(|t| t.votes.iter().any(|v| !v.replied))
                .map(|t| t.timeout_at),
            self.pending.as_ref().map(|t| t.activate_at),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    pub fn available(&self) -> &[bool] {
        &self.available
    }
    #[cfg(test)]
    pub fn active_stream(&self, p: &PreparedQcc, generator: &str) -> Option<usize> {
        p.generator_index
            .get(generator)
            .copied()
            .filter(|i| self.active_valid && self.active.streams.contains(i))
    }
    /// FIFO admission reserves the unused managed-frame capacity against background.
    /// The owner ALSO applies its ordinary total/per-class byte and frame bounds.
    pub fn background_buffer_allowed(
        &self,
        p: &PreparedQcc,
        direction: usize,
        priority: u8,
        total: u64,
        counts: [u64; 8],
    ) -> bool {
        if p.managed_priority_mask(direction) & (1 << priority) != 0 {
            // This method is for unbound background frames. A source can be
            // reclassified on ingress, but that cannot grant a CUC admission.
            return false;
        }
        let Some(r) = self
            .active
            .reservations
            .iter()
            .find(|r| r.direction == direction)
        else {
            return true;
        };
        let remaining = r
            .frames
            .iter()
            .zip(counts)
            .map(|(reserved, used)| reserved.saturating_sub(used))
            .sum::<u64>();
        total
            .checked_add(1)
            .and_then(|n| n.checked_add(remaining))
            .is_some_and(|n| n <= p.ports[direction].capacity_frames)
    }
    pub fn admit_source(
        &mut self,
        p: &PreparedQcc,
        generator: &str,
        ready: u64,
        payload_bytes: usize,
    ) -> Result<SourceAdmission, Rejection> {
        let admission = self.admit_source_inner(p, generator, ready, payload_bytes)?;
        self.audit.push(AuditStep::Source {
            generator: generator.to_string(),
            ready_ps: ready,
            payload_bytes,
        });
        Ok(admission)
    }
    fn admit_source_inner(
        &mut self,
        p: &PreparedQcc,
        generator: &str,
        ready: u64,
        payload_bytes: usize,
    ) -> Result<SourceAdmission, Rejection> {
        let i = *p
            .generator_index
            .get(generator)
            .ok_or(Rejection::UnknownStream)?;
        if !self.active_valid || !self.active.streams.contains(&i) {
            return Err(Rejection::InactiveStream);
        }
        if !self.raw_plan_valid(p, &self.active, ready) {
            return Err(Rejection::NoProvenBound(UnprovenBound::PhysicalStartup));
        }
        let s = &p.streams[i];
        if payload_bytes > s.requirement.max_frame_size as usize {
            return Err(Rejection::Oversize);
        }
        if self.last_ps.is_some_and(|last| ready < last) {
            return Err(Rejection::InvalidDeadline);
        }
        let cycle = p.config.cycle_ps.0;
        let interval = ready / cycle;
        let origin = interval * cycle;
        let offset = self.active.talker_offsets_ps[&s.requirement.id];
        if ready - origin > offset {
            return Err(Rejection::LateSource);
        }
        let counter = self
            .counters
            .entry(i)
            .or_insert(SourceCounter { interval, count: 0 });
        if interval < counter.interval {
            return Err(Rejection::InvalidDeadline);
        }
        if counter.interval != interval {
            counter.interval = interval;
            counter.count = 0;
        }
        if counter.count >= s.requirement.max_frames_per_interval {
            return Err(Rejection::ExcessBurst);
        }
        counter.count += 1;
        Ok(SourceAdmission {
            stream: i,
            version: self.version,
            interval_origin_ps: origin,
            path: s.paths[0].clone(),
        })
    }
    #[cfg(test)]
    pub fn route(
        &self,
        p: &PreparedQcc,
        version: u64,
        generator: &str,
        incoming: Option<usize>,
    ) -> Result<Option<usize>, Rejection> {
        if !self.active_valid || version != self.version {
            return Err(Rejection::StaleTopology);
        }
        let i = self
            .active_stream(p, generator)
            .ok_or(Rejection::InactiveStream)?;
        let path = &p.streams[i].paths[0];
        match incoming {
            None => Ok(path.first().copied()),
            Some(direction) => {
                let i = path
                    .iter()
                    .position(|d| *d == direction)
                    .ok_or(Rejection::UnavailablePath)?;
                Ok(path.get(i + 1).copied())
            }
        }
    }

    /// Process an exact FES deadline. The owner computes quiescence from actual
    /// queued, serializing and propagating frames on all affected directions.
    pub fn advance(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        quiescent: bool,
    ) -> Result<Vec<QccEffect>, Rejection> {
        let mut candidate = self.clone();
        let effects = candidate.advance_inner(p, now, quiescent)?;
        candidate.audit.push(AuditStep::Advance {
            at_ps: now,
            quiescent,
        });
        *self = candidate;
        Ok(effects)
    }
    fn advance_inner(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        quiescent: bool,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if self.last_ps.is_some_and(|last| now < last)
            || self.next_deadline(p).is_some_and(|due| due < now)
        {
            return Err(Rejection::InvalidDeadline);
        }
        self.last_ps = Some(now);
        let mut effects = Vec::new();
        if let Some(t) = self.pending.as_ref() {
            if t.timeout_at <= now && t.votes.iter().any(|v| !v.replied) {
                let t = self.pending.take().unwrap();
                effects.push(QccEffect::Rejected {
                    transaction: t.id,
                    reason: Rejection::Timeout,
                });
            }
        }
        if let Some(t) = self.pending.as_mut() {
            if t.ack_at <= now && now < t.timeout_at {
                for vote in &mut t.votes {
                    if !vote.replied && vote.mode != ParticipantMode::Silent {
                        vote.replied = true;
                        effects.push(QccEffect::ParticipantReply {
                            transaction: t.id.clone(),
                            generation: t.generation,
                            direction: vote.direction,
                            accepted: vote.mode == ParticipantMode::Accept,
                        });
                    }
                }
                if t.votes
                    .iter()
                    .any(|v| v.replied && v.mode == ParticipantMode::Reject)
                {
                    let t = self.pending.take().unwrap();
                    effects.push(QccEffect::Rejected {
                        transaction: t.id,
                        reason: Rejection::ParticipantRejected,
                    });
                }
            }
        }
        if self.pending.as_ref().is_some_and(|t| t.activate_at == now) {
            let t = self.pending.take().unwrap();
            let reason =
                if t.topology_epoch != self.topology_epoch || t.base_version != self.version {
                    Some(Rejection::StaleTopology)
                } else if t
                    .votes
                    .iter()
                    .any(|v| !v.replied || v.mode != ParticipantMode::Accept)
                {
                    Some(Rejection::NotAcknowledged)
                } else if !self.raw_plan_valid(p, &t.plan, now) {
                    Some(Rejection::NoProvenBound(UnprovenBound::PhysicalStartup))
                } else if !quiescent {
                    Some(Rejection::TransitionNotDrained)
                } else {
                    None
                };
            if let Some(reason) = reason {
                effects.push(QccEffect::Rejected {
                    transaction: t.id,
                    reason,
                });
            } else {
                let previous_version = self.version;
                self.version = add(self.version, 1)?;
                self.previous = Some(std::mem::replace(&mut self.active, t.plan));
                self.active_valid = true;
                self.counters.clear();
                effects.push(QccEffect::Installed {
                    transaction: t.id,
                    version: self.version,
                    previous_version,
                    plan: self.active.clone(),
                    rollback: t.rollback,
                });
            }
        }
        while let Some(operation) = p
            .config
            .operations
            .get(self.cursor)
            .filter(|o| o.at() == now)
        {
            let index = self.cursor;
            self.cursor += 1;
            match operation {
                ManagementOperation::Participant { port, mode, .. } => {
                    let direction = p
                        .ports
                        .iter()
                        .position(|d| &d.port == port)
                        .ok_or(Rejection::UnavailablePath)?;
                    self.participant_modes.insert(direction, *mode);
                }
                ManagementOperation::Cancel {
                    id, transaction, ..
                } => {
                    if self.pending.as_ref().is_some_and(|t| &t.id == transaction) {
                        let pending = self.pending.take().unwrap();
                        effects.push(QccEffect::Rejected {
                            transaction: pending.id,
                            reason: Rejection::Cancelled,
                        });
                    } else {
                        effects.push(QccEffect::Rejected {
                            transaction: id.clone(),
                            reason: Rejection::UnknownTransaction,
                        });
                    }
                }
                ManagementOperation::Prepare {
                    id,
                    expected_topology_epoch,
                    add,
                    remove,
                    activate_at_ps,
                    timeout_at_ps,
                    ..
                } => {
                    let mut streams: BTreeSet<_> = self.active.streams.iter().copied().collect();
                    let mut invalid = false;
                    for name in remove {
                        match p.stream_index.get(name) {
                            Some(i) => {
                                if !streams.remove(i) {
                                    invalid = true;
                                }
                            }
                            None => invalid = true,
                        }
                    }
                    for name in add {
                        match p.stream_index.get(name) {
                            Some(i) => {
                                if !streams.insert(*i) {
                                    invalid = true;
                                }
                            }
                            None => invalid = true,
                        }
                    }
                    let result = if invalid {
                        Err(Rejection::UnknownStream)
                    } else {
                        self.begin(
                            p,
                            index,
                            id,
                            expected_topology_epoch.0,
                            streams,
                            now,
                            activate_at_ps.0,
                            timeout_at_ps.0,
                            false,
                        )
                    };
                    match result {
                        Ok(effect) => effects.push(effect),
                        Err(reason) => effects.push(QccEffect::Rejected {
                            transaction: id.clone(),
                            reason,
                        }),
                    }
                }
                ManagementOperation::Rollback {
                    id,
                    expected_topology_epoch,
                    activate_at_ps,
                    timeout_at_ps,
                    ..
                } => {
                    let result = if let Some(previous) = &self.previous {
                        self.begin(
                            p,
                            index,
                            id,
                            expected_topology_epoch.0,
                            previous.streams.iter().copied().collect(),
                            now,
                            activate_at_ps.0,
                            timeout_at_ps.0,
                            true,
                        )
                    } else {
                        Err(Rejection::NoPreviousVersion)
                    };
                    match result {
                        Ok(effect) => effects.push(effect),
                        Err(reason) => effects.push(QccEffect::Rejected {
                            transaction: id.clone(),
                            reason,
                        }),
                    }
                }
            }
        }
        Ok(effects)
    }
    #[allow(clippy::too_many_arguments)]
    fn begin(
        &mut self,
        p: &PreparedQcc,
        operation: usize,
        id: &str,
        expected_epoch: u64,
        streams: BTreeSet<usize>,
        now: u64,
        activate_at: u64,
        timeout_at: u64,
        rollback: bool,
    ) -> Result<QccEffect, Rejection> {
        if self.pending.is_some() {
            return Err(Rejection::Busy);
        }
        if expected_epoch != self.topology_epoch {
            return Err(Rejection::StaleTopology);
        }
        let mut current = p.clone();
        let mut clocks = BTreeMap::new();
        for port in &mut current.ports {
            let clock = self
                .dependency_clocks
                .get(&port.source)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| {
                    super::clock::NodeClock::new(
                        0,
                        port.clock_offset_ps,
                        port.clock_rate_num,
                        port.clock_rate_den,
                    )
                })
                .map_err(|_| Rejection::NoProvenBound(UnprovenBound::ClockProjection))?;
            (port.clock_rate_num, port.clock_rate_den) = clock.rate();
            port.clock_fractional = clock
                .local_at(activate_at)
                .map_err(|_| Rejection::ArithmeticOverflow)?
                .denominator()
                != 1;
            clocks.insert(port.direction, clock);
        }
        let mut plan = solve(&current, &streams, self.topology_epoch, &self.available)?;
        for attachment in Self::raw_attachments(p, &plan.streams) {
            let certificate = self
                .dependency_raw
                .get(&attachment)
                .filter(|certificate| certificate.valid_at(now))
                .ok_or(Rejection::NoProvenBound(UnprovenBound::PhysicalStartup))?;
            plan.raw_certificates
                .insert(attachment, certificate.clone());
        }
        for port in &plan.ports {
            plan.schedules.push(compile_certificate(
                &current,
                port,
                activate_at,
                clocks.get(&port.direction),
            )?);
        }
        let ack_at = add(now, p.config.management_latency_ps.0)?;
        self.generation = add(self.generation, 1)?;
        // Full replacement installs even empty reserved-class schedules on every
        // managed port; removed participants therefore still vote on the change.
        let participants: Vec<_> = plan.ports.iter().map(|port| port.direction).collect();
        let votes = participants
            .iter()
            .map(|&direction| Vote {
                direction,
                mode: self
                    .participant_modes
                    .get(&direction)
                    .copied()
                    .unwrap_or(ParticipantMode::Accept),
                replied: false,
            })
            .collect();
        self.pending = Some(Pending {
            operation,
            id: id.to_string(),
            generation: self.generation,
            base_version: self.version,
            topology_epoch: self.topology_epoch,
            ack_at,
            timeout_at,
            activate_at,
            votes,
            plan,
            rollback,
        });
        Ok(QccEffect::Prepared {
            transaction: id.to_string(),
            generation: self.generation,
            activate_at_ps: activate_at,
            participants,
        })
    }
    /// The network owner supplies its actual current link view. This is a CNC
    /// topology epoch, never a second independently simulated physical topology.
    pub fn topology_changed(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        available: Vec<bool>,
    ) -> Result<Vec<QccEffect>, Rejection> {
        let mut candidate = self.clone();
        let effects = candidate.topology_inner(p, now, available.clone())?;
        candidate.audit.push(AuditStep::Topology {
            at_ps: now,
            available,
        });
        *self = candidate;
        Ok(effects)
    }
    fn topology_inner(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        available: Vec<bool>,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if available.len() != p.ports.len() || self.last_ps.is_some_and(|last| now < last) {
            return Err(Rejection::InvalidState);
        }
        self.topology_epoch = add(self.topology_epoch, 1)?;
        self.available = available;
        self.active_valid = false;
        self.last_ps = Some(now);
        let mut effects = Vec::new();
        if let Some(t) = self.pending.take() {
            effects.push(QccEffect::Rejected {
                transaction: t.id,
                reason: Rejection::StaleTopology,
            });
        }
        effects.push(QccEffect::Suspended {
            version: self.version,
            topology_epoch: self.topology_epoch,
        });
        Ok(effects)
    }
    /// One transaction owns the clock dependency and the gate suspension.
    /// The caller stages the corrected clock and this state in the same delta.
    pub fn clock_changed(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        node: usize,
        clock: super::clock::NodeClock,
    ) -> Result<Vec<QccEffect>, Rejection> {
        let mut candidate = self.clone();
        let effects = candidate.clock_inner(p, now, node, clock.clone())?;
        candidate.audit.push(AuditStep::Clock {
            at_ps: now,
            node,
            clock,
        });
        *self = candidate;
        Ok(effects)
    }
    fn clock_inner(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        node: usize,
        clock: super::clock::NodeClock,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if !p
            .ports
            .iter()
            .any(|port| port.source == node && p.depends_on_direction(port.direction))
        {
            return Ok(Vec::new());
        }
        if self
            .dependency_clocks
            .get(&node)
            .is_some_and(|old| old.generation() >= clock.generation())
        {
            return Err(Rejection::InvalidState);
        }
        self.dependency_clocks.insert(node, clock);
        self.topology_inner(p, now, self.available.clone())
    }
    #[cfg(test)]
    pub fn clock_history(&self) -> Vec<(u64, usize, super::clock::NodeClock)> {
        self.audit
            .iter()
            .filter_map(|step| match step {
                AuditStep::Clock { at_ps, node, clock } => Some((*at_ps, *node, clock.clone())),
                _ => None,
            })
            .collect()
    }
    pub fn protocol_changed(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        direction: usize,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if !p.depends_on_direction(direction) {
            return Ok(Vec::new());
        }
        let mut candidate = self.clone();
        let effects = candidate.protocol_inner(p, now, direction)?;
        candidate.audit.push(AuditStep::Protocol {
            at_ps: now,
            direction,
        });
        *self = candidate;
        Ok(effects)
    }
    fn protocol_inner(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        direction: usize,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if !p.depends_on_direction(direction) {
            return Err(Rejection::InvalidState);
        }
        self.dependency_raw
            .retain(|_, certificate| !certificate.merge.iter().any(|d| d.direction == direction));
        self.topology_inner(p, now, self.available.clone())
    }
    fn raw_attachments(p: &PreparedQcc, streams: &[usize]) -> BTreeSet<usize> {
        streams
            .iter()
            .flat_map(|i| &p.streams[*i].demands)
            .filter_map(|d| d.merge_control.as_ref()?.raw_attachment)
            .collect()
    }
    fn raw_plan_valid(&self, p: &PreparedQcc, plan: &ConfigurationPlan, now: u64) -> bool {
        if Self::raw_attachments(p, &plan.streams)
            != plan.raw_certificates.keys().copied().collect()
        {
            return false;
        }
        plan.raw_certificates
            .iter()
            .all(|(attachment, certificate)| {
                self.dependency_raw
                    .get(attachment)
                    .is_some_and(|current| current.valid_at(now) && certificate.same_epoch(current))
            })
    }
    pub fn raw_sources(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        certificates: BTreeMap<usize, RawSourceCertificate>,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if certificates == self.dependency_raw {
            return Ok(Vec::new());
        }
        let mut candidate = self.clone();
        let effects = candidate.raw_sources_inner(p, now, certificates.clone())?;
        candidate.audit.push(AuditStep::RawSources {
            at_ps: now,
            certificates,
        });
        *self = candidate;
        Ok(effects)
    }
    fn raw_sources_inner(
        &mut self,
        p: &PreparedQcc,
        now: u64,
        certificates: BTreeMap<usize, RawSourceCertificate>,
    ) -> Result<Vec<QccEffect>, Rejection> {
        if self.last_ps.is_some_and(|last| now < last)
            || certificates
                .iter()
                .any(|(a, c)| *a != c.attachment || !c.valid_at(now))
        {
            return Err(Rejection::InvalidState);
        }
        self.dependency_raw = certificates;
        if self
            .pending
            .as_ref()
            .is_some_and(|t| !self.raw_plan_valid(p, &t.plan, now))
            || self.active_valid && !self.raw_plan_valid(p, &self.active, now)
        {
            return self.topology_inner(p, now, self.available.clone());
        }
        Ok(Vec::new())
    }
    pub fn source_admissions(&self) -> Vec<(String, u64, usize)> {
        self.audit
            .iter()
            .filter_map(|step| match step {
                AuditStep::Source {
                    generator,
                    ready_ps,
                    payload_bytes,
                } => Some((generator.clone(), *ready_ps, *payload_bytes)),
                _ => None,
            })
            .collect()
    }
    pub fn advance_history(&self) -> Vec<(u64, bool)> {
        self.audit
            .iter()
            .filter_map(|step| match step {
                AuditStep::Advance { at_ps, quiescent } => Some((*at_ps, *quiescent)),
                _ => None,
            })
            .collect()
    }
    pub fn topology_history(&self) -> Vec<(u64, Vec<bool>)> {
        self.audit
            .iter()
            .filter_map(|step| match step {
                AuditStep::Topology { at_ps, available } => Some((*at_ps, available.clone())),
                _ => None,
            })
            .collect()
    }
    /// Operating schedule provenance, including initial closure and topology
    /// suspension. The owner regenerates TAS operating state from this ledger.
    pub fn installation_history(
        &self,
        p: &PreparedQcc,
    ) -> Result<Vec<(u64, u64, ConfigurationPlan)>, Rejection> {
        let mut replay = Self::new(p)?;
        let mut history = vec![(0, 0, replay.active.clone())];
        for step in &self.audit {
            let (at, effects) = match step {
                AuditStep::RawSources {
                    at_ps,
                    certificates,
                } => (
                    *at_ps,
                    replay.raw_sources_inner(p, *at_ps, certificates.clone())?,
                ),
                AuditStep::Advance { at_ps, quiescent } => {
                    (*at_ps, replay.advance_inner(p, *at_ps, *quiescent)?)
                }
                AuditStep::Topology { at_ps, available } => {
                    (*at_ps, replay.topology_inner(p, *at_ps, available.clone())?)
                }
                AuditStep::Protocol { at_ps, direction } => {
                    if !p.depends_on_direction(*direction) {
                        return Err(Rejection::InvalidState);
                    }
                    (*at_ps, replay.protocol_inner(p, *at_ps, *direction)?)
                }
                AuditStep::Clock { at_ps, node, clock } => {
                    (*at_ps, replay.clock_inner(p, *at_ps, *node, clock.clone())?)
                }
                AuditStep::Source {
                    generator,
                    ready_ps,
                    payload_bytes,
                } => {
                    replay.admit_source_inner(p, generator, *ready_ps, *payload_bytes)?;
                    continue;
                }
            };
            for effect in effects {
                match effect {
                    QccEffect::Installed { version, plan, .. } => history.push((at, version, plan)),
                    QccEffect::Suspended { version, .. } => history.push((
                        at,
                        version,
                        solve(
                            p,
                            &BTreeSet::new(),
                            replay.topology_epoch,
                            &replay.available,
                        )?,
                    )),
                    _ => {}
                }
            }
        }
        Ok(history)
    }
    /// Re-run the small management/contract transition service, not the packet
    /// simulation. The owner additionally binds topology and source audit inputs
    /// to its physical/control and accepted-frame ledgers during restore.
    pub fn validate(&self, p: &PreparedQcc) -> Result<(), Rejection> {
        let mut replay = Self::new(p)?;
        for step in &self.audit {
            match step {
                AuditStep::RawSources {
                    at_ps,
                    certificates,
                } => {
                    replay.raw_sources_inner(p, *at_ps, certificates.clone())?;
                }
                AuditStep::Advance { at_ps, quiescent } => {
                    replay.advance_inner(p, *at_ps, *quiescent)?;
                }
                AuditStep::Topology { at_ps, available } => {
                    replay.topology_inner(p, *at_ps, available.clone())?;
                }
                AuditStep::Protocol { at_ps, direction } => {
                    if !p.depends_on_direction(*direction) {
                        return Err(Rejection::InvalidState);
                    }
                    replay.protocol_inner(p, *at_ps, *direction)?;
                }
                AuditStep::Clock { at_ps, node, clock } => {
                    replay.clock_inner(p, *at_ps, *node, clock.clone())?;
                }
                AuditStep::Source {
                    generator,
                    ready_ps,
                    payload_bytes,
                } => {
                    replay.admit_source_inner(p, generator, *ready_ps, *payload_bytes)?;
                }
            }
        }
        let mut expected = self.clone();
        expected.audit.clear();
        if replay != expected {
            return Err(Rejection::InvalidState);
        }
        Ok(())
    }
}
/// Duration/mask entries consume one complete cycle. Background has its own
/// complementary windows; managed classes remain closed after withdrawal.
pub fn gate_entries(p: &PreparedQcc, port: &PortPlan) -> Result<Vec<(u64, u8)>, Rejection> {
    let mut result = Vec::new();
    let mut cursor = 0;
    let background = !p.managed_priority_mask(port.direction);
    let mut windows: Vec<_> = port
        .slots
        .iter()
        .map(|s| (s, 1u8 << s.priority))
        .chain(port.closed_slots.iter().map(|s| (s, 0)))
        .collect();
    windows.sort_by_key(|(s, _)| s.start_ps);
    for (slot, mask) in windows {
        if slot.start_ps < cursor
            || slot.end_ps <= slot.start_ps
            || slot.end_ps > p.config.cycle_ps.0
            || slot.priority > 7
        {
            return Err(Rejection::InvalidState);
        }
        if slot.start_ps > cursor {
            result.push((slot.start_ps - cursor, background));
        }
        result.push((slot.end_ps - slot.start_ps, mask));
        cursor = slot.end_ps;
    }
    if cursor < p.config.cycle_ps.0 {
        result.push((p.config.cycle_ps.0 - cursor, background));
    }
    Ok(result)
}

/// Compile a certified port plan to the production TAS representation.
pub fn schedule(
    p: &PreparedQcc,
    plan: &PortPlan,
    base_ps: u64,
    version: u64,
) -> Result<std::sync::Arc<crate::types::ethernet::tsn::Schedule>, Rejection> {
    schedule_clock(p, plan, base_ps, version, None)
}

/// CNC slots are physical-time reservations; their operating GCL boundaries
/// are projected into the clock actually owned by the transmitting node.
pub fn schedule_clock(
    p: &PreparedQcc,
    plan: &PortPlan,
    base_ps: u64,
    version: u64,
    clock: Option<&super::clock::NodeClock>,
) -> Result<std::sync::Arc<crate::types::ethernet::tsn::Schedule>, Rejection> {
    let certificate = compile_certificate(p, plan, base_ps, clock)?;
    compiled_schedule(&certificate, version)
}

pub(super) fn schedule_entries(
    certificate: &ScheduleCertificate,
    version: u64,
) -> Result<std::sync::Arc<crate::types::ethernet::tsn::Schedule>, Rejection> {
    use crate::types::ethernet::tsn::{GateEntry, OpenRun, Schedule};
    let local_base = certificate.base_ps;
    let cycle_ps = certificate.cycle_ps;
    let plan_direction = certificate.direction;
    let entries: Vec<_> = certificate
        .entries
        .iter()
        .map(|(duration_ps, open_mask)| GateEntry {
            operation: crate::types::ethernet::tsn::GateOperation::SetGateStates,
            duration_ps: *duration_ps,
            open_mask: *open_mask,
        })
        .collect();
    let mut prefix_ps = vec![0u64];
    let mut class_open_runs: [Vec<OpenRun>; 8] = std::array::from_fn(|_| Vec::new());
    let mut open_total_ps = [0u64; 8];
    for entry in &entries {
        prefix_ps.push(add(*prefix_ps.last().unwrap(), entry.duration_ps)?);
    }
    if *prefix_ps.last().unwrap() != cycle_ps {
        return Err(Rejection::InvalidState);
    }
    for class in 0..8 {
        for (i, entry) in entries.iter().enumerate() {
            if entry.open_mask & (1 << class) == 0 {
                continue;
            }
            open_total_ps[class] = add(open_total_ps[class], entry.duration_ps)?;
            let runs = &mut class_open_runs[class];
            if let Some(last) = runs
                .last_mut()
                .filter(|run| run.end_ps == prefix_ps[i] as u128)
            {
                last.end_ps = prefix_ps[i + 1] as u128;
            } else {
                runs.push(OpenRun {
                    start_ps: prefix_ps[i],
                    end_ps: prefix_ps[i + 1] as u128,
                });
            }
        }
        let runs = &mut class_open_runs[class];
        if runs.len() > 1
            && runs[0].start_ps == 0
            && runs.last().unwrap().end_ps == cycle_ps as u128
        {
            let head = runs[0].end_ps;
            runs.last_mut().unwrap().end_ps += head;
        }
    }
    Ok(std::sync::Arc::new(Schedule {
        id: format!("cnc_v{version}_d{plan_direction}"),
        base_ps: local_base,
        cycle_ps,
        entries,
        prefix_ps,
        class_open_runs,
        open_total_ps,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared() -> PreparedQcc {
        let requirement = StreamRequirement {
            id: "s".into(),
            generator: "g".into(),
            listeners: vec!["listener".into()],
            interval: Interval {
                numerator: 1,
                denominator: 1_000_000,
            },
            max_frames_per_interval: 2,
            max_frame_size: 46,
            max_latency_ps: Ps(1_000_000),
            earliest_transmit_offset_ps: Ps(100_000),
            latest_transmit_offset_ps: Ps(200_000),
            jitter_ps: Ps(0),
            rank: 1,
            num_seamless_trees: 1,
            paths: vec![PathRequirement {
                listener: "listener".into(),
                ports: vec!["a".into(), "b".into()],
            }],
        };
        let ports = (0..2)
            .map(|i| ManagedPort {
                port: ["a", "b"][i].into(),
                direction: i,
                source: i,
                destination: i + 1,
                bitrate_bps: 10_000_000_000,
                propagation_ps: 1000,
                source_processing_ps: 0,
                destination_processing_ps: 1000,
                calendar_domain: i,
                writable_schedule: true,
                clock_offset_ps: 0,
                clock_rate_num: 1,
                clock_rate_den: 1,
                clock_fractional: false,
                capacity_frames: 4,
                class_capacity_frames: [4; 8],
                class_capacity_bytes: [Some(1024); 8],
            })
            .collect();
        let config = QccConfig {
            architecture: "fully_centralized".into(),
            transport: "dir.cuc-cnc.local.v1".into(),
            standard_edition: "IEEE802.1Q-2022".into(),
            cycle_ps: Ps(1_000_000),
            management_latency_ps: Ps(1000),
            clock_error_budget_ps: Ps(0),
            search_budget: 1000,
            max_streams: 4,
            requirements: vec![requirement.clone()],
            operations: vec![ManagementOperation::Prepare {
                id: "install".into(),
                at_ps: Ps(0),
                expected_topology_epoch: Ps(0),
                add: vec!["s".into()],
                remove: vec![],
                activate_at_ps: Ps(1_000_000),
                timeout_at_ps: Ps(50_000),
            }],
        };
        PreparedQcc {
            config,
            ports,
            streams: vec![PreparedStream {
                requirement,
                priority: 3,
                mac_bytes: 68,
                source: 0,
                paths: vec![vec![0, 1]],
                demands: vec![],
                unproven: None,
            }],
            stream_index: BTreeMap::from([("s".into(), 0)]),
            generator_index: BTreeMap::from([("g".into(), 0)]),
            reserved_priorities: 1 << 3,
        }
    }
    fn install(p: &PreparedQcc) -> QccState {
        let mut s = QccState::new(p).unwrap();
        assert!(matches!(
            s.advance(p, 0, true).unwrap()[0],
            QccEffect::Prepared { .. }
        ));
        assert_eq!(s.next_deadline(p), Some(1000));
        s.advance(p, 1000, true).unwrap();
        assert_eq!(s.next_deadline(p), Some(1_000_000));
        assert!(matches!(
            s.advance(p, 1_000_000, true).unwrap()[0],
            QccEffect::Installed { version: 1, .. }
        ));
        s
    }
    #[test]
    fn qcc_plan_uses_per_frame_ceil_padding_ifg_and_path_processing() {
        let p = prepared();
        assert_eq!(occupied_ps(68, 10_000_000_000).unwrap(), 70_400);
        let plan = solve(&p, &BTreeSet::from([0]), 0, &[true, true]).unwrap();
        assert_eq!(
            plan.ports[0].slots[0],
            GateSlot {
                start_ps: 100_000,
                end_ps: 240_800,
                priority: 3
            }
        );
        assert_eq!(plan.ports[1].slots[0].start_ps, 242_800);
        assert_eq!(plan.latency_bounds_ps["s"], 385_600);
        assert_eq!(
            gate_entries(&p, &plan.ports[0]).unwrap(),
            vec![(100_000, 247), (140_800, 8), (759_200, 247)]
        );
        assert_eq!(plan.reservations[0].frames[3], 2);
        assert_eq!(plan.reservations[0].bytes[3], 136);
        let mut bad = p.clone();
        bad.ports[0].bitrate_bps = 1;
        assert_eq!(
            solve(&bad, &BTreeSet::from([0]), 0, &[true, true]),
            Err(Rejection::InfeasibleFrameCapacity)
        );
        bad = p.clone();
        bad.config.search_budget = 1;
        assert_eq!(
            solve(&bad, &BTreeSet::from([0]), 0, &[true, true]),
            Err(Rejection::SearchLimit)
        );
        bad = p.clone();
        bad.ports[0].capacity_frames = 1;
        assert_eq!(
            solve(&bad, &BTreeSet::from([0]), 0, &[true, true]),
            Err(Rejection::BufferCapacity)
        );
    }
    #[test]
    fn qcc_zero_latency_requests_discovered_bound_instead_of_zero_deadline() {
        let mut p = prepared();
        p.streams[0].requirement.max_latency_ps = Ps(0);
        let plan = solve(&p, &BTreeSet::from([0]), 0, &[true, true]).unwrap();
        assert_eq!(plan.latency_bounds_ps["s"], 385_600);
        p.streams[0].requirement.max_latency_ps = Ps(385_599);
        assert_eq!(
            solve(&p, &BTreeSet::from([0]), 0, &[true, true]),
            Err(Rejection::LatencyBound)
        );
        p.streams[0].requirement.max_latency_ps = Ps(385_600);
        assert!(solve(&p, &BTreeSet::from([0]), 0, &[true, true]).is_ok());
    }
    #[test]
    fn qcc_acks_commit_source_contract_and_background_reservation() {
        let p = prepared();
        let mut s = install(&p);
        assert!(s.background_buffer_allowed(&p, 0, 0, 1, [0; 8]));
        assert!(!s.background_buffer_allowed(&p, 0, 0, 2, [0; 8]));
        let mut counts = [0; 8];
        counts[3] = 1;
        assert!(s.background_buffer_allowed(&p, 0, 0, 2, counts));
        let a = s.admit_source(&p, "g", 1_050_000, 46).unwrap();
        assert_eq!(a.interval_origin_ps, 1_000_000);
        assert_eq!(a.path, vec![0, 1]);
        s.admit_source(&p, "g", 1_100_000, 46).unwrap();
        assert_eq!(
            s.admit_source(&p, "g", 1_100_000, 46),
            Err(Rejection::ExcessBurst)
        );
        assert_eq!(
            s.admit_source(&p, "g", 1_100_001, 46),
            Err(Rejection::LateSource)
        );
        assert_eq!(
            s.admit_source(&p, "g", 2_000_000, 47),
            Err(Rejection::Oversize)
        );
        assert_eq!(s.route(&p, 1, "g", None).unwrap(), Some(0));
        assert_eq!(s.route(&p, 1, "g", Some(0)).unwrap(), Some(1));
        assert_eq!(s.route(&p, 1, "g", Some(1)).unwrap(), None);
        s.validate(&p).unwrap();
        let bytes = serde_json::to_vec(&s).unwrap();
        let restored: QccState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, s);
        restored.validate(&p).unwrap();
        let mut forged = s.clone();
        forged.active.ports[0].slots[0].end_ps -= 1;
        assert_eq!(forged.validate(&p), Err(Rejection::InvalidState));
        forged = s.clone();
        forged.counters.get_mut(&0).unwrap().count = 0;
        assert_eq!(forged.validate(&p), Err(Rejection::InvalidState));
        forged = s.clone();
        forged.version += 1;
        assert_eq!(forged.validate(&p), Err(Rejection::InvalidState));
    }
    #[test]
    fn qcc_timeout_reject_busy_cancel_and_undrained_never_install() {
        for (mode, expected) in [
            (ParticipantMode::Reject, Rejection::ParticipantRejected),
            (ParticipantMode::Silent, Rejection::Timeout),
        ] {
            let mut p = prepared();
            p.config.operations.insert(
                0,
                ManagementOperation::Participant {
                    id: "mode".into(),
                    at_ps: Ps(0),
                    port: "a".into(),
                    mode,
                },
            );
            let mut s = QccState::new(&p).unwrap();
            s.advance(&p, 0, true).unwrap();
            let mut effects = s.advance(&p, 1000, true).unwrap();
            if mode == ParticipantMode::Silent {
                effects.extend(s.advance(&p, 50_000, true).unwrap());
            }
            assert!(
                effects
                    .iter()
                    .any(|e| matches!(e,QccEffect::Rejected{reason,..} if *reason==expected))
            );
            assert_eq!(s.version, 0);
            assert!(s.active.streams.is_empty());
            s.validate(&p).unwrap();
        }
        let p = prepared();
        let mut s = QccState::new(&p).unwrap();
        s.advance(&p, 0, true).unwrap();
        s.advance(&p, 1000, true).unwrap();
        assert!(matches!(
            s.advance(&p, 1_000_000, false).unwrap()[0],
            QccEffect::Rejected {
                reason: Rejection::TransitionNotDrained,
                ..
            }
        ));
        assert_eq!(s.version, 0);
        s.validate(&p).unwrap();
        let mut p = prepared();
        p.config.operations.push(ManagementOperation::Prepare {
            id: "busy".into(),
            at_ps: Ps(1),
            expected_topology_epoch: Ps(0),
            add: vec!["s".into()],
            remove: vec![],
            activate_at_ps: Ps(2_000_000),
            timeout_at_ps: Ps(40_000),
        });
        p.config.operations.push(ManagementOperation::Cancel {
            id: "cancel".into(),
            at_ps: Ps(2),
            transaction: "install".into(),
        });
        let mut s = QccState::new(&p).unwrap();
        s.advance(&p, 0, true).unwrap();
        assert!(matches!(
            s.advance(&p, 1, true).unwrap()[0],
            QccEffect::Rejected {
                reason: Rejection::Busy,
                ..
            }
        ));
        assert!(matches!(
            s.advance(&p, 2, true).unwrap()[0],
            QccEffect::Rejected {
                reason: Rejection::Cancelled,
                ..
            }
        ));
        assert!(s.next_deadline(&p).is_none());
        s.validate(&p).unwrap();
    }
    #[test]
    fn qcc_generated_schedule_drives_real_tas_and_managed_checkpoint_restore() {
        use crate::runtime::ethernet::tsn::{Head, PortInput, Sending, TsnDelta, TsnState};
        use crate::types::ethernet::tsn::{PreparedTsn, TsnOutput};
        let p = prepared();
        let prepared_tsn = PreparedTsn {
            outputs: p
                .ports
                .iter()
                .map(|port| TsnOutput {
                    port: port.port.clone(),
                    link_bps: port.bitrate_bps,
                    tas: None,
                    cbs: std::array::from_fn(|_| None),
                })
                .collect(),
            port_index: BTreeMap::from([("a".into(), 0), ("b".into(), 1)]),
            streams: vec![],
            stream_index: BTreeMap::new(),
            updates: vec![],
        };
        let mut state = TsnState::new(&prepared_tsn);
        let mut input = PortInput::default();
        input.heads[3] = Some(Head {
            occupancy_ps: 70_400,
        });
        input.backlog[3] = true;
        let inputs = BTreeMap::from([("a".into(), input.clone()), ("b".into(), input.clone())]);
        let empty = solve(&p, &BTreeSet::new(), 0, &[true, true]).unwrap();
        let initial: BTreeMap<_, _> = empty
            .ports
            .iter()
            .map(|port| {
                (
                    p.ports[port.direction].port.clone(),
                    schedule(&p, port, 0, 0).unwrap(),
                )
            })
            .collect();
        let delta = state
            .plan_install_in(TsnDelta::default(), 0, &initial, &inputs)
            .unwrap();
        assert!(!delta.selected.contains_key("a"));
        state.apply(delta);
        let plan = solve(&p, &BTreeSet::from([0]), 0, &[true, true]).unwrap();
        let schedules: BTreeMap<_, _> = plan
            .ports
            .iter()
            .map(|port| {
                (
                    p.ports[port.direction].port.clone(),
                    schedule(&p, port, 1_000_000, 1).unwrap(),
                )
            })
            .collect();
        let delta = state
            .plan_install_in(TsnDelta::default(), 1_000_000, &schedules, &inputs)
            .unwrap();
        assert!(!delta.selected.contains_key("a"));
        state.apply(delta);
        let delta = state
            .plan_port_clock_in(TsnDelta::default(), "a", 1_100_000, &input, None)
            .unwrap();
        assert_eq!(delta.selected.get("a"), Some(&3));
        state.apply(delta);
        let raw = state.checkpoint_fields();
        let mut restored = TsnState::restore_fields(&raw, TsnState::new(&prepared_tsn)).unwrap();
        let managed = schedules
            .iter()
            .map(|(port, schedule)| (port.clone(), (schedule.clone(), 2)))
            .collect();
        restored
            .validate_checkpoint_with_managed(1_100_000, &managed)
            .unwrap();
        let delta = restored
            .plan_port_clock_in(TsnDelta::default(), "a", 1_100_000, &input, None)
            .unwrap();
        assert_eq!(delta.selected.get("a"), Some(&3));
        let mut busy = inputs.clone();
        busy.get_mut("a").unwrap().sending = Some(Sending {
            priority: 0,
            release_ps: 2_000_000,
        });
        assert!(
            state
                .plan_install_in(TsnDelta::default(), 1_000_000, &schedules, &busy)
                .is_err()
        );
    }
    #[test]
    fn qcc_suspend_closes_managed_gate_without_inventing_current_tx_completion() {
        use crate::runtime::ethernet::tsn::{Head, PortInput, Sending, TsnDelta, TsnState};
        use crate::types::ethernet::tsn::{PreparedTsn, TsnOutput};
        let p = prepared();
        let prepared_tsn = PreparedTsn {
            outputs: vec![TsnOutput {
                port: "a".into(),
                link_bps: 10_000_000_000,
                tas: None,
                cbs: std::array::from_fn(|_| None),
            }],
            port_index: BTreeMap::from([("a".into(), 0)]),
            streams: vec![],
            stream_index: BTreeMap::new(),
            updates: vec![],
        };
        let mut tsn = TsnState::new(&prepared_tsn);
        let mut input = PortInput {
            sending: Some(Sending {
                priority: 3,
                release_ps: 90_000,
            }),
            ..PortInput::default()
        };
        input.backlog[3] = true;
        input.heads[3] = Some(Head {
            occupancy_ps: 70_400,
        });
        let plan = PortPlan {
            direction: 0,
            slots: vec![],
            closed_slots: vec![],
        };
        let schedules = BTreeMap::from([("a".into(), schedule(&p, &plan, 10_000, 1).unwrap())]);
        let inputs = BTreeMap::from([("a".into(), input.clone())]);
        assert!(
            tsn.plan_install_in(TsnDelta::default(), 10_000, &schedules, &inputs)
                .is_err()
        );
        let delta = tsn
            .plan_suspend_in(
                TsnDelta::default(),
                10_000,
                &schedules,
                &inputs,
                p.reserved_priorities,
            )
            .unwrap();
        assert!(!delta.selected.contains_key("a"));
        tsn.apply(delta);
        let state = tsn.checkpoint_fields();
        assert_eq!(
            state["ports"][0]["sending"]["release_ps"],
            serde_json::json!(90_000)
        );
        assert_eq!(
            state["ports"][0]["sending"]["priority"],
            serde_json::json!(3)
        );
        input.sending = None;
        let delta = tsn
            .plan_port_clock_in(TsnDelta::default(), "a", 90_000, &input, None)
            .unwrap();
        assert!(!delta.selected.contains_key("a"));
        let open = PortPlan {
            direction: 0,
            closed_slots: vec![],
            slots: vec![GateSlot {
                start_ps: 0,
                end_ps: 100_000,
                priority: 3,
            }],
        };
        let invalid = BTreeMap::from([("a".into(), schedule(&p, &open, 90_000, 2).unwrap())]);
        assert!(
            tsn.plan_suspend_in(
                TsnDelta::default(),
                90_000,
                &invalid,
                &inputs,
                p.reserved_priorities
            )
            .is_err()
        );
    }
    #[test]
    fn qcc_topology_aborts_candidate_and_rollback_recomputes_current_view() {
        let mut p = prepared();
        p.config.operations.push(ManagementOperation::Prepare {
            id: "remove".into(),
            at_ps: Ps(1_000_001),
            expected_topology_epoch: Ps(0),
            add: vec![],
            remove: vec!["s".into()],
            activate_at_ps: Ps(2_000_000),
            timeout_at_ps: Ps(1_050_000),
        });
        p.config.operations.push(ManagementOperation::Rollback {
            id: "rollback".into(),
            at_ps: Ps(2_000_001),
            expected_topology_epoch: Ps(0),
            activate_at_ps: Ps(3_000_000),
            timeout_at_ps: Ps(2_050_000),
        });
        let mut s = install(&p);
        s.advance(&p, 1_000_001, true).unwrap();
        s.advance(&p, 1_001_001, true).unwrap();
        s.advance(&p, 2_000_000, true).unwrap();
        assert_eq!(s.version, 2);
        assert!(s.active.streams.is_empty());
        s.advance(&p, 2_000_001, true).unwrap();
        s.advance(&p, 2_001_001, true).unwrap();
        assert!(matches!(
            s.advance(&p, 3_000_000, true).unwrap()[0],
            QccEffect::Installed {
                version: 3,
                rollback: true,
                ..
            }
        ));
        assert_eq!(s.active.streams, vec![0]);
        s.validate(&p).unwrap();
        let p = prepared();
        let mut s = QccState::new(&p).unwrap();
        s.advance(&p, 0, true).unwrap();
        let effects = s.topology_changed(&p, 500, vec![true, false]).unwrap();
        assert!(matches!(
            effects[0],
            QccEffect::Rejected {
                reason: Rejection::StaleTopology,
                ..
            }
        ));
        assert_eq!(s.topology_epoch, 1);
        assert!(!s.active_valid);
        assert!(s.next_deadline(&p).is_none());
        s.validate(&p).unwrap();
    }
    #[test]
    fn qcc_replicated_prefix_is_once_and_shared_branch_calendar_is_joint() {
        let mut p = prepared();
        let mut third = p.ports[1].clone();
        third.port = "c".into();
        third.direction = 2;
        p.ports.push(third);
        p.streams[0].paths = vec![vec![0, 1], vec![0, 2]];
        let plan = solve(&p, &BTreeSet::from([0]), 0, &[true; 3]).unwrap();
        assert_eq!(plan.reservations[0].frames[3], 2, "one prefix TX per frame");
        assert_eq!(plan.reservations[1].frames[3], 2);
        assert_eq!(plan.reservations[2].frames[3], 2);
        assert_eq!(plan.ports[1].slots[0].start_ps, 242_800);
        assert_eq!(plan.ports[2].slots[0].start_ps, 383_600);
        assert_eq!(plan.ports[1].closed_slots, plan.ports[2].slots);
        assert_eq!(plan.ports[2].closed_slots, plan.ports[1].slots);
        assert_eq!(plan.latency_bounds_ps["s"], 526_400);
        let masks = gate_entries(&p, &plan.ports[1]).unwrap();
        assert!(
            masks.contains(&(140_800, 0)),
            "other station owns the domain"
        );
    }
    #[test]
    fn qcc_unproven_lossless_request_rejects_transaction_without_rejecting_owner() {
        let mut p = prepared();
        p.streams[0].unproven = Some(UnprovenBound::PhysicalNoiseOrRetry);
        let mut state = QccState::new(&p).unwrap();
        assert!(matches!(
            &state.advance(&p, 0, true).unwrap()[0],
            QccEffect::Rejected {
                reason: Rejection::NoProvenBound(UnprovenBound::PhysicalNoiseOrRetry),
                ..
            }
        ));
        assert!(state.active.streams.is_empty());
        state.validate(&p).unwrap();
    }
    #[test]
    fn qcc_clock_dependency_suspends_then_reconfigures_and_rejects_snapshot_mutant() {
        use super::super::clock::NodeClock;
        let mut p = prepared();
        let mut state = install(&p);
        let mut clock = NodeClock::new(0, 0, 1, 1).unwrap();
        clock.step_by(1_000_001, 123).unwrap();
        let effects = state
            .clock_changed(&p, 1_000_001, 0, clock.clone())
            .unwrap();
        assert!(matches!(
            &effects[0],
            QccEffect::Suspended {
                topology_epoch: 1,
                ..
            }
        ));
        assert!(!state.active_valid);
        state.validate(&p).unwrap();
        let mut forged = state.clone();
        forged.dependency_clocks.clear();
        assert_eq!(forged.validate(&p), Err(Rejection::InvalidState));
        p.config.operations.push(ManagementOperation::Prepare {
            id: "renew".into(),
            at_ps: Ps(1_100_000),
            expected_topology_epoch: Ps(1),
            add: vec![],
            remove: vec![],
            activate_at_ps: Ps(2_000_000),
            timeout_at_ps: Ps(1_200_000),
        });
        state.advance(&p, 1_100_000, true).unwrap();
        state.advance(&p, 1_101_000, true).unwrap();
        state.advance(&p, 2_000_000, true).unwrap();
        assert!(state.active_valid);
        assert_eq!(state.version, 2);
        assert_eq!(state.clock_history(), vec![(1_000_001, 0, clock)]);
        state.validate(&p).unwrap();
    }
    #[test]
    fn qcc_operational_gcl_projects_affine_clock_into_actual_tas() {
        use super::super::clock::NodeClock;
        let p = prepared();
        let plan = solve(&p, &BTreeSet::from([0]), 0, &[true; 2]).unwrap();
        let clock = NodeClock::new(0, 17, 3, 2).unwrap();
        let local = schedule_clock(&p, &plan.ports[0], 1_000_000, 1, Some(&clock)).unwrap();
        assert_eq!(local.base_ps, 1_500_017);
        assert_eq!(local.cycle_ps, 1_500_000);
        assert!(!local.open(3, 1_650_016));
        assert!(local.open(3, 1_650_017));
        assert!(!local.open(3, 1_861_217));
    }
    #[test]
    fn qcc_joint_nonintegral_supercycle_rounds_inward_and_foreign_outward() {
        use super::super::clock::{LocalTime, NodeClock};
        let mut p = prepared();
        p.config.cycle_ps = Ps(1_000_001);
        p.config.search_budget = 100_000;
        p.ports.push(p.ports[1].clone());
        p.ports[2].direction = 2;
        for (index, (num, den)) in [(3, 2), (1, 3), (2, 5)].into_iter().enumerate() {
            p.ports[index].calendar_domain = 0;
            p.ports[index].clock_rate_num = num;
            p.ports[index].clock_rate_den = den;
        }
        p.streams[0].paths = vec![vec![0], vec![1], vec![2]];
        let plan = PortPlan {
            direction: 0,
            slots: vec![GateSlot {
                start_ps: 100,
                end_ps: 1000,
                priority: 3,
            }],
            closed_slots: vec![GateSlot {
                start_ps: 1000,
                end_ps: 1500,
                priority: 3,
            }],
        };
        let activation = 1_000_010;
        let clock = NodeClock::new(0, -77, 3, 2).unwrap();
        let cert = compile_certificate(&p, &plan, activation, Some(&clock)).unwrap();
        assert_eq!(cert.cycle_ps, 45_000_045); // C*LCM(2,3,5)*3/2
        let schedule = compiled_schedule(&cert, 1).unwrap();
        for repeat in 0..30 {
            let origin = activation + repeat * p.config.cycle_ps.0;
            let a = origin + 100;
            let b = origin + 1000;
            let first_local = clock.local_at(a).unwrap().ceil_ps().unwrap();
            let last_local = clock.local_at(b).unwrap().floor_ps().unwrap();
            let first = clock
                .deadline(a, LocalTime::integer(first_local))
                .unwrap()
                .global_ps;
            let last = clock
                .deadline(a, LocalTime::integer(last_local))
                .unwrap()
                .global_ps;
            assert!(first >= a && last <= b && last - first >= 898);
            let open = |at| {
                schedule.open(
                    3,
                    u64::try_from(clock.local_at(at).unwrap().floor_ps().unwrap()).unwrap(),
                )
            };
            assert!(!open(a - 1));
            assert!(open(first));
            assert!(!open(b));
            for class in 0..8 {
                assert!(!schedule.open(
                    class,
                    u64::try_from(clock.local_at(b).unwrap().floor_ps().unwrap()).unwrap()
                ));
            }
        }
    }
    #[test]
    fn qcc_fractional_unit_rate_anchor_keeps_guard_and_wraps_last_foreign_tail() {
        use super::super::clock::NodeClock;
        let mut p = prepared();
        p.ports[0].clock_fractional = true;
        assert_eq!(rounding_quantum(&p.ports[0]), 1);
        let mut clock = NodeClock::new(0, 0, 3, 2).unwrap();
        clock.slew(1, 1, 1).unwrap();
        let plan = PortPlan {
            direction: 0,
            slots: vec![GateSlot {
                start_ps: 100,
                end_ps: 200,
                priority: 3,
            }],
            closed_slots: vec![GateSlot {
                start_ps: 999_900,
                end_ps: 1_000_000,
                priority: 3,
            }],
        };
        let cert = compile_certificate(&p, &plan, 1000, Some(&clock)).unwrap();
        let schedule = compiled_schedule(&cert, 1).unwrap();
        assert_eq!(cert.base_ps, 1000);
        assert!(
            !schedule.open(0, 1000),
            "outward fractional tail wraps into cycle head"
        );
        assert!(!schedule.open(3, 1100));
        assert!(schedule.open(3, 1101));
        assert!(!schedule.open(3, 1200));
    }
    #[test]
    fn qcc_projection_rejects_negative_activation_budget_and_overflow_before_pending() {
        use super::super::clock::NodeClock;
        let mut p = prepared();
        let plan = PortPlan {
            direction: 0,
            slots: vec![],
            closed_slots: vec![],
        };
        let negative = NodeClock::new(0, -1000, 1, 1).unwrap();
        assert_eq!(
            compile_certificate(&p, &plan, 100, Some(&negative)),
            Err(Rejection::NoProvenBound(UnprovenBound::ClockProjection))
        );
        let costly = NodeClock::new(0, 0, 1, 999_983).unwrap();
        assert_eq!(
            compile_certificate(&p, &plan, 100, Some(&costly)),
            Err(Rejection::SearchLimit)
        );
        p.config.cycle_ps = Ps(u64::MAX);
        assert_eq!(
            compile_certificate(&p, &plan, 0, Some(&NodeClock::new(0, 0, 3, 2).unwrap())),
            Err(Rejection::ArithmeticOverflow)
        );
        p = prepared();
        p.ports[0].clock_offset_ps = -2_000_000;
        let mut state = QccState::new(&p).unwrap();
        assert!(matches!(
            &state.advance(&p, 0, true).unwrap()[0],
            QccEffect::Rejected {
                reason: Rejection::NoProvenBound(UnprovenBound::ClockProjection),
                ..
            }
        ));
        assert!(state.pending.is_none());
    }
    #[test]
    fn qcc_merge_peer_arrivals_carry_in_and_fixed_point_are_independently_bounded() {
        let envelope = MergeControlEnvelope {
            raw_attachment: None,
            admission_alignment_ps: 0,
            peer_direction: 1,
            local_lldp_interval_ps: 1_000_000,
            peer_lldp_interval_ps: 1_000_000,
            local_lldp_jitter_ps: 0,
            peer_lldp_jitter_ps: 0,
            peer_verify_jitter_ps: 0,
            lldp_occupied_ps: 100,
            verify_occupied_ps: 200,
            respond_occupied_ps: 300,
            carry_in_ps: 400,
        };
        assert_eq!(
            merge_interference(&envelope, 900).unwrap(),
            4 * 100 + 15 * 200 + 17 * 300
        );
        let demand = TransmissionDemand {
            direction: 0,
            priority: 3,
            vid: 10,
            mac_bytes: 68,
            occupied_ps: 500,
            propagation_ps: 0,
            destination_processing_ps: 0,
            merge_control: Some(envelope),
            start_jitter_ps: 0,
        };
        let mut budget = 0;
        assert_eq!(
            envelope::burst_service(Some(&demand), 500, 1, 20_000, &mut budget, 100).unwrap(),
            (9400, 8900)
        );
        assert_eq!(
            envelope::burst_service(Some(&demand), 500, 1, 9000, &mut 0, 100),
            Err(Rejection::InfeasibleFrameCapacity)
        );
        let p = prepared();
        let mut state = QccState::new(&p).unwrap();
        state.advance(&p, 0, true).unwrap();
        let mut forged = state.clone();
        forged.pending.as_mut().unwrap().plan.schedules[0].entries[0].0 += 1;
        assert_eq!(forged.validate(&p), Err(Rejection::InvalidState));
    }
}
