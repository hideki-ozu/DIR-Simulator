use super::super::input::{LayoutCapture, ProjectSnapshot};
use super::super::{random_id, template};
use super::*;
use std::path::Path;

fn project(profile: &str) -> ProjectSnapshot {
    let cwd = std::env::temp_dir().join(random_id("composition-test-").unwrap());
    let mut project = template::new_project(profile, "Composition", &cwd).unwrap();
    let id = project.ned_files().next().unwrap().id.clone();
    // Keep the valid starter, but exercise source preservation and editable ordinary modules.
    let raw = format!(
        "\u{feff}// original source comment\r\n{}\r\nmodule Assembly {{\r\n parameters:\r\n  int reservedName = default(1);\r\n}}\r\nmodule Leaf {{\r\n gates:\r\n  input /* retained gate comment */ inbound;\r\n  output outbound; // retained trailing comment\r\n}}\r\n",
        project.files[&id]
            .text
            .replace("\r\n", "\n")
            .replace('\n', "\r\n")
    );
    replace_source(&mut project, &id, raw);
    project
}

fn replace_source(project: &mut ProjectSnapshot, id: &str, raw: String) {
    let file = project.files.get_mut(id).unwrap();
    file.hash = hash(raw.as_bytes());
    file.origin_hash = file.hash.clone();
    file.text = raw.into();
    file.origin_text = file.text.clone();
    let parsed = crate::input::parse_ned(&file.text, &file.path, package(file)).unwrap();
    project.parsed.insert(id.into(), parsed);
    project.refresh_inputs();
}

fn key(model: &EditorSession, short: &str) -> String {
    let qualified = format!("Composition.{short}");
    for file in model.project.ned_files() {
        for (ordinal, shape) in shapes(model.current_parse(&file.id).unwrap())
            .iter()
            .enumerate()
        {
            if shape.name == qualified {
                return analysis::type_key(&file.id, ordinal, &shape.name);
            }
        }
    }
    panic!("Missing {qualified}");
}

fn structure(model: &EditorSession, short: &str) -> DeclarationShape {
    model.declaration(&key(model, short)).unwrap().2
}

fn contents(model: &EditorSession) -> Value {
    json!({
        "files":model.project.files.iter().map(|(id,f)| (id.clone(), f.text.to_string())).collect::<BTreeMap<_,_>>(),
        "layouts":model.project.layouts.iter().map(|(id,l)| (id.clone(), l.adopted.clone())).collect::<BTreeMap<_,_>>(),
    })
}

fn state(model: &EditorSession) -> Value {
    json!({
        "contents":contents(model), "snapshot":model.project.id, "revision":model.revision,
        "input_revision":model.input_revision,"context_epoch":model.context_epoch,
        "syntax":model.syntax,"parsed_hashes":model.parsed_hashes,
        "parsed":model.project.parsed.iter().map(|(id,p)| (id.clone(), shapes(p))).collect::<BTreeMap<_,_>>(),
        "diagnostics":model.diagnostics,"analysis":model.analysis,
        "undo":model.can_undo(),"redo":model.can_redo(),"dirty":model.dirty(),
        "digest":model.project.input_digest(),"last_output":model.last_output,
    })
}

fn rejects_unchanged(model: &mut EditorSession, kind: &str, payload: Value) -> EditorError {
    let before = state(model);
    let error = model.graph_command(kind, &payload).unwrap_err();
    assert_eq!(
        state(model),
        before,
        "failed {kind} changed model: {}",
        error.message
    );
    error
}

fn reparse(model: &mut EditorSession) {
    model.adopt_analysis(analysis::parse_project(&model.project), model.context_epoch);
}

