//! A bounded observation buffer backed by an owned, temporary on-disk journal.
use crate::{Diagnostic, snapshot::Point};
use std::{
    cell::RefCell,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

thread_local! { static DIRECTORY: RefCell<Option<PathBuf>> = const { RefCell::new(None) }; }
static NEXT: AtomicU64 = AtomicU64::new(0);
const BUFFER_LIMIT: usize = 4096;

pub(crate) fn directory() -> Option<PathBuf> {
    DIRECTORY.with(|d| d.borrow().clone())
}

pub(crate) fn with_spooling<T>(directory: &Path, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            DIRECTORY.with(|d| *d.borrow_mut() = self.0.take());
        }
    }
    let previous = DIRECTORY.with(|d| d.replace(Some(directory.to_path_buf())));
    let _restore = Restore(previous);
    run()
}
fn failure(operation: &str, error: impl std::fmt::Display) -> Diagnostic {
    let reason = match operation {
        "create" => "output_create_failed",
        "flush" => "output_flush_failed",
        _ => "output_write_failed",
    };
    Diagnostic::output(format!("Observation spool {operation}: {error}"))
        .with_reason(reason)
        .with_detail("operation", operation)
}
#[derive(Debug)]
pub struct PointSpool {
    path: PathBuf,
    writer: Mutex<File>,
    count: usize,
}
impl Drop for PointSpool {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
impl PointSpool {
    fn create(directory: &Path) -> Result<Self, Diagnostic> {
        for _ in 0..32 {
            let path = directory.join(format!(
                ".dir-observations-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(&path)
            {
                Ok(writer) => {
                    return Ok(Self {
                        path,
                        writer: Mutex::new(writer),
                        count: 0,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(failure("create", e)),
            }
        }
        Err(failure("create", "temporary journal names exhausted"))
    }
    fn append(&mut self, points: &[Point]) -> Result<(), Diagnostic> {
        let writer = self.writer.get_mut().map_err(|e| failure("lock", e))?;
        // Serialize the complete successful callback batch before changing the file.
        let mut buffer = Vec::new();
        for point in points {
            serde_json::to_writer(&mut buffer, point).map_err(|e| failure("encode", e))?;
            buffer.push(b'\n');
        }
        writer.write_all(&buffer).map_err(|e| failure("write", e))?;
        writer.flush().map_err(|e| failure("flush", e))?;
        self.count = self
            .count
            .checked_add(points.len())
            .ok_or_else(|| failure("count", "overflow"))?;
        Ok(())
    }
    fn iter(&self) -> Result<DiskPoints, Diagnostic> {
        Ok(DiskPoints {
            reader: BufReader::new(File::open(&self.path).map_err(|e| failure("read", e))?),
            line: String::new(),
            remaining: self.count,
        })
    }
}
pub struct DiskPoints {
    reader: BufReader<File>,
    line: String,
    remaining: usize,
}
impl Iterator for DiskPoints {
    type Item = Result<Point, Diagnostic>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.line.clear();
        Some(
            self.reader
                .read_line(&mut self.line)
                .map_err(|e| failure("read", e))
                .and_then(|n| {
                    if n == 0 {
                        Err(failure("read", "truncated observation journal"))
                    } else {
                        serde_json::from_str(&self.line).map_err(|e| failure("decode", e))
                    }
                }),
        )
    }
}
pub struct Points<'a> {
    disk: Option<DiskPoints>,
    memory: std::slice::Iter<'a, Point>,
}
impl Iterator for Points<'_> {
    type Item = Result<Point, Diagnostic>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(disk) = self.disk.as_mut() {
            if let Some(point) = disk.next() {
                return Some(point);
            }
            self.disk = None;
        }
        self.memory.next().cloned().map(Ok)
    }
}
impl crate::snapshot::CommonSnapshot {
    pub(crate) fn checkpoint(&mut self) -> bool {
        if self.spool_error.is_some() {
            return false;
        }
        if let Err(diagnostic) = self.spool_checkpoint(false) {
            self.spool_error = Some(diagnostic);
            return false;
        }
        true
    }
    pub fn iter_points(&self) -> Result<Points<'_>, Diagnostic> {
        Ok(Points {
            disk: self.point_spool.as_ref().map(|s| s.iter()).transpose()?,
            memory: self.points.iter(),
        })
    }
    pub fn select_points(
        &self,
        mut filter: impl FnMut(&Point) -> bool,
    ) -> Result<Vec<Point>, Diagnostic> {
        self.iter_points()?
            .filter_map(|p| match p {
                Ok(p) if filter(&p) => Some(Ok(p)),
                Ok(_) => None,
                Err(e) => Some(Err(e)),
            })
            .collect()
    }
    pub(crate) fn spool_checkpoint(&mut self, force: bool) -> Result<(), Diagnostic> {
        let directory = DIRECTORY.with(|d| d.borrow().clone());
        let Some(directory) = directory else {
            return Ok(());
        };
        if self.points.is_empty() || (!force && self.points.len() < BUFFER_LIMIT) {
            return Ok(());
        }
        if self.point_spool.is_none() {
            self.point_spool = Some(Arc::new(PointSpool::create(&directory)?));
        }
        Arc::get_mut(self.point_spool.as_mut().unwrap())
            .ok_or_else(|| failure("write", "journal has multiple owners"))?
            .append(&self.points)?;
        self.points.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_journal_is_reported_and_owned_storage_is_reclaimed() {
        let directory = std::env::temp_dir().join(format!(
            "dir-spool-fault-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let mut spool = PointSpool::create(&directory).unwrap();
        spool
            .append(&[Point {
                event_seq: Some(1),
                effect_seq: Some(0),
                time_ps: 2,
                target: "Main.a".into(),
                metric: "count".into(),
                value: 1,
                request_id: None,
                receiver: None,
                reason: None,
            }])
            .unwrap();
        fs::write(&spool.path, b"").unwrap();
        let diagnostic = spool
            .iter()
            .unwrap()
            .next()
            .unwrap()
            .unwrap_err()
            .normalized(0, true);
        assert_eq!(diagnostic.code, "E-0003");
        assert_eq!(diagnostic.reason, "output_write_failed");
        assert!(diagnostic.message.contains("truncated"));
        let path = spool.path.clone();
        drop(spool);
        assert!(!path.exists());
        fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn unavailable_spool_directory_keeps_output_error_classification() {
        let path = std::env::temp_dir().join(format!(
            "dir-no-spool-directory-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let diagnostic = PointSpool::create(&path).unwrap_err().normalized(0, true);
        assert_eq!(diagnostic.code, "E-0003");
        assert_eq!(diagnostic.reason, "output_create_failed");
        assert!(!path.exists());
    }
}
