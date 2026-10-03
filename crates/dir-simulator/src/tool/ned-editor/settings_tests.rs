use super::super::{analysis, template};
use super::*;

fn model(template: &str) -> EditorSession {
    let cwd = std::env::temp_dir().join(random_id("settings-test-").unwrap());
    EditorSession::new(template::new_project(template, "Settings", &cwd).unwrap())
}

fn rejected(model: &mut EditorSession, kind: &str, payload: Value) {
    let before = model.project.input_digest();
    let revision = model.revision;
    assert!(model.settings_command(kind, &payload).is_err());
    assert_eq!(model.project.input_digest(), before);
    assert_eq!(model.revision, revision);
}

#[test]
fn instance_parameters_validate_common_rules_and_undo_preserves_type_defaults() {
    let mut model = model("multibus");
    let original = model.project.input_digest();
    let ned = model.project.ned_files().next().unwrap().text.clone();
    for (name, literal) in [
        ("queueCapacity", "-1"),
        ("rxFilter", "\"invalid\""),
        ("txProcessingDelay", "2\nms"),
    ] {
        rejected(
            &mut model,
            "set_instance_parameter",
            json!({"instance_path":"Main.a","parameter_name":name,"literal":literal}),
        );
    }
    rejected(
        &mut model,
        "set_instance_parameter",
        json!({"instance_path":"Main.missing","parameter_name":"queueCapacity","literal":"8"}),
    );
    model
        .settings_command(
            "set_instance_parameter",
            &json!({"instance_path":"Main.a","parameter_name":"queueCapacity","literal":"8"}),
        )
        .unwrap();
    assert_eq!(
        analysis::prepare(&model.project)
            .unwrap()
            .can
            .controllers
            .iter()
            .find(|c| c.id == "Main.a")
            .unwrap()
            .queue_capacity,
        8
    );
    assert_eq!(model.project.ned_files().next().unwrap().text, ned);
    model.undo().unwrap();
    assert_eq!(model.project.input_digest(), original);
    model.redo().unwrap();
    model
        .settings_command(
            "set_instance_parameter",
            &json!({"instance_path":"Main.a","parameter_name":"queueCapacity","literal":null}),
        )
        .unwrap();
    assert!(
        !model
            .project
            .header
            .general
            .contains_key("Main.a.queueCapacity")
    );
}

#[test]
fn gateway_and_workload_forms_reject_invalid_ownership_and_preserve_other_settings() {
    let mut model = model("multibus-gateway");
    let mut gateway = document(&model, FileRole::ModelConfig).unwrap()["gateways"][0].clone();
    gateway["processing_delay"] = json!("25us");
    model.settings_command("set_gateway", &gateway).unwrap();
    let prepared = analysis::prepare(&model.project).unwrap();
    assert_eq!(prepared.gateway.gateways[0].processing_delay_ps, 25_000_000);
    let port = gateway["ports"][0].clone();
    rejected(
        &mut model,
        "set_workload",
        json!({"generators":[{"id":"traffic","kind":"can.explicit.v1","node":port,"frame":{"format":"standard","id":256,"data":"0102"},"times":["1ms"]}]}),
    );
    let mut invalid = gateway.clone();
    invalid["routes"][0]["egress"] = json!([gateway["routes"][0]["ingress"]]);
    rejected(&mut model, "set_gateway", invalid);
    model.settings_command("set_project_settings", &json!({"sim_time_limit":"20ms","metrics_window":"2ms","max_events":"100000","max_delta_cycles":"10000"})).unwrap();
    assert_eq!(
        document(&model, FileRole::ModelConfig).unwrap()["gateways"][0],
        gateway
    );
    assert_eq!(model.project.header.general["sim-time-limit"], "20ms");
}

