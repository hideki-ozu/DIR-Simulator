//! Condition-level acceptance evidence for XC08--XC20. These fixtures exercise the
//! existing editor transaction and resolver, with independent byte/state oracles.
use super::super::input::load_project;
use super::super::layout::{LayoutDocument, Position, TypeLayout};
use super::super::refactor::EditPlan;
use super::*;
use std::fs;
use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn session(name: &str) -> EditorSession {
    EditorSession::new(load_project(Path::new(name), &repository()).unwrap())
}
fn refresh(m: &mut EditorSession) {
    m.project.refresh_inputs();
    m.project.parsed = parse_current_inputs(&m.project).unwrap();
    m.parsed_hashes = m
        .project
        .ned_files()
        .map(|f| (f.id.clone(), f.hash.clone()))
        .collect();
}
fn update(m: &mut EditorSession, updates: Vec<(String, String)>) {
    m.commit(updates, vec![]).unwrap();
    refresh(m);
}
fn entry(m: &EditorSession, owner: &str, kind: &str, old: &str, new: &str) -> Value {
    json!({"target_symbol_id":symbol_id(&m.project,owner,kind,old).unwrap(),"expected_old_name":old,"new_local_name":new})
}
fn detach(m: &EditorSession, path: &str, destination: Value) -> Value {
    let inspection = inspect(&m.project).unwrap();
    let ty = &inspection
        .instances
        .iter()
        .find(|i| i.path == path)
        .unwrap()
        .type_qname;
    json!({"instance_id":instance_id(&m.project,path),"expected_type_id":symbol_id(&m.project,ty,"type",local(ty)).unwrap(),"destination":destination,"clone_names":[],"ancestor_names":[]})
}
fn existing(m: &EditorSession) -> Value {
    json!({"kind":"existing_ned","file_id":m.project.ned_files().next().unwrap().id})
}
/// Every source byte and adopted layout byte; metadata/directories are checked
/// separately because new-file COW also changes the virtual filesystem.
fn bytes(m: &EditorSession) -> BTreeMap<String, Vec<u8>> {
    let mut result: BTreeMap<_, _> = m
        .project
        .files
        .iter()
        .map(|(id, f)| (id.clone(), f.text.as_bytes().to_vec()))
        .collect();
    result.extend(m.project.layouts.iter().filter_map(|(id, c)| {
        c.adopted
            .as_ref()
            .map(|l| (format!("layout:{id}"), l.bytes().unwrap()))
    }));
    result
}
fn state(m: &EditorSession) -> String {
    format!(
        "{:?}",
        (
            bytes(m),
            &m.project.metadata,
            &m.project.directories,
            &m.project.layouts,
            m.revision,
            m.input_revision,
            m.context_epoch,
            m.can_undo(),
            m.can_redo()
        )
    )
}
fn assert_single_history_roundtrip(before: &EditorSession, after: &mut EditorSession) {
    let applied = bytes(after);
    assert_eq!(after.revision, before.revision + 1);
    after.undo().unwrap();
    assert_eq!(bytes(after), bytes(before));
    assert_eq!(after.project.metadata, before.project.metadata);
    // Disk capture caches only scanned NED directories; the first virtual
    // transaction also materializes ancestor listings. Compare both snapshots
    // after the same canonical refresh, including absence of clone directories.
    let mut before_project = before.project.clone();
    before_project.refresh_inputs();
    let mut after_project = after.project.clone();
    after_project.refresh_inputs();
    assert_eq!(after_project.directories, before_project.directories);
    assert_eq!(after.can_undo(), before.can_undo());
    after.redo().unwrap();
    assert_eq!(bytes(after), applied);
}
/// Path-indexed runtime values include complete controller settings, all bus
/// bindings, schedules, generator/route IDs, gateway limits and route endpoints.
/// No parser spans, file paths, or source hashes enter this behavior oracle.
fn runtime(i: &ReferenceInspection) -> Value {
    let p = &i.prepared;
    let controller = |n: usize| p.can.controllers[n].id.clone();
    let endpoints = |indexes: &[usize]| {
        let mut paths = indexes.iter().map(|n| controller(*n)).collect::<Vec<_>>();
        paths.sort();
        paths
    };
    let controllers: BTreeMap<_, _> = p.can.controllers.iter().enumerate().map(|(n,c)| (c.id.clone(),json!({"settings":c,"bus":p.can.buses[p.can.controller_buses[n]].id,"gateway":p.gateway.controller_gateways[n].map(|g|p.gateway.gateways[g].node.clone())}))).collect();
    let buses: BTreeMap<_, _> = p
        .can
        .buses
        .iter()
        .map(|b| (b.id.clone(), b.bitrate))
        .collect();
    let generators: BTreeMap<_, _> = p.can.generators.iter().map(|g|(g.id.clone(),json!({"node":controller(g.source),"frame":g.frame,"schedule":format!("{:?}",g.schedule)}))).collect();
    let gateways: BTreeMap<_, _> = p.gateway.gateways.iter().map(|g| {
        let routes: BTreeMap<_, _> = g.routes.iter().map(|r|(r.id.clone(),json!({"ingress":controller(r.ingress),"egress":endpoints(&r.egress),"format":r.format,"id_min":r.id_min,"id_max":r.id_max}))).collect();
        (g.node.clone(),json!({"ports":endpoints(&g.ports),"routes":routes,"processing":g.processing_delay_ps,"capacity":g.rx_queue_capacity,"hop_limit":g.hop_limit}))
    }).collect();
    json!({"network":p.common.network,"controllers":controllers,"buses":buses,"generators":generators,"gateways":gateways})
}
fn values(i: &ReferenceInspection) -> Value {
    json!(i.prepared.common.provenance.values.iter().map(|v|(v["key"].as_str().unwrap().to_owned(),json!({"type":v["declared_type"],"quantity":v["physical_quantity"],"value":v["normalized_value"],"source":v["adopted_source"]}))).collect::<BTreeMap<_,_>>())
}
fn declaration_shape(d: &crate::input::Declaration) -> Value {
    let attributes = |a: &BTreeMap<String, crate::input::Attribute>| {
        a.iter()
            .map(|(k, v)| (k.clone(), v.value().to_owned()))
            .collect::<BTreeMap<_, _>>()
    };
    let parameters: BTreeMap<_,_> = d.parameters().iter().map(|(n,p)|(n.clone(),json!({"scalar":p.scalar(),"unit":p.unit(),"default":p.default(),"attributes":attributes(p.attributes())}))).collect();
    json!({"name":d.name(),"kind":d.kind(),"implementation":d.implementation(),"attributes":attributes(d.attributes()),"parameters":parameters,"gates":d.gates(),"children":d.children(),"connections":d.connections().iter().map(|c|json!({"start":c.start(),"end":c.end(),"channel":c.channel()})).collect::<Vec<_>>()})
}
fn assert_cow_equivalent(before: &EditorSession, after: &EditorSession, changes: &EditChanges) {
    let a = inspect(&before.project).unwrap();
    let b = inspect(&after.project).unwrap();
    assert_eq!(runtime(&a), runtime(&b));
    assert_eq!(values(&a), values(&b));
    assert_eq!(
        serde_json::to_value(&a.channels).unwrap(),
        serde_json::to_value(&b.channels).unwrap()
    );
    let mut expected: BTreeMap<_, _> = a
        .instances
        .iter()
        .map(|i| (i.path.clone(), i.type_qname.clone()))
        .collect();
    for binding in changes.semantic_evidence["bindings"].as_array().unwrap() {
        let path = binding["instance"].as_str().unwrap();
        assert_eq!(expected[path], binding["before_type"].as_str().unwrap());
        expected.insert(path.into(), binding["after_type"].as_str().unwrap().into());
    }
    assert_eq!(
        expected,
        b.instances
            .iter()
            .map(|i| (i.path.clone(), i.type_qname.clone()))
            .collect()
    );
}
fn hierarchical() -> EditorSession {
    let mut m = session("examples/can/baseline.ini");
    let updates = m.project.files.values().map(|f| {
        let text = if f.roles.iter().any(|r|matches!(r,FileRole::Ned{..})) {
            format!("{}\nmodule Rack {{ parameters: int count = default(3); gates: input rx; output tx; submodules: unit: demo.Controller; connections: rx --> unit.rx; unit.tx --> tx; }}\n",f.text.replace("a: demo.Controller;","a: demo.Rack;").replace("b: demo.Controller;","b: demo.Rack;"))
        } else {
            f.text.replace("Main.a.","Main.a.unit.").replace("Main.b.","Main.b.unit.").replace("\"Main.a\"","\"Main.a.unit\"").replace("\"Main.b\"","\"Main.b.unit\"")
        };
        (f.id.clone(),text)
    }).collect();
    update(&mut m, updates);
    m
}