fn add_external(project: &mut ProjectSnapshot, filename: &str, source: &str) -> String {
    let original = project.ned_files().next().unwrap().clone();
    let path = original.path.with_file_name(filename);
    let id = format!("f{}", &hash(path.to_string_lossy().as_bytes())[..24]);
    let text = std::sync::Arc::<str>::from(format!("package Composition;\n{source}"));
    let digest = hash(text.as_bytes());
    let file = CapturedFile {
        id: id.clone(),
        path: path.clone(),
        roles: vec![FileRole::Ned {
            root_index: 0,
            relative: Path::new("Composition").join(filename),
            package: "Composition".into(),
        }],
        text: text.clone(),
        hash: digest.clone(),
        origin_text: text,
        origin_hash: digest,
        stat: None,
    };
    let parsed = crate::input::parse_ned(&file.text, &path, "Composition").unwrap();
    project.files.insert(id.clone(), file);
    project.parsed.insert(id.clone(), parsed);
    project.layouts.insert(
        id.clone(),
        LayoutCapture {
            path: path.with_file_name(format!("{filename}.layout.json")),
            raw: None,
            hash: None,
            stat: None,
            adopted: None,
            warning: None,
        },
    );
    project.refresh_inputs();
    id
}

#[test]
fn create_module_three_ports_and_inside_connections_are_atomic_drafts() {
    let mut model = EditorSession::new(project("multibus"));
    let original = contents(&model);
    let original_assembly = structure(&model, "Assembly");
    let parent = key(&model, "Main");
    let revision = model.revision;
    assert!(model.graph_command("create_module", &json!({
        "parent_type":parent,"type_name":"Dock","child_name":"dock","position":{"x":10,"y":20},
    })).unwrap());
    assert_eq!(model.revision, revision + 1);
    assert_eq!(structure(&model, "Assembly"), original_assembly);
    assert!(
        structure(&model, "Main")
            .children
            .contains(&("dock".into(), "Composition.Dock".into()))
    );
    let created = contents(&model);
    model.undo().unwrap();
    assert_eq!(contents(&model), original);
    model.redo().unwrap();
    assert_eq!(contents(&model), created);
    reparse(&mut model);
    let dock = key(&model, "Dock");
    for port in 0..3 {
        let before = contents(&model);
        let before_revision = model.revision;
        model
            .graph_command(
                "add_port",
                &json!({
                    "type_key":dock,"child_name":format!("p{port}"),
                    "input_gate":format!("from_{port}"),"output_gate":format!("to_{port}"),
                    "position":{"x":30+port*60,"y":70},
                }),
            )
            .unwrap();
        assert_eq!(model.revision, before_revision + 1);
        let shape = structure(&model, "Dock");
        assert_eq!(shape.children.len(), port as usize + 1);
        assert_eq!(shape.gates.len(), 2 * (port as usize + 1));
        assert_eq!(shape.connections.len(), 2 * (port as usize + 1));
        let ctrl = model.unique_type(&shape.children[port as usize].1).unwrap();
        assert_eq!(
            ctrl.implementation.as_deref(),
            Some("dir.can.MultibusController")
        );
        assert!(shape.connections.contains(&analysis::ConnectionShape {
            start: format!("from_{port}"),
            end: format!("p{port}.rx"),
            channel: None,
        }));
        assert!(shape.connections.contains(&analysis::ConnectionShape {
            start: format!("p{port}.tx"),
            end: format!("to_{port}"),
            channel: None,
        }));
        let after = contents(&model);
        model.undo().unwrap();
        assert_eq!(contents(&model), before);
        model.redo().unwrap();
        assert_eq!(contents(&model), after);
        reparse(&mut model);
    }
    let (id, _, _) = model.declaration(&dock).unwrap();
    let layout = model.project.layouts[&id].adopted.as_ref().unwrap();
    assert_eq!(layout.types["Composition.Dock"].nodes.len(), 3);
    assert_eq!(layout.types["Composition.Main"].nodes["dock"].x, 10.0);
    assert!(
        model.project.files[&id]
            .text
            .starts_with("\u{feff}// original source comment\r\n")
    );
    assert!(
        model.project.files[&id]
            .text
            .contains("/* retained gate comment */")
    );
    assert_eq!(model.analysis, "unchecked");
    assert!(model.prepared.is_none());
    model
        .graph_command(
            "add_gates",
            &json!({"type_key":dock,"gates":[
                {"name":"spare_in","output":false},{"name":"spare_out","output":true},
            ]}),
        )
        .unwrap();
    model
        .graph_command(
            "add_child",
            &json!({
                "parent_type":dock,"type_name":"@builtin:Controller","child_name":"spare",
            }),
        )
        .unwrap();
    for (from, to) in [("spare_in", "spare.rx"), ("spare.tx", "spare_out")] {
        model
            .graph_command("connect", &json!({"parent_type":dock,"from":from,"to":to}))
            .unwrap();
    }
    let error = rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":dock,"gate_name":"spare_in"}),
    );
    assert!(error.message.contains("Composition.Dock"));
    assert!(error.message.contains("spare_in --> spare.rx"));
    assert!(!model.project.config.parent().unwrap().exists());
}

