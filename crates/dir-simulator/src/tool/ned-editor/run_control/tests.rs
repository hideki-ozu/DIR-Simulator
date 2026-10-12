use super::*;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
#[path = "safety_tests.rs"]
mod safety_tests;
struct Fixture {
    root: PathBuf,
    project: ProjectSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(random_id("ned-run-test-").unwrap());
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let source = root.join("source");
        fs::create_dir_all(source.join("models/demo")).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (from, to) in [
            ("examples/can/baseline.ini", "project.ini"),
            ("examples/can/baseline.json", "baseline.json"),
            ("examples/can/models/demo/Main.ned", "models/demo/Main.ned"),
        ] {
            fs::copy(repo.join(from), source.join(to)).unwrap();
        }
        let project =
            super::super::input::load_project(&source.join("project.ini"), &source).unwrap();
        Self { root, project }
    }
    fn manager(&self) -> RunManager {
        RunManager::open(&self.root.join("state"), &self.project.input_regions()).unwrap()
    }
    fn stamp(&self, command: &str) -> RunStamp {
        RunStamp {
            session_id: "session".into(),
            command_id: command.into(),
            project_instance_id: "instance".into(),
            revision: "7".into(),
            input_revision: "4".into(),
            snapshot_id: self.project.id.clone(),
            input_digest: self.project.input_digest(),
            origin: json!({"kind":"disk"}),
            dirty_at_start: true,
        }
    }
    fn create(&self, m: &RunManager, command: &str) -> String {
        m.create_run(&self.project, self.stamp(command), 600000)
            .unwrap()
    }
    fn publish(&self, m: &RunManager, id: &str, partial: bool, reply: bool) {
        let mut st = m.shared.state.lock().unwrap();
        let r = st.records.get_mut(id).unwrap();
        r.started_at_utc = Some(utc());
        let mut p = super::super::analysis::prepare(&self.project).unwrap();
        if partial {
            p.common.max_events = 1;
        }
        let inputs = crate::output::result_input_evidence(&p);
        p.common.run_identity = Some(crate::input::RunIdentity {
            run_id: id.into(),
            started_at_utc: r.started_at_utc.clone().unwrap(),
        });
        let result =
            crate::run_with_diagnostics(p, &m.shared.root.join(id).join("result")).unwrap();
        let mut report = serde_json::to_value(&result).unwrap();
        report["diagnostics"] = serde_json::to_value(&result.diagnostics).unwrap();
        r.phase = "running".into();
        if reply {
            r.response = Some(worker::Message::Report {
                run_id: id.into(),
                started_at_utc: r.started_at_utc.clone().unwrap(),
                report,
                evidence: ResultReportEvidence::from(&result),
                inputs,
            });
            r.worker_response = "available".into();
        }
        persist(&m.shared, r).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn timeout_payload_is_strict_and_bounded() {
    assert_eq!(wall_timeout_ms(&json!({})).unwrap(), 600000);
    for n in ["1000", "3600000"] {
        validate_run_payload(&json!({"wall_timeout_ms":n})).unwrap();
    }
    for v in [
        json!(null),
        json!([]),
        json!({"wall_timeout_ms":1000}),
        json!({"wall_timeout_ms":"01000"}),
        json!({"wall_timeout_ms":"999"}),
        json!({"wall_timeout_ms":"3600001"}),
        json!({"other":1}),
        json!({"wall_timeout_ms":"18446744073709551616"}),
    ] {
        assert!(validate_run_payload(&v).is_err(), "{v}");
    }
}
#[test]
fn run_creation_is_durable_exactly_once_and_does_not_edit_project() {
    let f = Fixture::new();
    let m = f.manager();
    let digest = f.project.input_digest();
    let id = f.create(&m, "one");
    assert_eq!(f.create(&m, "one"), id);
    assert_eq!(m.list(None).unwrap()["runs"].as_array().unwrap().len(), 1);
    let mut altered = f.stamp("one");
    altered.dirty_at_start = false;
    assert_eq!(
        m.create_run(&f.project, altered, 600000)
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(digest, f.project.input_digest());
    let r = m.record(&id).unwrap();
    assert_eq!(r.stamp.revision, "7");
    assert_eq!(r.snapshot.snapshot_id, f.project.id);
    assert!(
        m.shared
            .root
            .join(&id)
            .join("input/snapshot.commit.json")
            .exists()
    );
    assert!(
        !Directory::open(&m.shared.root)
            .unwrap()
            .names()
            .unwrap()
            .iter()
            .any(|n| n.to_string_lossy().starts_with(".freeze-"))
    );
    let saved: RunRecord = serde_json::from_slice(
        &verify_owner(&m.shared, &r)
            .unwrap()
            .child("control")
            .unwrap()
            .read("record.json", worker::FRAME_LIMIT)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(saved.run_id, id);
}
#[test]
fn capacity_rejects_before_freeze_and_pin_bounds_are_independent() {
    let f = Fixture::new();
    let m = f.manager();
    let a = f.create(&m, "a");
    f.create(&m, "b");
    assert_eq!(m.check_capacity(&f.project).unwrap_err().status, 503);
    let before = Directory::open(&m.shared.root).unwrap().names().unwrap();
    assert!(m.create_run(&f.project, f.stamp("c"), 600000).is_err());
    assert_eq!(
        before,
        Directory::open(&m.shared.root).unwrap().names().unwrap()
    );
    m.cancel(&a).unwrap();
    m.shared
        .state
        .lock()
        .unwrap()
        .records
        .values_mut()
        .find(|r| r.phase == "queued")
        .unwrap()
        .snapshot
        .bytes = PIN_LIMIT;
    assert_eq!(m.check_capacity(&f.project).unwrap_err().status, 503);
    let mut st = m.shared.state.lock().unwrap();
    st.freeze = true;
    drop(st);
    assert_eq!(m.check_capacity(&f.project).unwrap_err().status, 503);
}
#[test]
fn readonly_coordinator_keeps_reads_and_rejects_mutations() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    let other = f.manager();
    assert_eq!(other.get(&id).unwrap()["run_id"], id);
    assert_eq!(other.cancel(&id).unwrap_err().status, 503);
    assert_eq!(
        other
            .create_run(&f.project, f.stamp("other"), 600000)
            .unwrap_err()
            .status,
        503
    );
    assert_eq!(other.build_viewer(&id).unwrap_err().status, 503);
    assert_eq!(other.delete(&id).unwrap_err().status, 503);
}
#[test]
fn queued_cancel_is_terminal_and_noop_repeated() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    let v = m.cancel(&id).unwrap();
    assert_eq!(v["phase"], "cancelled");
    assert_eq!(v["stop_reason"], "cancel");
    assert_eq!(m.cancel(&id).unwrap(), v);
}
#[test]
fn stop_race_keeps_valid_complete_or_partial_result() {
    for partial in [false, true] {
        let f = Fixture::new();
        let m = f.manager();
        let id = f.create(&m, "one");
        f.publish(&m, &id, partial, true);
        let mut st = m.shared.state.lock().unwrap();
        let r = st.records.get_mut(&id).unwrap();
        stop(&m.shared, r, "cancel").unwrap();
        stop(&m.shared, r, "timeout").unwrap();
        assert_eq!(r.stop_reason.as_deref(), Some("cancel"));
        finish_record(&m.shared, r).unwrap();
        assert_eq!(r.phase, if partial { "partial" } else { "completed" });
        assert_eq!(r.report.as_ref().unwrap()["partial"], partial);
        assert!(r.report.as_ref().unwrap()["diagnostics"].is_array());
        drop(st);
        assert_eq!(
            m.cancel(&id).unwrap()["phase"],
            if partial { "partial" } else { "completed" }
        );
    }
}
#[test]
fn manifest_only_recovery_has_null_wall_metrics_and_no_report() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, false, false);
    let mut st = m.shared.state.lock().unwrap();
    let r = st.records.get_mut(&id).unwrap();
    r.phase = "stopping".into();
    r.stop_reason = Some("timeout".into());
    recover_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "completed");
    assert_eq!(r.stop_reason.as_deref(), Some("timeout"));
    assert!(r.report.is_none());
    assert!(r.recovered_result.as_ref().unwrap()["event_processing_wall_seconds"].is_null());
    assert_eq!(r.worker_response, "unavailable");
}
#[test]
fn restart_never_reruns_queued_and_recovers_published_manifest() {
    let f = Fixture::new();
    let m = f.manager();
    let queued = f.create(&m, "queued");
    let done = f.create(&m, "done");
    f.publish(&m, &done, false, false);
    drop(m);
    let m = f.manager();
    assert_eq!(m.get(&queued).unwrap()["phase"], "interrupted");
    assert_eq!(m.get(&done).unwrap()["phase"], "completed");
    assert!(m.get(&done).unwrap()["report"].is_null());
}
#[test]
fn tampered_frozen_input_and_result_are_rejected() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, false, true);
    let r = m.record(&id).unwrap();
    let blob = &r.snapshot.files[0].bundle_blob_id;
    fs::write(m.shared.root.join(&id).join("input").join(blob), b"tamper").unwrap();
    let mut st = m.shared.state.lock().unwrap();
    let r = st.records.get_mut(&id).unwrap();
    finish_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "integrity_failed");
    assert_eq!(r.artifact_state, "invalid");
    let file_id = r_file_id(&r.snapshot);
    drop(st);
    assert!(m.source(&id, &file_id).is_err());
}
fn r_file_id(e: &SnapshotEvidence) -> String {
    e.files[0].file_id.clone()
}
#[test]
fn explicit_delete_keeps_tombstone_and_preserves_unknown_or_linked_objects() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let external = f.root.join("external");
    fs::write(&external, b"preserve").unwrap();
    std::os::unix::fs::symlink(&external, m.shared.root.join(&id).join("viewer/leak")).unwrap();
    assert!(m.delete(&id).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"preserve");
    fs::remove_file(m.shared.root.join(&id).join("viewer/leak")).unwrap();
    m.delete(&id).unwrap();
    assert_eq!(m.get(&id).unwrap_err().status, 410);
    assert_eq!(m.source(&id, "x").unwrap_err().status, 410);
    assert!(!m.shared.root.join(&id).exists());
    assert!(m.shared.root.join(format!(".deleted-{id}")).exists());
}
#[test]
fn active_cross_process_reference_prevents_delete() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let reference = reference_lock(&m.shared, &id, false).unwrap();
    assert_eq!(m.delete(&id).unwrap_err().status, 503);
    drop(reference);
    m.delete(&id).unwrap();
}
#[test]
fn staged_creation_and_tombstoned_deletion_recover_without_loading_partial_records() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let r = m.record(&id).unwrap();
    let root = Directory::open(&m.shared.root).unwrap();
    root.create(
        format!(".deleted-{id}"),
        &encode(&json!({"schema_version":1,"run_id":id,"owner":r.owner,"inventory":cleanup::approved(&m.shared,&r).unwrap()})).unwrap(),
    )
    .unwrap();
    // Simulate deletion interrupted after removing its record; tombstone owns cleanup.
    fs::remove_file(m.shared.root.join(&id).join("control/record.json")).unwrap();
    drop(root);
    drop(m);
    let m = f.manager();
    assert_eq!(m.get(&id).unwrap_err().status, 410);
    assert!(!m.shared.root.join(&id).exists());
    let root = Directory::open(&m.shared.root).unwrap();
    let staged_id = random_id("run-").unwrap();
    let stage = root.mkdir(format!(".freeze-{staged_id}")).unwrap();
    stage
        .create(
            "owner.json",
            &encode(&json!({"schema_version":1,"run_id":staged_id,"identity":stage.identity()}))
                .unwrap(),
        )
        .unwrap();
    stage.mkdir("input").unwrap();
    drop(stage);
    drop(root);
    drop(m);
    let m = f.manager();
    assert!(!m.shared.root.join(format!(".freeze-{staged_id}")).exists());
}
#[test]
fn terminal_classification_preserves_engine_meaning() {
    assert_eq!(classify_result("prep_failed", true, 2), "prepare_failed");
    assert_eq!(classify_result("execution_failed", true, 3), "partial");
    assert_eq!(classify_result("unknown", false, 0), "execution_failed");
    assert_eq!(classify_result("events_exhausted", false, 0), "completed");
    for (reason, phase) in [
        ("cancel", "cancelled"),
        ("timeout", "timed_out"),
        ("resource_limit", "resource_limited"),
        ("service_shutdown", "interrupted"),
    ] {
        assert_eq!(stop_phase(Some(reason)), phase);
    }
}
#[test]
fn bounded_frames_reject_oversize_duplicate_keys_and_short_data() {
    let mut bytes = Vec::new();
    worker::write_frame(&mut bytes, &worker::Message::Ready).unwrap();
    assert!(matches!(
        worker::read_frame::<worker::Message>(&mut bytes.as_slice()).unwrap(),
        worker::Message::Ready
    ));
    assert!(
        worker::read_frame::<Value>(
            &mut ((worker::FRAME_LIMIT as u32 + 1).to_be_bytes().as_slice())
        )
        .is_err()
    );
    let raw = b"{\"x\":1,\"x\":2}";
    let mut bytes = (raw.len() as u32).to_be_bytes().to_vec();
    bytes.extend(raw);
    assert!(worker::read_frame::<Value>(&mut bytes.as_slice()).is_err());
    assert!(worker::read_frame::<Value>(&mut [0, 0, 0, 2, b'{'].as_slice()).is_err());
    let text = vec![b'x'; LOG_LIMIT + 99];
    assert_eq!(stderr_ring(text.as_slice()).len(), LOG_LIMIT);
}
#[test]
fn hash_uses_position_independent_reads() {
    let f = Fixture::new();
    let path = f.root.join("file");
    fs::write(&path, b"abcdef").unwrap();
    let mut file = File::open(path).unwrap();
    let first = hash_file(&file).unwrap();
    let mut buf = [0; 3];
    file.read_exact(&mut buf).unwrap();
    assert_eq!(first, hash_file(&file).unwrap());
    assert_eq!(first, hash(b"abcdef"));
    file.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"def");
}
#[test]
#[ignore = "Requires freshly built same binary in DIR_NED_RUN_TEST_BINARY; run explicitly"]
fn real_worker_smoke_when_binary_supplied() {
    let binary =
        std::env::var_os("DIR_NED_RUN_TEST_BINARY").expect("Supply DIR_NED_RUN_TEST_BINARY");
    let f = Fixture::new();
    let m = RunManager::open_impl(
        &f.root.join("state"),
        &f.project.input_regions(),
        Some(Path::new(&binary)),
    )
    .unwrap();
    let id = f.create(&m, "one");
    fs::remove_dir_all(f.root.join("source")).unwrap();
    let until = Instant::now() + Duration::from_secs(15);
    let result = loop {
        let v = m.get(&id).unwrap();
        if !m.record(&id).unwrap().active() {
            break v;
        }
        assert!(Instant::now() < until, "worker did not finish: {v}");
        let state = m.shared.state.lock().unwrap();
        let _ = m.shared.wake.wait_timeout(state, Duration::from_millis(20));
    };
    assert_eq!(result["phase"], "completed", "{result}");
    assert!(result["report"]["diagnostics"].is_array());
    assert_eq!(result["worker_exit"]["reaped"], true);
    let artifact_list = m.artifacts(&id).unwrap();
    assert_eq!(artifact_list["artifacts"].as_array().unwrap().len(), 6);
    let mut download = m.download(&id, "results.json").unwrap();
    assert_eq!(m.delete(&id).unwrap_err().status, 503);
    let mut bytes = Vec::new();
    download.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["run_id"],
        id
    );
    drop(download);
    m.delete(&id).unwrap();
}