#[test]
fn xc08_shared_cow_compares_every_path_value_channel_and_runtime_binding() {
    let m = hierarchical();
    let changes = plan_detach(&m, &detach(&m, "Main.a.unit", existing(&m))).unwrap();
    let mut after = candidate(&m, &changes).unwrap();
    assert_cow_equivalent(&m, &after, &changes);
    let i = inspect(&after.project).unwrap();
    assert_eq!(
        i.instances
            .iter()
            .find(|i| i.path == "Main.b")
            .unwrap()
            .type_qname,
        "demo.Rack"
    );
    assert_eq!(
        i.instances
            .iter()
            .find(|i| i.path == "Main.b.unit")
            .unwrap()
            .type_qname,
        "demo.Controller"
    );
    assert_eq!(
        changes.semantic_evidence["bindings"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_single_history_roundtrip(&m, &mut after);
}

struct MultiRootFixture {
    root: PathBuf,
}
impl MultiRootFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(super::super::random_id("ned-xc-cow-").unwrap());
        fs::create_dir_all(root.join("models/demo")).unwrap();
        fs::create_dir_all(root.join("extra_models/demo")).unwrap();
        let original =
            fs::read_to_string(repository().join("examples/can/models/demo/Main.ned")).unwrap();
        let mut main = original
            .replace("a: demo.Controller;", "a: demo.Outer;")
            .replace("b: demo.Controller;", "b: demo.Outer;");
        let network = &original[original.find("network Main").unwrap()..];
        main.push_str(
            &network
                .replacen("network Main", "network Inactive", 1)
                .replace("a: demo.Controller;", "a: demo.Outer;")
                .replace("b: demo.Controller;", "b: demo.Outer;"),
        );
        fs::write(root.join("models/demo/Main.ned"), main).unwrap();
        fs::write(root.join("extra_models/demo/Rack.ned"),"package demo;\nmodule Rack { parameters: int count = default(3); gates: input rx; output tx; submodules: unit: demo.Controller; connections: rx --> demo.Wire --> unit.rx; unit.tx --> tx; }\n").unwrap();
        fs::write(root.join("extra_models/demo/Outer.ned"),"package demo;\nmodule Outer { gates: input rx; output tx; submodules: rack: demo.Rack; connections: rx --> rack.rx; rack.tx --> tx; }\n").unwrap();
        let ini = fs::read_to_string(repository().join("examples/can/baseline.ini"))
            .unwrap()
            .replace("\"models\"", "\"models\"; \"extra_models\"")
            .replace("Main.a.", "Main.a.rack.unit.")
            .replace("Main.b.", "Main.b.rack.unit.");
        fs::write(root.join("project.ini"), ini).unwrap();
        let workload = fs::read_to_string(repository().join("examples/can/baseline.json"))
            .unwrap()
            .replace("\"Main.a\"", "\"Main.a.rack.unit\"")
            .replace("\"Main.b\"", "\"Main.b.rack.unit\"");
        fs::write(root.join("baseline.json"), workload).unwrap();
        Self { root }
    }
    fn load(&self) -> EditorSession {
        EditorSession::new(load_project(Path::new("project.ini"), &self.root).unwrap())
    }
}
impl Drop for MultiRootFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn xc09_xc11_multiroot_ancestors_crosspackage_layout_and_source_identity_roundtrip() {
    let fixture = MultiRootFixture::new();
    let mut m = fixture.load();
    for file in m.project.ned_files().cloned().collect::<Vec<_>>() {
        let mut l = LayoutDocument::empty(
            file.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        for d in m.project.parsed[&file.id].declarations() {
            let mut t = TypeLayout::default();
            t.extra.insert("日本語型extra".into(), json!({"keep":true}));
            for (n, _) in d.children() {
                let mut p = Position::new(14.5, 27.0);
                p.collapsed = true;
                p.extra.insert("future".into(), json!(["配置", 3]));
                t.nodes.insert(n.clone(), p);
            }
            l.types.insert(d.name().into(), t);
        }
        l.extra.insert("document_extra".into(), json!([1, 2]));
        m.project.layouts.get_mut(&file.id).unwrap().adopted = Some(l);
    }
    let request = detach(
        &m,
        "Main.a.rack.unit",
        json!({"kind":"new_ned","root_id":"ned-root-1","relative_path":"copies/Clones.ned","format":{"newline":"CRLF","bom":true}}),
    );
    let changes = plan_detach(&m, &request).unwrap();
    let mut after = candidate(&m, &changes).unwrap();
    assert_eq!(
        changes.semantic_evidence["bindings"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        changes.semantic_evidence["stopping_binding"]["owner_type"],
        "demo.Main"
    );
    assert_cow_equivalent(&m, &after, &changes);
    let added = &changes.added[0];
    assert!(added.path.starts_with(&m.project.header.roots[1]));
    assert!(added.text.starts_with("\u{feff}package copies;\r\n"));
    let destination = after.project.layouts[&added.id].adopted.as_ref().unwrap();
    assert_eq!(destination.source_file, "Clones.ned");
    let before = inspect(&m.project).unwrap();
    let inspected = inspect(&after.project).unwrap();
    for binding in changes.semantic_evidence["bindings"].as_array().unwrap() {
        let old = binding["before_type"].as_str().unwrap();
        let new = binding["after_type"].as_str().unwrap();
        let source = before.declarations.iter().find(|d| d.qname == old).unwrap();
        let source_file = m
            .project
            .file_by_path(Path::new(&source.span.source))
            .unwrap();
        assert_eq!(
            destination.types[new],
            m.project.layouts[&source_file.id]
                .adopted
                .as_ref()
                .unwrap()
                .types[old]
        );
        let copied = inspected
            .declarations
            .iter()
            .find(|d| d.qname == new)
            .unwrap();
        assert_eq!(Path::new(&copied.span.source), added.path);
        assert!(
            copied
                .declaration
                .connections()
                .iter()
                .all(|c| c.channel().is_none_or(|q| q == "demo.Wire"))
        );
    }
    for (id, capture) in &m.project.layouts {
        assert_eq!(after.project.layouts[id].adopted, capture.adopted);
    }
    let inactive = |i: &ReferenceInspection| {
        i.declarations
            .iter()
            .find(|d| d.qname == "demo.Inactive")
            .unwrap()
            .declaration
            .clone()
    };
    assert_eq!(
        declaration_shape(&inactive(&before)),
        declaration_shape(&inactive(&inspected))
    );
    for f in m
        .project
        .ned_files()
        .filter(|f| f.path.starts_with(&m.project.header.roots[1]))
    {
        assert_eq!(
            after.project.files[&f.id].text.as_bytes(),
            f.text.as_bytes()
        );
    }
    assert_single_history_roundtrip(&m, &mut after);
    assert!(
        !after.project.layouts[&added.id]
            .adopted
            .as_ref()
            .unwrap()
            .types
            .is_empty()
    );
}

#[test]
fn xc10_unique_stop_and_unused_incoming_reference_compare_all_bindings() {
    for unused in [false, true] {
        let mut m = hierarchical();
        let id = m.project.ned_files().next().unwrap().id.clone();
        let mut text = m.project.files[&id]
            .text
            .replace("b: demo.Rack;", "b: demo.Controller;");
        if unused {
            text.push_str("\nmodule Unused { gates: input rx; output tx; submodules: rack: demo.Rack; connections: rx --> rack.rx; rack.tx --> tx; }\n");
        }
        let mut updates = vec![(id.clone(), text)];
        updates.extend(
            m.project
                .files
                .values()
                .filter(|f| {
                    f.roles.contains(&FileRole::Config) || f.roles.contains(&FileRole::Workload)
                })
                .map(|f| (f.id.clone(), f.text.replace("Main.b.unit", "Main.b"))),
        );
        update(&mut m, updates);
        let changes = plan_detach(&m, &detach(&m, "Main.a.unit", existing(&m))).unwrap();
        let after = candidate(&m, &changes).unwrap();
        assert_cow_equivalent(&m, &after, &changes);
        assert_eq!(
            changes.semantic_evidence["bindings"]
                .as_array()
                .unwrap()
                .len(),
            if unused { 2 } else { 1 }
        );
        let stopping = &changes.semantic_evidence["stopping_binding"];
        assert_eq!(
            stopping["owner_type"],
            if unused { "demo.Main" } else { "demo.Rack" }
        );
        assert_eq!(stopping["child"], if unused { "a" } else { "unit" });
        let before = inspect(&m.project).unwrap();
        let after = inspect(&after.project).unwrap();
        for d in &before.declarations {
            if d.qname == stopping["owner_type"].as_str().unwrap() {
                continue;
            }
            assert_eq!(
                declaration_shape(&d.declaration),
                declaration_shape(
                    &after
                        .declarations
                        .iter()
                        .find(|a| a.qname == d.qname)
                        .unwrap()
                        .declaration
                ),
                "original {}",
                d.qname
            );
        }
        let owner = after
            .declarations
            .iter()
            .find(|d| d.qname == stopping["owner_type"].as_str().unwrap())
            .unwrap();
        assert!(
            owner
                .declaration
                .children()
                .iter()
                .any(|(n, q)| n == stopping["child"].as_str().unwrap()
                    && q == stopping["after_type"].as_str().unwrap())
        );
    }
}

fn assert_rejected_unchanged(m: &EditorSession, result: Result<EditChanges>, label: &str) {
    let before = state(m);
    let e = match result {
        Ok(_) => panic!("accepted {label}"),
        Err(e) => e,
    };
    assert!(matches!(e.status, 400 | 413 | 422), "{label}: {e:?}");
    assert!(!e.message.is_empty(), "{label}");
    assert_eq!(state(m), before, "{label}");
}
#[test]
fn xc12_detach_invalid_targets_and_limits_are_atomic() {
    let m = hierarchical();
    let original = state(&m);
    let good = detach(&m, "Main.a.unit", existing(&m));
    let mut root = good.clone();
    root["instance_id"] = json!(instance_id(&m.project, "Main"));
    root["expected_type_id"] = json!(symbol_id(&m.project, "demo.Main", "type", "Main").unwrap());
    assert_rejected_unchanged(&m, plan_detach(&m, &root), "network root");
    let mut channel = good.clone();
    channel["instance_id"] = json!(instance_id(&m.project, "Main::a.tx"));
    assert_rejected_unchanged(&m, plan_detach(&m, &channel), "channel not child instance");
    let mut unknown = good.clone();
    unknown["expected_type_id"] = json!("unknown-type");
    assert_rejected_unchanged(&m, plan_detach(&m, &unknown), "unresolved expected type");
    let mut collision = good.clone();
    collision["clone_names"] = json!([{"source_type_id":symbol_id(&m.project,"demo.Controller","type","Controller").unwrap(),"proposed_local_name":"Rack"}]);
    assert_rejected_unchanged(&m, plan_detach(&m, &collision), "existing type name");
    for field in ["clone_names", "ancestor_names"] {
        let mut limit = good.clone();
        limit[field]=json!((0..65).map(|n| if field=="clone_names" {json!({"source_type_id":format!("source{n}"),"proposed_local_name":format!("Copy{n}")})}else{json!({"owner_instance_id":format!("owner{n}"),"proposed_local_name":format!("Copy{n}")})}).collect::<Vec<_>>());
        assert_rejected_unchanged(&m, plan_detach(&m, &limit), field);
    }
    assert_eq!(state(&m), original);
    // An unresolved NED child type is rejected by the same preview path before
    // any clone body or parent patch is published.
    let mut invalid = m.clone();
    let id = invalid.project.ned_files().next().unwrap().id.clone();
    let text = invalid.project.files[&id]
        .text
        .replace("unit: demo.Controller", "unit: demo.Missing");
    update(&mut invalid, vec![(id, text)]);
    let before = state(&invalid);
    assert_rejected_unchanged(
        &invalid,
        plan_detach(&invalid, &good),
        "unresolved child type",
    );
    assert_eq!(state(&invalid), before);
}

/// Apply a complete precomputed path map in one pass, so swaps cannot cascade.
fn map_string(s: &str, paths: &BTreeMap<String, String>) -> String {
    if let Some(value) = paths.get(s) {
        return value.clone();
    }
    // Channel IDs and expanded endpoints carry an instance path prefix.
    for (old, new) in paths.iter().rev() {
        if s.starts_with(old) && matches!(s.as_bytes().get(old.len()), Some(b'.' | b':')) {
            return format!("{new}{}", &s[old.len()..]);
        }
    }
    s.into()
}
fn map_json(value: Value, paths: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(s) => json!(map_string(&s, paths)),
        Value::Array(v) => Value::Array(v.into_iter().map(|v| map_json(v, paths)).collect()),
        Value::Object(v) => Value::Object(
            v.into_iter()
                .map(|(k, v)| {
                    let mut mapped = map_json(v, paths);
                    if matches!(k.as_str(), "ports" | "egress") {
                        mapped
                            .as_array_mut()
                            .unwrap()
                            .sort_by_key(|v| v.as_str().unwrap().to_owned());
                    }
                    (map_string(&k, paths), mapped)
                })
                .collect(),
        ),
        other => other,
    }
}
fn path_remap(changes: &EditChanges) -> BTreeMap<String, String> {
    changes
        .instance_remap
        .iter()
        .map(|r| {
            (
                r["before"].as_str().unwrap().into(),
                r["after"].as_str().unwrap().into(),
            )
        })
        .collect()
}
fn assert_runtime_renamed(before: &EditorSession, after: &EditorSession, changes: &EditChanges) {
    let a = inspect(&before.project).unwrap();
    let b = inspect(&after.project).unwrap();
    let paths = path_remap(changes);
    let mut expected = map_json(runtime(&a), &paths);
    // The runtime network field is a type name rather than an instance path.
    expected["network"] = json!(b.prepared.common.network);
    assert_eq!(expected, runtime(&b));
    assert_eq!(a.instances.len(), b.instances.len());
    let actual: BTreeSet<_> = b.instances.iter().map(|i| i.path.clone()).collect();
    assert_eq!(paths.values().cloned().collect::<BTreeSet<_>>(), actual);
}
fn adopt_layout(m: &mut EditorSession, owner: &str, child: &str) -> (String, LayoutDocument) {
    let id = m
        .project
        .ned_files()
        .find(|f| {
            m.project.parsed[&f.id]
                .declarations()
                .iter()
                .any(|d| d.name() == owner)
        })
        .unwrap()
        .id
        .clone();
    let mut layout = LayoutDocument::empty(
        m.project.files[&id]
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    );
    let mut ty = TypeLayout::default();
    let mut position = Position::new(4.0, 8.0);
    position.collapsed = true;
    position
        .extra
        .insert("future".into(), json!({"文字":"保存"}));
    ty.nodes.insert(child.into(), position);
    ty.extra.insert("type_extra".into(), json!(7));
    layout.types.insert(owner.into(), ty);
    layout.extra.insert("document_extra".into(), json!([1, 2]));
    m.project.layouts.get_mut(&id).unwrap().adopted = Some(layout.clone());
    (id, layout)
}

#[test]
fn xc13_xc15_aggregate_rename_updates_all_roles_channels_layout_and_ids() {
    let mut m = session("examples/gateway/fanout.ini");
    let updates=m.project.files.values().filter_map(|f| {
        if f.path.ends_with("Types.ned") {
            Some((f.id.clone(),f.text.replace("module Fanout {","// Fanout b tx_b count are comment text. 日本語\nmodule Fanout { parameters: int count = default(3);")))
        } else if f.roles.contains(&FileRole::Config) {
            Some((f.id.clone(),format!("{}\nMain.gw.count = 9\n[Channel Main::gw.tx_b]\ndelay = 1us\n",f.text)))
        }else{None}
    }).collect();
    update(&mut m, updates);
    let (layout_id, old_layout) = adopt_layout(&mut m, "gateway.Fanout", "b");
    let request = json!({"entries":[entry(&m,"gateway.Main","type","Main","Net"),entry(&m,"gateway.Fanout","type","Fanout","Hub"),entry(&m,"gateway.Fanout","child","b","portB"),entry(&m,"gateway.Fanout","gate","tx_b","out_b"),entry(&m,"gateway.Fanout","parameter","count","total")]});
    let before = bytes(&m);
    let changes = plan_rename(&m, &request).unwrap();
    assert_eq!(bytes(&m), before, "preview publishes no source/layout");
    let mut after = candidate(&m, &changes).unwrap();
    assert_runtime_renamed(&m, &after, &changes);
    let config = after
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::Config))
        .unwrap();
    assert!(config.text.contains("network = gateway.Net"));
    assert!(config.text.contains("Net.gw.total = 9"));
    assert!(config.text.contains("[Channel Net::gw.out_b]"));
    assert!(config.text.contains("Net.gw.portB.queueCapacity = 8"));
    let workload = after
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::Workload))
        .unwrap();
    assert!(workload.text.contains("\"node\": \"Net.src\""));
    let routing = after
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::ModelConfig))
        .unwrap();
    assert!(routing.text.contains("Net.gw.portB"));
    assert!(routing.text.contains("\"id\": \"fanout\""));
    let new_layout = after.project.layouts[&layout_id].adopted.as_ref().unwrap();
    assert_eq!(
        new_layout.types["gateway.Hub"].nodes["portB"],
        old_layout.types["gateway.Fanout"].nodes["b"]
    );
    assert_eq!(
        new_layout.types["gateway.Hub"].extra,
        old_layout.types["gateway.Fanout"].extra
    );
    assert_eq!(new_layout.extra, old_layout.extra);
    assert_eq!(new_layout.source_file, old_layout.source_file);
    assert!(after.project.ned_files().any(|f| {
        f.text
            .contains("// Fanout b tx_b count are comment text. 日本語")
    }));
    let a = inspect(&m.project).unwrap();
    let b = inspect(&after.project).unwrap();
    let paths = path_remap(&changes);
    let expected_channels: BTreeMap<_, _> = a
        .channels
        .iter()
        .map(|c| {
            let id = map_string(&c.id, &paths).replace("::gw.tx_b", "::gw.out_b");
            let start = map_string(&c.start_endpoint, &paths).replace(".gw.tx_b", ".gw.out_b");
            (
                id,
                json!({"type":c.type_qname,"start":start,"end":map_string(&c.end_endpoint,&paths)}),
            )
        })
        .collect();
    let channels: BTreeMap<_, _> = b
        .channels
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                json!({"type":c.type_qname,"start":c.start_endpoint,"end":c.end_endpoint}),
            )
        })
        .collect();
    assert_eq!(expected_channels, channels);
    let mut expected_values = map_json(values(&a), &paths);
    let obj = expected_values.as_object_mut().unwrap();
    let count = obj.remove("Net.gw.count").unwrap();
    obj.insert("Net.gw.total".into(), count);
    if let Some(channel) = obj.remove("Net::gw.tx_b.delay") {
        obj.insert("Net::gw.out_b.delay".into(), channel);
    }
    obj.get_mut("network").unwrap()["value"] = json!("\"gateway.Net\"");
    assert_eq!(expected_values, values(&b));
    assert_single_history_roundtrip(&m, &mut after);
}

