//! Bounded, descriptor-relative checkpoint storage. Publication never replaces a destination.
use crate::registry::{ModelError, ModelResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(n: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&n.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let text = String::deserialize(d)?;
        let n: u64 = text.parse().map_err(serde::de::Error::custom)?;
        if text != n.to_string() {
            return Err(serde::de::Error::custom("noncanonical u64"));
        }
        Ok(n)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CheckpointLimits {
    #[serde(with = "decimal")]
    pub max_total_bytes: u64,
    pub max_files: u32,
    #[serde(with = "decimal")]
    pub max_blob_bytes: u64,
    pub max_fes_entries: u32,
}
impl Default for CheckpointLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 1_073_741_824,
            max_files: 100_000,
            max_blob_bytes: 268_435_456,
            max_fes_entries: 1_000_000,
        }
    }
}
impl CheckpointLimits {
    pub fn validate(&self) -> ModelResult {
        if self.max_total_bytes == 0
            || self.max_files == 0
            || self.max_blob_bytes == 0
            || self.max_fes_entries == 0
            || self.max_blob_bytes > self.max_total_bytes
        {
            return Err(invalid("invalid checkpoint limits"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointDurability {
    Durable,
    PublishedDurabilityUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointReceipt {
    pub destination: PathBuf,
    pub manifest_sha256: String,
    #[serde(with = "decimal")]
    pub checkpoint_ordinal: u64,
    pub durability: CheckpointDurability,
    pub diagnostic: Option<String>,
}
type EncodedBundle = (Vec<u8>, BTreeMap<String, Vec<u8>>);

fn invalid(s: impl std::fmt::Display) -> ModelError {
    ModelError(format!("CheckpointInvalid: {s}"))
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
#[cfg(test)]
thread_local! { static IO_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn relative_name(s: &str) -> ModelResult {
    if s.is_empty()
        || s.len() > 4096
        || s.contains('\\')
        || s.contains('\0')
        || s.split('/').count() > 64
        || s.split('/').any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(invalid("invalid relative POSIX path"));
    }
    if s == "state.json" || s.starts_with("inputs/") {
        return Ok(());
    }
    if let Some(hash) = s
        .strip_prefix("objects/")
        .and_then(|v| v.strip_suffix(".bin"))
    {
        if is_hash(hash) {
            return Ok(());
        }
    }
    Err(invalid("unsupported bundle file path"))
}

struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueJson(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite JSON"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(v) = a.next_element::<UniqueJson>()? {
                    items.push(v.0);
                }
                Ok(UniqueJson(Value::Array(items)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if items.contains_key(&k) {
                        return Err(serde::de::Error::custom("duplicate JSON key"));
                    }
                    items.insert(k, a.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(items)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
pub(crate) fn strict_json(bytes: &[u8]) -> ModelResult<Value> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(invalid("JSON BOM"));
    }
    let v = serde_json::from_slice::<UniqueJson>(bytes)
        .map_err(invalid)?
        .0;
    fn depth(v: &Value, d: usize) -> ModelResult {
        if d > 64 {
            return Err(invalid("JSON depth exceeds 64"));
        }
        match v {
            Value::Array(a) => {
                for v in a {
                    depth(v, d + 1)?;
                }
            }
            Value::Object(a) => {
                for v in a.values() {
                    depth(v, d + 1)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    depth(&v, 1)?;
    Ok(v)
}
fn validate_files(
    files: &BTreeMap<String, Vec<u8>>,
    limits: &CheckpointLimits,
) -> ModelResult<u64> {
    limits.validate()?;
    if files
        .len()
        .checked_add(1)
        .is_none_or(|n| n > limits.max_files as usize)
        || !files.contains_key("state.json")
    {
        return Err(invalid("file count or missing state.json"));
    }
    let mut total = 0u64;
    for (name, bytes) in files {
        relative_name(name)?;
        let mut parent = Path::new(name).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            if path.to_str().is_some_and(|path| files.contains_key(path)) {
                return Err(invalid("file/directory path collision"));
            }
            parent = path.parent();
        }
        let size = u64::try_from(bytes.len()).map_err(invalid)?;
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("byte count overflow"))?;
        if total > limits.max_total_bytes {
            return Err(invalid("total byte limit"));
        }
        if name.starts_with("objects/")
            && (size > limits.max_blob_bytes || name != &format!("objects/{}.bin", sha(bytes)))
        {
            return Err(invalid("blob length or content hash path"));
        }
        if name.ends_with(".json") {
            let value = strict_json(bytes)?;
            if name == "state.json" {
                let fes = value
                    .get("fes")
                    .and_then(Value::as_array)
                    .ok_or_else(|| invalid("state.fes must be an array"))?;
                if fes.len() > limits.max_fes_entries as usize {
                    return Err(invalid("FES entry limit"));
                }
            }
        }
    }
    Ok(total)
}

fn declared_files(
    manifest: &Value,
    manifest_bytes: usize,
    limits: &CheckpointLimits,
) -> ModelResult<BTreeMap<String, u64>> {
    if manifest.get("schema_name").and_then(Value::as_str) != Some("dir.checkpoint.bundle")
        || manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("state_path").and_then(Value::as_str) != Some("state.json")
    {
        return Err(invalid("manifest schema/state_path"));
    }
    let rows = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("manifest.files"))?;
    if rows
        .len()
        .checked_add(1)
        .is_none_or(|n| n > limits.max_files as usize)
    {
        return Err(invalid("manifest file count limit"));
    }
    let mut files = BTreeMap::new();
    let mut total = manifest_bytes as u64;
    for row in rows {
        let obj = row
            .as_object()
            .ok_or_else(|| invalid("manifest file row"))?;
        if obj.len() != 3 {
            return Err(invalid("manifest file row fields"));
        }
        let path = row
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("file path"))?;
        relative_name(path)?;
        let text = row
            .get("bytes")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("file bytes"))?;
        let size: u64 = text.parse().map_err(invalid)?;
        if text != size.to_string() {
            return Err(invalid("noncanonical file byte count"));
        }
        let hash = row
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("file sha256"))?;
        if !is_hash(hash) {
            return Err(invalid("file sha256"));
        }
        if files.insert(path.to_owned(), size).is_some() {
            return Err(invalid("duplicate file path"));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("declared byte overflow"))?;
        if total > limits.max_total_bytes
            || (path.starts_with("objects/") && size > limits.max_blob_bytes)
        {
            return Err(invalid("manifest byte limit"));
        }
    }
    if !files.contains_key("state.json") {
        return Err(invalid("missing state.json"));
    }
    Ok(files)
}

/// Write a new bundle; only the returned receipt confirms its publication.
pub fn write_bundle(
    destination: &Path,
    manifest: Value,
    files: BTreeMap<String, Vec<u8>>,
    limits: &CheckpointLimits,
    ordinal: u64,
) -> ModelResult<CheckpointReceipt> {
    let bytes = encode_manifest(manifest, &files, limits, ordinal)?;
    platform::write(destination, &bytes, &files, ordinal)
}

pub(crate) fn encode_manifest(
    mut manifest: Value,
    files: &BTreeMap<String, Vec<u8>>,
    limits: &CheckpointLimits,
    ordinal: u64,
) -> ModelResult<Vec<u8>> {
    let total = validate_files(files, limits)?;
    if ordinal == u64::MAX {
        return Err(ModelError("ResourceExhausted: checkpoint ordinal".into()));
    }
    let map = manifest
        .as_object_mut()
        .ok_or_else(|| invalid("manifest must be an object"))?;
    if map.get("schema_name").and_then(Value::as_str) != Some("dir.checkpoint.bundle")
        || map.get("schema_version").and_then(Value::as_u64) != Some(1)
        || map.get("state_path").and_then(Value::as_str) != Some("state.json")
    {
        return Err(invalid("manifest schema/state_path"));
    }
    map.insert(
        "checkpoint_ordinal".into(),
        Value::String(ordinal.to_string()),
    );
    map.insert("files".into(), Value::Array(files.iter().map(|(path, bytes)| json!({"path":path,"bytes":bytes.len().to_string(),"sha256":sha(bytes)})).collect()));
    let bytes = serde_json::to_vec(&manifest).map_err(invalid)?;
    strict_json(&bytes)?;
    if total
        .checked_add(bytes.len() as u64)
        .is_none_or(|n| n > limits.max_total_bytes)
    {
        return Err(invalid("total byte limit including manifest"));
    }
    Ok(bytes)
}

pub fn read_bundle(
    path: &Path,
    limits: &CheckpointLimits,
) -> ModelResult<(Value, BTreeMap<String, Vec<u8>>)> {
    read_bundle_with_manifest_hash(path, limits).map(|(_, manifest, files)| (manifest, files))
}

pub(crate) fn read_bundle_with_manifest_hash(
    path: &Path,
    limits: &CheckpointLimits,
) -> ModelResult<(String, Value, BTreeMap<String, Vec<u8>>)> {
    limits.validate()?;
    let (manifest_bytes, files) = platform::read(path, limits)?;
    let manifest = strict_json(&manifest_bytes)?;
    declared_files(&manifest, manifest_bytes.len(), limits)?;
    if manifest.get("schema_name").and_then(Value::as_str) != Some("dir.checkpoint.bundle")
        || manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("state_path").and_then(Value::as_str) != Some("state.json")
    {
        return Err(invalid("manifest schema/state_path"));
    }
    let rows = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("manifest.files"))?;
    let mut listed = BTreeSet::new();
    for row in rows {
        let map = row
            .as_object()
            .ok_or_else(|| invalid("manifest file row"))?;
        if map.len() != 3 {
            return Err(invalid("manifest file row fields"));
        }
        let name = row
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("file path"))?;
        relative_name(name)?;
        if !listed.insert(name.to_owned()) {
            return Err(invalid("duplicate file path"));
        }
        let bytes = files
            .get(name)
            .ok_or_else(|| invalid("missing listed file"))?;
        if row.get("bytes").and_then(Value::as_str) != Some(bytes.len().to_string().as_str())
            || row.get("sha256").and_then(Value::as_str) != Some(sha(bytes).as_str())
        {
            return Err(invalid("file length/hash mismatch"));
        }
    }
    if !listed.iter().eq(files.keys()) {
        return Err(invalid("unlisted bundle file"));
    }
    let total = validate_files(&files, limits)?;
    if total
        .checked_add(manifest_bytes.len() as u64)
        .is_none_or(|n| n > limits.max_total_bytes)
    {
        return Err(invalid("total byte limit including manifest"));
    }
    Ok((sha(&manifest_bytes), manifest, files))
}

#[cfg(unix)]
mod platform {
    use super::*;
    use rustix::fd::{AsFd, OwnedFd};
    use rustix::fs::{self, AtFlags, CWD, FileType, Mode, OFlags};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Component;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    const DIR: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    fn directory(path: &Path) -> ModelResult<OwnedFd> {
        let mut fd = fs::openat(
            CWD,
            if path.is_absolute() { "/" } else { "." },
            DIR,
            Mode::empty(),
        )
        .map_err(invalid)?;
        for part in path.components() {
            match part {
                Component::RootDir | Component::CurDir => (),
                Component::Normal(name) => {
                    fd = fs::openat(&fd, name, DIR, Mode::empty()).map_err(invalid)?
                }
                _ => return Err(invalid("parent path components forbidden")),
            }
        }
        Ok(fd)
    }
    fn file_parent(root: &impl AsFd, path: &str) -> ModelResult<(OwnedFd, String)> {
        let mut fd = fs::openat(root, ".", DIR, Mode::empty()).map_err(invalid)?;
        let parts: Vec<_> = path.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            fd = fs::openat(&fd, *part, DIR, Mode::empty()).map_err(invalid)?;
        }
        Ok((fd, parts[parts.len() - 1].to_owned()))
    }
    struct Staging<'a> {
        parent: &'a OwnedFd,
        fd: OwnedFd,
        name: String,
        owned: Vec<String>,
        owned_dirs: Vec<String>,
        published: bool,
    }
    impl Drop for Staging<'_> {
        fn drop(&mut self) {
            if self.published {
                return;
            }
            for name in self.owned.iter().rev() {
                if let Ok((parent, leaf)) = file_parent(&self.fd, name) {
                    let _ = fs::unlinkat(&parent, leaf.as_str(), AtFlags::empty());
                }
            }
            for dir in self.owned_dirs.iter().rev() {
                if let Ok((parent, leaf)) = file_parent(&self.fd, dir) {
                    let _ = fs::unlinkat(&parent, leaf.as_str(), AtFlags::REMOVEDIR);
                }
            }
            let _ = fs::unlinkat(self.parent, self.name.as_str(), AtFlags::REMOVEDIR);
        }
    }
    fn write_file(stage: &mut Staging<'_>, name: &str, bytes: &[u8]) -> ModelResult {
        let mut parent = fs::openat(&stage.fd, ".", DIR, Mode::empty()).map_err(invalid)?;
        let parts: Vec<_> = name.split('/').collect();
        for (i, part) in parts[..parts.len() - 1].iter().enumerate() {
            match fs::mkdirat(&parent, *part, Mode::from_bits_truncate(0o700)) {
                Ok(()) => stage.owned_dirs.push(parts[..=i].join("/")),
                Err(rustix::io::Errno::EXIST) => (),
                Err(e) => return Err(invalid(e)),
            }
            #[cfg(test)]
            if IO_FAULT.with(|f| f.get() == Some("after_mkdir")) {
                return Err(invalid("injected after directory creation"));
            }
            parent = fs::openat(&parent, *part, DIR, Mode::empty()).map_err(invalid)?;
        }
        let leaf = parts[parts.len() - 1];
        let fd = fs::openat(
            &parent,
            leaf,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(invalid)?;
        stage.owned.push(name.to_owned());
        let mut file = File::from(fd);
        file.write_all(bytes).map_err(invalid)?;
        file.sync_all()
            .map_err(|e| ModelError(format!("DurabilityUnsupported: {e}")))?;
        fs::fsync(&parent).map_err(|e| ModelError(format!("DurabilityUnsupported: {e}")))?;
        Ok(())
    }
    pub(super) fn write(
        destination: &Path,
        manifest: &[u8],
        files: &BTreeMap<String, Vec<u8>>,
        ordinal: u64,
    ) -> ModelResult<CheckpointReceipt> {
        let parent_path = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let leaf = destination
            .file_name()
            .ok_or_else(|| invalid("destination name"))?;
        if destination
            .components()
            .last()
            .is_none_or(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(invalid("destination name"));
        }
        let parent = directory(parent_path)?;
        match fs::statat(&parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(ModelError("DestinationExists".into())),
            Err(rustix::io::Errno::NOENT) => (),
            Err(e) => return Err(invalid(e)),
        }
        let mut created = None;
        for _ in 0..128 {
            let ordinal = NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| invalid("staging sequence exhausted"))?;
            let name = format!(".dir-checkpoint-{}-{ordinal}", std::process::id());
            match fs::mkdirat(&parent, name.as_str(), Mode::from_bits_truncate(0o700)) {
                Ok(()) => {
                    created = Some(name);
                    break;
                }
                Err(rustix::io::Errno::EXIST) => (),
                Err(e) => return Err(invalid(e)),
            }
        }
        let name = created.ok_or_else(|| invalid("staging collisions"))?;
        let fd = match fs::openat(&parent, name.as_str(), DIR, Mode::empty()) {
            Ok(fd) => fd,
            Err(e) => {
                let _ = fs::unlinkat(&parent, name.as_str(), AtFlags::REMOVEDIR);
                return Err(invalid(e));
            }
        };
        let mut stage = Staging {
            parent: &parent,
            fd,
            name,
            owned: Vec::new(),
            owned_dirs: Vec::new(),
            published: false,
        };
        for (name, bytes) in files.iter().filter(|(n, _)| n.as_str() != "state.json") {
            write_file(&mut stage, name, bytes)?;
        }
        write_file(&mut stage, "state.json", &files["state.json"])?;
        write_file(&mut stage, "manifest.json", manifest)?;
        // Every nested directory must be synced after its final entry was added.
        let mut directories = BTreeSet::new();
        for name in files.keys() {
            let mut p = Path::new(name).parent();
            while let Some(path) = p {
                if path.as_os_str().is_empty() {
                    break;
                }
                directories.insert(path.to_str().unwrap().to_owned());
                p = path.parent();
            }
        }
        for name in directories.iter().rev() {
            let fd = {
                let (p, n) = file_parent(&stage.fd, name)?;
                fs::openat(p, n.as_str(), DIR, Mode::empty()).map_err(invalid)?
            };
            fs::fsync(fd).map_err(|e| ModelError(format!("DurabilityUnsupported: {e}")))?;
        }
        fs::fsync(&stage.fd).map_err(|e| ModelError(format!("DurabilityUnsupported: {e}")))?;
        #[cfg(test)]
        if IO_FAULT.with(|f| f.get() == Some("before_publish")) {
            return Err(invalid("injected before publication"));
        }
        #[cfg(test)]
        crate::acceptance_hooks::hit("checkpoint.before_publish");
        fs::renameat_with(
            &parent,
            stage.name.as_str(),
            &parent,
            leaf,
            fs::RenameFlags::NOREPLACE,
        )
        .map_err(|e| {
            if e == rustix::io::Errno::EXIST {
                ModelError("DestinationExists".into())
            } else {
                ModelError(format!("DurabilityUnsupported: {e}"))
            }
        })?;
        stage.published = true;
        #[cfg(test)]
        crate::acceptance_hooks::hit("checkpoint.after_publish");
        let sync = fs::fsync(&parent);
        #[cfg(test)]
        let sync = if IO_FAULT.with(|f| f.get() == Some("parent_sync")) {
            Err(rustix::io::Errno::IO)
        } else {
            sync
        };
        Ok(CheckpointReceipt {
            destination: destination.to_owned(),
            manifest_sha256: sha(manifest),
            checkpoint_ordinal: ordinal,
            durability: if sync.is_ok() {
                CheckpointDurability::Durable
            } else {
                CheckpointDurability::PublishedDurabilityUnknown
            },
            diagnostic: sync
                .err()
                .map(|e| format!("parent directory sync failed: {e}")),
        })
    }
    fn read_file(fd: OwnedFd, limit: u64) -> ModelResult<Vec<u8>> {
        let stat = fs::fstat(&fd).map_err(invalid)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_size < 0
            || stat.st_size as u64 > limit
        {
            return Err(invalid("nonregular/hardlinked/oversized bundle file"));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve(usize::try_from(stat.st_size).map_err(invalid)?)
            .map_err(invalid)?;
        File::from(fd)
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(invalid)?;
        if bytes.len() as u64 > limit || bytes.len() as i64 != stat.st_size {
            return Err(invalid("file changed or exceeded read limit"));
        }
        Ok(bytes)
    }
    pub(super) fn read(path: &Path, limits: &CheckpointLimits) -> ModelResult<EncodedBundle> {
        let root = directory(path)?;
        let manifest_fd = fs::openat(
            &root,
            "manifest.json",
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(invalid)?;
        let initial_manifest = read_file(manifest_fd, limits.max_total_bytes)?;
        let manifest_value = strict_json(&initial_manifest)?;
        let expected = declared_files(&manifest_value, initial_manifest.len(), limits)?;
        let mut files = BTreeMap::new();
        let mut dirs = BTreeSet::new();
        let mut total = 0u64;
        fn walk(
            fd: &OwnedFd,
            prefix: &str,
            files: &mut BTreeMap<String, Vec<u8>>,
            dirs: &mut BTreeSet<String>,
            total: &mut u64,
            limits: &CheckpointLimits,
            expected: &BTreeMap<String, u64>,
        ) -> ModelResult {
            let mut entries = fs::Dir::read_from(fd).map_err(invalid)?;
            while let Some(entry) = entries.read() {
                let entry = entry.map_err(invalid)?;
                let name = entry.file_name().to_str().map_err(invalid)?;
                if matches!(name, "." | "..") {
                    continue;
                }
                let path = if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}/{name}")
                };
                if path.len() > 4096 || path.split('/').count() > 64 {
                    return Err(invalid("directory depth/path limit"));
                }
                let stat = fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(invalid)?;
                if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                    if !matches!(path.as_str(), "inputs" | "objects")
                        && !path.starts_with("inputs/")
                    {
                        return Err(invalid("unexpected bundle directory"));
                    }
                    if dirs.len() as u64 >= u64::from(limits.max_files) * 64 {
                        return Err(invalid("directory count limit"));
                    }
                    dirs.insert(path.clone());
                    let child = fs::openat(fd, name, DIR, Mode::empty()).map_err(invalid)?;
                    walk(&child, &path, files, dirs, total, limits, expected)?;
                } else {
                    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
                        || stat.st_nlink != 1
                    {
                        return Err(invalid("nonregular/hardlinked bundle file"));
                    }
                    if path != "manifest.json" {
                        relative_name(&path)?;
                        if !expected.contains_key(&path) {
                            return Err(invalid("unlisted bundle file"));
                        }
                    }
                    if files.len() >= limits.max_files as usize {
                        return Err(invalid("file count limit"));
                    }
                    let limit = limits
                        .max_total_bytes
                        .checked_sub(*total)
                        .ok_or_else(|| invalid("total byte limit"))?;
                    let limit = if path == "manifest.json" {
                        limit
                    } else {
                        limit.min(expected[&path])
                    };
                    let limit = if path.starts_with("objects/") {
                        limit.min(limits.max_blob_bytes)
                    } else {
                        limit
                    };
                    let fd = fs::openat(
                        fd,
                        name,
                        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(invalid)?;
                    let bytes = read_file(fd, limit)?;
                    *total = total
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(|| invalid("total overflow"))?;
                    if files.insert(path, bytes).is_some() {
                        return Err(invalid("duplicate file name"));
                    }
                }
            }
            Ok(())
        }
        walk(
            &root, "", &mut files, &mut dirs, &mut total, limits, &expected,
        )?;
        let expected_dirs: BTreeSet<_> = files
            .keys()
            .flat_map(|name| {
                let parts: Vec<_> = name.split('/').collect();
                (1..parts.len())
                    .map(|n| parts[..n].join("/"))
                    .collect::<Vec<_>>()
            })
            .collect();
        if dirs != expected_dirs {
            return Err(invalid("unexpected empty directory"));
        }
        let manifest = files
            .remove("manifest.json")
            .ok_or_else(|| invalid("missing manifest.json"))?;
        if manifest != initial_manifest {
            return Err(invalid("manifest changed during read"));
        }
        Ok((manifest, files))
    }
}

#[cfg(not(unix))]
mod platform {
    use super::*;
    pub(super) fn write(
        _: &Path,
        _: &[u8],
        _: &BTreeMap<String, Vec<u8>>,
        _: u64,
    ) -> ModelResult<CheckpointReceipt> {
        Err(ModelError("DurabilityUnsupported".into()))
    }
    pub(super) fn read(_: &Path, _: &CheckpointLimits) -> ModelResult<EncodedBundle> {
        Err(ModelError("DurabilityUnsupported".into()))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let name = format!(
                "dir-checkpoint-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let path = std::env::temp_dir().join(name);
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn manifest() -> Value {
        json!({"schema_name":"dir.checkpoint.bundle","schema_version":1,"state_path":"state.json"})
    }
    fn files() -> BTreeMap<String, Vec<u8>> {
        let blob = b"model state".to_vec();
        BTreeMap::from([
            ("state.json".into(), b"{\"fes\":[]}".to_vec()),
            ("inputs/config.ini".into(), b"[General]\n".to_vec()),
            (format!("objects/{}.bin", sha(&blob)), blob),
        ])
    }
    #[test]
    fn checkpoint_bundle_roundtrip_and_no_self_hash() {
        let t = Temp::new();
        let p = t.0.join("saved");
        let r = write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 7).unwrap();
        assert_eq!(r.durability, CheckpointDurability::Durable);
        assert_eq!(r.checkpoint_ordinal, 7);
        let (m, f) = read_bundle(&p, &CheckpointLimits::default()).unwrap();
        assert_eq!(f, files());
        assert_eq!(m["checkpoint_ordinal"], "7");
        assert_eq!(m["files"].as_array().unwrap().len(), 3);
        assert_eq!(
            sha(&fs::read(p.join("manifest.json")).unwrap()),
            r.manifest_sha256
        );
        assert!(
            write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 8)
                .unwrap_err()
                .0
                .contains("DestinationExists")
        );
    }
    #[test]
    fn checkpoint_bundle_prepublication_failure_cleans_only_owned_staging() {
        let t = Temp::new();
        let p = t.0.join("saved");
        fs::write(t.0.join("unrelated"), b"keep").unwrap();
        for fault in ["after_mkdir", "before_publish"] {
            IO_FAULT.with(|f| f.set(Some(fault)));
            let result = write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 3);
            IO_FAULT.with(|f| f.set(None));
            assert!(result.is_err());
            assert!(!p.exists());
            assert_eq!(fs::read_dir(&t.0).unwrap().count(), 1, "{fault}");
        }
        assert_eq!(fs::read(t.0.join("unrelated")).unwrap(), b"keep");
        assert_eq!(
            write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 3)
                .unwrap()
                .checkpoint_ordinal,
            3
        );
    }
    #[test]
    fn checkpoint_bundle_parent_sync_failure_is_published_receipt() {
        let t = Temp::new();
        let p = t.0.join("saved");
        IO_FAULT.with(|f| f.set(Some("parent_sync")));
        let result = write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 4);
        IO_FAULT.with(|f| f.set(None));
        let r = result.unwrap();
        assert_eq!(
            r.durability,
            CheckpointDurability::PublishedDurabilityUnknown
        );
        assert!(r.diagnostic.is_some());
        assert_eq!(r.checkpoint_ordinal, 4);
        assert!(read_bundle(&p, &CheckpointLimits::default()).is_ok());
    }
    #[test]
    fn checkpoint_bundle_rejects_path_escape_and_object_alias() {
        let t = Temp::new();
        for name in [
            "../x",
            "/x",
            "inputs//x",
            "inputs/./x",
            "inputs/../x",
            "inputs/a\\b",
            "manifest.json",
            "objects/not-a-hash.bin",
        ] {
            let mut f = files();
            f.insert(name.into(), vec![]);
            assert!(
                write_bundle(
                    &t.0.join("saved"),
                    manifest(),
                    f,
                    &CheckpointLimits::default(),
                    0
                )
                .is_err(),
                "{name}"
            );
        }
        let mut collision = files();
        collision.insert("inputs/a".into(), vec![]);
        collision.insert("inputs/a/b".into(), vec![]);
        assert!(
            write_bundle(
                &t.0.join("saved"),
                manifest(),
                collision,
                &CheckpointLimits::default(),
                0
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(&t.0).unwrap().count(), 0);
        let mut f = files();
        f.insert(
            format!("objects/{}.bin", "0".repeat(64)),
            b"bad name hash".to_vec(),
        );
        assert!(
            write_bundle(
                &t.0.join("saved"),
                manifest(),
                f,
                &CheckpointLimits::default(),
                0
            )
            .is_err()
        );
    }
    #[test]
    fn checkpoint_bundle_rejects_corruption_missing_and_extra_files() {
        let t = Temp::new();
        for scenario in ["changed", "missing", "extra", "empty_dir"] {
            let p = t.0.join(scenario);
            write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 0).unwrap();
            match scenario {
                "changed" => fs::write(p.join("inputs/config.ini"), b"modified").unwrap(),
                "missing" => fs::remove_file(p.join("inputs/config.ini")).unwrap(),
                "extra" => fs::write(p.join("inputs/extra"), b"extra").unwrap(),
                _ => fs::create_dir(p.join("inputs/empty")).unwrap(),
            }
            assert!(
                read_bundle(&p, &CheckpointLimits::default()).is_err(),
                "{scenario}"
            );
        }
    }
    #[test]
    fn checkpoint_bundle_rejects_symlinks_and_hardlinks() {
        let t = Temp::new();
        let outside = t.0.join("outside");
        fs::write(&outside, b"outside").unwrap();
        for scenario in ["symlink", "hardlink"] {
            let p = t.0.join(scenario);
            write_bundle(&p, manifest(), files(), &CheckpointLimits::default(), 0).unwrap();
            let target = p.join("inputs/config.ini");
            fs::remove_file(&target).unwrap();
            if scenario == "symlink" {
                symlink(&outside, &target).unwrap();
            } else {
                fs::hard_link(&outside, &target).unwrap();
            }
            assert!(read_bundle(&p, &CheckpointLimits::default()).is_err());
            fs::remove_file(&target).unwrap();
        }
        symlink(&t.0, t.0.join("alias")).unwrap();
        assert!(
            write_bundle(
                &t.0.join("alias/saved"),
                manifest(),
                files(),
                &CheckpointLimits::default(),
                0
            )
            .is_err()
        );
    }
    #[test]
    fn checkpoint_bundle_limits_and_strict_json() {
        assert!(strict_json(b"{\"x\":0,\"x\":1}").is_err());
        assert!(strict_json(b"\xef\xbb\xbf{}").is_err());
        assert!(strict_json(format!("{}0{}", "[".repeat(65), "]".repeat(65)).as_bytes()).is_err());
        let limits = CheckpointLimits {
            max_files: 3,
            ..Default::default()
        };
        let t = Temp::new();
        assert!(write_bundle(&t.0.join("x"), manifest(), files(), &limits, 0).is_err());
        let limits = CheckpointLimits {
            max_total_bytes: 200,
            max_blob_bytes: 200,
            ..Default::default()
        };
        assert!(write_bundle(&t.0.join("x"), manifest(), files(), &limits, 0).is_err());
        let limits = CheckpointLimits {
            max_blob_bytes: 1,
            ..Default::default()
        };
        assert!(write_bundle(&t.0.join("x"), manifest(), files(), &limits, 0).is_err());
        let mut f = files();
        f.insert("state.json".into(), b"{\"fes\":[1,2]}".to_vec());
        let limits = CheckpointLimits {
            max_fes_entries: 1,
            ..Default::default()
        };
        assert!(write_bundle(&t.0.join("x"), manifest(), f, &limits, 0).is_err());
    }
}

#[cfg(test)]
pub(crate) fn vector_set_io_fault(fault: Option<&'static str>) {
    IO_FAULT.with(|f| f.set(fault));
}