#[test]
fn deleted_command_mapping_never_creates_a_second_run() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "once");
    m.cancel(&id).unwrap();
    m.delete(&id).unwrap();
    assert_eq!(f.create(&m, "once"), id);
    assert_eq!(m.get(&id).unwrap_err().status, 410);
    assert!(m.list(None).unwrap()["runs"].as_array().unwrap().is_empty());
}
#[test]
fn worker_unavailability_rejects_admission_without_disabling_history_reads() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.shared.state.lock().unwrap().worker_error = Some(invalid("injected unavailable hard limit"));
    assert_eq!(m.check_capacity(&f.project).unwrap_err().status, 503);
    assert_eq!(m.get(&id).unwrap()["run_id"], id);
    assert_eq!(m.cancel(&id).unwrap()["phase"], "cancelled");
}
#[test]
fn report_value_cannot_disagree_with_authenticated_worker_evidence() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, false, true);
    let mut state = m.shared.state.lock().unwrap();
    let r = state.records.get_mut(&id).unwrap();
    if let Some(worker::Message::Report { report, .. }) = &mut r.response {
        report["termination"] = json!("unknown");
    }
    finish_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "integrity_failed");
    assert_eq!(r.artifact_state, "invalid");
}
#[test]
fn run_list_pages_at_fifty_and_cursor_is_opaque_run_id() {
    let f = Fixture::new();
    let m = f.manager();
    for n in 0..51 {
        let id = f.create(&m, &format!("command-{n}"));
        m.cancel(&id).unwrap();
    }
    let first = m.list(None).unwrap();
    assert_eq!(first["runs"].as_array().unwrap().len(), 50);
    let second = m.list(first["next_cursor"].as_str()).unwrap();
    assert_eq!(second["runs"].as_array().unwrap().len(), 1);
    assert!(second["next_cursor"].is_null());
    assert!(m.list(Some("../escape")).is_err());
}
#[test]
fn failed_worker_retains_prior_diagnostics_and_original_termination() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    let mut state = m.shared.state.lock().unwrap();
    let r = state.records.get_mut(&id).unwrap();
    r.phase = "running".into();
    r.response = Some(worker::Message::Failed {
        diagnostic: crate::Diagnostic::output("publish failure").wire_value(),
        prior_diagnostics: vec![crate::Diagnostic::execution("prior failure").wire_value()],
        original_termination: Some("execution_failed".into()),
    });
    finish_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "output_failed");
    assert_eq!(r.diagnostics[0]["diagnostic"]["code"], "E-0002");
    assert_eq!(r.diagnostics[1]["diagnostic"]["code"], "E-0003");
    assert_eq!(r.value()["original_termination"], "execution_failed");
}

