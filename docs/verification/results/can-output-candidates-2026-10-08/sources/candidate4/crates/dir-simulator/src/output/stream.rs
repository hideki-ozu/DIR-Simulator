//! Archive-backed CAN/Gateway export. Its live state scales with topology and sort chunk size.
use super::{
    PROFILE,
    aggregate::{self, MetricValue, Record},
    contribution_sort::{self, Kind},
    disk_sort::{Sorted, Sorter},
    json, model_records,
};
use crate::{
    Diagnostic, allocation::reserve_vec, runtime::can_archive::ArchivedRequest, snapshot::Snapshot,
    types::PreparedSimulation,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default, Clone)]
struct Stats {
    requests: u128,
    processing: u128,
    waiting_tx: u128,
    pending: u128,
    in_flight: u128,
    success: u128,
    dropped: u128,
    rx: u128,
    received: u128,
    filtered: u128,
    rx_pending: u128,
    payload_bits: u128,
    serialized_bits: u128,
    occupied_bits: u128,
    received_bits: u128,
    delays: [(u128, u128); 4],
}
impl Stats {
    fn request(&mut self, status: &str) -> Result<(), Diagnostic> {
        self.requests = aggregate::add(self.requests, 1)?;
        let slot = match status {
            "processing" => Some(&mut self.processing),
            "waiting_tx" => Some(&mut self.waiting_tx),
            "pending" => Some(&mut self.pending),
            "in_flight" => Some(&mut self.in_flight),
            "success" => Some(&mut self.success),
            "dropped" => Some(&mut self.dropped),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = aggregate::add(*slot, 1)?;
        }
        Ok(())
    }
    fn receiver(&mut self, status: &str) -> Result<(), Diagnostic> {
        self.rx = aggregate::add(self.rx, 1)?;
        let slot = match status {
            "received" => Some(&mut self.received),
            "filtered" => Some(&mut self.filtered),
            "pending" => Some(&mut self.rx_pending),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = aggregate::add(*slot, 1)?;
        }
        Ok(())
    }
    fn delay(&mut self, index: usize, value: u64) -> Result<(), Diagnostic> {
        let slot = &mut self.delays[index];
        slot.0 = aggregate::add(slot.0, value as u128)?;
        slot.1 = aggregate::add(slot.1, 1)?;
        Ok(())
    }
}

pub(super) struct Prepared {
    pub records: Sorted,
    pub models: Option<Sorted>,
    pub requests: Option<Sorted>,
    pub receivers: Option<Sorted>,
    pub summary: Vec<Record>,
}

/// Stable IDs for every CAN timeline target, including both queue endpoints.
struct Topology {
    ids: BTreeMap<String, u64>,
    buses: Vec<usize>,
    controllers: Vec<usize>,
    tx_queues: Vec<usize>,
    tx_names: Vec<String>,
    rx_names: Vec<String>,
}
impl Topology {
    fn new(prepared: &PreparedSimulation) -> Result<Self, Diagnostic> {
        let mut topology = Self {
            ids: BTreeMap::new(),
            buses: Vec::new(),
            controllers: Vec::new(),
            tx_queues: Vec::new(),
            tx_names: Vec::new(),
            rx_names: Vec::new(),
        };
        for bus in &prepared.can.buses {
            let id = topology.insert(bus.id.clone())?;
            reserve_vec(&mut topology.buses, 1, "output_topology")?;
            topology.buses.push(id);
        }
        for controller in &prepared.can.controllers {
            let id = topology.insert(controller.id.clone())?;
            reserve_vec(&mut topology.controllers, 1, "output_topology")?;
            topology.controllers.push(id);
            let tx = format!("{}.txQueue", controller.id);
            let rx = format!("{}.rxQueue", controller.id);
            let tx_id = topology.insert(tx.clone())?;
            topology.insert(rx.clone())?;
            reserve_vec(&mut topology.tx_queues, 1, "output_topology")?;
            reserve_vec(&mut topology.tx_names, 1, "output_topology")?;
            reserve_vec(&mut topology.rx_names, 1, "output_topology")?;
            topology.tx_queues.push(tx_id);
            topology.tx_names.push(tx);
            topology.rx_names.push(rx);
        }
        Ok(topology)
    }
    fn insert(&mut self, name: String) -> Result<usize, Diagnostic> {
        let index = self.ids.len();
        let wire = u64::try_from(index).map_err(|_| Diagnostic::output("Topology too large"))?;
        if self.ids.insert(name, wire).is_some() {
            return Err(Diagnostic::output("Duplicate timeline target"));
        }
        Ok(index)
    }
    fn id(&self, target: &str) -> Result<u64, Diagnostic> {
        self.ids
            .get(target)
            .copied()
            .ok_or_else(|| Diagnostic::output("Unknown contribution target"))
    }
}

