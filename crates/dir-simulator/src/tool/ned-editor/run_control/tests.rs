use super::*;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
#[path = "acceptance_snapshot_tests.rs"]
mod acceptance_snapshot_tests;
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
    fn close_manager(&self, manager: RunManager) {
        let lock = Directory::open(&manager.shared.root)
            .unwrap()
            .open_file(".coordinator.lock")
            .unwrap();
        drop(manager);
        // `finished` is notified before the detached coordinator drops its Shared
        // Arc. Recovery requires the actual coordinator lock to be released.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                    assert!(
                        Instant::now() < deadline,
                        "Timed out waiting for the previous coordinator lock to be released"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("Cannot acquire the released coordinator lock: {error}"),
            }
        }
    }
    fn restart(&self, manager: RunManager) -> RunManager {
        self.close_manager(manager);
        let restarted = self.manager();
        assert!(
            restarted.shared.owner,
            "Restart must acquire coordinator ownership"
        );
        restarted
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
fn pinned_capacity_exact_boundary_and_one_byte_over_do_not_create_files() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "pinned");
    let incoming = run_snapshot::size(&f.project).unwrap();
    let before = Directory::open(&m.shared.root).unwrap().names().unwrap();
    // Exercise accounting directly without allocating PIN_LIMIT bytes. This
    // verifies the admission comparison, not physical disk/memory exhaustion.
    for (offset, permitted) in [(0, true), (1, true), (2, false)] {
        m.shared
            .state
            .lock()
            .unwrap()
            .records
            .get_mut(&id)
            .unwrap()
            .snapshot
            .bytes = PIN_LIMIT - incoming - 1 + offset;
        let result = m.check_capacity(&f.project);
        if permitted {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().status, 503);
            assert_eq!(
                m.create_run(&f.project, f.stamp("over-limit"), 600000)
                    .unwrap_err()
                    .status,
                503
            );
        }
        assert_eq!(
            before,
            Directory::open(&m.shared.root).unwrap().names().unwrap()
        );
        assert_eq!(m.list(None).unwrap()["runs"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn running_plus_two_waiters_rejects_third_and_cancel_releases_one_slot() {
    let f = Fixture::new();
    let m = f.manager();
    let active = f.create(&m, "active");
    m.shared
        .state
        .lock()
        .unwrap()
        .records
        .get_mut(&active)
        .unwrap()
        .phase = "running".into();
    let waiting = f.create(&m, "waiting-one");
    f.create(&m, "waiting-two");
    assert_eq!(m.check_capacity(&f.project).unwrap_err().status, 503);
    assert_eq!(
        m.create_run(&f.project, f.stamp("rejected"), 600000)
            .unwrap_err()
            .status,
        503
    );
    assert_eq!(m.cancel(&waiting).unwrap()["phase"], "cancelled");
    let replacement = f.create(&m, "replacement");
    assert_eq!(m.record(&replacement).unwrap().phase, "queued");
    assert_eq!(m.record(&active).unwrap().phase, "running");
    assert_eq!(
        m.shared
            .state
            .lock()
            .unwrap()
            .records
            .values()
            .filter(|r| r.phase == "queued")
            .count(),
        2
    );
}

#[test]
fn restart_preserves_durable_cancellation_and_same_command_identity() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "cancel-before-restart");
    let cancelled = m.cancel(&id).unwrap();
    let m = f.restart(m);
    assert_eq!(m.get(&id).unwrap()["phase"], "cancelled");
    assert_eq!(m.get(&id).unwrap()["stop_reason"], "cancel");
    assert_eq!(f.create(&m, "cancel-before-restart"), id);
    assert_eq!(m.cancel(&id).unwrap(), cancelled);
    assert_eq!(m.list(None).unwrap()["runs"].as_array().unwrap().len(), 1);
}

