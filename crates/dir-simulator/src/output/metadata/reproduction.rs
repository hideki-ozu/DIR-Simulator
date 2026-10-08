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
fn tsn_schedule(s: &crate::types::ethernet::tsn::Schedule) -> Value {
    let mut value = fields!(s;id,base_ps,cycle_ps,entries,prefix_ps,open_total_ps);
    value["class_open_runs"] = json!(
        s.class_open_runs
            .iter()
            .map(|runs| {
                runs.iter()
                    .map(|r| json!({"start_ps":r.start_ps,"end_ps":r.end_ps.to_string()}))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    );
    value
}
fn registered_model(m: &crate::registry::PreparedRegistered) -> Value {
    json!({"models":m.models.iter().map(|r|fields!(r;id,subject,implementation_key,parameters,connections,output_sources)).collect::<Vec<_>>(),"channels":m.channels.iter().map(|r|fields!(r;id,implementation_key,parameters)).collect::<Vec<_>>()})
}
/// Actual adapter-resolved data, including policy defaults and calculated wire
/// values. This is an additive reproduction snapshot, independent of runtime.
fn effective_model(p: &PreparedSimulation) -> Value {
    if let Some(network) = p.registered.as_ref().and_then(|m| m.network.as_ref()) {
        // A composed profile also uses the generic runtime, but its coordinator
        // parameters do not describe the network's resolved policies.
        let mut base = p.clone();
        base.registered = None;
        base.ethernet = Some(network.ethernet.clone());
        let ethernet = effective_model(&base);
        let dynamic = network.dynamic.as_ref().map(|m| {
            fields!(m;mac_age_ps,convergence_ps,bridges,links,registrable,limits,controls,generator_ip,mac_keys)
        });
        let tsn = network.tsn.as_ref().map(|m| {
            json!({
                "outputs":m.outputs.iter().map(|r|json!({"port":r.port,"link_bps":r.link_bps,"tas":r.tas.as_deref().map(tsn_schedule),"cbs":r.cbs.iter().map(|c|c.as_ref().map(|c|json!({"priority":c.priority,"idle_bps":c.idle_bps,"link_bps":c.link_bps,"hi":c.hi.to_string(),"lo":c.lo.to_string()}))).collect::<Vec<_>>()})).collect::<Vec<_>>(),
                "streams":m.streams.iter().map(|r|json!({"id":r.id,"key":r.key,"max_sdu_bytes":r.max_sdu_bytes,"gate":r.gate.as_deref().map(tsn_schedule),"meter":r.meter.as_ref().map(|m|json!({"committed_rate_bps":m.committed_rate_bps,"peak_rate_bps":m.peak_rate_bps,"committed_cap":m.committed_cap.to_string(),"peak_cap":m.peak_cap.to_string(),"yellow_drop":m.yellow_drop}))})).collect::<Vec<_>>(),
                "updates":m.updates.iter().map(|r|json!({"id":r.id,"submitted_at_ps":r.submitted_at_ps,"effective_at_ps":r.effective_at_ps,"port":r.port,"schedule":tsn_schedule(r.schedule.as_ref())})).collect::<Vec<_>>()
            })
        });
        let bridge = network.bridge.as_ref().map(|m| {
            base.ethernet = None;
            base.can = m.can.clone();
            json!({"can":effective_model(&base),"gateways":m.gateways})
        });
        let mut effective = registered_model(p.registered.as_ref().unwrap());
        effective["ethernet"] = ethernet;
        effective["dynamic"] = json!(dynamic);
        effective["tsn"] = json!(tsn);
        effective["bridge"] = json!(bridge);
        return effective;
    }
    if let Some(m) = &p.registered.as_ref().filter(|m| m.is_generic()) {
        return registered_model(m);
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


fn ledger_identified(build: &BTreeMap<String, String>) -> bool {
    let hash_valid = build.get("adoption_ledger_sha256").is_some_and(|hash| {
        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    let version_valid = build.get("adoption_ledger_version").is_some_and(|version| {
        let parts = version.split('.').collect::<Vec<_>>();
        parts.len() == 3
            && parts.iter().all(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
            })
    });
    hash_valid && version_valid
}

fn reproduction_identified(build: &BTreeMap<String, String>) -> bool {
    ledger_identified(build)
        && !build.values().any(|value| {
            value.is_empty() || value == "unknown" || value == "not-present"
        })
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
        json!(if reproduction_identified(&build) {
            "identified"
        } else {
            "partially_identified"
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
        if !ledger_identified(&build) {
            limitations.push(json!(
                "adoption ledger hash/document version is missing or invalid; distribution approval is not established"
            ));
        }
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

#[cfg(test)]
mod adoption_tests {
    use super::reproduction_identified;
    use std::collections::BTreeMap;

    fn identified_build() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("compiler".into(), "rustc 1.85.0".into()),
            ("adoption_ledger_sha256".into(), "a".repeat(64)),
            ("adoption_ledger_version".into(), "1.1.0".into()),
        ])
    }

    #[test]
    fn missing_ledger_never_identifies_reproduction() {
        let mut build = identified_build();
        assert!(reproduction_identified(&build));
        for missing in ["not-present", "unknown", "", "abc"] {
            build.insert("adoption_ledger_sha256".into(), missing.into());
            assert!(!reproduction_identified(&build));
        }
        build.remove("adoption_ledger_sha256");
        assert!(!reproduction_identified(&build));
    }

    #[test]
    fn ledger_version_and_other_build_fields_must_be_identified() {
        let mut build = identified_build();
        for missing in ["not-present", "unknown", "", "3309dbdb08e76eb3690d108790a0ad4fa7040c1b"] {
            build.insert("adoption_ledger_version".into(), missing.into());
            assert!(!reproduction_identified(&build));
        }
        build = identified_build();
        build.insert("compiler".into(), "unknown".into());
        assert!(!reproduction_identified(&build));
    }
}