#[test]
fn xc14_swap_three_cycle_and_parent_child_each_have_one_exact_history_entry() {
    for shape in ["swap", "three_cycle", "parent_child"] {
        let m = hierarchical();
        let entries = match shape {
            "swap" => vec![
                entry(&m, "demo.Main", "child", "a", "b"),
                entry(&m, "demo.Main", "child", "b", "a"),
            ],
            "three_cycle" => vec![
                entry(&m, "demo.Main", "child", "a", "b"),
                entry(&m, "demo.Main", "child", "b", "c"),
                entry(&m, "demo.Main", "child", "c", "a"),
            ],
            _ => vec![
                entry(&m, "demo.Main", "child", "a", "slot"),
                entry(&m, "demo.Rack", "child", "unit", "device"),
            ],
        };
        let changes = plan_rename(&m, &json!({"entries":entries})).unwrap();
        let mut after = candidate(&m, &changes).unwrap();
        assert_runtime_renamed(&m, &after, &changes);
        let mapped = path_remap(&changes);
        assert_eq!(
            mapped["Main.a.unit"],
            match shape {
                "swap" | "three_cycle" => "Main.b.unit",
                _ => "Main.slot.device",
            }
        );
        if shape == "swap" {
            assert_eq!(mapped["Main.b.unit"], "Main.a.unit");
        }
        if shape == "three_cycle" {
            assert_eq!(mapped["Main.b.unit"], "Main.c.unit");
            assert_eq!(mapped["Main.c"], "Main.a");
        }
        let expected_symbols: BTreeSet<_> = catalog(&m)
            .unwrap()
            .into_values()
            .map(|mut s| {
                if s.kind == "child" && s.owner == "demo.Main" {
                    s.name = match (shape, s.name.as_str()) {
                        ("swap" | "three_cycle", "a") => "b",
                        ("swap", "b") => "a",
                        ("three_cycle", "b") => "c",
                        ("three_cycle", "c") => "a",
                        ("parent_child", "a") => "slot",
                        _ => &s.name,
                    }
                    .into();
                } else if shape == "parent_child"
                    && s.owner == "demo.Rack"
                    && s.kind == "child"
                    && s.name == "unit"
                {
                    s.name = "device".into();
                }
                s
            })
            .collect();
        let final_symbols: BTreeSet<_> = catalog(&after).unwrap().into_values().collect();
        assert_eq!(
            expected_symbols, final_symbols,
            "no temporary or extra final symbols: {shape}"
        );
        assert_single_history_roundtrip(&m, &mut after);
        let i = inspect(&after.project).unwrap();
        assert_eq!(
            i.declarations.len(),
            inspect(&m.project).unwrap().declarations.len()
        );
    }
}

