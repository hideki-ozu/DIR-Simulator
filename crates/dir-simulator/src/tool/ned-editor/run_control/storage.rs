//! Owned descriptor-relative run storage. No logical input path is opened here.
use super::super::{EditorError, Result, random_id};
use rustix::fs::{self as fs, AtFlags, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

pub(crate) fn invalid(message: impl Into<String>) -> EditorError {
    EditorError::new("E-EDITOR-RUN-INTEGRITY", message, 422)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
    pub dev: u64,
    pub ino: u64,
}
fn identity(stat: &fs::Stat) -> Identity {
    Identity {
        dev: stat.st_dev,
        ino: stat.st_ino,
    }
}
fn name(name: &OsStr) -> Result<()> {
    if name.as_bytes().is_empty()
        || name.as_bytes().contains(&0)
        || name.as_bytes().contains(&b'/')
        || name == "."
        || name == ".."
    {
        return Err(invalid("Invalid storage component"));
    }
    Ok(())
}
pub(crate) struct Directory {
    path: PathBuf,
    chain: Vec<(File, Identity, OsString)>,
    uid: u32,
}
impl Directory {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::CurDir | Component::Prefix(_)
                )
            })
        {
            return Err(invalid("Expected absolute normalized storage directory"));
        }
        let root: File = fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| EditorError::io(path, e))?
        .into();
        let uid = std::fs::metadata("/proc/self")
            .map_err(|e| EditorError::io(path, e))?
            .uid();
        let id = identity(&fs::fstat(&root).map_err(|e| EditorError::io(path, e))?);
        let mut this = Self {
            path: path.into(),
            chain: vec![(root, id, OsString::from("/"))],
            uid,
        };
        for c in path.components() {
            if let Component::Normal(n) = c {
                let f: File = fs::openat(
                    this.fd(),
                    n,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| EditorError::io(path, e))?
                .into();
                let stat = fs::fstat(&f).map_err(|e| EditorError::io(path, e))?;
                this.chain.push((f, identity(&stat), n.to_owned()));
            }
        }
        this.verify()?;
        let st = fs::fstat(this.fd()).map_err(|e| EditorError::io(path, e))?;
        if st.st_uid != uid || st.st_mode & 0o077 != 0 {
            return Err(invalid(
                "Run directory must be owned by this user with mode 0700",
            ));
        }
        Ok(this)
    }
    pub(crate) fn ensure(path: &Path) -> Result<Self> {
        // Creation walks existing ancestors without following symlinks. Ancestors
        // shared with other applications need not have private modes.
        if !path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::CurDir | Component::Prefix(_)
                )
            })
        {
            return Err(invalid("Invalid state path"));
        }
        let mut parent: File = fs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| EditorError::io(path, e))?
        .into();
        for c in path.components() {
            if let Component::Normal(n) = c {
                match fs::openat(
                    &parent,
                    n,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => parent = fd.into(),
                    Err(e) if e == rustix::io::Errno::NOENT => {
                        fs::mkdirat(&parent, n, Mode::from_bits_truncate(0o700))
                            .map_err(|e| EditorError::io(path, e))?;
                        fs::fsync(&parent).map_err(|e| EditorError::io(path, e))?;
                        parent = fs::openat(
                            &parent,
                            n,
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        )
                        .map_err(|e| EditorError::io(path, e))?
                        .into();
                    }
                    Err(e) => return Err(EditorError::io(path, e)),
                }
            }
        }
        Self::open(path)
    }
    pub(crate) fn fd(&self) -> &File {
        &self.chain.last().expect("root descriptor").0
    }
    pub(crate) fn identity(&self) -> Identity {
        self.chain.last().expect("root descriptor").1.clone()
    }
    pub(crate) fn verify(&self) -> Result<()> {
        for pair in self.chain.windows(2) {
            let st = fs::statat(&pair[0].0, &pair[1].2, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|e| EditorError::io(&self.path, e))?;
            if FileType::from_raw_mode(st.st_mode) != FileType::Directory
                || identity(&st) != pair[1].1
            {
                return Err(invalid("Directory identity changed"));
            }
        }
        Ok(())
    }
    pub(crate) fn sync(&self) -> Result<()> {
        self.verify()?;
        fs::fsync(self.fd()).map_err(|e| EditorError::io(&self.path, e))
    }
    pub(crate) fn names(&self) -> Result<Vec<OsString>> {
        self.verify()?;
        let mut stream =
            fs::Dir::read_from(self.fd()).map_err(|e| EditorError::io(&self.path, e))?;
        let mut result = Vec::new();
        while let Some(entry) = stream.read() {
            let entry = entry.map_err(|e| EditorError::io(&self.path, e))?;
            let n = entry.file_name().to_bytes();
            if n != b"." && n != b".." {
                result.push(OsStr::from_bytes(n).to_owned());
            }
        }
        result.sort();
        Ok(result)
    }
    pub(crate) fn open_file(&self, n: impl AsRef<OsStr>) -> Result<File> {
        let n = n.as_ref();
        name(n)?;
        self.verify()?;
        let f: File = fs::openat(
            self.fd(),
            n,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|e| EditorError::io(&self.path.join(n), e))?
        .into();
        self.check_file(&f)?;
        Ok(f)
    }
    fn check_file(&self, f: &File) -> Result<()> {
        let s = fs::fstat(f).map_err(|e| EditorError::io(&self.path, e))?;
        if FileType::from_raw_mode(s.st_mode) != FileType::RegularFile
            || s.st_uid != self.uid
            || s.st_nlink != 1
            || s.st_mode & 0o022 != 0
        {
            return Err(invalid("Non-owned, writable or linked storage file"));
        }
        Ok(())
    }
    pub(crate) fn read(&self, n: impl AsRef<OsStr>, max: usize) -> Result<Vec<u8>> {
        let f = self.open_file(n)?;
        let before = f.metadata().map_err(|e| EditorError::io(&self.path, e))?;
        if before.len() > max as u64 {
            return Err(invalid("Storage file exceeds bound"));
        }
        let mut data = Vec::new();
        (&f).take(max as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|e| EditorError::io(&self.path, e))?;
        let after = f.metadata().map_err(|e| EditorError::io(&self.path, e))?;
        if data.len() > max
            || before.len() != after.len()
            || before.mtime() != after.mtime()
            || before.mtime_nsec() != after.mtime_nsec()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(invalid("Storage file changed while reading"));
        }
        self.check_file(&f)?;
        self.verify()?;
        Ok(data)
    }
    pub(crate) fn create_output(&self, n: impl AsRef<OsStr>) -> Result<File> {
        let n = n.as_ref();
        name(n)?;
        self.verify()?;
        let file: File = fs::openat(
            self.fd(),
            n,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|e| EditorError::io(&self.path.join(n), e))?
        .into();
        self.check_file(&file)?;
        file.sync_all()
            .map_err(|e| EditorError::io(&self.path, e))?;
        self.sync()?;
        Ok(file)
    }
    pub(crate) fn create(&self, n: impl AsRef<OsStr>, bytes: &[u8]) -> Result<()> {
        let n = n.as_ref();
        name(n)?;
        self.verify()?;
        let mut f: File = fs::openat(
            self.fd(),
            n,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(|e| EditorError::io(&self.path.join(n), e))?
        .into();
        f.write_all(bytes)
            .map_err(|e| EditorError::io(&self.path, e))?;
        f.sync_all().map_err(|e| EditorError::io(&self.path, e))?;
        self.sync()
    }
    pub(crate) fn replace(&self, n: impl AsRef<OsStr>, bytes: &[u8]) -> Result<()> {
        let n = n.as_ref();
        name(n)?;
        if self.names()?.iter().any(|known| known == n) {
            self.open_file(n)?;
        }
        let tmp = random_id(".write-")?;
        self.create(&tmp, bytes)?;
        self.verify()?;
        fs::renameat(self.fd(), &tmp, self.fd(), n).map_err(|e| EditorError::io(&self.path, e))?;
        self.sync()
    }
    pub(crate) fn mkdir(&self, n: impl AsRef<OsStr>) -> Result<Self> {
        let n = n.as_ref();
        name(n)?;
        self.verify()?;
        fs::mkdirat(self.fd(), n, Mode::from_bits_truncate(0o700))
            .map_err(|e| EditorError::io(&self.path.join(n), e))?;
        self.sync()?;
        Self::open(&self.path.join(n))
    }
    pub(crate) fn child(&self, n: impl AsRef<OsStr>) -> Result<Self> {
        let n = n.as_ref();
        name(n)?;
        self.verify()?;
        Self::open(&self.path.join(n))
    }
    pub(crate) fn bytes(&self) -> Result<u64> {
        self.verify()?;
        usage_bytes(self.fd(), self.uid, &mut std::collections::BTreeSet::new())
    }
}

