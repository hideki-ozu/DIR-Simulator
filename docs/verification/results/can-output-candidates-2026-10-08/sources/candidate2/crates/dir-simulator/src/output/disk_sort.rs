//! Bounded external sort for result rows whose wire order differs from retirement order.
use crate::{Diagnostic, allocation::reserve_vec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};

#[cfg(not(test))]
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
#[cfg(test)]
const CHUNK_BYTES: usize = 1024;
#[cfg(not(test))]
const FAN_IN: usize = 16;
#[cfg(test)]
const FAN_IN: usize = 3;
static NEXT: AtomicU64 = AtomicU64::new(0);

fn error(verb: &str, detail: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::output(format!("Result sort {verb}: {detail}"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Row {
    pub key: Vec<String>,
    pub tie: u64,
    pub value: Value,
}
impl PartialEq for Row {
    fn eq(&self, rhs: &Self) -> bool {
        self.key == rhs.key && self.tie == rhs.tie
    }
}
impl Eq for Row {}
impl PartialOrd for Row {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}
impl Ord for Row {
    fn cmp(&self, rhs: &Self) -> Ordering {
        self.key.cmp(&rhs.key).then(self.tie.cmp(&rhs.tie))
    }
}

/// Owns every run and removes it on both success and failure.
pub(super) struct Sorter {
    directory: PathBuf,
    chunk: Vec<Row>,
    chunk_bytes: usize,
    levels: Vec<Vec<PathBuf>>,
    next: u64,
    file_serial: u64,
}
impl Sorter {
    pub fn new_in(base: &Path) -> Result<Self, Diagnostic> {
        for _ in 0..32 {
            let path = base.join(format!(
                ".dir-result-sort-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        directory: path,
                        chunk: Vec::new(),
                        chunk_bytes: 0,
                        levels: Vec::new(),
                        next: 0,
                        file_serial: 0,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(error("create", e)),
            }
        }
        Err(error("create", "temporary names exhausted"))
    }
    pub fn push(&mut self, key: Vec<String>, value: Value) -> Result<(), Diagnostic> {
        let tie = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| error("count", "overflow"))?;
        let row = Row { key, tie, value };
        let size = serde_json::to_vec(&row)
            .map_err(|e| error("encode", e))?
            .len();
        if !self.chunk.is_empty() && self.chunk_bytes.saturating_add(size) > CHUNK_BYTES {
            self.flush()?;
        }
        reserve_vec(&mut self.chunk, 1, "output_sort_chunk")?;
        self.chunk.push(row);
        self.chunk_bytes = self.chunk_bytes.saturating_add(size);
        Ok(())
    }
    fn path(&mut self) -> Result<PathBuf, Diagnostic> {
        let n = self.file_serial;
        self.file_serial = self
            .file_serial
            .checked_add(1)
            .ok_or_else(|| error("count", "overflow"))?;
        Ok(self.directory.join(format!("run-{n}")))
    }
    fn write_row(writer: &mut BufWriter<File>, row: &Row) -> Result<(), Diagnostic> {
        serde_json::to_writer(&mut *writer, row).map_err(|e| error("write", e))?;
        writer.write_all(b"\n").map_err(|e| error("write", e))
    }
    fn flush(&mut self) -> Result<(), Diagnostic> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        self.chunk.sort();
        let path = self.path()?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| error("create", e))?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        for row in &self.chunk {
            Self::write_row(&mut writer, row)?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        self.promote(path, 0)?;
        self.chunk.clear();
        self.chunk_bytes = 0;
        Ok(())
    }
    fn promote(&mut self, mut path: PathBuf, mut level: usize) -> Result<(), Diagnostic> {
        loop {
            if self.levels.len() <= level {
                reserve_vec(&mut self.levels, 1, "output_sort_levels")?;
                self.levels.push(Vec::new());
            }
            reserve_vec(&mut self.levels[level], 1, "output_sort_runs")?;
            self.levels[level].push(path);
            if self.levels[level].len() < FAN_IN {
                return Ok(());
            }
            let paths = std::mem::take(&mut self.levels[level]);
            let merged = self.merge(&paths)?;
            for old in paths {
                fs::remove_file(old).map_err(|e| error("cleanup", e))?;
            }
            level += 1;
            path = merged;
        }
    }
    fn merge(&mut self, paths: &[PathBuf]) -> Result<PathBuf, Diagnostic> {
        let path = self.path()?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| error("create", e))?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        let mut readers = Vec::new();
        for path in paths {
            reserve_vec(&mut readers, 1, "output_sort_readers")?;
            readers.push(RunReader::open(path)?);
        }
        let mut heads = Vec::new();
        for reader in &mut readers {
            reserve_vec(&mut heads, 1, "output_sort_heads")?;
            heads.push(reader.next_row()?);
        }
        while let Some(index) = heads
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i, r)))
            .min_by(|a, b| a.1.cmp(b.1))
            .map(|(index, _)| index)
        {
            let row = heads[index].take().expect("selected sort head");
            Self::write_row(&mut writer, &row)?;
            heads[index] = readers[index].next_row()?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        Ok(path)
    }
    pub fn finish(mut self) -> Result<Sorted, Diagnostic> {
        self.flush()?;
        let mut pending = Vec::new();
        for level in &mut self.levels {
            reserve_vec(&mut pending, level.len(), "output_sort_pending")?;
            pending.append(level);
        }
        while pending.len() > 1 {
            let prior = std::mem::take(&mut pending);
            for group in prior.chunks(FAN_IN) {
                let path = self.merge(group)?;
                reserve_vec(&mut pending, 1, "output_sort_pending")?;
                pending.push(path);
                for old in group {
                    fs::remove_file(old).map_err(|e| error("cleanup", e))?;
                }
            }
        }
        let path = pending.pop();
        let directory = std::mem::take(&mut self.directory);
        Ok(Sorted { directory, path })
    }
}
impl Drop for Sorter {
    fn drop(&mut self) {
        if !self.directory.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

pub(super) struct Sorted {
    directory: PathBuf,
    path: Option<PathBuf>,
}
impl Sorted {
    pub fn iter(&self) -> Result<Rows, Diagnostic> {
        Ok(Rows(self.path.as_ref().map(RunReader::open).transpose()?))
    }
    #[cfg(test)]
    pub fn is_direct_ordered(&self) -> bool {
        self.path
            .as_ref()
            .is_some_and(|path| path.file_name().is_some_and(|name| name == "rows"))
    }
}
impl Drop for Sorted {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
pub(super) struct Rows(Option<RunReader>);
impl Iterator for Rows {
    type Item = Result<Row, Diagnostic>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .as_mut()
            .and_then(|reader| reader.next_row().transpose())
    }
}

/// Records are normally generated in wire order. Keep the complete original
/// stream until order has been checked, so a late violation can be replayed
/// into the stable external sorter without rerunning aggregation.
pub(super) struct OrderedWriter {
    directory: PathBuf,
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    previous: Option<Vec<String>>,
    disordered: bool,
    next: u64,
}
impl OrderedWriter {
    pub fn new_in(base: &Path) -> Result<Self, Diagnostic> {
        for _ in 0..32 {
            let directory = base.join(format!(
                ".dir-result-ordered-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let path = directory.join("rows");
                    let file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&path)
                        .map_err(|e| error("create", e));
                    return match file {
                        Ok(file) => Ok(Self {
                            directory,
                            path,
                            writer: Some(BufWriter::with_capacity(1024 * 1024, file)),
                            previous: None,
                            disordered: false,
                            next: 0,
                        }),
                        Err(e) => {
                            let _ = fs::remove_dir_all(&directory);
                            Err(e)
                        }
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(error("create", e)),
            }
        }
        Err(error("create", "temporary names exhausted"))
    }
    pub fn push(&mut self, key: Vec<String>, value: Value) -> Result<(), Diagnostic> {
        let tie = self.next;
        let next = tie
            .checked_add(1)
            .ok_or_else(|| error("count", "overflow"))?;
        let row = Row { key, tie, value };
        Sorter::write_row(
            self.writer.as_mut().expect("unfinished ordered writer"),
            &row,
        )?;
        if self
            .previous
            .as_ref()
            .is_some_and(|previous| previous > &row.key)
        {
            self.disordered = true;
        }
        self.previous = Some(row.key);
        self.next = next;
        Ok(())
    }
    pub fn finish(mut self) -> Result<Sorted, Diagnostic> {
        self.writer
            .as_mut()
            .unwrap()
            .flush()
            .map_err(|e| error("flush", e))?;
        self.writer.take();
        if self.disordered {
            let mut sorter = Sorter::new_in(self.directory.parent().unwrap())?;
            let mut reader = RunReader::open(&self.path)?;
            while let Some(row) = reader.next_row()? {
                sorter.push(row.key, row.value)?;
            }
            return sorter.finish();
        }
        let directory = std::mem::take(&mut self.directory);
        let path = std::mem::take(&mut self.path);
        Ok(Sorted {
            directory,
            path: Some(path),
        })
    }
}
impl Drop for OrderedWriter {
    fn drop(&mut self) {
        if !self.directory.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

/// Stable merge of the original point stream followed by the window stream.
pub(super) struct MergedRecords {
    pub points: Sorted,
    pub windows: Sorted,
}
impl MergedRecords {
    pub fn iter(&self) -> Result<MergedRows, Diagnostic> {
        let mut points = self.points.iter()?;
        let mut windows = self.windows.iter()?;
        let point = points.next().transpose()?;
        let window = windows.next().transpose()?;
        Ok(MergedRows {
            points,
            windows,
            point,
            window,
        })
    }
}
pub(super) struct MergedRows {
    points: Rows,
    windows: Rows,
    point: Option<Row>,
    window: Option<Row>,
}
impl Iterator for MergedRows {
    type Item = Result<Row, Diagnostic>;
    fn next(&mut self) -> Option<Self::Item> {
        let choose_point = match (&self.point, &self.window) {
            (Some(point), Some(window)) => point.key <= window.key,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return None,
        };
        let (head, stream) = if choose_point {
            (&mut self.point, &mut self.points)
        } else {
            (&mut self.window, &mut self.windows)
        };
        let row = head.take().expect("selected merge head");
        match stream.next().transpose() {
            Ok(next) => {
                *head = next;
                Some(Ok(row))
            }
            Err(error) => Some(Err(error)),
        }
    }
}
struct RunReader {
    reader: BufReader<File>,
    line: String,
}
impl RunReader {
    fn open(path: &PathBuf) -> Result<Self, Diagnostic> {
        Ok(Self {
            reader: BufReader::with_capacity(
                1024 * 1024,
                File::open(path).map_err(|e| error("read", e))?,
            ),
            line: String::new(),
        })
    }
    fn next_row(&mut self) -> Result<Option<Row>, Diagnostic> {
        self.line.clear();
        let n = self
            .reader
            .read_line(&mut self.line)
            .map_err(|e| error("read", e))?;
        if n == 0 {
            Ok(None)
        } else {
            serde_json::from_str(&self.line)
                .map(Some)
                .map_err(|e| error("decode", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multi_pass_sort_is_stable_and_cleans_owned_files() {
        let base = std::env::temp_dir().join(format!(
            "dir-sort-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut sorter = Sorter::new_in(&base).unwrap();
        for i in 0..200 {
            sorter
                .push(vec![format!("{:03}", (199 - i) / 2)], serde_json::json!(i))
                .unwrap();
        }
        assert!(sorter.file_serial > FAN_IN as u64);
        let sorted = sorter.finish().unwrap();
        let path = sorted.path.clone().unwrap();
        let pairs: Vec<_> = sorted
            .iter()
            .unwrap()
            .map(|row| {
                let row = row.unwrap();
                (row.key[0].clone(), row.value.as_u64().unwrap())
            })
            .collect();
        let mut expected: Vec<_> = (0..200)
            .map(|i| (format!("{:03}", (199 - i) / 2), i as u64))
            .collect();
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(pairs, expected);
        drop(sorted);
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(&base).unwrap();
    }
    #[test]
    fn truncated_run_is_reported_and_removed() {
        let base = std::env::temp_dir().join(format!(
            "dir-sort-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut sorter = Sorter::new_in(&base).unwrap();
        sorter.push(vec!["a".into()], serde_json::json!(1)).unwrap();
        let sorted = sorter.finish().unwrap();
        let path = sorted.path.clone().unwrap();
        fs::write(&path, b"{bad").unwrap();
        assert!(sorted.iter().unwrap().next().unwrap().is_err());
        drop(sorted);
        assert!(!path.exists());
        fs::remove_dir(&base).unwrap();
    }
    #[test]
    fn ordered_records_take_direct_path_and_merge_point_ties_first() {
        let base = std::env::temp_dir().join(format!(
            "dir-ordered-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut points = OrderedWriter::new_in(&base).unwrap();
        points
            .push(vec!["a".into()], serde_json::json!("point-a"))
            .unwrap();
        points
            .push(vec!["b".into()], serde_json::json!("point-b1"))
            .unwrap();
        points
            .push(vec!["b".into()], serde_json::json!("point-b2"))
            .unwrap();
        let mut windows = OrderedWriter::new_in(&base).unwrap();
        windows
            .push(vec!["b".into()], serde_json::json!("window-b"))
            .unwrap();
        windows
            .push(vec!["c".into()], serde_json::json!("window-c"))
            .unwrap();
        assert!(!points.disordered && !windows.disordered);
        let records = MergedRecords {
            points: points.finish().unwrap(),
            windows: windows.finish().unwrap(),
        };
        assert_eq!(
            records.points.path.as_ref().unwrap().file_name().unwrap(),
            "rows"
        );
        let values: Vec<_> = records
            .iter()
            .unwrap()
            .map(|row| row.unwrap().value)
            .collect();
        assert_eq!(
            values,
            vec![
                serde_json::json!("point-a"),
                serde_json::json!("point-b1"),
                serde_json::json!("point-b2"),
                serde_json::json!("window-b"),
                serde_json::json!("window-c"),
            ]
        );
        drop(records);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(&base).unwrap();
    }
    #[test]
    fn late_violation_replays_entire_prefix_with_stable_duplicates() {
        let base = std::env::temp_dir().join(format!(
            "dir-ordered-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut writer = OrderedWriter::new_in(&base).unwrap();
        for i in 0..200 {
            writer
                .push(vec![format!("{:03}", i / 2)], serde_json::json!(i))
                .unwrap();
        }
        let raw_path = writer.path.clone();
        writer
            .push(vec!["000".into()], serde_json::json!(200))
            .unwrap();
        writer
            .push(vec!["050".into()], serde_json::json!(201))
            .unwrap();
        assert!(writer.disordered);
        let sorted = writer.finish().unwrap();
        assert!(!raw_path.exists());
        let values: Vec<_> = sorted
            .iter()
            .unwrap()
            .map(|row| row.unwrap().value.as_u64().unwrap())
            .collect();
        let mut expected: Vec<_> = (0..202u64).collect();
        expected.sort_by_key(|i| {
            if *i == 200 {
                0
            } else if *i == 201 {
                50
            } else {
                i / 2
            }
        });
        assert_eq!(values, expected);
        drop(sorted);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(&base).unwrap();
    }
    #[test]
    fn corrupt_fallback_input_cleans_both_owned_directories() {
        let base = std::env::temp_dir().join(format!(
            "dir-ordered-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut writer = OrderedWriter::new_in(&base).unwrap();
        writer.push(vec!["b".into()], serde_json::json!(1)).unwrap();
        writer.push(vec!["a".into()], serde_json::json!(2)).unwrap();
        writer.writer.as_mut().unwrap().flush().unwrap();
        fs::write(&writer.path, b"{broken\n").unwrap();
        assert!(writer.finish().is_err());
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(&base).unwrap();
    }
}
