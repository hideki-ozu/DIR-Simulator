//! Bounded external sort for result rows whose wire order differs from retirement order.
use crate::{Diagnostic, allocation::reserve_vec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
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
const MAGIC: &[u8; 8] = b"DIRSORT1";
const FRAME_FIELDS: usize = 24;
const MAX_DEPTH: usize = 128;

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

/// Payload bytes are encoded once and remain opaque throughout all merge passes.
struct EncodedRow {
    key: Vec<u8>,
    tie: u64,
    payload: Vec<u8>,
}
impl EncodedRow {
    fn new(key: &[String], tie: u64, value: &Value) -> Result<Self, Diagnostic> {
        let mut payload = Vec::new();
        encode_value(&mut payload, value, 0)?;
        Ok(Self {
            key: encode_key(key)?,
            tie,
            payload,
        })
    }
    fn cmp(&self, rhs: &Self) -> Ordering {
        self.key.cmp(&rhs.key).then(self.tie.cmp(&rhs.tie))
    }
    fn frame_len(&self) -> Result<usize, Diagnostic> {
        FRAME_FIELDS
            .checked_add(self.key.len())
            .and_then(|n| n.checked_add(self.payload.len()))
            .ok_or_else(|| error("encode", "frame length overflow"))
    }
    fn decode(self) -> Result<Row, Diagnostic> {
        let mut decoder = Decoder {
            bytes: &self.payload,
            offset: 0,
        };
        let value = decoder.value(0)?;
        if decoder.offset != self.payload.len() {
            return Err(error("decode", "trailing payload bytes"));
        }
        Ok(Row {
            key: decode_key(&self.key)?,
            tie: self.tie,
            value,
        })
    }
}

fn bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<(), Diagnostic> {
    if out.capacity() - out.len() < value.len() {
        reserve_vec(out, value.len(), "output_sort_binary")?;
    }
    out.extend_from_slice(value);
    Ok(())
}
fn length(out: &mut Vec<u8>, n: usize) -> Result<(), Diagnostic> {
    let n = u64::try_from(n).map_err(|_| error("encode", "length overflow"))?;
    bytes(out, &n.to_le_bytes())
}
fn text(out: &mut Vec<u8>, s: &str) -> Result<(), Diagnostic> {
    length(out, s.len())?;
    bytes(out, s.as_bytes())
}
fn encode_value(out: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), Diagnostic> {
    if depth > MAX_DEPTH {
        return Err(error("encode", "value nesting too deep"));
    }
    match value {
        Value::Null => bytes(out, &[0]),
        Value::Bool(false) => bytes(out, &[1]),
        Value::Bool(true) => bytes(out, &[2]),
        Value::Number(n) if n.is_f64() => {
            bytes(out, &[5])?;
            bytes(out, &n.as_f64().expect("f64").to_bits().to_le_bytes())
        }
        Value::Number(n) if n.is_u64() => {
            bytes(out, &[3])?;
            bytes(out, &n.as_u64().expect("u64").to_le_bytes())
        }
        Value::Number(n) => {
            bytes(out, &[4])?;
            bytes(
                out,
                &n.as_i64()
                    .ok_or_else(|| error("encode", "unknown number kind"))?
                    .to_le_bytes(),
            )
        }
        Value::String(s) => {
            bytes(out, &[6])?;
            text(out, s)
        }
        Value::Array(a) => {
            bytes(out, &[7])?;
            length(out, a.len())?;
            for value in a {
                encode_value(out, value, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            bytes(out, &[8])?;
            length(out, o.len())?;
            for (key, value) in o {
                text(out, key)?;
                encode_value(out, value, depth + 1)?;
            }
            Ok(())
        }
    }
}
struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Decoder<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Diagnostic> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| error("decode", "truncated payload"))?;
        let data = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(data)
    }
    fn u64(&mut self) -> Result<u64, Diagnostic> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }
    fn count(&mut self, minimum: usize) -> Result<usize, Diagnostic> {
        let n = usize::try_from(self.u64()?).map_err(|_| error("decode", "length overflow"))?;
        if n > (self.bytes.len() - self.offset) / minimum {
            return Err(error("decode", "length exceeds remaining payload"));
        }
        Ok(n)
    }
    fn text(&mut self) -> Result<String, Diagnostic> {
        let n = self.count(1)?;
        let value = std::str::from_utf8(self.take(n)?).map_err(|e| error("decode", e))?;
        crate::allocation::copy_string(value, "output_sort_string")
    }
    fn value(&mut self, depth: usize) -> Result<Value, Diagnostic> {
        if depth > MAX_DEPTH {
            return Err(error("decode", "value nesting too deep"));
        }
        match self.take(1)?[0] {
            0 => Ok(Value::Null),
            1 => Ok(Value::Bool(false)),
            2 => Ok(Value::Bool(true)),
            3 => Ok(Value::from(self.u64()?)),
            4 => Ok(Value::from(i64::from_le_bytes(
                self.take(8)?.try_into().expect("eight bytes"),
            ))),
            5 => {
                let f = f64::from_bits(self.u64()?);
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .ok_or_else(|| error("decode", "nonfinite float"))
            }
            6 => self.text().map(Value::String),
            7 => {
                let n = self.count(1)?;
                let mut values = Vec::new();
                reserve_vec(&mut values, n, "output_sort_array")?;
                for _ in 0..n {
                    values.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(values))
            }
            8 => {
                let n = self.count(9)?;
                let mut values = serde_json::Map::new();
                for _ in 0..n {
                    let key = self.text()?;
                    let value = self.value(depth + 1)?;
                    if values.insert(key, value).is_some() {
                        return Err(error("decode", "duplicate object key"));
                    }
                }
                Ok(Value::Object(values))
            }
            _ => Err(error("decode", "unknown value tag")),
        }
    }
}

