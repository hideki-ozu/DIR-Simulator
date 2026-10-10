//! External-crate style extensions use only the public registration interfaces.
use dir_simulator::registry::*;
use dir_simulator::{prepare_with_registry, runtime};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
static FINISHED: Mutex<Vec<String>> = Mutex::new(Vec::new());
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-registry-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("ned/demo")).unwrap();
        std::fs::write(path.join("ned/demo/model.ned"), r#"package demo;
            simple Sender { parameters: @class("test.Sender"); gates: output out; }
            simple Receiver { parameters: @class("test.Receiver"); gates: input in; }
            channel Delay { parameters: @class("test.Delay"); double delay @unit(s) = default(3ps); }
            network Demo { submodules: a: demo.Sender; b: demo.Receiver; connections: a.out --> demo.Delay --> b.in; }
        "#).unwrap();
        std::fs::write(path.join("scenario.ini"), "[General]\nnetwork = demo.Demo\nned-path = \"ned\"\nmodel-profile = \"test.profile.v1\"\nsim-time-limit = 20ps\n").unwrap();
        Self(path)
    }
    fn config(&self) -> PathBuf {
        self.0.join("scenario.ini")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn schema() -> Schema {
    Schema::new("test.Message", 1)
}
fn validate(bytes: &[u8]) -> ModelResult {
    if bytes.len() != 1 {
        Err("one byte required".into())
    } else {
        Ok(())
    }
}
fn descriptor(key: &str, direction: Direction) -> ModuleDescriptor {
    ModuleDescriptor {
        implementation_key: key.into(),
        implementation_version: "1".into(),
        parameters: vec![],
        ports: vec![PortDescriptor {
            name: if direction == Direction::Output {
                "out"
            } else {
                "in"
            }
            .into(),
            direction,
            schema: schema(),
        }],
        events: vec![schema()],
        resources: vec!["select".into()],
    }
}
struct Sender;
impl Model for Sender {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        let cancelled = c.schedule_after(1, schema(), vec![99])?;
        assert!(c.cancel(&cancelled));
        assert!(!c.cancel(&cancelled));
        c.schedule_at(2, schema(), vec![1])?;
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        assert_eq!(e.payload, vec![1]);
        let channel = c.connections()[0].channels[0].clone();
        let ParameterValue::Quantity(delay) =
            c.channel_capability(&channel, &Schema::new("test.propagation", 1), &e.payload)?
        else {
            return Err("bad channel value".into());
        };
        c.send_at("out", c.now() + delay, schema(), vec![7])?;
        c.request_arbitration("select")?;
        c.request_arbitration("select")?;
        Ok(())
    }
    fn on_arbitration(&mut self, _: &str, c: &mut Context<'_>) -> ModelResult {
        let id = c.self_id().to_string();
        c.observe("test.arbitrations", &id, 1)
    }
}
struct Receiver {
    fail: bool,
}
impl Model for Receiver {
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        assert_eq!(e.time_ps, 5);
        assert_eq!(e.input_port.as_deref(), Some("in"));
        let id = c.self_id().to_string();
        c.observe("test.received", &id, 1)?;
        c.upsert_model_record(ModelRecord {
            schema: Schema::new("test.Record", 1),
            id: "request1".into(),
            subject: id,
            time_ps: c.now(),
            data: serde_json::json!({"byte":e.payload[0]}),
        })?;
        if self.fail {
            c.schedule_after(1, schema(), vec![2])?;
            return Err("intentional failure".into());
        }
        Ok(())
    }
    fn finish(&mut self, c: &FinishContext<'_>) -> ModelResult {
        FINISHED
            .lock()
            .unwrap()
            .push(c.snapshot.common.termination.clone());
        Ok(())
    }
}
struct Delay(u64);
impl Channel for Delay {
    fn capability(&self, s: &Schema, _: u64, _: &[u8]) -> ModelResult<ParameterValue> {
        if s != &Schema::new("test.propagation", 1) {
            return Err("unknown capability".into());
        }
        Ok(ParameterValue::Quantity(self.0))
    }
}
fn registry(fail: bool) -> Registry {
    let receiver: ModelFactory = if fail {
        |_| Ok(Box::new(Receiver { fail: true }))
    } else {
        |_| Ok(Box::new(Receiver { fail: false }))
    };
    registry_factories(|_| Ok(Box::new(Sender)), receiver)
}
fn registry_factories(sender: ModelFactory, receiver: ModelFactory) -> Registry {
    registry_factories_in(Registry::new(), sender, receiver)
}
fn registry_factories_in(
    mut r: Registry,
    sender: ModelFactory,
    receiver: ModelFactory,
) -> Registry {
    r.register_event(EventDescriptor {
        schema: schema(),
        phase: 1,
        validate,
    })
    .unwrap();
    r.register_module(
        "test.Sender",
        sender,
        descriptor("test.Sender", Direction::Output),
    )
    .unwrap();
    r.register_module(
        "test.Receiver",
        receiver,
        descriptor("test.Receiver", Direction::Input),
    )
    .unwrap();
    r.register_channel(
        "test.Delay",
        |c| {
            let ParameterValue::Quantity(n) = c.parameters["delay"] else {
                return Err("bad delay".into());
            };
            Ok(Box::new(Delay(n)))
        },
        ChannelDescriptor {
            implementation_key: "test.Delay".into(),
            implementation_version: "1".into(),
            parameters: vec![ParameterDescriptor {
                name: "delay".into(),
                ned_type: "double".into(),
                dimension: Dimension::Time,
                minimum: Some(0.0),
                maximum: None,
                required: true,
            }],
            capabilities: vec![Schema::new("test.propagation", 1)],
        },
    )
    .unwrap();
    for name in ["test.received", "test.arbitrations"] {
        r.register_metric(MetricDescriptor {
            name: name.into(),
            unit: "count".into(),
        })
        .unwrap();
    }
    r.register_model_record(ModelRecordDescriptor {
        schema: Schema::new("test.Record", 1),
        validate: |v| {
            if v.get("byte").is_some() {
                Ok(())
            } else {
                Err("missing byte".into())
            }
        },
    })
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "test.profile.v1".into(),
        implementation_version: "1".into(),
        modules: vec!["test.Sender".into(), "test.Receiver".into()],
        channels: vec!["test.Delay".into()],
        events: vec![schema()],
        metrics: vec!["test.received".into(), "test.arbitrations".into()],
        model_records: vec![Schema::new("test.Record", 1)],
        output_schema_version: 2,
        validate: |_, _| Ok(()),
    })
    .unwrap();
    r
}
#[test]
fn custom_payload_channel_timer_cancel_and_arbitration_execute_deterministically() {
    let f = Fixture::new();
    let prepared = prepare_with_registry(&f.config(), registry(false)).unwrap();
    assert_eq!(prepared.node_count(), 2);
    let first = runtime::simulate(&prepared).unwrap();
    let second = runtime::simulate(&prepared).unwrap();
    assert_eq!(first.common.termination, "events_exhausted");
    assert_eq!(first.common.committed_events, 3); // sender, one merged arbitration, receiver
    assert_eq!(first.common.points.len(), 2);
    assert_eq!(
        first
            .registered
            .as_ref()
            .unwrap()
            .deliveries
            .iter()
            .map(|e| e.time_ps)
            .collect::<Vec<_>>(),
        vec![2, 5]
    );
    assert_eq!(
        serde_json::to_value(&first.registered).unwrap(),
        serde_json::to_value(&second.registered).unwrap()
    );
    assert_eq!(
        first
            .registered
            .unwrap()
            .model_record("test.Record", "request1")
            .unwrap()
            .data["byte"],
        7
    );
}
#[test]
fn callback_failure_discards_every_effect_and_preserves_pending_current() {
    let f = Fixture::new();
    let prepared = prepare_with_registry(&f.config(), registry(true)).unwrap();
    let snapshot = runtime::simulate(&prepared).unwrap();
    assert_eq!(snapshot.common.termination, "execution_failed");
    assert!(snapshot.common.partial);
    assert_eq!(snapshot.common.committed_events, 2);
    assert_eq!(snapshot.common.pending_events, 1);
    assert_eq!(snapshot.common.points.len(), 1);
    assert!(snapshot.registered.unwrap().model_records.is_empty());
}
#[test]
fn duplicate_registration_does_not_replace_original() {
    let mut registry = registry(false);
    assert!(
        registry
            .register_event(EventDescriptor {
                schema: schema(),
                phase: 0,
                validate
            })
            .is_err()
    );
    assert_eq!(registry.event(&schema()).unwrap().phase, 1);
}
#[test]
fn unknown_schema_and_port_version_are_prepare_errors() {
    let f = Fixture::new();
    let mut r = registry(false);
    r.register_module(
        "test.Other",
        |_| Ok(Box::new(Sender)),
        ModuleDescriptor {
            implementation_key: "test.Other".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![Schema::new("test.Unknown", 3)],
            resources: vec![],
        },
    )
    .unwrap();
    assert!(
        prepare_with_registry(&f.config(), r)
            .unwrap_err()
            .message
            .contains("unregistered module event")
    );
}
#[test]
fn strategy_registrations_are_executable() {
    let mut r = Registry::new();
    r.register_arbitration("test.first", |events| Ok((!events.is_empty()).then_some(0)))
        .unwrap();
    r.register_queue("test.bounded", |q, _, capacity| Ok(q.len() < capacity))
        .unwrap();
    r.register_generator("test.once", |_| Ok(vec![(1, schema(), vec![4])]))
        .unwrap();
    r.register_output("test.json", |s| {
        serde_json::to_vec(s).map_err(|e| e.to_string().into())
    })
    .unwrap();
    let event = Envelope {
        event_id: 0,
        source: "a".into(),
        destination: "b".into(),
        request_id: None,
        time_ps: 0,
        input_port: None,
        schema: schema(),
        payload: vec![0],
    };
    assert_eq!(
        r.arbitrate("test.first", std::slice::from_ref(&event))
            .unwrap(),
        Some(0)
    );
    assert!(
        !r.queue_accepts("test.bounded", std::slice::from_ref(&event), &event, 1)
            .unwrap()
    );
    assert_eq!(r.generate("test.once", &BTreeMap::new()).unwrap()[0].0, 1);
    assert!(
        !r.encode_output("test.json", &RegisteredSnapshot::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn initialization_error_discards_effects_and_finishes_successful_initializations() {
    static FINISH_COUNT: AtomicU64 = AtomicU64::new(0);
    struct Initializer;
    impl Model for Initializer {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            let id = c.self_id().to_string();
            c.observe("test.received", &id, 9)?;
            c.schedule_after(1, schema(), vec![1])?;
            if c.self_id().ends_with(".b") {
                Err("init failed".into())
            } else {
                Ok(())
            }
        }
        fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
            panic!("must not dispatch")
        }
        fn finish(&mut self, c: &FinishContext<'_>) -> ModelResult {
            assert_eq!(c.snapshot.common.termination, "prep_failed");
            assert!(c.snapshot.common.points.is_empty());
            FINISH_COUNT.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    let f = Fixture::new();
    let r = registry_factories(|_| Ok(Box::new(Initializer)), |_| Ok(Box::new(Initializer)));
    let snapshot = runtime::simulate(&prepare_with_registry(&f.config(), r).unwrap()).unwrap();
    assert_eq!(snapshot.common.termination, "prep_failed");
    assert_eq!(snapshot.common.committed_events, 0);
    assert_eq!(
        snapshot.common.diagnostics[0].target.as_deref(),
        Some("Demo.b")
    );
    assert_eq!(FINISH_COUNT.load(Ordering::Relaxed), 1);
}

/// DIR-TEST-0081: allocation, initialization, and cleanup are observed through
/// public factories; cleanup must continue against one frozen snapshot.
#[test]
fn acceptance_lifecycle_failures_release_models_and_keep_frozen_prefix() {
    static MODE: AtomicU64 = AtomicU64::new(0);
    static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Lifecycle(String);
    impl Drop for Lifecycle {
        fn drop(&mut self) {
            CALLS.lock().unwrap().push(format!("drop:{}", self.0));
        }
    }
    impl Model for Lifecycle {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            CALLS.lock().unwrap().push(format!("init:{}", self.0));
            if MODE.load(Ordering::Relaxed) == 1 && self.0.ends_with(".b") {
                c.observe("test.received", &self.0, 99)?;
                c.schedule_at(1, schema(), vec![99])?;
                return Err("acceptance initialize failure".into());
            }
            if self.0.ends_with(".a") {
                c.schedule_at(10, schema(), vec![1])?;
                c.schedule_at(20, schema(), vec![2])?;
            }
            Ok(())
        }
        fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
            c.observe("test.received", &self.0, u64::from(e.payload[0]))?;
            c.upsert_model_record(ModelRecord {
                schema: Schema::new("test.Record", 1),
                id: format!("event-{}", c.now()),
                subject: self.0.clone(),
                time_ps: c.now(),
                data: serde_json::json!({"byte":e.payload[0]}),
            })?;
            if MODE.load(Ordering::Relaxed) >= 2
                && MODE.load(Ordering::Relaxed) <= 3
                && c.now() == 20
            {
                c.schedule_after(1, schema(), vec![3])?;
                c.request_arbitration("select")?;
                return Err("acceptance runtime failure".into());
            }
            Ok(())
        }
        fn finish(&mut self, c: &FinishContext<'_>) -> ModelResult {
            let mode = MODE.load(Ordering::Relaxed);
            let expected = if mode == 1 {
                0
            } else if mode <= 3 {
                1
            } else {
                2
            };
            assert_eq!(c.snapshot.common.committed_events, expected);
            assert_eq!(c.snapshot.common.points.len(), expected as usize);
            assert_eq!(
                c.snapshot.common.last_event_time_ps,
                if expected == 0 {
                    None
                } else if expected == 1 {
                    Some(10)
                } else {
                    Some(20)
                }
            );
            CALLS.lock().unwrap().push(format!("finish:{}", self.0));
            if mode >= 3 && self.0.ends_with(".b") {
                Err("acceptance finish failure".into())
            } else {
                Ok(())
            }
        }
    }
    fn factory(c: &ModelConfig) -> ModelResult<Box<dyn Model>> {
        CALLS.lock().unwrap().push(format!("allocate:{}", c.id));
        if MODE.load(Ordering::Relaxed) == 0 && c.id.ends_with(".b") {
            Err("acceptance factory failure".into())
        } else {
            Ok(Box::new(Lifecycle(c.id.clone())))
        }
    }
    for mode in 0..=4 {
        MODE.store(mode, Ordering::Relaxed);
        CALLS.lock().unwrap().clear();
        let f = Fixture::new();
        let mut p =
            prepare_with_registry(&f.config(), registry_factories(factory, factory)).unwrap();
        p.common.time_limit_ps = 30;
        let s = runtime::simulate(&p).unwrap();
        let calls = CALLS.lock().unwrap().clone();
        assert!(calls.ends_with(&if mode == 0 {
            vec!["drop:Demo.a".into()]
        } else {
            vec!["drop:Demo.b".into(), "drop:Demo.a".into()]
        }));
        let finishes: Vec<_> = calls
            .iter()
            .filter(|x| x.starts_with("finish:"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            finishes,
            if mode == 0 {
                vec![]
            } else if mode == 1 {
                vec!["finish:Demo.a"]
            } else {
                vec!["finish:Demo.b", "finish:Demo.a"]
            }
        );
        if mode <= 1 {
            assert_eq!(s.common.termination, "prep_failed");
            assert_eq!(s.common.committed_events, 0);
            assert_eq!(s.common.pending_events, 0);
            assert!(s.common.points.is_empty());
            assert_eq!(
                s.common.diagnostics[0].reason,
                if mode == 0 {
                    "allocation_failed"
                } else {
                    "initialize_failed"
                }
            );
        } else {
            assert_eq!(s.common.termination, "execution_failed");
            assert!(s.common.partial);
            let records = &s.registered.as_ref().unwrap().model_records;
            if mode <= 3 {
                assert_eq!(s.common.committed_events, 1);
                assert_eq!(s.common.pending_events, 1);
                assert_eq!(records.len(), 1);
                assert_eq!(s.common.diagnostics[0].time_ps, Some(20));
                assert!(s.common.diagnostics[0].message.contains("runtime failure"));
            } else {
                assert_eq!(s.common.committed_events, 2);
                assert_eq!(records.len(), 2);
            }
            assert_eq!(s.common.diagnostics.len(), if mode == 3 { 2 } else { 1 });
            if mode >= 3 {
                assert_eq!(s.common.diagnostics.last().unwrap().reason, "finish_failed");
            }
        }
    }
}

/// DIR-TEST-0008/0083: release, arrival fixed point, sorted resources and
/// backwards-phase work use the actual public registered scheduler.
#[test]
fn acceptance_phase_fixed_point_resource_order_and_next_delta_repeat() {
    static REVERSE: AtomicU64 = AtomicU64::new(0);
    struct Phased;
    let release = || Schema::new("test.Release", 1);
    impl Model for Phased {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            if c.self_id().ends_with(".a") {
                c.schedule_at(10, Schema::new("test.Release", 1), vec![0])?;
            }
            c.schedule_at(10, schema(), vec![1])?;
            Ok(())
        }
        fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
            let id = c.self_id().to_owned();
            let value = u64::from(e.payload[0]) + if id.ends_with(".b") { 10 } else { 0 };
            c.observe("test.received", &id, value)?;
            if id.ends_with(".a") && e.payload[0] == 1 {
                c.schedule_at(10, schema(), vec![2])?;
                let order = if REVERSE.load(Ordering::Relaxed) == 0 {
                    ["B", "A"]
                } else {
                    ["A", "B"]
                };
                for resource in order {
                    c.request_arbitration(resource)?;
                }
            }
            Ok(())
        }
        fn on_arbitration(&mut self, resource: &str, c: &mut Context<'_>) -> ModelResult {
            let id = c.self_id().to_owned();
            c.observe("test.received", &id, if resource == "A" { 4 } else { 5 })?;
            if resource == "A" {
                c.schedule_at(c.now(), schema(), vec![3])?;
                c.request_arbitration("B")?;
            }
            Ok(())
        }
    }
    let mut reference = None;
    for reversed in [0, 1] {
        REVERSE.store(reversed, Ordering::Relaxed);
        for _ in 0..3 {
            let f = Fixture::new();
            let ned = f.0.join("ned/demo/model.ned");
            std::fs::write(
                &ned,
                std::fs::read_to_string(&ned)
                    .unwrap()
                    .replace("test.Sender", "test.PhasedSender")
                    .replace("test.Receiver", "test.PhasedReceiver"),
            )
            .unwrap();
            std::fs::write(
                f.config(),
                std::fs::read_to_string(f.config())
                    .unwrap()
                    .replace("test.profile.v1", "test.phased.v1"),
            )
            .unwrap();
            let mut r = registry(false);
            r.register_event(EventDescriptor {
                schema: release(),
                phase: 0,
                validate,
            })
            .unwrap();
            for (key, direction) in [
                ("test.PhasedSender", Direction::Output),
                ("test.PhasedReceiver", Direction::Input),
            ] {
                let mut d = descriptor(key, direction);
                d.events.push(release());
                d.resources = vec!["A".into(), "B".into()];
                r.register_module(key, |_| Ok(Box::new(Phased)), d).unwrap();
            }
            let mut profile = r.profile("test.profile.v1").unwrap().clone();
            profile.name = "test.phased.v1".into();
            profile.modules = vec!["test.PhasedSender".into(), "test.PhasedReceiver".into()];
            profile.events.push(release());
            r.register_profile(profile).unwrap();
            let snapshot =
                runtime::simulate(&prepare_with_registry(&f.config(), r).unwrap()).unwrap();
            assert_eq!(snapshot.common.termination, "events_exhausted");
            let points = &snapshot.common.points;
            assert_eq!(
                points.iter().map(|p| p.value).collect::<Vec<_>>(),
                [0, 1, 11, 2, 4, 5, 3, 5]
            );
            assert!(points.iter().all(|p| p.time_ps == 10));
            assert_eq!(
                points
                    .iter()
                    .map(|p| p.event_seq.unwrap())
                    .collect::<Vec<_>>(),
                (0..8).collect::<Vec<_>>()
            );
            let typed = serde_json::to_value(&snapshot.registered).unwrap();
            if let Some(expected) = &reference {
                assert_eq!(&typed, expected);
            } else {
                reference = Some(typed);
            }
        }
    }
}
#[test]
fn panic_is_diagnostic_and_cleanup_receives_committed_prefix() {
    static FINISH_COUNT: AtomicU64 = AtomicU64::new(0);
    struct Panics;
    impl Model for Panics {
        fn on_event(&mut self, _: &Envelope, c: &mut Context<'_>) -> ModelResult {
            let id = c.self_id().to_string();
            c.observe("test.received", &id, 100)?;
            panic!("extension panic")
        }
        fn finish(&mut self, c: &FinishContext<'_>) -> ModelResult {
            assert_eq!(c.snapshot.common.committed_events, 2);
            assert_eq!(c.snapshot.common.points.len(), 1);
            FINISH_COUNT.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    let f = Fixture::new();
    let r = registry_factories(|_| Ok(Box::new(Sender)), |_| Ok(Box::new(Panics)));
    let snapshot = runtime::simulate(&prepare_with_registry(&f.config(), r).unwrap()).unwrap();
    assert_eq!(snapshot.common.termination, "execution_failed");
    assert!(snapshot.common.diagnostics[0].message.contains("panicked"));
    assert_eq!(FINISH_COUNT.load(Ordering::Relaxed), 1);
}
#[test]
fn event_limit_and_half_open_time_boundary_are_enforced() {
    let f = Fixture::new();
    let mut prepared = prepare_with_registry(&f.config(), registry(false)).unwrap();
    prepared.common.max_events = 1;
    let failed = runtime::simulate(&prepared).unwrap();
    assert_eq!(failed.common.committed_events, 1);
    assert_eq!(failed.common.pending_events, 2);
    assert_eq!(failed.common.diagnostics[0].code, "E-0004");
    prepared.common.max_events = 100;
    prepared.common.time_limit_ps = 5;
    let stopped = runtime::simulate(&prepared).unwrap();
    assert_eq!(stopped.common.termination, "time_limit");
    assert_eq!(stopped.common.pending_events, 1);
    assert_eq!(stopped.common.committed_events, 2);
}
#[test]
fn custom_profile_validates_owned_json_and_factory_receives_it() {
    let f = Fixture::new();
    std::fs::write(f.0.join("model.json"), r#"{"answer":42}"#).unwrap();
    let config = std::fs::read_to_string(f.config()).unwrap() + "model-config = \"model.json\"\n";
    std::fs::write(f.config(), config).unwrap();
    let mut r = registry(false);
    r.register_profile_input_validator("test.profile.v1", |input| {
        if input
            .model_config
            .as_ref()
            .and_then(|v| v["answer"].as_u64())
            == Some(42)
        {
            Ok(())
        } else {
            Err("answer missing".into())
        }
    })
    .unwrap();
    let prepared = prepare_with_registry(&f.config(), r).unwrap();
    assert_eq!(
        prepared.registered.unwrap().models[0]
            .profile_input
            .model_config
            .as_ref()
            .unwrap()["answer"],
        42
    );
}
#[test]
fn registered_builtin_adapter_preserves_builtin_result() {
    let config =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gateway/fanout.ini");
    let plain = dir_simulator::prepare(&config).unwrap();
    let registered = prepare_with_registry(&config, Registry::default()).unwrap();
    assert!(!registered.registered.as_ref().unwrap().is_generic());
    assert_eq!(plain.node_count(), registered.node_count());
    let a = runtime::simulate(&plain).unwrap();
    let b = runtime::simulate(&registered).unwrap();
    assert_eq!(a.common.committed_events, b.common.committed_events);
    assert_eq!(format!("{:?}", a.can), format!("{:?}", b.can));
}
#[test]
fn custom_run_publishes_registered_records_and_metrics() {
    let f = Fixture::new();
    let prepared = prepare_with_registry(&f.config(), registry(false)).unwrap();
    let report = dir_simulator::run(prepared, &f.0.join("results")).unwrap();
    assert_eq!(report.exit_code, 0);
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(f.0.join("results/results.json")).unwrap())
            .unwrap();
    assert_eq!(result["schema_version"], 2);
    assert_eq!(
        result["metadata"]["models"],
        serde_json::json!([
            {"type":"test.Receiver","version":"1","assumptions":[]},
            {"type":"test.Sender","version":"1","assumptions":[]}
        ])
    );
    assert_eq!(
        result["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(f.0.join("results/manifest.json").is_file());
}

#[test]
fn live_channel_settings_override_captured_initial_state_after_prepare() {
    let f = Fixture::new();
    let mut prepared = prepare_with_registry(&f.config(), registry(false)).unwrap();
    prepared.common.time_limit_ps = 0;
    let channel_id = prepared.registered.as_ref().unwrap().channels[0].id.clone();
    prepared.registered.as_mut().unwrap().channels[0]
        .parameters
        .insert("delay".into(), ParameterValue::Quantity(9));
    let output = f.0.join("adjusted-channel");
    dir_simulator::run(prepared, &output).unwrap();
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    for field in ["initial_channel_state", "initial_state"] {
        let channel = result["metadata"][field]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["instance"] == channel_id)
            .unwrap();
        let state: serde_json::Value =
            serde_json::from_str(channel["state"].as_str().unwrap()).unwrap();
        assert_eq!(state["settings"]["delay"], "9ps");
    }
    let provenance = result["metadata"]["value_provenance"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == format!("{channel_id}.delay"))
        .unwrap();
    assert_eq!(provenance["normalized_value"], "3ps");
    let config = result["metadata"]["config"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == format!("{channel_id}.delay"))
        .unwrap();
    assert_eq!(config["value"], "9ps");
}

#[test]
fn custom_preparation_failure_uses_registered_implementation_versions() {
    let f = Fixture::new();
    let config = std::fs::read_to_string(f.config()).unwrap();
    std::fs::write(f.config(), format!("{config}max-events = 0\n")).unwrap();
    let output = f.0.join("failed-results");
    let report =
        dir_simulator::run_config_with_registry(&f.config(), &output, registry(false)).unwrap();
    assert_eq!(report.exit_code, 2);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    assert_eq!(result["schema_version"], 2);
    assert_eq!(
        result["metadata"]["models"],
        serde_json::json!([
            {"type":"test.Receiver","version":"1","assumptions":[]},
            {"type":"test.Sender","version":"1","assumptions":[]}
        ])
    );
    assert_eq!(result["simulation"]["model_records"], serde_json::json!([]));
}

#[test]
fn custom_profile_can_use_a_builtin_name_without_inheriting_its_catalog() {
    let f = Fixture::new();
    let mut r = registry_factories_in(
        Registry::empty(),
        |_| Ok(Box::new(Sender)),
        |_| Ok(Box::new(Receiver { fail: false })),
    );
    let mut p = r.profile("test.profile.v1").unwrap().clone();
    p.name = "can.cc.ideal.v1".into();
    r.register_profile(p).unwrap();
    let config = std::fs::read_to_string(f.config())
        .unwrap()
        .replace("test.profile.v1", "can.cc.ideal.v1");
    std::fs::write(f.config(), format!("{config}max-events = 0\n")).unwrap();
    let output = f.0.join("named-profile-result");
    let report = dir_simulator::run_config_with_registry(&f.config(), &output, r).unwrap();
    assert_eq!(report.exit_code, 2);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    assert_eq!(result["schema_version"], 2);
    assert_eq!(
        result["metadata"]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["metric_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["test.arbitrations", "test.received"]
    );
    assert_eq!(
        result["metadata"]["model_schemas"],
        serde_json::json!([{"schema_name":"test.Record","schema_version":1}])
    );
}

fn custom_composed_name_uses_registered_models_and_catalog(profile: &str) {
    let f = Fixture::new();
    let mut r = registry_factories_in(
        Registry::empty(),
        |_| Ok(Box::new(Sender)),
        |_| Ok(Box::new(Receiver { fail: false })),
    );
    let mut descriptor = r.profile("test.profile.v1").unwrap().clone();
    descriptor.name = profile.into();
    r.register_profile(descriptor).unwrap();
    let config = std::fs::read_to_string(f.config())
        .unwrap()
        .replace("test.profile.v1", profile);
    std::fs::write(f.config(), &config).unwrap();
    let prepared = dir_simulator::registry::prepare_with_registry_and_source(
        &f.config(),
        &f.0,
        &dir_simulator::input::FsInputSource,
        r.clone(),
    )
    .unwrap();
    assert_eq!(prepared.node_count(), 2);
    assert_eq!(prepared.common.profile, profile);
    let output = f.0.join("success");
    let report = dir_simulator::run_config_with_registry(&f.config(), &output, r.clone()).unwrap();
    assert_eq!(report.exit_code, 0);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    let configs = result["metadata"]["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["logical_path"] == "config")
        .collect::<Vec<_>>();
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0]["content_utf8"], config);
    assert_eq!(result["simulation"]["committed_events"], "3");
    assert_eq!(result["simulation"]["model_records"][0]["data"]["byte"], 7);
    assert_custom_catalog(&result);

    std::fs::write(f.config(), format!("{config}max-events = 0\n")).unwrap();
    let failed = f.0.join("failed");
    let report = dir_simulator::run_config_with_registry(&f.config(), &failed, r).unwrap();
    assert_eq!(report.exit_code, 2);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(failed.join("results.json")).unwrap()).unwrap();
    assert_custom_catalog(&result);
}
fn assert_custom_catalog(result: &serde_json::Value) {
    assert_eq!(result["schema_version"], 2);
    assert_eq!(
        result["metadata"]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["metric_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["test.arbitrations", "test.received"]
    );
    assert_eq!(
        result["metadata"]["model_schemas"],
        serde_json::json!([{"schema_name":"test.Record","schema_version":1}])
    );
    assert_eq!(
        result["metadata"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["test.Receiver", "test.Sender"]
    );
}
#[test]
fn custom_dynamic_builtin_name_uses_custom_registry() {
    custom_composed_name_uses_registered_models_and_catalog("ethernet.l2.dynamic.v1");
}
#[test]
fn custom_tsn_builtin_name_uses_custom_registry() {
    custom_composed_name_uses_registered_models_and_catalog("ethernet.tsn.v1");
}
#[test]
fn custom_gateway_builtin_name_uses_custom_registry() {
    custom_composed_name_uses_registered_models_and_catalog("can.ethernet.gateway.v1");
}

#[test]
fn profile_rejects_module_schemas_outside_its_allowlist() {
    let f = Fixture::new();
    let mut r = registry(false);
    let mut profile = r.profile("test.profile.v1").unwrap().clone();
    profile.name = "test.restricted.v1".into();
    profile.events.clear();
    r.register_profile(profile).unwrap();
    let config = std::fs::read_to_string(f.config())
        .unwrap()
        .replace("test.profile.v1", "test.restricted.v1");
    std::fs::write(f.config(), config).unwrap();
    let error = prepare_with_registry(&f.config(), r).unwrap_err();
    assert!(error.message.contains("module event excluded by profile"));
}

#[test]
fn channel_capability_checks_descriptor_even_when_implementation_is_permissive() {
    struct Queries;
    impl Model for Queries {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            let channel = c.connections()[0].channels[0].clone();
            assert!(
                c.channel_capability(&channel, &Schema::new("test.propagation", 2), &[])
                    .is_err()
            );
            assert!(
                c.channel_capability(&channel, &Schema::new("test.other", 1), &[])
                    .is_err()
            );
            assert_eq!(
                c.channel_capability(&channel, &Schema::new("test.propagation", 1), &[])?,
                ParameterValue::Quantity(3)
            );
            Ok(())
        }
        fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
            Ok(())
        }
    }
    struct Permissive;
    impl Channel for Permissive {
        fn capability(&self, _: &Schema, _: u64, _: &[u8]) -> ModelResult<ParameterValue> {
            Ok(ParameterValue::Quantity(3))
        }
    }
    let f = Fixture::new();
    let mut r = registry_factories(
        |_| Ok(Box::new(Queries)),
        |_| Ok(Box::new(Receiver { fail: false })),
    );
    r.register_channel(
        "test.Permissive",
        |_| Ok(Box::new(Permissive)),
        ChannelDescriptor {
            implementation_key: "test.Permissive".into(),
            implementation_version: "1".into(),
            parameters: vec![ParameterDescriptor {
                name: "delay".into(),
                ned_type: "double".into(),
                dimension: Dimension::Time,
                minimum: None,
                maximum: None,
                required: true,
            }],
            capabilities: vec![Schema::new("test.propagation", 1)],
        },
    )
    .unwrap();
    let mut profile = r.profile("test.profile.v1").unwrap().clone();
    profile.name = "test.permissive.v1".into();
    profile.channels = vec!["test.Permissive".into()];
    r.register_profile(profile).unwrap();
    let ned = f.0.join("ned/demo/model.ned");
    std::fs::write(
        &ned,
        std::fs::read_to_string(&ned)
            .unwrap()
            .replace("test.Delay", "test.Permissive"),
    )
    .unwrap();
    std::fs::write(
        f.config(),
        std::fs::read_to_string(f.config())
            .unwrap()
            .replace("test.profile.v1", "test.permissive.v1"),
    )
    .unwrap();
    let snapshot = runtime::simulate(&prepare_with_registry(&f.config(), r).unwrap()).unwrap();
    assert_eq!(snapshot.common.termination, "events_exhausted");
    assert!(snapshot.common.diagnostics.is_empty());
}

#[test]
fn zero_time_limit_without_initial_events_still_reports_time_limit() {
    struct Idle;
    impl Model for Idle {
        fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
            Ok(())
        }
    }
    let f = Fixture::new();
    let r = registry_factories(|_| Ok(Box::new(Idle)), |_| Ok(Box::new(Idle)));
    let mut prepared = prepare_with_registry(&f.config(), r).unwrap();
    prepared.common.time_limit_ps = 0;
    let snapshot = runtime::simulate(&prepared).unwrap();
    assert_eq!(snapshot.common.termination, "time_limit");
    assert_eq!(snapshot.common.committed_events, 0);
    assert_eq!(snapshot.common.pending_events, 0);
}

#[test]
fn cancellation_excludes_current_timer_and_removes_future_tombstones_from_pending() {
    struct Cancels {
        current: Option<TimerToken>,
        future: Option<TimerToken>,
    }
    impl Model for Cancels {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            self.current = Some(c.schedule_at(1, schema(), vec![1])?);
            self.future = Some(c.schedule_at(19, schema(), vec![2])?);
            Ok(())
        }
        fn on_event(&mut self, _: &Envelope, c: &mut Context<'_>) -> ModelResult {
            assert!(!c.cancel(self.current.as_ref().unwrap()));
            assert!(c.cancel(self.future.as_ref().unwrap()));
            assert!(!c.cancel(self.future.as_ref().unwrap()));
            Ok(())
        }
    }
    let f = Fixture::new();
    let r = registry_factories(
        |_| {
            Ok(Box::new(Cancels {
                current: None,
                future: None,
            }))
        },
        |_| Ok(Box::new(Receiver { fail: false })),
    );
    let snapshot = runtime::simulate(&prepare_with_registry(&f.config(), r).unwrap()).unwrap();
    assert_eq!(snapshot.common.termination, "events_exhausted");
    assert_eq!(snapshot.common.committed_events, 1);
    assert_eq!(snapshot.common.pending_events, 0);
}