fn time_key(t: u64) -> String {
    format!("{t:016x}")
}
fn option_key(value: Option<&str>) -> String {
    match value {
        None => "0".into(),
        Some(value) => format!("1{value}"),
    }
}
fn record_key(row: &Record, callback_group: Option<(u64, usize)>) -> Vec<String> {
    let mut key = vec![
        time_key(row.time_ps.or(row.end_ps).unwrap()),
        if row.time_ps.is_none() {
            "0"
        } else if row.event_seq.is_none() {
            "1"
        } else {
            "2"
        }
        .into(),
    ];
    if let Some((group, effect)) = callback_group {
        key.push(time_key(group));
        key.push(time_key(effect as u64));
    } else {
        key.extend([
            row.target.clone(),
            row.metric.clone(),
            option_key(row.reason.as_deref()),
            option_key(row.receiver.as_deref()),
        ]);
    }
    key
}
fn put_record(
    sorter: &mut Sorter,
    row: &Record,
    callback_group: Option<(u64, usize)>,
) -> Result<(), Diagnostic> {
    sorter.push(record_key(row, callback_group), json::record_value(row))
}
fn contribution(
    sorter: &mut contribution_sort::Sorter,
    topology: &Topology,
    time: u64,
    kind: Kind,
    target: &str,
    value: i128,
) -> Result<(), Diagnostic> {
    sorter.push(time, topology.id(target)?, kind, value)
}
struct BundleSinks<'a> {
    models: &'a mut Option<Sorter>,
    requests: &'a mut Option<Sorter>,
    receivers: &'a mut Option<Sorter>,
    contributions: &'a mut contribution_sort::Sorter,
    topology: &'a Topology,
    stats: &'a mut BTreeMap<String, Stats>,
    gateway_pending: &'a mut [u128],
}
fn one(
    prepared: &PreparedSimulation,
    row: ArchivedRequest,
    version: u32,
    sinks: &mut BundleSinks<'_>,
    h: u64,
) -> Result<(), Diagnostic> {
    let (models, requests, receivers, contributions, stats, gateway_pending) = (
        &mut *sinks.models,
        &mut *sinks.requests,
        &mut *sinks.receivers,
        &mut *sinks.contributions,
        &mut *sinks.stats,
        &mut *sinks.gateway_pending,
    );
    let topology = sinks.topology;
    let r = &row.request;
    for target in [&r.source, &r.bus, &String::from("$all")] {
        stats
            .entry(target.clone())
            .or_default()
            .request(&r.status)?;
    }
    if let Some(sof) = r.sof_ps {
        let tx = sof
            .checked_sub(r.generated_ps)
            .ok_or_else(|| Diagnostic::output("Invalid TX wait"))?;
        let arb = sof
            .checked_sub(
                r.tx_enqueued_ps
                    .ok_or_else(|| Diagnostic::output("Missing TX enqueue"))?,
            )
            .ok_or_else(|| Diagnostic::output("Invalid arbitration wait"))?;
        for target in [&r.source, &r.bus, &String::from("$all")] {
            stats.entry(target.clone()).or_default().delay(0, tx)?;
            stats.entry(target.clone()).or_default().delay(1, arb)?;
        }
        if sof < h {
            contribution(contributions, topology, sof, Kind::Busy, &r.bus, 1)?;
            contribution(contributions, topology, sof, Kind::Frame, &r.bus, 1)?;
            let end_busy = r.release_ps.or(r.planned_release_ps).unwrap_or(h).min(h);
            let end_frame = r.eof_ps.or(r.planned_eof_ps).unwrap_or(h).min(h);
            contribution(contributions, topology, end_busy, Kind::Busy, &r.bus, -1)?;
            contribution(contributions, topology, end_frame, Kind::Frame, &r.bus, -1)?;
        }
    }
    if let Some(eof) = r.eof_ps {
        let bus = stats.entry(r.bus.clone()).or_default();
        bus.payload_bits = aggregate::add(bus.payload_bits, r.payload_bits as u128)?;
        bus.serialized_bits = aggregate::add(bus.serialized_bits, r.frame_bits as u128)?;
        for target in [&r.bus, &String::from("$all")] {
            let transfer = eof
                .checked_sub(r.sof_ps.ok_or_else(|| Diagnostic::output("Missing SOF"))?)
                .ok_or_else(|| Diagnostic::output("Invalid transfer delay"))?;
            stats
                .entry(target.clone())
                .or_default()
                .delay(2, transfer)?;
        }
        if eof < h {
            contribution(
                contributions,
                topology,
                eof,
                Kind::Payload,
                &r.bus,
                r.payload_bits as i128,
            )?;
            contribution(
                contributions,
                topology,
                eof,
                Kind::Serialized,
                &r.bus,
                r.frame_bits as i128,
            )?;
        }
    }
    if let Some(release) = r.release_ps {
        let bus = stats.entry(r.bus.clone()).or_default();
        bus.occupied_bits = aggregate::add(bus.occupied_bits, r.frame_bits as u128 + 3)?;
        if release < h {
            contribution(
                contributions,
                topology,
                release,
                Kind::Occupied,
                &r.bus,
                r.frame_bits as i128 + 3,
            )?;
        }
    }
    for receiver in &row.receivers {
        for target in [&receiver.receiver, &r.bus, &String::from("$all")] {
            stats
                .entry(target.clone())
                .or_default()
                .receiver(&receiver.status)?;
        }
        if let Some(t) = receiver.received_ps {
            let receiver_stats = stats.entry(receiver.receiver.clone()).or_default();
            receiver_stats.received_bits =
                aggregate::add(receiver_stats.received_bits, r.payload_bits as u128)?;
            for target in [&receiver.receiver, &r.bus, &String::from("$all")] {
                let delivery = t
                    .checked_sub(r.generated_ps)
                    .ok_or_else(|| Diagnostic::output("Invalid delivery delay"))?;
                stats
                    .entry(target.clone())
                    .or_default()
                    .delay(3, delivery)?;
            }
            if t < h {
                contribution(
                    contributions,
                    topology,
                    t,
                    Kind::Received,
                    &receiver.receiver,
                    r.payload_bits as i128,
                )?;
            }
        }
        if version == 1 {
            receivers.as_mut().unwrap().push(vec![r.request_id.clone(), receiver.receiver.clone()],
                json!({"request_id":receiver.request_id,"receiver":receiver.receiver,"status":receiver.status,"observed_ps":json::opt_d(receiver.observed_ps),"received_ps":json::opt_d(receiver.received_ps)}))?;
        }
    }
    for f in &row.forwards {
        if f.status == "processing" {
            let pending = gateway_pending
                .get_mut(f.gateway)
                .ok_or_else(|| Diagnostic::output("Unknown Gateway in archived forward"))?;
            *pending = aggregate::add(*pending, 1)?;
        }
    }
    if version == 1 {
        requests
            .as_mut()
            .unwrap()
            .push(vec![r.request_id.clone()], model_records::request_json(r))?;
    } else {
        let request_id = r.request_id.clone();
        let mut mini = Snapshot::empty(prepared);
        mini.can.requests.push(row.request);
        mini.can.receivers = row.receivers;
        mini.gateway.forwards = row.forwards;
        mini.gateway.rx_buffers = row.rx_buffers;
        mini.gateway.request_lineage.insert(request_id, row.lineage);
        for value in model_records::records(prepared, &mini)? {
            models.as_mut().unwrap().push(
                vec![
                    value["schema_name"].as_str().unwrap_or_default().into(),
                    value["subject"].as_str().unwrap_or_default().into(),
                    value["record_id"].as_str().unwrap_or_default().into(),
                ],
                value,
            )?;
        }
    }
    Ok(())
}

