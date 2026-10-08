//! Preparation through the public registry preserves the parser's adopted input locations.
use dir_simulator::{
    Diagnostic, input::inspect_config, prepare_with_registry, registry::*, run_config_with_registry,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const NED: &str = "\u{feff}package demo;\r\n// 日本語\r\nsimple Sender { parameters: @class(\"test.Sender\"); string label = default(\"日本\"); int count = default(2); gates: output out; }\r\nsimple Receiver { parameters: @class(\"test.Receiver\"); gates: input in; }\r\nnetwork Demo { submodules: a: demo.Sender; b: demo.Receiver; connections: a.out --> b.in; }\r\n";
const INI: &str = "\u{feff}[General]\r\n# 日本語\r\nnetwork = demo.Demo\r\nned-path = \"ned\"\r\nmodel-profile = \"test.diag.v1\"\r\nsim-time-limit = 20ps\r\n";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(ini: &str, ned: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-registry-diagnostics-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("ned/demo")).unwrap();
        fs::write(path.join("scenario.ini"), ini).unwrap();
        fs::write(path.join("ned/demo/model.ned"), ned).unwrap();
        Self(path)
    }
    fn config(&self) -> PathBuf {
        self.0.join("scenario.ini")
    }
    fn error(&self) -> Diagnostic {
        prepare_with_registry(&self.config(), registry()).unwrap_err()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct NeverConstructed;
impl Model for NeverConstructed {
    fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
        Ok(())
    }
}
fn registry() -> Registry {
    let schema = Schema::new("test.Payload", 1);
    let mut registry = Registry::new();
    registry
        .register_event(EventDescriptor {
            schema: schema.clone(),
            phase: 1,
            validate: |_| Ok(()),
        })
        .unwrap();
    for (key, direction, parameters) in [
        (
            "test.Sender",
            Direction::Output,
            vec![
                ParameterDescriptor {
                    name: "label".into(),
                    ned_type: "string".into(),
                    dimension: Dimension::Dimensionless,
                    minimum: None,
                    maximum: None,
                    required: true,
                },
                ParameterDescriptor {
                    name: "count".into(),
                    ned_type: "int".into(),
                    dimension: Dimension::Dimensionless,
                    minimum: Some(0.0),
                    maximum: Some(10.0),
                    required: true,
                },
            ],
        ),
        ("test.Receiver", Direction::Input, vec![]),
    ] {
        registry
            .register_module(
                key,
                |_| Ok(Box::new(NeverConstructed)),
                ModuleDescriptor {
                    implementation_key: key.into(),
                    implementation_version: "1".into(),
                    parameters,
                    ports: vec![PortDescriptor {
                        name: if direction == Direction::Output {
                            "out"
                        } else {
                            "in"
                        }
                        .into(),
                        direction,
                        schema: schema.clone(),
                    }],
                    events: vec![schema.clone()],
                    resources: vec![],
                },
            )
            .unwrap();
    }
    registry
        .register_profile(ProfileDescriptor {
            name: "test.diag.v1".into(),
            implementation_version: "1".into(),
            modules: vec!["test.Sender".into(), "test.Receiver".into()],
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
fn assert_span(diagnostic: &Diagnostic, path: &Path, text: &str, token: &str, start: usize) {
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
    let (line, column) = point(text, start);
    let (end_line, end_column) = point(text, start + token.len());
    assert_eq!(
        diagnostic.source,
        Some(path.to_string_lossy().into_owned()),
        "{diagnostic:?}"
    );
    assert_eq!(
        (
            diagnostic.line,
            diagnostic.column,
            diagnostic.end_line,
            diagnostic.end_column
        ),
        (Some(line), Some(column), Some(end_line), Some(end_column)),
        "{diagnostic:?}"
    );
    assert!(diagnostic.time_ps.is_none());
    assert!(diagnostic.event_seq.is_none());
}

#[test]
fn generic_execution_settings_use_adopted_ini_token_and_stable_reason() {
    for (key, value, reason) in [
        ("sim-time-limit", "1bad", "invalid_unit"),
        ("max-events", "abc", "invalid_type"),
        ("max-delta-cycles", "0", "invalid_range"),
        ("metrics-window", "0ps", "invalid_range"),
        (
            "model-profile",
            "\"unknown.diag.v1\"",
            "unsupported_profile",
        ),
    ] {
        let ini = if key == "sim-time-limit" {
            INI.replace("20ps", value)
        } else if key == "model-profile" {
            INI.replace("\"test.diag.v1\"", value)
        } else {
            format!("{INI}{key} = \t{value}\r\n")
        };
        let fixture = Fixture::new(&ini, NED);
        let diagnostic = fixture.error();
        assert_eq!(diagnostic.reason, reason, "{key}: {diagnostic:?}");
        assert_eq!(diagnostic.target.as_deref(), Some(key));
        assert_span(
            &diagnostic,
            &fixture.config(),
            &ini,
            value,
            ini.rfind(value).unwrap(),
        );
        assert!(diagnostic.details.as_ref().unwrap()["actual"].is_string());
        assert!(diagnostic.details.as_ref().unwrap()["expected"].is_string());
    }
}

#[test]
fn generic_parameter_bounds_select_default_and_override_sources() {
    let ned = NED.replace("default(2)", "default(19)");
    let fixture = Fixture::new(INI, &ned);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_range");
    assert_eq!(diagnostic.target.as_deref(), Some("demo.Sender.count"));
    assert_span(
        &diagnostic,
        &fixture.0.join("ned/demo/model.ned"),
        &ned,
        "19",
        ned.find("19").unwrap(),
    );
    let ini = format!("{INI}Demo.a.count = 19\r\n");
    let fixture = Fixture::new(&ini, NED);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_range");
    assert_eq!(diagnostic.target.as_deref(), Some("Demo.a.count"));
    assert_span(
        &diagnostic,
        &fixture.config(),
        &ini,
        "19",
        ini.find("19").unwrap(),
    );
}

#[test]
fn unused_registered_declaration_defaults_still_obey_registry_bounds() {
    let unused = "simple Unused { parameters: @class(\"test.Sender\"); string label = default(\"日本\"); int count = default(19); gates: output out; }\r\n";
    let ned = format!("{NED}{unused}");
    let fixture = Fixture::new(INI, &ned);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_range");
    assert_eq!(diagnostic.target.as_deref(), Some("demo.Unused.count"));
    assert_span(
        &diagnostic,
        &fixture.0.join("ned/demo/model.ned"),
        &ned,
        "19",
        ned.rfind("19").unwrap(),
    );
}

#[test]
fn generic_unknown_types_and_declaration_schema_retain_ned_sources() {
    let ned = NED.replace("a: demo.Sender", "a: demo.Missing");
    let fixture = Fixture::new(INI, &ned);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "unknown_type");
    assert_span(
        &diagnostic,
        &fixture.0.join("ned/demo/model.ned"),
        &ned,
        "demo.Missing",
        ned.find("demo.Missing").unwrap(),
    );
    let ned = NED.replace("int count", "double count");
    let fixture = Fixture::new(INI, &ned);
    let diagnostic = fixture.error();
    assert_eq!(diagnostic.reason, "invalid_type");
    assert_eq!(diagnostic.target.as_deref(), Some("demo.Sender.count"));
    let token = "double count = default(2);";
    assert_span(
        &diagnostic,
        &fixture.0.join("ned/demo/model.ned"),
        &ned,
        token,
        ned.find(token).unwrap(),
    );
}

#[test]
fn structural_config_reference_errors_use_known_value_spans() {
    for (key, value) in [
        ("ned-path", "not_quoted"),
        ("workload", "not_quoted"),
        ("model-config", "not_quoted"),
        ("model-profile", "not_quoted"),
    ] {
        let text = if key == "ned-path" {
            INI.replace("\"ned\"", value)
        } else if key == "model-profile" {
            INI.replace("\"test.diag.v1\"", value)
        } else {
            format!("{INI}{key} = {value}\r\n")
        };
        let path = Path::new("/virtual/設定.ini");
        let diagnostic = inspect_config(&text, path, Path::new("/virtual")).unwrap_err();
        assert_eq!(diagnostic.reason, "invalid_type");
        assert_eq!(diagnostic.target.as_deref(), Some(key));
        assert_span(&diagnostic, path, &text, value, text.find(value).unwrap());
    }
}

#[test]
fn generic_run_publishes_same_annotated_preparation_diagnostic() {
    let ini = format!("{INI}Demo.a.count = 19\r\n");
    let fixture = Fixture::new(&ini, NED);
    let output = fixture.0.join("output");
    let report = run_config_with_registry(&fixture.config(), &output, registry()).unwrap();
    assert_eq!(report.exit_code, 2);
    let diagnostic = report.primary_diagnostic.unwrap();
    assert_eq!(diagnostic.reason, "invalid_range");
    assert_span(
        &diagnostic,
        &fixture.config(),
        &ini,
        "19",
        ini.find("19").unwrap(),
    );
    let persisted: Value =
        serde_json::from_slice(&fs::read(output.join("diagnostics.jsonl")).unwrap()).unwrap();
    assert_eq!(persisted, serde_json::to_value(diagnostic).unwrap());
    assert_eq!(persisted["seq"], "0");
    assert_eq!(persisted["primary"], true);
}

#[test]
fn unused_registered_double_and_channel_defaults_obey_bounds() {
    struct NeverChannel;
    impl Channel for NeverChannel {
        fn capability(&self, _: &Schema, _: u64, _: &[u8]) -> ModelResult<ParameterValue> {
            Ok(ParameterValue::Quantity(0))
        }
    }
    for (declaration, token, key) in [
        (
            "simple Unused { parameters: @class(\"test.Fraction\"); double ratio = default(2.5); }",
            "2.5",
            "demo.Unused.ratio",
        ),
        (
            "channel Unused { parameters: @class(\"test.Limited\"); double delay @unit(s) = default(19ps); }",
            "19ps",
            "demo.Unused.delay",
        ),
    ] {
        let mut r = registry();
        r.register_module(
            "test.Fraction",
            |_| Ok(Box::new(NeverConstructed)),
            ModuleDescriptor {
                implementation_key: "test.Fraction".into(),
                implementation_version: "1".into(),
                parameters: vec![ParameterDescriptor {
                    name: "ratio".into(),
                    ned_type: "double".into(),
                    dimension: Dimension::Dimensionless,
                    minimum: Some(0.0),
                    maximum: Some(1.0),
                    required: true,
                }],
                ports: vec![],
                events: vec![],
                resources: vec![],
            },
        )
        .unwrap();
        r.register_channel(
            "test.Limited",
            |_| Ok(Box::new(NeverChannel)),
            ChannelDescriptor {
                implementation_key: "test.Limited".into(),
                implementation_version: "1".into(),
                parameters: vec![ParameterDescriptor {
                    name: "delay".into(),
                    ned_type: "double".into(),
                    dimension: Dimension::Time,
                    minimum: Some(0.0),
                    maximum: Some(10.0),
                    required: true,
                }],
                capabilities: vec![],
            },
        )
        .unwrap();
        let ned = format!("{NED}{declaration}\r\n");
        let fixture = Fixture::new(INI, &ned);
        let diagnostic = prepare_with_registry(&fixture.config(), r).unwrap_err();
        assert_eq!(diagnostic.reason, "invalid_range");
        assert_eq!(diagnostic.target.as_deref(), Some(key));
        assert_span(
            &diagnostic,
            &fixture.0.join("ned/demo/model.ned"),
            &ned,
            token,
            ned.rfind(token).unwrap(),
        );
    }
}

#[test]
fn static_input_callback_failure_does_not_invent_source_or_target() {
    let ini = format!("{INI}model-config = \"model.json\"\r\nworkload = \"workload.json\"\r\n");
    let fixture = Fixture::new(&ini, NED);
    fs::write(fixture.0.join("model.json"), "{}").unwrap();
    fs::write(fixture.0.join("workload.json"), "{}").unwrap();
    let mut r = registry();
    r.register_profile_input_validator("test.diag.v1", |_| Err("callback rejected input".into()))
        .unwrap();
    let diagnostic = prepare_with_registry(&fixture.config(), r).unwrap_err();
    assert_eq!(diagnostic.reason, "model_config_invalid");
    assert!(diagnostic.source.is_none());
    assert!(diagnostic.line.is_none());
    assert!(diagnostic.column.is_none());
    assert!(diagnostic.target.is_none());
    assert_eq!(
        diagnostic.details.as_ref().unwrap()["operation"],
        "validate_profile_input"
    );
}