#[test]
fn custom_profile_cannot_publish_schema_two_layout_under_schema_one() {
    let f = Fixture::new();
    let mut r = registry(false);
    let mut profile = r.profile("test.profile.v1").unwrap().clone();
    profile.name = "test.legacy.v1".into();
    profile.output_schema_version = 1;
    r.register_profile(profile).unwrap();
    std::fs::write(
        f.config(),
        std::fs::read_to_string(f.config())
            .unwrap()
            .replace("test.profile.v1", "test.legacy.v1"),
    )
    .unwrap();
    assert!(
        prepare_with_registry(&f.config(), r)
            .unwrap_err()
            .message
            .contains("require output schema version 2")
    );
}

fn add_coordinator(
    mut registry: Registry,
    target: &str,
    outputs: Vec<BorrowedOutput>,
    factory: ModelFactory,
) -> Registry {
    registry
        .register_module(
            "test.Coordinator",
            factory,
            descriptor("test.Coordinator", Direction::Output),
        )
        .unwrap();
    let mut profile = registry.profile("test.profile.v1").unwrap().clone();
    profile.name = "test.coordinated.v1".into();
    profile.modules.push("test.Coordinator".into());
    registry.register_profile(profile).unwrap();
    registry
        .register_coordinator(
            "test.coordinated.v1",
            CoordinatorDescriptor {
                target: target.into(),
                implementation_key: "test.Coordinator".into(),
                parameters: BTreeMap::new(),
                outputs,
            },
        )
        .unwrap();
    registry
}
fn select_coordinator_profile(f: &Fixture) {
    std::fs::write(
        f.config(),
        std::fs::read_to_string(f.config())
            .unwrap()
            .replace("test.profile.v1", "test.coordinated.v1"),
    )
    .unwrap();
}