fn emit_point_group(
    sorter: &mut Sorter,
    group: &mut Vec<Record>,
    callback: u64,
) -> Result<(), Diagnostic> {
    if group.is_empty() {
        return Ok(());
    }
    if group[0].event_seq.is_some() {
        group.sort_by(|a, b| {
            (
                &a.target,
                &a.metric,
                a.request_id.as_deref(),
                a.receiver.as_deref(),
            )
                .cmp(&(
                    &b.target,
                    &b.metric,
                    b.request_id.as_deref(),
                    b.receiver.as_deref(),
                ))
        });
        for (effect, row) in group.iter_mut().enumerate() {
            row.effect_seq = Some(effect as u128);
            put_record(sorter, row, Some((callback, effect)))?;
        }
    } else {
        for row in group.iter() {
            put_record(sorter, row, None)?;
        }
    }
    group.clear();
    Ok(())
}

fn points(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    sorter: &mut Sorter,
    contributions: &mut contribution_sort::Sorter,
    topology: &Topology,
    queue_max: &mut BTreeMap<String, u64>,
) -> Result<(), Diagnostic> {
    let mut group = Vec::new();
    let mut previous_event: Option<Option<u64>> = None;
    let mut callback = 0u64;
    let skip = snapshot.common.end_ps == 0 && !snapshot.common.partial;
    for (index, point) in snapshot.common.iter_points()?.enumerate() {
        let point = point?;
        if previous_event.is_some_and(|previous| previous != point.event_seq) {
            emit_point_group(sorter, &mut group, callback)?;
            callback = callback
                .checked_add(1)
                .ok_or_else(|| Diagnostic::output("Point callback count overflow"))?;
        }
        previous_event = Some(point.event_seq);
        if point.metric == "queue_length" || point.metric == "gw_rx_queue_length" {
            let entry = queue_max.entry(point.target.clone()).or_default();
            *entry = (*entry).max(point.value);
            if point.time_ps < snapshot.common.end_ps {
                contribution(
                    contributions,
                    topology,
                    point.time_ps,
                    Kind::Queue,
                    &point.target,
                    point.value as i128,
                )?;
            }
        }
        if skip {
            continue;
        }
        reserve_vec(&mut group, 1, "output_point_group")?;
        group.push(Record::point(
            &point,
            &point.metric,
            MetricValue::Integer(point.value as u128),
            index,
        ));
        if point.metric == "queue_length" {
            let controller = point
                .target
                .strip_suffix(".txQueue")
                .and_then(|id| prepared.can.controllers.iter().find(|c| c.id == id))
                .ok_or_else(|| Diagnostic::output("Queue point refers to an unknown queue"))?;
            reserve_vec(&mut group, 1, "output_point_group")?;
            group.push(Record::point(
                &point,
                "buffer_utilization",
                aggregate::ratio(point.value as u128, controller.queue_capacity as u128)?,
                index,
            ));
        }
    }
    emit_point_group(sorter, &mut group, callback)
}

