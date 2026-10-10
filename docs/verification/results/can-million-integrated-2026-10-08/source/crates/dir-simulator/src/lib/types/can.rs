//! Classical CAN configuration.
use super::common::Schedule;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub format: String,
    pub id: u32,
    pub data: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Controller {
    pub id: String,
    pub queue_capacity: u64,
    pub tx_processing_ps: u64,
    pub rx_processing_ps: u64,
    pub rx_filter: String,
    pub tx_channel_ps: u64,
    pub rx_channel_ps: u64,
}

#[derive(Debug, Clone)]
pub struct Generator {
    pub id: String,
    pub source: usize,
    pub frame: Frame,
    pub schedule: Schedule,
}

#[derive(Debug, Clone, Serialize)]
pub struct Bus {
    pub id: String,
    pub bitrate: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedCan {
    pub bus_id: String,
    pub bitrate: u64,
    pub buses: Vec<Bus>,
    pub controller_buses: Vec<usize>,
    pub controllers: Vec<Controller>,
    pub generators: Vec<Generator>,
}