#[test]
fn coordinator_borrows_frozen_output_preserves_lineage_and_finishes_before_simples() {
    static ORDER: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Traced {
        id: String,
    }
    impl Model for Traced {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            ORDER.lock().unwrap().push(format!("init:{}", c.self_id()));
            if c.self_id().starts_with("@profile:") {
                assert_eq!(c.subject(), "Demo");
                // A model cannot turn an external path into a newly granted handle.
                assert!(c.send_at("Demo.a.out", 1, schema(), vec![7]).is_err());
                c.send_request_at("out", 1, schema(), vec![7], Some("root-request".into()))?;
                c.upsert_model_record(ModelRecord {
                    schema: Schema::new("test.Record", 1),
                    id: "coordinator".into(),
                    subject: c.subject().into(),
                    time_ps: 0,
                    data: serde_json::json!({"byte": 7}),
                })?;
                let subject = c.subject().to_owned();
                c.observe("test.received", &subject, 1)?;
            }
            Ok(())
        }
        fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
            assert_eq!(c.self_id(), "Demo.b");
            assert_eq!(e.source, "Demo.a");
            assert_eq!(e.request_id.as_deref(), Some("root-request"));
            assert_eq!(e.input_port.as_deref(), Some("in"));
            Ok(())
        }
        fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
            ORDER.lock().unwrap().push(format!("finish:{}", self.id));
            Ok(())
        }
    }
    let factory: ModelFactory = |config| {
        Ok(Box::new(Traced {
            id: config.id.clone(),
        }))
    };
    let registry = add_coordinator(
        registry_factories(factory, factory),
        "Demo",
        vec![BorrowedOutput {
            handle: "out".into(),
            source: "Demo.a".into(),
            output_port: "out".into(),
        }],
        factory,
    );
    let f = Fixture::new();
    select_coordinator_profile(&f);
    let prepared = prepare_with_registry(&f.config(), registry).unwrap();
    assert_eq!(prepared.node_count(), 2);
    let snapshot = runtime::simulate(&prepared).unwrap();
    assert_eq!(snapshot.common.termination, "events_exhausted");
    assert_eq!(snapshot.common.committed_events, 1);
    assert_eq!(snapshot.common.points[0].target, "Demo");
    assert_eq!(snapshot.common.points[0].event_seq, None);
    assert_eq!(snapshot.common.points[0].effect_seq, None);
    assert_eq!(
        snapshot
            .registered
            .unwrap()
            .model_record("test.Record", "coordinator")
            .unwrap()
            .subject,
        "Demo"
    );
    assert_eq!(
        *ORDER.lock().unwrap(),
        vec![
            "init:Demo.a",
            "init:Demo.b",
            "init:@profile:test.coordinated.v1:Demo",
            "finish:@profile:test.coordinated.v1:Demo",
            "finish:Demo.b",
            "finish:Demo.a",
        ]
    );
}