#[test]
fn restart_after_durable_stop_classifies_manifest_and_response_independently() {
    for (manifest, reply) in [(false, false), (true, false), (true, true)] {
        let f = Fixture::new();
        let m = f.manager();
        let id = f.create(&m, "stop-before-restart");
        if manifest {
            f.publish(&m, &id, false, reply);
        } else {
            let mut state = m.shared.state.lock().unwrap();
            let record = state.records.get_mut(&id).unwrap();
            record.phase = "running".into();
            persist(&m.shared, record).unwrap();
        }
        assert_eq!(m.cancel(&id).unwrap()["phase"], "stopping");
        let m = f.restart(m);
        let record = m.record(&id).unwrap();
        assert_eq!(record.stop_reason.as_deref(), Some("cancel"));
        assert_eq!(
            record.phase,
            if manifest { "completed" } else { "cancelled" }
        );
        assert_eq!(record.report.is_some(), reply);
        assert_eq!(record.recovered_result.is_some(), manifest && !reply);
        assert_eq!(f.create(&m, "stop-before-restart"), id);
        assert_eq!(m.list(None).unwrap()["runs"].as_array().unwrap().len(), 1);
    }
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
    // Hold the old Shared alive to deterministically reproduce the interval
    // between the finished notification and the coordinator lock's release.
    let shared = m.shared.clone();
    drop(m);
    assert!(shared.state.lock().unwrap().finished);
    let readonly = f.manager();
    assert!(!readonly.shared.owner);
    assert_eq!(readonly.get(&queued).unwrap()["phase"], "interrupted");
    assert_eq!(readonly.get(&done).unwrap()["phase"], "running");
    assert_eq!(readonly.cancel(&done).unwrap_err().status, 503);
    drop(shared);
    let m = f.restart(readonly);
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
    let m = f.restart(m);
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
    let m = f.restart(m);
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
    let result = await_real_worker_completion(&m, &id).value();
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
    assert!(!m.shared.state.lock().unwrap().pending.is_empty());
    m.check_capacity(&f.project).unwrap();
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
    f.close_manager(m);
    assert!(RunManager::open(&f.root.join("state"), &f.project.input_regions()).is_err());
    assert_eq!(
        fs::read(path.join(id).join("result/unknown")).unwrap(),
        b"still-preserve"
    );
}

#[test]
fn recovered_pre_manifest_run_can_be_deleted() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    f.publish(&m, &id, false, false);
    let result = m.shared.root.join(&id).join("result");
    let pending = result.join(format!(".tmp-{id}-manifest"));
    fs::rename(result.join("manifest.json"), &pending).unwrap();
    let m = f.restart(m);
    let runs = m.list(None).unwrap();
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["run_id"], id);
    assert_eq!(runs[0]["artifact_state"], "unpublished");
    assert!(pending.is_file());
    m.delete(&id).unwrap();
    assert!(!result.parent().unwrap().exists());
    assert!(m.list(None).unwrap()["runs"].as_array().unwrap().is_empty());
}

#[test]
fn pending_manifest_cleanup_rejects_foreign_names_directories_and_symlinks() {
    use std::os::unix::fs::symlink;
    for kind in ["foreign", "nested", "directory", "symlink"] {
        let f = Fixture::new();
        let m = f.manager();
        let id = f.create(&m, "one");
        m.cancel(&id).unwrap();
        let result = m.shared.root.join(&id).join("result");
        let path = result.join(if kind == "foreign" {
            ".tmp-other-manifest".to_owned()
        } else {
            format!(".tmp-{id}-manifest")
        });
        let external = f.root.join("preserved");
        fs::write(&external, b"preserve").unwrap();
        match kind {
            "directory" => fs::create_dir(&path).unwrap(),
            "nested" => {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("manifest.json"), b"unknown").unwrap();
            }
            "symlink" => symlink(&external, &path).unwrap(),
            _ => fs::write(&path, b"unknown").unwrap(),
        }
        assert!(m.delete(&id).is_err(), "{kind}");
        assert!(fs::symlink_metadata(&path).is_ok(), "{kind}");
        assert_eq!(fs::read(&external).unwrap(), b"preserve");
    }
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
    await_phase_with_timeout(manager, id, Duration::from_secs(20), wanted)
}
fn await_real_worker_completion(manager: &RunManager, id: &str) -> RunRecord {
    // Debug CI workers can need more than 15-20 seconds to prepare, simulate,
    // and authenticate the baseline artifacts. This bounds the test harness;
    // it does not change the run's product wall timeout or control-path waits.
    await_phase_with_timeout(manager, id, Duration::from_secs(60), |r| !r.active())
}
fn await_phase_with_timeout(
    manager: &RunManager,
    id: &str,
    timeout: Duration,
    wanted: impl Fn(&RunRecord) -> bool,
) -> RunRecord {
    let deadline = Instant::now() + timeout;
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
            // Sending SIGSTOP is not an acknowledgement of the stopped state.
            // Do not race cancellation's SIGTERM against pending stop delivery.
            use std::os::fd::AsFd;
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let stopped = rustix::process::waitid(
                    rustix::process::WaitId::PidFd(pidfd.as_fd()),
                    rustix::process::WaitidOptions::STOPPED
                        | rustix::process::WaitidOptions::NOWAIT
                        | rustix::process::WaitidOptions::NOHANG,
                )
                .unwrap()
                .is_some_and(|status| status.stopped());
                if stopped {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Worker did not acknowledge SIGSTOP"
                );
                std::thread::yield_now();
            }
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

