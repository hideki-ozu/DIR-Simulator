//! Explicit local CUC/CNC management profile. This is not a Qcc wire encoding.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ps(pub u64);
impl Serialize for Ps {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for Ps {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let value = text.parse::<u64>().map_err(serde::de::Error::custom)?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom(
                "expected canonical u64 decimal time",
            ));
        }
        Ok(Self(value))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    pub numerator: u32,
    pub denominator: u32,
}
impl Interval {
    pub fn ps(&self) -> Option<u64> {
        if self.numerator == 0 || self.denominator == 0 {
            return None;
        }
        let n = self.numerator as u128 * 1_000_000_000_000;
        if n % self.denominator as u128 != 0 {
            return None;
        }
        u64::try_from(n / self.denominator as u128).ok()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamRequirement {
    pub id: String,
    pub generator: String,
    pub listeners: Vec<String>,
    pub interval: Interval,
    pub max_frames_per_interval: u16,
    pub max_frame_size: u16,
    pub max_latency_ps: Ps,
    pub earliest_transmit_offset_ps: Ps,
    pub latest_transmit_offset_ps: Ps,
    pub jitter_ps: Ps,
    pub rank: u8,
    pub num_seamless_trees: u8,
    /// One explicitly selected egress-port route to each listener per tree.
    pub paths: Vec<PathRequirement>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathRequirement {
    pub listener: String,
    pub ports: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantMode {
    Accept,
    Reject,
    Silent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagementOperation {
    Prepare {
        id: String,
        at_ps: Ps,
        expected_topology_epoch: Ps,
        add: Vec<String>,
        remove: Vec<String>,
        activate_at_ps: Ps,
        timeout_at_ps: Ps,
    },
    Rollback {
        id: String,
        at_ps: Ps,
        expected_topology_epoch: Ps,
        activate_at_ps: Ps,
        timeout_at_ps: Ps,
    },
    Cancel {
        id: String,
        at_ps: Ps,
        transaction: String,
    },
    Participant {
        id: String,
        at_ps: Ps,
        port: String,
        mode: ParticipantMode,
    },
}
impl ManagementOperation {
    pub fn at(&self) -> u64 {
        match self {
            Self::Prepare { at_ps, .. }
            | Self::Rollback { at_ps, .. }
            | Self::Cancel { at_ps, .. }
            | Self::Participant { at_ps, .. } => at_ps.0,
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Prepare { id, .. }
            | Self::Rollback { id, .. }
            | Self::Cancel { id, .. }
            | Self::Participant { id, .. } => id,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QccConfig {
    pub architecture: String,
    pub transport: String,
    pub standard_edition: String,
    pub cycle_ps: Ps,
    pub management_latency_ps: Ps,
    pub clock_error_budget_ps: Ps,
    pub search_budget: u32,
    pub max_streams: u16,
    pub requirements: Vec<StreamRequirement>,
    pub operations: Vec<ManagementOperation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedPort {
    pub port: String,
    pub direction: usize,
    pub source: usize,
    pub destination: usize,
    pub bitrate_bps: u64,
    pub propagation_ps: u64,
    pub source_processing_ps: u64,
    pub capacity_frames: u64,
    pub class_capacity_frames: [u64; 8],
    pub class_capacity_bytes: [Option<u64>; 8],
    pub destination_processing_ps: u64,
    /// Directions with one half-duplex serializer share this calendar.
    pub calendar_domain: usize,
    pub writable_schedule: bool,
    pub clock_offset_ps: i128,
    pub clock_rate_num: u64,
    pub clock_rate_den: u64,
    pub clock_fractional: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnprovenBound {
    PhysicalNoiseOrRetry,
    PhysicalSignalEnvelope,
    PhysicalStartup,
    ClockDisciplineEnvelope,
    ClockProjection,
    ScheduleOwnership,
    ProducerEnvelope,
    IngressServiceEnvelope,
    ControlServiceEnvelope,
    Classification,
    ReplicationRoute,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeControlEnvelope {
    pub peer_direction: usize,
    pub raw_attachment: Option<usize>,
    pub admission_alignment_ps: u64,
    pub local_lldp_interval_ps: u64,
    pub peer_lldp_interval_ps: u64,
    pub local_lldp_jitter_ps: u64,
    pub peer_lldp_jitter_ps: u64,
    pub peer_verify_jitter_ps: u64,
    pub lldp_occupied_ps: u64,
    pub verify_occupied_ps: u64,
    pub respond_occupied_ps: u64,
    pub carry_in_ps: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransmissionDemand {
    pub direction: usize,
    pub priority: u8,
    pub vid: u16,
    pub mac_bytes: u64,
    /// Entire physical reservation, including coding/access and CBS recovery.
    pub occupied_ps: u64,
    pub propagation_ps: u64,
    pub destination_processing_ps: u64,
    pub merge_control: Option<MergeControlEnvelope>,
    pub start_jitter_ps: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedStream {
    pub requirement: StreamRequirement,
    pub priority: u8,
    pub mac_bytes: u64,
    pub source: usize,
    pub paths: Vec<Vec<usize>>,
    pub demands: Vec<TransmissionDemand>,
    /// An unprovable management request must not reject the executing graph.
    pub unproven: Option<UnprovenBound>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedQcc {
    pub config: QccConfig,
    pub ports: Vec<ManagedPort>,
    pub streams: Vec<PreparedStream>,
    pub stream_index: BTreeMap<String, usize>,
    pub generator_index: BTreeMap<String, usize>,
    pub reserved_priorities: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateSlot {
    pub start_ps: u64,
    pub end_ps: u64,
    pub priority: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortPlan {
    pub direction: usize,
    pub slots: Vec<GateSlot>,
    /// Another station owns the shared physical domain during these windows.
    pub closed_slots: Vec<GateSlot>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BufferReservation {
    pub direction: usize,
    pub frames: [u64; 8],
    pub bytes: [u64; 8],
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleCertificate {
    pub direction: usize,
    pub activation_ps: u64,
    pub base_ps: u64,
    pub cycle_ps: u64,
    pub entries: Vec<(u64, u8)>,
    pub clock: crate::runtime::network::clock::NodeClock,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPlan {
    pub streams: Vec<usize>,
    pub ports: Vec<PortPlan>,
    pub latency_bounds_ps: BTreeMap<String, u64>,
    pub talker_offsets_ps: BTreeMap<String, u64>,
    pub talker_latest_offsets_ps: BTreeMap<String, u64>,
    pub topology_epoch: u64,
    pub reservations: Vec<BufferReservation>,
    pub schedules: Vec<ScheduleCertificate>,
    pub raw_certificates: BTreeMap<usize, RawSourceCertificate>,
}

impl PreparedQcc {
    /// Queue ownership is local to an actual output and class. An unrelated
    /// generator elsewhere in the graph is not managed by this CNC request.
    pub fn managed_priority_mask(&self, direction: usize) -> u8 {
        self.streams
            .iter()
            .filter(|s| !matches!(s.unproven, Some(UnprovenBound::ProducerEnvelope)))
            .filter_map(|s| {
                s.demands
                    .iter()
                    .find(|d| d.direction == direction)
                    .map(|d| d.priority)
                    .or_else(|| {
                        s.paths
                            .iter()
                            .any(|p| p.contains(&direction))
                            .then_some(s.priority)
                    })
            })
            .fold(0, |mask, priority| mask | (1 << priority))
    }
}

impl PreparedQcc {
    pub fn depends_on_direction(&self, direction: usize) -> bool {
        let Some(port) = self.ports.get(direction) else {
            return false;
        };
        self.streams.iter().any(|stream| {
            stream
                .paths
                .iter()
                .flatten()
                .any(|d| self.ports[*d].calendar_domain == port.calendar_domain)
                || stream.demands.iter().any(|d| {
                    d.merge_control
                        .as_ref()
                        .is_some_and(|control| control.peer_direction == direction)
                })
        })
    }
}

/// Stable source identity and the current LLDP lifetime used by a raw guarantee.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMergeDependency {
    pub direction: usize,
    pub epoch: u64,
    pub control_generation: u64,
    pub source_mac: [u8; 6],
    pub port_id: Vec<u8>,
    pub add_frag_size: u8,
    pub capability_evidence: String,
    pub received_verify_packet: u64,
    pub received_respond_packet: u64,
    pub capability_generation: u64,
    pub capability_expires_ps: u64,
    pub enabled: bool,
    pub verified: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSourceCertificate {
    pub attachment: usize,
    pub physical: crate::runtime::ethernet::phy::medium::RawTimingCertificate,
    pub merge: Vec<RawMergeDependency>,
}
impl RawSourceCertificate {
    pub fn same_epoch(&self, other: &Self) -> bool {
        self.attachment == other.attachment
            && self.physical == other.physical
            && self.merge.len() == other.merge.len()
            && self.merge.iter().zip(&other.merge).all(|(a, b)| {
                a.direction == b.direction
                    && a.epoch == b.epoch
                    && a.source_mac == b.source_mac
                    && a.port_id == b.port_id
                    && a.add_frag_size == b.add_frag_size
                    && a.enabled == b.enabled
                    && a.verified == b.verified
            })
    }
    pub fn valid_at(&self, now: u64) -> bool {
        self.physical.bilateral_ready
            && self.physical.initial_idle_verified
            && self.merge.len() == 2
            && self.merge.iter().all(|d| {
                d.enabled
                    && d.verified
                    && !d.capability_evidence.is_empty()
                    && d.capability_expires_ps > now
            })
    }
}