#[derive(Default)]
struct Timeline {
    at: u64,
    busy: i128,
    frame: i128,
    queue: u64,
    busy_window: u128,
    frame_window: u128,
    queue_window: u128,
    busy_total: u128,
    frame_total: u128,
    queue_total: u128,
    payload: u128,
    serialized: u128,
    occupied: u128,
    received: u128,
}
impl Timeline {
    fn advance(&mut self, time: u64) -> Result<(), Diagnostic> {
        let duration = time
            .checked_sub(self.at)
            .ok_or_else(|| Diagnostic::output("Metric timeline is not chronological"))?
            as u128;
        let busy = if self.busy > 0 { duration } else { 0 };
        let frame = if self.frame > 0 { duration } else { 0 };
        let queue = aggregate::mul(self.queue as u128, duration)?;
        self.busy_window = aggregate::add(self.busy_window, busy)?;
        self.frame_window = aggregate::add(self.frame_window, frame)?;
        self.queue_window = aggregate::add(self.queue_window, queue)?;
        self.busy_total = aggregate::add(self.busy_total, busy)?;
        self.frame_total = aggregate::add(self.frame_total, frame)?;
        self.queue_total = aggregate::add(self.queue_total, queue)?;
        self.at = time;
        Ok(())
    }
    fn reset(&mut self) {
        self.busy_window = 0;
        self.frame_window = 0;
        self.queue_window = 0;
        self.payload = 0;
        self.serialized = 0;
        self.occupied = 0;
        self.received = 0;
    }
}

