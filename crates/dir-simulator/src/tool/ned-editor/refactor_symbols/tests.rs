use super::super::input::load_project;
use super::*;
use std::path::Path;
fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn session(name: &str) -> EditorSession {
    let project = load_project(Path::new(name), &root()).unwrap();
    EditorSession::new(project)
}
fn refresh(model: &mut EditorSession) {
    model.project.parsed = parse_current_inputs(&model.project).unwrap();
    model.parsed_hashes = model
        .project
        .ned_files()
        .map(|f| (f.id.clone(), f.hash.clone()))
        .collect();
}
fn hierarchical() -> EditorSession {
    let mut m = session("examples/can/baseline.ini");
    let file = m.project.ned_files().next().unwrap();
    let id = file.id.clone();
    let mut text = file.text.to_string();
    text = text
        .replace("a: demo.Controller;", "a: demo.Rack;")
        .replace("b: demo.Controller;", "b: demo.Rack;");
    text.push_str("\nmodule Rack { parameters: int count = default(3); gates: input rx; output tx; submodules: unit: demo.Controller; connections: rx --> unit.rx; unit.tx --> tx; }\n");
    let mut updates = vec![(id, text)];
    updates.extend(
        m.project
            .files
            .values()
            .filter(|f| {
                f.roles
                    .iter()
                    .any(|r| matches!(r, FileRole::Config | FileRole::Workload))
            })
            .map(|f| {
                (
                    f.id.clone(),
                    f.text
                        .replace("Main.a.", "Main.a.unit.")
                        .replace("Main.b.", "Main.b.unit.")
                        .replace("\"Main.a\"", "\"Main.a.unit\"")
                        .replace("\"Main.b\"", "\"Main.b.unit\""),
                )
            }),
    );
    m.commit(updates, vec![]).unwrap();
    refresh(&mut m);
    m
}
fn entry(m: &EditorSession, owner: &str, kind: &str, old: &str, new: &str) -> Value {
    json!({"target_symbol_id":symbol_id(&m.project,owner,kind,old).unwrap(),"expected_old_name":old,"new_local_name":new})
}
fn detach_request(m: &EditorSession, path: &str, destination: Value) -> Value {
    let i = inspect(&m.project).unwrap();
    let ty = &i
        .instances
        .iter()
        .find(|i| i.path == path)
        .unwrap()
        .type_qname;
    json!({"instance_id":instance_id(&m.project,path),"expected_type_id":symbol_id(&m.project,ty,"type",local(ty)).unwrap(),"destination":destination,"clone_names":[],"ancestor_names":[]})
}
#[test]
fn rename_all_kinds_network_and_shared_paths_preserves_values_and_undo() {
    let mut m = hierarchical();
    let config = m
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::Config))
        .unwrap();
    let id = config.id.clone();
    let text = format!(
        "{}\nMain.a.count = 9\n[Channel Main::a.tx]\ndelay = 1us\n",
        config.text
    );
    m.commit(vec![(id, text)], vec![]).unwrap();
    refresh(&mut m);
    let before = m.project.files.clone();
    let request = json!({"entries":[entry(&m,"demo.Main","type","Main","Net"),entry(&m,"demo.Rack","type","Rack","Slot"),entry(&m,"demo.Rack","child","unit","device"),entry(&m,"demo.Rack","gate","tx","outbound"),entry(&m,"demo.Rack","gate","rx","inbound"),entry(&m,"demo.Rack","parameter","count","amount")]});
    let changes = plan_rename(&m, &request).unwrap();
    let mut after = candidate(&m, &changes).unwrap();
    let inspected = inspect(&after.project).unwrap();
    assert!(inspected.instances.iter().any(|i| i.path == "Net.a.device"));
    assert!(
        after
            .project
            .files
            .values()
            .any(|f| f.text.contains("Net.a.amount = 9"))
    );
    assert!(
        after
            .project
            .files
            .values()
            .any(|f| f.text.contains("[Channel Net::a.outbound]"))
    );
    assert!(!changes.symbol_remap.is_empty());
    assert!(
        changes
            .instance_remap
            .iter()
            .any(|r| r["before"] == "Main.a.unit" && r["after"] == "Net.a.device")
    );
    after.undo().unwrap();
    for (id, file) in before {
        assert_eq!(after.project.files[&id].text, file.text);
    }
}
#[test]
fn rename_simultaneous_cycles_and_schema_rejection() {
    let m = hierarchical();
    let request = json!({"entries":[entry(&m,"demo.Main","child","a","b"),entry(&m,"demo.Main","child","b","c"),entry(&m,"demo.Main","child","c","a")]});
    let changes = plan_rename(&m, &request).unwrap();
    assert!(
        changes
            .instance_remap
            .iter()
            .any(|r| r["before"] == "Main.a.unit" && r["after"] == "Main.b.unit")
    );
    assert!(
        plan_rename(
            &m,
            &json!({"entries":[entry(&m,"demo.Controller","gate","tx","transmit")]})
        )
        .is_err()
    );
    assert!(
        plan_rename(
            &m,
            &json!({"entries":[entry(&m,"demo.Main","child","a","b")]})
        )
        .is_err()
    );
    assert!(validate_rename_payload(&json!({"entries":[],"unknown":true})).is_err());
}
#[test]
fn detach_shared_parent_keeps_other_bindings_and_undo_redo() {
    let m = hierarchical();
    let destination = m.project.ned_files().next().unwrap().id.clone();
    let changes = plan_detach(
        &m,
        &detach_request(
            &m,
            "Main.a.unit",
            json!({"kind":"existing_ned","file_id":destination}),
        ),
    )
    .unwrap();
    let mut after = candidate(&m, &changes).unwrap();
    let inspected = inspect(&after.project).unwrap();
    let ty = |path: &str| {
        inspected
            .instances
            .iter()
            .find(|i| i.path == path)
            .unwrap()
            .type_qname
            .clone()
    };
    assert_eq!(ty("Main.b"), "demo.Rack");
    assert_eq!(ty("Main.b.unit"), "demo.Controller");
    assert_ne!(ty("Main.a"), "demo.Rack");
    assert_ne!(ty("Main.a.unit"), "demo.Controller");
    assert_eq!(
        changes.semantic_evidence["bindings"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let text = after.project.files.clone();
    after.undo().unwrap();
    for (id, f) in &m.project.files {
        assert_eq!(after.project.files[id].text, f.text);
    }
    after.redo().unwrap();
    for (id, f) in text {
        assert_eq!(after.project.files[&id].text, f.text);
    }
}
#[test]
fn detach_unique_parent_stops_before_network_and_unused_use_forces_copy() {
    let mut m = hierarchical();
    let f = m.project.ned_files().next().unwrap();
    let id = f.id.clone();
    let text = f.text.replace("b: demo.Rack;", "b: demo.Controller;");
    let mut updates = vec![(id.clone(), text)];
    updates.extend(
        m.project
            .files
            .values()
            .filter(|f| {
                f.roles
                    .iter()
                    .any(|r| matches!(r, FileRole::Config | FileRole::Workload))
            })
            .map(|f| (f.id.clone(), f.text.replace("Main.b.unit", "Main.b"))),
    );
    m.commit(updates, vec![]).unwrap();
    refresh(&mut m);
    let request = detach_request(
        &m,
        "Main.a.unit",
        json!({"kind":"existing_ned","file_id":id}),
    );
    let changes = plan_detach(&m, &request).unwrap();
    assert_eq!(
        changes.semantic_evidence["bindings"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        changes.semantic_evidence["stopping_binding"]["owner_type"],
        "demo.Rack"
    );
    let f = &m.project.files[&id];
    let text = format!(
        "{}\nmodule Unused {{ gates: input rx; output tx; submodules: rack: demo.Rack; connections: rx --> rack.rx; rack.tx --> tx; }}\n",
        f.text
    );
    m.commit(vec![(id.clone(), text)], vec![]).unwrap();
    refresh(&mut m);
    let changes = plan_detach(
        &m,
        &detach_request(
            &m,
            "Main.a.unit",
            json!({"kind":"existing_ned","file_id":id}),
        ),
    )
    .unwrap();
    assert_eq!(
        changes.semantic_evidence["bindings"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        changes.semantic_evidence["stopping_binding"]["owner_type"],
        "demo.Main"
    );
}
#[test]
fn detach_cross_package_new_file_preserves_comments_and_required_ancestor_chain() {
    let mut m = hierarchical();
    let f = m.project.ned_files().next().unwrap();
    let id = f.id.clone();
    let text = f.text.replace(
        "demo.Controller",
        "demo /* 日本語 Controller */ . Controller",
    );
    m.commit(vec![(id, text)], vec![]).unwrap();
    refresh(&mut m);
    let request = detach_request(
        &m,
        "Main.a.unit",
        json!({"kind":"new_ned","root_id":"ned-root-0","relative_path":"deep/copies/Clones.ned","format":{"newline":"CRLF","bom":true}}),
    );
    let changes = plan_detach(&m, &request).unwrap();
    assert_eq!(changes.added.len(), 1);
    assert!(
        changes.added[0]
            .text
            .starts_with("\u{feff}package deep.copies;\r\n")
    );
    assert!(changes.added[0].text.contains("/* 日本語 Controller */"));
    let after = candidate(&m, &changes).unwrap();
    let inspected = inspect(&after.project).unwrap();
    assert!(
        inspected
            .instances
            .iter()
            .any(|i| i.path == "Main.a.unit" && i.type_qname.starts_with("deep.copies."))
    );
    assert!(
        inspected
            .instances
            .iter()
            .any(|i| i.path == "Main.b.unit" && i.type_qname == "demo.Controller")
    );
}
#[test]
fn rename_gateway_json_tokens_and_indices_follow_path_identity() {
    let m = session("examples/gateway/fanout.ini");
    let request = json!({"entries":[entry(&m,"gateway.Main","child","src","zzz"),entry(&m,"gateway.Main","child","sinkC","aaa"),entry(&m,"gateway.Fanout","child","b","c"),entry(&m,"gateway.Fanout","child","c","b")]});
    let changes = plan_rename(&m, &request).unwrap();
    let after = candidate(&m, &changes).unwrap();
    assert!(
        after
            .project
            .files
            .values()
            .any(|f| f.roles.contains(&FileRole::Workload) && f.text.contains("Main.zzz"))
    );
    assert!(
        after
            .project
            .files
            .values()
            .any(|f| f.roles.contains(&FileRole::ModelConfig) && f.text.contains("Main.gw.c"))
    );
    assert_eq!(
        changes.semantic_evidence["contract"],
        "resolved_alpha_equivalence"
    );
}

#[test]
fn rename_bus_order_cycle_keeps_all_bus_settings() {
    let m = session("examples/gateway/fanout.ini");
    let request = json!({"entries":[entry(&m,"gateway.Main","child","busA","busC"),entry(&m,"gateway.Main","child","busC","busA")]});
    let changes = plan_rename(&m, &request).unwrap();
    let after = candidate(&m, &changes).unwrap();
    let prepared = inspect(&after.project).unwrap().prepared;
    assert_eq!(
        prepared
            .can
            .buses
            .iter()
            .find(|b| b.id == "Main.busA")
            .unwrap()
            .bitrate,
        1_000_000
    );
    assert_eq!(
        prepared
            .can
            .buses
            .iter()
            .find(|b| b.id == "Main.busC")
            .unwrap()
            .bitrate,
        500_000
    );
}
#[test]
fn detach_deep_shared_chain_and_inactive_network_remain_unchanged() {
    let mut m = hierarchical();
    let f = m.project.ned_files().next().unwrap();
    let id = f.id.clone();
    let mut text = f
        .text
        .replace("a: demo.Rack;", "a: demo.Outer;")
        .replace("b: demo.Rack;", "b: demo.Outer;");
    text.push_str("\nmodule Outer { gates: input rx; output tx; submodules: rack: demo.Rack; connections: rx --> rack.rx; rack.tx --> tx; }\n");
    let network = m.project.parsed[&id]
        .declarations()
        .iter()
        .find(|d| d.name() == "demo.Main")
        .unwrap();
    text.push_str(
        &f.text[network.span().start_byte..network.span().end_byte].replacen(
            "network Main",
            "network Inactive",
            1,
        ),
    );
    let mut changes = vec![(id.clone(), text)];
    changes.extend(
        m.project
            .files
            .values()
            .filter(|f| {
                f.roles
                    .iter()
                    .any(|r| matches!(r, FileRole::Config | FileRole::Workload))
            })
            .map(|f| {
                (
                    f.id.clone(),
                    f.text
                        .replace("Main.a.unit", "Main.a.rack.unit")
                        .replace("Main.b.unit", "Main.b.rack.unit"),
                )
            }),
    );
    m.commit(changes, vec![]).unwrap();
    refresh(&mut m);
    let result = plan_detach(
        &m,
        &detach_request(
            &m,
            "Main.a.rack.unit",
            json!({"kind":"existing_ned","file_id":id}),
        ),
    )
    .unwrap();
    assert_eq!(
        result.semantic_evidence["bindings"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let after = candidate(&m, &result).unwrap();
    let before = inspect(&m.project).unwrap();
    let after = inspect(&after.project).unwrap();
    let inactive = |i: &ReferenceInspection| {
        i.declarations
            .iter()
            .find(|d| d.qname == "demo.Inactive")
            .unwrap()
            .declaration
            .children()
            .to_vec()
    };
    assert_eq!(inactive(&before), inactive(&after));
}

#[test]
fn rename_layout_cycles_preserve_extra_and_stale_collisions_refuse() {
    use super::super::layout::{LayoutDocument, Position, TypeLayout};
    let mut m = hierarchical();
    let id = m.project.ned_files().next().unwrap().id.clone();
    let mut layout = LayoutDocument::empty(
        m.project.files[&id]
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    let mut ty = TypeLayout::default();
    let mut a = Position::new(1.0, 2.0);
    a.extra.insert("future".into(), json!({"value":3}));
    ty.nodes.insert("a".into(), a.clone());
    ty.nodes.insert("b".into(), Position::new(9.0, 10.0));
    ty.extra.insert("custom".into(), json!("keep"));
    layout.types.insert("demo.Main".into(), ty);
    layout.extra.insert("future_document".into(), json!(true));
    m.project.layouts.get_mut(&id).unwrap().adopted = Some(layout.clone());
    let result=plan_rename(&m,&json!({"entries":[entry(&m,"demo.Main","child","a","b"),entry(&m,"demo.Main","child","b","a")]})).unwrap();
    let after = result.layouts[0].1.as_ref().unwrap();
    assert_eq!(after.types["demo.Main"].nodes["b"], a);
    assert_eq!(after.extra, layout.extra);
    assert_eq!(
        after.types["demo.Main"].extra,
        layout.types["demo.Main"].extra
    );
    let mut stale = layout;
    stale.types.insert("demo.Net".into(), TypeLayout::default());
    m.project.layouts.get_mut(&id).unwrap().adopted = Some(stale);
    assert!(
        plan_rename(
            &m,
            &json!({"entries":[entry(&m,"demo.Main","type","Main","Net")]})
        )
        .is_err()
    );
}
#[test]
fn detach_layout_copy_retains_original_and_destination_extra() {
    use super::super::layout::{LayoutDocument, Position, TypeLayout};
    let mut m = hierarchical();
    let id = m.project.ned_files().next().unwrap().id.clone();
    let filename = m.project.files[&id]
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut layout = LayoutDocument::empty(filename);
    let mut ty = TypeLayout::default();
    let mut p = Position::new(14.0, 20.0);
    p.collapsed = true;
    p.extra.insert("future".into(), json!([1, 2]));
    ty.nodes.insert("unit".into(), p);
    ty.extra.insert("custom".into(), json!("keep"));
    layout.types.insert("demo.Rack".into(), ty.clone());
    layout.extra.insert("future_document".into(), json!(true));
    m.project.layouts.get_mut(&id).unwrap().adopted = Some(layout.clone());
    let result = plan_detach(
        &m,
        &detach_request(
            &m,
            "Main.a.unit",
            json!({"kind":"existing_ned","file_id":id}),
        ),
    )
    .unwrap();
    let after = result.layouts[0].1.as_ref().unwrap();
    assert_eq!(after.types["demo.Rack"], ty);
    assert_eq!(after.types["demo.RackFora"], ty);
    assert_eq!(after.extra, layout.extra);
}
#[test]
fn rename_escaped_json_and_ned_trivia_keep_nonreference_bytes() {
    let mut m = session("examples/gateway/fanout.ini");
    let changes = m
        .project
        .files
        .values()
        .map(|f| {
            let raw = if f
                .roles
                .iter()
                .any(|r| matches!(r, FileRole::Workload | FileRole::ModelConfig))
            {
                f.text.replace("Main.", "Main\\u002e")
            } else if f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })) {
                f.text.replace(
                    "gateway.Controller",
                    "gateway /* Controller unchanged comment */ . Controller",
                )
            } else {
                f.text.to_string()
            };
            (
                f.id.clone(),
                format!("\u{feff}{}", raw.replace('\n', "\r\n")),
            )
        })
        .collect();
    m.commit(changes, vec![]).unwrap();
    refresh(&mut m);
    let result=plan_rename(&m,&json!({"entries":[entry(&m,"gateway.Controller","type","Controller","Device"),entry(&m,"gateway.Main","child","src","sender")]})).unwrap();
    let after = candidate(&m, &result).unwrap();
    assert!(
        after
            .project
            .files
            .values()
            .any(|f| f.text.contains("/* Controller unchanged comment */"))
    );
    assert!(
        after
            .project
            .files
            .values()
            .any(|f| f.text.contains("dir.can.MultibusController"))
    );
    assert!(
        after
            .project
            .files
            .values()
            .filter(|f| f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })))
            .all(|f| f.text.starts_with('\u{feff}'))
    );
    assert!(after.project.files.values().any(|f|f.roles.contains(&FileRole::Workload)&&f.text.contains("\"id\": \"source\"")));
}
#[test]
fn detach_second_root_new_package_and_collision_rejection() {
    let mut m = hierarchical();
    let config = m
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::Config))
        .unwrap();
    let id = config.id.clone();
    let raw = config.text.replace(
        "ned-path = \"models\"",
        "ned-path = \"models\"; \"extra_models\"",
    );
    m.commit(vec![(id, raw)], vec![]).unwrap();
    m.project.refresh_inputs();
    refresh(&mut m);
    let request = detach_request(
        &m,
        "Main.a.unit",
        json!({"kind":"new_ned","root_id":"ned-root-1","relative_path":"copies/New.ned","format":{"newline":"LF","bom":false}}),
    );
    let changes = plan_detach(&m, &request).unwrap();
    assert!(
        changes.added[0]
            .path
            .starts_with(&m.project.header.roots[1])
    );
    let after = candidate(&m, &changes).unwrap();
    assert!(
        inspect(&after.project)
            .unwrap()
            .instances
            .iter()
            .any(|i| i.path == "Main.a.unit" && i.type_qname.starts_with("copies."))
    );
    let dest = m.project.ned_files().next().unwrap().id.clone();
    let mut colliding = detach_request(
        &m,
        "Main.a.unit",
        json!({"kind":"existing_ned","file_id":dest}),
    );
    colliding["clone_names"] = json!([{"source_type_id":symbol_id(&m.project,"demo.Controller","type","Controller").unwrap(),"proposed_local_name":"Rack"}]);
    assert!(plan_detach(&m, &colliding).is_err());
}