#[test]
fn coordinator_rejects_noncompound_targets_and_unowned_or_incompatible_outputs() {
    let f = Fixture::new();
    select_coordinator_profile(&f);
    for (target, source, port, handle, expected) in [
        ("Demo.a", "Demo.a", "out", "out", "not a compound"),
        ("Demo", "another.Network.a", "out", "out", "outside target"),
        (
            "Demo",
            "Demo.b",
            "in",
            "out",
            "unknown coordinator borrowed output",
        ),
        (
            "Demo",
            "Demo.a",
            "out",
            "unknown",
            "undeclared coordinator output handle",
        ),
    ] {
        let registry = add_coordinator(
            registry(false),
            target,
            vec![BorrowedOutput {
                handle: handle.into(),
                source: source.into(),
                output_port: port.into(),
            }],
            |_| Ok(Box::new(Sender)),
        );
        let error = prepare_with_registry(&f.config(), registry).unwrap_err();
        assert!(error.message.contains(expected), "{}", error.message);
    }
}

#[test]
fn failed_callback_does_not_commit_cancellation_of_an_existing_timer() {
    struct CancelsThenFails {
        future: Option<TimerToken>,
    }
    impl Model for CancelsThenFails {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            c.schedule_at(1, schema(), vec![1])?;
            self.future = Some(c.schedule_at(19, schema(), vec![2])?);
            Ok(())
        }
        fn on_event(&mut self, _: &Envelope, c: &mut Context<'_>) -> ModelResult {
            assert!(c.cancel(self.future.as_ref().unwrap()));
            let subject = c.subject().to_owned();
            c.observe("test.received", &subject, 10)?;
            Err("rollback cancellation".into())
        }
    }
    let f = Fixture::new();
    let r = registry_factories(
        |_| Ok(Box::new(CancelsThenFails { future: None })),
        |_| Ok(Box::new(Receiver { fail: false })),
    );
    let snapshot = runtime::simulate(&prepare_with_registry(&f.config(), r).unwrap()).unwrap();
    assert_eq!(snapshot.common.termination, "execution_failed");
    assert_eq!(snapshot.common.committed_events, 0);
    assert_eq!(snapshot.common.pending_events, 2);
    assert!(snapshot.common.points.is_empty());
}

