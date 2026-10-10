//! Committed transaction journal and exact queue/busy measurements.
use std::collections::BTreeMap;
#[derive(Debug, Clone)]
pub struct ActivePlan {
    pub resource: String,
    pub hop: u64,
    pub start_ps: u64,
    pub planned_end_ps: u64,
    pub from: String,
    pub to: Option<String>,
    pub downstream: Option<String>,
    pub response: Option<String>,
    pub address_end_ps: Option<u64>,
    pub nominal_data_end_ps: Option<u64>,
    pub data_end_ps: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: String,
    pub generator: usize,
    pub time_ps: u64,
    pub generated_ps: u64,
    pub status: String,
    pub target: Option<String>,
    pub start_ps: Option<u64>,
    pub completed_ps: Option<u64>,
    pub response: Option<String>,
    pub drop_reason: Option<String>,
    pub active_plan: Option<ActivePlan>,
    pub hop: u64,
}
#[derive(Debug, Clone)]
pub struct Transfer {
    pub request: usize,
    pub plan: ActivePlan,
    pub end_ps: u64,
}
#[derive(Debug, Clone, Default)]
pub struct Gauge {
    pub changes: Vec<(u64, u64)>,
    pub maximum: u64,
}
impl Gauge {
    pub fn set(&mut self, t: u64, n: u64) {
        self.maximum = self.maximum.max(n);
        self.changes.push((t, n));
    }
    pub fn area(&self, h: u64) -> u128 {
        let mut last = 0;
        let mut n = 0;
        let mut sum = 0;
        for &(t, v) in &self.changes {
            if t > h {
                break;
            }
            sum += (t - last) as u128 * n as u128;
            last = t;
            n = v;
        }
        sum + (h - last) as u128 * n as u128
    }
}
#[derive(Debug, Clone, Default)]
pub struct SocSnapshot {
    pub transactions: Vec<Transaction>,
    pub transfers: Vec<Transfer>,
    pub queues: BTreeMap<String, Gauge>,
    pub busy: BTreeMap<String, Vec<(u64, u64)>>,
}
