from pathlib import Path
root=Path('/tmp/dir-opt-five-2026-10-08')
p=root/'candidate3/crates/dir-simulator/src/output/publish.rs';s=p.read_text();pos=s.rindex('\n}')
s=s[:pos]+'''
    #[test]
    fn paired_writer_failure_never_publishes_either_output() {
        let prepared = crate::prepare(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/can/baseline.ini")).unwrap();
        let snapshot = Snapshot::empty(&prepared);
        for side in 0..3 {
            let out = std::env::temp_dir().join(format!("dir-paired-failure-{}-{side}", std::process::id()));
            fs::create_dir(&out).unwrap();
            let error = stream(&out, "pair-failure", 1, &snapshot, |stage| {
                stage.write_pair("results.json", "events.csv", |left, right| {
                    if side == 0 { return Err(Diagnostic::output("injected before left")); }
                    left.write_all(b"left complete\\n").unwrap();
                    if side == 1 { return Err(Diagnostic::output("injected before right")); }
                    right.write_all(b"right complete\\n").unwrap();
                    Err(Diagnostic::output("injected after both writes"))
                })
            }).unwrap_err();
            assert_eq!(error.code, "E-0003");
            assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
            fs::remove_dir(out).unwrap();
        }
    }
'''+s[pos:];p.write_text(s)
p=root/'candidate5/crates/dir-simulator/src/output/stream.rs';s=p.read_text();pos=s.rindex('\n}')
s=s[:pos]+'''
    #[test]
    fn unknown_archived_statistics_target_is_a_diagnostic() {
        let output = directory();
        let prepared = prepared();
        let snapshot = crate::runtime::spool::with_spooling(&output, || crate::runtime::simulate(&prepared)).unwrap();
        let archive = fs::read_dir(&output).unwrap().map(|e| e.unwrap().path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".dir-can-ledger-")).unwrap();
        let text = fs::read_to_string(&archive).unwrap();
        let mut lines = text.lines();
        let mut first: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        first["request"]["source"] = json!("unknown-topology-node");
        let mut altered = serde_json::to_string(&first).unwrap();
        altered.push('\\n');
        for line in lines { altered.push_str(line); altered.push('\\n'); }
        fs::write(&archive, altered).unwrap();
        let error = super::super::export(&prepared, &snapshot, &output).unwrap_err();
        assert_eq!(error.code, "E-0003");
        assert!(!output.join("manifest.json").exists());
        assert!(!fs::read_dir(&output).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".dir-result-sort-")));
        drop(snapshot);
        fs::remove_dir_all(output).unwrap();
    }
'''+s[pos:];p.write_text(s)
(root/'candidate3-design.md').write_text('''# Candidate 3: one-pass JSON and CSV export

Verified Cargo features for serde_json are default, float_roundtrip and std; preserve_order is absent. Iterate existing sorted Map entries directly in canonical() while keeping string escapes, integer and f64 formatting. The existing independent canonical golden covers reversed keys, controls and Unicode.

Staging.write_pair opens two distinct permitted names with create_new, supplies two bounded hashing BufWriters, flushes/closes both and records both hashes only after success. The common manifest remains last; RAII removes any failed private staging. Existing single-file writer semantics are preserved by sharing open/close helpers. No threads or all-row buffering are added.

JSON records iteration assigns final seq exactly once, projects the same Value to the CSV row writer, then appends JSON. Header and other JSON arrays, summary and diagnostics retain their existing order and format. Generic registered and in-memory paths keep their existing row iteration. This candidate measures the combined scope of the original candidate3: canonical re-sort removal plus shared parse/read, not separate additive effects.

Verify golden wire bytes, paired writer failure before/after each side, memory/spooled schema1 and schema2 equivalence, full locked gates and independent CLI hash comparisons. No production adoption is implied.
''')
(root/'candidate5-design.md').write_text('''# Candidate 5: indexed statistics updates

StatsTable stores a topology-sized Vec<Stats>, a borrowed-name index map and a fixed $all slot. Initialize all controller/bus/$all subjects, including idle ones. Each request resolves source and bus once; each receiver resolves its name once. Reuse those numeric slots for status, delays and bit sums rather than allocating cloned names and looking up maps repeatedly. All checked u128 arithmetic and original duplicate-target update semantics remain.

Existing summaries retain their target/metric/reason/receiver sort order and output format. Their small topology reads can use the index map; the hot update loop performs numeric indexing. Unknown archived names produce output diagnostics before indexing, and owned sort files are removed on failure. The map is built once per export and uses no global registry or changes to runtime/public input.

Validate the semantic-corrupt archived source diagnostic and all existing memory-versus-spooled output tests, then isolate effects with the same pinned binary/input/hash methodology. Contribution/record sorting and JSON/CSV output passes are unchanged. No production adoption is implied.
''')
