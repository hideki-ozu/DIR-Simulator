use super::*;

// A deliberately small model policy exercises the common layer independently.
struct TinyRules;
impl ModelRules for TinyRules {
    fn validate_schema(&self, declaration: &Declaration) -> Result<()> {
        match declaration.implementation() {
            Some("tiny.Source") if declaration.simple() => declaration.require_parameters(&[
                ("count", "int", None),
                ("label", "string", None),
                ("enabled", "bool", None),
                ("ratio", "double", None),
                ("size", "int", Some("B")),
            ]),
            Some("tiny.Sink" | "tiny.OtherSink") if declaration.simple() => {
                declaration.require_parameters(&[])
            }
            _ => Err(declaration.fail("unsupported tiny implementation")),
        }
    }
    fn validate_value(
        &self,
        declaration: &Declaration,
        name: &str,
        value: &TypedValue,
    ) -> Result<()> {
        if name == "count" && matches!(value, TypedValue::Integer(n) if *n < 0) {
            return Err(declaration.fail("tiny count must be nonnegative"));
        }
        Ok(())
    }
    fn payload(&self, declaration: &Declaration, _: &str) -> Option<&'static str> {
        match declaration.implementation() {
            Some("tiny.Source" | "tiny.Sink") => Some("tiny.value"),
            Some("tiny.OtherSink") => Some("tiny.other"),
            _ => None,
        }
    }
}
const MODEL: &str = r#"
package tiny;
simple Source {
    parameters:
        @class("tiny.Source");
        int count = default(1);
        string label = default("hello");
        bool enabled = default(true);
        double ratio = default(-0.5);
        int size @unit(B) = default(1KiB);
    gates: output emit;
}
simple Sink { parameters: @class("tiny.Sink"); gates: input accept; }
channel Wire { parameters: @class("dir.link.FixedDelay"); double delay @unit(s) = default(1ns); }
module Box {
    parameters: int index = default(-2);
    gates: output out;
    submodules: source: tiny.Source;
    connections: source.emit --> tiny.Wire --> out;
}
network Main {
    submodules: box: tiny.Box; sink: tiny.Sink;
    connections: box.out --> tiny.Wire --> sink.accept;
}
"#;
fn declarations(model: &str) -> BTreeMap<String, Declaration> {
    parse(model, Path::new("tiny/Main.ned"), "tiny")
        .unwrap()
        .into_iter()
        .map(|d| (d.name.clone(), d))
        .collect()
}

#[test]
fn non_can_rules_resolve_values_compound_paths_and_shared_channels() {
    let types = declarations(MODEL);
    let overrides = BTreeMap::from([
        ("Main.box.source.count".into(), "7".into()),
        ("Main.box.source.label".into(), "\"override\"".into()),
    ]);
    let resolved = resolve(&types, "tiny.Main", &overrides, &TinyRules).unwrap();
    assert_eq!(
        resolved.instances().map(|(id, _)| id).collect::<Vec<_>>(),
        ["Main", "Main.box", "Main.box.source", "Main.sink"]
    );
    assert_eq!(resolved.module_paths(), ["Main.box"]);
    assert!(matches!(
        resolved.values("Main.box")["index"],
        TypedValue::Integer(-2)
    ));
    let values = resolved.values("Main.box.source");
    assert!(matches!(values["count"], TypedValue::Integer(7)));
    assert!(matches!(&values["label"], TypedValue::String(s) if s == "override"));
    assert!(matches!(values["enabled"], TypedValue::Boolean));
    assert!(matches!(values["ratio"], TypedValue::Double));
    assert!(matches!(values["size"], TypedValue::Quantity(1024)));
    let overrides = BTreeMap::from([(
        "Main::box.out".into(),
        BTreeMap::from([("delay".into(), "3ns".into())]),
    )]);
    let channels = resolved.resolve_channels(&overrides, &TinyRules).unwrap();
    assert_eq!(channels.len(), 2);
    let path = resolved.trace("Main.box.source.emit").unwrap();
    assert_eq!(path.end, "Main.sink.accept");
    assert_eq!(path.delay(&channels).unwrap(), 4000);
    let overrides = BTreeMap::from([(
        "Main::box.out".into(),
        BTreeMap::from([("delay".into(), "18446744073709551615ps".into())]),
    )]);
    let channels = resolved.resolve_channels(&overrides, &TinyRules).unwrap();
    assert!(
        path.delay(&channels)
            .unwrap_err()
            .message
            .contains("channel path delay overflow")
    );
}

#[test]
fn non_can_rules_reject_payload_mismatch_through_compound_boundary() {
    let types = declarations(&MODEL.replace("@class(\"tiny.Sink\")", "@class(\"tiny.OtherSink\")"));
    let error = resolve(&types, "tiny.Main", &BTreeMap::new(), &TinyRules)
        .err()
        .unwrap();
    assert!(
        error
            .message
            .contains("incompatible payload path: Main.box.source.emit --> Main.sink.accept"),
        "{}",
        error.message
    );
}

#[test]
fn non_can_rules_validate_unused_declarations_and_defaults_before_overrides() {
    let unused = MODEL.replace("simple Source", "simple Unused");
    let unused = unused[unused.find("simple Unused").unwrap()..unused.find("simple Sink").unwrap()]
        .replace("default(1)", "default(-1)");
    let types = declarations(&format!("{MODEL}\n{unused}"));
    let overrides = BTreeMap::from([("Main.missing.count".into(), "1".into())]);
    let error = resolve(&types, "tiny.Main", &overrides, &TinyRules)
        .err()
        .unwrap();
    assert!(error.message.contains("tiny.Unused"));
    assert!(error.message.contains("tiny count must be nonnegative"));

    let types = declarations(&MODEL.replace("default(1)", "default(-1)"));
    let overrides = BTreeMap::from([("Main.box.source.count".into(), "1".into())]);
    assert!(
        resolve(&types, "tiny.Main", &overrides, &TinyRules)
            .err()
            .unwrap()
            .message
            .contains("tiny count must be nonnegative")
    );

    let types = declarations(&MODEL.replace("int count", "double count"));
    assert!(
        resolve(&types, "tiny.Main", &overrides, &TinyRules)
            .err()
            .unwrap()
            .message
            .contains("parameter schema mismatch: count")
    );
}