#[test]
fn xc16_segment_boundaries_other_owner_and_unused_type_references_are_independent() {
    let mut m = hierarchical();
    let updates=m.project.files.values().map(|f| {
        let text=if f.roles.iter().any(|r|matches!(r,FileRole::Ned{..})) {
            format!("{}\nmodule Unused {{ gates: input rx; output tx; submodules: unit: demo.Rack; connections: rx --> unit.rx; unit.tx --> tx; }}\n",f.text.replace("b: demo.Rack;","aa: demo.Rack;").replace("b.tx","aa.tx").replace("b.rx","aa.rx"))
        }else{f.text.replace("Main.b.","Main.aa.").replace("\"Main.b\"","\"Main.aa\"")};
        (f.id.clone(),text)
    }).collect();
    update(&mut m, updates);
    let request = json!({"entries":[entry(&m,"demo.Main","child","a","renamed"),entry(&m,"demo.Rack","type","Rack","Slot"),entry(&m,"demo.Rack","child","unit","device"),entry(&m,"demo.Rack","gate","tx","outbound")]});
    let changes = plan_rename(&m, &request).unwrap();
    let after = candidate(&m, &changes).unwrap();
    assert_runtime_renamed(&m, &after, &changes);
    let paths = path_remap(&changes);
    assert_eq!(paths["Main.a.unit"], "Main.renamed.device");
    assert_eq!(paths["Main.aa.unit"], "Main.aa.device");
    let i = inspect(&after.project).unwrap();
    let unused = &i
        .declarations
        .iter()
        .find(|d| d.qname == "demo.Unused")
        .unwrap()
        .declaration;
    assert_eq!(
        unused.children(),
        &[("unit".to_owned(), "demo.Slot".to_owned())]
    );
    assert!(
        unused
            .connections()
            .iter()
            .any(|c| c.start() == "unit.outbound")
    );
    let controller = &i
        .declarations
        .iter()
        .find(|d| d.qname == "demo.Controller")
        .unwrap()
        .declaration;
    assert!(controller.gates().contains_key("tx"));
    assert!(!controller.gates().contains_key("outbound"));
    let slot = &i
        .declarations
        .iter()
        .find(|d| d.qname == "demo.Slot")
        .unwrap()
        .declaration;
    assert!(slot.children().iter().any(|(n, _)| n == "device"));
    // Unused.unit belongs to another owner and therefore keeps its local name.
    assert!(!unused.children().iter().any(|(n, _)| n == "device"));
}

