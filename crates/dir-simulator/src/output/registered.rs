//! Common schema-2 publication for statically registered models.
use super::{json::canonical, publish::digest};
use crate::{Diagnostic, PreparedSimulation, snapshot::Snapshot};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn parameter_value(
    value: &crate::registry::ParameterValue,
    dimension: crate::registry::Dimension,
) -> String {
    use crate::registry::{Dimension, ParameterValue};
    match value {
        ParameterValue::Integer(n) => n.to_string(),
        ParameterValue::Double(n) => canonical(&json!(n)),
        ParameterValue::Boolean(v) => v.to_string(),
        ParameterValue::String(v) => canonical(&json!(v)),
        ParameterValue::Quantity(n) => format!(
            "{n}{}",
            match dimension {
                Dimension::Time => "ps",
                Dimension::Bitrate => "bit/s",
                Dimension::Data => "byte",
                Dimension::Dimensionless => "",
            }
        ),
    }
}

pub(super) fn metadata(
    prepared: &PreparedSimulation,
    timestamp: &str,
    sources: Vec<Value>,
    hashes: Vec<Value>,
    mut config: BTreeMap<String, String>,
) -> Result<Value, Diagnostic> {
    if prepared
        .registered
        .as_ref()
        .is_some_and(|registered| registered.network.is_some())
    {
        return super::network::metadata(prepared, timestamp, sources, hashes, config);
    }
    let registered = prepared
        .registered
        .as_ref()
        .ok_or_else(|| Diagnostic::output("registered configuration missing"))?;
    let profile = registered
        .registry
        .profile(&prepared.common.profile)
        .ok_or_else(|| Diagnostic::output("registered profile missing"))?;
    for model in &registered.models {
        for (key, value) in &model.parameters {
            config.insert(
                format!("{}.{key}", model.id),
                parameter_value(
                    value,
                    registered.registry.modules[&model.implementation_key]
                        .descriptor
                        .parameters
                        .iter()
                        .find(|p| p.name == *key)
                        .map(|p| p.dimension)
                        .unwrap_or(crate::registry::Dimension::Dimensionless),
                ),
            );
        }
    }
    for channel in &registered.channels {
        for (key, value) in &channel.parameters {
            config.insert(
                format!("{}.{key}", channel.id),
                parameter_value(
                    value,
                    registered.registry.channels[&channel.implementation_key]
                        .descriptor
                        .parameters
                        .iter()
                        .find(|p| p.name == *key)
                        .map(|p| p.dimension)
                        .unwrap_or(crate::registry::Dimension::Dimensionless),
                ),
            );
        }
    }
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let metrics:Vec<_>=profile.metrics.iter().filter(|name|registered.registry.metrics.contains_key(*name)).map(|name| {let unit=registered.registry.metrics.get(name).map(|m|m.unit.as_str()).unwrap_or("count");json!({"metric_id":name,"version":"1","unit":unit,"value_kind":"integer","sampling":"point","aggregation":"identity"})}).collect();
    let schemas: Vec<_> = profile
        .model_records
        .iter()
        .map(|s| json!({"schema_name":s.name,"schema_version":s.version}))
        .collect();
    let models:BTreeMap<_,_>=registered.models.iter().map(|m|(m.implementation_key.clone(),json!({"type":m.implementation_key,"version":registered.registry.modules[&m.implementation_key].descriptor.implementation_version,"assumptions":[]}))).collect();
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":profile.name,"models":models.into_values().collect::<Vec<_>>(),"initial_state":registered.models.iter().map(|m|json!({"instance":m.id,"state":canonical(&json!({"parameters":m.parameters}))})).collect::<Vec<_>>(),"metrics":metrics,"model_schemas":schemas,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null,"implementation_coverage":{"profile":profile.name,"reproduction_conditions":"identified","limitations":[]}});
    let metrics_v2 = metric_catalog_v2(prepared);
    if !metrics_v2.is_empty() {
        result["metrics_v2"] = json!(metrics_v2);
    }
    Ok(result)
}