#[test]
fn post_rename_fsync_failure_keeps_one_assigned_identity_on_retry() {
    let f = Fixture::new();
    let m = f.manager();
    storage::FAIL_PUBLICATION_SYNC.with(|fail| fail.set(true));
    assert!(m.create_run(&f.project, f.stamp("one"), 600000).is_err());
    let pending = m
        .shared
        .state
        .lock()
        .unwrap()
        .pending
        .values()
        .next()
        .unwrap()
        .run_id
        .clone();
    assert_eq!(m.check_capacity(&f.project).unwrap_err().status, 503);
    assert_eq!(f.create(&m, "one"), pending);
    assert_eq!(m.list(None).unwrap()["runs"].as_array().unwrap().len(), 1);
    assert!(m.shared.state.lock().unwrap().pending.is_empty());
}
#[test]
fn failed_stop_persistence_does_not_accept_or_publish_stopping() {
    for queued in [true, false] {
        let f = Fixture::new();
        let m = f.manager();
        let id = f.create(&m, "one");
        if !queued {
            m.shared
                .state
                .lock()
                .unwrap()
                .records
                .get_mut(&id)
                .unwrap()
                .phase = "running".into();
        }
        let record = m.shared.root.join(&id).join("control/record.json");
        let external = f.root.join("linked-record");
        fs::hard_link(&record, &external).unwrap();
        assert!(m.cancel(&id).is_err());
        let r = m.record(&id).unwrap();
        assert_eq!(r.phase, if queued { "queued" } else { "running" });
        assert!(r.stop_reason.is_none());
        fs::remove_file(external).unwrap();
        assert_eq!(
            m.cancel(&id).unwrap()["phase"],
            if queued { "cancelled" } else { "stopping" }
        );
    }
}
#[test]
fn unknown_descendants_are_preserved_before_delete_and_during_tombstone_recovery() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let unknown = m.shared.root.join(&id).join("result/unknown");
    fs::write(&unknown, b"preserve").unwrap();
    assert!(m.delete(&id).is_err());
    assert_eq!(fs::read(&unknown).unwrap(), b"preserve");
    fs::remove_file(&unknown).unwrap();
    let r = m.record(&id).unwrap();
    let inventory = cleanup::approved(&m.shared, &r).unwrap();
    Directory::open(&m.shared.root)
        .unwrap()
        .create(
            format!(".deleted-{id}"),
            &encode(&json!({"schema_version":1,"run_id":id,"owner":r.owner,"inventory":inventory}))
                .unwrap(),
        )
        .unwrap();
    fs::write(&unknown, b"still-preserve").unwrap();
    let path = m.shared.root.clone();
    drop(m);
    assert!(RunManager::open(&f.root.join("state"), &f.project.input_regions()).is_err());
    assert_eq!(
        fs::read(path.join(id).join("result/unknown")).unwrap(),
        b"still-preserve"
    );
}
#[test]
fn owned_internal_hard_links_can_be_removed_but_external_links_cannot() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let result = m.shared.root.join(&id).join("result");
    let staged = result.join(format!(".tmp-{id}"));
    fs::create_dir(&staged).unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(staged.join("results.json"), b"unpublished").unwrap();
    fs::hard_link(staged.join("results.json"), result.join("results.json")).unwrap();
    let external = f.root.join("external");
    fs::hard_link(result.join("results.json"), &external).unwrap();
    assert!(m.delete(&id).is_err());
    fs::remove_file(external).unwrap();
    m.delete(&id).unwrap();
    assert!(!result.exists());
}
#[test]
fn published_restart_keeps_completion_time_and_diagnostic_count() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, true, true);
    let mut state = m.shared.state.lock().unwrap();
    let r = state.records.get_mut(&id).unwrap();
    finish_record(&m.shared, r).unwrap();
    let ended = r.ended_at_utc.clone();
    let count = r.diagnostics.len();
    recover_record(&m.shared, r).unwrap();
    assert_eq!(r.ended_at_utc, ended);
    assert_eq!(r.diagnostics.len(), count);
}

