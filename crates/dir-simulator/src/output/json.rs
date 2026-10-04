//! Canonical JSON result and diagnostic rendering.
use super::{
    PROFILE,
    aggregate::{MetricValue, Record},
    metadata, model_records,
};
use crate::snapshot::Snapshot;
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn opt_d(n: Option<u64>) -> Value {
    n.map(|n| Value::String(n.to_string()))
        .unwrap_or(Value::Null)
}
pub(super) fn number_wire(n: f64) -> String {
    if n == 0.0 {
        "0.0000000000000000e+0".into()
    } else {
        let text = format!("{n:.16e}");
        let (mantissa, exponent) = text.split_once('e').expect("scientific format");
        let exponent: i32 = exponent.parse().expect("numeric exponent");
        format!("{mantissa}e{exponent:+}")
    }
}

/// Required canonical JSON differs from serde_json in control escapes and float spelling.
pub(super) fn canonical(value: &Value) -> String {
    fn string(s: &str) -> String {
        let mut result = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => result.push_str("\\\""),
                '\\' => result.push_str("\\\\"),
                c if c < '\u{20}' => result.push_str(&format!("\\u{:04x}", c as u32)),
                c => result.push(c),
            }
        }
        result.push('"');
        result
    }
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => string(s),
        Value::Number(n) if n.is_f64() => number_wire(n.as_f64().expect("f64")),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(o) => {
            let sorted: BTreeMap<_, _> = o.iter().collect();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(k, v)| format!("{}:{}", string(k), canonical(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

pub(super) fn record_value(record: &Record) -> Value {
    let value = match record.value {
        MetricValue::Integer(n) => Value::String(n.to_string()),
        MetricValue::Number(n) => json!(n),
        MetricValue::Null => Value::Null,
    };
    json!({"seq":record.seq.to_string(),"event_seq":opt_d(record.event_seq),
        "effect_seq":record.effect_seq.map(|n| n.to_string()),
        "target":record.target,"metric":record.metric,"unit":record.unit,
        "value_kind":record.value_kind,"value":value,"time_ps":opt_d(record.time_ps),
        "start_ps":opt_d(record.start_ps),"end_ps":opt_d(record.end_ps),
        "request_id":record.request_id,"receiver":record.receiver,"reason":record.reason,
        "sample_count":record.sample_count.map(|n| n.to_string())})
}

pub(super) fn result(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    run_id: &str,
    timestamp: &str,
    records: &[Record],
    summary: &[Record],
) -> Result<Value, Diagnostic> {
    let requests = model_records::requests(snapshot);
    let receivers = model_records::receivers(snapshot);
    let mut result = json!({"schema_version":1,"run_id":run_id,"metadata":metadata::build(prepared,timestamp)?,"simulation":{"termination":snapshot.common.termination,"partial":snapshot.common.partial,"start_ps":"0","end_ps":snapshot.common.end_ps.to_string(),"last_event_time_ps":opt_d(snapshot.common.last_event_time_ps),"committed_events":snapshot.common.committed_events.to_string(),"pending_events":snapshot.common.pending_events.to_string(),"records":records.iter().map(record_value).collect::<Vec<_>>(),"summary":summary.iter().map(record_value).collect::<Vec<_>>(),"requests":requests,"receivers":receivers}});
    let version = if prepared.common.profile == PROFILE {
        1
    } else {
        2
    };
    if version == 2 {
        result["schema_version"] = json!(2);
        result["simulation"]
            .as_object_mut()
            .unwrap()
            .remove("requests");
        result["simulation"]
            .as_object_mut()
            .unwrap()
            .remove("receivers");
        result["simulation"]["model_records"] = json!(model_records::records(prepared, snapshot)?);
    }
    Ok(result)
}

pub(super) fn diagnostics(snapshot: &Snapshot) -> Vec<u8> {
    let mut diagnostics = String::new();
    for (i, diagnostic) in snapshot.common.diagnostics.iter().enumerate() {
        let object = json!({"schema_version":1,"seq":i.to_string(),"severity":"error","primary":i==0,"code":diagnostic.code,"phase":diagnostic.stage,"reason":"runtime_error","message":diagnostic.message,"source":null,"line":null,"column":null,"end_line":null,"end_column":null,"target":null,"time_ps":snapshot.common.end_ps.to_string(),"event_seq":null,"details":diagnostic.details.as_ref().cloned().unwrap_or_else(|| json!({}))});
        diagnostics.push_str(&canonical(&object));
        diagnostics.push('\n');
    }
    diagnostics.into_bytes()
}
