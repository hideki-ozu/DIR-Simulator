use super::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

fn create_file_payload() -> Value {
    json!({
        "operation":"create_file",
        "operation_payload":{
            "role":"ned",
            "root_id":"ned-root-0",
            "relative_path":"demo/Extension.ned",
            "template_id":"ned.module_empty",
            "declaration":{"kind":"module","local_name":"Extension"},
            "format":{"newline":"LF","bom":false}
        }
    })
}

fn preview(f: &Fixture) -> Value {
    let request = f.command("preview_edit", create_file_payload());
    let (status, accepted) = f.send(request);
    assert_eq!(status, 202, "{accepted}");
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    done
}

fn plan_query(client: &str, epoch: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("client_id".into(), client.into()),
        ("writer_epoch".into(), epoch.into()),
    ])
}

fn plan_page(f: &Fixture, id: &str, query: BTreeMap<String, String>) -> (u16, Value) {
    Controller::handle(
        &f.shared,
        "GET",
        &format!("/api/edit-plans/{id}"),
        &query,
        None,
    )
}

fn current_plan_page(f: &Fixture, id: &str) -> (u16, Value) {
    let epoch = f.shared.lock().unwrap().writer_epoch.to_string();
    plan_page(f, id, plan_query(&f.client, &epoch))
}

fn apply_confirmation(f: &Fixture, payload: Value) -> (u16, Value) {
    let c = f.shared.lock().unwrap();
    let request = json!({
        "client_id":f.client,
        "writer_epoch":c.writer_epoch.to_string(),
        "base_revision":c.revision().to_string(),
        "kind":"apply_edit_plan",
        "payload":payload
    });
    drop(c);
    call(&f.shared, "POST", "/api/confirmations", Some(request))
}

fn disk_snapshot(root: &Path) -> (BTreeSet<PathBuf>, BTreeMap<PathBuf, Vec<u8>>) {
    fn walk(
        root: &Path,
        current: &Path,
        directories: &mut BTreeSet<PathBuf>,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                directories.insert(relative);
                walk(root, &path, directories, files);
            } else if kind.is_file() {
                files.insert(relative, fs::read(path).unwrap());
            }
        }
    }

    let mut directories = BTreeSet::new();
    let mut files = BTreeMap::new();
    walk(root, root, &mut directories, &mut files);
    (directories, files)
}

#[derive(Debug, PartialEq, Eq)]
struct ModelSnapshot {
    revision: u64,
    input_revision: u64,
    project_id: String,
    input_digest: String,
    dirty: bool,
    can_undo: bool,
    can_redo: bool,
    state: String,
}

fn model_snapshot(f: &Fixture) -> ModelSnapshot {
    let c = f.shared.lock().unwrap();
    let m = c.model.as_ref().unwrap();
    ModelSnapshot {
        revision: m.revision,
        input_revision: m.input_revision,
        project_id: m.project.id.clone(),
        input_digest: m.project.input_digest(),
        dirty: m.dirty(),
        can_undo: m.can_undo(),
        can_redo: m.can_redo(),
        state: format!(
            "{:?}",
            (
                &m.project.files,
                &m.project.metadata,
                &m.project.directories,
                &m.project.layouts,
                &m.project.parsed,
                &m.syntax,
                &m.parsed_hashes,
                &m.analysis,
                &m.diagnostics
            )
        ),
    }
}

fn confirmed_apply(f: &Fixture, plan_id: &str, digest: &str) -> (Value, Value) {
    let payload = json!({"plan_id":plan_id,"plan_digest":digest});
    let (status, confirmation) = apply_confirmation(f, payload.clone());
    assert_eq!(status, 200, "{confirmation}");
    assert_eq!(confirmation["plan_digest"], digest);

    let mut request = f.command("apply_edit_plan", payload);
    request["impact_ack"] = confirmation["confirmation"].clone();
    let (status, accepted) = f.send(request.clone());
    assert_eq!(status, 202, "{accepted}");
    let done = terminal(&f.shared, &accepted);
    assert_eq!(done["operation_status"], 200, "{done}");
    (request, done)
}

