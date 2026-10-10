//! Projection of a composed runtime onto the established schema-2 publication.
use super::aggregate::Record;
use crate::{Diagnostic, PreparedSimulation, snapshot::Snapshot};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn decimals(value: Value) -> Value {
    match value {
        Value::Number(number) => Value::String(number.to_string()),
        Value::Array(values) => Value::Array(values.into_iter().map(decimals).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, decimals(value)))
                .collect(),
        ),
        value => value,
    }
}

pub(super) fn metadata(
    prepared: &PreparedSimulation,
    timestamp: &str,
    sources: Vec<Value>,
    hashes: Vec<Value>,
    config: BTreeMap<String, String>,
) -> Result<Value, Diagnostic> {
    let registered = prepared.registered.as_ref().unwrap();
    let network = registered.network.as_ref().unwrap();
    let mut base = prepared.clone();
    base.registered = None;
    base.common.profile = "ethernet.l2.vlan.v1".into();
    let mut metadata = super::ethernet::metadata(&base, timestamp, sources, hashes, config)?;
    metadata["model_profile"] = json!(prepared.common.profile);
    metadata["models"] = json!([{"type":prepared.common.profile,"version":"1","assumptions":["abstract transaction network","single integer-picosecond FES","full duplex store and forward","single VLAN tag","eight priority FIFO classes","ideal shared clock"]}]);
    metadata["model_schemas"] = json!(
        registered
            .registry
            .profile(&prepared.common.profile)
            .unwrap()
            .model_records
            .iter()
            .map(|schema| json!({"schema_name":schema.name,"schema_version":schema.version}))
            .collect::<Vec<_>>()
    );
    let mut metrics = metadata["metrics"].as_array().unwrap().clone();
    let catalog = super::preparation::profile_catalog(&prepared.common.profile).unwrap();
    for metric in catalog["metrics"].as_array().unwrap() {
        if !metrics
            .iter()
            .any(|row| row["metric_id"] == metric["metric_id"])
        {
            metrics.push(metric.clone());
        }
    }
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    metadata["metrics"] = json!(metrics);
    metadata["network_subject"] = json!("@network");
    if let Some(config) = metadata["config"].as_array_mut() {
        if let Some(row) = config.iter_mut().find(|row| row["key"] == "model-profile") {
            row["value"] = json!(super::json::canonical(&json!(prepared.common.profile)));
        }
    }
    metadata["config_sha256"] = json!(super::publish::digest(
        super::json::canonical(&metadata["config"]).as_bytes()
    ));
    if let Some(dynamic) = &network.dynamic {
        let initial =
            crate::runtime::ethernet::dynamic::DynamicState::new(dynamic, &network.ethernet)?;
        let raw = if prepared.common.profile == "ethernet.tsn.v1" {
            &network.config["base"]["config"]["dynamic"]
        } else {
            &network.config["dynamic"]
        };
        metadata["ethernet_dynamic"] = json!({"config":raw,"initial_policy":decimals(serde_json::to_value(initial.snapshot_policy()).map_err(|error| Diagnostic::output(error.to_string()))?)});
        if let Some(flows) = metadata["flows"].as_array_mut() {
            for flow in flows {
                let ip = network
                    .ethernet
                    .generators
                    .iter()
                    .find(|generator| generator.flow_id.as_deref() == flow["flow_id"].as_str())
                    .and_then(|generator| dynamic.generator_ip.get(&generator.id))
                    .and_then(Option::as_ref);
                flow["ip_multicast"] = json!(ip);
            }
        }
    }
    if prepared.common.profile == "ethernet.tsn.v1" {
        metadata["ethernet_tsn"] = network.config["tsn"].clone();
    }
    if let Some(bridge) = &network.bridge {
        metadata["topology"] = json!({"controllers":bridge.can.controllers.iter().enumerate().map(|(index,controller)|json!({"id":controller.id,"bus":bridge.can.buses[bridge.can.controller_buses[index]].id,"tx_channel_delay_ps":controller.tx_channel_ps.to_string(),"rx_channel_delay_ps":controller.rx_channel_ps.to_string()})).collect::<Vec<_>>()});
        metadata["can_ethernet"] = json!({"gateways":network.config["gateways"]});
    }
    metadata["implementation_coverage"] = json!({"profile":prepared.common.profile,"reproduction_conditions":"identified","limitations":["abstract policies and codec; no IEEE protocol conformance claim"]});
    Ok(metadata)
}

