//! End-to-end tests use public callbacks, real files and a second process.
use dir_simulator::registry::*;
use dir_simulator::runtime::session::*;
use dir_simulator::{prepare_with_registry, runtime};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
thread_local! {static CONTROL:RefCell<Option<Controller>>=const{RefCell::new(None)};static PAUSE:Cell<bool>=const{Cell::new(false)};static FAIL_EVENT:Cell<bool>=const{Cell::new(false)};static FAIL_INITIALIZE:Cell<bool>=const{Cell::new(false)};static FAIL_RESTORE:Cell<bool>=const{Cell::new(false)};static NO_EVENTS:Cell<bool>=const{Cell::new(false)};static CALLS:RefCell<Vec<String>>=const{RefCell::new(Vec::new())};}
fn call(value: &str) {
    CALLS.with(|c| c.borrow_mut().push(value.into()));
}
fn schema() -> Schema {
    Schema::new("test.Tick", 1)
}
struct Probe {
    count: u64,
    rng: u64,
    timers: Vec<TimerToken>,
}
impl Model for Probe {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        call("initialize");
        if FAIL_INITIALIZE.with(Cell::get) {
            return Err("injected initialize failure".into());
        }
        if NO_EVENTS.with(Cell::get) {
            return Ok(());
        }
        for (i, t) in [3, 7, 7, 8].iter().enumerate() {
            self.timers
                .push(c.schedule_at(*t, schema(), vec![i as u8])?);
        }
        c.cancel(&self.timers[3]);
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        self.count += 1;
        self.rng = self
            .rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let modern = c
            .registry()
            .profile("test.control.v1")
            .unwrap()
            .metrics
            .iter()
            .any(|metric| metric == "test.gauge");
        let target = c.subject().to_owned();
        if e.payload[0] == 0 {
            c.observe("test.legacy", &target, 11)?;
            if modern {
                c.set_gauge_u64("test.gauge", &target, 4)?;
            }
            c.observe("test.legacy", &target, 12)?;
            if modern {
                c.observe_point_u64("test.minimum", &target, 5)?;
            }
        }
        if e.payload[0] == 1 {
            if modern {
                c.set_gauge_u64("test.gauge", &target, 9)?;
            }
            c.observe("test.legacy", &target, 13)?;
            if modern {
                c.set_gauge_u64("test.gauge", &target, 1)?;
                c.observe_point_u64("test.minimum", &target, 1)?;
            }
        }
        let channel = c.connections()[0].channels[0].clone();
        let value = c.channel_capability(&channel, &Schema::new("test.state", 1), &[])?;
        c.upsert_model_record(ModelRecord {
            schema: Schema::new("test.Record", 1),
            id: self.count.to_string(),
            subject: c.subject().into(),
            time_ps: c.now(),
            data: json!({"rng":self.rng.to_string(),"query":format!("{value:?}")}),
        })?;
        if e.payload[0] == 1 && FAIL_EVENT.with(Cell::get) {
            CONTROL.with(|ctl| {
                let ctl = ctl.borrow();
                if let Some(c) = ctl.as_ref() {
                    c.submit(ControlCommand::Pause).unwrap();
                    c.submit(ControlCommand::Stop).unwrap();
                }
            });
            return Err("injected event failure".into());
        }
        if e.payload[0] == 1 && PAUSE.with(|p| p.replace(false)) {
            CONTROL.with(|ctl| {
                ctl.borrow()
                    .as_ref()
                    .unwrap()
                    .submit(ControlCommand::Pause)
                    .unwrap()
            });
        }
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        let timers = self
            .timers
            .iter()
            .map(|t| c.encode_timer_ref(t))
            .collect::<ModelResult<Vec<_>>>()?;
        serde_json::to_vec(
            &json!({"count":self.count.to_string(),"rng":self.rng.to_string(),"timers":timers}),
        )
        .map_err(|e| ModelError(e.to_string()))
    }
    fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
        call("finish_model");
        Ok(())
    }
}
struct Link(Cell<u64>);
impl Channel for Link {
    fn capability(&self, _: &Schema, _: u64, _: &[u8]) -> ModelResult<ParameterValue> {
        let old = self.0.get();
        self.0.set(old + 1);
        Ok(ParameterValue::Quantity(old))
    }
    fn checkpoint(&self, _: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        Ok(self.0.get().to_le_bytes().to_vec())
    }
    fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
        call("finish_channel");
        Ok(())
    }
}
fn registry() -> Registry {
    registry_with_metric_v2(true)
}
fn registry_with_metric_v2(include_v2: bool) -> Registry {
    let mut r = Registry::empty();
    r.register_event(EventDescriptor {
        schema: schema(),
        phase: 1,
        validate: |p| {
            if p.len() == 1 {
                Ok(())
            } else {
                Err("one byte".into())
            }
        },
    })
    .unwrap();
    r.register_module(
        "test.Probe",
        |_| {
            call("factory_model");
            Ok(Box::new(Probe {
                count: 0,
                rng: 17,
                timers: vec![],
            }))
        },
        ModuleDescriptor {
            implementation_key: "test.Probe".into(),
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
        "test.Link",
        |_| {
            call("factory_channel");
            Ok(Box::new(Link(Cell::new(0))))
        },
        ChannelDescriptor {
            implementation_key: "test.Link".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            capabilities: vec![Schema::new("test.state", 1)],
        },
    )
    .unwrap();
    r.register_metric(MetricDescriptor {
        name: "test.legacy".into(),
        unit: "count".into(),
    })
    .unwrap();
    r.register_metric_v2(
        MetricDescriptorV2 {
            name: "test.gauge".into(),
            unit: "items".into(),
            version: 1,
            value_kind: "integer".into(),
            sampling: MetricSampling::Gauge,
            aggregation: MetricAggregation::TimeMean,
        },
        vec!["Demo.a".into()],
        BTreeMap::from([("Demo.a".into(), 2)]),
    )
    .unwrap();
    r.register_metric_v2(
        MetricDescriptorV2 {
            name: "test.minimum".into(),
            unit: "items".into(),
            version: 1,
            value_kind: "integer".into(),
            sampling: MetricSampling::Point,
            aggregation: MetricAggregation::Min,
        },
        vec!["Demo.a".into()],
        BTreeMap::new(),
    )
    .unwrap();
    r.register_model_record(ModelRecordDescriptor {
        schema: Schema::new("test.Record", 1),
        validate: |_| Ok(()),
    })
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "test.control.v1".into(),
        implementation_version: "1".into(),
        modules: vec!["test.Probe".into()],
        channels: vec!["test.Link".into()],
        events: vec![schema()],
        metrics: if include_v2 {
            vec![
                "test.gauge".into(),
                "test.minimum".into(),
                "test.legacy".into(),
            ]
        } else {
            vec!["test.legacy".into()]
        },
        model_records: vec![Schema::new("test.Record", 1)],
        output_schema_version: if include_v2 { 3 } else { 2 },
        validate: |_, _| Ok(()),
    })
    .unwrap();
    r.register_execution_capabilities(
        "test.control.v1",
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: include_v2,
            checkpoint: true,
        },
    )
    .unwrap();
    let descriptor = |key: &str| CheckpointCodecDescriptor {
        implementation_key: key.into(),
        implementation_version: "1".into(),
        codec_name: "test.json".into(),
        codec_version: 1,
        external_state: "none".into(),
        max_bytes: 10240,
    };
    r.register_model_checkpoint_codec(descriptor("test.Probe"), |_, _, bytes, c| {
        call("restore_model");
        if FAIL_RESTORE.with(Cell::get) {
            return Err("injected restore failure".into());
        }
        let v: Value = serde_json::from_slice(bytes).map_err(|e| ModelError(e.to_string()))?;
        let timers = serde_json::from_value::<Vec<TimerReference>>(v["timers"].clone())
            .map_err(|e| ModelError(e.to_string()))?
            .iter()
            .map(|t| c.decode_timer_ref(t))
            .collect::<ModelResult<Vec<_>>>()?;
        Ok(Box::new(Probe {
            count: v["count"].as_str().unwrap().parse().unwrap(),
            rng: v["rng"].as_str().unwrap().parse().unwrap(),
            timers,
        }))
    })
    .unwrap();
    r.register_channel_checkpoint_codec(descriptor("test.Link"), |_, _, bytes, _| {
        call("restore_channel");
        let bytes: [u8; 8] = bytes
            .try_into()
            .map_err(|_| ModelError("link codec".into()))?;
        Ok(Box::new(Link(Cell::new(u64::from_le_bytes(bytes)))))
    })
    .unwrap();
    r
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/demo")).unwrap();
        std::fs::write(root.join("ned/demo/model.ned"),"package demo; simple Probe { parameters: @class(\"test.Probe\"); gates: output out; input in; } channel Link { parameters: @class(\"test.Link\"); } network Demo { submodules: a: demo.Probe; connections: a.out --> demo.Link --> a.in; }").unwrap();
        std::fs::write(root.join("scenario.ini"),"[General]\nnetwork=demo.Demo\nned-path=\"ned\"\nmodel-profile=\"test.control.v1\"\nsim-time-limit=10ps\nmetrics-window=4ps\n").unwrap();
        Self(root)
    }
    fn prepared(&self) -> dir_simulator::PreparedSimulation {
        prepare_with_registry(&self.0.join("scenario.ini"), registry()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn session(f: &Fixture, pause: bool) -> (RunSession, Controller) {
    CALLS.with(|c| c.borrow_mut().clear());
    let (s, c) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    CONTROL.with(|ctl| *ctl.borrow_mut() = Some(c.clone()));
    PAUSE.with(|p| p.set(pause));
    (s, c)
}
fn summary(s: &RunSession, h: u64) -> Vec<Value> {
    s.snapshot()
        .registered
        .as_ref()
        .unwrap()
        .metrics_v2
        .summary(&s.prepared().registered.as_ref().unwrap().registry, h, 4)
        .unwrap()
}
fn fingerprint(s: &RunSession) -> Value {
    json!({"records":s.snapshot().registered.as_ref().unwrap().model_records,"metrics":s.snapshot().registered.as_ref().unwrap().metrics_v2,"events":s.snapshot().common.committed_events,"last":s.snapshot().common.last_event_time_ps,"pending":s.snapshot().common.pending_events})
}
#[test]
fn ready_pause_mailbox_and_cleanup() {
    let f = Fixture::new();
    let (mut s, c) = session(&f, false);
    let h = c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
    assert_eq!(c.ack(&h).unwrap().unwrap().status, ControlStatus::Applied);
    assert!(CALLS.with(|c| c.borrow().is_empty()));
    let token = s.pause_token().unwrap();
    for _ in 1..64 {
        c.submit(ControlCommand::Pause).unwrap();
    }
    assert_eq!(c.submit(ControlCommand::Stop).unwrap_err().code, "Busy");
    s.poll_control().unwrap();
    assert!(s.replay_log().iter().skip(1).all(|r| r.boundary_seq == 1));
    assert!(c.retry(h.request_id, ControlCommand::Pause).is_ok());
    assert_eq!(
        c.retry(h.request_id, ControlCommand::Stop)
            .unwrap_err()
            .code,
        "RequestConflict"
    );
    c.forget(&h).unwrap();
    assert_eq!(
        c.retry(h.request_id, ControlCommand::Pause)
            .unwrap_err()
            .code,
        "DuplicateExpired"
    );
    let ack = s.resume(token).unwrap();
    assert_eq!(ack.status, ControlStatus::Applied);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(s.close().diagnostics.len(), 0);
    s.close();
    assert_eq!(
        CALLS.with(|c| c
            .borrow()
            .iter()
            .filter(|v| v.starts_with("finish"))
            .cloned()
            .collect::<Vec<_>>()),
        ["finish_model", "finish_channel"]
    );
}
#[test]
fn pause_at_same_time_export_is_non_destructive_and_resume_matches() {
    let f = Fixture::new();
    let (mut baseline, _) = session(&f, false);
    assert_eq!(baseline.run_until_yield().unwrap(), RunState::Completed);
    let expected = fingerprint(&baseline);
    let (mut s, _) = session(&f, true);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Paused);
    assert_eq!(s.snapshot().common.committed_events, 2);
    assert_eq!(s.snapshot().common.pending_events, 1);
    let before = fingerprint(&s);
    let token = s.pause_token().unwrap();
    let sums = summary(&s, 7);
    assert_eq!(
        sums.iter()
            .find(|v| v["metric"] == "test.gauge" && v["start_ps"].is_null())
            .unwrap()["value"],
        json!(22.0 / 7.0)
    );
    assert_eq!(
        sums.iter()
            .find(|v| v["metric"] == "test.minimum" && v["start_ps"].is_null())
            .unwrap()["value"],
        json!("1")
    );
    assert_eq!(
        sums.iter()
            .find(|v| v["metric"] == "test.minimum" && v["start_ps"] == "0")
            .unwrap()["value"],
        json!("5")
    );
    for name in ["first", "second"] {
        let ack = s.export(&f.0.join(name)).unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let result: Value =
            serde_json::from_slice(&std::fs::read(f.0.join(name).join("results.json")).unwrap())
                .unwrap();
        assert_eq!(result["schema_version"], 3);
        assert_eq!(result["simulation"]["termination"], "paused");
        assert_eq!(
            result["simulation"]["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["metric"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "test.legacy",
                "test.gauge",
                "test.legacy",
                "test.minimum",
                "test.gauge",
                "test.legacy",
                "test.gauge",
                "test.minimum"
            ]
        );
    }
    assert_eq!(fingerprint(&s), before);
    assert_eq!(
        s.export(&f.0.join("first")).unwrap().status,
        ControlStatus::InvalidState
    );
    assert_eq!(s.pause_token(), Some(token.clone()));
    assert_eq!(
        s.resume(token.clone()).unwrap().status,
        ControlStatus::Applied
    );
    assert_eq!(s.resume(token).unwrap().status, ControlStatus::InvalidState);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(fingerprint(&s), expected);
    let gauge = summary(&s, 10);
    assert_eq!(
        gauge
            .iter()
            .find(|v| v["metric"] == "test.gauge" && v["start_ps"].is_null())
            .unwrap()["value"],
        2.5
    );
    assert_eq!(
        gauge
            .iter()
            .filter(|v| v["metric"] == "test.gauge" && !v["start_ps"].is_null())
            .map(|v| v["value"].as_f64().unwrap())
            .collect::<Vec<_>>(),
        [2.5, 3.25, 1.0]
    );
}
#[test]
fn disk_checkpoint_restores_without_initialize_and_rejects_corruption() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let path = f.0.join("checkpoint");
    let ack = s.checkpoint(&path, CheckpointLimits::default()).unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    assert!(ack.checkpoint_receipt.is_some());
    let before = fingerprint(&s);
    CALLS.with(|c| c.borrow_mut().clear());
    let (mut restored, token) =
        restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
    assert_eq!(fingerprint(&restored), before);
    assert_eq!(
        CALLS.with(|c| c.borrow().clone()),
        ["restore_channel", "restore_model"]
    );
    restored.resume(token).unwrap();
    restored.run_until_yield().unwrap();
    s.resume(s.pause_token().unwrap()).unwrap();
    s.run_until_yield().unwrap();
    assert_eq!(fingerprint(&restored), fingerprint(&s));
    std::fs::write(path.join("extra.bin"), b"alien").unwrap();
    assert_eq!(
        restore_checkpoint(&path, registry(), CheckpointLimits::default())
            .err()
            .unwrap()
            .code,
        "CheckpointInvalid"
    );
}
#[test]
fn ready_checkpoint_restores_empty_arena_and_pending_stop_rejects_save() {
    let f = Fixture::new();
    let (mut s, c) = session(&f, false);
    c.submit(ControlCommand::Pause).unwrap();
    s.run_until_yield().unwrap();
    let ack = s
        .checkpoint(&f.0.join("ready"), CheckpointLimits::default())
        .unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    CALLS.with(|c| c.borrow_mut().clear());
    let (mut restored, token) =
        restore_checkpoint(&f.0.join("ready"), registry(), CheckpointLimits::default()).unwrap();
    assert!(CALLS.with(|c| c.borrow().is_empty()));
    restored.resume(token).unwrap();
    restored.run_until_yield().unwrap();
    assert!(CALLS.with(|c| c.borrow().iter().any(|v| v == "initialize")));
    c.submit(ControlCommand::Stop).unwrap();
    let ack = s
        .checkpoint(&f.0.join("stopped"), CheckpointLimits::default())
        .unwrap();
    assert_eq!(ack.status, ControlStatus::InvalidState);
    assert!(!f.0.join("stopped").exists());
}
#[test]
fn process_checkpoint_worker() {
    let Ok(mode) = std::env::var("DIR_CONTROL_CHILD_MODE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("DIR_CONTROL_CHILD_ROOT").unwrap());
    let config = root.join("scenario.ini");
    if mode == "save" {
        let (mut s, c) = RunSession::new(
            prepare_with_registry(&config, registry()).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        CONTROL.with(|ctl| *ctl.borrow_mut() = Some(c));
        PAUSE.with(|p| p.set(true));
        s.run_until_yield().unwrap();
        let ack = s
            .checkpoint(
                &root.join("process-checkpoint"),
                CheckpointLimits::default(),
            )
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    } else {
        let (mut s, t) = restore_checkpoint(
            &root.join("process-checkpoint"),
            registry(),
            CheckpointLimits::default(),
        )
        .unwrap();
        s.resume(t).unwrap();
        s.run_until_yield().unwrap();
        std::fs::write(
            root.join("process-result.json"),
            serde_json::to_vec(&fingerprint(&s)).unwrap(),
        )
        .unwrap();
    }
}
#[test]
fn checkpoint_continues_in_a_new_process_with_source_files_removed() {
    let f = Fixture::new();
    let (mut baseline, _) = session(&f, false);
    baseline.run_until_yield().unwrap();
    let exe = std::env::current_exe().unwrap();
    for mode in ["save", "restore"] {
        if mode == "restore" {
            std::fs::remove_file(f.0.join("scenario.ini")).unwrap();
            std::fs::remove_dir_all(f.0.join("ned")).unwrap();
        }
        let output = std::process::Command::new(&exe)
            .args(["--exact", "process_checkpoint_worker", "--nocapture"])
            .env("DIR_CONTROL_CHILD_MODE", mode)
            .env("DIR_CONTROL_CHILD_ROOT", &f.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let restored: Value =
        serde_json::from_slice(&std::fs::read(f.0.join("process-result.json")).unwrap()).unwrap();
    assert_eq!(restored, fingerprint(&baseline));
}
#[test]
fn synchronous_run_keeps_virtual_metric_results() {
    let f = Fixture::new();
    let prepared = f.prepared();
    let snapshot = runtime::simulate(&prepared).unwrap();
    let (mut controlled, _) = session(&f, false);
    controlled.run_until_yield().unwrap();
    assert_eq!(
        snapshot.common.committed_events,
        controlled.snapshot().common.committed_events
    );
    assert_eq!(
        snapshot.registered.unwrap().metrics_v2,
        controlled
            .snapshot()
            .registered
            .as_ref()
            .unwrap()
            .metrics_v2
    );
}
#[test]
fn replay_preserves_multiple_polls_at_one_commit_ordinal() {
    let f = Fixture::new();
    let (mut original, c) = session(&f, true);
    original.run_until_yield().unwrap();
    for _ in 0..2 {
        c.submit(ControlCommand::Pause).unwrap();
        original.poll_control().unwrap();
    }
    original.resume(original.pause_token().unwrap()).unwrap();
    original.run_until_yield().unwrap();
    let log = original.replay_log().to_vec();
    assert_eq!(
        log.iter().map(|r| r.boundary_seq).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    PAUSE.with(|p| p.set(false));
    let replayed = replay_with_control(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
        &log,
    )
    .unwrap();
    assert_eq!(fingerprint(&replayed), fingerprint(&original));
    let mut bad = log;
    bad[0].commit_ordinal = 99;
    assert_eq!(
        replay_with_control(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
            &bad
        )
        .err()
        .unwrap()
        .code,
        "ReplayMismatch"
    );
}
#[test]
fn network_owned_step_pause_resume_matches_synchronous_result() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/ethernet/dynamic/unicast.ini");
    let prepared = dir_simulator::prepare(&path).unwrap();
    let expected = runtime::simulate(&prepared).unwrap();
    let (mut s, c) = RunSession::new(prepared, RunOptions::default()).unwrap();
    assert_eq!(s.advance_one().unwrap(), RunState::Running);
    assert_eq!(s.advance_one().unwrap(), RunState::Running);
    assert_eq!(s.snapshot().common.committed_events, 1);
    c.submit(ControlCommand::Pause).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Paused);
    let token = s.pause_token().unwrap();
    s.resume(token).unwrap();
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(
        s.snapshot().common.committed_events,
        expected.common.committed_events
    );
    assert_eq!(
        serde_json::to_value(&s.snapshot().registered.as_ref().unwrap().model_records).unwrap(),
        serde_json::to_value(&expected.registered.unwrap().model_records).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&s.snapshot().common.points).unwrap(),
        serde_json::to_value(&expected.common.points).unwrap()
    );
}

#[test]
fn failed_callback_wins_over_pause_stop_and_discards_metric_batch() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, false);
    FAIL_EVENT.with(|flag| flag.set(true));
    assert_eq!(s.run_until_yield().unwrap(), RunState::Failed);
    FAIL_EVENT.with(|flag| flag.set(false));
    assert_eq!(s.snapshot().common.committed_events, 1);
    assert_eq!(
        s.snapshot()
            .registered
            .as_ref()
            .unwrap()
            .metrics_v2
            .records
            .len(),
        2
    );
    assert!(
        s.replay_log()
            .iter()
            .all(|r| r.status == ControlStatus::EngineFailed)
    );
    assert_eq!(s.replay_log().len(), 2);
    assert_eq!(
        CALLS.with(|c| c
            .borrow()
            .iter()
            .filter(|v| v.starts_with("finish"))
            .cloned()
            .collect::<Vec<_>>()),
        ["finish_model", "finish_channel"]
    );
    let before = fingerprint(&s);
    let ack = s.export(&f.0.join("failed")).unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    let output: Value =
        serde_json::from_slice(&std::fs::read(f.0.join("failed/results.json")).unwrap()).unwrap();
    assert_eq!(output["control"]["horizon_ps"], "3");
    assert_eq!(output["simulation"]["end_ps"], "3");
    assert_eq!(fingerprint(&s), before);
}
#[test]
fn initialize_failure_cleans_constructed_channel_once() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, false);
    FAIL_INITIALIZE.with(|flag| flag.set(true));
    assert_eq!(s.run_until_yield().unwrap(), RunState::Failed);
    FAIL_INITIALIZE.with(|flag| flag.set(false));
    s.close();
    s.close();
    assert_eq!(
        CALLS.with(|c| c
            .borrow()
            .iter()
            .filter(|v| v.starts_with("finish"))
            .cloned()
            .collect::<Vec<_>>()),
        ["finish_channel"]
    );
    assert_eq!(s.snapshot().common.committed_events, 0);
}
#[test]
fn stop_supersedes_ready_pause_without_factories_and_foreign_token_is_rejected() {
    let f = Fixture::new();
    let (mut first, c) = session(&f, false);
    let pause = c.submit(ControlCommand::Pause).unwrap();
    let stop = c.submit(ControlCommand::Stop).unwrap();
    assert_eq!(first.run_until_yield().unwrap(), RunState::Stopped);
    assert_eq!(
        c.ack(&pause).unwrap().unwrap().status,
        ControlStatus::Superseded
    );
    assert_eq!(
        c.ack(&stop).unwrap().unwrap().status,
        ControlStatus::Applied
    );
    assert!(CALLS.with(|c| c.borrow().is_empty()));
    let (mut second, c2) = session(&f, false);
    c2.submit(ControlCommand::Pause).unwrap();
    second.run_until_yield().unwrap();
    let token = second.pause_token().unwrap();
    assert_eq!(
        first.resume(token.clone()).unwrap().status,
        ControlStatus::InvalidState
    );
    assert_eq!(second.pause_token(), Some(token));
}

