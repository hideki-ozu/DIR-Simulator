use super::*;

fn creation() -> Value {
    json!({"operation":"create_file","operation_payload":{
        "role":"ned","root_id":"ned-root-0","relative_path":"demo/Acceptance.ned",
        "template_id":"ned.module_empty","declaration":{"kind":"module","local_name":"Acceptance"},
        "format":{"newline":"LF","bom":false}}})
}

fn observable(f: &Fixture) -> Value {
    let c = f.shared.lock().unwrap();
    let m = c.model.as_ref().unwrap();
    json!({"revision":m.revision,"input_revision":m.input_revision,
        "digest":m.project.input_digest(),"dirty":m.dirty(),"undo":m.can_undo(),"redo":m.can_redo(),
        "files":m.project.files.iter().map(|(id,f)|json!({"id":id,"path":f.path,"hash":f.hash,"text":f.text.as_ref()})).collect::<Vec<_>>()})
}

#[test]
fn xc20_writer_takeover_alone_invalidates_preview_without_editing_inputs() {
    let f = Fixture::new();
    let done = f.execute("preview_edit", creation());
    assert_eq!(done["operation_status"], 200, "{done}");
    let plan = done["result"]["plan_id"].as_str().unwrap();
    let before = observable(&f);
    let (_, reader) = call(
        &f.shared,
        "POST",
        "/api/writer",
        Some(json!({"action":"register"})),
    );
    let epoch = f.shared.lock().unwrap().writer_epoch.to_string();
    let (status, claimed) = call(
        &f.shared,
        "POST",
        "/api/writer",
        Some(json!({
        "action":"claim","client_id":reader["client_id"],"expected_writer_epoch":epoch})),
    );
    assert_eq!(status, 200, "{claimed}");
    for (client, epoch, expected) in [
        (f.client.clone(), epoch, 403),
        (
            reader["client_id"].as_str().unwrap().to_owned(),
            claimed["writer_epoch"].as_str().unwrap().to_owned(),
            409,
        ),
    ] {
        let query = BTreeMap::from([("client_id".into(), client), ("writer_epoch".into(), epoch)]);
        let (status, response) = Controller::handle(
            &f.shared,
            "GET",
            &format!("/api/edit-plans/{plan}"),
            &query,
            None,
        );
        assert_eq!(status, expected, "{response}");
    }
    assert_eq!(observable(&f), before);
    assert!(!f.root.join("input/models/demo/Acceptance.ned").exists());
}

