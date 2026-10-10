//! Direct CSV projection of typed common records.
use super::aggregate::{MetricValue, Record};
use super::json::number_wire;
use serde_json::Value;

const HEADER: &str = "schema_version,run_id,seq,event_seq,effect_seq,target,metric,unit,value_kind,value,time_ps,start_ps,end_ps,request_id,receiver,reason,sample_count\n";
fn decimal<T: ToString>(value: Option<T>) -> String {
    value.map(|n| n.to_string()).unwrap_or_default()
}

pub(super) fn encode(rows: &[Record], run_id: &str, version: u32) -> Vec<u8> {
    let mut output = Vec::new();
    write(&mut output, rows, run_id, version).expect("writing to memory");
    output
}

pub(super) fn write(
    writer: &mut dyn std::io::Write,
    rows: &[Record],
    run_id: &str,
    version: u32,
) -> Result<(), crate::Diagnostic> {
    fn cell(s: String) -> String {
        if s.contains([',', '"', '\r', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    }
    writer
        .write_all(HEADER.as_bytes())
        .map_err(|e| crate::Diagnostic::output(format!("Cannot write CSV header: {e}")))?;
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
        writer
            .write_all(cells.join(",").as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(|e| crate::Diagnostic::output(format!("Cannot write CSV record: {e}")))?;
    }
    Ok(())
}

pub(super) fn write_stream(
    writer: &mut dyn std::io::Write,
    rows: impl IntoIterator<Item = Result<Value, crate::Diagnostic>>,
    run_id: &str,
    version: u32,
) -> Result<(), crate::Diagnostic> {
    fn cell(s: String) -> String {
        if s.contains([',', '"', '\r', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    }
    fn text(row: &Value, field: &str) -> String {
        row[field].as_str().unwrap_or_default().to_owned()
    }
    writer
        .write_all(HEADER.as_bytes())
        .map_err(|e| crate::Diagnostic::output(format!("Cannot write CSV header: {e}")))?;
    for row in rows {
        let row = row?;
        let value = if row["value"].is_null() {
            String::new()
        } else if let Some(n) = row["value"].as_f64() {
            number_wire(n)
        } else {
            text(&row, "value")
        };
        let cells = [
            version.to_string(),
            run_id.to_owned(),
            text(&row, "seq"),
            text(&row, "event_seq"),
            text(&row, "effect_seq"),
            text(&row, "target"),
            text(&row, "metric"),
            text(&row, "unit"),
            text(&row, "value_kind"),
            value,
            text(&row, "time_ps"),
            text(&row, "start_ps"),
            text(&row, "end_ps"),
            text(&row, "request_id"),
            text(&row, "receiver"),
            text(&row, "reason"),
            text(&row, "sample_count"),
        ];
        let line = cells
            .into_iter()
            .enumerate()
            .map(|(i, s)| if i < 2 { s } else { cell(s) })
            .collect::<Vec<_>>()
            .join(",");
        writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(|e| crate::Diagnostic::output(format!("Cannot write CSV record: {e}")))?;
    }
    Ok(())
}