pub(super) fn summary(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<Vec<Record>, Diagnostic> {
    if snapshot.ethernet.is_none() {
        return Ok(Vec::new());
    }
    let mut base = prepared.clone();
    base.common.profile = "ethernet.l2.vlan.v1".into();
    let (_, mut summary) = super::ethernet::records(&base, snapshot)?;
    audit_summary(snapshot, &mut summary)?;
    if let Some(bridge) = prepared
        .registered
        .as_ref()
        .and_then(|registered| registered.network.as_ref())
        .and_then(|network| network.bridge.as_ref())
    {
        let mut can_base = prepared.clone();
        can_base.registered = None;
        can_base.ethernet = None;
        can_base.common.profile = "can.cc.multibus.v1".into();
        let mut can_snapshot = Snapshot::empty(&can_base);
        can_snapshot.common.end_ps = snapshot.common.end_ps;
        can_snapshot.common.partial = snapshot.common.partial;
        can_snapshot.common.termination = snapshot.common.termination.clone();
        let targets = bridge
            .can
            .controllers
            .iter()
            .flat_map(|controller| [controller.id.clone(), format!("{}.txQueue", controller.id)])
            .chain(bridge.can.buses.iter().map(|bus| bus.id.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        for point in snapshot.common.iter_points()? {
            let point = point?;
            if targets.contains(&point.target) {
                can_snapshot.common.points.push(point);
            }
        }
        can_snapshot.can.bus_state = snapshot.can.bus_state.clone();
        can_snapshot.can.bus_states = snapshot.can.bus_states.clone();
        can_snapshot.can.requests = snapshot.can.requests.clone();
        can_snapshot.can.receivers = snapshot.can.receivers.clone();
        let (_, mut can_summary) = super::aggregate::records(&can_base, &can_snapshot)?;
        for record in &mut can_summary {
            if record.target == "$all" {
                record.target = "$can".into();
            }
        }
        summary.extend(can_summary);
        let records = &snapshot.registered.as_ref().unwrap().model_records;
        let conversions = records
            .values()
            .filter(|record| record.schema.name == "dir.can_ethernet.conversion")
            .collect::<Vec<_>>();
        let branches = records
            .values()
            .filter(|record| record.schema.name == "dir.can_ethernet.branch")
            .collect::<Vec<_>>();
        let segments = records
            .values()
            .filter(|record| record.schema.name == "dir.can_ethernet.segment")
            .collect::<Vec<_>>();
        let horizon = snapshot.common.end_ps;
        let mut counts = Vec::new();
        for (index, gateway) in bridge.gateways.iter().enumerate() {
            let rows = conversions
                .iter()
                .filter(|record| record.data["gateway"] == gateway.instance)
                .collect::<Vec<_>>();
            let rejected = rows
                .iter()
                .filter(|record| record.data["status"] == "rejected")
                .count() as u128;
            counts.extend([
                (
                    &gateway.instance,
                    "gateway.conversion.attempted",
                    rows.len() as u128,
                ),
                (
                    &gateway.instance,
                    "gateway.conversion.accepted",
                    rows.len() as u128 - rejected,
                ),
                (&gateway.instance, "gateway.conversion.rejected", rejected),
            ]);
            let ids = rows
                .iter()
                .map(|record| record.id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            for (status, metric) in [
                ("waiting", "gateway.branch.waiting"),
                ("admitted", "gateway.branch.admitted"),
                ("dropped", "gateway.branch.dropped"),
            ] {
                counts.push((
                    &gateway.instance,
                    metric,
                    branches
                        .iter()
                        .filter(|record| {
                            record.data["conversion_id"]
                                .as_str()
                                .is_some_and(|id| ids.contains(id))
                                && record.data["status"] == status
                        })
                        .count() as u128,
                ));
            }
            for (field, metric) in [
                ("rx_occupancy", "gateway.rx.occupancy"),
                ("rx_max", "gateway.rx.max"),
            ] {
                let value = snapshot
                    .network
                    .as_ref()
                    .and_then(|network| network["bridge"][field][index].as_str())
                    .and_then(|value| value.parse::<u128>().ok())
                    .unwrap_or(0);
                counts.push((&gateway.instance, metric, value));
            }
        }
        let target_count = segments
            .iter()
            .flat_map(|record| record.data["targets"].as_array().unwrap())
            .count() as u128;
        let completed = segments
            .iter()
            .flat_map(|record| record.data["targets"].as_array().unwrap())
            .filter(|target| target["completed_ps"].is_string())
            .count() as u128;
        for (target, metric, count) in counts {
            summary.push(Record::aggregate(
                target,
                metric,
                super::aggregate::MetricValue::Integer(count),
                0,
                horizon,
            ));
        }
        summary.push(Record::aggregate(
            "@network",
            "gateway.end_to_end.target_count",
            super::aggregate::MetricValue::Integer(target_count),
            0,
            horizon,
        ));
        summary.push(Record::aggregate(
            "@network",
            "gateway.end_to_end.completed",
            super::aggregate::MetricValue::Integer(completed),
            0,
            horizon,
        ));
        summary.push(Record::aggregate(
            "@network",
            "gateway.end_to_end.completion_ratio",
            super::aggregate::ratio(completed, target_count)?,
            0,
            horizon,
        ));
        for segment in segments {
            let origin = segment.data["origin_id"].as_str().unwrap();
            let generated = origin_generated(snapshot, origin)?;
            for target in segment.data["targets"].as_array().unwrap() {
                if let Some(completed) = target["completed_ps"]
                    .as_str()
                    .and_then(|time| time.parse::<u64>().ok())
                {
                    let latency = completed
                        .checked_sub(generated)
                        .ok_or_else(|| Diagnostic::output("negative end-to-end latency"))?;
                    let mut record = Record::aggregate(
                        target["terminal_id"].as_str().unwrap(),
                        "gateway.end_to_end.latency_ps",
                        super::aggregate::MetricValue::Integer(latency as u128),
                        0,
                        horizon,
                    );
                    record.request_id = Some(origin.into());
                    record.receiver = target["terminal_id"].as_str().map(str::to_owned);
                    record.reason = Some(segment.id.clone());
                    record.sample_count = Some(1);
                    summary.push(record);
                }
            }
        }
    }
    Ok(super::aggregate::finalize(Vec::new(), summary).1)
}

fn origin_generated(snapshot: &Snapshot, origin: &str) -> Result<u64, Diagnostic> {
    let can = |id| {
        snapshot
            .can
            .requests
            .iter()
            .find(|request| request.request_id == id)
            .map(|request| request.generated_ps)
    };
    let ethernet = |id| {
        snapshot
            .ethernet
            .as_ref()
            .unwrap()
            .frames
            .iter()
            .find(|frame| frame.frame_id == id)
            .map(|frame| frame.generated_ps)
    };
    let qualified = if let Some(id) = origin.strip_prefix("can:") {
        can(id)
    } else if let Some(id) = origin.strip_prefix("ethernet:") {
        ethernet(id)
    } else {
        None
    };
    let generated = if qualified.is_some() {
        qualified
    } else {
        match (can(origin), ethernet(origin)) {
            (Some(_), Some(_)) => {
                return Err(Diagnostic::output("ambiguous unqualified native origin"));
            }
            (can, ethernet) => can.or(ethernet),
        }
    };
    generated.ok_or_else(|| Diagnostic::output("segment references unknown native origin"))
}

fn audit_summary(snapshot: &Snapshot, summary: &mut Vec<Record>) -> Result<(), Diagnostic> {
    use super::aggregate::{MetricValue, ratio};
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    let mut counts = BTreeMap::<(&str, &str), u128>::new();
    let mut dynamic = false;
    let mut epoch = 0;
    for row in records.values() {
        let data = &row.data;
        if row.schema.name == "ethernet.dynamic.control" {
            dynamic = true;
            let metric = match data["kind"].as_str().unwrap() {
                "mac_learn" => Some("ethernet.dynamic.mac_learned"),
                "mac_refresh" => Some("ethernet.dynamic.mac_refreshed"),
                "mac_move" => Some("ethernet.dynamic.mac_moved"),
                "mac_expire" => Some("ethernet.dynamic.mac_expired"),
                "mac_flush" => Some("ethernet.dynamic.mac_flushed"),
                "mac_capacity" => Some("ethernet.dynamic.mac_table_full"),
                "membership_set" | "membership_leave" if data["outcome"] == "changed" => {
                    Some("ethernet.dynamic.membership_updated")
                }
                "membership_expire" => Some("ethernet.dynamic.membership_expired"),
                "router_set" | "router_leave" if data["outcome"] == "changed" => {
                    Some("ethernet.dynamic.router_updated")
                }
                "router_expire" => Some("ethernet.dynamic.router_expired"),
                "vlan_register" | "vlan_unregister" if data["outcome"] == "changed" => {
                    Some("ethernet.dynamic.registration_updated")
                }
                "registration_expire" => Some("ethernet.dynamic.registration_expired"),
                "link_set" if data["outcome"] == "changed" => {
                    Some("ethernet.dynamic.topology_changed")
                }
                "tree_publish" => Some("ethernet.dynamic.tree_published"),
                _ => None,
            };
            if let Some(metric) = metric {
                *counts.entry(("@network", metric)).or_default() += 1;
            }
            let metric = match data["outcome"].as_str().unwrap() {
                "changed" => "ethernet.dynamic.control_changed",
                "stale" => "ethernet.dynamic.control_stale",
                _ => "ethernet.dynamic.control_no_op",
            };
            *counts.entry(("@network", metric)).or_default() += 1;
        } else if row.schema.name == "ethernet.dynamic.policy" {
            dynamic = true;
            epoch = epoch.max(
                data["policy_epoch"]
                    .as_str()
                    .unwrap()
                    .parse::<u128>()
                    .unwrap(),
            );
        } else if row.schema.name == "ethernet.dynamic.transfer"
            || row.schema.name == "ethernet.dynamic.reception"
        {
            for (reason, metric) in [
                ("link_down", "ethernet.dynamic.filtered.link_down"),
                ("stp_discarding", "ethernet.dynamic.filtered.stp_discarding"),
                (
                    "vlan_unregistered",
                    "ethernet.dynamic.filtered.vlan_unregistered",
                ),
            ] {
                if data["drop_reason"] == reason || data["reason"] == reason {
                    *counts.entry(("@network", metric)).or_default() += 1;
                }
            }
        } else if row.schema.name == "ethernet.tsn.policing" {
            for (color, metric) in [
                ("green", "ethernet.tsn.policing_green"),
                ("yellow", "ethernet.tsn.policing_yellow"),
                ("red", "ethernet.tsn.policing_red"),
            ] {
                if data["color"] == color {
                    *counts.entry(("@network", metric)).or_default() += 1;
                }
            }
            if data["verdict"].as_str().unwrap().starts_with("psfp_") {
                *counts
                    .entry(("@network", "ethernet.tsn.policing_dropped"))
                    .or_default() += 1;
            }
        } else if row.schema.name == "ethernet.tsn.decision" {
            let metric = match data["reason"].as_str().unwrap() {
                "gate_closed" => Some("ethernet.tsn.gate_wait"),
                "guard_blocked" => Some("ethernet.tsn.guard_wait"),
                "credit_negative" => Some("ethernet.tsn.credit_wait"),
                _ => None,
            };
            if let Some(metric) = metric {
                *counts
                    .entry((data["port"].as_str().unwrap(), metric))
                    .or_default() += 1;
            }
        } else if row.schema.name == "ethernet.tsn.gate" {
            *counts
                .entry((data["port"].as_str().unwrap(), "ethernet.tsn.gate_open"))
                .or_default() += data["open_priorities"].as_array().unwrap().len() as u128;
        }
    }
    let horizon = snapshot.common.end_ps;
    if dynamic {
        summary.push(Record::aggregate(
            "@network",
            "ethernet.dynamic.policy_epoch",
            MetricValue::Integer(epoch),
            0,
            horizon,
        ));
    }
    for ((target, metric), count) in counts {
        summary.push(Record::aggregate(
            target,
            metric,
            MetricValue::Integer(count),
            0,
            horizon,
        ));
    }
    if let Some(ports) = snapshot
        .network
        .as_ref()
        .and_then(|network| network["tsn"]["ports"].as_array())
    {
        for port in ports {
            for credit in port["credits"].as_array().unwrap() {
                let magnitude = credit["credit"]["magnitude"]
                    .as_str()
                    .unwrap()
                    .parse::<u128>()
                    .unwrap();
                let mut value = ratio(magnitude, 1_000_000_000_000)?;
                if credit["credit"]["negative"] == true {
                    if let MetricValue::Number(number) = &mut value {
                        *number = -*number;
                    }
                }
                let mut record = Record::aggregate(
                    port["port"].as_str().unwrap(),
                    "ethernet.tsn.credit",
                    value,
                    0,
                    horizon,
                );
                record.reason = Some(format!("priority:{}", credit["priority"].as_str().unwrap()));
                summary.push(record);
            }
        }
    }
    Ok(())
}
