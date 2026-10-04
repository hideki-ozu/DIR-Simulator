//! Ethernet observations and metadata projected from committed model state.
use super::aggregate::{MetricValue, Record, finalize, ratio};
use super::json::{canonical, opt_d};
use super::publish::digest;
use crate::snapshot::Snapshot;
use crate::snapshot::ethernet::EthernetSnapshot;
use crate::types::ethernet::{
    EthernetDevice, EthernetPortPolicy, EthernetVlanTag, EthernetWireFrame,
};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;
mod flow;

pub(super) const METRICS: &str = "queue_length queue_max queue_mean ethernet.generated ethernet.transfer_offered ethernet.queued ethernet.transmitting ethernet.serialized ethernet.dropped ethernet.received ethernet.filtered ethernet.forwarded ethernet.processing ethernet.link_utilization ethernet.payload_bits ethernet.mac_bits ethernet.wire_bits ethernet.occupied_bits ethernet.payload_throughput_bps ethernet.delivery_ps ethernet.delivery_mean_ps";
const QOS_METRICS: &str = "queue_bytes queue_bytes_max queue_bytes_mean ethernet.queue_wait_ps ethernet.serialization_ps ethernet.propagation_ps ethernet.processing_ps ethernet.flow.generated ethernet.flow.received ethernet.flow.copy_dropped ethernet.flow.filtered ethernet.flow.processing ethernet.flow.source_processing ethernet.flow.unfinished_copies ethernet.flow.deadline_missed ethernet.flow.deadline_sample_count ethernet.flow.deadline_miss_ratio ethernet.flow.delivery_mean_ps ethernet.flow.delivery_max_ps ethernet.flow.delivery_p50_ps ethernet.flow.delivery_p95_ps ethernet.flow.delivery_p99_ps ethernet.flow.delivery_jitter_ps ethernet.flow.queue_wait_mean_ps ethernet.flow.serialization_mean_ps ethernet.flow.propagation_mean_ps ethernet.flow.processing_mean_ps";

const FILTER_REASONS: [&str; 8] = [
    "ingress_frame_type",
    "ingress_vlan_membership",
    "destination_mismatch",
    "multicast_not_subscribed",
    "same_ingress",
    "no_vlan_egress",
    "multicast_no_egress",
    "unknown_multicast",
];

fn is_qos(prepared: &PreparedSimulation) -> bool {
    matches!(
        prepared.common.profile.as_str(),
        "ethernet.l2.qos.v1" | "ethernet.l2.vlan.v1"
    )
}
fn tag_json(tag: Option<&EthernetVlanTag>) -> Value {
    tag.map_or(Value::Null, |tag| json!({"vid":tag.vid.to_string(),"pcp":tag.pcp.to_string(),"dei":tag.dei.to_string()}))
}
fn wire_json(wire: &EthernetWireFrame) -> Value {
    json!({"src_mac":wire.src_mac,"dst_mac":wire.dst_mac,"ether_type":wire.ether_type.to_string(),
        "data_hex":wire.data_hex,"pad_bytes":wire.pad_bytes.to_string(),"mac_bytes":wire.mac_bytes.to_string(),
        "tag":tag_json(wire.tag.as_ref()),"fcs_hex":wire.fcs_hex,"mac_hex":wire.mac_hex})
}
fn port_json(port: &EthernetPortPolicy) -> Value {
    json!({"port":port.port,"ingress":port.ingress,"pvid":port.pvid.to_string(),
        "admit":port.admit,"default_priority":port.default_priority.to_string(),
        "vlans":port.vlans.iter().map(|(vid,tagged)|json!({"vid":vid.to_string(),"tagged":tagged})).collect::<Vec<_>>()})
}
fn vlan_device_json(device: &EthernetDevice) -> Value {
    if device.kind == "endpoint" {
        json!({"multicast":device.subscriptions.iter().map(|(vid,mac)|json!({"vid":vid.to_string(),"dst_mac":mac})).collect::<Vec<_>>()})
    } else {
        json!({"vlan_fdb":device.vlan_fdb.iter().map(|((vid,mac),egress)|json!({"vid":vid.to_string(),"dst_mac":mac,"egress":egress})).collect::<Vec<_>>(),
            "multicast":device.multicast.iter().map(|((vid,mac),egresses)|json!({"vid":vid.to_string(),"dst_mac":mac,"egresses":egresses})).collect::<Vec<_>>(),
            "unknown_multicast":device.unknown_multicast})
    }
}

