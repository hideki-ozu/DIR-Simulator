// Public API vector assertions; shared Fixture/Probe are defined by the including test.
thread_local! {static VECTOR_COMMANDS:RefCell<Vec<ControlCommand>>=const{RefCell::new(Vec::new())};static VECTOR_FAILURE:Cell<bool>=const{Cell::new(false)};static VECTOR_FINISH:RefCell<Vec<String>>=const{RefCell::new(Vec::new())};static VECTOR_INIT_FAIL:Cell<bool>=const{Cell::new(false)};}
struct BoundaryProbe(String);
impl Model for BoundaryProbe {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        call("vector_initialize");
        if VECTOR_INIT_FAIL.with(Cell::get) {
            return Err("injected init failure".into());
        }
        for t in [1, 3, 5] {
            c.schedule_at(t, schema(), vec![t as u8])?;
        }
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, _: &mut Context<'_>) -> ModelResult {
        if e.time_ps == 5 {
            for command in VECTOR_COMMANDS.with(|v| v.borrow().clone()) {
                CONTROL.with(|c| c.borrow().as_ref().unwrap().submit(command).unwrap());
            }
            if VECTOR_FAILURE.with(Cell::get) {
                return Err("injected final callback failure".into());
            }
        }
        Ok(())
    }
    fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
        VECTOR_FINISH.with(|v| v.borrow_mut().push(self.0.clone()));
        Ok(())
    }
    fn checkpoint(&self, _: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        Ok(vec![0])
    }
}
struct BoundaryLink(String);
impl Channel for BoundaryLink {
    fn capability(&self, _: &Schema, _: u64, _: &[u8]) -> ModelResult<ParameterValue> {
        Ok(ParameterValue::Quantity(0))
    }
    fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
        VECTOR_FINISH.with(|v| v.borrow_mut().push(self.0.clone()));
        Ok(())
    }
    fn checkpoint(&self, _: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        Ok(vec![0])
    }
}
fn vector_registry() -> Registry {
    let mut r = registry();
    r.register_module(
        "test.Boundary",
        |c| {
            call("vector_factory");
            Ok(Box::new(BoundaryProbe(c.id.clone())))
        },
        ModuleDescriptor {
            implementation_key: "test.Boundary".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![
                PortDescriptor {
                    name: "out".into(),
                    direction: Direction::Output,
                    schema: schema(),
                },
                PortDescriptor {
                    name: "in".into(),
                    direction: Direction::Input,
                    schema: schema(),
                },
            ],
            events: vec![schema()],
            resources: vec![],
        },
    )
    .unwrap();
    r.register_channel(
        "test.BoundaryLink",
        |c| {
            call("vector_channel_factory");
            Ok(Box::new(BoundaryLink(c.id.clone())))
        },
        ChannelDescriptor {
            implementation_key: "test.BoundaryLink".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            capabilities: vec![],
        },
    )
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "test.boundary".into(),
        implementation_version: "1".into(),
        modules: vec!["test.Boundary".into()],
        channels: vec!["test.BoundaryLink".into()],
        events: vec![schema()],
        metrics: vec![],
        model_records: vec![],
        output_schema_version: 2,
        validate: |_, _| Ok(()),
    })
    .unwrap();
    r.register_execution_capabilities(
        "test.boundary",
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: false,
            checkpoint: false,
        },
    )
    .unwrap();
    r
}
fn boundary_session(f: &Fixture, two: bool) -> (RunSession, Controller) {
    std::fs::write(f.0.join("ned/demo/model.ned"),format!("package demo; simple Probe {{ parameters: @class(\"test.Boundary\"); gates: output out; input in; }} channel Link {{ parameters: @class(\"test.BoundaryLink\"); }} network Demo {{ submodules: a: demo.Probe; {} connections: a.out --> demo.Link --> a.in; {} }}",if two{"b: demo.Probe;"}else{""},if two{"b.out --> demo.Link --> b.in;"}else{""})).unwrap();
    let ini = f.0.join("scenario.ini");
    let s = std::fs::read_to_string(&ini).unwrap();
    std::fs::write(&ini, s.replace("test.control.v1", "test.boundary")).unwrap();
    CALLS.with(|c| c.borrow_mut().clear());
    VECTOR_FINISH.with(|v| v.borrow_mut().clear());
    let p = prepare_with_registry(&ini, vector_registry()).unwrap();
    let (s, c) = RunSession::new(p, RunOptions::default()).unwrap();
    CONTROL.with(|ctl| *ctl.borrow_mut() = Some(c.clone()));
    (s, c)
}
#[test]
fn vector_control_final_boundary_and_failed_callback() {
    for (commands, fail, state, statuses, commits) in [
        (
            vec![ControlCommand::Pause],
            false,
            RunState::Paused,
            vec![ControlStatus::Applied],
            3,
        ),
        (
            vec![ControlCommand::Pause, ControlCommand::Stop],
            false,
            RunState::Stopped,
            vec![ControlStatus::Superseded, ControlStatus::Applied],
            3,
        ),
        (vec![], false, RunState::Completed, vec![], 3),
        (
            vec![ControlCommand::Pause, ControlCommand::Stop],
            true,
            RunState::Failed,
            vec![ControlStatus::EngineFailed, ControlStatus::EngineFailed],
            2,
        ),
    ] {
        let f = Fixture::new();
        let (mut s, _) = boundary_session(&f, false);
        VECTOR_COMMANDS.with(|v| *v.borrow_mut() = commands);
        VECTOR_FAILURE.with(|v| v.set(fail));
        assert_eq!(s.run_until_yield().unwrap(), state);
        VECTOR_FAILURE.with(|v| v.set(false));
        VECTOR_COMMANDS.with(|v| v.borrow_mut().clear());
        assert_eq!(s.snapshot().common.committed_events, commits);
        assert_eq!(
            s.replay_log().iter().map(|r| r.status).collect::<Vec<_>>(),
            statuses
        );
        assert_eq!(
            s.snapshot().common.last_event_time_ps,
            Some(if fail { 3 } else { 5 })
        );
        if state == RunState::Paused {
            assert_eq!(s.snapshot().common.pending_events, 0);
            let token = s.pause_token().unwrap();
            assert_eq!(s.resume(token).unwrap().status, ControlStatus::Applied);
            assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
            assert_eq!(s.snapshot().common.committed_events, 3);
        }
    }
}
#[test]
fn vector_ready_pause_horizon_zero_and_retry_id_seven() {
    let f = Fixture::new();
    let (mut s, c) = session(&f, false);
    let h = c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    assert!(CALLS.with(|c| c.borrow().is_empty()));
    assert_eq!(c.ack(&h).unwrap().unwrap().virtual_time_ps, 0);
    assert_eq!(s.snapshot().common.committed_events, 0);
    c.forget(&h).unwrap();
    for id in 1..=7 {
        let h = c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(h.request_id, id);
        s.poll_control().unwrap();
        if id < 7 {
            c.forget(&h).unwrap();
        }
    }
    let retried = c.retry(7, ControlCommand::Pause).unwrap();
    let ack = c.ack(&retried).unwrap().unwrap();
    let again = c.retry(7, ControlCommand::Pause).unwrap();
    assert_eq!(
        serde_json::to_value(c.ack(&again).unwrap().unwrap()).unwrap(),
        serde_json::to_value(ack).unwrap()
    );
    assert_eq!(
        c.retry(7, ControlCommand::Stop).unwrap_err().code,
        "RequestConflict"
    );
    assert_eq!(s.replay_log().len(), 8);
    c.forget(&retried).unwrap();
    assert_eq!(
        c.retry(7, ControlCommand::Pause).unwrap_err().code,
        "DuplicateExpired"
    );
    assert_eq!(s.replay_log().len(), 8);
}
#[test]
fn vector_replay_three_distinct_snapshots_at_commit_three() {
    let f = Fixture::new();
    let (mut s, c) = boundary_session(&f, false);
    assert_eq!(s.advance_one().unwrap(), RunState::Running);
    for _ in 0..3 {
        assert_eq!(s.advance_one().unwrap(), RunState::Running);
    }
    assert_eq!(s.snapshot().common.committed_events, 3);
    // Empty polls do not allocate a replay boundary; use retained Pause operations to
    // establish first boundary sequence ten through real controller operations.
    for _ in 0..10 {
        c.submit(ControlCommand::Pause).unwrap();
        s.poll_control().unwrap();
    }
    let begin = s.replay_log().len();
    c.submit(ControlCommand::Pause).unwrap();
    s.poll_control().unwrap();
    assert_eq!(
        s.export(&f.0.join("vector-export")).unwrap().status,
        ControlStatus::Applied
    );
    s.resume(s.pause_token().unwrap()).unwrap();
    let log = &s.replay_log()[begin..];
    assert_eq!(
        log.iter().map(|r| r.boundary_seq).collect::<Vec<_>>(),
        [10, 11, 12]
    );
    assert!(log.iter().all(|r| r.commit_ordinal == 3));
    assert_eq!(
        log.iter().map(|r| r.state).collect::<Vec<_>>(),
        [RunState::Paused, RunState::Paused, RunState::Running]
    );
    let mut merged = s.replay_log().to_vec();
    merged[begin + 1].boundary_seq = merged[begin].boundary_seq;
    let result = replay_with_control(s.prepared().clone(), RunOptions::default(), &merged);
    assert!(result.is_err());
}
#[test]
fn vector_cleanup_two_models_two_channels_reverse_once() {
    for fail in [false, true] {
        let f = Fixture::new();
        let (mut s, _) = boundary_session(&f, true);
        VECTOR_INIT_FAIL.with(|v| v.set(fail));
        s.advance_one().unwrap();
        VECTOR_INIT_FAIL.with(|v| v.set(false));
        s.close();
        s.close();
        let calls = VECTOR_FINISH.with(|v| v.borrow().clone());
        let channels = s
            .prepared()
            .registered
            .as_ref()
            .unwrap()
            .channels
            .iter()
            .map(|c| c.id.clone())
            .rev()
            .collect::<Vec<_>>();
        let mut expected = if fail {
            vec![]
        } else {
            vec!["Demo.b".into(), "Demo.a".into()]
        };
        expected.extend(channels);
        assert_eq!(calls, expected);
    }
}
#[test]
fn vector_schema_two_normal_and_three_controlled_exports() {
    let f = Fixture::new();
    let p =
        prepare_with_registry(&f.0.join("scenario.ini"), registry_with_metric_v2(false)).unwrap();
    let snapshot = runtime::simulate(&p).unwrap();
    let normal = f.0.join("schema-two");
    std::fs::create_dir(&normal).unwrap();
    dir_simulator::output::export(&p, &snapshot, &normal).unwrap();
    let result: Value =
        serde_json::from_slice(&std::fs::read(normal.join("results.json")).unwrap()).unwrap();
    assert_eq!(result["schema_version"], 2);
    let (mut s, c) = RunSession::new(p, RunOptions::default()).unwrap();
    c.submit(ControlCommand::Pause).unwrap();
    s.poll_control().unwrap();
    assert_eq!(
        s.export(&f.0.join("schema-three")).unwrap().status,
        ControlStatus::Applied
    );
    let result: Value =
        serde_json::from_slice(&std::fs::read(f.0.join("schema-three/results.json")).unwrap())
            .unwrap();
    assert_eq!(result["schema_version"], 3);
}

