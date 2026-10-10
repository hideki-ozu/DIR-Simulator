//! Stable AC0008-NED-NEG diagnostics, including the import/extends reason contract.
use dir_simulator::registry::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
static CALLBACKS: AtomicU64 = AtomicU64::new(0);
static CALLBACK_TEST: Mutex<()> = Mutex::new(());
struct Probe;
impl Model for Probe {
    fn initialize(&mut self, context: &mut Context<'_>) -> ModelResult {
        context.schedule_at(1, Schema::new("test.ProbeEvent", 1), vec![])?;
        Ok(())
    }
    fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
        CALLBACKS.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
fn registry() -> Registry {
    let schema = Schema::new("test.ProbeEvent", 1);
    let mut registry = Registry::new();
    registry
        .register_event(EventDescriptor {
            schema: schema.clone(),
            phase: 1,
            validate: |_| Ok(()),
        })
        .unwrap();
    registry
        .register_module(
            "test.Probe",
            |_| Ok(Box::new(Probe)),
            ModuleDescriptor {
                implementation_key: "test.Probe".into(),
                implementation_version: "1".into(),
                parameters: vec![],
                ports: vec![],
                events: vec![schema.clone()],
                resources: vec![],
            },
        )
        .unwrap();
    registry
        .register_profile(ProfileDescriptor {
            name: "test.nedrejection.v1".into(),
            implementation_version: "1".into(),
            modules: vec!["test.Probe".into()],
            channels: vec![],
            events: vec![schema],
            metrics: vec![],
            model_records: vec![],
            output_schema_version: 2,
            validate: |_, _| Ok(()),
        })
        .unwrap();
    registry
}
fn point(content: &str, end: usize) -> (usize, usize) {
    let prefix = &content[..end];
    (
        prefix.chars().filter(|&c| c == '\n').count() + 1,
        prefix.rsplit('\n').next().unwrap().chars().count() + 1,
    )
}
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
        expectation: (&str, &str),
        mutation: impl Fn(String) -> String,
    ) {
        let (reason, message) = expectation;
        let path = self.0.join("models/demo/Main.ned");
        let input = fs::read_to_string(&path).unwrap();
        let changed = mutation(input.clone());
        assert_ne!(changed, input, "mutation must actually change the fixture");
        let start = if token == "<EOF>" {
            changed.len()
        } else {
            changed.find(locator).unwrap() + offset
        };
        let end = if token == "<EOF>" {
            start
        } else {
            start + token.len()
        };
        if token != "<EOF>" {
            assert_eq!(&changed[start..end], token);
        }
        fs::write(&path, &changed).unwrap();
        let diagnostic = match dir_simulator::prepare(&self.0.join("scenario.ini")) {
            Ok(_) => panic!("unsupported or invalid NED was accepted"),
            Err(diagnostic) => diagnostic,
        };
        assert_eq!(diagnostic.code, "E-0001");
        assert_eq!(diagnostic.reason, reason);
        assert_eq!(diagnostic.stage, "prepare");
        assert_eq!(diagnostic.source.as_deref(), path.to_str());
        assert_eq!(diagnostic.target, None);
        assert_eq!(diagnostic.time_ps, None);
        assert_eq!(diagnostic.event_seq, None);
        assert!(diagnostic.primary);
        let (line, column) = point(&changed, start);
        let (end_line, end_column) = point(&changed, end);
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
        assert_eq!(
            diagnostic.message,
            if matches!(
                message,
                "unsupported NED token"
                    | "newline in string"
                    | "unterminated string"
                    | "unterminated block comment"
                    | "unexpected minus"
            ) {
                format!(
                    "{}: {}:{line}:{column}: {message}",
                    self.0.join("scenario.ini").display(),
                    path.display()
                )
            } else {
                format!(
                    "{}: {}:{line}:{column}: {message} (got {token})",
                    self.0.join("scenario.ini").display(),
                    path.display()
                )
            }
        );
        let callback_count_asserted = reason == "unsupported_syntax";
        let callback_counts = if callback_count_asserted {
            // Prove that this registered model executes a real event before testing rejection.
            // Serialize the positive controls and rejection counters across the three cases.
            let _guard = CALLBACK_TEST.lock().unwrap();
            let registered = Fixture::new();
            let config = registered.0.join("scenario.ini");
            let registered_path = registered.0.join("models/demo/Main.ned");
            let ned = "package demo;\nsimple Controller { parameters: @class(\"test.Probe\"); }\nnetwork Main { submodules: a: demo.Controller; }\n";
            fs::write(&registered_path, ned).unwrap();
            fs::write(&config, "[General]\nnetwork = demo.Main\nned-path = \"models\"\nmodel-profile = \"test.nedrejection.v1\"\nsim-time-limit = 10ps\n").unwrap();
            CALLBACKS.store(0, Ordering::Relaxed);
            let prepared = dir_simulator::prepare_with_registry(&config, registry()).unwrap();
            let snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
            assert_eq!(snapshot.common.committed_events, 1);
            let positive_control = CALLBACKS.load(Ordering::Relaxed);
            assert_eq!(positive_control, 1);
            CALLBACKS.store(0, Ordering::Relaxed);
            let registered_changed = mutation(ned.into());
            fs::write(&registered_path, &registered_changed).unwrap();
            let rejected = dir_simulator::prepare_with_registry(&config, registry()).unwrap_err();
            assert_eq!(rejected.code, diagnostic.code);
            assert_eq!(rejected.reason, reason);
            assert_eq!(rejected.stage, "prepare");
            assert_eq!(rejected.source.as_deref(), registered_path.to_str());
            assert_eq!(rejected.target, None);
            assert_eq!(rejected.time_ps, None);
            assert_eq!(rejected.event_seq, None);
            assert!(rejected.primary);
            let registered_start = registered_changed.find(locator).unwrap() + offset;
            let (registered_line, registered_column) = point(&registered_changed, registered_start);
            let (registered_end_line, registered_end_column) =
                point(&registered_changed, registered_start + token.len());
            assert_eq!(
                (
                    rejected.line,
                    rejected.column,
                    rejected.end_line,
                    rejected.end_column
                ),
                (
                    Some(registered_line),
                    Some(registered_column),
                    Some(registered_end_line),
                    Some(registered_end_column)
                ),
            );
            assert_eq!(rejected.details.as_ref().unwrap()["actual"], token);
            assert_eq!(rejected.details.as_ref().unwrap()["expected"], message);
            let rejected_callbacks = CALLBACKS.load(Ordering::Relaxed);
            assert_eq!(rejected_callbacks, 0);
            Some((positive_control, rejected_callbacks))
        } else {
            None
        };
        println!(
            "AC0008_DIAGNOSTIC {}",
            serde_json::json!({"case_id": case_id, "diagnostic": diagnostic,
                "asserted_token": token, "input_start_byte": start,
                "input_end_byte": end, "callback_count_asserted": callback_count_asserted,
                "callback_count": callback_counts.map(|counts| counts.1),
                "callback_positive_control_count": callback_counts.map(|counts| counts.0)})
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
        ("unsupported_syntax", "unsupported NED token"),
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
        ("unsupported_syntax", "unsupported declaration"),
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
        ("unsupported_syntax", "expected {"),
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
        ("syntax_error", "unsupported NED token"),
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
        ("syntax_error", "type reference must be fully qualified"),
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
        ("syntax_error", "expected input/output scalar gate"),
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
        ("syntax_error", "expected :"),
        |s| s.replacen("connections:", "connections allowunconnected:", 1),
    );
}