#[test]
fn derived_primary_bus_allowance_does_not_hide_other_runtime_changes() {
    let m = session("examples/gateway/fanout.ini");
    let request = json!({"entries":[entry(&m,"gateway.Main","child","busA","busC"),entry(&m,"gateway.Main","child","busC","busA")]});
    let changes = plan_rename(&m, &request).unwrap();
    assert!(
        changes
            .semantic_evidence
            .get("derived_primary_bus_difference")
            .is_some()
    );
    let after_model = candidate(&m, &changes).unwrap();
    let before = inspect(&m.project).unwrap();
    let after = inspect(&after_model.project).unwrap();
    let mut names = Names::default();
    names.symbols.insert(
        Symbol {
            owner: "gateway.Main".into(),
            kind: "child".into(),
            name: "busA".into(),
        },
        "busC".into(),
    );
    names.symbols.insert(
        Symbol {
            owner: "gateway.Main".into(),
            kind: "child".into(),
            name: "busC".into(),
        },
        "busA".into(),
    );
    let mut broken = after.clone();
    broken.prepared.can.buses[1].bitrate += 1;
    assert!(
        semantic::verify_rename(&m.project, &after_model.project, &before, &broken, &names)
            .is_err()
    );
    let mut broken = after;
    broken.prepared.can.controllers[0].rx_channel_ps += 1;
    assert!(
        semantic::verify_rename(&m.project, &after_model.project, &before, &broken, &names)
            .is_err()
    );
}

#[test]
fn rename_type_swap_composes_original_owner_member_namespaces() {
    let m = hierarchical();
    let request = json!({"entries":[entry(&m,"demo.Controller","type","Controller","Rack"),entry(&m,"demo.Rack","type","Rack","Controller"),entry(&m,"demo.Rack","child","unit","outbound"),entry(&m,"demo.Rack","gate","tx","unit"),entry(&m,"demo.Rack","parameter","count","unit")]});
    let changes = plan_rename(&m, &request).unwrap();
    let after = candidate(&m, &changes).unwrap();
    let inspected = inspect(&after.project).unwrap();
    assert!(
        inspected
            .instances
            .iter()
            .any(|i| i.path == "Main.a.outbound" && i.type_qname == "demo.Rack")
    );
    let ty = &inspected
        .declarations
        .iter()
        .find(|d| d.qname == "demo.Controller")
        .unwrap()
        .declaration;
    assert!(ty.gates().contains_key("unit"));
    assert!(ty.parameters().contains_key("unit"));
    assert!(ty.children().iter().any(|(n, _)| n == "outbound"));
}
