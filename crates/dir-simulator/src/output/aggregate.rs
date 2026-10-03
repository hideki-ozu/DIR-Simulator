//! Exact common metric aggregation, independent of output formats.
use crate::snapshot::{Point, Request, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use std::collections::BTreeMap;

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
pub(super) fn mul(a: u128, b: u128) -> Result<u128, Diagnostic> {
    a.checked_mul(b).ok_or_else(overflow)
}
/// Convert an exact positive u128 rational to binary64, with a single ties-even rounding.
/// Long division avoids converting either operand to floating point first.
pub(super) fn ratio(n: u128, denominator: u128) -> Result<MetricValue, Diagnostic> {
    if denominator == 0 {
        return Ok(MetricValue::Null);
    }
    if n == 0 {
        return Ok(MetricValue::Number(0.0));
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
    Ok(MetricValue::Number(number))
}

/// Common result values retain exact counts until a writer renders them.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MetricValue {
    Integer(u128),
    Number(f64),
    Null,
}

#[derive(Clone, Debug)]
pub(super) struct Record {
    pub(super) seq: u128,
    pub(super) event_seq: Option<u64>,
    pub(super) effect_seq: Option<u128>,
    pub(super) target: String,
    pub(super) metric: String,
    pub(super) unit: &'static str,
    pub(super) value_kind: &'static str,
    pub(super) value: MetricValue,
    pub(super) time_ps: Option<u64>,
    pub(super) start_ps: Option<u64>,
    pub(super) end_ps: Option<u64>,
    pub(super) request_id: Option<String>,
    pub(super) receiver: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) sample_count: Option<u128>,
    order: usize,
    kind: u8,
}
impl Record {
    pub(super) fn aggregate(
        target: &str,
        metric: &str,
        value: MetricValue,
        start: u64,
        end: u64,
    ) -> Self {
        let (unit, value_kind, _, _) = descriptor(metric);
        Self {
            seq: 0,
            event_seq: None,
            effect_seq: None,
            target: target.into(),
            metric: metric.into(),
            unit,
            value_kind,
            value,
            time_ps: None,
            start_ps: Some(start),
            end_ps: Some(end),
            request_id: None,
            receiver: None,
            reason: None,
            sample_count: None,
            order: 0,
            kind: 0,
        }
    }
    fn point(point: &Point, metric: &str, value: MetricValue, order: usize) -> Self {
        let mut record = Self::aggregate(&point.target, metric, value, 0, 0);
        record.time_ps = Some(point.time_ps);
        record.start_ps = None;
        record.end_ps = None;
        record.event_seq = point.event_seq;
        record.effect_seq = point.effect_seq.map(u128::from);
        record.request_id = point.request_id.clone();
        record.receiver = point.receiver.clone();
        record.reason = point.reason.clone();
        record.order = order;
        record.kind = if point.event_seq.is_some() { 2 } else { 1 };
        record
    }
    fn target(&self) -> &str {
        &self.target
    }
    fn metric(&self) -> &str {
        &self.metric
    }
    fn time(&self) -> u64 {
        self.time_ps.or(self.end_ps).expect("record time")
    }
    fn key(&self) -> (&str, &str, Option<&str>, Option<&str>) {
        (
            self.target(),
            self.metric(),
            self.reason.as_deref(),
            self.receiver.as_deref(),
        )
    }
}