#[test]
fn workload_forms_accept_explicit_and_periodic_and_reject_invalid_frames() {
    let mut model = model("multibus");
    let explicit = json!({"id":"first","kind":"can.explicit.v1","node":"Main.a","frame":{"format":"standard","id":256,"data":"01020304"},"times":["1ms","2ms"]});
    let periodic = json!({"id":"periodic","kind":"can.periodic.v1","node":"Main.b","frame":{"format":"extended","id":8192,"data":""},"start":"0ms","phase":"1ms","period":"5ms","count":2});
    model
        .settings_command("set_workload", &json!({"generators":[explicit,periodic]}))
        .unwrap();
    let prepared = analysis::prepare(&model.project).unwrap();
    assert_eq!(prepared.can.generators.len(), 2);
    assert_eq!(prepared.can.generators[0].frame.data, "01020304");
    let before = document(&model, FileRole::Workload).unwrap();
    for (key, value) in [("data", json!("xyz")), ("id", json!(2048))] {
        let mut g = before["generators"][0].clone();
        g["frame"][key] = value;
        rejected(&mut model, "set_workload", json!({"generators":[g]}));
    }
    let mut g = before["generators"][0].clone();
    g["times"] = json!(["2ms", "1ms"]);
    rejected(&mut model, "set_workload", json!({"generators":[g]}));
}

#[test]
fn malformed_or_unknown_json_is_never_silently_replaced_by_forms() {
    for raw in [
        "{\"schema_version\":1,\"schema_version\":1,\"generators\":[]}",
        "\u{feff}{\"schema_version\":1,\"schema_version\":1,\"generators\":[]}",
        "{\"schema_version\":1,\"generators\":[],\"unknown\":true}",
        "{\"schema_version\":1,\"generators\":null}",
    ] {
        let mut model = model("multibus");
        let id = model
            .project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::Workload))
            .unwrap()
            .id
            .clone();
        let file = model.project.files.get_mut(&id).unwrap();
        file.text = raw.into();
        file.hash = hash(raw.as_bytes());
        rejected(&mut model, "set_workload", json!({"generators":[]}));
        assert_eq!(model.project.files[&id].text.as_ref(), raw);
    }
}

#[test]
fn bom_json_forms_match_common_parsers_and_preserve_encoding_through_history() {
    for newline in ["\n", "\r\n"] {
        let mut model = model("multibus-gateway");
        for role in [FileRole::Workload, FileRole::ModelConfig] {
            let id = model
                .project
                .files
                .values()
                .find(|f| f.roles.contains(&role))
                .unwrap()
                .id
                .clone();
            let raw = format!(
                "\u{feff}{}",
                model.project.files[&id].text.replace('\n', newline)
            );
            let file = model.project.files.get_mut(&id).unwrap();
            file.text = raw.into();
            file.hash = hash(file.text.as_bytes());
        }
        analysis::prepare(&model.project).unwrap();
        for role in [FileRole::Workload, FileRole::ModelConfig] {
            let mut doc = document(&model, role.clone()).unwrap();
            let id = model
                .project
                .files
                .values()
                .find(|f| f.roles.contains(&role))
                .unwrap()
                .id
                .clone();
            let original = model.project.files[&id].text.clone();
            if role == FileRole::Workload {
                doc["generators"][0]["frame"]["data"] = json!("0102");
                model
                    .settings_command("set_workload", &json!({"generators":doc["generators"]}))
                    .unwrap();
            } else {
                doc["gateways"][0]["processing_delay"] = json!("25us");
                model
                    .settings_command("set_gateway", &doc["gateways"][0])
                    .unwrap();
            }
            let edited = model.project.files[&id].text.clone();
            assert!(edited.starts_with('\u{feff}'));
            assert!(edited.ends_with(newline));
            if newline == "\r\n" {
                assert!(!edited.replace("\r\n", "").contains(['\r', '\n']));
            }
            assert_eq!(document(&model, role).unwrap(), doc);
            analysis::prepare(&model.project).unwrap();
            model.undo().unwrap();
            assert_eq!(model.project.files[&id].text, original);
            model.redo().unwrap();
            assert_eq!(model.project.files[&id].text, edited);
            analysis::prepare(&model.project).unwrap();
        }
    }
}

#[test]
fn empty_project_has_no_stale_overrides_and_remains_an_editable_draft() {
    let mut model = model("multibus-empty");
    assert!(
        model
            .project
            .header
            .general
            .keys()
            .all(|key| !key.starts_with("Main."))
    );
    assert!(model.project.header.channels.is_empty());
    assert_eq!(
        document(&model, FileRole::Workload).unwrap()["generators"],
        json!([])
    );
    assert!(analysis::prepare(&model.project).is_err());
    model.settings_command("set_project_settings",&json!({"sim_time_limit":"10ms","metrics_window":"1ms","max_events":"100000","max_delta_cycles":"10000"})).unwrap();
    assert!(!model.project.config.exists());
}

