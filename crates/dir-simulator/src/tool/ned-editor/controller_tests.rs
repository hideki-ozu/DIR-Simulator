use super::super::analysis;
use super::*;
use std::fs;

#[test]
fn new_project_requires_confirmation_and_replay_preserves_one_replacement() {
    let f = Fixture::new();
    let before = f
        .shared
        .lock()
        .unwrap()
        .model
        .as_ref()
        .unwrap()
        .project
        .input_digest();
    let payload = json!({"template":"multibus-gateway","project_name":"NewProject"});
    assert_eq!(
        f.execute("new_project", payload.clone())["code"],
        "E-EDITOR-CONFIRMATION"
    );
    assert_eq!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .project
            .input_digest(),
        before
    );
    let preview = f.confirm("new_project", payload.clone());
    let mut payload = payload;
    payload["discard_ack"] = preview["confirmation"].clone();
    let request = f.command("new_project", payload);
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202);
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    assert_eq!(f.send(request).1, done);
    let c = f.shared.lock().unwrap();
    let m = c.model.as_ref().unwrap();
    assert!(m.revision > 1);
    assert!(m.never_exported);
    assert!(m.last_output.is_none());
    assert!(!m.can_undo());
    assert_eq!(
        analysis::prepare(&m.project)
            .unwrap()
            .gateway
            .gateways
            .len(),
        1
    );
    let view = super::super::view::project(&c, &BTreeMap::new());
    assert_eq!(view["project_origin"], "new");
    assert_eq!(view["project_name"], "NewProject");
    assert_eq!(view["capabilities"]["reload"]["enabled"], false);
    assert!(
        view["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["kind"] != "source_project")
    );
    assert_eq!(view["catalog"].as_array().unwrap().len(), 7);
    drop(c);
    assert_eq!(f.execute("reload", json!({}))["code"], "E-EDITOR-NO-ORIGIN");
}

#[test]
fn ini_json_source_edits_update_settings_and_undo_without_changing_paths() {
    let f = Fixture::new();
    let (id, hash, text) = {
        let c = f.shared.lock().unwrap();
        let p = &c.model.as_ref().unwrap().project;
        let file = p.file_by_path(&p.config).unwrap();
        (file.id.clone(), file.hash.clone(), file.text.to_string())
    };
    let changed = text.replace("500kbps", "250kbps");
    assert_eq!(
        f.execute(
            "replace_source",
            json!({"file_id":id,"expected_hash":hash,"text":changed})
        )["operation_status"],
        200
    );
    assert_eq!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .project
            .header
            .general["Main.bus.bitrate"],
        "250kbps"
    );
    assert_eq!(f.execute("validate", json!({}))["operation_status"], 200);
    assert_eq!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .prepared
            .as_ref()
            .unwrap()
            .can
            .bitrate,
        250_000
    );
    assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    let hash = f
        .shared
        .lock()
        .unwrap()
        .model
        .as_ref()
        .unwrap()
        .project
        .files[&id]
        .hash
        .clone();
    let response = f.execute(
        "replace_source",
        json!({"file_id":id,"expected_hash":hash,"text":text.replace("\"models\"","\"other\"")}),
    );
    assert_eq!(response["code"], "E-EDITOR-INPUT-PATH");
    let target = f.destination("newjson");
    let p = json!({"template":"multibus","project_name":"Editable"});
    let ack = f.confirm("new_project", p.clone());
    let mut p = p;
    p["discard_ack"] = ack["confirmation"].clone();
    assert_eq!(f.execute("new_project", p)["operation_status"], 200);
    let (id, hash) = {
        let c = f.shared.lock().unwrap();
        let file = c
            .model
            .as_ref()
            .unwrap()
            .project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::ModelConfig))
            .unwrap();
        (file.id.clone(), file.hash.clone())
    };
    assert_eq!(f.execute("replace_source",json!({"file_id":id,"expected_hash":hash,"text":"{\"schema_version\":1,\"gateways\":[],\"unknown\":true}"}))["operation_status"],200);
    let failure = f.execute("save_as_project", json!({"destination_id":target}));
    assert_ne!(failure["operation_status"], 200);
    assert!(!f.root.join("exports/newjson/project.ini").exists());
    assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    assert_eq!(
        f.execute("save_as_project", json!({"destination_id":target}))["operation_status"],
        200
    );
}

