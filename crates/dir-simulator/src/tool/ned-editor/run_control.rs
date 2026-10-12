//! Durable asynchronous runs, independently locked from editor state.
mod cleanup;
pub(crate) mod storage;
#[cfg(test)]
mod tests;
mod viewer_server;
pub mod worker;
use super::input::ProjectSnapshot;
use super::run_snapshot::{self, SnapshotEvidence};
use super::{EditorError, Result, hash, random_id};
use crate::output::{
    ResultBundleExpectation, ResultInputEvidence, ResultReportEvidence, ValidatedResultBundle,
    validate_result_bundle,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
pub(crate) use storage::hash_file;
use storage::{Directory, Identity, invalid};

const PIN_LIMIT: u64 = 512 * 1024 * 1024;
const RUN_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const RUNS_LIMIT: u64 = 10 * 1024 * 1024 * 1024;
const LOG_LIMIT: usize = 1024 * 1024;
const CONTROL_LIMIT: usize = 256 * 1024 * 1024 + 3 * worker::FRAME_LIMIT;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunStamp {
    pub session_id: String,
    pub command_id: String,
    pub project_instance_id: String,
    pub revision: String,
    pub input_revision: String,
    pub snapshot_id: String,
    pub input_digest: String,
    pub origin: Value,
    pub dirty_at_start: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RunRecord {
    schema_version: u32,
    run_id: String,
    owner: Identity,
    stamp: RunStamp,
    snapshot: SnapshotEvidence,
    wall_timeout_ms: String,
    created_ms: String,
    started_at_utc: Option<String>,
    ended_at_utc: Option<String>,
    phase: String,
    stop_reason: Option<String>,
    report: Option<Value>,
    recovered_result: Option<Value>,
    diagnostics: Vec<Value>,
    artifact_state: String,
    viewer_state: String,
    worker_response: String,
    response: Option<worker::Message>,
    manifest_sha256: Option<String>,
    worker_exit: Option<Value>,
    #[serde(default)]
    worker_pid: Option<String>,
    binary_sha256: String,
    binary_version: String,
    resource_observation: Option<Value>,
}
impl RunRecord {
    fn active(&self) -> bool {
        matches!(
            self.phase.as_str(),
            "queued" | "preparing" | "running" | "verifying" | "stopping"
        )
    }
    fn value(&self) -> Value {
        json!({"schema_version":1,"run_id":self.run_id,"source_stamp":self.stamp,"stamp":self.stamp,"phase":self.phase,"stop_reason":self.stop_reason,"report":self.report,"recovered_result":self.recovered_result,"diagnostics":self.diagnostics,"original_termination":match &self.response {Some(worker::Message::Failed{original_termination,..})=>original_termination.clone(),_=>None},"artifact_state":self.artifact_state,"viewer_state":self.viewer_state,"worker_response":self.worker_response,"created_ms":self.created_ms,"started_at_utc":self.started_at_utc,"ended_at_utc":self.ended_at_utc,"wall_timeout_ms":self.wall_timeout_ms,"worker_exit":self.worker_exit,"resource_observation":self.resource_observation})
    }
}
struct State {
    records: BTreeMap<String, RunRecord>,
    pending: BTreeMap<(String, String), RunRecord>,
    freeze: bool,
    freeze_bytes: u64,
    downloads: BTreeMap<String, usize>,
    viewers: BTreeMap<String, viewer_server::ViewerLease>,
    viewer_service: Option<Arc<viewer_server::ViewerService>>,
    viewer_generations: BTreeMap<String, u64>,
    viewer_renders: usize,
    finished: bool,
    worker_error: Option<EditorError>,
}
struct Shared {
    root: PathBuf,
    input_regions: Vec<PathBuf>,
    owner: bool,
    _lock: File,
    state: Mutex<State>,
    wake: Condvar,
    shutdown: AtomicBool,
    binary: PathBuf,
    binary_hash: String,
    launch_worker: bool,
}
struct Lifetime {
    shared: Weak<Shared>,
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        if let Some(s) = self.shared.upgrade() {
            s.shutdown.store(true, Ordering::Release);
            s.wake.notify_all();
            if s.owner {
                let state = s.state.lock().unwrap();
                let _ = s
                    .wake
                    .wait_timeout_while(state, Duration::from_secs(5), |st| !st.finished);
            }
        }
    }
}
#[derive(Clone)]
pub(crate) struct RunManager {
    shared: Arc<Shared>,
    _lifetime: Arc<Lifetime>,
}
impl std::io::Read for RunDownload {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let stat = rustix::fs::fstat(&self.file)
            .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;
        if stat.st_dev != self.identity.dev
            || stat.st_ino != self.identity.ino
            || stat.st_nlink != 1
        {
            return Err(std::io::Error::other(
                "Authenticated artifact identity changed",
            ));
        }
        std::io::Read::read(&mut self.file, buf)
    }
}
pub(crate) struct RunDownload {
    pub file: File,
    pub filename: String,
    pub bytes: u64,
    pub content_type: &'static str,
    pub sha256: String,
    identity: crate::output::ArtifactIdentity,
    _reference: DownloadReference,
}
struct DownloadReference {
    shared: Arc<Shared>,
    run_id: String,
    _lease: File,
}
impl Drop for DownloadReference {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        if let Some(n) = state.downloads.get_mut(&self.run_id) {
            *n = n.saturating_sub(1);
        }
    }
}
fn busy(message: &str) -> EditorError {
    EditorError::new("E-EDITOR-RUN-BUSY", message, 503)
}
fn missing() -> EditorError {
    EditorError::new("E-EDITOR-RUN-NOT-FOUND", "Unknown run", 404)
}
fn gone() -> EditorError {
    EditorError::new("E-EDITOR-RUN-GONE", "Run has been deleted", 410)
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn utc() -> String {
    crate::output::new_run_identity()
        .map(|i| i.started_at_utc)
        .unwrap_or_else(|_| format!("unix-ms:{}", now_ms()))
}
fn valid_id(id: &str) -> Result<()> {
    if id.len() != 68
        || !id.starts_with("run-")
        || !id[4..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(missing());
    }
    Ok(())
}
fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
pub(crate) fn validate_run_payload(payload: &Value) -> Result<()> {
    wall_timeout_ms(payload).map(|_| ())
}
pub(crate) fn wall_timeout_ms(payload: &Value) -> Result<u64> {
    let map = payload
        .as_object()
        .ok_or_else(|| EditorError::new("E-EDITOR-RUN-PAYLOAD", "Expected payload object", 400))?;
    if map.keys().any(|k| k != "wall_timeout_ms") {
        return Err(EditorError::new(
            "E-EDITOR-RUN-PAYLOAD",
            "Unknown run field",
            400,
        ));
    }
    let Some(v) = map.get("wall_timeout_ms") else {
        return Ok(600000);
    };
    let error = || {
        EditorError::new(
            "E-EDITOR-RUN-PAYLOAD",
            "wall_timeout_ms must be a canonical decimal string in 1000..3600000",
            400,
        )
    };
    let s = v.as_str().ok_or_else(error)?;
    if s.is_empty() || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(error());
    }
    let n = s.parse::<u64>().map_err(|_| error())?;
    if !(1000..=3600000).contains(&n) {
        return Err(error());
    }
    Ok(n)
}
fn decimal(s: &str) -> bool {
    s == "0"
        || (!s.starts_with('0')
            && !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
            && s.parse::<u64>().is_ok())
}
fn encode(v: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|e| invalid(e.to_string()))
}
fn record_dir(shared: &Shared, id: &str) -> Result<Directory> {
    valid_id(id)?;
    Directory::open(&shared.root.join(id))
}
fn verify_owner(shared: &Shared, r: &RunRecord) -> Result<Directory> {
    let dir = record_dir(shared, &r.run_id)?;
    if dir.identity() != r.owner {
        return Err(invalid("Run directory ownership identity mismatch"));
    }
    let owner: Value = serde_json::from_slice(&dir.read("owner.json", 4096)?)
        .map_err(|e| invalid(e.to_string()))?;
    if owner != json!({"schema_version":1,"run_id":r.run_id,"identity":r.owner}) {
        return Err(invalid("Run ownership marker mismatch"));
    }
    Ok(dir)
}
fn persist(shared: &Shared, r: &RunRecord) -> Result<()> {
    let dir = verify_owner(shared, r)?;
    persist_at(&dir, r)
}
fn persist_at(dir: &Directory, r: &RunRecord) -> Result<()> {
    let control = dir.child("control")?;
    let bytes = encode(r)?;
    if bytes.len() > CONTROL_LIMIT {
        return Err(invalid("Run record exceeds bound"));
    }
    control.replace("record.json", &bytes)?;
    let provenance = json!({"schema_version":1,"run_id":r.run_id,"source_stamp":r.stamp,"snapshot_codec_version":1,"bundle_digest":r.snapshot.bundle_digest,"inputs":r.snapshot.files,"binary_version":r.binary_version,"binary_sha256":r.binary_sha256,"limits":{"snapshot_bytes":"268435456","snapshot_files":"10000","active_pinned_bytes":"536870912","address_space_bytes":"4294967296","file_size_bytes":"1073741824","run_bytes_soft":"2147483648","runs_bytes_soft":"10737418240","wall_timeout_ms":r.wall_timeout_ms},"started_at_utc":r.started_at_utc,"ended_at_utc":r.ended_at_utc,"result_manifest_sha256":r.manifest_sha256,"worker_exit":r.worker_exit,"worker_response":r.worker_response,"stop_reason":r.stop_reason,"resource_observation":r.resource_observation});
    control.replace("provenance.json", &encode(&provenance)?)
}
fn load_records(root: &Path) -> Result<BTreeMap<String, RunRecord>> {
    let root_dir = Directory::open(root)?;
    let mut records = BTreeMap::new();
    for name in root_dir.names()? {
        let Some(id) = name.to_str() else {
            continue;
        };
        if !id.starts_with("run-") {
            continue;
        }
        valid_id(id)?;
        if root_dir
            .names()?
            .iter()
            .any(|n| n == format!(".deleted-{id}").as_str())
        {
            continue;
        }
        let dir = root_dir.child(id)?;
        let control = dir.child("control")?;
        let record: RunRecord =
            serde_json::from_slice(&control.read("record.json", CONTROL_LIMIT)?)
                .map_err(|e| invalid(e.to_string()))?;
        if record.run_id != id || record.schema_version != 1 || record.owner != dir.identity() {
            return Err(invalid("Durable run identity mismatch"));
        }
        let owner: Value = serde_json::from_slice(&dir.read("owner.json", 4096)?)
            .map_err(|e| invalid(e.to_string()))?;
        if owner != json!({"schema_version":1,"run_id":id,"identity":record.owner}) {
            return Err(invalid("Durable run ownership marker mismatch"));
        }
        records.insert(id.into(), record);
    }
    Ok(records)
}
impl RunManager {
    pub(crate) fn open(state_root: &Path, input_regions: &[PathBuf]) -> Result<Self> {
        Self::open_impl(state_root, input_regions, None)
    }
    fn open_impl(
        state_root: &Path,
        input_regions: &[PathBuf],
        worker_binary: Option<&Path>,
    ) -> Result<Self> {
        let root = state_root.join("runs");
        if input_regions.iter().any(|p| overlap(&root, p)) {
            return Err(invalid("Run storage overlaps input region"));
        }
        let dir = Directory::ensure(&root)?;
        if !dir.names()?.iter().any(|n| n == ".coordinator.lock") {
            match dir.create(".coordinator.lock", b"") {
                Ok(()) => {}
                Err(e) => {
                    if !dir.names()?.iter().any(|n| n == ".coordinator.lock") {
                        return Err(e);
                    }
                }
            }
        }
        let lock = dir.open_file(".coordinator.lock")?;
        let owner =
            match rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => true,
                Err(e) if e == rustix::io::Errno::WOULDBLOCK => false,
                Err(e) => return Err(EditorError::io(&root, e)),
            };
        let binary = worker_binary
            .map(Path::to_path_buf)
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)
            .map_err(|e| EditorError::io(&root, e))?;
        let binary_hash =
            hash_file(&File::open(&binary).map_err(|e| EditorError::io(&binary, e))?)?;
        let shared = Arc::new(Shared {
            root,
            input_regions: input_regions.to_vec(),
            owner,
            _lock: lock,
            state: Mutex::new(State {
                records: BTreeMap::new(),
                pending: BTreeMap::new(),
                freeze: false,
                freeze_bytes: 0,
                downloads: BTreeMap::new(),
                viewers: BTreeMap::new(),
                viewer_service: None,
                viewer_generations: BTreeMap::new(),
                viewer_renders: 0,
                finished: !owner,
                worker_error: None,
            }),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
            binary,
            binary_hash,
            launch_worker: cfg!(not(test)) || worker_binary.is_some(),
        });
        if owner {
            recover_cleanup(&shared)?;
        }
        let mut records = load_records(&shared.root)?;
        if owner {
            for r in records.values_mut() {
                recover_record(&shared, r)?;
            }
        }
        shared.state.lock().unwrap().records = records;
        if owner {
            if shared.launch_worker {
                if let Err(error) = probe_worker(&shared) {
                    shared.state.lock().unwrap().worker_error = Some(error);
                }
            }
            let s = shared.clone();
            std::thread::Builder::new()
                .name("ned-run-coordinator".into())
                .spawn(move || coordinate(s))
                .map_err(|e| EditorError::new("E-EDITOR-RUN-START", e.to_string(), 500))?;
        }
        Ok(Self {
            _lifetime: Arc::new(Lifetime {
                shared: Arc::downgrade(&shared),
            }),
            shared,
        })
    }
    fn mutation(&self) -> Result<()> {
        if !self.shared.owner {
            return Err(busy("Run coordinator is held by another service"));
        }
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Err(busy("Run service is shutting down"));
        }
        Ok(())
    }
    fn capacity_locked(&self, project: &ProjectSnapshot, state: &State) -> Result<u64> {
        self.mutation()?;
        if let Some(error) = &state.worker_error {
            return Err(busy(&format!("Safe worker unavailable: {error}")));
        }
        let bytes = run_snapshot::size(project)?;
        if !state.pending.is_empty() {
            return Err(busy("A run publication requires durability recovery"));
        }
        if state.freeze {
            return Err(busy("A run is being frozen"));
        }
        let queued = state
            .records
            .values()
            .filter(|r| r.phase == "queued")
            .count();
        if queued >= 2 {
            return Err(busy("Run queue is full"));
        }
        let pinned = state.records.values().filter(|r| r.active()).try_fold(
            state.freeze_bytes,
            |n, r| {
                n.checked_add(r.snapshot.bytes)
                    .ok_or_else(|| invalid("Pinned input size overflow"))
            },
        )?;
        if pinned.saturating_add(bytes) > PIN_LIMIT {
            return Err(busy("Pinned input capacity is full"));
        }
        if project
            .input_regions()
            .iter()
            .chain(&self.shared.input_regions)
            .any(|p| overlap(p, &self.shared.root))
        {
            return Err(invalid("Run storage overlaps input region"));
        }
        let total = Directory::open(&self.shared.root)?.bytes()?;
        if total.saturating_add(bytes) > RUNS_LIMIT {
            return Err(busy("Run storage capacity is full"));
        }
        for r in state.records.values() {
            if verify_owner(&self.shared, r)?.bytes()? > RUN_LIMIT {
                return Err(busy("A run exceeds its storage limit"));
            }
        }
        Ok(bytes)
    }
    pub(crate) fn check_capacity(&self, project: &ProjectSnapshot) -> Result<()> {
        let state = self.shared.state.lock().unwrap();
        self.capacity_locked(project, &state).map(|_| ())
    }
    pub(crate) fn create_run(
        &self,
        project: &ProjectSnapshot,
        stamp: RunStamp,
        timeout: u64,
    ) -> Result<String> {
        self.mutation()?;
        if stamp.session_id.is_empty()
            || stamp.command_id.is_empty()
            || stamp.project_instance_id.is_empty()
            || !(1000..=3600000).contains(&timeout)
            || stamp.snapshot_id != project.id
            || stamp.input_digest != project.input_digest()
            || !decimal(&stamp.revision)
            || !decimal(&stamp.input_revision)
        {
            return Err(invalid("Run stamp or timeout mismatch"));
        }
        let mut state = self.shared.state.lock().unwrap();
        let command_key = (stamp.session_id.clone(), stamp.command_id.clone());
        if let Some(pending) = state.pending.get(&command_key).cloned() {
            if pending.stamp != stamp || pending.wall_timeout_ms != timeout.to_string() {
                return Err(EditorError::new(
                    "E-EDITOR-RUN-COMMAND-CONFLICT",
                    "Pending command was reused",
                    409,
                ));
            }
            verify_owner(&self.shared, &pending)?;
            persist(&self.shared, &pending)?;
            Directory::open(&self.shared.root)?.sync()?;
            state.pending.remove(&command_key);
            state
                .records
                .insert(pending.run_id.clone(), pending.clone());
            drop(state);
            self.shared.wake.notify_all();
            return Ok(pending.run_id);
        }
        if let Some(old) = state.records.values().find(|r| {
            r.stamp.session_id == stamp.session_id && r.stamp.command_id == stamp.command_id
        }) {
            if old.stamp == stamp && old.wall_timeout_ms == timeout.to_string() {
                return Ok(old.run_id.clone());
            }
            return Err(EditorError::new(
                "E-EDITOR-RUN-COMMAND-CONFLICT",
                "Run command identity was reused",
                409,
            ));
        }
        let timeout_text = timeout.to_string();
        let root = Directory::open(&self.shared.root)?;
        for name in root.names()? {
            let Some(name) = name.to_str().filter(|s| s.starts_with(".deleted-")) else {
                continue;
            };
            let deleted: Value = serde_json::from_slice(&root.read(name, CONTROL_LIMIT)?)
                .map_err(|e| invalid(e.to_string()))?;
            if deleted["stamp"]["session_id"] == stamp.session_id
                && deleted["stamp"]["command_id"] == stamp.command_id
            {
                if deleted["stamp"]
                    == serde_json::to_value(&stamp).map_err(|e| invalid(e.to_string()))?
                    && deleted["wall_timeout_ms"].as_str() == Some(timeout_text.as_str())
                {
                    return deleted["run_id"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid("Deleted run mapping invalid"));
                }
                return Err(EditorError::new(
                    "E-EDITOR-RUN-COMMAND-CONFLICT",
                    "Deleted run command identity was reused",
                    409,
                ));
            }
        }
        let bytes = self.capacity_locked(project, &state)?;
        state.freeze = true;
        state.freeze_bytes = bytes;
        drop(state);
        let mut created: Option<(String, Identity)> = None;
        let result = (|| {
            let id = random_id("run-")?;
            let staging = format!(".freeze-{id}");
            let dir = Directory::open(&self.shared.root)?.mkdir(&staging)?;
            created = Some((staging.clone(), dir.identity()));
            dir.create(
                "owner.json",
                &encode(&json!({"schema_version":1,"run_id":id,"identity":dir.identity()}))?,
            )?;
            dir.mkdir("input")?;
            dir.mkdir("control")?.create("references.lock", b"")?;
            dir.mkdir("viewer")?;
            dir.mkdir("result")?;
            let snapshot =
                run_snapshot::freeze(project, &self.shared.root.join(&staging).join("input"))?;
            dir.child("control")?.create(
                "input-inventory.json",
                &encode(&dir.child("input")?.inventory()?)?,
            )?;
            let r = RunRecord {
                schema_version: 1,
                run_id: id.clone(),
                owner: dir.identity(),
                stamp,
                snapshot,
                wall_timeout_ms: timeout.to_string(),
                created_ms: now_ms().to_string(),
                started_at_utc: None,
                ended_at_utc: None,
                phase: "queued".into(),
                stop_reason: None,
                report: None,
                recovered_result: None,
                diagnostics: vec![],
                artifact_state: "unpublished".into(),
                viewer_state: "absent".into(),
                worker_response: "pending".into(),
                response: None,
                manifest_sha256: None,
                worker_exit: None,
                worker_pid: None,
                binary_sha256: self.shared.binary_hash.clone(),
                binary_version: env!("CARGO_PKG_VERSION").into(),
                resource_observation: None,
            };
            persist_at(&dir, &r)?;
            dir.sync()?;
            // Retain the assigned ID before the rename: a post-rename fsync error
            // must never make a later retry allocate another run.
            self.shared
                .state
                .lock()
                .unwrap()
                .pending
                .insert(command_key.clone(), r.clone());
            if let Err(error) =
                Directory::open(&self.shared.root)?.rename_child(&staging, &id, &r.owner)
            {
                if verify_owner(&self.shared, &r).is_err() {
                    self.shared
                        .state
                        .lock()
                        .unwrap()
                        .pending
                        .remove(&command_key);
                }
                return Err(error);
            }
            self.shared
                .state
                .lock()
                .unwrap()
                .pending
                .remove(&command_key);
            self.shared
                .state
                .lock()
                .unwrap()
                .records
                .insert(id.clone(), r);
            Ok(id)
        })();
        let mut state = self.shared.state.lock().unwrap();
        state.freeze = false;
        state.freeze_bytes = 0;
        drop(state);
        if result.is_err() {
            if let Some((id, owner)) = created {
                let _ = Directory::open(&self.shared.root).and_then(|root| {
                    let stage = root.child(&id)?;
                    let inventory = cleanup::approved_stage(&stage, "failed-freeze")?;
                    root.remove_child_checked(&id, &owner, &inventory)
                });
            }
        }
        self.shared.wake.notify_all();
        result
    }
    fn record(&self, id: &str) -> Result<RunRecord> {
        valid_id(id)?;
        if Directory::open(&self.shared.root)?
            .names()?
            .iter()
            .any(|n| n == format!(".deleted-{id}").as_str())
        {
            return Err(gone());
        }
        if !self.shared.owner {
            let records = load_records(&self.shared.root)?;
            return records.get(id).cloned().ok_or_else(missing);
        }
        self.shared
            .state
            .lock()
            .unwrap()
            .records
            .get(id)
            .cloned()
            .ok_or_else(missing)
    }
    pub(crate) fn get(&self, id: &str) -> Result<Value> {
        Ok(self.record(id)?.value())
    }
    pub(crate) fn list(&self, cursor: Option<&str>) -> Result<Value> {
        if let Some(c) = cursor {
            valid_id(c)?;
        }
        let records = if self.shared.owner {
            self.shared.state.lock().unwrap().records.clone()
        } else {
            load_records(&self.shared.root)?
        };
        let mut rows = records
            .values()
            .filter(|r| cursor.is_none_or(|c| r.run_id.as_str() > c));
        let page: Vec<_> = rows.by_ref().take(50).map(RunRecord::value).collect();
        let next = if rows.next().is_some() {
            page.last()
                .and_then(|r| r["run_id"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        Ok(json!({"schema_version":1,"runs":page,"next_cursor":next,"readonly":!self.shared.owner}))
    }
    pub(crate) fn cancel(&self, id: &str) -> Result<Value> {
        self.mutation()?;
        self.record(id)?;
        let mut state = self.shared.state.lock().unwrap();
        let r = state.records.get_mut(id).ok_or_else(missing)?;
        if r.phase == "queued" {
            let mut candidate = r.clone();
            candidate.stop_reason = Some("cancel".into());
            candidate.phase = "cancelled".into();
            candidate.ended_at_utc = Some(utc());
            persist(&self.shared, &candidate)?;
            *r = candidate;
        } else if matches!(r.phase.as_str(), "preparing" | "running") {
            stop(&self.shared, r, "cancel")?;
        }
        let value = r.value();
        drop(state);
        self.shared.wake.notify_all();
        Ok(value)
    }
    pub(crate) fn source(&self, id: &str, file_id: &str) -> Result<Value> {
        let r = self.record(id)?;
        verify_owner(&self.shared, &r)?;
        let p = restore_record(&self.shared, &r)?;
        let f = p.files.get(file_id).ok_or_else(missing)?;
        Ok(
            json!({"run_id":id,"file_id":file_id,"original_logical_path":f.path,"frozen_hash":f.hash,"text":f.text.as_ref(),"readonly":true,"source_stamp":r.stamp}),
        )
    }
    pub(crate) fn inputs(&self, id: &str) -> Result<Value> {
        let r = self.record(id)?;
        verify_owner(&self.shared, &r)?;
        let p = restore_record(&self.shared, &r)?;
        let files: Vec<_> = p.files.values().map(|f| json!({"file_id":f.id,"original_logical_path":f.path,"frozen_hash":f.hash,"roles":f.roles})).collect();
        Ok(json!({"run_id":id,"files":files,"source_stamp":r.stamp,"readonly":true}))
    }
    pub(crate) fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.wake.notify_all();
        if self.shared.owner {
            let state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            let _guard = self
                .shared
                .wake
                .wait_while(state, |s| {
                    (!s.finished && self.shared.launch_worker) || s.viewer_renders != 0
                })
                .unwrap_or_else(|e| e.into_inner());
        }
    }
    pub(crate) fn artifacts(&self, id: &str) -> Result<Value> {
        let r = self.record(id)?;
        let mut entries: Vec<Value> = if r.artifact_state == "published" {
            let bundle = validated(&self.shared, &r)?;
            bundle.artifacts.values().map(|a|json!({"artifact_id":a.name,"filename":a.name,"bytes":a.bytes.to_string(),"sha256":a.sha256,"url":format!("/api/runs/{id}/artifacts/{}",a.name)})).collect()
        } else {
            Vec::new()
        };
        if let Some((_, bytes, sha256)) = large_diagnostic_file(&self.shared, &r)? {
            entries.push(json!({"artifact_id":"large-worker-response","filename":"large-worker-response.json","bytes":bytes.to_string(),"sha256":sha256,"url":format!("/api/runs/{id}/artifacts/large-worker-response")}));
        }
        let dir = verify_owner(&self.shared, &r)?.child("control")?;
        let bytes = dir.read("provenance.json", CONTROL_LIMIT)?;
        entries.push(json!({"artifact_id":"provenance","filename":"provenance.json","bytes":bytes.len().to_string(),"sha256":hash(&bytes),"url":format!("/api/runs/{id}/artifacts/provenance")}));
        Ok(
            json!({"run_id":id,"format":"authenticated_artifact_list","artifacts":entries,"sources_included":false}),
        )
    }
    pub(crate) fn download(&self, id: &str, artifact_id: &str) -> Result<RunDownload> {
        let r = self.record(id)?;
        let lease = reference_lock(&self.shared, id, false)?;
        {
            let mut state = self.shared.state.lock().unwrap();
            if !state.records.contains_key(id) && self.shared.owner {
                return Err(gone());
            }
            *state.downloads.entry(id.into()).or_default() += 1;
        }
        let reference = DownloadReference {
            shared: self.shared.clone(),
            run_id: id.into(),
            _lease: lease,
        };
        let (file, filename, bytes, sha256, identity) = if ["provenance", "large-worker-response"]
            .contains(&artifact_id)
        {
            let f = if artifact_id == "large-worker-response" {
                large_diagnostic_file(&self.shared, &r)?
                    .ok_or_else(missing)?
                    .0
            } else {
                verify_owner(&self.shared, &r)?
                    .child("control")?
                    .open_file("provenance.json")?
            };
            let n = f
                .metadata()
                .map_err(|e| EditorError::io(&self.shared.root, e))?
                .len();
            let sha = hash_file(&f)?;
            let stat = rustix::fs::fstat(&f).map_err(|e| EditorError::io(&self.shared.root, e))?;
            (
                f,
                if artifact_id == "provenance" {
                    "provenance.json".into()
                } else {
                    "large-worker-response.json".into()
                },
                n,
                sha,
                crate::output::ArtifactIdentity {
                    dev: stat.st_dev,
                    ino: stat.st_ino,
                },
            )
        } else {
            let mut bundle = validated(&self.shared, &r)?;
            let a: crate::output::ValidatedArtifact =
                bundle.artifacts.remove(artifact_id).ok_or_else(missing)?;
            (a.file, a.name, a.bytes, a.sha256, a.identity)
        };
        Ok(RunDownload {
            file,
            filename,
            bytes,
            content_type: "application/octet-stream",
            sha256,
            identity,
            _reference: reference,
        })
    }
    pub(crate) fn delete(&self, id: &str) -> Result<Value> {
        self.mutation()?;
        self.record(id)?;
        let mut state = self.shared.state.lock().unwrap();
        if state.downloads.get(id).copied().unwrap_or(0) > 0 {
            return Err(busy("Run has active downloads"));
        }
        if let Some(v) = state.viewers.get(id) {
            if v.active() {
                return Err(busy("Run has an active Viewer capability; revoke it first"));
            }
        }
        let r = state.records.get(id).ok_or_else(missing)?;
        if r.active() {
            return Err(busy("Only terminal runs can be deleted"));
        }
        let _exclusive = reference_lock(&self.shared, id, true)?;
        let dir = verify_owner(&self.shared, r)?;
        let known = ["owner.json", "input", "control", "result", "viewer"];
        if dir
            .names()?
            .iter()
            .any(|n| n.to_str().is_none_or(|n| !known.contains(&n)))
        {
            return Err(invalid("Unknown run-owned root entry; cleanup refused"));
        }
        let inventory = cleanup::approved(&self.shared, r)?;
        let root = Directory::open(&self.shared.root)?;
        // Tombstone first prevents recovery or download of partially deleted data.
        root.create(
            format!(".deleted-{id}"),
            &encode(
                &json!({"schema_version":1,"run_id":id,"owner":r.owner,"inventory":inventory,"stamp":r.stamp,"wall_timeout_ms":r.wall_timeout_ms,"deleted_at_utc":utc()}),
            )?,
        )?;
        root.remove_child_checked(id, &r.owner, &inventory)?;
        state.records.remove(id);
        state.viewers.remove(id);
        Ok(json!({"run_id":id,"status":"deleted"}))
    }
    pub(crate) fn build_viewer(&self, id: &str) -> Result<Value> {
        viewer_server::build(self, id)
    }
    pub(crate) fn revoke_viewer(&self, id: &str) -> Result<Value> {
        viewer_server::revoke(self, id)
    }
}
fn large_diagnostic_file(shared: &Shared, r: &RunRecord) -> Result<Option<(File, u64, String)>> {
    let diagnostic = match &r.response {
        Some(
            worker::Message::PreparedFailed { diagnostic }
            | worker::Message::Failed { diagnostic, .. },
        ) => diagnostic,
        _ => return Ok(None),
    };
    if diagnostic["details_artifact"] != "large-worker-response" {
        return Ok(None);
    }
    let expected_hash = diagnostic["details_sha256"]
        .as_str()
        .ok_or_else(|| invalid("Missing full diagnostic hash"))?;
    let expected_bytes = diagnostic["details_bytes"]
        .as_str()
        .filter(|s| decimal(s))
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| invalid("Missing full diagnostic size"))?;
    let file = verify_owner(shared, r)?
        .child("control")?
        .open_file("large-worker-response.json")?;
    let bytes = file.metadata().map_err(|e| invalid(e.to_string()))?.len();
    let sha = hash_file(&file)?;
    if bytes != expected_bytes || sha != expected_hash {
        return Err(invalid(
            "Full diagnostic artifact differs from worker summary",
        ));
    }
    Ok(Some((file, bytes, sha)))
}
fn reference_lock(shared: &Shared, id: &str, exclusive: bool) -> Result<File> {
    let lock = record_dir(shared, id)?
        .child("control")?
        .open_file("references.lock")?;
    let operation = if exclusive {
        rustix::fs::FlockOperation::NonBlockingLockExclusive
    } else {
        rustix::fs::FlockOperation::NonBlockingLockShared
    };
    rustix::fs::flock(&lock, operation).map_err(|e| {
        if e == rustix::io::Errno::WOULDBLOCK {
            busy("Run has active cross-process references")
        } else {
            EditorError::io(&shared.root, e)
        }
    })?;
    Ok(lock)
}

