//! Prepared shared bus, AHB and finite-slot XY mesh models.
use std::collections::BTreeMap;
#[derive(Debug, Clone)]
pub struct Source {
    pub node: String,
    pub capacity: u64,
    pub priority: u64,
    pub router: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct Target {
    pub node: String,
    pub base: u64,
    pub size: u64,
    pub cycles: u64,
    pub errors: Vec<(u64, u64)>,
}
#[derive(Debug, Clone)]
pub struct Router {
    pub node: String,
    pub x: u64,
    pub y: u64,
}
#[derive(Debug, Clone)]
pub struct Transaction {
    pub operation: Option<String>,
    pub address: Option<u64>,
    pub bytes: u64,
    pub destination: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct Generator {
    pub id: String,
    pub source: usize,
    pub times: Vec<u64>,
    pub transaction: Transaction,
}
#[derive(Debug, Clone)]
pub struct PreparedSoc {
    pub profile: String,
    pub clock_period: u64,
    pub bus: String,
    pub arbitration: String,
    pub bytes_per_cycle: u64,
    pub link_cycles: u64,
    pub input_capacity: u64,
    pub sources: Vec<Source>,
    pub targets: Vec<Target>,
    pub routers: Vec<Router>,
    pub generators: Vec<Generator>,
    pub nodes: BTreeMap<String, String>,
    pub edges: BTreeMap<String, String>,
}
impl PreparedSoc {
    pub fn prefix(&self) -> &str {
        if self.profile.starts_with("soc.") {
            "soc"
        } else if self.profile.starts_with("ahb.") {
            "ahb"
        } else {
            "noc"
        }
    }
}
