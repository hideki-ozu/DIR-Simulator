//! Validated Ethernet L2 configuration and immutable serialized frame bytes.
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Serialize)]
pub struct EthernetDevice {
    pub id: String,
    pub kind: String,
    pub mac: Option<String>,
    pub queue_capacity: u64,
    pub tx_processing_delay_ps: u64,
    pub rx_processing_delay_ps: u64,
    pub forward_delay_ps: u64,
    pub fdb: BTreeMap<String, String>,
    pub vlan_fdb: BTreeMap<(u16, String), String>,
    pub multicast: BTreeMap<(u16, String), Vec<String>>,
    pub subscriptions: BTreeSet<(u16, String)>,
    pub unknown_multicast: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct EthernetDirection {
    pub channel_id: String,
    pub from_port: String,
    pub to_port: String,
    pub source: usize,
    pub destination: usize,
    pub bitrate_bps: u64,
    pub delay_ps: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EthernetVlanTag {
    pub vid: u16,
    pub pcp: u8,
    pub dei: u8,
}
#[derive(Debug, Clone, Serialize)]
pub struct EthernetPortPolicy {
    pub port: String,
    pub ingress: String,
    pub device: usize,
    pub pvid: u16,
    pub admit: String,
    pub default_priority: u8,
    pub vlans: BTreeMap<u16, bool>,
}
#[derive(Debug, Clone, Serialize)]
pub struct EthernetWireFrame {
    pub src_mac: String,
    pub dst_mac: String,
    pub ether_type: u16,
    pub data_hex: String,
    pub tag: Option<EthernetVlanTag>,
    pub pad_bytes: u64,
    pub mac_bytes: u64,
    pub fcs_hex: String,
    pub mac_hex: String,
}
#[derive(Debug, Clone)]
pub struct EthernetGenerator {
    pub id: String,
    pub source: usize,
    pub source_vlan_id: Option<u16>,
    pub times_ps: Vec<u64>,
    pub frame: EthernetWireFrame,
    pub schedule: Option<EthernetSchedule>,
    pub flow_id: Option<String>,
    pub priority: u8,
    pub deadline_ps: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct PreparedEthernet {
    pub devices: Vec<EthernetDevice>,
    pub directions: Vec<EthernetDirection>,
    pub generators: Vec<EthernetGenerator>,
    pub outputs: Vec<EthernetOutputConfig>,
    pub port_policies: Vec<EthernetPortPolicy>,
    pub media: Option<EthernetMediaConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EthernetQueueConfig {
    pub priority: u8,
    pub capacity_frames: u64,
    pub capacity_bytes: Option<u64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct EthernetOutputConfig {
    pub port: String,
    pub scheduler: String,
    pub queues: Vec<EthernetQueueConfig>,
}
#[derive(Debug, Clone)]
pub enum EthernetSchedule {
    Periodic {
        start_ps: u64,
        phase_ps: u64,
        period_ps: u64,
        end_ps: Option<u64>,
        count: Option<u64>,
    },
    Burst {
        start_ps: u64,
        period_ps: u64,
        burst_count: Option<u64>,
        frames_per_burst: u64,
        spacing_ps: u64,
        end_ps: Option<u64>,
    },
}
impl EthernetGenerator {
    pub fn time(&self, ordinal: u64) -> Option<u128> {
        match &self.schedule {
            None => usize::try_from(ordinal)
                .ok()
                .and_then(|ordinal| self.times_ps.get(ordinal))
                .map(|time| *time as u128),
            Some(EthernetSchedule::Periodic {
                start_ps,
                phase_ps,
                period_ps,
                end_ps,
                count,
            }) => {
                if count.is_some_and(|count| ordinal >= count) {
                    return None;
                }
                let time =
                    *start_ps as u128 + *phase_ps as u128 + ordinal as u128 * *period_ps as u128;
                (!end_ps.is_some_and(|end| time >= end as u128)).then_some(time)
            }
            Some(EthernetSchedule::Burst {
                start_ps,
                period_ps,
                burst_count,
                frames_per_burst,
                spacing_ps,
                end_ps,
            }) => {
                let burst = ordinal / *frames_per_burst;
                if burst_count.is_some_and(|count| burst >= count) {
                    return None;
                }
                let time = *start_ps as u128
                    + burst as u128 * *period_ps as u128
                    + (ordinal % *frames_per_burst) as u128 * *spacing_ps as u128;
                (!end_ps.is_some_and(|end| time >= end as u128)).then_some(time)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EthernetPhyEnd {
    pub role: String,
    pub tx_latency_ps: u64,
    pub rx_latency_ps: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct EthernetPhysicalLink {
    pub id: String,
    pub a: String,
    pub b: String,
    pub phy_mode: String,
    pub duplex: String,
    pub a_phy: EthernetPhyEnd,
    pub b_phy: EthernetPhyEnd,
    pub directions: [usize; 2],
}
#[derive(Debug, Clone, Serialize)]
pub struct EthernetMediaConfig {
    pub seed: u64,
    pub physical_links: Vec<EthernetPhysicalLink>,
}
