//! End-to-end diagnostics preserve the offending token and the common wire contract.
use dir_simulator::{
    Diagnostic,
    input::{inspect_config, parse_ned},
    prepare,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

const NED: &str = include_str!("../../../docs/verification/fixtures/can/models/demo/Main.ned");
const INI: &str = "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(ini: &str, ned: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-diagnostics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("models/demo")).unwrap();
        fs::write(path.join("models/demo/Main.ned"), ned).unwrap();
        fs::write(path.join("run.ini"), ini).unwrap();
        Self(path)
    }
    fn error(&self) -> Diagnostic {
        prepare(&self.0.join("run.ini")).unwrap_err()
    }
    fn workload(text: &str) -> Self {
        let fixture = Self::new(&format!("{INI}workload = \"workload.json\"\n"), NED);
        fs::write(fixture.0.join("workload.json"), text).unwrap();
        fixture
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn assert_range(d: &Diagnostic, text: &str, start: usize, token: &str) {
    fn point(text: &str, offset: usize) -> (usize, usize) {
        let prefix = text[..offset]
            .strip_prefix('\u{feff}')
            .unwrap_or(&text[..offset])
            .replace("\r\n", "\n");
        (
            prefix.chars().filter(|&c| c == '\n').count() + 1,
            prefix.rsplit('\n').next().unwrap().chars().count() + 1,
        )
    }
    assert_eq!(text.get(start..start + token.len()), Some(token));
    let (line, column) = point(text, start);
    let (end_line, end_column) = point(text, start + token.len());
    assert_eq!(
        (d.line, d.column, d.end_line, d.end_column),
        (Some(line), Some(column), Some(end_line), Some(end_column)),
        "{d:?}"
    );
}

#[test]
fn wire_contract_is_complete_nullable_and_string_only() {
    let mut diagnostic =
        Diagnostic::execution("failure").with_runtime("finish", Some(27), None, Some("Main.a"));
    diagnostic.details = Some(json!({"number":7,"boolean":true,"text":"same"}));
    let value = serde_json::to_value(diagnostic.normalized(1, false)).unwrap();
    let fields = [
        "schema_version",
        "seq",
        "severity",
        "primary",
        "code",
        "phase",
        "reason",
        "message",
        "source",
        "line",
        "column",
        "end_line",
        "end_column",
        "target",
        "time_ps",
        "event_seq",
        "details",
    ];
    assert_eq!(value.as_object().unwrap().len(), fields.len());
    for field in fields {
        assert!(value.get(field).is_some(), "missing {field}");
    }
    assert_eq!(value["seq"], "1");
    assert_eq!(value["primary"], false);
    assert_eq!(value["phase"], "finish");
    assert_eq!(value["time_ps"], "27");
    assert!(value["source"].is_null());
    assert!(value["line"].is_null());
    assert!(value["event_seq"].is_null());
    assert_eq!(
        value["details"],
        json!({"number":"7","boolean":"true","text":"same"})
    );
    assert!(value.get("stage").is_none());
    let unknown = serde_json::to_value(
        Diagnostic::execution("custom failure").with_reason("private_plugin_kind"),
    )
    .unwrap();
    assert_eq!(unknown["reason"], "model_failed");
    assert_eq!(unknown["details"]["original_reason"], "private_plugin_kind");
}

#[test]
fn ini_unknown_key_points_to_key_after_bom_crlf_and_unicode_comment() {
    let text = "\u{feff}[General]\r\n# 日本語\r\n\t typo = 1ps\r\n";
    let path = Path::new("/virtual/設定.ini");
    let diagnostic = inspect_config(text, path, Path::new("/virtual")).unwrap_err();
    assert_eq!(diagnostic.reason, "unknown_parameter");
    assert_eq!(diagnostic.target.as_deref(), Some("typo"));
    assert_eq!(diagnostic.source.as_deref(), Some("/virtual/設定.ini"));
    assert_range(&diagnostic, text, text.find("typo").unwrap(), "typo");
    assert_eq!(diagnostic.details.as_ref().unwrap()["actual"], "typo");
}

#[test]
fn ini_invalid_value_points_to_adopted_value_not_same_default() {
    let text = format!(
        "\u{feff}{}Main.a.queueCapacity = \t\"wrong\"\r\n",
        INI.replace('\n', "\r\n")
    );
    let fixture = Fixture::new(&text, NED);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_type");
    assert_eq!(diagnostic.target.as_deref(), Some("Main.a.queueCapacity"));
    assert_eq!(
        diagnostic.source,
        Some(fixture.0.join("run.ini").to_string_lossy().into_owned())
    );
    assert_range(
        &diagnostic,
        &text,
        text.find("\"wrong\"").unwrap(),
        "\"wrong\"",
    );
}

#[test]
fn ini_unknown_parameter_points_to_complete_assignment_key() {
    let text = format!("{INI}Main.a.typo = 7\n");
    let fixture = Fixture::new(&text, NED);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "unknown_parameter");
    assert_eq!(diagnostic.target.as_deref(), Some("Main.a.typo"));
    assert_range(
        &diagnostic,
        &text,
        text.find("Main.a.typo").unwrap(),
        "Main.a.typo",
    );
}