#[test]
fn xc17_every_unsupported_reference_variant_refuses_without_exact_override_compensation() {
    for variant in [
        "wildcard",
        "recursive_wildcard",
        "duplicate_ini",
        "unresolved",
        "unsupported_schema",
        "bare_alias",
    ] {
        let mut m = session("examples/gateway/fanout.ini");
        let updates = m
            .project
            .files
            .values()
            .filter_map(|f| {
                let text = match variant {
                    "wildcard" if f.roles.contains(&FileRole::Config) => {
                        format!("{}\n*.queueCapacity = 8\n", f.text)
                    }
                    "recursive_wildcard" if f.roles.contains(&FileRole::Config) => {
                        format!("{}\n**.queueCapacity = 8\n", f.text)
                    }
                    "duplicate_ini" if f.roles.contains(&FileRole::Config) => {
                        format!("{}\nMain.gw.b.queueCapacity = 8\n", f.text)
                    }
                    "unresolved" if f.roles.contains(&FileRole::Workload) => {
                        f.text.replace("\"Main.src\"", "\"Main.missing\"")
                    }
                    "unsupported_schema" if f.roles.contains(&FileRole::Workload) => f
                        .text
                        .replace("\"schema_version\": 2", "\"schema_version\": 99"),
                    // Both the active network and an unused module have a member b;
                    // bare aliases are rejected even if one candidate looks obvious.
                    "bare_alias" if f.roles.contains(&FileRole::ModelConfig) => {
                        f.text.replace("\"Main.gw.b\"", "\"b\"")
                    }
                    _ => return None,
                };
                Some((f.id.clone(), text))
            })
            .collect();
        m.commit(updates, vec![]).unwrap();
        m.project.refresh_inputs();
        let before = state(&m);
        let request = json!({"entries":[entry(&m,"gateway.Main","child","src","sender")]});
        let e = match plan_rename(&m, &request) {
            Ok(_) => panic!("accepted {variant}"),
            Err(e) => e,
        };
        assert_eq!(e.status, 422, "{variant}: {e:?}");
        assert!(
            e.diagnostic.is_some(),
            "resolver reason required: {variant}: {e:?}"
        );
        let diagnostic = e.diagnostic.as_ref().unwrap();
        assert!(!diagnostic.reason.is_empty(), "{variant}");
        assert!(!diagnostic.message.is_empty(), "{variant}");
        assert_eq!(
            state(&m),
            before,
            "no override insertion or textual repair for {variant}"
        );
    }
}

