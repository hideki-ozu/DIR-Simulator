//! Prepared inputs for the abstract ideal-clock TAS/CBS/PSFP profile.
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};

pub const Q: u128 = 1_000_000_000_000;
#[derive(Debug, Clone, Serialize)]
pub struct GateEntry {
    pub duration_ps: u64,
    pub open_mask: u8,
}
#[derive(Debug, Clone, Serialize)]
pub struct OpenRun {
    pub start_ps: u64,
    /// May extend beyond a cycle when tail and head are continuously open.
    pub end_ps: u128,
}
#[derive(Debug, Clone, Serialize)]
pub struct Schedule {
    pub id: String,
    pub base_ps: u64,
    pub cycle_ps: u64,
    pub entries: Vec<GateEntry>,
    pub prefix_ps: Vec<u64>,
    pub class_open_runs: [Vec<OpenRun>; 8],
    pub open_total_ps: [u64; 8],
}
#[derive(Debug, Clone, Serialize)]
pub struct CbsConfig {
    pub priority: u8,
    pub idle_bps: u64,
    pub link_bps: u64,
    pub hi: u128,
    pub lo: u128,
}
#[derive(Debug, Clone)]
pub struct TsnOutput {
    pub port: String,
    pub link_bps: u64,
    pub tas: Option<Arc<Schedule>>,
    pub cbs: [Option<CbsConfig>; 8],
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StreamKey {
    pub ingress: String,
    pub dst_mac: String,
    pub vid: u16,
    pub priority: u8,
}
#[derive(Debug, Clone, Serialize)]
pub struct MeterConfig {
    pub committed_rate_bps: u64,
    pub peak_rate_bps: u64,
    pub committed_cap: u128,
    pub peak_cap: u128,
    pub yellow_drop: bool,
}
#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub id: String,
    pub key: StreamKey,
    pub max_sdu_bytes: u64,
    pub gate: Option<Arc<Schedule>>,
    pub meter: Option<MeterConfig>,
}
#[derive(Debug, Clone)]
pub struct GclUpdate {
    pub id: String,
    pub submitted_at_ps: u64,
    pub effective_at_ps: u64,
    pub port: String,
    pub schedule: Arc<Schedule>,
}
#[derive(Debug, Clone)]
pub struct PreparedTsn {
    pub outputs: Vec<TsnOutput>,
    pub port_index: BTreeMap<String, usize>,
    pub streams: Vec<StreamConfig>,
    pub stream_index: BTreeMap<StreamKey, usize>,
    pub updates: Vec<GclUpdate>,
}
/// Native numerator, never converted through i128 or floating point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Credit {
    pub negative: bool,
    pub magnitude: u128,
}
impl Credit {
    pub fn new(negative: bool, magnitude: u128) -> Self {
        Self {
            negative: negative && magnitude != 0,
            magnitude,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditMode {
    Sending,
    Frozen,
    Accumulating,
    Recovering,
    Zero,
}

impl PreparedTsn {
    /// Normalized TSN metadata; parent supplies the separately prepared dynamic base.
    pub fn metadata(&self) -> serde_json::Value {
        use serde_json::{Value, json};
        let schedule = |s: &Schedule, stream: bool| {
            let mut v = json!({"base_time_ps":s.base_ps.to_string(),"cycle_time_ps":s.cycle_ps.to_string(),
                "entries":s.entries.iter().map(|e|if stream {json!({"duration_ps":e.duration_ps.to_string(),"open":e.open_mask!=0})}
                else {json!({"duration_ps":e.duration_ps.to_string(),"open_priorities":(0..8).filter(|c|e.open_mask&(1<<c)!=0).collect::<Vec<_>>()})}).collect::<Vec<_>>()});
            if !stream {
                v["id"] = json!(s.id);
            }
            v
        };
        json!({"clock":"ideal_shared","abstract_rules":["ifg-inclusive-guard","release-based-credit","independent-two-bucket"],
            "initial_credit":"0","initial_tokens":"full",
            "outputs":self.outputs.iter().map(|o|json!({"port":o.port,"tas":o.tas.as_ref().map_or(Value::Null,|s|schedule(s,false)),
                "cbs":o.cbs.iter().flatten().map(|c|json!({"priority":c.priority,"idle_slope_bps":c.idle_bps.to_string(),
                    "hi_credit_bits":(c.hi/Q).to_string(),"lo_credit_bits":(c.lo/Q).to_string()})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "streams":self.streams.iter().map(|s|json!({"id":s.id,"ingress":s.key.ingress,"dst_mac":s.key.dst_mac,"vid":s.key.vid,"priority":s.key.priority,
                "max_sdu_bytes":s.max_sdu_bytes.to_string(),"gate":s.gate.as_ref().map_or(Value::Null,|g|schedule(g,true)),
                "meter":s.meter.as_ref().map(|m|json!({"committed_rate_bps":m.committed_rate_bps.to_string(),"peak_rate_bps":m.peak_rate_bps.to_string(),
                    "committed_burst_bytes":(m.committed_cap/(8*Q)).to_string(),"peak_burst_bytes":(m.peak_cap/(8*Q)).to_string(),
                    "yellow_action":if m.yellow_drop {"drop"} else {"pass"}}))})).collect::<Vec<_>>(),
            "gcl_updates":self.updates.iter().map(|u|json!({"id":u.id,"submitted_at_ps":u.submitted_at_ps.to_string(),"effective_at_ps":u.effective_at_ps.to_string(),
                "port":u.port,"schedule":schedule(&u.schedule,false)})).collect::<Vec<_>>()})
    }
}
