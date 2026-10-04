//! CAN FD schema2 projection and the four explicitly registered counters.
use super::aggregate::{MetricValue, Record, finalize};
use super::json::{canonical, opt_d};
use super::publish::digest;
use crate::snapshot::Snapshot;
use crate::snapshot::canfd::CanFdSnapshot;
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) const METRICS: &str = "canfd.generated canfd.serialized canfd.dropped canfd.received";
pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let model = prepared.canfd.as_ref().expect("CAN FD input");
    let state = snapshot
        .canfd
        .as_ref()
        .ok_or_else(|| Diagnostic::output("missing CAN FD snapshot"))?;
    let mut records = Vec::new();
    for target in std::iter::once("$all").chain(model.controllers.iter().map(|c| c.id.as_str())) {
        let requests: Vec<_> = state
            .requests
            .iter()
            .filter(|r| {
                target == "$all"
                    || model.controllers[model.generators[r.generator].source].id == target
            })
            .collect();
        let received = state
            .receptions
            .iter()
            .filter(|r| {
                r.completed_ps.is_some()
                    && (target == "$all" || model.controllers[r.receiver].id == target)
            })
            .count();
        for (metric, value) in [
            ("canfd.generated", requests.len()),
            (
                "canfd.serialized",
                requests.iter().filter(|r| r.eof_ps.is_some()).count(),
            ),
            (
                "canfd.dropped",
                requests.iter().filter(|r| r.state == "dropped").count(),
            ),
            ("canfd.received", received),
        ] {
            records.push(Record::aggregate(
                target,
                metric,
                MetricValue::Integer(value as u128),
                0,
                snapshot.common.end_ps,
            ));
        }
    }
    Ok(finalize(Vec::new(), records))
}
pub(super) fn model_records(prepared: &PreparedSimulation, state: &CanFdSnapshot) -> Vec<Value> {
    let model = prepared.canfd.as_ref().expect("CAN FD input");
    let envelope = |schema: &str,
                    id: &str,
                    subject: &str,
                    request: Option<&str>,
                    time: u64,
                    data: Value| json!({"schema_name":schema,"schema_version":1,"record_id":id,"subject":subject,"request_id":request,"origin_request_id":null,"time_ps":time.to_string(),"data":data});
    let mut rows = Vec::new();
    for g in &model.generators {
        let f = &g.frame;
        rows.push(envelope("dir.canfd.frame",&g.id,&model.controllers[g.source].id,None,0,json!({"format":f.format,"id":f.id,"data":f.data,"dlc":f.dlc,"brs":f.brs,"nominal_bits":f.nominal_bits.to_string(),"data_bits":f.data_bits.to_string(),"evidence":f.evidence,"binding_sha256":f.binding_sha256,"nominal_rate":f.nominal_rate.to_string(),"data_rate":f.data_rate.to_string(),"fidelity":"externally-precomputed-phase-bits","wire_validation":"structural-only"})));
    }
    for r in &state.requests {
        let g = &model.generators[r.generator];
        let source = &model.controllers[g.source].id;
        rows.push(envelope("dir.canfd.request",&r.request_id,source,Some(&r.request_id),r.time_ps,json!({"frame_id":g.id,"source":source,"bus":model.bus_id,"generated_ps":r.generated_ps.to_string(),"ready_ps":opt_d(r.ready_ps),"sof_ps":opt_d(r.sof_ps),"eof_ps":opt_d(r.eof_ps),"release_ps":opt_d(r.release_ps),"planned_ready_ps":r.planned_ready_ps.to_string(),"planned_eof_ps":opt_d(r.planned_eof_ps),"planned_release_ps":opt_d(r.planned_release_ps),"state":r.state,"drop_reason":r.drop_reason})));
    }
    for reception in &state.receptions {
        let r = &state.requests[reception.request];
        let g = &model.generators[r.generator];
        let receiver = &model.controllers[reception.receiver].id;
        rows.push(envelope("dir.canfd.reception",&format!("{}:{receiver}",r.request_id),receiver,Some(&r.request_id),reception.time_ps,json!({"frame_id":g.id,"receiver":receiver,"planned_arrival_ps":reception.planned_arrival_ps.to_string(),"planned_completed_ps":reception.planned_completed_ps.to_string(),"arrival_ps":opt_d(reception.arrival_ps),"completed_ps":opt_d(reception.completed_ps),"state":reception.state})));
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
    let model = prepared.canfd.as_ref().expect("CAN FD input");
    config.insert(
        "model-profile".into(),
        canonical(&json!(prepared.common.profile)),
    );
    config.insert(
        format!("{}.nominalBitrate", model.bus_id),
        format!("{}bit/s", model.nominal_rate),
    );
    config.insert(
        format!("{}.dataBitrate", model.bus_id),
        format!("{}bit/s", model.data_rate),
    );
    config.insert(
        format!("{}.profile", model.bus_id),
        canonical(&json!(prepared.common.profile)),
    );
    let mut initial_state = vec![
        json!({"instance":prepared.common.network,"state":"{}"}),
        json!({"instance":model.bus_id,"state":canonical(&json!({"current":null,"release_token":null,"dirty":false,"nominal_rate":model.nominal_rate.to_string(),"data_rate":model.data_rate.to_string()}))}),
    ];
    for c in &model.controllers {
        for (name, value) in [
            ("queueCapacity", c.queue_capacity.to_string()),
            ("txProcessingDelay", format!("{}ps", c.tx_processing_ps)),
            ("rxProcessingDelay", format!("{}ps", c.rx_processing_ps)),
            ("rxFilter", canonical(&json!(c.rx_filter))),
        ] {
            config.insert(format!("{}.{name}", c.id), value);
        }
        initial_state.push(json!({"instance":c.id,"state":canonical(&json!({"queue":[],"generators":model.generators.iter().filter(|g|model.controllers[g.source].id==c.id).map(|g|json!({"id":g.id,"next_ordinal":"0","next_time_ps":g.times_ps.first().map(u64::to_string)})).collect::<Vec<_>>()}))}));
    }
    initial_state.extend(
        prepared
            .common
            .module_paths
            .iter()
            .map(|id| json!({"instance":id,"state":"{}"})),
    );
    initial_state.sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let mut metrics:Vec<_>=METRICS.split_whitespace().map(|id|json!({"metric_id":id,"version":"1","unit":"count","value_kind":"integer","sampling":"summary","aggregation":"sum"})).collect();
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    let topology = json!({"bus":{"id":model.bus_id,"nominal_rate":model.nominal_rate.to_string(),"data_rate":model.data_rate.to_string()},"controllers":model.controllers.iter().map(|c|json!({"id":c.id,"bus":model.bus_id,"queue_capacity":c.queue_capacity.to_string(),"tx_processing_delay_ps":c.tx_processing_ps.to_string(),"rx_processing_delay_ps":c.rx_processing_ps.to_string(),"rx_filter":c.rx_filter,"tx_channel_delay_ps":c.tx_channel_ps.to_string(),"rx_channel_delay_ps":c.rx_channel_ps.to_string()})).collect::<Vec<_>>()});
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(source_hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":prepared.common.profile,
        "model_schemas":[{"schema_name":"dir.canfd.frame","schema_version":1},{"schema_name":"dir.canfd.reception","schema_version":1},{"schema_name":"dir.canfd.request","schema_version":1}],
        "models":[{"type":prepared.common.profile,"version":"1","protocol":"dir.canfd","capability":"canfd-bus","assumptions":["externally supplied phase bit counts","structural-only wire validation","single active Bus","ideal ACK","error-active ESI","nonpreemptive arbitration","drop-newest ready queues","fixed independent processing and propagation delays"]}],"fidelity":"externally-precomputed-phase-bits","wire_validation":"structural-only","initial_state":initial_state,"metrics":metrics,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null,"topology":topology,"canfd_topology":topology,
        "implementation_coverage":{"profile":prepared.common.profile,"reproduction_conditions":"partially_identified","limitations":["external bit counts and evidence are not independently verified","public Registry/Envelope event codec API unavailable","start timestamp and run_id collected at export rather than execution start","build provenance compiler/toolchain/commit/lock unavailable"]}});
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
    if let Ok(executable) = std::env::current_exe().and_then(std::fs::read) {
        result["binary_sha256"] = json!(digest(&executable));
    }
    result["os"] = json!(std::env::consts::OS);
    result["cpu"] = json!(std::env::consts::ARCH);
    Ok(result)
}
