//! Common provenance enrichment applies to every built-in profile.
use crate::types::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs};

macro_rules! fields {
    ($record:expr; $($field:ident),+ $(,)?) => {{
        let record = $record;
        let mut object = serde_json::Map::new();
        $(object.insert(stringify!($field).into(), json!(record.$field));)+
        Value::Object(object)
    }};
}
fn schedule(schedule: &crate::types::Schedule) -> Value {
    match schedule {
        crate::types::Schedule::Explicit(times) => json!({"kind":"explicit","times_ps":times}),
        crate::types::Schedule::Periodic {
            start,
            phase,
            period,
            end,
            count,
        } => {
            json!({"kind":"periodic","start_ps":start,"phase_ps":phase,"period_ps":period,"end_ps":end,"count":count})
        }
    }
}
fn ethernet_schedule(schedule: &crate::types::ethernet::EthernetSchedule) -> Value {
    use crate::types::ethernet::EthernetSchedule;
    match schedule {
        EthernetSchedule::Periodic {
            start_ps,
            phase_ps,
            period_ps,
            end_ps,
            count,
        } => {
            json!({"kind":"periodic","start_ps":start_ps,"phase_ps":phase_ps,"period_ps":period_ps,"end_ps":end_ps,"count":count})
        }
        EthernetSchedule::Burst {
            start_ps,
            period_ps,
            burst_count,
            frames_per_burst,
            spacing_ps,
            end_ps,
        } => {
            json!({"kind":"burst","start_ps":start_ps,"period_ps":period_ps,"burst_count":burst_count,"frames_per_burst":frames_per_burst,"spacing_ps":spacing_ps,"end_ps":end_ps})
        }
    }
}
/// Actual adapter-resolved data, including policy defaults and calculated wire
/// values. This is an additive reproduction snapshot, independent of runtime.
fn effective_model(p: &PreparedSimulation) -> Value {
    if let Some(m) = &p.registered.as_ref().filter(|m| m.is_generic()) {
        return json!({"models":m.models.iter().map(|r|fields!(r;id,subject,implementation_key,parameters,connections,output_sources)).collect::<Vec<_>>(),"channels":m.channels.iter().map(|r|fields!(r;id,implementation_key,parameters)).collect::<Vec<_>>()});
    }
    if let Some(m) = &p.axi {
        let managers = m
            .managers
            .iter()
            .map(|r| fields!(r;id,max_outstanding,b_ready,r_ready))
            .collect::<Vec<_>>();
        let mut ram = fields!(&m.ram;id,base,size,initial,aw_ready,w_ready,ar_ready,read_latency_cycles,write_response_cycles);
        ram["error_ranges"] = json!(
            m.ram
                .error_ranges
                .iter()
                .map(|r| fields!(r;start,end,access))
                .collect::<Vec<_>>()
        );
        return json!({"clock_period_ps":m.clock_period_ps,"managers":managers,"interconnect":m.interconnect,"ram":ram,"generators":m.generators.iter().map(|r|json!({"id":r.id,"source":r.source,"times_ps":r.times_ps,"transaction":fields!(&r.transaction;operation,address,beats,write_data,write_strobes)})).collect::<Vec<_>>()});
    }
    if let Some(m) = &p.soc {
        let mut value = fields!(m;profile,clock_period,bus,arbitration,bytes_per_cycle,link_cycles,input_capacity,nodes,edges);
        value["sources"] = json!(
            m.sources
                .iter()
                .map(|r| fields!(r;node,capacity,priority,router))
                .collect::<Vec<_>>()
        );
        value["targets"] = json!(
            m.targets
                .iter()
                .map(|r| fields!(r;node,base,size,cycles,errors))
                .collect::<Vec<_>>()
        );
        value["routers"] = json!(
            m.routers
                .iter()
                .map(|r| fields!(r;node,x,y))
                .collect::<Vec<_>>()
        );
        value["generators"] = json!(m.generators.iter().map(|r|json!({"id":r.id,"source":r.source,"times_ps":r.times,"transaction":fields!(&r.transaction;operation,address,bytes,destination)})).collect::<Vec<_>>());
        return value;
    }
    if let Some(m) = &p.memory_ipc {
        return json!({"placements":m.placements.iter().map(|(node,kind)|json!({"node":node,"kind":kind.name()})).collect::<Vec<_>>(),"resources":m.resources.iter().map(|r|{let mut v=fields!(r;node,queue_capacity,size,initial,faults,banks,row_bytes,width_bytes,ports,slots,payload_bytes,capacity,chunk_bytes,times,producers,consumers);v["kind"]=json!(r.kind.name());v}).collect::<Vec<_>>(),"generators":m.generators.iter().map(|r|json!({"id":r.id,"node":r.node,"times_ps":r.times,"request":fields!(&r.request;op,actor,address,length,bytes,src,dst,src_address,dst_address)})).collect::<Vec<_>>()});
    }
    if let Some(m) = &p.ethernet {
        let devices = m.devices.iter().map(|r|{let mut v=fields!(r;id,kind,mac,queue_capacity,tx_processing_delay_ps,rx_processing_delay_ps,forward_delay_ps,fdb,unknown_multicast);v["vlan_fdb"]=json!(r.vlan_fdb.iter().map(|((vid,mac),port)|json!({"vid":vid,"mac":mac,"port":port})).collect::<Vec<_>>());v["multicast"]=json!(r.multicast.iter().map(|((vid,mac),ports)|json!({"vid":vid,"mac":mac,"ports":ports})).collect::<Vec<_>>());v["subscriptions"]=json!(r.subscriptions);v}).collect::<Vec<_>>();
        return json!({"devices":devices,"directions":m.directions,"outputs":m.outputs,"port_policies":m.port_policies,"media":m.media,"generators":m.generators.iter().map(|r|json!({"id":r.id,"source":r.source,"source_vlan_id":r.source_vlan_id,"times_ps":r.times_ps,"schedule":r.schedule.as_ref().map(ethernet_schedule),"flow_id":r.flow_id,"priority":r.priority,"deadline_ps":r.deadline_ps,"frame":r.frame})).collect::<Vec<_>>()});
    }
    if let Some(m) = &p.canfd {
        return json!({"bus_id":m.bus_id,"nominal_rate":m.nominal_rate,"data_rate":m.data_rate,"controllers":m.controllers,"generators":m.generators.iter().map(|r|json!({"id":r.id,"source":r.source,"times_ps":r.times_ps,"frame":fields!(&r.frame;format,id,data,dlc,brs,nominal_bits,data_bits,evidence,binding_sha256,nominal_rate,data_rate,duration_ps,occupancy_ps,arbitration)})).collect::<Vec<_>>()});
    }
    json!({"bus_id":p.can.bus_id,"bitrate":p.can.bitrate,"buses":p.can.buses,"controller_buses":p.can.controller_buses,"controllers":p.can.controllers,"generators":p.can.generators.iter().map(|r|json!({"id":r.id,"source":r.source,"frame":r.frame,"schedule":schedule(&r.schedule)})).collect::<Vec<_>>(),"gateways":p.gateway.gateways,"controller_gateways":p.gateway.controller_gateways})
}