// NUL -> 00ff; component terminator -> 0000. UTF8 byte order matches String
// order; concatenated terminated components also preserve Vec prefix order.
fn encode_key(key: &[String]) -> Result<Vec<u8>, Diagnostic> {
    let mut encoded = Vec::new();
    let capacity = key.iter().try_fold(0usize, |size, part| {
        part.len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(2))
            .and_then(|n| size.checked_add(n))
            .ok_or_else(|| error("encode", "key length overflow"))
    })?;
    reserve_vec(&mut encoded, capacity, "output_sort_key_encode")?;
    for part in key {
        for &b in part.as_bytes() {
            if b == 0 {
                encoded.extend_from_slice(&[0, 255]);
            } else {
                encoded.push(b);
            }
        }
        encoded.extend_from_slice(&[0, 0]);
    }
    Ok(encoded)
}
fn key_parts(
    key: &[u8],
    mut visit: impl FnMut(&[u8]) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    let mut start = 0;
    let mut segment = 0;
    let mut i = 0;
    while i < key.len() {
        if key[i] != 0 {
            i += 1;
            continue;
        }
        std::str::from_utf8(&key[segment..i]).map_err(|e| error("decode", e))?;
        let escape = *key
            .get(i + 1)
            .ok_or_else(|| error("decode", "truncated key escape"))?;
        match escape {
            0 => {
                visit(&key[start..i])?;
                start = i + 2;
            }
            255 => {}
            _ => return Err(error("decode", "invalid key escape")),
        }
        i += 2;
        segment = i;
    }
    if start != key.len() {
        return Err(error("decode", "unterminated key component"));
    }
    Ok(())
}
fn decode_key(key: &[u8]) -> Result<Vec<String>, Diagnostic> {
    let mut parts = Vec::new();
    key_parts(key, |encoded| {
        let mut decoded = Vec::new();
        reserve_vec(&mut decoded, encoded.len(), "output_sort_key")?;
        let mut i = 0;
        while i < encoded.len() {
            decoded.push(encoded[i]);
            i += if encoded[i] == 0 { 2 } else { 1 };
        }
        let part = String::from_utf8(decoded).map_err(|e| error("decode", e))?;
        reserve_vec(&mut parts, 1, "output_sort_key_parts")?;
        parts.push(part);
        Ok(())
    })?;
    Ok(parts)
}

