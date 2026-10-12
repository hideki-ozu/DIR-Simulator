use super::*;

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