pub(super) fn descriptor(metric: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match metric {
        "ethernet.queue_wait_ps"
        | "ethernet.serialization_ps"
        | "ethernet.propagation_ps"
        | "ethernet.processing_ps" => ("ps", "integer", "point", "identity"),
        "ethernet.flow.deadline_miss_ratio" => ("1", "number", "summary", "ratio"),
        "ethernet.flow.delivery_max_ps" => ("ps", "integer", "summary", "max"),
        "ethernet.flow.delivery_p50_ps"
        | "ethernet.flow.delivery_p95_ps"
        | "ethernet.flow.delivery_p99_ps" => ("ps", "integer", "summary", "nearest_rank"),
        "ethernet.flow.delivery_jitter_ps" => ("ps", "integer", "summary", "range"),
        name if name.starts_with("ethernet.flow.") && name.ends_with("_mean_ps") => {
            ("ps", "number", "summary", "sample_mean")
        }
        "ethernet.delivery_ps" => ("ps", "integer", "point", "identity"),
        "ethernet.delivery_mean_ps" => ("ps", "number", "summary", "sample_mean"),
        "ethernet.link_utilization" => ("1", "number", "window_summary", "occupancy_ratio"),
        "ethernet.payload_bits"
        | "ethernet.mac_bits"
        | "ethernet.wire_bits"
        | "ethernet.occupied_bits" => ("bit", "integer", "window_summary", "sum"),
        "ethernet.payload_throughput_bps" => ("bit/s", "number", "window_summary", "rate"),
        _ => ("count", "integer", "summary", "sum"),
    }
}

fn checked_add(a: u128, b: u128) -> Result<u128, Diagnostic> {
    a.checked_add(b)
        .ok_or_else(|| Diagnostic::output("Ethernet metric sum overflow"))
}