#[test]
fn preview_edit_is_read_only_and_page_contains_source_diff_id_and_hashes() {
    let f = Fixture::new();
    let before_model = model_snapshot(&f);
    let before_disk = disk_snapshot(&f.root);
    let source_before = f.source();

    let done = preview(&f);

    assert_eq!(model_snapshot(&f), before_model);
    assert_eq!(disk_snapshot(&f.root), before_disk);
    assert_eq!(f.source(), source_before);

    let result = &done["result"];
    let plan_id = result["plan_id"].as_str().unwrap();
    let digest = result["plan_digest"].as_str().unwrap();
    assert!(plan_id.starts_with("plan-"));
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );

    let (status, page) = current_plan_page(&f, plan_id);
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["plan_id"], plan_id);
    assert_eq!(page["plan_digest"], digest);
    assert_eq!(page["base_revision"], before_model.revision.to_string());
    assert_eq!(
        page["input_revision"],
        before_model.input_revision.to_string()
    );
    assert_eq!(page["input_digest"], before_model.input_digest);
    assert_eq!(page["operation"], "create_file");
    assert_eq!(page["impacts"].as_array().unwrap().len(), 1);

    let impact = &page["impacts"][0];
    assert_eq!(impact["kind"], "added_file");
    assert_eq!(impact["before"], Value::Null);
    assert_eq!(
        impact["path"],
        f.root
            .join("input/models/demo/Extension.ned")
            .to_string_lossy()
            .as_ref()
    );
    let file_id = impact["file_id"].as_str().unwrap();
    let source = impact["after"].as_str().unwrap();
    let expected_hash = super::super::super::hash(source.as_bytes());
    assert!(source.contains("package demo;"));
    assert!(source.contains("module Extension {}"));
    assert_eq!(impact["after_hash"], expected_hash);
    assert_eq!(page["added_files"][0]["id"], file_id);
    assert_eq!(page["added_files"][0]["hash"], expected_hash);
}

#[test]
fn apply_requires_matching_confirmation_and_rejects_missing_wrong_and_stale_plan() {
    let f = Fixture::new();
    let done = preview(&f);
    let plan_id = done["result"]["plan_id"].as_str().unwrap().to_owned();
    let digest = done["result"]["plan_digest"].as_str().unwrap().to_owned();
    let before = model_snapshot(&f);

    let (status, missing_ack) = f.send(f.command(
        "apply_edit_plan",
        json!({"plan_id":plan_id,"plan_digest":digest}),
    ));
    assert_eq!(status, 409, "{missing_ack}");
    assert_eq!(missing_ack["operation_status"], 409);
    assert_eq!(missing_ack["code"], "E-EDITOR-CONFIRMATION");
    assert_eq!(model_snapshot(&f), before);

    let wrong_digest = format!(
        "{}{}",
        if digest.starts_with('0') { '1' } else { '0' },
        &digest[1..]
    );
    let (status, rejected_confirmation) =
        apply_confirmation(&f, json!({"plan_id":plan_id,"plan_digest":wrong_digest}));
    assert_eq!(status, 409, "{rejected_confirmation}");
    assert_eq!(rejected_confirmation["code"], "E-EDITOR-REVISION");

    let (status, confirmation) =
        apply_confirmation(&f, json!({"plan_id":plan_id,"plan_digest":digest}));
    assert_eq!(status, 200, "{confirmation}");
    let mut wrong_apply_request = f.command(
        "apply_edit_plan",
        json!({"plan_id":plan_id,"plan_digest":wrong_digest}),
    );
    wrong_apply_request["impact_ack"] = confirmation["confirmation"].clone();
    let (status, wrong_apply) = f.send(wrong_apply_request);
    assert_eq!(status, 409, "{wrong_apply}");
    assert_eq!(wrong_apply["operation_status"], 409);
    assert_eq!(wrong_apply["code"], "E-EDITOR-REVISION");
    assert_eq!(model_snapshot(&f), before);

    let mut source = f.source().2;
    source.push_str("\n// revision changed after preview\n");
    assert_eq!(f.edit(&source)["operation_status"], 200);
    let (status, stale_confirmation) =
        apply_confirmation(&f, json!({"plan_id":plan_id,"plan_digest":digest}));
    assert_eq!(status, 409, "{stale_confirmation}");
    assert_eq!(stale_confirmation["code"], "E-EDITOR-REVISION");
    assert!(!f.root.join("input/models/demo/Extension.ned").exists());
}