#[test]
fn port_controller_adapts_to_can_without_changing_profile_or_settings() {
    let mut model = EditorSession::new(project("can"));
    let assembly = key(&model, "Assembly");
    let config = model
        .project
        .file_by_path(&model.project.config)
        .unwrap()
        .text
        .clone();
    model
        .graph_command(
            "add_port",
            &json!({"type_key":assembly,"child_name":"p0",
                "input_gate":"incoming","output_gate":"outgoing","position":{"x":0,"y":0},
            }),
        )
        .unwrap();
    let controller = model
        .unique_type(&structure(&model, "Assembly").children[0].1)
        .unwrap();
    assert_eq!(
        controller.implementation.as_deref(),
        Some("dir.can.Controller")
    );
    assert_eq!(
        model.project.header.profile.as_deref(),
        Some("can.cc.ideal.v1")
    );
    assert_eq!(
        model
            .project
            .file_by_path(&model.project.config)
            .unwrap()
            .text,
        config
    );
}

#[test]
fn bus_gate_batches_and_deletions_enforce_pair_count_and_atomic_history() {
    let mut model = EditorSession::new(project("multibus"));
    let bus = key(&model, "MultibusBus");
    for gates in [
        json!([{"name":"rx_c","output":false}]),
        json!([{"name":"tx_c","output":true},{"name":"tx_d","output":true}]),
    ] {
        rejects_unchanged(
            &mut model,
            "add_gates",
            json!({"type_key":bus,"gates":gates}),
        );
    }
    let before = contents(&model);
    model
        .graph_command(
            "add_gates",
            &json!({"type_key":bus,"gates":[
                {"name":"rx_c","output":false},{"name":"tx_c","output":true},
            ]}),
        )
        .unwrap();
    let added = contents(&model);
    assert_eq!(structure(&model, "MultibusBus").gates.len(), 6);
    model.undo().unwrap();
    assert_eq!(contents(&model), before);
    model.redo().unwrap();
    assert_eq!(contents(&model), added);
    reparse(&mut model);
    rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":bus,"gate_name":"rx_c"}),
    );
    rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":bus,"gate_name":"rx_c","paired_gate":"rx_a"}),
    );
    model
        .graph_command(
            "delete_gate",
            &json!({"type_key":bus,"gate_name":"rx_c","paired_gate":"tx_c"}),
        )
        .unwrap();
    assert_eq!(structure(&model, "MultibusBus").gates.len(), 4);
    let deleted = contents(&model);
    model.undo().unwrap();
    assert_eq!(contents(&model), added);
    model.redo().unwrap();
    assert_eq!(contents(&model), deleted);
    reparse(&mut model);
    let error = rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":bus,"gate_name":"rx_a","paired_gate":"tx_a"}),
    );
    assert!(error.message.contains("at least two pairs"));
    let controller = key(&model, "Controller");
    rejects_unchanged(
        &mut model,
        "add_gates",
        json!({"type_key":controller,"gates":[{"name":"third","output":true}]}),
    );
    rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":controller,"gate_name":"tx"}),
    );
    let root = key(&model, "Main");
    rejects_unchanged(
        &mut model,
        "add_gates",
        json!({"type_key":root,"gates":[{"name":"third","output":true}]}),
    );
}