#[test]
fn no_config_creates_virtual_default_without_source_target() {
    let f = Fixture::new();
    let registry =
        TargetRegistry::open(&f.root.join("exports"), &f.root.join("new-state")).unwrap();
    let c = Controller::with_config(None, f.root.clone(), registry).unwrap();
    let m = c.model.as_ref().unwrap();
    assert!(
        matches!(&m.project.origin,super::super::input::ProjectOrigin::New { template, name } if template=="multibus" && name=="Untitled")
    );
    assert!(!m.project.config.exists());
    assert!(!f.root.join(".ned-editor-drafts").exists());
    assert!(c.registry.targets().is_empty());
    assert_eq!(
        analysis::prepare(&m.project).unwrap().common.profile,
        "can.cc.multibus.v1"
    );
}

struct Fixture {
    root: PathBuf,
    shared: Shared,
    client: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(random_id("ned-controller-test-").unwrap());
        fs::create_dir_all(root.join("input/models/demo")).unwrap();
        fs::create_dir(root.join("exports")).unwrap();
        fs::write(
            root.join("input/models/demo/Main.ned"),
            include_str!("../../../../../docs/verification/fixtures/can/models/demo/Main.ned"),
        )
        .unwrap();
        fs::write(root.join("input/project.ini"), "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n").unwrap();
        let registry = TargetRegistry::open(&root.join("exports"), &root.join("state")).unwrap();
        let shared = Arc::new(Mutex::new(
            Controller::new(root.join("input/project.ini"), root.clone(), registry).unwrap(),
        ));
        {
            let c = shared.lock().unwrap();
            assert!(
                c.model.is_some(),
                "Startup diagnostics: {:?}",
                c.diagnostics
            );
        }
        let (status, writer) = call(
            &shared,
            "POST",
            "/api/writer",
            Some(json!({"action":"register"})),
        );
        assert_eq!(status, 200);
        let client = writer["client_id"].as_str().unwrap().to_owned();
        Self {
            root,
            shared,
            client,
        }
    }
    fn command(&self, kind: &str, payload: Value) -> Value {
        let c = self.shared.lock().unwrap();
        let sequence = c.clients[&self.client].high_watermark + 1;
        json!({"schema_version":1,"session_id":c.session_id,"client_id":self.client,"writer_epoch":c.writer_epoch.to_string(),"client_sequence":sequence.to_string(),"command_id":format!("{}:{sequence}",self.client),"base_revision":c.revision().to_string(),"kind":kind,"payload":payload})
    }
    fn send(&self, request: Value) -> (u16, Value) {
        call(&self.shared, "POST", "/api/commands", Some(request))
    }
    fn execute(&self, kind: &str, payload: Value) -> Value {
        let (status, response) = self.send(self.command(kind, payload));
        if status == 202 {
            terminal(&self.shared, &response)
        } else {
            response
        }
    }
    fn source(&self) -> (String, String, String) {
        let c = self.shared.lock().unwrap();
        let f = c
            .model
            .as_ref()
            .unwrap()
            .project
            .ned_files()
            .next()
            .unwrap();
        (f.id.clone(), f.hash.clone(), f.text.to_string())
    }
    fn edit(&self, text: &str) -> Value {
        let (id, hash, _) = self.source();
        self.execute(
            "replace_source",
            json!({"file_id":id,"expected_hash":hash,"text":text}),
        )
    }
    fn confirm(&self, kind: &str, payload: Value) -> Value {
        let c = self.shared.lock().unwrap();
        let request = json!({"client_id":self.client,"writer_epoch":c.writer_epoch.to_string(),"base_revision":c.revision().to_string(),"kind":kind,"payload":payload});
        drop(c);
        let (status, result) = call(&self.shared, "POST", "/api/confirmations", Some(request));
        assert_eq!(status, 200, "{result}");
        result
    }
    fn destination(&self, relative: &str) -> String {
        let (status, result) = call(
            &self.shared,
            "POST",
            "/api/destinations",
            Some(json!({"export_root_id":"export-root","relative_directory":relative})),
        );
        assert_eq!(status, 200, "{result}");
        result["id"].as_str().unwrap().to_owned()
    }
    fn network(&self) -> String {
        let c = self.shared.lock().unwrap();
        let m = c.model.as_ref().unwrap();
        let f = m.project.ned_files().next().unwrap();
        let declarations = shapes(m.project.parsed.get(&f.id).unwrap());
        let i = declarations
            .iter()
            .position(|d| d.kind == "network")
            .unwrap();
        super::super::analysis::type_key(&f.id, i, &declarations[i].name)
    }
    fn source_target(&self) -> String {
        self.shared
            .lock()
            .unwrap()
            .registry
            .targets()
            .into_iter()
            .find(|t| t["kind"] == "source_project")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn call(shared: &Shared, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    Controller::handle(shared, method, path, &BTreeMap::new(), body)
}
fn terminal(shared: &Shared, accepted: &Value) -> Value {
    let path = format!("/api/jobs/{}", accepted["job_id"].as_str().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, value) = call(shared, "GET", &path, None);
        assert_eq!(status, 200);
        if value["terminal"] == true {
            return value;
        }
        assert!(Instant::now() < deadline, "Job did not finish: {value}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn await_parse(f: &Fixture) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if f.shared.lock().unwrap().model.as_ref().unwrap().analysis != "pending" {
            return;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn strict_json_rejects_nested_duplicate_keys_and_trailing_tokens() {
    for bytes in [
        br#"{"a":1,"a":2}"#.as_slice(),
        br#"{"a":[{"b":1,"b":2}]}"#,
        br#"[1] null"#,
    ] {
        assert_eq!(strict_json(bytes).unwrap_err().status, 400);
    }
    assert_eq!(
        strict_json(br#"{"a":[{"b":1}]} "#).unwrap(),
        json!({"a":[{"b":1}]})
    );
}
#[test]
fn commands_preserve_invalid_source_and_idempotent_terminal_results() {
    let f = Fixture::new();
    let (id, hash, _) = f.source();
    let request = f.command(
        "replace_source",
        json!({"file_id":id,"expected_hash":hash,"text":"network Broken {"}),
    );
    let (status, first) = f.send(request.clone());
    assert_eq!(status, 200);
    assert_eq!(first["sequence_consumed"], true);
    assert_eq!(first["applied_revision"], "2");
    assert_eq!(f.send(request.clone()), (200, first.clone()));
    let (status, lookup) = call(
        &f.shared,
        "GET",
        &format!("/api/commands/{}", request["command_id"].as_str().unwrap()),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(lookup, first);
    let mut changed = request;
    changed["payload"]["text"] = "different".into();
    let (status, response) = f.send(changed);
    assert_eq!(status, 409);
    assert_eq!(response["sequence_consumed"], false);
    await_parse(&f);
    assert_eq!(f.source().2, "network Broken {");
    assert_eq!(
        f.shared.lock().unwrap().model.as_ref().unwrap().analysis,
        "syntax_error"
    );
    assert!(
        fs::read_to_string(f.root.join("input/models/demo/Main.ned"))
            .unwrap()
            .contains("network Main")
    );
}
#[test]
fn malformed_auth_epoch_busy_and_revision_have_distinct_acceptance_boundaries() {
    let f = Fixture::new();
    for bad in ["01", "-1", "18446744073709551616"] {
        let mut request = f.command("undo", json!({}));
        request["writer_epoch"] = bad.into();
        let (status, response) = f.send(request);
        assert_eq!(status, 400);
        assert_eq!(response["sequence_consumed"], false);
    }
    let mut request = f.command("undo", json!({}));
    request["surprise"] = true.into();
    assert_eq!(f.send(request).0, 400);
    let mut request = f.command("undo", json!({}));
    request["payload"]["surprise"] = true.into();
    assert_eq!(f.send(request).0, 400);
    let mut request = f.command("undo", json!({}));
    request["session_id"] = "wrong".into();
    assert_eq!(f.send(request).0, 403);
    let mut request = f.command("undo", json!({}));
    request["writer_epoch"] = "0".into();
    assert_eq!(f.send(request).0, 403);
    f.shared.lock().unwrap().busy = Some("save".into());
    let (_, busy) = f.send(f.command("undo", json!({})));
    assert_eq!(busy["accepted"], true);
    assert_eq!(busy["sequence_consumed"], true);
    f.shared.lock().unwrap().busy = None;
    let mut request = f.command("undo", json!({}));
    request["base_revision"] = "0".into();
    let (_, stale) = f.send(request);
    assert_eq!(stale["accepted"], true);
    assert_eq!(stale["sequence_consumed"], true);
    assert_eq!(stale["next_sequence"], "3");
    assert_eq!(f.execute("undo", json!({}))["next_sequence"], "4");
}
#[test]
fn first_tab_writer_claim_requires_idle_epoch_and_preserves_replay() {
    let f = Fixture::new();
    let request = f.command("undo", json!({}));
    let first = f.send(request.clone());
    let (_, second) = call(
        &f.shared,
        "POST",
        "/api/writer",
        Some(json!({"action":"register"})),
    );
    assert_eq!(second["writer"], false);
    let claim =
        json!({"action":"claim","client_id":second["client_id"],"expected_writer_epoch":"1"});
    f.shared.lock().unwrap().busy = Some("validate".into());
    assert_eq!(
        call(&f.shared, "POST", "/api/writer", Some(claim.clone())).0,
        409
    );
    f.shared.lock().unwrap().busy = None;
    let (_, claimed) = call(&f.shared, "POST", "/api/writer", Some(claim.clone()));
    assert_eq!(claimed["writer_epoch"], "2");
    assert_eq!(call(&f.shared, "POST", "/api/writer", Some(claim)).0, 403);
    assert_eq!(f.send(request), first);
    assert_eq!(f.send(f.command("undo", json!({}))).0, 403);
}
#[test]
fn source_hash_role_and_undo_redo_are_checked_without_losing_history() {
    let f = Fixture::new();
    let (_, _, source) = f.source();
    assert_eq!(
        f.edit(&format!("{source}\n// changed"))["operation_status"],
        200
    );
    assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    assert_eq!(f.source().2, source);
    assert_eq!(f.execute("redo", json!({}))["operation_status"], 200);
    assert!(f.source().2.contains("// changed"));
    let (id, _, _) = f.source();
    let failure = f.execute(
        "replace_source",
        json!({"file_id":id,"expected_hash":"stale","text":"wrong"}),
    );
    assert_eq!(failure["code"], "E-EDITOR-PATCH");
    assert_eq!(failure["sequence_consumed"], true);
    let config_id = {
        let c = f.shared.lock().unwrap();
        c.model
            .as_ref()
            .unwrap()
            .project
            .files
            .values()
            .find(|f| f.roles.contains(&FileRole::Config))
            .unwrap()
            .id
            .clone()
    };
    assert_eq!(
        f.execute(
            "replace_source",
            json!({"file_id":config_id,"expected_hash":"anything","text":"wrong"})
        )["operation_status"],
        409
    );
    assert!(f.shared.lock().unwrap().model.as_ref().unwrap().can_undo());
}
#[test]
fn graph_confirmations_are_bound_one_use_and_jobs_share_terminal_ledger() {
    let f = Fixture::new();
    let key = f.network();
    let payload = json!({"parent_type":key,"child_name":"extra","type_name":"demo.Controller","position":{"x":150,"y":50}});
    assert_eq!(
        f.execute("add_child", payload.clone())["code"],
        "E-EDITOR-CONFIRMATION"
    );
    let confirm = f.confirm("add_child", payload.clone());
    assert!(!confirm["impacts"].as_array().unwrap().is_empty());
    let mut request = f.command("add_child", payload.clone());
    request["impact_ack"] = confirm["confirmation"].clone();
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202);
    assert_eq!(accepted["sequence_consumed"], false);
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200);
    assert_eq!(done["sequence_consumed"], true);
    assert_eq!(f.send(request).1, done);
    let mut reused = f.command("add_child", payload);
    reused["impact_ack"] = confirm["confirmation"].clone();
    assert_eq!(f.send(reused).1["code"], "E-EDITOR-CONFIRMATION");
    assert!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .declaration(&key)
            .unwrap()
            .2
            .children
            .iter()
            .any(|(n, _)| n == "extra")
    );
}
#[test]
fn changed_payload_or_revision_invalidates_confirmation() {
    let f = Fixture::new();
    let payload = json!({"element_key":super::super::analysis::node_key(&f.network(),"a")});
    let confirmation = f.confirm("delete_child", payload.clone());
    let mut request = f.command("delete_child", json!({"element_key":"different"}));
    request["impact_ack"] = confirmation["confirmation"].clone();
    assert_eq!(f.send(request).1["code"], "E-EDITOR-CONFIRMATION");
    let confirmation = f.confirm("delete_child", payload.clone());
    let (_, _, source) = f.source();
    f.edit(&format!("{source}\n// changed"));
    let mut request = f.command("delete_child", payload);
    request["impact_ack"] = confirmation["confirmation"].clone();
    assert_eq!(f.send(request).1["code"], "E-EDITOR-CONFIRMATION");
}

#[test]
fn settings_use_bound_confirmations_async_ledger_and_instance_scope() {
    let f = Fixture::new();
    let payload = json!({"instance_path":"Main.a","parameter_name":"queueCapacity","literal":"8"});
    assert_eq!(
        f.execute("set_instance_parameter", payload.clone())["code"],
        "E-EDITOR-CONFIRMATION"
    );
    let before = f.source().2;
    let confirmation = f.confirm("set_instance_parameter", payload.clone());
    let mut request = f.command("set_instance_parameter", payload);
    request["impact_ack"] = confirmation["confirmation"].clone();
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202);
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    assert_eq!(f.send(request).1, done);
    let c = f.shared.lock().unwrap();
    let m = c.model.as_ref().unwrap();
    assert_eq!(m.project.header.general["Main.a.queueCapacity"], "8");
    drop(c);
    assert_eq!(f.source().2, before);
    let result = f.execute("undo", json!({}));
    assert_eq!(result["operation_status"], 200, "{result}");
}

#[test]
fn new_command_payloads_reject_unknown_fields_and_settings_stale_confirmation() {
    let f = Fixture::new();
    let payload = json!({"sim_time_limit":"20ms","metrics_window":"1ms","max_events":"1000000","max_delta_cycles":"1000"});
    let mut invalid = payload.clone();
    invalid["profile"] = json!("arbitrary");
    assert_eq!(f.send(f.command("set_project_settings", invalid)).0, 400);
    let ack = f.confirm("set_project_settings", payload.clone());
    f.edit(&format!("{}\n// newer source\n", f.source().2));
    let mut request = f.command("set_project_settings", payload);
    request["impact_ack"] = ack["confirmation"].clone();
    assert_eq!(f.send(request).1["code"], "E-EDITOR-CONFIRMATION");
    assert_eq!(
        f.send(f.command(
            "add_gates",
            json!({"type_key":f.network(),"gates":[{"name":"x","output":false,"unsupported":true}]})
        ))
        .0,
        400
    );
}
#[test]
fn validation_is_async_and_semantic_failure_uses_common_diagnostic() {
    let f = Fixture::new();
    let (status, accepted) = f.send(f.command("validate", json!({})));
    assert_eq!(status, 202);
    assert_eq!(terminal(&f.shared, &accepted)["operation_status"], 200);
    let old_sequence = f.shared.lock().unwrap().view_sequence;
    let (_, _, source) = f.source();
    f.edit(&source.replace("default(64)", "default(-1)"));
    let result = f.execute("validate", json!({}));
    assert_eq!(result["operation_status"], 422);
    let c = f.shared.lock().unwrap();
    let m = c.model.as_ref().unwrap();
    assert!(
        m.diagnostics
            .iter()
            .any(|d| d["origin"] == "semantic" && d["code"].as_str().unwrap().starts_with("E-"))
    );
    assert!(c.view_sequence > old_sequence);
    assert!(m.prepared.is_none());
    assert!(c.busy.is_none());
}
#[test]
fn parse_debounce_adopts_latest_input_without_revision_increment() {
    let f = Fixture::new();
    let (_, _, source) = f.source();
    for i in 0..12 {
        assert_eq!(
            f.edit(&format!("{source}\n// generation {i}"))["operation_status"],
            200
        );
    }
    let revision = f.shared.lock().unwrap().revision();
    await_parse(&f);
    let c = f.shared.lock().unwrap();
    let m = c.model.as_ref().unwrap();
    let file = m.project.ned_files().next().unwrap();
    assert_eq!(m.revision, revision);
    assert_eq!(m.parsed_hashes[&file.id], file.hash);
    assert!(file.text.ends_with("generation 11"));
}
#[test]
fn save_as_uses_snapshot_after_source_removal_and_marks_only_complete() {
    let f = Fixture::new();
    let (_, _, source) = f.source();
    f.edit(&source.replace("default(64)", "default(65)"));
    let destination = f.destination("fresh");
    fs::remove_dir_all(f.root.join("input")).unwrap();
    let result = f.execute("save_as_project", json!({"destination_id":destination}));
    assert_eq!(result["operation_status"], 200, "{result}");
    let c = f.shared.lock().unwrap();
    assert!(!c.model.as_ref().unwrap().dirty());
    assert!(!c.model.as_ref().unwrap().never_exported);
    assert!(
        c.registry
            .targets()
            .iter()
            .any(|t| t["id"] == destination && t["kind"] == "managed_export")
    );
    assert!(
        fs::read_to_string(f.root.join("exports/fresh/ned/0001/demo/Main.ned"))
            .unwrap()
            .contains("default(65)")
    );
    assert!(result["result"]["revision"].is_string());
    assert!(result["result"]["input_revision"].is_string());
}
#[test]
fn invalid_save_keeps_dirty_and_creates_no_output() {
    let f = Fixture::new();
    let (_, _, source) = f.source();
    f.edit(&source.replace("default(64)", "default(-1)"));
    let destination = f.destination("invalid");
    let result = f.execute("save_as_project", json!({"destination_id":destination}));
    assert_eq!(result["operation_status"], 422, "{result}");
    assert!(f.shared.lock().unwrap().model.as_ref().unwrap().dirty());
    assert!(!f.root.join("exports/invalid").exists());
}
#[test]
fn overwrite_requires_exact_preview_and_detects_external_change() {
    let f = Fixture::new();
    let destination = f.source_target();
    let (_, _, source) = f.source();
    f.edit(&source.replace("default(64)", "default(65)"));
    let preview = f.confirm("overwrite_project", json!({"destination_id":destination}));
    assert!(preview["plan_digest"].is_string());
    fs::write(
        f.root.join("input/models/demo/Main.ned"),
        format!("{source}\n// external"),
    )
    .unwrap();
    let result = f.execute(
        "overwrite_project",
        json!({"destination_id":destination,"overwrite_ack":preview["confirmation"]}),
    );
    assert_eq!(result["operation_status"], 409, "{result}");
    assert!(f.shared.lock().unwrap().model.as_ref().unwrap().dirty());
    assert!(
        fs::read_to_string(f.root.join("input/models/demo/Main.ned"))
            .unwrap()
            .ends_with("// external")
    );
}
#[test]
fn dirty_reload_requires_confirmation_and_failed_reload_preserves_model() {
    let f = Fixture::new();
    let (_, _, source) = f.source();
    f.edit(&format!("{source}\n// local"));
    assert_eq!(
        f.execute("reload", json!({}))["code"],
        "E-EDITOR-CONFIRMATION"
    );
    let preview = f.confirm("reload", json!({}));
    fs::write(
        f.root.join("input/models/demo/Main.ned"),
        "network Broken {",
    )
    .unwrap();
    let result = f.execute("reload", json!({"discard_ack":preview["confirmation"]}));
    assert_eq!(result["operation_status"], 422);
    assert!(f.source().2.ends_with("// local"));
    assert!(f.shared.lock().unwrap().model.as_ref().unwrap().can_undo());
    fs::write(f.root.join("input/models/demo/Main.ned"), &source).unwrap();
    let preview = f.confirm("reload", json!({}));
    assert_eq!(
        f.execute("reload", json!({"discard_ack":preview["confirmation"]}))["operation_status"],
        200
    );
    let c = f.shared.lock().unwrap();
    assert!(!c.model.as_ref().unwrap().dirty());
    assert!(!c.model.as_ref().unwrap().can_undo());
    drop(c);
    assert_eq!(f.source().2, source);
}
#[test]
fn failed_startup_is_browsable_and_reload_can_load_after_source_appears() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("input/project.ini")).unwrap();
    let registry = TargetRegistry::open(&f.root.join("exports"), &f.root.join("state")).unwrap();
    let shared = Arc::new(Mutex::new(
        Controller::new(f.root.join("input/project.ini"), f.root.clone(), registry).unwrap(),
    ));
    assert!(shared.lock().unwrap().model.is_none());
    assert_eq!(call(&shared, "GET", "/api/session", None).0, 200);
    let (_, writer) = call(
        &shared,
        "POST",
        "/api/writer",
        Some(json!({"action":"register"})),
    );
    fs::write(f.root.join("input/project.ini"),"[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n").unwrap();
    let c = shared.lock().unwrap();
    let request = json!({"schema_version":1,"session_id":c.session_id,"client_id":writer["client_id"],"writer_epoch":writer["writer_epoch"],"client_sequence":"1","command_id":format!("{}:1",writer["client_id"].as_str().unwrap()),"base_revision":"0","kind":"reload","payload":{}});
    drop(c);
    let (status, accepted) = call(&shared, "POST", "/api/commands", Some(request));
    assert_eq!(status, 202);
    assert_eq!(terminal(&shared, &accepted)["operation_status"], 200);
    assert!(shared.lock().unwrap().model.is_some());
    assert!(shared.lock().unwrap().diagnostics.is_empty());
}
#[test]
fn ledger_retains_a_thousand_responses_and_high_watermark_prevents_reexecution() {
    let f = Fixture::new();
    let first = f.command("undo", json!({}));
    assert_eq!(f.send(first.clone()).0, 200);
    for _ in 0..LEDGER_LIMIT {
        assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    }
    assert_eq!(f.shared.lock().unwrap().ledger.len(), LEDGER_LIMIT);
    let (status, result) = f.send(first);
    assert_eq!(status, 410);
    assert_eq!(result["sequence_consumed"], false);
    assert_eq!(
        f.shared.lock().unwrap().clients[&f.client].high_watermark,
        1001
    );
}
#[test]
fn recovery_protection_blocks_commands_and_destination_registration() {
    let f = Fixture::new();
    f.shared.lock().unwrap().recovery_only = true;
    let blocked = f.execute("undo", json!({}));
    assert_eq!(blocked["code"], "E-EDITOR-RECOVERY");
    assert_eq!(blocked["sequence_consumed"], true);
    assert_eq!(
        call(
            &f.shared,
            "POST",
            "/api/destinations",
            Some(json!({"export_root_id":"export-root","relative_directory":"blocked"}))
        )
        .0,
        409
    );
    assert_eq!(call(&f.shared, "GET", "/api/session", None).0, 200);
    assert_eq!(call(&f.shared, "GET", "/api/jobs/unknown", None).0, 404);
    let recovery = f.execute(
        "recover",
        json!({"recovery_id":"unknown","action":"complete"}),
    );
    assert_eq!(recovery["operation_status"], 422);
    assert!(f.shared.lock().unwrap().recovery_only);
}

