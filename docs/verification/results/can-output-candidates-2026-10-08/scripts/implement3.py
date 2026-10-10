from pathlib import Path
root=Path('/tmp/dir-opt-five-2026-10-08/candidate3/crates/dir-simulator/src')
p=root/'output/json.rs';s=p.read_text();s=s.replace('use std::collections::BTreeMap;\n','').replace('                let sorted: BTreeMap<_, _> = o.iter().collect();\n','').replace('for (i, (key, value)) in sorted.into_iter().enumerate()', 'for (i, (key, value)) in o.iter().enumerate()')
s=s.replace('pub(super) fn write_stream_result(\n    writer: &mut dyn std::io::Write,','pub(super) fn write_stream_result(\n    writer: &mut dyn std::io::Write,\n    csv_writer: &mut dyn std::io::Write,')
old='''        stream.records.iter()?.enumerate().map(|(i, row)| {
            row.map(|row| {
                let mut value = row.value;
                value["seq"] = json!(i.to_string());
                value
            })
        }),'''
new='''        stream.records.iter()?.enumerate().map(|(i, row)| {
            let mut value = row?.value;
            value["seq"] = json!(i.to_string());
            super::csv::write_stream_row(csv_writer, &value, run_id, version)?;
            Ok(value)
        }),'''
assert old in s;s=s.replace(old,new)
s=s.replace('    write_bytes(writer, b",\\\"records\\\":")?;\n    write_array_fallible(', '    super::csv::write_stream_header(csv_writer)?;\n    write_bytes(writer, b",\\\"records\\\":")?;\n    write_array_fallible(',1)
p.write_text(s)
p=root/'output/csv.rs';s=p.read_text();start=s.index('pub(super) fn write_stream(');head=s[:start];old=s[start:]
body=old[old.index('    fn cell('):old.index('    writer\n        .write_all(HEADER')]
row=old[old.index('        let value = if row["value"]'):old.rindex('    }\n    Ok(())')]
row='\n'.join(line[4:] if line.startswith('    ') else line for line in row.splitlines())
new='''pub(super) fn write_stream_header(writer: &mut dyn std::io::Write) -> Result<(), crate::Diagnostic> {
    writer.write_all(HEADER.as_bytes())
        .map_err(|e| crate::Diagnostic::output(format!("Cannot write CSV header: {e}")))
}

pub(super) fn write_stream_row(
    writer: &mut dyn std::io::Write,
    row: &Value,
    run_id: &str,
    version: u32,
) -> Result<(), crate::Diagnostic> {
'''+body+row+'''\n    Ok(())
}

pub(super) fn write_stream(
    writer: &mut dyn std::io::Write,
    rows: impl IntoIterator<Item = Result<Value, crate::Diagnostic>>,
    run_id: &str,
    version: u32,
) -> Result<(), crate::Diagnostic> {
    write_stream_header(writer)?;
    for row in rows { write_stream_row(writer, &row?, run_id, version)?; }
    Ok(())
}
'''
p.write_text(head+new)
p=root/'output/publish.rs';s=p.read_text();a=s.index('impl Staging {');b=s.index('/// Stream each file',a)
s=s[:a]+'''impl Staging {
    fn open(&self, name: &str) -> Result<BufWriter<HashWriter>, Diagnostic> {
        if !["results.json", "events.csv", "summary.csv", "diagnostics.jsonl"].contains(&name)
            || self.files.contains_key(name) {
            return Err(Diagnostic::output("unknown or duplicate output file"));
        }
        let file = OpenOptions::new().write(true).create_new(true)
            .open(self.directory.join(name))
            .map_err(|e| Diagnostic::output(format!("Cannot create {name}: {e}")))?;
        Ok(BufWriter::with_capacity(64 * 1024, HashWriter { file, hash: Sha256::new(), bytes: 0 }))
    }
    fn close(name: &str, mut writer: BufWriter<HashWriter>) -> Result<ValueFile, Diagnostic> {
        writer.flush().map_err(|e| Diagnostic::output(format!("Cannot flush {name}: {e}")))?;
        let writer = writer.into_inner().map_err(|e| Diagnostic::output(format!("Cannot close {name}: {e}")))?;
        Ok(ValueFile { sha256: format!("{:x}", writer.hash.finalize()), bytes: writer.bytes })
    }
    pub(super) fn write(&mut self, name: &str, write: impl FnOnce(&mut dyn Write) -> Result<(), Diagnostic>) -> Result<(), Diagnostic> {
        let mut writer = self.open(name)?;
        write(&mut writer)?;
        let value = Self::close(name, writer)?;
        self.files.insert(name.into(), value);
        Ok(())
    }
    pub(super) fn write_pair(&mut self, left: &str, right: &str,
        write: impl FnOnce(&mut dyn Write, &mut dyn Write) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        if left == right { return Err(Diagnostic::output("duplicate output pair")); }
        let mut a = self.open(left)?;
        let mut b = self.open(right)?;
        write(&mut a, &mut b)?;
        let a = Self::close(left, a)?;
        let b = Self::close(right, b)?;
        self.files.insert(left.into(), a);
        self.files.insert(right.into(), b);
        Ok(())
    }
}
'''+s[b:];p.write_text(s)
p=root/'output.rs';s=p.read_text();a=s.index('            stage.write("results.json", |writer| {',s.index('let streamed = stream::prepare'));b=s.index('            stage.write("summary.csv"',a)
s=s[:a]+'''            stage.write_pair("results.json", "events.csv", |writer, csv_writer| {
                json::write_stream_result(writer, csv_writer, prepared, snapshot, &run_id, &timestamp, &streamed)
            })?;
'''+s[b:];p.write_text(s)
