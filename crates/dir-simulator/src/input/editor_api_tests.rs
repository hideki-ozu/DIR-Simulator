use super::*;
use std::cell::RefCell;

#[derive(Clone)]
struct StoredError(io::ErrorKind, String);
fn capture<T>(result: io::Result<T>) -> std::result::Result<T, StoredError> {
    result.map_err(|error| StoredError(error.kind(), error.to_string()))
}
fn restore<T: Clone>(result: &std::result::Result<T, StoredError>) -> io::Result<T> {
    result
        .clone()
        .map_err(|error| io::Error::new(error.0, error.1))
}
type StoredEntries = Vec<(OsString, std::result::Result<InputKind, StoredError>)>;
#[derive(Default)]
struct MemorySource {
    metadata: RefCell<BTreeMap<PathBuf, std::result::Result<InputMetadata, StoredError>>>,
    directories: RefCell<BTreeMap<PathBuf, std::result::Result<StoredEntries, StoredError>>>,
    text: RefCell<BTreeMap<PathBuf, std::result::Result<String, StoredError>>>,
    calls: RefCell<Vec<(&'static str, PathBuf)>>,
}
impl InputSource for MemorySource {
    fn metadata(&self, path: &Path) -> io::Result<InputMetadata> {
        self.calls.borrow_mut().push(("metadata", path.into()));
        self.metadata
            .borrow()
            .get(path)
            .map(restore)
            .unwrap_or_else(missing)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<InputDirEntry>> {
        self.calls.borrow_mut().push(("read_dir", path.into()));
        let entries = self
            .directories
            .borrow()
            .get(path)
            .map(restore)
            .unwrap_or_else(missing)?;
        Ok(entries
            .into_iter()
            .map(|(name, kind)| InputDirEntry {
                name,
                kind: restore(&kind),
            })
            .collect())
    }
    fn read_utf8(&self, path: &Path) -> io::Result<String> {
        self.calls.borrow_mut().push(("read_utf8", path.into()));
        self.text
            .borrow()
            .get(path)
            .map(restore)
            .unwrap_or_else(missing)
    }
}
fn missing<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "not in captured snapshot",
    ))
}
struct RecordingSource(MemorySource);
impl InputSource for RecordingSource {
    fn metadata(&self, path: &Path) -> io::Result<InputMetadata> {
        let result = capture(FsInputSource.metadata(path));
        self.0.metadata.borrow_mut().insert(path.into(), result);
        self.0.metadata(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<InputDirEntry>> {
        let result = capture(FsInputSource.read_dir(path)).map(|entries| {
            entries
                .into_iter()
                .map(|entry| (entry.name, capture(entry.kind)))
                .collect()
        });
        self.0.directories.borrow_mut().insert(path.into(), result);
        self.0.read_dir(path)
    }
    fn read_utf8(&self, path: &Path) -> io::Result<String> {
        let result = capture(FsInputSource.read_utf8(path));
        self.0.text.borrow_mut().insert(path.into(), result);
        self.0.read_utf8(path)
    }
}

fn compare_sources(config: &Path, cwd: &Path) {
    let disk = prepare_with_source(config, cwd, &FsInputSource);
    let recording = RecordingSource(MemorySource::default());
    let captured = prepare_with_source(config, cwd, &recording);
    let disk_calls = recording.0.calls.replace(Vec::new());
    let snapshot = prepare_with_source(config, cwd, &recording.0);
    assert_eq!(format!("{disk:#?}"), format!("{captured:#?}"));
    assert_eq!(format!("{disk:#?}"), format!("{snapshot:#?}"));
    assert_eq!(
        disk_calls,
        *recording.0.calls.borrow(),
        "I/O order for {}",
        config.display()
    );
    if config.is_absolute() {
        assert_eq!(format!("{disk:#?}"), format!("{:#?}", prepare(config)));
    }
}

#[test]
fn filesystem_and_snapshot_match_can_gateway_success_and_failure_fixtures() {
    let cwd = std::env::current_dir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures");
    for directory in ["can", "gw"] {
        let mut configs: Vec<_> = fs::read_dir(root.join(directory))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "ini"))
            .collect();
        configs.sort();
        assert!(!configs.is_empty());
        for config in configs {
            compare_sources(&config, &cwd);
        }
    }
    compare_sources(
        Path::new("../../examples/gateway/fanout.ini"),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    );
}