#[test]
fn compound_single_and_pair_deletion_preserve_comments_and_other_source_bytes() {
    let mut model = EditorSession::new(project("multibus"));
    let leaf = key(&model, "Leaf");
    let (id, _, _) = model.declaration(&leaf).unwrap();
    let original = contents(&model);
    let prefix = model.project.files[&id]
        .text
        .split("module Leaf")
        .next()
        .unwrap()
        .to_owned();
    model
        .graph_command(
            "delete_gate",
            &json!({"type_key":leaf,"gate_name":"inbound"}),
        )
        .unwrap();
    assert_eq!(
        structure(&model, "Leaf").gates,
        BTreeMap::from([("outbound".into(), true)])
    );
    assert!(model.project.files[&id].text.starts_with(&prefix));
    assert!(
        model.project.files[&id]
            .text
            .contains("/* retained gate comment */")
    );
    assert!(
        model.project.files[&id]
            .text
            .contains("// retained trailing comment")
    );
    model.undo().unwrap();
    assert_eq!(contents(&model), original);
    reparse(&mut model);
    model
        .graph_command(
            "delete_gate",
            &json!({"type_key":leaf,"gate_name":"inbound","paired_gate":"outbound"}),
        )
        .unwrap();
    assert!(structure(&model, "Leaf").gates.is_empty());
    assert!(
        model.project.files[&id]
            .text
            .contains("/* retained gate comment */")
    );
    assert!(
        model.project.files[&id]
            .text
            .contains("// retained trailing comment")
    );
}

#[test]
fn all_parent_files_and_instances_are_checked_before_gate_deletion() {
    let mut project = project("multibus");
    add_external(
        &mut project,
        "Users.ned",
        "module First {\n submodules:\n  left: Composition.Leaf;\n  right: Composition.Leaf;\n connections:\n  left.outbound --> right.inbound;\n}\nmodule Second {\n submodules:\n  a: Composition.Leaf;\n  b: Composition.Leaf;\n connections:\n  a.outbound --> b.inbound;\n}\n",
    );
    add_external(
        &mut project,
        "BusUsers.ned",
        "module BusUsers {\n submodules:\n  bus: Composition.MultibusBus;\n  ctrl: Composition.Controller;\n connections:\n  ctrl.tx --> bus.rx_a;\n  bus.tx_a --> ctrl.rx;\n}\nmodule Taken {}\n",
    );
    let mut model = EditorSession::new(project);
    let bus = key(&model, "MultibusBus");
    let used = rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({
            "type_key":bus,"gate_name":"rx_a","paired_gate":"tx_a",
        }),
    );
    assert!(used.message.contains("BusUsers.ned"));
    assert!(used.message.contains("ctrl.tx --> bus.rx_a"));
    assert!(used.message.contains("bus.tx_a --> ctrl.rx"));
    model
        .graph_command(
            "add_gates",
            &json!({"type_key":bus,"gates":[
                {"name":"rx_c","output":false},{"name":"tx_c","output":true},
            ]}),
        )
        .unwrap();
    model
        .graph_command(
            "delete_gate",
            &json!({
                "type_key":bus,"gate_name":"rx_c","paired_gate":"tx_c",
            }),
        )
        .unwrap();
    let root = key(&model, "Main");
    rejects_unchanged(
        &mut model,
        "create_module",
        json!({
            "parent_type":root,"type_name":"Taken","child_name":"taken","position":{"x":0,"y":0},
        }),
    );
    let leaf = key(&model, "Leaf");
    for gate in ["inbound", "outbound"] {
        let error = rejects_unchanged(
            &mut model,
            "delete_gate",
            json!({"type_key":leaf,"gate_name":gate}),
        );
        for required in [
            "Users.ned",
            "Composition.First",
            "Composition.Second",
            "left.outbound --> right.inbound",
            "a.outbound --> b.inbound",
            "::connection::0",
        ] {
            assert!(
                error.message.contains(required),
                "missing {required}: {}",
                error.message
            );
        }
    }
    // An unused gate on a shared definition is still editable by the model;
    // the controller owns shared-definition confirmation.
    model
        .graph_command(
            "add_gates",
            &json!({"type_key":leaf,"gates":[{"name":"spare","output":false}]}),
        )
        .unwrap();
    model
        .graph_command("delete_gate", &json!({"type_key":leaf,"gate_name":"spare"}))
        .unwrap();
}

