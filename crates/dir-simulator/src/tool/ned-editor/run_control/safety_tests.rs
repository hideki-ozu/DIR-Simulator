use super::*;

#[test]
fn viewer_recovery_snapshot_serializes_late_response_reclamation() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let record = m.record(&id).unwrap();
    let viewer = verify_owner(&m.shared, &record)
        .unwrap()
        .child("viewer")
        .unwrap();
    cleanup::reserve_viewer(&m.shared, &record, "race.html").unwrap();
    let file = viewer.create_output("race.html").unwrap();
    cleanup::finish_viewer(&m.shared, &record, "race.html", &file).unwrap();
    let stat = rustix::fs::fstat(&file).unwrap();
    let identity = Identity {
        dev: stat.st_dev,
        ino: stat.st_ino,
    };
    let (snapshot_tx, snapshot_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let other = m.clone();
    let other_record = record.clone();
    let recovery = std::thread::spawn(move || {
        cleanup::recover_viewers_with_snapshot(&other.shared, &other_record, || {
            snapshot_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        })
    });
    snapshot_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let protected = m.shared.viewer_inventory.try_lock().is_err();
    if !protected {
        // Reproduce the original schedule: final response cleanup removes X
        // after prepare has snapshotted X but before its batch starts.
        cleanup::remove_viewer(&m.shared, &record, "race.html", &identity).unwrap();
    }
    // Always release/join before asserting, including a regression failure.
    release_tx.send(()).unwrap();
    recovery.join().unwrap().unwrap();
    assert!(
        protected,
        "Inventory snapshot and batch must share one lock"
    );
    cleanup::remove_viewer(&m.shared, &record, "race.html", &identity).unwrap();
    assert!(viewer.names().unwrap().is_empty());
    viewer.create("race.html", b"unknown replacement").unwrap();
    assert!(cleanup::remove_viewer(&m.shared, &record, "race.html", &identity).is_err());
    assert_eq!(
        viewer.read("race.html", 100).unwrap(),
        b"unknown replacement"
    );
}

#[test]
fn obsolete_viewer_generations_are_reclaimed_after_the_last_response_reference() {
    use std::io::Write;
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let record = m.record(&id).unwrap();
    let viewer = verify_owner(&m.shared, &record)
        .unwrap()
        .child("viewer")
        .unwrap();
    for generation in 0..20 {
        let name = format!("viewer-{generation}.html");
        cleanup::reserve_viewer(&m.shared, &record, &name).unwrap();
        let mut file = viewer.create_output(&name).unwrap();
        let owner = viewer_server::OwnedViewer::new(&m.shared, &record, &name, &file).unwrap();
        cleanup::finish_viewer(&m.shared, &record, &name, &file).unwrap();
        file.write_all(b"partial failed or obsolete HTML").unwrap();
        let path = m.shared.root.join(&id).join("viewer").join(&name);
        let response = (
            file.try_clone().unwrap(),
            reference_lock(&m.shared, &id, false).unwrap(),
            owner.clone(),
        );
        drop(file);
        drop(owner);
        assert!(
            path.exists(),
            "An accepted response must retain its own generation"
        );
        assert_eq!(m.delete(&id).unwrap_err().status, 503);
        drop(response);
        assert!(
            !path.exists(),
            "The last reader must reclaim this owned generation"
        );
    }
    assert!(viewer.names().unwrap().is_empty());
    let inventory: BTreeMap<String, Option<Identity>> = serde_json::from_slice(
        &verify_owner(&m.shared, &record)
            .unwrap()
            .child("control")
            .unwrap()
            .read("viewer-inventory.json", CONTROL_LIMIT)
            .unwrap(),
    )
    .unwrap();
    assert!(inventory.is_empty());
}