#[test]
fn inflight_jobs_leave_get_responsive_and_reserve_sequence_until_terminal() {
    let f = Fixture::new();
    let gate = Arc::new(std::sync::Barrier::new(2));
    f.shared.lock().unwrap().worker_gate = Some(gate.clone());
    let request = f.command("validate", json!({}));
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202);
    assert_eq!(f.send(request.clone()), (202, accepted.clone()));
    assert_eq!(call(&f.shared, "GET", "/api/session", None).0, 200);
    let (_, job) = call(
        &f.shared,
        "GET",
        &format!("/api/jobs/{}", accepted["job_id"].as_str().unwrap()),
        None,
    );
    assert_eq!(job["terminal"], false);
    assert_eq!(job["sequence_consumed"], false);
    let mut next = request.clone();
    next["client_sequence"] = "2".into();
    next["command_id"] = format!("{}:2", f.client).into();
    let (status, rejected) = f.send(next);
    assert_eq!(status, 409);
    assert_eq!(rejected["sequence_consumed"], false);
    let (_, second) = call(
        &f.shared,
        "POST",
        "/api/writer",
        Some(json!({"action":"register"})),
    );
    assert_eq!(call(&f.shared,"POST","/api/writer",Some(json!({"action":"claim","client_id":second["client_id"],"expected_writer_epoch":"1"}))).0,409);
    gate.wait();
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["next_sequence"], "2");
    assert_eq!(f.send(request).1, done);
}