#[test]
fn backwards_phase_runs_after_current_arbitration_and_hits_delta_guard_with_context() {
    fn complete() -> Schema {
        Schema::new("test.Complete", 1)
    }
    struct Phases;
    impl Model for Phases {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            c.schedule_at(1, schema(), vec![1])?;
            Ok(())
        }
        fn on_event(&mut self, event: &Envelope, c: &mut Context<'_>) -> ModelResult {
            if event.schema == schema() {
                let subject = c.subject().to_owned();
                c.observe("test.received", &subject, 1)?;
                c.schedule_at(c.now(), complete(), vec![1])?;
                c.request_arbitration("select")?;
                let subject = c.subject().to_owned();
                c.observe("test.received", &subject, 4)?;
            } else {
                let subject = c.subject().to_owned();
                c.observe("test.received", &subject, 3)?;
            }
            Ok(())
        }
        fn on_arbitration(&mut self, _: &str, c: &mut Context<'_>) -> ModelResult {
            let subject = c.subject().to_owned();
            c.observe("test.received", &subject, 2)
        }
    }
    let f = Fixture::new();
    let mut r = registry(false);
    r.register_event(EventDescriptor {
        schema: complete(),
        phase: 0,
        validate,
    })
    .unwrap();
    let mut module = descriptor("test.Phases", Direction::Output);
    module.events.push(complete());
    r.register_module("test.Phases", |_| Ok(Box::new(Phases)), module)
        .unwrap();
    let mut profile = r.profile("test.profile.v1").unwrap().clone();
    profile.name = "test.phases.v1".into();
    profile.modules.push("test.Phases".into());
    profile.events.push(complete());
    r.register_profile(profile).unwrap();
    let ned = f.0.join("ned/demo/model.ned");
    std::fs::write(
        &ned,
        std::fs::read_to_string(&ned)
            .unwrap()
            .replace("test.Sender", "test.Phases"),
    )
    .unwrap();
    std::fs::write(
        f.config(),
        std::fs::read_to_string(f.config())
            .unwrap()
            .replace("test.profile.v1", "test.phases.v1"),
    )
    .unwrap();
    let mut prepared = prepare_with_registry(&f.config(), r).unwrap();
    let snapshot = runtime::simulate(&prepared).unwrap();
    assert_eq!(
        snapshot
            .common
            .points
            .iter()
            .map(|point| point.value)
            .collect::<Vec<_>>(),
        vec![1, 4, 2, 3]
    );
    assert_eq!(
        snapshot
            .common
            .points
            .iter()
            .map(|point| point.event_seq)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(0), Some(2), Some(1)]
    );
    assert_eq!(
        snapshot
            .common
            .points
            .iter()
            .map(|point| point.effect_seq)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(0), Some(0)]
    );
    prepared.common.max_delta_cycles = 1;
    let failed = runtime::simulate(&prepared).unwrap();
    assert_eq!(failed.common.committed_events, 2);
    assert_eq!(failed.common.pending_events, 1);
    let diagnostic = &failed.common.diagnostics[0];
    assert_eq!(diagnostic.code, "E-0004");
    assert_eq!(diagnostic.reason, "delta_cycle_limit");
    assert_eq!(diagnostic.time_ps, Some(1));
    assert_eq!(diagnostic.event_seq, Some(1));
    assert_eq!(diagnostic.target.as_deref(), Some("Demo.a"));
}

