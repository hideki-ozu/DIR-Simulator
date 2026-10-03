//! Direct CSV projection of typed common records.
use super::aggregate::{MetricValue, Record};
use super::json::number_wire;

const HEADER: &str = "schema_version,run_id,seq,event_seq,effect_seq,target,metric,unit,value_kind,value,time_ps,start_ps,end_ps,request_id,receiver,reason,sample_count\n";
fn decimal<T: ToString>(value: Option<T>) -> String {
    value.map(|n| n.to_string()).unwrap_or_default()
}

pub(super) fn encode(rows: &[Record], run_id: &str, version: u32) -> Vec<u8> {
    fn cell(s: String) -> String {
        if s.contains([',', '"', '\r', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    }
    let mut output = HEADER.to_string();
    for row in rows {
        let value = match row.value {
            MetricValue::Integer(n) => n.to_string(),
            MetricValue::Number(n) => number_wire(n),
            MetricValue::Null => String::new(),
        };
        let cells = [
            version.to_string(),
            run_id.into(),
            row.seq.to_string(),
            decimal(row.event_seq),
            decimal(row.effect_seq),
            row.target.clone(),
            row.metric.clone(),
            row.unit.into(),
            row.value_kind.into(),
            value,
            decimal(row.time_ps),
            decimal(row.start_ps),
            decimal(row.end_ps),
            row.request_id.clone().unwrap_or_default(),
            row.receiver.clone().unwrap_or_default(),
            row.reason.clone().unwrap_or_default(),
            decimal(row.sample_count),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, text)| if i < 2 { text } else { cell(text) })
        .collect::<Vec<_>>();
        output.push_str(&cells.join(","));
        output.push('\n');
    }
    output.into_bytes()
}
