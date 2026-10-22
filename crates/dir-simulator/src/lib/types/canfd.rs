//! Validated CAN FD topology and versioned frame representations.
use super::Controller;

#[derive(Debug, Clone)]
pub struct IsoCanFdWire {
    pub esi: bool,
    pub bits: Vec<u8>,
    pub crc_width: u8,
    pub crc: u32,
    pub stuff_count: u8,
    pub stuff_count_code: u8,
    pub dynamic_stuff_positions: Vec<usize>,
    pub fixed_stuff_positions: Vec<usize>,
    pub brs_index: usize,
    pub crc_delimiter_index: usize,
}

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
    pub iso: Option<IsoCanFdWire>,
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
    pub profile: String,
    pub nominal_rate: u64,
    pub data_rate: u64,
    pub controllers: Vec<Controller>,
    pub generators: Vec<CanFdGenerator>,
}