/// Trusted producer length travels with each run through merges and final reads.
/// A shorter file is corruption even if it ends exactly after a complete row.
#[derive(Clone, Debug)]
pub(super) struct Run {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Owns every run and removes it on both success and failure.
pub(super) struct Sorter {
    directory: PathBuf,
    chunk: Vec<EncodedRow>,
    chunk_bytes: usize,
    levels: Vec<Vec<Run>>,
    next: u64,
    file_serial: u64,
    max_frame_bytes: usize,
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
                        max_frame_bytes: 0,
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
        let row = EncodedRow::new(&key, tie, &value)?;
        let frame_len = row.frame_len()?;
        let size = frame_len
            .checked_add(8)
            .ok_or_else(|| error("encode", "frame length overflow"))?;
        self.max_frame_bytes = self.max_frame_bytes.max(frame_len);
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
    fn write_row(writer: &mut BufWriter<File>, row: &EncodedRow) -> Result<u64, Diagnostic> {
        let fields = [
            row.frame_len()? as u64,
            row.key.len() as u64,
            row.payload.len() as u64,
            row.tie,
        ];
        for field in fields {
            writer
                .write_all(&field.to_le_bytes())
                .map_err(|e| error("write", e))?;
        }
        writer
            .write_all(&row.key)
            .and_then(|()| writer.write_all(&row.payload))
            .map_err(|e| error("write", e))?;
        fields[0]
            .checked_add(8)
            .ok_or_else(|| error("write", "run length overflow"))
    }
    fn flush(&mut self) -> Result<(), Diagnostic> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        self.chunk.sort_by(EncodedRow::cmp);
        let path = self.path()?;
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| error("create", e))?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        writer.write_all(MAGIC).map_err(|e| error("write", e))?;
        let mut bytes = MAGIC.len() as u64;
        for row in &self.chunk {
            bytes = bytes
                .checked_add(Self::write_row(&mut writer, row)?)
                .ok_or_else(|| error("write", "run length overflow"))?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        self.promote(Run { path, bytes }, 0)?;
        self.chunk.clear();
        self.chunk_bytes = 0;
        Ok(())
    }
    fn promote(&mut self, mut path: Run, mut level: usize) -> Result<(), Diagnostic> {
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
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| error("create", e))?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, file);
        writer.write_all(MAGIC).map_err(|e| error("write", e))?;
        let mut readers = Vec::new();
        for path in paths {
            reserve_vec(&mut readers, 1, "output_sort_readers")?;
            readers.push(RunReader::open(
                &path.path,
                self.max_frame_bytes,
                path.bytes,
            )?);
        }
        let mut heads = Vec::new();
        for reader in &mut readers {
            reserve_vec(&mut heads, 1, "output_sort_heads")?;
            heads.push(reader.next_row()?);
        }
        let mut bytes = MAGIC.len() as u64;
        while let Some(index) = heads
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i, r)))
            .min_by(|a, b| a.1.cmp(b.1))
            .map(|(index, _)| index)
        {
            let row = heads[index].take().expect("selected sort head");
            bytes = bytes
                .checked_add(Self::write_row(&mut writer, &row)?)
                .ok_or_else(|| error("write", "run length overflow"))?;
            heads[index] = readers[index].next_row()?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        Ok(Run { path, bytes })
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
            max_frame_bytes: self.max_frame_bytes,
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
    max_frame_bytes: usize,
    bytes: u64,
}
impl Sorted {
    pub fn iter(&self) -> Result<Rows, Diagnostic> {
        Ok(Rows(
            self.path
                .as_ref()
                .map(|path| RunReader::open(path, self.max_frame_bytes, self.bytes))
                .transpose()?,
        ))
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
            .map(|row| row.and_then(EncodedRow::decode))
    }
}
/// Records are normally generated in wire order. Keep the complete original
/// binary stream until order has been checked, so a late violation can be replayed
/// into the stable external sorter without rerunning aggregation.
pub(super) struct OrderedWriter {
    directory: PathBuf,
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    previous: Option<Vec<u8>>,
    disordered: bool,
    next: u64,
    max_frame_bytes: usize,
    bytes: u64,
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
                        Ok(file) => {
                            let mut result = Self {
                                directory,
                                path,
                                writer: Some(BufWriter::with_capacity(1024 * 1024, file)),
                                previous: None,
                                disordered: false,
                                next: 0,
                                max_frame_bytes: 0,
                                bytes: MAGIC.len() as u64,
                            };
                            result
                                .writer
                                .as_mut()
                                .unwrap()
                                .write_all(MAGIC)
                                .map_err(|e| error("write", e))?;
                            Ok(result)
                        }
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
        let row = EncodedRow::new(&key, tie, &value)?;
        let frame_len = row.frame_len()?;
        self.bytes = self
            .bytes
            .checked_add(Sorter::write_row(
                self.writer.as_mut().expect("unfinished ordered writer"),
                &row,
            )?)
            .ok_or_else(|| error("write", "run length overflow"))?;
        if self
            .previous
            .as_ref()
            .is_some_and(|previous| previous > &row.key)
        {
            self.disordered = true;
        }
        self.max_frame_bytes = self.max_frame_bytes.max(frame_len);
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
            let mut reader = RunReader::open(&self.path, self.max_frame_bytes, self.bytes)?;
            while let Some(row) = reader.next_row()? {
                let row = row.decode()?;
                sorter.push(row.key, row.value)?;
            }
            return sorter.finish();
        }
        let directory = std::mem::take(&mut self.directory);
        let path = std::mem::take(&mut self.path);
        Ok(Sorted {
            directory,
            path: Some(path),
            max_frame_bytes: self.max_frame_bytes,
            bytes: self.bytes,
        })
    }
}
impl Drop for OrderedWriter {
    fn drop(&mut self) {
        self.writer.take();
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
    remaining: u64,
    max_frame_bytes: usize,
}
impl RunReader {
    fn open(
        path: &PathBuf,
        max_frame_bytes: usize,
        expected_bytes: u64,
    ) -> Result<Self, Diagnostic> {
        let file = File::open(path).map_err(|e| error("read", e))?;
        let size = file.metadata().map_err(|e| error("read", e))?.len();
        if size != expected_bytes {
            return Err(error("read", "run length differs from producer"));
        }
        let mut reader = BufReader::with_capacity(1024 * 1024, file);
        let mut magic = [0u8; 8];
        reader
            .read_exact(&mut magic)
            .map_err(|e| error("read", e))?;
        if &magic != MAGIC {
            return Err(error("decode", "invalid run header"));
        }
        Ok(Self {
            reader,
            remaining: size
                .checked_sub(8)
                .ok_or_else(|| error("decode", "truncated run header"))?,
            max_frame_bytes,
        })
    }
    fn word(&mut self) -> Result<u64, Diagnostic> {
        let mut word = [0u8; 8];
        self.reader
            .read_exact(&mut word)
            .map_err(|e| error("read", e))?;
        Ok(u64::from_le_bytes(word))
    }
    fn next_row(&mut self) -> Result<Option<EncodedRow>, Diagnostic> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if self.remaining < 8 {
            return Err(error("decode", "truncated frame length"));
        }
        let frame = self.word()?;
        if frame > self.remaining - 8
            || frame > self.max_frame_bytes as u64
            || frame < FRAME_FIELDS as u64 + 1
        {
            return Err(error("decode", "invalid or truncated frame length"));
        }
        let key_len = self.word()?;
        let payload_len = self.word()?;
        let tie = self.word()?;
        if key_len
            .checked_add(payload_len)
            .and_then(|n| n.checked_add(FRAME_FIELDS as u64))
            != Some(frame)
            || payload_len == 0
        {
            return Err(error("decode", "inconsistent frame lengths"));
        }
        let key_len =
            usize::try_from(key_len).map_err(|_| error("decode", "key length overflow"))?;
        let payload_len =
            usize::try_from(payload_len).map_err(|_| error("decode", "payload length overflow"))?;
        let mut key = Vec::new();
        reserve_vec(&mut key, key_len, "output_sort_key_frame")?;
        key.resize(key_len, 0);
        self.reader
            .read_exact(&mut key)
            .map_err(|e| error("read", e))?;
        key_parts(&key, |_| Ok(()))?;
        let mut payload = Vec::new();
        reserve_vec(&mut payload, payload_len, "output_sort_payload_frame")?;
        payload.resize(payload_len, 0);
        self.reader
            .read_exact(&mut payload)
            .map_err(|e| error("read", e))?;
        self.remaining -= frame + 8;
        Ok(Some(EncodedRow { key, tie, payload }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "dir-binary-sort-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn encoded(value: &Value) -> EncodedRow {
        let mut payload = Vec::new();
        encode_value(&mut payload, value, 0).unwrap();
        EncodedRow {
            key: encode_key(&["".into(), "nul\0é😀".into()]).unwrap(),
            tie: u64::MAX,
            payload,
        }
    }
    #[test]
    fn binary_value_tags_and_numeric_bits_match_authored_vectors() {
        for (value, expected) in [
            (Value::Null, vec![0]),
            (Value::Bool(false), vec![1]),
            (Value::Bool(true), vec![2]),
            (Value::from(u64::MAX), [vec![3], vec![255; 8]].concat()),
            (Value::from(i64::MIN), vec![4, 0, 0, 0, 0, 0, 0, 0, 128]),
            (Value::from(-0.0), vec![5, 0, 0, 0, 0, 0, 0, 0, 128]),
            (
                Value::from("a\0é"),
                vec![6, 4, 0, 0, 0, 0, 0, 0, 0, 97, 0, 195, 169],
            ),
        ] {
            let row = encoded(&value);
            assert_eq!(row.payload, expected);
            let row = row.decode().unwrap();
            assert_eq!(row.tie, u64::MAX);
            assert_eq!(row.value, value);
            assert_eq!(row.key, ["", "nul\0é😀"]);
        }
        let floats = [
            -0.0,
            0.0,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::MAX,
            -1.5,
            1.0 / 3.0,
        ];
        let value = Value::Array(floats.map(Value::from).into());
        let row = encoded(&value).decode().unwrap();
        for (actual, expected) in row.value.as_array().unwrap().iter().zip(floats) {
            assert_eq!(actual.as_f64().unwrap().to_bits(), expected.to_bits());
        }
        let value = serde_json::json!({"":[],"\0":[null,false,true,u64::MAX,i64::MIN],"a":{"q":"\"\\\r\n/日本語"},"z":{}});
        let before = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let after = encoded(&value).decode().unwrap().value;
        assert_eq!(after, value);
        assert_eq!(
            after
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            before
        );
    }
    #[test]
    fn escaped_byte_keys_preserve_vector_string_order_and_roundtrip() {
        let parts = [
            "",
            "\0",
            "\0a",
            "\u{1}",
            "a",
            "a\0",
            "aa",
            "aé",
            "é",
            "😀",
            "\u{10ffff}",
        ];
        let mut keys = vec![vec![]];
        for a in parts {
            keys.push(vec![a.to_owned()]);
            for b in parts {
                keys.push(vec![a.to_owned(), b.to_owned()]);
            }
        }
        keys.push(vec!["".into(), "".into(), "".into()]);
        let encoded = keys
            .iter()
            .map(|k| encode_key(k).unwrap())
            .collect::<Vec<_>>();
        for (i, a) in keys.iter().enumerate() {
            assert_eq!(decode_key(&encoded[i]).unwrap(), *a);
            for (j, b) in keys.iter().enumerate() {
                assert_eq!(encoded[i].cmp(&encoded[j]), a.cmp(b), "{a:?} vs {b:?}");
            }
        }
        assert_eq!(
            encode_key(&["\0".into(), "".into()]).unwrap(),
            [0, 255, 0, 0, 0, 0]
        );
    }
    #[test]
    fn malformed_payload_tags_counts_utf8_depth_and_trailing_bytes_reject() {
        let mut cases = vec![
            vec![],
            vec![99],
            vec![3],
            vec![4],
            vec![5],
            vec![6],
            vec![0, 0],
            vec![5, 0, 0, 0, 0, 0, 0, 240, 127], // +infinity
            vec![6, 1, 0, 0, 0, 0, 0, 0, 0, 255],
        ];
        for tag in [6, 7, 8] {
            cases.push([vec![tag], u64::MAX.to_le_bytes().to_vec()].concat());
        }
        // Duplicate object keys, each with a complete null value.
        cases.push(vec![
            8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        let mut deep = Vec::new();
        for _ in 0..MAX_DEPTH + 2 {
            deep.extend_from_slice(&[7, 1, 0, 0, 0, 0, 0, 0, 0]);
        }
        deep.push(0);
        cases.push(deep);
        for payload in cases {
            assert!(
                EncodedRow {
                    key: vec![],
                    tie: 0,
                    payload
                }
                .decode()
                .is_err()
            );
        }
        for key in [vec![0], vec![0, 1], vec![97], vec![255, 0, 0], vec![0, 255]] {
            assert!(decode_key(&key).is_err());
        }
        // Boundary depth is valid; one deeper is rejected before further recursion.
        let mut value = Value::Null;
        for _ in 0..MAX_DEPTH {
            value = Value::Array(vec![value]);
        }
        assert_eq!(encoded(&value).decode().unwrap().value, value);
        value = Value::Array(vec![value]);
        let mut payload = Vec::new();
        assert!(encode_value(&mut payload, &value, 0).is_err());
    }
    #[test]
    fn oversized_truncated_and_inconsistent_frames_reject_before_allocation() {
        let base = base();
        let path = base.join("run");
        let frames = [
            [MAGIC.to_vec(), u64::MAX.to_le_bytes().to_vec()].concat(),
            [MAGIC.to_vec(), vec![1, 2, 3]].concat(),
            [
                MAGIC.to_vec(),
                25u64.to_le_bytes().to_vec(),
                1u64.to_le_bytes().to_vec(),
                1u64.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
                vec![0],
            ]
            .concat(),
            // Physically present, but larger than the trusted producer's maximum.
            [
                MAGIC.to_vec(),
                25u64.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
                1u64.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
                vec![0],
            ]
            .concat(),
        ];
        for data in frames {
            fs::write(&path, &data).unwrap();
            let mut reader =
                RunReader::open(&path, 24, fs::metadata(&path).unwrap().len()).unwrap();
            assert!(reader.next_row().is_err());
        }
        fs::write(
            &path,
            [
                MAGIC.to_vec(),
                26u64.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
                1u64.to_le_bytes().to_vec(),
                0u64.to_le_bytes().to_vec(),
                vec![0, 0],
            ]
            .concat(),
        )
        .unwrap();
        assert!(
            RunReader::open(&path, 100, fs::metadata(&path).unwrap().len())
                .unwrap()
                .next_row()
                .is_err()
        );
        fs::write(&path, b"BADMAGIC").unwrap();
        assert!(RunReader::open(&path, 100, fs::metadata(&path).unwrap().len()).is_err());
        fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn merge_keeps_payload_opaque_and_failure_cleans_owned_files() {
        let base = base();
        let mut sorter = Sorter::new_in(&base).unwrap();
        sorter.max_frame_bytes = 25;
        let mut paths = Vec::new();
        for payload in [vec![99], vec![0]] {
            let path = sorter.path().unwrap();
            let mut writer = BufWriter::new(File::create(&path).unwrap());
            writer.write_all(MAGIC).unwrap();
            Sorter::write_row(
                &mut writer,
                &EncodedRow {
                    key: vec![],
                    tie: paths.len() as u64,
                    payload,
                },
            )
            .unwrap();
            writer.flush().unwrap();
            paths.push(Run {
                path,
                bytes: 8 + 8 + 25,
            });
        }
        let path = sorter.merge(&paths).unwrap();
        let mut reader = RunReader::open(&path.path, 25, path.bytes).unwrap();
        let row = reader.next_row().unwrap().unwrap();
        assert_eq!(row.payload, [99]);
        assert!(row.decode().is_err());
        assert_eq!(reader.next_row().unwrap().unwrap().payload, [0]);
        assert!(reader.next_row().unwrap().is_none());
        // Corrupt a run's bound and force an actual merge failure under Sorter RAII.
        fs::write(
            &paths[0].path,
            [MAGIC.to_vec(), u64::MAX.to_le_bytes().to_vec()].concat(),
        )
        .unwrap();
        assert!(sorter.merge(&paths).is_err());
        drop(sorter);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(&base).unwrap();
    }
    #[test]
    fn binary_multi_pass_sort_preserves_special_keys_full_values_and_stable_ties() {
        let base = base();
        let mut sorter = Sorter::new_in(&base).unwrap();
        let mut expected = Vec::new();
        let keys = [
            vec![],
            vec!["".into()],
            vec!["\0".into()],
            vec!["a".into()],
            vec!["a".into(), "".into()],
            vec!["é😀".into()],
        ];
        for i in 0..300u64 {
            let key = keys[(299 - i) as usize % keys.len()].clone();
            let value = serde_json::json!({"ordinal":i,"data":"x".repeat(40),"nested":[null,-0.0,i64::MIN,u64::MAX]});
            sorter.push(key.clone(), value.clone()).unwrap();
            expected.push(Row { key, tie: i, value });
        }
        assert!(sorter.file_serial > FAN_IN as u64);
        expected.sort();
        let sorted = sorter.finish().unwrap();
        let actual = sorted
            .iter()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.key, expected.key);
            assert_eq!(actual.tie, expected.tie);
            assert_eq!(actual.value, expected.value);
            assert_eq!(
                actual.value["nested"][1].as_f64().unwrap().to_bits(),
                (-0.0f64).to_bits()
            );
        }
        drop(sorted);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(&base).unwrap();
    }
    #[test]
    fn a_single_row_larger_than_chunk_has_no_new_row_size_limit() {
        let base = base();
        let mut sorter = Sorter::new_in(&base).unwrap();
        let value = Value::from("é\0".repeat(CHUNK_BYTES));
        sorter
            .push(vec!["oversized".into()], value.clone())
            .unwrap();
        assert!(sorter.max_frame_bytes > CHUNK_BYTES);
        let sorted = sorter.finish().unwrap();
        let actual = sorted.iter().unwrap().next().unwrap().unwrap();
        assert_eq!(actual.value, value);
        drop(sorted);
        fs::remove_dir(&base).unwrap();
    }
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
        assert!(
            sorted
                .iter()
                .map(|mut rows| rows.next().unwrap().is_err())
                .unwrap_or(true)
        );
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
    #[test]
    fn binary_ordered_late_fallback_and_merge_reopen_preserve_special_values() {
        let base = base();
        let mut points = OrderedWriter::new_in(&base).unwrap();
        let keys: Vec<Vec<String>> = vec![
            vec![],
            vec!["".into()],
            vec!["\0".into()],
            vec!["a".into()],
            vec!["a".into(), "".into()],
            vec!["é😀".into()],
        ];
        let mut expected = Vec::new();
        for i in 0..300u64 {
            let key = keys[i as usize / 50].clone();
            let value = serde_json::json!({"ordinal":i,"text":"\"\r\n\0é😀".repeat(30),"numbers":[u64::MAX,i64::MIN,-0.0,f64::from_bits(1)]});
            points.push(key.clone(), value.clone()).unwrap();
            expected.push((key, value));
        }
        let late = serde_json::json!({"ordinal":300,"late":true});
        points.push(vec![], late.clone()).unwrap();
        expected.push((vec![], late));
        assert!(points.disordered);
        let mut windows = OrderedWriter::new_in(&base).unwrap();
        let window = serde_json::json!({"window":true});
        windows.push(vec!["a".into()], window.clone()).unwrap();
        expected.push((vec!["a".into()], window));
        // Stable sorting represents the original points-then-windows insertion order.
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        let records = MergedRecords {
            points: points.finish().unwrap(),
            windows: windows.finish().unwrap(),
        };
        assert!(!records.points.is_direct_ordered());
        assert!(records.windows.is_direct_ordered());
        for _ in 0..2 {
            let actual = records
                .iter()
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(actual.len(), expected.len());
            for (row, (key, value)) in actual.iter().zip(&expected) {
                assert_eq!(&row.key, key);
                assert_eq!(&row.value, value);
                if row.value.get("numbers").is_some() {
                    assert_eq!(
                        row.value["numbers"][2].as_f64().unwrap().to_bits(),
                        (-0.0f64).to_bits()
                    );
                    assert_eq!(row.value["numbers"][3].as_f64().unwrap().to_bits(), 1);
                }
            }
        }
        drop(records);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn direct_and_fallback_binary_lengths_use_the_producers_bound() {
        for fallback in [false, true] {
            let base = base();
            let mut writer = OrderedWriter::new_in(&base).unwrap();
            writer.push(vec!["b".into()], Value::Null).unwrap();
            if fallback {
                writer.push(vec!["a".into()], Value::Null).unwrap();
            }
            writer.writer.as_mut().unwrap().flush().unwrap();
            let path = writer.path.clone();
            // A complete valid-looking larger frame must be rejected before allocating its body.
            let row = EncodedRow::new(&["b".into()], 0, &Value::from("x".repeat(4096))).unwrap();
            let mut file = BufWriter::new(File::create(&path).unwrap());
            file.write_all(MAGIC).unwrap();
            Sorter::write_row(&mut file, &row).unwrap();
            file.flush().unwrap();
            drop(file);
            if fallback {
                assert!(writer.finish().is_err());
            } else {
                let sorted = writer.finish().unwrap();
                assert!(
                    sorted
                        .iter()
                        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
                        .is_err()
                );
                drop(sorted);
            }
            assert!(!path.exists());
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
            fs::remove_dir(base).unwrap();
        }
    }
    #[test]
    fn late_binary_decode_and_truncation_failures_clean_the_paired_publication_stage() {
        for corruption in [
            "tag",
            "partial-length",
            "partial-fields",
            "partial-payload",
            "whole-row",
        ] {
            use super::super::{
                aggregate::{MetricValue, Record},
                json, publish, stream,
            };
            use std::io::{Seek, SeekFrom};
            let base = base();
            let prepared = crate::prepare(
                &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/can/baseline.ini"),
            )
            .unwrap();
            let snapshot = crate::snapshot::Snapshot::empty(&prepared);
            let mut points = OrderedWriter::new_in(&base).unwrap();
            let mut last_payload_offset = 0;
            let mut last_frame_offset = 0;
            let mut offset = MAGIC.len() as u64;
            for i in 0..150u64 {
                let record = Record::aggregate(
                    &format!("quoted,\"\r\né😀{}", "x".repeat(1024)),
                    "queue_length",
                    MetricValue::Integer(i as u128),
                    0,
                    i + 1,
                );
                let key = vec![format!("{i:016x}")];
                let value = json::record_value(&record);
                let encoded = EncodedRow::new(&key, i, &value).unwrap();
                last_frame_offset = offset;
                last_payload_offset = offset + 8 + FRAME_FIELDS as u64 + encoded.key.len() as u64;
                offset += 8 + encoded.frame_len().unwrap() as u64;
                points.push(key, value).unwrap();
            }
            let points = points.finish().unwrap();
            let mut file = OpenOptions::new()
                .write(true)
                .open(points.path.as_ref().unwrap())
                .unwrap();
            match corruption {
                "tag" => {
                    file.seek(SeekFrom::Start(last_payload_offset)).unwrap();
                    file.write_all(&[99]).unwrap();
                }
                "partial-length" => file.set_len(last_frame_offset + 3).unwrap(),
                "partial-fields" => file.set_len(last_frame_offset + 8 + 11).unwrap(),
                "partial-payload" => file.set_len(offset - 1).unwrap(),
                "whole-row" => file.set_len(last_frame_offset).unwrap(),
                _ => unreachable!(),
            }
            drop(file);
            let records = MergedRecords {
                points,
                windows: OrderedWriter::new_in(&base).unwrap().finish().unwrap(),
            };
            let result = stream::Prepared {
                records,
                models: None,
                requests: Some(Sorter::new_in(&base).unwrap().finish().unwrap()),
                receivers: Some(Sorter::new_in(&base).unwrap().finish().unwrap()),
                summary: Vec::new(),
            };
            let error = publish::stream(&base, "late-binary", 1, &snapshot, |stage| {
                stage.write_pair("results.json", "events.csv", |left, right| {
                    let error = json::write_stream_result(
                        left,
                        right,
                        &prepared,
                        &snapshot,
                        "late-binary",
                        "2026-10-08T00:00:00Z",
                        &result,
                    )
                    .unwrap_err();
                    if corruption == "tag" {
                        // Enough prefix rows reached both real staging files before the late failure.
                        assert!(
                            fs::metadata(base.join(".tmp-late-binary/results.json"))
                                .unwrap()
                                .len()
                                > 0
                        );
                        assert!(
                            fs::metadata(base.join(".tmp-late-binary/events.csv"))
                                .unwrap()
                                .len()
                                > 0
                        );
                    }
                    Err(error)
                })
            })
            .unwrap_err();
            assert_eq!(error.code, "E-0003");
            assert!(!base.join("manifest.json").exists());
            assert!(!base.join("results.json").exists());
            assert!(!base.join("events.csv").exists());
            assert!(!base.join(".tmp-late-binary").exists());
            drop(result);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
            fs::remove_dir(base).unwrap();
        }
    }
    #[test]
    fn pr37_every_partial_binary_frame_rejects_in_sorted_and_ordered_streams() {
        let base = base();
        for ordered in [false, true] {
            let value = serde_json::json!({"payload":"é\0", "bits":u64::MAX});
            let sorted = if ordered {
                let mut writer = OrderedWriter::new_in(&base).unwrap();
                for key in ["a", "b"] {
                    writer.push(vec![key.into()], value.clone()).unwrap();
                }
                writer.finish().unwrap()
            } else {
                let mut sorter = Sorter::new_in(&base).unwrap();
                for key in ["b", "a"] {
                    sorter.push(vec![key.into()], value.clone()).unwrap();
                }
                sorter.finish().unwrap()
            };
            let path = sorted.path.as_ref().unwrap();
            let data = fs::read(path).unwrap();
            for cut in 0..=data.len() {
                fs::write(path, &data[..cut]).unwrap();
                let result = sorted
                    .iter()
                    .and_then(|rows| rows.collect::<Result<Vec<_>, _>>());
                if cut == data.len() {
                    assert_eq!(result.unwrap().len(), 2, "ordered={ordered}, cut={cut}");
                } else {
                    let error = result.expect_err(&format!(
                        "accepted partial binary row: ordered={ordered}, cut={cut}"
                    ));
                    assert_eq!(error.code, "E-0003", "ordered={ordered}, cut={cut}");
                }
            }
            drop(sorted);
        }
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_binary_fallback_and_merge_reject_partial_headers_lengths_and_bodies() {
        let base = base();
        let value = serde_json::json!({"payload":"é\0", "bits":u64::MAX});
        let frame = EncodedRow::new(&["b".into()], 0, &value)
            .unwrap()
            .frame_len()
            .unwrap();
        let full = MAGIC.len() + 2 * (8 + frame);
        for cut in 0..=full {
            let mut writer = OrderedWriter::new_in(&base).unwrap();
            for key in ["b", "a"] {
                writer.push(vec![key.into()], value.clone()).unwrap();
            }
            assert!(writer.disordered);
            writer.writer.as_mut().unwrap().flush().unwrap();
            let data = fs::read(&writer.path).unwrap();
            fs::write(&writer.path, &data[..cut]).unwrap();
            let result = writer
                .finish()
                .and_then(|sorted| sorted.iter()?.collect::<Result<Vec<_>, _>>());
            let mut merger = Sorter::new_in(&base).unwrap();
            merger.max_frame_bytes = frame;
            let path = merger.path().unwrap();
            fs::write(&path, &data[..cut]).unwrap();
            let merged = merger.merge(&[Run {
                path,
                bytes: data.len() as u64,
            }]);
            if cut == full {
                let expected = 2;
                assert_eq!(result.unwrap().len(), expected, "fallback cut={cut}");
                let merged = merged.unwrap();
                let mut reader = RunReader::open(&merged.path, frame, merged.bytes).unwrap();
                let mut count = 0;
                while reader.next_row().unwrap().is_some() {
                    count += 1;
                }
                assert_eq!(count, expected, "merge cut={cut}");
            } else {
                assert_eq!(
                    result
                        .expect_err(&format!("fallback accepted cut={cut}"))
                        .code,
                    "E-0003"
                );
                assert_eq!(
                    merged.expect_err(&format!("merge accepted cut={cut}")).code,
                    "E-0003"
                );
            }
            drop(merger);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0, "cut={cut}");
        }
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_finalized_general_and_ordered_producers_reject_whole_row_loss() {
        for ordered in [false, true] {
            let base = base();
            let value = Value::Null;
            let sorted = if ordered {
                let mut writer = OrderedWriter::new_in(&base).unwrap();
                writer.push(vec!["a".into()], value.clone()).unwrap();
                writer.push(vec!["b".into()], value.clone()).unwrap();
                writer.finish().unwrap()
            } else {
                let mut sorter = Sorter::new_in(&base).unwrap();
                sorter.push(vec!["a".into()], value.clone()).unwrap();
                sorter.push(vec!["b".into()], value.clone()).unwrap();
                sorter.finish().unwrap()
            };
            let retained = MAGIC.len()
                + 8
                + EncodedRow::new(&["a".into()], 0, &value)
                    .unwrap()
                    .frame_len()
                    .unwrap();
            OpenOptions::new()
                .write(true)
                .open(sorted.path.as_ref().unwrap())
                .unwrap()
                .set_len(retained as u64)
                .unwrap();
            let result = sorted
                .iter()
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>());
            assert_eq!(
                result
                    .expect_err("finalized producer silently lost a complete row")
                    .code,
                "E-0003",
                "ordered={ordered}"
            );
            drop(sorted);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
            fs::remove_dir(base).unwrap();
        }
    }
    #[test]
    fn pr37_retained_fallback_producer_rejects_whole_row_loss() {
        let base = base();
        let mut writer = OrderedWriter::new_in(&base).unwrap();
        writer.push(vec!["b".into()], Value::Null).unwrap();
        writer.push(vec!["a".into()], Value::Null).unwrap();
        writer.writer.as_mut().unwrap().flush().unwrap();
        let retained = MAGIC.len()
            + 8
            + EncodedRow::new(&["b".into()], 0, &Value::Null)
                .unwrap()
                .frame_len()
                .unwrap();
        OpenOptions::new()
            .write(true)
            .open(&writer.path)
            .unwrap()
            .set_len(retained as u64)
            .unwrap();
        let error = match writer.finish() {
            Err(error) => error,
            Ok(_) => panic!("fallback silently lost a complete row"),
        };
        assert_eq!(error.code, "E-0003");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_intermediate_general_merge_rejects_whole_row_loss() {
        let base = base();
        let mut sorter = Sorter::new_in(&base).unwrap();
        sorter.push(vec!["a".into()], Value::Null).unwrap();
        sorter.push(vec!["b".into()], Value::Null).unwrap();
        sorter.flush().unwrap();
        let run = sorter.levels[0][0].clone();
        let retained = MAGIC.len()
            + 8
            + EncodedRow::new(&["a".into()], 0, &Value::Null)
                .unwrap()
                .frame_len()
                .unwrap();
        OpenOptions::new()
            .write(true)
            .open(&run.path)
            .unwrap()
            .set_len(retained as u64)
            .unwrap();
        assert_eq!(
            sorter
                .merge(&[run])
                .expect_err("merge silently lost a complete row")
                .code,
            "E-0003"
        );
        drop(sorter);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_valid_empty_and_record_boundary_eof_remain_accepted() {
        let base = base();
        for count in 0..3u64 {
            let mut sorter = Sorter::new_in(&base).unwrap();
            let mut ordered = OrderedWriter::new_in(&base).unwrap();
            for i in 0..count {
                sorter.push(vec![i.to_string()], Value::Null).unwrap();
                ordered.push(vec![i.to_string()], Value::Null).unwrap();
            }
            for sorted in [sorter.finish().unwrap(), ordered.finish().unwrap()] {
                assert_eq!(
                    sorted
                        .iter()
                        .unwrap()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap()
                        .len(),
                    count as usize
                );
            }
            let mut fallback = OrderedWriter::new_in(&base).unwrap();
            for i in (0..count).rev() {
                fallback.push(vec![i.to_string()], Value::Null).unwrap();
            }
            let sorted = fallback.finish().unwrap();
            assert_eq!(
                sorted
                    .iter()
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
                    .len(),
                count as usize
            );
        }
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
    #[test]
    fn pr37_binary_reader_rejects_late_shrink_beyond_prefetched_rows() {
        let base = base();
        let mut writer = OrderedWriter::new_in(&base).unwrap();
        let value = Value::from("x".repeat(64 * 1024));
        for i in 0..40u64 {
            writer.push(vec![format!("{i:03}")], value.clone()).unwrap();
        }
        let sorted = writer.finish().unwrap();
        assert!(sorted.bytes > 2 * 1024 * 1024);
        let mut reader = sorted.iter().unwrap();
        assert!(reader.next().unwrap().is_ok());
        let retained = MAGIC.len()
            + 8
            + EncodedRow::new(&["000".into()], 0, &value)
                .unwrap()
                .frame_len()
                .unwrap();
        OpenOptions::new()
            .write(true)
            .open(sorted.path.as_ref().unwrap())
            .unwrap()
            .set_len(retained as u64)
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
        assert!(read < 40);
        drop(reader);
        drop(sorted);
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
}
