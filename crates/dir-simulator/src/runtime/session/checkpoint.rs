//! Complete generic-engine checkpoints. Decode builds a private arena without callbacks.
use super::*;
use crate::input::{InputDirEntry, InputKind, InputMetadata, InputSource};
use crate::runtime::registered::{Engine, Work};
use std::collections::BTreeSet;
use std::io;
mod causal_prefix;
pub(super) mod migration;

fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new("CheckpointInvalid", message)
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ControlError> {
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}
fn u64value(v: &Value) -> Result<u64, ControlError> {
    let s = v
        .as_str()
        .ok_or_else(|| invalid("expected decimal string"))?;
    let n: u64 = s.parse().map_err(|_| invalid("invalid decimal u64"))?;
    if s != n.to_string() {
        return Err(invalid("noncanonical u64"));
    }
    Ok(n)
}
fn build() -> BTreeMap<String, String> {
    let values: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/build_provenance.rs"));
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
fn binary() -> Result<String, ControlError> {
    std::env::current_exe()
        .and_then(std::fs::read)
        .map(|b| sha(&b))
        .map_err(|e| ControlError::new("FingerprintMismatch", e.to_string()))
}
fn registry_catalog(registry: &Registry, profile: &str) -> Value {
    let p = &registry.profiles[profile];
    json!({"profile":{"name":p.name,"implementation_version":p.implementation_version,"modules":p.modules,"channels":p.channels,"events":p.events,"metrics":p.metrics,"model_records":p.model_records,"output_schema_version":p.output_schema_version},"modules":p.modules.iter().map(|k|(&registry.modules[k].descriptor.implementation_key,format!("{:?}",registry.modules[k].descriptor))).collect::<BTreeMap<_,_>>(),"channels":p.channels.iter().map(|k|(&registry.channels[k].descriptor.implementation_key,format!("{:?}",registry.channels[k].descriptor))).collect::<BTreeMap<_,_>>(),"events":p.events.iter().map(|s|json!({"schema":s,"phase":registry.events[s].phase})).collect::<Vec<_>>(),"metrics_v1":p.metrics.iter().filter_map(|k|registry.metrics.get(k)).map(|m|json!({"name":m.name,"unit":m.unit})).collect::<Vec<_>>(),"metrics_v2":p.metrics.iter().filter_map(|k|registry.metrics_v2.get(k)).collect::<Vec<_>>(),"metric_v2_limits":registry.metric_v2_limits,"capabilities":registry.execution_capabilities(profile),"model_codecs":p.modules.iter().filter_map(|k|registry.model_codecs.get(k).map(|c|&c.descriptor)).collect::<Vec<_>>(),"channel_codecs":p.channels.iter().filter_map(|k|registry.channel_codecs.get(k).map(|c|&c.descriptor)).collect::<Vec<_>>()})
}
fn merge_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .and_then(|n| n.extensions.as_ref())
        .is_some_and(|e| e.mac_merge.is_some())
}
fn distributed_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .and_then(|n| n.extensions.as_ref())
        .and_then(|e| e.replication.as_ref())
        .is_some_and(|r| {
            r.streams.iter().any(|s| {
                matches!(
                    s.placement,
                    crate::types::network::ReplicationPlacement::BridgePair(_)
                )
            })
        })
}
fn vector_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .and_then(|n| n.extensions.as_ref())
        .and_then(|e| e.replication.as_ref())
        .is_some_and(|r| {
            r.streams.iter().any(|s| {
                s.recovery_algorithm
                    == crate::types::network::ReplicationRecoveryAlgorithm::Vector8021cb2017
            })
        })
}
fn gptp_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .is_some_and(|network| network.gptp.is_some())
}
fn named_phy_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .is_some_and(|n| {
            n.phy_attachments
                .iter()
                .any(|p| !p.config.physical_ports.is_empty())
        })
}
fn phy_service_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .is_some_and(|n| n.has_phy_mac_service())
}
fn media_pause_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .is_some_and(|n| n.mac_control.is_some() || n.ethernet.media.is_some())
}
fn gateway_replication_codec(prepared: &PreparedSimulation) -> bool {
    prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref())
        .is_some_and(|network| network.has_gateway_replication())
}
#[allow(clippy::too_many_arguments)] // Preserve the established owner transaction interface.
fn codec_catalog(
    registry: &Registry,
    profile: &str,
    distributed: bool,
    mac_merge: bool,
    vector: bool,
    gptp: bool,
    named_phy: bool,
    phy_service: bool,
    gateway_replication: bool,
    media_pause: bool,
) -> Value {
    let selected = &registry.profiles[profile];
    if registry.builtin_profiles.contains(profile)
        && registry.execution_capabilities(profile).checkpoint
        && matches!(
            profile,
            "ethernet.l2.dynamic.v1"
                | "ethernet.tsn.v1"
                | "can.ethernet.gateway.v1"
                | "can.ethernet.tsn.v1"
                | "ethernet.redundancy.abstract.v1"
                | "ethernet.frer.rtag.v1"
                | "ethernet.network.composite.v1"
                | "ethernet.preemption.abstract.v1"
                | "ethernet.control.wire.experimental.v1"
        )
    {
        return json!([{"implementation_key":"dir.network.Coordinator","implementation_version":"1","codec_name":"dir.network.owned-state","codec_version":if media_pause {19} else if phy_service {18} else if named_phy {17} else if gateway_replication {16} else if gptp {14} else if vector && mac_merge {13} else if vector {12} else if mac_merge {11} else if distributed {10} else {1},"external_state":"none"}]);
    }
    json!(selected.modules.iter().filter_map(|k|registry.model_codecs.get(k).map(|c|&c.descriptor)).chain(selected.channels.iter().filter_map(|k|registry.channel_codecs.get(k).map(|c|&c.descriptor))).map(|d|json!({"implementation_key":d.implementation_key,"implementation_version":d.implementation_version,"codec_name":d.codec_name,"codec_version":d.codec_version,"external_state":d.external_state})).collect::<Vec<_>>())
}
fn input_catalog(prepared: &PreparedSimulation) -> Value {
    let mut inputs=prepared.common.inputs.iter().map(|i|json!({"path":i.path,"bytes":i.content.len().to_string(),"sha256":sha(i.content.as_bytes())})).collect::<Vec<_>>();
    inputs.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    json!(inputs)
}
pub(super) fn prepared_fingerprint(prepared: &PreparedSimulation) -> Result<String, ControlError> {
    let registry = prepared
        .registered
        .as_ref()
        .map(|r| registry_catalog(&r.registry, &prepared.common.profile));
    if let Some(registry) = registry {
        return Ok(sha(&serde_json::to_vec(
            &json!({"input":input_catalog(prepared),"registry":registry}),
        )
        .unwrap()));
    }
    Ok(sha(&serde_json::to_vec(&json!({"input":input_catalog(prepared),"profile":prepared.common.profile,"time_limit_ps":prepared.common.time_limit_ps.to_string(),"max_events":prepared.common.max_events.to_string(),"max_delta_cycles":prepared.common.max_delta_cycles.to_string(),"metrics_window_ps":prepared.common.metrics_window_ps.to_string()})).unwrap()))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputMapping {
    logical_path: PathBuf,
    bundle_path: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodecReference {
    kind: String,
    id: String,
    implementation_key: String,
    implementation_version: String,
    codec_name: String,
    codec_version: u32,
    blob_path: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommittedKey {
    time_ps: String,
    delta: String,
    phase: u8,
    sequence: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    engine_phase: String,
    virtual_time_ps: String,
    last_event_time_ps: Option<String>,
    commit_ordinal: String,
    next_event_id: String,
    next_sequence: String,
    last_committed_key: Option<CommittedKey>,
    fes: Vec<Value>,
    dirty: Vec<Value>,
    live_timers: Vec<Value>,
    initialized_model_ids: Vec<String>,
    constructed_channel_ids: Vec<String>,
    journal: Value,
    metric_accumulators: crate::runtime::metric_v2::MetricStateV2,
    source_dispatchers: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    network_backend: Option<Value>,
    codec_refs: Vec<CodecReference>,
    replay_log: Vec<ReplayEntry>,
    replay_cursor: String,
    next_boundary_seq: String,
    next_control_id: String,
    next_control_seq: String,
    next_pause_generation: String,
    config_path: PathBuf,
    input_mapping: Vec<InputMapping>,
    input_directories: Vec<PathBuf>,
    cwd: PathBuf,
    options: RunOptions,
    next_export_ordinal: String,
    next_checkpoint_ordinal: String,
}
fn envelope_value(envelope: &Envelope) -> Value {
    let mut v = json!(envelope);
    v["event_id"] = json!(envelope.event_id.to_string());
    v["time_ps"] = json!(envelope.time_ps.to_string());
    v
}
fn decode_envelope(mut v: Value) -> Result<Envelope, ControlError> {
    let id = u64value(&v["event_id"])?;
    let time = u64value(&v["time_ps"])?;
    v["event_id"] = json!(id);
    v["time_ps"] = json!(time);
    parse(v)
}
fn token_value(token: &TimerToken) -> Value {
    json!({"owner":token.owner,"id":token.id.to_string()})
}
fn decode_token(v: &Value) -> Result<TimerToken, ControlError> {
    Ok(TimerToken {
        owner: v["owner"]
            .as_str()
            .ok_or_else(|| invalid("timer owner missing"))?
            .into(),
        id: u64value(&v["id"])?,
    })
}
impl RunSession {
    pub(super) fn checkpoint_now(
        &mut self,
        destination: &Path,
        logical_destination: &Path,
        limits: &CheckpointLimits,
    ) -> Result<CheckpointReceipt, ControlError> {
        if !self.options.checkpoint_enabled {
            return Err(ControlError::new(
                "UnsupportedCapability",
                "checkpoint was not enabled",
            ));
        }
        if self.state != RunState::Paused {
            return Err(ControlError::new(
                "CheckpointInvalidState",
                "checkpoint requires Paused",
            ));
        }
        let ordinal = self.checkpoint_ordinal;
        let next = ordinal.checked_add(1).ok_or_else(|| {
            ControlError::new("ResourceExhausted", "checkpoint ordinals exhausted")
        })?;
        let previous = self.state;
        self.state = RunState::Exporting;
        let result =
            self.encode_checkpoint(destination, logical_destination, limits, ordinal, next);
        self.state = previous;
        if result.is_ok() {
            self.checkpoint_ordinal = next;
        }
        result
    }
    fn encode_checkpoint(
        &self,
        destination: &Path,
        logical_destination: &Path,
        limits: &CheckpointLimits,
        ordinal: u64,
        next: u64,
    ) -> Result<CheckpointReceipt, ControlError> {
        let engine = self.engine.registered()?;
        let registered = &engine.registered;
        let registry = &registered.registry;
        let mut files = BTreeMap::new();
        let mut inputs = Vec::new();
        let mut sorted_inputs = engine.prepared.common.inputs.iter().collect::<Vec<_>>();
        sorted_inputs.sort_by_key(|i| &i.path);
        for (index, input) in sorted_inputs.into_iter().enumerate() {
            let path = format!("inputs/{index}.bin");
            files.insert(path.clone(), input.content.as_bytes().to_vec());
            inputs.push(InputMapping {
                logical_path: input.path.clone(),
                bundle_path: path,
            });
        }
        let checkpoint_work = engine.checkpoint_work();
        let reservations = checkpoint_work
            .iter()
            .map(|(_, work)| work)
            .filter_map(|w| match w {
                Work::Delivery { token: Some(t), .. } if engine.live.contains(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if reservations != engine.live {
            return Err(invalid("live timer reservation mismatch"));
        }
        let mut codecs = Vec::new();
        if self.initialized && registered.network.is_none() {
            for config in &registered.channels {
                let codec = &registry.channel_codecs[&config.implementation_key];
                let context = CheckpointEncodeContext {
                    owner: &config.id,
                    next_event_id: engine.event_id,
                    live_timers: &engine.live,
                    reservations: &reservations,
                };
                let bytes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.channels[&config.id].checkpoint(&context)
                }))
                .map_err(|_| ControlError::new("CheckpointFailed", "channel codec panicked"))?
                .map_err(|e| ControlError::new("CheckpointFailed", e.0))?;
                add_codec(
                    &mut files,
                    &mut codecs,
                    &codec.descriptor,
                    "channel",
                    &config.id,
                    bytes,
                    limits,
                )?;
            }
            for config in &registered.models {
                let codec = &registry.model_codecs[&config.implementation_key];
                let context = CheckpointEncodeContext {
                    owner: &config.id,
                    next_event_id: engine.event_id,
                    live_timers: &engine.live,
                    reservations: &reservations,
                };
                let bytes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.models[&config.id].checkpoint(&context)
                }))
                .map_err(|_| ControlError::new("CheckpointFailed", "model codec panicked"))?
                .map_err(|e| ControlError::new("CheckpointFailed", e.0))?;
                add_codec(
                    &mut files,
                    &mut codecs,
                    &codec.descriptor,
                    "model",
                    &config.id,
                    bytes,
                    limits,
                )?;
            }
        }
        let fes=checkpoint_work.iter().filter_map(|(key,work)|{
            if matches!(work,Work::Delivery{token:Some(t),..} if !engine.live.contains(t)){return None;}
            let work=match work{Work::Delivery{envelope,token}=>json!({"tag":"Delivery","envelope":envelope_value(envelope),"token":token.as_ref().map(token_value)}),Work::Arbitration{owner,resource}=>json!({"tag":"Arbitration","owner":owner,"resource":resource})};
            Some(json!({"time_ps":key.0.to_string(),"delta":key.1.to_string(),"phase":key.2,"sequence":key.3.to_string(),"work":work}))
        }).collect::<Vec<_>>();
        if fes.len() > limits.max_fes_entries as usize {
            return Err(invalid("FES entry limit exceeded"));
        }
        let points = engine
            .snapshot
            .common
            .iter_points()
            .map_err(ControlError::from)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(ControlError::from)?;
        let mut checkpoint_replay = self.replay.clone();
        checkpoint_replay.push(ReplayEntry {
            boundary_seq: self.next_boundary_seq - 1,
            boundary_kind: BoundaryKind::OwnerOperation,
            commit_ordinal: engine.snapshot.common.committed_events,
            request_order: 0,
            command: ControlCommand::Checkpoint {
                destination: logical_destination.into(),
                limits: limits.clone(),
            }
            .canonical(),
            status: ControlStatus::Applied,
            state: RunState::Paused,
        });
        let journal = json!({
            "model_records": engine.snapshot.registered.as_ref().unwrap().model_records,
            "deliveries": engine.snapshot.registered.as_ref().unwrap().deliveries,
            "points": points,
            "diagnostics": engine.snapshot.common.diagnostics,
            "termination": engine.snapshot.common.termination,
            "end_ps": engine.snapshot.common.end_ps.to_string(),
            "partial": engine.snapshot.common.partial
        });
        let dirty = engine.checkpoint_dirty().iter().map(|(time,delta,owner,resource)|json!({
            "time_ps":time.to_string(),"delta":delta.to_string(),"owner":owner,"resource":resource
        })).collect();
        let state = State {
            version: 1,
            engine_phase: if self.initialized {
                "initialized"
            } else {
                "uninitialized"
            }
            .into(),
            virtual_time_ps: self.horizon().to_string(),
            last_event_time_ps: engine
                .snapshot
                .common
                .last_event_time_ps
                .map(|time| time.to_string()),
            commit_ordinal: engine.snapshot.common.committed_events.to_string(),
            next_event_id: engine.event_id.to_string(),
            next_sequence: engine.sequence.to_string(),
            last_committed_key: engine
                .last_committed_key
                .map(|(time, delta, phase, sequence)| CommittedKey {
                    time_ps: time.to_string(),
                    delta: delta.to_string(),
                    phase,
                    sequence: sequence.to_string(),
                }),
            fes,
            dirty,
            live_timers: engine.live.iter().map(token_value).collect(),
            initialized_model_ids: engine.initialized.iter().cloned().collect(),
            constructed_channel_ids: engine.channels.keys().cloned().collect(),
            journal,
            metric_accumulators: engine
                .snapshot
                .registered
                .as_ref()
                .unwrap()
                .metrics_v2
                .clone(),
            source_dispatchers: Vec::new(),
            network_backend: engine.checkpoint_network(),
            codec_refs: codecs.clone(),
            replay_log: checkpoint_replay.clone(),
            replay_cursor: checkpoint_replay.len().to_string(),
            next_boundary_seq: self.next_boundary_seq.to_string(),
            next_control_id: self.controller.lock()?.next_id.to_string(),
            next_control_seq: self
                .next_control_seq
                .checked_add(1)
                .ok_or_else(|| invalid("control sequence exhausted"))?
                .to_string(),
            next_pause_generation: self.next_pause_generation.to_string(),
            config_path: engine.prepared.common.config_path.clone(),
            input_mapping: inputs,
            input_directories: engine
                .prepared
                .common
                .provenance
                .roots
                .iter()
                .map(|root| root.canonical_path.clone())
                .collect(),
            cwd: engine
                .prepared
                .common
                .config_path
                .parent()
                .unwrap_or(Path::new("/"))
                .into(),
            options: self.options.clone(),
            next_export_ordinal: self.export_ordinal.to_string(),
            next_checkpoint_ordinal: next.to_string(),
        };
        if state.network_backend.as_ref().is_some_and(|backend| {
            serde_json::to_vec(backend).map_or(true, |v| v.len() as u64 > limits.max_blob_bytes)
        }) {
            return Err(invalid("network backend blob limit exceeded"));
        }
        files.insert(
            "state.json".into(),
            serde_json::to_vec(&state).map_err(|e| invalid(e.to_string()))?,
        );
        let build = build();
        let selected = &registry.profiles[&engine.prepared.common.profile];
        let codec_catalog = codec_catalog(
            registry,
            &engine.prepared.common.profile,
            distributed_codec(&engine.prepared),
            merge_codec(&engine.prepared),
            vector_codec(&engine.prepared),
            gptp_codec(&engine.prepared),
            named_phy_codec(&engine.prepared),
            phy_service_codec(&engine.prepared),
            gateway_replication_codec(&engine.prepared),
            media_pause_codec(&engine.prepared),
        );
        let mut manifest = json!({"schema_name":"dir.checkpoint.bundle","schema_version":1,"binary_sha256":binary()?,"build_source_sha256":build.get("source_sha256").or_else(||build.get("build_source_sha256")).ok_or_else(||invalid("missing build source hash"))?,"cargo_lock_sha256":build.get("cargo_lock_sha256").ok_or_else(||invalid("missing lock hash"))?,"registry_fingerprint":sha(&serde_json::to_vec(&registry_catalog(registry,&engine.prepared.common.profile)).unwrap()),"input_fingerprint":sha(&serde_json::to_vec(&input_catalog(&engine.prepared)).unwrap()),"profile":engine.prepared.common.profile,"result_schema_version":selected.output_schema_version,"run_lineage":self.lineage,"checkpoint_ordinal":ordinal.to_string(),"commit_ordinal":state.commit_ordinal,"horizon_ps":state.virtual_time_ps,"state_path":"state.json","codec_catalog":codec_catalog,"codecs":codecs,"replay_sha256":sha(&serde_json::to_vec(&checkpoint_replay).unwrap())});
        if !self.migration_history.is_empty() {
            manifest["migration_history"] = json!(self.migration_history);
        }
        super::super::checkpoint_bundle::write_bundle(destination, manifest, files, limits, ordinal)
            .map_err(|e| ControlError::new("CheckpointFailed", e.0))
    }
}
fn add_codec(
    files: &mut BTreeMap<String, Vec<u8>>,
    refs: &mut Vec<CodecReference>,
    d: &CheckpointCodecDescriptor,
    kind: &str,
    id: &str,
    bytes: Vec<u8>,
    limits: &CheckpointLimits,
) -> Result<(), ControlError> {
    if bytes.len() as u64 > d.max_bytes || bytes.len() as u64 > limits.max_blob_bytes {
        return Err(invalid("codec blob size exceeded"));
    }
    let blob_path = format!("objects/{}.bin", sha(&bytes));
    files.entry(blob_path.clone()).or_insert(bytes);
    refs.push(CodecReference {
        kind: kind.into(),
        id: id.into(),
        implementation_key: d.implementation_key.clone(),
        implementation_version: d.implementation_version.clone(),
        codec_name: d.codec_name.clone(),
        codec_version: d.codec_version,
        blob_path,
    });
    Ok(())
}
struct FrozenInputSource {
    files: BTreeMap<PathBuf, String>,
    directories: BTreeSet<PathBuf>,
}
impl FrozenInputSource {
    fn new(
        mapping: &[InputMapping],
        declared_directories: &[PathBuf],
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, ControlError> {
        let mut captured = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let declared = declared_directories.iter().collect::<BTreeSet<_>>();
        if declared.len() != declared_directories.len() {
            return Err(invalid("duplicate captured input directory"));
        }
        for directory in declared_directories {
            if !directory.is_absolute()
                || directory.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
            {
                return Err(invalid("invalid captured input directory"));
            }
            for ancestor in directory.ancestors() {
                directories.insert(ancestor.to_path_buf());
            }
        }
        for entry in mapping {
            if !entry.logical_path.is_absolute() {
                return Err(invalid("input logical path must be absolute"));
            }
            let bytes = files
                .get(&entry.bundle_path)
                .ok_or_else(|| invalid("missing captured input"))?;
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| invalid("captured input is not UTF-8"))?;
            if captured.insert(entry.logical_path.clone(), text).is_some() {
                return Err(invalid("duplicate captured input"));
            }
            for parent in entry.logical_path.ancestors().skip(1) {
                directories.insert(parent.into());
            }
        }
        Ok(Self {
            files: captured,
            directories,
        })
    }
    fn missing() -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "MissingCapturedInput")
    }
}
impl InputSource for FrozenInputSource {
    fn metadata(&self, path: &Path) -> io::Result<InputMetadata> {
        if self.files.contains_key(path) {
            Ok(InputMetadata {
                kind: InputKind::File,
            })
        } else if self.directories.contains(path) {
            Ok(InputMetadata {
                kind: InputKind::Directory,
            })
        } else {
            Err(Self::missing())
        }
    }
    fn read_utf8(&self, path: &Path) -> io::Result<String> {
        self.files.get(path).cloned().ok_or_else(Self::missing)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<InputDirEntry>> {
        if !self.directories.contains(path) {
            return Err(Self::missing());
        }
        let mut entries = BTreeMap::new();
        for item in self.directories.iter().chain(self.files.keys()) {
            if item.parent() == Some(path) {
                entries.insert(
                    item.file_name().unwrap().to_os_string(),
                    if self.files.contains_key(item) {
                        InputKind::File
                    } else {
                        InputKind::Directory
                    },
                );
            }
        }
        Ok(entries
            .into_iter()
            .map(|(name, kind)| InputDirEntry {
                name,
                kind: Ok(kind),
            })
            .collect())
    }
}
fn captured_checkpoint_source(
    state: &State,
    manifest: &Value,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<FrozenInputSource, ControlError> {
    if !state.cwd.is_absolute() || manifest["codecs"] != json!(state.codec_refs) {
        return Err(invalid("manifest/state codec references differ"));
    }
    let input_paths = state
        .input_mapping
        .iter()
        .map(|m| m.bundle_path.as_str())
        .collect::<BTreeSet<_>>();
    let actual_inputs = files
        .keys()
        .filter(|p| p.starts_with("inputs/"))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let blobs = state
        .codec_refs
        .iter()
        .map(|r| r.blob_path.as_str())
        .collect::<BTreeSet<_>>();
    let actual_blobs = files
        .keys()
        .filter(|p| p.starts_with("objects/"))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if input_paths.len() != state.input_mapping.len()
        || input_paths != actual_inputs
        || blobs != actual_blobs
    {
        return Err(invalid("unreferenced or duplicate input/blob mapping"));
    }
    FrozenInputSource::new(&state.input_mapping, &state.input_directories, files)
}
fn prepare_checkpoint_input(
    state: &State,
    source: &FrozenInputSource,
    registry: Registry,
) -> Result<PreparedSimulation, ControlError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::registry::prepare_with_registry_and_source(
            &state.config_path,
            &state.cwd,
            source,
            registry,
        )
    }))
    .map_err(|_| ControlError::new("RestoreFailed", "input validator panicked"))?
    .map_err(|e| ControlError::new("FingerprintMismatch", e.to_string()))
}
pub fn restore_checkpoint(
    path: &Path,
    registry: Registry,
    limits: CheckpointLimits,
) -> Result<(RunSession, ResumeToken), ControlError> {
    limits.validate().map_err(|e| invalid(e.0))?;
    let (manifest, files) =
        super::super::checkpoint_bundle::read_bundle(path, &limits).map_err(|e| invalid(e.0))?;
    restore_bundle(
        &manifest,
        &files,
        registry,
        limits,
        &migration::ExecutionIdentity::current()?,
    )
}

