from pathlib import Path
p=Path('crates/dir-simulator/src/output/disk_sort.rs');t=p.read_text()
t=t.replace('fs::remove_file(old.path)','fs::remove_file(&old.path)')
for maxsize in ['24','100']:
 t=t.replace(f'RunReader::open(&path, {maxsize})',f'RunReader::open(&path, {maxsize}, fs::metadata(&path).unwrap().len())')
t=t.replace('paths.push(path);','paths.push(Run { path, bytes: 8 + 8 + 25 });',1)
t=t.replace('RunReader::open(&path, 25)','RunReader::open(&path.path, 25, path.bytes)',1)
t=t.replace('&paths[0],','&paths[0].path,',1)
t=t.replace('assert!(sorted.iter().unwrap().next().unwrap().is_err());','assert!(sorted.iter().and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).is_err());')
# Every shortening of an existing finalized two-row producer must now fail.
start=t.index('fn pr37_every_partial_binary_frame')
end=t.index('    #[test]',start)
part=t[start:end]
a=part.index('            let boundary =');b=part.index('            for cut',a);part=part[:a]+part[b:]
a=part.index('                if [MAGIC.len(), boundary, data.len()].contains(&cut) {');b=part.index('                } else {',a)
part=part[:a]+'''                if cut == data.len() {
                    assert_eq!(result.unwrap().len(), 2, "ordered={ordered}, cut={cut}");
'''+part[b:]
t=t[:start]+part+t[end:]
start=t.index('fn pr37_binary_fallback_and_merge')
end=t.index('    #[test]',start)
part=t[start:end].replace('        let boundary = MAGIC.len() + 8 + frame;\n','')
part=part.replace('let merged = merger.merge(&[path]);','let merged = merger.merge(&[Run { path, bytes: data.len() as u64 }]);')
a=part.index('            if [MAGIC.len(), boundary, full].contains(&cut) {');b=part.index('                assert_eq!(result.unwrap().len(), expected,',a)
part=part[:a]+'''            if cut == full {
                let expected = 2;
'''+part[b:]
part=part.replace('let mut reader = RunReader::open(&merged.unwrap(), frame).unwrap();','let merged = merged.unwrap();\n                let mut reader = RunReader::open(&merged.path, frame, merged.bytes).unwrap();')
t=t[:start]+part+t[end:]
t=t.replace('.open(&run)','.open(&run.path)')
# Only tag corruption is detected after rows flow. Size mismatches fail before scan.
start=t.index('fn late_binary_decode_and_truncation_failures');end=t.index('    #[test]',start)
part=t[start:end]
a=part.index('                    // Enough prefix rows');b=part.index('                    Err(error)',a)
part=part[:a]+'''                    if corruption == "tag" {
'''+part[a:b]+'''                    }
'''+part[b:]
t=t[:start]+part+t[end:]
pos=t.rfind('\n}')
t=t[:pos]+r'''
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
                assert_eq!(sorted.iter().unwrap().collect::<Result<Vec<_>,_>>().unwrap().len(), count as usize);
            }
            let mut fallback = OrderedWriter::new_in(&base).unwrap();
            for i in (0..count).rev() { fallback.push(vec![i.to_string()], Value::Null).unwrap(); }
            let sorted = fallback.finish().unwrap();
            assert_eq!(sorted.iter().unwrap().collect::<Result<Vec<_>,_>>().unwrap().len(), count as usize);
        }
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        fs::remove_dir(base).unwrap();
    }
'''+t[pos:];p.write_text(t)
p=Path('crates/dir-simulator/src/output/contribution_sort.rs');t=p.read_text()
t=t.replace('fs::remove_file(old.path)','fs::remove_file(&old.path)')
t=t.replace('assert!(sorted.iter().unwrap().next().unwrap().is_err());','assert!(sorted.iter().and_then(|rows| rows.collect::<Result<Vec<_>, _>>()).is_err());',1)
t=t.replace('''                path: Some(path.clone()),
            };''','''                path: Some(path.clone()),
                bytes: cut as u64,
            };''',1)
t=t.replace('let merged = sorter.merge(&[path]);','let merged = sorter.merge(&[Run { path, bytes: cut as u64 }]);',1)
t=t.replace('RunReader::open(&merged.unwrap())','{ let merged = merged.unwrap(); RunReader::open(&merged.path, merged.bytes) }',1)
t=t.replace('.open(&run)','.open(&run.path)')
p.write_text(t)
