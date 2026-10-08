//! Immutable composite Gateway configuration and lineage shared by both media.
use super::{Frame, PreparedCan, ethernet::PreparedEthernet};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanFormat {
    Standard,
    Extended,
}
impl CanFormat {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Extended => "extended",
        }
    }
    pub fn max_id(&self) -> u32 {
        match self {
            Self::Standard => 2047,
            Self::Extended => 0x1fffffff,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirCanPacket {
    pub format: CanFormat,
    pub id: u32,
    pub data: Vec<u8>,
}
impl DirCanPacket {
    pub fn frame(&self) -> Frame {
        Frame {
            format: self.format.name().into(),
            id: self.id,
            data: self.data.iter().fold(String::new(), |mut out, b| {
                write!(out, "{b:02x}").expect("writing to String");
                out
            }),
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub enum BridgeEgress {
    Ethernet {
        source: usize,
        port: String,
        dst_mac: String,
        vid: u16,
        pcp: u8,
    },
    Can {
        source: usize,
        port: String,
        format: CanFormat,
        can_id: u32,
    },
}
impl BridgeEgress {
    pub fn port(&self) -> &str {
        match self {
            Self::Ethernet { port, .. } | Self::Can { port, .. } => port,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeRule {
    pub id: String,
    pub direction: String,
    pub ingress: String,
    pub format: CanFormat,
    pub can_id: u32,
    pub vid: Option<u16>,
    pub pcp: Option<u8>,
    pub egresses: Vec<BridgeEgress>,
}
#[derive(Debug, Clone, Serialize)]
pub struct BridgeGateway {
    pub instance: String,
    pub can_ports: Vec<usize>,
    pub ethernet_endpoint: usize,
    pub rx_capacity: u64,
    pub conversion_delay_ps: u64,
    pub max_hops: u8,
    pub rules: Vec<BridgeRule>,
}
#[derive(Debug, Clone)]
pub struct PreparedCanEthernet {
    pub can: PreparedCan,
    pub ethernet: PreparedEthernet,
    pub gateways: Vec<BridgeGateway>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLineage {
    pub origin_id: String,
    pub parent_id: String,
    pub visited_gateways: Vec<String>,
    pub branch_lineage: Vec<String>,
    pub segment_id: String,
    pub generated_ps: u64,
}
impl BridgeLineage {
    /// Native media keep their legacy record IDs; cross-media roots use a namespace.
    pub(crate) fn native(media: &str, record_id: &str, generated_ps: u64) -> Self {
        let origin_id = format!("{media}:{record_id}");
        Self {
            segment_id: origin_id.clone(),
            origin_id,
            parent_id: record_id.into(),
            visited_gateways: Vec::new(),
            branch_lineage: Vec::new(),
            generated_ps,
        }
    }
}
/// Length-prefixed components avoid delimiter collisions and array-order dependence.
pub fn lineage_id(parts: &[&str]) -> String {
    parts.iter().fold(String::new(), |mut out, s| {
        write!(out, "{}:{s}", s.len()).expect("writing to String");
        out
    })
}