#[test]
fn ordinary_bad_token_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-BAD-TOKEN",
        "%",
        0,
        "%",
        ("syntax_error", "unsupported NED token"),
        |s| format!("{s}%"),
    );
}

#[test]
fn missing_opening_brace_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-MISSING-OPEN-BRACE",
        "network Main ;",
        "network Main ".len(),
        ";",
        ("syntax_error", "expected {"),
        |s| s.replacen("network Main {", "network Main ;", 1),
    );
}

#[test]
fn missing_closing_brace_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-MISSING-CLOSE-BRACE",
        "",
        0,
        "<EOF>",
        (
            "syntax_error",
            "expected ordered parameters/gates/submodules/connections section",
        ),
        |s| s[..s.rfind('}').unwrap()].into(),
    );
}

#[test]
fn malformed_string_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-BAD-STRING",
        "default(\"unterminated",
        "default(".len(),
        "\"",
        ("syntax_error", "newline in string"),
        |s| s.replacen("default(\"*\")", "default(\"unterminated)", 1),
    );
}

#[test]
fn import_like_string_does_not_change_lexical_failure_reason() {
    Fixture::new().reject(
        "ISSUE44-IMPORT-STRING",
        "%",
        0,
        "%",
        ("syntax_error", "unsupported NED token"),
        |s| {
            format!(
                "{}%",
                s.replacen(
                    "default(\"*\")",
                    "default(\"import demo.*; extends demo.Base\")",
                    1
                )
            )
        },
    );
}

