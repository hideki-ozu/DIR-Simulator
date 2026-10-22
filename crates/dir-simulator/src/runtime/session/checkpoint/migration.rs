//! Explicit offline migration. Normal restore keeps its exact identity checks.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointIdentity {
    pub bundle_schema_version: u32,
    pub profile: String,
    pub result_schema_version: u32,
    pub binary_sha256: String,
    pub build_source_sha256: String,
    pub cargo_lock_sha256: String,
    pub registry_fingerprint: String,
    pub codec_catalog: Value,
}
#[derive(Clone)]
pub(super) struct ExecutionIdentity {
    binary: String,
    source: String,
    lock: String,
}
impl ExecutionIdentity {
    pub(super) fn current() -> Result<Self, ControlError> {
        let build = build();
        Ok(Self {
            binary: binary()?,
            source: build
                .get("source_sha256")
                .or_else(|| build.get("build_source_sha256"))
                .ok_or_else(|| invalid("build source hash missing"))?
                .clone(),
            lock: build
                .get("cargo_lock_sha256")
                .ok_or_else(|| invalid("lock hash missing"))?
                .clone(),
        })
    }
    pub(super) fn verify(&self, manifest: &Value) -> Result<(), ControlError> {
        for (name, expected) in [
            ("binary_sha256", &self.binary),
            ("build_source_sha256", &self.source),
            ("cargo_lock_sha256", &self.lock),
        ] {
            if manifest[name] != json!(expected) {
                return Err(ControlError::new(
                    "FingerprintMismatch",
                    format!("{name} differs"),
                ));
            }
        }
        Ok(())
    }
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl CheckpointIdentity {
    fn from_manifest(manifest: &Value) -> Result<Self, ControlError> {
        let text = |key: &str| {
            manifest[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(format!("missing {key}")))
        };
        let version = |key: &str| {
            manifest[key]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid(format!("invalid {key}")))
        };
        let identity = Self {
            bundle_schema_version: version("schema_version")?,
            profile: text("profile")?,
            result_schema_version: version("result_schema_version")?,
            binary_sha256: text("binary_sha256")?,
            build_source_sha256: text("build_source_sha256")?,
            cargo_lock_sha256: text("cargo_lock_sha256")?,
            registry_fingerprint: text("registry_fingerprint")?,
            codec_catalog: manifest
                .get("codec_catalog")
                .cloned()
                .ok_or_else(|| invalid("codec catalog missing"))?,
        };
        identity.validate()?;
        Ok(identity)
    }
    fn validate(&self) -> Result<(), ControlError> {
        if self.bundle_schema_version != 1
            || self.profile.is_empty()
            || self.result_schema_version == 0
            || !self.codec_catalog.is_array()
            || [
                &self.binary_sha256,
                &self.build_source_sha256,
                &self.cargo_lock_sha256,
                &self.registry_fingerprint,
            ]
            .iter()
            .any(|v| !hash_valid(v))
        {
            return Err(invalid("invalid migration identity"));
        }
        Ok(())
    }
    fn execution(&self) -> ExecutionIdentity {
        ExecutionIdentity {
            binary: self.binary_sha256.clone(),
            source: self.build_source_sha256.clone(),
            lock: self.cargo_lock_sha256.clone(),
        }
    }
}

/// Inspect a bundle without running factories. The caller must establish trust
/// in a source identity before allowing it in a migration descriptor.
pub fn checkpoint_identity(
    path: &Path,
    limits: &CheckpointLimits,
) -> Result<CheckpointIdentity, ControlError> {
    let (manifest, _) = super::super::super::checkpoint_bundle::read_bundle(path, limits)
        .map_err(|e| invalid(e.0))?;
    CheckpointIdentity::from_manifest(&manifest)
}
pub fn current_checkpoint_identity(
    registry: &Registry,
    profile: &str,
) -> Result<CheckpointIdentity, ControlError> {
    let selected = registry
        .profiles
        .get(profile)
        .ok_or_else(|| ControlError::new("UnsupportedMigration", "target profile unavailable"))?;
    let execution = ExecutionIdentity::current()?;
    let identity = CheckpointIdentity {
        bundle_schema_version: 1,
        profile: profile.into(),
        result_schema_version: selected.output_schema_version,
        binary_sha256: execution.binary,
        build_source_sha256: execution.source,
        cargo_lock_sha256: execution.lock,
        registry_fingerprint: sha(
            &serde_json::to_vec(&registry_catalog(registry, profile)).unwrap()
        ),
        codec_catalog: codec_catalog(
            registry, profile, false, false, false, false, false, false, false, false,
        ),
    };
    identity.validate()?;
    Ok(identity)
}