impl Directory {
    pub(crate) fn rename_child(&self, old: &str, new: &str, expected: &Identity) -> Result<()> {
        name(OsStr::new(old))?;
        name(OsStr::new(new))?;
        if self.child(old)?.identity() != *expected {
            return Err(invalid("Publication identity mismatch"));
        }
        self.verify()?;
        fs::renameat_with(self.fd(), old, self.fd(), new, fs::RenameFlags::NOREPLACE)
            .map_err(|e| EditorError::io(&self.path, e))?;
        #[cfg(test)]
        if FAIL_PUBLICATION_SYNC.with(|fail| fail.replace(false)) {
            return Err(invalid("Injected post-rename fsync failure"));
        }
        self.sync()
    }
}
#[cfg(test)]
thread_local! { pub(crate) static FAIL_PUBLICATION_SYNC:std::cell::Cell<bool>=const {std::cell::Cell::new(false)}; }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OwnedEntry {
    pub path: String,
    pub identity: Identity,
    pub directory: bool,
    pub bytes: u64,
}
impl Directory {
    pub(crate) fn inventory(&self) -> Result<Vec<OwnedEntry>> {
        let mut entries = Vec::new();
        let mut links = std::collections::BTreeMap::<(u64, u64), (u64, u64)>::new();
        self.scan("", &mut entries, &mut links)?;
        if links.values().any(|(observed, actual)| observed != actual) {
            return Err(invalid("Hard link escapes the owned inventory"));
        }
        Ok(entries)
    }
    fn scan(
        &self,
        prefix: &str,
        entries: &mut Vec<OwnedEntry>,
        links: &mut std::collections::BTreeMap<(u64, u64), (u64, u64)>,
    ) -> Result<()> {
        for n in self.names()? {
            let raw = n
                .to_str()
                .ok_or_else(|| invalid("Unknown non-UTF8 storage entry"))?;
            let path = if prefix.is_empty() {
                raw.to_owned()
            } else {
                format!("{prefix}/{raw}")
            };
            let st = fs::statat(self.fd(), &n, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|e| EditorError::io(&self.path, e))?;
            let kind = FileType::from_raw_mode(st.st_mode);
            if st.st_uid != self.uid
                || st.st_mode & 0o022 != 0
                || !matches!(kind, FileType::Directory | FileType::RegularFile)
            {
                return Err(invalid("Non-owned inventory entry"));
            }
            let directory = kind == FileType::Directory;
            entries.push(OwnedEntry {
                path: path.clone(),
                identity: identity(&st),
                directory,
                bytes: if directory {
                    0
                } else {
                    st.st_size
                        .try_into()
                        .map_err(|_| invalid("Negative file size"))?
                },
            });
            if directory {
                self.child(&n)?.scan(&path, entries, links)?;
            } else {
                let pair = links
                    .entry((st.st_dev, st.st_ino))
                    .or_insert((0, st.st_nlink));
                if pair.1 != st.st_nlink {
                    return Err(invalid("Inventory link count changed"));
                }
                pair.0 += 1;
            }
        }
        Ok(())
    }
    /// Unknown descendants and inode substitutions are retained even after a
    /// partially completed previous deletion. Missing inventoried files are okay.
    pub(crate) fn remove_child_checked(
        &self,
        n: &str,
        expected: &Identity,
        inventory: &[OwnedEntry],
    ) -> Result<()> {
        let child = self.child(n)?;
        if child.identity() != *expected {
            return Err(invalid("Cleanup identity mismatch"));
        }
        let mut current = child.inventory()?;
        for entry in &current {
            if !inventory.iter().any(|known| {
                known.path == entry.path
                    && known.identity == entry.identity
                    && known.directory == entry.directory
            }) {
                return Err(invalid(format!("Unknown cleanup entry: {}", entry.path)));
            }
        }
        let mut counts = std::collections::BTreeMap::<(u64, u64), u64>::new();
        for e in current.iter().filter(|e| !e.directory) {
            *counts.entry((e.identity.dev, e.identity.ino)).or_default() += 1;
        }
        current.sort_by(|a, b| {
            b.path
                .split('/')
                .count()
                .cmp(&a.path.split('/').count())
                .then_with(|| b.path.cmp(&a.path))
        });
        for entry in current {
            let path = Path::new(&entry.path);
            let parent = child.child_relative(path.parent().unwrap_or(Path::new("")))?;
            parent.verify()?;
            let name = path
                .file_name()
                .ok_or_else(|| invalid("Invalid inventory path"))?;
            let st = fs::statat(parent.fd(), name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|e| EditorError::io(&parent.path, e))?;
            if identity(&st) != entry.identity {
                return Err(invalid("Cleanup entry identity changed"));
            }
            if !entry.directory {
                let remaining = counts
                    .get_mut(&(entry.identity.dev, entry.identity.ino))
                    .unwrap();
                if st.st_nlink != *remaining {
                    return Err(invalid("Cleanup hard link count changed"));
                }
                *remaining -= 1;
            }
            fs::unlinkat(
                parent.fd(),
                name,
                if entry.directory {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                },
            )
            .map_err(|e| EditorError::io(&parent.path, e))?;
            parent.sync()?;
        }
        self.verify()?;
        fs::unlinkat(self.fd(), n, AtFlags::REMOVEDIR)
            .map_err(|e| EditorError::io(&self.path, e))?;
        self.sync()
    }
    fn child_relative(&self, path: &Path) -> Result<Self> {
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(invalid("Invalid inventory parent"));
        }
        Self::open(&self.path.join(path))
    }
}

