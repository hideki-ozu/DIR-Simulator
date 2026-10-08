//! Immutable externally supplied CAN FD phase lengths and validated topology.
use super::Controller;

#[derive(Debug, Clone)]
pub struct CanFdFrame {
    pub format: String,
    pub id: u32,
    pub data: String,
    pub dlc: u8,
    pub brs: bool,
    pub nominal_bits: u64,
    pub data_bits: u64,
    pub evidence: String,
    pub binding_sha256: String,
    pub nominal_rate: u64,
    pub data_rate: u64,
    pub duration_ps: u64,
    pub occupancy_ps: u64,
    pub arbitration: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct CanFdGenerator {
    pub id: String,
    pub source: usize,
    pub times_ps: Vec<u64>,
    pub frame: CanFdFrame,
}
#[derive(Debug, Clone)]
pub struct PreparedCanFd {
    pub bus_id: String,
    pub nominal_rate: u64,
    pub data_rate: u64,
    pub controllers: Vec<Controller>,
    pub generators: Vec<CanFdGenerator>,
}
