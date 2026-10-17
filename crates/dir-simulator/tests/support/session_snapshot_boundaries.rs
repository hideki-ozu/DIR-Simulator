// Public committed-result boundaries; included beside the shared Probe fixture.
fn snapshot_boundary_export(s: &mut RunSession, f: &Fixture, name: &str, horizon: u64) {
    let before = fingerprint(s);
    let end = s.snapshot().common.end_ps;
    let token = s.pause_token();
    let ack = s.export(&f.0.join(name)).unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    assert_eq!(ack.virtual_time_ps, horizon);
    let result: Value =
        serde_json::from_slice(&std::fs::read(f.0.join(name).join("results.json")).unwrap())
            .unwrap();
    assert_eq!(result["simulation"]["end_ps"], horizon.to_string());
    assert_eq!(result["control"]["horizon_ps"], horizon.to_string());
    assert_eq!(s.snapshot().common.end_ps, end);
    assert_eq!(s.pause_token(), token);
    assert_eq!(fingerprint(s), before);
}

#[test]
fn snapshot_boundary_ready_and_ready_pause_are_zero_without_factories() {
    let f = Fixture::new();
    let (mut s, c) = session(&f, false);
    assert_eq!(s.snapshot().common.end_ps, 0, "Ready exposes H=0, not T=10");
    let pause = c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    assert_eq!(c.ack(&pause).unwrap().unwrap().virtual_time_ps, 0);
    assert_eq!(s.snapshot().common.end_ps, 0);
    assert!(CALLS.with(|calls| calls.borrow().is_empty()));
    assert!(
        s.snapshot()
            .registered
            .as_ref()
            .unwrap()
            .model_records
            .is_empty()
    );
    snapshot_boundary_export(&mut s, &f, "ready-pause", 0);
    s.resume(s.pause_token().unwrap()).unwrap();
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(s.snapshot().common.end_ps, 10);
}

#[test]
fn snapshot_boundary_running_pause_stop_and_same_time_pending_keep_successful_h() {
    let f = Fixture::new();
    let (mut s, c) = session(&f, false);
    assert_eq!(s.advance_one().unwrap(), RunState::Running);
    assert_eq!(s.snapshot().common.end_ps, 0);
    assert_eq!(s.advance_one().unwrap(), RunState::Running);
    assert_eq!(s.snapshot().common.last_event_time_ps, Some(3));
    assert_eq!(
        s.snapshot().common.end_ps,
        3,
        "one successful callback ends at H=3"
    );
    let pause = c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    assert_eq!(c.ack(&pause).unwrap().unwrap().virtual_time_ps, 3);
    snapshot_boundary_export(&mut s, &f, "paused-three", 3);
    let stop = c.submit(ControlCommand::Stop).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Stopped);
    assert_eq!(c.ack(&stop).unwrap().unwrap().virtual_time_ps, 3);
    assert_eq!(s.snapshot().common.end_ps, 3);
    snapshot_boundary_export(&mut s, &f, "stopped-three", 3);

    let (mut s, c) = session(&f, true);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
    assert_eq!(s.snapshot().common.end_ps, 7);
    assert_eq!(s.snapshot().common.committed_events, 2);
    assert_eq!(s.snapshot().common.pending_events, 1);
    let token = s.pause_token().unwrap();
    c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    assert_eq!(s.pause_token(), Some(token.clone()));
    assert_eq!(s.snapshot().common.end_ps, 7);
    s.resume(token).unwrap();
    assert_eq!(s.advance_one().unwrap(), RunState::Running);
    assert_eq!(s.snapshot().common.committed_events, 3);
    assert_eq!(s.snapshot().common.end_ps, 7);
    assert_eq!(s.advance_one().unwrap(), RunState::Completed);
    assert_eq!(s.snapshot().common.end_ps, 10);
}

#[test]
fn snapshot_boundary_both_completion_drivers_restore_t_and_match_sync() {
    for stepped in [false, true] {
        let f = Fixture::new();
        CONTROL.with(|control| *control.borrow_mut() = None);
        let expected = runtime::simulate(&f.prepared()).unwrap();
        let (mut s, _) = session(&f, true);
        assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
        assert_eq!(s.snapshot().common.end_ps, 7);
        s.resume(s.pause_token().unwrap()).unwrap();
        if stepped {
            while s.advance_one().unwrap() == RunState::Running {}
        } else {
            s.run_until_yield().unwrap();
        }
        assert_eq!(s.state(), RunState::Completed);
        assert_eq!(s.snapshot().common.end_ps, 10);
        assert_eq!(s.snapshot().common.last_event_time_ps, Some(7));
        assert_eq!(s.snapshot().common.end_ps, expected.common.end_ps);
        assert_eq!(
            serde_json::to_value(&s.snapshot().common.points).unwrap(),
            serde_json::to_value(&expected.common.points).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap(),
            serde_json::to_value(&expected.registered.as_ref().unwrap().model_records).unwrap()
        );
        snapshot_boundary_export(&mut s, &f, "completed", 10);
    }
}

