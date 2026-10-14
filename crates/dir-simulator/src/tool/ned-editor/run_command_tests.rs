use super::*;

fn get(f: &Fixture, path: &str) -> (u16, Value) {
    call(&f.shared, "GET", path, None)
}

fn assert_run_completion_preserves_current_diagnostics(outcome: &str) {
    for kind in [
        "start_run",
        "cancel_run",
        "build_viewer",
        "delete_run",
        "revoke_viewer",
    ] {
        let f = Fixture::new();
        let request = f.command(
            kind,
            if kind == "start_run" {
                json!({})
            } else {
                json!({"run_id":"run-fixture"})
            },
        );
        let command = Command::parse(&request).unwrap();
        let mut c = f.shared.lock().unwrap();
        let stamp = Stamp::of(c.model.as_ref().unwrap());
        c.diagnostics = ["semantic", "io", "load"]
            .into_iter()
            .map(|origin| {
                diagnostic(
                    origin,
                    &EditorError::new(
                        &format!("E-CURRENT-{origin}"),
                        format!("current {origin} diagnostic"),
                        422,
                    ),
                    stamp.input_revision,
                )
            })
            .collect();
        c.model.as_mut().unwrap().diagnostics.push(diagnostic(
            "semantic",
            &EditorError::new("E-CURRENT-MODEL", "current model semantic diagnostic", 422),
            stamp.input_revision,
        ));
        let controller_before = c.diagnostics.clone();
        let model_before = c.model.as_ref().unwrap().diagnostics.clone();
        let view_before =
            super::super::super::view::project(&c, &BTreeMap::new())["diagnostics"].clone();
        let job = "job-run-completion".to_owned();
        c.ledger.insert(
            command.id.clone(),
            LedgerEntry {
                request,
                response: json!({"operation_status":202}),
                job: Some(job.clone()),
            },
        );
        c.clients.get_mut(&f.client).unwrap().inflight = Some(command.id.clone());
        c.busy = Some(kind.into());
        let result = json!({"status":"run_result","run_id":"run-fixture"});
        let completed = match outcome {
            "success" => Ok(Ok(Work::RunResult(result.clone()))),
            "failure" => Ok(Err(EditorError::new(
                "E-HISTORICAL-RUN",
                "historical run operation failed",
                422,
            ))),
            "panic" => Err(Box::new("run operation worker panic") as Box<dyn std::any::Any + Send>),
            _ => unreachable!(),
        };
        c.complete_operation(&command, Some(&stamp), completed);
        assert_eq!(
            c.diagnostics, controller_before,
            "{kind}/{outcome}: controller diagnostics changed"
        );
        assert_eq!(
            c.model.as_ref().unwrap().diagnostics,
            model_before,
            "{kind}/{outcome}: model diagnostics changed"
        );
        assert_eq!(
            super::super::super::view::project(&c, &BTreeMap::new())["diagnostics"],
            view_before,
            "{kind}/{outcome}: current view diagnostics changed"
        );
        let response = &c.ledger[&command.id].response;
        let expected_status = match outcome {
            "success" => 200,
            "failure" => 422,
            _ => 500,
        };
        assert_eq!(response["operation_status"], expected_status);
        assert_eq!(response["terminal"], true);
        assert_eq!(response["sequence_consumed"], true);
        assert_eq!(response["command_id"], command.id);
        assert_eq!(response["job_id"], job);
        if outcome == "success" {
            assert_eq!(response["result"], result);
        } else {
            assert_eq!(
                response["code"],
                if outcome == "failure" {
                    "E-HISTORICAL-RUN"
                } else {
                    "E-EDITOR-WORKER"
                }
            );
            assert_eq!(
                response["message"],
                if outcome == "failure" {
                    "historical run operation failed"
                } else {
                    "Operation worker failed"
                }
            );
        }
        assert_eq!(c.jobs[&job], *response);
        assert_eq!(c.clients[&f.client].high_watermark, command.sequence);
        assert!(c.clients[&f.client].inflight.is_none());
        assert!(c.busy.is_none());
        assert!(!c.recovery_only);
    }
}

#[test]
fn run_command_completion_success_preserves_current_diagnostics() {
    assert_run_completion_preserves_current_diagnostics("success");
}

#[test]
fn run_command_completion_failure_preserves_current_diagnostics() {
    assert_run_completion_preserves_current_diagnostics("failure");
}

#[test]
fn run_command_completion_panic_preserves_current_diagnostics() {
    assert_run_completion_preserves_current_diagnostics("panic");
}