fn window_rows(
    prepared: &PreparedSimulation,
    topology: &Topology,
    state: &[Timeline],
    stats: &BTreeMap<String, Stats>,
    start: u64,
    end: u64,
    full: bool,
) -> Result<Vec<Record>, Diagnostic> {
    let mut rows = Vec::new();
    let length = (end - start) as u128;
    for (bus_index, bus) in prepared.can.buses.iter().enumerate() {
        let s = &state[topology.buses[bus_index]];
        let st = |metric, value| Record::aggregate(&bus.id, metric, value, start, end);
        let payload = if full {
            stats[&bus.id].payload_bits
        } else {
            s.payload
        };
        let serialized = if full {
            stats[&bus.id].serialized_bits
        } else {
            s.serialized
        };
        let occupied = if full {
            stats[&bus.id].occupied_bits
        } else {
            s.occupied
        };
        rows.extend([
            st(
                "bus_utilization",
                aggregate::ratio(if full { s.busy_total } else { s.busy_window }, length)?,
            ),
            st(
                "frame_utilization",
                aggregate::ratio(if full { s.frame_total } else { s.frame_window }, length)?,
            ),
            st("payload_bits", MetricValue::Integer(payload)),
            st("serialized_bits", MetricValue::Integer(serialized)),
            st("occupied_bits", MetricValue::Integer(occupied)),
            st(
                "payload_throughput_bps",
                aggregate::ratio(aggregate::mul(payload, 1_000_000_000_000)?, length)?,
            ),
            st(
                "serialized_throughput_bps",
                aggregate::ratio(aggregate::mul(serialized, 1_000_000_000_000)?, length)?,
            ),
        ]);
    }
    for (controller_index, c) in prepared.can.controllers.iter().enumerate() {
        let s = &state[topology.controllers[controller_index]];
        let bits = if full {
            stats[&c.id].received_bits
        } else {
            s.received
        };
        let mut row = Record::aggregate(
            &c.id,
            "received_payload_bits",
            MetricValue::Integer(bits),
            start,
            end,
        );
        row.receiver = Some(c.id.clone());
        rows.push(row);
        let mut row = Record::aggregate(
            &c.id,
            "received_throughput_bps",
            aggregate::ratio(aggregate::mul(bits, 1_000_000_000_000)?, length)?,
            start,
            end,
        );
        row.receiver = Some(c.id.clone());
        rows.push(row);
        let queue = &topology.tx_names[controller_index];
        rows.push(Record::aggregate(
            queue,
            "buffer_utilization_mean",
            aggregate::ratio(
                if full {
                    state[topology.tx_queues[controller_index]].queue_total
                } else {
                    state[topology.tx_queues[controller_index]].queue_window
                },
                aggregate::mul(c.queue_capacity as u128, length)?,
            )?,
            start,
            end,
        ));
    }
    Ok(rows)
}

