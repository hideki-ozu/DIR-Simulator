//! Schema2 resource projections and exact per-resource transaction metrics.
use super::aggregate::{MetricValue, Record, finalize, ratio};
use super::json::{canonical, opt_d};
use super::publish::digest;
use crate::snapshot::{Snapshot, memory_ipc::*};
use crate::types::{Diagnostic, PreparedSimulation, memory_ipc::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub(super) const METRICS: &str = "memory_ipc.offered memory_ipc.completed memory_ipc.rejected memory_ipc.committed_bytes memory_ipc.queue_length memory_ipc.queue_mean memory_ipc.busy_ratio memory_ipc.latency_ps memory_ipc.ddr_refreshes memory_ipc.notifications";
pub(super) fn descriptor(id: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match id {
        "memory_ipc.committed_bytes" => ("byte", "integer", "summary", "sum"),
        "memory_ipc.queue_length" => ("count", "integer", "point", "identity"),
        "memory_ipc.queue_mean" => ("count", "number", "summary", "time_mean"),
        "memory_ipc.busy_ratio" => ("1", "number", "summary", "occupancy_ratio"),
        "memory_ipc.latency_ps" => ("ps", "integer", "point", "identity"),
        _ => ("count", "integer", "summary", "sum"),
    }
}
fn overflow() -> Diagnostic {
    Diagnostic {
        schema_version: 1,
        code: "E-0004".into(),
        stage: "output".into(),
        message: "memory/IPC exact aggregation overflow".into(),
        details: None,
    }
}
fn checked_add(a: u128, b: u128) -> Result<u128, Diagnostic> {
    a.checked_add(b).ok_or_else(overflow)
}
fn checked_mul(a: u128, b: u128) -> Result<u128, Diagnostic> {
    a.checked_mul(b).ok_or_else(overflow)
}
fn record(target: &str, metric: &str, value: MetricValue, start: u64, end: u64) -> Record {
    let mut r = Record::aggregate(target, metric, value, start, end);
    let (unit, kind, _, _) = descriptor(metric);
    r.unit = unit;
    r.value_kind = kind;
    r
}
pub(super) fn records(
    p: &PreparedSimulation,
    s: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let m = p.memory_ipc.as_ref().unwrap();
    let st = s
        .memory_ipc
        .as_ref()
        .ok_or_else(|| Diagnostic::output("missing memory/IPC snapshot"))?;
    let h = s.common.end_ps;
    let mut points = Vec::new();
    let mut summaries = Vec::new();
    for (order, point) in s.common.points.iter().enumerate() {
        if point.time_ps >= h && !s.common.partial {
            continue;
        }
        let mut r = Record::point(
            point,
            &point.metric,
            MetricValue::Integer(point.value as u128),
            order,
        );
        let (unit, kind, _, _) = descriptor(&point.metric);
        r.unit = unit;
        r.value_kind = kind;
        points.push(r);
    }
    for (n, res) in m.resources.iter().enumerate() {
        let qs = st
            .requests
            .iter()
            .filter(|q| q.node == n)
            .collect::<Vec<_>>();
        let completed = qs.iter().filter(|q| q.status == "completed").count();
        let rejected = qs
            .iter()
            .filter(|q| matches!(q.status.as_str(), "rejected" | "failed"))
            .count();
        let bytes = qs
            .iter()
            .map(|q| {
                if res.kind == Kind::Dma {
                    q.committed as u128
                } else if q.status == "completed"
                    && q.reason.as_deref() == Some("ok")
                    && matches!(q.spec.op.as_str(), "write" | "publish" | "send")
                {
                    q.spec.bytes.as_ref().unwrap().len() as u128
                } else {
                    0
                }
            })
            .try_fold(0u128, checked_add)?;
        for (metric, value) in [
            ("memory_ipc.offered", qs.len() as u128),
            ("memory_ipc.completed", completed as u128),
            ("memory_ipc.rejected", rejected as u128),
            ("memory_ipc.committed_bytes", bytes),
        ] {
            summaries.push(record(&res.node, metric, MetricValue::Integer(value), 0, h));
        }
        let mut integral = 0u128;
        let mut prev = 0;
        let mut value = 0usize;
        for q in st.queues.iter().filter(|q| q.node == n && q.time <= h) {
            integral = checked_add(
                integral,
                checked_mul((q.time - prev) as u128, value as u128)?,
            )?;
            prev = q.time;
            value = q.value;
        }
        integral = checked_add(integral, checked_mul((h - prev) as u128, value as u128)?)?;
        summaries.push(record(
            &res.node,
            "memory_ipc.queue_mean",
            ratio(integral, h as u128)?,
            0,
            h,
        ));
        let busy: u128 = qs
            .iter()
            .filter_map(|q| {
                q.started.map(|start| {
                    let end = q.completed.or(q.planned).unwrap_or(h).min(h);
                    if res.kind == Kind::Dma {
                        q.completed.unwrap_or(h).min(h).saturating_sub(start) as u128
                    } else if matches!(q.reason.as_deref(), Some("memory_fault" | "full" | "empty"))
                        && q.status == "rejected"
                    {
                        0
                    } else {
                        end.saturating_sub(start) as u128
                    }
                })
            })
            .try_fold(0u128, checked_add)?;
        let denominator = checked_mul(
            h as u128,
            if res.kind == Kind::Sram {
                res.ports as u128
            } else {
                1
            },
        )?;
        summaries.push(record(
            &res.node,
            "memory_ipc.busy_ratio",
            ratio(busy, denominator)?,
            0,
            h,
        ));
        if res.kind == Kind::Ddr {
            summaries.push(record(
                &res.node,
                "memory_ipc.ddr_refreshes",
                MetricValue::Integer(st.resources[n].refreshes as u128),
                0,
                h,
            ));
        }
        if matches!(res.kind, Kind::Dma | Kind::Mailbox) {
            let count = if res.kind == Kind::Dma {
                qs.iter().filter(|q| q.notified.is_some()).count()
            } else {
                st.notifications
                    .iter()
                    .filter(|v| v.node == n && v.delivered.is_some())
                    .count()
            };
            summaries.push(record(
                &res.node,
                "memory_ipc.notifications",
                MetricValue::Integer(count as u128),
                0,
                h,
            ));
        }
    }
    Ok(finalize(points, summaries))
}
fn envelope(
    schema: &str,
    id: &str,
    node: &str,
    request: Option<&str>,
    origin: Option<&str>,
    time: u64,
    data: Value,
) -> Value {
    json!({"schema_name":schema,"schema_version":1,"record_id":id,"subject":node,"request_id":request,"origin_request_id":origin,"time_ps":time.to_string(),"data":data})
}
pub(super) fn model_records(p: &PreparedSimulation, s: &MemoryIpcSnapshot) -> Vec<Value> {
    let m = p.memory_ipc.as_ref().unwrap();
    let mut rows = Vec::new();
    for q in &s.requests {
        let res = &m.resources[q.node];
        let r = &q.spec;
        rows.push(envelope("memory-ipc.request",&q.id,&res.node,Some(&q.id),q.origin.map(|i|s.requests[i].id.as_str()),q.time,json!({"model":res.kind.name(),"op":r.op,"actor":r.actor,"status":q.status,"reason":q.reason,"generated_ps":q.generated.to_string(),"started_ps":opt_d(q.started),"planned_completion_ps":opt_d(q.planned),"completed_ps":opt_d(q.completed),"address":opt_d(r.address),"length":opt_d(r.length),"input_hex":r.bytes.as_ref().map(|b|hex(b)),"output_hex":q.output.as_ref().map(|b|hex(b)),"slot":q.slot,"port":q.port,"dispatch_ordinal":opt_d(q.dispatch),"bank":q.bank,"row":q.row,"row_hit":q.row_hit,"message_id":q.message,"src":r.src.map(|n|m.resources[n].node.as_str()),"dst":r.dst.map(|n|m.resources[n].node.as_str()),"src_address":opt_d(r.src_address),"dst_address":opt_d(r.dst_address),"committed_bytes":q.committed.to_string(),"data_done_ps":opt_d(q.data_done),"planned_notify_ps":opt_d(q.planned_notify),"notified_ps":opt_d(q.notified)})));
    }
    for (n, r) in m.resources.iter().enumerate() {
        let st = &s.resources[n];
        let ids = st
            .queue
            .iter()
            .map(|&i| s.requests[i].id.clone())
            .collect::<Vec<_>>();
        let active = st.active.map(|i| s.requests[i].id.as_str());
        let (schema, data) = match r.kind {
            Kind::Ddr | Kind::Sram => (
                "memory-ipc.memory",
                json!({"kind":r.kind.name(),"size":r.size.to_string(),"hex":hex(&st.bytes),"open_rows":st.open_rows,"ports":st.ports.iter().map(|p|p.map(|i|s.requests[i].id.as_str())).collect::<Vec<_>>(),"refresh_pending":st.refresh_pending,"refresh_started_ps":opt_d(st.refresh_started),"refresh_planned_end_ps":opt_d(st.refresh_planned_end),"refresh_ended_ps":opt_d(st.refresh_ended),"queue":ids}),
            ),
            Kind::Shared => (
                "memory-ipc.shared",
                json!({"slots":st.slots.iter().enumerate().map(|(i,sl)|json!({"index":i,"state":sl.state,"owner":sl.owner,"message_id":sl.message,"hex":hex(&sl.bytes)})).collect::<Vec<_>>(),"ready":st.ready,"active":active,"queue":ids}),
            ),
            Kind::Dma => (
                "memory-ipc.dma",
                json!({"active":active,"queue":ids,"child":st.child.map(|i|s.requests[i].id.as_str()),"chunk_index":st.chunk_index.to_string(),"chunk_hex":st.chunk_hex.as_ref().map(|b|hex(b))}),
            ),
            Kind::Mailbox => (
                "memory-ipc.mailbox",
                json!({"messages":st.messages.iter().map(|v|json!({"message_id":v.id,"hex":hex(&v.bytes),"enqueued_ps":v.enqueued.to_string()})).collect::<Vec<_>>(),"active":active,"queue":ids}),
            ),
        };
        rows.push(envelope(
            schema, &r.node, &r.node, None, None, st.time, data,
        ));
    }
    for v in &s.notifications {
        let r = &m.resources[v.node];
        rows.push(envelope("memory-ipc.notification",&v.id,&r.node,None,None,v.delivered.unwrap_or(v.enqueued),json!({"message_id":v.id,"receivers":r.consumers,"enqueued_ps":v.enqueued.to_string(),"planned_ps":v.planned.to_string(),"delivered_ps":opt_d(v.delivered)})));
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
    p: &PreparedSimulation,
    timestamp: &str,
    sources: Vec<Value>,
    hashes: Vec<Value>,
    mut config: BTreeMap<String, String>,
) -> Result<Value, Diagnostic> {
    let m = p.memory_ipc.as_ref().unwrap();
    config.insert("model-profile".into(), canonical(&json!(p.common.profile)));
    if let Some(source) = sources.iter().find(|s| s["logical_path"] == "model-config") {
        config.insert("model-config".into(), canonical(&source["canonical_path"]));
        config.insert(
            "model-config-content".into(),
            source["content_utf8"].as_str().unwrap_or("").into(),
        );
    }
    let config = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect::<Vec<_>>();
    let mut metrics=METRICS.split_whitespace().map(|metric|{let (unit,kind,sampling,aggregation)=descriptor(metric);json!({"metric_id":metric,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation})}).collect::<Vec<_>>();
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    let mut initial_state = vec![json!({"instance":p.common.network,"state":"{}"})];
    for r in &m.resources {
        let data = match r.kind {
            Kind::Ddr | Kind::Sram => {
                json!({"kind":r.kind.name(),"size":r.size.to_string(),"hex":hex(&r.initial),"open_rows":vec![Value::Null;r.banks],"ports":vec![Value::Null;r.ports],"refresh_pending":false,"refresh_started_ps":null,"refresh_planned_end_ps":null,"refresh_ended_ps":null,"queue":[]})
            }
            Kind::Shared => {
                json!({"slots":(0..r.slots).map(|i|json!({"index":i,"state":"free","owner":null,"message_id":null,"hex":""})).collect::<Vec<_>>(),"ready":[],"active":null,"queue":[]})
            }
            Kind::Dma => {
                json!({"active":null,"queue":[],"child":null,"chunk_index":"0","chunk_hex":null})
            }
            Kind::Mailbox => json!({"messages":[],"active":null,"queue":[]}),
        };
        initial_state.push(json!({"instance":r.node,"state":canonical(&data)}));
    }
    for node in &p.common.module_paths {
        if !initial_state.iter().any(|v| v["instance"] == *node) {
            initial_state.push(json!({"instance":node,"state":"{}"}));
        }
    }
    initial_state.sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let mut schemas = [
        "memory-ipc.request",
        "memory-ipc.memory",
        "memory-ipc.shared",
        "memory-ipc.dma",
        "memory-ipc.mailbox",
        "memory-ipc.notification",
    ]
    .iter()
    .map(|s| json!({"schema_name":s,"schema_version":1}))
    .collect::<Vec<_>>();
    schemas.sort_by(|a, b| a["schema_name"].as_str().cmp(&b["schema_name"].as_str()));
    let mut codecs=[("Offer",1),("Complete",0),("RefreshDue",0),("RefreshEnd",0),("DmaSetup",0),("DmaResponse",1),("DmaNotify",1),("MailNotify",1)].iter().map(|(kind,phase)|json!({"protocol":"dir.memory-ipc.transaction","message":kind,"schema_name":format!("dir.memory-ipc.transaction.{kind}"),"schema_version":1,"phase":phase})).collect::<Vec<_>>();
    codecs.sort_by(|a, b| a["schema_name"].as_str().cmp(&b["schema_name"].as_str()));
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":p.common.profile,"models":[{"type":p.common.profile,"version":"1","protocol":"dir.memory-ipc.transaction","assumptions":["deterministic abstract transaction service","completion-time byte visibility","FIFO admission before dispatch","DMA shares configured memory resources"]}],"model_schemas":schemas,"message_schemas":codecs,"metrics":metrics,"initial_state":initial_state,"time_resolution_ps":"1","window_ps":p.common.metrics_window_ps.to_string(),"seed":null,"topology":{"resources":m.resources.iter().map(|r|json!({"id":r.node,"kind":r.kind.name()})).collect::<Vec<_>>()},"implementation_coverage":{"profile":p.common.profile,"reproduction_conditions":"partially_identified","limitations":["abstract policy does not model physical DDR commands or cache coherence","public Registry/Envelope API unavailable","build compiler/toolchain/commit provenance unavailable"]}});
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
    if let Ok(bytes) = std::env::current_exe().and_then(std::fs::read) {
        result["binary_sha256"] = json!(digest(&bytes));
    }
    Ok(result)
}
