//! Prepared private coordinator payloads for the composed network profiles.
use super::ethernet::{PreparedEthernet, dynamic::PreparedDynamicEthernet};
use serde_json::Value;
#[path = "network/bindings.rs"]
pub mod bindings;
#[path = "network/configuration.rs"]
pub mod configuration;
#[path = "network/gptp.rs"]
pub mod gptp;
#[path = "network/mac_control.rs"]
pub mod mac_control;
#[path = "network/mac_merge.rs"]
pub mod mac_merge;

#[derive(Debug, Clone)]
pub struct PreparedNetwork {
    pub ethernet: PreparedEthernet,
    pub dynamic: Option<PreparedDynamicEthernet>,
    pub tsn: Option<super::ethernet::tsn::PreparedTsn>,
    pub bridge: Option<super::can_ethernet::PreparedCanEthernet>,
    pub extensions: Option<PreparedNetworkExtensions>,
    pub clocks: Option<PreparedClocks>,
    pub gptp: Option<gptp::PreparedGptp>,
    pub phy_attachments: Vec<PreparedPhyAttachment>,
    pub shared_lans: Vec<PreparedSharedLan>,
    pub(crate) provider_l2: Option<crate::runtime::network::provider_l2::ProviderTopology>,
    pub(crate) multicast_active: Option<crate::runtime::network::multicast_active::Config>,
    pub(crate) qcc: Option<configuration::PreparedQcc>,
    pub wire: Option<super::wire::PreparedWire>,
    pub mac_control: Option<mac_control::PreparedMacControl>,
    /// Final resource claims for composed profiles; ordinary legacy profiles
    /// retain their existing dispatch until they enter composition.
    pub bindings: Option<bindings::PreparedBindings>,
    pub config: Value,
    pub workload: Value,
}

#[derive(Debug, Clone)]
pub struct PreparedSharedLan {
    pub id: String,
    /// Existing output-direction identities, each physically connected to one NED hub.
    pub members: Vec<usize>,
    pub bitrate_bps: u64,
    pub delay_ps: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedPhyAttachment {
    pub config: crate::runtime::ethernet::phy::attachment::AttachmentConfig,
    pub directions: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct PreparedClocks {
    /// One initial clock per Ethernet device, in device order.
    pub initial: Vec<PreparedNodeClock>,
    pub adjustments: Vec<PreparedClockAdjustment>,
}

#[derive(Debug, Clone)]
pub struct PreparedNodeClock {
    pub offset_ps: i128,
    pub rate_num: u64,
    pub rate_den: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedClockAdjustment {
    pub node: usize,
    pub at_ps: u64,
    pub action: PreparedClockAction,
}

#[derive(Debug, Clone, Copy)]
pub enum PreparedClockAction {
    StepBy { offset_ps: i128 },
    Slew { rate_num: u64, rate_den: u64 },
}

#[derive(Debug, Clone)]
pub struct NetworkLimits {
    pub max_pending_branches: u32,
    pub max_control_events: u32,
}
#[derive(Debug, Clone)]
pub struct PreparedNetworkExtensions {
    pub limits: NetworkLimits,
    pub replication: Option<PreparedReplication>,
    pub preemption: Option<PreparedPreemption>,
    pub mac_merge: Option<mac_merge::PreparedMacMerge>,
}
#[derive(Debug, Clone)]
pub struct PreparedReplication {
    pub streams: Vec<ReplicationStream>,
    pub generator_stream: std::collections::BTreeMap<String, usize>,
    pub shared_ports: Vec<String>,
}
/// Immutable ingress identity. Gateway selectors never become synthetic generators.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplicationSource {
    Generator {
        id: String,
    },
    GatewayEgress {
        gateway_id: String,
        rule_id: String,
        egress_index: usize,
    },
}
impl PreparedNetwork {
    pub fn has_phy_mac_service(&self) -> bool {
        self.phy_attachments.iter().any(|attachment| {
            attachment.config.medium.gigabit_raw_merge
                || self.wire.is_some()
                || self.mac_control.is_some()
                || self.qcc.is_some()
                || attachment.directions.iter().any(|&i| {
                    let port = &self.ethernet.directions[i].from_port;
                    self.tsn.as_ref().is_some_and(|tsn| {
                        tsn.outputs.iter().any(|o| {
                            o.port == *port
                                && (o.tas.is_some() || o.cbs.iter().any(Option::is_some))
                        }) || tsn.updates.iter().any(|u| u.port == *port)
                    })
                })
        })
    }
    pub fn has_gateway_replication(&self) -> bool {
        self.extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .is_some_and(|r| {
                r.streams
                    .iter()
                    .any(|s| matches!(s.source, ReplicationSource::GatewayEgress { .. }))
            })
    }
}
#[derive(Debug, Clone)]
pub struct ReplicationStream {
    pub stream_id: String,
    pub source_generator: String,
    pub source: ReplicationSource,
    pub source_device: usize,
    pub source_port: Option<String>,
    pub replicate_at: String,
    pub eliminate_at: String,
    pub history_capacity: usize,
    pub wire_format: ReplicationWireFormat,
    pub recovery_timeout_ps: Option<u64>,
    pub recovery_algorithm: ReplicationRecoveryAlgorithm,
    pub initial_history_words: Vec<u64>,
    pub paths: Vec<ReplicationPath>,
    pub placement: ReplicationPlacement,
}
#[derive(Debug, Clone)]
pub enum ReplicationPlacement {
    LegacyEndpoint,
    BridgePair(PreparedBridgePair),
}
#[derive(Debug, Clone)]
pub struct PreparedBridgePair {
    pub replicate_device: usize,
    pub eliminate_device: usize,
    pub destination_device: usize,
    pub prefix: PreparedPhysicalRoute,
    pub suffix: PreparedPhysicalRoute,
}
#[derive(Debug, Clone)]
pub struct PreparedPhysicalRoute {
    pub ports: Vec<String>,
    pub directions: Vec<usize>,
    pub endpoints: Vec<(usize, usize)>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationRecoveryAlgorithm {
    ProjectHistoryV1,
    Vector8021cb2017,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationWireFormat {
    Abstract,
    Rtag8021cb2017,
}
#[derive(Debug, Clone)]
pub struct ReplicationPath {
    pub id: String,
    pub ports: Vec<String>,
    pub directions: Vec<usize>,
    pub endpoints: Vec<(usize, usize)>,
}
#[derive(Debug, Clone)]
pub struct PreparedPreemption {
    /// Same index as PreparedEthernet::directions.
    pub outputs: Vec<PreemptionOutput>,
    pub participating: Vec<bool>,
}
#[derive(Debug, Clone)]
pub struct PreemptionOutput {
    pub port: String,
    pub express_mask: u8,
    pub preemptable_mask: u8,
    pub fragment_payload_bytes: u64,
    pub fragment_overhead_bytes: u64,
    pub resume_gap_ps: u64,
}