#[test]
fn apply_is_ledger_idempotent_and_undo_redo_reuses_added_file_id_and_layout() {
    let f = Fixture::new();
    let done = preview(&f);
    let plan_id = done["result"]["plan_id"].as_str().unwrap();
    let digest = done["result"]["plan_digest"].as_str().unwrap();
    let (status, page) = current_plan_page(&f, plan_id);
    assert_eq!(status, 200, "{page}");
    let file_id = page["added_files"][0]["id"].as_str().unwrap().to_owned();
    let file_hash = page["added_files"][0]["hash"].as_str().unwrap().to_owned();
    let expected_path = f.root.join("input/models/demo/Extension.ned");
    let expected_layout = f.root.join("input/models/demo/Extension.ned.layout.json");

    let (request, applied) = confirmed_apply(&f, plan_id, digest);
    assert_eq!(applied["applied_revision"], "2");
    assert_eq!(applied["result"]["status"], "edit_applied");
    assert_eq!(applied["result"]["plan_id"], plan_id);
    assert_eq!(f.send(request.clone()), (200, applied.clone()));
    {
        let c = f.shared.lock().unwrap();
        let m = c.model.as_ref().unwrap();
        assert_eq!(m.revision, 2);
        assert_eq!(m.input_revision, 2);
        assert!(m.dirty());
        assert!(m.can_undo());
        assert!(!m.can_redo());
        let file = m.project.files.get(&file_id).unwrap();
        assert_eq!(file.path, expected_path);
        assert_eq!(file.hash, file_hash);
        let layout = m.project.layouts.get(&file_id).unwrap();
        assert_eq!(layout.path, expected_layout);
        assert!(layout.adopted.is_none());
    }

    let reapplied = f.execute(
        "apply_edit_plan",
        json!({"plan_id":plan_id,"plan_digest":digest}),
    );
    assert_eq!(reapplied["operation_status"], 409);
    assert_eq!(reapplied["code"], "E-EDITOR-PLAN-APPLIED");

    assert_eq!(f.execute("undo", json!({}))["operation_status"], 200);
    {
        let c = f.shared.lock().unwrap();
        let m = c.model.as_ref().unwrap();
        assert_eq!(m.revision, 3);
        assert!(!m.project.files.contains_key(&file_id));
        assert!(!m.project.layouts.contains_key(&file_id));
        assert!(!m.can_undo());
        assert!(m.can_redo());
    }
    assert_eq!(f.execute("redo", json!({}))["operation_status"], 200);
    {
        let c = f.shared.lock().unwrap();
        let m = c.model.as_ref().unwrap();
        assert_eq!(m.revision, 4);
        assert_eq!(m.project.files[&file_id].path, expected_path);
        assert_eq!(m.project.files[&file_id].hash, file_hash);
        assert_eq!(m.project.layouts[&file_id].path, expected_layout);
        assert!(m.project.layouts[&file_id].adopted.is_none());
        assert!(m.can_undo());
        assert!(!m.can_redo());
    }
    assert!(!expected_path.exists());
    assert!(!expected_layout.exists());
}

#[test]
fn expired_and_replaced_edit_plans_return_gone_without_sleeping() {
    let f = Fixture::new();
    let first = preview(&f);
    let old_id = first["result"]["plan_id"].as_str().unwrap().to_owned();
    {
        let mut c = f.shared.lock().unwrap();
        c.edit_plan.as_mut().unwrap().created = Instant::now() - super::super::PLAN_TTL;
    }
    let (status, expired) = current_plan_page(&f, &old_id);
    assert_eq!(status, 410, "{expired}");
    assert_eq!(expired["code"], "E-EDITOR-EXPIRED");

    let second = preview(&f);
    let new_id = second["result"]["plan_id"].as_str().unwrap();
    assert_ne!(new_id, old_id);
    let (status, replaced) = current_plan_page(&f, &old_id);
    assert_eq!(status, 410, "{replaced}");
    assert_eq!(replaced["code"], "E-EDITOR-EXPIRED");
    let (status, current) = current_plan_page(&f, new_id);
    assert_eq!(status, 200, "{current}");
    assert_eq!(current["plan_id"], new_id);
}

