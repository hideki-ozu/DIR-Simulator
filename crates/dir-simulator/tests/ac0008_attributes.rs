//! Issue #41: exercise the public parser metadata interface, separately from execution.
use dir_simulator::input::{Attribute, AttributeOwner, parse_ned};
use std::path::Path;

const PATH: &str = "/attributecheck/Endpoint.ned";
const FIXTURE: &str = include_str!(
    "../../../docs/verification/results/ac0008-review-2026-10-10/attribute-metadata.ned"
);
const POSITIONED: &str = concat!(
    "\u{feff}package attributecheck;\r\n",
    "simple Endpoint {\r\n",
    " parameters:\r\n",
    "  @display(\"日本語\"); @description(\"送信\\n受信\");\r\n",
    "  int queueCapacity @display(\"容量\") @description(\"件数\") = default(64);\r\n",
    "}\r\n"
);
fn assert_attribute(attribute: &Attribute, name: &str, value: &str, parameter: Option<&str>) {
    assert_eq!(attribute.name(), name);
    assert_eq!(attribute.value(), value);
    let owner = if let Some(parameter) = parameter {
        AttributeOwner::Parameter {
            qname: "attributecheck.Endpoint".into(),
            parameter: parameter.into(),
        }
    } else {
        AttributeOwner::Type {
            qname: "attributecheck.Endpoint".into(),
        }
    };
    assert_eq!(attribute.owner(), &owner);
    assert_eq!(attribute.span().source, PATH);
}
fn assert_span(attribute: &Attribute, expected: (usize, usize, usize, usize, usize)) {
    let span = attribute.span();
    let (start_byte, end_byte, line, column, end_column) = expected;
    assert_eq!((span.start_byte, span.end_byte), (start_byte, end_byte));
    assert_eq!((span.line, span.column), (line, column));
    assert_eq!((span.end_line, span.end_column), (line, end_column));
}

