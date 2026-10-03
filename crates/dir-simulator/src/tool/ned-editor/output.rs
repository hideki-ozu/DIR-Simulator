//! Explicit project output with descriptor-relative publication and durable recovery.
use super::input::{FileRole, ProjectSnapshot, SnapshotInputSource};
use super::{EditorError, Result, absolute, hash, random_id};
use crate::input::{inspect_config, prepare_with_source};
use rustix::fs::{self as rfs, AtFlags, FileType, FlockOperation, Mode, OFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Identity {
    dev: u64,
    ino: u64,
}
impl Identity {
    fn stat(s: &rfs::Stat) -> Self {
        Self {
            dev: s.st_dev,
            ino: s.st_ino,
        }
    }
}
fn conflict(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-SAVE-CONFLICT", message, 409)
}
fn invalid(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-RECOVERY-INVALID", message, 422)
}
fn check_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.to_str().is_none()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
    {
        return Err(EditorError::new(
            "E-EDITOR-TARGET-DENIED",
            "Expected a normalized absolute UTF-8 path",
            403,
        ));
    }
    Ok(())
}
struct Directory {
    chain: Vec<(PathBuf, File, Identity)>,
}
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        check_path(path)?;
        let root: File = rfs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| EditorError::io(path, e))?
        .into();
        let id = Identity::stat(&rfs::fstat(&root).map_err(|e| EditorError::io(path, e))?);
        let mut result = Self {
            chain: vec![(PathBuf::from("/"), root, id)],
        };
        for component in path.components() {
            if let Component::Normal(name) = component {
                let parent = &result.chain.last().unwrap().1;
                let file: File = rfs::openat(
                    parent,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| EditorError::io(path, e))?
                .into();
                let identity =
                    Identity::stat(&rfs::fstat(&file).map_err(|e| EditorError::io(path, e))?);
                let logical = result.chain.last().unwrap().0.join(name);
                result.chain.push((logical, file, identity));
            }
        }
        result.verify()?;
        Ok(result)
    }
    fn fd(&self) -> &File {
        &self.chain.last().unwrap().1
    }
    fn identity(&self) -> Identity {
        self.chain.last().unwrap().2.clone()
    }
    fn verify(&self) -> Result<()> {
        for window in self.chain.windows(2) {
            let (path, _, expected) = &window[1];
            let stat = rfs::statat(
                &window[0].1,
                path.file_name().unwrap(),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(|e| EditorError::io(path, e))?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
                || Identity::stat(&stat) != *expected
            {
                return Err(conflict(format!(
                    "Directory identity changed: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
    fn sync(&self) -> Result<()> {
        rfs::fsync(self.fd()).map_err(|e| EditorError::io(&self.chain.last().unwrap().0, e))
    }
    fn names(&self) -> Result<Vec<OsString>> {
        let mut stream = rfs::Dir::read_from(self.fd())
            .map_err(|e| EditorError::io(&self.chain.last().unwrap().0, e))?;
        let mut names = Vec::new();
        while let Some(entry) = stream.read() {
            let entry = entry.map_err(|e| EditorError::io(&self.chain.last().unwrap().0, e))?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                names.push(OsStr::from_bytes(name).to_owned());
            }
        }
        names.sort();
        Ok(names)
    }
}
fn parent(path: &Path) -> Result<Directory> {
    Directory::open(path.parent().ok_or_else(|| invalid("Missing parent"))?)
}
fn stat_optional(path: &Path) -> Result<Option<rfs::Stat>> {
    if path == Path::new("/") {
        return rfs::fstat(Directory::open(path)?.fd())
            .map(Some)
            .map_err(|e| EditorError::io(path, e));
    }
    let mut existing = path.parent().ok_or_else(|| invalid("Missing parent"))?;
    while let Err(e) = Directory::open(existing) {
        // Only a genuinely missing ancestor is absent; symlinks and non-directories are errors.
        match std::fs::symlink_metadata(existing) {
            Err(io) if io.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or(e.clone())?;
            }
            _ => return Err(e),
        }
    }
    if existing != path.parent().unwrap() {
        return Ok(None);
    }
    let dir = Directory::open(existing)?;
    match rfs::statat(
        dir.fd(),
        path.file_name().unwrap(),
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Ok(stat) => Ok(Some(stat)),
        Err(e) if e == rustix::io::Errno::NOENT => Ok(None),
        Err(e) => Err(EditorError::io(path, e)),
    }
}
fn read_regular(path: &Path, own_link: bool) -> Result<(Vec<u8>, u32, Identity)> {
    let dir = parent(path)?;
    dir.verify()?;
    let fd = rfs::openat(
        dir.fd(),
        path.file_name().unwrap(),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| EditorError::io(path, e))?;
    let mut file: File = fd.into();
    let stat = rfs::fstat(&file).map_err(|e| EditorError::io(path, e))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || (stat.st_nlink != 1 && !(own_link && stat.st_nlink == 2))
    {
        return Err(conflict(format!(
            "Non-regular or hard-linked target: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| EditorError::io(path, e))?;
    let after = rfs::fstat(&file).map_err(|e| EditorError::io(path, e))?;
    if Identity::stat(&stat) != Identity::stat(&after)
        || stat.st_size != after.st_size
        || stat.st_mode != after.st_mode
        || stat.st_nlink != after.st_nlink
    {
        return Err(conflict("File changed during observation"));
    }
    dir.verify()?;
    Ok((bytes, stat.st_mode & 0o7777, Identity::stat(&stat)))
}
fn mkdir_owned(path: &Path, mode: u32) -> Result<Identity> {
    let dir = parent(path)?;
    dir.verify()?;
    rfs::mkdirat(
        dir.fd(),
        path.file_name().unwrap(),
        Mode::from_bits_truncate(mode),
    )
    .map_err(|e| EditorError::io(path, e))?;
    dir.sync()?;
    Directory::open(path).map(|directory| directory.identity())
}
fn ensure_state_dir(path: &Path) -> Result<()> {
    check_path(path)?;
    if stat_optional(path)?.is_none() {
        let ancestor = path.parent().ok_or_else(|| invalid("No state parent"))?;
        if stat_optional(ancestor)?.is_none() {
            ensure_state_dir(ancestor)?;
        } else {
            Directory::open(ancestor)?;
        }
        mkdir_owned(path, 0o700)?;
    }
    let directory = Directory::open(path)?;
    let metadata = directory
        .fd()
        .metadata()
        .map_err(|e| EditorError::io(path, e))?;
    let uid = std::fs::metadata("/proc/self")
        .map_err(|e| EditorError::io(path, e))?
        .uid();
    if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(EditorError::new(
            "E-EDITOR-STATE",
            "State directory must be owned by the current user and not writable by other users",
            403,
        ));
    }
    Ok(())
}
fn create_file(path: &Path, bytes: &[u8], mode: u32, set_mode: bool) -> Result<(u32, Identity)> {
    let dir = parent(path)?;
    dir.verify()?;
    let file: File = rfs::openat(
        dir.fd(),
        path.file_name().unwrap(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(mode),
    )
    .map_err(|e| EditorError::io(path, e))?
    .into();
    let identity = Identity::stat(&rfs::fstat(&file).map_err(|e| EditorError::io(path, e))?);
    let mut file = file;
    file.write_all(bytes)
        .map_err(|e| EditorError::io(path, e))?;
    if set_mode {
        rfs::fchmod(&file, Mode::from_bits_truncate(mode)).map_err(|e| EditorError::io(path, e))?;
    }
    file.sync_all().map_err(|e| EditorError::io(path, e))?;
    let actual = file
        .metadata()
        .map_err(|e| EditorError::io(path, e))?
        .mode()
        & 0o7777;
    dir.verify()?;
    dir.sync()?;
    Ok((actual, identity))
}
fn atomic_state(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_file_name(random_id(".state-")?);
    create_file(&temp, bytes, 0o600, true)?;
    let dir = parent(path)?;
    dir.verify()?;
    rfs::renameat(
        dir.fd(),
        temp.file_name().unwrap(),
        dir.fd(),
        path.file_name().unwrap(),
    )
    .map_err(|e| EditorError::io(path, e))?;
    dir.sync()
}

fn registry_lease(state_root: &Path) -> Result<File> {
    let directory = Directory::open(state_root)?;
    let file: File = rfs::openat(
        directory.fd(),
        ".registry.lock",
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|e| EditorError::io(state_root, e))?
    .into();
    let stat = rfs::fstat(&file).map_err(|e| EditorError::io(state_root, e))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(invalid("Invalid registry lock inode"));
    }
    rfs::flock(&file, FlockOperation::NonBlockingLockExclusive)
        .map_err(|e| EditorError::new("E-EDITOR-TARGET-BUSY", e.to_string(), 409))?;
    directory.verify()?;
    Ok(file)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Before {
    Absent,
    Present { hash: String, mode: u32 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TargetKind {
    NewProject,
    SourceProject,
    ManagedExport,
}
impl TargetKind {
    fn name(&self) -> &str {
        match self {
            Self::NewProject => "new_project",
            Self::SourceProject => "source_project",
            Self::ManagedExport => "managed_export",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Target {
    id: String,
    path: PathBuf,
    kind: TargetKind,
    generation: u64,
    baseline: BTreeMap<PathBuf, Before>,
    roots: Vec<PathBuf>,
    ned: BTreeSet<PathBuf>,
    directories: BTreeSet<PathBuf>,
    optional_layouts: BTreeSet<PathBuf>,
    protected_layouts: BTreeSet<PathBuf>,
    allowed_files: BTreeSet<PathBuf>,
    parent_identity: Identity,
    root_identity: Option<Identity>,
    config: PathBuf,
    cwd: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryData {
    schema_version: u32,
    export_root: PathBuf,
    targets: BTreeMap<String, Target>,
    input_regions: Vec<PathBuf>,
}
#[derive(Clone)]
pub(crate) struct TargetRegistry {
    export_root: PathBuf,
    state_root: PathBuf,
    data: RegistryData,
}
impl TargetRegistry {
    pub fn open(export_root: &Path, state_root: &Path) -> Result<Self> {
        let cwd = std::env::current_dir().map_err(|e| EditorError::io(export_root, e))?;
        let export_root = absolute(export_root, &cwd);
        let state_root = absolute(state_root, &cwd);
        Directory::open(&export_root)?;
        ensure_state_dir(&state_root)?;
        ensure_state_dir(&state_root.join("recovery"))?;
        ensure_state_dir(&state_root.join("locks"))?;
        let _lease = registry_lease(&state_root)?;
        let registry_path = state_root.join("targets.json");
        let data = if stat_optional(&registry_path)?.is_some() {
            let (bytes, _, _) = read_regular(&registry_path, false)?;
            let value = super::controller::strict_json(&bytes)?;
            let data: RegistryData =
                serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
            if data.schema_version != 1 || data.export_root != export_root {
                return Err(invalid("Registry schema or export root mismatch"));
            }
            data
        } else {
            RegistryData {
                schema_version: 1,
                export_root: export_root.clone(),
                targets: BTreeMap::new(),
                input_regions: Vec::new(),
            }
        };
        let mut registry = Self {
            export_root,
            state_root,
            data,
        };
        registry.validate_registry()?;
        registry.reconcile_completed()?;
        Ok(registry)
    }
    fn validate_registry(&self) -> Result<()> {
        for (id, t) in &self.data.targets {
            if id != &t.id || !id.starts_with("target-") {
                return Err(invalid("Invalid registered target ID"));
            }
            check_path(&t.path)?;
            check_path(&t.config)?;
            check_path(&t.cwd)?;
            if t.kind != TargetKind::SourceProject
                && (!t.path.starts_with(&self.export_root)
                    || t.path == self.export_root
                    || self.data.input_regions.iter().any(|p| overlap(p, &t.path)))
            {
                return Err(invalid(
                    "Registered output outside permitted root or overlaps input",
                ));
            }
            for path in &t.allowed_files {
                check_path(path)?;
                if t.kind != TargetKind::SourceProject && !path.starts_with(&t.path) {
                    return Err(invalid("Registered file escaped output root"));
                }
            }
            if t.baseline.keys().any(|p| !t.allowed_files.contains(p)) {
                return Err(invalid("Baseline is not independently authorized"));
            }
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        let path = self.state_root.join("targets.json");
        if stat_optional(&path)?.is_some() {
            let (bytes, _, _) = read_regular(&path, false)?;
            let data: RegistryData =
                serde_json::from_value(super::controller::strict_json(&bytes)?)
                    .map_err(|e| invalid(e.to_string()))?;
            if data.schema_version != 1 || data.export_root != self.export_root {
                return Err(invalid("Registry configuration mismatch"));
            }
            self.data = data;
            self.validate_registry()?;
        }
        Ok(())
    }
    fn persist(&self) -> Result<()> {
        atomic_state(
            &self.state_root.join("targets.json"),
            &serde_json::to_vec_pretty(&self.data).map_err(|e| invalid(e.to_string()))?,
        )
    }
    pub fn register_source(&mut self, project: &ProjectSnapshot) -> Result<String> {
        if !matches!(project.origin, super::input::ProjectOrigin::Disk)
            || project.files.values().any(|f| f.stat.is_none())
        {
            return Err(EditorError::new(
                "E-EDITOR-TARGET-DENIED",
                "An unsaved project requires Save As",
                403,
            ));
        }
        let _lease = registry_lease(&self.state_root)?;
        self.refresh()?;
        self.reconcile_completed()?;
        let regions = project.input_regions();
        if regions.iter().any(|p| overlap(p, &self.state_root)) {
            return Err(EditorError::new(
                "E-EDITOR-STATE",
                "State root overlaps input region; select a different state directory",
                403,
            ));
        }
        let path = project
            .config
            .parent()
            .ok_or_else(|| invalid("Config parent missing"))?
            .to_path_buf();
        let existing = self
            .data
            .targets
            .values()
            .find(|t| t.kind == TargetKind::SourceProject && t.config == project.config)
            .cloned();
        let id = existing
            .as_ref()
            .map(|t| t.id.clone())
            .unwrap_or(random_id("target-")?);
        let mut baseline = BTreeMap::new();
        let mut allowed = BTreeSet::new();
        let mut ned = BTreeSet::new();
        for f in project.files.values() {
            baseline.insert(
                f.path.clone(),
                Before::Present {
                    hash: f.origin_hash.clone(),
                    mode: f
                        .stat
                        .as_ref()
                        .ok_or_else(|| invalid("Source file has no disk baseline"))?
                        .mode,
                },
            );
            allowed.insert(f.path.clone());
            if f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })) {
                ned.insert(f.path.clone());
            }
        }
        let mut protected = BTreeSet::new();
        for layout in project.layouts.values() {
            allowed.insert(layout.path.clone());
            baseline.insert(
                layout.path.clone(),
                match (&layout.hash, &layout.stat) {
                    (Some(hash), Some(stat)) => Before::Present {
                        hash: hash.clone(),
                        mode: stat.mode,
                    },
                    _ => Before::Absent,
                },
            );
            if layout.warning.is_some() {
                protected.insert(layout.path.clone());
            }
        }
        let t = Target {
            id: id.clone(),
            path: path.clone(),
            kind: TargetKind::SourceProject,
            generation: existing.map_or(0, |t| t.generation + 1),
            baseline,
            roots: project.header.roots.clone(),
            ned,
            directories: project
                .directories
                .keys()
                .cloned()
                .chain(
                    project
                        .files
                        .values()
                        .filter_map(|file| file.path.parent().map(Path::to_path_buf)),
                )
                .collect(),
            optional_layouts: project
                .layouts
                .values()
                .map(|layout| layout.path.clone())
                .collect(),
            protected_layouts: protected,
            allowed_files: allowed,
            parent_identity: parent(&path)?.identity(),
            root_identity: Some(Directory::open(&path)?.identity()),
            config: project.config.clone(),
            cwd: project.cwd.clone(),
        };
        self.data.targets.retain(|_, target| {
            target.kind == TargetKind::SourceProject
                || !regions.iter().any(|region| overlap(region, &target.path))
        });
        self.data.input_regions = regions;
        self.data.targets.insert(id.clone(), t);
        self.persist()?;
        Ok(id)
    }
    pub fn register_destination(&mut self, relative: &str) -> Result<Value> {
        let _lease = registry_lease(&self.state_root)?;
        self.refresh()?;
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(EditorError::new(
                "E-EDITOR-TARGET-DENIED",
                "Destination must be a relative child directory",
                403,
            ));
        }
        let path = self.export_root.join(relative);
        check_path(&path)?;
        if self.data.input_regions.iter().any(|p| overlap(p, &path))
            || overlap(&path, &self.state_root)
        {
            return Err(EditorError::new(
                "E-EDITOR-TARGET-DENIED",
                "Output overlaps input or editor state",
                403,
            ));
        }
        if let Some(t) = self
            .data
            .targets
            .values()
            .find(|t| t.path == path && t.kind == TargetKind::ManagedExport)
        {
            return Ok(target_value(t));
        }
        if self
            .data
            .targets
            .values()
            .any(|t| t.kind == TargetKind::ManagedExport && overlap(&t.path, &path))
        {
            return Err(conflict("Destination overlaps a managed project"));
        }
        let parent_identity = parent(&path)?.identity();
        let root_identity = match stat_optional(&path)? {
            Some(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
                let dir = Directory::open(&path)?;
                if !dir.names()?.is_empty() {
                    return Err(conflict("Destination must be absent or empty"));
                }
                Some(dir.identity())
            }
            Some(_) => return Err(conflict("Destination must be a real directory")),
            None => None,
        };
        let id = random_id("target-")?;
        let target = Target {
            id: id.clone(),
            path: path.clone(),
            kind: TargetKind::NewProject,
            generation: 0,
            baseline: BTreeMap::new(),
            roots: Vec::new(),
            ned: BTreeSet::new(),
            directories: BTreeSet::new(),
            optional_layouts: BTreeSet::new(),
            protected_layouts: BTreeSet::new(),
            allowed_files: BTreeSet::new(),
            parent_identity,
            root_identity,
            config: path.join("project.ini"),
            cwd: path.clone(),
        };
        let value = target_value(&target);
        self.data.targets.insert(id, target);
        self.persist()?;
        Ok(value)
    }
    pub fn targets(&self) -> Vec<Value> {
        self.data.targets.values().map(target_value).collect()
    }
    pub fn export_roots(&self) -> Vec<Value> {
        vec![json!({"id":"export-root","path":self.export_root})]
    }
    pub fn pending_recoveries(&self) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        for name in Directory::open(&self.state_root.join("recovery"))?.names()? {
            let id = name.to_string_lossy().to_string();
            if !safe_id(&id, "save-") {
                continue;
            }
            if stat_optional(&manifest_path(self, &id))?.is_none() {
                continue;
            }
            let manifest = self.load_manifest(&id)?;
            if manifest.state != "completed" && manifest.state != "restored" {
                result.push(json!({"id":id,"path":manifest.target.path,"state":manifest.state}));
            }
        }
        Ok(result)
    }
}
fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn target_value(t: &Target) -> Value {
    json!({"id":t.id,"path":t.path,"kind":t.kind.name()})
}
fn safe_id(id: &str, prefix: &str) -> bool {
    id.starts_with(prefix)
        && id[prefix.len()..].len() == 64
        && id[prefix.len()..].bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PlannedFile {
    path: PathBuf,
    bytes: Vec<u8>,
    hash: String,
    before: Before,
    rank: u8,
}
#[derive(Clone)]
pub(crate) struct SavePlan {
    pub id: String,
    pub revision: u64,
    pub input_revision: u64,
    pub snapshot_id: String,
    pub digest: String,
    pub target_id: String,
    pub output_path: PathBuf,
    pub requires_layout_confirmation: bool,
    target: Target,
    files: Vec<PlannedFile>,
    directories: Vec<PathBuf>,
    source: SnapshotInputSource,
    baseline_digest: String,
}
fn baseline_digest(t: &Target) -> String {
    hash(&serde_json::to_vec(t).expect("serializable target"))
}
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn patch_ini(
    text: &str,
    config: &Path,
    cwd: &Path,
    roots: &[PathBuf],
    workload: Option<&Path>,
    model_config: Option<&Path>,
) -> Result<String> {
    let old = inspect_config(text, config, cwd)
        .map_err(|e| EditorError::common("E-EDITOR-INI-INDEX", e))?;
    let mut replacements = BTreeMap::from([(
        "ned-path",
        roots
            .iter()
            .map(|p| quoted(p.to_str().unwrap()))
            .collect::<Vec<_>>()
            .join(";"),
    )]);
    if let Some(path) = workload {
        replacements.insert("workload", quoted(path.to_str().unwrap()));
    }
    if let Some(path) = model_config {
        replacements.insert("model-config", quoted(path.to_str().unwrap()));
    }
    let mut patches = Vec::new();
    let mut offset = 0usize;
    let mut general = false;
    let mut found = BTreeSet::new();
    for original in text.split_inclusive('\n') {
        let bom = if offset == 0 {
            original.len() - original.trim_start_matches('\u{feff}').len()
        } else {
            0
        };
        let line = original[bom..]
            .trim_end_matches('\n')
            .trim_end_matches('\r');
        let stripped = line.trim_matches([' ', '\t']);
        if stripped.starts_with('[') {
            general = stripped == "[General]";
        } else if general && !stripped.is_empty() && !stripped.starts_with(['#', ';']) {
            let equals = line
                .find('=')
                .ok_or_else(|| invalid("INI assignment index missing"))?;
            let key = line[..equals].trim_matches([' ', '\t']);
            if let Some(value) = replacements.get(key) {
                let right = &line[equals + 1..];
                let start = equals + 1 + right.len() - right.trim_start_matches([' ', '\t']).len();
                let end = line.len() - (right.len() - right.trim_end_matches([' ', '\t']).len());
                if old.general.get(key).map(String::as_str) != Some(&line[start..end])
                    || !found.insert(key.to_string())
                {
                    return Err(EditorError::new(
                        "E-EDITOR-INI-INDEX",
                        "INI index does not match common inspection",
                        422,
                    ));
                }
                patches.push((offset + bom + start, offset + bom + end, value.clone()));
            }
        }
        offset += original.len();
    }
    if replacements.keys().any(|key| !found.contains(*key)) {
        return Err(EditorError::new(
            "E-EDITOR-INI-INDEX",
            "Reference key missing from INI",
            422,
        ));
    }
    let mut result = text.to_string();
    for (start, end, value) in patches.into_iter().rev() {
        result.replace_range(start..end, &value);
    }
    let new = inspect_config(&result, config, cwd)
        .map_err(|e| EditorError::common("E-EDITOR-INI-INDEX", e))?;
    for (key, value) in &old.general {
        if !replacements.contains_key(key.as_str()) && new.general.get(key) != Some(value) {
            return Err(invalid("Non-reference INI value changed"));
        }
    }
    if old.channels != new.channels {
        return Err(invalid("Channel INI changed"));
    }
    Ok(result)
}
pub(crate) fn build_plan(
    project: &ProjectSnapshot,
    revision: u64,
    input_revision: u64,
    target_id: &str,
    registry: &TargetRegistry,
) -> Result<SavePlan> {
    let target = registry
        .data
        .targets
        .get(target_id)
        .ok_or_else(|| EditorError::new("E-EDITOR-TARGET-DENIED", "Unknown target ID", 403))?
        .clone();
    let source_target = target.kind == TargetKind::SourceProject;
    if source_target
        && (!matches!(project.origin, super::input::ProjectOrigin::Disk)
            || project.files.values().any(|f| f.stat.is_none()))
    {
        return Err(EditorError::new(
            "E-TARGET-SHAPE",
            "New project files require Save As",
            409,
        ));
    }
    let current = project
        .file_by_path(&project.config)
        .ok_or_else(|| invalid("Captured INI missing"))?;
    let actual = crate::input::inspect_config(&current.text, &project.config, &project.cwd)
        .map_err(|d| EditorError::common("E-EDITOR-VALIDATION", d))?;
    if actual.roots != project.header.roots
        || actual.workload != project.header.workload
        || actual.model_config != project.header.model_config
    {
        return Err(invalid(
            "Current INI references do not match the captured project",
        ));
    }
    if source_target && target.config != project.config {
        return Err(EditorError::new(
            "E-TARGET-SHAPE",
            "Source target belongs to a different input project",
            409,
        ));
    }
    let roots = if source_target {
        project.header.roots.clone()
    } else {
        project
            .header
            .roots
            .iter()
            .enumerate()
            .map(|(i, _)| target.path.join(format!("ned/{:04}", i + 1)))
            .collect()
    };
    let mut files = BTreeMap::<PathBuf, (Vec<u8>, u8)>::new();
    let mut directories = BTreeSet::new();
    let mut workload = None;
    let mut model_config = None;
    for original_dir in project.directories.keys() {
        for (i, root) in project.header.roots.iter().enumerate() {
            if let Ok(relative) = original_dir.strip_prefix(root) {
                directories.insert(roots[i].join(relative));
            }
        }
    }
    if !source_target {
        directories.insert(target.path.clone());
    }
    for file in project.files.values() {
        for role in &file.roles {
            let (path, rank) = match role {
                FileRole::Config => continue,
                FileRole::Ned {
                    root_index,
                    relative,
                    ..
                } => (
                    if source_target {
                        file.path.clone()
                    } else {
                        roots[*root_index].join(relative)
                    },
                    0,
                ),
                FileRole::Workload => {
                    let path = if source_target {
                        file.path.clone()
                    } else {
                        target.path.join("data/workload.json")
                    };
                    workload = Some(path.clone());
                    (path, 1)
                }
                FileRole::ModelConfig => {
                    let path = if source_target {
                        file.path.clone()
                    } else {
                        target.path.join("data/model-config.json")
                    };
                    model_config = Some(path.clone());
                    (path, 1)
                }
            };
            if let Some((existing, _)) = files.get(&path) {
                if existing.as_slice() != file.text.as_bytes() {
                    return Err(invalid("Output role collision"));
                }
            }
            files.insert(path.clone(), (file.text.as_bytes().to_vec(), rank));
            directories.insert(path.parent().unwrap().to_path_buf());
        }
    }
    let mut requires_layout_confirmation = false;
    for file in project.ned_files() {
        let layout = &project.layouts[&file.id];
        let path = if source_target {
            layout.path.clone()
        } else {
            let (i, relative) = file
                .roles
                .iter()
                .find_map(|role| {
                    if let FileRole::Ned {
                        root_index,
                        relative,
                        ..
                    } = role
                    {
                        Some((*root_index, relative))
                    } else {
                        None
                    }
                })
                .unwrap();
            let ned = roots[i].join(relative);
            ned.with_file_name(format!(
                "{}.layout.json",
                ned.file_name().unwrap().to_str().unwrap()
            ))
        };
        let empty;
        let document = if let Some(document) = &layout.adopted {
            Some(document)
        } else if matches!(target.baseline.get(&path), Some(Before::Present { .. }))
            && !target.protected_layouts.contains(&path)
        {
            // Undo back to automatic placement must clear saved manual coordinates.
            empty = super::layout::LayoutDocument::empty(
                file.path.file_name().unwrap().to_str().unwrap().into(),
            );
            Some(&empty)
        } else {
            None
        };
        if let Some(document) = document {
            let bytes = if source_target
                && layout.adopted.is_some()
                && layout.raw.as_ref().is_some_and(|raw| {
                    super::layout::LayoutDocument::parse(raw).is_ok_and(|old| &old == document)
                }) {
                layout.raw.as_ref().unwrap().as_bytes().to_vec()
            } else {
                document.bytes()?
            };
            if target.protected_layouts.contains(&path) {
                requires_layout_confirmation = true;
            }
            super::layout::LayoutDocument::parse(
                std::str::from_utf8(&bytes).map_err(|e| invalid(e.to_string()))?,
            )?;
            files.insert(path, (bytes, 2));
        }
    }
    let original_ini = project
        .file_by_path(&project.config)
        .ok_or_else(|| invalid("Captured INI missing"))?;
    let ini = if source_target {
        original_ini.text.to_string()
    } else {
        let relative_roots: Vec<_> = roots
            .iter()
            .map(|p| p.strip_prefix(&target.path).unwrap().to_path_buf())
            .collect();
        patch_ini(
            &original_ini.text,
            &project.config,
            &project.cwd,
            &relative_roots,
            workload
                .as_deref()
                .map(|p| p.strip_prefix(&target.path).unwrap()),
            model_config
                .as_deref()
                .map(|p| p.strip_prefix(&target.path).unwrap()),
        )?
    };
    files.insert(target.config.clone(), (ini.into_bytes(), 3));
    directories.insert(target.config.parent().unwrap().to_path_buf());
    for root in &roots {
        directories.insert(root.clone());
    }
    let planned_paths: BTreeSet<_> = files.keys().cloned().collect();
    if source_target
        && planned_paths
            .iter()
            .any(|p| !target.allowed_files.contains(p))
    {
        return Err(EditorError::new(
            "E-TARGET-SHAPE",
            "Source target path set changed; export a new project",
            409,
        ));
    }
    if target.kind == TargetKind::ManagedExport
        && planned_paths
            .difference(&target.optional_layouts)
            .cloned()
            .collect::<BTreeSet<_>>()
            != target
                .allowed_files
                .difference(&target.optional_layouts)
                .cloned()
                .collect::<BTreeSet<_>>()
    {
        return Err(EditorError::new(
            "E-TARGET-SHAPE",
            "Managed project file set changed; export a new project",
            409,
        ));
    }
    let files: Vec<_> = files
        .into_iter()
        .map(|(path, (bytes, rank))| PlannedFile {
            before: target
                .baseline
                .get(&path)
                .cloned()
                .unwrap_or(Before::Absent),
            hash: hash(&bytes),
            path,
            bytes,
            rank,
        })
        .collect();
    let directories: Vec<_> = directories.into_iter().collect();
    let virtual_files = files
        .iter()
        .map(|f| {
            Ok((
                f.path.clone(),
                Arc::<str>::from(
                    std::str::from_utf8(&f.bytes).map_err(|e| invalid(e.to_string()))?,
                ),
            ))
        })
        .collect::<Result<_>>()?;
    let source = SnapshotInputSource::virtual_project(virtual_files, &directories);
    let baseline_digest = baseline_digest(&target);
    let digest = hash(
        &serde_json::to_vec(&(
            revision,
            input_revision,
            &project.id,
            target_id,
            &baseline_digest,
            &files,
            &directories,
            &target.config,
            &target.cwd,
            requires_layout_confirmation,
        ))
        .map_err(|e| invalid(e.to_string()))?,
    );
    Ok(SavePlan {
        id: random_id("save-")?,
        revision,
        input_revision,
        snapshot_id: project.id.clone(),
        digest,
        target_id: target_id.into(),
        output_path: target.path.clone(),
        requires_layout_confirmation,
        target,
        files,
        directories,
        source,
        baseline_digest,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SaveOutcome {
    Complete,
    Rejected,
    RecoveryRequired,
    Restored,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct FileSaveResult {
    pub path: PathBuf,
    pub state: String,
    pub hash: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SaveReport {
    pub save_id: String,
    pub revision: u64,
    pub input_revision: u64,
    pub snapshot_id: String,
    pub plan_digest: String,
    pub state: SaveOutcome,
    pub error: Option<EditorError>,
    pub files: Vec<FileSaveResult>,
    pub recovery_id: Option<String>,
    pub output_path: PathBuf,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalFile {
    path: PathBuf,
    before: Before,
    before_blob: Option<String>,
    after_blob: String,
    after_hash: String,
    after_size: u64,
    actual_mode: Option<u32>,
    temp_name: String,
    temp_attempt: u64,
    orphan_temps: Vec<String>,
    temp_identity: Option<Identity>,
    rank: u8,
    progress: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalDirectory {
    path: PathBuf,
    before: Option<Identity>,
    created: Option<Identity>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    id: String,
    revision: u64,
    input_revision: u64,
    snapshot_id: String,
    plan_digest: String,
    target: Target,
    registration_digest: String,
    state: String,
    action: Option<String>,
    files: Vec<JournalFile>,
    directories: Vec<JournalDirectory>,
    completed_target: Option<Target>,
}
fn manifest_path(registry: &TargetRegistry, id: &str) -> PathBuf {
    registry
        .state_root
        .join("recovery")
        .join(id)
        .join("manifest.json")
}
fn save_manifest(registry: &TargetRegistry, m: &Manifest) -> Result<()> {
    atomic_state(
        &manifest_path(registry, &m.id),
        &serde_json::to_vec_pretty(m).map_err(|e| invalid(e.to_string()))?,
    )
}
impl TargetRegistry {
    fn load_manifest(&self, id: &str) -> Result<Manifest> {
        if !safe_id(id, "save-") {
            return Err(invalid("Invalid recovery ID"));
        }
        let path = manifest_path(self, id);
        let (bytes, _, _) = read_regular(&path, false)?;
        let value = super::controller::strict_json(&bytes).map_err(|e| invalid(e.message))?;
        let m: Manifest = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
        if m.schema_version != 1
            || m.id != id
            || ![
                "prepared",
                "publishing",
                "recovery_required",
                "completed",
                "restored",
            ]
            .contains(&m.state.as_str())
            || m.action
                .as_deref()
                .is_some_and(|a| a != "complete" && a != "restore")
        {
            return Err(invalid("Invalid recovery schema/state"));
        }
        if baseline_digest(&m.target) != m.registration_digest {
            return Err(invalid("Recovery target digest mismatch"));
        }
        let registered = self
            .data
            .targets
            .get(&m.target.id)
            .ok_or_else(|| invalid("Recovery target has no independent registration"))?;
        if registered.path != m.target.path
            || registered.kind != m.target.kind
                && !(registered.kind == TargetKind::ManagedExport
                    && m.target.kind == TargetKind::NewProject)
            || registered.allowed_files != m.target.allowed_files
            || registered.roots != m.target.roots
            || registered.directories != m.target.directories
        {
            return Err(invalid("Recovery target authorization mismatch"));
        }
        if registered.generation == m.target.generation
            && baseline_digest(registered) != m.registration_digest
        {
            return Err(invalid("Recovery registration digest mismatch"));
        }
        let mut seen = BTreeSet::new();
        for (i, file) in m.files.iter().enumerate() {
            if !registered.allowed_files.contains(&file.path)
                || !seen.insert(file.path.clone())
                || file.after_blob != format!("after/{i:04}.bin")
                || file
                    .before_blob
                    .as_ref()
                    .is_some_and(|p| p != &format!("before/{i:04}.bin"))
                || file.temp_attempt > 256
                || file.temp_name != temporary_name(&m.id, i, file.temp_attempt)
                || file.orphan_temps.iter().any(|name| {
                    !(0..file.temp_attempt)
                        .any(|attempt| name == &temporary_name(&m.id, i, attempt))
                })
                || file.actual_mode.is_some_and(|mode| mode > 0o7777)
            {
                return Err(invalid("Invalid recovery file mapping"));
            }
            if matches!(file.before, Before::Present { .. }) != file.before_blob.is_some() {
                return Err(invalid("Invalid before backup"));
            }
            let after = self.blob(&m, &file.after_blob)?;
            if after.len() as u64 != file.after_size || hash(&after) != file.after_hash {
                return Err(invalid("After backup hash/size mismatch"));
            }
            if let (
                Before::Present {
                    hash: expected,
                    mode,
                },
                Some(blob),
            ) = (&file.before, &file.before_blob)
            {
                if *mode > 0o7777 || hash(&self.blob(&m, blob)?) != *expected {
                    return Err(invalid("Before backup hash/mode mismatch"));
                }
            }
        }
        for dir in &m.directories {
            if !registered.directories.contains(&dir.path) {
                return Err(invalid("Unregistered recovery directory"));
            }
        }
        if let Some(completed) = &m.completed_target {
            if completed.id != registered.id
                || completed.path != registered.path
                || completed.allowed_files != registered.allowed_files
                || completed.generation != m.target.generation + 1
            {
                return Err(invalid("Invalid completion checkpoint"));
            }
        }
        Ok(m)
    }
    fn blob(&self, m: &Manifest, name: &str) -> Result<Vec<u8>> {
        if !name.starts_with("before/") && !name.starts_with("after/")
            || name.components_count() != 2
        {
            return Err(invalid("Invalid backup name"));
        }
        read_regular(
            &self.state_root.join("recovery").join(&m.id).join(name),
            false,
        )
        .map(|(bytes, _, _)| bytes)
    }
    fn reconcile_completed(&mut self) -> Result<()> {
        for name in Directory::open(&self.state_root.join("recovery"))?.names()? {
            let id = name.to_string_lossy();
            if !safe_id(&id, "save-") {
                continue;
            }
            let path = manifest_path(self, &id);
            // A crash before durable initial manifest leaves an unpublished orphan directory.
            if stat_optional(&path)?.is_none() {
                continue;
            }
            let m = self.load_manifest(&id)?;
            if m.state == "completed" {
                let completed = m
                    .completed_target
                    .ok_or_else(|| invalid("Completed journal missing baseline"))?;
                if self.data.targets[&completed.id].generation < completed.generation {
                    self.data.targets.insert(completed.id.clone(), completed);
                    self.persist()?;
                }
                // Retire the terminal manifest only after its baseline is durable.
                // Old copied blobs remain private orphans; no target path is cleaned here.
                let path = manifest_path(self, &id);
                let directory = parent(&path)?;
                directory.verify()?;
                rfs::unlinkat(directory.fd(), path.file_name().unwrap(), AtFlags::empty())
                    .map_err(|e| EditorError::io(&path, e))?;
                directory.sync()?;
            } else if m.state == "restored" {
                let path = manifest_path(self, &id);
                let directory = parent(&path)?;
                directory.verify()?;
                rfs::unlinkat(directory.fd(), path.file_name().unwrap(), AtFlags::empty())
                    .map_err(|e| EditorError::io(&path, e))?;
                directory.sync()?;
            }
        }
        Ok(())
    }
}
trait BackupName {
    fn components_count(&self) -> usize;
}
impl BackupName for str {
    fn components_count(&self) -> usize {
        Path::new(self).components().count()
    }
}
fn observe(path: &Path, own_link: bool) -> Result<Before> {
    match stat_optional(path)? {
        None => Ok(Before::Absent),
        Some(stat) => {
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(conflict(format!(
                    "Target is not a regular file: {}",
                    path.display()
                )));
            }
            let (bytes, mode, _) = read_regular(path, own_link)?;
            Ok(Before::Present {
                hash: hash(&bytes),
                mode,
            })
        }
    }
}
fn check_before(path: &Path, expected: &Before) -> Result<()> {
    if observe(path, false)? != *expected {
        return Err(conflict(format!(
            "Target baseline changed: {}",
            path.display()
        )));
    }
    Ok(())
}
fn scan_ned(
    root: &Path,
    ned: &mut BTreeSet<PathBuf>,
    directories: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let directory = Directory::open(root)?;
    directories.insert(root.into());
    for name in directory.names()? {
        let path = root.join(&name);
        let stat = rfs::statat(directory.fd(), &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| EditorError::io(&path, e))?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Symlink => {
                return Err(conflict(format!(
                    "Symlink appeared under output NED root: {}",
                    path.display()
                )));
            }
            FileType::Directory => scan_ned(&path, ned, directories)?,
            FileType::RegularFile if path.extension().is_some_and(|e| e == "ned") => {
                ned.insert(path);
            }
            _ => {}
        }
    }
    directory.verify()
}
fn check_shape(target: &Target, manifest: Option<&Manifest>) -> Result<()> {
    if parent(&target.path)?.identity() != target.parent_identity {
        return Err(conflict("Target parent identity changed"));
    }
    if target.kind == TargetKind::NewProject && manifest.is_none() {
        match (&target.root_identity, stat_optional(&target.path)?) {
            (None, None) => return Ok(()),
            (Some(expected), Some(stat))
                if FileType::from_raw_mode(stat.st_mode) == FileType::Directory
                    && Identity::stat(&stat) == *expected
                    && Directory::open(&target.path)?.names()?.is_empty() =>
            {
                return Ok(());
            }
            _ => return Err(conflict("New output is no longer absent or empty")),
        }
    }
    if let Some(expected) = &target.root_identity {
        if Directory::open(&target.path)?.identity() != *expected {
            return Err(conflict("Target root identity changed"));
        }
    }
    if target.kind == TargetKind::NewProject {
        // Only plan-owned entries may exist in a partially published new project.
        if stat_optional(&target.path)?.is_none() {
            return Ok(());
        }
        let manifest = manifest.ok_or_else(|| invalid("Missing new project recovery manifest"))?;
        let permitted_files: BTreeSet<_> = target
            .allowed_files
            .iter()
            .cloned()
            .chain(manifest.files.iter().flat_map(|file| {
                std::iter::once(file.path.with_file_name(&file.temp_name)).chain(
                    file.orphan_temps
                        .iter()
                        .map(|name| file.path.with_file_name(name)),
                )
            }))
            .collect();
        check_new_tree(&target.path, &permitted_files, &target.directories)?;
        return Ok(());
    }
    let mut actual = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for root in &target.roots {
        scan_ned(root, &mut actual, &mut directories)?;
    }
    let expected_dirs: BTreeSet<_> = target
        .directories
        .iter()
        .filter(|p| target.roots.iter().any(|root| p.starts_with(root)))
        .cloned()
        .collect();
    if actual != target.ned || directories != expected_dirs {
        return Err(conflict("NED file/directory set changed"));
    }
    Ok(())
}
fn check_new_tree(
    path: &Path,
    files: &BTreeSet<PathBuf>,
    directories: &BTreeSet<PathBuf>,
) -> Result<()> {
    let dir = Directory::open(path)?;
    for name in dir.names()? {
        let child = path.join(&name);
        let stat = rfs::statat(dir.fd(), &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| EditorError::io(&child, e))?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory if directories.contains(&child) => {
                check_new_tree(&child, files, directories)?
            }
            FileType::RegularFile if files.contains(&child) => {}
            _ => {
                return Err(conflict(format!(
                    "Unknown entry in new project: {}",
                    child.display()
                )));
            }
        }
    }
    dir.verify()
}
fn lock_targets(
    registry: &TargetRegistry,
    target: &Target,
    files: &[PlannedFile],
) -> Result<Vec<File>> {
    let mut keys = BTreeSet::from([format!("project:{}", target.path.display())]);
    keys.extend(
        target
            .baseline
            .keys()
            .map(|p| format!("path:{}", p.display())),
    );
    keys.extend(files.iter().map(|f| format!("path:{}", f.path.display())));
    // Directory inode locks also coordinate servers using different state roots.
    // They are held only during output and never create an input-side lock file.
    let mut directory_paths = BTreeSet::from([target.path.parent().unwrap().to_path_buf()]);
    directory_paths.extend(
        target
            .baseline
            .keys()
            .filter_map(|p| p.parent().map(Path::to_path_buf)),
    );
    directory_paths.extend(
        files
            .iter()
            .filter_map(|f| f.path.parent().map(Path::to_path_buf)),
    );
    let mut directory_locks = BTreeMap::new();
    for path in directory_paths {
        if stat_optional(&path)?.is_none() {
            continue;
        }
        let directory = Directory::open(&path)?;
        let descriptor = directory
            .fd()
            .try_clone()
            .map_err(|e| EditorError::io(&path, e))?;
        directory_locks
            .entry(directory.identity())
            .or_insert(descriptor);
    }
    let mut held = Vec::new();
    for (_, descriptor) in directory_locks {
        rfs::flock(&descriptor, FlockOperation::NonBlockingLockExclusive)
            .map_err(|e| EditorError::new("E-EDITOR-TARGET-BUSY", e.to_string(), 409))?;
        held.push(descriptor);
    }
    let locks = Directory::open(&registry.state_root.join("locks"))?;
    for key in keys {
        let name = format!("{}.lock", hash(key.as_bytes()));
        let file: File = rfs::openat(
            locks.fd(),
            name.as_str(),
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|e| EditorError::io(&registry.state_root, e))?
        .into();
        let stat = rfs::fstat(&file).map_err(|e| EditorError::io(&registry.state_root, e))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
            return Err(invalid("Invalid lock inode"));
        }
        rfs::flock(&file, FlockOperation::NonBlockingLockExclusive)
            .map_err(|e| EditorError::new("E-EDITOR-TARGET-BUSY", e.to_string(), 409))?;
        held.push(file);
    }
    locks.verify()?;
    Ok(held)
}
fn full_plan_directories(plan: &SavePlan) -> Vec<PathBuf> {
    let mut directories: BTreeSet<_> = plan.directories.iter().cloned().collect();
    if plan.target.kind != TargetKind::SourceProject {
        for path in plan
            .directories
            .iter()
            .chain(plan.files.iter().map(|f| &f.path))
        {
            for parent in path
                .ancestors()
                .skip(usize::from(plan.files.iter().any(|f| &f.path == path)))
            {
                if parent.starts_with(&plan.output_path) {
                    directories.insert(parent.into());
                }
            }
        }
    }
    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|p| (p.components().count(), p.clone()));
    directories
}
fn initial_manifest(plan: &SavePlan, registry: &mut TargetRegistry) -> Result<Manifest> {
    let mut target = plan.target.clone();
    let directories = full_plan_directories(plan);
    if target.kind == TargetKind::NewProject {
        target.allowed_files = plan.files.iter().map(|f| f.path.clone()).collect();
        target.optional_layouts = plan
            .files
            .iter()
            .filter(|file| file.rank == 0)
            .map(|file| {
                file.path.with_file_name(format!(
                    "{}.layout.json",
                    file.path.file_name().unwrap().to_str().unwrap()
                ))
            })
            .collect();
        target
            .allowed_files
            .extend(target.optional_layouts.iter().cloned());
        for path in &target.optional_layouts {
            target
                .baseline
                .entry(path.clone())
                .or_insert(Before::Absent);
        }
        target.roots = plan
            .directories
            .iter()
            .filter(|p| p.parent() == Some(plan.output_path.join("ned").as_path()))
            .cloned()
            .collect();
        target.ned = plan
            .files
            .iter()
            .filter(|f| f.rank == 0)
            .map(|f| f.path.clone())
            .collect();
        target.directories = directories.iter().cloned().collect();
        registry
            .data
            .targets
            .insert(target.id.clone(), target.clone());
        registry.persist()?;
    }
    let root = registry.state_root.join("recovery").join(&plan.id);
    mkdir_owned(&root, 0o700)?;
    mkdir_owned(&root.join("before"), 0o700)?;
    mkdir_owned(&root.join("after"), 0o700)?;
    let mut files = Vec::new();
    for (i, file) in plan.files.iter().enumerate() {
        let before_blob = if let Before::Present { hash: expected, .. } = &file.before {
            let (bytes, _, _) = read_regular(&file.path, false)?;
            if hash(&bytes) != *expected {
                return Err(conflict("Target changed while copying backup"));
            }
            let name = format!("before/{i:04}.bin");
            create_file(&root.join(&name), &bytes, 0o600, true)?;
            Some(name)
        } else {
            None
        };
        let after_blob = format!("after/{i:04}.bin");
        create_file(&root.join(&after_blob), &file.bytes, 0o600, true)?;
        files.push(JournalFile {
            path: file.path.clone(),
            before: file.before.clone(),
            before_blob,
            after_blob,
            after_hash: file.hash.clone(),
            after_size: file.bytes.len() as u64,
            actual_mode: match file.before {
                Before::Present { mode, .. } => Some(mode),
                Before::Absent => None,
            },
            temp_name: temporary_name(&plan.id, i, 0),
            temp_attempt: 0,
            orphan_temps: Vec::new(),
            temp_identity: None,
            rank: file.rank,
            progress: "not_attempted".into(),
        });
    }
    let mut dirs = Vec::new();
    for path in directories {
        let before = match stat_optional(&path)? {
            Some(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
                Some(Identity::stat(&stat))
            }
            Some(_) => return Err(conflict("Planned directory is not a directory")),
            None => None,
        };
        dirs.push(JournalDirectory {
            path,
            before,
            created: None,
        });
    }
    let m = Manifest {
        schema_version: 1,
        id: plan.id.clone(),
        revision: plan.revision,
        input_revision: plan.input_revision,
        snapshot_id: plan.snapshot_id.clone(),
        plan_digest: plan.digest.clone(),
        registration_digest: baseline_digest(&target),
        target,
        state: "prepared".into(),
        action: None,
        files,
        directories: dirs,
        completed_target: None,
    };
    save_manifest(registry, &m)?;
    checkpoint("journal_prepared")?;
    Ok(m)
}
fn report_plan(plan: &SavePlan, error: EditorError) -> SaveReport {
    SaveReport {
        save_id: plan.id.clone(),
        revision: plan.revision,
        input_revision: plan.input_revision,
        snapshot_id: plan.snapshot_id.clone(),
        plan_digest: plan.digest.clone(),
        state: SaveOutcome::Rejected,
        error: Some(error),
        files: plan
            .files
            .iter()
            .map(|f| FileSaveResult {
                path: f.path.clone(),
                state: "not_attempted".into(),
                hash: None,
            })
            .collect(),
        recovery_id: None,
        output_path: plan.output_path.clone(),
    }
}
fn report_manifest(m: &Manifest, state: SaveOutcome, error: Option<EditorError>) -> SaveReport {
    let mut files: Vec<FileSaveResult> = m
        .files
        .iter()
        .map(|file| FileSaveResult {
            path: file.path.clone(),
            state: if state == SaveOutcome::Restored {
                "restored".into()
            } else {
                file.progress.clone()
            },
            hash: match observe(&file.path, false) {
                Ok(Before::Present { hash, .. }) => Some(hash),
                _ => None,
            },
        })
        .collect();
    for file in &m.files {
        for name in &file.orphan_temps {
            let path = file.path.with_file_name(name);
            if stat_optional(&path).ok().flatten().is_some() {
                files.push(FileSaveResult {
                    path,
                    state: "preserved_temporary".into(),
                    hash: None,
                });
            }
        }
        let path = file.path.with_file_name(&file.temp_name);
        if stat_optional(&path)
            .ok()
            .flatten()
            .is_some_and(|stat| file.temp_identity.as_ref() != Some(&Identity::stat(&stat)))
        {
            files.push(FileSaveResult {
                path,
                state: "preserved_temporary".into(),
                hash: None,
            });
        }
    }
    SaveReport {
        save_id: m.id.clone(),
        revision: m.revision,
        input_revision: m.input_revision,
        snapshot_id: m.snapshot_id.clone(),
        plan_digest: m.plan_digest.clone(),
        recovery_id: if state == SaveOutcome::RecoveryRequired {
            Some(m.id.clone())
        } else {
            None
        },
        state,
        error,
        files,
        output_path: m.target.path.clone(),
    }
}
pub(crate) fn save_plan(
    plan: SavePlan,
    registry: &mut TargetRegistry,
    authorized_digest: Option<&str>,
    replace_layout_confirmed: bool,
) -> SaveReport {
    let check = (|| {
        let target = registry
            .data
            .targets
            .get(&plan.target_id)
            .ok_or_else(|| invalid("Target disappeared"))?;
        if baseline_digest(target) != plan.baseline_digest {
            return Err(conflict("Target registration changed"));
        }
        if plan.target.kind != TargetKind::NewProject
            && authorized_digest != Some(plan.digest.as_str())
        {
            return Err(EditorError::new(
                "E-EDITOR-OVERWRITE-ACK",
                "Explicit confirmation does not match this exact plan",
                409,
            ));
        }
        if plan.requires_layout_confirmation && !replace_layout_confirmed {
            return Err(EditorError::new(
                "E-EDITOR-LAYOUT-CONFIRMATION",
                "Invalid layout replacement needs explicit confirmation",
                409,
            ));
        }
        prepare_with_source(&plan.target.config, &plan.target.cwd, &plan.source)
            .map_err(|e| EditorError::common("E-EDITOR-SAVE-VALIDATION", e))?;
        check_shape(&plan.target, None)?;
        for (path, before) in &plan.target.baseline {
            check_before(path, before)?;
        }
        for file in &plan.files {
            check_before(&file.path, &file.before)?;
        }
        Ok(())
    })();
    if let Err(error) = check {
        return report_plan(&plan, error);
    }
    let _lease = match registry_lease(&registry.state_root) {
        Ok(lease) => lease,
        Err(error) => return report_plan(&plan, error),
    };
    if let Err(error) = registry.refresh() {
        return report_plan(&plan, error);
    }
    if registry
        .data
        .targets
        .get(&plan.target_id)
        .map(baseline_digest)
        .as_deref()
        != Some(plan.baseline_digest.as_str())
    {
        return report_plan(
            &plan,
            conflict("Registered baseline changed before reservation"),
        );
    }
    let held = match lock_targets(registry, &plan.target, &plan.files) {
        Ok(held) => held,
        Err(e) => return report_plan(&plan, e),
    };
    let checked = (|| {
        check_shape(&plan.target, None)?;
        for (path, before) in &plan.target.baseline {
            check_before(path, before)?;
        }
        for file in &plan.files {
            check_before(&file.path, &file.before)?;
        }
        Ok(())
    })();
    if let Err(error) = checked {
        return report_plan(&plan, error);
    }
    let mut manifest = match initial_manifest(&plan, registry) {
        Ok(m) => m,
        Err(error) => {
            if let Ok(m) = registry.load_manifest(&plan.id) {
                return report_manifest(&m, SaveOutcome::RecoveryRequired, Some(error));
            }
            return report_plan(&plan, error);
        }
    };
    let result = complete(registry, &mut manifest);
    drop(held);
    match result {
        Ok(()) => report_manifest(&manifest, SaveOutcome::Complete, None),
        Err(error) => {
            if manifest.state != "completed" {
                manifest.state = "recovery_required".into();
                let _ = save_manifest(registry, &manifest);
            }
            report_manifest(&manifest, SaveOutcome::RecoveryRequired, Some(error))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observed {
    Before,
    After,
}
fn own_temporary_link(file: &JournalFile) -> Result<bool> {
    let Some(target) = stat_optional(&file.path)? else {
        return Ok(false);
    };
    if target.st_nlink != 2 {
        return Ok(false);
    }
    let temp = file.path.with_file_name(&file.temp_name);
    let Some(stat) = stat_optional(&temp)? else {
        return Ok(false);
    };
    Ok(file
        .temp_identity
        .as_ref()
        .is_some_and(|id| id == &Identity::stat(&stat))
        && Identity::stat(&target) == Identity::stat(&stat)
        && stat.st_nlink == 2
        && FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile)
}
fn classify(file: &JournalFile) -> Result<Observed> {
    let actual = observe(&file.path, own_temporary_link(file)?)?;
    if actual == file.before {
        return Ok(Observed::Before);
    }
    if let Some(mode) = file.actual_mode {
        if actual
            == (Before::Present {
                hash: file.after_hash.clone(),
                mode,
            })
        {
            return Ok(Observed::After);
        }
    }
    Err(EditorError::new(
        "E-EDITOR-RECOVERY-CONFLICT",
        format!("Neither before nor after matches {}", file.path.display()),
        409,
    ))
}
fn check_recovery(registry: &TargetRegistry, m: &Manifest) -> Result<Vec<Observed>> {
    check_shape(&m.target, Some(m))?;
    for (path, before) in &m.target.baseline {
        if !m.files.iter().any(|f| &f.path == path) {
            check_before(path, before)?;
        }
    }
    for directory in &m.directories {
        if let Some(stat) = stat_optional(&directory.path)? {
            if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                return Err(conflict("Recovery directory changed kind"));
            }
            if let Some(expected) = directory.before.as_ref().or(directory.created.as_ref()) {
                if &Identity::stat(&stat) != expected {
                    return Err(conflict("Recovery directory changed identity"));
                }
            }
        } else if directory.before.is_some() {
            return Err(conflict("Existing recovery directory disappeared"));
        }
    }
    // Verify every backup before any target mutation, also on retries.
    for f in &m.files {
        let bytes = registry.blob(m, &f.after_blob)?;
        if bytes.len() as u64 != f.after_size || hash(&bytes) != f.after_hash {
            return Err(invalid("After blob corrupted"));
        }
        if let (Before::Present { hash: expected, .. }, Some(name)) = (&f.before, &f.before_blob) {
            if hash(&registry.blob(m, name)?) != *expected {
                return Err(invalid("Before blob corrupted"));
            }
        }
    }
    m.files.iter().map(classify).collect()
}
fn clean_temp(file: &JournalFile) -> Result<()> {
    let path = file.path.with_file_name(&file.temp_name);
    let Some(stat) = stat_optional(&path)? else {
        return Ok(());
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(conflict("Temporary path changed kind"));
    }
    let id = Identity::stat(&stat);
    if file.temp_identity.as_ref() != Some(&id) {
        return Ok(()); // Unidentified regular artifacts are preserved, never deleted.
    }
    let (bytes, mode, _) = read_regular(&path, own_temporary_link(file)?)?;
    if hash(&bytes) != file.after_hash || file.actual_mode != Some(mode) {
        return Ok(()); // Changed regular artifacts belong to neither recovery state; preserve them.
    }
    let dir = parent(&path)?;
    dir.verify()?;
    let now = rfs::statat(
        dir.fd(),
        path.file_name().unwrap(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|e| EditorError::io(&path, e))?;
    if Identity::stat(&now) != id {
        return Err(conflict("Temporary identity changed before unlink"));
    }
    rfs::unlinkat(dir.fd(), path.file_name().unwrap(), AtFlags::empty())
        .map_err(|e| EditorError::io(&path, e))?;
    dir.sync()
}
fn temporary_name(id: &str, index: usize, attempt: u64) -> String {
    if attempt == 0 {
        format!(".ned-editor-{id}-{index:04}.tmp")
    } else {
        format!(".ned-editor-{id}-{index:04}-{attempt}.tmp")
    }
}
fn stage(registry: &TargetRegistry, m: &mut Manifest, index: usize) -> Result<()> {
    loop {
        let f = m.files[index].clone();
        let path = f.path.with_file_name(&f.temp_name);
        if let Some(stat) = stat_optional(&path)? {
            if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                return Err(conflict("Staging path changed kind"));
            }
            if f.temp_identity.as_ref() == Some(&Identity::stat(&stat)) {
                let (bytes, mode, _) = read_regular(&path, own_temporary_link(&f)?)?;
                if hash(&bytes) == f.after_hash && f.actual_mode == Some(mode) {
                    return Ok(());
                }
            }
            // A create→journal crash left an inode we cannot safely claim. Keep it.
            if f.temp_attempt >= 256 {
                return Err(conflict("Too many preserved staging artifacts"));
            }
            m.files[index].orphan_temps.push(f.temp_name);
            m.files[index].temp_attempt += 1;
            m.files[index].temp_name = temporary_name(&m.id, index, m.files[index].temp_attempt);
            m.files[index].temp_identity = None;
            save_manifest(registry, m)?;
            continue;
        }
        let bytes = registry.blob(m, &f.after_blob)?;
        let (mode, set_mode) = match f.before {
            Before::Present { mode, .. } => (mode, true),
            Before::Absent => (f.actual_mode.unwrap_or(0o666), f.actual_mode.is_some()),
        };
        let (mode, id) = create_file(&path, &bytes, mode, set_mode)?;
        checkpoint("after_stage_file_sync")?;
        m.files[index].actual_mode = Some(mode);
        m.files[index].temp_identity = Some(id);
        save_manifest(registry, m)?;
        return checkpoint("staged");
    }
}

fn publish(registry: &TargetRegistry, m: &mut Manifest, index: usize) -> Result<()> {
    let f = m.files[index].clone();
    if classify(&f)? != Observed::Before {
        return Err(conflict("Target changed before publication"));
    }
    if m.target.kind == TargetKind::NewProject {
        check_shape(&m.target, Some(m))?;
    }
    let dir = parent(&f.path)?;
    dir.verify()?;
    let temp = f.path.with_file_name(&f.temp_name);
    let stat = rfs::statat(
        dir.fd(),
        temp.file_name().unwrap(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|e| EditorError::io(&temp, e))?;
    if f.temp_identity.as_ref() != Some(&Identity::stat(&stat)) || stat.st_nlink != 1 {
        return Err(conflict("Staged inode changed"));
    }
    #[cfg(test)]
    PUBLICATION_COLLISION.with(|collision| {
        if let Some(path) = collision.borrow_mut().take() {
            create_file(&path, b"foreign", 0o600, true)?;
        }
        Ok::<_, EditorError>(())
    })?;
    match f.before {
        Before::Absent => rfs::linkat(
            dir.fd(),
            temp.file_name().unwrap(),
            dir.fd(),
            f.path.file_name().unwrap(),
            AtFlags::empty(),
        )
        .map_err(|e| EditorError::io(&f.path, e))?,
        Before::Present { .. } => rfs::renameat(
            dir.fd(),
            temp.file_name().unwrap(),
            dir.fd(),
            f.path.file_name().unwrap(),
        )
        .map_err(|e| EditorError::io(&f.path, e))?,
    }
    checkpoint("after_publish")?;
    dir.sync()?;
    checkpoint("after_parent_sync")?;
    if matches!(f.before, Before::Absent) {
        clean_temp(&f)?;
    }
    m.files[index].progress = "saved".into();
    save_manifest(registry, m)?;
    checkpoint("after_progress")
}
fn complete(registry: &mut TargetRegistry, m: &mut Manifest) -> Result<()> {
    let observed = check_recovery(registry, m)?;
    // Validate immutable after bytes at their final paths, including on restart.
    let files = m
        .files
        .iter()
        .map(|f| {
            let bytes = registry.blob(m, &f.after_blob)?;
            let text = String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
            Ok((f.path.clone(), Arc::<str>::from(text)))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let directories: Vec<_> = m.directories.iter().map(|d| d.path.clone()).collect();
    let source = SnapshotInputSource::virtual_project(files, &directories);
    prepare_with_source(&m.target.config, &m.target.cwd, &source)
        .map_err(|e| EditorError::common("E-EDITOR-SAVE-VALIDATION", e))?;
    for i in 0..m.directories.len() {
        if stat_optional(&m.directories[i].path)?.is_none() {
            if m.directories[i].before.is_some() {
                return Err(conflict("Existing directory disappeared"));
            }
            let identity = mkdir_owned(&m.directories[i].path, 0o777)?;
            checkpoint("after_mkdir")?;
            m.directories[i].created = Some(identity);
            save_manifest(registry, m)?;
        }
    }
    m.state = "publishing".into();
    save_manifest(registry, m)?;
    for (i, state) in observed.iter().enumerate() {
        if *state == Observed::Before {
            let f = &m.files[i];
            let unchanged = matches!(&f.before,Before::Present{hash,mode} if hash==&f.after_hash && Some(*mode)==f.actual_mode);
            if unchanged {
                m.files[i].progress = "unchanged".into();
            } else {
                stage(registry, m, i)?;
            }
        }
    }
    let mut order: Vec<_> = (0..m.files.len()).collect();
    order.sort_by_key(|i| (m.files[*i].rank, m.files[*i].path.clone()));
    for i in order {
        match classify(&m.files[i])? {
            Observed::After => {
                clean_temp(&m.files[i])?;
                parent(&m.files[i].path)?.sync()?;
                m.files[i].progress = "saved".into();
                save_manifest(registry, m)?;
            }
            Observed::Before if m.files[i].progress == "unchanged" => {}
            Observed::Before => publish(registry, m, i)?,
        }
    }
    check_recovery(registry, m)?;
    let mut completed = m.target.clone();
    for f in &m.files {
        let actual = observe(&f.path, false)?;
        if actual
            != (Before::Present {
                hash: f.after_hash.clone(),
                mode: f.actual_mode.ok_or_else(|| invalid("Mode not finalized"))?,
            })
        {
            return Err(conflict("Final output verification failed"));
        }
        completed.baseline.insert(f.path.clone(), actual);
        completed.protected_layouts.remove(&f.path);
    }
    completed.generation += 1;
    if completed.kind == TargetKind::NewProject {
        completed.kind = TargetKind::ManagedExport;
        completed.root_identity = Some(Directory::open(&completed.path)?.identity());
    }
    m.completed_target = Some(completed.clone());
    m.state = "completed".into();
    save_manifest(registry, m)?;
    checkpoint("after_completed_manifest")?;
    registry
        .data
        .targets
        .insert(completed.id.clone(), completed);
    registry.persist()?;
    checkpoint("after_registry")
}
fn restore_before(registry: &mut TargetRegistry, m: &mut Manifest) -> Result<()> {
    check_recovery(registry, m)?;
    let mut order: Vec<_> = (0..m.files.len()).collect();
    order.sort_by_key(|i| {
        (
            std::cmp::Reverse(m.files[*i].rank),
            std::cmp::Reverse(m.files[*i].path.clone()),
        )
    });
    for i in order {
        let f = m.files[i].clone();
        if classify(&f)? == Observed::After {
            match &f.before {
                Before::Absent => {
                    let dir = parent(&f.path)?;
                    dir.verify()?;
                    // Hash/mode classification was just repeated; CAS against noncooperative writers is not claimed.
                    rfs::unlinkat(dir.fd(), f.path.file_name().unwrap(), AtFlags::empty())
                        .map_err(|e| EditorError::io(&f.path, e))?;
                    dir.sync()?;
                }
                Before::Present { mode, .. } => {
                    let bytes = registry.blob(
                        m,
                        f.before_blob
                            .as_ref()
                            .ok_or_else(|| invalid("Before blob missing"))?,
                    )?;
                    let temp = f
                        .path
                        .with_file_name(format!(".ned-editor-restore-{}-{i:04}.tmp", m.id));
                    // Restore artifacts are full before bytes; an existing matching artifact can be reused after a crash.
                    if stat_optional(&temp)?.is_none() {
                        create_file(&temp, &bytes, *mode, true)?;
                    } else {
                        let (old, oldmode, _) = read_regular(&temp, false)?;
                        if old != bytes || oldmode != *mode {
                            return Err(conflict("Unknown restore temporary file; preserved"));
                        }
                    }
                    if classify(&f)? != Observed::After {
                        return Err(conflict("Target changed before restore"));
                    }
                    let dir = parent(&f.path)?;
                    dir.verify()?;
                    rfs::renameat(
                        dir.fd(),
                        temp.file_name().unwrap(),
                        dir.fd(),
                        f.path.file_name().unwrap(),
                    )
                    .map_err(|e| EditorError::io(&f.path, e))?;
                    dir.sync()?;
                }
            }
        }
        // Own pending stage is removable only with recorded identity and expected after bytes.
        clean_temp(&f)?;
        m.files[i].progress = "restored".into();
        save_manifest(registry, m)?;
        checkpoint("after_restore")?;
    }
    let mut dirs: Vec<_> = m
        .directories
        .iter()
        .filter(|d| d.before.is_none() && d.created.is_some())
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.path.components().count()));
    for d in dirs {
        if let Some(stat) = stat_optional(&d.path)? {
            if Some(Identity::stat(&stat)) != d.created {
                return Err(conflict("Created directory identity changed"));
            }
            let directory = Directory::open(&d.path)?;
            if directory.names()?.is_empty() {
                let dir = parent(&d.path)?;
                dir.verify()?;
                rfs::unlinkat(dir.fd(), d.path.file_name().unwrap(), AtFlags::REMOVEDIR)
                    .map_err(|e| EditorError::io(&d.path, e))?;
                dir.sync()?;
            }
        }
    }
    m.state = "restored".into();
    save_manifest(registry, m)
}
pub(crate) fn recover(
    registry: &mut TargetRegistry,
    recovery_id: &str,
    action: &str,
) -> SaveReport {
    let empty = |e: EditorError| SaveReport {
        save_id: recovery_id.into(),
        revision: 0,
        input_revision: 0,
        snapshot_id: String::new(),
        plan_digest: String::new(),
        state: SaveOutcome::Rejected,
        error: Some(e),
        files: Vec::new(),
        recovery_id: Some(recovery_id.into()),
        output_path: PathBuf::new(),
    };
    if action != "complete" && action != "restore" {
        return empty(invalid("Recovery action must be complete or restore"));
    }
    let _lease = match registry_lease(&registry.state_root) {
        Ok(lease) => lease,
        Err(error) => return empty(error),
    };
    if let Err(error) = registry.refresh() {
        return empty(error);
    }
    let mut m = match registry.load_manifest(recovery_id) {
        Ok(m) => m,
        Err(e) => return empty(e),
    };
    if m.state == "completed" {
        if let Some(target) = m.completed_target.clone() {
            if registry
                .data
                .targets
                .get(&target.id)
                .is_some_and(|registered| registered.generation < target.generation)
            {
                registry.data.targets.insert(target.id.clone(), target);
                if let Err(e) = registry.persist() {
                    return report_manifest(&m, SaveOutcome::RecoveryRequired, Some(e));
                }
            }
        }
        return report_manifest(&m, SaveOutcome::Complete, None);
    }
    if m.state == "restored" {
        return report_manifest(&m, SaveOutcome::Restored, None);
    }
    if m.action.as_deref().is_some_and(|chosen| chosen != action) {
        return report_manifest(
            &m,
            SaveOutcome::RecoveryRequired,
            Some(conflict("Recovery action is already bound; cannot flip")),
        );
    }
    let planned = m
        .files
        .iter()
        .map(|f| PlannedFile {
            path: f.path.clone(),
            bytes: Vec::new(),
            hash: f.after_hash.clone(),
            before: f.before.clone(),
            rank: f.rank,
        })
        .collect::<Vec<_>>();
    let held = match lock_targets(registry, &m.target, &planned) {
        Ok(v) => v,
        Err(e) => return report_manifest(&m, SaveOutcome::RecoveryRequired, Some(e)),
    };
    let result = (|| {
        check_recovery(registry, &m)?;
        m.action = Some(action.into());
        save_manifest(registry, &m)?;
        if action == "complete" {
            complete(registry, &mut m)
        } else {
            restore_before(registry, &mut m)
        }
    })();
    drop(held);
    match result {
        Ok(()) => report_manifest(
            &m,
            if action == "complete" {
                SaveOutcome::Complete
            } else {
                SaveOutcome::Restored
            },
            None,
        ),
        Err(e) => {
            if m.state != "completed" {
                m.state = "recovery_required".into();
                let _ = save_manifest(registry, &m);
            }
            report_manifest(&m, SaveOutcome::RecoveryRequired, Some(e))
        }
    }
}
#[cfg(test)]
thread_local! { static FAILURE:std::cell::RefCell<Option<(String,usize)>>=const {std::cell::RefCell::new(None)}; }
#[cfg(test)]
thread_local! { static PUBLICATION_COLLISION:std::cell::RefCell<Option<PathBuf>>=const {std::cell::RefCell::new(None)}; }
fn checkpoint(_name: &str) -> Result<()> {
    #[cfg(test)]
    {
        FAILURE.with(|state| {
            let mut state = state.borrow_mut();
            if let Some((name, count)) = state.as_mut() {
                if name == _name {
                    if *count == 0 {
                        *state = None;
                        return Err(EditorError::new("E-EDITOR-FAILURE-INJECTED", _name, 500));
                    }
                    *count -= 1;
                }
            }
            Ok(())
        })
    }
    #[cfg(not(test))]
    Ok(())
}
#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
