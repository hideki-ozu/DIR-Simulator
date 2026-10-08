from pathlib import Path
p=Path('crates/dir-simulator/src/output/disk_sort.rs');t=p.read_text()
t=t.replace('/// Owns every run and removes it on both success and failure.','''/// Trusted producer length travels with each run through merges and final reads.
/// A shorter file is corruption even if it ends exactly after a complete row.
#[derive(Clone, Debug)]
pub(super) struct Run {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Owns every run and removes it on both success and failure.''',1)
t=t.replace('levels: Vec<Vec<PathBuf>>','levels: Vec<Vec<Run>>',1)
t=t.replace('fn write_row(writer: &mut BufWriter<File>, row: &EncodedRow) -> Result<(), Diagnostic>','fn write_row(writer: &mut BufWriter<File>, row: &EncodedRow) -> Result<u64, Diagnostic>',1)
t=t.replace('''            .map_err(|e| error("write", e))
    }
    fn flush(&mut self)''','''            .map_err(|e| error("write", e))?;
        fields[0].checked_add(8).ok_or_else(|| error("write", "run length overflow"))
    }
    fn flush(&mut self)''',1)
t=t.replace('''        for row in &self.chunk {
            Self::write_row(&mut writer, row)?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        self.promote(path, 0)?;''','''        let mut bytes = MAGIC.len() as u64;
        for row in &self.chunk {
            bytes = bytes.checked_add(Self::write_row(&mut writer, row)?)
                .ok_or_else(|| error("write", "run length overflow"))?;
        }
        writer.flush().map_err(|e| error("flush", e))?;
        self.promote(Run { path, bytes }, 0)?;''',1)
t=t.replace('fn promote(&mut self, mut path: PathBuf, mut level: usize)','fn promote(&mut self, mut path: Run, mut level: usize)',1)
t=t.replace('fs::remove_file(old)', 'fs::remove_file(old.path)')
t=t.replace('fn merge(&mut self, paths: &[PathBuf]) -> Result<PathBuf, Diagnostic>','fn merge(&mut self, paths: &[Run]) -> Result<Run, Diagnostic>',1)
t=t.replace('readers.push(RunReader::open(path, self.max_frame_bytes)?);','readers.push(RunReader::open(&path.path, self.max_frame_bytes, path.bytes)?);',1)
t=t.replace('''        while let Some(index) = heads''','''        let mut bytes = MAGIC.len() as u64;
        while let Some(index) = heads''',1)
t=t.replace('''            Self::write_row(&mut writer, &row)?;
            heads[index]''','''            bytes = bytes.checked_add(Self::write_row(&mut writer, &row)?)
                .ok_or_else(|| error("write", "run length overflow"))?;
            heads[index]''',1)
t=t.replace('''        Ok(path)
    }
    pub fn finish''','''        Ok(Run { path, bytes })
    }
    pub fn finish''',1)
t=t.replace('fs::remove_file(old)', 'fs::remove_file(&old.path)')
t=t.replace('''        let path = pending.pop();
        let directory''','''        let run = pending.pop();
        let bytes = run.as_ref().map_or(0, |run| run.bytes);
        let path = run.map(|run| run.path);
        let directory''',1)
t=t.replace('''            max_frame_bytes: self.max_frame_bytes,
        })''','''            max_frame_bytes: self.max_frame_bytes,
            bytes,
        })''',1)
t=t.replace('''    max_frame_bytes: usize,
}
impl Sorted''','''    max_frame_bytes: usize,
    bytes: u64,
}
impl Sorted''',1)
t=t.replace('.map(|path| RunReader::open(path, self.max_frame_bytes))','.map(|path| RunReader::open(path, self.max_frame_bytes, self.bytes))',1)
t=t.replace('''    max_frame_bytes: usize,
}
impl OrderedWriter''','''    max_frame_bytes: usize,
    bytes: u64,
}
impl OrderedWriter''',1)
t=t.replace('''                                max_frame_bytes: 0,
''','''                                max_frame_bytes: 0,
                                bytes: MAGIC.len() as u64,
''',1)
t=t.replace('''        Sorter::write_row(
            self.writer.as_mut().expect("unfinished ordered writer"),
            &row,
        )?;''','''        self.bytes = self.bytes.checked_add(Sorter::write_row(
            self.writer.as_mut().expect("unfinished ordered writer"),
            &row,
        )?).ok_or_else(|| error("write", "run length overflow"))?;''',1)