pub(super) fn records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(Vec<Record>, Vec<Record>), Diagnostic> {
    let ethernet = prepared
        .ethernet
        .as_ref()
        .expect("Ethernet prepared payload");
    let state = snapshot
        .ethernet
        .as_ref()
        .ok_or_else(|| Diagnostic::output("Missing Ethernet snapshot"))?;
    let h = snapshot.common.end_ps;
    let mut points: Vec<_> = snapshot
        .common
        .points
        .iter()
        .enumerate()
        .map(|(order, point)| {
            Record::point(
                point,
                &point.metric,
                MetricValue::Integer(point.value.into()),
                order,
            )
        })
        .collect();
    let mut summary = Vec::new();
    for target in ethernet
        .devices
        .iter()
        .map(|device| device.id.as_str())
        .chain(std::iter::once("$all"))
    {
        let generated = state
            .frames
            .iter()
            .filter(|frame| target == "$all" || frame.source == target)
            .count();
        summary.push(Record::aggregate(
            target,
            "ethernet.generated",
            MetricValue::Integer(generated as u128),
            0,
            h,
        ));
        for status in ["received", "filtered", "forwarded", "processing"] {
            let count = state
                .receptions
                .iter()
                .filter(|reception| {
                    (target == "$all" || reception.device == target) && reception.status == status
                })
                .count();
            summary.push(Record::aggregate(
                target,
                &format!("ethernet.{status}"),
                MetricValue::Integer(count as u128),
                0,
                h,
            ));
        }
        if prepared.common.profile == "ethernet.l2.vlan.v1" {
            let filtered = state
                .receptions
                .iter()
                .filter(|r| (target == "$all" || r.device == target) && r.status == "filtered")
                .collect::<Vec<_>>();
            if filtered.iter().any(|r| {
                !r.reason
                    .as_deref()
                    .is_some_and(|reason| FILTER_REASONS.contains(&reason))
            }) {
                return Err(Diagnostic::output(
                    "Unknown VLAN reception filtering reason",
                ));
            }
            for reason in FILTER_REASONS {
                let count = filtered
                    .iter()
                    .filter(|r| r.reason.as_deref() == Some(reason))
                    .count();
                summary.push(Record::aggregate(
                    target,
                    &format!("ethernet.filtered.{reason}"),
                    MetricValue::Integer(count as u128),
                    0,
                    h,
                ));
            }
        }
        let samples: Vec<_> = snapshot
            .common
            .points
            .iter()
            .filter(|point| {
                point.metric == "ethernet.delivery_ps"
                    && (target == "$all" || point.target == target)
            })
            .collect();
        let sum = samples
            .iter()
            .try_fold(0u128, |sum, point| checked_add(sum, point.value.into()))?;
        let mut mean = Record::aggregate(
            target,
            "ethernet.delivery_mean_ps",
            ratio(sum, samples.len() as u128)?,
            0,
            h,
        );
        mean.sample_count = Some(samples.len() as u128);
        summary.push(mean);
    }
    for target in ethernet
        .directions
        .iter()
        .map(|direction| direction.from_port.as_str())
        .chain(std::iter::once("$all"))
    {
        let transfers: Vec<_> = state
            .transfers
            .iter()
            .filter(|transfer| target == "$all" || transfer.from_port == target)
            .collect();
        summary.push(Record::aggregate(
            target,
            "ethernet.transfer_offered",
            MetricValue::Integer(transfers.len() as u128),
            0,
            h,
        ));
        for status in ["queued", "transmitting", "serialized", "dropped"] {
            let count = transfers
                .iter()
                .filter(|transfer| transfer.status == status)
                .count();
            let mut row = Record::aggregate(
                target,
                &format!("ethernet.{status}"),
                MetricValue::Integer(count as u128),
                0,
                h,
            );
            if status == "dropped" {
                row.reason = Some("queue_full".into());
            }
            summary.push(row);
        }
    }
    let mut queues: Vec<_> = ethernet
        .directions
        .iter()
        .map(|direction| {
            (
                format!("{}.queue", direction.from_port),
                "queue_length",
                "queue_max",
                "queue_mean",
            )
        })
        .collect();
    if is_qos(prepared) {
        for output in &ethernet.outputs {
            for class in &output.queues {
                let id = format!("{}.queue.{}", output.port, class.priority);
                queues.push((id.clone(), "queue_length", "queue_max", "queue_mean"));
                queues.push((id, "queue_bytes", "queue_bytes_max", "queue_bytes_mean"));
            }
        }
        let (components, summaries) = flow::observations(prepared, snapshot)?;
        for (order, point) in components {
            points.push(Record::point(
                &point,
                &point.metric,
                MetricValue::Integer(point.value.into()),
                order,
            ));
        }
        summary.extend(summaries);
    }
    for (queue, metric, max_metric, mean_metric) in queues {
        let changes: Vec<_> = snapshot
            .common
            .points
            .iter()
            .filter(|point| point.target == queue && point.metric == metric)
            .collect();
        let maximum = changes.iter().map(|point| point.value).max().unwrap_or(0);
        summary.push(Record::aggregate(
            &queue,
            max_metric,
            MetricValue::Integer(maximum.into()),
            0,
            h,
        ));
        let mut previous = 0;
        let mut length = 0u64;
        let mut integral = 0u128;
        for point in changes {
            integral = checked_add(
                integral,
                (point.time_ps - previous) as u128 * length as u128,
            )?;
            previous = point.time_ps;
            length = point.value;
        }
        integral = checked_add(integral, (h - previous) as u128 * length as u128)?;
        summary.push(Record::aggregate(
            &queue,
            mean_metric,
            ratio(integral, h.into())?,
            0,
            h,
        ));
    }
    let mut start = 0;
    while start < h {
        let end = start
            .saturating_add(prepared.common.metrics_window_ps)
            .min(h);
        points.extend(window_records(prepared, snapshot, start, end, false)?);
        start = end;
    }
    summary.extend(window_records(
        prepared,
        snapshot,
        0,
        h,
        snapshot.common.partial,
    )?);
    if h == 0 && !snapshot.common.partial {
        points.clear();
    }
    Ok(finalize(points, summary))
}