#[test]
fn finish_can_retain_frozen_spool_after_full_buffer_and_tail_are_committed() {
    static RETAINED: Mutex<Option<std::sync::Arc<runtime::spool::PointSpool>>> = Mutex::new(None);
    struct Observes;
    impl Model for Observes {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            c.schedule_at(1, schema(), vec![1])?;
            Ok(())
        }
        fn on_event(&mut self, event: &Envelope, c: &mut Context<'_>) -> ModelResult {
            let count = if event.payload == [1] { 4096 } else { 1 };
            for _ in 0..count {
                let subject = c.subject().to_owned();
                c.observe("test.received", &subject, 1)?;
            }
            if event.payload == [1] {
                c.schedule_at(2, schema(), vec![2])?;
            }
            Ok(())
        }
        fn finish(&mut self, c: &FinishContext<'_>) -> ModelResult {
            assert!(c.snapshot.common.spool_error.is_none());
            assert!(c.snapshot.common.points.is_empty());
            assert_eq!(c.snapshot.common.iter_points().unwrap().count(), 4097);
            *RETAINED.lock().unwrap() = c.snapshot.common.point_spool.clone();
            Ok(())
        }
    }
    let f = Fixture::new();
    let registry = registry_factories(
        |_| Ok(Box::new(Observes)),
        |_| Ok(Box::new(Receiver { fail: false })),
    );
    let prepared = prepare_with_registry(&f.config(), registry).unwrap();
    let output = f.0.join("result");
    let report = dir_simulator::run(prepared, &output).unwrap();
    assert_eq!(report.termination, "events_exhausted");
    assert_eq!(report.exit_code, 0);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    assert_eq!(
        result["simulation"]["records"].as_array().unwrap().len(),
        4097
    );
    *RETAINED.lock().unwrap() = None;
    assert!(!std::fs::read_dir(&output).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".dir-observations-")
    }));
}