pub(super) fn descriptor(metric: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match metric {
        "gw_copy_created" | "gw_copy_submitted" | "gw_hop_dropped" | "gw_route_filtered"
        | "gw_rx_dropped" => ("count", "integer", "point", "identity"),
        "queue_length" | "gw_rx_queue_length" => ("count", "integer", "point", "identity"),
        "queue_max" | "gw_rx_queue_max" => ("count", "integer", "summary", "max"),
        "tx_wait_ps"
        | "arbitration_wait_ps"
        | "transfer_ps"
        | "delivery_ps"
        | "gw_tx_buffer_wait_ps" => ("ps", "integer", "point", "identity"),
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
pub(super) const METRICS: &str = "queue_length queue_max generated admitted processing pending in_flight success dropped unfinished attempts retries failed receiver_opportunities received filtered rx_pending tx_wait_ps arbitration_wait_ps transfer_ps delivery_ps tx_wait_mean_ps arbitration_wait_mean_ps transfer_mean_ps delivery_mean_ps queue_mean buffer_utilization buffer_utilization_mean bus_utilization frame_utilization payload_bits serialized_bits occupied_bits received_payload_bits payload_throughput_bps serialized_throughput_bps received_throughput_bps";

pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let h = snapshot.common.end_ps;
    let mut points = Vec::new();
    for (i, point) in snapshot.common.points.iter().enumerate() {
        points.push(Record::point(
            point,
            &point.metric,
            MetricValue::Integer(point.value as u128),
            i,
        ));
        if point.metric == "queue_length" {
            let capacity = prepared
                .can
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
        let event = points[offset].event_seq;
        let mut end = offset + 1;
        while end < points.len() && points[end].event_seq == event {
            end += 1;
        }
        if event.is_some() {
            points[offset..end].sort_by(|a, b| {
                (
                    a.target(),
                    a.metric(),
                    a.request_id.as_deref(),
                    a.receiver.as_deref(),
                )
                    .cmp(&(
                        b.target(),
                        b.metric(),
                        b.request_id.as_deref(),
                        b.receiver.as_deref(),
                    ))
            });
            for (effect, point) in points[offset..end].iter_mut().enumerate() {
                point.order = group;
                point.effect_seq = Some(effect as u128);
            }
        }
        offset = end;
        group += 1;
    }
    if h == 0 && !snapshot.common.partial {
        points.clear();
    }
    let mut summary = Vec::new();
    let mut targets = prepared
        .can
        .controllers
        .iter()
        .map(|c| c.id.clone())
        .collect::<Vec<_>>();
    targets.extend(prepared.can.buses.iter().map(|b| b.id.clone()));
    let bus_ids: Vec<_> = prepared.can.buses.iter().map(|b| b.id.as_str()).collect();
    let request_buses: BTreeMap<_, _> = snapshot
        .can
        .requests
        .iter()
        .map(|r| (r.request_id.as_str(), r.bus.as_str()))
        .collect();
    targets.push("$all".into());
    for target in &targets {
        let reqs: Vec<_> = snapshot
            .can
            .requests
            .iter()
            .filter(|r| target == "$all" || r.source == *target || r.bus == *target)
            .collect();
        let count = |status: &str| reqs.iter().filter(|r| r.status == status).count() as u128;
        let processing = count("processing");
        let waiting_tx = count("waiting_tx");
        let pending = count("pending");
        let flight = count("in_flight");
        let success = count("success");
        let dropped = count("dropped");
        let admitted = add(add(pending, flight)?, success)?;
        let unfinished = add(add(add(processing, waiting_tx)?, pending)?, flight)?;
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
            let mut record = Record::aggregate(target, metric, MetricValue::Integer(count), 0, h);
            if metric == "dropped" {
                record.reason = Some("queue_full".into());
            }
            summary.push(record);
        }
        if prepared.common.profile != "can.cc.ideal.v1" {
            summary.push(Record::aggregate(
                target,
                "waiting_tx",
                MetricValue::Integer(waiting_tx),
                0,
                h,
            ));
        }
        let receivers: Vec<_> = snapshot
            .can
            .receivers
            .iter()
            .filter(|r| {
                target == "$all"
                    || r.receiver == *target
                    || (bus_ids.contains(&target.as_str())
                        && request_buses.get(r.request_id.as_str()) == Some(&target.as_str()))
            })
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
            let mut record = Record::aggregate(target, metric, MetricValue::Integer(count), 0, h);
            if target != "$all" && !bus_ids.contains(&target.as_str()) {
                record.receiver = Some(target.clone());
            }
            summary.push(record);
        }
        for (metric, point_metric) in [
            ("tx_wait_mean_ps", "tx_wait_ps"),
            ("arbitration_wait_mean_ps", "arbitration_wait_ps"),
            ("transfer_mean_ps", "transfer_ps"),
            ("delivery_mean_ps", "delivery_ps"),
        ] {
            if metric == "transfer_mean_ps"
                && target != "$all"
                && !bus_ids.contains(&target.as_str())
            {
                continue;
            }
            let mut sum = 0;
            let mut count = 0;
            for point in &snapshot.common.points {
                if point.metric == point_metric
                    && (target == "$all"
                        || point.target == *target
                        || (bus_ids.contains(&target.as_str())
                            && point
                                .request_id
                                .as_deref()
                                .and_then(|id| request_buses.get(id))
                                == Some(&target.as_str())))
                {
                    sum = add(sum, point.value as u128)?;
                    count = add(count, 1)?;
                }
            }
            let mut record = Record::aggregate(target, metric, ratio(sum, count)?, 0, h);
            record.sample_count = Some(count);
            if metric == "delivery_mean_ps"
                && target != "$all"
                && !bus_ids.contains(&target.as_str())
            {
                record.receiver = Some(target.clone());
            }
            summary.push(record);
        }
    }
    for (index, gateway) in prepared.gateway.gateways.iter().enumerate() {
        for &port in &gateway.ports {
            let queue = format!("{}.rxQueue", prepared.can.controllers[port].id);
            let maximum = snapshot
                .common
                .points
                .iter()
                .filter(|p| p.target == queue && p.metric == "gw_rx_queue_length")
                .map(|p| p.value)
                .max()
                .unwrap_or(0);
            summary.push(Record::aggregate(
                &queue,
                "gw_rx_queue_max",
                MetricValue::Integer(maximum as u128),
                0,
                h,
            ));
        }
        summary.push(Record::aggregate(
            &gateway.node,
            "gw_processing_pending",
            MetricValue::Integer(
                snapshot
                    .gateway
                    .forwards
                    .iter()
                    .filter(|f| f.gateway == index && f.status == "processing")
                    .count() as u128,
            ),
            0,
            h,
        ));
    }
    for c in &prepared.can.controllers {
        let queue = format!("{}.txQueue", c.id);
        let qpoints: Vec<_> = snapshot
            .common
            .points
            .iter()
            .filter(|p| p.target == queue && p.metric == "queue_length")
            .collect();
        let max = qpoints.iter().map(|p| p.value).max().unwrap_or(0);
        let integral = queue_integral(&qpoints, 0, h)?;
        summary.push(Record::aggregate(
            &queue,
            "queue_max",
            MetricValue::Integer(max as u128),
            0,
            h,
        ));
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
            MetricValue::Integer(
                snapshot
                    .can
                    .requests
                    .iter()
                    .filter(|r| r.source == c.id && r.status == "dropped")
                    .count() as u128,
            ),
            0,
            h,
        );
        drop.reason = Some("queue_full".into());
        summary.push(drop);
    }
    let mut start = 0;
    while start < h {
        let end = start
            .saturating_add(prepared.common.metrics_window_ps)
            .min(h);
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
                        a.effect_seq
                            .expect("event effect")
                            .cmp(&b.effect_seq.expect("event effect"))
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
                r.seq = i as u128;
                r
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

pub(super) fn occupancy(
    requests: &[Request],
    start: u64,
    end: u64,
    frame: bool,
) -> Result<u128, Diagnostic> {
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
    for bus in &prepared.can.buses {
        let bus_requests: Vec<_> = snapshot
            .can
            .requests
            .iter()
            .filter(|r| r.bus == bus.id)
            .cloned()
            .collect();
        let mut payload = 0;
        let mut serialized = 0;
        let mut occupied = 0;
        for r in &bus_requests {
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
                ratio(occupancy(&bus_requests, start, end, false)?, length)?,
            ),
            (
                "frame_utilization",
                ratio(occupancy(&bus_requests, start, end, true)?, length)?,
            ),
            ("payload_bits", MetricValue::Integer(payload)),
            ("serialized_bits", MetricValue::Integer(serialized)),
            ("occupied_bits", MetricValue::Integer(occupied)),
            (
                "payload_throughput_bps",
                ratio(mul(payload, 1_000_000_000_000)?, length)?,
            ),
            (
                "serialized_throughput_bps",
                ratio(mul(serialized, 1_000_000_000_000)?, length)?,
            ),
        ] {
            rows.push(Record::aggregate(&bus.id, metric, value, start, end));
        }
    }
    let lookup: BTreeMap<_, _> = snapshot
        .can
        .requests
        .iter()
        .map(|r| (r.request_id.as_str(), r))
        .collect();
    for c in &prepared.can.controllers {
        let mut bits = 0;
        for receiver in &snapshot.can.receivers {
            if receiver.receiver == c.id && completed(receiver.received_ps) {
                let request = lookup
                    .get(receiver.request_id.as_str())
                    .ok_or_else(|| Diagnostic::output("Receiver refers to an unknown request"))?;
                bits = add(bits, request.payload_bits as u128)?;
            }
        }
        for (metric, value) in [
            ("received_payload_bits", MetricValue::Integer(bits)),
            (
                "received_throughput_bps",
                ratio(mul(bits, 1_000_000_000_000)?, length)?,
            ),
        ] {
            let mut row = Record::aggregate(&c.id, metric, value, start, end);
            row.receiver = Some(c.id.clone());
            rows.push(row);
        }
        let queue = format!("{}.txQueue", c.id);
        let qpoints: Vec<_> = snapshot
            .common
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