/// Owned, authenticated source contents. Adapters may change model state and
/// frozen inputs; target preparation and full private restore validate the result.
pub struct MigrationPayload {
    pub state: Value,
    pub files: BTreeMap<String, Vec<u8>>,
}
impl MigrationPayload {
    pub fn replace_codec(
        &mut self,
        kind: &str,
        id: &str,
        descriptor: &CheckpointCodecDescriptor,
        bytes: Vec<u8>,
    ) -> ModelResult {
        if !matches!(kind, "model" | "channel") {
            return Err("invalid codec migration kind".into());
        }
        let refs = self.state["codec_refs"]
            .as_array_mut()
            .ok_or_else(|| ModelError("codec references missing".into()))?;
        let reference = refs
            .iter_mut()
            .find(|r| r["kind"] == kind && r["id"] == id)
            .ok_or_else(|| ModelError("codec migration owner missing".into()))?;
        let old = reference["blob_path"]
            .as_str()
            .ok_or_else(|| ModelError("codec blob path missing".into()))?
            .to_owned();
        let path = format!("objects/{}.bin", sha(&bytes));
        *reference = json!({"kind":kind,"id":id,"implementation_key":descriptor.implementation_key,"implementation_version":descriptor.implementation_version,"codec_name":descriptor.codec_name,"codec_version":descriptor.codec_version,"blob_path":path});
        if !refs.iter().any(|r| r["blob_path"] == old) {
            self.files.remove(&old);
        }
        self.files.insert(path, bytes);
        Ok(())
    }
}
pub struct CheckpointMigration {
    pub id: String,
    pub version: u32,
    /// Trusted exact source execution, registry and codec identities.
    pub source: CheckpointIdentity,
    /// Exact target; execution hashes must identify this process's build.
    pub target: CheckpointIdentity,
    pub transform: fn(&mut MigrationPayload) -> ModelResult,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationStep {
    pub version: u32,
    pub migration_id: String,
    pub migration_version: u32,
    pub source_manifest_sha256: String,
    pub lineage: String,
    pub source: CheckpointIdentity,
    pub target: CheckpointIdentity,
}
pub(super) fn decode_history(manifest: &Value) -> Result<Vec<MigrationStep>, ControlError> {
    let history: Vec<MigrationStep> = match manifest.get("migration_history") {
        None => Vec::new(),
        Some(value) => serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))?,
    };
    if history.len() > 1024 {
        return Err(invalid("migration history limit"));
    }
    for (index, step) in history.iter().enumerate() {
        step.source.validate()?;
        step.target.validate()?;
        if step.version != 1
            || step.migration_id.is_empty()
            || step.migration_version == 0
            || !hash_valid(&step.source_manifest_sha256)
            || manifest["run_lineage"] != step.lineage
            || index > 0 && history[index - 1].target != step.source
        {
            return Err(invalid("invalid migration provenance chain"));
        }
    }
    if history.last().is_some_and(|last| {
        CheckpointIdentity::from_manifest(manifest).as_ref() != Ok(&last.target)
    }) {
        return Err(invalid("migration target identity differs"));
    }
    Ok(history)
}

