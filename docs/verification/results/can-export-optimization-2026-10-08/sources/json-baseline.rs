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

fn write_bytes(writer: &mut dyn std::io::Write, bytes: &[u8]) -> Result<(), Diagnostic> {
    writer
        .write_all(bytes)
        .map_err(|e| Diagnostic::output(format!("Cannot write result JSON: {e}")))
}
fn write_value(writer: &mut dyn std::io::Write, value: &Value) -> Result<(), Diagnostic> {
    write_bytes(writer, canonical(value).as_bytes())
}
fn write_array(
    writer: &mut dyn std::io::Write,
    rows: impl IntoIterator<Item = Value>,
) -> Result<(), Diagnostic> {
    write_bytes(writer, b"[")?;
    for (i, row) in rows.into_iter().enumerate() {
        if i > 0 {
            write_bytes(writer, b",")?;
        }
        write_value(writer, &row)?;
    }
    write_bytes(writer, b"]")
}

fn write_array_fallible(
    writer: &mut dyn std::io::Write,
    rows: impl IntoIterator<Item = Result<Value, Diagnostic>>,
) -> Result<(), Diagnostic> {
    write_bytes(writer, b"[")?;
    for (i, row) in rows.into_iter().enumerate() {
        if i > 0 {
            write_bytes(writer, b",")?;
        }
        write_value(writer, &row?)?;
    }
    write_bytes(writer, b"]")
}

pub(super) fn write_stream_result(
    writer: &mut dyn std::io::Write,
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    run_id: &str,
    timestamp: &str,
    stream: &super::stream::Prepared,
) -> Result<(), Diagnostic> {
    let version = if prepared.common.profile == PROFILE {
        1
    } else {
        2
    };
    let metadata = metadata::build(prepared, timestamp)?;
    write_bytes(writer, b"{\"metadata\":")?;
    write_value(writer, &metadata)?;
    write_bytes(writer, b",\"run_id\":")?;
    write_value(writer, &json!(run_id))?;
    write_bytes(
        writer,
        format!(",\"schema_version\":{version},\"simulation\":{{\"committed_events\":").as_bytes(),
    )?;
    write_value(writer, &json!(snapshot.common.committed_events.to_string()))?;
    write_bytes(writer, b",\"end_ps\":")?;
    write_value(writer, &json!(snapshot.common.end_ps.to_string()))?;
    write_bytes(writer, b",\"last_event_time_ps\":")?;
    write_value(writer, &opt_d(snapshot.common.last_event_time_ps))?;
    if version == 2 {
        write_bytes(writer, b",\"model_records\":")?;
        write_array_fallible(
            writer,
            stream
                .models
                .as_ref()
                .unwrap()
                .iter()?
                .map(|row| row.map(|row| row.value)),
        )?;
    }
    write_bytes(writer, b",\"partial\":")?;
    write_value(writer, &json!(snapshot.common.partial))?;
    write_bytes(writer, b",\"pending_events\":")?;
    write_value(writer, &json!(snapshot.common.pending_events.to_string()))?;
    if version == 1 {
        write_bytes(writer, b",\"receivers\":")?;
        write_array_fallible(
            writer,
            stream
                .receivers
                .as_ref()
                .unwrap()
                .iter()?
                .map(|row| row.map(|row| row.value)),
        )?;
    }
    write_bytes(writer, b",\"records\":")?;
    write_array_fallible(
        writer,
        stream.records.iter()?.enumerate().map(|(i, row)| {
            row.map(|row| {
                let mut value = row.value;
                value["seq"] = json!(i.to_string());
                value
            })
        }),
    )?;
    if version == 1 {
        write_bytes(writer, b",\"requests\":")?;
        write_array_fallible(
            writer,
            stream
                .requests
                .as_ref()
                .unwrap()
                .iter()?
                .map(|row| row.map(|row| row.value)),
        )?;
    }
    write_bytes(writer, b",\"start_ps\":\"0\",\"summary\":")?;
    write_array(writer, stream.summary.iter().map(record_value))?;
    write_bytes(writer, b",\"termination\":")?;
    write_value(writer, &json!(snapshot.common.termination))?;
    write_bytes(writer, b"}}\n")
}

/// Render arrays a row at a time instead of building a second full result tree.
pub(super) fn write_result(
    writer: &mut dyn std::io::Write,
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    run_id: &str,
    timestamp: &str,
    records: &[Record],
    summary: &[Record],
) -> Result<(), Diagnostic> {
    let version = if prepared.common.profile == PROFILE {
        1
    } else {
        2
    };
    let metadata = metadata::build(prepared, timestamp)?;
    write_bytes(writer, b"{\"metadata\":")?;
    write_value(writer, &metadata)?;
    write_bytes(writer, b",\"run_id\":")?;
    write_value(writer, &json!(run_id))?;
    write_bytes(
        writer,
        format!(",\"schema_version\":{version},\"simulation\":{{\"committed_events\":").as_bytes(),
    )?;
    write_value(writer, &json!(snapshot.common.committed_events.to_string()))?;
    write_bytes(writer, b",\"end_ps\":")?;
    write_value(writer, &json!(snapshot.common.end_ps.to_string()))?;
    write_bytes(writer, b",\"last_event_time_ps\":")?;
    write_value(writer, &opt_d(snapshot.common.last_event_time_ps))?;
    if version == 2 {
        write_bytes(writer, b",\"model_records\":")?;
        write_array(writer, model_records::records(prepared, snapshot)?)?;
    }
    write_bytes(writer, b",\"partial\":")?;
    write_value(writer, &json!(snapshot.common.partial))?;
    write_bytes(writer, b",\"pending_events\":")?;
    write_value(writer, &json!(snapshot.common.pending_events.to_string()))?;
    if version == 1 {
        write_bytes(writer, b",\"receivers\":")?;
        let mut rows: Vec<_> = snapshot.can.receivers.iter().collect();
        rows.sort_by_key(|r| (&r.request_id, &r.receiver));
        write_array(writer,rows.into_iter().map(|r|json!({"request_id":r.request_id,"receiver":r.receiver,"status":r.status,"observed_ps":opt_d(r.observed_ps),"received_ps":opt_d(r.received_ps)})))?;
    }
    write_bytes(writer, b",\"records\":")?;
    write_array(writer, records.iter().map(record_value))?;
    if version == 1 {
        write_bytes(writer, b",\"requests\":")?;
        let mut rows: Vec<_> = snapshot.can.requests.iter().collect();
        rows.sort_by_key(|r| &r.request_id);
        write_array(writer, rows.into_iter().map(model_records::request_json))?;
    }
    write_bytes(writer, b",\"start_ps\":\"0\",\"summary\":")?;
    write_array(writer, summary.iter().map(record_value))?;
    write_bytes(writer, b",\"termination\":")?;
    write_value(writer, &json!(snapshot.common.termination))?;
    write_bytes(writer, b"}}\n")
}

pub(super) fn diagnostics(snapshot: &Snapshot) -> Vec<u8> {
    let mut diagnostics = String::new();
    for (i, diagnostic) in snapshot.common.diagnostics.iter().enumerate() {
        let mut diagnostic = diagnostic.normalized(i as u64, i == 0);
        if diagnostic.stage == "run" && diagnostic.time_ps.is_none() {
            diagnostic.time_ps = Some(snapshot.common.end_ps);
        }
        let object = diagnostic.wire_value();
        diagnostics.push_str(&canonical(&object));
        diagnostics.push('\n');
    }
    diagnostics.into_bytes()
}