#[test]
fn malformed_confirmation_payload_is_rejected_without_panicking_or_locking_session() {
    let f = Fixture::new();
    for payload in [json!([]), json!(null), json!("text")] {
        let c = f.shared.lock().unwrap();
        let request = json!({"client_id":f.client,"writer_epoch":c.writer_epoch.to_string(),"base_revision":c.revision().to_string(),"kind":"overwrite_project","payload":payload});
        drop(c);
        let (status, result) = call(&f.shared, "POST", "/api/confirmations", Some(request));
        assert_eq!(status, 400);
        assert_eq!(result["sequence_consumed"], false);
    }
    assert!(f.shared.lock().unwrap().busy.is_none());
    assert_eq!(f.execute("undo", json!({}))["next_sequence"], "2");
}

#[test]
fn protected_layout_replacement_needs_bound_ack_and_preserves_input_revision() {
    let f = Fixture::new();
    let path = f.root.join("input/models/demo/Main.ned.layout.json");
    fs::write(&path, "broken layout").unwrap();
    assert_eq!(f.execute("reload", json!({}))["operation_status"], 200);
    let revision = f
        .shared
        .lock()
        .unwrap()
        .model
        .as_ref()
        .unwrap()
        .input_revision;
    assert_eq!(
        f.execute(
            "set_layout",
            json!({"type_key":f.network(),"positions":{"a":{"x":-10,"y":20,"collapsed":false}}})
        )["operation_status"],
        200
    );
    assert_eq!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .input_revision,
        revision
    );
    let destination = f.source_target();
    let preview = f.confirm("overwrite_project", json!({"destination_id":destination}));
    assert_eq!(preview["requires_layout_confirmation"], true);
    let failure = f.execute(
        "overwrite_project",
        json!({"destination_id":destination,"overwrite_ack":preview["confirmation"]}),
    );
    assert_eq!(failure["code"], "E-EDITOR-LAYOUT-CONFIRMATION");
    assert_eq!(fs::read_to_string(&path).unwrap(), "broken layout");
    let layout = f.confirm("replace_layout", json!({"destination_id":destination}));
    let overwrite = f.confirm("overwrite_project", json!({"destination_id":destination}));
    let done=f.execute("overwrite_project",json!({"destination_id":destination,"overwrite_ack":overwrite["confirmation"],"replace_layout_ack":layout["confirmation"]}));
    assert_eq!(done["operation_status"], 200, "{done}");
    assert!(
        super::super::layout::LayoutDocument::parse(&fs::read_to_string(&path).unwrap()).is_ok()
    );
    assert!(!f.shared.lock().unwrap().model.as_ref().unwrap().dirty());
}

