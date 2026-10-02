//! CAN schema-1 aggregation and manifest-last publication.
use crate::snapshot::{Point, Request, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const PROFILE: &str = "can.cc.ideal.v1";
const HEADER: &str = "schema_version,run_id,seq,event_seq,effect_seq,target,metric,unit,value_kind,value,time_ps,start_ps,end_ps,request_id,receiver,reason,sample_count\n";
const COLUMNS: [&str; 15] = [
    "seq",
    "event_seq",
    "effect_seq",
    "target",
    "metric",
    "unit",
    "value_kind",
    "value",
    "time_ps",
    "start_ps",
    "end_ps",
    "request_id",
    "receiver",
    "reason",
    "sample_count",
];

fn overflow() -> Diagnostic {
    Diagnostic {
        schema_version: 1,
        code: "E-0004".into(),
        stage: "output".into(),
        message: "Exact result aggregation overflow".into(),
    }
}
fn add(a: u128, b: u128) -> Result<u128, Diagnostic> {
    a.checked_add(b).ok_or_else(overflow)
}
fn mul(a: u128, b: u128) -> Result<u128, Diagnostic> {
    a.checked_mul(b).ok_or_else(overflow)
}
fn d(n: u128) -> Value {
    Value::String(n.to_string())
}
fn opt_d(n: Option<u64>) -> Value {
    n.map(|n| d(n as u128)).unwrap_or(Value::Null)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Convert an exact positive u128 rational to binary64, with a single ties-even rounding.
/// Long division avoids converting either operand to floating point first.
fn ratio(n: u128, denominator: u128) -> Result<Value, Diagnostic> {
    if denominator == 0 {
        return Ok(Value::Null);
    }
    if n == 0 {
        return Ok(json!(0.0));
    }
    let integer = n / denominator;
    let mut remainder = n % denominator;
    let next_bit = |r: &mut u128| -> u128 {
        // 2*r >= denominator, without overflowing 2*r.
        if *r >= denominator - *r {
            *r -= denominator - *r;
            1
        } else {
            *r *= 2;
            0
        }
    };
    let mut exponent: i32;
    let mut significand: u128;
    let guard: u128;
    let sticky: bool;
    if integer > 0 {
        let bits = 128 - integer.leading_zeros();
        exponent = bits as i32 - 1;
        if bits > 53 {
            let shift = bits - 53;
            significand = integer >> shift;
            guard = (integer >> (shift - 1)) & 1;
            sticky = (integer & ((1u128 << (shift - 1)) - 1)) != 0 || remainder != 0;
        } else {
            significand = integer;
            for _ in bits..53 {
                significand = (significand << 1) | next_bit(&mut remainder);
            }
            guard = next_bit(&mut remainder);
            sticky = remainder != 0;
        }
    } else {
        exponent = -1;
        while next_bit(&mut remainder) == 0 {
            exponent -= 1;
        }
        significand = 1;
        for _ in 1..53 {
            significand = (significand << 1) | next_bit(&mut remainder);
        }
        guard = next_bit(&mut remainder);
        sticky = remainder != 0;
    }
    if guard == 1 && (sticky || significand & 1 == 1) {
        significand += 1;
    }
    if significand == 1u128 << 53 {
        significand >>= 1;
        exponent += 1;
    }
    let number = (significand as f64) * 2.0f64.powi(exponent - 52);
    if !number.is_finite() {
        return Err(overflow());
    }
    Ok(json!(number))
}

fn number_wire(n: f64) -> String {
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
fn canonical(value: &Value) -> String {
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

#[derive(Clone, Debug)]
struct Record {
    value: Value,
    order: usize,
    kind: u8,
}
impl Record {
    fn aggregate(target: &str, metric: &str, value: Value, start: u64, end: u64) -> Self {
        let (unit, value_kind, _, _) = descriptor(metric);
        Self {
            value: json!({"seq":"0","event_seq":null,"effect_seq":null,"target":target,"metric":metric,"unit":unit,"value_kind":value_kind,"value":value,"time_ps":null,"start_ps":start.to_string(),"end_ps":end.to_string(),"request_id":null,"receiver":null,"reason":null,"sample_count":null}),
            order: 0,
            kind: 0,
        }
    }
    fn point(point: &Point, metric: &str, value: Value, order: usize) -> Self {
        let mut record = Self::aggregate(&point.target, metric, value, 0, 0);
        record.value["time_ps"] = d(point.time_ps as u128);
        record.value["start_ps"] = Value::Null;
        record.value["end_ps"] = Value::Null;
        record.value["event_seq"] = opt_d(point.event_seq);
        record.value["effect_seq"] = opt_d(point.effect_seq);
        record.value["request_id"] = json!(point.request_id);
        record.value["receiver"] = json!(point.receiver);
        record.order = order;
        record.kind = if point.event_seq.is_some() { 2 } else { 1 };
        record
    }
    fn target(&self) -> &str {
        self.value["target"].as_str().unwrap()
    }
    fn metric(&self) -> &str {
        self.value["metric"].as_str().unwrap()
    }
    fn time(&self) -> u64 {
        self.value["time_ps"]
            .as_str()
            .or_else(|| self.value["end_ps"].as_str())
            .unwrap()
            .parse()
            .unwrap()
    }
    fn key(&self) -> (&str, &str, Option<&str>, Option<&str>) {
        (
            self.target(),
            self.metric(),
            self.value["reason"].as_str(),
            self.value["receiver"].as_str(),
        )
    }
}

fn descriptor(metric: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match metric {
        "queue_length" => ("count", "integer", "point", "identity"),
        "queue_max" => ("count", "integer", "summary", "max"),
        "tx_wait_ps" | "arbitration_wait_ps" | "transfer_ps" | "delivery_ps" => {
            ("ps", "integer", "point", "identity")
        }
        "tx_wait_mean_ps"
        | "arbitration_wait_mean_ps"
        | "transfer_mean_ps"
        | "delivery_mean_ps" => ("ps", "number", "summary", "sample_mean"),
        "queue_mean" => ("count", "number", "summary", "time_mean"),
        "buffer_utilization" => ("1", "number", "point", "identity"),
        "buffer_utilization_mean" => ("1", "number", "window_summary", "time_mean"),
        "bus_utilization" | "frame_utilization" => {
            ("1", "number", "window_summary", "occupancy_ratio")
        }
        "payload_bits" | "serialized_bits" | "occupied_bits" | "received_payload_bits" => {
            ("bit", "integer", "window_summary", "sum")
        }
        "payload_throughput_bps" | "serialized_throughput_bps" | "received_throughput_bps" => {
            ("bit/s", "number", "window_summary", "rate")
        }
        _ => ("count", "integer", "summary", "sum"),
    }
}
const METRICS: &str = "queue_length queue_max generated admitted processing pending in_flight success dropped unfinished attempts retries failed receiver_opportunities received filtered rx_pending tx_wait_ps arbitration_wait_ps transfer_ps delivery_ps tx_wait_mean_ps arbitration_wait_mean_ps transfer_mean_ps delivery_mean_ps queue_mean buffer_utilization buffer_utilization_mean bus_utilization frame_utilization payload_bits serialized_bits occupied_bits received_payload_bits payload_throughput_bps serialized_throughput_bps received_throughput_bps";

fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Value>, Vec<Value>), Diagnostic> {
    let h = snapshot.end_ps;
    let mut points = Vec::new();
    for (i, point) in snapshot.points.iter().enumerate() {
        points.push(Record::point(
            point,
            &point.metric,
            d(point.value as u128),
            i,
        ));
        if point.metric == "queue_length" {
            let capacity = prepared
                .controllers
                .iter()
                .find(|c| format!("{}.txQueue", c.id) == point.target)
                .map(|c| c.queue_capacity)
                .ok_or_else(|| Diagnostic::output("Queue point refers to an unknown queue"))?;
            points.push(Record::point(
                point,
                "buffer_utilization",
                ratio(point.value as u128, capacity as u128)?,
                i,
            ));
        }
    }
    // Preserve contiguous callback commit order, independent of reservation sequence.
    // A derived buffer point belongs to its queue callback and gets a separate effect.
    let mut offset = 0;
    let mut group = 0;
    while offset < points.len() {
        let event = points[offset].value["event_seq"].clone();
        let mut end = offset + 1;
        while end < points.len() && points[end].value["event_seq"] == event {
            end += 1;
        }
        if !event.is_null() {
            points[offset..end].sort_by(|a, b| {
                (
                    a.target(),
                    a.metric(),
                    a.value["request_id"].as_str(),
                    a.value["receiver"].as_str(),
                )
                    .cmp(&(
                        b.target(),
                        b.metric(),
                        b.value["request_id"].as_str(),
                        b.value["receiver"].as_str(),
                    ))
            });
            for (effect, point) in points[offset..end].iter_mut().enumerate() {
                point.order = group;
                point.value["effect_seq"] = d(effect as u128);
            }
        }
        offset = end;
        group += 1;
    }
    if h == 0 && !snapshot.partial {
        points.clear();
    }
    let mut summary = Vec::new();
    let mut targets = prepared
        .controllers
        .iter()
        .map(|c| c.id.clone())
        .collect::<Vec<_>>();
    targets.push(prepared.bus_id.clone());
    targets.push("$all".into());
    for target in &targets {
        let reqs: Vec<_> = snapshot
            .requests
            .iter()
            .filter(|r| target == "$all" || r.source == *target || r.bus == *target)
            .collect();
        let count = |status: &str| reqs.iter().filter(|r| r.status == status).count() as u128;
        let processing = count("processing");
        let pending = count("pending");
        let flight = count("in_flight");
        let success = count("success");
        let dropped = count("dropped");
        let admitted = add(add(pending, flight)?, success)?;
        let unfinished = add(add(processing, pending)?, flight)?;
        let quantities = [
            ("generated", reqs.len() as u128),
            ("admitted", admitted),
            ("processing", processing),
            ("pending", pending),
            ("in_flight", flight),
            ("success", success),
            ("dropped", dropped),
            ("unfinished", unfinished),
            ("attempts", add(flight, success)?),
            ("retries", 0),
            ("failed", 0),
        ];
        for (metric, count) in quantities {
            let mut record = Record::aggregate(target, metric, d(count), 0, h);
            if metric == "dropped" {
                record.value["reason"] = json!("queue_full");
            }
            summary.push(record);
        }
        let receivers: Vec<_> = snapshot
            .receivers
            .iter()
            .filter(|r| target == "$all" || r.receiver == *target || target == &prepared.bus_id)
            .collect();
        for (metric, status) in [
            ("receiver_opportunities", None),
            ("received", Some("received")),
            ("filtered", Some("filtered")),
            ("rx_pending", Some("pending")),
        ] {
            let count = receivers
                .iter()
                .filter(|r| status.is_none_or(|s| r.status == s))
                .count() as u128;
            let mut record = Record::aggregate(target, metric, d(count), 0, h);
            if target != "$all" && target != &prepared.bus_id {
                record.value["receiver"] = json!(target);
            }
            summary.push(record);
        }
        for (metric, point_metric) in [
            ("tx_wait_mean_ps", "tx_wait_ps"),
            ("arbitration_wait_mean_ps", "arbitration_wait_ps"),
            ("transfer_mean_ps", "transfer_ps"),
            ("delivery_mean_ps", "delivery_ps"),
        ] {
            if metric == "transfer_mean_ps" && target != "$all" && target != &prepared.bus_id {
                continue;
            }
            let mut sum = 0;
            let mut count = 0;
            for point in &snapshot.points {
                if point.metric == point_metric
                    && (target == "$all" || target == &prepared.bus_id || point.target == *target)
                {
                    sum = add(sum, point.value as u128)?;
                    count = add(count, 1)?;
                }
            }
            let mut record = Record::aggregate(target, metric, ratio(sum, count)?, 0, h);
            record.value["sample_count"] = d(count);
            if metric == "delivery_mean_ps" && target != "$all" && target != &prepared.bus_id {
                record.value["receiver"] = json!(target);
            }
            summary.push(record);
        }
    }
    for c in &prepared.controllers {
        let queue = format!("{}.txQueue", c.id);
        let qpoints: Vec<_> = snapshot
            .points
            .iter()
            .filter(|p| p.target == queue && p.metric == "queue_length")
            .collect();
        let max = qpoints.iter().map(|p| p.value).max().unwrap_or(0);
        let integral = queue_integral(&qpoints, 0, h)?;
        summary.push(Record::aggregate(&queue, "queue_max", d(max as u128), 0, h));
        summary.push(Record::aggregate(
            &queue,
            "queue_mean",
            ratio(integral, h as u128)?,
            0,
            h,
        ));
        let mut drop = Record::aggregate(
            &queue,
            "dropped",
            d(snapshot
                .requests
                .iter()
                .filter(|r| r.source == c.id && r.status == "dropped")
                .count() as u128),
            0,
            h,
        );
        drop.value["reason"] = json!("queue_full");
        summary.push(drop);
    }
    let mut start = 0;
    while start < h {
        let end = start.saturating_add(prepared.metrics_window_ps).min(h);
        if end == start {
            return Err(Diagnostic::output("Metric window must be positive"));
        }
        points.extend(interval_metrics(prepared, snapshot, start, end, false)?);
        start = end;
    }
    summary.extend(interval_metrics(prepared, snapshot, 0, h, true)?);
    points.sort_by(|a, b| {
        a.time()
            .cmp(&b.time())
            .then(a.kind.cmp(&b.kind))
            .then_with(|| {
                if a.kind == 2 {
                    a.order.cmp(&b.order).then_with(|| {
                        a.value["effect_seq"]
                            .as_str()
                            .unwrap()
                            .parse::<u128>()
                            .unwrap()
                            .cmp(
                                &b.value["effect_seq"]
                                    .as_str()
                                    .unwrap()
                                    .parse::<u128>()
                                    .unwrap(),
                            )
                    })
                } else {
                    a.key().cmp(&b.key())
                }
            })
    });
    summary.sort_by(|a, b| a.key().cmp(&b.key()));
    let numbered = |rows: Vec<Record>| {
        rows.into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                r.value["seq"] = d(i as u128);
                r.value
            })
            .collect()
    };
    Ok((numbered(points), numbered(summary)))
}

fn queue_integral(points: &[&Point], start: u64, end: u64) -> Result<u128, Diagnostic> {
    let mut previous = start;
    let mut value = 0u64;
    let mut integral = 0;
    for point in points {
        if point.time_ps > end {
            break;
        }
        if point.time_ps <= start {
            value = point.value;
            continue;
        }
        integral = add(
            integral,
            mul(value as u128, (point.time_ps - previous) as u128)?,
        )?;
        previous = point.time_ps;
        value = point.value;
    }
    add(integral, mul(value as u128, (end - previous) as u128)?)
}

fn occupancy(requests: &[Request], start: u64, end: u64, frame: bool) -> Result<u128, Diagnostic> {
    let mut intervals = Vec::new();
    for r in requests {
        if let Some(sof) = r.sof_ps {
            let stop = if frame {
                r.eof_ps.or(r.planned_eof_ps)
            } else {
                r.release_ps.or(r.planned_release_ps)
            }
            .unwrap_or(end);
            let a = sof.max(start);
            let b = stop.min(end);
            if a < b {
                intervals.push((a, b));
            }
        }
    }
    intervals.sort();
    let mut total = 0;
    let mut covered = start;
    for (a, b) in intervals {
        if b > covered {
            total = add(total, (b - a.max(covered)) as u128)?;
            covered = b;
        }
    }
    Ok(total)
}

fn interval_metrics(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    start: u64,
    end: u64,
    full: bool,
) -> Result<Vec<Record>, Diagnostic> {
    let mut rows = Vec::new();
    let length = (end - start) as u128;
    let completed = |t: Option<u64>| t.is_some_and(|t| full || (start <= t && t < end));
    let mut payload = 0;
    let mut serialized = 0;
    let mut occupied = 0;
    for r in &snapshot.requests {
        if completed(r.eof_ps) {
            payload = add(payload, r.payload_bits as u128)?;
            serialized = add(serialized, r.frame_bits as u128)?;
        }
        if completed(r.release_ps) {
            occupied = add(occupied, add(r.frame_bits as u128, 3)?)?;
        }
    }
    for (metric, value) in [
        (
            "bus_utilization",
            ratio(occupancy(&snapshot.requests, start, end, false)?, length)?,
        ),
        (
            "frame_utilization",
            ratio(occupancy(&snapshot.requests, start, end, true)?, length)?,
        ),
        ("payload_bits", d(payload)),
        ("serialized_bits", d(serialized)),
        ("occupied_bits", d(occupied)),
        (
            "payload_throughput_bps",
            ratio(mul(payload, 1_000_000_000_000)?, length)?,
        ),
        (
            "serialized_throughput_bps",
            ratio(mul(serialized, 1_000_000_000_000)?, length)?,
        ),
    ] {
        rows.push(Record::aggregate(
            &prepared.bus_id,
            metric,
            value,
            start,
            end,
        ));
    }
    let lookup: BTreeMap<_, _> = snapshot
        .requests
        .iter()
        .map(|r| (r.request_id.as_str(), r))
        .collect();
    for c in &prepared.controllers {
        let mut bits = 0;
        for receiver in &snapshot.receivers {
            if receiver.receiver == c.id && completed(receiver.received_ps) {
                let request = lookup
                    .get(receiver.request_id.as_str())
                    .ok_or_else(|| Diagnostic::output("Receiver refers to an unknown request"))?;
                bits = add(bits, request.payload_bits as u128)?;
            }
        }
        for (metric, value) in [
            ("received_payload_bits", d(bits)),
            (
                "received_throughput_bps",
                ratio(mul(bits, 1_000_000_000_000)?, length)?,
            ),
        ] {
            let mut row = Record::aggregate(&c.id, metric, value, start, end);
            row.value["receiver"] = json!(c.id);
            rows.push(row);
        }
        let queue = format!("{}.txQueue", c.id);
        let qpoints: Vec<_> = snapshot
            .points
            .iter()
            .filter(|p| p.target == queue && p.metric == "queue_length")
            .collect();
        rows.push(Record::aggregate(
            &queue,
            "buffer_utilization_mean",
            ratio(
                queue_integral(&qpoints, start, end)?,
                mul(c.queue_capacity as u128, length)?,
            )?,
            start,
            end,
        ));
    }
    Ok(rows)
}

fn request_json(r: &Request) -> Value {
    json!({"request_id":r.request_id,"source":r.source,"bus":r.bus,"status":r.status,"generated_ps":r.generated_ps.to_string(),"ready_ps":opt_d(r.ready_ps),"sof_ps":opt_d(r.sof_ps),"eof_ps":opt_d(r.eof_ps),"payload_bits":r.payload_bits.to_string(),"serialized_bits":r.frame_bits.to_string(),"attempts":if r.sof_ps.is_some(){"1"}else{"0"},"retries":"0","drop_reason":if r.status=="dropped"{Some("queue_full")}else{None},"model_fields":{"profile":PROFILE,"schema_version":1,"crc15":r.crc15.to_string(),"stuff_bits":r.stuff_bits.to_string(),"frame_bits":r.frame_bits.to_string(),"intermission_bits":"3","bitrate_bps":r.bitrate_bps.to_string(),"planned_eof_ps":opt_d(r.planned_eof_ps),"planned_release_ps":opt_d(r.planned_release_ps),"release_ps":opt_d(r.release_ps)}})
}

fn csv(rows: &[Value], run_id: &str) -> Vec<u8> {
    fn cell(s: String) -> String {
        if s.contains([',', '"', '\r', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    }
    let mut output = HEADER.to_string();
    for row in rows {
        let mut cells = vec!["1".into(), run_id.into()];
        for key in COLUMNS {
            let text = match &row[key] {
                Value::Null => String::new(),
                Value::String(s) => s.clone(),
                v => canonical(v),
            };
            cells.push(cell(text));
        }
        output.push_str(&cells.join(","));
        output.push('\n');
    }
    output.into_bytes()
}

fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (seconds / 86400) as i64;
    // Gregorian civil date conversion, epoch 1970-01-01.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}
fn run_id() -> Result<String, Diagnostic> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| Diagnostic::output(format!("Cannot obtain UUID entropy: {e}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("writing to String");
    }
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn metadata(prepared: &PreparedSimulation, timestamp: &str) -> Result<Value, Diagnostic> {
    let mut sources = Vec::new();
    for input in &prepared.inputs {
        // Preparation already resolved the path. Export must not reacquire inputs:
        // the source can be moved or removed after a successful prepare call.
        let canonical_path = &input.path;
        let logical = match input.path.extension().and_then(|s| s.to_str()) {
            Some("ini") => "config".into(),
            Some("json") => "workload".into(),
            _ => format!(
                "root0/{}",
                input.path.file_name().unwrap_or_default().to_string_lossy()
            ),
        };
        sources.push(json!({"logical_path":logical,"canonical_path":canonical_path.to_string_lossy(),"sha256":digest(input.content.as_bytes()),"content_utf8":input.content}));
    }
    sources.sort_by(|a, b| a["logical_path"].as_str().cmp(&b["logical_path"].as_str()));
    let source_hashes: Vec<_> = sources
        .iter()
        .map(|s| json!({"logical_path":s["logical_path"],"sha256":s["sha256"]}))
        .collect();
    let mut config = BTreeMap::new();
    config.insert("network".to_string(), canonical(&json!(prepared.network)));
    config.insert("time-limit".into(), format!("{}ps", prepared.time_limit_ps));
    config.insert(
        "metrics-window".into(),
        format!("{}ps", prepared.metrics_window_ps),
    );
    config.insert("max-events".into(), prepared.max_events.to_string());
    config.insert(
        "max-delta-cycles".into(),
        prepared.max_delta_cycles.to_string(),
    );
    config.insert(
        format!("{}.bitrate", prepared.bus_id),
        format!("{}bit/s", prepared.bitrate),
    );
    for c in &prepared.controllers {
        for (name, value) in [
            ("queueCapacity", c.queue_capacity.to_string()),
            ("txProcessingDelay", format!("{}ps", c.tx_processing_ps)),
            ("rxProcessingDelay", format!("{}ps", c.rx_processing_ps)),
            ("rxFilter", canonical(&json!(c.rx_filter))),
            ("txChannelDelay", format!("{}ps", c.tx_channel_ps)),
            ("rxChannelDelay", format!("{}ps", c.rx_channel_ps)),
        ] {
            config.insert(format!("{}.{}", c.id, name), value);
        }
    }
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let mut metrics: Vec<_> = METRICS.split_whitespace().map(|metric| { let (unit,kind,sampling,aggregation) = descriptor(metric); json!({"metric_id":metric,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation}) }).collect();
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    let mut initial_state = vec![
        json!({"instance":prepared.network,"state":"{}"}),
        json!({"instance":prepared.bus_id,"state":canonical(&json!({"profile":PROFILE,"state":"idle","active_request":null}))}),
    ];
    for (i, c) in prepared.controllers.iter().enumerate() {
        let mut generators: Vec<_> = prepared.generators.iter().filter(|g| g.source == i).map(|g| json!({"id":g.id,"next_ordinal":"0","next_time_ps":g.schedule.time(0).map(|n| n.to_string())})).collect();
        generators.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        initial_state.push(json!({"instance":c.id,"state":canonical(&json!({"profile":PROFILE,"queue":[],"processing":[],"transmitting":null,"receivers":[],"generators":generators}))}));
    }
    initial_state.sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(source_hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","models":[{"type":PROFILE,"version":"1","assumptions":["ideal ACK","no retransmission","no error injection","content-dependent frame duration","fixed propagation delay","no source self-delivery","filter independent of ACK"]}],"initial_state":initial_state,"metrics":metrics,"time_resolution_ps":"1","window_ps":prepared.metrics_window_ps.to_string(),"seed":null});
    for key in [
        "git_commit",
        "binary_sha256",
        "compiler",
        "target_triple",
        "os",
        "cpu",
        "cargo_lock_sha256",
        "toolchain",
        "adoption_ledger_version",
    ] {
        result[key] = json!("unknown");
    }
    if let Ok(exe) = std::env::current_exe().and_then(fs::read) {
        result["binary_sha256"] = json!(digest(&exe));
    }
    result["os"] = json!(std::env::consts::OS);
    result["cpu"] = json!(std::env::consts::ARCH);
    result["implementation_coverage"] = json!({"profile":"can-v0.1","reproduction_conditions":"partially_identified","limitations":["start timestamp and run_id collected at export rather than execution start","source logical roots inferred from filename; source root mapping unavailable","resolved config covers prepared fields rather than every source setting","initial channel states unavailable","build provenance compiler/toolchain/commit/lock unavailable","OS release and CPU model unavailable"]});
    Ok(result)
}

/// Write the five required files within an already reserved empty output directory.
/// Completed data remains for diagnosis on publication failure; manifest is published last.
pub fn export(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    output: &Path,
) -> Result<(), Diagnostic> {
    let run_id = run_id()?;
    let timestamp = utc_now();
    let (records, summary) = records(prepared, snapshot)?;
    let mut requests: Vec<_> = snapshot.requests.iter().map(request_json).collect();
    requests.sort_by(|a, b| a["request_id"].as_str().cmp(&b["request_id"].as_str()));
    let mut receivers: Vec<_> = snapshot.receivers.iter().map(|r| json!({"request_id":r.request_id,"receiver":r.receiver,"status":r.status,"observed_ps":opt_d(r.observed_ps),"received_ps":opt_d(r.received_ps)})).collect();
    receivers.sort_by(|a, b| {
        (a["request_id"].as_str(), a["receiver"].as_str())
            .cmp(&(b["request_id"].as_str(), b["receiver"].as_str()))
    });
    let result = json!({"schema_version":1,"run_id":run_id,"metadata":metadata(prepared,&timestamp)?,"simulation":{"termination":snapshot.termination,"partial":snapshot.partial,"start_ps":"0","end_ps":snapshot.end_ps.to_string(),"last_event_time_ps":opt_d(snapshot.last_event_time_ps),"committed_events":snapshot.committed_events.to_string(),"pending_events":snapshot.pending_events.to_string(),"records":records,"summary":summary,"requests":requests,"receivers":receivers}});
    let mut diagnostics = String::new();
    for (i, diagnostic) in snapshot.diagnostics.iter().enumerate() {
        let object = json!({"schema_version":1,"seq":i.to_string(),"severity":"error","primary":i==0,"code":diagnostic.code,"phase":diagnostic.stage,"reason":"runtime_error","message":diagnostic.message,"source":null,"line":null,"column":null,"end_line":null,"end_column":null,"target":null,"time_ps":snapshot.end_ps.to_string(),"event_seq":null,"details":{}});
        diagnostics.push_str(&canonical(&object));
        diagnostics.push('\n');
    }
    let files = BTreeMap::from([
        ("diagnostics.jsonl", diagnostics.into_bytes()),
        (
            "events.csv",
            csv(result["simulation"]["records"].as_array().unwrap(), &run_id),
        ),
        (
            "results.json",
            format!("{}\n", canonical(&result)).into_bytes(),
        ),
        (
            "summary.csv",
            csv(result["simulation"]["summary"].as_array().unwrap(), &run_id),
        ),
    ]);
    let manifest_files: Vec<_> = files.iter().map(|(name,bytes)| json!({"name":name,"sha256":digest(bytes),"bytes":bytes.len().to_string()})).collect();
    let manifest = json!({"schema_version":1,"run_id":run_id,"status":"complete","termination":snapshot.termination,"partial":snapshot.partial,"metadata_ref":"results.json#/metadata","files":manifest_files});
    let temporary = output.join(format!(".tmp-{run_id}"));
    fs::create_dir(&temporary)
        .map_err(|e| Diagnostic::output(format!("Cannot create output staging directory: {e}")))?;
    let publish = || -> std::io::Result<()> {
        for (name, bytes) in &files {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(temporary.join(name))?;
            file.write_all(bytes)?;
            file.flush()?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary.join("manifest.json"))?;
        file.write_all(format!("{}\n", canonical(&manifest)).as_bytes())?;
        file.flush()?;
        drop(file);
        // Hard-link publication supplies the no-replace property missing from std::fs::rename.
        // Each link exposes only a fully written file, within the same filesystem.
        for name in files
            .keys()
            .copied()
            .chain(std::iter::once("manifest.json"))
        {
            fs::hard_link(temporary.join(name), output.join(name))?;
            fs::remove_file(temporary.join(name))?;
        }
        Ok(())
    }();
    let _ = fs::remove_dir_all(&temporary);
    publish.map_err(|e| Diagnostic::output(format!("Cannot publish result files: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Receiver;
    use crate::types::Controller;

    fn fixture() -> (PreparedSimulation, Snapshot) {
        let controller = |id: &str| Controller {
            id: id.into(),
            queue_capacity: 2,
            tx_processing_ps: 0,
            rx_processing_ps: 0,
            rx_filter: "all".into(),
            tx_channel_ps: 0,
            rx_channel_ps: 0,
        };
        let prepared = PreparedSimulation {
            network: "Net".into(),
            bus_id: "Net.bus".into(),
            bitrate: 500_000,
            controllers: vec![
                controller("Net.a"),
                controller("Net.b"),
                controller("Net.c"),
            ],
            generators: vec![],
            time_limit_ps: 30,
            metrics_window_ps: 10,
            max_events: 100,
            max_delta_cycles: 100,
            channel_count: 0,
            inputs: vec![],
        };
        let mut points = vec![];
        for target in ["Net.a.txQueue", "Net.b.txQueue", "Net.c.txQueue"] {
            points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: 0,
                target: target.into(),
                metric: "queue_length".into(),
                value: 0,
                request_id: None,
                receiver: None,
            });
        }
        for (seq, time, target, metric, value, receiver) in [
            (1, 2, "Net.a.txQueue", "queue_length", 1, None),
            (7, 5, "Net.a.txQueue", "queue_length", 0, None),
            (7, 5, "Net.a", "tx_wait_ps", 5, None),
            (7, 5, "Net.a", "arbitration_wait_ps", 3, None),
            (3, 15, "Net.bus", "transfer_ps", 10, None),
            (9, 18, "Net.b", "delivery_ps", 18, Some("Net.b")),
            (11, 20, "Net.c", "delivery_ps", 20, Some("Net.c")),
        ] {
            points.push(Point {
                event_seq: Some(seq),
                effect_seq: Some(0),
                time_ps: time,
                target: target.into(),
                metric: metric.into(),
                value,
                request_id: Some("load:0".into()),
                receiver: receiver.map(str::to_string),
            });
        }
        let request = Request {
            request_id: "load:0".into(),
            source: "Net.a".into(),
            bus: "Net.bus".into(),
            status: "success".into(),
            generated_ps: 0,
            ready_ps: Some(2),
            sof_ps: Some(5),
            eof_ps: Some(15),
            planned_eof_ps: Some(15),
            planned_release_ps: Some(18),
            release_ps: Some(18),
            payload_bits: 8,
            frame_bits: 52,
            crc15: 0,
            stuff_bits: 0,
            bitrate_bps: 500_000,
        };
        let snapshot = Snapshot {
            termination: "events_exhausted".into(),
            partial: false,
            end_ps: 30,
            last_event_time_ps: Some(20),
            committed_events: 8,
            pending_events: 0,
            bus_state: "idle".into(),
            requests: vec![request],
            receivers: vec![
                Receiver {
                    request_id: "load:0".into(),
                    receiver: "Net.b".into(),
                    status: "received".into(),
                    observed_ps: Some(15),
                    received_ps: Some(18),
                },
                Receiver {
                    request_id: "load:0".into(),
                    receiver: "Net.c".into(),
                    status: "received".into(),
                    observed_ps: Some(15),
                    received_ps: Some(20),
                },
            ],
            points,
            diagnostics: vec![],
        };
        (prepared, snapshot)
    }
    fn row<'a>(rows: &'a [Value], target: &str, metric: &str) -> &'a Value {
        rows.iter()
            .find(|r| r["target"] == target && r["metric"] == metric)
            .unwrap()
    }
    fn number(rows: &[Value], target: &str, metric: &str) -> f64 {
        row(rows, target, metric)["value"].as_f64().unwrap()
    }
    fn count(rows: &[Value], target: &str, metric: &str) -> u128 {
        row(rows, target, metric)["value"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }
    fn temp_dir() -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("dir-simulator-output-test-{}", run_id().unwrap()));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn analytic_summary_and_conservation() {
        let (prepared, snapshot) = fixture();
        let (_, summary) = records(&prepared, &snapshot).unwrap();
        for target in ["Net.a", "Net.bus", "$all"] {
            assert_eq!(count(&summary, target, "generated"), 1);
            assert_eq!(
                count(&summary, target, "generated"),
                count(&summary, target, "processing")
                    + count(&summary, target, "admitted")
                    + count(&summary, target, "dropped")
            );
            assert_eq!(
                count(&summary, target, "admitted"),
                count(&summary, target, "pending")
                    + count(&summary, target, "in_flight")
                    + count(&summary, target, "success")
            );
        }
        assert_eq!(count(&summary, "$all", "receiver_opportunities"), 2);
        assert_eq!(number(&summary, "$all", "tx_wait_mean_ps"), 5.0);
        assert_eq!(number(&summary, "$all", "arbitration_wait_mean_ps"), 3.0);
        assert_eq!(number(&summary, "$all", "transfer_mean_ps"), 10.0);
        assert_eq!(number(&summary, "$all", "delivery_mean_ps"), 19.0);
        assert_eq!(number(&summary, "Net.a.txQueue", "queue_mean"), 0.1);
        assert_eq!(
            number(&summary, "Net.a.txQueue", "buffer_utilization_mean"),
            0.05
        );
        assert_eq!(count(&summary, "Net.a.txQueue", "queue_max"), 1);
        assert_eq!(number(&summary, "Net.bus", "bus_utilization"), 13.0 / 30.0);
        assert_eq!(
            number(&summary, "Net.bus", "frame_utilization"),
            10.0 / 30.0
        );
        assert_eq!(count(&summary, "Net.bus", "payload_bits"), 8);
        assert_eq!(count(&summary, "Net.bus", "serialized_bits"), 52);
        assert_eq!(count(&summary, "Net.bus", "occupied_bits"), 55);
        assert_eq!(count(&summary, "Net.b", "received_payload_bits"), 8);
    }

    #[test]
    fn windows_are_half_open_and_short_window_uses_actual_length() {
        let (mut prepared, snapshot) = fixture();
        prepared.metrics_window_ps = 20;
        let (events, _) = records(&prepared, &snapshot).unwrap();
        let windows: Vec<_> = events
            .iter()
            .filter(|r| r["metric"] == "received_payload_bits" && r["target"] == "Net.c")
            .collect();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0]["value"], "0");
        assert_eq!(windows[1]["value"], "8");
        let throughput = events
            .iter()
            .find(|r| {
                r["metric"] == "received_throughput_bps"
                    && r["target"] == "Net.c"
                    && r["start_ps"] == "20"
            })
            .unwrap();
        assert_eq!(throughput["value"].as_f64().unwrap(), 800_000_000_000.0);
    }

    #[test]
    fn in_flight_and_intermission_are_clipped() {
        let (prepared, mut snapshot) = fixture();
        snapshot.end_ps = 10;
        snapshot.requests[0].status = "in_flight".into();
        snapshot.requests[0].eof_ps = None;
        snapshot.requests[0].release_ps = None;
        snapshot.receivers.clear();
        snapshot.points.retain(|p| p.time_ps < 10);
        let (_, summary) = records(&prepared, &snapshot).unwrap();
        assert_eq!(count(&summary, "$all", "in_flight"), 1);
        assert_eq!(count(&summary, "$all", "unfinished"), 1);
        assert_eq!(count(&summary, "Net.bus", "payload_bits"), 0);
        assert_eq!(number(&summary, "Net.bus", "bus_utilization"), 0.5);
        assert_eq!(number(&summary, "Net.bus", "frame_utilization"), 0.5);
        assert!(row(&summary, "$all", "transfer_mean_ps")["value"].is_null());
        snapshot.end_ps = 17;
        snapshot.requests[0].status = "success".into();
        snapshot.requests[0].eof_ps = Some(15);
        let (_, summary) = records(&prepared, &snapshot).unwrap();
        assert_eq!(number(&summary, "Net.bus", "bus_utilization"), 12.0 / 17.0);
        assert_eq!(
            number(&summary, "Net.bus", "frame_utilization"),
            10.0 / 17.0
        );
        assert_eq!(count(&summary, "Net.bus", "occupied_bits"), 0);
    }

    #[test]
    fn zero_duration_and_capacity_have_null_ratios() {
        let (mut prepared, mut snapshot) = fixture();
        prepared.controllers[0].queue_capacity = 0;
        snapshot.end_ps = 0;
        snapshot.requests.clear();
        snapshot.receivers.clear();
        snapshot.points.retain(|p| p.event_seq.is_none());
        let (events, summary) = records(&prepared, &snapshot).unwrap();
        assert!(events.is_empty());
        assert_eq!(count(&summary, "Net.a.txQueue", "queue_max"), 0);
        for (target, metric) in [
            ("Net.bus", "bus_utilization"),
            ("Net.a.txQueue", "queue_mean"),
            ("Net.a.txQueue", "buffer_utilization_mean"),
            ("$all", "delivery_mean_ps"),
        ] {
            assert!(row(&summary, target, metric)["value"].is_null());
        }
        snapshot.end_ps = 30;
        let (events, summary) = records(&prepared, &snapshot).unwrap();
        assert_eq!(number(&summary, "Net.bus", "bus_utilization"), 0.0);
        assert!(
            events
                .iter()
                .filter(|p| p["target"] == "Net.a.txQueue" && p["metric"] == "buffer_utilization")
                .all(|p| p["value"].is_null())
        );
        assert!(row(&summary, "Net.a.txQueue", "buffer_utilization_mean")["value"].is_null());
    }

    #[test]
    fn abnormal_boundary_points_are_summary_only() {
        let (prepared, mut snapshot) = fixture();
        snapshot.end_ps = 20;
        snapshot.partial = true;
        snapshot.termination = "execution_failed".into();
        let (events, summary) = records(&prepared, &snapshot).unwrap();
        assert_eq!(count(&summary, "Net.c", "received_payload_bits"), 8);
        assert_eq!(
            events
                .iter()
                .filter(|r| r["metric"] == "received_payload_bits" && r["target"] == "Net.c")
                .map(|r| r["value"].as_str().unwrap().parse::<u128>().unwrap())
                .sum::<u128>(),
            0
        );
        assert!(
            events
                .iter()
                .any(|r| r["time_ps"] == "20" && r["metric"] == "delivery_ps")
        );
    }

    #[test]
    fn rational_conversion_is_single_rounding_and_checked() {
        // Converting 2^53+1 first would incorrectly lose the exact numerator.
        assert_eq!(
            ratio(9_007_199_254_740_993, 3).unwrap().as_f64().unwrap(),
            3_002_399_751_580_331.0
        );
        assert_eq!(ratio(1, 3).unwrap().as_f64().unwrap(), 1.0 / 3.0);
        assert_eq!(ratio(u128::MAX, u128::MAX).unwrap().as_f64().unwrap(), 1.0);
        assert_eq!(
            ratio(1, u128::MAX).unwrap().as_f64().unwrap(),
            2.0f64.powi(-128)
        );
        assert_eq!(
            ratio(9_007_199_254_740_993, 1).unwrap().as_f64().unwrap(),
            9_007_199_254_740_992.0
        );
        assert_eq!(
            ratio(9_007_199_254_740_995, 1).unwrap().as_f64().unwrap(),
            9_007_199_254_740_996.0
        );
        assert_eq!(mul(u128::MAX, 2).unwrap_err().code, "E-0004");
        assert_eq!(number_wire(0.5), "5.0000000000000000e-1");
        assert_eq!(number_wire(-0.0), "0.0000000000000000e+0");
        assert_eq!(
            canonical(&json!({"z":"\n\t\u{1}","a":"日本語/"})),
            "{\"a\":\"日本語/\",\"z\":\"\\u000a\\u0009\\u0001\"}"
        );
    }

    #[test]
    fn publication_hashes_and_csv_match_json_and_existing_files_survive() {
        let (prepared, snapshot) = fixture();
        let out = temp_dir();
        export(&prepared, &snapshot, &out).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(out.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["files"].as_array().unwrap().len(), 4);
        for file in manifest["files"].as_array().unwrap() {
            let bytes = fs::read(out.join(file["name"].as_str().unwrap())).unwrap();
            assert_eq!(file["sha256"], digest(&bytes));
            assert_eq!(file["bytes"], bytes.len().to_string());
        }
        let bytes = fs::read(out.join("results.json")).unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            fs::read(out.join("events.csv")).unwrap(),
            csv(
                result["simulation"]["records"].as_array().unwrap(),
                result["run_id"].as_str().unwrap()
            )
        );
        assert_eq!(
            fs::read(out.join("summary.csv")).unwrap(),
            csv(
                result["simulation"]["summary"].as_array().unwrap(),
                result["run_id"].as_str().unwrap()
            )
        );
        assert_eq!(result["metadata"]["metrics"].as_array().unwrap().len(), 37);
        assert_eq!(
            export(&prepared, &snapshot, &out).unwrap_err().code,
            "E-0003"
        );
        assert_eq!(fs::read(out.join("results.json")).unwrap(), bytes);
        fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn callback_order_is_not_reservation_order_and_effects_are_unique() {
        let (prepared, snapshot) = fixture();
        let (events, _) = records(&prepared, &snapshot).unwrap();
        let sof: Vec<_> = events.iter().filter(|r| r["event_seq"] == "7").collect();
        for (i, row) in sof.iter().enumerate() {
            assert_eq!(row["effect_seq"], i.to_string());
        }
        assert_eq!(sof.len(), 4);
        for (i, row) in events.iter().enumerate() {
            assert_eq!(row["seq"], i.to_string());
        }
    }
    #[test]
    fn zero_time_peak_and_overlapping_occupancy_are_preserved() {
        let (prepared, mut snapshot) = fixture();
        let mut peak = snapshot.points[3].clone();
        peak.value = 2;
        snapshot.points.insert(3, peak);
        let (_, summary) = records(&prepared, &snapshot).unwrap();
        assert_eq!(count(&summary, "Net.a.txQueue", "queue_max"), 2);
        assert_eq!(number(&summary, "Net.a.txQueue", "queue_mean"), 0.1);
        let mut a = snapshot.requests[0].clone();
        a.sof_ps = Some(2);
        a.release_ps = Some(8);
        let mut b = a.clone();
        b.sof_ps = Some(6);
        b.release_ps = Some(15);
        assert_eq!(occupancy(&[a, b], 0, 30, false).unwrap(), 13);
    }

    #[test]
    fn publication_collision_keeps_existing_file_and_omits_manifest() {
        let (prepared, snapshot) = fixture();
        let out = temp_dir();
        fs::write(out.join("events.csv"), b"existing user data").unwrap();
        assert_eq!(
            export(&prepared, &snapshot, &out).unwrap_err().code,
            "E-0003"
        );
        assert_eq!(
            fs::read(out.join("events.csv")).unwrap(),
            b"existing user data"
        );
        assert!(!out.join("manifest.json").exists());
        assert!(fs::read_dir(&out).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp-")
        }));
        fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn metadata_hashes_raw_input_and_canonical_configuration() {
        use crate::types::InputSnapshot;
        let (mut prepared, _) = fixture();
        let directory = temp_dir();
        let input = directory.join("sample.ini");
        let raw = "[General]\n# 日本語\n";
        fs::write(&input, raw).unwrap();
        prepared.inputs.push(InputSnapshot {
            path: input,
            content: raw.into(),
        });
        let metadata = metadata(&prepared, "2026-10-03T00:00:00Z").unwrap();
        assert_eq!(metadata["sources"][0]["logical_path"], "config");
        assert_eq!(metadata["sources"][0]["content_utf8"], raw);
        assert_eq!(metadata["sources"][0]["sha256"], digest(raw.as_bytes()));
        assert_eq!(
            metadata["config_sha256"],
            digest(canonical(&metadata["config"]).as_bytes())
        );
        assert_eq!(
            metadata["input_sha256"],
            digest(
                canonical(&json!([{"logical_path":"config","sha256":digest(raw.as_bytes())}]))
                    .as_bytes()
            )
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn diagnostic_time_is_failed_boundary_not_last_successful_event() {
        let (prepared, mut snapshot) = fixture();
        snapshot.partial = true;
        snapshot.termination = "execution_failed".into();
        snapshot.end_ps = 30;
        snapshot.last_event_time_ps = Some(20);
        snapshot
            .diagnostics
            .push(Diagnostic::execution("event limit reached at next event"));
        let out = temp_dir();
        export(&prepared, &snapshot, &out).unwrap();
        let diagnostic: Value =
            serde_json::from_slice(&fs::read(out.join("diagnostics.jsonl")).unwrap()).unwrap();
        assert_eq!(diagnostic["time_ps"], "30");
        fs::remove_dir_all(out).unwrap();
    }
}