pub(super) fn metric_catalog_v2(prepared: &PreparedSimulation) -> Vec<Value> {
    let registered = prepared
        .registered
        .as_ref()
        .expect("registered metric catalog");
    let profile = &registered.registry.profiles[&prepared.common.profile];
    profile.metrics.iter().filter_map(|name|registered.registry.metrics_v2.get(name)).map(|m|json!({"name":m.descriptor.name,"unit":m.descriptor.unit,"version":m.descriptor.version.to_string(),"value_kind":m.descriptor.value_kind,"sampling":m.descriptor.sampling,"aggregation":m.descriptor.aggregation,"targets":m.targets,"initial_gauges":m.initial_gauges.iter().map(|(k,v)|(k,v.to_string())).collect::<BTreeMap<_,_>>(),"max_metric_targets":registered.registry.metric_v2_limits.max_metric_targets.to_string(),"max_observations_per_batch":registered.registry.metric_v2_limits.max_observations_per_batch.to_string()})).collect()
}

fn row(prepared: &PreparedSimulation, seq: usize, p: &crate::snapshot::Point) -> Value {
    let unit = prepared
        .registered
        .as_ref()
        .unwrap()
        .registry
        .metrics
        .get(&p.metric)
        .map(|m| m.unit.as_str())
        .unwrap_or("count");
    json!({"seq":seq.to_string(),"event_seq":p.event_seq.map(|n|n.to_string()),"effect_seq":p.effect_seq.map(|n|n.to_string()),"target":p.target,"metric":p.metric,"unit":unit,"value_kind":"integer","value":p.value.to_string(),"time_ps":p.time_ps.to_string(),"start_ps":null,"end_ps":null,"request_id":p.request_id,"receiver":p.receiver,"reason":p.reason,"sample_count":null})
}
fn bytes(writer: &mut dyn std::io::Write, value: &[u8]) -> Result<(), Diagnostic> {
    writer
        .write_all(value)
        .map_err(|e| Diagnostic::output(format!("Cannot write registered result: {e}")))
}
fn value(writer: &mut dyn std::io::Write, v: &Value) -> Result<(), Diagnostic> {
    bytes(writer, canonical(v).as_bytes())
}
// Length-prefixed root namespace is shared with the specified network identifier format.
fn qualified_frame_id(id: &str) -> String {
    let schema = "ethernet.dynamic.frame";
    format!("{}:{schema}{}:{id}", schema.len(), id.len())
}
fn model_record_value(
    prepared: &PreparedSimulation,
    record: &crate::registry::ModelRecord,
    records: &BTreeMap<String, crate::registry::ModelRecord>,
) -> Value {
    let network = prepared.registered.as_ref().unwrap().network.is_some();
    let mut data = record.data.clone();
    // Project-owned protocol profiles use the same exact decimal sequence
    // representation as network records. JSON numbers lose u64 precision in JS.
    if matches!(
        prepared.common.profile.as_str(),
        "axi4.concurrent.v1"
            | "ahb.pipeline.v1"
            | "noc.flit.v1"
            | "soc.execution.v1"
            | "ethernet.phy.v1"
            | "can.cc.protocol.v1"
            | "can.mixed.protocol.v1"
            | "can.gateway.protocol.v1"
    ) {
        if let Some(value) = data.get_mut("effect_seq") {
            if let Some(sequence) = value.as_u64() {
                *value = json!(sequence.to_string());
            }
        }
    }

    let mut subject = record.subject.clone();
    let mut request = Value::Null;
    let mut origin = Value::Null;
    if network {
        request = data
            .get("frame_id")
            .or_else(|| data.get("request_id"))
            .cloned()
            .unwrap_or(Value::Null);
        if record.schema.name.ends_with(".frame") {
            request = json!(record.id);
        }
        origin = data.get("origin_id").cloned().unwrap_or(Value::Null);
        let referenced = |schema: &str, id: &Value| -> Option<&Value> {
            id.as_str()
                .and_then(|id| records.get(&format!("{schema}:{id}")))
                .map(|record| &record.data)
        };
        match record.schema.name.as_str() {
            "dir.redundancy.origin" | "dir.redundancy.branch" | "dir.redundancy.recovery" => {
                subject = data["stream_id"].as_str().unwrap_or(&subject).into();
                let root = if record.schema.name == "dir.redundancy.origin" {
                    Some(&data)
                } else {
                    referenced("dir.redundancy.origin", &data["origin_id"])
                };
                request = root
                    .map(|root| root["source_frame_id"].clone())
                    .unwrap_or(Value::Null);
                origin = data["origin_id"].clone();
            }
            "dir.preemption.transfer"
            | "dir.preemption.fragment"
            | "dir.preemption.gap"
            | "dir.preemption.decision"
            | "dir.preemption.delivery" => {
                let transfer_id = data
                    .get("transfer_id")
                    .filter(|id| !id.is_null())
                    .or_else(|| data.get("active_transfer_id").filter(|id| !id.is_null()))
                    .or_else(|| data.get("suspended_transfer_id").filter(|id| !id.is_null()))
                    .cloned()
                    .unwrap_or(Value::Null);
                let transfer = referenced("dir.preemption.transfer", &transfer_id);
                request = data
                    .get("frame_id")
                    .cloned()
                    .or_else(|| transfer.map(|t| t["frame_id"].clone()))
                    .unwrap_or(Value::Null);
                if record.schema.name == "dir.preemption.delivery" {
                    subject = referenced("ethernet.dynamic.frame", &request)
                        .and_then(|f| f["source"].as_str())
                        .unwrap_or(&subject)
                        .into();
                } else {
                    subject = data
                        .get("from_port")
                        .or_else(|| data.get("port"))
                        .and_then(Value::as_str)
                        .unwrap_or(&subject)
                        .into();
                }
                origin = request
                    .as_str()
                    .map(|id| json!(qualified_frame_id(id)))
                    .unwrap_or(Value::Null);
            }
            "dir.composite.ingress" => {
                if let Some(conversion) =
                    referenced("dir.can_ethernet.conversion", &data["conversion_id"])
                {
                    subject = conversion["gateway"].as_str().unwrap_or(&subject).into();
                    request = conversion["parent_id"].clone();
                    origin = conversion["origin_id"].clone();
                }
            }
            "ethernet.control.wire.event" | "ethernet.control.wire.transition" => {
                subject = data
                    .get("port")
                    .and_then(Value::as_str)
                    .filter(|p| !p.is_empty())
                    .or_else(|| data.get("bridge").and_then(Value::as_str))
                    .unwrap_or(&subject)
                    .into();
                request = data.get("packet_id").cloned().unwrap_or(Value::Null);
                origin = Value::Null;
            }
            "ethernet.wire.boundary" | "ethernet.wire.membership" => {
                subject = data["ingress_port"].as_str().unwrap_or(&subject).into();
                request = data.get("packet_id").cloned().unwrap_or(Value::Null);
                origin = Value::Null;
            }
            "ethernet.wire.membership_timer" => {
                subject = data["switch"].as_str().unwrap_or(&subject).into();
            }

            "ethernet.frame" => {
                subject = data["source"].as_str().unwrap_or(&subject).into();
                data.as_object_mut().unwrap().remove("effect_seq");
                origin = Value::Null;
            }
            "ethernet.transfer" => {
                subject = data["from_port"].as_str().unwrap_or(&subject).into();
                data.as_object_mut().unwrap().remove("effect_seq");
                origin = Value::Null;
            }
            "ethernet.reception" => {
                subject = data["ingress"]
                    .as_str()
                    .and_then(|ingress| ingress.rsplit_once('.').map(|(owner, _)| owner))
                    .unwrap_or(&subject)
                    .into();
                data.as_object_mut().unwrap().remove("effect_seq");
                origin = Value::Null;
            }
            "can.request" => {
                subject = data["source"].as_str().unwrap_or(&subject).into();
                origin = data["model_fields"]["origin_request_id"].clone();
                data.as_object_mut().unwrap().remove("effect_seq");
            }
            "can.receiver" => {
                subject = data["receiver"].as_str().unwrap_or(&subject).into();
                origin = request.clone();
                data.as_object_mut().unwrap().remove("effect_seq");
            }
            _ => {}
        }
    }
    json!({"schema_name":record.schema.name,"schema_version":record.schema.version,"record_id":record.id,"subject":subject,"request_id":request,"origin_request_id":origin,"time_ps":record.time_ps.to_string(),"data":data})
}