/// Validate old and transformed arenas without executing initialize, event or
/// finish callbacks, then publish a new no-clobber bundle. Source is unchanged.
pub fn migrate_checkpoint(
    source: &Path,
    destination: &Path,
    source_registry: Registry,
    target_registry: Registry,
    adapter: &CheckpointMigration,
    limits: CheckpointLimits,
) -> Result<CheckpointReceipt, ControlError> {
    use super::super::super::checkpoint_bundle as bundle;
    limits.validate().map_err(|e| invalid(e.0))?;
    adapter.source.validate()?;
    adapter.target.validate()?;
    if adapter.id.is_empty() || adapter.version == 0 || adapter.source == adapter.target {
        return Err(ControlError::new(
            "UnsupportedMigration",
            "migration requires a named, distinct version edge",
        ));
    }
    let (source_hash, mut manifest, files) =
        bundle::read_bundle_with_manifest_hash(source, &limits).map_err(|e| invalid(e.0))?;
    if CheckpointIdentity::from_manifest(&manifest)? != adapter.source {
        return Err(ControlError::new(
            "UnsupportedMigration",
            "source schema, build, implementation or codec edge differs",
        ));
    }
    // The supplied old registry must truly decode the old state before transform.
    let (mut validated, _) = restore_bundle(
        &manifest,
        &files,
        source_registry,
        limits.clone(),
        &adapter.source.execution(),
    )?;
    validated.cleaned = true;
    drop(validated);
    let mut history = decode_history(&manifest)?;
    let source_state = bundle::strict_json(
        files
            .get("state.json")
            .ok_or_else(|| invalid("state missing"))?,
    )
    .map_err(|e| invalid(e.0))?;
    let mut payload = MigrationPayload {
        state: source_state.clone(),
        files,
    };
    payload.files.remove("state.json");
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (adapter.transform)(&mut payload)
    }))
    .map_err(|_| ControlError::new("MigrationFailed", "migration adapter panicked"))?
    .map_err(|e| ControlError::new("MigrationFailed", e.0))?;
    // A model migration cannot rewrite committed control history or its boundary.
    for key in [
        "version",
        "engine_phase",
        "virtual_time_ps",
        "last_event_time_ps",
        "commit_ordinal",
        "last_committed_key",
        "replay_log",
        "replay_cursor",
        "next_boundary_seq",
        "next_control_id",
        "next_control_seq",
        "next_pause_generation",
        "next_export_ordinal",
        "next_checkpoint_ordinal",
    ] {
        if payload.state[key] != source_state[key] {
            return Err(ControlError::new(
                "MigrationFailed",
                format!("migration changed control field {key}"),
            ));
        }
    }
    let state: State =
        serde_json::from_value(payload.state.clone()).map_err(|e| invalid(e.to_string()))?;
    manifest["codecs"] = json!(state.codec_refs);
    let captured = captured_checkpoint_source(&state, &manifest, &payload.files)?;
    let prepared = prepare_checkpoint_input(&state, &captured, target_registry)?;
    let registry = prepared
        .registered
        .as_ref()
        .ok_or_else(|| invalid("target registered backend missing"))?
        .registry
        .as_ref()
        .clone();
    let target_identity = current_checkpoint_identity(&registry, &prepared.common.profile)?;
    if target_identity != adapter.target {
        return Err(ControlError::new(
            "UnsupportedMigration",
            "prepared target differs from declared target edge",
        ));
    }
    for (key, value) in [
        ("binary_sha256", json!(target_identity.binary_sha256)),
        (
            "build_source_sha256",
            json!(target_identity.build_source_sha256),
        ),
        (
            "cargo_lock_sha256",
            json!(target_identity.cargo_lock_sha256),
        ),
        (
            "registry_fingerprint",
            json!(target_identity.registry_fingerprint),
        ),
        ("codec_catalog", target_identity.codec_catalog.clone()),
        ("profile", json!(target_identity.profile)),
        (
            "result_schema_version",
            json!(target_identity.result_schema_version),
        ),
        (
            "input_fingerprint",
            json!(sha(&serde_json::to_vec(&input_catalog(&prepared)).unwrap())),
        ),
    ] {
        manifest[key] = value;
    }
    history.push(MigrationStep {
        version: 1,
        migration_id: adapter.id.clone(),
        migration_version: adapter.version,
        source_manifest_sha256: source_hash,
        lineage: manifest["run_lineage"]
            .as_str()
            .ok_or_else(|| invalid("lineage missing"))?
            .into(),
        source: adapter.source.clone(),
        target: target_identity,
    });
    manifest["migration_history"] = json!(history);
    payload.files.insert(
        "state.json".into(),
        serde_json::to_vec(&state).map_err(|e| invalid(e.to_string()))?,
    );
    let ordinal = u64value(&manifest["checkpoint_ordinal"])?;
    let encoded = bundle::encode_manifest(manifest, &payload.files, &limits, ordinal)
        .map_err(|e| invalid(e.0))?;
    let manifest = bundle::strict_json(&encoded).map_err(|e| invalid(e.0))?;
    let (mut target, _) = restore_bundle(
        &manifest,
        &payload.files,
        registry,
        limits.clone(),
        &ExecutionIdentity::current()?,
    )?;
    target.cleaned = true;
    drop(target);
    bundle::write_bundle(destination, manifest, payload.files, &limits, ordinal)
        .map_err(|e| ControlError::new("MigrationFailed", e.0))
}
