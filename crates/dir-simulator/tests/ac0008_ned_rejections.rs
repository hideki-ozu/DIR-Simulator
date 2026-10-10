//! Stable AC0008-NED-NEG cases characterize actual rejection diagnostics.
//! Import/extends still report syntax_error rather than the specification's
//! unsupported_syntax; passing characterization does not close that gap.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = std::env::temp_dir().join(format!(
            "dir-ac0008-ned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(directory.join("models/demo")).unwrap();
        fs::copy(
            root.join("docs/verification/fixtures/can/models/demo/Main.ned"),
            directory.join("models/demo/Main.ned"),
        )
        .unwrap();
        fs::write(directory.join("scenario.ini"), "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n").unwrap();
        Self(directory)
    }
    fn reject(
        &self,
        case_id: &str,
        locator: &str,
        offset: usize,
        token: &str,
        message: &str,
        mutation: impl FnOnce(String) -> String,
    ) {
        let path = self.0.join("models/demo/Main.ned");
        let input = fs::read_to_string(&path).unwrap();
        let changed = mutation(input.clone());
        assert_ne!(changed, input, "mutation must actually change the fixture");
        let start = changed.find(locator).unwrap() + offset;
        assert_eq!(&changed[start..start + token.len()], token);
        fs::write(&path, &changed).unwrap();
        let diagnostic = match dir_simulator::prepare(&self.0.join("scenario.ini")) {
            Ok(_) => panic!("unsupported or invalid NED was accepted"),
            Err(diagnostic) => diagnostic,
        };
        assert_eq!(diagnostic.code, "E-0001");
        assert_eq!(diagnostic.reason, "syntax_error");
        assert_eq!(diagnostic.stage, "prepare");
        assert_eq!(diagnostic.source.as_deref(), path.to_str());
        assert_eq!(diagnostic.target, None);
        assert_eq!(diagnostic.time_ps, None);
        assert_eq!(diagnostic.event_seq, None);
        assert!(diagnostic.primary);
        let point = |end: usize| {
            let prefix = &changed[..end];
            (
                prefix.chars().filter(|&c| c == '\n').count() + 1,
                prefix.rsplit('\n').next().unwrap().chars().count() + 1,
            )
        };
        let (line, column) = point(start);
        let (end_line, end_column) = point(start + token.len());
        assert_eq!(
            (
                diagnostic.line,
                diagnostic.column,
                diagnostic.end_line,
                diagnostic.end_column
            ),
            (Some(line), Some(column), Some(end_line), Some(end_column)),
        );
        assert_eq!(diagnostic.details.as_ref().unwrap()["actual"], token);
        assert_eq!(diagnostic.details.as_ref().unwrap()["expected"], message);
        println!(
            "AC0008_DIAGNOSTIC {}",
            serde_json::json!({"case_id": case_id, "diagnostic": diagnostic,
                "asserted_token": token, "input_start_byte": start,
                "input_end_byte": start + token.len(), "callback_count_asserted": false})
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn import_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(
        "NED-S1-T01-R09-O001",
        "import demo.*;",
        "import demo.".len(),
        "*",
        "unsupported NED token",
        |s| s.replacen("package demo;", "package demo;\nimport demo.*;", 1),
    );
}
#[test]
fn explicit_import_name_is_rejected_with_ned_keyword_source() {
    // Separate the import keyword rejection from the wildcard lexical error.
    Fixture::new().reject(
        "NED-S1-T01-R09-O001",
        "import demo.Controller;",
        0,
        "import",
        "unsupported declaration",
        |s| s.replacen("package demo;", "package demo;\nimport demo.Controller;", 1),
    );
}
#[test]
fn inheritance_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(
        "NED-S1-T01-R09-O002",
        "extends demo.Base",
        0,
        "extends",
        "expected {",
        |s| s.replacen("network Main", "network Main extends demo.Base", 1),
    );
}
#[test]
fn vector_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(
        "NED-S1-T01-R09-O008",
        "a[2]",
        1,
        "[",
        "unsupported NED token",
        |s| s.replacen("a: demo.Controller", "a[2]: demo.Controller", 1),
    );
}
#[test]
fn unqualified_child_type_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(
        "NED-S1-T01-R06-O009",
        "a: Controller;",
        "a: Controller".len(),
        ";",
        "type reference must be fully qualified",
        |s| s.replacen("a: demo.Controller", "a: Controller", 1),
    );
}
#[test]
fn inout_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(
        "NED-S1-T01-R09-O009",
        "inout tx;",
        0,
        "inout",
        "expected input/output scalar gate",
        |s| s.replacen("output tx;", "inout tx;", 1),
    );
}
#[test]
fn allowunconnected_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(
        "NED-S1-T01-R09-O012",
        "connections allowunconnected:",
        "connections ".len(),
        "allowunconnected",
        "expected :",
        |s| s.replacen("connections:", "connections allowunconnected:", 1),
    );
}
