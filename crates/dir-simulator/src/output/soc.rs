//! Exact journal projection and thirteen registered SoC metric descriptors.
use super::aggregate::{MetricValue, Record, finalize, mul, ratio};
use super::json::{canonical, opt_d};
use super::publish::digest;
use crate::snapshot::{Snapshot, soc::SocSnapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub(super) fn descriptor(metric: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match metric.split('.').next_back().unwrap_or("") {
        "queue_mean" => ("count", "number", "summary", "time_mean"),
        "queue_max" => ("count", "integer", "summary", "max"),
        "wait_mean_ps" | "latency_mean_ps" => ("ps", "number", "summary", "sample_mean"),
        "utilization" => ("1", "number", "window_summary", "occupancy_ratio"),
        "delivered_bits" => ("bit", "integer", "window_summary", "sum"),
        "throughput_bps" => ("bit/s", "number", "window_summary", "rate"),
        _ => ("count", "integer", "summary", "sum"),
    }
}
const NAMES: [&str; 13] = [
    "generated",
    "completed",
    "dropped",
    "pending",
    "active",
    "errors",
    "queue_mean",
    "queue_max",
    "wait_mean_ps",
    "latency_mean_ps",
    "utilization",
    "delivered_bits",
    "throughput_bps",
];
pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let m = prepared.soc.as_ref().unwrap();
    let s = snapshot
        .soc
        .as_ref()
        .ok_or_else(|| Diagnostic::output("missing SoC snapshot"))?;
    let h = snapshot.common.end_ps;
    let p = m.prefix();
    let metric = |n: &str| format!("{p}.{n}");
    let mut summary = Vec::new();
    let mut windows = Vec::new();
    for target in std::iter::once("$all").chain(m.sources.iter().map(|s| s.node.as_str())) {
        let rows: Vec<_> = s
            .transactions
            .iter()
            .filter(|r| {
                target == "$all" || m.sources[m.generators[r.generator].source].node == target
            })
            .collect();
        for (name, count) in [
            ("generated", rows.len()),
            (
                "completed",
                rows.iter().filter(|r| r.status == "completed").count(),
            ),
            (
                "dropped",
                rows.iter().filter(|r| r.status == "dropped").count(),
            ),
            (
                "pending",
                rows.iter().filter(|r| r.status == "pending").count(),
            ),
            (
                "active",
                rows.iter().filter(|r| r.status == "active").count(),
            ),
            (
                "errors",
                rows.iter()
                    .filter(|r| r.status == "completed" && r.response.as_deref() == Some("ERROR"))
                    .count(),
            ),
        ] {
            summary.push(Record::aggregate(
                target,
                &metric(name),
                MetricValue::Integer(count as u128),
                0,
                h,
            ));
        }
        for (name, start) in [("wait_mean_ps", true), ("latency_mean_ps", false)] {
            let samples: Vec<u64> = rows
                .iter()
                .filter_map(|r| {
                    if start { r.start_ps } else { r.completed_ps }.map(|t| t - r.generated_ps)
                })
                .collect();
            let mut row = Record::aggregate(
                target,
                &metric(name),
                ratio(
                    samples.iter().map(|&n| n as u128).sum(),
                    samples.len() as u128,
                )?,
                0,
                h,
            );
            row.sample_count = Some(samples.len() as u128);
            summary.push(row);
        }
    }
    for (id, g) in &s.queues {
        summary.push(Record::aggregate(
            id,
            &metric("queue_mean"),
            ratio(g.area(h), h as u128)?,
            0,
            h,
        ));
        summary.push(Record::aggregate(
            id,
            &metric("queue_max"),
            MetricValue::Integer(g.maximum as u128),
            0,
            h,
        ));
    }
    let mut intervals = vec![(0, h, true)];
    let mut start = 0;
    while start < h {
        let end = start
            .saturating_add(prepared.common.metrics_window_ps)
            .min(h);
        if end <= start {
            return Err(Diagnostic::output("metrics window must be positive"));
        }
        intervals.push((start, end, false));
        start = end;
    }
    for (start, end, full) in intervals {
        let out = if full { &mut summary } else { &mut windows };
        for target in std::iter::once("$all").chain(m.sources.iter().map(|s| s.node.as_str())) {
            let bits: u128 = s
                .transactions
                .iter()
                .filter(|r| {
                    r.response.as_deref() == Some("OKAY")
                        && r.completed_ps
                            .is_some_and(|t| full || (start <= t && t < end))
                        && (target == "$all"
                            || m.sources[m.generators[r.generator].source].node == target)
                })
                .map(|r| m.generators[r.generator].transaction.bytes as u128 * 8)
                .sum();
            out.push(Record::aggregate(
                target,
                &metric("delivered_bits"),
                MetricValue::Integer(bits),
                start,
                end,
            ));
            out.push(Record::aggregate(
                target,
                &metric("throughput_bps"),
                ratio(mul(bits, 1_000_000_000_000)?, (end - start) as u128)?,
                start,
                end,
            ));
        }
        for (resource, busy) in &s.busy {
            let duration: u128 = busy
                .iter()
                .map(|&(a, b)| b.min(end).saturating_sub(a.max(start)) as u128)
                .sum();
            out.push(Record::aggregate(
                resource,
                &metric("utilization"),
                ratio(duration, (end - start) as u128)?,
                start,
                end,
            ));
        }
    }
    Ok(finalize(windows, summary))
}
pub(super) fn model_records(prepared: &PreparedSimulation, s: &SocSnapshot) -> Vec<Value> {
    let m = prepared.soc.as_ref().unwrap();
    let envelope = |schema: String,
                    id: String,
                    subject: &str,
                    request: &str,
                    t: u64,
                    data: Value| json!({"schema_name":schema,"schema_version":1,"record_id":id,"subject":subject,"request_id":request,"origin_request_id":null,"time_ps":t.to_string(),"data":data});
    let mut rows = Vec::new();
    for r in &s.transactions {
        let g = &m.generators[r.generator];
        let t = &g.transaction;
        let plan=r.active_plan.as_ref().map(|p|json!({"resource":p.resource,"hop":p.hop.to_string(),"start_ps":p.start_ps.to_string(),"planned_end_ps":p.planned_end_ps.to_string()}));
        rows.push(envelope(format!("{}.transaction",m.prefix()),r.id.clone(),&m.sources[g.source].node,&r.id,r.time_ps,json!({"source":m.sources[g.source].node,"target":r.target,"operation":t.operation,"address":opt_d(t.address),"bytes":t.bytes.to_string(),"generated_ps":r.generated_ps.to_string(),"status":r.status,"start_ps":opt_d(r.start_ps),"completed_ps":opt_d(r.completed_ps),"response":r.response,"drop_reason":r.drop_reason,"active_plan":plan})));
    }
    for t in &s.transfers {
        let r = &s.transactions[t.request];
        let p = &t.plan;
        let bytes = m.generators[r.generator].transaction.bytes;
        rows.push(envelope(format!("{}.transfer",m.prefix()),format!("{}:{}",r.id,p.hop),&p.resource,&r.id,t.end_ps,json!({"hop":p.hop.to_string(),"from":p.from,"to":p.to,"start_ps":p.start_ps.to_string(),"end_ps":t.end_ps.to_string(),"bytes":bytes.to_string(),"response":p.response})));
    }
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
    let m = prepared.soc.as_ref().unwrap();
    let p = m.prefix();
    config.insert("model-profile".into(), canonical(&json!(m.profile)));
    config.insert("clock_period".into(), format!("{}ps", m.clock_period));
    config.insert("effective.model".into(),canonical(&json!({"profile":m.profile,"clock_period_ps":m.clock_period.to_string(),"bus":m.bus,"arbitration":m.arbitration,"bytes_per_cycle":m.bytes_per_cycle.to_string(),"link_cycles":m.link_cycles.to_string(),"input_capacity":m.input_capacity.to_string(),"sources":m.sources.iter().map(|s|json!({"node":s.node,"capacity":s.capacity.to_string(),"priority":s.priority.to_string(),"router":s.router.map(|r|m.routers[r].node.clone())})).collect::<Vec<_>>(),"targets":m.targets.iter().map(|t|json!({"node":t.node,"base":t.base.to_string(),"size":t.size.to_string(),"cycles":t.cycles.to_string(),"errors":t.errors})).collect::<Vec<_>>(),"routers":m.routers.iter().map(|r|json!({"node":r.node,"x":r.x.to_string(),"y":r.y.to_string()})).collect::<Vec<_>>() })));
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let mut metrics:Vec<_>=NAMES.iter().map(|n|{let id=format!("{p}.{n}");let(unit,value_kind,sampling,aggregation)=descriptor(&id);json!({"metric_id":id,"version":"1","unit":unit,"value_kind":value_kind,"sampling":sampling,"aggregation":aggregation})}).collect();
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    let topology = json!({"bus":m.bus,"clock_period_ps":m.clock_period.to_string(),"arbitration":m.arbitration,"bytes_per_cycle":m.bytes_per_cycle.to_string(),"link_cycles":m.link_cycles.to_string(),"input_capacity":m.input_capacity.to_string(),"sources":m.sources.iter().map(|s|json!({"id":s.node,"capacity":s.capacity.to_string(),"priority":s.priority.to_string(),"router":s.router.map(|r|m.routers[r].node.clone())})).collect::<Vec<_>>(),"targets":m.targets.iter().map(|t|json!({"id":t.node,"base":t.base.to_string(),"size":t.size.to_string(),"cycles":t.cycles.to_string(),"error_ranges":t.errors.iter().map(|(a,b)|json!({"start":a.to_string(),"end":b.to_string()})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"routers":m.routers.iter().map(|r|json!({"id":r.node,"x":r.x.to_string(),"y":r.y.to_string()})).collect::<Vec<_>>(),"connections":m.edges.iter().map(|(from,to)|json!({"from":from,"to":to})).collect::<Vec<_>>()});
    let mut initial_state:Vec<_>=m.nodes.iter().map(|(id,role)|{let state=if role.ends_with("Bus"){json!({"queue":[],"cursor":"0","active":null,"dirty":false})}else if role.ends_with("Router"){let queues:Vec<_>=m.edges.values().filter_map(|to|to.strip_prefix(&format!("{id}."))).map(|gate|json!({"id":format!("{id}:{gate}"),"queue":[],"reserved":"0"})).collect();let outputs:Vec<_>=m.edges.keys().filter_map(|from|from.strip_prefix(&format!("{id}."))).map(|gate|json!({"id":format!("{id}:{gate}"),"cursor":"0","active":null})).collect();json!({"inputs":queues,"outputs":outputs})}else{json!({"queue":[],"active":null,"generators":m.generators.iter().filter(|g|m.sources[g.source].node==*id).map(|g|json!({"id":g.id,"next_ordinal":"0"})).collect::<Vec<_>>()})};json!({"instance":id,"state":canonical(&state)})}).collect();
    initial_state.push(json!({"instance":prepared.common.network,"state":"{}"}));
    initial_state.extend(
        prepared
            .common
            .module_paths
            .iter()
            .filter(|id| **id != prepared.common.network)
            .map(|id| json!({"instance":id,"state":"{}"})),
    );
    initial_state.sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(source_hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":m.profile,"model_schemas":[{"schema_name":format!("{p}.transaction"),"schema_version":1},{"schema_name":format!("{p}.transfer"),"schema_version":1}],"models":[{"type":m.profile,"version":"1","protocol":p,"capability":"transaction-evaluation","assumptions":["abstract byte quantities","nonpreemptive resources","finite FIFO capacity","deterministic clock-edge arbitration"]}],"initial_state":initial_state,"metrics":metrics,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null,"topology":topology,"soc_topology":topology,"implementation_coverage":{"profile":m.profile,"reproduction_conditions":"partially_identified","limitations":["abstract transaction model without signal or data-value fidelity","public Registry/Envelope event codec API unavailable"]}});
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
    result["os"] = json!(std::env::consts::OS);
    result["cpu"] = json!(std::env::consts::ARCH);
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::read) {
        result["binary_sha256"] = json!(digest(&exe));
    }
    Ok(result)
}