fn windows(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    topology: &Topology,
    contributions: contribution_sort::Sorted,
    records: &mut Sorter,
    stats: &BTreeMap<String, Stats>,
) -> Result<Vec<Timeline>, Diagnostic> {
    let h = snapshot.common.end_ps;
    let mut state = Vec::new();
    reserve_vec(&mut state, topology.ids.len(), "output_timelines")?;
    state.resize_with(topology.ids.len(), Timeline::default);
    let mut iter = contributions.iter()?;
    let mut next = iter.next().transpose()?;
    let mut start = 0;
    while start < h {
        let end = start
            .saturating_add(prepared.common.metrics_window_ps)
            .min(h);
        if end == start {
            return Err(Diagnostic::output("Metric window must be positive"));
        }
        while let Some(row) = next {
            if row.time >= end {
                break;
            }
            let index = usize::try_from(row.target)
                .map_err(|_| Diagnostic::output("Unknown contribution target"))?;
            let timeline = state
                .get_mut(index)
                .ok_or_else(|| Diagnostic::output("Unknown contribution target"))?;
            timeline.advance(row.time)?;
            match row.kind {
                Kind::Busy => {
                    timeline.busy = timeline
                        .busy
                        .checked_add(row.amount)
                        .filter(|value| *value >= 0)
                        .ok_or_else(|| Diagnostic::output("Invalid bus occupancy contribution"))?;
                }
                Kind::Frame => {
                    timeline.frame = timeline
                        .frame
                        .checked_add(row.amount)
                        .filter(|value| *value >= 0)
                        .ok_or_else(|| {
                            Diagnostic::output("Invalid frame occupancy contribution")
                        })?;
                }
                Kind::Queue => {
                    timeline.queue = u64::try_from(row.amount)
                        .map_err(|_| Diagnostic::output("Invalid queue value"))?;
                }
                Kind::Payload => {
                    timeline.payload = aggregate::add(timeline.payload, row.amount as u128)?
                }
                Kind::Serialized => {
                    timeline.serialized = aggregate::add(timeline.serialized, row.amount as u128)?
                }
                Kind::Occupied => {
                    timeline.occupied = aggregate::add(timeline.occupied, row.amount as u128)?
                }
                Kind::Received => {
                    timeline.received = aggregate::add(timeline.received, row.amount as u128)?
                }
            }
            next = iter.next().transpose()?;
        }
        for value in &mut state {
            value.advance(end)?;
        }
        for row in window_rows(prepared, topology, &state, stats, start, end, false)? {
            put_record(records, &row, None)?;
        }
        for value in &mut state {
            value.reset();
        }
        start = end;
    }
    Ok(state)
}