#[test]
fn viewer_recovery_reconciles_missing_files_but_preserves_unknown_and_replaced_files() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let record = m.record(&id).unwrap();
    let viewer = verify_owner(&m.shared, &record)
        .unwrap()
        .child("viewer")
        .unwrap();
    cleanup::reserve_viewer(&m.shared, &record, "owned.html").unwrap();
    let file = viewer.create_output("owned.html").unwrap();
    cleanup::finish_viewer(&m.shared, &record, "owned.html", &file).unwrap();
    let path = m.shared.root.join(&id).join("viewer/owned.html");
    fs::remove_file(&path).unwrap();
    cleanup::recover_viewers(&m.shared, &record).unwrap();
    cleanup::reserve_viewer(&m.shared, &record, "unknown.html").unwrap();
    viewer.create("unknown.html", b"preserve").unwrap();
    cleanup::recover_viewers(&m.shared, &record).unwrap();
    assert!(m.shared.root.join(&id).join("viewer/unknown.html").exists());
    cleanup::reserve_viewer(&m.shared, &record, "replaced.html").unwrap();
    let file = viewer.create_output("replaced.html").unwrap();
    let owner =
        viewer_server::OwnedViewer::new(&m.shared, &record, "replaced.html", &file).unwrap();
    cleanup::finish_viewer(&m.shared, &record, "replaced.html", &file).unwrap();
    let path = m.shared.root.join(&id).join("viewer/replaced.html");
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"external replacement").unwrap();
    drop(owner);
    assert_eq!(fs::read(&path).unwrap(), b"external replacement");
    assert!(cleanup::recover_viewers(&m.shared, &record).is_err());
    drop(file);
}

#[test]
fn monitored_limits_stop_even_when_durable_record_cannot_be_written() {
    for reason in ["timeout", "resource_limit"] {
        let f = Fixture::new();
        let m = f.manager();
        let id = f.create(&m, "one");
        let record = m.shared.root.join(&id).join("control/record.json");
        let linked = f.root.join("external-record");
        fs::hard_link(&record, &linked).unwrap();
        let mut state = m.shared.state.lock().unwrap();
        let r = state.records.get_mut(&id).unwrap();
        r.phase = "running".into();
        monitored_stop(&m.shared, r, reason);
        assert_eq!(r.phase, "stopping");
        assert_eq!(r.stop_reason.as_deref(), Some(reason));
        assert_eq!(r.diagnostics.last().unwrap()["durable_stop"], false);
        let durable: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        assert_eq!(durable["phase"], "queued");
        assert_eq!(durable["stop_reason"], Value::Null);
        drop(state);
        fs::remove_file(linked).unwrap();
    }
}

#[test]
fn viewer_reserved_name_never_authorizes_an_unknown_file_identity() {
    let f = Fixture::new();
    let m = f.manager();
    let id = f.create(&m, "one");
    m.cancel(&id).unwrap();
    let r = m.record(&id).unwrap();
    let name = "viewer-failed.html";
    cleanup::reserve_viewer(&m.shared, &r, name).unwrap();
    let viewer = verify_owner(&m.shared, &r)
        .unwrap()
        .child("viewer")
        .unwrap();
    viewer.create(name, b"unknown allocation").unwrap();
    assert!(m.delete(&id).is_err());
    let owned = viewer.open_file(name).unwrap();
    cleanup::finish_viewer(&m.shared, &r, name, &owned).unwrap();
    let path = m.shared.root.join(&id).join("viewer").join(name);
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"external replacement").unwrap();
    assert!(m.delete(&id).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"external replacement");
}

#[test]
fn shutdown_waits_for_a_viewer_renderer_after_coordinator_finishes() {
    let f = Fixture::new();
    let m = f.manager();
    {
        let mut state = m.shared.state.lock().unwrap();
        state.finished = true;
        state.viewer_renders = 1;
    }
    let other = m.clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        other.shutdown();
        done_tx.send(()).unwrap();
    });
    started_rx.recv().unwrap();
    while !m.shared.shutdown.load(Ordering::Acquire) {
        std::thread::yield_now();
    }
    assert!(done_rx.try_recv().is_err());
    {
        let mut state = m.shared.state.lock().unwrap();
        state.viewer_renders = 0;
        m.shared.wake.notify_all();
    }
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    thread.join().unwrap();
}
