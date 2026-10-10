//! Fixed-width, bounded external sort for CAN metric window contributions.
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
    levels: Vec<Vec<PathBuf>>,
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
        self.promote(path, 0)?;
        self.chunk.clear();
        Ok(())
    }
    fn promote(&mut self, mut path: PathBuf, mut level: usize) -> Result<(), Diagnostic> {
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
                fs::remove_file(old).map_err(|e| error("cleanup", e))?;
            }
            level = level
                .checked_add(1)
                .ok_or_else(|| error("level", "overflow"))?;
            path = merged;
        }
    }
    fn merge(&mut self, paths: &[PathBuf]) -> Result<PathBuf, Diagnostic> {
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
            readers.push(RunReader::open(source)?);
        }
        for reader in &mut readers {
            reserve_vec(&mut heads, 1, "output_contribution_heads")?;
            heads.push(reader.next_row()?);
        }
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
            writer
                .write_all(&row.encode())
                .map_err(|e| error("write", e))?;
            heads[index] = readers[index].next_row()?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        Ok(path)
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
        Ok(Rows(self.path.as_deref().map(RunReader::open).transpose()?))
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
}
impl RunReader {
    fn open(path: &Path) -> Result<Self, Diagnostic> {
        Ok(Self {
            reader: BufReader::new(File::open(path).map_err(|e| error("read", e))?),
        })
    }
    fn next_row(&mut self) -> Result<Option<Row>, Diagnostic> {
        let mut bytes = [0; ROW_BYTES];
        match self
            .reader
            .read(&mut bytes[..1])
            .map_err(|e| error("read", e))?
        {
            0 => Ok(None),
            1 => {
                self.reader
                    .read_exact(&mut bytes[1..])
                    .map_err(|e| error("read", e))?;
                Row::decode(bytes).map(Some)
            }
            _ => unreachable!(),
        }
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
        assert!(sorted.iter().unwrap().next().unwrap().is_err());
        drop(sorted);
        assert!(!path.exists());
        fs::remove_dir(base).unwrap();
    }
}