#[test]
fn inspect_is_structural_and_preserves_invalid_execution_values() {
    let text = "\u{feff}[General]\r\nnetwork = unknown\r\nned-path = \"../models\";\"/vendor/types\"\r\nmodel-profile = \"unsupported\"\r\nmodel-config = \"../data/config.json\"\r\nworkload = \"jobs=a;#b.json\"\r\nsim-time-limit = -999ps\r\nMain.a.queueCapacity = -1\r\n";
    let header = inspect_config(
        text,
        Path::new("project/scenario.ini"),
        Path::new("/launch"),
    )
    .unwrap();
    assert_eq!(header.config, Path::new("/launch/project/scenario.ini"));
    assert_eq!(header.cwd, Path::new("/launch"));
    assert_eq!(header.network.as_deref(), Some("unknown"));
    assert_eq!(header.profile.as_deref(), Some("unsupported"));
    assert_eq!(
        header.roots,
        [
            PathBuf::from("/launch/models"),
            PathBuf::from("/vendor/types")
        ]
    );
    assert_eq!(
        header.workload.as_deref(),
        Some(Path::new("/launch/project/jobs=a;#b.json"))
    );
    assert_eq!(
        header.model_config.as_deref(),
        Some(Path::new("/launch/data/config.json"))
    );
    assert_eq!(header.general["sim-time-limit"], "-999ps");
    let minimal = inspect_config(
        "[General]\nned-path = \"models\"\n",
        Path::new("/p/x.ini"),
        Path::new("/"),
    )
    .unwrap();
    assert!(minimal.network.is_none() && minimal.profile.is_none());
    for malformed in [
        "[General]\nned-path = \"models\";\n",
        "[General]\nned-path = \"models\"\nworkload = \"\"\n",
        "[General]\nned-path = \"models\"\nmodel-profile = unquoted\n",
        "[General]\nned-path = \"models\"\nned-path = \"models\"\n",
    ] {
        assert!(inspect_config(malformed, Path::new("/p/x.ini"), Path::new("/")).is_err());
    }
}

#[test]
fn syntax_api_exposes_structure_without_model_value_validation() {
    let text = "package demo;\nsimple Bad { parameters: int capacity = default(-99999999999999999999999999999); gates: output tx; }\nnetwork Main { submodules: a: demo.Bad; connections: a.tx --> a.tx; }\n";
    let parsed = parse_ned(text, Path::new("/models/demo/Main.ned"), "demo").unwrap();
    let declarations = parsed.declarations();
    let simple = &declarations[0];
    assert_eq!(simple.name(), "demo.Bad");
    assert_eq!(simple.kind(), "simple");
    assert!(simple.implementation().is_none());
    let capacity = &simple.parameters()["capacity"];
    assert_eq!(capacity.scalar(), "int");
    assert!(capacity.unit().is_none());
    assert_eq!(capacity.default(), Some("-99999999999999999999999999999"));
    assert!(simple.gates()["tx"]);
    let main = &declarations[1];
    assert_eq!(main.children(), &[("a".into(), "demo.Bad".into())]);
    let connection = &main.connections()[0];
    assert_eq!(connection.start(), "a.tx");
    assert_eq!(connection.end(), "a.tx");
    assert!(connection.channel().is_none());
    assert!(parse_ned(text, Path::new("Main.ned"), "other").is_err());
    assert!(parse_ned(text, Path::new("Main.ned"), "").is_err());
    assert!(
        parse_ned(
            "package demo; network Main {",
            Path::new("Main.ned"),
            "demo"
        )
        .is_err()
    );
}

#[test]
fn snapshot_missing_paths_never_fall_back_to_disk() {
    let source = MemorySource::default();
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gateway/fanout.ini");
    assert!(config.exists());
    let failure =
        prepare_with_source(&config, &std::env::current_dir().unwrap(), &source).unwrap_err();
    assert!(failure.message.contains("not in captured snapshot"));
}

#[test]
fn deferred_directory_errors_keep_sorted_traversal_order() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gateway");
    let recording = RecordingSource(MemorySource::default());
    let config = root.join("fanout.ini");
    let cwd = std::env::current_dir().unwrap();
    prepare_with_source(&config, &cwd, &recording).unwrap();
    let ned_root = absolute(&root.join("models"), &cwd);
    recording.0.directories.borrow_mut().insert(
        ned_root.clone(),
        Ok(vec![
            (
                "z".into(),
                Err(StoredError(
                    io::ErrorKind::PermissionDenied,
                    "later error".into(),
                )),
            ),
            (
                "a".into(),
                Err(StoredError(
                    io::ErrorKind::PermissionDenied,
                    "first error".into(),
                )),
            ),
        ]),
    );
    let error = prepare_with_source(&config, &cwd, &recording.0).unwrap_err();
    assert!(
        error.message.contains("/models/a: first error"),
        "{}",
        error.message
    );
    assert!(!error.message.contains("later error"));
}

