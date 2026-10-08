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
    let metrics:Vec<_>=profile.metrics.iter().map(|name| {let unit=registered.registry.metrics.get(name).map(|m|m.unit.as_str()).unwrap_or("count");json!({"metric_id":name,"version":"1","unit":unit,"value_kind":"integer","sampling":"point","aggregation":"identity"})}).collect();
    let schemas: Vec<_> = profile
        .model_records
        .iter()
        .map(|s| json!({"schema_name":s.name,"schema_version":s.version}))
        .collect();
    let models:BTreeMap<_,_>=registered.models.iter().map(|m|(m.implementation_key.clone(),json!({"type":m.implementation_key,"version":registered.registry.modules[&m.implementation_key].descriptor.implementation_version,"assumptions":[]}))).collect();
    Ok(
        json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":profile.name,"models":models.into_values().collect::<Vec<_>>(),"initial_state":registered.models.iter().map(|m|json!({"instance":m.id,"state":canonical(&json!({"parameters":m.parameters}))})).collect::<Vec<_>>(),"metrics":metrics,"model_schemas":schemas,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null,"implementation_coverage":{"profile":profile.name,"reproduction_conditions":"identified","limitations":[]}}),
    )
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
fn model_record_value(
    prepared: &PreparedSimulation,
    record: &crate::registry::ModelRecord,
) -> Value {
    let network = prepared.registered.as_ref().unwrap().network.is_some();
    let mut data = record.data.clone();
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
        match record.schema.name.as_str() {
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
            descriptor.output_schema_version
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
        value(writer, &model_record_value(prepared, r))?;
    }
    bytes(writer, b"],\"partial\":")?;
    value(writer, &json!(snapshot.common.partial))?;
    bytes(writer, b",\"pending_events\":")?;
    value(writer, &json!(snapshot.common.pending_events.to_string()))?;
    bytes(writer, b",\"records\":[")?;
    for (seq, p) in snapshot.common.iter_points()?.enumerate() {
        if seq > 0 {
            bytes(writer, b",")?;
        }
        value(writer, &row(prepared, seq, &p?))?;
    }
    bytes(writer, b"],\"start_ps\":\"0\",\"summary\":")?;
    let summary = if prepared.registered.as_ref().unwrap().network.is_some() {
        super::network::summary(prepared, snapshot)?
    } else {
        Vec::new()
    };
    value(
        writer,
        &json!(
            summary
                .iter()
                .map(super::json::record_value)
                .collect::<Vec<_>>()
        ),
    )?;
    bytes(writer, b",\"termination\":")?;
    value(writer, &json!(snapshot.common.termination))?;
    bytes(writer, b"}}\n")
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
    for (seq, p) in snapshot.common.iter_points()?.enumerate() {
        let row = row(prepared, seq, &p?);
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