#[test]
fn run_command_freezes_dirty_input_once_and_preserves_history_and_checkpoint() {
    let f = Fixture::new();
    let view = get(&f, "/api/session").1;
    assert_eq!(
        view["capabilities"]["detach_instance_type"]["enabled"],
        true
    );
    assert_eq!(view["capabilities"]["rename_symbols"]["enabled"], true);
    let (file_id, _, text) = f.source();
    let edited = format!("{text}\n// unsaved run input\n");
    assert_eq!(f.edit(&edited)["operation_status"], 200);
    let before = {
        let c = f.shared.lock().unwrap();
        let m = c.model.as_ref().unwrap();
        (
            m.revision,
            m.input_revision,
            m.project_instance_id.clone(),
            m.project.id.clone(),
            m.project.input_digest(),
            m.dirty(),
            m.can_undo(),
            m.can_redo(),
        )
    };
    let request = f.command("start_run", json!({}));
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202, "{accepted}");
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    assert_eq!(done["result"]["status"], "run_created");
    let id = done["result"]["run_id"].as_str().unwrap();
    for _ in 0..100 {
        assert_eq!(f.send(request.clone()).1, done);
    }
    assert_eq!(get(&f, "/api/runs").1["runs"].as_array().unwrap().len(), 1);
    let frozen = get(&f, &format!("/api/runs/{id}/input/{file_id}"));
    assert_eq!(frozen.0, 200, "{:?}", frozen.1);
    assert_eq!(frozen.1["text"], edited);
    assert_eq!(frozen.1["source_stamp"]["project_instance_id"], before.2);
    assert_eq!(frozen.1["source_stamp"]["snapshot_id"], before.3);
    assert_eq!(frozen.1["source_stamp"]["input_digest"], before.4);
    assert_eq!(frozen.1["source_stamp"]["dirty_at_start"], true);
    {
        let c = f.shared.lock().unwrap();
        let m = c.model.as_ref().unwrap();
        assert_eq!(
            (
                m.revision,
                m.input_revision,
                m.project_instance_id.clone(),
                m.project.id.clone(),
                m.project.input_digest(),
                m.dirty(),
                m.can_undo(),
                m.can_redo()
            ),
            before
        );
    }
    let mut conflicting = request;
    conflicting["payload"] = json!({"wall_timeout_ms":"1000"});
    assert_eq!(f.send(conflicting).0, 409);
    assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    assert_eq!(
        get(&f, &format!("/api/runs/{id}/input/{file_id}")).1["text"],
        edited
    );
    assert_eq!(
        get(&f, &format!("/api/runs/{id}/inputs")).1["files"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        f.execute("cancel_run", json!({"run_id":id}))["operation_status"],
        200
    );
    assert_eq!(get(&f, &format!("/api/runs/{id}")).1["phase"], "cancelled");
}

#[test]
fn run_capacity_rejection_does_not_consume_sequence_or_create_run() {
    let f = Fixture::new();
    for _ in 0..2 {
        assert_eq!(f.execute("start_run", json!({}))["operation_status"], 200);
    }
    let request = f.command("start_run", json!({}));
    let high = f.shared.lock().unwrap().clients[&f.client].high_watermark;
    let rejected = f.send(request.clone());
    assert_eq!(rejected.0, 503, "{:?}", rejected.1);
    assert_eq!(rejected.1["sequence_consumed"], false);
    {
        let c = f.shared.lock().unwrap();
        assert_eq!(c.clients[&f.client].high_watermark, high);
        assert!(
            !c.ledger
                .contains_key(request["command_id"].as_str().unwrap())
        );
    }
    let list = get(&f, "/api/runs").1;
    assert_eq!(list["runs"].as_array().unwrap().len(), 2);
    let id = list["runs"][0]["run_id"].as_str().unwrap();
    f.shared.lock().unwrap().runs.cancel(id).unwrap();
    let (status, accepted) = f.send(request);
    assert_eq!(status, 202, "{accepted}");
    assert_eq!(terminal(&f.shared, &accepted)["operation_status"], 200);
}