#[test]
fn xc18_equal_value_collisions_in_each_namespace_are_preview_rejections() {
    for namespace in [
        "symbol",
        "ini_key",
        "channel_id",
        "layout_node",
        "layout_type",
    ] {
        let mut m = hierarchical();
        if namespace == "ini_key" || namespace == "channel_id" {
            let f = m
                .project
                .files
                .values()
                .find(|f| f.roles.contains(&FileRole::Config))
                .unwrap();
            let suffix = if namespace == "ini_key" {
                "\nMain.a.unit.queueCapacity = 64\n"
            } else {
                "\n[Channel Main::a.tx]\ndelay = 1us\n[Channel Main::a.tx]\ndelay = 1us\n"
            };
            let updates = vec![(f.id.clone(), format!("{}{suffix}", f.text))];
            m.commit(updates, vec![]).unwrap();
            m.project.refresh_inputs();
        }
        if namespace.starts_with("layout") {
            let (id, mut layout) = adopt_layout(&mut m, "demo.Main", "a");
            if namespace == "layout_node" {
                let position = layout.types["demo.Main"].nodes["a"].clone();
                layout
                    .types
                    .get_mut("demo.Main")
                    .unwrap()
                    .nodes
                    .insert("renamed".into(), position);
            } else {
                layout
                    .types
                    .insert("demo.Net".into(), layout.types["demo.Main"].clone());
            }
            m.project.layouts.get_mut(&id).unwrap().adopted = Some(layout);
        }
        let before = state(&m);
        let entries = match namespace {
            "symbol" => vec![entry(&m, "demo.Main", "child", "a", "b")],
            "layout_type" => vec![entry(&m, "demo.Main", "type", "Main", "Net")],
            _ => vec![entry(&m, "demo.Main", "child", "a", "renamed")],
        };
        assert_rejected_unchanged(&m, plan_rename(&m, &json!({"entries":entries})), namespace);
        assert_eq!(
            state(&m),
            before,
            "equal values never authorize merge: {namespace}"
        );
    }
}

