use dir_simulator::input::parse_ned;
use std::path::Path;

#[test]
fn baseline_parser_retains_display_unicode_value() {
    let text = "package attributecheck; simple Endpoint { parameters: @class(\"dir.ethernet.Endpoint\"); @display(\"日本語\"); }";
    let parsed = parse_ned(text, Path::new("attributecheck/Endpoint.ned"), "attributecheck").unwrap();
    assert!(format!("{parsed:?}").contains("日本語"), "parsed declaration has discarded the display Unicode value: {parsed:?}");
}