#[test]
fn generic_prepare_defers_model_factories_until_runtime() {
    static FACTORIES: AtomicU64 = AtomicU64::new(0);
    let f = Fixture::new();
    let r = registry_factories(
        |_| {
            FACTORIES.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(Sender))
        },
        |_| {
            FACTORIES.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(Receiver { fail: false }))
        },
    );
    FACTORIES.store(0, Ordering::Relaxed);
    let prepared = prepare_with_registry(&f.config(), r.clone()).unwrap();
    assert!(prepared.registered.as_ref().unwrap().is_generic());
    assert_eq!(FACTORIES.load(Ordering::Relaxed), 0);
    let snapshot = runtime::simulate(&prepared).unwrap();
    assert_eq!(snapshot.common.termination, "events_exhausted");
    assert_eq!(snapshot.common.committed_events, 3);
    assert_eq!(FACTORIES.load(Ordering::Relaxed), 2);
    FACTORIES.store(0, Ordering::Relaxed);
    let config = std::fs::read_to_string(f.config()).unwrap();
    std::fs::write(
        f.config(),
        config.replace("test.profile.v1", "test.missing.v1"),
    )
    .unwrap();
    assert!(prepare_with_registry(&f.config(), r).is_err());
    assert_eq!(FACTORIES.load(Ordering::Relaxed), 0);
}
#[test]
fn explicit_run_config_selects_custom_registry_instead_of_default() {
    let f = Fixture::new();
    let default = dir_simulator::run_config_with_registry(
        &f.config(),
        &f.0.join("default-results"),
        Registry::default(),
    )
    .unwrap();
    assert_eq!(default.exit_code, 2);
    assert_eq!(default.termination, "prep_failed");
    assert_eq!(default.committed_events, "0");
    let output = f.0.join("custom-results");
    let report =
        dir_simulator::run_config_with_registry(&f.config(), &output, registry(false)).unwrap();
    assert_eq!(report.exit_code, 0);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output.join("results.json")).unwrap()).unwrap();
    assert_eq!(
        result["simulation"]["model_records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(result["simulation"]["model_records"][0]["data"]["byte"], 7);
}
