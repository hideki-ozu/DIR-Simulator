from pathlib import Path
root=Path('/tmp/dir-v1.1.4-preparation-2026-10-08/worktree')
p=root/'crates/dir-simulator/src/output/disk_sort.rs';t=p.read_text();pos=t.rfind('\n}')
t=t[:pos]+r'''
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
            let boundary = MAGIC.len() + 8 + EncodedRow::new(&["a".into()], 0, &value).unwrap().frame_len().unwrap();
            for cut in 0..=data.len() {
                fs::write(path, &data[..cut]).unwrap();
                let result = sorted.iter().and_then(|rows| rows.collect::<Result<Vec<_>, _>>());
                if [MAGIC.len(), boundary, data.len()].contains(&cut) {
                    let expected = if cut == MAGIC.len() { 0 } else if cut == boundary { 1 } else { 2 };
                    assert_eq!(result.unwrap().len(), expected, "ordered={ordered}, cut={cut}");
                } else {
                    let error = result.expect_err(&format!("accepted partial binary row: ordered={ordered}, cut={cut}"));
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
        let frame = EncodedRow::new(&["b".into()], 0, &value).unwrap().frame_len().unwrap();
        let boundary = MAGIC.len() + 8 + frame;
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
            let result = writer.finish().and_then(|sorted| sorted.iter()?.collect::<Result<Vec<_>, _>>());
            let mut merger = Sorter::new_in(&base).unwrap();
            merger.max_frame_bytes = frame;
            let path = merger.path().unwrap();
            fs::write(&path, &data[..cut]).unwrap();
            let merged = merger.merge(&[path]);
            if [MAGIC.len(), boundary, full].contains(&cut) {
                let expected = if cut == MAGIC.len() { 0 } else if cut == boundary { 1 } else { 2 };
                assert_eq!(result.unwrap().len(), expected, "fallback cut={cut}");
                let mut reader = RunReader::open(&merged.unwrap(), frame).unwrap();
                let mut count = 0;
                while reader.next_row().unwrap().is_some() { count += 1; }
                assert_eq!(count, expected, "merge cut={cut}");
            } else {
                assert_eq!(result.expect_err(&format!("fallback accepted cut={cut}")).code, "E-0003");
                assert_eq!(merged.expect_err(&format!("merge accepted cut={cut}")).code, "E-0003");
            }
            drop(merger);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0, "cut={cut}");
        }
        fs::remove_dir(base).unwrap();
    }
'''+t[pos:]
# Broaden the existing real paired-publication corruption test with truncation probes.
t=t.replace('fn late_binary_decode_failure_cleans_the_paired_publication_stage() {','fn late_binary_decode_and_truncation_failures_clean_the_paired_publication_stage() {\n        for corruption in ["tag", "partial-length", "partial-fields", "partial-payload"] {')
t=t.replace('''        file.seek(SeekFrom::Start(last_payload_offset)).unwrap();
        file.write_all(&[99]).unwrap(); // valid frame/key, invalid final Value tag
''','''        match corruption {
            "tag" => {
                file.seek(SeekFrom::Start(last_payload_offset)).unwrap();
                file.write_all(&[99]).unwrap();
            }
            "partial-length" => file.set_len(last_payload_offset - FRAME_FIELDS as u64 - 18 + 3).unwrap(),
            "partial-fields" => file.set_len(last_payload_offset - 18 - 3).unwrap(),
            "partial-payload" => file.set_len(offset - 1).unwrap(),
            _ => unreachable!(),
        }
''')
# Close loop immediately before this test's closing brace.
needle='''        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn pr37_every_partial'''
# Exact whitespace before appended tests differs; locate function next test robustly.
start=t.index('fn late_binary_decode_and_truncation_failures')
end=t.index('    #[test]\n    fn pr37_every_partial',start)
part=t[start:end]
assert part.rstrip().endswith('}')
last=part.rfind('    }')
part=part[:last]+'        }\n'+part[last:]
t=t[:start]+part+t[end:]
p.write_text(t)
p=root/'crates/dir-simulator/src/output/contribution_sort.rs';t=p.read_text();pos=t.rfind('\n}')
t=t[:pos]+r'''
    #[test]
    fn pr37_fixed_rows_reject_every_partial_record_in_reader_and_merge() {
        let base = std::env::temp_dir().join(format!(
            "dir-contribution-pr37-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        let rows = [
            Row { time: 0, tie: 0, target: 0, kind: Kind::Busy, amount: 1 },
            Row { time: 1, tie: 1, target: 0, kind: Kind::Busy, amount: -1 },
        ];
        let data = [rows[0].encode(), rows[1].encode()].concat();
        for cut in 0..=data.len() {
            let mut sorter = Sorter::new_in(&base).unwrap();
            let path = sorter.path().unwrap();
            fs::write(&path, &data[..cut]).unwrap();
            let sorted = Sorted { directory: PathBuf::new(), path: Some(path.clone()) };
            let result = sorted.iter().unwrap().collect::<Result<Vec<_>, _>>();
            let merged = sorter.merge(&[path]);
            if cut % ROW_BYTES == 0 {
                assert_eq!(result.unwrap(), rows[..cut / ROW_BYTES], "reader cut={cut}");
                let mut reader = RunReader::open(&merged.unwrap()).unwrap();
                let mut actual = Vec::new();
                while let Some(row) = reader.next_row().unwrap() { actual.push(row); }
                assert_eq!(actual, rows[..cut / ROW_BYTES], "merge cut={cut}");
            } else {
                assert_eq!(result.expect_err(&format!("reader accepted cut={cut}")).code, "E-0003");
                assert_eq!(merged.expect_err(&format!("merge accepted cut={cut}")).code, "E-0003");
            }
            drop(sorted);
            drop(sorter);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0, "cut={cut}");
        }
        fs::remove_dir(base).unwrap();
    }
'''+t[pos:];p.write_text(t)