#[test]
fn checkpoint_fingerprint_and_codec_mismatch_are_rejected_before_factories() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let path = f.0.join("mismatch");
    assert_eq!(
        s.checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let manifest_path = path.join("manifest.json");
    let bytes = std::fs::read(&manifest_path).unwrap();
    let mut manifest: Value = serde_json::from_slice(&bytes).unwrap();
    CALLS.with(|c| c.borrow_mut().clear());
    manifest["binary_sha256"] = json!("0".repeat(64));
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        restore_checkpoint(&path, registry(), CheckpointLimits::default())
            .err()
            .unwrap()
            .code,
        "FingerprintMismatch"
    );
    assert!(CALLS.with(|c| c.borrow().is_empty()));
    let mut manifest: Value = serde_json::from_slice(&bytes).unwrap();
    manifest["codec_catalog"][0]["codec_version"] = json!(2);
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        restore_checkpoint(&path, registry(), CheckpointLimits::default())
            .err()
            .unwrap()
            .code,
        "UnsupportedCodec"
    );
    assert!(CALLS.with(|c| c.borrow().is_empty()));
    std::fs::write(&manifest_path, bytes).unwrap();
    assert!(restore_checkpoint(&path, registry(), CheckpointLimits::default()).is_ok());
}
#[test]
fn failed_restore_drops_private_prefix_without_finish_and_preserves_bundle() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let path = f.0.join("restore-failure");
    assert_eq!(
        s.checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let manifest = std::fs::read(path.join("manifest.json")).unwrap();
    let state = std::fs::read(path.join("state.json")).unwrap();
    CALLS.with(|c| c.borrow_mut().clear());
    FAIL_RESTORE.with(|v| v.set(true));
    let result = restore_checkpoint(&path, registry(), CheckpointLimits::default());
    FAIL_RESTORE.with(|v| v.set(false));
    assert_eq!(result.err().unwrap().code, "RestoreFailed");
    assert_eq!(
        CALLS.with(|c| c.borrow().clone()),
        ["restore_channel", "restore_model"]
    );
    assert_eq!(std::fs::read(path.join("manifest.json")).unwrap(), manifest);
    assert_eq!(std::fs::read(path.join("state.json")).unwrap(), state);
    assert!(restore_checkpoint(&path, registry(), CheckpointLimits::default()).is_ok());
}
#[test]
fn checkpoint_publication_failure_preserves_token_and_does_not_consume_ordinal() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let token = s.pause_token().unwrap();
    let before = fingerprint(&s);
    let existing = f.0.join("existing");
    std::fs::create_dir(&existing).unwrap();
    let ack = s
        .checkpoint(&existing, CheckpointLimits::default())
        .unwrap();
    assert_eq!(ack.status, ControlStatus::InvalidState);
    assert!(ack.checkpoint_receipt.is_none());
    assert_eq!(s.pause_token(), Some(token.clone()));
    assert_eq!(fingerprint(&s), before);
    let ack = s
        .checkpoint(&f.0.join("valid"), CheckpointLimits::default())
        .unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    assert_eq!(ack.checkpoint_receipt.unwrap().checkpoint_ordinal, 0);
    assert_eq!(s.resume(token).unwrap().status, ControlStatus::Applied);
    assert_eq!(s.run_until_yield().unwrap(), RunState::Completed);
}

