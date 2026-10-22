//! Caller-driven cooperative sessions. Only the owner executes callbacks or I/O.
pub use super::checkpoint_bundle::{CheckpointDurability, CheckpointLimits, CheckpointReceipt};
use crate::registry::*;
use crate::{Diagnostic, PreparedSimulation};
use backend::SessionBackend;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
mod backend;
mod checkpoint;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    Ready,
    Initializing,
    Running,
    Paused,
    Exporting,
    Stopped,
    Completed,
    Failed,
    Closed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeToken {
    session_id: String,
    generation: u64,
    fingerprint: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunOptions {
    pub max_exports: u32,
    pub checkpoint_enabled: bool,
}
impl Default for RunOptions {
    fn default() -> Self {
        Self {
            max_exports: 1024,
            checkpoint_enabled: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlCommand {
    Pause,
    Resume(ResumeToken),
    Stop,
    Export {
        destination: PathBuf,
    },
    Checkpoint {
        destination: PathBuf,
        limits: CheckpointLimits,
    },
}
impl ControlCommand {
    fn canonical(&self) -> Value {
        match self {
            Self::Pause => json!({"command":"Pause"}),
            Self::Resume(t) => {
                json!({"command":"Resume","pause_generation":t.generation.to_string()})
            }
            Self::Stop => json!({"command":"Stop"}),
            Self::Export { destination } => json!({"command":"Export","destination":destination}),
            Self::Checkpoint {
                destination,
                limits,
            } => json!({"command":"Checkpoint","destination":destination,"limits":limits}),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlStatus {
    Applied,
    Pending,
    Superseded,
    InvalidState,
    Busy,
    EngineFailed,
    SessionClosed,
    ResourceExhausted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryKind {
    BeforeInitialize,
    AfterInitialize,
    AfterCommit,
    PausedPoll,
    OwnerOperation,
}
pub(crate) mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        let n: u64 = s.parse().map_err(serde::de::Error::custom)?;
        if s != n.to_string() {
            return Err(serde::de::Error::custom("noncanonical integer"));
        }
        Ok(n)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlAck {
    #[serde(with = "decimal")]
    pub request_id: u64,
    #[serde(with = "decimal")]
    pub control_seq: u64,
    #[serde(with = "decimal")]
    pub boundary_seq: u64,
    pub boundary_kind: BoundaryKind,
    #[serde(with = "decimal")]
    pub commit_ordinal: u64,
    #[serde(with = "decimal")]
    pub virtual_time_ps: u64,
    pub state: RunState,
    pub status: ControlStatus,
    pub export_ordinal: Option<String>,
    pub checkpoint_receipt: Option<CheckpointReceipt>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlError {
    pub code: String,
    pub message: String,
}
impl ControlError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ControlError {}
impl From<Diagnostic> for ControlError {
    fn from(e: Diagnostic) -> Self {
        Self::new(&e.code, e.message)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestHandle {
    pub request_id: u64,
    session_id: String,
}
#[derive(Clone)]
pub struct Controller {
    inner: Arc<Mutex<Mailbox>>,
    session_id: String,
}
struct Slot {
    command: ControlCommand,
    ack: Option<ControlAck>,
}
struct Mailbox {
    pending: VecDeque<u64>,
    slots: BTreeMap<u64, Slot>,
    next_id: u64,
    checkpoint_gate: bool,
    closed: bool,
}
impl Controller {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Mailbox>, ControlError> {
        self.inner
            .lock()
            .map_err(|_| ControlError::new("ControlUnavailable", "control mailbox mutex poisoned"))
    }
    fn enqueue(
        &self,
        command: ControlCommand,
        owner_checkpoint: bool,
    ) -> Result<RequestHandle, ControlError> {
        let mut mailbox = self.lock()?;
        if mailbox.closed {
            return Err(ControlError::new("SessionClosed", "session is closed"));
        }
        if mailbox.checkpoint_gate || mailbox.slots.len() == 64 {
            return Err(ControlError::new(
                "Busy",
                "control mailbox is full or gated",
            ));
        }
        if matches!(&command,ControlCommand::Export{destination}|ControlCommand::Checkpoint{destination,..} if !destination.is_absolute())
        {
            return Err(ControlError::new(
                "InvalidDestination",
                "controller commands require an absolute destination resolved from the caller's output root",
            ));
        }
        if matches!(command, ControlCommand::Checkpoint { .. }) && !owner_checkpoint {
            return Err(ControlError::new(
                "InvalidState",
                "checkpoint is an owner-only operation",
            ));
        }
        let id = mailbox.next_id;
        let next = id.checked_add(1).ok_or_else(|| {
            ControlError::new("ResourceExhausted", "control request IDs exhausted")
        })?;
        mailbox.slots.insert(id, Slot { command, ack: None });
        mailbox.pending.push_back(id);
        mailbox.next_id = next;
        if owner_checkpoint {
            mailbox.checkpoint_gate = true;
        }
        Ok(RequestHandle {
            request_id: id,
            session_id: self.session_id.clone(),
        })
    }
    pub fn submit(&self, command: ControlCommand) -> Result<RequestHandle, ControlError> {
        self.enqueue(command, false)
    }
    pub fn retry(
        &self,
        request_id: u64,
        command: ControlCommand,
    ) -> Result<RequestHandle, ControlError> {
        let mailbox = self.lock()?;
        if let Some(slot) = mailbox.slots.get(&request_id) {
            if slot.command != command {
                return Err(ControlError::new(
                    "RequestConflict",
                    "request ID has different canonical command",
                ));
            }
        } else {
            return Err(ControlError::new(
                if request_id < mailbox.next_id {
                    "DuplicateExpired"
                } else {
                    "UnknownRequest"
                },
                "request ID is not retained",
            ));
        }
        Ok(RequestHandle {
            request_id,
            session_id: self.session_id.clone(),
        })
    }
    pub fn ack(&self, handle: &RequestHandle) -> Result<Option<ControlAck>, ControlError> {
        if handle.session_id != self.session_id {
            return Err(ControlError::new(
                "UnknownRequest",
                "request belongs to another session",
            ));
        }
        let mailbox = self.lock()?;
        mailbox
            .slots
            .get(&handle.request_id)
            .map(|s| s.ack.clone())
            .ok_or_else(|| {
                ControlError::new(
                    if handle.request_id < mailbox.next_id {
                        "DuplicateExpired"
                    } else {
                        "UnknownRequest"
                    },
                    "request ID is not retained",
                )
            })
    }
    pub fn forget(&self, handle: &RequestHandle) -> Result<(), ControlError> {
        if handle.session_id != self.session_id {
            return Err(ControlError::new(
                "UnknownRequest",
                "request belongs to another session",
            ));
        }
        let mut mailbox = self.lock()?;
        match mailbox.slots.get(&handle.request_id) {
            Some(s) if s.ack.is_some() => {
                mailbox.slots.remove(&handle.request_id);
                Ok(())
            }
            Some(_) => Err(ControlError::new("Busy", "request is pending")),
            None => Err(ControlError::new(
                "DuplicateExpired",
                "request ID is not retained",
            )),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReplayEntry {
    #[serde(with = "decimal")]
    pub boundary_seq: u64,
    pub boundary_kind: BoundaryKind,
    #[serde(with = "decimal")]
    pub commit_ordinal: u64,
    pub request_order: u32,
    pub command: Value,
    pub status: ControlStatus,
    pub state: RunState,
}
#[derive(Clone, Debug, Default)]
pub struct CloseReport {
    pub diagnostics: Vec<Diagnostic>,
}
pub struct RunSession {
    engine: SessionBackend,
    controller: Controller,
    state: RunState,
    options: RunOptions,
    initialized: bool,
    cleaned: bool,
    token: Option<ResumeToken>,
    fingerprint: String,
    next_pause_generation: u64,
    next_control_seq: u64,
    next_boundary_seq: u64,
    export_ordinal: u64,
    checkpoint_ordinal: u64,
    replay: Vec<ReplayEntry>,
    output_root: PathBuf,
    lineage: String,
    migration_history: Vec<checkpoint::migration::MigrationStep>,
    replay_artifact_root: Option<PathBuf>,
    replay_destinations: BTreeMap<PathBuf, PathBuf>,
}
impl RunSession {
    pub fn new(
        prepared: impl Into<Arc<PreparedSimulation>>,
        options: RunOptions,
    ) -> Result<(Self, Controller), ControlError> {
        let prepared = prepared.into();
        if options.max_exports == 0 {
            return Err(ControlError::new(
                "InvalidOptions",
                "max_exports must be positive",
            ));
        }
        if let Some(registered) = prepared.registered.as_ref().filter(|r| r.is_generic()) {
            let capabilities = registered
                .registry
                .execution_capabilities(&prepared.common.profile);
            if !capabilities.cooperative_control {
                return Err(ControlError::new(
                    "UnsupportedCapability",
                    "profile does not declare cooperative control",
                ));
            }
            if options.checkpoint_enabled && !capabilities.checkpoint {
                return Err(ControlError::new(
                    "UnsupportedCapability",
                    "profile does not declare checkpoint capability",
                ));
            }
            if options.checkpoint_enabled && registered.network.is_none() {
                if !capabilities.checkpoint {
                    return Err(ControlError::new(
                        "UnsupportedCapability",
                        "profile does not declare checkpoint capability",
                    ));
                }
                let profile = &registered.registry.profiles[&prepared.common.profile];
                if profile
                    .modules
                    .iter()
                    .any(|key| !registered.registry.model_codecs.contains_key(key))
                    || profile
                        .channels
                        .iter()
                        .any(|key| !registered.registry.channel_codecs.contains_key(key))
                {
                    return Err(ControlError::new(
                        "UnsupportedCapability",
                        "selected implementation lacks an explicit codec",
                    ));
                }
                for model in &registered.models {
                    if !registered
                        .registry
                        .model_codecs
                        .contains_key(&model.implementation_key)
                    {
                        return Err(ControlError::new(
                            "UnsupportedCapability",
                            format!("missing model codec: {}", model.implementation_key),
                        ));
                    }
                }
                for channel in &registered.channels {
                    if !registered
                        .registry
                        .channel_codecs
                        .contains_key(&channel.implementation_key)
                    {
                        return Err(ControlError::new(
                            "UnsupportedCapability",
                            format!("missing channel codec: {}", channel.implementation_key),
                        ));
                    }
                }
            }
        } else if options.checkpoint_enabled {
            return Err(ControlError::new(
                "UnsupportedCapability",
                "legacy checkpoint codec is unavailable",
            ));
        }
        let identity = crate::output::new_run_identity()?;
        let fingerprint = checkpoint::prepared_fingerprint(&prepared)?;
        let controller = Controller {
            inner: Arc::new(Mutex::new(Mailbox {
                pending: VecDeque::new(),
                slots: BTreeMap::new(),
                next_id: 0,
                checkpoint_gate: false,
                closed: false,
            })),
            session_id: identity.run_id.clone(),
        };
        let mut engine = SessionBackend::new(prepared)?;
        // A controlled result exposes only the successful prefix until normal
        // completion. Keep the synchronous engine's initial observation T intact.
        engine.snapshot_mut().common.end_ps = 0;
        let session = Self {
            engine,
            controller: controller.clone(),
            state: RunState::Ready,
            options,
            initialized: false,
            cleaned: false,
            token: None,
            fingerprint,
            next_pause_generation: 0,
            next_control_seq: 0,
            next_boundary_seq: 0,
            export_ordinal: 0,
            checkpoint_ordinal: 0,
            replay: Vec::new(),
            output_root: std::env::current_dir()
                .map_err(|e| ControlError::new("InvalidOptions", e.to_string()))?,
            lineage: identity.run_id,
            migration_history: Vec::new(),
            replay_artifact_root: None,
            replay_destinations: BTreeMap::new(),
        };
        Ok((session, controller))
    }
    /// Private output root created by control replay. Artifacts are retained
    /// after Drop; the caller may remove this directory when no longer needed.
    pub fn replay_artifact_root(&self) -> Option<&Path> {
        self.replay_artifact_root.as_deref()
    }
    fn replay_destination(&mut self, id: u64, logical: &Path) -> PathBuf {
        if let Some(root) = &self.replay_artifact_root {
            self.replay_destinations
                .entry(logical.to_path_buf())
                .or_insert_with(|| root.join(format!("request-{id}")))
                .clone()
        } else {
            logical.to_path_buf()
        }
    }
    fn prepare_replay_artifacts(&mut self, log: &[ReplayEntry]) -> Result<(), ControlError> {
        if log.iter().any(|entry| {
            matches!(
                entry.command["command"].as_str(),
                Some("Export" | "Checkpoint")
            )
        }) {
            use std::os::unix::fs::DirBuilderExt;
            let root = self
                .output_root
                .join(std::env::temp_dir())
                .join(format!("dir-replay-{}", self.controller.session_id));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .map_err(|error| {
                    ControlError::new(
                        "ReplayOutputFailed",
                        format!("cannot create isolated replay output root: {error}"),
                    )
                })?;
            self.replay_artifact_root = Some(root);
        }
        Ok(())
    }
    /// A controller for this session, including a newly restored session.
    pub fn controller(&self) -> Controller {
        self.controller.clone()
    }
    pub fn state(&self) -> RunState {
        self.state
    }
    /// Current committed metadata and the most recently materialized row view.
    /// Use `observe` to refresh archive-backed rows at a caller observation boundary.
    pub fn snapshot(&self) -> &crate::snapshot::Snapshot {
        self.engine.snapshot()
    }
    /// Materialize a non-consuming prefix without advancing callbacks.
    pub fn observe(&mut self) -> Result<&crate::snapshot::Snapshot, ControlError> {
        self.engine.refresh_view(self.horizon())?;
        Ok(self.engine.snapshot())
    }
    pub fn prepared(&self) -> &PreparedSimulation {
        &self.engine.prepared
    }
    pub fn pause_token(&self) -> Option<ResumeToken> {
        self.token.clone()
    }
    pub fn replay_log(&self) -> &[ReplayEntry] {
        &self.replay
    }
    fn horizon(&self) -> u64 {
        match self.state {
            RunState::Completed => self.engine.prepared.common.time_limit_ps,
            RunState::Closed => self.engine.snapshot().common.end_ps,
            _ => self
                .engine
                .snapshot()
                .common
                .last_event_time_ps
                .unwrap_or(0),
        }
    }
    fn pause(&mut self) -> Result<(), ControlError> {
        if self.state != RunState::Paused {
            let generation = self.next_pause_generation;
            self.next_pause_generation = generation.checked_add(1).ok_or_else(|| {
                ControlError::new("ResourceExhausted", "pause generation exhausted")
            })?;
            self.token = Some(ResumeToken {
                session_id: self.controller.session_id.clone(),
                generation,
                fingerprint: self.fingerprint.clone(),
            });
        }
        self.state = RunState::Paused;
        Ok(())
    }
    fn cleanup(&mut self) {
        if self.state == RunState::Failed {
            self.engine.snapshot_mut().common.end_ps = self
                .engine
                .snapshot()
                .common
                .last_event_time_ps
                .unwrap_or(0);
        }
        if !self.cleaned {
            self.cleaned = true;
            if self.state == RunState::Failed {
                if let Err(mut diagnostic) = self
                    .engine
                    .refresh_view(self.engine.snapshot().common.end_ps)
                {
                    diagnostic.primary = false;
                    self.engine
                        .snapshot_mut()
                        .common
                        .diagnostics
                        .push(diagnostic);
                }
            }
            self.engine.finish();
        }
    }
    fn boundary(&mut self, kind: BoundaryKind, failed: bool) -> Result<(), ControlError> {
        let pending = {
            let mut mailbox = self.controller.lock()?;
            {
                let ids = mailbox.pending.drain(..).collect::<Vec<_>>();
                ids.into_iter()
                    .map(|id| {
                        let command = mailbox.slots[&id].command.clone();
                        (id, command)
                    })
                    .collect::<Vec<_>>()
            }
        };
        self.process(pending, kind, failed)
    }
    fn process(
        &mut self,
        pending: Vec<(u64, ControlCommand)>,
        kind: BoundaryKind,
        failed: bool,
    ) -> Result<(), ControlError> {
        if self.state != RunState::Closed && (!pending.is_empty() || !self.engine.is_legacy()) {
            // Also reached by replay's direct engine steps. Refresh at the
            // committed boundary before publishing acknowledgements or exports.
            let horizon = self.horizon();
            self.engine.snapshot_mut().common.end_ps = horizon;
            self.engine.refresh_view(horizon)?;
        }
        if pending.is_empty() {
            return Ok(());
        }
        let boundary = self.next_boundary_seq;
        self.next_boundary_seq = boundary
            .checked_add(1)
            .ok_or_else(|| ControlError::new("ResourceExhausted", "boundary sequence exhausted"))?;
        let stop = pending
            .iter()
            .any(|(_, c)| matches!(c, ControlCommand::Stop));
        let mut pause_error = None;
        if !failed
            && stop
            && !matches!(
                self.state,
                RunState::Closed | RunState::Completed | RunState::Failed
            )
        {
            self.state = RunState::Stopped;
            self.token = None;
        } else if !failed
            && !stop
            && pending
                .iter()
                .any(|(_, c)| matches!(c, ControlCommand::Pause))
            && matches!(
                self.state,
                RunState::Ready | RunState::Initializing | RunState::Running | RunState::Paused
            )
        {
            if let Err(error) = self.pause() {
                pause_error = Some(error);
            }
        }
        for (order, (id, command)) in pending.into_iter().enumerate() {
            let mut error = None;
            let mut export_ordinal = None;
            let mut receipt = None;
            let status = if failed {
                ControlStatus::EngineFailed
            } else if self.state == RunState::Closed {
                ControlStatus::SessionClosed
            } else {
                match &command {
                    ControlCommand::Stop => ControlStatus::Applied,
                    ControlCommand::Pause => {
                        if stop {
                            ControlStatus::Superseded
                        } else if let Some(failure) = &pause_error {
                            error = Some(failure.to_string());
                            ControlStatus::ResourceExhausted
                        } else if self.state == RunState::Paused {
                            ControlStatus::Applied
                        } else {
                            ControlStatus::InvalidState
                        }
                    }
                    ControlCommand::Resume(token) => {
                        if stop {
                            ControlStatus::Superseded
                        } else if self.state == RunState::Paused
                            && self.token.as_ref() == Some(token)
                        {
                            self.token = None;
                            self.state = RunState::Running;
                            ControlStatus::Applied
                        } else {
                            error = Some("invalid or consumed ResumeToken".into());
                            ControlStatus::InvalidState
                        }
                    }
                    ControlCommand::Export { destination } => {
                        let physical = self.replay_destination(id, destination);
                        match self.export_now(&physical) {
                            Ok(ordinal) => {
                                export_ordinal = Some(ordinal.to_string());
                                ControlStatus::Applied
                            }
                            Err(e) => {
                                error = Some(e.to_string());
                                ControlStatus::InvalidState
                            }
                        }
                    }
                    ControlCommand::Checkpoint {
                        destination,
                        limits,
                    } => {
                        let physical = self.replay_destination(id, destination);
                        match self.checkpoint_now(&physical, destination, limits) {
                            Ok(value) => {
                                receipt = Some(value);
                                ControlStatus::Applied
                            }
                            Err(e) => {
                                error = Some(e.to_string());
                                ControlStatus::InvalidState
                            }
                        }
                    }
                }
            };
            let seq = self.next_control_seq;
            self.next_control_seq = seq.checked_add(1).ok_or_else(|| {
                ControlError::new("ResourceExhausted", "control sequence exhausted")
            })?;
            let ack = ControlAck {
                request_id: id,
                control_seq: seq,
                boundary_seq: boundary,
                boundary_kind: kind,
                commit_ordinal: self.engine.snapshot().common.committed_events,
                virtual_time_ps: self.horizon(),
                state: self.state,
                status,
                export_ordinal,
                checkpoint_receipt: receipt,
                error,
            };
            self.replay.push(ReplayEntry {
                boundary_seq: boundary,
                boundary_kind: kind,
                commit_ordinal: ack.commit_ordinal,
                request_order: order as u32,
                command: command.canonical(),
                status,
                state: self.state,
            });
            if let Some(slot) = self.controller.lock()?.slots.get_mut(&id) {
                slot.ack = Some(ack);
            }
        }
        Ok(())
    }
    pub fn poll_control(&mut self) -> Result<RunState, ControlError> {
        let kind = if self.state == RunState::Ready {
            BoundaryKind::BeforeInitialize
        } else {
            BoundaryKind::PausedPoll
        };
        self.boundary(kind, false)?;
        Ok(self.state)
    }
    pub fn run_until_yield(&mut self) -> Result<RunState, ControlError> {
        if matches!(
            self.state,
            RunState::Stopped | RunState::Completed | RunState::Failed | RunState::Closed
        ) {
            self.poll_control()?;
            return Ok(self.state);
        }
        self.boundary(
            if self.state == RunState::Ready {
                BoundaryKind::BeforeInitialize
            } else {
                BoundaryKind::PausedPoll
            },
            false,
        )?;
        if !matches!(self.state, RunState::Ready | RunState::Running) {
            return Ok(self.state);
        }
        if !self.initialized {
            self.state = RunState::Initializing;
            match self.engine.initialize() {
                Ok(()) => {
                    self.initialized = true;
                    self.engine.refresh_view(
                        self.engine
                            .snapshot()
                            .common
                            .last_event_time_ps
                            .unwrap_or(0),
                    )?;
                    self.state = RunState::Running;
                    self.boundary(BoundaryKind::AfterInitialize, false)?;
                }
                Err(e) => {
                    self.engine.fail_initialization(e);
                    self.state = RunState::Failed;
                    self.boundary(BoundaryKind::AfterInitialize, true)?;
                    self.cleanup();
                    return Ok(self.state);
                }
            }
        }
        while self.state == RunState::Running {
            let committed = self.engine.step();
            if self.engine.snapshot().common.partial
                || self.engine.snapshot().common.spool_error.is_some()
            {
                self.state = RunState::Failed;
                self.boundary(BoundaryKind::AfterCommit, true)?;
                self.cleanup();
                break;
            }
            self.boundary(BoundaryKind::AfterCommit, false)?;
            if self.state != RunState::Running {
                break;
            }
            if !committed {
                self.state = RunState::Completed;
                self.engine.snapshot_mut().common.end_ps = self.horizon();
                self.engine
                    .refresh_view(self.engine.snapshot().common.end_ps)?;
                self.cleanup();
                if self.engine.snapshot().common.partial {
                    self.state = RunState::Failed;
                    self.engine.snapshot_mut().common.end_ps = self.horizon();
                    self.engine.refresh_view(self.horizon())?;
                }
                break;
            }
        }
        Ok(self.state)
    }
    /// Advance initialization or one event transaction, then return ownership.
    /// This also makes deterministic control boundaries available to interactive callers.
    pub fn advance_one(&mut self) -> Result<RunState, ControlError> {
        self.boundary(
            if self.state == RunState::Ready {
                BoundaryKind::BeforeInitialize
            } else {
                BoundaryKind::PausedPoll
            },
            false,
        )?;
        if !matches!(self.state, RunState::Ready | RunState::Running) {
            return Ok(self.state);
        }
        if !self.initialized {
            self.state = RunState::Initializing;
            match self.engine.initialize() {
                Ok(()) => {
                    self.initialized = true;
                    self.engine.refresh_view(0)?;
                    self.state = RunState::Running;
                    self.boundary(BoundaryKind::AfterInitialize, false)?;
                }
                Err(e) => {
                    self.engine.fail_initialization(e);
                    self.state = RunState::Failed;
                    self.boundary(BoundaryKind::AfterInitialize, true)?;
                    self.cleanup();
                }
            }
            return Ok(self.state);
        }
        let committed = self.engine.step();
        if self.engine.snapshot().common.partial
            || self.engine.snapshot().common.spool_error.is_some()
        {
            self.state = RunState::Failed;
            self.boundary(BoundaryKind::AfterCommit, true)?;
            self.cleanup();
            return Ok(self.state);
        }
        self.boundary(BoundaryKind::AfterCommit, false)?;
        if self.state == RunState::Running && !committed {
            self.state = RunState::Completed;
            self.engine.snapshot_mut().common.end_ps = self.horizon();
            self.engine
                .refresh_view(self.engine.snapshot().common.end_ps)?;
            self.cleanup();
            if self.engine.snapshot().common.partial {
                self.state = RunState::Failed;
                self.engine.snapshot_mut().common.end_ps = self.horizon();
                self.engine.refresh_view(self.horizon())?;
            }
        }
        Ok(self.state)
    }
    fn owner(&mut self, command: ControlCommand) -> Result<ControlAck, ControlError> {
        let handle = self.controller.submit(command)?;
        self.boundary(BoundaryKind::OwnerOperation, false)?;
        self.controller
            .ack(&handle)?
            .ok_or_else(|| ControlError::new("Busy", "request remains pending"))
    }
    pub fn resume(&mut self, token: ResumeToken) -> Result<ControlAck, ControlError> {
        self.owner(ControlCommand::Resume(token))
    }
    pub fn export(&mut self, destination: &Path) -> Result<ControlAck, ControlError> {
        let destination = if destination.is_absolute() {
            destination.into()
        } else {
            self.output_root.join(destination)
        };
        self.owner(ControlCommand::Export { destination })
    }
    pub fn export_with_root(
        &mut self,
        root: &Path,
        destination: &Path,
    ) -> Result<ControlAck, ControlError> {
        if !root.is_absolute() {
            return Err(ControlError::new(
                "InvalidDestination",
                "output root must be absolute",
            ));
        }
        self.owner(ControlCommand::Export {
            destination: if destination.is_absolute() {
                destination.into()
            } else {
                root.join(destination)
            },
        })
    }
    fn export_now(&mut self, destination: &Path) -> Result<u64, ControlError> {
        if !matches!(
            self.state,
            RunState::Paused | RunState::Stopped | RunState::Completed | RunState::Failed
        ) {
            return Err(ControlError::new(
                "InvalidState",
                "export requires a quiescent session",
            ));
        }
        if self.export_ordinal >= u64::from(self.options.max_exports) {
            return Err(ControlError::new(
                "ExportLimit",
                "maximum successful exports reached",
            ));
        }
        if std::fs::symlink_metadata(destination).is_ok() {
            return Err(ControlError::new(
                "DestinationExists",
                "export destination exists",
            ));
        }
        let ordinal = self.export_ordinal;
        let previous = self.state;
        let horizon = self.horizon();
        self.engine.refresh_view(horizon)?;
        let common = &mut self.engine.snapshot_mut().common;
        let old_end = common.end_ps;
        let old_termination = common.termination.clone();
        let old_partial = common.partial;
        common.end_ps = horizon;
        if matches!(previous, RunState::Paused | RunState::Stopped) {
            common.termination = if previous == RunState::Paused {
                "paused"
            } else {
                "stopped"
            }
            .into();
            common.partial = true;
        }
        let control = json!({"version":"1","session_id":self.controller.session_id,"export_ordinal":ordinal.to_string(),"state":previous,"commit_ordinal":self.engine.snapshot().common.committed_events.to_string(),"horizon_ps":horizon.to_string(),"pending_events":self.engine.snapshot().common.pending_events.to_string(),"resume_supported":previous==RunState::Paused,"replay_sha256":sha(&serde_json::to_vec(&self.replay).unwrap())});
        if let Some(registered) = self.engine.snapshot_mut().registered.as_mut() {
            registered.control = Some(control.clone());
        }
        self.state = RunState::Exporting;
        let previous_gate = {
            let mut mailbox = self.controller.lock()?;
            let previous = mailbox.checkpoint_gate;
            mailbox.checkpoint_gate = true;
            previous
        };
        let staging = destination
            .parent()
            .unwrap_or(&self.output_root)
            .join(format!(
                ".dir-export-{}-{ordinal}",
                self.controller.session_id
            ));
        let mut staging_owned = false;
        let result = std::fs::create_dir(&staging)
            .map_err(|e| ControlError::new("ExportFailed", e.to_string()))
            .and_then(|()| {
                staging_owned = true;
                if self.engine.is_legacy() {
                    crate::output::export_controlled_legacy(
                        &self.engine.prepared,
                        self.engine.snapshot(),
                        &staging,
                        &control,
                    )
                } else {
                    crate::output::export(&self.engine.prepared, self.engine.snapshot(), &staging)
                }
                .map_err(ControlError::from)?;
                rustix::fs::renameat_with(
                    rustix::fs::CWD,
                    &staging,
                    rustix::fs::CWD,
                    destination,
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(|e| {
                    ControlError::new(
                        if e == rustix::io::Errno::EXIST {
                            "DestinationExists"
                        } else {
                            "ExportFailed"
                        },
                        e.to_string(),
                    )
                })?;
                Ok(())
            });
        self.state = previous;
        self.controller.lock()?.checkpoint_gate = previous_gate;
        if let Some(registered) = self.engine.snapshot_mut().registered.as_mut() {
            registered.control = None;
        }
        self.engine.snapshot_mut().common.end_ps = old_end;
        self.engine.snapshot_mut().common.termination = old_termination;
        self.engine.snapshot_mut().common.partial = old_partial;
        if result.is_err() && staging_owned {
            for name in [
                "results.json",
                "events.csv",
                "summary.csv",
                "diagnostics.jsonl",
                "manifest.json",
            ] {
                let _ = std::fs::remove_file(staging.join(name));
            }
            let _ = std::fs::remove_dir(&staging);
        }
        result?;
        self.export_ordinal += 1;
        Ok(ordinal)
    }
    pub fn checkpoint(
        &mut self,
        destination: &Path,
        limits: CheckpointLimits,
    ) -> Result<ControlAck, ControlError> {
        limits
            .validate()
            .map_err(|e| ControlError::new("InvalidLimits", e.0))?;
        let destination = if destination.is_absolute() {
            destination.into()
        } else {
            self.output_root.join(destination)
        };
        let handle = self.controller.enqueue(
            ControlCommand::Checkpoint {
                destination,
                limits,
            },
            true,
        )?;
        let pending = {
            let mut mailbox = self.controller.lock()?;
            mailbox.pending.drain(..).collect::<Vec<_>>()
        };
        let mut previous = Vec::new();
        let mut checkpoint = None;
        {
            let mailbox = self.controller.lock()?;
            for id in pending {
                let command = mailbox.slots[&id].command.clone();
                if id == handle.request_id {
                    checkpoint = Some((id, command));
                } else {
                    previous.push((id, command));
                }
            }
        }
        let result = self
            .process(previous, BoundaryKind::PausedPoll, false)
            .and_then(|()| {
                self.process(
                    vec![checkpoint.unwrap()],
                    BoundaryKind::OwnerOperation,
                    false,
                )
            });
        self.controller.lock()?.checkpoint_gate = false;
        result?;
        self.controller
            .ack(&handle)?
            .ok_or_else(|| ControlError::new("Busy", "checkpoint pending"))
    }
    /// Checkpoint owner operation retaining the common mailbox request handle.
    pub fn checkpoint_with_handle(
        &mut self,
        destination: &Path,
        limits: CheckpointLimits,
    ) -> Result<(RequestHandle, ControlAck), ControlError> {
        let destination = if destination.is_absolute() {
            destination.to_path_buf()
        } else {
            self.output_root.join(destination)
        };
        let ack = self.checkpoint(&destination, limits.clone())?;
        let handle = self.controller.retry(
            ack.request_id,
            ControlCommand::Checkpoint {
                destination,
                limits,
            },
        )?;
        Ok((handle, ack))
    }
    pub fn close(&mut self) -> CloseReport {
        if self.state == RunState::Closed {
            return CloseReport::default();
        }
        let before = self.engine.snapshot().common.diagnostics.len();
        self.token = None;
        self.cleanup();
        self.state = RunState::Closed;
        let _ = self.boundary(BoundaryKind::OwnerOperation, false);
        if let Ok(mut mailbox) = self.controller.lock() {
            mailbox.closed = true;
        }
        CloseReport {
            diagnostics: self.engine.snapshot().common.diagnostics[before..].to_vec(),
        }
    }
}
impl Drop for RunSession {
    fn drop(&mut self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.close()));
    }
}
pub fn run_with_control(
    mut session: RunSession,
) -> Result<(RunSession, RunState, Option<ResumeToken>), ControlError> {
    let state = session.run_until_yield()?;
    let token = session.pause_token();
    Ok((session, state, token))
}
pub(crate) fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub use checkpoint::migration::{
    CheckpointIdentity, CheckpointMigration, MigrationPayload, MigrationStep, checkpoint_identity,
    current_checkpoint_identity, migrate_checkpoint,
};
pub use checkpoint::restore_checkpoint;

/// Re-run control snapshots at their recorded logical boundaries. Export and
/// checkpoint operations run in a fresh private output root, retaining the
/// canonical logical destinations in the replay log. Original artifacts are
/// preserved. Failed I/O dependent on the original environment may mismatch.
/// No wall-clock timing or opaque token bytes are replayed.
pub fn replay_with_control(
    prepared: impl Into<Arc<PreparedSimulation>>,
    options: RunOptions,
    log: &[ReplayEntry],
) -> Result<RunSession, ControlError> {
    let (mut session, _) = RunSession::new(prepared, options)?;
    session.prepare_replay_artifacts(log)?;
    if let Err(mut error) = replay_entries(&mut session, log) {
        if let Some(root) = session.replay_artifact_root() {
            error.message = format!(
                "{}; replay artifacts retained at {}",
                error.message,
                root.display()
            );
        }
        return Err(error);
    }
    Ok(session)
}
fn replay_entries(session: &mut RunSession, log: &[ReplayEntry]) -> Result<(), ControlError> {
    let mismatch = |message: &str| ControlError::new("ReplayMismatch", message);
    let mut cursor = 0;
    let mut after_commit = None;
    let mut after_initialize = false;
    while cursor < log.len() {
        let head = &log[cursor];
        let end = log[cursor..]
            .iter()
            .position(|r| r.boundary_seq != head.boundary_seq)
            .map_or(log.len(), |n| cursor + n);
        let group = &log[cursor..end];
        if head.boundary_seq != session.next_boundary_seq
            || group.iter().enumerate().any(|(i, r)| {
                r.boundary_kind != head.boundary_kind
                    || r.commit_ordinal != head.commit_ordinal
                    || r.request_order != i as u32
            })
        {
            return Err(mismatch("invalid replay snapshot grouping"));
        }
        if head.commit_ordinal < session.engine.snapshot().common.committed_events {
            return Err(mismatch("replay commit ordinal regressed"));
        }
        let recorded_terminal = if matches!(
            head.boundary_kind,
            BoundaryKind::PausedPoll | BoundaryKind::OwnerOperation
        ) && group.iter().all(|entry| entry.state == head.state)
            && matches!(head.state, RunState::Completed | RunState::Failed)
        {
            Some(head.state)
        } else {
            None
        };
        let needs_initialization = head.boundary_kind != BoundaryKind::BeforeInitialize
            && (recorded_terminal.is_some()
                || (head.boundary_kind == BoundaryKind::PausedPoll
                    && session.state == RunState::Ready)
                || !matches!(
                    head.boundary_kind,
                    BoundaryKind::PausedPoll | BoundaryKind::OwnerOperation
                )
                || head.commit_ordinal > 0);
        if needs_initialization && !session.initialized {
            if !matches!(session.state, RunState::Ready | RunState::Running) {
                return Err(mismatch("initialization requested from a quiescent state"));
            }
            session.state = RunState::Initializing;
            if let Err(error) = session.engine.initialize() {
                session.engine.fail_initialization(error);
                session.state = RunState::Failed;
            } else {
                session.initialized = true;
                session.state = RunState::Running;
            }
        }
        while session.engine.snapshot().common.committed_events < head.commit_ordinal {
            if session.state != RunState::Running {
                return Err(mismatch(
                    "requested commit ordinal is unreachable from current state",
                ));
            }
            if !session.engine.step() || session.engine.snapshot().common.partial {
                return Err(mismatch("requested commit ordinal was not reached"));
            }
        }
        if let Some(expected_terminal) = recorded_terminal {
            if session.state == RunState::Running {
                let ordinal = session.engine.snapshot().common.committed_events;
                let actual = session.advance_one()?;
                if actual != expected_terminal
                    || session.engine.snapshot().common.committed_events != ordinal
                {
                    return Err(mismatch(
                        "recorded terminal state is not the next execution boundary",
                    ));
                }
            } else if session.state != expected_terminal {
                return Err(mismatch("recorded terminal state differs from execution"));
            }
        }
        let failed = group
            .iter()
            .all(|r| r.status == ControlStatus::EngineFailed);
        if failed && session.state != RunState::Failed {
            if session.state != RunState::Running
                || session.engine.step()
                || !session.engine.snapshot().common.partial
            {
                return Err(mismatch("expected callback failure did not occur"));
            }
            session.state = RunState::Failed;
        }
        let valid = match head.boundary_kind {
            BoundaryKind::BeforeInitialize => {
                !session.initialized && session.state == RunState::Ready && head.commit_ordinal == 0
            }
            BoundaryKind::AfterInitialize => {
                head.commit_ordinal == 0
                    && (session.initialized || session.state == RunState::Failed)
                    && !after_initialize
            }
            BoundaryKind::AfterCommit => {
                session.initialized
                    && head.commit_ordinal > 0
                    && after_commit != Some(head.commit_ordinal)
            }
            BoundaryKind::PausedPoll => matches!(
                session.state,
                RunState::Paused
                    | RunState::Stopped
                    | RunState::Completed
                    | RunState::Failed
                    | RunState::Running
            ),
            BoundaryKind::OwnerOperation => true,
        };
        if !valid {
            return Err(mismatch(
                "recorded boundary kind differs from execution state",
            ));
        }
        if head.boundary_kind == BoundaryKind::AfterInitialize {
            after_initialize = true;
        }
        if head.boundary_kind == BoundaryKind::AfterCommit {
            after_commit = Some(head.commit_ordinal);
        }
        let mut requests = Vec::new();
        for entry in group {
            let command = match entry.command["command"].as_str() {
                Some("Pause") => ControlCommand::Pause,
                Some("Stop") => ControlCommand::Stop,
                Some("Resume") => {
                    let token = session
                        .pause_token()
                        .ok_or_else(|| mismatch("recorded Resume has no current pause token"))?;
                    if entry.command["pause_generation"] != json!(token.generation.to_string()) {
                        return Err(mismatch("recorded pause generation differs"));
                    }
                    ControlCommand::Resume(token)
                }
                Some("Export") => ControlCommand::Export {
                    destination: serde_json::from_value(entry.command["destination"].clone())
                        .map_err(|_| mismatch("invalid export destination"))?,
                },
                Some("Checkpoint") => ControlCommand::Checkpoint {
                    destination: serde_json::from_value(entry.command["destination"].clone())
                        .map_err(|_| mismatch("invalid checkpoint destination"))?,
                    limits: serde_json::from_value(entry.command["limits"].clone())
                        .map_err(|_| mismatch("invalid checkpoint limits"))?,
                },
                _ => return Err(mismatch("unknown replay command")),
            };
            let checkpoint = matches!(command, ControlCommand::Checkpoint { .. });
            let handle = session.controller.enqueue(command.clone(), checkpoint)?;
            requests.push((handle.request_id, command));
        }
        session.controller.lock()?.pending.clear();
        let before = session.replay.len();
        session.process(requests, head.boundary_kind, failed)?;
        session.controller.lock()?.checkpoint_gate = false;
        for (actual, expected) in session.replay[before..].iter().zip(group) {
            if actual.boundary_seq != expected.boundary_seq
                || actual.boundary_kind != expected.boundary_kind
                || actual.commit_ordinal != expected.commit_ordinal
                || actual.request_order != expected.request_order
                || actual.command != expected.command
                || actual.status != expected.status
                || actual.state != expected.state
            {
                return Err(mismatch(
                    if matches!(
                        expected.command["command"].as_str(),
                        Some("Export" | "Checkpoint")
                    ) {
                        "replayed acknowledgement differs; isolated output I/O may not reproduce the original failure's environmental preconditions"
                    } else {
                        "replayed acknowledgement differs"
                    },
                ));
            }
        }
        if failed {
            session.cleanup();
        }
        session
            .controller
            .lock()?
            .slots
            .retain(|_, slot| slot.ack.is_none());
        cursor = end;
    }
    session.run_until_yield()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller(next_id: u64) -> Controller {
        Controller {
            inner: Arc::new(Mutex::new(Mailbox {
                pending: VecDeque::new(),
                slots: BTreeMap::new(),
                next_id,
                checkpoint_gate: false,
                closed: false,
            })),
            session_id: "test".into(),
        }
    }
    #[test]
    fn mailbox_id_sentinel_and_busy_do_not_consume_ids() {
        let c = controller(u64::MAX);
        assert_eq!(
            c.submit(ControlCommand::Pause).unwrap_err().code,
            "ResourceExhausted"
        );
        assert_eq!(c.lock().unwrap().next_id, u64::MAX);
        let c = controller(0);
        for id in 0..64 {
            assert_eq!(c.submit(ControlCommand::Pause).unwrap().request_id, id);
        }
        assert_eq!(c.submit(ControlCommand::Pause).unwrap_err().code, "Busy");
        assert_eq!(c.lock().unwrap().next_id, 64);
    }
    #[test]
    fn pause_generation_exhaustion_is_an_ack_without_state_change() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet/dynamic/unicast.ini");
        let (mut session, c) =
            RunSession::new(crate::prepare(&path).unwrap(), RunOptions::default()).unwrap();
        session.next_pause_generation = u64::MAX;
        let handle = c.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Ready);
        assert_eq!(
            c.ack(&handle).unwrap().unwrap().status,
            ControlStatus::ResourceExhausted
        );
        assert!(!session.initialized);
        assert!(session.token.is_none());
    }
    #[test]
    fn close_acknowledges_pending_requests_as_session_closed() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet/dynamic/unicast.ini");
        let (mut session, c) =
            RunSession::new(crate::prepare(&path).unwrap(), RunOptions::default()).unwrap();
        let handle = c.submit(ControlCommand::Pause).unwrap();
        session.close();
        assert_eq!(
            c.ack(&handle).unwrap().unwrap().status,
            ControlStatus::SessionClosed
        );
        assert_eq!(
            c.submit(ControlCommand::Stop).unwrap_err().code,
            "SessionClosed"
        );
    }
}

#[cfg(test)]
mod spool_checkpoint_tests {
    use super::*;
    struct Probe;
    impl Model for Probe {
        fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
            let target = c.subject().to_owned();
            for value in [1, 2, 7] {
                c.observe("test.point", &target, value)?;
            }
            Ok(())
        }
        fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
            Err("unexpected event".into())
        }
        fn checkpoint(&self, _: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
            Ok(Vec::new())
        }
    }
    fn registry() -> Registry {
        let mut r = Registry::empty();
        r.register_module(
            "test.Probe",
            |_| Ok(Box::new(Probe)),
            ModuleDescriptor {
                implementation_key: "test.Probe".into(),
                implementation_version: "1".into(),
                parameters: vec![],
                ports: vec![],
                events: vec![],
                resources: vec![],
            },
        )
        .unwrap();
        r.register_metric(MetricDescriptor {
            name: "test.point".into(),
            unit: "count".into(),
        })
        .unwrap();
        r.register_profile(ProfileDescriptor {
            name: "test.spool.v1".into(),
            implementation_version: "1".into(),
            modules: vec!["test.Probe".into()],
            channels: vec![],
            events: vec![],
            metrics: vec!["test.point".into()],
            model_records: vec![],
            output_schema_version: 2,
            validate: |_, _| Ok(()),
        })
        .unwrap();
        r.register_execution_capabilities(
            "test.spool.v1",
            ExecutionCapabilities {
                cooperative_control: true,
                metrics_v2: false,
                checkpoint: true,
            },
        )
        .unwrap();
        r.register_model_checkpoint_codec(
            CheckpointCodecDescriptor {
                implementation_key: "test.Probe".into(),
                implementation_version: "1".into(),
                codec_name: "test.empty".into(),
                codec_version: 1,
                external_state: "none".into(),
                max_bytes: 1,
            },
            |_, _, bytes, _| {
                if bytes.is_empty() {
                    Ok(Box::new(Probe))
                } else {
                    Err("invalid stateless payload".into())
                }
            },
        )
        .unwrap();
        r
    }
    #[test]
    fn committed_spool_prefix_is_captured_without_a_process_file_path() {
        let root = std::env::temp_dir().join(format!(
            "dir-control-spool-{}",
            crate::output::new_run_identity().unwrap().run_id
        ));
        std::fs::create_dir_all(root.join("ned/demo")).unwrap();
        std::fs::write(root.join("ned/demo/model.ned"),"package demo; simple Probe { parameters: @class(\"test.Probe\"); } network Demo { submodules: a: demo.Probe; }").unwrap();
        std::fs::write(root.join("scenario.ini"),"[General]\nnetwork=demo.Demo\nned-path=\"ned\"\nmodel-profile=\"test.spool.v1\"\nsim-time-limit=10ps\n").unwrap();
        let prepared =
            crate::prepare_with_registry(&root.join("scenario.ini"), registry()).unwrap();
        let (mut session, c) = RunSession::new(
            prepared,
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        session.advance_one().unwrap();
        super::super::spool::with_spooling(&root, || {
            session.engine.snapshot_mut().common.spool_checkpoint(true)
        })
        .unwrap();
        assert!(session.engine.snapshot().common.point_spool.is_some());
        assert!(session.engine.snapshot().common.points.is_empty());
        c.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        let bundle = root.join("checkpoint");
        let ack = session
            .checkpoint(&bundle, CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let (restored, _) =
            restore_checkpoint(&bundle, registry(), CheckpointLimits::default()).unwrap();
        let points = restored
            .snapshot()
            .common
            .iter_points()
            .unwrap()
            .map(|p| p.unwrap().value)
            .collect::<Vec<_>>();
        assert_eq!(points, [1, 2, 7]);
        assert!(restored.snapshot().common.point_spool.is_none());
        let state = std::fs::read_to_string(bundle.join("state.json")).unwrap();
        assert!(!state.contains(".dir-observations"));
        drop(restored);
        drop(session);
        std::fs::remove_dir_all(root).unwrap();
    }
    crate::runtime::ethernet::dynamic::tests::vector_session_private_cases!();
}
