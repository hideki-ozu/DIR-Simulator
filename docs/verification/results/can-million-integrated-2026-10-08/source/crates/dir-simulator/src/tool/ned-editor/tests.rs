use super::input::load_project;
use super::model::EditorSession;
use super::*;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(random_id("dir-editor-test-").unwrap());
        fs::create_dir_all(root.join("models/demo")).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (from, to) in [
            ("examples/can/baseline.ini", "project.ini"),
            ("examples/can/baseline.json", "baseline.json"),
            ("examples/can/models/demo/Main.ned", "models/demo/Main.ned"),
        ] {
            fs::copy(repo.join(from), root.join(to)).unwrap();
        }
        Self { root }
    }
    fn load(&self) -> input::ProjectSnapshot {
        load_project(&self.root.join("project.ini"), &self.root).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn network(m: &EditorSession) -> String {
    for file in m.project.ned_files() {
        let ds = analysis::shapes(m.current_parse(&file.id).unwrap());
        if let Some(i) = ds.iter().position(|d| d.kind == "network") {
            return analysis::type_key(&file.id, i, &ds[i].name);
        }
    }
    panic!("Fixture network is missing")
}
fn sync_parse(m: &mut EditorSession) {
    m.adopt_analysis(analysis::parse_project(&m.project), m.context_epoch);
}

#[test]
fn builtin_gateway_migrates_atomically_and_undo_restores_entire_baseline() {
    let f = Fixture::new();
    let original = f.load();
    let mut m = EditorSession::new(original.clone());
    let key = network(&m);
    let original_digest = original.input_digest();
    assert!(m.graph_command("add_child", &json!({"parent_type":key,"child_name":"gw","type_name":"@builtin:Gateway","position":{"x":10,"y":20}})).unwrap());
    assert_eq!(
        m.project.header.profile.as_deref(),
        Some("can.cc.multibus.v1")
    );
    assert!(m.project.header.model_config.is_some());
    assert_eq!(m.project.files.len(), original.files.len() + 1);
    assert!(
        m.project
            .ned_files()
            .all(|f| !f.text.contains("\"dir.can.Controller\"")
                && !f.text.contains("\"dir.can.Bus\""))
    );
    let migrated = m.project.input_digest();
    assert!(m.undo().unwrap());
    sync_parse(&mut m);
    assert_eq!(m.project.input_digest(), original_digest);
    assert!(m.project.header.model_config.is_none());
    assert!(!m.dirty());
    assert_eq!(
        analysis::prepare(&m.project).unwrap().common.profile,
        "can.cc.ideal.v1"
    );
    assert!(m.redo().unwrap());
    sync_parse(&mut m);
    assert_eq!(m.project.input_digest(), migrated);
    let key = network(&m);
    m.graph_command(
        "delete_child",
        &json!({"element_key":analysis::node_key(&key,"gw")}),
    )
    .unwrap();
    let ready = analysis::prepare(&m.project).unwrap();
    assert_eq!(ready.common.profile, "can.cc.multibus.v1");
    assert_eq!(ready.can.controllers.len(), 3);
}

#[test]
fn builtin_collision_and_invalid_payload_preserve_user_definitions() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let id = m.project.ned_files().next().unwrap().id.clone();
    let before = m.project.files[&id].text.to_string();
    let key = network(&m);
    m.graph_command(
        "add_child",
        &json!({"parent_type":key,"child_name":"extra","type_name":"@builtin:Controller"}),
    )
    .unwrap();
    assert!(
        m.project.files[&id]
            .text
            .starts_with(&before[..before.find("network Main").unwrap()])
    );
    let child = m
        .declaration(&key)
        .unwrap()
        .2
        .children
        .into_iter()
        .find(|(n, _)| n == "extra")
        .unwrap();
    assert_ne!(child.1, "demo.Controller");
    let unchanged = m.project.input_digest();
    assert!(
        m.graph_command(
            "add_child",
            &json!({"parent_type":key,"child_name":"bad","type_name":"@builtin:Unknown"})
        )
        .is_err()
    );
    assert_eq!(m.project.input_digest(), unchanged);
    assert!(
        m.graph_command(
            "add_child",
            &json!({"parent_type":key,"child_name":"a","type_name":"@builtin:Gateway"})
        )
        .is_err()
    );
    assert_eq!(m.project.input_digest(), unchanged);
    assert_eq!(m.project.header.profile.as_deref(), None);
}