fn rewrite_checkpoint_state(path: &std::path::Path, change: impl FnOnce(&mut Value)) {
    use sha2::{Digest, Sha256};
    let state_path = path.join("state.json");
    let mut state: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    change(&mut state);
    let bytes = serde_json::to_vec(&state).unwrap();
    std::fs::write(state_path, &bytes).unwrap();
    let manifest_path = path.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let entry = manifest["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["path"] == "state.json")
        .unwrap();
    entry["bytes"] = json!(bytes.len().to_string());
    entry["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    std::fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}
#[test]
fn restore_rejects_same_time_pending_work_before_last_committed_delta_phase_or_sequence() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    for field in ["delta", "phase", "sequence"] {
        let path = f.0.join(format!("corrupt-{field}"));
        assert_eq!(
            s.checkpoint(&path, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        rewrite_checkpoint_state(&path, |state| match field {
            "delta" => state["last_committed_key"][field] = json!("1"),
            "phase" => state["last_committed_key"][field] = json!(2),
            _ => state["last_committed_key"][field] = state["fes"][0][field].clone(),
        });
        CALLS.with(|calls| calls.borrow_mut().clear());
        let error = restore_checkpoint(&path, registry(), CheckpointLimits::default())
            .err()
            .unwrap();
        assert_eq!(error.code, "CheckpointInvalid", "{}", error);
        assert!(error.message.contains("last committed key"), "{}", error);
        assert!(CALLS.with(|calls| calls.borrow().is_empty()));
    }
}
#[test]
fn restore_rejects_model_record_map_key_that_disagrees_with_schema_and_id() {
    let f = Fixture::new();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let path = f.0.join("bad-record-key");
    assert_eq!(
        s.checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    rewrite_checkpoint_state(&path, |state| {
        let records = state["journal"]["model_records"].as_object_mut().unwrap();
        let old = records.keys().next().unwrap().clone();
        let record = records.remove(&old).unwrap();
        records.insert("test.Record:wrong-id".into(), record);
    });
    CALLS.with(|calls| calls.borrow_mut().clear());
    let error = restore_checkpoint(&path, registry(), CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "CheckpointInvalid");
    assert!(error.message.contains("schema:id"), "{}", error);
    assert!(CALLS.with(|calls| calls.borrow().is_empty()));
}
#[test]
fn checkpoint_captures_an_explicit_empty_ned_root_without_disk_fallback() {
    let f = Fixture::new();
    std::fs::create_dir(f.0.join("empty")).unwrap();
    let config = f.0.join("scenario.ini");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(
        &config,
        text.replace("ned-path=\"ned\"", "ned-path=\"ned\";\"empty\""),
    )
    .unwrap();
    let (mut s, _) = session(&f, true);
    s.run_until_yield().unwrap();
    let before = fingerprint(&s);
    let path = f.0.join("empty-root");
    assert_eq!(
        s.checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let state: Value =
        serde_json::from_slice(&std::fs::read(path.join("state.json")).unwrap()).unwrap();
    assert!(
        state["input_directories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|directory| directory.as_str() == f.0.join("empty").to_str())
    );
    std::fs::remove_dir(f.0.join("empty")).unwrap();
    std::fs::remove_dir_all(f.0.join("ned")).unwrap();
    std::fs::remove_file(config).unwrap();
    let (restored, _) = restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
    assert_eq!(fingerprint(&restored), before);
}

#[test]
fn replay_reaches_completed_before_post_completion_stop_and_export() {
    let f = Fixture::new();
    let (mut original, c) = session(&f, false);
    assert_eq!(original.run_until_yield().unwrap(), RunState::Completed);
    c.submit(ControlCommand::Stop).unwrap();
    original.poll_control().unwrap();
    let destination = f.0.join("completed-export");
    assert_eq!(
        original.export(&destination).unwrap().status,
        ControlStatus::Applied
    );
    let original_bytes = std::fs::read(destination.join("results.json")).unwrap();
    let replayed =
        replay_with_control(f.prepared(), RunOptions::default(), original.replay_log()).unwrap();
    assert_eq!(replayed.state(), RunState::Completed);
    assert_eq!(fingerprint(&replayed), fingerprint(&original));
    let root = replayed.replay_artifact_root().unwrap().to_path_buf();
    let output = root.join("request-1/results.json");
    assert!(output.exists());
    let result: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(result["control"]["state"], "Completed");
    assert_eq!(
        std::fs::read(destination.join("results.json")).unwrap(),
        original_bytes
    );
    drop(replayed);
    assert!(root.exists());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn replay_zero_event_terminal_operations_run_initialization_and_completion() {
    let f = Fixture::new();
    NO_EVENTS.with(|flag| flag.set(true));
    let (mut original, c) = session(&f, false);
    assert_eq!(original.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(original.snapshot().common.committed_events, 0);
    c.submit(ControlCommand::Stop).unwrap();
    original.poll_control().unwrap();
    assert_eq!(
        original.export(&f.0.join("zero-export")).unwrap().status,
        ControlStatus::Applied
    );
    let result = replay_with_control(f.prepared(), RunOptions::default(), original.replay_log());
    NO_EVENTS.with(|flag| flag.set(false));
    let replayed = result.unwrap();
    assert_eq!(replayed.state(), RunState::Completed);
    assert_eq!(fingerprint(&replayed), fingerprint(&original));
    let root = replayed.replay_artifact_root().unwrap().to_path_buf();
    drop(replayed);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn replay_writes_real_paused_exports_and_checkpoints_in_an_isolated_namespace() {
    let f = Fixture::new();
    let (mut original, c) = session(&f, false);
    c.submit(ControlCommand::Pause).unwrap();
    original.poll_control().unwrap();
    let export = f.0.join("original-export");
    let checkpoint = f.0.join("original-checkpoint");
    assert_eq!(
        original.export(&export).unwrap().status,
        ControlStatus::Applied
    );
    assert_eq!(
        original
            .checkpoint(&checkpoint, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let export_bytes = std::fs::read(export.join("results.json")).unwrap();
    let checkpoint_bytes = std::fs::read(checkpoint.join("state.json")).unwrap();
    let replayed = replay_with_control(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
        original.replay_log(),
    )
    .unwrap();
    assert_eq!(replayed.state(), RunState::Paused);
    let root = replayed.replay_artifact_root().unwrap().to_path_buf();
    assert!(root.join("request-1/results.json").is_file());
    let replay_checkpoint = root.join("request-2");
    assert!(replay_checkpoint.join("manifest.json").is_file());
    let (mut restored, token) =
        restore_checkpoint(&replay_checkpoint, registry(), CheckpointLimits::default()).unwrap();
    assert_eq!(
        restored.replay_log().last().unwrap().command["destination"],
        json!(checkpoint)
    );
    restored.resume(token).unwrap();
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(
        std::fs::read(export.join("results.json")).unwrap(),
        export_bytes
    );
    assert_eq!(
        std::fs::read(checkpoint.join("state.json")).unwrap(),
        checkpoint_bytes
    );
    drop(restored);
    drop(replayed);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn ready_pause_restore_requires_the_exact_initial_metric_state() {
    let f = Fixture::new();
    let (mut original, c) = session(&f, false);
    c.submit(ControlCommand::Pause).unwrap();
    original.poll_control().unwrap();
    for (index, field) in ["last_value", "total", "bucket_time", "next_record_sequence"]
        .iter()
        .enumerate()
    {
        let path = f.0.join(format!("bad-initial-{index}"));
        assert_eq!(
            original
                .checkpoint(&path, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        rewrite_checkpoint_state(&path, |state| {
            if *field == "next_record_sequence" {
                state["metric_accumulators"][field] = json!(1);
            } else {
                let acc = state["metric_accumulators"]["accumulators"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|acc| acc["metric"] == "test.gauge")
                    .unwrap();
                match *field {
                    "last_value" => acc[field] = json!(999),
                    "total" => acc[field]["area"] = json!("1"),
                    _ => acc[field] = json!(0),
                }
            }
        });
        CALLS.with(|calls| calls.borrow_mut().clear());
        let error = restore_checkpoint(&path, registry(), CheckpointLimits::default())
            .err()
            .unwrap();
        assert_eq!(error.code, "CheckpointInvalid");
        assert!(CALLS.with(|calls| calls.borrow().is_empty()));
    }
}
#[test]
fn restore_generation_overflow_is_rejected_before_any_private_factory() {
    let f = Fixture::new();
    let (mut original, _) = session(&f, true);
    original.run_until_yield().unwrap();
    let path = f.0.join("overflow-generation");
    assert_eq!(
        original
            .checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    rewrite_checkpoint_state(&path, |state| {
        state["next_pause_generation"] = json!(u64::MAX.to_string())
    });
    CALLS.with(|calls| calls.borrow_mut().clear());
    let error = restore_checkpoint(&path, registry(), CheckpointLimits::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "ResourceExhausted");
    assert!(CALLS.with(|calls| calls.borrow().is_empty()));
}
#[test]
fn metric_v2_failure_horizon_is_successful_prefix_and_legacy_v1_keeps_failure_time() {
    let f = Fixture::new();
    CONTROL.with(|controller| *controller.borrow_mut() = None);
    FAIL_EVENT.with(|flag| flag.set(true));
    let modern = runtime::simulate(&f.prepared()).unwrap();
    let legacy_prepared =
        prepare_with_registry(&f.0.join("scenario.ini"), registry_with_metric_v2(false)).unwrap();
    let legacy = runtime::simulate(&legacy_prepared).unwrap();
    FAIL_EVENT.with(|flag| flag.set(false));
    assert_eq!(modern.common.last_event_time_ps, Some(3));
    assert_eq!(modern.common.end_ps, 3);
    assert_eq!(modern.common.diagnostics[0].time_ps, Some(7));
    let values = modern
        .registered
        .as_ref()
        .unwrap()
        .metrics_v2
        .summary(
            &f.prepared().registered.unwrap().registry,
            modern.common.end_ps,
            4,
        )
        .unwrap();
    assert_eq!(
        values
            .iter()
            .find(|value| value["metric"] == "test.gauge" && value["start_ps"].is_null())
            .unwrap()["value"],
        2.0
    );
    assert_eq!(legacy.common.end_ps, 7);
    assert_eq!(legacy.common.last_event_time_ps, Some(3));
    assert_eq!(legacy.common.diagnostics[0].time_ps, Some(7));
    let (mut controlled, c) = RunSession::new(legacy_prepared, RunOptions::default()).unwrap();
    CONTROL.with(|controller| *controller.borrow_mut() = Some(c));
    FAIL_EVENT.with(|flag| flag.set(true));
    assert_eq!(controlled.run_until_yield().unwrap(), RunState::Failed);
    FAIL_EVENT.with(|flag| flag.set(false));
    assert_eq!(controlled.snapshot().common.end_ps, 3);
    assert_eq!(controlled.snapshot().common.diagnostics[0].time_ps, Some(7));
}

include!("support/control_metric_vectors.rs");

include!("support/session_snapshot_boundaries.rs");