#[test]
fn xc32_exact_hundred_inflight_and_terminal_retries_preserve_one_frozen_run() {
    let f = Fixture::new();
    let before = observable(&f);
    let gate = Arc::new(std::sync::Barrier::new(2));
    f.shared.lock().unwrap().worker_gate = Some(gate.clone());
    let request = f.command("start_run", json!({}));
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202, "{accepted}");
    for _ in 0..100 {
        assert_eq!(f.send(request.clone()), (202, accepted.clone()));
    }
    let mut different = request.clone();
    different["payload"] = json!({"wall_timeout_ms":"1000"});
    let conflict = f.send(different);
    assert_eq!(conflict.0, 409, "{conflict:?}");
    // The reserved client sequence blocks both edits and run-management actions.
    for (kind, payload) in [
        ("start_run", json!({})),
        ("undo", json!({})),
        ("redo", json!({})),
        ("validate", json!({})),
        ("preview_edit", creation()),
        ("cancel_run", json!({"run_id":"pending"})),
        ("build_viewer", json!({"run_id":"pending"})),
        ("revoke_viewer", json!({"run_id":"pending"})),
        ("delete_run", json!({"run_id":"pending"})),
    ] {
        let mut next = request.clone();
        next["client_sequence"] = "2".into();
        next["command_id"] = format!("{}:2", f.client).into();
        next["kind"] = kind.into();
        next["payload"] = payload;
        let rejected = f.send(next);
        assert_eq!(rejected.0, 409, "{kind}: {rejected:?}");
        assert_eq!(rejected.1["sequence_consumed"], false, "{kind}");
    }
    assert_eq!(
        f.shared.lock().unwrap().clients[&f.client].high_watermark,
        0
    );
    gate.wait();
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    for _ in 0..100 {
        assert_eq!(f.send(request.clone()).1, done);
    }
    assert_eq!(done["next_sequence"], "2");
    assert_eq!(
        call(&f.shared, "GET", "/api/runs", None).1["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(observable(&f), before);
    let id = done["result"]["run_id"].as_str().unwrap();
    assert_eq!(
        f.execute("cancel_run", json!({"run_id":id}))["operation_status"],
        200
    );
}

#[test]
fn xc20_actual_reload_invalidates_same_bytes_preview_before_apply() {
    let f = Fixture::new();
    let done = f.execute("preview_edit", creation());
    assert_eq!(done["operation_status"], 200, "{done}");
    let payload =
        json!({"plan_id":done["result"]["plan_id"],"plan_digest":done["result"]["plan_digest"]});
    let old_id = f
        .shared
        .lock()
        .unwrap()
        .model
        .as_ref()
        .unwrap()
        .project
        .id
        .clone();
    assert_eq!(f.execute("reload", json!({}))["operation_status"], 200);
    assert_ne!(
        f.shared.lock().unwrap().model.as_ref().unwrap().project.id,
        old_id
    );
    let before = observable(&f);
    let rejected = f.execute("apply_edit_plan", payload);
    assert_eq!(rejected["operation_status"], 409, "{rejected}");
    assert_eq!(rejected["code"], "E-EDITOR-REVISION");
    assert_eq!(observable(&f), before);
}

#[test]
fn xc22_actual_preview_and_apply_candidate_panics_never_adopt_or_destroy_redo() {
    for kind in ["preview_edit", "apply_edit_plan"] {
        let f = Fixture::new();
        let original = f.source().2;
        assert_eq!(
            f.edit(&(original.clone() + "\n// earlier edit\n"))["operation_status"],
            200
        );
        assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
        assert!(f.shared.lock().unwrap().model.as_ref().unwrap().can_redo());
        let mut request = if kind == "apply_edit_plan" {
            let plan = f.execute("preview_edit", creation());
            assert_eq!(plan["operation_status"], 200, "{plan}");
            let payload = json!({"plan_id":plan["result"]["plan_id"],"plan_digest":plan["result"]["plan_digest"]});
            let confirmation = f.confirm(kind, payload.clone());
            let mut request = f.command(kind, payload);
            request["impact_ack"] = confirmation["confirmation"].clone();
            request
        } else {
            f.command(kind, creation())
        };
        let before = observable(&f);
        let disk = fs::read(f.root.join("input/models/demo/Main.ned")).unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        {
            let mut c = f.shared.lock().unwrap();
            c.panic_after_candidate = true;
            c.worker_gate = Some(gate.clone());
        }
        let (status, accepted) = f.send(request.clone());
        assert_eq!(status, 202, "{accepted}");
        assert_eq!(observable(&f), before);
        gate.wait();
        let done = terminal(&f.shared, &accepted);
        assert_eq!(done["operation_status"], 500, "{done}");
        assert_eq!(done["terminal"], true);
        assert_eq!(done["sequence_consumed"], true);
        assert!(f.shared.lock().unwrap().busy.is_none());
        assert_eq!(observable(&f), before);
        assert_eq!(
            fs::read(f.root.join("input/models/demo/Main.ned")).unwrap(),
            disk
        );
        assert!(!f.root.join("input/models/demo/Acceptance.ned").exists());
        assert_eq!(f.send(std::mem::take(&mut request)).1, done);
        assert_eq!(f.execute("validate", json!({}))["operation_status"], 200);
        assert_eq!(f.execute("redo", json!({}))["operation_status"], 200);
        assert!(f.source().2.ends_with("// earlier edit\n"));
    }
}