#[test]
fn ned_eof_has_empty_range_and_correct_unicode_scalar_column() {
    let text = "\u{feff}package demo;\r\n// 日本語\r\nsimple Example { parameters: string label = default(\"日本\");";
    let diagnostic = parse_ned(text, Path::new("/virtual/example.ned"), "demo").unwrap_err();
    assert_eq!(diagnostic.reason, "syntax_error");
    assert_range(&diagnostic, text, text.len(), "");
    let with_newline = format!("{text}\r\n");
    let diagnostic =
        parse_ned(&with_newline, Path::new("/virtual/example.ned"), "demo").unwrap_err();
    assert_range(&diagnostic, &with_newline, with_newline.len(), "");
}

#[test]
fn ned_unknown_child_reference_points_to_type_token() {
    let ned = NED.replacen("a: demo.Controller;", "a: demo.Missing;", 1);
    let fixture = Fixture::new(INI, &ned);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "unknown_type");
    assert_eq!(diagnostic.target.as_deref(), Some("demo.Missing"));
    assert_range(
        &diagnostic,
        &ned,
        ned.find("demo.Missing").unwrap(),
        "demo.Missing",
    );
    assert_eq!(diagnostic.details.as_ref().unwrap()["type"], "demo.Missing");
}

#[test]
fn nested_json_error_uses_exact_value_despite_repeated_keys() {
    let text = "\u{feff}{\r\n\"schema_version\":1,\r\n\"generators\":[{\"id\":\"g\",\"kind\":\"can.explicit.v1\",\"node\":\"Main.a\",\"frame\":{\"format\":\"standard\",\"id\":\"日本語\",\"data\":\"\"},\"times\":[\"0ps\"]}]}";
    let fixture = Fixture::workload(text);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_type");
    assert_eq!(diagnostic.target.as_deref(), Some("/generators/0/frame/id"));
    assert_eq!(
        diagnostic.source,
        Some(
            fixture
                .0
                .join("workload.json")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert_range(
        &diagnostic,
        text,
        text.find("\"日本語\"").unwrap(),
        "\"日本語\"",
    );
}

#[test]
fn duplicate_json_key_points_to_second_key_and_retains_related_location() {
    let text = "{\"schema_version\":1,\"generators\":[],\"schema_version\":1}";
    let fixture = Fixture::workload(text);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "duplicate_definition");
    assert_eq!(diagnostic.target.as_deref(), Some("/schema_version"));
    assert_range(
        &diagnostic,
        text,
        text.rfind("\"schema_version\"").unwrap(),
        "\"schema_version\"",
    );
    assert_eq!(diagnostic.details.as_ref().unwrap()["related_line"], "1");
    assert_eq!(diagnostic.details.as_ref().unwrap()["related_column"], "2");
}

#[test]
fn unreadable_input_has_path_and_null_positions() {
    let fixture = Fixture::new(INI, NED);
    let path = fixture.0.join("missing.ini");
    let diagnostic = prepare(&path).unwrap_err();
    assert_eq!(diagnostic.reason, "input_unreadable");
    assert_eq!(diagnostic.source, Some(path.to_string_lossy().into_owned()));
    assert_eq!(
        (
            diagnostic.line,
            diagnostic.column,
            diagnostic.end_line,
            diagnostic.end_column
        ),
        (None, None, None, None)
    );
    assert!(diagnostic.time_ps.is_none());
    assert!(diagnostic.event_seq.is_none());
    assert!(diagnostic.details.as_ref().unwrap()["os_error"].is_string());
}

#[test]
fn preparation_stderr_and_jsonl_share_primary_object() {
    let fixture = Fixture::new(&format!("{INI}Main.a.typo = 1\n"), NED);
    let output = fixture.0.join("output");
    let process = Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(["run", "--config"])
        .arg(fixture.0.join("run.ini"))
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(process.status.code(), Some(2));
    let stderr: Value = serde_json::from_slice(&process.stderr).unwrap();
    let persisted: Value =
        serde_json::from_slice(&fs::read(output.join("diagnostics.jsonl")).unwrap()).unwrap();
    assert_eq!(stderr, persisted);
    assert_eq!(stderr["seq"], "0");
    assert_eq!(stderr["primary"], true);
    assert_eq!(stderr["phase"], "prepare");
    assert!(stderr["time_ps"].is_null());
    assert!(stderr["event_seq"].is_null());
}

#[test]
fn runtime_guard_retains_candidate_time_and_reserved_sequence() {
    let fixture = Fixture::workload(
        r#"{"schema_version":1,"generators":[{"id":"g","kind":"can.explicit.v1","node":"Main.a","frame":{"format":"standard","id":3,"data":""},"times":["0ps","1ps"]}]}"#,
    );
    fs::write(
        fixture.0.join("run.ini"),
        format!("{INI}workload = \"workload.json\"\nmax-events = 1\n"),
    )
    .unwrap();
    let output = fixture.0.join("output");
    let report = dir_simulator::run(prepare(&fixture.0.join("run.ini")).unwrap(), &output).unwrap();
    assert_eq!(report.committed_events, "1");
    let diagnostic = report.primary_diagnostic.unwrap();
    assert_eq!(diagnostic.code, "E-0004");
    assert_eq!(diagnostic.reason, "event_limit");
    assert_eq!(diagnostic.stage, "run");
    assert_eq!(diagnostic.time_ps, Some(report.finish_ps.parse().unwrap()));
    assert!(diagnostic.event_seq.is_some());
    assert!(diagnostic.source.is_none());
    assert!(diagnostic.line.is_none());
    let persisted: Value =
        serde_json::from_slice(&fs::read(output.join("diagnostics.jsonl")).unwrap()).unwrap();
    assert_eq!(persisted, serde_json::to_value(diagnostic).unwrap());
}

#[test]
fn stderr_and_jsonl_use_same_control_character_escaping() {
    let fixture = Fixture::new(&INI.replace("1ms", "1\tbad"), NED);
    let output = fixture.0.join("output");
    let process = Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(["run", "--config"])
        .arg(fixture.0.join("run.ini"))
        .arg("--output")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(process.status.code(), Some(2));
    let persisted = fs::read(output.join("diagnostics.jsonl")).unwrap();
    assert_eq!(process.stderr, persisted);
    assert!(String::from_utf8(persisted).unwrap().contains("\\u0009"));
}

#[test]
fn every_model_family_locates_semantic_json_field_by_identity() {
    fn copy_directory(source: &Path, target: &Path) {
        fs::create_dir_all(target).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let destination = target.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_directory(&entry.path(), &destination);
            } else {
                fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for (family, config, json_file, pointer) in [
        (
            "axi",
            "read-write.ini",
            "model.json",
            "/managers/0/max_outstanding",
        ),
        (
            "soc",
            "soc-round-robin.ini",
            "soc-round-robin.model.json",
            "/targets/0/size",
        ),
        (
            "memory-ipc",
            "copy.ini",
            "model.json",
            "/ddr/0/queue_capacity",
        ),
        (
            "ethernet",
            "unicast.ini",
            "model-v1.json",
            "/endpoints/0/mac",
        ),
        (
            "canfd",
            "precomputed.ini",
            "workload.json",
            "/generators/0/frame/id",
        ),
        (
            "gateway",
            "fanout.ini",
            "routing.json",
            "/gateways/0/routes/0/id_max",
        ),
    ] {
        let fixture = Fixture::new(INI, NED);
        fs::remove_dir_all(fixture.0.join("models")).unwrap();
        copy_directory(&examples.join(family), &fixture.0);
        prepare(&fixture.0.join(config)).unwrap_or_else(|e| panic!("baseline {family}: {e:?}"));
        let path = fixture.0.join(json_file);
        let mut json: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        *json.pointer_mut(pointer).unwrap() = json!("日本語");
        let text = format!(
            "\u{feff}{}\r\n",
            serde_json::to_string_pretty(&json)
                .unwrap()
                .replace('\n', "\r\n")
        );
        fs::write(&path, &text).unwrap();
        let diagnostic = prepare(&fixture.0.join(config)).unwrap_err();
        assert_eq!(
            diagnostic.reason, "invalid_type",
            "{family}: {diagnostic:?}"
        );
        assert_eq!(
            diagnostic.target.as_deref(),
            Some(pointer),
            "{family}: {diagnostic:?}"
        );
        assert_eq!(
            diagnostic.source,
            Some(path.to_string_lossy().into_owned()),
            "{family}: {diagnostic:?}"
        );
        assert_range(
            &diagnostic,
            &text,
            text.find("\"日本語\"").unwrap(),
            "\"日本語\"",
        );
        assert!(diagnostic.details.as_ref().unwrap()["actual"].is_string());
        assert!(diagnostic.details.as_ref().unwrap()["expected"].is_string());
        if family == "axi" {
            *json.pointer_mut(pointer).unwrap() = json!(2);
            json["managers"].as_array_mut().unwrap().pop();
            let text = serde_json::to_string_pretty(&json).unwrap();
            fs::write(&path, &text).unwrap();
            let diagnostic = prepare(&fixture.0.join(config)).unwrap_err();
            assert_eq!(diagnostic.target.as_deref(), Some("/"));
            assert_range(&diagnostic, &text, 0, &text);
        }
    }
}

#[test]
fn json_eof_is_a_zero_width_range_after_final_scalar_or_newline() {
    for text in [
        "\u{feff}{\"generators\":[\"日本語\"",
        "\u{feff}{\"generators\":[\"日本語\"\r\n",
    ] {
        let fixture = Fixture::workload(text);
        let diagnostic = fixture.error();
        assert_eq!(diagnostic.reason, "syntax_error");
        assert_range(&diagnostic, text, text.len(), "");
    }
}

#[test]
fn channel_errors_use_ini_section_reference_or_adopted_value() {
    let text = format!("{INI}[Channel Main::missing]\ndelay = 1ps\n");
    let fixture = Fixture::new(&text, NED);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_connection");
    assert_eq!(diagnostic.target.as_deref(), Some("Main::missing"));
    assert_range(
        &diagnostic,
        &text,
        text.find("Main::missing").unwrap(),
        "Main::missing",
    );
    let text = format!("{INI}[Channel Main::a.tx]\ndelay = 1bad\n");
    fs::write(fixture.0.join("run.ini"), &text).unwrap();
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_unit");
    assert_eq!(diagnostic.target.as_deref(), Some("Main::a.tx.delay"));
    assert_range(&diagnostic, &text, text.find("1bad").unwrap(), "1bad");
}