#[test]
fn startup_checks_recovery_before_missing_input_and_can_complete_without_model() {
    let f = Fixture::new();
    let destination = f.destination("recoverable");
    let saved = f.execute("save_as_project", json!({"destination_id":destination}));
    assert_eq!(saved["operation_status"], 200);
    let save_id = saved["result"]["save_id"].as_str().unwrap();
    // Recreate the persisted crash window after all files were published but before completion was recorded.
    let path = f
        .root
        .join("state/recovery")
        .join(save_id)
        .join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["state"] = "publishing".into();
    manifest["action"] = Value::Null;
    manifest["completed_target"] = Value::Null;
    fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    fs::remove_dir_all(f.root.join("input")).unwrap();
    let registry = TargetRegistry::open(&f.root.join("exports"), &f.root.join("state")).unwrap();
    let shared = Arc::new(Mutex::new(
        Controller::new(f.root.join("input/project.ini"), f.root.clone(), registry).unwrap(),
    ));
    {
        let c = shared.lock().unwrap();
        assert!(c.model.is_none());
        assert!(c.recovery_only);
        assert_eq!(c.recovery.len(), 1);
        assert!(c.diagnostics.is_empty());
    }
    let (_, writer) = call(
        &shared,
        "POST",
        "/api/writer",
        Some(json!({"action":"register"})),
    );
    let c = shared.lock().unwrap();
    let request = json!({"schema_version":1,"session_id":c.session_id,"client_id":writer["client_id"],"writer_epoch":writer["writer_epoch"],"client_sequence":"1","command_id":format!("{}:1",writer["client_id"].as_str().unwrap()),"base_revision":"0","kind":"recover","payload":{"recovery_id":save_id,"action":"complete"}});
    drop(c);
    let (status, accepted) = call(&shared, "POST", "/api/commands", Some(request));
    assert_eq!(status, 202);
    let done = terminal(&shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    let c = shared.lock().unwrap();
    assert!(c.model.is_none());
    assert!(!c.recovery_only);
    assert!(c.recovery.is_empty());
}
