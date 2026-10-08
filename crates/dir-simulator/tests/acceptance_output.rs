//! DIR-TEST-0070..0073: authored Input A at the committed result boundary.
//! These short abstract durations test result processing, not native CAN timing.
use dir_simulator::{
    PreparedSimulation, output,
    snapshot::{CanSnapshot, CommonSnapshot, GatewaySnapshot, Point, Receiver, Request, Snapshot},
    types::{Bus, Controller, PreparedCan, PreparedCommon, PreparedGateway},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
const H: u64 = 35;
const PROFILE: &str = "can.cc.ideal.v1";
// Independently stated descriptor table, 結果処理検証仕様書.md DIR-TEST-0071.
const DESCRIPTORS: [(&str, &str, &str, &str, &str); 37] = [
    ("queue_length", "count", "integer", "point", "identity"),
    ("queue_max", "count", "integer", "summary", "max"),
    ("generated", "count", "integer", "summary", "sum"),
    ("admitted", "count", "integer", "summary", "sum"),
    ("processing", "count", "integer", "summary", "sum"),
    ("pending", "count", "integer", "summary", "sum"),
    ("in_flight", "count", "integer", "summary", "sum"),
    ("success", "count", "integer", "summary", "sum"),
    ("dropped", "count", "integer", "summary", "sum"),
    ("unfinished", "count", "integer", "summary", "sum"),
    ("attempts", "count", "integer", "summary", "sum"),
    ("retries", "count", "integer", "summary", "sum"),
    ("failed", "count", "integer", "summary", "sum"),
    (
        "receiver_opportunities",
        "count",
        "integer",
        "summary",
        "sum",
    ),
    ("received", "count", "integer", "summary", "sum"),
    ("filtered", "count", "integer", "summary", "sum"),
    ("rx_pending", "count", "integer", "summary", "sum"),
    ("tx_wait_ps", "ps", "integer", "point", "identity"),
    ("arbitration_wait_ps", "ps", "integer", "point", "identity"),
    ("transfer_ps", "ps", "integer", "point", "identity"),
    ("delivery_ps", "ps", "integer", "point", "identity"),
    ("tx_wait_mean_ps", "ps", "number", "summary", "sample_mean"),
    (
        "arbitration_wait_mean_ps",
        "ps",
        "number",
        "summary",
        "sample_mean",
    ),
    ("transfer_mean_ps", "ps", "number", "summary", "sample_mean"),
    ("delivery_mean_ps", "ps", "number", "summary", "sample_mean"),
    ("queue_mean", "count", "number", "summary", "time_mean"),
    ("buffer_utilization", "1", "number", "point", "identity"),
    (
        "buffer_utilization_mean",
        "1",
        "number",
        "window_summary",
        "time_mean",
    ),
    (
        "bus_utilization",
        "1",
        "number",
        "window_summary",
        "occupancy_ratio",
    ),
    (
        "frame_utilization",
        "1",
        "number",
        "window_summary",
        "occupancy_ratio",
    ),
    ("payload_bits", "bit", "integer", "window_summary", "sum"),
    ("serialized_bits", "bit", "integer", "window_summary", "sum"),
    ("occupied_bits", "bit", "integer", "window_summary", "sum"),
    (
        "received_payload_bits",
        "bit",
        "integer",
        "window_summary",
        "sum",
    ),
    (
        "payload_throughput_bps",
        "bit/s",
        "number",
        "window_summary",
        "rate",
    ),
    (
        "serialized_throughput_bps",
        "bit/s",
        "number",
        "window_summary",
        "rate",
    ),
    (
        "received_throughput_bps",
        "bit/s",
        "number",
        "window_summary",
        "rate",
    ),
];

fn d(n: Option<u64>) -> Value {
    n.map(|n| json!(n.to_string())).unwrap_or(Value::Null)
}
// All authored operands here are exactly representable integers below 2^53.
// A single hardware division gives the independently rounded binary64 rational.
fn rational(n: u64, denominator: u64) -> Value {
    if denominator == 0 {
        Value::Null
    } else {
        json!(n as f64 / denominator as f64)
    }
}
fn controller(id: &str) -> Controller {
    Controller {
        id: id.into(),
        queue_capacity: 2,
        tx_processing_ps: 0,
        rx_processing_ps: 0,
        rx_filter: "all".into(),
        tx_channel_ps: 0,
        rx_channel_ps: 0,
    }
}
fn input_a() -> (PreparedSimulation, Snapshot) {
    let prepared = PreparedSimulation {
        registered: None,
        ethernet: None,
        canfd: None,
        axi: None,
        soc: None,
        memory_ipc: None,
        common: PreparedCommon {
            provenance: Default::default(),
            run_identity: None,
            profile: PROFILE.into(),
            module_paths: vec![],
            network: "Net".into(),
            time_limit_ps: H,
            metrics_window_ps: 10,
            max_events: 100,
            max_delta_cycles: 100,
            channel_count: 0,
            config_path: PathBuf::new(),
            model_config_path: None,
            workload_path: None,
            inputs: vec![],
        },
        can: PreparedCan {
            buses: vec![Bus {
                id: "Net.bus".into(),
                bitrate: 500_000,
            }],
            controller_buses: vec![0; 3],
            bus_id: "Net.bus".into(),
            bitrate: 500_000,
            controllers: ["Net.a", "Net.b", "Net.c"].map(controller).into(),
            generators: vec![],
        },
        gateway: PreparedGateway {
            gateways: vec![],
            controller_gateways: vec![None; 3],
        },
    };
    // g,q,s,e,release,payload,serialized,H-state; authored Input A table.
    let table = [
        (0, Some(2), Some(5), Some(15), Some(18), 8, 52, "success"),
        (3, Some(4), Some(18), Some(28), Some(31), 16, 60, "success"),
        (20, Some(22), Some(32), None, None, 8, 52, "in_flight"),
        (29, Some(30), None, None, None, 8, 52, "pending"),
        (32, Some(33), None, None, None, 8, 52, "pending"),
        (3, Some(4), None, None, None, 8, 52, "dropped"),
        (34, None, None, None, None, 8, 52, "processing"),
    ];
    let requests = table
        .into_iter()
        .enumerate()
        .map(
            |(i, (g, q, s, e, release, payload, frame, status))| Request {
                request_id: format!("load:{i}"),
                source: "Net.a".into(),
                bus: "Net.bus".into(),
                status: status.into(),
                generated_ps: g,
                ready_ps: q,
                tx_enqueued_ps: if status == "dropped" { None } else { q },
                sof_ps: s,
                eof_ps: e,
                planned_eof_ps: if i == 2 { Some(42) } else { e },
                planned_release_ps: if i == 2 { Some(45) } else { release },
                release_ps: release,
                payload_bits: payload,
                frame_bits: frame,
                crc15: 0,
                stuff_bits: 0,
                bitrate_bps: 500_000,
            },
        )
        .collect();
    let receivers = [
        ("load:0", "Net.b", 16, Some(18), "received"),
        ("load:0", "Net.c", 17, Some(20), "received"),
        ("load:1", "Net.b", 29, Some(30), "received"),
        ("load:1", "Net.c", 29, None, "filtered"),
    ]
    .into_iter()
    .map(|(id, receiver, o, r, status)| Receiver {
        request_id: id.into(),
        receiver: receiver.into(),
        status: status.into(),
        observed_ps: Some(o),
        received_ps: r,
    })
    .collect();
    let mut points = ["Net.a.txQueue", "Net.b.txQueue", "Net.c.txQueue"]
        .into_iter()
        .map(|target| point(None, 0, target, "queue_length", 0, None, None, None))
        .collect::<Vec<_>>();
    // Reservation identifiers are authored, not native runtime sequence predictions.
    // The two observations at t=4 share one committed callback, with distinct effects.
    for (event, time, metric, value, id, receiver, reason) in [
        (1, 2, "queue_length", 1, "load:0", None, None),
        (2, 4, "queue_length", 2, "load:1", None, None),
        (2, 4, "queue_length", 2, "load:5", None, Some("queue_full")),
        (3, 5, "queue_length", 1, "load:0", None, None),
        (3, 5, "tx_wait_ps", 5, "load:0", None, None),
        (3, 5, "arbitration_wait_ps", 3, "load:0", None, None),
        (4, 15, "transfer_ps", 10, "load:0", None, None),
        (5, 18, "queue_length", 0, "load:1", None, None),
        (5, 18, "tx_wait_ps", 15, "load:1", None, None),
        (5, 18, "arbitration_wait_ps", 14, "load:1", None, None),
        (6, 18, "delivery_ps", 18, "load:0", Some("Net.b"), None),
        (7, 20, "delivery_ps", 20, "load:0", Some("Net.c"), None),
        (8, 22, "queue_length", 1, "load:2", None, None),
        (9, 28, "transfer_ps", 10, "load:1", None, None),
        (10, 30, "queue_length", 2, "load:3", None, None),
        (11, 30, "delivery_ps", 27, "load:1", Some("Net.b"), None),
        (12, 32, "queue_length", 1, "load:2", None, None),
        (12, 32, "tx_wait_ps", 12, "load:2", None, None),
        (12, 32, "arbitration_wait_ps", 10, "load:2", None, None),
        (13, 33, "queue_length", 2, "load:4", None, None),
    ] {
        let target = match metric {
            "queue_length" => "Net.a.txQueue",
            "transfer_ps" => "Net.bus",
            "delivery_ps" => receiver.unwrap(),
            _ => "Net.a",
        };
        points.push(point(
            Some(event),
            time,
            target,
            metric,
            value,
            Some(id),
            receiver,
            reason,
        ));
    }
    let snapshot = Snapshot {
        registered: None,
        network: None,
        ethernet: None,
        canfd: None,
        axi: None,
        soc: None,
        memory_ipc: None,
        common: CommonSnapshot {
            point_spool: None,
            spool_error: None,
            termination: "time_limit".into(),
            partial: false,
            end_ps: H,
            last_event_time_ps: Some(34),
            committed_events: 20,
            pending_events: 5,
            points,
            diagnostics: vec![],
        },
        can: CanSnapshot {
            archive: None,
            bus_state: "frame".into(),
            bus_states: vec!["frame".into()],
            requests,
            receivers,
        },
        gateway: GatewaySnapshot {
            forwards: vec![],
            rx_buffers: vec![],
            request_lineage: Default::default(),
        },
    };
    (prepared, snapshot)
}
#[allow(clippy::too_many_arguments)]
fn point(
    event: Option<u64>,
    time: u64,
    target: &str,
    metric: &str,
    value: u64,
    id: Option<&str>,
    receiver: Option<&str>,
    reason: Option<&str>,
) -> Point {
    Point {
        event_seq: event,
        effect_seq: event.map(|_| 0),
        time_ps: time,
        target: target.into(),
        metric: metric.into(),
        value,
        request_id: id.map(str::to_owned),
        receiver: receiver.map(str::to_owned),
        reason: reason.map(str::to_owned),
    }
}
struct Export {
    path: PathBuf,
    result: Value,
    retain: bool,
}
impl Export {
    fn new(p: &PreparedSimulation, s: &Snapshot) -> Self {
        // Optional evidence retention does not alter the authored input or assertions.
        let artifacts = std::env::var_os("DIR_ACCEPTANCE_OUTPUT_ARTIFACTS");
        let base = artifacts
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).unwrap();
        let name = std::thread::current().name().unwrap_or("output").to_owned();
        let path = base.join(format!(
            "{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        output::export(p, s, &path).unwrap();
        let result = serde_json::from_slice(&fs::read(path.join("results.json")).unwrap()).unwrap();
        Self {
            path,
            result,
            retain: artifacts.is_some(),
        }
    }
    fn summary(&self, target: &str, metric: &str) -> &Value {
        self.result["simulation"]["summary"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["target"] == target && r["metric"] == metric)
            .unwrap()
    }
    fn value(&self, target: &str, metric: &str) -> Value {
        self.summary(target, metric)["value"].clone()
    }
}
impl Drop for Export {
    fn drop(&mut self) {
        if !self.retain {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn row(target: &str, metric: &str, value: Value, start: u64, end: u64) -> Value {
    let (_, unit, kind, _, _) = DESCRIPTORS.iter().find(|d| d.0 == metric).unwrap();
    json!({"event_seq":null,"effect_seq":null,"target":target,"metric":metric,"unit":unit,
        "value_kind":kind,"value":value,"time_ps":null,"start_ps":start.to_string(),"end_ps":end.to_string(),
        "request_id":null,"receiver":null,"reason":null,"sample_count":null})
}
fn receiver_row(target: &str, metric: &str, value: Value, start: u64, end: u64) -> Value {
    let mut r = row(target, metric, value, start, end);
    r["receiver"] = json!(target);
    r
}
fn mean(target: &str, metric: &str, n: u64, samples: u64) -> Value {
    let mut r = row(target, metric, rational(n, samples), 0, H);
    r["sample_count"] = d(Some(samples));
    if metric == "delivery_mean_ps" && ["Net.a", "Net.b", "Net.c"].contains(&target) {
        r["receiver"] = json!(target);
    }
    r
}
fn expected_summary() -> Vec<Value> {
    let mut rows = vec![];
    for target in ["Net.a", "Net.b", "Net.c", "Net.bus", "$all"] {
        let active = ["Net.a", "Net.bus", "$all"].contains(&target);
        for (metric, n) in [
            ("generated", 7),
            ("admitted", 5),
            ("processing", 1),
            ("pending", 2),
            ("in_flight", 1),
            ("success", 2),
            ("dropped", 1),
            ("unfinished", 4),
            ("attempts", 3),
            ("retries", 0),
            ("failed", 0),
        ] {
            let mut r = row(target, metric, d(Some(if active { n } else { 0 })), 0, H);
            if metric == "dropped" {
                r["reason"] = json!("queue_full");
            }
            rows.push(r);
        }
        let counts = match target {
            "Net.b" => [2, 2, 0, 0],
            "Net.c" => [2, 1, 1, 0],
            "Net.a" => [0, 0, 0, 0],
            _ => [4, 3, 1, 0],
        };
        for (metric, n) in [
            "receiver_opportunities",
            "received",
            "filtered",
            "rx_pending",
        ]
        .into_iter()
        .zip(counts)
        {
            rows.push(if ["Net.a", "Net.b", "Net.c"].contains(&target) {
                receiver_row(target, metric, d(Some(n)), 0, H)
            } else {
                row(target, metric, d(Some(n)), 0, H)
            });
        }
        rows.push(mean(
            target,
            "tx_wait_mean_ps",
            if active { 32 } else { 0 },
            if active { 3 } else { 0 },
        ));
        rows.push(mean(
            target,
            "arbitration_wait_mean_ps",
            if active { 27 } else { 0 },
            if active { 3 } else { 0 },
        ));
        if ["Net.bus", "$all"].contains(&target) {
            rows.push(mean(target, "transfer_mean_ps", 20, 2));
        }
        let (n, samples) = match target {
            "Net.a" => (0, 0),
            "Net.b" => (45, 2),
            "Net.c" => (20, 1),
            _ => (65, 3),
        };
        rows.push(mean(target, "delivery_mean_ps", n, samples));
    }
    for queue in ["Net.a.txQueue", "Net.b.txQueue", "Net.c.txQueue"] {
        let active = queue == "Net.a.txQueue";
        rows.push(row(
            queue,
            "queue_max",
            d(Some(if active { 2 } else { 0 })),
            0,
            H,
        ));
        rows.push(row(
            queue,
            "queue_mean",
            rational(if active { 34 } else { 0 }, 35),
            0,
            H,
        ));
        let mut r = row(queue, "dropped", d(Some(u64::from(active))), 0, H);
        r["reason"] = json!("queue_full");
        rows.push(r);
    }
    rows.extend(expected_intervals(0, H, 29, 23, 24, 112, 118, 24, 8, 34));
    rows
}
// Authored independent interval vectors: lengths/areas/bit quantities from the spec.
#[allow(clippy::too_many_arguments)]
fn expected_intervals(
    start: u64,
    end: u64,
    busy: u64,
    frame: u64,
    payload: u64,
    serialized: u64,
    occupied: u64,
    b: u64,
    c: u64,
    area: u64,
) -> Vec<Value> {
    let length = end - start;
    let mut rows = vec![
        row(
            "Net.bus",
            "bus_utilization",
            rational(busy, length),
            start,
            end,
        ),
        row(
            "Net.bus",
            "frame_utilization",
            rational(frame, length),
            start,
            end,
        ),
    ];
    for (metric, n) in [
        ("payload_bits", payload),
        ("serialized_bits", serialized),
        ("occupied_bits", occupied),
    ] {
        rows.push(row("Net.bus", metric, d(Some(n)), start, end));
    }
    for (metric, n) in [
        ("payload_throughput_bps", payload),
        ("serialized_throughput_bps", serialized),
    ] {
        rows.push(row(
            "Net.bus",
            metric,
            rational(n * 1_000_000_000_000, length),
            start,
            end,
        ));
    }
    for (target, n) in [("Net.a", 0), ("Net.b", b), ("Net.c", c)] {
        rows.push(receiver_row(
            target,
            "received_payload_bits",
            d(Some(n)),
            start,
            end,
        ));
        rows.push(receiver_row(
            target,
            "received_throughput_bps",
            rational(n * 1_000_000_000_000, length),
            start,
            end,
        ));
        rows.push(row(
            &format!("{target}.txQueue"),
            "buffer_utilization_mean",
            rational(if target == "Net.a" { area } else { 0 }, 2 * length),
            start,
            end,
        ));
    }
    rows
}
#[allow(clippy::too_many_arguments)]
fn observed(
    event: Option<u64>,
    effect: Option<u64>,
    time: u64,
    target: &str,
    metric: &str,
    value: Value,
    id: Option<&str>,
    receiver: Option<&str>,
    reason: Option<&str>,
) -> Value {
    let mut r = row(target, metric, value, 0, 0);
    r["event_seq"] = d(event);
    r["effect_seq"] = d(effect);
    r["time_ps"] = d(Some(time));
    r["start_ps"] = Value::Null;
    r["end_ps"] = Value::Null;
    r["request_id"] = json!(id);
    r["receiver"] = json!(receiver);
    r["reason"] = json!(reason);
    r
}
fn expected_records() -> Vec<Value> {
    let mut rows = vec![];
    for queue in ["Net.a.txQueue", "Net.b.txQueue", "Net.c.txQueue"] {
        rows.push(observed(
            None,
            None,
            0,
            queue,
            "queue_length",
            d(Some(0)),
            None,
            None,
            None,
        ));
        rows.push(observed(
            None,
            None,
            0,
            queue,
            "buffer_utilization",
            json!(0.0),
            None,
            None,
            None,
        ));
    }
    for (event, time, q, id, buffer_effect, queue_effect, reason) in [
        (1, 2, 1, "load:0", 0, 1, None),
        (2, 4, 2, "load:1", 0, 2, None),
        (2, 4, 2, "load:5", 1, 3, Some("queue_full")),
        (3, 5, 1, "load:0", 2, 3, None),
        (5, 18, 0, "load:1", 2, 3, None),
        (8, 22, 1, "load:2", 0, 1, None),
        (10, 30, 2, "load:3", 0, 1, None),
        (12, 32, 1, "load:2", 2, 3, None),
        (13, 33, 2, "load:4", 0, 1, None),
    ] {
        rows.push(observed(
            Some(event),
            Some(queue_effect),
            time,
            "Net.a.txQueue",
            "queue_length",
            d(Some(q)),
            Some(id),
            None,
            reason,
        ));
        rows.push(observed(
            Some(event),
            Some(buffer_effect),
            time,
            "Net.a.txQueue",
            "buffer_utilization",
            rational(q, 2),
            Some(id),
            None,
            reason,
        ));
    }
    for (event, time, id, wait, arbitration) in [
        (3, 5, "load:0", 5, 3),
        (5, 18, "load:1", 15, 14),
        (12, 32, "load:2", 12, 10),
    ] {
        rows.push(observed(
            Some(event),
            Some(0),
            time,
            "Net.a",
            "arbitration_wait_ps",
            d(Some(arbitration)),
            Some(id),
            None,
            None,
        ));
        rows.push(observed(
            Some(event),
            Some(1),
            time,
            "Net.a",
            "tx_wait_ps",
            d(Some(wait)),
            Some(id),
            None,
            None,
        ));
    }
    for (event, time, id) in [(4, 15, "load:0"), (9, 28, "load:1")] {
        rows.push(observed(
            Some(event),
            Some(0),
            time,
            "Net.bus",
            "transfer_ps",
            d(Some(10)),
            Some(id),
            None,
            None,
        ));
    }
    for (event, time, id, target, latency) in [
        (6, 18, "load:0", "Net.b", 18),
        (7, 20, "load:0", "Net.c", 20),
        (11, 30, "load:1", "Net.b", 27),
    ] {
        rows.push(observed(
            Some(event),
            Some(0),
            time,
            target,
            "delivery_ps",
            d(Some(latency)),
            Some(id),
            Some(target),
            None,
        ));
    }
    for (start, end, busy, frame, payload, serialized, occupied, b, c, area) in [
        (0, 10, 5, 5, 0, 0, 0, 0, 0, 9),
        (10, 20, 10, 7, 8, 52, 55, 8, 0, 8),
        (20, 30, 10, 8, 16, 60, 0, 0, 8, 8),
        (30, 35, 4, 3, 0, 0, 63, 16, 0, 9),
    ] {
        rows.extend(expected_intervals(
            start, end, busy, frame, payload, serialized, occupied, b, c, area,
        ));
    }
    rows
}
fn compare_rows(actual: &Value, mut expected: Vec<Value>) {
    let actual = actual.as_array().unwrap();
    // Wire ordering: windows precede observations at a shared boundary; our
    // authored callback reservation numbers also enumerate their commit order.
    let key = |r: &Value| {
        let n = |field: &str| r[field].as_str().map(|s| s.parse::<u64>().unwrap());
        let kind = if r["time_ps"].is_null() {
            0
        } else if r["event_seq"].is_null() {
            1
        } else {
            2
        };
        (
            n("time_ps").or(n("end_ps")).unwrap(),
            kind,
            if kind == 2 {
                n("event_seq").unwrap()
            } else {
                0
            },
            if kind == 2 {
                n("effect_seq").unwrap()
            } else {
                0
            },
            r["target"].as_str().unwrap().to_owned(),
            r["metric"].as_str().unwrap().to_owned(),
            r["reason"].as_str().map(str::to_owned),
            r["receiver"].as_str().map(str::to_owned),
        )
    };
    expected.sort_by_key(key);
    for (i, r) in expected.iter_mut().enumerate() {
        r["seq"] = d(Some(i as u64));
    }
    assert_eq!(*actual, expected);
}
fn csv_projection(export: &Export, file: &str, rows: &Value) {
    let csv = fs::read_to_string(export.path.join(file)).unwrap();
    let mut lines = csv.lines();
    let header = lines.next().unwrap().split(',').collect::<Vec<_>>();
    assert_eq!(
        header,
        [
            "schema_version",
            "run_id",
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
            "sample_count"
        ]
    );
    let reconstructed = lines
        .map(|line| {
            let cells = line.split(',').collect::<Vec<_>>();
            assert_eq!(cells.len(), 17);
            assert_eq!(cells[0], "1");
            assert_eq!(cells[1], export.result["run_id"].as_str().unwrap());
            let mut r = serde_json::Map::new();
            for i in 2..17 {
                let v = if cells[i].is_empty() {
                    Value::Null
                } else if i == 9 && cells[8] == "number" {
                    json!(cells[i].parse::<f64>().unwrap())
                } else {
                    json!(cells[i])
                };
                r.insert(header[i].into(), v);
            }
            Value::Object(r)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reconstructed,
        *rows.as_array().unwrap(),
        "{file} complete 15-column projection"
    );
}

#[test]
fn dir_test_0070_0071_input_a_all_37_descriptors_points_windows_and_csv() {
    let (p, s) = input_a();
    let out = Export::new(&p, &s);
    let v = &out.result;
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["simulation"]["termination"], "time_limit");
    assert_eq!(v["simulation"]["partial"], false);
    assert_eq!(v["simulation"]["end_ps"], "35");
    assert_eq!(v["simulation"]["last_event_time_ps"], "34");
    assert_eq!(v["simulation"]["committed_events"], "20");
    assert_eq!(v["simulation"]["pending_events"], "5");
    let mut expected=DESCRIPTORS.iter().map(|(metric,unit,kind,sampling,aggregation)| json!({"metric_id":metric,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation})).collect::<Vec<_>>();
    expected.sort_by_key(|r| r["metric_id"].as_str().unwrap().to_owned());
    assert_eq!(v["metadata"]["metrics"], json!(expected));
    compare_rows(&v["simulation"]["records"], expected_records());
    compare_rows(&v["simulation"]["summary"], expected_summary());
    let union = v["simulation"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .chain(v["simulation"]["summary"].as_array().unwrap())
        .map(|r| r["metric"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(union, DESCRIPTORS.iter().map(|d| d.0).collect());
    // Canonical ordering must retain t=4 acceptance/full-drop and their distinct effects.
    let queues = v["simulation"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["target"] == "Net.a.txQueue" && r["metric"] == "queue_length")
        .collect::<Vec<_>>();
    assert_eq!(
        queues
            .iter()
            .map(|r| (r["time_ps"].clone(), r["value"].clone()))
            .collect::<Vec<_>>(),
        [
            (0, 0),
            (2, 1),
            (4, 2),
            (4, 2),
            (5, 1),
            (18, 0),
            (22, 1),
            (30, 2),
            (32, 1),
            (33, 2)
        ]
        .into_iter()
        .map(|(t, q)| (d(Some(t)), d(Some(q))))
        .collect::<Vec<_>>()
    );
    assert_eq!(queues[2]["request_id"], "load:1");
    assert_eq!(queues[3]["request_id"], "load:5");
    assert_ne!(queues[2]["effect_seq"], queues[3]["effect_seq"]);
    csv_projection(&out, "events.csv", &v["simulation"]["records"]);
    csv_projection(&out, "summary.csv", &v["simulation"]["summary"]);
}

#[test]
fn dir_test_0072_input_a_complete_ledgers_per_receiver_and_conservation() {
    let (p, s) = input_a();
    let out = Export::new(&p, &s);
    let v = &out.result["simulation"];
    let expected_requests=[
        (0_u64,0_u64,Some(2),Some(5),Some(15),Some(18),8_u64,52_u64,"success"),
        (1,3,Some(4),Some(18),Some(28),Some(31),16,60,"success"),
        (2,20,Some(22),Some(32),None,None,8,52,"in_flight"),
        (3,29,Some(30),None,None,None,8,52,"pending"),(4,32,Some(33),None,None,None,8,52,"pending"),
        (5,3,Some(4),None,None,None,8,52,"dropped"),(6,34,None,None,None,None,8,52,"processing"),
    ].into_iter().map(|(id,g,q,sof,eof,release,payload,frame,status)| json!({"request_id":format!("load:{id}"),"source":"Net.a","bus":"Net.bus","status":status,"generated_ps":g.to_string(),"ready_ps":d(q),"sof_ps":d(sof),"eof_ps":d(eof),"payload_bits":payload.to_string(),"serialized_bits":frame.to_string(),"attempts":if sof.is_some(){"1"}else{"0"},"retries":"0","drop_reason":if id==5{Some("queue_full")}else{None},"model_fields":{"profile":PROFILE,"schema_version":1,"crc15":"0","stuff_bits":"0","frame_bits":frame.to_string(),"intermission_bits":"3","bitrate_bps":"500000","planned_eof_ps":d(if id==2{Some(42)}else{eof}),"planned_release_ps":d(if id==2{Some(45)}else{release}),"release_ps":d(release)}})).collect::<Vec<_>>();
    assert_eq!(v["requests"], json!(expected_requests));
    let expected_receivers=[("load:0","Net.b",16_u64,Some(18),"received"),("load:0","Net.c",17,Some(20),"received"),("load:1","Net.b",29,Some(30),"received"),("load:1","Net.c",29,None,"filtered")].into_iter().map(|(id,receiver,o,r,status)| json!({"request_id":id,"receiver":receiver,"status":status,"observed_ps":o.to_string(),"received_ps":d(r)})).collect::<Vec<_>>();
    assert_eq!(v["receivers"], json!(expected_receivers));
    let requests = v["requests"].as_array().unwrap();
    let counts = ["processing", "pending", "in_flight", "success", "dropped"]
        .map(|status| requests.iter().filter(|r| r["status"] == status).count());
    assert_eq!(counts, [1, 2, 1, 2, 1]);
    assert_eq!(requests.len(), 1 + 5 + 1);
    assert_eq!(counts[1] + counts[2] + counts[3], 5);
    assert_eq!(counts[0] + counts[1] + counts[2], 4);
    let receivers = v["receivers"].as_array().unwrap();
    let received = receivers
        .iter()
        .filter(|r| r["status"] == "received")
        .count();
    let filtered = receivers
        .iter()
        .filter(|r| r["status"] == "filtered")
        .count();
    let pending = receivers
        .iter()
        .filter(|r| r["status"] == "pending")
        .count();
    assert_eq!((receivers.len(), received, filtered, pending), (4, 3, 1, 0));
    assert_eq!(receivers.len(), received + filtered + pending);
    for target in ["Net.a", "Net.bus", "$all"] {
        let n = |m| {
            out.value(target, m)
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
        };
        assert_eq!(
            n("generated"),
            n("processing") + n("admitted") + n("dropped")
        );
        assert_eq!(n("admitted"), n("success") + n("in_flight") + n("pending"));
        assert_eq!(
            n("unfinished"),
            n("processing") + n("in_flight") + n("pending")
        );
        assert_eq!(n("attempts"), n("in_flight") + n("success"));
    }
    for (target, opportunities, received, filtered, latency, samples) in [
        ("Net.a", 0, 0, 0, 0, 0),
        ("Net.b", 2, 2, 0, 45, 2),
        ("Net.c", 2, 1, 1, 20, 1),
        ("Net.bus", 4, 3, 1, 65, 3),
        ("$all", 4, 3, 1, 65, 3),
    ] {
        for (metric, n) in [
            ("receiver_opportunities", opportunities),
            ("received", received),
            ("filtered", filtered),
            ("rx_pending", 0),
        ] {
            assert_eq!(out.value(target, metric), d(Some(n)));
        }
        assert_eq!(
            out.value(target, "delivery_mean_ps"),
            rational(latency, samples)
        );
        assert_eq!(
            out.summary(target, "delivery_mean_ps")["sample_count"],
            d(Some(samples))
        );
    }
    assert_ne!(out.value("$all", "delivery_mean_ps"), rational(85, 4));
    assert_ne!(out.value("$all", "delivery_mean_ps"), json!(23.0));
}

fn cut_input_a(h: u64) -> (PreparedSimulation, Snapshot) {
    let (mut p, mut s) = input_a();
    p.common.time_limit_ps = h;
    s.common.end_ps = h;
    s.can.requests.retain(|r| r.generated_ps < h);
    s.common.points.retain(|point| point.time_ps < h);
    for r in &mut s.can.requests {
        if r.ready_ps.is_some_and(|t| t >= h) {
            r.ready_ps = None;
            r.tx_enqueued_ps = None;
        }
        if r.sof_ps.is_some_and(|t| t >= h) {
            r.sof_ps = None;
            r.planned_eof_ps = None;
            r.planned_release_ps = None;
        }
        if r.eof_ps.is_some_and(|t| t >= h) {
            r.eof_ps = None;
        }
        if r.release_ps.is_some_and(|t| t >= h) {
            r.release_ps = None;
        }
        if r.status != "dropped" {
            r.status = if r.eof_ps.is_some() {
                "success"
            } else if r.sof_ps.is_some() {
                "in_flight"
            } else if r.tx_enqueued_ps.is_some() {
                "pending"
            } else {
                "processing"
            }
            .into();
        }
    }
    s.can.receivers.retain(|rx| {
        s.can
            .requests
            .iter()
            .any(|r| r.request_id == rx.request_id && r.eof_ps.is_some())
    });
    for rx in &mut s.can.receivers {
        if rx.observed_ps.is_some_and(|t| t >= h) {
            rx.observed_ps = None;
            rx.status = "pending".into();
        }
        if rx.received_ps.is_some_and(|t| t >= h) {
            rx.received_ps = None;
            rx.status = "pending".into();
        }
    }
    s.common.last_event_time_ps = Some(if h == 10 { 5 } else { 28 });
    (p, s)
}
#[test]
fn dir_test_0072_h29_receiver_observations_at_h_remain_pending() {
    let (p, s) = cut_input_a(29);
    let out = Export::new(&p, &s);
    for (metric, n) in [
        ("receiver_opportunities", 4),
        ("received", 2),
        ("filtered", 0),
        ("rx_pending", 2),
        ("success", 2),
    ] {
        assert_eq!(out.value("$all", metric), d(Some(n)));
    }
    let receivers = out.result["simulation"]["receivers"].as_array().unwrap();
    assert_eq!(receivers.len(), 4);
    for rx in receivers.iter().filter(|r| r["request_id"] == "load:1") {
        assert_eq!(rx["status"], "pending");
        assert!(rx["observed_ps"].is_null());
        assert!(rx["received_ps"].is_null());
    }
    let q = out.result["simulation"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["request_id"] == "load:2")
        .unwrap();
    assert_eq!(q["status"], "pending");
    assert!(q["sof_ps"].is_null());
}
#[test]
fn dir_test_0073_h10_clips_in_flight_and_does_not_pre_generate() {
    let (p, s) = cut_input_a(10);
    let out = Export::new(&p, &s);
    for (metric, n) in [
        ("generated", 3),
        ("admitted", 2),
        ("pending", 1),
        ("dropped", 1),
        ("success", 0),
        ("in_flight", 1),
        ("unfinished", 2),
        ("attempts", 1),
        ("receiver_opportunities", 0),
    ] {
        assert_eq!(out.value("$all", metric), d(Some(n)));
    }
    assert_eq!(
        out.result["simulation"]["requests"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(
        out.result["simulation"]["receivers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for metric in ["payload_bits", "serialized_bits", "occupied_bits"] {
        assert_eq!(out.value("Net.bus", metric), json!("0"));
    }
    for metric in ["bus_utilization", "frame_utilization"] {
        assert_eq!(out.value("Net.bus", metric), json!(0.5));
    }
    assert_eq!(out.value("Net.a.txQueue", "queue_mean"), rational(9, 10));
    assert_eq!(
        out.value("Net.a.txQueue", "buffer_utilization_mean"),
        rational(9, 20)
    );
    for metric in ["transfer_mean_ps", "delivery_mean_ps"] {
        assert!(out.value("$all", metric).is_null());
        assert_eq!(out.summary("$all", metric)["sample_count"], "0");
    }
}
fn empty(h: u64) -> (PreparedSimulation, Snapshot) {
    let (mut p, mut s) = input_a();
    p.common.time_limit_ps = h;
    s.common.end_ps = h;
    s.can.requests.clear();
    s.can.receivers.clear();
    s.can.bus_state = "idle".into();
    s.can.bus_states = vec!["idle".into()];
    s.common.points.retain(|p| p.event_seq.is_none());
    s.common.last_event_time_ps = None;
    s.common.committed_events = 0;
    s.common.pending_events = 0;
    (p, s)
}
#[test]
fn dir_test_0070_zero_capacity_points_and_0073_empty_zero_and_short_windows() {
    let (mut p, mut s) = empty(30);
    p.can.controllers.push(Controller {
        queue_capacity: 0,
        ..controller("Net.d")
    });
    p.can.controller_buses.push(0);
    p.gateway.controller_gateways.push(None);
    s.common.points.push(point(
        None,
        0,
        "Net.d.txQueue",
        "queue_length",
        0,
        None,
        None,
        None,
    ));
    p.common.metrics_window_ps = 100;
    let out = Export::new(&p, &s);
    assert_eq!(out.result["simulation"]["end_ps"], "30");
    assert!(out.result["simulation"]["last_event_time_ps"].is_null());
    assert_eq!(out.result["simulation"]["committed_events"], "0");
    let windows = out.result["simulation"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["time_ps"].is_null())
        .collect::<Vec<_>>();
    assert_eq!(
        windows.len(),
        19,
        "one interval: bus7 + four controllers' receiver2/queue1"
    );
    for r in &windows {
        assert_eq!(r["start_ps"], "0");
        assert_eq!(r["end_ps"], "30");
        if r["target"] == "Net.d.txQueue" && r["metric"] == "buffer_utilization_mean" {
            assert!(r["value"].is_null());
        } else if r["value_kind"] == "number" {
            assert_eq!(r["value"], json!(0.0));
        } else {
            assert_eq!(r["value"], "0");
        }
    }
    for r in out.result["simulation"]["records"].as_array().unwrap() {
        if r["target"] == "Net.d.txQueue" && r["metric"] == "buffer_utilization" {
            assert!(r["value"].is_null());
        }
    }
    let dpoints = out.result["simulation"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["target"] == "Net.d.txQueue" && !r["time_ps"].is_null())
        .collect::<Vec<_>>();
    assert_eq!(dpoints.len(), 2);
    assert_eq!(
        dpoints
            .iter()
            .find(|r| r["metric"] == "queue_length")
            .unwrap()["value"],
        "0"
    );
    assert!(
        dpoints
            .iter()
            .find(|r| r["metric"] == "buffer_utilization")
            .unwrap()["value"]
            .is_null()
    );
    assert!(
        out.value("Net.d.txQueue", "buffer_utilization_mean")
            .is_null()
    );
    for (target, metric) in [
        ("Net.a.txQueue", "queue_mean"),
        ("Net.a.txQueue", "buffer_utilization_mean"),
        ("Net.bus", "bus_utilization"),
        ("Net.bus", "frame_utilization"),
        ("Net.bus", "payload_throughput_bps"),
    ] {
        assert_eq!(out.value(target, metric), json!(0.0));
    }
    for r in out.result["simulation"]["summary"].as_array().unwrap() {
        if r["metric"].as_str().unwrap().ends_with("mean_ps") {
            assert!(r["value"].is_null());
            assert_eq!(r["sample_count"], "0");
        } else if r["target"] == "Net.d.txQueue" && r["metric"] == "buffer_utilization_mean" {
            assert!(r["value"].is_null());
        } else if r["value_kind"] == "number" {
            assert_eq!(r["value"], json!(0.0));
        } else {
            assert_eq!(r["value"], "0");
        }
    }
    let (p, s) = empty(0);
    let out = Export::new(&p, &s);
    for field in ["records", "requests", "receivers"] {
        assert!(
            out.result["simulation"][field]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    for r in out.result["simulation"]["summary"].as_array().unwrap() {
        assert_eq!(r["start_ps"], "0");
        assert_eq!(r["end_ps"], "0");
        if r["value_kind"] == "number" {
            assert!(r["value"].is_null());
        } else {
            assert_eq!(r["value"], "0");
        }
        if r["metric"].as_str().unwrap().ends_with("mean_ps") {
            assert_eq!(r["sample_count"], "0");
        } else {
            assert!(r["sample_count"].is_null());
        }
    }
}
#[test]
fn dir_test_0073_instantaneous_peak_and_overlapping_interval_union() {
    let (p, mut s) = empty(30);
    s.common.points.push(point(
        Some(1),
        5,
        "Net.a.txQueue",
        "queue_length",
        1,
        None,
        None,
        None,
    ));
    s.common.points.push(point(
        Some(2),
        5,
        "Net.a.txQueue",
        "queue_length",
        0,
        None,
        None,
        None,
    ));
    s.common.committed_events = 2;
    s.common.last_event_time_ps = Some(5);
    let out = Export::new(&p, &s);
    assert_eq!(out.value("Net.a.txQueue", "queue_max"), json!("1"));
    assert_eq!(out.value("Net.a.txQueue", "queue_mean"), json!(0.0));
    let q = out.result["simulation"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["time_ps"] == "5" && r["metric"] == "queue_length")
        .map(|r| r["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(q, vec![json!("1"), json!("0")]);
    // Explicit generic union helper data: this is not a valid CAN owner schedule.
    let (p, mut s) = empty(10);
    let template = input_a().1.can.requests.remove(0);
    s.can.requests = [(1, 6), (4, 8), (8, 9)]
        .into_iter()
        .enumerate()
        .map(|(i, (start, end))| Request {
            request_id: format!("union:{i}"),
            status: "success".into(),
            generated_ps: 0,
            ready_ps: Some(0),
            tx_enqueued_ps: Some(0),
            sof_ps: Some(start),
            eof_ps: Some(end),
            release_ps: Some(end),
            planned_eof_ps: Some(end),
            planned_release_ps: Some(end),
            ..template.clone()
        })
        .collect();
    let out = Export::new(&p, &s);
    assert_eq!(out.value("Net.bus", "bus_utilization"), rational(8, 10));
    assert_eq!(out.value("Net.bus", "frame_utilization"), rational(8, 10));
}
