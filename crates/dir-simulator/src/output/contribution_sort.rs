//! Fixed-width, bounded external sort for CAN metric window contributions.
use super::disk_sort::Run;
use crate::{Diagnostic, allocation::reserve_vec};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(not(test))]
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
#[cfg(test)]
const CHUNK_BYTES: usize = 1024;
#[cfg(not(test))]
const FAN_IN: usize = 16;
#[cfg(test)]
const FAN_IN: usize = 3;
const ROW_BYTES: usize = 8 + 8 + 8 + 1 + 16;
static NEXT: AtomicU64 = AtomicU64::new(0);

fn error(verb: &str, detail: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::output(format!("Contribution sort {verb}: {detail}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum Kind {
    Busy,
    Frame,
    Queue,
    Payload,
    Serialized,
    Occupied,
    Received,
}
impl Kind {
    fn code(self) -> u8 {
        self as u8
    }
    fn from_code(code: u8) -> Result<Self, Diagnostic> {
        match code {
            0 => Ok(Self::Busy),
            1 => Ok(Self::Frame),
            2 => Ok(Self::Queue),
            3 => Ok(Self::Payload),
            4 => Ok(Self::Serialized),
            5 => Ok(Self::Occupied),
            6 => Ok(Self::Received),
            _ => Err(error("decode", "invalid contribution kind")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Row {
    pub time: u64,
    pub tie: u64,
    pub target: u64,
    pub kind: Kind,
    pub amount: i128,
}
impl Row {
    fn encode(self) -> [u8; ROW_BYTES] {
        let mut bytes = [0; ROW_BYTES];
        bytes[0..8].copy_from_slice(&self.time.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.tie.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.target.to_le_bytes());
        bytes[24] = self.kind.code();
        bytes[25..41].copy_from_slice(&self.amount.to_le_bytes());
        bytes
    }
    fn decode(bytes: [u8; ROW_BYTES]) -> Result<Self, Diagnostic> {
        Ok(Self {
            time: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            tie: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            target: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            kind: Kind::from_code(bytes[24])?,
            amount: i128::from_le_bytes(bytes[25..41].try_into().unwrap()),
        })
    }
    fn key(self) -> (u64, u64) {
        (self.time, self.tie)
    }
}

/// Owns all intermediate runs, including partially written files on errors.
pub(super) struct Sorter {
    directory: PathBuf,
    chunk: Vec<Row>,
    levels: Vec<Vec<Run>>,
    next: u64,
    file_serial: u64,
}
impl Sorter {
    pub fn new_in(base: &Path) -> Result<Self, Diagnostic> {
        for _ in 0..32 {
            let path = base.join(format!(
                ".dir-contribution-sort-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        directory: path,
                        chunk: Vec::new(),
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
    pub fn push(
        &mut self,
        time: u64,
        target: u64,
        kind: Kind,
        amount: i128,
    ) -> Result<(), Diagnostic> {
        let tie = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| error("count", "overflow"))?;
        if self.chunk.len() >= CHUNK_BYTES / std::mem::size_of::<Row>() {
            self.flush()?;
        }
        reserve_vec(&mut self.chunk, 1, "output_contribution_chunk")?;
        self.chunk.push(Row {
            time,
            tie,
            target,
            kind,
            amount,
        });
        Ok(())
    }
    fn path(&mut self) -> Result<PathBuf, Diagnostic> {
        let n = self.file_serial;
        self.file_serial = n.checked_add(1).ok_or_else(|| error("count", "overflow"))?;
        Ok(self.directory.join(format!("run-{n}")))
    }
    fn flush(&mut self) -> Result<(), Diagnostic> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        self.chunk.sort_unstable_by_key(|row| row.key());
        let path = self.path()?;
        let mut writer = BufWriter::new(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|e| error("create", e))?,
        );
        for row in &self.chunk {
            writer
                .write_all(&row.encode())
                .map_err(|e| error("write", e))?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        let bytes = u64::try_from(self.chunk.len())
            .ok()
            .and_then(|count| count.checked_mul(ROW_BYTES as u64))
            .ok_or_else(|| error("write", "run length overflow"))?;
        self.promote(Run { path, bytes }, 0)?;
        self.chunk.clear();
        Ok(())
    }
    fn promote(&mut self, mut path: Run, mut level: usize) -> Result<(), Diagnostic> {
        loop {
            if self.levels.len() <= level {
                reserve_vec(&mut self.levels, 1, "output_contribution_levels")?;
                self.levels.push(Vec::new());
            }
            reserve_vec(&mut self.levels[level], 1, "output_contribution_runs")?;
            self.levels[level].push(path);
            if self.levels[level].len() < FAN_IN {
                return Ok(());
            }
            let paths = std::mem::take(&mut self.levels[level]);
            let merged = self.merge(&paths)?;
            for old in paths {
                fs::remove_file(&old.path).map_err(|e| error("cleanup", e))?;
            }
            level = level
                .checked_add(1)
                .ok_or_else(|| error("level", "overflow"))?;
            path = merged;
        }
    }
    fn merge(&mut self, paths: &[Run]) -> Result<Run, Diagnostic> {
        let path = self.path()?;
        let mut writer = BufWriter::new(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|e| error("create", e))?,
        );
        let mut readers = Vec::new();
        let mut heads = Vec::new();
        for source in paths {
            reserve_vec(&mut readers, 1, "output_contribution_readers")?;
            readers.push(RunReader::open(&source.path, source.bytes)?);
        }
        for reader in &mut readers {
            reserve_vec(&mut heads, 1, "output_contribution_heads")?;
            heads.push(reader.next_row()?);
        }
        let mut bytes = 0u64;
        while let Some(index) = heads
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.map(|r| (i, r)))
            .min_by_key(|(_, r)| r.key())
            .map(|(i, _)| i)
        {
            let row = heads[index]
                .take()
                .ok_or_else(|| error("merge", "missing head"))?;
            bytes = bytes
                .checked_add(ROW_BYTES as u64)
                .ok_or_else(|| error("write", "run length overflow"))?;
            writer
                .write_all(&row.encode())
                .map_err(|e| error("write", e))?;
            heads[index] = readers[index].next_row()?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        Ok(Run { path, bytes })
    }
    pub fn finish(mut self) -> Result<Sorted, Diagnostic> {
        self.flush()?;
        let mut pending = Vec::new();
        for level in &mut self.levels {
            reserve_vec(&mut pending, level.len(), "output_contribution_pending")?;
            pending.append(level);
        }
        while pending.len() > 1 {
            let prior = std::mem::take(&mut pending);
            for group in prior.chunks(FAN_IN) {
                let path = self.merge(group)?;
                reserve_vec(&mut pending, 1, "output_contribution_pending")?;
                pending.push(path);
                for old in group {
                    fs::remove_file(&old.path).map_err(|e| error("cleanup", e))?;
                }
            }
        }
        let run = pending.pop();
        let bytes = run.as_ref().map_or(0, |run| run.bytes);
        let path = run.map(|run| run.path);
        let directory = std::mem::take(&mut self.directory);
        Ok(Sorted {
            directory,
            path,
            bytes,
        })
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
    bytes: u64,
}
impl Sorted {
    pub fn iter(&self) -> Result<Rows, Diagnostic> {
        Ok(Rows(
            self.path
                .as_deref()
                .map(|path| RunReader::open(path, self.bytes))
                .transpose()?,
        ))
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
struct RunReader {
    reader: BufReader<File>,
    remaining: u64,
}
impl RunReader {
    fn open(path: &Path, expected_bytes: u64) -> Result<Self, Diagnostic> {
        let file = File::open(path).map_err(|e| error("read", e))?;
        if file.metadata().map_err(|e| error("read", e))?.len() != expected_bytes {
            return Err(error("read", "run length differs from producer"));
        }
        Ok(Self {
            reader: BufReader::new(file),
            remaining: expected_bytes,
        })
    }
    fn next_row(&mut self) -> Result<Option<Row>, Diagnostic> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if self.remaining < ROW_BYTES as u64 {
            return Err(error("decode", "truncated contribution record"));
        }
        let mut bytes = [0; ROW_BYTES];
        self.reader
            .read_exact(&mut bytes)
            .map_err(|e| error("read", e))?;
        self.remaining -= ROW_BYTES as u64;
        Row::decode(bytes).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_is_fixed_and_rejects_invalid_kind() {
        let row = Row {
            time: 7,
            tie: 3,
            target: 2,
            kind: Kind::Queue,
            amount: -123,
        };
        let mut bytes = row.encode();
        assert_eq!(bytes.len(), ROW_BYTES);
        assert_eq!(Row::decode(bytes).unwrap(), row);
        bytes[24] = 255;
        assert!(Row::decode(bytes).is_err());
    }
    #[test]
    fn same_time_order_survives_multiple_runs_and_truncation_is_reported() {
        let base = std::env::temp_dir().join(format!(
            "dir-contribution-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut sorter = Sorter::new_in(&base).unwrap();
        for i in 0..200 {
            sorter
                .push((199 - i) as u64 % 4, 0, Kind::Queue, i)
                .unwrap();
        }
        assert!(sorter.file_serial > FAN_IN as u64);
        let sorted = sorter.finish().unwrap();
        let rows: Vec<_> = sorted.iter().unwrap().map(Result::unwrap).collect();
        assert!(rows.windows(2).all(|pair| pair[0].key() < pair[1].key()));
        assert_eq!(
            rows.iter()
                .filter(|r| r.time == 0)
                .map(|r| r.amount)
                .collect::<Vec<_>>(),
            (0..200).filter(|i| (199 - i) % 4 == 0).collect::<Vec<_>>()
        );
        let path = sorted.path.clone().unwrap();
        fs::write(&path, &rows[0].encode()[..ROW_BYTES - 1]).unwrap();
        assert!(
            sorted
                .iter()
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
                .is_err()
        );
        drop(sorted);
        assert!(!path.exists());
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_fixed_rows_reject_every_partial_record_in_reader_and_merge() {
        let base = std::env::temp_dir().join(format!(
            "dir-contribution-pr37-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let rows = [
            Row {
                time: 0,
                tie: 0,
                target: 0,
                kind: Kind::Busy,
                amount: 1,
            },
            Row {
                time: 1,
                tie: 1,
                target: 0,
                kind: Kind::Busy,
                amount: -1,
            },
        ];
        let data = [rows[0].encode(), rows[1].encode()].concat();
        for cut in 0..=data.len() {
            let mut sorter = Sorter::new_in(&base).unwrap();
            let path = sorter.path().unwrap();
            fs::write(&path, &data[..cut]).unwrap();
            let sorted = Sorted {
                directory: PathBuf::new(),
                path: Some(path.clone()),
                bytes: cut as u64,
            };
            let result = sorted.iter().unwrap().collect::<Result<Vec<_>, _>>();
            let merged = sorter.merge(&[Run {
                path,
                bytes: cut as u64,
            }]);
            if cut % ROW_BYTES == 0 {
                assert_eq!(result.unwrap(), rows[..cut / ROW_BYTES], "reader cut={cut}");
                let mut reader = {
                    let merged = merged.unwrap();
                    RunReader::open(&merged.path, merged.bytes)
                }
                .unwrap();
                let mut actual = Vec::new();
                while let Some(row) = reader.next_row().unwrap() {
                    actual.push(row);
                }
                assert_eq!(actual, rows[..cut / ROW_BYTES], "merge cut={cut}");
            } else {
                assert_eq!(
                    result
                        .expect_err(&format!("reader accepted cut={cut}"))
                        .code,
                    "E-0003"
                );
                assert_eq!(
                    merged.expect_err(&format!("merge accepted cut={cut}")).code,
                    "E-0003"
                );
            }
            drop(sorted);
            drop(sorter);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0, "cut={cut}");
        }
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_finalized_contribution_producer_rejects_whole_row_loss() {
        let base = std::env::temp_dir().join(format!(
            "dir-pr37-contrib-final-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut sorter = Sorter::new_in(&base).unwrap();
        sorter.push(0, 0, Kind::Busy, 1).unwrap();
        sorter.push(1, 0, Kind::Busy, -1).unwrap();
        let sorted = sorter.finish().unwrap();
        OpenOptions::new()
            .write(true)
            .open(sorted.path.as_ref().unwrap())
            .unwrap()
            .set_len(ROW_BYTES as u64)
            .unwrap();
        assert_eq!(
            sorted
                .iter()
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
                .expect_err("contribution output silently lost a complete row")
                .code,
            "E-0003"
        );
        drop(sorted);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_intermediate_contribution_merge_rejects_whole_row_loss() {
        let base = std::env::temp_dir().join(format!(
            "dir-pr37-contrib-merge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut sorter = Sorter::new_in(&base).unwrap();
        sorter.push(0, 0, Kind::Busy, 1).unwrap();
        sorter.push(1, 0, Kind::Busy, -1).unwrap();
        sorter.flush().unwrap();
        let run = sorter.levels[0][0].clone();
        OpenOptions::new()
            .write(true)
            .open(&run.path)
            .unwrap()
            .set_len(ROW_BYTES as u64)
            .unwrap();
        assert_eq!(
            sorter
                .merge(&[run])
                .expect_err("contribution merge silently lost a complete row")
                .code,
            "E-0003"
        );
        drop(sorter);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_contribution_reader_rejects_truncation_after_open() {
        let base = std::env::temp_dir().join(format!(
            "dir-pr37-contrib-open-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let mut sorter = Sorter::new_in(&base).unwrap();
        for i in 0..1024u64 {
            sorter.push(i, 0, Kind::Queue, i as i128).unwrap();
        }
        let sorted = sorter.finish().unwrap();
        assert!(sorted.bytes > 4 * 8192);
        let mut reader = sorted.iter().unwrap();
        assert!(reader.next().unwrap().is_ok());
        OpenOptions::new()
            .write(true)
            .open(sorted.path.as_ref().unwrap())
            .unwrap()
            .set_len(ROW_BYTES as u64)
            .unwrap();
        let mut read = 1;
        loop {
            match reader
                .next()
                .expect("unexpected clean EOF after late shrink")
            {
                Ok(_) => read += 1,
                Err(error) => {
                    assert_eq!(error.code, "E-0003");
                    break;
                }
            }
        }
        assert!(read < 1024);
        drop(reader);
        drop(sorted);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
}