#[test]
fn default_package_is_rejected_by_the_common_grammar() {
    assert!(
        crate::input::parse_ned(
            "module Assembly {}\nnetwork Main {}\n",
            Path::new("Main.ned"),
            ""
        )
        .is_err()
    );
}

#[test]
fn stale_and_unparseable_dependencies_prevent_proving_a_gate_unused() {
    let mut project = project("multibus");
    let dependency = add_external(&mut project, "Dependency.ned", "module Other {}\n");
    let mut model = EditorSession::new(project);
    let leaf = key(&model, "Leaf");
    model
        .commit(
            vec![(
                dependency.clone(),
                "package Composition; module Broken {".into(),
            )],
            vec![],
        )
        .unwrap();
    let error = rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":leaf,"gate_name":"inbound"}),
    );
    assert!(error.message.contains("Dependency.ned"));
    reparse(&mut model);
    assert_eq!(model.syntax[&dependency], "syntax_error");
    rejects_unchanged(
        &mut model,
        "delete_gate",
        json!({"type_key":leaf,"gate_name":"inbound"}),
    );
    model.undo().unwrap();
    reparse(&mut model);
    model
        .graph_command(
            "delete_gate",
            &json!({"type_key":leaf,"gate_name":"inbound"}),
        )
        .unwrap();
}

#[test]
fn bad_names_collisions_payloads_and_late_failures_leave_the_entire_model_unchanged() {
    let mut model = EditorSession::new(project("multibus"));
    let root = key(&model, "Main");
    let assembly = key(&model, "Assembly");
    for invalid in [
        "",
        "1bad",
        "x.y",
        "x; module Injected {}",
        "default",
        "true",
        "moduleinterface",
        "input",
    ] {
        rejects_unchanged(
            &mut model,
            "create_module",
            json!({"parent_type":root,
            "type_name":invalid,"child_name":"newChild","position":{"x":0,"y":0}}),
        );
        rejects_unchanged(
            &mut model,
            "create_module",
            json!({"parent_type":root,
            "type_name":"NewType","child_name":invalid,"position":{"x":0,"y":0}}),
        );
        rejects_unchanged(
            &mut model,
            "add_gates",
            json!({"type_key":assembly,"gates":[{"name":invalid,"output":false}]}),
        );
        rejects_unchanged(
            &mut model,
            "add_port",
            json!({"type_key":assembly,"child_name":"p0",
            "input_gate":invalid,"output_gate":"outbound","position":{"x":0,"y":0}}),
        );
    }
    rejects_unchanged(
        &mut model,
        "create_module",
        json!({"parent_type":root,"type_name":"Leaf","child_name":"newChild","position":{"x":0,"y":0}}),
    );
    rejects_unchanged(
        &mut model,
        "create_module",
        json!({"parent_type":root,"type_name":"NewType","child_name":"newChild","position":{"x":1000001,"y":0}}),
    );
    for gates in [
        json!([]),
        json!([{"name":"reservedName","output":false}]),
        json!([{"name":"duplicate","output":false},{"name":"duplicate","output":true}]),
        json!([{"name":"valid","output":"false"}]),
    ] {
        rejects_unchanged(
            &mut model,
            "add_gates",
            json!({"type_key":assembly,"gates":gates}),
        );
    }
    for (child, input, output) in [
        ("p0", "same", "same"),
        ("p0", "p0", "outbound"),
        ("p0", "incoming", "reservedName"),
    ] {
        rejects_unchanged(
            &mut model,
            "add_port",
            json!({"type_key":assembly,
            "child_name":child,"input_gate":input,"output_gate":output,"position":{"x":0,"y":0}}),
        );
    }
    // Gate insertion succeeds in the clone, then builtin child creation fails
    // because the file has no adopted layout slot. Nothing reaches the original.
    let (id, _, _) = model.declaration(&assembly).unwrap();
    model.project.layouts.remove(&id);
    rejects_unchanged(
        &mut model,
        "add_port",
        json!({"type_key":assembly,
        "child_name":"p0","input_gate":"incoming","output_gate":"outgoing","position":{"x":0,"y":0}}),
    );
}
