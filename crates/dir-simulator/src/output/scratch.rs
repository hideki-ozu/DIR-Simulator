//! Exact ownership of sort spill files; never recursively delete unknown paths.
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
    ops::Deref,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(super) struct Directory {
    path: PathBuf,
    identity: Option<(u64, u64)>,
    files: BTreeMap<PathBuf, (u64, u64)>,
}
impl Directory {
    pub fn create(path: PathBuf) -> io::Result<Self> {
        fs::create_dir(&path)?;
        let metadata = fs::symlink_metadata(&path)?;
        Ok(Self {
            path,
            identity: Some((metadata.dev(), metadata.ino())),
            files: BTreeMap::new(),
        })
    }
    fn check(&self) -> io::Result<()> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.is_dir() && Some((metadata.dev(), metadata.ino())) == self.identity {
            Ok(())
        } else {
            Err(io::Error::other("sort directory ownership changed"))
        }
    }
    pub fn open_new(&mut self, path: &Path) -> io::Result<File> {
        self.check()?;
        if path.parent() != Some(self.path.as_path()) {
            return Err(io::Error::other("sort file outside owned directory"));
        }
        let file = OpenOptions::new().create_new(true).write(true).open(path)?;
        let metadata = file.metadata()?;
        self.files
            .insert(path.to_path_buf(), (metadata.dev(), metadata.ino()));
        Ok(file)
    }
    pub fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        self.check()?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || self.files.get(path) != Some(&(metadata.dev(), metadata.ino())) {
            return Err(io::Error::other("sort file ownership changed"));
        }
        fs::remove_file(path)?;
        self.files.remove(path);
        Ok(())
    }
    pub fn cleanup(&mut self) {
        if self.identity.is_none() || self.check().is_err() {
            return;
        }
        // The map contains only currently live runs, not the full spill history.
        let files = std::mem::take(&mut self.files);
        for (path, identity) in files {
            if fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && (m.dev(), m.ino()) == identity)
            {
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_dir(&self.path);
        self.identity = None;
    }
}
impl Deref for Directory {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_preserves_unknown_and_replaced_entries() {
        let root = std::env::temp_dir().join(format!("dir-owned-spill-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let path = root.join("spill");
        let mut directory = Directory::create(path.clone()).unwrap();
        let owned = path.join("run-0");
        drop(directory.open_new(&owned).unwrap());
        let replaced = path.join("run-1");
        drop(directory.open_new(&replaced).unwrap());
        fs::rename(&replaced, root.join("original")).unwrap();
        fs::write(&replaced, b"replacement").unwrap();
        fs::write(path.join("unknown"), b"foreign").unwrap();
        assert!(directory.remove_file(&replaced).is_err());
        drop(directory);
        assert!(!owned.exists());
        assert_eq!(fs::read(&replaced).unwrap(), b"replacement");
        assert_eq!(fs::read(path.join("unknown")).unwrap(), b"foreign");
        fs::remove_dir_all(root).unwrap();
    }
}
