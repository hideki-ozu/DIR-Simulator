//! Aggregation, wire-format, and publication regressions.
use super::aggregate::{MetricValue, Record, mul, occupancy};
use super::json::{canonical, number_wire, record_value};
use super::metadata::build as metadata;
use super::publish::{digest, run_id};
use super::{PROFILE, aggregate, csv, export};
use crate::snapshot::{Point, Request, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::fs;

fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Value>, Vec<Value>), Diagnostic> {
    let (events, summary) = aggregate::records(prepared, snapshot)?;
    Ok((
        events.iter().map(record_value).collect(),
        summary.iter().map(record_value).collect(),
    ))
}
fn ratio(n: u128, denominator: u128) -> Result<Value, Diagnostic> {
    aggregate::ratio(n, denominator).map(|value| match value {
        MetricValue::Number(n) => json!(n),
        MetricValue::Integer(n) => json!(n.to_string()),
        MetricValue::Null => Value::Null,
    })
}

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
        ethernet: None,
        canfd: None,
        axi: None,
        soc: None,
        memory_ipc: None,
        common: crate::types::PreparedCommon {
            profile: PROFILE.into(),
            module_paths: vec![],
            network: "Net".into(),
            time_limit_ps: 30,
            metrics_window_ps: 10,
            max_events: 100,
            max_delta_cycles: 100,
            channel_count: 0,
            config_path: std::path::PathBuf::new(),
            model_config_path: None,
            workload_path: None,
            inputs: vec![],
        },
        can: crate::types::PreparedCan {
            buses: vec![crate::types::Bus {
                id: "Net.bus".into(),
                bitrate: 500_000,
            }],
            controller_buses: vec![0; 3],
            bus_id: "Net.bus".into(),
            bitrate: 500_000,
            controllers: vec![
                controller("Net.a"),
                controller("Net.b"),
                controller("Net.c"),
            ],
            generators: vec![],
        },
        gateway: crate::types::PreparedGateway {
            gateways: vec![],
            controller_gateways: vec![None; 3],
        },
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
            reason: None,
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
            reason: None,
        });
    }
    let request = Request {
        request_id: "load:0".into(),
        source: "Net.a".into(),
        bus: "Net.bus".into(),
        status: "success".into(),
        generated_ps: 0,
        ready_ps: Some(2),
        tx_enqueued_ps: Some(2),
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
        ethernet: None,
        canfd: None,
        axi: None,
        soc: None,
        memory_ipc: None,
        common: crate::snapshot::CommonSnapshot {
            termination: "events_exhausted".into(),
            partial: false,
            end_ps: 30,
            last_event_time_ps: Some(20),
            committed_events: 8,
            pending_events: 0,
            points,
            diagnostics: vec![],
        },
        can: crate::snapshot::CanSnapshot {
            bus_state: "idle".into(),
            bus_states: vec!["idle".into()],
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
        },
        gateway: crate::snapshot::GatewaySnapshot {
            forwards: vec![],
            rx_buffers: vec![],
            request_lineage: std::collections::BTreeMap::from([(
                "load:0".into(),
                crate::snapshot::RequestLineage {
                    origin_request_id: "load:0".into(),
                    parent_request_id: None,
                    gw_hops: 0,
                },
            )]),
        },
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
fn missing_gateway_lineage_is_an_output_error() {
    let (prepared, mut snapshot) = fixture();
    snapshot.gateway.request_lineage.clear();
    let error = super::model_records::records(&prepared, &snapshot).unwrap_err();
    assert_eq!(error.code, "E-0003");
    assert!(error.message.contains("load:0"));
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
    prepared.common.metrics_window_ps = 20;
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
    snapshot.common.end_ps = 10;
    snapshot.can.requests[0].status = "in_flight".into();
    snapshot.can.requests[0].eof_ps = None;
    snapshot.can.requests[0].release_ps = None;
    snapshot.can.receivers.clear();
    snapshot.common.points.retain(|p| p.time_ps < 10);
    let (_, summary) = records(&prepared, &snapshot).unwrap();
    assert_eq!(count(&summary, "$all", "in_flight"), 1);
    assert_eq!(count(&summary, "$all", "unfinished"), 1);
    assert_eq!(count(&summary, "Net.bus", "payload_bits"), 0);
    assert_eq!(number(&summary, "Net.bus", "bus_utilization"), 0.5);
    assert_eq!(number(&summary, "Net.bus", "frame_utilization"), 0.5);
    assert!(row(&summary, "$all", "transfer_mean_ps")["value"].is_null());
    snapshot.common.end_ps = 17;
    snapshot.can.requests[0].status = "success".into();
    snapshot.can.requests[0].eof_ps = Some(15);
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
    prepared.can.controllers[0].queue_capacity = 0;
    snapshot.common.end_ps = 0;
    snapshot.can.requests.clear();
    snapshot.can.receivers.clear();
    snapshot.common.points.retain(|p| p.event_seq.is_none());
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
    snapshot.common.end_ps = 30;
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
    snapshot.common.end_ps = 20;
    snapshot.common.partial = true;
    snapshot.common.termination = "execution_failed".into();
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
        csv_from_json(
            result["simulation"]["records"].as_array().unwrap(),
            result["run_id"].as_str().unwrap(),
            1
        )
    );
    assert_eq!(
        fs::read(out.join("summary.csv")).unwrap(),
        csv_from_json(
            result["simulation"]["summary"].as_array().unwrap(),
            result["run_id"].as_str().unwrap(),
            1
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
    let mut peak = snapshot.common.points[3].clone();
    peak.value = 2;
    snapshot.common.points.insert(3, peak);
    let (_, summary) = records(&prepared, &snapshot).unwrap();
    assert_eq!(count(&summary, "Net.a.txQueue", "queue_max"), 2);
    assert_eq!(number(&summary, "Net.a.txQueue", "queue_mean"), 0.1);
    let mut a = snapshot.can.requests[0].clone();
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
    prepared.common.config_path = input.clone();
    prepared.common.inputs.push(InputSnapshot {
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
    snapshot.common.partial = true;
    snapshot.common.termination = "execution_failed".into();
    snapshot.common.end_ps = 30;
    snapshot.common.last_event_time_ps = Some(20);
    snapshot
        .common
        .diagnostics
        .push(Diagnostic::execution("event limit reached at next event"));
    let out = temp_dir();
    export(&prepared, &snapshot, &out).unwrap();
    let diagnostic: Value =
        serde_json::from_slice(&fs::read(out.join("diagnostics.jsonl")).unwrap()).unwrap();
    assert_eq!(diagnostic["time_ps"], "30");
    fs::remove_dir_all(out).unwrap();
}

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

fn csv_from_json(rows: &[Value], run_id: &str, version: u32) -> Vec<u8> {
    fn cell(s: String) -> String {
        if s.contains([',', '"', '\r', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s
        }
    }
    let mut output = HEADER.to_string();
    for row in rows {
        let mut cells = vec![version.to_string(), run_id.into()];
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

#[test]
fn typed_writers_preserve_large_integers_and_nullable_cells() {
    let mut row = Record::aggregate(
        "日本語",
        "generated",
        MetricValue::Integer(u128::MAX),
        0,
        u64::MAX,
    );
    row.seq = u128::MAX;
    row.event_seq = Some(u64::MAX);
    row.effect_seq = Some(u128::MAX);
    row.sample_count = Some(u128::MAX);
    let max = "340282366920938463463374607431768211455";
    let max64 = "18446744073709551615";
    let bytes = csv::encode(std::slice::from_ref(&row), "fixed-run", 2);
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        format!(
            "{HEADER}2,fixed-run,{max},{max64},{max},日本語,generated,count,integer,{max},,0,{max64},,,,{max}\n"
        )
    );
    let rendered = record_value(&row);
    for field in ["seq", "effect_seq", "value", "sample_count"] {
        assert_eq!(rendered[field].as_str(), Some(max));
    }
    for field in ["time_ps", "request_id", "receiver", "reason"] {
        assert!(rendered[field].is_null(), "{field}");
    }
    assert_eq!(rendered["start_ps"], "0");
    assert_eq!(rendered["end_ps"], max64);

    row.value = MetricValue::Null;
    row.sample_count = None;
    row.effect_seq = None;
    assert!(record_value(&row)["value"].is_null());
    assert_eq!(
        String::from_utf8(csv::encode(&[row], "fixed-run", 1)).unwrap(),
        format!(
            "{HEADER}1,fixed-run,{max},{max64},,日本語,generated,count,integer,,,0,{max64},,,,\n"
        )
    );
}

#[test]
fn typed_writers_distinguish_zero_null_and_exact_float_spelling() {
    let cases = [
        (MetricValue::Integer(0), "0", "\"0\""),
        (
            MetricValue::Number(0.0),
            "0.0000000000000000e+0",
            "0.0000000000000000e+0",
        ),
        (
            MetricValue::Number(-0.0),
            "0.0000000000000000e+0",
            "0.0000000000000000e+0",
        ),
        (
            MetricValue::Number(0.5),
            "5.0000000000000000e-1",
            "5.0000000000000000e-1",
        ),
        (
            MetricValue::Number(1.0 / 3.0),
            "3.3333333333333331e-1",
            "3.3333333333333331e-1",
        ),
        (
            MetricValue::Number(1e20),
            "1.0000000000000000e+20",
            "1.0000000000000000e+20",
        ),
        (MetricValue::Null, "", "null"),
    ];
    for (value, csv_value, json_value) in cases {
        let row = Record::aggregate("target", "queue_mean", value, 0, 10);
        assert_eq!(canonical(&record_value(&row)["value"]), json_value);
        assert_eq!(
            String::from_utf8(csv::encode(&[row], "fixed-run", 1)).unwrap(),
            format!(
                "{HEADER}1,fixed-run,0,,,target,queue_mean,count,number,{csv_value},,0,10,,,,\n"
            )
        );
    }
}

#[test]
fn typed_csv_quotes_commas_quotes_cr_lf_and_preserves_unicode() {
    let mut row = Record::aggregate(
        "日本語,\"対象\"\r\n次",
        "generated",
        MetricValue::Integer(7),
        0,
        10,
    );
    row.request_id = Some("request,\"quoted\"".into());
    row.receiver = Some("受信\rnode".into());
    row.reason = Some("line\nbreak".into());
    assert_eq!(
        String::from_utf8(csv::encode(&[row.clone()], "fixed-run", 1)).unwrap(),
        format!(
            "{HEADER}1,fixed-run,0,,,\"日本語,\"\"対象\"\"\r\n次\",generated,count,integer,7,,0,10,\"request,\"\"quoted\"\"\",\"受信\rnode\",\"line\nbreak\",\n"
        )
    );
    assert_eq!(
        canonical(&record_value(&row)["target"]),
        "\"日本語,\\\"対象\\\"\\u000d\\u000a次\""
    );
}

#[test]
fn typed_sort_preserves_numeric_effects_callback_groups_and_option_order() {
    let (prepared, mut snapshot) = fixture();
    snapshot.common.points.clear();
    let point = |event_seq, metric: &str, reason: Option<&str>| Point {
        event_seq,
        effect_seq: Some(99),
        time_ps: 5,
        target: "same".into(),
        metric: metric.into(),
        value: 1,
        request_id: None,
        receiver: None,
        reason: reason.map(str::to_string),
    };
    for reason in [Some("z"), Some(""), None] {
        snapshot.common.points.push(point(None, "initial", reason));
    }
    for i in (0..12).rev() {
        snapshot
            .common
            .points
            .push(point(Some(9), &format!("metric{i:02}"), None));
    }
    snapshot.common.points.push(point(Some(1), "later", None));
    snapshot.common.points.push(point(Some(9), "repeat", None));
    let (events, _) = aggregate::records(&prepared, &snapshot).unwrap();
    let at_five: Vec<_> = events.iter().filter(|r| r.time_ps == Some(5)).collect();
    assert_eq!(
        at_five
            .iter()
            .map(|r| r.reason.as_deref())
            .take(3)
            .collect::<Vec<_>>(),
        [None, Some(""), Some("z")]
    );
    assert_eq!(
        at_five
            .iter()
            .map(|r| r.event_seq)
            .skip(3)
            .collect::<Vec<_>>(),
        [vec![Some(9); 12], vec![Some(1), Some(9)]].concat()
    );
    for (effect, row) in at_five[3..15].iter().enumerate() {
        assert_eq!(row.effect_seq, Some(effect as u128));
        assert_eq!(row.metric, format!("metric{effect:02}"));
    }
    assert_eq!(at_five[15].effect_seq, Some(0));
    assert_eq!(at_five[16].effect_seq, Some(0));
    for (seq, row) in events.iter().enumerate() {
        assert_eq!(row.seq, seq as u128);
    }
}