fn memory_project(ini: &str, ned: &str) -> MemorySource {
    let source = MemorySource::default();
    for path in [
        "/",
        "/editor-fixture",
        "/editor-fixture/models",
        "/editor-fixture/models/demo",
    ] {
        source.metadata.borrow_mut().insert(
            path.into(),
            Ok(InputMetadata {
                kind: InputKind::Directory,
            }),
        );
    }
    for (path, content) in [
        ("/editor-fixture/scenario.ini", ini),
        ("/editor-fixture/models/demo/Main.ned", ned),
    ] {
        source.metadata.borrow_mut().insert(
            path.into(),
            Ok(InputMetadata {
                kind: InputKind::File,
            }),
        );
        source
            .text
            .borrow_mut()
            .insert(path.into(), Ok(content.into()));
    }
    source.directories.borrow_mut().insert(
        "/editor-fixture/models".into(),
        Ok(vec![("demo".into(), Ok(InputKind::Directory))]),
    );
    source.directories.borrow_mut().insert(
        "/editor-fixture/models/demo".into(),
        Ok(vec![("Main.ned".into(), Ok(InputKind::File))]),
    );
    source
}
const CAN_INI: &str = "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n";
const CAN_NED: &str =
    include_str!("../../../../docs/verification/fixtures/can/models/demo/Main.ned");

#[test]
fn crlf_line_comments_preserve_valid_ned_and_reject_bare_carriage_returns() {
    let source = format!(
        "\u{feff}// leading comment\r\n{}\r\n// trailing comment\r\n",
        CAN_NED.replace("\r\n", "\n").replace('\n', "\r\n")
    );
    let parsed = parse_ned(&source, Path::new("Main.ned"), "demo").unwrap();
    assert_eq!(parsed.declarations().len(), 4);
    assert!(
        parse_ned(
            "// invalid\rcomment\npackage demo;\nnetwork Main {}",
            Path::new("Main.ned"),
            "demo"
        )
        .is_err()
    );
}

#[test]
fn semantic_invalid_values_are_parseable_but_fail_full_snapshot_prepare() {
    for (invalid, expected) in [
        ("-1", "out of range or unsupported value"),
        ("4294967296", "out of range or unsupported value"),
        ("99999999999999999999999999999", "int exceeds i64"),
    ] {
        let ned = CAN_NED.replace("default(64)", &format!("default({invalid})"));
        let source = memory_project(CAN_INI, &ned);
        let config = Path::new("/editor-fixture/scenario.ini");
        let header = inspect_config(CAN_INI, config, Path::new("/")).unwrap();
        assert_eq!(header.network.as_deref(), Some("demo.Main"));
        parse_ned(
            &ned,
            Path::new("/editor-fixture/models/demo/Main.ned"),
            "demo",
        )
        .unwrap();
        let failure = prepare_with_source(config, Path::new("/"), &source).unwrap_err();
        assert!(failure.message.contains(expected), "{}", failure.message);
    }
    let invalid_time = CAN_INI.replace("1ms", "-1ms");
    let config = Path::new("/editor-fixture/scenario.ini");
    inspect_config(&invalid_time, config, Path::new("/")).unwrap();
    let source = memory_project(&invalid_time, CAN_NED);
    let failure = prepare_with_source(config, Path::new("/"), &source).unwrap_err();
    assert!(failure.message.contains("negative quantity: -1ms"));
    assert!(
        !source
            .calls
            .borrow()
            .iter()
            .any(|(operation, _)| *operation == "read_dir")
    );
}

#[test]
fn existing_profile_error_precedes_time_and_root_errors() {
    let ini = format!(
        "{}model-profile = \"unsupported\"\n",
        CAN_INI.replace("1ms", "-1ms")
    );
    let source = memory_project(&ini, CAN_NED);
    source
        .metadata
        .borrow_mut()
        .remove(Path::new("/editor-fixture/models"));
    let config = Path::new("/editor-fixture/scenario.ini");
    inspect_config(&ini, config, Path::new("/")).unwrap();
    let failure = prepare_with_source(config, Path::new("/"), &source).unwrap_err();
    assert_eq!(failure.code, "E-0001");
    assert_eq!(
        failure.message,
        "/editor-fixture/scenario.ini: unsupported model-profile: unsupported"
    );
}
