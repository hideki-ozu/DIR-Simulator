//! Explicit versioned Classical CAN digital-bus protocol input.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub id: u32,
    pub extended: bool,
    pub remote: bool,
    pub dlc: u8,
    pub data: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationRx {
    pub capacity: usize,
    pub service_ps: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Controller {
    pub id: String,
    #[serde(default)]
    pub listen_only: bool,
    #[serde(default)]
    pub tec: u16,
    #[serde(default)]
    pub rec: u16,
    #[serde(default)]
    pub automatic_recovery: bool,
    #[serde(default)]
    pub recover_at_ps: Option<u64>,
    #[serde(default)]
    pub responders: Vec<Frame>,
    #[serde(default)]
    pub application_rx: Option<ApplicationRx>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    pub request_id: String,
    pub attempt: u64,
    pub kind: String,
    /// Physical, stuffed wire bit index; ACK faults omit this field.
    #[serde(default)]
    pub bit: Option<usize>,
    /// Receiver-local corruption avoids changing the transmitter's own sample.
    #[serde(default)]
    pub receiver: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overload {
    pub request_id: String,
    /// Number of consecutive overload frames: one or two.
    pub count: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cancellation {
    pub request_id: String,
    pub at_ps: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub bitrate: u64,
    pub controllers: Vec<Controller>,
    #[serde(default)]
    pub faults: Vec<Fault>,
    #[serde(default)]
    pub overloads: Vec<Overload>,
    #[serde(default)]
    pub cancellations: Vec<Cancellation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Schedule {
    #[serde(rename = "explicit.v1")]
    Explicit { times_ps: Vec<u64> },
    #[serde(rename = "uniform.splitmix64.v1")]
    Uniform {
        seed: u64,
        start_ps: u64,
        count: u64,
        min_ps: u64,
        max_ps: u64,
    },
    #[serde(rename = "bernoulli.splitmix64.v1")]
    Bernoulli {
        seed: u64,
        start_ps: u64,
        count: u64,
        tick_ps: u64,
        numerator: u64,
        denominator: u64,
    },
    #[serde(rename = "geometric.splitmix64.v1")]
    Geometric {
        seed: u64,
        start_ps: u64,
        count: u64,
        tick_ps: u64,
        numerator: u64,
        denominator: u64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    pub id: String,
    pub controller: String,
    pub frame: Frame,
    pub schedule: Schedule,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub schema_version: u32,
    pub generators: Vec<Generator>,
}

/// Explicit ideal-digital compound bus contract; independent of hardware controller modes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fraction {
    pub num: u64,
    pub den: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "protocol", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProtocolFrame {
    Cc {
        id: u32,
        extended: bool,
        remote: bool,
        dlc: u8,
        data: Vec<u8>,
    },
    IsoFd {
        id: u32,
        extended: bool,
        brs: bool,
        data: Vec<u8>,
    },
}
impl ProtocolFrame {
    pub fn id(&self) -> u32 {
        match self {
            Self::Cc { id, .. } | Self::IsoFd { id, .. } => *id,
        }
    }
    pub fn extended(&self) -> bool {
        match self {
            Self::Cc { extended, .. } | Self::IsoFd { extended, .. } => *extended,
        }
    }
    pub fn remote(&self) -> bool {
        matches!(self, Self::Cc { remote: true, .. })
    }
    pub fn data(&self) -> &[u8] {
        match self {
            Self::Cc { data, .. } | Self::IsoFd { data, .. } => data,
        }
    }
    pub fn cc(&self) -> Option<Frame> {
        match self {
            Self::Cc {
                id,
                extended,
                remote,
                dlc,
                data,
            } => Some(Frame {
                id: *id,
                extended: *extended,
                remote: *remote,
                dlc: *dlc,
                data: data.clone(),
            }),
            _ => None,
        }
    }
}
fn pending_default() -> usize {
    64
}
fn pxhd_default() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolController {
    pub id: String,
    pub capability: String,
    #[serde(default)]
    pub tec: u16,
    #[serde(default)]
    pub rec: u16,
    #[serde(default)]
    pub automatic_recovery: bool,
    #[serde(default)]
    pub recover_at_ps: Option<u64>,
    #[serde(default = "pending_default")]
    pub pending_capacity: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolBus {
    pub id: String,
    pub nominal_bps: u64,
    pub data_bps: u64,
    pub nominal_sample: Fraction,
    pub data_sample: Fraction,
    pub controllers: Vec<ProtocolController>,
    #[serde(default = "pxhd_default")]
    pub pxhd: bool,
    #[serde(default)]
    pub hardware_model: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolFault {
    pub request_id: String,
    pub attempt: u64,
    pub kind: String,
    /// Index in the selected controller's local frame samples.
    #[serde(default)]
    pub bit: Option<usize>,
    #[serde(default)]
    pub receiver: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayPort {
    pub bus: String,
    pub controller: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRule {
    pub id: String,
    pub source_bus: String,
    pub id_mask: u32,
    pub id_value: u32,
    #[serde(default)]
    pub extended: Option<bool>,
    #[serde(default)]
    pub remote: Option<bool>,
    pub destination_bus: String,
    pub controller: String,
    pub protocol: String,
    #[serde(default)]
    pub rewrite_id: Option<u32>,
    #[serde(default)]
    pub rewrite_extended: Option<bool>,
    #[serde(default)]
    pub brs: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolGateway {
    pub id: String,
    pub ports: Vec<GatewayPort>,
    pub ingress_capacity: usize,
    pub service_ps: u64,
    pub version: u64,
    pub rules: Vec<RouteRule>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteUpdate {
    pub gateway: String,
    pub at_ps: u64,
    pub version: u64,
    pub rules: Vec<RouteRule>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompoundConfig {
    pub schema_version: u32,
    pub standard_edition: String,
    pub execution_contract: String,
    pub buses: Vec<ProtocolBus>,
    #[serde(default)]
    pub gateways: Vec<ProtocolGateway>,
    #[serde(default)]
    pub route_updates: Vec<RouteUpdate>,
    #[serde(default)]
    pub faults: Vec<ProtocolFault>,
    #[serde(default)]
    pub cancellations: Vec<Cancellation>,
    pub max_gateway_hops: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolGenerator {
    pub id: String,
    pub bus: String,
    pub controller: String,
    pub frame: ProtocolFrame,
    pub schedule: Schedule,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompoundWorkload {
    pub schema_version: u32,
    pub generators: Vec<ProtocolGenerator>,
}