fn window_records(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    start: u64,
    end: u64,
    include_boundary: bool,
) -> Result<Vec<Record>, Diagnostic> {
    let ethernet = prepared.ethernet.as_ref().expect("Ethernet input");
    let state = snapshot.ethernet.as_ref().expect("Ethernet state");
    let frames: BTreeMap<_, _> = state
        .frames
        .iter()
        .map(|frame| (frame.frame_id.as_str(), frame))
        .collect();
    let mut result = Vec::new();
    for direction in &ethernet.directions {
        let mut busy = 0u128;
        let mut payload = 0u128;
        let mut mac = 0u128;
        let mut wire = 0u128;
        let mut occupied = 0u128;
        for transfer in state
            .transfers
            .iter()
            .filter(|transfer| transfer.from_port == direction.from_port)
        {
            let frame = frames
                .get(transfer.frame_id.as_str())
                .ok_or_else(|| Diagnostic::output("Unknown Ethernet frame reference"))?;
            let copy_wire = if prepared.common.profile == "ethernet.l2.vlan.v1" {
                &transfer.wire
            } else {
                &frame.wire
            };
            if let Some(sof) = transfer.sof_ps {
                let release = transfer.release_ps.unwrap_or(snapshot.common.end_ps);
                busy = checked_add(
                    busy,
                    release.min(end).saturating_sub(sof.max(start)) as u128,
                )?;
            }
            if transfer.eof_ps.is_some_and(|time| {
                start <= time && (time < end || include_boundary && time == end)
            }) {
                payload = checked_add(payload, copy_wire.data_hex.len() as u128 * 4)?;
                mac = checked_add(mac, copy_wire.mac_bytes as u128 * 8)?;
                wire = checked_add(wire, (copy_wire.mac_bytes as u128 + 8) * 8)?;
            }
            if transfer.release_ps.is_some_and(|time| {
                start <= time && (time < end || include_boundary && time == end)
            }) {
                occupied = checked_add(occupied, (copy_wire.mac_bytes as u128 + 20) * 8)?;
            }
        }
        let target = &direction.from_port;
        for (metric, value) in [
            ("ethernet.payload_bits", payload),
            ("ethernet.mac_bits", mac),
            ("ethernet.wire_bits", wire),
            ("ethernet.occupied_bits", occupied),
        ] {
            result.push(Record::aggregate(
                target,
                metric,
                MetricValue::Integer(value),
                start,
                end,
            ));
        }
        result.push(Record::aggregate(
            target,
            "ethernet.link_utilization",
            ratio(busy, (end - start).into())?,
            start,
            end,
        ));
        let rate = payload
            .checked_mul(1_000_000_000_000)
            .ok_or_else(|| Diagnostic::output("Ethernet throughput overflow"))?;
        result.push(Record::aggregate(
            target,
            "ethernet.payload_throughput_bps",
            ratio(rate, (end - start).into())?,
            start,
            end,
        ));
    }
    Ok(result)
}