fn authenticate_snapshot(r: &RunRecord, evidence: &SnapshotEvidence) -> Result<()> {
    if encode(evidence)? != encode(&r.snapshot)?
        || r.stamp.snapshot_id != evidence.snapshot_id
        || r.stamp.input_digest != evidence.input_digest
    {
        return Err(invalid(
            "Durable stamp/source evidence differs from frozen bundle",
        ));
    }
    Ok(())
}
fn restore_record(shared: &Shared, r: &RunRecord) -> Result<ProjectSnapshot> {
    let (project, evidence) = run_snapshot::restore_with_evidence(
        &shared.root.join(&r.run_id).join("input"),
        &r.snapshot.bundle_digest,
    )?;
    authenticate_snapshot(r, &evidence)?;
    Ok(project)
}
fn expectation(shared: &Shared, r: &RunRecord) -> Result<ResultBundleExpectation> {
    validate_worker_identity(r)?;
    let p = restore_record(shared, r)?;
    // Common preparation supplies the same logical source normalization on recovery.
    let prepared = super::analysis::prepare(&p)
        .map_err(|d| EditorError::common("E-EDITOR-RUN-INTEGRITY", d))?;
    let frozen_inputs: Vec<ResultInputEvidence> = crate::output::result_input_evidence(&prepared);
    if let Some(worker::Message::Report {
        report, evidence, ..
    }) = &r.response
    {
        let actual: ResultReportEvidence =
            serde_json::from_value(report.clone()).map_err(|e| invalid(e.to_string()))?;
        if encode(&actual)? != encode(evidence)?
            || !report.get("diagnostics").is_some_and(Value::is_array)
        {
            return Err(invalid("Worker report and report evidence disagree"));
        }
    }
    let (acquired_inputs, report) = match &r.response {
        Some(worker::Message::Report {
            inputs, evidence, ..
        }) => (Some(inputs.clone()), Some(evidence.clone())),
        Some(worker::Message::Running { inputs, .. }) => (Some(inputs.clone()), None),
        _ => (Some(frozen_inputs.clone()), None),
    };
    Ok(ResultBundleExpectation {
        run_id: r.run_id.clone(),
        started_at_utc: r.started_at_utc.clone(),
        frozen_inputs,
        acquired_inputs,
        report,
        manifest_sha256: r.manifest_sha256.clone(),
    })
}
fn validated(shared: &Shared, r: &RunRecord) -> Result<ValidatedResultBundle> {
    verify_owner(shared, r)?;
    if r.artifact_state != "published" {
        return Err(EditorError::new(
            "E-EDITOR-RUN-UNPUBLISHED",
            "No verified published result",
            409,
        ));
    }
    validate_result_bundle(
        &shared.root.join(&r.run_id).join("result"),
        &expectation(shared, r)?,
    )
    .map_err(|d| EditorError::common("E-EDITOR-RUN-INTEGRITY", d))
}
fn stop(shared: &Shared, r: &mut RunRecord, reason: &str) -> Result<()> {
    if r.stop_reason.is_none() {
        let mut candidate = r.clone();
        candidate.stop_reason = Some(reason.into());
        candidate.phase = "stopping".into();
        persist(shared, &candidate)?;
        *r = candidate;
    }
    Ok(())
}