fn summaries(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    topology: &Topology,
    stats: &BTreeMap<String, Stats>,
    gateway_pending: &[u128],
    queue_max: &BTreeMap<String, u64>,
    state: &[Timeline],
) -> Result<Vec<Record>, Diagnostic> {
    let h = snapshot.common.end_ps;
    let mut rows = Vec::new();
    let mut targets: Vec<_> = prepared
        .can
        .controllers
        .iter()
        .map(|c| c.id.clone())
        .collect();
    targets.extend(prepared.can.buses.iter().map(|b| b.id.clone()));
    targets.push("$all".into());
    for target in &targets {
        let s = &stats[target];
        let admitted = aggregate::add(aggregate::add(s.pending, s.in_flight)?, s.success)?;
        let unfinished = aggregate::add(
            aggregate::add(aggregate::add(s.processing, s.waiting_tx)?, s.pending)?,
            s.in_flight,
        )?;
        for (metric, count) in [
            ("generated", s.requests),
            ("admitted", admitted),
            ("processing", s.processing),
            ("pending", s.pending),
            ("in_flight", s.in_flight),
            ("success", s.success),
            ("dropped", s.dropped),
            ("unfinished", unfinished),
            ("attempts", aggregate::add(s.in_flight, s.success)?),
            ("retries", 0),
            ("failed", 0),
        ] {
            let mut row = Record::aggregate(target, metric, MetricValue::Integer(count), 0, h);
            if metric == "dropped" {
                row.reason = Some("queue_full".into());
            }
            rows.push(row);
        }
        if prepared.common.profile != PROFILE {
            rows.push(Record::aggregate(
                target,
                "waiting_tx",
                MetricValue::Integer(s.waiting_tx),
                0,
                h,
            ));
        }
        let controller = prepared.can.controllers.iter().any(|c| c.id == *target);
        for (metric, count) in [
            ("receiver_opportunities", s.rx),
            ("received", s.received),
            ("filtered", s.filtered),
            ("rx_pending", s.rx_pending),
        ] {
            let mut row = Record::aggregate(target, metric, MetricValue::Integer(count), 0, h);
            if controller {
                row.receiver = Some(target.clone());
            }
            rows.push(row);
        }
        for (index, metric) in [
            "tx_wait_mean_ps",
            "arbitration_wait_mean_ps",
            "transfer_mean_ps",
            "delivery_mean_ps",
        ]
        .iter()
        .enumerate()
        {
            if controller && index == 2 {
                continue;
            }
            let (sum, count) = s.delays[index];
            let mut row = Record::aggregate(target, metric, aggregate::ratio(sum, count)?, 0, h);
            row.sample_count = Some(count);
            if controller && index == 3 {
                row.receiver = Some(target.clone());
            }
            rows.push(row);
        }
    }
    for (index, gateway) in prepared.gateway.gateways.iter().enumerate() {
        for &port in &gateway.ports {
            let queue = &topology.rx_names[port];
            rows.push(Record::aggregate(
                queue,
                "gw_rx_queue_max",
                MetricValue::Integer(*queue_max.get(queue).unwrap_or(&0) as u128),
                0,
                h,
            ));
        }
        rows.push(Record::aggregate(
            &gateway.node,
            "gw_processing_pending",
            MetricValue::Integer(gateway_pending[index]),
            0,
            h,
        ));
    }
    for (index, c) in prepared.can.controllers.iter().enumerate() {
        let queue = &topology.tx_names[index];
        rows.push(Record::aggregate(
            queue,
            "queue_max",
            MetricValue::Integer(*queue_max.get(queue).unwrap_or(&0) as u128),
            0,
            h,
        ));
        rows.push(Record::aggregate(
            queue,
            "queue_mean",
            aggregate::ratio(state[topology.tx_queues[index]].queue_total, h as u128)?,
            0,
            h,
        ));
        let mut row = Record::aggregate(
            queue,
            "dropped",
            MetricValue::Integer(stats[&c.id].dropped),
            0,
            h,
        );
        row.reason = Some("queue_full".into());
        rows.push(row);
    }
    rows.extend(window_rows(prepared, topology, state, stats, 0, h, true)?);
    rows.sort_by(|a, b| {
        (
            &a.target,
            &a.metric,
            a.reason.as_deref(),
            a.receiver.as_deref(),
        )
            .cmp(&(
                &b.target,
                &b.metric,
                b.reason.as_deref(),
                b.receiver.as_deref(),
            ))
    });
    for (index, row) in rows.iter_mut().enumerate() {
        row.seq = index as u128;
    }
    Ok(rows)
}