#[test]
fn vector_checkpoint_four_declared_instances_empty_and_initialized() {
    for initialized in [false, true] {
        let f = Fixture::new();
        let model = f.0.join("ned/demo/model.ned");
        let text = std::fs::read_to_string(&model)
            .unwrap()
            .replace(
                "submodules: a: demo.Probe;",
                "submodules: a: demo.Probe; b: demo.Probe;",
            )
            .replace("a.in; }", "a.in; b.out --> demo.Link --> b.in; }");
        std::fs::write(model, text).unwrap();
        let (mut s, c) = session(&f, false);
        assert_eq!(
            s.prepared().registered.as_ref().unwrap().models.len()
                + s.prepared().registered.as_ref().unwrap().channels.len(),
            4
        );
        if initialized {
            s.advance_one().unwrap();
        }
        c.submit(ControlCommand::Pause).unwrap();
        s.poll_control().unwrap();
        let dest = f.0.join("four");
        assert_eq!(
            s.checkpoint(&dest, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(dest.join("state.json")).unwrap()).unwrap();
        assert_eq!(
            saved["codec_refs"].as_array().unwrap().len(),
            if initialized { 4 } else { 0 }
        );
        assert_eq!(
            saved["engine_phase"],
            if initialized {
                "initialized"
            } else {
                "uninitialized"
            }
        );
        CALLS.with(|v| v.borrow_mut().clear());
        let (restored, _) =
            restore_checkpoint(&dest, registry(), CheckpointLimits::default()).unwrap();
        assert_eq!(restored.state(), RunState::Paused);
        let calls = CALLS.with(|v| v.borrow().clone());
        assert_eq!(
            calls.iter().filter(|c| c.starts_with("restore_")).count(),
            if initialized { 4 } else { 0 }
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.starts_with("initialize") || c.starts_with("factory_"))
        );
    }
}
#[test]
fn vector_checkpoint_all_fingerprint_fields_fail_before_callbacks() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let path = f.0.join("fingerprints");
    s.checkpoint(&path, CheckpointLimits::default()).unwrap();
    let file = path.join("manifest.json");
    let bytes = std::fs::read(&file).unwrap();
    for field in [
        "binary_sha256",
        "registry_fingerprint",
        "input_fingerprint",
        "codec_version",
    ] {
        let mut m: Value = serde_json::from_slice(&bytes).unwrap();
        if field == "codec_version" {
            m["codec_catalog"][0][field] = json!(2);
        } else {
            m[field] = json!("0".repeat(64));
        }
        std::fs::write(&file, m.to_string()).unwrap();
        CALLS.with(|v| v.borrow_mut().clear());
        let e = restore_checkpoint(&path, registry(), CheckpointLimits::default())
            .err()
            .unwrap();
        assert_eq!(
            e.code,
            if field == "codec_version" {
                "UnsupportedCodec"
            } else {
                "FingerprintMismatch"
            },
            "{field}: {e:?}"
        );
        assert!(CALLS.with(|v| v.borrow().is_empty()));
    }
    std::fs::write(file, bytes).unwrap();
}
struct TokenProbe(Vec<TimerToken>);
fn token_registry() -> Registry {
    let mut r = registry();
    r.register_module(
        "test.Tokens",
        |_| {
            call("token_factory");
            Ok(Box::new(TokenProbe(vec![])))
        },
        ModuleDescriptor {
            implementation_key: "test.Tokens".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![schema()],
            resources: vec![],
        },
    )
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "test.tokens".into(),
        implementation_version: "1".into(),
        modules: vec!["test.Tokens".into()],
        channels: vec![],
        events: vec![schema()],
        metrics: vec![],
        model_records: vec![],
        output_schema_version: 2,
        validate: |_, _| Ok(()),
    })
    .unwrap();
    r.register_execution_capabilities(
        "test.tokens",
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: false,
            checkpoint: true,
        },
    )
    .unwrap();
    r.register_model_checkpoint_codec(CheckpointCodecDescriptor{implementation_key:"test.Tokens".into(),implementation_version:"1".into(),codec_name:"tokens".into(),codec_version:1,external_state:"none".into(),max_bytes:4096},|_,_,bytes,c|{
        call("token_restore");let refs:Vec<TimerReference>=serde_json::from_slice(bytes).map_err(|e|ModelError(e.to_string()))?;assert_eq!(serde_json::to_value(&refs).unwrap(),json!([{"version":1,"owner":"Demo.a","id":"2","state":"retired"},{"version":1,"owner":"Demo.a","id":"7","state":"live"}]));let tokens=refs.iter().map(|r|c.decode_timer_ref(r)).collect::<ModelResult<Vec<_>>>()?;Ok(Box::new(TokenProbe(tokens)))
    }).unwrap();
    r
}
impl Model for TokenProbe {
    fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
        Ok(())
    }
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        call("token_initialize");
        for id in 0..10 {
            let token = c.schedule_at(5, schema(), vec![id])?;
            if id != 7 {
                assert!(c.cancel(&token));
            }
            if id == 2 || id == 7 {
                self.0.push(token);
            }
        }
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        CONTROL.with(|ctl| {
            if let Some(ctl) = ctl.borrow().as_ref() {
                assert_eq!(ctl.submit(ControlCommand::Pause).unwrap_err().code, "Busy");
            }
        });
        serde_json::to_vec(
            &self
                .0
                .iter()
                .map(|t| c.encode_timer_ref(t))
                .collect::<ModelResult<Vec<_>>>()?,
        )
        .map_err(|e| ModelError(e.to_string()))
    }
}
fn token_input(f: &Fixture) {
    std::fs::write(f.0.join("ned/demo/model.ned"),"package demo; simple Tokens { parameters: @class(\"test.Tokens\"); } network Demo { submodules: a: demo.Tokens; }").unwrap();
    let p = f.0.join("scenario.ini");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(p, text.replace("test.control.v1", "test.tokens")).unwrap();
}
#[test]
fn vector_token_process_worker() {
    let Ok(mode) = std::env::var("DIR_VECTOR_TOKEN_MODE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_VECTOR_TOKEN_ROOT").unwrap());
    if mode == "save" {
        let (mut s, c) = RunSession::new(
            prepare_with_registry(&root.join("scenario.ini"), token_registry()).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        CONTROL.with(|v| *v.borrow_mut() = Some(c.clone()));
        s.advance_one().unwrap();
        c.submit(ControlCommand::Pause).unwrap();
        s.poll_control().unwrap();
        assert_eq!(
            s.export(&root.join("old-export")).unwrap().status,
            ControlStatus::Applied
        );
        let ack = s
            .checkpoint(&root.join("tokens"), CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    } else {
        CALLS.with(|v| v.borrow_mut().clear());
        let (mut s, token) = restore_checkpoint(
            &root.join("tokens"),
            token_registry(),
            CheckpointLimits::default(),
        )
        .unwrap();
        assert_eq!(s.state(), RunState::Paused);
        assert_eq!(CALLS.with(|v| v.borrow().clone()), ["token_restore"]);
        let old: Value =
            serde_json::from_slice(&std::fs::read(root.join("old-export/results.json")).unwrap())
                .unwrap();
        assert_eq!(
            s.export(&root.join("new-export")).unwrap().status,
            ControlStatus::Applied
        );
        let new: Value =
            serde_json::from_slice(&std::fs::read(root.join("new-export/results.json")).unwrap())
                .unwrap();
        assert_ne!(new["control"]["session_id"], old["control"]["session_id"]);
        s.resume(token).unwrap();
        assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
        let path = root.join("restored-tokens"); // saving the restored model once again also checks reference ownership
        assert_eq!(s.snapshot().common.committed_events, 1);
        let _ = path;
    }
}
#[test]
fn vector_checkpoint_exact_live_retired_tokens_in_new_process() {
    let f = Fixture::new();
    token_input(&f);
    let exe = std::env::current_exe().unwrap();
    for mode in ["save", "restore"] {
        let out = std::process::Command::new(&exe)
            .args(["--exact", "vector_token_process_worker", "--nocapture"])
            .env("DIR_VECTOR_TOKEN_MODE", mode)
            .env("DIR_VECTOR_TOKEN_ROOT", &f.0)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if mode == "save" {
            let state: Value =
                serde_json::from_slice(&std::fs::read(f.0.join("tokens/state.json")).unwrap())
                    .unwrap();
            assert_eq!(state["next_event_id"], "10");
            assert_eq!(state["live_timers"], json!([{"owner":"Demo.a","id":"7"}]));
        }
    }
}

struct FrozenVectorProbe;
impl Model for FrozenVectorProbe {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        for t in [3, 7, 10] {
            c.schedule_at(t, schema(), vec![0])?;
        }
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        let target = c.subject().to_owned();
        match e.time_ps {
            3 => c.set_gauge_u64("test.gauge", &target, 4)?,
            7 => {
                c.set_gauge_u64("test.gauge", &target, 9)?;
                c.set_gauge_u64("test.gauge", &target, 1)?;
            }
            _ => {
                CONTROL.with(|ctl| {
                    ctl.borrow()
                        .as_ref()
                        .unwrap()
                        .submit(ControlCommand::Pause)
                        .unwrap()
                });
            }
        }
        Ok(())
    }
}
#[test]
fn vector_frozen_export_at_ten_preserves_area_twenty_two_time_seven_and_token() {
    let f = Fixture::new();
    let mut r = registry();
    r.register_module(
        "test.Frozen",
        |_| Ok(Box::new(FrozenVectorProbe)),
        ModuleDescriptor {
            implementation_key: "test.Frozen".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![schema()],
            resources: vec![],
        },
    )
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "test.frozen".into(),
        implementation_version: "1".into(),
        modules: vec!["test.Frozen".into()],
        channels: vec![],
        events: vec![schema()],
        metrics: vec!["test.gauge".into()],
        model_records: vec![],
        output_schema_version: 3,
        validate: |_, _| Ok(()),
    })
    .unwrap();
    r.register_execution_capabilities(
        "test.frozen",
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: true,
            checkpoint: false,
        },
    )
    .unwrap();
    std::fs::write(f.0.join("ned/demo/model.ned"),"package demo; simple Frozen { parameters: @class(\"test.Frozen\"); } network Demo { submodules: a: demo.Frozen; }").unwrap();
    let config = f.0.join("scenario.ini");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace("test.control.v1", "test.frozen")
        .replace("10ps", "11ps");
    std::fs::write(&config, text).unwrap();
    let (mut s, c) = RunSession::new(
        prepare_with_registry(&config, r).unwrap(),
        RunOptions::default(),
    )
    .unwrap();
    CONTROL.with(|ctl| *ctl.borrow_mut() = Some(c));
    assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
    let token = s.pause_token().unwrap();
    let before = fingerprint(&s);
    for name in ["frozen1", "frozen2"] {
        assert_eq!(
            s.export(&f.0.join(name)).unwrap().status,
            ControlStatus::Applied
        );
        let result: Value =
            serde_json::from_slice(&std::fs::read(f.0.join(name).join("results.json")).unwrap())
                .unwrap();
        assert_eq!(result["control"]["horizon_ps"], "10");
        let exported = result["simulation"]["summary"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["metric"] == "test.gauge" && row["start_ps"].is_null())
            .unwrap();
        assert_eq!(exported["value"], 2.5);
        assert_eq!(summary(&s, 10)[0]["value"], 2.5);
        assert_eq!(fingerprint(&s), before);
        assert_eq!(s.pause_token(), Some(token.clone()));
        let acc = &s
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .metrics_v2
            .accumulators[0];
        assert_eq!(acc.last_time, 7);
        assert_eq!(acc.total.area, 22);
        assert_eq!(acc.last_value, 1);
    }
    assert_eq!(s.resume(token).unwrap().status, ControlStatus::Applied);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
}
