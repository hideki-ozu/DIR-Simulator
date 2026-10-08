//! Native TSN observations. JSON data uses canonical decimal strings throughout.
use crate::types::ethernet::tsn::{Credit, CreditMode};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct GateRecord {
    pub time_ps: u64,
    pub port: String,
    pub schedule_id: Option<String>,
    pub generation: u64,
    pub open_priorities: Vec<u8>,
    pub next_boundary_ps: Option<u128>,
    pub cause: String,
}
#[derive(Debug, Clone)]
pub struct CreditRecord {
    pub time_ps: u64,
    pub port: String,
    pub priority: u8,
    pub credit: Credit,
    pub slope: Credit,
    pub mode: CreditMode,
    pub cause: String,
}
#[derive(Debug, Clone)]
pub struct PolicingRecord {
    pub time_ps: u64,
    pub stream_id: Option<String>,
    pub reception_id: String,
    pub ingress: String,
    pub mac_bytes: u64,
    pub verdict: String,
    pub reason: String,
    pub color: Option<String>,
    pub committed_before: Option<u128>,
    pub committed_after: Option<u128>,
    pub peak_before: Option<u128>,
    pub peak_after: Option<u128>,
    pub consumed_bits: u128,
}
#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub time_ps: u64,
    pub port: String,
    pub transfer_id: Option<String>,
    pub priority: Option<u8>,
    pub state: String,
    pub policy_epoch: u64,
    pub schedule_generation: u64,
    pub next_wake_ps: Option<u128>,
    pub reason: String,
}
#[derive(Debug, Clone)]
pub enum TsnRecord {
    Gate(GateRecord),
    Credit(CreditRecord),
    Policing(PolicingRecord),
    Decision(DecisionRecord),
}
fn decimal<T: ToString>(n: Option<T>) -> Value {
    n.map(|v| Value::String(v.to_string()))
        .unwrap_or(Value::Null)
}
impl TsnRecord {
    pub fn schema(&self) -> &'static str {
        match self {
            Self::Gate(_) => "ethernet.tsn.gate",
            Self::Credit(_) => "ethernet.tsn.credit",
            Self::Policing(_) => "ethernet.tsn.policing",
            Self::Decision(_) => "ethernet.tsn.decision",
        }
    }
    pub fn time_ps(&self) -> u64 {
        match self {
            Self::Gate(r) => r.time_ps,
            Self::Credit(r) => r.time_ps,
            Self::Policing(r) => r.time_ps,
            Self::Decision(r) => r.time_ps,
        }
    }
    /// Parent assigns the sequence after accepting the complete callback effects.
    pub fn data(&self, effect_seq: u64) -> Value {
        let mut v = match self {
            Self::Gate(r) => {
                json!({"port":r.port,"schedule_id":r.schedule_id,"generation":r.generation.to_string(),"open_priorities":r.open_priorities,"next_boundary_ps":decimal(r.next_boundary_ps),"cause":r.cause})
            }
            Self::Credit(r) => {
                json!({"port":r.port,"priority":r.priority.to_string(),"sign":if r.credit.negative {"negative"} else {"positive"},"magnitude":r.credit.magnitude.to_string(),"scale":"1000000000000","slope_bps":format!("{}{}",if r.slope.negative {"-"} else {""},r.slope.magnitude),"cause":r.cause})
            }
            Self::Policing(r) => {
                json!({"stream_id":r.stream_id,"reception_id":r.reception_id,"ingress":r.ingress,"mac_bytes":r.mac_bytes.to_string(),"verdict":if r.verdict=="drop" {&r.reason} else {&r.verdict},"color":r.color,"committed_before":decimal(r.committed_before),"committed_after":decimal(r.committed_after),"peak_before":decimal(r.peak_before),"peak_after":decimal(r.peak_after),"consumed_bits":r.consumed_bits.to_string()})
            }
            Self::Decision(r) => {
                json!({"port":r.port,"transfer_id":r.transfer_id,"priority":r.priority.map(|priority|priority.to_string()),"state":r.state,"policy_epoch":r.policy_epoch.to_string(),"schedule_generation":r.schedule_generation.to_string(),"next_wake_ps":decimal(r.next_wake_ps),"reason":r.reason})
            }
        };
        v["effect_seq"] = json!(effect_seq.to_string());
        v
    }
}
#[derive(Debug, Clone)]
pub struct CreditSnapshot {
    pub priority: u8,
    pub credit: Credit,
    pub last_ps: u64,
    pub mode: CreditMode,
    pub backlog: bool,
    pub sending: bool,
}
#[derive(Debug, Clone)]
pub struct PortTsnSnapshot {
    pub port: String,
    pub schedule_id: Option<String>,
    pub schedule_generation: u64,
    pub wake_generation: u64,
    pub next_wake_ps: Option<u128>,
    pub credits: Vec<CreditSnapshot>,
}
#[derive(Debug, Clone)]
pub struct MeterSnapshot {
    pub stream_id: String,
    pub committed: u128,
    pub peak: u128,
    pub last_evaluated_ps: u64,
}
#[derive(Debug, Clone)]
pub struct TsnSnapshot {
    pub ports: Vec<PortTsnSnapshot>,
    pub meters: Vec<MeterSnapshot>,
    pub update_cursor: usize,
}

pub const SCHEMAS: [(&str, u32); 4] = [
    ("ethernet.tsn.gate", 1),
    ("ethernet.tsn.credit", 1),
    ("ethernet.tsn.policing", 1),
    ("ethernet.tsn.decision", 1),
];
impl TsnSnapshot {
    pub fn data(&self) -> Value {
        json!({"ports":self.ports.iter().map(|p|json!({"port":p.port,"schedule_id":p.schedule_id,
            "schedule_generation":p.schedule_generation.to_string(),"wake_generation":p.wake_generation.to_string(),
            "next_wake_ps":decimal(p.next_wake_ps),"credits":p.credits.iter().map(|c|json!({"priority":c.priority,
                "sign":if c.credit.negative {"-"} else {"+"},"magnitude":c.credit.magnitude.to_string(),"scale":"1000000000000",
                "last_ps":c.last_ps.to_string(),"mode":c.mode,"backlog":c.backlog,"sending":c.sending})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "meters":self.meters.iter().map(|m|json!({"stream_id":m.stream_id,"committed":m.committed.to_string(),
                "peak":m.peak.to_string(),"last_evaluated_ps":m.last_evaluated_ps.to_string()})).collect::<Vec<_>>(),
            "update_cursor":self.update_cursor.to_string()})
    }
}