#[test]
fn manifest_only_recovery_requires_every_reconstructed_acquired_input() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, false, false);
    let mut state = m.shared.state.lock().unwrap();
    let r = state.records.get_mut(&id).unwrap();
    let expected = expectation(&m.shared, r).unwrap();
    assert_eq!(
        expected.acquired_inputs.as_ref().unwrap().len(),
        expected.frozen_inputs.len()
    );
    let result_path = m.shared.root.join(&id).join("result/results.json");
    let mut result: Value = serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
    result["metadata"]["sources"].as_array_mut().unwrap().pop();
    let hashes: Vec<_> = result["metadata"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| json!({"logical_path":s["logical_path"],"sha256":s["sha256"]}))
        .collect();
    result["metadata"]["input_sha256"] = json!(hash(&serde_json::to_vec(&hashes).unwrap()));
    let data = serde_json::to_vec(&result).unwrap();
    fs::write(&result_path, &data).unwrap();
    let manifest_path = m.shared.root.join(&id).join("result/manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    for artifact in manifest["files"].as_array_mut().unwrap() {
        if artifact["name"] == "results.json" {
            artifact["sha256"] = json!(hash(&data));
            artifact["bytes"] = json!(data.len().to_string());
        }
    }
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    finish_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "integrity_failed");
}

