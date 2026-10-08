//! Prepared transaction resources and immutable workload requests.
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ddr,
    Sram,
    Shared,
    Dma,
    Mailbox,
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ddr => "ddr",
            Self::Sram => "sram",
            Self::Shared => "shared",
            Self::Dma => "dma",
            Self::Mailbox => "mailbox",
        }
    }
}
#[derive(Debug, Clone)]
pub struct Resource {
    pub node: String,
    pub kind: Kind,
    pub queue_capacity: usize,
    pub size: usize,
    pub initial: Vec<u8>,
    pub faults: Vec<(u64, u64)>,
    pub banks: usize,
    pub row_bytes: u64,
    pub width_bytes: u64,
    pub ports: usize,
    pub slots: usize,
    pub payload_bytes: usize,
    pub capacity: usize,
    pub chunk_bytes: u64,
    pub times: BTreeMap<String, u64>,
    pub producers: Vec<String>,
    pub consumers: Vec<String>,
}
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub op: String,
    pub actor: Option<String>,
    pub address: Option<u64>,
    pub length: Option<u64>,
    pub bytes: Option<Vec<u8>>,
    pub src: Option<usize>,
    pub dst: Option<usize>,
    pub src_address: Option<u64>,
    pub dst_address: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct Generator {
    pub id: String,
    pub node: usize,
    pub times: Vec<u64>,
    pub request: Request,
}
#[derive(Debug, Clone, Default)]
pub struct PreparedMemoryIpc {
    pub placements: BTreeMap<String, Kind>,
    pub resources: Vec<Resource>,
    pub generators: Vec<Generator>,
}
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}
