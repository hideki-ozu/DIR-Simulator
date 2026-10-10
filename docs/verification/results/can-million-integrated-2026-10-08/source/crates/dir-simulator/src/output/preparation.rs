//! Publication of a captured preparation failure without executing any model.
use super::{
    aggregate, csv,
    json::canonical,
    publish::{digest, publish_files},
};
use crate::{
    Diagnostic,
    input::{RunIdentity, inspect_config},
    types::InputSnapshot,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(crate) fn profile_catalog(profile: &str) -> Option<Value> {
    let mut names: Vec<String> = Vec::new();
    let mut schemas: Vec<(&str, u32)> = Vec::new();
    let metrics = match profile {
        "can.cc.ideal.v1" => aggregate::METRICS,
        "can.cc.multibus.v1" => {
            schemas.extend([
                ("can.receiver", 1),
                ("can.request", 1),
                ("gw.forward", 1),
                ("gw.rx_buffer", 1),
            ]);
            names.extend("gw_copy_created gw_copy_submitted gw_hop_dropped gw_route_filtered gw_processing_pending gw_rx_queue_length gw_rx_queue_max gw_rx_dropped gw_tx_buffer_wait_ps waiting_tx".split_whitespace().map(String::from));
            aggregate::METRICS
        }
        "axi4.transaction.v1" => {
            schemas.extend([
                ("axi.handshake", 1),
                ("axi.memory", 1),
                ("axi.transaction", 1),
            ]);
            super::axi::METRICS
        }
        "soc.shared.v1" | "ahb.transaction.v1" | "noc.xy.v1" => {
            let prefix = profile.split('.').next()?;
            names.extend(
                super::soc::NAMES
                    .iter()
                    .map(|name| format!("{prefix}.{name}")),
            );
            return Some(catalog(
                profile,
                names,
                vec![
                    json!({"schema_name":format!("{prefix}.transaction"),"schema_version":1}),
                    json!({"schema_name":format!("{prefix}.transfer"),"schema_version":1}),
                ],
            ));
        }
        "memory.ipc.transaction.v1" => {
            schemas.extend(
                [
                    "memory-ipc.request",
                    "memory-ipc.memory",
                    "memory-ipc.shared",
                    "memory-ipc.dma",
                    "memory-ipc.mailbox",
                    "memory-ipc.notification",
                ]
                .iter()
                .map(|name| (*name, 1)),
            );
            super::memory_ipc::METRICS
        }
        "can.fd.precomputed.v1" => {
            schemas.extend([
                ("dir.canfd.frame", 1),
                ("dir.canfd.reception", 1),
                ("dir.canfd.request", 1),
            ]);
            super::canfd::METRICS
        }
        "ethernet.l2.store-forward.v1"
        | "ethernet.l2.qos.v1"
        | "ethernet.l2.vlan.v1"
        | "ethernet.l2.store-forward.v2"
        | "ethernet.l2.100base-t1.v1" => {
            let media = matches!(
                profile,
                "ethernet.l2.store-forward.v2" | "ethernet.l2.100base-t1.v1"
            );
            let qos = matches!(profile, "ethernet.l2.qos.v1" | "ethernet.l2.vlan.v1");
            schemas.extend([
                ("ethernet.frame", 1),
                ("ethernet.reception", 1),
                ("ethernet.transfer", 1),
            ]);
            if qos {
                names.extend(
                    super::ethernet::QOS_METRICS
                        .split_whitespace()
                        .map(String::from),
                );
                schemas[0].1 = 2;
                schemas[2].1 = 2;
            }
            if profile == "ethernet.l2.vlan.v1" {
                schemas = [
                    ("ethernet.frame", 3),
                    ("ethernet.reception", 2),
                    ("ethernet.transfer", 3),
                ]
                .to_vec();
                names.extend(
                    super::ethernet::FILTER_REASONS
                        .iter()
                        .map(|reason| format!("ethernet.filtered.{reason}")),
                );
            }
            if media {
                schemas.extend([("ethernet.attempt", 1), ("ethernet.phy_link", 1)]);
                schemas[2].1 = 2;
            }
            if media {
                super::ethernet::media::METRICS
            } else {
                super::ethernet::METRICS
            }
        }
        "ethernet.l2.dynamic.v1" | "ethernet.tsn.v1" | "can.ethernet.gateway.v1" => {
            names.extend(
                super::ethernet::QOS_METRICS
                    .split_whitespace()
                    .map(String::from),
            );
            names.extend(
                super::ethernet::FILTER_REASONS
                    .iter()
                    .chain(super::ethernet::NETWORK_FILTER_REASONS.iter())
                    .map(|reason| format!("ethernet.filtered.{reason}")),
            );
            if profile == "can.ethernet.gateway.v1" {
                names.extend(aggregate::METRICS.split_whitespace().map(String::from));
                schemas.extend([
                    ("ethernet.frame", 3),
                    ("ethernet.transfer", 3),
                    ("ethernet.reception", 2),
                    ("can.request", 1),
                    ("can.receiver", 1),
                    ("dir.can_ethernet.conversion", 1),
                    ("dir.can_ethernet.branch", 1),
                    ("dir.can_ethernet.segment", 1),
                ]);
                names.extend("gateway.conversion.attempted gateway.conversion.accepted gateway.conversion.rejected gateway.rx.occupancy gateway.rx.max gateway.branch.waiting gateway.branch.admitted gateway.branch.dropped gateway.end_to_end.target_count gateway.end_to_end.completed gateway.end_to_end.latency_ps gateway.end_to_end.completion_ratio".split_whitespace().map(String::from));
            } else {
                schemas.extend([
                    ("ethernet.dynamic.frame", 1),
                    ("ethernet.dynamic.transfer", 1),
                    ("ethernet.dynamic.reception", 1),
                    ("ethernet.dynamic.control", 1),
                    ("ethernet.dynamic.policy", 1),
                ]);
                names.extend("ethernet.dynamic.mac_learned ethernet.dynamic.mac_refreshed ethernet.dynamic.mac_moved ethernet.dynamic.mac_expired ethernet.dynamic.mac_flushed ethernet.dynamic.mac_table_full ethernet.dynamic.membership_updated ethernet.dynamic.membership_expired ethernet.dynamic.router_updated ethernet.dynamic.router_expired ethernet.dynamic.registration_updated ethernet.dynamic.registration_expired ethernet.dynamic.topology_changed ethernet.dynamic.tree_published ethernet.dynamic.policy_epoch ethernet.dynamic.control_changed ethernet.dynamic.control_no_op ethernet.dynamic.control_stale ethernet.dynamic.filtered.link_down ethernet.dynamic.filtered.stp_discarding ethernet.dynamic.filtered.vlan_unregistered".split_whitespace().map(String::from));
                if profile == "ethernet.tsn.v1" {
                    schemas.extend([
                        ("ethernet.tsn.gate", 1),
                        ("ethernet.tsn.credit", 1),
                        ("ethernet.tsn.policing", 1),
                        ("ethernet.tsn.decision", 1),
                    ]);
                    names.extend("ethernet.tsn.gate_open ethernet.tsn.credit ethernet.tsn.policing_green ethernet.tsn.policing_yellow ethernet.tsn.policing_red ethernet.tsn.policing_dropped ethernet.tsn.gate_wait ethernet.tsn.guard_wait ethernet.tsn.credit_wait".split_whitespace().map(String::from));
                }
            }
            super::ethernet::METRICS
        }
        _ => return None,
    };
    names.extend(metrics.split_whitespace().map(String::from));
    Some(catalog(
        profile,
        names,
        schemas
            .into_iter()
            .map(|(name, version)| json!({"schema_name":name,"schema_version":version}))
            .collect(),
    ))
}
fn catalog(profile: &str, mut names: Vec<String>, mut schemas: Vec<Value>) -> Value {
    names.sort();
    names.dedup();
    schemas.sort_by(|a, b| a["schema_name"].as_str().cmp(&b["schema_name"].as_str()));
    let metrics:Vec<_>=names.into_iter().map(|name| { let(unit,kind,sampling,aggregation)=aggregate::descriptor(&name);json!({"metric_id":name,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation}) }).collect();
    json!({"model_profile":profile,"metrics":metrics,"model_schemas":schemas})
}

pub(crate) fn export_preparation_failure(
    config_path: &Path,
    cwd: &Path,
    inputs: &[InputSnapshot],
    identity: &RunIdentity,
    diagnostic: &Diagnostic,
    output: &Path,
    registry: &crate::registry::Registry,
) -> Result<(), Diagnostic> {
    let config = crate::input::absolute(config_path, cwd);
    let header = inputs
        .iter()
        .find(|i| i.path == config)
        .and_then(|i| inspect_config(&i.content, &i.path, cwd).ok());
    let profile = header
        .as_ref()
        .and_then(|h| h.profile.as_deref())
        .unwrap_or("can.cc.ideal.v1");
    let selected=header.as_ref().and_then(|_|registry.profile(profile)).map(|p|{
        if registry.builtin_profiles.contains(profile) || matches!(profile,"ethernet.l2.dynamic.v1"|"ethernet.tsn.v1"|"can.ethernet.gateway.v1") { profile_catalog(profile).unwrap() } else {json!({"model_profile":profile,"model_schemas":p.model_records.iter().map(|s|json!({"schema_name":s.name,"schema_version":s.version})).collect::<Vec<_>>(),"metrics":p.metrics.iter().map(|name|json!({"metric_id":name,"version":"1","unit":registry.metrics.get(name).map(|m|m.unit.as_str()).unwrap_or("count"),"value_kind":"integer","sampling":"point","aggregation":"identity"})).collect::<Vec<_>>()} )}
    });
    let version = if selected.is_some() {
        registry.profile(profile).unwrap().output_schema_version
    } else {
        1
    };
    let models=registry.profile(profile).filter(|_|selected.is_some()).map(|p| {
        if registry.builtin_profiles.contains(profile) {vec![json!({"type":profile,"version":p.implementation_version,"assumptions":[]})]}
        else {p.modules.iter().filter_map(|key|registry.module(key).map(|m|(key.clone(),json!({"type":key,"version":m.implementation_version,"assumptions":[]})))).collect::<BTreeMap<_,_>>().into_values().collect()}
    }).unwrap_or_default();
    let mut sources:Vec<_>=inputs.iter().map(|i| {
        let logical=if i.path==config {"config".to_string()}else if header.as_ref().is_some_and(|h|h.model_config.as_ref()==Some(&i.path)){"model-config".into()}else if header.as_ref().is_some_and(|h|h.workload.as_ref()==Some(&i.path)){"workload".into()}else{header.as_ref().and_then(|h|h.roots.iter().enumerate().find_map(|(n,r)|i.path.strip_prefix(r).ok().map(|p|format!("root{n}/{}",p.to_string_lossy())))).unwrap_or_else(||i.path.to_string_lossy().into_owned())};
        json!({"logical_path":logical,"canonical_path":i.path,"sha256":digest(i.content.as_bytes()),"content_utf8":i.content})
    }).collect();
    sources.sort_by(|a, b| a["logical_path"].as_str().cmp(&b["logical_path"].as_str()));
    let hashes: Vec<_> = sources
        .iter()
        .map(|s| json!({"logical_path":s["logical_path"],"sha256":s["sha256"]}))
        .collect();
    let window_ps = header
        .as_ref()
        .and_then(|h| h.general.get("metrics-window"))
        .and_then(|value| crate::input::parse_time(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(1_000_000_000);
    let mut metadata = json!({"started_at_utc":identity.started_at_utc,"finished_at_utc":super::metadata::utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(hashes)).as_bytes()),"config":[],"config_sha256":digest(b"[]"),"initial_state":[],"initial_channel_state":[],"topology":{"controllers":[]},"models":[],"metrics":[],"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","time_resolution_ps":"1","window_ps":window_ps.to_string(),"seed":null});
    for (key, value) in super::metadata::build_environment() {
        metadata[key] = json!(value);
    }
    if let Some(catalog) = selected {
        metadata["metrics"] = catalog["metrics"].clone();
        metadata["models"] = json!(models);
        if version == 2 {
            metadata["model_profile"] = json!(profile);
            metadata["model_schemas"] = catalog["model_schemas"].clone();
        }
    }
    let mut simulation = json!({"termination":"prep_failed","partial":true,"start_ps":"0","end_ps":"0","last_event_time_ps":null,"committed_events":"0","pending_events":"0","records":[],"summary":[]});
    if version == 2 {
        simulation["model_records"] = json!([]);
    } else {
        simulation["requests"] = json!([]);
        simulation["receivers"] = json!([]);
    }
    let result = json!({"schema_version":version,"run_id":identity.run_id,"metadata":metadata,"simulation":simulation});
    let files = BTreeMap::from([
        (
            "results.json",
            format!("{}\n", canonical(&result)).into_bytes(),
        ),
        ("events.csv", csv::encode(&[], &identity.run_id, version)),
        ("summary.csv", csv::encode(&[], &identity.run_id, version)),
        (
            "diagnostics.jsonl",
            format!("{}\n", canonical(&diagnostic.wire_value())).into_bytes(),
        ),
    ]);
    publish_files(
        output,
        &identity.run_id,
        version,
        "prep_failed",
        true,
        files,
    )
}