pub(super) fn prepare(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    output: &Path,
) -> Result<Prepared, Diagnostic> {
    let archive = snapshot
        .can
        .archive
        .as_ref()
        .ok_or_else(|| Diagnostic::output("Missing CAN archive"))?;
    let version = if prepared.common.profile == PROFILE {
        1
    } else {
        2
    };
    let mut models = if version == 2 {
        Some(Sorter::new_in(output)?)
    } else {
        None
    };
    let mut requests = if version == 1 {
        Some(Sorter::new_in(output)?)
    } else {
        None
    };
    let mut receivers = if version == 1 {
        Some(Sorter::new_in(output)?)
    } else {
        None
    };
    let topology = Topology::new(prepared)?;
    let mut contributions = contribution_sort::Sorter::new_in(output)?;
    let mut records = Sorter::new_in(output)?;
    let mut stats = BTreeMap::new();
    for c in &prepared.can.controllers {
        stats.insert(c.id.clone(), Stats::default());
    }
    for bus in &prepared.can.buses {
        stats.insert(bus.id.clone(), Stats::default());
    }
    stats.insert("$all".into(), Stats::default());
    let mut gateway_pending = vec![0; prepared.gateway.gateways.len()];
    for bundle in archive.iter()? {
        one(
            prepared,
            bundle?,
            version,
            &mut BundleSinks {
                models: &mut models,
                requests: &mut requests,
                receivers: &mut receivers,
                contributions: &mut contributions,
                topology: &topology,
                stats: &mut stats,
                gateway_pending: &mut gateway_pending,
            },
            snapshot.common.end_ps,
        )?;
    }
    let mut queue_max = BTreeMap::new();
    points(
        prepared,
        snapshot,
        &mut records,
        &mut contributions,
        &topology,
        &mut queue_max,
    )?;
    let state = windows(
        prepared,
        snapshot,
        &topology,
        contributions.finish()?,
        &mut records,
        &stats,
    )?;
    let summary = summaries(
        prepared,
        snapshot,
        &topology,
        &stats,
        &gateway_pending,
        &queue_max,
        &state,
    )?;
    Ok(Prepared {
        records: records.finish()?,
        models: models.map(Sorter::finish).transpose()?,
        requests: requests.map(Sorter::finish).transpose()?,
        receivers: receivers.map(Sorter::finish).transpose()?,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dir-stream-output-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn prepared() -> PreparedSimulation {
        let config =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/can/baseline.ini");
        crate::prepare(&config).unwrap()
    }
    #[test]
    fn topology_ids_cover_queues_and_unknown_targets_are_diagnostic() {
        let prepared = prepared();
        let topology = Topology::new(&prepared).unwrap();
        for bus in &prepared.can.buses {
            assert!(topology.id(&bus.id).is_ok());
        }
        for controller in &prepared.can.controllers {
            assert!(topology.id(&controller.id).is_ok());
            assert!(topology.id(&format!("{}.txQueue", controller.id)).is_ok());
            assert!(topology.id(&format!("{}.rxQueue", controller.id)).is_ok());
        }
        assert!(topology.id("unknown timeline target").is_err());
    }
    #[test]
    fn failed_sort_reservation_cleans_runs_without_publishing_manifest() {
        let output = directory();
        let prepared = prepared();
        let snapshot =
            crate::runtime::spool::with_spooling(&output, || crate::runtime::simulate(&prepared))
                .unwrap();
        assert!(snapshot.can.archive.is_some());
        crate::allocation::fail_next_reservation("output_sort_chunk");
        let error = super::super::export(&prepared, &snapshot, &output).unwrap_err();
        assert_eq!(error.reason, "allocation_failed");
        assert!(!output.join("manifest.json").exists());
        assert!(!fs::read_dir(&output).unwrap().any(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name.starts_with(".dir-result-sort-") || name.starts_with(".dir-contribution-sort-")
        }));
        drop(snapshot);
        fs::remove_dir_all(output).unwrap();
    }
    #[test]
    fn corrupt_archive_fails_before_manifest_publication() {
        let output = directory();
        let prepared = prepared();
        let snapshot =
            crate::runtime::spool::with_spooling(&output, || crate::runtime::simulate(&prepared))
                .unwrap();
        let archive = fs::read_dir(&output)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".dir-can-ledger-")
            })
            .unwrap();
        fs::write(&archive, b"{broken\n").unwrap();
        assert!(super::super::export(&prepared, &snapshot, &output).is_err());
        assert!(!output.join("manifest.json").exists());
        assert!(!fs::read_dir(&output).unwrap().any(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name.starts_with(".dir-result-sort-") || name.starts_with(".dir-contribution-sort-")
        }));
        drop(snapshot);
        fs::remove_dir_all(output).unwrap();
    }
}