#[test]
fn stamp_and_correspondence_tampering_are_integrity_failures() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, false, false);
    let mut state = m.shared.state.lock().unwrap();
    let r = state.records.get_mut(&id).unwrap();
    r.snapshot.files[0].original_hash_at_load = "tampered".into();
    finish_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "integrity_failed");
    r.stamp.input_digest = "other-input".into();
    recover_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "integrity_failed");
}
#[test]
fn existing_shared_readable_state_parent_can_hold_private_runs() {
    let f = Fixture::new();
    let state = f.root.join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
    let m = f.manager();
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&fs::metadata(&m.shared.root).unwrap()) & 0o777,
        0o700
    );
    f.create(&m, "one");
}

fn await_phase(manager: &RunManager, id: &str, wanted: impl Fn(&RunRecord) -> bool) -> RunRecord {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let r = manager.record(id).unwrap();
        if wanted(&r) {
            return r;
        }
        assert!(
            Instant::now() < deadline,
            "Timed out waiting for worker: {}",
            r.value()
        );
        let state = manager.shared.state.lock().unwrap();
        let _ = manager
            .shared
            .wake
            .wait_timeout(state, Duration::from_millis(20));
    }
}
fn long_project(f: &mut Fixture) {
    let config = f
        .project
        .files
        .values_mut()
        .find(|f| f.roles.contains(&super::super::input::FileRole::Config))
        .unwrap();
    config.text =
        std::sync::Arc::from(config.text.replace("20ms", "0ps") + "\nmax-events = 1000000000000\n");
    config.hash = hash(config.text.as_bytes());
    let workload = f
        .project
        .files
        .values_mut()
        .find(|f| f.roles.contains(&super::super::input::FileRole::Workload))
        .unwrap();
    let mut value: Value = serde_json::from_str(&workload.text).unwrap();
    for generator in value["generators"].as_array_mut().unwrap() {
        generator["count"] = json!(1000000000000u64);
    }
    workload.text = std::sync::Arc::from(serde_json::to_string(&value).unwrap());
    workload.hash = hash(workload.text.as_bytes());
    f.project.refresh_inputs();
}
#[test]
#[ignore = "Requires freshly built same binary in DIR_NED_RUN_TEST_BINARY; run explicitly"]
fn real_worker_prepare_failure_timeout_force_kill_and_shutdown() {
    let binary =
        std::env::var_os("DIR_NED_RUN_TEST_BINARY").expect("Supply DIR_NED_RUN_TEST_BINARY");
    for kind in ["prepare_failure", "timeout", "cancel_stopped", "shutdown"] {
        let mut f = Fixture::new();
        if kind == "prepare_failure" {
            let id = f.project.ned_files().next().unwrap().id.clone();
            let file = f.project.files.get_mut(&id).unwrap();
            file.text = std::sync::Arc::from("broken ned syntax");
            file.hash = hash(file.text.as_bytes());
        } else {
            long_project(&mut f);
        }
        let m = RunManager::open_impl(
            &f.root.join("state"),
            &f.project.input_regions(),
            Some(Path::new(&binary)),
        )
        .unwrap();
        let id = m
            .create_run(
                &f.project,
                f.stamp(kind),
                if kind == "timeout" { 1000 } else { 600000 },
            )
            .unwrap();
        if kind == "cancel_stopped" {
            let r = await_phase(&m, &id, |r| r.phase == "running");
            let pid =
                rustix::process::Pid::from_raw(r.worker_pid.unwrap().parse().unwrap()).unwrap();
            let pidfd =
                rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).unwrap();
            rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::Stop).unwrap();
            m.cancel(&id).unwrap();
        } else if kind == "shutdown" {
            await_phase(&m, &id, |r| r.phase == "running");
            m.shutdown();
        }
        let r = await_phase(&m, &id, |r| !r.active());
        let expected = match kind {
            "prepare_failure" => "prepare_failed",
            "timeout" => "timed_out",
            "cancel_stopped" => "cancelled",
            _ => "interrupted",
        };
        assert_eq!(r.phase, expected, "{}", r.value());
        assert_eq!(r.worker_exit.as_ref().unwrap()["reaped"], true);
        assert_eq!(r.artifact_state, "unpublished");
        if kind == "cancel_stopped" {
            assert_eq!(
                r.worker_exit.as_ref().unwrap()["signal"],
                rustix::process::Signal::Kill as i32
            );
        }
    }
}

