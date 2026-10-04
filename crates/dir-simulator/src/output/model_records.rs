//! CAN and gateway schema-specific records.
use super::{PROFILE, json::opt_d};
use crate::snapshot::{Request, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn request_json(r: &Request) -> Value {
    json!({"request_id":r.request_id,"source":r.source,"bus":r.bus,"status":r.status,"generated_ps":r.generated_ps.to_string(),"ready_ps":opt_d(r.ready_ps),"sof_ps":opt_d(r.sof_ps),"eof_ps":opt_d(r.eof_ps),"payload_bits":r.payload_bits.to_string(),"serialized_bits":r.frame_bits.to_string(),"attempts":if r.sof_ps.is_some(){"1"}else{"0"},"retries":"0","drop_reason":if r.status=="dropped"{Some("queue_full")}else{None},"model_fields":{"profile":PROFILE,"schema_version":1,"crc15":r.crc15.to_string(),"stuff_bits":r.stuff_bits.to_string(),"frame_bits":r.frame_bits.to_string(),"intermission_bits":"3","bitrate_bps":r.bitrate_bps.to_string(),"planned_eof_ps":opt_d(r.planned_eof_ps),"planned_release_ps":opt_d(r.planned_release_ps),"release_ps":opt_d(r.release_ps)}})
}

pub(super) fn normalized_gateway(
    prepared: &PreparedSimulation,
    gateway: &crate::types::Gateway,
) -> Value {
    json!({"node":gateway.node,"ports":gateway.ports.iter().map(|&p| &prepared.can.controllers[p].id).collect::<Vec<_>>(),
        "processing_delay_ps":gateway.processing_delay_ps.to_string(),"hop_limit":gateway.hop_limit.to_string(),
        "rx_queue_capacity":gateway.rx_queue_capacity.to_string(),
        "routes":gateway.routes.iter().map(|r| json!({"id":r.id,"ingress":prepared.can.controllers[r.ingress].id,"egress":r.egress.iter().map(|&p| &prepared.can.controllers[p].id).collect::<Vec<_>>(),"format":r.format,"id_min":r.id_min.to_string(),"id_max":r.id_max.to_string()})).collect::<Vec<_>>()})
}
pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<Vec<Value>, Diagnostic> {
    if let Some(ethernet) = &snapshot.ethernet {
        return Ok(super::ethernet::model_records(prepared, ethernet));
    }
    let lineage =
        |id: &str| {
            snapshot.gateway.request_lineage.get(id).ok_or_else(|| {
                Diagnostic::output(format!("missing Gateway lineage for request {id}"))
            })
        };
    let envelope = |schema: &str,
                    id: String,
                    subject: String,
                    request: String,
                    origin: String,
                    time: u64,
                    data: Value| {
        json!({"schema_name":schema,"schema_version":1,"record_id":id,"subject":subject,"request_id":request,"origin_request_id":origin,"time_ps":time.to_string(),"data":data})
    };
    let mut rows = Vec::new();
    for r in &snapshot.can.requests {
        let mut data = request_json(r);
        data["model_fields"]["profile"] = json!(prepared.common.profile);
        data["model_fields"]["origin_request_id"] =
            json!(lineage(&r.request_id)?.origin_request_id);
        data["model_fields"]["parent_request_id"] =
            json!(lineage(&r.request_id)?.parent_request_id);
        data["model_fields"]["gw_hops"] = json!(lineage(&r.request_id)?.gw_hops.to_string());
        data["model_fields"]["tx_enqueued_ps"] = opt_d(r.tx_enqueued_ps);
        rows.push(envelope(
            "can.request",
            r.request_id.clone(),
            r.source.clone(),
            r.request_id.clone(),
            lineage(&r.request_id)?.origin_request_id.clone(),
            r.release_ps
                .or(r.eof_ps)
                .or(r.sof_ps)
                .or(r.tx_enqueued_ps)
                .or(r.ready_ps)
                .unwrap_or(r.generated_ps),
            data,
        ));
    }
    let lookup: BTreeMap<_, _> = snapshot
        .can
        .requests
        .iter()
        .map(|r| (r.request_id.as_str(), r))
        .collect();
    for r in &snapshot.can.receivers {
        let request = lookup[r.request_id.as_str()];
        let data = json!({"request_id":r.request_id,"receiver":r.receiver,"status":r.status,"observed_ps":opt_d(r.observed_ps),"received_ps":opt_d(r.received_ps)});
        rows.push(envelope(
            "can.receiver",
            format!("{}/{}", r.request_id, r.receiver),
            r.receiver.clone(),
            r.request_id.clone(),
            lineage(&request.request_id)?.origin_request_id.clone(),
            r.received_ps
                .or(r.observed_ps)
                .or(request.eof_ps)
                .unwrap_or(request.generated_ps),
            data,
        ));
    }
    for f in &snapshot.gateway.forwards {
        let parent = &snapshot.can.requests[f.parent_request];
        let gateway = &prepared.gateway.gateways[f.gateway];
        let data = json!({"forward_id":f.forward_id,"parent_request_id":parent.request_id,"origin_request_id":lineage(&parent.request_id)?.origin_request_id,"gateway":gateway.node,"ingress":prepared.can.controllers[f.ingress].id,
            "egress":f.egress.map(|p| &prepared.can.controllers[p].id),"route_id":f.route_id,"gw_hops":f.gw_hops.to_string(),"received_ps":f.received_ps.to_string(),"planned_forward_ps":opt_d(f.planned_forward_ps),"forwarded_ps":opt_d(f.forwarded_ps),"child_request_id":f.child_request_id,"status":f.status,"reason":f.reason});
        rows.push(envelope(
            "gw.forward",
            f.forward_id.clone(),
            gateway.node.clone(),
            parent.request_id.clone(),
            lineage(&parent.request_id)?.origin_request_id.clone(),
            f.forwarded_ps.unwrap_or(f.received_ps),
            data,
        ));
    }
    for buffer in &snapshot.gateway.rx_buffers {
        let parent = &snapshot.can.requests[buffer.parent_request];
        let origin = &lineage(&parent.request_id)?.origin_request_id;
        let ingress = &prepared.can.controllers[buffer.ingress].id;
        let data = json!({"buffer_id":buffer.buffer_id,"parent_request_id":parent.request_id,
            "origin_request_id":origin,"gateway":prepared.gateway.gateways[buffer.gateway].node,
            "ingress":ingress,"capacity":buffer.capacity.to_string(),
            "received_ps":buffer.received_ps.to_string(),"released_ps":opt_d(buffer.released_ps),
            "status":buffer.status,"reason":buffer.reason,
            "egress":buffer.egress.iter().map(|&p| &prepared.can.controllers[p].id).collect::<Vec<_>>()});
        rows.push(envelope(
            "gw.rx_buffer",
            buffer.buffer_id.clone(),
            ingress.clone(),
            parent.request_id.clone(),
            origin.clone(),
            buffer.released_ps.unwrap_or(buffer.received_ps),
            data,
        ));
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
    Ok(rows)
}

pub(super) fn requests(snapshot: &Snapshot) -> Vec<Value> {
    let mut requests: Vec<_> = snapshot.can.requests.iter().map(request_json).collect();
    requests.sort_by(|a, b| a["request_id"].as_str().cmp(&b["request_id"].as_str()));
    requests
}

pub(super) fn receivers(snapshot: &Snapshot) -> Vec<Value> {
    let mut receivers: Vec<_> = snapshot.can.receivers.iter().map(|r| json!({"request_id":r.request_id,"receiver":r.receiver,"status":r.status,"observed_ps":opt_d(r.observed_ps),"received_ps":opt_d(r.received_ps)})).collect();
    receivers.sort_by(|a, b| {
        (a["request_id"].as_str(), a["receiver"].as_str())
            .cmp(&(b["request_id"].as_str(), b["receiver"].as_str()))
    });
    receivers
}
