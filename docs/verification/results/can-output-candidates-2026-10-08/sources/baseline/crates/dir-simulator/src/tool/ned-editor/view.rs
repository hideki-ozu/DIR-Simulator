use super::analysis::{self, DeclarationShape};
use super::controller::Controller;
use super::input::{FileRole, ProjectOrigin};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn role(file: &super::input::CapturedFile) -> &'static str {
    match file.roles.first() {
        Some(FileRole::Config) => "config",
        Some(FileRole::Workload) => "workload",
        Some(FileRole::ModelConfig) => "model_config",
        _ => "ned",
    }
}

fn expand(
    types: &BTreeMap<String, (String, DeclarationShape)>,
    name: &str,
    path: &str,
    depth: usize,
    stack: &mut BTreeSet<String>,
    out: &mut Vec<Value>,
) {
    if out.len() >= 10_000 {
        return;
    }
    let Some((key, d)) = types.get(name) else {
        out.push(json!({"path":path,"type_name":name,"unresolved":true,"depth":depth}));
        return;
    };
    let cycle = stack.contains(name) || depth >= 128;
    out.push(json!({"path":path,"type_name":name,"type_key":key,"kind":d.kind,"implementation":d.implementation,"depth":depth,"unresolved":false,"cycle":cycle}));
    if cycle {
        return;
    }
    stack.insert(name.into());
    for (child, ty) in &d.children {
        expand(types, ty, &format!("{path}.{child}"), depth + 1, stack, out);
    }
    stack.remove(name);
}
pub(crate) fn project(c: &Controller, q: &BTreeMap<String, String>) -> Value {
    let mut value = json!({"schema_version":1,"session_id":c.session_id,"revision":"0","input_revision":"0","view_sequence":c.view_sequence.to_string(),"request_scope_generation":q.get("request_scope_generation"),"origin":c.config,"network":null,"dirty":false,"never_exported":true,"busy":c.busy,"writer":{"client_id":c.writer_client,"epoch":c.writer_epoch.to_string()},"files":[],"types":[],"instances":[],"source":null,"graph":null,"parameters":[],"diagnostics":c.diagnostics,"analysis":"unavailable","outputs":c.registry.targets(),"export_roots":c.registry.export_roots(),"recovery":c.recovery,"recovery_only":c.recovery_only,"limits":{"body_bytes":"67108864","history_bytes":"33554432","history_operations":200},"can_undo":false,"can_redo":false});
    value["templates"] = json!(super::template::templates());
    value["catalog"] = json!(super::template::catalog());
    let Some(m) = &c.model else {
        return value;
    };
    value["revision"] = json!(m.revision.to_string());
    value["input_revision"] = json!(m.input_revision.to_string());
    value["dirty"] = json!(m.dirty());
    value["never_exported"] = json!(m.never_exported);
    value["network"] = json!(m.project.header.network);
    value["analysis"] = json!(m.analysis);
    value["can_undo"] = json!(m.can_undo());
    value["can_redo"] = json!(m.can_redo());
    value["last_output"] = json!(m.last_output);
    value["differs_from_origin"] = json!(m.differs_from_origin());
    let new = matches!(m.project.origin, ProjectOrigin::New { .. });
    value["project_origin"] = json!(if new { "new" } else { "disk" });
    value["project_template"] = json!(match &m.project.origin {
        ProjectOrigin::New { template, .. } => Some(template.as_str()),
        ProjectOrigin::Disk => None,
    });
    value["project_name"] = json!(match &m.project.origin {
        ProjectOrigin::New { name, .. } => name.clone(),
        ProjectOrigin::Disk => m
            .project
            .config
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    });
    value["capabilities"] = json!({"reload":{"enabled":!new,"reason":if new {"新規プロジェクトはまだ読込元がありません"}else{""}}});
    value["outputs"] = json!(
        c.registry
            .targets()
            .into_iter()
            .filter(|t| t["kind"] != "source_project"
                || (!new
                    && m.project.files.values().all(|f| f.stat.is_some())
                    && t["path"].as_str().is_some_and(
                        |p| Some(std::path::Path::new(p)) == m.project.config.parent()
                    )))
            .collect::<Vec<_>>()
    );
    value["files"]=json!(m.project.files.values().map(|f|json!({"id":f.id,"path":f.path,"hash":f.hash,"dirty":m.file_dirty(&f.id),"syntax":m.syntax.get(&f.id),"role":role(f)})).collect::<Vec<_>>());
    let general = &m.project.header.general;
    value["project_settings"] = json!({"sim_time_limit":general.get("sim-time-limit").map(String::as_str).unwrap_or("10ms"),"metrics_window":general.get("metrics-window").map(String::as_str).unwrap_or("1ms"),"max_events":general.get("max-events").map(String::as_str).unwrap_or("100000000"),"max_delta_cycles":general.get("max-delta-cycles").map(String::as_str).unwrap_or("1000000")});
    let mut config_errors = vec![];
    for (role, key) in [
        (FileRole::Workload, "workload"),
        (FileRole::ModelConfig, "gateway_settings"),
    ] {
        match super::settings::document(m, role) {
            Ok(doc) => {
                value[key] = if key == "gateway_settings" {
                    doc["gateways"].clone()
                } else {
                    doc
                };
            }
            Err(e) => {
                value[key] = Value::Null;
                config_errors.push(e.message);
            }
        }
    }
    value["configuration_errors"] = json!(config_errors);
    let mut types = Vec::new();
    let mut by_name = BTreeMap::new();
    let mut duplicate = BTreeSet::new();
    let mut selected = None;
    for f in m.project.ned_files() {
        if let Some(parsed) = m.project.parsed.get(&f.id) {
            for (i, d) in analysis::shapes(parsed).into_iter().enumerate() {
                let key = analysis::type_key(&f.id, i, &d.name);
                types.push(json!({"key":key,"file_id":f.id,"name":d.name,"kind":d.kind,"implementation":d.implementation}));
                if m.current_parse(&f.id).is_ok()
                    && by_name
                        .insert(d.name.clone(), (key.clone(), d.clone()))
                        .is_some()
                {
                    duplicate.insert(d.name.clone());
                }
                if q.get("type_key") == Some(&key) {
                    selected = Some((f.id.clone(), key, d));
                }
            }
        }
    }
    for name in duplicate {
        by_name.remove(&name);
    }
    value["types"] = json!(types);
    let mut instances = Vec::new();
    if let Some(network) = m.project.header.network.as_ref() {
        let root = network.rsplit('.').next().unwrap_or(network);
        expand(
            &by_name,
            network,
            root,
            0,
            &mut BTreeSet::new(),
            &mut instances,
        );
    }
    value["instances"] = json!(instances);
    let file = q
        .get("file_id")
        .and_then(|id| m.project.files.get(id))
        .or_else(|| {
            selected
                .as_ref()
                .and_then(|(id, _, _)| m.project.files.get(id))
        })
        .or_else(|| m.project.ned_files().next());
    if let Some(f) = file {
        value["source"] =
            json!({"file_id":f.id,"text":f.text.as_ref(),"hash":f.hash,"role":role(f)});
        if selected.is_none() {
            if let Some(parsed) = m.project.parsed.get(&f.id) {
                let ds = analysis::shapes(parsed);
                let ordinal = ds.iter().position(|d| d.kind == "network").unwrap_or(0);
                if let Some(d) = ds.get(ordinal) {
                    selected = Some((
                        f.id.clone(),
                        analysis::type_key(&f.id, ordinal, &d.name),
                        d.clone(),
                    ));
                }
            }
        }
    }
    if let Some((id, key, d)) = selected {
        let f = &m.project.files[&id];
        let stale = m.parsed_hashes.get(&id) != Some(&f.hash);
        let layout = m
            .project
            .layouts
            .get(&id)
            .and_then(|l| l.adopted.as_ref())
            .and_then(|l| l.types.get(&d.name));
        let nodes:Vec<_>=d.children.iter().enumerate().map(|(i,(name,ty))|{
            let automatic=super::layout::Position::new((i%4)as f64*240.0,(i/4)as f64*160.0);let position=layout.and_then(|l|l.nodes.get(name)).unwrap_or(&automatic);let shape=by_name.get(ty).map(|(_,d)|d);
            let gates:Vec<_>=shape.map(|d|d.gates.iter().map(|(gate,output)|json!({"name":gate,"output":output,"key":format!("{name}.{gate}")})).collect()).unwrap_or_default();
            json!({"key":analysis::node_key(&key,name),"name":name,"type_name":ty,"x":position.x,"y":position.y,"collapsed":position.collapsed,"gates":gates,"unresolved":shape.is_none(),"implementation":shape.and_then(|d|d.implementation.as_deref())})
        }).collect();
        let own: Vec<_> = d
            .gates
            .iter()
            .map(|(name, output)| json!({"name":name,"output":output,"key":name}))
            .collect();
        let connections:Vec<_>=d.connections.iter().enumerate().map(|(i,c)|json!({"key":analysis::connection_key(&key,i),"start":c.start,"end":c.end,"channel":c.channel})).collect();
        value["graph"] = json!({"type_key":key,"file_id":id,"type_name":d.name,"kind":d.kind,"implementation":d.implementation,"gate_editable":d.kind=="module" || matches!(d.implementation.as_deref(),Some("dir.can.Bus"|"dir.can.MultibusBus")),"source_hash":m.parsed_hashes.get(&id),"stale":stale,"nodes":nodes,"own_gates":own,"connections":connections});
        let instance = q.get("instance_path").filter(|path| {
            value["instances"].as_array().is_some_and(|xs| {
                xs.iter().any(|i| {
                    i["path"].as_str() == Some(path) && i["type_name"].as_str() == Some(&d.name)
                })
            })
        });
        let parameters:Vec<_>=d.parameters.iter().map(|(name,p)|{
            let override_value=instance.and_then(|path|m.project.header.general.get(&format!("{path}.{name}")));
            let effective=instance.and_then(|path|m.prepared.as_ref().and_then(|prepared|{
                if let Some(ctrl)=prepared.can.controllers.iter().find(|ctrl|&ctrl.id==path){match name.as_str(){"queueCapacity"=>Some(ctrl.queue_capacity.to_string()),"txProcessingDelay"=>Some(format!("{}ps",ctrl.tx_processing_ps)),"rxProcessingDelay"=>Some(format!("{}ps",ctrl.rx_processing_ps)),"rxFilter"=>Some(ctrl.rx_filter.clone()),_=>None}}else if name=="bitrate"{prepared.can.buses.iter().find(|b|&b.id==path).map(|b|format!("{}bps",b.bitrate))}else{None}
            }));
            json!({"key":analysis::parameter_key(&key,name),"name":name,"scalar":p.scalar,"unit":p.unit,"default":p.default,"override":override_value,"effective":effective,"instance_path":instance})
        }).collect();
        value["parameters"] = json!(parameters);
    }
    let mut diagnostics = c.diagnostics.clone();
    diagnostics.extend(m.diagnostics.clone());
    for (id, l) in &m.project.layouts {
        if let Some(w) = &l.warning {
            diagnostics.push(json!({"origin":"layout","code":"W-EDITOR-LAYOUT","severity":"warning","message":w,"file_id":id,"input_revision":m.input_revision.to_string()}));
        }
    }
    value["diagnostics"] = json!(diagnostics);
    value
}
