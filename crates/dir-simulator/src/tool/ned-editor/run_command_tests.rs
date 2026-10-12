use super::*;

fn get(f: &Fixture, path: &str) -> (u16, Value) {
    call(&f.shared, "GET", path, None)
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