pub(super) fn write_result(
    writer: &mut dyn std::io::Write,
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    run_id: &str,
    timestamp: &str,
) -> Result<(), Diagnostic> {
    let registered = snapshot
        .registered
        .as_ref()
        .ok_or_else(|| Diagnostic::output("registered snapshot missing"))?;
    let descriptor = prepared
        .registered
        .as_ref()
        .and_then(|r| r.registry.profile(&prepared.common.profile))
        .ok_or_else(|| Diagnostic::output("registered profile missing"))?;
    let mut metadata = super::metadata::build(prepared, timestamp)?;
    if let Some(network) = &snapshot.network {
        metadata["network_runtime"] = network.clone();
    }
    if prepared.common.profile == "can.ethernet.gateway.v1" {
        if let Some(ethernet) = &snapshot.ethernet {
            for frame in &ethernet.frames {
                if let Some(flow_id) = &frame.flow_id {
                    let flows = metadata["flows"].as_array_mut().unwrap();
                    if !flows
                        .iter()
                        .any(|flow| flow["flow_id"].as_str() == Some(flow_id.as_str()))
                    {
                        flows.push(json!({"flow_id":flow_id,"priority":frame.priority.to_string(),"deadline_ps":frame.deadline_ps.map(|time|time.to_string()),"dst_mac":frame.wire.dst_mac,"tag":frame.wire.tag.as_ref().map(|tag|json!({"vid":tag.vid.to_string(),"pcp":tag.pcp.to_string(),"dei":tag.dei.to_string()})),"source_vlan_id":frame.source_vlan_id.map(|vid|vid.to_string())}));
                    }
                }
            }
            metadata["flows"]
                .as_array_mut()
                .unwrap()
                .sort_by(|a, b| a["flow_id"].as_str().cmp(&b["flow_id"].as_str()));
        }
    }
    if snapshot.common.termination == "prep_failed" {
        metadata["config"] = json!([]);
        metadata["config_sha256"] = json!(digest(b"[]"));
        metadata["initial_state"] = json!([]);
        metadata["initial_channel_state"] = json!([]);
    }
    bytes(writer, b"{\"metadata\":")?;
    value(writer, &metadata)?;
    bytes(writer, b",\"run_id\":")?;
    value(writer, &json!(run_id))?;
    bytes(
        writer,
        format!(
            ",\"schema_version\":{},\"simulation\":{{\"committed_events\":",
            if registered.control.is_some() {
                3
            } else {
                descriptor.output_schema_version
            }
        )
        .as_bytes(),
    )?;
    value(writer, &json!(snapshot.common.committed_events.to_string()))?;
    bytes(writer, b",\"end_ps\":")?;
    value(writer, &json!(snapshot.common.end_ps.to_string()))?;
    bytes(writer, b",\"last_event_time_ps\":")?;
    value(
        writer,
        &json!(snapshot.common.last_event_time_ps.map(|n| n.to_string())),
    )?;
    bytes(writer, b",\"model_records\":[")?;
    let mut model_records: Vec<_> = registered.model_records.values().collect();
    model_records.sort_by(|a, b| {
        (&a.schema.name, &a.subject, &a.id).cmp(&(&b.schema.name, &b.subject, &b.id))
    });
    for (i, r) in model_records.into_iter().enumerate() {
        if i > 0 {
            bytes(writer, b",")?;
        }
        value(
            writer,
            &model_record_value(prepared, r, &registered.model_records),
        )?;
    }
    bytes(writer, b"],\"partial\":")?;
    value(writer, &json!(snapshot.common.partial))?;
    bytes(writer, b",\"pending_events\":")?;
    value(writer, &json!(snapshot.common.pending_events.to_string()))?;
    bytes(writer, b",\"records\":[")?;
    for (seq, record) in metric_record_values(prepared, snapshot)?.iter().enumerate() {
        if seq > 0 {
            bytes(writer, b",")?;
        }
        value(writer, record)?;
    }
    bytes(writer, b"],\"start_ps\":\"0\",\"summary\":")?;
    let summary = if prepared.registered.as_ref().unwrap().network.is_some() {
        super::network::summary(prepared, snapshot)?
    } else {
        Vec::new()
    };
    let mut summary_values = summary
        .iter()
        .map(super::json::record_value)
        .collect::<Vec<_>>();
    summary_values.extend(metric_summary_values(prepared, snapshot)?);
    value(writer, &json!(summary_values))?;
    if !registered.metrics_v2.accumulators.is_empty() { /* V2 summaries are written by the combined projection below. */
    }
    bytes(writer, b",\"termination\":")?;
    value(writer, &json!(snapshot.common.termination))?;
    bytes(writer, b"}")?;
    if let Some(control) = &registered.control {
        bytes(writer, b",\"control\":")?;
        value(writer, control)?;
    }
    bytes(writer, b"}\n")
}

