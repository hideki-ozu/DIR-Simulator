use super::*;

#[test]
fn raw_text_assets_cannot_close_their_html_element() {
    assert_eq!(
        escape_closing_tag("// </SCRIPT> 日本語 </script>", "script"),
        "// <\\/SCRIPT> 日本語 <\\/script>"
    );
    assert_eq!(
        escape_closing_tag("x::after{content:'</style>'}", "style"),
        "x::after{content:'<\\/style>'}"
    );
}