#[path = "large_response_tests.rs"]
mod large_responses;

#[test]
fn large_failure_keeps_output_classification_and_original_ordered_diagnostics() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    let control = record_dir(&m.shared, &id)
        .unwrap()
        .child("control")
        .unwrap();
    let original = worker::Message::Failed {
        diagnostic: crate::Diagnostic::output("x".repeat(worker::FRAME_LIMIT + 1)).wire_value(),
        prior_diagnostics: vec![crate::Diagnostic::execution("earlier diagnostic").wire_value()],
        original_termination: Some("execution_failed".into()),
    };
    let mut wire = Vec::new();
    worker::publish_response(&control, &mut wire, &original).unwrap();
    let reply = worker::read_frame::<worker::Message>(&mut wire.as_slice()).unwrap();
    assert_eq!(
        control
            .read(worker::LARGE_RESPONSE_FILE, CONTROL_LIMIT)
            .unwrap(),
        serde_json::to_vec(&original).unwrap()
    );
    let mut state = m.shared.state.lock().unwrap();
    let r = state.records.get_mut(&id).unwrap();
    r.response = Some(reply);
    finish_record(&m.shared, r).unwrap();
    assert_eq!(r.phase, "output_failed");
    assert_eq!(r.value()["original_termination"], "execution_failed");
    assert_eq!(r.diagnostics[0]["diagnostic"]["code"], "E-0002");
    assert_eq!(r.diagnostics[1]["diagnostic"]["code"], "E-0003");
}