#[test]
#[ignore = "Requires freshly built same binary in DIR_NED_RUN_TEST_BINARY and Python 3; run explicitly"]
fn real_worker_matches_saved_baseline_cli_manifest_records_metrics_and_diagnostics() {
    let binary =
        std::env::var_os("DIR_NED_RUN_TEST_BINARY").expect("Supply DIR_NED_RUN_TEST_BINARY");
    let f = Fixture::new();
    let baseline = f.root.join("baseline-result");
    let cli = std::process::Command::new(&binary)
        .args(["run", "--config"])
        .arg(f.root.join("source/project.ini"))
        .arg("--output")
        .arg(&baseline)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let cli_report: Value = serde_json::from_slice(&cli.stdout).unwrap();
    let cli_result: Value =
        serde_json::from_slice(&fs::read(baseline.join("results.json")).unwrap()).unwrap();
    let prepared = super::super::analysis::prepare(&f.project).unwrap();
    let expected_inputs = crate::output::result_input_evidence(&prepared);
    let baseline_bundle = crate::output::validate_result_bundle(
        &baseline,
        &ResultBundleExpectation {
            run_id: cli_result["run_id"].as_str().unwrap().to_owned(),
            started_at_utc: Some(
                cli_result["metadata"]["started_at_utc"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            ),
            frozen_inputs: expected_inputs.clone(),
            acquired_inputs: Some(expected_inputs),
            report: Some(serde_json::from_value(cli_report.clone()).unwrap()),
            manifest_sha256: None,
        },
    )
    .unwrap();
    assert_eq!(baseline_bundle.artifacts.len(), 5);
    let m = RunManager::open_impl(
        &f.root.join("state"),
        &f.project.input_regions(),
        Some(Path::new(&binary)),
    )
    .unwrap();
    // Eliminate disk fallback after the saved baseline was measured.
    fs::remove_dir_all(f.root.join("source")).unwrap();
    let id = f.create(&m, "baseline-equivalence");
    let record = await_real_worker_completion(&m, &id);
    assert_eq!(record.phase, "completed", "{}", record.value());
    assert_eq!(record.artifact_state, "published");
    let bundle = validated(&m.shared, &record).unwrap();
    assert_eq!(bundle.artifacts.len(), 5);
    // Reuse the existing performance acceptance normalizer: full simulation,
    // events/summary with only the run ID removed, and all manifest hashes/sizes.
    let normalizer = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/performance/measure_can_million.py");
    let checked = std::process::Command::new("python3")
        .arg("-c")
        .arg("import importlib.util,json,pathlib,sys; s=importlib.util.spec_from_file_location('normalizer',sys.argv[1]); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(json.dumps([m.verify_output(pathlib.Path(p)) for p in sys.argv[2:]]))")
        .arg(normalizer)
        .arg(&baseline)
        .arg(m.shared.root.join(&id).join("result"))
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let comparisons: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(comparisons[0]["manifest_verified"], true);
    assert_eq!(comparisons[1]["manifest_verified"], true);
    assert_eq!(
        comparisons[0]["deterministic"],
        comparisons[1]["deterministic"]
    );
    let frozen_result: Value = serde_json::from_slice(
        &fs::read(m.shared.root.join(&id).join("result/results.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        cli_result["schema_version"],
        frozen_result["schema_version"]
    );
    let mut cli_metadata = cli_result["metadata"].clone();
    let mut frozen_metadata = frozen_result["metadata"].clone();
    for metadata in [&mut cli_metadata, &mut frozen_metadata] {
        // Execution timestamps differ; all descriptors, source hashes/content,
        // topology, initial state, configuration and metric versions must agree.
        assert!(metadata["started_at_utc"].is_string());
        assert!(metadata["finished_at_utc"].is_string());
        metadata.as_object_mut().unwrap().remove("started_at_utc");
        metadata.as_object_mut().unwrap().remove("finished_at_utc");
    }
    assert_eq!(cli_metadata, frozen_metadata);
    assert_eq!(
        fs::read(baseline.join("diagnostics.jsonl")).unwrap(),
        fs::read(m.shared.root.join(&id).join("result/diagnostics.jsonl")).unwrap()
    );
    let mut cli_evidence = cli_report;
    let mut frozen_evidence = record.report.unwrap();
    assert_eq!(
        cli_evidence["output_path"],
        baseline.to_string_lossy().as_ref()
    );
    assert_eq!(
        frozen_evidence["output_path"],
        m.shared
            .root
            .join(&id)
            .join("result")
            .to_string_lossy()
            .as_ref()
    );
    for report in [&mut cli_evidence, &mut frozen_evidence] {
        let path = Path::new(report["output_path"].as_str().unwrap()).join("manifest.json");
        assert_eq!(report["manifest_path"], path.to_string_lossy().as_ref());
        report.as_object_mut().unwrap().remove("output_path");
        report.as_object_mut().unwrap().remove("manifest_path");
    }
    // Wall time is measured independently by the two executions.
    cli_evidence
        .as_object_mut()
        .unwrap()
        .remove("event_processing_wall_seconds");
    frozen_evidence
        .as_object_mut()
        .unwrap()
        .remove("event_processing_wall_seconds");
    // Worker reports attach diagnostics, while the CLI prints its RunReport.
    assert_eq!(frozen_evidence["diagnostics"], json!([]));
    frozen_evidence
        .as_object_mut()
        .unwrap()
        .remove("diagnostics");
    assert_eq!(cli_evidence, frozen_evidence);
    assert!(record.diagnostics.is_empty());
}

#[test]
#[ignore = "Requires freshly built same binary in DIR_NED_RUN_TEST_BINARY; run explicitly"]
fn real_worker_syntax_and_semantic_prepare_failures_map_frozen_source_without_output() {
    let binary =
        std::env::var_os("DIR_NED_RUN_TEST_BINARY").expect("Supply DIR_NED_RUN_TEST_BINARY");
    for syntax in [true, false] {
        let mut f = Fixture::new();
        let file_id = f.project.ned_files().next().unwrap().id.clone();
        let file = f.project.files.get_mut(&file_id).unwrap();
        let original = file.text.to_string();
        let text = if syntax {
            // EOF syntax error after Unicode/BOM/CRLF exercises source mapping.
            format!(
                "\u{feff}// 日本語\r\n{}",
                original
                    .replace('\n', "\r\n")
                    .trim_end()
                    .trim_end_matches('}')
            )
        } else {
            format!(
                "\u{feff}// 日本語\r\n{}",
                original
                    .replacen("a: demo.Controller;", "a: demo.Missing;", 1)
                    .replace('\n', "\r\n")
            )
        };
        let source_path = file.path.clone();
        file.text = std::sync::Arc::from(text.clone());
        file.hash = hash(text.as_bytes());
        f.project.refresh_inputs();
        let expected = super::super::analysis::prepare(&f.project).unwrap_err();
        assert_eq!(
            expected.reason,
            if syntax {
                "syntax_error"
            } else {
                "unknown_type"
            }
        );
        let start = if syntax {
            text.len()
        } else {
            text.find("demo.Missing").unwrap()
        };
        let end = if syntax {
            start
        } else {
            start + "demo.Missing".len()
        };
        assert_eq!(
            text.get(start..end),
            Some(if syntax { "" } else { "demo.Missing" })
        );
        // Calculate positions independently from the diagnostic implementation.
        // Columns count Unicode scalars, BOM is hidden, CRLF is one newline.
        let point = |offset| {
            let prefix = &text[..offset];
            let prefix = prefix
                .strip_prefix('\u{feff}')
                .unwrap_or(prefix)
                .replace("\r\n", "\n");
            (
                prefix.chars().filter(|&c| c == '\n').count() + 1,
                prefix.rsplit('\n').next().unwrap().chars().count() + 1,
            )
        };
        let (line, column) = point(start);
        let (end_line, end_column) = point(end);
        assert_eq!(
            expected.source.as_deref(),
            Some(source_path.to_str().unwrap())
        );
        assert_eq!(expected.line, Some(line));
        assert_eq!(expected.column, Some(column));
        assert_eq!(expected.end_line, Some(end_line));
        assert_eq!(expected.end_column, Some(end_column));
        let m = RunManager::open_impl(
            &f.root.join("state"),
            &f.project.input_regions(),
            Some(Path::new(&binary)),
        )
        .unwrap();
        fs::remove_dir_all(f.root.join("source")).unwrap();
        let id = f.create(&m, if syntax { "syntax" } else { "semantic" });
        let record = await_phase(&m, &id, |r| !r.active());
        assert_eq!(record.phase, "prepare_failed", "{}", record.value());
        assert_eq!(record.artifact_state, "unpublished");
        assert!(record.report.is_none());
        assert_eq!(record.worker_exit.as_ref().unwrap()["reaped"], true);
        assert!(matches!(
            record.response,
            Some(worker::Message::PreparedFailed { .. })
        ));
        assert_eq!(record.diagnostics.len(), 1);
        assert_eq!(record.diagnostics[0]["phase"], "prepare");
        assert_eq!(record.diagnostics[0]["diagnostic"], expected.wire_value());
        assert_eq!(
            record.diagnostics[0]["source_stamp"],
            serde_json::to_value(&record.stamp).unwrap()
        );
        assert_eq!(m.source(&id, &file_id).unwrap()["text"], text);
        let result = m.shared.root.join(&id).join("result");
        assert!(
            fs::read_dir(result).unwrap().next().is_none(),
            "prepare failure must not export any result artifacts"
        );
        assert_eq!(
            m.artifacts(&id).unwrap()["artifacts"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
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
    let r = await_real_worker_completion(&m, &id);
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