#[test]
fn snapshot_boundary_zero_event_resume_completes_at_t_with_no_last_event() {
    for stepped in [false, true] {
        let f = Fixture::new();
        let (mut s, c) = session(&f, false);
        c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        assert_eq!(s.snapshot().common.end_ps, 0);
        s.resume(s.pause_token().unwrap()).unwrap();
        NO_EVENTS.with(|flag| flag.set(true));
        if stepped {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            assert_eq!(s.snapshot().common.end_ps, 0);
            assert_eq!(s.advance_one().unwrap(), RunState::Completed);
        } else {
            assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        }
        NO_EVENTS.with(|flag| flag.set(false));
        assert_eq!(s.snapshot().common.last_event_time_ps, None);
        assert_eq!(s.snapshot().common.committed_events, 0);
        assert_eq!(s.snapshot().common.end_ps, 10);
        snapshot_boundary_export(&mut s, &f, "empty-completed", 10);
    }
}

#[test]
fn snapshot_boundary_time_limit_completion_keeps_unexecuted_candidate_pending() {
    for stepped in [false, true] {
        let f = Fixture::new();
        let mut prepared = f.prepared();
        prepared.common.time_limit_ps = 7;
        let (mut s, c) = RunSession::new(prepared, RunOptions::default()).unwrap();
        CONTROL.with(|control| *control.borrow_mut() = Some(c.clone()));
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
        c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        assert_eq!(s.snapshot().common.end_ps, 3);
        s.resume(s.pause_token().unwrap()).unwrap();
        let state = if stepped {
            s.advance_one()
        } else {
            s.run_until_yield()
        };
        assert_eq!(state.unwrap(), RunState::Completed);
        assert_eq!(s.snapshot().common.end_ps, 7);
        assert_eq!(s.snapshot().common.last_event_time_ps, Some(3));
        assert_eq!(s.snapshot().common.committed_events, 1);
        assert_eq!(s.snapshot().common.pending_events, 2);
        assert_eq!(s.snapshot().common.termination, "time_limit");
    }
}

#[test]
fn snapshot_boundary_failed_callback_exposes_last_success_for_both_drivers() {
    for stepped in [false, true] {
        let f = Fixture::new();
        let (mut s, _) = session(&f, false);
        FAIL_EVENT.with(|flag| flag.set(true));
        if stepped {
            while s.advance_one().unwrap() == RunState::Running {}
        } else {
            s.run_until_yield().unwrap();
        }
        FAIL_EVENT.with(|flag| flag.set(false));
        assert_eq!(s.state(), RunState::Failed);
        assert_eq!(s.snapshot().common.end_ps, 3);
        assert_eq!(s.snapshot().common.diagnostics[0].time_ps, Some(7));
        snapshot_boundary_export(&mut s, &f, "failed-prefix", 3);
    }
}

