//! AXI committed transaction, handshake, memory and exact metric projection.
use super::aggregate::{MetricValue, Record, finalize, ratio};
use super::json::{canonical, opt_d};
use super::publish::digest;
use crate::snapshot::axi::{AxiRequest, AxiSnapshot};
use crate::snapshot::{Point, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("String write");
    }
    text
}

pub(super) const METRICS: &str = "axi.generated axi.completed axi.dropped axi.pending axi.active axi.okay axi.slverr axi.decerr axi.queue_length axi.outstanding axi.queue_mean axi.outstanding_mean axi.queue_max axi.outstanding_max axi.bus_utilization axi.wait_ps axi.latency_ps axi.wait_mean_ps axi.latency_mean_ps axi.read_bits axi.written_bits axi.read_throughput_bps axi.written_throughput_bps axi.channel_stall_cycles";

pub(super) fn descriptor(metric: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match metric {
        "axi.queue_length" | "axi.outstanding" | "axi.channel_stall_cycles" => {
            ("count", "integer", "point", "identity")
        }
        "axi.queue_mean" | "axi.outstanding_mean" => ("count", "number", "summary", "time_mean"),
        "axi.queue_max" | "axi.outstanding_max" => ("count", "integer", "summary", "max"),
        "axi.bus_utilization" => ("1", "number", "window_summary", "occupancy_ratio"),
        "axi.wait_ps" | "axi.latency_ps" => ("ps", "integer", "point", "identity"),
        "axi.wait_mean_ps" | "axi.latency_mean_ps" => ("ps", "number", "summary", "sample_mean"),
        "axi.read_bits" | "axi.written_bits" => ("bit", "integer", "window_summary", "sum"),
        "axi.read_throughput_bps" | "axi.written_throughput_bps" => {
            ("bit/s", "number", "window_summary", "rate")
        }
        _ => ("count", "integer", "summary", "sum"),
    }
}

fn gauge(points: &[Point], target: &str, metric: &str, h: u64) -> (u128, u64) {
    let (mut previous, mut time, mut area, mut maximum) = (0u64, 0u64, 0u128, 0u64);
    for point in points
        .iter()
        .filter(|p| p.target == target && p.metric == metric && p.time_ps <= h)
    {
        area += u128::from(previous) * u128::from(point.time_ps - time);
        previous = point.value;
        time = point.time_ps;
        maximum = maximum.max(previous);
    }
    area += u128::from(previous) * u128::from(h - time);
    (area, maximum)
}

fn interval_metrics(
    prepared: &PreparedSimulation,
    state: &AxiSnapshot,
    start: u64,
    end: u64,
    include_end: bool,
) -> Result<Vec<Record>, Diagnostic> {
    let model = prepared.axi.as_ref().expect("AXI prepared input");
    let busy: u128 = state
        .requests
        .iter()
        .filter_map(|r| r.grant_ps.map(|g| (g, r.completed_ps.unwrap_or(end))))
        .map(|(a, b)| u128::from(b.min(end).saturating_sub(a.max(start))))
        .sum();
    let mut read = 0u128;
    let mut written = 0u128;
    for hs in state
        .handshakes
        .iter()
        .filter(|hs| start <= hs.time_ps && (hs.time_ps < end || include_end && hs.time_ps == end))
    {
        if state.requests[hs.request].response.as_deref() != Some("OKAY") {
            continue;
        }
        if hs.channel == "R" {
            read += 32;
        }
        if hs.channel == "W" {
            written += 8 * u128::from(hs.wstrb.unwrap_or(0).count_ones());
        }
    }
    let duration = u128::from(end - start);
    Ok([
        ("axi.bus_utilization", ratio(busy, duration)?),
        ("axi.read_bits", MetricValue::Integer(read)),
        ("axi.written_bits", MetricValue::Integer(written)),
        (
            "axi.read_throughput_bps",
            ratio(read * 1_000_000_000_000, duration)?,
        ),
        (
            "axi.written_throughput_bps",
            ratio(written * 1_000_000_000_000, duration)?,
        ),
    ]
    .into_iter()
    .map(|(metric, value)| Record::aggregate(&model.interconnect, metric, value, start, end))
    .collect())
}

pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let model = prepared.axi.as_ref().expect("AXI prepared input");
    let state = snapshot
        .axi
        .as_ref()
        .ok_or_else(|| Diagnostic::output("missing AXI committed snapshot"))?;
    let h = snapshot.common.end_ps;
    let mut records: Vec<_> = snapshot
        .common
        .points
        .iter()
        .enumerate()
        .filter(|(_, p)| p.metric.starts_with("axi.") && (h > 0 || snapshot.common.partial))
        .map(|(i, p)| Record::point(p, &p.metric, MetricValue::Integer(u128::from(p.value)), i))
        .collect();
    let mut summary = Vec::new();
    let targets: Vec<_> = model
        .managers
        .iter()
        .map(|m| m.id.as_str())
        .chain([model.interconnect.as_str(), "$all"])
        .collect();
    for target in targets {
        let requests: Vec<&AxiRequest> = state
            .requests
            .iter()
            .filter(|r| {
                target == "$all"
                    || target == model.interconnect
                    || model.managers[model.generators[r.generator].source].id == target
            })
            .collect();
        for (metric, count) in [
            ("axi.generated", requests.len()),
            (
                "axi.completed",
                requests.iter().filter(|r| r.status == "completed").count(),
            ),
            (
                "axi.dropped",
                requests.iter().filter(|r| r.status == "dropped").count(),
            ),
            (
                "axi.pending",
                requests.iter().filter(|r| r.status == "pending").count(),
            ),
            (
                "axi.active",
                requests.iter().filter(|r| r.status == "active").count(),
            ),
            (
                "axi.okay",
                requests
                    .iter()
                    .filter(|r| r.status == "completed" && r.response.as_deref() == Some("OKAY"))
                    .count(),
            ),
            (
                "axi.slverr",
                requests
                    .iter()
                    .filter(|r| r.status == "completed" && r.response.as_deref() == Some("SLVERR"))
                    .count(),
            ),
            (
                "axi.decerr",
                requests
                    .iter()
                    .filter(|r| r.status == "completed" && r.response.as_deref() == Some("DECERR"))
                    .count(),
            ),
        ] {
            let mut row =
                Record::aggregate(target, metric, MetricValue::Integer(count as u128), 0, h);
            if metric == "axi.dropped" {
                row.reason = Some("outstanding_full".into());
            }
            summary.push(row);
        }
        for (metric, latency) in [("axi.wait_mean_ps", false), ("axi.latency_mean_ps", true)] {
            let samples: Vec<_> = requests
                .iter()
                .filter_map(|r| {
                    (if latency { r.completed_ps } else { r.grant_ps }).map(|t| t - r.generated_ps)
                })
                .collect();
            let mut row = Record::aggregate(
                target,
                metric,
                ratio(
                    samples.iter().map(|&n| u128::from(n)).sum(),
                    samples.len() as u128,
                )?,
                0,
                h,
            );
            row.sample_count = Some(samples.len() as u128);
            summary.push(row);
        }
    }
    for manager in &model.managers {
        for (point, mean, max) in [
            ("axi.queue_length", "axi.queue_mean", "axi.queue_max"),
            (
                "axi.outstanding",
                "axi.outstanding_mean",
                "axi.outstanding_max",
            ),
        ] {
            let (area, maximum) = gauge(&snapshot.common.points, &manager.id, point, h);
            summary.push(Record::aggregate(
                &manager.id,
                mean,
                ratio(area, u128::from(h))?,
                0,
                h,
            ));
            summary.push(Record::aggregate(
                &manager.id,
                max,
                MetricValue::Integer(u128::from(maximum)),
                0,
                h,
            ));
        }
    }
    let mut start = 0;
    while start < h {
        let end = start
            .saturating_add(prepared.common.metrics_window_ps)
            .min(h);
        records.extend(interval_metrics(prepared, state, start, end, false)?);
        start = end;
    }
    summary.extend(interval_metrics(
        prepared,
        state,
        0,
        h,
        snapshot.common.partial,
    )?);
    Ok(finalize(records, summary))
}

pub(super) fn model_records(prepared: &PreparedSimulation, state: &AxiSnapshot) -> Vec<Value> {
    let model = prepared.axi.as_ref().expect("AXI prepared input");
    let envelope = |schema: &str,
                    id: &str,
                    subject: &str,
                    request: Option<&str>,
                    time: u64,
                    data: Value| json!({"schema_name":schema,"schema_version":1,"record_id":id,"subject":subject,"request_id":request,"origin_request_id":null,"time_ps":time.to_string(),"data":data});
    let mut rows = Vec::new();
    for r in &state.requests {
        let g = &model.generators[r.generator];
        rows.push(envelope("axi.transaction",&r.request_id,&model.managers[g.source].id,Some(&r.request_id),r.time_ps,json!({"manager":model.managers[g.source].id,"interconnect":model.interconnect,"target":model.ram.id,"operation":g.transaction.operation,"address":g.transaction.address.to_string(),"beats":g.transaction.beats.to_string(),"generated_ps":r.generated_ps.to_string(),"eligible_ps":r.eligible_ps.to_string(),"status":r.status,"grant_ps":opt_d(r.grant_ps),"completed_ps":opt_d(r.completed_ps),"response":r.response,"read_data":r.read_data,"drop_reason":r.drop_reason})));
    }
    for hs in &state.handshakes {
        let request = &state.requests[hs.request].request_id;
        let id = format!("{request}:{}:{}", hs.channel, hs.beat.unwrap_or(0));
        rows.push(envelope("axi.handshake",&id,&model.interconnect,Some(request),hs.time_ps,json!({"channel":hs.channel,"beat":opt_d(hs.beat),"valid_since_ps":hs.valid_since_ps.to_string(),"address":opt_d(hs.address),"data_hex":hs.data_hex,"wstrb":hs.wstrb.map(|n|n.to_string()),"last":hs.last,"response":hs.response})));
    }
    let data_hex = hex(&state.memory);
    rows.push(envelope("axi.memory",&model.ram.id,&model.ram.id,None,state.memory_time_ps,json!({"base":model.ram.base.to_string(),"size":model.ram.size.to_string(),"data_hex":data_hex})));
    rows.sort_by(|a, b| {
        (
            a["schema_name"].as_str(),
            a["subject"].as_str(),
            a["record_id"].as_str(),
        )
            .cmp(&(
                b["schema_name"].as_str(),
                b["subject"].as_str(),
                b["record_id"].as_str(),
            ))
    });
    rows
}