pub(super) fn model_records(prepared: &PreparedSimulation, state: &EthernetSnapshot) -> Vec<Value> {
    let qos = is_qos(prepared);
    let vlan = prepared.common.profile == "ethernet.l2.vlan.v1";
    let envelope = |schema: &str,
                    id: &str,
                    subject: &str,
                    frame_id: &str,
                    time: u64,
                    data: Value| {
        json!({"schema_name":schema,"schema_version":1,"record_id":id,"subject":subject,"request_id":frame_id,"origin_request_id":null,"time_ps":time.to_string(),"data":data})
    };
    let mut rows = Vec::new();
    for frame in &state.frames {
        let wire = &frame.wire;
        rows.push(envelope("ethernet.frame", &frame.frame_id, &frame.source, &frame.frame_id, frame.time_ps,
            json!({"source":frame.source,"src_mac":wire.src_mac,"dst_mac":wire.dst_mac,"ether_type":wire.ether_type.to_string(),"data_hex":wire.data_hex,"pad_bytes":wire.pad_bytes.to_string(),"mac_bytes":wire.mac_bytes.to_string(),"fcs_hex":wire.fcs_hex,"mac_hex":wire.mac_hex,"generated_ps":frame.generated_ps.to_string(),"ready_ps":opt_d(frame.ready_ps)})));
    }
    for transfer in &state.transfers {
        rows.push(envelope("ethernet.transfer", &transfer.transfer_id, &transfer.from_port, &transfer.frame_id, transfer.time_ps,
            json!({"frame_id":transfer.frame_id,"parent_transfer_id":transfer.parent_transfer_id,"from_port":transfer.from_port,"to_port":transfer.to_port,"queued_ps":transfer.queued_ps.to_string(),"sof_ps":opt_d(transfer.sof_ps),"eof_ps":opt_d(transfer.eof_ps),"release_ps":opt_d(transfer.release_ps),"arrival_ps":opt_d(transfer.arrival_ps),"planned_eof_ps":opt_d(transfer.planned_eof_ps),"planned_release_ps":opt_d(transfer.planned_release_ps),"planned_arrival_ps":opt_d(transfer.planned_arrival_ps),"status":transfer.status,"drop_reason":transfer.drop_reason})));
    }
    for reception in &state.receptions {
        rows.push(envelope("ethernet.reception", &reception.reception_id, &reception.device, &reception.frame_id, reception.time_ps,
            json!({"frame_id":reception.frame_id,"transfer_id":reception.transfer_id,"ingress":reception.ingress,"observed_ps":reception.observed_ps.to_string(),"ready_ps":opt_d(reception.ready_ps),"planned_ready_ps":opt_d(reception.planned_ready_ps),"status":reception.status,"reason":reception.reason,"egress_transfer_ids":reception.egress_transfer_ids})));
    }
    if qos {
        for (row, frame) in rows.iter_mut().take(state.frames.len()).zip(&state.frames) {
            row["schema_version"] = json!(2);
            row["data"]["flow_id"] = json!(frame.flow_id);
            row["data"]["priority"] = json!(frame.priority.to_string());
            row["data"]["deadline_ps"] = opt_d(frame.deadline_ps);
        }
        for (row, transfer) in rows
            .iter_mut()
            .skip(state.frames.len())
            .take(state.transfers.len())
            .zip(&state.transfers)
        {
            row["schema_version"] = json!(2);
            row["data"]["queue_id"] = json!(transfer.queue_id);
            row["data"]["priority"] = json!(transfer.priority.to_string());
        }
    }
    if vlan {
        for (row, frame) in rows.iter_mut().take(state.frames.len()).zip(&state.frames) {
            row["schema_version"] = json!(3);
            row["data"]["tag"] = tag_json(frame.wire.tag.as_ref());
            row["data"]["source_vlan_id"] = json!(
                frame
                    .source_vlan_id
                    .expect("VLAN source classification")
                    .to_string()
            );
        }
        for (row, transfer) in rows
            .iter_mut()
            .skip(state.frames.len())
            .take(state.transfers.len())
            .zip(&state.transfers)
        {
            row["schema_version"] = json!(3);
            row["data"]["vlan_id"] = json!(
                transfer
                    .vlan_id
                    .expect("VLAN transfer classification")
                    .to_string()
            );
            row["data"]["wire"] = wire_json(&transfer.wire);
        }
        for (row, reception) in rows
            .iter_mut()
            .skip(state.frames.len() + state.transfers.len())
            .zip(&state.receptions)
        {
            row["schema_version"] = json!(2);
            row["data"]["vlan_id"] = json!(
                reception
                    .vlan_id
                    .expect("VLAN ingress classification")
                    .to_string()
            );
            row["data"]["priority"] = json!(reception.priority.to_string());
        }
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
    let ethernet = prepared.ethernet.as_ref().expect("Ethernet input");
    let qos = is_qos(prepared);
    let vlan = prepared.common.profile == "ethernet.l2.vlan.v1";
    let outputs: Vec<_> = ethernet.outputs.iter().map(|output| json!({"port":output.port,"scheduler":output.scheduler,
        "queues":output.queues.iter().map(|queue|json!({"priority":queue.priority.to_string(),"capacity_frames":queue.capacity_frames.to_string(),"capacity_bytes":opt_d(queue.capacity_bytes)})).collect::<Vec<_>>()})).collect();
    if qos {
        for output in &outputs {
            config.insert(
                format!("@output:{}", output["port"].as_str().unwrap()),
                canonical(output),
            );
        }
    }
    let ports: Vec<_> = ethernet.port_policies.iter().map(port_json).collect();
    if vlan {
        for port in &ports {
            config.insert(
                format!("@port:{}", port["port"].as_str().unwrap()),
                canonical(port),
            );
        }
        for device in &ethernet.devices {
            config.insert(
                format!("@vlan:{}", device.id),
                canonical(&vlan_device_json(device)),
            );
        }
    }
    config.insert(
        "model-profile".into(),
        canonical(&json!(prepared.common.profile)),
    );
    if let Some(source) = sources
        .iter()
        .find(|source| source["logical_path"] == "model-config")
    {
        config.insert("model-config".into(), canonical(&source["canonical_path"]));
    }
    for device in &ethernet.devices {
        config.insert(
            format!("{}.queueCapacity", device.id),
            device.queue_capacity.to_string(),
        );
        for (field, value) in [
            ("txProcessingDelay", device.tx_processing_delay_ps),
            ("rxProcessingDelay", device.rx_processing_delay_ps),
            ("forwardDelay", device.forward_delay_ps),
        ] {
            config.insert(format!("{}.{field}", device.id), format!("{value}ps"));
        }
    }
    for direction in &ethernet.directions {
        config.insert(
            format!("{}.bitrate", direction.channel_id),
            format!("{}bit/s", direction.bitrate_bps),
        );
        config.insert(
            format!("{}.delay", direction.channel_id),
            format!("{}ps", direction.delay_ps),
        );
    }
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let mut metric_names: Vec<_> = METRICS.split_whitespace().collect();
    if qos {
        metric_names.extend(QOS_METRICS.split_whitespace());
    }
    let filter_metrics: Vec<_> = FILTER_REASONS
        .iter()
        .map(|reason| format!("ethernet.filtered.{reason}"))
        .collect();
    if vlan {
        metric_names.extend(filter_metrics.iter().map(String::as_str));
    }
    metric_names.sort_unstable();
    let metrics: Vec<_> = metric_names.into_iter().map(|metric| {
        let (unit,kind,sampling,aggregation) = super::aggregate::descriptor(metric);
        json!({"metric_id":metric,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation})
    }).collect();
    let mut initial_state = vec![json!({"instance":prepared.common.network,"state":"{}"})];
    initial_state.extend(
        prepared
            .common
            .module_paths
            .iter()
            .map(|path| json!({"instance":path,"state":"{}"})),
    );
    for (index, device) in ethernet.devices.iter().enumerate() {
        let directions: Vec<_> = ethernet
            .directions
            .iter()
            .filter(|direction| direction.source == index)
            .collect();
        let queues: BTreeMap<_, _> = directions
            .iter()
            .map(|direction| {
                let queue = if qos {
                    json!(
                        (0..8)
                            .map(|priority| (priority.to_string(), json!([])))
                            .collect::<BTreeMap<_, _>>()
                    )
                } else {
                    json!([])
                };
                (direction.from_port.rsplit('.').next().unwrap(), queue)
            })
            .collect();
        let links: BTreeMap<_, _> = directions
            .iter()
            .map(|direction| {
                (
                    direction.from_port.rsplit('.').next().unwrap(),
                    json!({"owner":null,"state":"idle"}),
                )
            })
            .collect();
        let mut state = if device.kind == "endpoint" {
            let generators: Vec<_> = ethernet.generators.iter().filter(|generator|generator.source==index)
                .map(|generator| {
                    let next = generator.time(0);
                    let mut cursor = json!({"id":generator.id,"next_ordinal":"0","next_time_ps":next.and_then(|time|u64::try_from(time).ok()).map(|time|time.to_string())});
                    if qos {
                        cursor["next_candidate_ps"] = json!(next.map(|time|time.to_string()));
                    }
                    cursor
                }).collect();
            json!({"mac":device.mac,"queues":queues,"links":links,"generators":generators})
        } else {
            let fdb: Vec<_> = device
                .fdb
                .iter()
                .map(|(dst_mac, egress)| json!({"dst_mac":dst_mac,"egress":egress}))
                .collect();
            json!({"fdb":fdb,"queues":queues,"links":links})
        };
        if vlan {
            state
                .as_object_mut()
                .unwrap()
                .extend(vlan_device_json(device).as_object().unwrap().clone());
            state["ports"] = json!(
                ethernet
                    .port_policies
                    .iter()
                    .filter(|port| port.device == index)
                    .map(port_json)
                    .collect::<Vec<_>>()
            );
        }
        initial_state.push(json!({"instance":device.id,"state":canonical(&state)}));
    }
    initial_state.extend(ethernet.directions.iter().map(|direction|json!({"instance":direction.channel_id,"state":canonical(&json!({"bitrate_bps":direction.bitrate_bps.to_string(),"delay_ps":direction.delay_ps.to_string()}))})));
    initial_state.sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let mut topology = json!({"devices":ethernet.devices.iter().map(|device|json!({"id":device.id,"kind":device.kind,"mac":device.mac,"queue_capacity":device.queue_capacity.to_string()})).collect::<Vec<_>>(),
        "directions":ethernet.directions.iter().map(|direction|json!({"from_port":direction.from_port,"to_port":direction.to_port,"bitrate_bps":direction.bitrate_bps.to_string(),"delay_ps":direction.delay_ps.to_string()})).collect::<Vec<_>>()});
    if qos {
        topology["outputs"] = json!(outputs);
    }
    if vlan {
        topology["ports"] = json!(ports);
        for (row, device) in topology["devices"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(&ethernet.devices)
        {
            row.as_object_mut()
                .unwrap()
                .extend(vlan_device_json(device).as_object().unwrap().clone());
        }
    }
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(source_hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,
        "runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","model_profile":prepared.common.profile,
        "model_schemas":[{"schema_name":"ethernet.frame","schema_version":1},{"schema_name":"ethernet.reception","schema_version":1},{"schema_name":"ethernet.transfer","schema_version":1}],
        "models":[{"type":prepared.common.profile,"version":"1","assumptions":["Ethernet II untagged","full duplex","static FDB","store and forward","error-free fixed-delay links","one FIFO per output","fixed independent processing delays"]}],
        "initial_state":initial_state,"metrics":metrics,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null,"topology":topology,"ethernet_topology":topology,
        "implementation_coverage":{"profile":prepared.common.profile,"reproduction_conditions":"partially_identified","limitations":["start timestamp and run_id collected at export rather than execution start","source logical roots inferred from filename","public Registry/Envelope event codec API unavailable","build provenance compiler/toolchain/commit/lock unavailable","OS release and CPU model unavailable"]}});
    if qos {
        result["model_schemas"] = json!([{"schema_name":"ethernet.frame","schema_version":2},{"schema_name":"ethernet.reception","schema_version":1},{"schema_name":"ethernet.transfer","schema_version":2}]);
        result["models"][0]["assumptions"] = json!([
            "Ethernet II untagged",
            "full duplex",
            "static FDB",
            "store and forward",
            "error-free fixed-delay links",
            "eight priority FIFO classes per output",
            "FIFO or nonpreemptive strict priority",
            "waiting MAC-byte capacities",
            "fixed independent processing delays"
        ]);
        let flows: BTreeMap<_,_> = ethernet.generators.iter().filter_map(|generator|generator.flow_id.as_ref().map(|id| {
            let mut flow = json!({"flow_id":id,"priority":generator.priority.to_string(),"deadline_ps":opt_d(generator.deadline_ps),"dst_mac":generator.frame.dst_mac});
            if vlan {
                flow["tag"] = tag_json(generator.frame.tag.as_ref());
                flow["source_vlan_id"] = json!(generator.source_vlan_id.expect("VLAN source classification").to_string());
            }
            (id.clone(),flow)
        })).collect();
        result["flows"] = json!(flows.into_values().collect::<Vec<_>>());
    }
    if vlan {
        result["model_schemas"] = json!([{"schema_name":"ethernet.frame","schema_version":3},{"schema_name":"ethernet.reception","schema_version":2},{"schema_name":"ethernet.transfer","schema_version":3}]);
        result["models"][0]["assumptions"] = json!([
            "Ethernet II with optional single 0x8100 VLAN tag",
            "full duplex",
            "connected tree",
            "static VLAN-scoped FDB and multicast forwarding",
            "explicit endpoint group subscriptions",
            "per-port VLAN admission and ingress priority classification",
            "store and forward",
            "source padding retained across tag changes",
            "error-free fixed-delay links",
            "eight priority FIFO classes per output",
            "FIFO or nonpreemptive strict priority",
            "waiting copy MAC-byte capacities",
            "fixed independent processing delays"
        ]);
    }
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