// Monitoring must enforce a physical limit even if the control filesystem fails.
// User cancellation still reports a persistence error instead of a durable ack.
fn monitored_stop(shared: &Shared, r: &mut RunRecord, reason: &str) {
    if let Err(error) = stop(shared, r, reason) {
        r.stop_reason = Some(reason.into());
        r.phase = "stopping".into();
        r.diagnostics
            .push(json!({"phase":"control","durable_stop":false,"error":error}));
    }
}

fn stop_phase(reason: Option<&str>) -> &'static str {
    match reason {
        Some("cancel") => "cancelled",
        Some("timeout") => "timed_out",
        Some("resource_limit") => "resource_limited",
        _ => "interrupted",
    }
}
fn diagnostic(r: &mut RunRecord, phase: &str, d: &Value) {
    r.diagnostics
        .push(json!({"run_id":r.run_id,"source_stamp":r.stamp,"phase":phase,"diagnostic":d}));
}
fn validate_worker_identity(r: &RunRecord) -> Result<()> {
    if let Some(
        worker::Message::Running {
            run_id,
            started_at_utc,
            ..
        }
        | worker::Message::Report {
            run_id,
            started_at_utc,
            ..
        },
    ) = &r.response
    {
        if run_id != &r.run_id || r.started_at_utc.as_deref() != Some(started_at_utc) {
            return Err(invalid(
                "Worker response identity differs from durable run identity",
            ));
        }
    }
    Ok(())
}
fn finish_record(shared: &Shared, r: &mut RunRecord) -> Result<()> {
    if let Err(error) = validate_worker_identity(r) {
        r.phase = "integrity_failed".into();
        r.artifact_state = "invalid".into();
        r.diagnostics.push(
            json!({"run_id":r.run_id,"source_stamp":r.stamp,"phase":"verifying","error":error}),
        );
        return persist(shared, r);
    }
    let dir = verify_owner(shared, r)?;
    let has_manifest = if dir.names()?.iter().any(|n| n == "result") {
        dir.child("result")?
            .names()?
            .iter()
            .any(|n| n == "manifest.json")
    } else {
        false
    };
    if has_manifest {
        let checked = expectation(shared, r).and_then(|e| {
            validate_result_bundle(&shared.root.join(&r.run_id).join("result"), &e)
                .map_err(|d| EditorError::common("E-EDITOR-RUN-INTEGRITY", d))
        });
        match checked {
            Ok(b) => {
                r.manifest_sha256 = Some(b.manifest_sha256);
                r.artifact_state = "published".into();
                r.phase = classify_result(&b.termination, b.partial, b.exit_code).into();
                match &r.response {
                    Some(worker::Message::Report { report, .. }) => r.report = Some(report.clone()),
                    _ => {
                        r.report = None;
                        r.recovered_result = Some(
                            json!({"run_id":b.run_id,"inputs":b.inputs,"schema_version":b.schema_version,"termination":b.termination,"partial":b.partial,"exit_code":b.exit_code,"committed_events":b.committed_events,"finish_ps":b.finish_ps,"last_event_time_ps":b.last_event_time_ps,"event_processing_wall_seconds":null}),
                        );
                        r.worker_response = "unavailable".into();
                    }
                }
            }
            Err(e) => {
                r.phase = "integrity_failed".into();
                r.artifact_state = "invalid".into();
                r.diagnostics.push(
                    json!({"run_id":r.run_id,"source_stamp":r.stamp,"phase":"verifying","error":e}),
                );
            }
        }
    } else {
        r.artifact_state = "unpublished".into();
        match r.response.clone() {
            Some(worker::Message::PreparedFailed { diagnostic: d }) => {
                r.phase = "prepare_failed".into();
                diagnostic(r, "prepare", &d);
            }
            Some(worker::Message::Failed {
                diagnostic: d,
                prior_diagnostics,
                original_termination,
            }) => {
                r.phase = if original_termination.is_some()
                    || d["phase"] == "output"
                    || d["code"] == "E-0003"
                {
                    "output_failed"
                } else {
                    "execution_failed"
                }
                .into();
                for d in &prior_diagnostics {
                    diagnostic(r, "execution", d);
                }
                diagnostic(r, "output", &d);
            }
            Some(worker::Message::InfrastructureFailed { message }) => {
                r.phase = if r.stop_reason.is_some() {
                    stop_phase(r.stop_reason.as_deref())
                } else {
                    "execution_failed"
                }
                .into();
                r.diagnostics.push(json!({"run_id":r.run_id,"source_stamp":r.stamp,"phase":"worker","message":message}));
            }
            Some(worker::Message::Report { .. }) => {
                r.phase = "integrity_failed".into();
                r.artifact_state = "invalid".into();
            }
            _ => {
                r.phase = if r.stop_reason.is_some() {
                    stop_phase(r.stop_reason.as_deref())
                } else {
                    "execution_failed"
                }
                .into()
            }
        }
    }
    if let Some(worker::Message::Report { report, .. }) = &r.response {
        r.diagnostics.retain(|d| d["phase"] != "execution");
        if let Some(ds) = report.get("diagnostics").and_then(Value::as_array) {
            for d in ds {
                r.diagnostics.push(json!({"run_id":r.run_id,"source_stamp":r.stamp,"phase":"execution","diagnostic":d}));
            }
        }
    }
    if r.ended_at_utc.is_none() {
        r.ended_at_utc = Some(utc());
    }
    persist(shared, r)
}
fn classify_result(termination: &str, partial: bool, exit: u8) -> &'static str {
    if termination == "prep_failed" || exit == 2 {
        "prepare_failed"
    } else if partial && exit == 3 {
        "partial"
    } else if !partial && exit == 0 && matches!(termination, "events_exhausted" | "time_limit") {
        "completed"
    } else {
        "execution_failed"
    }
}
fn recover_record(shared: &Shared, r: &mut RunRecord) -> Result<()> {
    verify_owner(shared, r)?;
    if r.phase == "deleted" {
        return Ok(());
    }
    let control = record_dir(shared, &r.run_id)?.child("control")?;
    let snapshot = run_snapshot::inspect(
        &shared.root.join(&r.run_id).join("input"),
        &r.snapshot.bundle_digest,
    )
    .and_then(|e| authenticate_snapshot(r, &e));
    if let Err(error) = snapshot {
        r.phase = "integrity_failed".into();
        r.artifact_state = "invalid".into();
        r.viewer_state = "absent".into();
        r.diagnostics.push(
            json!({"run_id":r.run_id,"source_stamp":r.stamp,"phase":"recovery","error":error}),
        );
        return persist(shared, r);
    }
    if control.names()?.iter().any(|n| n == "worker-response.json") {
        let response = control.read("worker-response.json", worker::FRAME_LIMIT)?;
        r.response = Some(serde_json::from_slice(&response).map_err(|e| invalid(e.to_string()))?);
        r.worker_response = "available".into();
    }
    r.viewer_state = "absent".into();
    if r.active() || r.artifact_state == "published" {
        if r.stop_reason.is_none() && r.active() {
            r.stop_reason = Some("service_shutdown".into());
        }
        finish_record(shared, r)?;
    } else {
        persist(shared, r)?;
    }
    Ok(())
}
fn spawn_worker(
    shared: &Shared,
    request: &worker::Request,
) -> Result<(std::process::Child, Instant)> {
    if hash_file(&File::open(&shared.binary).map_err(|e| EditorError::io(&shared.binary, e))?)?
        != shared.binary_hash
    {
        return Err(invalid(
            "Worker executable changed after coordinator startup",
        ));
    }
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    // process_group is a safe standard-library API; no pre_exec/unsafe closure.
    let started = Instant::now();
    let mut child = Command::new(&shared.binary)
        .arg("--ned-editor-run-worker")
        .process_group(0)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| EditorError::new("E-EDITOR-RUN-WORKER", e.to_string(), 500))?;
    if let Err(e) = worker::write_frame(child.stdin.as_mut().expect("worker pipe"), request) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    child.stdin.take();
    Ok((child, started))
}
fn probe_worker(shared: &Shared) -> Result<()> {
    let request = worker::Request {
        schema_version: 1,
        probe: true,
        parent_pid: std::process::id(),
        run_id: String::new(),
        started_at_utc: String::new(),
        run_root: PathBuf::new(),
        bundle_digest: String::new(),
    };
    let (mut child, _) = spawn_worker(shared, &request)?;
    let pid = rustix::process::Pid::from_raw(child.id() as i32)
        .ok_or_else(|| invalid("Invalid worker PID"))?;
    let pidfd = match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(invalid(format!("Stable worker identity unavailable: {e}")));
        }
    };
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(worker::read_frame::<worker::Message>(&mut stdout));
    });
    let response = rx.recv_timeout(Duration::from_secs(5));
    let ok = matches!(response, Ok(Ok(worker::Message::Ready)));
    if !ok {
        let _ = rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::Kill);
    }
    let status = child.wait().map_err(|e| invalid(e.to_string()))?;
    if !ok || !status.success() {
        return Err(busy("Safe worker startup/resource limits unavailable"));
    }
    Ok(())
}
fn coordinate(shared: Arc<Shared>) {
    loop {
        let mut state = shared.state.lock().unwrap();
        if shared.shutdown.load(Ordering::Acquire) {
            for r in state.records.values_mut().filter(|r| r.phase == "queued") {
                r.phase = "interrupted".into();
                r.stop_reason = Some("service_shutdown".into());
                r.ended_at_utc = Some(utc());
                let _ = persist(&shared, r);
            }
            state.finished = true;
            shared.wake.notify_all();
            break;
        }
        let next = if shared.launch_worker {
            state
                .records
                .values()
                .filter(|r| r.phase == "queued")
                .min_by_key(|r| (r.created_ms.parse::<u64>().unwrap_or(0), r.run_id.clone()))
                .map(|r| r.run_id.clone())
        } else {
            None
        };
        if let Some(id) = next {
            let r = state.records.get_mut(&id).unwrap();
            r.phase = "preparing".into();
            r.started_at_utc = Some(utc());
            if let Err(e) = persist(&shared, r) {
                r.phase = "output_failed".into();
                r.diagnostics.push(json!({"phase":"control","error":e}));
                drop(state);
                continue;
            }
            let request = worker::Request {
                schema_version: 1,
                probe: false,
                parent_pid: std::process::id(),
                run_id: id.clone(),
                started_at_utc: r.started_at_utc.clone().unwrap(),
                run_root: shared.root.join(&id),
                bundle_digest: r.snapshot.bundle_digest.clone(),
            };
            let timeout = r.wall_timeout_ms.parse::<u64>().unwrap_or(600000);
            drop(state);
            execute_child(&shared, &id, request, timeout);
        } else {
            let _ = shared.wake.wait_timeout(state, Duration::from_millis(200));
        }
    }
}
fn stderr_ring(mut reader: impl std::io::Read) -> Vec<u8> {
    use std::collections::VecDeque;
    let mut tail = VecDeque::with_capacity(LOG_LIMIT);
    let mut buf = [0; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let remove = tail.len().saturating_add(n).saturating_sub(LOG_LIMIT);
                tail.drain(..remove);
                tail.extend(&buf[..n]);
            }
        }
    }
    tail.into()
}
fn execute_child(shared: &Shared, id: &str, request: worker::Request, timeout: u64) {
    let (mut child, started) = match spawn_worker(shared, &request) {
        Ok(c) => c,
        Err(e) => {
            let mut st = shared.state.lock().unwrap();
            let r = st.records.get_mut(id).unwrap();
            r.response = Some(worker::Message::InfrastructureFailed {
                message: e.to_string(),
            });
            let _ = finish_record(shared, r);
            return;
        }
    };
    let pid = rustix::process::Pid::from_raw(child.id() as i32).expect("owned child PID");
    {
        let mut state = shared.state.lock().unwrap();
        let r = state.records.get_mut(id).unwrap();
        r.worker_pid = Some(child.id().to_string());
        if let Err(error) = persist(shared, r) {
            let _ = child.kill();
            let _ = child.wait();
            r.response = Some(worker::Message::InfrastructureFailed {
                message: error.to_string(),
            });
            let _ = finish_record(shared, r);
            return;
        }
    }
    let pidfd = match rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            let mut st = shared.state.lock().unwrap();
            let r = st.records.get_mut(id).unwrap();
            r.response = Some(worker::Message::InfrastructureFailed {
                message: format!("Stable process identity unavailable: {e}"),
            });
            let _ = finish_record(shared, r);
            return;
        }
    };
    let stderr = child.stderr.take().unwrap();
    let log_thread = std::thread::spawn(move || stderr_ring(stderr));
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let response_thread = std::thread::spawn(move || {
        loop {
            match worker::read_frame::<worker::Message>(&mut stdout) {
                Ok(m) => {
                    if tx.send(Ok(m)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
        // Drain invalid or trailing bytes so a protocol failure cannot block reap.
        stderr_ring(stdout)
    });
    let mut stopping_at: Option<Instant> = None;
    let mut monitored = Instant::now();
    let mut protocol_closed = false;
    let status = loop {
        let message = rx.recv_timeout(Duration::from_millis(100));
        let mut state = shared.state.lock().unwrap();
        let r = state.records.get_mut(id).unwrap();
        match message {
            Ok(Ok(worker::Message::Ready)) => {}
            Ok(Ok(message @ worker::Message::Running { .. })) => {
                if r.phase == "preparing" {
                    r.phase = "running".into();
                }
                r.response = Some(message);
                let _ = persist(shared, r);
            }
            Ok(Ok(m)) => {
                if !matches!(r.phase.as_str(), "verifying" | "stopping") {
                    r.phase = "verifying".into();
                }
                r.response = Some(m);
                r.worker_response = "available".into();
                let _ = persist(shared, r);
            }
            Ok(Err(e)) => {
                protocol_closed = true;
                if r.response
                    .as_ref()
                    .is_none_or(|m| matches!(m, worker::Message::Running { .. }))
                {
                    r.diagnostics.push(json!({"run_id":id,"source_stamp":r.stamp,"phase":"worker","message":e.to_string()}));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => protocol_closed = true,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Completion acceptance and stop requests use this same run-state lock.
        match child.try_wait() {
            Ok(Some(status)) => {
                if r.phase != "stopping" {
                    r.phase = "verifying".into();
                }
                break Some(status);
            }
            Err(e) => {
                r.diagnostics
                    .push(json!({"phase":"worker","message":e.to_string()}));
                let _ = stop(shared, r, "service_shutdown");
            }
            Ok(None) => {}
        }
        if shared.shutdown.load(Ordering::Acquire)
            && matches!(r.phase.as_str(), "preparing" | "running")
        {
            if let Err(error) = stop(shared, r, "service_shutdown") {
                // Process shutdown cannot leave an orphan when the filesystem is
                // unavailable. Preserve this exception explicitly, without claiming
                // a durable cancellation acknowledgment.
                r.stop_reason = Some("service_shutdown".into());
                r.phase = "stopping".into();
                r.diagnostics
                    .push(json!({"phase":"control","durable_stop":false,"error":error}));
            }
        }
        if started.elapsed() >= Duration::from_millis(timeout)
            && matches!(r.phase.as_str(), "preparing" | "running")
        {
            monitored_stop(shared, r, "timeout");
        }
        if monitored.elapsed() >= Duration::from_secs(1) {
            monitored = Instant::now();
            let usage = record_dir(shared, id)
                .and_then(|d| d.bytes())
                .and_then(|run| {
                    Directory::open(&shared.root)?
                        .bytes()
                        .map(|total| (run, total))
                });
            match usage {
                Ok((run, total)) if run > RUN_LIMIT || total > RUNS_LIMIT => {
                    r.resource_observation = Some(
                        json!({"run_bytes":run.to_string(),"runs_bytes":total.to_string(),"soft_limit_overshoot":true}),
                    );
                    monitored_stop(shared, r, "resource_limit");
                }
                Err(e) => {
                    r.diagnostics.push(json!({"phase":"monitor","error":e}));
                    monitored_stop(shared, r, "resource_limit");
                }
                _ => {}
            }
        }
        if protocol_closed && r.response.is_none() && r.phase == "preparing" {
            let _ = stop(shared, r, "service_shutdown");
        }
        if r.phase == "stopping" {
            if stopping_at.is_none() {
                stopping_at = Some(Instant::now());
                let _ = rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::Term);
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Term);
            } else if stopping_at.unwrap().elapsed() >= Duration::from_secs(2) {
                let _ = rustix::process::pidfd_send_signal(&pidfd, rustix::process::Signal::Kill);
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::Kill);
                drop(state);
                break child.wait().ok();
            }
        }
        drop(state);
    };
    // try_wait may observe exit ahead of pipe reader scheduling; consume its bounded
    // remaining frames before final classification. The child cannot be signalled now.
    let stdout_tail = response_thread.join().unwrap_or_default();
    let logs = log_thread.join().unwrap_or_default();
    let mut state = shared.state.lock().unwrap();
    let r = state.records.get_mut(id).unwrap();
    for message in rx.try_iter().flatten() {
        if !matches!(
            message,
            worker::Message::Ready | worker::Message::Running { .. }
        ) {
            r.response = Some(message);
            r.worker_response = "available".into();
        }
    }
    if let Ok(control) = record_dir(shared, id).and_then(|d| d.child("control")) {
        let _ = control.replace("stderr.log", &logs);
        let _ = control.replace("stdout.log", &stdout_tail);
        if let Some(response) = &r.response {
            if !matches!(response, worker::Message::Running { .. }) {
                let _ = control.replace(
                    "worker-response.json",
                    &encode(response).unwrap_or_default(),
                );
            }
        }
    }
    use std::os::unix::process::ExitStatusExt;
    if status.is_some_and(|s| s.signal() == Some(rustix::process::Signal::Xfsz as i32))
        && r.stop_reason.is_none()
    {
        r.stop_reason = Some("resource_limit".into());
    }
    r.worker_exit = status.map(|s| json!({"code":s.code(),"signal":s.signal(),"reaped":true}));
    if let Err(e) = finish_record(shared, r) {
        r.phase = "integrity_failed".into();
        r.artifact_state = "invalid".into();
        r.diagnostics.push(json!({"phase":"verifying","error":e}));
        let _ = persist(shared, r);
    }
    shared.wake.notify_all();
}
fn recover_cleanup(shared: &Shared) -> Result<()> {
    let root = Directory::open(&shared.root)?;
    for name in root.names()? {
        let Some(n) = name.to_str() else {
            continue;
        };
        if let Some(id) = n.strip_prefix(".deleted-") {
            valid_id(id)?;
            let tombstone: Value = serde_json::from_slice(&root.read(n, CONTROL_LIMIT)?)
                .map_err(|e| invalid(e.to_string()))?;
            if tombstone["run_id"] != id || tombstone["schema_version"] != 1 {
                return Err(invalid("Invalid deletion tombstone"));
            }
            let owner: Identity = serde_json::from_value(tombstone["owner"].clone())
                .map_err(|e| invalid(e.to_string()))?;
            if root.names()?.iter().any(|known| known == id) {
                let inventory: Vec<storage::OwnedEntry> =
                    serde_json::from_value(tombstone["inventory"].clone())
                        .map_err(|e| invalid(e.to_string()))?;
                root.remove_child_checked(id, &owner, &inventory)?;
            }
        } else if let Some(id) = n.strip_prefix(".freeze-") {
            if valid_id(id).is_err() {
                continue;
            }
            let dir = root.child(n)?;
            // No marker means ownership was never durably established; preserve it.
            if !dir.names()?.iter().any(|known| known == "owner.json") {
                continue;
            }
            let marker: Value = serde_json::from_slice(&dir.read("owner.json", 4096)?)
                .map_err(|e| invalid(e.to_string()))?;
            if marker != json!({"schema_version":1,"run_id":id,"identity":dir.identity()}) {
                return Err(invalid("Invalid freeze ownership marker"));
            }
            if dir.names()?.iter().any(|known| {
                known.to_str().is_none_or(|s| {
                    !["owner.json", "input", "control", "result", "viewer"].contains(&s)
                })
            }) {
                return Err(invalid("Unknown staged run entry"));
            }
            let inventory = cleanup::approved_stage(&dir, id)?;
            root.remove_child_checked(n, &dir.identity(), &inventory)?;
        }
    }
    Ok(())
}