fn os() -> String {
    let release = fs::read_to_string("/etc/os-release").ok().and_then(|text| {
        text.lines().find_map(|line| {
            line.strip_prefix("PRETTY_NAME=")
                .map(|value| value.trim_matches('"').to_owned())
        })
    });
    release
        .map(|release| format!("{} / {release}", std::env::consts::OS))
        .unwrap_or_else(|| std::env::consts::OS.into())
}
fn cpu() -> String {
    let model = fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| {
        text.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| matches!(key.trim(), "model name" | "Hardware" | "Processor"))
                .map(|(_, value)| value.trim().to_owned())
        })
    });
    model
        .map(|model| format!("{} / {model}", std::env::consts::ARCH))
        .unwrap_or_else(|| std::env::consts::ARCH.into())
}
fn binary_hash() -> String {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        #[cfg(target_os = "linux")]
        let bytes = fs::read("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let bytes = std::env::current_exe().and_then(fs::read);
        bytes
            .map(|bytes| super::digest(&bytes))
            .unwrap_or_else(|_| "unknown".into())
    })
    .clone()
}

pub(super) fn build_environment() -> BTreeMap<String, String> {
    let build: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/build_provenance.rs"));
    let mut values = build
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect::<BTreeMap<_, _>>();
    values.insert("os".into(), os());
    values.insert("cpu".into(), cpu());
    values.insert("binary_sha256".into(), binary_hash());
    values
}