pub(crate) fn hash_file(file: &File) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::FileExt;
    let mut h = Sha256::new();
    let mut b = [0; 65536];
    let mut offset = 0;
    loop {
        let n = file
            .read_at(&mut b, offset)
            .map_err(|e| invalid(e.to_string()))?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
        offset += n as u64;
    }
    Ok(format!("{:x}", h.finalize()))
}

// Soft quota observation reads metadata only. Common export creates/removes
// staging links while running: disappearance is ordinary, and one inode is
// charged once. Downloads/cleanup still use the strict authenticated inventory.
fn usage_bytes(
    fd: &File,
    uid: u32,
    seen: &mut std::collections::BTreeSet<(u64, u64)>,
) -> Result<u64> {
    let mut stream = fs::Dir::read_from(fd).map_err(|e| invalid(e.to_string()))?;
    let mut total = 0u64;
    while let Some(entry) = stream.read() {
        let entry = entry.map_err(|e| invalid(e.to_string()))?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let n = OsStr::from_bytes(bytes);
        let stat = match fs::statat(fd, n, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(s) => s,
            Err(e) if e == rustix::io::Errno::NOENT => continue,
            Err(e) => return Err(invalid(e.to_string())),
        };
        if stat.st_uid != uid || stat.st_mode & 0o022 != 0 {
            return Err(invalid("Non-owned quota entry"));
        }
        let size = match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile => {
                if seen.insert((stat.st_dev, stat.st_ino)) {
                    u64::try_from(stat.st_size).map_err(|_| invalid("Negative quota size"))?
                } else {
                    0
                }
            }
            FileType::Directory => {
                let child: File = match fs::openat(
                    fd,
                    n,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(f) => f.into(),
                    Err(e) if e == rustix::io::Errno::NOENT => continue,
                    Err(e) => return Err(invalid(e.to_string())),
                };
                let observed = fs::fstat(&child).map_err(|e| invalid(e.to_string()))?;
                if observed.st_uid != uid || observed.st_mode & 0o077 != 0 {
                    return Err(invalid("Non-private quota directory"));
                }
                usage_bytes(&child, uid, seen)?
            }
            _ => return Err(invalid("Unexpected quota object")),
        };
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("Quota size overflow"))?;
    }
    Ok(total)
}
