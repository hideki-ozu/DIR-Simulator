//! Derived resource bindings. Existing protocol and queue owners retain state.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Duplex {
    Full,
    Half,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhysicalOwner {
    IdealPointToPoint,
    ConfiguredMedia { link: usize },
    IdealShared { segment: usize },
    Sampled { attachment: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacOwner {
    Ordinary,
    ProjectPreemption,
    Clause99,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampSource {
    IdealMac,
    ConfiguredMedia,
    SampledMacMarkers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockDiscipline {
    Identity,
    Static,
    Scripted,
    Gptp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleWriter {
    External,
    Qcc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalDomain {
    pub id: String,
    pub owner: PhysicalOwner,
    pub duplex: Duplex,
    pub point_to_point: bool,
    pub mac_endpoints: usize,
    pub waveform_endpoints: usize,
    pub timestamps: TimestampSource,
    pub retry_owned_by_medium: bool,
    /// Complete ordinary MAC frames and raw Clause99 packets are distinct
    /// transport services; declaring a Merge MAC does not add the latter.
    pub raw_merge_transport: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortBinding {
    pub direction: usize,
    pub physical_domain: usize,
    pub physical_endpoint: Option<String>,
    pub mac: MacOwner,
    pub bitrate_bps: u64,
    pub clock_node: usize,
    pub discipline: ClockDiscipline,
    pub shaper: Option<usize>,
    pub schedule_writer: Option<ScheduleWriter>,
    pub forwarding_component: Option<String>,
    pub protocol_adapters: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedBindings {
    /// Indexed by the existing Ethernet direction identity.
    pub ports: Vec<PortBinding>,
    pub domains: Vec<PhysicalDomain>,
}