#[test]
fn json_identifiers_accept_ned_reserved_words_and_disabled_generators() {
    let mut model = model("multibus-gateway");
    let mut gateway = document(&model, FileRole::ModelConfig).unwrap()["gateways"][0].clone();
    gateway["routes"][0]["id"] = json!("network");
    model.settings_command("set_gateway", &gateway).unwrap();
    let mut workload = document(&model, FileRole::Workload).unwrap();
    workload["generators"][0]["id"] = json!("input");
    model
        .settings_command(
            "set_workload",
            &json!({"generators":workload["generators"]}),
        )
        .unwrap();
    analysis::prepare(&model.project).unwrap();
}

#[test]
fn missing_workload_file_is_added_virtually_and_undo_restores_input_set() {
    let mut model = model("can");
    let id = model
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::Workload))
        .unwrap()
        .id
        .clone();
    model.project.files.remove(&id);
    let config = model
        .project
        .file_by_path(&model.project.config)
        .unwrap()
        .id
        .clone();
    let raw = model.project.files[&config]
        .text
        .lines()
        .filter(|line| !line.starts_with("workload ="))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let file = model.project.files.get_mut(&config).unwrap();
    file.text = raw.into();
    file.hash = hash(file.text.as_bytes());
    model.project.refresh_inputs();
    let mut model = EditorSession::new(model.project);
    let before = model.project.input_digest();
    model
        .settings_command("set_workload", &json!({"generators":[]}))
        .unwrap();
    let new = model
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&FileRole::Workload))
        .unwrap();
    assert!(new.stat.is_none());
    assert!(!new.path.exists());
    analysis::prepare(&model.project).unwrap();
    model.undo().unwrap();
    assert_eq!(model.project.input_digest(), before);
    assert!(model.project.header.workload.is_none());
    model.redo().unwrap();
    analysis::prepare(&model.project).unwrap();
}

#[test]
fn gateway_settings_can_be_removed_after_the_module_instance_was_deleted() {
    let mut model = model("multibus-gateway");
    let (file, ordinal, name) = model
        .project
        .ned_files()
        .find_map(|file| {
            analysis::shapes(model.current_parse(&file.id).unwrap())
                .iter()
                .enumerate()
                .find(|(_, d)| d.kind == "network")
                .map(|(n, d)| (file.id.clone(), n, d.name.clone()))
        })
        .unwrap();
    let key = analysis::type_key(&file, ordinal, &name);
    model
        .graph_command(
            "delete_child",
            &json!({"element_key":analysis::node_key(&key,"gw")}),
        )
        .unwrap();
    assert!(!instances(&model).unwrap().contains_key("Main.gw"));
    assert_eq!(
        document(&model, FileRole::ModelConfig).unwrap()["gateways"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    model
        .settings_command("delete_gateway", &json!({"node":"Main.gw"}))
        .unwrap();
    assert_eq!(
        document(&model, FileRole::ModelConfig).unwrap()["gateways"],
        json!([])
    );
    model.undo().unwrap();
    assert_eq!(
        document(&model, FileRole::ModelConfig).unwrap()["gateways"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn unrepresentable_route_ports_never_get_silently_normalized_by_a_form() {
    for egress in [
        json!(["Main.gw.b", "Main.gw.b"]),
        json!(["Main.gw.b", "Main.gw.missing"]),
    ] {
        let mut model = model("multibus-gateway");
        let mut doc = document(&model, FileRole::ModelConfig).unwrap();
        doc["gateways"][0]["routes"][0]["egress"] = egress;
        let id = model
            .project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::ModelConfig))
            .unwrap()
            .id
            .clone();
        let raw = serde_json::to_string(&doc).unwrap();
        let file = model.project.files.get_mut(&id).unwrap();
        file.text = raw.into();
        file.hash = hash(file.text.as_bytes());
        assert!(document(&model, FileRole::ModelConfig).is_err());
        rejected(&mut model, "delete_gateway", json!({"node":"Main.gw"}));
    }
}
