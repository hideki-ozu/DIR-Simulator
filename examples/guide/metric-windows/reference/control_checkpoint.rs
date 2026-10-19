//! cargo run -p dir-simulator --example control_checkpoint -- save examples/control-checkpoint/scenario.ini /tmp/dir-checkpoint-demo
//! cargo run -p dir-simulator --example control_checkpoint -- restore /tmp/dir-checkpoint-demo
use dir_simulator::registry::*;
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, Controller, RunOptions, RunSession, RunState,
    prepare_with_registry, restore_checkpoint,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::PathBuf,
};
thread_local! {static CONTROLLER:RefCell<Option<Controller>>=const{RefCell::new(None)};static SHOULD_PAUSE:Cell<bool>=const{Cell::new(false)};}
struct Counter {
    count: u64,
    rng: u64,
    timers: Vec<TimerToken>,
}
fn tick() -> Schema {
    Schema::new("demo.Tick", 1)
}
impl Model for Counter {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        for (i, time) in [3, 7, 7, 8].iter().enumerate() {
            self.timers
                .push(c.schedule_at(*time, tick(), vec![i as u8])?);
        }
        c.cancel(&self.timers[3]);
        Ok(())
    }
    fn on_event(&mut self, event: &Envelope, c: &mut Context<'_>) -> ModelResult {
        self.count += 1;
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let subject = c.subject().to_owned();
        if event.payload[0] == 0 {
            c.set_gauge_u64("demo.depth", &subject, 4)?;
            c.observe_point_u64("demo.minimum", &subject, 5)?;
        }
        if event.payload[0] == 1 {
            c.set_gauge_u64("demo.depth", &subject, 9)?;
            c.set_gauge_u64("demo.depth", &subject, 1)?;
            c.observe_point_u64("demo.minimum", &subject, 1)?;
        }
        c.upsert_model_record(ModelRecord {
            schema: Schema::new("demo.Counter", 1),
            id: self.count.to_string(),
            subject,
            time_ps: c.now(),
            data: json!({"count":self.count.to_string(),"rng":self.rng.to_string()}),
        })?;
        if event.payload[0] == 1 && SHOULD_PAUSE.with(|p| p.replace(false)) {
            CONTROLLER
                .with(|controller| {
                    controller
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .submit(ControlCommand::Pause)
                })
                .map_err(|e| ModelError(e.to_string()))?;
        }
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        let refs = self
            .timers
            .iter()
            .map(|t| c.encode_timer_ref(t))
            .collect::<ModelResult<Vec<_>>>()?;
        serde_json::to_vec(
            &json!({"count":self.count.to_string(),"rng":self.rng.to_string(),"timers":refs}),
        )
        .map_err(|e| ModelError(e.to_string()))
    }
}
fn registry() -> Result<Registry, Box<dyn std::error::Error>> {
    let mut r = Registry::empty();
    r.register_event(EventDescriptor {
        schema: tick(),
        phase: 1,
        validate: |v| {
            if v.len() == 1 {
                Ok(())
            } else {
                Err("one byte expected".into())
            }
        },
    })?;
    r.register_module(
        "demo.Counter",
        |_| {
            Ok(Box::new(Counter {
                count: 0,
                rng: 17,
                timers: vec![],
            }))
        },
        ModuleDescriptor {
            implementation_key: "demo.Counter".into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![tick()],
            resources: vec![],
        },
    )?;
    for (name, sampling, aggregation, initial) in [
        (
            "demo.depth",
            MetricSampling::Gauge,
            MetricAggregation::TimeMean,
            BTreeMap::from([("Demo.counter".into(), 2)]),
        ),
        (
            "demo.minimum",
            MetricSampling::Point,
            MetricAggregation::Min,
            BTreeMap::new(),
        ),
    ] {
        r.register_metric_v2(
            MetricDescriptorV2 {
                name: name.into(),
                unit: "items".into(),
                version: 1,
                value_kind: "integer".into(),
                sampling,
                aggregation,
            },
            vec!["Demo.counter".into()],
            initial,
        )?;
    }
    r.register_model_record(ModelRecordDescriptor {
        schema: Schema::new("demo.Counter", 1),
        validate: |v| {
            if v.is_object() {
                Ok(())
            } else {
                Err("object expected".into())
            }
        },
    })?;
    r.register_profile(ProfileDescriptor {
        name: "demo.control.v1".into(),
        implementation_version: "1".into(),
        modules: vec!["demo.Counter".into()],
        channels: vec![],
        events: vec![tick()],
        metrics: vec!["demo.depth".into(), "demo.minimum".into()],
        model_records: vec![Schema::new("demo.Counter", 1)],
        output_schema_version: 3,
        validate: |_, _| Ok(()),
    })?;
    r.register_execution_capabilities(
        "demo.control.v1",
        ExecutionCapabilities {
            cooperative_control: true,
            metrics_v2: true,
            checkpoint: true,
        },
    )?;
    r.register_model_checkpoint_codec(
        CheckpointCodecDescriptor {
            implementation_key: "demo.Counter".into(),
            implementation_version: "1".into(),
            codec_name: "demo.json".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 65536,
        },
        |_, _, bytes, c| {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|e| ModelError(e.to_string()))?;
            let refs: Vec<TimerReference> = serde_json::from_value(value["timers"].clone())
                .map_err(|e| ModelError(e.to_string()))?;
            let timers = refs
                .iter()
                .map(|r| c.decode_timer_ref(r))
                .collect::<ModelResult<Vec<_>>>()?;
            let number = |key: &str| {
                value[key]
                    .as_str()
                    .ok_or_else(|| ModelError("missing codec field".into()))?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| ModelError(e.to_string()))
            };
            Ok(Box::new(Counter {
                count: number("count")?,
                rng: number("rng")?,
                timers,
            }))
        },
    )?;
    Ok(r)
}
fn applied(ack: dir_simulator::ControlAck) -> Result<(), Box<dyn std::error::Error>> {
    if ack.status != ControlStatus::Applied {
        return Err(ack
            .error
            .unwrap_or_else(|| format!("{:?}", ack.status))
            .into());
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mode = args.first().map(String::as_str).unwrap_or("");
    if !matches!(mode, "save" | "restore" | "run")
        || args.len() != if mode == "restore" { 2 } else { 3 }
    {
        return Err("usage: control_checkpoint save|run CONFIG OUTPUT_ROOT; control_checkpoint restore OUTPUT_ROOT".into());
    }
    let root = std::env::current_dir()?.join(args.last().unwrap());
    if mode == "restore" {
        let (mut session, token) = restore_checkpoint(
            &root.join("checkpoint"),
            registry()?,
            CheckpointLimits::default(),
        )?;
        applied(session.resume(token)?)?;
        assert_eq!(session.run_until_yield()?, RunState::Completed);
        applied(session.export(&root.join("restored"))?)?;
        println!(
            "restored result: {}",
            root.join("restored/results.json").display()
        );
    } else {
        std::fs::create_dir_all(&root)?;
        let prepared = prepare_with_registry(&PathBuf::from(&args[1]), registry()?)?;
        let (mut session, controller) = RunSession::new(
            prepared,
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )?;
        CONTROLLER.with(|c| *c.borrow_mut() = Some(controller));
        SHOULD_PAUSE.with(|p| p.set(mode == "save"));
        let state = session.run_until_yield()?;
        if mode == "save" {
            assert_eq!(state, RunState::Paused);
            applied(session.export(&root.join("paused"))?)?;
            let ack = session.checkpoint(&root.join("checkpoint"), CheckpointLimits::default())?;
            println!(
                "checkpoint receipt: {}",
                serde_json::to_string(&ack.checkpoint_receipt)?
            );
            applied(ack)?;
        } else {
            assert_eq!(state, RunState::Completed);
            applied(session.export(&root.join("completed"))?)?;
            println!(
                "completed result: {}",
                root.join("completed/results.json").display()
            );
        }
    }
    Ok(())
}