fn restore_bundle(
    manifest: &Value,
    files: &BTreeMap<String, Vec<u8>>,
    registry: Registry,
    limits: CheckpointLimits,
    execution: &migration::ExecutionIdentity,
) -> Result<(RunSession, ResumeToken), ControlError> {
    execution.verify(manifest)?;
    let migration_history = migration::decode_history(manifest)?;
    let profile = manifest["profile"]
        .as_str()
        .ok_or_else(|| invalid("profile missing"))?;
    if !registry.profiles.contains_key(profile) {
        return Err(ControlError::new(
            "FingerprintMismatch",
            "profile unavailable",
        ));
    }
    let mut registry = registry;
    let mut state_early = None;
    let mut prepared_early = None;
    let current_fingerprint = json!(sha(&serde_json::to_vec(&registry_catalog(
        &registry, profile
    ))
    .unwrap()));
    if manifest["registry_fingerprint"] != current_fingerprint
        && manifest["result_schema_version"] == 3
        && registry.builtin_profiles.contains(profile)
        && matches!(
            profile,
            "ethernet.l2.dynamic.v1"
                | "ethernet.tsn.v1"
                | "can.ethernet.gateway.v1"
                | "can.ethernet.tsn.v1"
                | "ethernet.redundancy.abstract.v1"
                | "ethernet.frer.rtag.v1"
                | "ethernet.network.composite.v1"
                | "ethernet.preemption.abstract.v1"
                | "ethernet.control.wire.experimental.v1"
        )
    {
        let state: State = serde_json::from_slice(
            files
                .get("state.json")
                .ok_or_else(|| invalid("state missing"))?,
        )
        .map_err(|e| invalid(e.to_string()))?;
        let source = captured_checkpoint_source(&state, manifest, files)?;
        let prepared = prepare_checkpoint_input(&state, &source, registry.clone())?;
        registry = prepared
            .registered
            .as_ref()
            .ok_or_else(|| invalid("captured network registry missing"))?
            .registry
            .as_ref()
            .clone();
        state_early = Some(state);
        prepared_early = Some(prepared);
    }
    if manifest["registry_fingerprint"]
        != json!(sha(&serde_json::to_vec(&registry_catalog(
            &registry, profile
        ))
        .unwrap()))
    {
        return Err(ControlError::new(
            "FingerprintMismatch",
            "registry catalog differs",
        ));
    }
    let state: State = if let Some(state) = state_early {
        state
    } else {
        serde_json::from_slice(
            files
                .get("state.json")
                .ok_or_else(|| invalid("state missing"))?,
        )
        .map_err(|e| invalid(e.to_string()))?
    };
    let prepared = if let Some(prepared) = prepared_early {
        prepared
    } else {
        let source = captured_checkpoint_source(&state, manifest, files)?;
        prepare_checkpoint_input(&state, &source, registry.clone())?
    };
    if manifest["codec_catalog"]
        != codec_catalog(
            &registry,
            profile,
            distributed_codec(&prepared),
            merge_codec(&prepared),
            vector_codec(&prepared),
            gptp_codec(&prepared),
            named_phy_codec(&prepared),
            phy_service_codec(&prepared),
            gateway_replication_codec(&prepared),
            media_pause_codec(&prepared),
        )
    {
        return Err(ControlError::new(
            "UnsupportedCodec",
            "manifest codec catalog differs",
        ));
    }
    let prepared_directories = prepared
        .common
        .provenance
        .roots
        .iter()
        .map(|root| root.canonical_path.clone())
        .collect::<Vec<_>>();
    if prepared_directories != state.input_directories {
        return Err(ControlError::new(
            "FingerprintMismatch",
            "captured NED source roots differ",
        ));
    }
    if prepared.common.profile != profile
        || manifest["result_schema_version"]
            != json!(
                prepared.registered.as_ref().unwrap().registry.profiles[profile]
                    .output_schema_version
            )
        || manifest["input_fingerprint"]
            != json!(sha(&serde_json::to_vec(&input_catalog(&prepared)).unwrap()))
    {
        return Err(ControlError::new(
            "FingerprintMismatch",
            "captured input or selected schema differs",
        ));
    }
    let (mut session, _) = RunSession::new(prepared, state.options.clone())?;
    // No restore factory runs until all core references have been validated.
    restore_core(&mut session, &state, manifest, &limits)?;
    let verified_network = causal_prefix::validate(session.engine.registered()?, &state, &limits)?;
    let engine = session.engine.registered_mut()?;
    let registry = &engine.registered.registry;
    for reference in &state.codec_refs {
        let descriptor = if reference.kind == "model" {
            let config = engine
                .registered
                .models
                .iter()
                .find(|m| m.id == reference.id)
                .ok_or_else(|| invalid("unknown codec model"))?;
            if reference.implementation_key != config.implementation_key {
                return Err(invalid("codec/model implementation mismatch"));
            }
            &registry.model_codecs[&config.implementation_key].descriptor
        } else {
            let config = engine
                .registered
                .channels
                .iter()
                .find(|m| m.id == reference.id)
                .ok_or_else(|| invalid("unknown codec channel"))?;
            if reference.implementation_key != config.implementation_key {
                return Err(invalid("codec/channel implementation mismatch"));
            }
            &registry.channel_codecs[&config.implementation_key].descriptor
        };
        validate_codec_ref(reference, descriptor, files)?;
    }
    let reservations = engine
        .queue
        .values()
        .filter_map(|w| match w {
            Work::Delivery { token: Some(t), .. } => Some(t.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let pending_timers = engine
        .queue
        .values()
        .filter_map(|work| match work {
            Work::Delivery {
                token: Some(token),
                envelope,
            } if engine.live.contains(token) => Some((token.clone(), envelope.clone())),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    if state.engine_phase == "initialized" && engine.registered.network.is_none() {
        let configs = &engine.registered.channels;
        for config in configs {
            let c = state
                .codec_refs
                .iter()
                .find(|c| c.kind == "channel" && c.id == config.id)
                .ok_or_else(|| invalid("channel codec missing"))?;
            let codec = &registry.channel_codecs[&config.implementation_key];
            validate_codec_ref(c, &codec.descriptor, files)?;
            let context = CheckpointDecodeContext {
                owner: &config.id,
                next_event_id: engine.event_id,
                live_timers: &engine.live,
                reservations: &reservations,
                pending_timers: &pending_timers,
            };
            let instance = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (codec.restore)(config, c.codec_version, &files[&c.blob_path], &context)
            }))
            .map_err(|_| ControlError::new("RestoreFailed", "channel restore panicked"))?
            .map_err(|e| ControlError::new("RestoreFailed", e.0))?;
            engine.channels.insert(config.id.clone(), instance);
        }
        for config in &engine.registered.models {
            let c = state
                .codec_refs
                .iter()
                .find(|c| c.kind == "model" && c.id == config.id)
                .ok_or_else(|| invalid("model codec missing"))?;
            let codec = &registry.model_codecs[&config.implementation_key];
            validate_codec_ref(c, &codec.descriptor, files)?;
            let context = CheckpointDecodeContext {
                owner: &config.id,
                next_event_id: engine.event_id,
                live_timers: &engine.live,
                reservations: &reservations,
                pending_timers: &pending_timers,
            };
            let instance = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (codec.restore)(config, c.codec_version, &files[&c.blob_path], &context)
            }))
            .map_err(|_| ControlError::new("RestoreFailed", "model restore panicked"))?
            .map_err(|e| ControlError::new("RestoreFailed", e.0))?;
            engine.models.insert(config.id.clone(), instance);
        }
        engine.initialized = state.initialized_model_ids.iter().cloned().collect();
        session.initialized = true;
    }
    if engine.registered.network.is_some() && state.engine_phase == "initialized" {
        let horizon = u64value(&json!(state.virtual_time_ps))?;
        if let Some(replayed) = verified_network {
            engine
                .adopt_verified_network_from(replayed, horizon)
                .map_err(|e| invalid(e.message))?;
        } else {
            engine
                .restore_checkpoint_network(
                    state
                        .network_backend
                        .as_ref()
                        .ok_or_else(|| invalid("network backend checkpoint missing"))?,
                    horizon,
                )
                .map_err(|e| invalid(e.message))?;
        }
        session.initialized = true;
    }
    session.migration_history = migration_history;
    session.state = RunState::Running;
    session.pause()?;
    // The private-arena guard remains active through every fallible publication check.
    session.cleaned = false;
    let token = session.pause_token().unwrap();
    Ok((session, token))
}
fn validate_codec_ref(
    c: &CodecReference,
    d: &CheckpointCodecDescriptor,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ControlError> {
    if c.implementation_key != d.implementation_key
        || c.implementation_version != d.implementation_version
        || c.codec_name != d.codec_name
        || c.codec_version != d.codec_version
    {
        return Err(ControlError::new(
            "UnsupportedCodec",
            "codec reference differs",
        ));
    }
    let bytes = files
        .get(&c.blob_path)
        .ok_or_else(|| invalid("codec blob missing"))?;
    if bytes.len() as u64 > d.max_bytes || c.blob_path != format!("objects/{}.bin", sha(bytes)) {
        return Err(invalid("invalid codec blob"));
    }
    Ok(())
}
fn restore_core(
    session: &mut RunSession,
    state: &State,
    manifest: &Value,
    limits: &CheckpointLimits,
) -> Result<(), ControlError> {
    if session.engine.registered()?.registered.network.is_none() && state.network_backend.is_some()
    {
        return Err(invalid("unexpected network backend"));
    }
    if state.network_backend.as_ref().is_some_and(|backend| {
        serde_json::to_vec(backend).map_or(true, |v| v.len() as u64 > limits.max_blob_bytes)
    }) {
        return Err(invalid("network backend blob limit exceeded"));
    }
    if state.version != 1
        || !matches!(state.engine_phase.as_str(), "uninitialized" | "initialized")
        || !state.source_dispatchers.is_empty()
    {
        return Err(invalid("unsupported core state version or backend"));
    }
    let number = |s: &str| u64value(&json!(s));
    let horizon = number(&state.virtual_time_ps)?;
    let commit = number(&state.commit_ordinal)?;
    if manifest["commit_ordinal"] != json!(state.commit_ordinal)
        || manifest["horizon_ps"] != json!(state.virtual_time_ps)
    {
        return Err(invalid("manifest/state ordinal mismatch"));
    }
    let engine = session.engine.registered_mut()?;
    engine.event_id = number(&state.next_event_id)?;
    engine.sequence = number(&state.next_sequence)?;
    engine.snapshot.common.committed_events = commit;
    engine.snapshot.common.last_event_time_ps = state
        .last_event_time_ps
        .as_deref()
        .map(number)
        .transpose()?;
    if engine.snapshot.common.last_event_time_ps.unwrap_or(0) != horizon
        || horizon >= engine.prepared.common.time_limit_ps && horizon != 0
        || u64value(&state.journal["end_ps"])? != horizon
    {
        return Err(invalid("invalid checkpoint horizon"));
    }
    engine.last_committed_key = match (&state.last_committed_key, commit) {
        (None, 0) => {
            if engine.snapshot.common.last_event_time_ps.is_some() {
                return Err(invalid("uncommitted engine has a last event time"));
            }
            None
        }
        (Some(key), count) if count > 0 => {
            let time = number(&key.time_ps)?;
            let delta = number(&key.delta)?;
            let sequence = number(&key.sequence)?;
            if time != horizon
                || engine.snapshot.common.last_event_time_ps != Some(time)
                || delta >= engine.prepared.common.max_delta_cycles
                || key.phase > 2
                || sequence >= engine.sequence
            {
                return Err(invalid("invalid last committed key"));
            }
            Some((time, delta, key.phase, sequence))
        }
        _ => return Err(invalid("commit count and last committed key differ")),
    };
    let model_ids = engine
        .registered
        .models
        .iter()
        .map(|m| m.id.clone())
        .collect::<BTreeSet<_>>();
    let channel_ids = engine
        .registered
        .channels
        .iter()
        .map(|m| m.id.clone())
        .collect::<BTreeSet<_>>();
    if state.fes.len() > limits.max_fes_entries as usize {
        return Err(invalid("FES entry limit exceeded"));
    }
    let mut reservations = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    let mut event_ids = BTreeSet::new();
    for entry in &state.fes {
        let time = u64value(&entry["time_ps"])?;
        let delta = u64value(&entry["delta"])?;
        let phase = entry["phase"]
            .as_u64()
            .ok_or_else(|| invalid("phase missing"))?;
        let sequence = u64value(&entry["sequence"])?;
        if time < horizon
            || sequence >= engine.sequence
            || !sequences.insert(sequence)
            || delta >= engine.prepared.common.max_delta_cycles
        {
            return Err(invalid("invalid FES ordering or sequence"));
        }
        if engine
            .last_committed_key
            .is_some_and(|last| (time, delta, phase as u8, sequence) <= last)
        {
            return Err(invalid("FES entry precedes the last committed key"));
        }
        let work = &entry["work"];
        let work = match work["tag"].as_str() {
            Some("Delivery") => {
                let envelope = decode_envelope(work["envelope"].clone())?;
                if phase > 1
                    || envelope.time_ps != time
                    || !model_ids.contains(&envelope.destination)
                    || !model_ids.contains(&envelope.source)
                    || envelope.event_id >= engine.event_id
                    || !event_ids.insert(envelope.event_id)
                {
                    return Err(invalid("invalid delivery reference"));
                }
                let registry = &engine.registered.registry;
                let profile = &registry.profiles[&engine.prepared.common.profile];
                let event = registry
                    .events
                    .get(&envelope.schema)
                    .ok_or_else(|| invalid("unknown event schema"))?;
                if !profile.events.contains(&envelope.schema)
                    || u64::from(event.phase) != phase
                    || std::panic::catch_unwind(|| (event.validate)(&envelope.payload))
                        .map_or(true, |r| r.is_err())
                {
                    return Err(invalid("invalid delivery schema/payload"));
                }
                let token = if work["token"].is_null() {
                    None
                } else {
                    let token = decode_token(&work["token"])?;
                    if token.owner != envelope.destination
                        || token.owner != envelope.source
                        || token.id != envelope.event_id
                        || !reservations.insert(token.clone())
                    {
                        return Err(invalid("invalid timer reservation"));
                    }
                    Some(token)
                };
                Work::Delivery { envelope, token }
            }
            Some("Arbitration") => {
                let owner = work["owner"]
                    .as_str()
                    .ok_or_else(|| invalid("arbitration owner missing"))?;
                let resource = work["resource"]
                    .as_str()
                    .ok_or_else(|| invalid("arbitration resource missing"))?;
                validate_resource(engine, owner, resource)?;
                if phase != 2 {
                    return Err(invalid("arbitration phase must be 2"));
                }
                Work::Arbitration {
                    owner: owner.into(),
                    resource: resource.into(),
                }
            }
            _ => return Err(invalid("unknown work tag")),
        };
        if engine
            .queue
            .insert((time, delta, phase as u8, sequence), work)
            .is_some()
        {
            return Err(invalid("duplicate FES key"));
        }
    }
    for token in &state.live_timers {
        let token = decode_token(token)?;
        if token.id >= engine.event_id
            || !model_ids.contains(&token.owner)
            || !engine.live.insert(token)
        {
            return Err(invalid("invalid live token"));
        }
    }
    if reservations != engine.live {
        return Err(invalid("live timers and FES reservations differ"));
    }
    for entry in &state.dirty {
        let time = u64value(&entry["time_ps"])?;
        let delta = u64value(&entry["delta"])?;
        let owner = entry["owner"]
            .as_str()
            .ok_or_else(|| invalid("dirty owner missing"))?;
        let resource = entry["resource"]
            .as_str()
            .ok_or_else(|| invalid("dirty resource missing"))?;
        validate_resource(engine, owner, resource)?;
        if time < horizon
            || delta >= engine.prepared.common.max_delta_cycles
            || engine
                .last_committed_key
                .is_some_and(|last| (time, delta, 2) <= (last.0, last.1, last.2))
            || !engine
                .dirty
                .insert((time, delta, owner.into(), resource.into()))
        {
            return Err(invalid("invalid dirty resource"));
        }
    }
    let ref_ids = state
        .codec_refs
        .iter()
        .map(|r| (r.kind.clone(), r.id.clone()))
        .collect::<BTreeSet<_>>();
    if ref_ids.len() != state.codec_refs.len()
        || state.codec_refs.iter().any(|r| match r.kind.as_str() {
            "model" => !model_ids.contains(&r.id),
            "channel" => !channel_ids.contains(&r.id),
            _ => true,
        })
    {
        return Err(invalid("duplicate or unknown codec instance"));
    }
    let initialized = state
        .initialized_model_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let channels = state
        .constructed_channel_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if initialized.len() != state.initialized_model_ids.len()
        || channels.len() != state.constructed_channel_ids.len()
    {
        return Err(invalid("duplicate initialized ID"));
    }
    if state.engine_phase == "uninitialized" {
        if !initialized.is_empty()
            || !channels.is_empty()
            || !engine.queue.is_empty()
            || !engine.dirty.is_empty()
            || !engine.live.is_empty()
            || !state.codec_refs.is_empty()
            || state.network_backend.is_some()
            || commit != 0
            || engine.event_id != 0
            || engine.sequence != 0
            || horizon != 0
        {
            return Err(invalid("uninitialized checkpoint has runtime state"));
        }
    } else if engine.registered.network.is_some() {
        if !initialized.is_empty()
            || !channels.is_empty()
            || !state.codec_refs.is_empty()
            || state.network_backend.is_none()
        {
            return Err(invalid("invalid network backend completeness"));
        }
    } else if initialized != model_ids
        || channels != channel_ids
        || state.codec_refs.len() != model_ids.len() + channel_ids.len()
    {
        return Err(invalid("initialized arena is incomplete"));
    }
    if state.engine_phase == "uninitialized"
        && state.metric_accumulators != engine.snapshot.registered.as_ref().unwrap().metrics_v2
    {
        return Err(invalid(
            "uninitialized Metric V2 state differs from frozen initial catalog",
        ));
    }
    let expected = engine
        .snapshot
        .registered
        .as_ref()
        .unwrap()
        .metrics_v2
        .accumulators
        .clone();
    if state.metric_accumulators.accumulators.len() != expected.len() {
        return Err(invalid("metric catalog size mismatch"));
    }
    for (a, e) in state.metric_accumulators.accumulators.iter().zip(&expected) {
        if a.metric != e.metric
            || a.target != e.target
            || a.last_time > horizon
            || a.cursor > horizon
            || a.bucket_time.is_some_and(|t| t > horizon)
            || a.windows.keys().any(|start| {
                *start > horizon || *start % engine.prepared.common.metrics_window_ps != 0
            })
        {
            return Err(invalid("invalid metric accumulator reference"));
        }
    }
    let registered = engine.snapshot.registered.as_mut().unwrap();
    registered.metrics_v2 = state.metric_accumulators.clone();
    registered.model_records = parse(state.journal["model_records"].clone())?;
    registered.deliveries = parse(state.journal["deliveries"].clone())?;
    engine.snapshot.common.points = parse(state.journal["points"].clone())?;
    let metric_state = &engine.snapshot.registered.as_ref().unwrap().metrics_v2;
    if metric_state.next_record_sequence
        != engine.snapshot.common.points.len() as u64 + metric_state.records.len() as u64
        || metric_state
            .records
            .windows(2)
            .any(|pair| pair[0].seq >= pair[1].seq)
        || metric_state.records.iter().any(|record| {
            record.seq >= metric_state.next_record_sequence
                || record.time_ps > horizon
                || !expected
                    .iter()
                    .any(|a| a.metric == record.metric && a.target == record.target)
        })
    {
        return Err(invalid("metric record sequence or catalog mismatch"));
    }
    for (key, record) in &engine.snapshot.registered.as_ref().unwrap().model_records {
        if key != &format!("{}:{}", record.schema.name, record.id) {
            return Err(invalid("model record journal key differs from schema:id"));
        }
        let descriptor = engine
            .registered
            .registry
            .records
            .get(&record.schema)
            .ok_or_else(|| invalid("unknown journal model record schema"))?;
        if record.time_ps > horizon
            || !engine
                .registered
                .models
                .iter()
                .any(|m| m.subject == record.subject)
            || !record.data.is_object()
            || std::panic::catch_unwind(|| (descriptor.validate)(&record.data))
                .map_or(true, |r| r.is_err())
        {
            return Err(invalid("invalid model record journal"));
        }
    }
    if !state.journal["diagnostics"]
        .as_array()
        .ok_or_else(|| invalid("diagnostics missing"))?
        .is_empty()
    {
        return Err(invalid("paused checkpoint cannot have diagnostics"));
    }
    engine.snapshot.common.end_ps = u64value(&state.journal["end_ps"])?;
    engine.snapshot.common.termination = state.journal["termination"]
        .as_str()
        .ok_or_else(|| invalid("termination missing"))?
        .into();
    engine.snapshot.common.partial = state.journal["partial"]
        .as_bool()
        .ok_or_else(|| invalid("partial missing"))?;
    engine.snapshot.common.pending_events = (engine.queue.len() + engine.dirty.len()) as u64;
    session.next_boundary_seq = number(&state.next_boundary_seq)?;
    session.next_control_seq = number(&state.next_control_seq)?;
    session.next_pause_generation = number(&state.next_pause_generation)?;
    if session.next_pause_generation.checked_add(1).is_none() {
        return Err(ControlError::new(
            "ResourceExhausted",
            "restored pause generation exhausted",
        ));
    }
    session.export_ordinal = number(&state.next_export_ordinal)?;
    session.checkpoint_ordinal = number(&state.next_checkpoint_ordinal)?;
    if u64value(&manifest["checkpoint_ordinal"])?.checked_add(1) != Some(session.checkpoint_ordinal)
    {
        return Err(invalid("checkpoint ordinal mismatch"));
    }
    let next_id = number(&state.next_control_id)?;
    if session.next_control_seq > next_id || session.next_boundary_seq > next_id {
        return Err(invalid("control sequence exceeds accepted request count"));
    }
    session.controller.lock()?.next_id = next_id;
    if number(&state.replay_cursor)? != state.replay_log.len() as u64
        || manifest["replay_sha256"] != json!(sha(&serde_json::to_vec(&state.replay_log).unwrap()))
    {
        return Err(invalid("replay hash/cursor mismatch"));
    }
    session.replay = state.replay_log.clone();
    session.lineage = manifest["run_lineage"]
        .as_str()
        .ok_or_else(|| invalid("lineage missing"))?
        .into();
    if session.lineage.is_empty() {
        return Err(invalid("empty run lineage"));
    }
    // A failed restore owns private objects only; Drop must not call finish on them.
    session.cleaned = true;
    Ok(())
}
fn validate_resource(engine: &Engine, owner: &str, resource: &str) -> Result<(), ControlError> {
    let config = engine
        .registered
        .models
        .iter()
        .find(|m| m.id == owner)
        .ok_or_else(|| invalid("unknown resource owner"))?;
    if !engine.registered.registry.modules[&config.implementation_key]
        .descriptor
        .resources
        .iter()
        .any(|r| r == resource)
    {
        return Err(invalid("unknown resource"));
    }
    Ok(())
}