#[test]
fn snapshot_boundary_checkpoint_journal_restore_and_resume_match_public_h() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
    let path = f.0.join("checkpoint");
    assert_eq!(
        s.checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let state: Value =
        serde_json::from_slice(&std::fs::read(path.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["virtual_time_ps"], "7");
    assert_eq!(state["journal"]["end_ps"], "7");
    let (mut restored, token) =
        restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
    assert_eq!(restored.snapshot().common.end_ps, 7);
    assert_eq!(fingerprint(&restored), fingerprint(&s));
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    s.resume(s.pause_token().unwrap()).unwrap();
    s.run_until_yield().unwrap();
    assert_eq!(restored.snapshot().common.end_ps, 10);
    assert_eq!(fingerprint(&restored), fingerprint(&s));
}

#[test]
fn snapshot_boundary_restore_rejects_inconsistent_journal_end_before_factories() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
    for invalid_end in ["0", "6", "8", "10", "11"] {
        let path = f.0.join(format!("bad-end-{invalid_end}"));
        assert_eq!(
            s.checkpoint(&path, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        rewrite_checkpoint_state(&path, |state| {
            state["journal"]["end_ps"] = json!(invalid_end)
        });
        CALLS.with(|calls| calls.borrow_mut().clear());
        let result = restore_checkpoint(&path, registry(), CheckpointLimits::default());
        assert!(
            result.is_err(),
            "journal end {invalid_end} differs from paused H=7 but restore accepted it"
        );
        let error = result.err().unwrap();
        assert_eq!(error.code, "CheckpointInvalid");
        assert!(error.message.contains("horizon"), "{error}");
        assert!(CALLS.with(|calls| calls.borrow().is_empty()));
    }
}

fn snapshot_boundary_sample(relative: &str) -> dir_simulator::PreparedSimulation {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    let mut prepared = dir_simulator::prepare(&path).unwrap();
    prepared.common.time_limit_ps = 99_000_000;
    prepared
}

#[test]
fn snapshot_boundary_network_ready_pause_is_empty_and_resume_matches_sync() {
    for sample in [
        "examples/ethernet/dynamic/unicast.ini",
        "examples/ethernet-wire/passive.ini",
        "examples/network-extensions/preemption.ini",
    ] {
        let prepared = snapshot_boundary_sample(sample);
        let expected = runtime::simulate(&prepared).unwrap();
        let (mut s, c) = RunSession::new(prepared, RunOptions::default()).unwrap();
        assert_eq!(s.snapshot().common.end_ps, 0, "{sample}");
        c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        assert_eq!(s.snapshot().common.end_ps, 0);
        assert!(s.snapshot().network.is_none());
        assert!(s.snapshot().ethernet.is_none());
        assert!(
            s.snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .is_empty()
        );
        s.resume(s.pause_token().unwrap()).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(s.snapshot().common.end_ps, 99_000_000);
        assert_eq!(s.snapshot().network, expected.network, "{sample}");
        assert_eq!(
            serde_json::to_value(&s.snapshot().common.points).unwrap(),
            serde_json::to_value(&expected.common.points).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap(),
            serde_json::to_value(&expected.registered.as_ref().unwrap().model_records).unwrap()
        );
    }
}

#[test]
fn snapshot_boundary_wire_busy_and_preemption_occupied_are_clipped_before_export() {
    let f = Fixture::new();
    for preempt in [false, true] {
        let sample = if preempt {
            "examples/network-extensions/preemption.ini"
        } else {
            "examples/ethernet-wire/passive.ini"
        };
        let (mut s, c) =
            RunSession::new(snapshot_boundary_sample(sample), RunOptions::default()).unwrap();
        // Observe an actual in-flight transaction with a future planned end.
        let mut observed = false;
        for _ in 0..1000 {
            assert_eq!(s.advance_one().unwrap(), RunState::Running);
            let h = s.snapshot().common.last_event_time_ps.unwrap_or(0);
            if preempt {
                observed = s
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
                    .values()
                    .any(|r| {
                        r.schema.name == "dir.preemption.fragment"
                            && r.data["end_ps"].is_null()
                            && r.data["start_ps"].as_str().unwrap().parse::<u64>().unwrap() < h
                    });
            } else if let Some(network) = &s.snapshot().network {
                observed = network["wire"]["tx"].as_array().is_some_and(|rows| {
                    rows.iter().any(|r| {
                        r["sof_ps"]
                            .as_str()
                            .is_some_and(|start| start.parse::<u64>().unwrap() < h)
                            && r["release_ps"].is_null()
                    })
                });
            }
            if observed {
                break;
            }
        }
        assert!(observed, "sample must reach an in-flight prefix: {sample}");
        let h = s.snapshot().common.last_event_time_ps.unwrap();
        assert_eq!(s.snapshot().common.end_ps, h);
        c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        if preempt {
            let records = &s.snapshot().registered.as_ref().unwrap().model_records;
            let fragment = records
                .values()
                .find(|r| r.schema.name == "dir.preemption.fragment" && r.data["end_ps"].is_null())
                .unwrap();
            let start: u64 = fragment.data["start_ps"].as_str().unwrap().parse().unwrap();
            let transfer = records
                .values()
                .find(|r| {
                    r.schema.name == "dir.preemption.transfer"
                        && r.data["transfer_id"] == fragment.data["transfer_id"]
                })
                .unwrap();
            assert_eq!(transfer.data["occupied_ps"], (h - start).to_string());
            assert!(fragment.data["end_ps"].is_null());
            assert!(transfer.data["eof_ps"].is_null());
        } else {
            let rows = s.snapshot().network.as_ref().unwrap()["wire"]["tx"]
                .as_array()
                .unwrap();
            for row in rows {
                if let Some(start) = row["sof_ps"].as_str() {
                    let start: u64 = start.parse().unwrap();
                    let release: u64 = row["planned_release_ps"].as_str().unwrap().parse().unwrap();
                    assert_eq!(
                        row["busy_ps"],
                        h.min(release).saturating_sub(start).to_string()
                    );
                }
            }
        }
        let frozen_network = s.snapshot().network.clone();
        let frozen_records =
            serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap();
        let prefix_name = if preempt { "preemption" } else { "wire" };
        snapshot_boundary_export(&mut s, &f, &format!("{prefix_name}-paused"), h);
        assert_eq!(s.snapshot().network, frozen_network);
        assert_eq!(
            serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap(),
            frozen_records
        );
        c.submit(ControlCommand::Stop).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Stopped);
        assert_eq!(s.snapshot().common.end_ps, h);
        snapshot_boundary_export(&mut s, &f, &format!("{prefix_name}-stopped"), h);
        assert_eq!(s.snapshot().network, frozen_network);
    }
}

#[test]
fn snapshot_boundary_closed_ack_preserves_completed_observation_horizon() {
    let f = Fixture::new();
    let (mut s, c) = session(&f, false);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(s.snapshot().common.end_ps, 10);
    assert!(s.snapshot().common.last_event_time_ps.unwrap() < 10);
    let pending = c.submit(ControlCommand::Pause).unwrap();
    s.close();
    let ack = c.ack(&pending).unwrap().unwrap();
    assert_eq!(ack.status, ControlStatus::SessionClosed);
    assert_eq!(ack.state, RunState::Closed);
    assert_eq!(ack.virtual_time_ps, 10);
    assert_eq!(s.snapshot().common.end_ps, 10);
}
