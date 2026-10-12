use super::*;
use crate::types::{PreparedSimulation, Schedule};

pub(super) struct CloneBinding {
    pub source: String,
    pub qname: String,
    pub instance: String,
    pub child_rebind: Option<(String, String)>,
}
pub(super) struct StopBinding {
    pub owner: String,
    pub child: String,
    pub qname: String,
    pub instance: String,
}
fn attributes(values: &BTreeMap<String, crate::input::Attribute>) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(k, v)| (k.clone(), json!(v.value())))
            .collect(),
    )
}
fn endpoint(d: &crate::input::Declaration, endpoint: &str, names: &Names) -> String {
    if let Some((child, gate)) = endpoint.split_once('.') {
        let childtype = &d
            .children()
            .iter()
            .find(|(n, _)| n == child)
            .expect("validated endpoint")
            .1;
        format!(
            "{}.{}",
            names.name(d.name(), "child", child),
            names.name(childtype, "gate", gate)
        )
    } else {
        names.name(d.name(), "gate", endpoint)
    }
}
fn declaration(d: &crate::input::Declaration, names: &Names) -> Value {
    let parameters:BTreeMap<_,_>=d.parameters().iter().map(|(n,p)|(names.name(d.name(),"parameter",n),json!({"scalar":p.scalar(),"unit":p.unit(),"default":p.default(),"attributes":attributes(p.attributes())}))).collect();
    let gates: BTreeMap<_, _> = d
        .gates()
        .iter()
        .map(|(n, v)| (names.name(d.name(), "gate", n), *v))
        .collect();
    json!({"kind":d.kind(),"implementation":d.implementation(),"attributes":attributes(d.attributes()),"parameters":parameters,"gates":gates,"children":d.children().iter().map(|(n,q)|(names.name(d.name(),"child",n),names.qname(q))).collect::<Vec<_>>(),"connections":d.connections().iter().map(|c|json!({"start":endpoint(d,c.start(),names),"end":endpoint(d,c.end(),names),"channel":c.channel().map(|q|names.qname(q))})).collect::<Vec<_>>()})
}
fn declarations(i: &ReferenceInspection, names: &Names) -> BTreeMap<String, Value> {
    i.declarations
        .iter()
        .map(|d| (names.qname(&d.qname), declaration(&d.declaration, names)))
        .collect()
}
fn path_map(i: &ReferenceInspection, names: &Names) -> Result<BTreeMap<String, String>> {
    i.instances
        .iter()
        .map(|x| Ok((x.path.clone(), names.decoded(&x.path, &x.segments)?)))
        .collect()
}
fn full_endpoint(
    i: &ReferenceInspection,
    endpoint: &str,
    paths: &BTreeMap<String, String>,
    names: &Names,
) -> String {
    let (path, gate) = endpoint.rsplit_once('.').expect("expanded endpoint");
    let owner = &i
        .instances
        .iter()
        .find(|x| x.path == path)
        .expect("expanded owner")
        .type_qname;
    format!("{}.{}", paths[path], names.name(owner, "gate", gate))
}
fn channel_map(
    i: &ReferenceInspection,
    paths: &BTreeMap<String, String>,
    names: &Names,
) -> BTreeMap<String, String> {
    i.channels
        .iter()
        .map(|c| {
            let (parent, start) = c.id.split_once("::").expect("channel ID");
            let owner = &i
                .declarations
                .iter()
                .find(|d| {
                    d.qname
                        == i.instances
                            .iter()
                            .find(|x| x.path == parent)
                            .expect("channel parent")
                            .type_qname
                })
                .expect("channel parent type")
                .declaration;
            (
                c.id.clone(),
                format!("{}::{}", paths[parent], endpoint(owner, start, names)),
            )
        })
        .collect()
}
fn channels(i: &ReferenceInspection, paths: &BTreeMap<String, String>, names: &Names) -> Value {
    let ids = channel_map(i, paths, names);
    json!(i.channels.iter().map(|c|(ids[&c.id].clone(),json!({"type":names.qname(&c.type_qname),"start":full_endpoint(i,&c.start_endpoint,paths,names),"end":full_endpoint(i,&c.end_endpoint,paths,names)}))).collect::<BTreeMap<_,_>>())
}
fn schedule(s: &Schedule) -> Value {
    match s {
        Schedule::Explicit(times) => json!({"explicit":times}),
        Schedule::Periodic {
            start,
            phase,
            period,
            end,
            count,
        } => json!({"start":start,"phase":phase,"period":period,"end":end,"count":count}),
    }
}
fn prepared(
    p: &PreparedSimulation,
    paths: &BTreeMap<String, String>,
    network: &str,
) -> Result<Value> {
    if p.registered.is_some()
        || p.ethernet.is_some()
        || p.canfd.is_some()
        || p.axi.is_some()
        || p.soc.is_some()
        || p.memory_ipc.is_some()
    {
        return Err(error("Unexpected prepared model variant"));
    }
    let primary = p
        .can
        .buses
        .first()
        .ok_or_else(|| error("Missing prepared primary bus"))?;
    if p.can.bus_id != primary.id || p.can.bitrate != primary.bitrate {
        return Err(error(
            "Legacy primary bus fields do not match their documented derivation",
        ));
    }
    let path = |s: &str| -> Result<String> {
        paths
            .get(s)
            .cloned()
            .ok_or_else(|| error("Unmapped prepared instance"))
    };
    let controller = |index: usize| -> Result<String> {
        path(
            &p.can
                .controllers
                .get(index)
                .ok_or_else(|| error("Invalid prepared controller index"))?
                .id,
        )
    };
    let bus = |index: usize| -> Result<String> {
        path(
            &p.can
                .buses
                .get(index)
                .ok_or_else(|| error("Invalid prepared bus index"))?
                .id,
        )
    };
    let mut controllers = BTreeMap::new();
    for (index, c) in p.can.controllers.iter().enumerate() {
        controllers.insert(path(&c.id)?,json!({"bus":bus(p.can.controller_buses[index])?,"queue":c.queue_capacity,"tx_processing":c.tx_processing_ps,"rx_processing":c.rx_processing_ps,"filter":c.rx_filter,"tx_channel":c.tx_channel_ps,"rx_channel":c.rx_channel_ps,"gateway":p.gateway.controller_gateways[index].map(|g|path(&p.gateway.gateways[g].node)).transpose()?}));
    }
    let buses = p
        .can
        .buses
        .iter()
        .map(|b| Ok((path(&b.id)?, b.bitrate)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let generators=p.can.generators.iter().map(|g|Ok((g.id.clone(),json!({"source":controller(g.source)?,"frame":g.frame,"schedule":schedule(&g.schedule)})))).collect::<Result<BTreeMap<_,_>>>()?;
    let mut gateways = BTreeMap::new();
    for g in &p.gateway.gateways {
        let mut ports = g
            .ports
            .iter()
            .map(|&n| controller(n))
            .collect::<Result<Vec<_>>>()?;
        ports.sort();
        let mut routes = BTreeMap::new();
        for r in &g.routes {
            let mut egress = r
                .egress
                .iter()
                .map(|&n| controller(n))
                .collect::<Result<Vec<_>>>()?;
            egress.sort();
            routes.insert(r.id.clone(),json!({"ingress":controller(r.ingress)?,"egress":egress,"format":r.format,"min":r.id_min,"max":r.id_max}));
        }
        gateways.insert(path(&g.node)?,json!({"ports":ports,"routes":routes,"processing":g.processing_delay_ps,"rx_capacity":g.rx_queue_capacity,"hop_limit":g.hop_limit}));
    }
    let mut modules = p
        .common
        .module_paths
        .iter()
        .map(|s| path(s))
        .collect::<Result<Vec<_>>>()?;
    modules.sort();
    Ok(
        json!({"profile":p.common.profile,"network":network,"modules":modules,"time_limit":p.common.time_limit_ps,"metrics_window":p.common.metrics_window_ps,"max_events":p.common.max_events,"max_delta_cycles":p.common.max_delta_cycles,"channel_count":p.common.channel_count,"config_path":p.common.config_path,"workload_path":p.common.workload_path,"model_config_path":p.common.model_config_path,"controllers":controllers,"buses":buses,"generators":generators,"gateways":gateways}),
    )
}
fn values(
    i: &ReferenceInspection,
    paths: &BTreeMap<String, String>,
    names: &Names,
) -> Result<Value> {
    let channels = channel_map(i, paths, names);
    let mut result = BTreeMap::new();
    for value in &i.prepared.common.provenance.values {
        let key = value["key"]
            .as_str()
            .ok_or_else(|| error("Incomplete effective value evidence"))?;
        if !key.contains('.') {
            if !matches!(
                key,
                "network"
                    | "time-limit"
                    | "metrics-window"
                    | "max-events"
                    | "max-delta-cycles"
                    | "model-profile"
            ) {
                return Err(error("Unknown runtime effective value key"));
            }
            let normalized = if key == "network" {
                json!(
                    serde_json::to_string(&names.qname(&i.prepared.common.network))
                        .map_err(|e| error(e.to_string()))?
                )
            } else {
                value["normalized_value"].clone()
            };
            result.insert(key.to_owned(),json!({"declared_type":value["declared_type"],"quantity":value["physical_quantity"],"value":normalized,"adopted_source":value["adopted_source"]}));
            continue;
        }
        let (path, parameter) = key
            .rsplit_once('.')
            .ok_or_else(|| error("Invalid effective value key"))?;
        let (mapped, owner) = if let Some(mapped) = paths.get(path) {
            (
                mapped.clone(),
                i.instances
                    .iter()
                    .find(|x| x.path == path)
                    .unwrap()
                    .type_qname
                    .clone(),
            )
        } else if let Some(mapped) = channels.get(path) {
            (
                mapped.clone(),
                i.channels
                    .iter()
                    .find(|x| x.id == path)
                    .unwrap()
                    .type_qname
                    .clone(),
            )
        } else {
            return Err(error("Unmapped effective value owner"));
        };
        let key = format!("{mapped}.{}", names.name(&owner, "parameter", parameter));
        result.insert(key,json!({"declared_type":value["declared_type"],"quantity":value["physical_quantity"],"value":value["normalized_value"],"adopted_source":value["adopted_source"]}));
    }
    Ok(json!(result))
}
fn inputs(
    before_project: &ProjectSnapshot,
    after_project: &ProjectSnapshot,
    before: &ReferenceInspection,
    names: &Names,
) -> Result<()> {
    let old_header = crate::input::inspect_config(
        &before_project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::Config))
            .ok_or_else(|| error("Missing config"))?
            .text,
        &before_project.config,
        &before_project.cwd,
    )
    .map_err(|d| EditorError::common("E-EDITOR-SEMANTIC", d))?;
    let new_header = crate::input::inspect_config(
        &after_project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::Config))
            .ok_or_else(|| error("Missing config"))?
            .text,
        &after_project.config,
        &after_project.cwd,
    )
    .map_err(|d| EditorError::common("E-EDITOR-SEMANTIC", d))?;
    let mut channels = BTreeMap::new();
    // Build from the original map so simultaneous swaps never overwrite values.
    let general: BTreeMap<String, String> = old_header
        .general
        .iter()
        .map(|(key, value)| {
            let renamed = before
                .uses
                .iter()
                .find(|u| {
                    u.syntax_kind == ReferenceSyntaxKind::IniParameterKey && u.decoded_value == *key
                })
                .map(|u| names.decoded(key, &u.segments))
                .transpose()?
                .unwrap_or_else(|| key.clone());
            Ok((
                renamed,
                if key == "network" {
                    names.qname(value)
                } else {
                    value.clone()
                },
            ))
        })
        .collect::<Result<_>>()?;
    for (id, params) in &old_header.channels {
        let u = before
            .uses
            .iter()
            .find(|u| u.syntax_kind == ReferenceSyntaxKind::IniChannelId && u.decoded_value == *id)
            .ok_or_else(|| error("Missing Channel reference evidence"))?;
        let new_id = names.decoded(id, &u.segments)?;
        let mut mapped = BTreeMap::new();
        for (key, value) in params {
            let target = before
                .uses
                .iter()
                .find(|r| {
                    r.syntax_kind == ReferenceSyntaxKind::IniChannelParameterKey
                        && r.instance_path.as_deref() == Some(id)
                        && r.decoded_value == *key
                })
                .ok_or_else(|| error("Missing channel parameter evidence"))?;
            mapped.insert(
                names.target(&target.target).unwrap_or_else(|| key.clone()),
                value.clone(),
            );
        }
        channels.insert(new_id, mapped);
    }
    if general != new_header.general || channels != new_header.channels {
        return Err(error(
            "INI assignment/value/order identity comparison failed",
        ));
    }
    for file in before_project.files.values().filter(|f| {
        f.roles
            .iter()
            .any(|r| matches!(r, FileRole::Workload | FileRole::ModelConfig))
    }) {
        let mut expected = crate::input::parse_json(&file.text)
            .map_err(|d| EditorError::common("E-EDITOR-SEMANTIC", d))?;
        for u in before.uses.iter().filter(|u| {
            u.span.source == file.path.to_string_lossy()
                && u.syntax_kind == ReferenceSyntaxKind::JsonStringToken
        }) {
            let pointer = u
                .json_pointer
                .as_deref()
                .ok_or_else(|| error("Missing JSON pointer"))?;
            let value = expected
                .pointer_mut(pointer)
                .ok_or_else(|| error("Missing JSON reference location"))?;
            *value = json!(names.decoded(&u.decoded_value, &u.segments)?);
        }
        let actual = crate::input::parse_json(&after_project.files[&file.id].text)
            .map_err(|d| EditorError::common("E-EDITOR-SEMANTIC", d))?;
        if expected != actual {
            return Err(error(
                "JSON reference/ID/value/array-order comparison failed",
            ));
        }
    }
    Ok(())
}
fn compare_common(
    before_project: &ProjectSnapshot,
    after_project: &ProjectSnapshot,
    before: &ReferenceInspection,
    after: &ReferenceInspection,
    names: &Names,
) -> Result<()> {
    let paths = path_map(before, names)?;
    let identity = path_map(after, &Names::default())?;
    if channels(before, &paths, names) != channels(after, &identity, &Names::default()) {
        return Err(error("Expanded channel comparison failed"));
    }
    if values(before, &paths, names)? != values(after, &identity, &Names::default())? {
        return Err(error("Effective parameter value/source comparison failed"));
    }
    if prepared(
        &before.prepared,
        &paths,
        &names.qname(&before.prepared.common.network),
    )? != prepared(&after.prepared, &identity, &after.prepared.common.network)?
    {
        return Err(error("Prepared CAN/Gateway behavior comparison failed"));
    }
    inputs(before_project, after_project, before, names)
}
fn evidence(contract: &str) -> Value {
    json!({"contract":contract,"verified":["all_declarations_members_attributes_defaults_connections","all_instance_bindings","expanded_channels_and_gate_endpoints","effective_parameter_values_and_adopted_sources","prepared_can_gateway_fields_via_instance_identity","ini_assignments_values","json_reference_ids_values_array_order","patch_external_bytes"],"permitted_differences":["source_spans","input_hashes_and_provenance_spans","snapshot_local_symbol_ids","planned_names_and_path_ids"]})
}
pub(super) fn verify_rename(
    before_project: &ProjectSnapshot,
    after_project: &ProjectSnapshot,
    before: &ReferenceInspection,
    after: &ReferenceInspection,
    names: &Names,
) -> Result<Value> {
    if declarations(before, names) != declarations(after, &Names::default()) {
        return Err(error("Declaration alpha-equivalence failed"));
    }
    let expected: BTreeMap<_, _> = before
        .instances
        .iter()
        .map(|i| {
            Ok((
                names.decoded(&i.path, &i.segments)?,
                names.qname(&i.type_qname),
            ))
        })
        .collect::<Result<_>>()?;
    let actual: BTreeMap<_, _> = after
        .instances
        .iter()
        .map(|i| (i.path.clone(), i.type_qname.clone()))
        .collect();
    if expected != actual {
        return Err(error("Instance alpha-equivalence failed"));
    }
    compare_common(before_project, after_project, before, after, names)?;
    let mut proof = evidence("resolved_alpha_equivalence");
    let paths = path_map(before, names)?;
    let expected_bus = &paths[&before.prepared.can.bus_id];
    let mut differing = vec![];
    if *expected_bus != after.prepared.can.bus_id {
        differing.push("PreparedCan.bus_id");
    }
    if before.prepared.can.bitrate != after.prepared.can.bitrate {
        differing.push("PreparedCan.bitrate");
    }
    if !differing.is_empty() {
        for field in &differing {
            proof["permitted_differences"]
                .as_array_mut()
                .unwrap()
                .push(json!(field));
        }
        proof["derived_primary_bus_difference"] = json!({"fields":differing,"derivation":"lexically_first_prepared_bus","before_mapped_id":expected_bus,"before_bitrate":before.prepared.can.bitrate,"after_id":after.prepared.can.bus_id,"after_bitrate":after.prepared.can.bitrate,"first_bus_invariant_verified":true,"all_bus_values_and_runtime_bindings_verified":true});
    }
    Ok(proof)
}
pub(super) fn verify_detach(
    before_project: &ProjectSnapshot,
    after_project: &ProjectSnapshot,
    before: &ReferenceInspection,
    after: &ReferenceInspection,
    clones: &[CloneBinding],
    stop: &StopBinding,
) -> Result<Value> {
    let names = Names::default();
    let mut expected = declarations(before, &names);
    fn rebind(shape: &mut Value, child: &str, qname: &str) -> Result<()> {
        let rows = shape["children"]
            .as_array_mut()
            .ok_or_else(|| error("Missing declaration children"))?;
        let row = rows
            .iter_mut()
            .find(|r| r[0].as_str() == Some(child))
            .ok_or_else(|| error("Missing planned child binding"))?;
        row[1] = json!(qname);
        Ok(())
    }
    for clone in clones {
        let mut shape = expected[&clone.source].clone();
        if let Some((child, qname)) = &clone.child_rebind {
            rebind(&mut shape, child, qname)?;
        }
        expected.insert(clone.qname.clone(), shape);
    }
    rebind(
        expected
            .get_mut(&stop.owner)
            .ok_or_else(|| error("Unknown stopping parent"))?,
        &stop.child,
        &stop.qname,
    )?;
    if expected != declarations(after, &names) {
        return Err(error("Unexpected declaration change outside the COW plan"));
    }
    let mut expected: BTreeMap<_, _> = before
        .instances
        .iter()
        .map(|i| (i.path.clone(), i.type_qname.clone()))
        .collect();
    for clone in clones {
        expected.insert(clone.instance.clone(), clone.qname.clone());
    }
    if expected
        != after
            .instances
            .iter()
            .map(|i| (i.path.clone(), i.type_qname.clone()))
            .collect()
    {
        return Err(error("Unexpected nonselected instance binding change"));
    }
    compare_common(before_project, after_project, before, after, &names)?;
    Ok(evidence("instance_type_cow_equivalence"))
}
