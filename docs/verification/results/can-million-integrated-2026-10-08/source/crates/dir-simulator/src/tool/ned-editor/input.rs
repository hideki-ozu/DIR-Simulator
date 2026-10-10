use super::layout::LayoutDocument;
use super::{EditorError, Result, absolute, hash, random_id};
use crate::input::{
    InputDirEntry, InputKind, InputMetadata, InputSource, ParsedNed, ProjectHeader, inspect_config,
    parse_ned,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum FileRole {
    Config,
    Ned {
        root_index: usize,
        relative: PathBuf,
        package: String,
    },
    Workload,
    ModelConfig,
}
#[derive(Clone, Debug)]
pub(crate) struct CapturedFile {
    pub id: String,
    pub path: PathBuf,
    pub roles: Vec<FileRole>,
    pub text: Arc<str>,
    pub hash: String,
    pub origin_text: Arc<str>,
    pub origin_hash: String,
    pub stat: Option<CapturedStat>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CapturedStat {
    pub mode: u32,
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
}
impl CapturedStat {
    pub fn from_metadata(m: &fs::Metadata) -> Self {
        Self {
            mode: m.mode() & 0o7777,
            dev: m.dev(),
            ino: m.ino(),
            nlink: m.nlink(),
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct LayoutCapture {
    pub path: PathBuf,
    pub raw: Option<Arc<str>>,
    pub hash: Option<String>,
    pub stat: Option<CapturedStat>,
    pub adopted: Option<LayoutDocument>,
    pub warning: Option<String>,
}
#[derive(Clone, Debug)]
pub(crate) enum ProjectOrigin {
    Disk,
    New { template: String, name: String },
}
#[derive(Clone, Debug)]
pub(crate) struct ProjectSnapshot {
    pub origin: ProjectOrigin,
    pub id: String,
    pub config: PathBuf,
    pub cwd: PathBuf,
    pub header: ProjectHeader,
    pub files: BTreeMap<String, CapturedFile>,
    pub directories: BTreeMap<PathBuf, Vec<(OsString, InputKind)>>,
    pub metadata: BTreeMap<PathBuf, InputKind>,
    pub layouts: BTreeMap<String, LayoutCapture>,
    pub parsed: BTreeMap<String, ParsedNed>,
}
impl ProjectSnapshot {
    pub fn refresh_inputs(&mut self) {
        let directories: Vec<_> = self
            .directories
            .keys()
            .cloned()
            .chain(self.header.roots.iter().cloned())
            .collect();
        let source = SnapshotInputSource::virtual_project(
            self.files
                .values()
                .map(|f| (f.path.clone(), f.text.clone()))
                .collect(),
            &directories,
        );
        self.metadata.extend(source.metadata);
        for (path, entries) in source.directories {
            let captured = self.directories.entry(path).or_default();
            for entry in entries {
                if !captured.iter().any(|(name, _)| name == &entry.0) {
                    captured.push(entry);
                }
            }
            captured.sort_by(|a, b| a.0.cmp(&b.0));
        }
        if let Some(config) = self.file_by_path(&self.config) {
            if let Ok(header) = inspect_config(&config.text, &self.config, &self.cwd) {
                self.header = header;
            }
        }
    }
    pub fn ned_files(&self) -> impl Iterator<Item = &CapturedFile> {
        self.files.values().filter(|f| {
            f.roles
                .iter()
                .any(|role| matches!(role, FileRole::Ned { .. }))
        })
    }
    pub fn file_by_path(&self, path: &Path) -> Option<&CapturedFile> {
        self.files.values().find(|f| f.path == path)
    }
    pub fn input_digest(&self) -> String {
        let mut bytes = b"ned-editor-input-v1".to_vec();
        for root in &self.header.roots {
            add_hash_part(&mut bytes, root.to_string_lossy().as_bytes());
        }
        for f in self.files.values() {
            add_hash_part(&mut bytes, f.path.to_string_lossy().as_bytes());
            add_hash_part(&mut bytes, f.text.as_bytes());
        }
        hash(&bytes)
    }
    pub fn source(&self) -> SnapshotInputSource {
        SnapshotInputSource {
            metadata: self.metadata.clone(),
            directories: self.directories.clone(),
            files: self
                .files
                .values()
                .map(|f| (f.path.clone(), f.text.clone()))
                .collect(),
        }
    }
    pub fn input_regions(&self) -> Vec<PathBuf> {
        let mut paths = self.header.roots.clone();
        for f in self
            .files
            .values()
            .filter(|f| !f.roles.iter().all(|r| matches!(r, FileRole::Ned { .. })))
        {
            if let Some(parent) = f.path.parent() {
                paths.push(parent.to_path_buf());
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }
}
fn add_hash_part(bytes: &mut Vec<u8>, part: &[u8]) {
    bytes.extend_from_slice(&(part.len() as u64).to_le_bytes());
    bytes.extend_from_slice(part);
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotInputSource {
    pub metadata: BTreeMap<PathBuf, InputKind>,
    pub directories: BTreeMap<PathBuf, Vec<(OsString, InputKind)>>,
    pub files: BTreeMap<PathBuf, Arc<str>>,
}
impl SnapshotInputSource {
    /// Build prospective output input without opening any paths on disk.
    pub fn virtual_project(files: BTreeMap<PathBuf, Arc<str>>, directories: &[PathBuf]) -> Self {
        let mut result = Self {
            files,
            ..Self::default()
        };
        let file_paths: Vec<_> = result.files.keys().cloned().collect();
        for path in directories {
            result.add_directory(path);
        }
        for path in file_paths {
            if let Some(parent) = path.parent() {
                result.add_directory(parent);
            }
            result.metadata.insert(path.clone(), InputKind::File);
            if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                result
                    .directories
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push((name.to_owned(), InputKind::File));
            }
        }
        for entries in result.directories.values_mut() {
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries.dedup_by(|a, b| a.0 == b.0);
        }
        result
    }
    fn add_directory(&mut self, path: &Path) {
        let mut ancestors: Vec<_> = path.ancestors().collect();
        ancestors.reverse();
        for dir in ancestors {
            self.metadata
                .insert(dir.to_path_buf(), InputKind::Directory);
            self.directories.entry(dir.to_path_buf()).or_default();
            if let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) {
                let entries = self.directories.entry(parent.to_path_buf()).or_default();
                if !entries.iter().any(|(n, _)| n == name) {
                    entries.push((name.to_owned(), InputKind::Directory));
                }
            }
        }
    }
}
impl InputSource for SnapshotInputSource {
    fn metadata(&self, path: &Path) -> io::Result<InputMetadata> {
        self.metadata
            .get(path)
            .copied()
            .map(|kind| InputMetadata { kind })
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<InputDirEntry>> {
        self.directories
            .get(path)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(name, kind)| InputDirEntry {
                        name: name.clone(),
                        kind: Ok(*kind),
                    })
                    .collect()
            })
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
    fn read_utf8(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(path)
            .map(|v| v.to_string())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

fn input_error(path: &Path, error: impl std::fmt::Display) -> EditorError {
    EditorError::new(
        "E-EDITOR-INPUT-IO",
        format!("{}: {error}", path.display()),
        500,
    )
}
fn kind(m: &fs::Metadata) -> InputKind {
    if m.file_type().is_symlink() {
        InputKind::Symlink
    } else if m.is_file() {
        InputKind::File
    } else if m.is_dir() {
        InputKind::Directory
    } else {
        InputKind::Other
    }
}
fn checked_ancestors(path: &Path, metadata: &mut BTreeMap<PathBuf, InputKind>) -> Result<()> {
    for p in path.ancestors() {
        let m = fs::symlink_metadata(p).map_err(|e| input_error(p, e))?;
        if kind(&m) == InputKind::Symlink {
            return Err(EditorError::new(
                "E-EDITOR-INPUT-PATH",
                format!("Symlink input is unsupported: {}", p.display()),
                422,
            ));
        }
        metadata.insert(p.to_path_buf(), kind(&m));
    }
    Ok(())
}
fn read_bytes(
    path: &Path,
    metadata: &mut BTreeMap<PathBuf, InputKind>,
) -> Result<(Vec<u8>, CapturedStat)> {
    checked_ancestors(path, metadata)?;
    let before = fs::symlink_metadata(path).map_err(|e| input_error(path, e))?;
    if !before.is_file() {
        return Err(EditorError::new(
            "E-EDITOR-INPUT-PATH",
            "Input must be a regular file",
            422,
        ));
    }
    let mut f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .map_err(|e| input_error(path, e))?;
    let opened = f.metadata().map_err(|e| input_error(path, e))?;
    if CapturedStat::from_metadata(&before) != CapturedStat::from_metadata(&opened) {
        return Err(EditorError::new(
            "E-EDITOR-INPUT-CHANGING",
            "Input changed while opening",
            409,
        ));
    }
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| input_error(path, e))?;
    let after = f.metadata().map_err(|e| input_error(path, e))?;
    drop(f);
    if CapturedStat::from_metadata(&opened) != CapturedStat::from_metadata(&after)
        || opened.len() != after.len()
        || opened.modified().ok() != after.modified().ok()
    {
        return Err(EditorError::new(
            "E-EDITOR-INPUT-CHANGING",
            "Input changed while reading",
            409,
        ));
    }
    Ok((bytes, CapturedStat::from_metadata(&after)))
}
fn read_capture(
    path: &Path,
    roles: Vec<FileRole>,
    metadata: &mut BTreeMap<PathBuf, InputKind>,
) -> Result<CapturedFile> {
    let (bytes, stat) = read_bytes(path, metadata)?;
    let digest = hash(&bytes);
    let text: Arc<str> = String::from_utf8(bytes)
        .map_err(|e| input_error(path, e))?
        .into();
    Ok(CapturedFile {
        id: format!("f{}", &hash(path.to_string_lossy().as_bytes())[..24]),
        path: path.to_path_buf(),
        roles,
        text: text.clone(),
        hash: digest.clone(),
        origin_text: text,
        origin_hash: digest,
        stat: Some(stat),
    })
}
fn add_file(project: &mut ProjectSnapshot, path: &Path, role: FileRole) -> Result<()> {
    if let Some(existing) = project.files.values_mut().find(|f| f.path == path) {
        existing.roles.push(role);
        return Ok(());
    }
    let f = read_capture(path, vec![role], &mut project.metadata)?;
    project.files.insert(f.id.clone(), f);
    Ok(())
}
fn collect_ned(
    project: &mut ProjectSnapshot,
    root: &Path,
    directory: &Path,
    index: usize,
) -> Result<()> {
    checked_ancestors(directory, &mut project.metadata)?;
    if project.metadata.get(directory) != Some(&InputKind::Directory) {
        return Err(EditorError::new(
            "E-EDITOR-INPUT-PATH",
            "NED root must be a directory",
            422,
        ));
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|e| input_error(directory, e))?
        .collect::<io::Result<Vec<_>>>()
        .map_err(|e| input_error(directory, e))?;
    entries.sort_by_key(|e| e.file_name());
    let mut captured = Vec::new();
    for e in entries {
        let path = e.path();
        if path.to_str().is_none() {
            return Err(EditorError::new(
                "E-EDITOR-INPUT-PATH",
                "Non-UTF-8 NED path",
                422,
            ));
        }
        let m = fs::symlink_metadata(&path).map_err(|e| input_error(&path, e))?;
        let k = kind(&m);
        if k == InputKind::Symlink {
            return Err(EditorError::new(
                "E-EDITOR-INPUT-PATH",
                format!("Symlink in NED root: {}", path.display()),
                422,
            ));
        }
        captured.push((e.file_name(), k));
        project.metadata.insert(path.clone(), k);
        if k == InputKind::Directory {
            collect_ned(project, root, &path, index)?;
        } else if k == InputKind::File && path.extension().is_some_and(|e| e == "ned") {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| input_error(&path, e))?
                .to_path_buf();
            let package = relative
                .parent()
                .unwrap_or(Path::new(""))
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join(".");
            add_file(
                project,
                &path,
                FileRole::Ned {
                    root_index: index,
                    relative,
                    package,
                },
            )?;
        }
    }
    project
        .directories
        .insert(directory.to_path_buf(), captured);
    Ok(())
}
fn collect_once(config: &Path, cwd: &Path) -> Result<ProjectSnapshot> {
    let config = absolute(config, cwd);
    let mut metadata = BTreeMap::new();
    let ini = read_capture(&config, vec![FileRole::Config], &mut metadata)?;
    let header = inspect_config(&ini.text, &config, cwd)
        .map_err(|e| EditorError::common("E-EDITOR-LOAD-SYNTAX", e))?;
    let mut project = ProjectSnapshot {
        origin: ProjectOrigin::Disk,
        id: random_id("snapshot-")?,
        config,
        cwd: cwd.to_path_buf(),
        header,
        files: BTreeMap::from([(ini.id.clone(), ini)]),
        directories: BTreeMap::new(),
        metadata,
        layouts: BTreeMap::new(),
        parsed: BTreeMap::new(),
    };
    for (i, root) in project.header.roots.clone().iter().enumerate() {
        if project.header.roots[..i]
            .iter()
            .any(|p| root.starts_with(p) || p.starts_with(root))
        {
            return Err(EditorError::new(
                "E-EDITOR-INPUT-PATH",
                "Overlapping NED roots",
                422,
            ));
        }
        collect_ned(&mut project, root, root, i)?;
    }
    if let Some(path) = project.header.workload.clone() {
        add_file(&mut project, &path, FileRole::Workload)?;
    }
    if let Some(path) = project.header.model_config.clone() {
        add_file(&mut project, &path, FileRole::ModelConfig)?;
    }
    for file in project.ned_files().cloned().collect::<Vec<_>>() {
        let path = file.path.with_file_name(format!(
            "{}.layout.json",
            file.path.file_name().unwrap().to_string_lossy()
        ));
        let mut capture = LayoutCapture {
            path: path.clone(),
            raw: None,
            hash: None,
            stat: None,
            adopted: None,
            warning: None,
        };
        match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() => {
                let (bytes, stat) = read_bytes(&path, &mut project.metadata)?;
                capture.hash = Some(hash(&bytes));
                capture.stat = Some(stat);
                match String::from_utf8(bytes) {
                    Ok(raw) => {
                        match LayoutDocument::parse(&raw) {
                            Ok(l) => capture.adopted = Some(l),
                            Err(e) => capture.warning = Some(e.message),
                        }
                        capture.raw = Some(raw.into());
                    }
                    Err(e) => capture.warning = Some(format!("Invalid layout UTF-8: {e}")),
                }
            }
            Ok(_) => {
                return Err(EditorError::new(
                    "E-EDITOR-INPUT-PATH",
                    "Layout must be a regular non-symlink file",
                    422,
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(input_error(&path, e)),
        }
        project.layouts.insert(file.id.clone(), capture);
    }
    Ok(project)
}
fn observation_digest(project: &ProjectSnapshot) -> String {
    let mut data = project.input_digest().into_bytes();
    for (path, entries) in &project.directories {
        add_hash_part(&mut data, path.to_string_lossy().as_bytes());
        for (name, k) in entries {
            add_hash_part(&mut data, name.to_string_lossy().as_bytes());
            data.push(*k as u8);
        }
    }
    for f in project.files.values() {
        add_hash_part(&mut data, format!("{:?}", f.stat).as_bytes());
    }
    for (id, l) in &project.layouts {
        add_hash_part(&mut data, id.as_bytes());
        add_hash_part(&mut data, l.hash.as_deref().unwrap_or("").as_bytes());
        add_hash_part(&mut data, format!("{:?}", l.stat).as_bytes());
    }
    hash(&data)
}
pub(crate) fn load_project(config: &Path, cwd: &Path) -> Result<ProjectSnapshot> {
    for _ in 0..3 {
        let attempt = (|| {
            let a = collect_once(config, cwd)?;
            let b = collect_once(config, cwd)?;
            Ok::<_, EditorError>((a, b))
        })();
        let (a, mut b) = match attempt {
            Ok(v) => v,
            Err(e) if e.code == "E-EDITOR-INPUT-CHANGING" => continue,
            Err(e) => return Err(e),
        };
        if observation_digest(&a) != observation_digest(&b) {
            continue;
        }
        for f in b.ned_files().cloned().collect::<Vec<_>>() {
            let package = f
                .roles
                .iter()
                .find_map(|r| {
                    if let FileRole::Ned { package, .. } = r {
                        Some(package.as_str())
                    } else {
                        None
                    }
                })
                .unwrap();
            let parsed = parse_ned(&f.text, &f.path, package)
                .map_err(|e| EditorError::common("E-EDITOR-LOAD-SYNTAX", e))?;
            b.parsed.insert(f.id.clone(), parsed);
        }
        return Ok(b);
    }
    Err(EditorError::new(
        "E-EDITOR-INPUT-CHANGING",
        "Input changed during all three read attempts",
        409,
    ))
}