#[test]
fn xc19_bom_mixed_newlines_unicode_and_json_escape_keep_every_nonpatch_byte() {
    let mut m = session("examples/gateway/fanout.ini");
    let updates = m
        .project
        .files
        .values()
        .map(|f| {
            let mut text = f.text.to_string();
            if f.roles.contains(&FileRole::Workload) {
                text = text.replace("\"Main.src\"", "\"Main\\u002esrc\"");
            }
            if f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })) {
                text.push_str("\n// 日本語とéと絵文字🛰 src Main.src は非参照\n");
            }
            if f.roles.contains(&FileRole::Config) {
                text.push_str("\n# 日本語 src Main.src は非参照\nMain.src.queueCapacity = 17\n");
            }
            let mut mixed = String::from("\u{feff}");
            for (line, text) in text.split_inclusive('\n').enumerate() {
                mixed.push_str(text.trim_end_matches('\n'));
                mixed.push_str(if line % 2 == 0 { "\r\n" } else { "\n" });
            }
            (f.id.clone(), mixed)
        })
        .collect();
    update(&mut m, updates);
    let before = bytes(&m);
    let request = json!({"entries":[entry(&m,"gateway.Main","child","src","sender")]});
    let changes = plan_rename(&m, &request).unwrap();
    let mut after = candidate(&m, &changes).unwrap();
    assert_runtime_renamed(&m, &after, &changes);
    for (id, f) in &m.project.files {
        let expected = if f.roles.contains(&FileRole::Workload) {
            f.text.replace("\"Main\\u002esrc\"", "\"Main.sender\"")
        } else if f.roles.contains(&FileRole::Config) {
            f.text
                .replace("Main.src.queueCapacity", "Main.sender.queueCapacity")
        } else if f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })) {
            f.text
                .replace("src: gateway.Controller;", "sender: gateway.Controller;")
                .replace("  src.tx -->", "  sender.tx -->")
                .replace(" --> src.rx;", " --> sender.rx;")
        } else {
            f.text.to_string()
        };
        assert_eq!(
            after.project.files[id].text.as_bytes(),
            expected.as_bytes(),
            "exact bytes outside the explicit tokens: {}",
            f.path.display()
        );
        assert!(after.project.files[id].text.starts_with('\u{feff}'));
        assert!(after.project.files[id].text.contains("\r\n"));
        assert!(
            after.project.files[id]
                .text
                .replace("\r\n", "")
                .contains('\n')
        );
    }
    // Every original reference span remains a UTF-8 boundary in the mixed input.
    for u in inspect(&m.project).unwrap().uses {
        let f = m.project.file_by_path(Path::new(&u.span.source)).unwrap();
        assert!(f.text.is_char_boundary(u.span.start_byte));
        assert!(f.text.is_char_boundary(u.span.end_byte));
    }
    assert_eq!(bytes(&m), before);
    assert_single_history_roundtrip(&m, &mut after);
}