#[test]
fn template_export_is_standalone_and_draft_cannot_register_source() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("exports")).unwrap();
    let mut registry =
        output::TargetRegistry::open(&f.root.join("exports"), &f.root.join("state")).unwrap();
    for kind in ["can", "multibus"] {
        let p = template::new_project(kind, "TestProject", &f.root).unwrap();
        assert!(!p.config.exists());
        assert!(registry.register_source(&p).is_err());
        let destination = registry.register_destination(kind).unwrap();
        let plan =
            output::build_plan(&p, 1, 1, destination["id"].as_str().unwrap(), &registry).unwrap();
        let report = output::save_plan(plan, &mut registry, None, false);
        assert_eq!(
            report.state,
            output::SaveOutcome::Complete,
            "{:?}",
            report.error
        );
        let config = f.root.join("exports").join(kind).join("project.ini");
        let ready = crate::input::prepare(&config).unwrap();
        assert_eq!(
            ready.common.profile,
            if kind == "can" {
                "can.cc.ideal.v1"
            } else {
                "can.cc.multibus.v1"
            }
        );
        assert!(
            ready
                .common
                .inputs
                .iter()
                .all(|i| i.path.starts_with(f.root.join("exports").join(kind)))
        );
    }
}

#[test]
fn edits_preserve_captured_empty_directories() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("models/empty")).unwrap();
    let mut m = EditorSession::new(f.load());
    let key = network(&m);
    m.graph_command(
        "set_layout",
        &json!({"type_key":key,"positions":{"a":{"x":1,"y":2}}}),
    )
    .unwrap();
    assert!(
        m.project
            .directories
            .contains_key(&f.root.join("models/empty"))
    );
    m.graph_command(
        "add_child",
        &json!({"parent_type":key,"child_name":"extra","type_name":"@builtin:Controller"}),
    )
    .unwrap();
    assert!(
        m.project
            .directories
            .contains_key(&f.root.join("models/empty"))
    );
}

#[test]
fn migration_preserves_unrelated_profile_parameters_and_original_values() {
    let f = Fixture::new();
    let ned = f.root.join("models/demo/Main.ned");
    let raw = fs::read_to_string(&ned).unwrap();
    fs::write(
        &ned,
        raw.replace(
            "network Main {",
            "network Main {\n parameters:\n  string profile = default(\"can.cc.ideal.v1\");",
        ),
    )
    .unwrap();
    let ini = f.root.join("project.ini");
    let raw = fs::read_to_string(&ini).unwrap();
    fs::write(&ini,format!("{raw}\nMain.profile = \"can.cc.ideal.v1\"\nMain.bus.profile = \"can.cc.ideal.v1\"\nMain.a.txProcessingDelay = 123ps\n")).unwrap();
    let mut m = EditorSession::new(f.load());
    let key = network(&m);
    m.graph_command(
        "add_child",
        &json!({"parent_type":key,"child_name":"gw","type_name":"@builtin:Gateway"}),
    )
    .unwrap();
    assert_eq!(
        m.project.header.general["Main.profile"],
        "\"can.cc.ideal.v1\""
    );
    assert_eq!(
        m.project.header.general["Main.bus.profile"],
        "\"can.cc.multibus.v1\""
    );
    assert_eq!(
        m.project.header.general["Main.a.txProcessingDelay"],
        "123ps"
    );
    assert_eq!(
        m.declaration(&key).unwrap().2.parameters["profile"]
            .default
            .as_deref(),
        Some("\"can.cc.ideal.v1\"")
    );
}

