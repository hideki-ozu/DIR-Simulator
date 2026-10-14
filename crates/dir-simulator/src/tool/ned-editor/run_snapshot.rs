//! Durable, immutable input bundles. Logical source paths are data, never blob paths.
use super::input::{
    CapturedFile, CapturedStat, FileRole, LayoutCapture, ProjectOrigin, ProjectSnapshot,
};
use super::layout::LayoutDocument;
use super::run_control::storage::Directory;
use super::{EditorError, Result, hash, random_id};
use crate::input::{InputKind, ProjectHeader, parse_ned};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) const CODEC_VERSION: u32 = 1;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MANIFEST: &str = "snapshot.json";
const COMMIT: &str = "snapshot.commit.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FrozenFile {
    pub file_id: String,
    #[serde(with = "roles_codec")]
    pub roles: Vec<FileRole>,
    #[serde(with = "path_codec")]
    pub original_logical_path: PathBuf,
    pub original_hash_at_load: String,
    pub frozen_hash: String,
    pub bundle_blob_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SnapshotEvidence {
    pub bundle_digest: String,
    pub input_digest: String,
    pub snapshot_id: String,
    pub bytes: u64,
    pub files: Vec<FrozenFile>,
}

// Unix paths and directory names need not be UTF-8. They must survive the codec exactly.
#[derive(Clone, Serialize, Deserialize)]
struct LogicalPath(Vec<u8>);
impl LogicalPath {
    fn capture(path: &Path) -> Self {
        Self(path.as_os_str().as_bytes().to_vec())
    }
    fn restore(&self) -> PathBuf {
        PathBuf::from(OsString::from_vec(self.0.clone()))
    }
}
// Evidence remains ordinary JSON strings for UTF-8 paths, with an exact byte form
// for Unix paths that serde's standard PathBuf codec cannot represent.
mod path_codec {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Text(String),
        Bytes { unix_bytes: Vec<u8> },
    }
    pub fn serialize<S: serde::Serializer>(
        path: &Path,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let stored = match path.to_str() {
            Some(text) => Stored::Text(text.into()),
            None => Stored::Bytes {
                unix_bytes: path.as_os_str().as_bytes().to_vec(),
            },
        };
        stored.serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<PathBuf, D::Error> {
        Ok(match Stored::deserialize(deserializer)? {
            Stored::Text(text) => text.into(),
            Stored::Bytes { unix_bytes } => OsString::from_vec(unix_bytes).into(),
        })
    }
}
mod roles_codec {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Stored {
        Config,
        Ned {
            root_index: usize,
            #[serde(with = "path_codec")]
            relative: PathBuf,
            package: String,
        },
        Workload,
        ModelConfig,
    }
    pub fn serialize<S: serde::Serializer>(
        roles: &[FileRole],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        roles
            .iter()
            .map(|role| match role {
                FileRole::Config => Stored::Config,
                FileRole::Ned {
                    root_index,
                    relative,
                    package,
                } => Stored::Ned {
                    root_index: *root_index,
                    relative: relative.clone(),
                    package: package.clone(),
                },
                FileRole::Workload => Stored::Workload,
                FileRole::ModelConfig => Stored::ModelConfig,
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Vec<FileRole>, D::Error> {
        Ok(Vec::<Stored>::deserialize(deserializer)?
            .into_iter()
            .map(|role| match role {
                Stored::Config => FileRole::Config,
                Stored::Ned {
                    root_index,
                    relative,
                    package,
                } => FileRole::Ned {
                    root_index,
                    relative,
                    package,
                },
                Stored::Workload => FileRole::Workload,
                Stored::ModelConfig => FileRole::ModelConfig,
            })
            .collect())
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    File,
    Directory,
    Symlink,
    Other,
}
impl From<InputKind> for Kind {
    fn from(kind: InputKind) -> Self {
        match kind {
            InputKind::File => Self::File,
            InputKind::Directory => Self::Directory,
            InputKind::Symlink => Self::Symlink,
            InputKind::Other => Self::Other,
        }
    }
}
impl From<Kind> for InputKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::File => Self::File,
            Kind::Directory => Self::Directory,
            Kind::Symlink => Self::Symlink,
            Kind::Other => Self::Other,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Origin {
    Disk,
    New { template: String, name: String },
}
#[derive(Serialize, Deserialize)]
struct Header {
    config: LogicalPath,
    cwd: LogicalPath,
    network: Option<String>,
    profile: Option<String>,
    roots: Vec<LogicalPath>,
    workload: Option<LogicalPath>,
    model_config: Option<LogicalPath>,
    general: BTreeMap<String, String>,
    channels: BTreeMap<String, BTreeMap<String, String>>,
}
impl Header {
    fn capture(h: &ProjectHeader) -> Self {
        Self {
            config: LogicalPath::capture(&h.config),
            cwd: LogicalPath::capture(&h.cwd),
            network: h.network.clone(),
            profile: h.profile.clone(),
            roots: h.roots.iter().map(|p| LogicalPath::capture(p)).collect(),
            workload: h.workload.as_deref().map(LogicalPath::capture),
            model_config: h.model_config.as_deref().map(LogicalPath::capture),
            general: h.general.clone(),
            channels: h.channels.clone(),
        }
    }
    fn restore(self) -> ProjectHeader {
        ProjectHeader {
            config: self.config.restore(),
            cwd: self.cwd.restore(),
            network: self.network,
            profile: self.profile,
            roots: self.roots.iter().map(LogicalPath::restore).collect(),
            workload: self.workload.map(|p| p.restore()),
            model_config: self.model_config.map(|p| p.restore()),
            general: self.general,
            channels: self.channels,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct File {
    id: String,
    path: LogicalPath,
    #[serde(with = "roles_codec")]
    roles: Vec<FileRole>,
    hash: String,
    origin_hash: String,
    blob: String,
    origin_blob: String,
    stat: Option<CapturedStat>,
}
#[derive(Serialize, Deserialize)]
struct Layout {
    file_id: String,
    path: LogicalPath,
    raw_blob: Option<String>,
    hash: Option<String>,
    stat: Option<CapturedStat>,
    adopted: Option<LayoutDocument>,
    warning: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Blob {
    id: String,
    bytes: u64,
    hash: String,
}
type DirectoryEntries = Vec<(Vec<u8>, Kind)>;
#[derive(Serialize, Deserialize)]
struct Manifest {
    schema_version: u32,
    snapshot_id: String,
    input_digest: String,
    origin: Origin,
    project_id: String,
    config: LogicalPath,
    cwd: LogicalPath,
    header: Header,
    files: Vec<File>,
    directories: Vec<(LogicalPath, DirectoryEntries)>,
    metadata: Vec<(LogicalPath, Kind)>,
    layouts: Vec<Layout>,
    blobs: Vec<Blob>,
}
#[derive(Serialize, Deserialize)]
struct Commit {
    schema_version: u32,
    bundle_digest: String,
    bytes: u64,
}
type BlobBytes = BTreeMap<String, Vec<u8>>;
type CaptureResult = (Manifest, BlobBytes, Vec<u8>, usize);
type LoadResult = (Manifest, BlobBytes, SnapshotEvidence);
fn integrity(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-RUN-INTEGRITY", message, 422)
}
fn capacity() -> EditorError {
    EditorError::new(
        "E-EDITOR-RUN-CAPACITY",
        "Snapshot exceeds 256 MiB or 10,000 input files",
        413,
    )
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| integrity(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| integrity(e.to_string()))
}
fn part(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}
fn bundle_digest(manifest: &[u8], blobs: &BTreeMap<String, Vec<u8>>) -> String {
    let mut digest = Sha256::new();
    part(&mut digest, b"ned-editor-run-snapshot-v1");
    part(&mut digest, manifest);
    for (id, bytes) in blobs {
        part(&mut digest, id.as_bytes());
        part(&mut digest, bytes);
    }
    format!("{:x}", digest.finalize())
}
fn add_blob(
    blobs: &mut BTreeMap<String, Vec<u8>>,
    bytes: &[u8],
    total: &mut usize,
) -> Result<String> {
    *total = total
        .checked_add(bytes.len())
        .filter(|n| *n <= MAX_BYTES)
        .ok_or_else(capacity)?;
    let id = random_id("blob-")?;
    blobs.insert(id.clone(), bytes.to_vec());
    Ok(id)
}

fn capture(project: &ProjectSnapshot) -> Result<CaptureResult> {
    if project.files.len() > MAX_FILES {
        return Err(capacity());
    }
    let mut blobs = BTreeMap::new();
    let mut total = 0usize;
    let mut files = Vec::new();
    let mut paths = BTreeSet::new();
    for (id, file) in &project.files {
        if id != &file.id
            || !paths.insert(&file.path)
            || hash(file.text.as_bytes()) != file.hash
            || hash(file.origin_text.as_bytes()) != file.origin_hash
        {
            return Err(integrity("Captured file identity or hash mismatch"));
        }
        let blob = add_blob(&mut blobs, file.text.as_bytes(), &mut total)?;
        let origin_blob = if file.text == file.origin_text {
            blob.clone()
        } else {
            add_blob(&mut blobs, file.origin_text.as_bytes(), &mut total)?
        };
        files.push(File {
            id: id.clone(),
            path: LogicalPath::capture(&file.path),
            roles: file.roles.clone(),
            hash: file.hash.clone(),
            origin_hash: file.origin_hash.clone(),
            blob,
            origin_blob,
            stat: file.stat.clone(),
        });
    }
    let mut layouts = Vec::new();
    for (file_id, layout) in &project.layouts {
        if !project.files.contains_key(file_id) {
            return Err(integrity("Layout refers to an unknown captured file"));
        }
        let raw_blob = layout
            .raw
            .as_ref()
            .map(|raw| add_blob(&mut blobs, raw.as_bytes(), &mut total))
            .transpose()?;
        if layout
            .raw
            .as_ref()
            .is_some_and(|raw| Some(hash(raw.as_bytes())) != layout.hash)
        {
            return Err(integrity("Captured layout hash mismatch"));
        }
        layouts.push(Layout {
            file_id: file_id.clone(),
            path: LogicalPath::capture(&layout.path),
            raw_blob,
            hash: layout.hash.clone(),
            stat: layout.stat.clone(),
            adopted: layout.adopted.clone(),
            warning: layout.warning.clone(),
        });
    }
    let manifest = Manifest {
        schema_version: CODEC_VERSION,
        snapshot_id: project.id.clone(),
        input_digest: project.input_digest(),
        origin: match &project.origin {
            ProjectOrigin::Disk => Origin::Disk,
            ProjectOrigin::New { template, name } => Origin::New {
                template: template.clone(),
                name: name.clone(),
            },
        },
        project_id: project.id.clone(),
        config: LogicalPath::capture(&project.config),
        cwd: LogicalPath::capture(&project.cwd),
        header: Header::capture(&project.header),
        files,
        directories: project
            .directories
            .iter()
            .map(|(path, entries)| {
                (
                    LogicalPath::capture(path),
                    entries
                        .iter()
                        .map(|(name, kind)| (name.as_bytes().to_vec(), (*kind).into()))
                        .collect(),
                )
            })
            .collect(),
        metadata: project
            .metadata
            .iter()
            .map(|(path, kind)| (LogicalPath::capture(path), (*kind).into()))
            .collect(),
        layouts,
        blobs: blobs
            .iter()
            .map(|(id, bytes)| Blob {
                id: id.clone(),
                bytes: bytes.len() as u64,
                hash: hash(bytes),
            })
            .collect(),
    };
    let raw = encode(&manifest)?;
    total = total
        .checked_add(raw.len())
        .filter(|n| *n <= MAX_BYTES)
        .ok_or_else(capacity)?;
    Ok((manifest, blobs, raw, total))
}
/// Precheck the complete durable bundle capacity, including origin and layout bytes.
pub(crate) fn size(project: &ProjectSnapshot) -> Result<u64> {
    Ok(capture(project)?.3 as u64)
}
/// Freeze captured memory only; the original source tree is never opened.
pub(crate) fn freeze(project: &ProjectSnapshot, input: &Path) -> Result<SnapshotEvidence> {
    let (manifest, blobs, raw, total) = capture(project)?;
    let digest = bundle_digest(&raw, &blobs);
    let evidence = evidence(&manifest, &digest, total);
    // All representation and capacity checks precede the first filesystem mutation.
    let directory = Directory::open(input)?;
    if !directory.names()?.is_empty() {
        return Err(integrity("Snapshot input directory is not empty"));
    }
    let blob_dir = directory.mkdir("blobs")?;
    for (id, bytes) in &blobs {
        blob_dir.create(id, bytes)?;
    }
    blob_dir.sync()?;
    directory.create(MANIFEST, &raw)?;
    directory.sync()?;
    #[cfg(test)]
    crate::acceptance_hooks::hit("snapshot.before_commit");
    directory.replace(
        COMMIT,
        &encode(&Commit {
            schema_version: CODEC_VERSION,
            bundle_digest: digest,
            bytes: total as u64,
        })?,
    )?;
    #[cfg(test)]
    crate::acceptance_hooks::hit("snapshot.after_commit");
    directory.sync()?;
    #[cfg(test)]
    crate::acceptance_hooks::hit("snapshot.after_sync");
    Ok(evidence)
}
fn evidence(manifest: &Manifest, digest: &str, bytes: usize) -> SnapshotEvidence {
    SnapshotEvidence {
        bundle_digest: digest.into(),
        input_digest: manifest.input_digest.clone(),
        snapshot_id: manifest.snapshot_id.clone(),
        bytes: bytes as u64,
        files: manifest
            .files
            .iter()
            .map(|file| FrozenFile {
                file_id: file.id.clone(),
                roles: file.roles.clone(),
                original_logical_path: file.path.restore(),
                original_hash_at_load: file.origin_hash.clone(),
                frozen_hash: file.hash.clone(),
                bundle_blob_id: file.blob.clone(),
            })
            .collect(),
    }
}
fn valid_blob_id(id: &str) -> bool {
    id.len() == 69 && id.starts_with("blob-") && id[5..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn load(input: &Path, expected_digest: &str) -> Result<LoadResult> {
    let directory = Directory::open(input)?;
    let expected_names = [
        OsString::from("blobs"),
        OsString::from(COMMIT),
        OsString::from(MANIFEST),
    ];
    if directory.names()?.into_iter().collect::<BTreeSet<_>>()
        != expected_names.into_iter().collect()
    {
        return Err(integrity(
            "Unexpected or incomplete snapshot directory entries",
        ));
    }
    let commit: Commit = decode(&directory.read(COMMIT, 4096)?)?;
    if commit.schema_version != CODEC_VERSION || commit.bundle_digest != expected_digest {
        return Err(integrity("Snapshot commit identity mismatch"));
    }
    if commit.bytes > MAX_BYTES as u64 {
        return Err(capacity());
    }
    let raw = directory.read(MANIFEST, MAX_BYTES)?;
    let manifest: Manifest = decode(&raw)?;
    if manifest.schema_version != CODEC_VERSION {
        return Err(integrity("Unsupported snapshot codec"));
    }
    if manifest.files.len() > MAX_FILES || manifest.blobs.len() > MAX_FILES * 3 {
        return Err(capacity());
    }
    let blob_dir = Directory::open(&input.join("blobs"))?;
    let mut total = raw.len();
    let mut blobs = BTreeMap::new();
    for blob in &manifest.blobs {
        if !valid_blob_id(&blob.id) || blobs.contains_key(&blob.id) {
            return Err(integrity("Invalid or duplicate blob ID"));
        }
        let size = usize::try_from(blob.bytes).map_err(|_| capacity())?;
        total = total
            .checked_add(size)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(capacity)?;
        let bytes = blob_dir.read(&blob.id, size)?;
        if bytes.len() != size || hash(&bytes) != blob.hash {
            return Err(integrity("Snapshot blob size or hash mismatch"));
        }
        blobs.insert(blob.id.clone(), bytes);
    }
    if blob_dir.names()?.into_iter().collect::<BTreeSet<_>>()
        != blobs.keys().map(OsString::from).collect()
    {
        return Err(integrity("Unexpected snapshot blob entries"));
    }
    let digest = bundle_digest(&raw, &blobs);
    if digest != expected_digest || total as u64 != commit.bytes {
        return Err(integrity("Snapshot bundle digest or size mismatch"));
    }
    let evidence = evidence(&manifest, &digest, total);
    Ok((manifest, blobs, evidence))
}
pub(crate) fn inspect(input: &Path, expected_digest: &str) -> Result<SnapshotEvidence> {
    let (manifest, blobs, evidence) = load(input, expected_digest)?;
    let project = reconstruct(manifest, blobs)?;
    if project.input_digest() != evidence.input_digest {
        return Err(integrity("Snapshot input digest mismatch"));
    }
    Ok(evidence)
}
pub(crate) fn restore(input: &Path, expected_digest: &str) -> Result<ProjectSnapshot> {
    restore_with_evidence(input, expected_digest).map(|(project, _)| project)
}
pub(crate) fn restore_with_evidence(
    input: &Path,
    expected_digest: &str,
) -> Result<(ProjectSnapshot, SnapshotEvidence)> {
    let (manifest, blobs, evidence) = load(input, expected_digest)?;
    let mut project = reconstruct(manifest, blobs)?;
    if project.input_digest() != evidence.input_digest {
        return Err(integrity("Snapshot input digest mismatch"));
    }
    // Parse only successful files. Bad edited input belongs to worker prepare diagnostics.
    let parsed = project
        .ned_files()
        .filter_map(|file| {
            let package = file.roles.iter().find_map(|role| match role {
                FileRole::Ned { package, .. } => Some(package),
                _ => None,
            })?;
            parse_ned(&file.text, &file.path, package)
                .ok()
                .map(|ned| (file.id.clone(), ned))
        })
        .collect();
    project.parsed = parsed;
    Ok((project, evidence))
}
fn text_blob(blobs: &BTreeMap<String, Vec<u8>>, id: &str, expected_hash: &str) -> Result<Arc<str>> {
    let bytes = blobs
        .get(id)
        .ok_or_else(|| integrity("Missing snapshot blob"))?;
    if hash(bytes) != expected_hash {
        return Err(integrity("File-to-blob hash mismatch"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| integrity("Snapshot source is not UTF-8"))?;
    Ok(Arc::from(text))
}
fn reconstruct(manifest: Manifest, blobs: BTreeMap<String, Vec<u8>>) -> Result<ProjectSnapshot> {
    let mut files = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for file in manifest.files {
        let path = file.path.restore();
        if files.contains_key(&file.id) || !paths.insert(path.clone()) {
            return Err(integrity("Duplicate captured file identity or path"));
        }
        let text = text_blob(&blobs, &file.blob, &file.hash)?;
        let origin_text = text_blob(&blobs, &file.origin_blob, &file.origin_hash)?;
        files.insert(
            file.id.clone(),
            CapturedFile {
                id: file.id,
                path,
                roles: file.roles,
                text,
                hash: file.hash,
                origin_text,
                origin_hash: file.origin_hash,
                stat: file.stat,
            },
        );
    }
    let mut layouts = BTreeMap::new();
    for layout in manifest.layouts {
        if !files.contains_key(&layout.file_id) || layouts.contains_key(&layout.file_id) {
            return Err(integrity("Invalid layout file identity"));
        }
        let raw = match (&layout.raw_blob, &layout.hash) {
            (Some(id), Some(hash)) => Some(text_blob(&blobs, id, hash)?),
            // Invalid UTF-8 layouts have a captured hash/stat/warning but no UTF-8 raw text.
            (None, _) => None,
            _ => return Err(integrity("Layout blob/hash presence mismatch")),
        };
        layouts.insert(
            layout.file_id,
            LayoutCapture {
                path: layout.path.restore(),
                raw,
                hash: layout.hash,
                stat: layout.stat,
                adopted: layout.adopted,
                warning: layout.warning,
            },
        );
    }
    let mut directories = BTreeMap::new();
    for (path, entries) in manifest.directories {
        if directories
            .insert(
                path.restore(),
                entries
                    .into_iter()
                    .map(|(name, kind)| (OsString::from_vec(name), kind.into()))
                    .collect(),
            )
            .is_some()
        {
            return Err(integrity("Duplicate logical directory"));
        }
    }
    let mut metadata = BTreeMap::new();
    for (path, kind) in manifest.metadata {
        if metadata.insert(path.restore(), kind.into()).is_some() {
            return Err(integrity("Duplicate logical metadata"));
        }
    }
    Ok(ProjectSnapshot {
        origin: match manifest.origin {
            Origin::Disk => ProjectOrigin::Disk,
            Origin::New { template, name } => ProjectOrigin::New { template, name },
        },
        id: manifest.project_id,
        config: manifest.config.restore(),
        cwd: manifest.cwd.restore(),
        header: manifest.header.restore(),
        files,
        directories,
        metadata,
        layouts,
        parsed: BTreeMap::new(),
    })
}

#[cfg(test)]
#[path = "run_snapshot/tests.rs"]
mod tests;