#[test]
fn xc20_edit_undo_and_reload_context_each_invalidate_a_rename_plan() {
    for event in ["edit", "undo", "reload"] {
        let mut m = hierarchical();
        // Ensure Undo has a real preceding editor command to reverse.
        let config = m
            .project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::Config))
            .unwrap();
        let (id, text) = (
            config.id.clone(),
            format!("{}\n# preceding edit\n", config.text),
        );
        update(&mut m, vec![(id.clone(), text)]);
        let request = json!({"operation":"rename_symbols","operation_payload":{"entries":[entry(&m,"demo.Main","child","a","renamed")]}});
        let plan = EditPlan::build(&m, &request).unwrap();
        assert!(plan.matches(&m));
        match event {
            "edit" => {
                let text = format!("{}\n# edit after preview\n", m.project.files[&id].text);
                update(&mut m, vec![(id, text)]);
            }
            "undo" => {
                assert!(m.undo().unwrap());
                refresh(&mut m);
            }
            _ => {
                // The model boundary receives a new project snapshot on Reload;
                // all source bytes may be identical while the snapshot ID changes.
                let text = bytes(&m);
                m.project.id = super::super::random_id("snapshot-reloaded-").unwrap();
                m.context_epoch += 1;
                assert_eq!(bytes(&m), text);
            }
        }
        let before = state(&m);
        assert!(!plan.matches(&m), "{event}");
        let e = match plan.apply(&m) {
            Ok(_) => panic!("stale {event} plan applied"),
            Err(e) => e,
        };
        assert_eq!(e.status, 409, "{event}: {e:?}");
        assert_eq!(e.code, "E-EDITOR-REVISION");
        assert_eq!(state(&m), before);
        let fresh = json!({"operation":"rename_symbols","operation_payload":{"entries":[entry(&m,"demo.Main","child","a","renamed")]}});
        let fresh = EditPlan::build(&m, &fresh).unwrap();
        assert!(
            fresh.apply(&m).is_ok(),
            "new preview succeeds after {event}"
        );
    }
}
