//! Owned, sequential CAN ledgers. Runtime handles never refer to archived rows.
use crate::{
    Diagnostic,
    snapshot::{ForwardRecord, Receiver, Request, RequestLineage, RxBufferRecord},
};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    ops::{Index, IndexMut},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One frozen request and its receiver/Gateway rows, independent of runtime handles.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ArchivedRequest {
    pub request: Request,
    pub lineage: RequestLineage,
    pub receivers: Vec<Receiver>,
    /// parent_request is normalized to zero (the request in this bundle).
    pub forwards: Vec<ForwardRecord>,
    pub rx_buffers: Vec<RxBufferRecord>,
}

#[derive(Debug)]
pub struct CanArchive {
    path: PathBuf,
    writer: BufWriter<File>,
    count: usize,
    peak_live_requests: usize,
    peak_live_receivers: usize,
}
fn error(operation: &str, e: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::output(format!("CAN ledger {operation}: {e}")).with_detail("operation", operation)
}
impl CanArchive {
    pub(crate) fn create() -> Result<Option<Self>, Diagnostic> {
        let Some(directory) = super::spool::directory() else {
            return Ok(None);
        };
        for _ in 0..32 {
            let path = directory.join(format!(
                ".dir-can-ledger-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new().create_new(true).write(true).open(&path) {
                Ok(file) => {
                    return Ok(Some(Self {
                        path,
                        writer: BufWriter::with_capacity(1024 * 1024, file),
                        count: 0,
                        peak_live_requests: 0,
                        peak_live_receivers: 0,
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(error("create", e)),
            }
        }
        Err(error("create", "temporary names exhausted"))
    }
    pub(crate) fn append(&mut self, row: &ArchivedRequest) -> Result<(), Diagnostic> {
        let count = self
            .count
            .checked_add(1)
            .ok_or_else(|| error("count", "overflow"))?;
        serde_json::to_writer(&mut self.writer, row).map_err(|e| error("write", e))?;
        self.writer
            .write_all(b"\n")
            .map_err(|e| error("write", e))?;
        self.count = count;
        Ok(())
    }
    pub(crate) fn finish(&mut self) -> Result<(), Diagnostic> {
        self.writer.flush().map_err(|e| error("flush", e))
    }
    pub fn iter(&self) -> Result<ArchivedRequests, Diagnostic> {
        Ok(ArchivedRequests {
            reader: BufReader::with_capacity(
                1024 * 1024,
                File::open(&self.path).map_err(|e| error("read", e))?,
            ),
            line: String::new(),
            remaining: self.count,
        })
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn peak_live_requests(&self) -> usize {
        self.peak_live_requests
    }
    pub fn peak_live_receivers(&self) -> usize {
        self.peak_live_receivers
    }
    pub(crate) fn observe_live(&mut self, requests: usize, receivers: usize) {
        self.peak_live_requests = self.peak_live_requests.max(requests);
        self.peak_live_receivers = self.peak_live_receivers.max(receivers);
    }
}
impl Drop for CanArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
pub struct ArchivedRequests {
    reader: BufReader<File>,
    line: String,
    remaining: usize,
}
impl Iterator for ArchivedRequests {
    type Item = Result<ArchivedRequest, Diagnostic>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.line.clear();
        Some(
            self.reader
                .read_line(&mut self.line)
                .map_err(|e| error("read", e))
                .and_then(|n| {
                    if n == 0 {
                        Err(error("read", "truncated ledger"))
                    } else {
                        serde_json::from_str(&self.line).map_err(|e| error("decode", e))
                    }
                }),
        )
    }
}

/// Stable monotonic handles with storage proportional to live runtime state.
#[derive(Debug)]
pub(super) struct Active<T> {
    pub(super) values: BTreeMap<usize, T>,
    next: usize,
}
impl<T> Default for Active<T> {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            next: 0,
        }
    }
}
impl<T> Active<T> {
    pub(super) fn len(&self) -> usize {
        self.next
    }
    pub(super) fn push(&mut self, value: T) {
        self.values.insert(self.next, value);
        self.next += 1;
    }
    pub(super) fn extend(&mut self, values: impl IntoIterator<Item = T>) {
        for value in values {
            self.push(value);
        }
    }
    pub(super) fn get(&self, id: usize) -> Option<&T> {
        self.values.get(&id)
    }
    pub(super) fn remove(&mut self, id: usize) -> Option<T> {
        self.values.remove(&id)
    }
    pub(super) fn into_vec(self) -> Vec<T> {
        self.values.into_values().collect()
    }
}
impl<T> Index<usize> for Active<T> {
    type Output = T;
    fn index(&self, id: usize) -> &T {
        &self.values[&id]
    }
}
impl<T> IndexMut<usize> for Active<T> {
    fn index_mut(&mut self, id: usize) -> &mut T {
        self.values.get_mut(&id).expect("live runtime handle")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncated_owned_archive_reports_output_error_and_reclaims_storage() {
        let directory = std::env::temp_dir().join(format!(
            "dir-can-archive-fault-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let archive = super::super::spool::with_spooling(&directory, CanArchive::create)
            .unwrap()
            .unwrap();
        let path = archive.path.clone();
        let mut archive = archive;
        archive.count = 1;
        let d = archive.iter().unwrap().next().unwrap().unwrap_err();
        assert_eq!(d.code, "E-0003");
        assert!(d.message.contains("truncated"));
        drop(archive);
        assert!(!path.exists());
        fs::remove_dir(directory).unwrap();
    }
}