fn assert_property_diagnostic(text: &str, property: &str, message: &str) {
    let diagnostic = parse_ned(text, Path::new(PATH), "attributecheck").unwrap_err();
    let start = text.rfind(property).unwrap();
    let prefix = text[..start].trim_start_matches('\u{feff}');
    let line = prefix.matches('\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap().chars().count() + 1;
    assert_eq!(diagnostic.code, "E-0001");
    assert_eq!(diagnostic.reason, "syntax_error");
    assert_eq!(diagnostic.source.as_deref(), Some(PATH));
    assert_eq!(
        (diagnostic.line, diagnostic.column),
        (Some(line), Some(column))
    );
    assert_eq!(
        (diagnostic.end_line, diagnostic.end_column),
        (Some(line), Some(column + property.chars().count()))
    );
    assert_eq!(
        diagnostic.message,
        format!("{PATH}:{line}:{column}: {message} (got @)")
    );
    assert_eq!(diagnostic.details.as_ref().unwrap()["actual"], "@");
    assert_eq!(diagnostic.details.as_ref().unwrap()["expected"], message);
}

#[test]
fn public_attributes_retain_unicode_escapes_and_typed_owners_with_exact_bom_crlf_spans() {
    let parsed = parse_ned(POSITIONED, Path::new(PATH), "attributecheck").unwrap();
    let declaration = &parsed.declarations()[0];
    assert_eq!(declaration.name(), "attributecheck.Endpoint");
    let display = &declaration.attributes()["display"];
    assert_attribute(display, "display", "日本語", None);
    assert_span(display, (63, 84, 4, 3, 18));
    let description = &declaration.attributes()["description"];
    assert_attribute(description, "description", "送信\n受信", None);
    assert_span(description, (86, 116, 4, 20, 42));
    let parameter = &declaration.parameters()["queueCapacity"];
    let display = &parameter.attributes()["display"];
    assert_attribute(display, "display", "容量", Some("queueCapacity"));
    assert_span(display, (139, 157, 5, 21, 35));
    let description = &parameter.attributes()["description"];
    assert_attribute(description, "description", "件数", Some("queueCapacity"));
    assert_span(description, (158, 180, 5, 36, 54));
    assert_eq!(parameter.default(), Some("64"));
    assert_eq!(parameter.scalar(), "int");
    assert_eq!(parameter.unit(), None);
}

#[test]
fn supplied_fixture_keeps_grammar_rejection_and_valid_order_retains_quote_backslash() {
    // The supplied oracle places an attribute after default; the specification
    // requires parameter properties before default. Keep that input rejected.
    let rejected = parse_ned(FIXTURE, Path::new(PATH), "attributecheck").unwrap_err();
    assert_eq!(rejected.reason, "syntax_error");
    assert_eq!((rejected.line, rejected.column), (Some(7), Some(35)));
    let corrected = FIXTURE.replace(
        "= default(64) @description(\"parameter 所有者\")",
        "@description(\"parameter 所有者\") = default(64)",
    );
    let parsed = parse_ned(&corrected, Path::new(PATH), "attributecheck").unwrap();
    let declaration = &parsed.declarations()[0];
    assert_attribute(
        &declaration.attributes()["display"],
        "display",
        "日本語 \"引用\" \\",
        None,
    );
    assert_attribute(
        &declaration.attributes()["description"],
        "description",
        "配送期限",
        None,
    );
    assert_attribute(
        &declaration.parameters()["queueCapacity"].attributes()["description"],
        "description",
        "parameter 所有者",
        Some("queueCapacity"),
    );
    assert_eq!(
        parsed.declarations()[1].children(),
        [
            ("a".into(), "attributecheck.Endpoint".into()),
            ("b".into(), "attributecheck.Endpoint".into()),
        ]
    );
    let escaped = corrected.replace(
        r#"@description("配送期限")"#,
        r#"@description("説明 \"引用\" \\")"#,
    );
    let parsed = parse_ned(&escaped, Path::new(PATH), "attributecheck").unwrap();
    let description = &parsed.declarations()[0].attributes()["description"];
    assert_attribute(description, "description", "説明 \"引用\" \\", None);
    assert_span(description, (139, 175, 6, 3, 31));
}

#[test]
fn comments_inside_strings_are_data_and_class_unit_values_keep_existing_accessors() {
    let text = concat!(
        "package /*x*/ attributecheck;\n",
        "simple Endpoint { parameters:\n",
        " @class(\"dir.ethernet.Endpoint\");\n",
        " @description(\"// /* = ; # */\");\n",
        " double delay @ unit ( s ) @display(\"秒\\t時間\\r終了\") = default(0 /*x*/ ps);\n",
        "}\n",
    );
    let parsed = parse_ned(text, Path::new(PATH), "attributecheck").unwrap();
    let declaration = &parsed.declarations()[0];
    assert_attribute(
        &declaration.attributes()["description"],
        "description",
        "// /* = ; # */",
        None,
    );
    assert_attribute(
        &declaration.attributes()["class"],
        "class",
        "dir.ethernet.Endpoint",
        None,
    );
    assert_eq!(declaration.implementation(), Some("dir.ethernet.Endpoint"));
    assert_span(&declaration.attributes()["class"], (61, 92, 3, 2, 33));
    let parameter = &declaration.parameters()["delay"];
    assert_attribute(&parameter.attributes()["unit"], "unit", "s", Some("delay"));
    assert_span(&parameter.attributes()["unit"], (141, 153, 5, 15, 27));
    assert_attribute(
        &parameter.attributes()["display"],
        "display",
        "秒\t時間\r終了",
        Some("delay"),
    );
    assert_eq!(parameter.unit(), Some("s"));
    assert_eq!(parameter.default(), Some("0 ps"));
    assert_eq!(parameter.scalar(), "double");
}

#[test]
fn duplicate_metadata_unknown_property_and_invalid_escape_are_rejected() {
    for property in ["display", "description"] {
        for parameter in [false, true] {
            let repeated = format!("@{property}(\"one\") @{property}(\"two\")");
            let body = if parameter {
                format!("int count {repeated};")
            } else {
                format!("@{property}(\"one\"); @{property}(\"two\");")
            };
            let text = format!("package attributecheck; simple Endpoint {{ parameters: {body} }}");
            let token = format!("@{property}(\"two\")");
            assert_property_diagnostic(&text, &token, &format!("duplicate property {property}"));
        }
    }
    for (body, token, message) in [
        (
            r#"@class("one"); @class("two");"#,
            r#"@class("two")"#,
            "duplicate property class",
        ),
        (
            "int count @unit(s) @unit(s);",
            "@unit(s)",
            "duplicate property unit",
        ),
    ] {
        let text = format!("package attributecheck; simple Endpoint {{ parameters: {body} }}");
        assert_property_diagnostic(&text, token, message);
    }
    let text = concat!(
        "\u{feff}package attributecheck;\r\n",
        "simple Endpoint {\r\n",
        " parameters:\r\n",
        "  bool enabled @display(\"日本語\") @unit(s) = default(true);\r\n",
        "}\r\n",
    );
    assert_property_diagnostic(text, "@unit(s)", "unit on nonnumeric parameter");
    let text = "package attributecheck; simple Endpoint { parameters: @unknown(\"x\"); }";
    let diagnostic = parse_ned(text, Path::new(PATH), "attributecheck").unwrap_err();
    assert_eq!(diagnostic.code, "E-0001");
    assert_eq!(diagnostic.reason, "unsupported_syntax");
    assert_eq!(
        (diagnostic.line, diagnostic.column, diagnostic.end_column),
        (Some(1), Some(56), Some(63))
    );
    for value in [r#""invalid\q""#, r#""invalid\u1234""#] {
        let text =
            format!("package attributecheck; simple Endpoint {{ parameters: @display({value}); }}");
        let diagnostic = parse_ned(&text, Path::new(PATH), "attributecheck").unwrap_err();
        assert_eq!(diagnostic.code, "E-0001");
        assert_eq!(diagnostic.reason, "syntax_error");
        assert!(diagnostic.message.contains("invalid string escape"));
        assert_eq!(
            (diagnostic.line, diagnostic.column, diagnostic.end_column),
            (Some(1), Some(64), Some(65))
        );
    }
}