pub(super) fn write_csv(
    writer: &mut dyn std::io::Write,
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    run_id: &str,
    version: u32,
) -> Result<(), Diagnostic> {
    let fields = [
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
    bytes(writer,b"schema_version,run_id,seq,event_seq,effect_seq,target,metric,unit,value_kind,value,time_ps,start_ps,end_ps,request_id,receiver,reason,sample_count\n")?;
    let rows = metric_record_values(prepared, snapshot)?;
    for row in rows {
        bytes(writer, format!("{version},{run_id}").as_bytes())?;
        for field in fields {
            bytes(writer, b",")?;
            if let Some(text) = row[field].as_str() {
                if text.contains([',', '"', '\r', '\n']) {
                    bytes(
                        writer,
                        format!("\"{}\"", text.replace('"', "\"\"")).as_bytes(),
                    )?;
                } else {
                    bytes(writer, text.as_bytes())?;
                }
            }
        }
        bytes(writer, b"\n")?;
    }
    Ok(())
}

pub(super) fn metric_summary_values(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<Vec<Value>, Diagnostic> {
    let registered = snapshot
        .registered
        .as_ref()
        .ok_or_else(|| Diagnostic::output("registered snapshot missing"))?;
    registered
        .metrics_v2
        .summary(
            &prepared.registered.as_ref().unwrap().registry,
            snapshot.common.end_ps,
            prepared.common.metrics_window_ps,
        )
        .map_err(|e| Diagnostic::output(e.0))
}
pub(super) fn write_summary_csv(
    writer: &mut dyn std::io::Write,
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    run_id: &str,
    version: u32,
) -> Result<(), Diagnostic> {
    bytes(writer,b"schema_version,run_id,seq,event_seq,effect_seq,target,metric,unit,value_kind,value,time_ps,start_ps,end_ps,request_id,receiver,reason,sample_count\n")?;
    for (seq, row) in metric_summary_values(prepared, snapshot)?
        .iter()
        .enumerate()
    {
        bytes(writer, format!("{version},{run_id},{seq}").as_bytes())?;
        for field in [
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
        ] {
            bytes(writer, b",")?;
            let text = match &row[field] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => String::new(),
            };
            if text.contains([',', '"', '\r', '\n']) {
                bytes(
                    writer,
                    format!("\"{}\"", text.replace('"', "\"\"")).as_bytes(),
                )?;
            } else {
                bytes(writer, text.as_bytes())?;
            }
        }
        bytes(writer, b"\n")?;
    }
    Ok(())
}

fn metric_record_values(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<Vec<Value>, Diagnostic> {
    let legacy = snapshot
        .common
        .iter_points()?
        .collect::<Result<Vec<_>, _>>()?;
    let metrics = &snapshot.registered.as_ref().unwrap().metrics_v2;
    if metrics.records.is_empty() {
        return Ok(legacy
            .iter()
            .enumerate()
            .map(|(seq, p)| row(prepared, seq, p))
            .collect());
    }
    let total = legacy
        .len()
        .checked_add(metrics.records.len())
        .ok_or_else(|| Diagnostic::output("record count overflow"))?;
    let metric_rows = metrics.record_values(&prepared.registered.as_ref().unwrap().registry, 0);
    let mut legacy = legacy.iter();
    let mut next_metric = metric_rows.into_iter().peekable();
    let mut result = Vec::with_capacity(total);
    for seq in 0..total {
        if next_metric
            .peek()
            .is_some_and(|v| v["seq"].as_str() == Some(seq.to_string().as_str()))
        {
            result.push(next_metric.next().unwrap());
        } else {
            result.push(row(
                prepared,
                seq,
                legacy
                    .next()
                    .ok_or_else(|| Diagnostic::output("metric journal sequence is inconsistent"))?,
            ));
        }
    }
    if next_metric.next().is_some() || legacy.next().is_some() {
        return Err(Diagnostic::output(
            "metric journal sequence is inconsistent",
        ));
    }
    Ok(result)
}