pub(super) fn enrich(prepared: &PreparedSimulation, result: &mut Value) -> Result<(), Diagnostic> {
    let provenance = &prepared.common.provenance;
    result["source_roots"] = json!(provenance.roots);
    result["declarations"] = json!(provenance.declarations);
    result["value_provenance"] = json!(provenance.values);
    let mut config = result["config"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["key"].as_str().unwrap().to_owned(),
                v["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // Public PreparedSimulation fields may be adjusted by a library caller.
    // Values used by execution take precedence over the captured input ledger.
    for (key, value) in &provenance.resolved_config {
        config.entry(key.clone()).or_insert_with(|| value.clone());
    }
    // Filesystem locations live in sources/source_roots. References in the
    // canonical effective configuration use stable logical source identifiers.
    if prepared.common.model_config_path.is_some() {
        config.insert(
            "model-config".into(),
            super::canonical(&json!("model-config")),
        );
    }
    config.remove("model-config-content");
    let effective = effective_model(prepared);
    result["effective_value_provenance"] = json!(crate::input::effective_value_provenance(
        prepared, &effective
    )?);
    config.insert("@prepared-model".into(), super::canonical(&effective));
    let config = config
        .into_iter()
        .map(|(key, value)| json!({"key":key,"value":value}))
        .collect::<Vec<_>>();
    result["config_sha256"] = json!(super::digest(super::canonical(&json!(config)).as_bytes()));
    result["config"] = json!(config);
    let mut initial = result["initial_state"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["instance"].as_str().unwrap().to_owned(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    // Some profile builders append module placeholders after richer states.
    // Preserve the actual state when the same instance is listed twice.
    for state in result["initial_state"].as_array().unwrap() {
        if state["state"] != "{}" {
            initial.insert(state["instance"].as_str().unwrap().into(), state.clone());
        }
    }
    let mut channels = provenance.channels.clone();
    if let Some(registered) = prepared.registered.as_ref().filter(|r| r.is_generic()) {
        for channel in &registered.channels {
            let descriptor = &registered.registry.channels[&channel.implementation_key].descriptor;
            let settings = channel
                .parameters
                .iter()
                .map(|(key, value)| {
                    let dimension = descriptor
                        .parameters
                        .iter()
                        .find(|p| p.name == *key)
                        .map(|p| p.dimension)
                        .unwrap_or(crate::registry::Dimension::Dimensionless);
                    (
                        key.clone(),
                        super::super::registered::parameter_value(value, dimension),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            if let Some(row) = channels
                .iter_mut()
                .find(|row| row["instance"] == channel.id)
            {
                let mut state: Value = serde_json::from_str(row["state"].as_str().unwrap())
                    .map_err(|e| {
                        Diagnostic::output(format!("Invalid captured channel state: {e}"))
                    })?;
                state["settings"] = json!(settings);
                row["state"] = json!(super::canonical(&state));
            }
        }
    }
    for channel in &channels {
        initial.insert(
            channel["instance"].as_str().unwrap().into(),
            channel.clone(),
        );
    }
    result["initial_state"] = json!(initial.into_values().collect::<Vec<_>>());
    result["initial_channel_state"] = json!(channels);
    let build = super::build_environment();
    for (key, value) in &build {
        result[key] = json!(value);
    }
    if let Some(identity) = &prepared.common.run_identity {
        result["started_at_utc"] = json!(identity.started_at_utc);
    }
    result["implementation_coverage"]["reproduction_conditions"] =
        json!(if build.values().any(|v| v == "unknown") {
            "partially_identified"
        } else {
            "identified"
        });
    if let Some(limitations) = result["implementation_coverage"]["limitations"].as_array_mut() {
        limitations.retain(|v| {
            v.as_str().is_none_or(|s| {
                !s.contains("build provenance")
                    && !s.contains("build compiler")
                    && !s.contains("OS release")
                    && !s.contains("source logical roots")
                    && !s.contains("resolved config covers")
                    && !s.contains("initial channel states")
                    && !s.contains("start timestamp and run_id")
                    && !s.contains("public Registry/Envelope")
                    && !s.contains("prepare failures use CLI")
            })
        });
        if prepared.common.run_identity.is_none() {
            limitations.push(json!(
                "direct export without execution identity uses export time for started_at_utc"
            ));
            result["implementation_coverage"]["reproduction_conditions"] =
                json!("partially_identified");
        }
    }
    Ok(())
}
