//! CAN-to-CAN gateway configuration and CAN controller composition.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Route {
    pub id: String,
    pub ingress: usize,
    pub egress: Vec<usize>,
    pub format: String,
    pub id_min: u32,
    pub id_max: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gateway {
    pub node: String,
    pub ports: Vec<usize>,
    pub routes: Vec<Route>,
    pub processing_delay_ps: u64,
    /// Frame capacity of each Gateway ingress, including processing and TX waits.
    pub rx_queue_capacity: u64,
    pub hop_limit: u32,
}

#[derive(Debug, Clone)]
pub struct PreparedGateway {
    pub gateways: Vec<Gateway>,
    pub controller_gateways: Vec<Option<usize>>,
}