#[test]
fn import_like_comment_does_not_change_lexical_failure_reason() {
    Fixture::new().reject(
        "ISSUE44-IMPORT-COMMENT",
        "%",
        0,
        "%",
        ("syntax_error", "unsupported NED token"),
        |s| format!("// import demo.*; extends demo.Base\n{s}%"),
    );
}

#[test]
fn complete_import_does_not_change_later_bad_expression_reason_or_priority() {
    Fixture::new().reject(
        "ISSUE44-IMPORT-BAD-EXPRESSION",
        "default(1 + 2)",
        "default(1 ".len(),
        "+",
        ("syntax_error", "unsupported NED token"),
        |s| {
            s.replacen("package demo;", "package demo;\nimport demo.Controller;", 1)
                .replacen("default(64)", "default(1 + 2)", 1)
        },
    );
}

#[test]
fn complete_import_does_not_change_later_malformed_string_reason() {
    Fixture::new().reject(
        "ISSUE44-IMPORT-BAD-STRING",
        "default(\"unterminated",
        "default(".len(),
        "\"",
        ("syntax_error", "newline in string"),
        |s| {
            s.replacen("package demo;", "package demo;\nimport demo.Controller;", 1)
                .replacen("default(\"*\")", "default(\"unterminated)", 1)
        },
    );
}

#[test]
fn inheritance_does_not_change_later_lexical_failure_reason_or_priority() {
    Fixture::new().reject(
        "ISSUE44-EXTENDS-BAD-TOKEN",
        "%",
        0,
        "%",
        ("syntax_error", "unsupported NED token"),
        |s| {
            format!(
                "{}%",
                s.replacen("network Main", "network Main extends demo.Base", 1)
            )
        },
    );
}

#[test]
fn nested_import_wildcard_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-NESTED-IMPORT",
        "import demo.*;",
        "import demo.".len(),
        "*",
        ("syntax_error", "unsupported NED token"),
        |s| s.replacen("network Main {", "network Main { import demo.*;", 1),
    );
}

#[test]
fn malformed_import_path_wildcard_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-MALFORMED-IMPORT",
        "import demo..*;",
        "import demo..".len(),
        "*",
        ("syntax_error", "unsupported NED token"),
        |s| s.replacen("package demo;", "package demo;\nimport demo..*;", 1),
    );
}

#[test]
fn unknown_property_still_reports_unsupported_syntax() {
    Fixture::new().reject(
        "ISSUE44-UNKNOWN-PROPERTY",
        "@unknown(",
        1,
        "unknown",
        ("unsupported_syntax", "unsupported property: unknown"),
        |s| s.replacen("@class(", "@unknown(", 1),
    );
}

#[test]
fn bare_import_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-BARE-IMPORT",
        "import",
        0,
        "import",
        ("syntax_error", "unsupported declaration"),
        |_| "package demo;\nimport".into(),
    );
}

#[test]
fn import_without_name_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-IMPORT-WITHOUT-NAME",
        "import ;",
        0,
        "import",
        ("syntax_error", "unsupported declaration"),
        |_| "package demo;\nimport ;".into(),
    );
}

#[test]
fn incomplete_import_name_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-INCOMPLETE-IMPORT-NAME",
        "import demo.",
        0,
        "import",
        ("syntax_error", "unsupported declaration"),
        |_| "package demo;\nimport demo.".into(),
    );
}

#[test]
fn bare_extends_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-BARE-EXTENDS",
        "extends",
        0,
        "extends",
        ("syntax_error", "expected {"),
        |_| "package demo;\nnetwork Main extends".into(),
    );
}

#[test]
fn extends_without_name_remains_syntax_error() {
    Fixture::new().reject(
        "ISSUE44-EXTENDS-WITHOUT-NAME",
        "extends {",
        0,
        "extends",
        ("syntax_error", "expected {"),
        |_| "package demo;\nnetwork Main extends {}".into(),
    );
}