#[test]
fn soft_quota_counts_internal_staging_links_once() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    let result = verify_owner(&m.shared, &m.record(&id).unwrap())
        .unwrap()
        .child("result")
        .unwrap();
    result.create("results.json", b"four").unwrap();
    let staged = result.mkdir(format!(".tmp-{id}")).unwrap();
    fs::hard_link(
        m.shared.root.join(&id).join("result/results.json"),
        m.shared
            .root
            .join(&id)
            .join("result")
            .join(format!(".tmp-{id}/results.json")),
    )
    .unwrap();
    assert_eq!(result.bytes().unwrap(), 4);
    assert!(staged.open_file("results.json").is_err());
}
#[test]
#[ignore = "Requires freshly built same binary in DIR_NED_RUN_TEST_BINARY; run explicitly"]
fn real_worker_viewer_render_without_http() {
    let binary =
        std::env::var_os("DIR_NED_RUN_TEST_BINARY").expect("Supply DIR_NED_RUN_TEST_BINARY");
    let f = Fixture::new();
    let m = RunManager::open_impl(
        &f.root.join("state"),
        &f.project.input_regions(),
        Some(Path::new(&binary)),
    )
    .unwrap();
    let id = f.create(&m, "one");
    let r = await_phase(&m, &id, |r| !r.active());
    assert_eq!(r.phase, "completed");
    let mut bundle = validated(&m.shared, &r).unwrap();
    let input = bundle.artifacts.remove("results.json").unwrap();
    let viewer = verify_owner(&m.shared, &r)
        .unwrap()
        .child("viewer")
        .unwrap();
    let name = "viewer-real.html";
    cleanup::reserve_viewer(&m.shared, &r, name).unwrap();
    let output = viewer.create_output(name).unwrap();
    cleanup::finish_viewer(&m.shared, &r, name, &output).unwrap();
    m.shared
        .state
        .lock()
        .unwrap()
        .viewer_generations
        .insert(id.clone(), 1);
    let policy = viewer_server::render_with_limits(&m.shared, &r, &input.file, &output, 1).unwrap();
    let file = viewer.open_file(name).unwrap();
    assert_eq!(hash_file(&file).unwrap(), policy.sha256);
    assert_eq!(policy.script_hashes.len(), 6);
    assert_eq!(policy.style_hashes.len(), 1);
    let bytes = viewer.read(name, 1024 * 1024).unwrap();
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("type=\"application/json\"")
    );
}