#[test]
fn plan_reads_enforce_writer_and_current_writer_sees_stale_revision_conflict() {
    let f = Fixture::new();
    let done = preview(&f);
    let plan_id = done["result"]["plan_id"].as_str().unwrap().to_owned();
    let epoch = f.shared.lock().unwrap().writer_epoch.to_string();

    let (status, reader) = call(
        &f.shared,
        "POST",
        "/api/writer",
        Some(json!({"action":"register"})),
    );
    assert_eq!(status, 200);
    let reader_id = reader["client_id"].as_str().unwrap();
    let (status, readonly) = plan_page(&f, &plan_id, plan_query(reader_id, &epoch));
    assert_eq!(status, 403, "{readonly}");
    assert_eq!(readonly["code"], "E-EDITOR-WRITER");

    let mut source = f.source().2;
    source.push_str("\n// stale plan\n");
    assert_eq!(f.edit(&source)["operation_status"], 200);
    let (status, stale) = current_plan_page(&f, &plan_id);
    assert_eq!(status, 409, "{stale}");
    assert_eq!(stale["code"], "E-EDITOR-REVISION");

    let (status, claimed) = call(
        &f.shared,
        "POST",
        "/api/writer",
        Some(json!({
            "action":"claim",
            "client_id":reader_id,
            "expected_writer_epoch":epoch
        })),
    );
    assert_eq!(status, 200, "{claimed}");
    let new_epoch = claimed["writer_epoch"].as_str().unwrap();
    let old_writer = plan_query(&f.client, &epoch);
    let (status, rejected_old_epoch) = plan_page(&f, &plan_id, old_writer);
    assert_eq!(status, 403, "{rejected_old_epoch}");
    assert_eq!(rejected_old_epoch["code"], "E-EDITOR-WRITER");
    let (status, rejected_stale_plan) = plan_page(&f, &plan_id, plan_query(reader_id, new_epoch));
    assert_eq!(status, 409, "{rejected_stale_plan}");
    assert_eq!(rejected_stale_plan["code"], "E-EDITOR-REVISION");
}

#[test]
fn preview_job_reserves_sequence_until_terminal() {
    let f = Fixture::new();
    let gate = Arc::new(std::sync::Barrier::new(2));
    f.shared.lock().unwrap().worker_gate = Some(gate.clone());
    let request = f.command("preview_edit", create_file_payload());
    let (status, accepted) = f.send(request.clone());
    let (_, pending) = call(
        &f.shared,
        "GET",
        &format!("/api/jobs/{}", accepted["job_id"].as_str().unwrap()),
        None,
    );
    let mut next = request.clone();
    next["client_sequence"] = "2".into();
    next["command_id"] = format!("{}:2", f.client).into();
    next["kind"] = "undo".into();
    next["payload"] = json!({});
    let (overtake_status, overtake) = f.send(next);
    gate.wait();
    let done = terminal(&f.shared, &accepted);

    assert_eq!(status, 202, "{accepted}");
    assert_eq!(pending["terminal"], false);
    assert_eq!(pending["sequence_consumed"], false);
    assert_eq!(overtake_status, 409, "{overtake}");
    assert_eq!(overtake["sequence_consumed"], false);
    assert_eq!(done["operation_status"], 200, "{done}");
    assert_eq!(done["next_sequence"], "2");
    assert_eq!(f.send(request).1, done);
}

#[test]
fn bad_plan_cursors_and_unknown_query_fields_are_400_and_preserve_source() {
    let f = Fixture::new();
    let source_before = f.source();
    let disk_before = disk_snapshot(&f.root);
    let done = preview(&f);
    let plan_id = done["result"]["plan_id"].as_str().unwrap();
    let epoch = f.shared.lock().unwrap().writer_epoch.to_string();
    let query = plan_query(&f.client, &epoch);

    let mut malformed_cursor = query.clone();
    malformed_cursor.insert("cursor".into(), "not-a-cursor".into());
    let (status, response) = plan_page(&f, plan_id, malformed_cursor);
    assert_eq!(status, 400, "{response}");

    let mut wrong_digest_cursor = query.clone();
    wrong_digest_cursor.insert("cursor".into(), "wrong-digest:0".into());
    let (status, response) = plan_page(&f, plan_id, wrong_digest_cursor);
    assert_eq!(status, 400, "{response}");

    let mut unknown_field = query;
    unknown_field.insert("unexpected".into(), "1".into());
    let (status, response) = plan_page(&f, plan_id, unknown_field);
    assert_eq!(status, 400, "{response}");

    assert_eq!(f.source(), source_before);
    assert_eq!(disk_snapshot(&f.root), disk_before);
}
