//! Genuine model-v1 producer and model-v2 consumer with different state codecs.
use dir_simulator::prepare_with_registry;
use dir_simulator::registry::*;
use dir_simulator::runtime::{checkpoint_bundle, session::*};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
thread_local! { static CALLS:RefCell<Vec<String>>=const { RefCell::new(Vec::new()) }; }
fn call(s: &str) {
    CALLS.with(|v| v.borrow_mut().push(s.into()));
}
fn schema() -> Schema {
    Schema::new("test.migration.Tick", 1)
}
fn record() -> Schema {
    Schema::new("test.migration.Count", 1)
}
fn audit(n: u64) -> u64 {
    [1u64, 3, 7]
        .into_iter()
        .take(n as usize)
        .fold(0, |sum, time| sum ^ (time + 1))
}
struct Old {
    count: u64,
    timer: Option<TimerToken>,
}
struct New {
    count: u64,
    audit: u64,
    timer: Option<TimerToken>,
}
fn initialize(c: &mut Context<'_>) -> ModelResult<TimerToken> {
    call("initialize");
    c.schedule_at(1, schema(), vec![0])
}
fn tick(count: u64, e: &Envelope, c: &mut Context<'_>) -> ModelResult<Option<TimerToken>> {
    call("event");
    if e.payload != vec![(count - 1) as u8] {
        return Err("wrong tick ordinal".into());
    }
    c.upsert_model_record(ModelRecord {
        schema: record(),
        id: count.to_string(),
        subject: c.subject().into(),
        time_ps: c.now(),
        data: json!({"count":count.to_string()}),
    })?;
    if count < 3 {
        Ok(Some(c.schedule_at(
            [1, 3, 7][count as usize],
            schema(),
            vec![count as u8],
        )?))
    } else {
        Ok(None)
    }
}
impl Model for Old {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        self.timer = Some(initialize(c)?);
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        self.count += 1;
        self.timer = tick(self.count, e, c)?;
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        serde_json::to_vec(&json!({"legacy_count":self.count.to_string(),"next":self.timer.as_ref().map(|t|c.encode_timer_ref(t)).transpose()?})).map_err(|e|e.to_string().into())
    }
    fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
        call("finish");
        Ok(())
    }
}
impl Model for New {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        self.timer = Some(initialize(c)?);
        Ok(())
    }
    fn on_event(&mut self, e: &Envelope, c: &mut Context<'_>) -> ModelResult {
        self.count += 1;
        self.audit ^= c.now() + 1;
        self.timer = tick(self.count, e, c)?;
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        serde_json::to_vec(&json!({"completed":self.count.to_string(),"audit_xor":self.audit.to_string(),"timer":self.timer.as_ref().map(|t|c.encode_timer_ref(t)).transpose()?})).map_err(|e|e.to_string().into())
    }
    fn finish(&mut self, _: &FinishContext<'_>) -> ModelResult {
        call("finish");
        Ok(())
    }
}
fn descriptor(version: u32) -> CheckpointCodecDescriptor {
    CheckpointCodecDescriptor {
        implementation_key: "test.MigratingCounter".into(),
        implementation_version: version.to_string(),
        codec_name: "test.counter.state".into(),
        codec_version: version,
        external_state: "none".into(),
        max_bytes: 4096,
    }
}
fn count(v: &Value, key: &str) -> ModelResult<u64> {
    let value = v[key]
        .as_str()
        .ok_or_else(|| ModelError("missing counter".into()))?;
    let n: u64 = value
        .parse()
        .map_err(|_| ModelError("bad counter".into()))?;
    if n > 3 || value != n.to_string() {
        return Err("counter out of range".into());
    }
    Ok(n)
}
fn timer(v: &Value, c: &CheckpointDecodeContext<'_>, n: u64) -> ModelResult<Option<TimerToken>> {
    let reference: Option<TimerReference> =
        serde_json::from_value(v.clone()).map_err(|e| ModelError(e.to_string()))?;
    if reference.is_some() != (n < 3) {
        return Err("timer/counter mismatch".into());
    }
    reference
        .as_ref()
        .map(|t| c.decode_timer_ref(t))
        .transpose()
}
fn registry(version: u32) -> Registry {
    let mut r = Registry::empty();
    r.register_event(EventDescriptor {
        schema: schema(),
        phase: 1,
        validate: |p| {
            if p.len() == 1 && p[0] < 3 {
                Ok(())
            } else {
                Err("bad tick".into())
            }
        },
    })
    .unwrap();
    let factory: fn(&ModelConfig) -> ModelResult<Box<dyn Model>> = if version == 1 {
        |_| {
            Ok(Box::new(Old {
                count: 0,
                timer: None,
            }))
        }
    } else {
        |_| {
            Ok(Box::new(New {
                count: 0,
                audit: 0,
                timer: None,
            }))
        }
    };
    r.register_module(
        "test.MigratingCounter",
        factory,
        ModuleDescriptor {
            implementation_key: "test.MigratingCounter".into(),
            implementation_version: version.to_string(),
            parameters: vec![],
            ports: vec![],
            events: vec![schema()],
            resources: vec![],
        },
    )
    .unwrap();
    r.register_model_record(ModelRecordDescriptor {
        schema: record(),
        validate: |v| {
            if v.as_object().is_some_and(|o| o.len() == 1) && count(v, "count").is_ok() {
                Ok(())
            } else {
                Err("bad count record".into())
            }
        },
    })
    .unwrap();
    r.register_profile(ProfileDescriptor {
        name: "test.migration.v1".into(),
        implementation_version: version.to_string(),
        modules: vec!["test.MigratingCounter".into()],
        channels: vec![],
        events: vec![schema()],
        metrics: vec![],
        model_records: vec![record()],
        output_schema_version: 3,
        validate: |_, _| Ok(()),
    })
    .unwrap();
    r.register_execution_capabilities(
        "test.migration.v1",
        ExecutionCapabilities {
            cooperative_control: true,
            checkpoint: true,
            metrics_v2: false,
        },
    )
    .unwrap();
    if version == 1 {
        r.register_model_checkpoint_codec(descriptor(1), |_, version, bytes, c| {
            call("restore1");
            if version != 1 {
                return Err("old codec version".into());
            }
            let v: Value = serde_json::from_slice(bytes).map_err(|e| ModelError(e.to_string()))?;
            if v.as_object().is_none_or(|v| v.len() != 2) {
                return Err("old codec shape".into());
            }
            let n = count(&v, "legacy_count")?;
            Ok(Box::new(Old {
                count: n,
                timer: timer(&v["next"], c, n)?,
            }))
        })
        .unwrap();
    } else {
        r.register_model_checkpoint_codec(descriptor(2), |_, version, bytes, c| {
            call("restore2");
            if version != 2 {
                return Err("new codec version".into());
            }
            let v: Value = serde_json::from_slice(bytes).map_err(|e| ModelError(e.to_string()))?;
            if v.as_object().is_none_or(|v| v.len() != 3) {
                return Err("new codec shape".into());
            }
            let n = count(&v, "completed")?;
            let expected_audit = audit(n).to_string();
            if v["audit_xor"].as_str() != Some(expected_audit.as_str()) {
                return Err("counter checksum".into());
            }
            Ok(Box::new(New {
                count: n,
                audit: audit(n),
                timer: timer(&v["timer"], c, n)?,
            }))
        })
        .unwrap();
    }
    r
}
fn transform(payload: &mut MigrationPayload) -> ModelResult {
    let reference = payload.state["codec_refs"][0].clone();
    let old: Value =
        serde_json::from_slice(&payload.files[reference["blob_path"].as_str().unwrap()])
            .map_err(|e| ModelError(e.to_string()))?;
    let n = count(&old, "legacy_count")?;
    payload.replace_codec("model",reference["id"].as_str().unwrap(),&descriptor(2),serde_json::to_vec(&json!({"completed":n.to_string(),"audit_xor":audit(n).to_string(),"timer":old["next"]})).unwrap())
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-real-migration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/migration")).unwrap();
        std::fs::write(root.join("ned/migration/main.ned"),"package migration; simple Counter { parameters: @class(\"test.MigratingCounter\"); } network Main { submodules: count: migration.Counter; }").unwrap();
        std::fs::write(root.join("run.ini"),"[General]\nnetwork=migration.Main\nned-path=\"ned\"\nmodel-profile=\"test.migration.v1\"\nsim-time-limit=10ps\n").unwrap();
        Self(root)
    }
    fn session(&self, version: u32) -> (RunSession, Controller) {
        RunSession::new(
            prepare_with_registry(&self.0.join("run.ini"), registry(version)).unwrap(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap()
    }
    fn old_bundle(&self) -> PathBuf {
        let (mut s, controller) = self.session(1);
        s.advance_one().unwrap();
        s.advance_one().unwrap();
        controller.submit(ControlCommand::Pause).unwrap();
        s.poll_control().unwrap();
        let destination = self.0.join("old");
        assert_eq!(
            s.checkpoint(&destination, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        destination
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn adapter(path: &Path) -> CheckpointMigration {
    CheckpointMigration {
        id: "test.counter.v1-to-v2".into(),
        version: 1,
        source: checkpoint_identity(path, &CheckpointLimits::default()).unwrap(),
        target: current_checkpoint_identity(&registry(2), "test.migration.v1").unwrap(),
        transform,
    }
}
fn saved_model(path: &Path) -> Value {
    let (_, files) = checkpoint_bundle::read_bundle(path, &CheckpointLimits::default()).unwrap();
    let state: Value = serde_json::from_slice(&files["state.json"]).unwrap();
    serde_json::from_slice(&files[state["codec_refs"][0]["blob_path"].as_str().unwrap()]).unwrap()
}

#[test]
fn real_v1_state_migrates_offline_to_v2_and_continues_like_native_v2() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let old = fixture.old_bundle();
    let plan = adapter(&old);
    let limits = CheckpointLimits::default();
    let before = checkpoint_bundle::read_bundle(&old, &limits).unwrap();
    let source_bytes = std::fs::read(old.join("manifest.json")).unwrap();
    assert_eq!(saved_model(&old)["legacy_count"], "1");
    assert_eq!(
        restore_checkpoint(&old, registry(2), limits.clone())
            .err()
            .unwrap()
            .code,
        "FingerprintMismatch"
    );
    CALLS.with(|v| v.borrow_mut().clear());
    let migrated = fixture.0.join("migrated");
    migrate_checkpoint(
        &old,
        &migrated,
        registry(1),
        registry(2),
        &plan,
        limits.clone(),
    )
    .unwrap();
    assert_eq!(CALLS.with(|v| v.borrow().clone()), ["restore1", "restore2"]);
    assert_eq!(
        checkpoint_bundle::read_bundle(&old, &limits).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(old.join("manifest.json")).unwrap(),
        source_bytes
    );
    assert_eq!(
        checkpoint_identity(&migrated, &limits).unwrap(),
        plan.target
    );
    let (manifest, _) = checkpoint_bundle::read_bundle(&migrated, &limits).unwrap();
    assert_eq!(
        manifest["migration_history"][0]["source_manifest_sha256"],
        format!("{:x}", Sha256::digest(&source_bytes))
    );
    assert_eq!(manifest["run_lineage"], before.0["run_lineage"]);
    assert_eq!(saved_model(&migrated)["completed"], "1");
    assert_eq!(saved_model(&migrated)["audit_xor"], "2");

    let (mut restored, token) = restore_checkpoint(&migrated, registry(2), limits.clone()).unwrap();
    restored.resume(token).unwrap();
    restored.advance_one().unwrap();
    restored.controller().submit(ControlCommand::Pause).unwrap();
    restored.poll_control().unwrap();
    let next = fixture.0.join("continued-checkpoint");
    restored.checkpoint(&next, limits.clone()).unwrap();
    assert_eq!(saved_model(&next)["completed"], "2");
    assert_eq!(saved_model(&next)["audit_xor"], "6");
    assert_eq!(
        checkpoint_bundle::read_bundle(&next, &limits).unwrap().0["migration_history"],
        manifest["migration_history"]
    );
    let (mut continued, token) = restore_checkpoint(&next, registry(2), limits.clone()).unwrap();
    continued.resume(token).unwrap();
    assert_eq!(continued.run_until_yield().unwrap(), RunState::Completed);
    let (mut native, _) = fixture.session(2);
    assert_eq!(native.run_until_yield().unwrap(), RunState::Completed);
    continued
        .export(&fixture.0.join("migrated-result"))
        .unwrap();
    native.export(&fixture.0.join("native-result")).unwrap();
    let result = |name: &str| {
        serde_json::from_slice::<Value>(
            &std::fs::read(fixture.0.join(name).join("results.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(
        result("migrated-result")["simulation"],
        result("native-result")["simulation"]
    );
    let preserved = std::fs::read(migrated.join("manifest.json")).unwrap();
    assert!(migrate_checkpoint(&old, &migrated, registry(1), registry(2), &plan, limits).is_err());
    assert_eq!(
        std::fs::read(migrated.join("manifest.json")).unwrap(),
        preserved
    );
}

#[test]
fn unsupported_edges_and_invalid_transforms_never_publish() {
    let fixture = Fixture::new();
    let old = fixture.old_bundle();
    let limits = CheckpointLimits::default();
    let mut plan = adapter(&old);
    plan.source.codec_catalog[0]["codec_version"] = json!(99);
    let destination = fixture.0.join("unknown-edge");
    CALLS.with(|v| v.borrow_mut().clear());
    assert_eq!(
        migrate_checkpoint(
            &old,
            &destination,
            registry(1),
            registry(2),
            &plan,
            limits.clone()
        )
        .err()
        .unwrap()
        .code,
        "UnsupportedMigration"
    );
    assert!(CALLS.with(|v| v.borrow().is_empty()));
    assert!(!destination.exists());
    let mut plan = adapter(&old);
    plan.target.profile = "unknown.profile".into();
    let destination = fixture.0.join("wrong-target");
    assert_eq!(
        migrate_checkpoint(
            &old,
            &destination,
            registry(1),
            registry(2),
            &plan,
            limits.clone()
        )
        .err()
        .unwrap()
        .code,
        "UnsupportedMigration"
    );
    assert!(!destination.exists());
    let bad: [fn(&mut MigrationPayload) -> ModelResult; 5] = [
        |p| {
            transform(p)?;
            p.state["virtual_time_ps"] = json!("0");
            Ok(())
        },
        |p| {
            transform(p)?;
            p.state["last_committed_key"]["phase"] = json!(0);
            Ok(())
        },
        |p| {
            transform(p)?;
            p.state["journal"]["model_records"] = json!({"bad":false});
            Ok(())
        },
        |p| {
            transform(p)?;
            let reference = p.state["codec_refs"][0].clone();
            let mut blob: Value =
                serde_json::from_slice(&p.files[reference["blob_path"].as_str().unwrap()]).unwrap();
            blob["timer"]["id"] = json!("999");
            p.replace_codec(
                "model",
                reference["id"].as_str().unwrap(),
                &descriptor(2),
                serde_json::to_vec(&blob).unwrap(),
            )
        },
        |_| panic!("injected migration failure"),
    ];
    for (index, transform) in bad.into_iter().enumerate() {
        let mut plan = adapter(&old);
        plan.transform = transform;
        let destination = fixture.0.join(format!("invalid-{index}"));
        assert!(
            migrate_checkpoint(
                &old,
                &destination,
                registry(1),
                registry(2),
                &plan,
                limits.clone()
            )
            .is_err()
        );
        assert!(!destination.exists());
    }
}

#[test]
fn authentic_but_semantically_invalid_old_state_is_rejected_before_transform() {
    let fixture = Fixture::new();
    let old = fixture.old_bundle();
    let limits = CheckpointLimits::default();
    let (manifest, mut files) = checkpoint_bundle::read_bundle(&old, &limits).unwrap();
    let state: Value = serde_json::from_slice(&files["state.json"]).unwrap();
    let path = state["codec_refs"][0]["blob_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut blob: Value = serde_json::from_slice(&files[&path]).unwrap();
    blob["next"]["owner"] = json!("foreign");
    // The public bundle writer authenticates bytes; state semantics still need
    // the actual old codec, so reconstruct references with the real hash helper.
    let mut payload = MigrationPayload { state, files };
    payload
        .replace_codec(
            "model",
            "Main.count",
            &descriptor(1),
            serde_json::to_vec(&blob).unwrap(),
        )
        .unwrap();
    files = payload.files;
    files.insert(
        "state.json".into(),
        serde_json::to_vec(&payload.state).unwrap(),
    );
    let mut manifest = manifest;
    manifest["codecs"] = payload.state["codec_refs"].clone();
    let corrupt = fixture.0.join("malformed-old");
    checkpoint_bundle::write_bundle(&corrupt, manifest, files, &limits, 0).unwrap();
    let mut plan = adapter(&corrupt);
    plan.transform = |_| {
        call("transform");
        Ok(())
    };
    CALLS.with(|v| v.borrow_mut().clear());
    let destination = fixture.0.join("rejected-old");
    assert!(
        migrate_checkpoint(
            &corrupt,
            &destination,
            registry(1),
            registry(2),
            &plan,
            limits
        )
        .is_err()
    );
    assert!(!CALLS.with(|v| {
        v.borrow()
            .iter()
            .any(|s| s == "transform" || s == "event" || s == "finish" || s == "initialize")
    }));
    assert!(!destination.exists());
}