pub(super) fn metadata(
    prepared: &PreparedSimulation,
    timestamp: &str,
    sources: Vec<Value>,
    source_hashes: Vec<Value>,
    mut config: BTreeMap<String, String>,
) -> Result<Value, Diagnostic> {
    let model = prepared.axi.as_ref().expect("AXI prepared input");
    config.insert(
        "model-profile".into(),
        canonical(&json!(prepared.common.profile)),
    );
    config.insert(
        "clock_period".into(),
        format!("{}ps", model.clock_period_ps),
    );
    for m in &model.managers {
        config.insert(m.id.clone(),canonical(&json!({"max_outstanding":m.max_outstanding.to_string(),"b_ready":m.b_ready,"r_ready":m.r_ready})));
    }
    config.insert(model.ram.id.clone(),canonical(&json!({"base":model.ram.base.to_string(),"size":model.ram.size.to_string(),"initial":hex(&model.ram.initial),"aw_ready":model.ram.aw_ready,"w_ready":model.ram.w_ready,"ar_ready":model.ram.ar_ready,"read_latency_cycles":model.ram.read_latency_cycles.to_string(),"write_response_cycles":model.ram.write_response_cycles.to_string(),"error_ranges":model.ram.error_ranges.iter().map(|r|json!({"start":r.start.to_string(),"end":r.end.to_string(),"access":r.access})).collect::<Vec<_>>()})));
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let mut initial: BTreeMap<String, Value> = prepared
        .common
        .module_paths
        .iter()
        .map(|id| (id.clone(), json!({})))
        .collect();
    initial.insert(prepared.common.network.clone(), json!({}));
    for m in &model.managers {
        initial.insert(m.id.clone(), json!({"queue":[],"outstanding":"0"}));
    }
    initial.insert(
        model.interconnect.clone(),
        json!({"cursor":"0","active":null}),
    );
    initial.insert(model.ram.id.clone(),json!({"base":model.ram.base.to_string(),"size":model.ram.size.to_string(),"data_hex":hex(&model.ram.initial)}));
    let initial_state: Vec<_> = initial
        .into_iter()
        .map(|(instance, state)| json!({"instance":instance,"state":canonical(&state)}))
        .collect();
    let mut metrics:Vec<_>=METRICS.split_whitespace().map(|id|{let(unit,kind,sampling,aggregation)=descriptor(id);json!({"metric_id":id,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation})}).collect();
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    let topology = json!({"interconnect":model.interconnect,"ram":model.ram.id,"managers":model.managers.iter().map(|m|json!({"id":m.id,"interconnect":model.interconnect})).collect::<Vec<_>>(),"clock_period_ps":model.clock_period_ps.to_string()});
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(source_hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":prepared.common.profile,"model_schemas":[{"schema_name":"axi.handshake","schema_version":1},{"schema_name":"axi.memory","schema_version":1},{"schema_name":"axi.transaction","schema_version":1}],"models":[{"type":prepared.common.profile,"version":"1","assumptions":["32-bit aligned INCR bursts","one active transaction","round-robin managers","abstract five-channel READY/VALID"]}],"initial_state":initial_state,"metrics":metrics,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null,"topology":topology,"implementation_coverage":{"profile":prepared.common.profile,"reproduction_conditions":"partially_identified","limitations":["public Registry/Envelope event codec API unavailable","start timestamp and run_id collected at export rather than execution start","build provenance compiler/toolchain/commit/lock unavailable"]}});
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
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::read) {
        result["binary_sha256"] = json!(digest(&exe));
    }
    result["os"] = json!(std::env::consts::OS);
    result["cpu"] = json!(std::env::consts::ARCH);
    Ok(result)
}