t=t.replace('RunReader::open(&self.path, self.max_frame_bytes)?','RunReader::open(&self.path, self.max_frame_bytes, self.bytes)?',1)
t=t.replace('''            max_frame_bytes: self.max_frame_bytes,
        })''','''            max_frame_bytes: self.max_frame_bytes,
            bytes: self.bytes,
        })''',1)
t=t.replace('fn open(path: &PathBuf, max_frame_bytes: usize)','fn open(path: &PathBuf, max_frame_bytes: usize, expected_bytes: u64)',1)
t=t.replace('''        let size = file.metadata().map_err(|e| error("read", e))?.len();
''','''        let size = file.metadata().map_err(|e| error("read", e))?.len();
        if size != expected_bytes {
            return Err(error("read", "run length differs from producer"));
        }
''',1)
p.write_text(t)
p=Path('crates/dir-simulator/src/output/contribution_sort.rs');t=p.read_text()
t=t.replace('use crate::{Diagnostic, allocation::reserve_vec};','use super::disk_sort::Run;\nuse crate::{Diagnostic, allocation::reserve_vec};',1)
t=t.replace('levels: Vec<Vec<PathBuf>>','levels: Vec<Vec<Run>>',1)
t=t.replace('''        self.promote(path, 0)?;''','''        let bytes = u64::try_from(self.chunk.len()).ok()
            .and_then(|count| count.checked_mul(ROW_BYTES as u64))
            .ok_or_else(|| error("write", "run length overflow"))?;
        self.promote(Run { path, bytes }, 0)?;''',1)
t=t.replace('fn promote(&mut self, mut path: PathBuf, mut level: usize)','fn promote(&mut self, mut path: Run, mut level: usize)',1)
t=t.replace('fs::remove_file(old)', 'fs::remove_file(old.path)')
t=t.replace('fn merge(&mut self, paths: &[PathBuf]) -> Result<PathBuf, Diagnostic>','fn merge(&mut self, paths: &[Run]) -> Result<Run, Diagnostic>',1)
t=t.replace('readers.push(RunReader::open(source)?);','readers.push(RunReader::open(&source.path, source.bytes)?);',1)
t=t.replace('''        while let Some(index) = heads''','''        let mut bytes = 0u64;
        while let Some(index) = heads''',1)
t=t.replace('''            writer
                .write_all(&row.encode())''','''            bytes = bytes.checked_add(ROW_BYTES as u64)
                .ok_or_else(|| error("write", "run length overflow"))?;
            writer
                .write_all(&row.encode())''',1) # first occurrence is flush; fix location below
# Ensure accounting is in merge only (flush derives exact chunk count above).
t=t.replace('''        for row in &self.chunk {
            bytes = bytes.checked_add(ROW_BYTES as u64)
                .ok_or_else(|| error("write", "run length overflow"))?;
''','''        for row in &self.chunk {
''',1)
t=t.replace('''                .ok_or_else(|| error("merge", "missing head"))?;
            writer''','''                .ok_or_else(|| error("merge", "missing head"))?;
            bytes = bytes.checked_add(ROW_BYTES as u64)
                .ok_or_else(|| error("write", "run length overflow"))?;
            writer''',1)
t=t.replace('''        Ok(path)
    }
    pub fn finish''','''        Ok(Run { path, bytes })
    }
    pub fn finish''',1)
t=t.replace('fs::remove_file(old)', 'fs::remove_file(&old.path)')
t=t.replace('''        let path = pending.pop();
        let directory''','''        let run = pending.pop();
        let bytes = run.as_ref().map_or(0, |run| run.bytes);
        let path = run.map(|run| run.path);
        let directory''',1)
t=t.replace('Ok(Sorted { directory, path })','Ok(Sorted { directory, path, bytes })',1)
t=t.replace('''    path: Option<PathBuf>,
}
impl Sorted''','''    path: Option<PathBuf>,
    bytes: u64,
}
impl Sorted''',1)
t=t.replace('self.path.as_deref().map(RunReader::open)','self.path.as_deref().map(|path| RunReader::open(path, self.bytes))',1)
t=t.replace('fn open(path: &Path)','fn open(path: &Path, expected_bytes: u64)',1)
t=t.replace('''        Ok(Self {
            reader: BufReader::new(File::open(path).map_err(|e| error("read", e))?),
        })''','''        let file = File::open(path).map_err(|e| error("read", e))?;
        if file.metadata().map_err(|e| error("read", e))?.len() != expected_bytes {
            return Err(error("read", "run length differs from producer"));
        }
        Ok(Self {
            reader: BufReader::new(file),
        })''',1)
p.write_text(t)