#[test]
fn run_frozen_source_survives_later_edit_reload_new_project_and_creation() {
    let f = Fixture::new();
    let (file_id, _, original) = f.source();
    let frozen_text = format!("{original}\n// original run bytes\n");
    assert_eq!(f.edit(&frozen_text)["operation_status"], 200);
    let created = f.execute("start_run", json!({}));
    assert_eq!(created["operation_status"], 200, "{created}");
    let run_id = created["result"]["run_id"].as_str().unwrap();
    let frozen_path = format!("/api/runs/{run_id}/input/{file_id}");
    let stamp = get(&f, &frozen_path).1["source_stamp"].clone();
    let assert_frozen = || {
        let response = get(&f, &frozen_path);
        assert_eq!(response.0, 200);
        assert_eq!(response.1["text"], frozen_text);
        assert_eq!(response.1["source_stamp"], stamp);
    };
    assert_eq!(
        f.edit(&format!("{original}\n// later edit\n"))["operation_status"],
        200
    );
    assert_frozen();
    assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    assert_frozen();
    let confirmation = f.confirm("reload", json!({}));
    assert_eq!(
        f.execute(
            "reload",
            json!({"discard_ack":confirmation["confirmation"]})
        )["operation_status"],
        200
    );
    assert_frozen();
    let mut payload = json!({"template":"multibus","project_name":"DifferentCurrentProject"});
    let confirmation = f.confirm("new_project", payload.clone());
    payload["discard_ack"] = confirmation["confirmation"].clone();
    assert_eq!(f.execute("new_project", payload)["operation_status"], 200);
    let preview = f.execute(
        "preview_edit",
        json!({"operation":"create_file","operation_payload":{
            "role":"ned","root_id":"ned-root-0","relative_path":"pkg/Extra.ned",
            "template_id":"ned.module_empty","declaration":{"kind":"module","local_name":"Extra"},
            "format":{"newline":"LF","bom":false}
        }}),
    );
    assert_eq!(preview["operation_status"], 200, "{preview}");
    let apply = json!({"plan_id":preview["result"]["plan_id"],"plan_digest":preview["result"]["plan_digest"]});
    let confirmation = f.confirm("apply_edit_plan", apply.clone());
    let mut request = f.command("apply_edit_plan", apply);
    request["impact_ack"] = confirmation["confirmation"].clone();
    let (status, accepted) = f.send(request);
    assert!(status == 200 || status == 202, "{accepted}");
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    assert_frozen();
    assert!(!f.root.join("input/models/pkg/Extra.ned").exists());
    assert_eq!(
        fs::read_to_string(f.root.join("input/models/demo/Main.ned")).unwrap(),
        original
    );
}

#[test]
fn invalid_run_payload_is_unconsumed_and_leaves_model_and_run_inventory_unchanged() {
    let f = Fixture::new();
    let before = get(&f, "/api/session").1;
    let sequence = f.shared.lock().unwrap().clients[&f.client].high_watermark;
    for payload in [
        json!({"wall_timeout_ms":"999"}),
        json!({"wall_timeout_ms":"3600001"}),
        json!({"wall_timeout_ms":1000}),
        json!({"wall_timeout_ms":"01000"}),
        json!({"unexpected":true}),
    ] {
        let (status, response) = f.send(f.command("start_run", payload.clone()));
        assert_eq!(status, 400, "{payload}: {response}");
        assert_eq!(response["sequence_consumed"], false);
        assert_eq!(
            f.shared.lock().unwrap().clients[&f.client].high_watermark,
            sequence
        );
        let now = get(&f, "/api/session").1;
        assert_eq!(now["revision"], before["revision"]);
        assert_eq!(now["input_revision"], before["input_revision"]);
        assert_eq!(now["dirty"], before["dirty"]);
        assert!(
            get(&f, "/api/runs").1["runs"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn project_generation_changes_only_on_successful_replacement() {
    let f = Fixture::new();
    let before = f
        .shared
        .lock()
        .unwrap()
        .model
        .as_ref()
        .unwrap()
        .project_instance_id
        .clone();
    let (_, _, text) = f.source();
    f.edit(&format!("{text}\n// edit"));
    f.execute("undo", json!({}));
    assert_eq!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .project_instance_id,
        before
    );
    let payload = json!({"template":"can","project_name":"NewGeneration"});
    let ack = f.confirm("new_project", payload.clone());
    let mut payload = payload;
    payload["discard_ack"] = ack["confirmation"].clone();
    assert_eq!(f.execute("new_project", payload)["operation_status"], 200);
    assert_ne!(
        f.shared
            .lock()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .project_instance_id,
        before
    );
}

#[test]
fn failed_publication_terminal_replay_allows_new_http_command_after_durable_repair() {
    let f = Fixture::new();
    f.shared.lock().unwrap().runs.fail_next_publication_sync();
    let request = f.command("start_run", json!({}));
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202);
    let failed = terminal(&f.shared, &accepted);
    assert_ne!(failed["operation_status"], 200);
    for _ in 0..5 {
        assert_eq!(f.send(request.clone()).1, failed);
    }
    let fresh = f.execute("start_run", json!({}));
    assert_eq!(fresh["operation_status"], 200, "{fresh}");
    let rows = get(&f, "/api/runs").1;
    assert_eq!(rows["runs"].as_array().unwrap().len(), 2);
    assert_eq!(f.send(request).1, failed);
    assert_eq!(get(&f, "/api/runs").1["runs"].as_array().unwrap().len(), 2);
}