#[test]
fn input_is_released_and_snapshot_prepare_survives_source_removal() {
    let f = Fixture::new();
    let p = f.load();
    fs::remove_dir_all(f.root.join("models")).unwrap();
    fs::remove_file(f.root.join("project.ini")).unwrap();
    fs::remove_file(f.root.join("baseline.json")).unwrap();
    assert_eq!(analysis::prepare(&p).unwrap().can.controllers.len(), 3);
}
#[test]
fn syntax_load_is_rejected_but_semantic_error_is_imported() {
    let f = Fixture::new();
    let ini = f.root.join("project.ini");
    let old = fs::read_to_string(&ini).unwrap();
    fs::write(&ini, old.replace("500kbps", "0bps")).unwrap();
    let p = f.load();
    assert!(analysis::prepare(&p).is_err());
    fs::write(
        f.root.join("models/demo/Main.ned"),
        "package demo; network Main { submodules: broken }",
    )
    .unwrap();
    assert_eq!(
        load_project(&ini, &f.root).unwrap_err().code,
        "E-EDITOR-LOAD-SYNTAX"
    );
}
#[test]
fn invalid_utf8_layout_warns_and_preserves_original_hash() {
    let f = Fixture::new();
    let path = f.root.join("models/demo/Main.ned.layout.json");
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let p = f.load();
    let l = p.layouts.values().next().unwrap();
    assert!(l.warning.is_some());
    assert!(l.adopted.is_none());
    assert_eq!(l.hash.as_deref(), Some(hash(&[0xff, 0xfe]).as_str()));
    assert!(l.stat.is_some());
}
#[test]
fn atomic_graph_child_delete_preserves_comments_and_is_undoable() {
    let f = Fixture::new();
    let path = f.root.join("models/demo/Main.ned");
    let original = fs::read_to_string(&path).unwrap().replace(
        "a: demo.Controller;",
        "a: /* keep 日本語 */ demo.Controller;",
    );
    fs::write(&path, &original).unwrap();
    let mut m = EditorSession::new(f.load());
    let key = network(&m);
    assert!(
        m.graph_command(
            "delete_child",
            &json!({"element_key":analysis::node_key(&key,"a")})
        )
        .unwrap()
    );
    let edited = m.project.ned_files().next().unwrap();
    assert!(edited.text.contains("/* keep 日本語 */"));
    let (_, _, d) = m.declaration(&key).unwrap();
    assert_eq!(d.children.len(), 3);
    assert_eq!(d.connections.len(), 4);
    assert!(m.undo().unwrap());
    sync_parse(&mut m);
    assert_eq!(
        m.project.ned_files().next().unwrap().text.as_ref(),
        original
    );
    assert!(!m.dirty());
    assert!(m.redo().unwrap());
    sync_parse(&mut m);
    assert_eq!(m.declaration(&key).unwrap().2.connections.len(), 4);
    assert_eq!(fs::read_to_string(path).unwrap(), original);
}
#[test]
fn graph_can_pair_is_one_transaction_with_two_edges() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let key = network(&m);
    m.graph_command(
        "disconnect",
        &json!({"connection_key":analysis::connection_key(&key,1)}),
    )
    .unwrap();
    m.graph_command(
        "disconnect",
        &json!({"connection_key":analysis::connection_key(&key,0)}),
    )
    .unwrap();
    let before = m.project.ned_files().next().unwrap().text.clone();
    m.graph_command("connect_can_pair",&json!({"parent_type":key,"controller":"a","bus":"bus","input_gate":"rx_a","output_gate":"tx_a","tx_channel":"demo.Wire","rx_channel":"demo.Wire"})).unwrap();
    assert_eq!(m.declaration(&key).unwrap().2.connections.len(), 6);
    assert!(analysis::prepare(&m.project).is_ok());
    m.undo().unwrap();
    sync_parse(&mut m);
    assert_eq!(m.project.ned_files().next().unwrap().text, before);
}
#[test]
fn default_literal_and_layout_undo_preserve_input_revision_boundary() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let file = m.project.ned_files().next().unwrap();
    let id = file.id.clone();
    let key = analysis::type_key(&id, 0, "demo.Controller");
    m.graph_command("set_default",&json!({"parameter_key":analysis::parameter_key(&key,"rxFilter"),"literal":"\"x\\\"日本語\""})).unwrap();
    assert_eq!(
        m.declaration(&key).unwrap().2.parameters["rxFilter"]
            .default
            .as_deref(),
        Some("\"x\\\"日本語\"")
    );
    let revision = m.input_revision;
    let net = network(&m);
    m.graph_command(
        "set_layout",
        &json!({"type_key":net,"positions":{"a":{"x":-123.5,"y":700,"collapsed":true}}}),
    )
    .unwrap();
    assert_eq!(m.input_revision, revision);
    m.undo().unwrap();
    assert_eq!(m.input_revision, revision);
    m.undo().unwrap();
    sync_parse(&mut m);
    assert!(!m.dirty());
    assert!(m.can_redo());
}
#[test]
fn bad_graph_candidate_does_not_change_revision_or_history() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let key = network(&m);
    let revision = m.revision;
    assert!(
        m.graph_command(
            "connect",
            &json!({"parent_type":key,"from":"a.rx","to":"bus.rx_a"})
        )
        .is_err()
    );
    assert_eq!(m.revision, revision);
    assert!(!m.can_undo());
    assert!(!m.dirty());
}
#[test]
fn old_parse_result_never_replaces_newer_source() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let old = analysis::parse_project(&m.project);
    let file = m.project.ned_files().next().unwrap();
    let id = file.id.clone();
    let source = format!("{}\n// latest", file.text);
    m.commit(vec![(id.clone(), source)], vec![]).unwrap();
    m.adopt_analysis(old, m.context_epoch);
    assert_eq!(m.syntax[&id], "pending");
    assert!(m.current_parse(&id).is_err());
    sync_parse(&mut m);
    assert_eq!(m.syntax[&id], "parsed");
}
#[test]
fn no_op_keeps_redo_and_reload_resets_history_with_new_context() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let id = m.project.ned_files().next().unwrap().id.clone();
    let text = m.project.files[&id].text.to_string();
    m.commit(vec![(id.clone(), format!("{text}\n"))], vec![])
        .unwrap();
    m.undo().unwrap();
    assert!(!m.commit(vec![(id, text)], vec![]).unwrap());
    assert!(m.can_redo());
    let revision = m.revision;
    let input = m.input_revision;
    let epoch = m.context_epoch;
    let old_snapshot = m.project.id.clone();
    m.reload(f.load()).unwrap();
    assert!(m.revision > revision);
    assert_eq!(m.input_revision, input);
    assert!(m.context_epoch > epoch);
    assert_ne!(m.project.id, old_snapshot);
    assert!(!m.can_redo());
}
#[test]
fn default_literal_cannot_inject_ignored_property_statement() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let file = m.project.ned_files().next().unwrap();
    let key = analysis::type_key(&file.id, 0, "demo.Controller");
    let revision = m.revision;
    assert!(m.graph_command("set_default",&json!({"parameter_key":analysis::parameter_key(&key,"queueCapacity"),"literal":"1); @description(\"injected\""})).is_err());
    assert_eq!(m.revision, revision);
    assert!(!m.dirty());
}
#[test]
fn default_literal_cannot_hide_other_statements_in_a_comment() {
    let f = Fixture::new();
    let path = f.root.join("models/demo/Main.ned");
    let original = fs::read_to_string(&path).unwrap().replace(
        "int queueCapacity = default(64);",
        "int queueCapacity = default(64); @description(\"original\"); /* sentinel */",
    );
    fs::write(&path, original).unwrap();
    let mut m = EditorSession::new(f.load());
    let file = m.project.ned_files().next().unwrap();
    let key = analysis::type_key(&file.id, 0, "demo.Controller");
    assert!(m.graph_command("set_default",&json!({"parameter_key":analysis::parameter_key(&key,"queueCapacity"),"literal":"1); @description(\"injected\"); /*"})).is_err());
    assert!(!m.dirty());
}
#[test]
fn malformed_reconnect_channel_is_rejected_without_deleting_original() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let key = network(&m);
    assert!(m.graph_command("reconnect",&json!({"connection_key":analysis::connection_key(&key,0),"endpoint_side":"start","new_endpoint":"a.tx","channel":17})).is_err());
    assert!(!m.dirty());
    assert_eq!(
        m.declaration(&key).unwrap().2.connections[0]
            .channel
            .as_deref(),
        Some("demo.Wire")
    );
}
#[test]
fn pending_dependency_gates_are_unresolved_in_other_file_projection() {
    let f = Fixture::new();
    let path = f.root.join("models/demo/Main.ned");
    let original = fs::read_to_string(&path).unwrap();
    let split = original.find("network Main").unwrap();
    fs::write(f.root.join("models/demo/Types.ned"), &original[..split]).unwrap();
    fs::write(path, format!("package demo;\n{}", &original[split..])).unwrap();
    let state = Fixture {
        root: std::env::temp_dir().join(random_id("editor-view-state-").unwrap()),
    };
    fs::create_dir_all(state.root.join("exports")).unwrap();
    let registry =
        output::TargetRegistry::open(&state.root.join("exports"), &state.root.join("state"))
            .unwrap();
    let mut c =
        controller::Controller::new(f.root.join("project.ini"), f.root.clone(), registry).unwrap();
    let m = c.model.as_mut().unwrap();
    let key = network(m);
    let id = m
        .project
        .ned_files()
        .find(|file| file.path.ends_with("Types.ned"))
        .unwrap()
        .id
        .clone();
    m.commit(
        vec![(id, "package demo; simple Controller {".into())],
        vec![],
    )
    .unwrap();
    sync_parse(m);
    let q = std::collections::BTreeMap::from([("type_key".into(), key)]);
    let v = view::project(&c, &q);
    assert_eq!(v["graph"]["stale"], false);
    assert!(
        v["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["unresolved"] == true && n["gates"].as_array().unwrap().is_empty())
    );
}
#[test]
fn instance_projection_distinguishes_ini_override_from_prepared_value() {
    let f = Fixture::new();
    let state = Fixture {
        root: std::env::temp_dir().join(random_id("editor-view-state-").unwrap()),
    };
    fs::create_dir_all(state.root.join("exports")).unwrap();
    let registry =
        output::TargetRegistry::open(&state.root.join("exports"), &state.root.join("state"))
            .unwrap();
    let mut c =
        controller::Controller::new(f.root.join("project.ini"), f.root.clone(), registry).unwrap();
    let m = c.model.as_mut().unwrap();
    m.prepared = Some(std::sync::Arc::new(analysis::prepare(&m.project).unwrap()));
    let id = m.project.ned_files().next().unwrap().id.clone();
    let q = std::collections::BTreeMap::from([
        (
            "type_key".into(),
            analysis::type_key(&id, 0, "demo.Controller"),
        ),
        ("instance_path".into(), "Main.a".into()),
    ]);
    let v = view::project(&c, &q);
    let p = v["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "queueCapacity")
        .unwrap();
    assert_eq!(p["override"], "64");
    assert_eq!(p["effective"], "64");
    assert_eq!(p["instance_path"], "Main.a");
}

#[test]
fn history_evicts_the_oldest_operation_after_two_hundred_edits() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let id = m.project.ned_files().next().unwrap().id.clone();
    let original = m.project.files[&id].text.to_string();
    for step in 1..=201 {
        m.commit(
            vec![(id.clone(), format!("{original}\n// step {step}"))],
            vec![],
        )
        .unwrap();
    }
    let latest_revision = m.revision;
    for _ in 0..200 {
        assert!(m.undo().unwrap());
    }
    assert!(!m.undo().unwrap());
    assert_eq!(
        m.project.files[&id].text.as_ref(),
        format!("{original}\n// step 1")
    );
    assert!(m.revision > latest_revision);
    assert!(m.can_redo());
}

#[test]
fn history_limit_counts_forward_and_reverse_bytes_and_rejects_atomically() {
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let id = m.project.ned_files().next().unwrap().id.clone();
    let original = m.project.files[&id].text.to_string();
    let extent = 32 * 1024 * 1024;
    let first = format!("{original}{}", "a".repeat(extent));
    assert!(m.commit(vec![(id.clone(), first)], vec![]).unwrap());
    let revision = m.revision;
    let before_hash = m.project.files[&id].hash.clone();
    let second = format!("{original}{}", "b".repeat(extent));
    assert_eq!(
        m.commit(vec![(id.clone(), second)], vec![])
            .unwrap_err()
            .code,
        "E-EDITOR-HISTORY-LIMIT"
    );
    assert_eq!(m.revision, revision);
    assert_eq!(m.project.files[&id].hash, before_hash);
    assert!(m.undo().unwrap());
    assert_eq!(m.project.files[&id].text.as_ref(), original);
    assert!(!m.can_undo());
}

#[test]
fn default_and_reconnect_change_only_the_selected_token_span() {
    let f = Fixture::new();
    let path = f.root.join("models/demo/Main.ned");
    let original=fs::read_to_string(&path).unwrap().replace("int queueCapacity = default(64);", "int queueCapacity /* A */ =   default ( /* B */ 64 /* C */ ) /* D */ ;").replace("a.tx --> demo.Wire --> bus.rx_a;", "a . tx\t/* start stays */ -->   demo . Wire /* channel stays */ -->\tbus . rx_a ; /* tail */\r\n");
    fs::write(path, original).unwrap();
    let mut m = EditorSession::new(f.load());
    let id = m.project.ned_files().next().unwrap().id.clone();
    let ctrl = analysis::type_key(&id, 0, "demo.Controller");
    let before = m.project.files[&id].text.to_string();
    m.graph_command(
        "set_default",
        &json!({"parameter_key":analysis::parameter_key(&ctrl,"queueCapacity"),"literal":"65"}),
    )
    .unwrap();
    assert_eq!(
        m.project.files[&id].text.as_ref(),
        before.replacen("64 /* C */", "65 /* C */", 1)
    );
    let net = network(&m);
    m.graph_command(
        "disconnect",
        &json!({"connection_key":analysis::connection_key(&net,4)}),
    )
    .unwrap();
    let before = m.project.files[&id].text.to_string();
    let revision = m.revision;
    assert!(!m.graph_command("reconnect",&json!({"connection_key":analysis::connection_key(&net,0),"endpoint_side":"start","new_endpoint":"a.tx"})).unwrap());
    assert_eq!(m.revision, revision);
    m.graph_command("reconnect",&json!({"connection_key":analysis::connection_key(&net,0),"endpoint_side":"start","new_endpoint":"c.tx"})).unwrap();
    assert_eq!(
        m.project.files[&id].text.as_ref(),
        before.replacen("a . tx", "c.tx", 1)
    );
    m.graph_command("reconnect",&json!({"connection_key":analysis::connection_key(&net,0),"endpoint_side":"start","new_endpoint":"c.tx","channel":null})).unwrap();
    let text = m.project.files[&id].text.as_ref();
    assert!(text.contains("c.tx\t/* start stays */ -->   "));
    assert!(text.contains("/* channel stays */"));
    assert!(text.contains("\tbus . rx_a ; /* tail */\r\n"));
    assert!(
        m.declaration(&net).unwrap().2.connections[0]
            .channel
            .is_none()
    );
    m.graph_command("reconnect",&json!({"connection_key":analysis::connection_key(&net,0),"endpoint_side":"start","new_endpoint":"a.tx","channel":"demo.Wire"})).unwrap();
    let c = &m.declaration(&net).unwrap().2.connections[0];
    assert_eq!(c.start, "a.tx");
    assert_eq!(c.channel.as_deref(), Some("demo.Wire"));
}

#[test]
fn input_rejects_non_utf8_ned_symlink_and_overlapping_roots() {
    let f = Fixture::new();
    fs::write(f.root.join("models/demo/Main.ned"), [0xff]).unwrap();
    assert!(load_project(&f.root.join("project.ini"), &f.root).is_err());
    let f = Fixture::new();
    let file = f.root.join("models/demo/Main.ned");
    fs::rename(&file, f.root.join("original.ned")).unwrap();
    std::os::unix::fs::symlink(f.root.join("original.ned"), &file).unwrap();
    assert_eq!(
        load_project(&f.root.join("project.ini"), &f.root)
            .unwrap_err()
            .code,
        "E-EDITOR-INPUT-PATH"
    );
    let f = Fixture::new();
    let ini = f.root.join("project.ini");
    let text = fs::read_to_string(&ini).unwrap().replace(
        "ned-path = \"models\"",
        "ned-path = \"models\";\"models/demo\"",
    );
    fs::write(&ini, text).unwrap();
    assert_eq!(
        load_project(&ini, &f.root).unwrap_err().code,
        "E-EDITOR-INPUT-PATH"
    );
}
