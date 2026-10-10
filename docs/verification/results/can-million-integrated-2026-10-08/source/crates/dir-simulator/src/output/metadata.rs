//! Prepared-input provenance and result metadata.
use super::{
    PROFILE,
    aggregate::{METRICS, descriptor},
    json::canonical,
    model_records::normalized_gateway,
    publish::digest,
};
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
mod reproduction;
pub(super) fn build_environment() -> BTreeMap<String, String> {
    reproduction::build_environment()
}

pub(super) fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (seconds / 86400) as i64;
    // Gregorian civil date conversion, epoch 1970-01-01.
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}
pub(super) fn build(prepared: &PreparedSimulation, timestamp: &str) -> Result<Value, Diagnostic> {
    let mut result = build_profile(prepared, timestamp)?;
    reproduction::enrich(prepared, &mut result)?;
    Ok(result)
}
fn build_profile(prepared: &PreparedSimulation, timestamp: &str) -> Result<Value, Diagnostic> {
    let mut sources = Vec::new();
    for input in &prepared.common.inputs {
        // Preparation already resolved the path. Export must not reacquire inputs:
        // the source can be moved or removed after a successful prepare call.
        let canonical_path = &input.path;
        let logical = prepared
            .common
            .provenance
            .logical_path(prepared, &input.path);
        sources.push(json!({"logical_path":logical,"canonical_path":canonical_path.to_string_lossy(),"sha256":digest(input.content.as_bytes()),"content_utf8":input.content}));
    }
    sources.sort_by(|a, b| a["logical_path"].as_str().cmp(&b["logical_path"].as_str()));
    let source_hashes: Vec<_> = sources
        .iter()
        .map(|s| json!({"logical_path":s["logical_path"],"sha256":s["sha256"]}))
        .collect();
    let mut config = BTreeMap::new();
    config.insert(
        "network".to_string(),
        canonical(&json!(prepared.common.network)),
    );
    config.insert(
        "time-limit".into(),
        format!("{}ps", prepared.common.time_limit_ps),
    );
    config.insert(
        "metrics-window".into(),
        format!("{}ps", prepared.common.metrics_window_ps),
    );
    config.insert("max-events".into(), prepared.common.max_events.to_string());
    config.insert(
        "max-delta-cycles".into(),
        prepared.common.max_delta_cycles.to_string(),
    );
    if prepared.registered.as_ref().is_some_and(|r| r.is_generic()) {
        return super::registered::metadata(prepared, timestamp, sources, source_hashes, config);
    }
    if prepared.ethernet.is_some() {
        return super::ethernet::metadata(prepared, timestamp, sources, source_hashes, config);
    }
    if prepared.canfd.is_some() {
        return super::canfd::metadata(prepared, timestamp, sources, source_hashes, config);
    }
    if prepared.axi.is_some() {
        return super::axi::metadata(prepared, timestamp, sources, source_hashes, config);
    }
    if prepared.soc.is_some() {
        return super::soc::metadata(prepared, timestamp, sources, source_hashes, config);
    }
    if prepared.memory_ipc.is_some() {
        return super::memory_ipc::metadata(prepared, timestamp, sources, source_hashes, config);
    }
    config.insert(
        format!("{}.bitrate", prepared.can.bus_id),
        format!("{}bit/s", prepared.can.bitrate),
    );
    if prepared.common.profile != PROFILE {
        config.insert(
            "model-profile".into(),
            canonical(&json!(prepared.common.profile)),
        );
        for bus in &prepared.can.buses {
            config.insert(
                format!("{}.bitrate", bus.id),
                format!("{}bit/s", bus.bitrate),
            );
        }
        if let Some(source) = sources.iter().find(|s| s["logical_path"] == "model-config") {
            config.insert("model-config".into(), canonical(&source["canonical_path"]));
        }
        for gateway in &prepared.gateway.gateways {
            config.insert(
                format!("@profile:{}:{}", prepared.common.profile, gateway.node),
                canonical(&normalized_gateway(prepared, gateway)),
            );
        }
    }
    for c in &prepared.can.controllers {
        for (name, value) in [
            ("queueCapacity", c.queue_capacity.to_string()),
            ("txProcessingDelay", format!("{}ps", c.tx_processing_ps)),
            ("rxProcessingDelay", format!("{}ps", c.rx_processing_ps)),
            ("rxFilter", canonical(&json!(c.rx_filter))),
            ("txChannelDelay", format!("{}ps", c.tx_channel_ps)),
            ("rxChannelDelay", format!("{}ps", c.rx_channel_ps)),
        ] {
            config.insert(format!("{}.{}", c.id, name), value);
        }
    }
    let config: Vec<_> = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect();
    let mut metrics: Vec<_> = METRICS.split_whitespace().map(|metric| { let (unit,kind,sampling,aggregation) = descriptor(metric); json!({"metric_id":metric,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation}) }).collect();
    if prepared.common.profile != PROFILE {
        metrics.extend(["gw_copy_created", "gw_copy_submitted", "gw_hop_dropped", "gw_route_filtered", "gw_processing_pending", "gw_rx_queue_length", "gw_rx_queue_max", "gw_rx_dropped", "gw_tx_buffer_wait_ps", "waiting_tx"].into_iter().map(|metric| { let (unit,kind,sampling,aggregation) = descriptor(metric); json!({"metric_id":metric,"version":"1","unit":unit,"value_kind":kind,"sampling":sampling,"aggregation":aggregation}) }));
    }
    metrics.sort_by(|a, b| a["metric_id"].as_str().cmp(&b["metric_id"].as_str()));
    let mut initial_state = vec![
        json!({"instance":prepared.common.network,"state":"{}"}),
        json!({"instance":prepared.can.bus_id,"state":canonical(&json!({"profile":PROFILE,"state":"idle","active_request":null}))}),
    ];
    if prepared.common.profile != PROFILE {
        initial_state.truncate(1);
        initial_state.extend(
            prepared
                .common
                .module_paths
                .iter()
                .map(|path| json!({"instance":path,"state":"{}"})),
        );
        initial_state.extend(prepared.can.buses.iter().map(|bus| json!({"instance":bus.id,"state":canonical(&json!({"profile":prepared.common.profile,"state":"idle","active_request":null}))})));
        initial_state.extend(prepared.gateway.gateways.iter().map(|gw| json!({"instance":format!("@profile:{}:{}",prepared.common.profile,gw.node),"state":canonical(&json!({"profile":prepared.common.profile,"forward_records":[],"pending":[],"rx_buffers":[]}))})));
    }
    for (i, c) in prepared.can.controllers.iter().enumerate() {
        let mut generators: Vec<_> = prepared.can.generators.iter().filter(|g| g.source == i).map(|g| json!({"id":g.id,"next_ordinal":"0","next_time_ps":g.schedule.time(0).map(|n| n.to_string())})).collect();
        generators.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        initial_state.push(json!({"instance":c.id,"state":canonical(&json!({"profile":prepared.common.profile,"queue":[],"processing":[],"transmitting":null,"receivers":[],"generators":generators}))}));
    }
    initial_state.sort_by(|a, b| a["instance"].as_str().cmp(&b["instance"].as_str()));
    let mut result = json!({"started_at_utc":timestamp,"finished_at_utc":utc_now(),"sources":sources,"input_sha256":digest(canonical(&json!(source_hashes)).as_bytes()),"config_sha256":digest(canonical(&json!(config)).as_bytes()),"config":config,"runtime_version":env!("CARGO_PKG_VERSION"),"model_registry_version":"1","models":[{"type":prepared.common.profile,"version":"1","assumptions":["ideal ACK","no retransmission","no error injection","content-dependent frame duration","fixed propagation delay","no source self-delivery","filter independent of ACK"]}],"initial_state":initial_state,"metrics":metrics,"time_resolution_ps":"1","window_ps":prepared.common.metrics_window_ps.to_string(),"seed":null});
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
    result["topology"] = json!({"controllers":prepared.can.controllers.iter().enumerate()
        .map(|(index,c)| json!({"id":c.id,"bus":prepared.can.buses[prepared.can.controller_buses[index]].id,
            "tx_channel_delay_ps":c.tx_channel_ps.to_string(),"rx_channel_delay_ps":c.rx_channel_ps.to_string()}))
        .collect::<Vec<_>>()});
    result["os"] = json!(std::env::consts::OS);
    result["cpu"] = json!(std::env::consts::ARCH);
    result["implementation_coverage"] = json!({"profile":"can-v0.1","reproduction_conditions":"partially_identified","limitations":["start timestamp and run_id collected at export rather than execution start","source logical roots inferred from filename; source root mapping unavailable","resolved config covers prepared fields rather than every source setting","initial channel states unavailable","build provenance compiler/toolchain/commit/lock unavailable","OS release and CPU model unavailable"]});
    if prepared.common.profile != PROFILE {
        result["model_profile"] = json!(prepared.common.profile);
        result["model_schemas"] = json!([{"schema_name":"can.receiver","schema_version":1},{"schema_name":"can.request","schema_version":1},{"schema_name":"gw.forward","schema_version":1},{"schema_name":"gw.rx_buffer","schema_version":1}]);
        result["implementation_coverage"]["profile"] = json!(prepared.common.profile);
        result["implementation_coverage"]["limitations"].as_array_mut().unwrap().extend([
            json!("profile uses the internal CAN engine; public Registry/Envelope event codec API unavailable"),
            json!("prepare failures use CLI E-0001 diagnostics without schema2 result publication"),
        ]);
    }
    Ok(result)
}
