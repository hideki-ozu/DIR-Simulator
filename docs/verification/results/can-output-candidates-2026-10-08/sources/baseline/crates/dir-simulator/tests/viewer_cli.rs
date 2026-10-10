use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-viewer-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(args)
        .output()
        .unwrap()
}
fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
fn parse(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}
fn fixture() -> Value {
    json!({"schema_version":1,"run_id":"00000000-0000-4000-8000-000000000001","metadata":{"metrics":[]},"simulation":{"termination":"events_exhausted","partial":false,"start_ps":"0","end_ps":"0","last_event_time_ps":null,"committed_events":"0","pending_events":"0","records":[],"summary":[],"requests":[],"receivers":[]}})
}

#[test]
fn help_and_argument_errors() {
    for args in [vec!["--help"], vec!["view", "--help"]] {
        let out = cli(&args);
        assert!(out.status.success());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .contains("view --input results.json --output viewer.html")
        );
    }
    for args in [
        vec!["view"],
        vec!["view", "--input"],
        vec!["view", "--input", "a"],
        vec!["view", "--output", "a"],
        vec!["view", "--config", "a", "--output", "b"],
        vec!["view", "--input", "a", "--input", "b", "--output", "c"],
        vec!["view", "--input", "a", "--output", "b", "--output", "c"],
        vec!["view", "--input", "a", "--output", "b", "--unknown"],
    ] {
        let out = cli(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(parse(&out.stderr)["code"], "E-0001");
    }
}

#[test]
fn local_html_is_self_contained_and_hostile_json_is_inert() {
    let temp = Temp::new();
    let input = temp.0.join("results with spaces.json");
    let output = temp.0.join("viewer with spaces.html");
    let mut result = fixture();
    let hostile = "</script><script>alert('bad')</script> 日本語\u{2028}\u{2029}";
    result["metadata"]["notes"] = json!(hostile);
    let original = serde_json::to_vec(&result).unwrap();
    fs::write(&input, &original).unwrap();
    let out = cli(&["view", "--input", path(&input), "--output", path(&output)]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    let report = parse(&out.stdout);
    let published = PathBuf::from(report["viewer_path"].as_str().unwrap());
    assert!(published.is_absolute());
    assert_eq!(published, output);
    let html = fs::read_to_string(&published).unwrap();
    assert!(html.contains("<style>"));
    assert!(!html.contains("<link rel=\"stylesheet\" href=\"style.css\">"));
    assert!(!html.contains("<script src=\"model.js\"></script>"));
    assert!(!html.contains("<script src=\"app.js\"></script>"));
    assert!(!html.contains("src=\"https://"));
    assert!(!html.contains("href=\"https://"));
    let marker = "<script id=\"embedded-results\" type=\"application/json\">";
    let start = html.find(marker).unwrap() + marker.len();
    let (embedded, rest) = html[start..].split_once("</script>").unwrap();
    assert!(!embedded.contains('<'));
    assert!(embedded.contains("\\u003c/script>"));
    assert!(embedded.contains("\\u2028"));
    assert!(embedded.contains("\\u2029"));
    assert_eq!(parse(embedded.as_bytes()), result);
    assert!(rest.trim_start().starts_with("<script>"));
    assert!(html.contains("DIRViewerModel"));
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
}

#[test]
fn invalid_input_and_unknown_schema_do_not_create_output() {
    let temp = Temp::new();
    let input = temp.0.join("input.json");
    let output = temp.0.join("viewer.html");
    let args = ["view", "--input", path(&input), "--output", path(&output)];
    let out = cli(&args);
    assert_eq!(out.status.code(), Some(2));
    assert!(!output.exists());
    for contents in [
        b"{invalid".as_slice(),
        b"{\"schema_version\":2,\"simulation\":{}}",
        b"{\"schema_version\":1,\"simulation\":[]}",
        b"{\"schema_version\":\"1\",\"simulation\":{}}",
        b"\xff",
    ] {
        fs::write(&input, contents).unwrap();
        let out = cli(&args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(parse(&out.stderr)["code"], "E-0001");
        assert!(!output.exists());
        assert_eq!(fs::read(&input).unwrap(), contents);
    }
}

#[test]
fn output_collisions_equal_input_and_missing_parent_preserve_data() {
    let temp = Temp::new();
    let input = temp.0.join("input.json");
    let output = temp.0.join("viewer.html");
    let original = serde_json::to_vec(&fixture()).unwrap();
    fs::write(&input, &original).unwrap();
    fs::write(&output, b"keep existing viewer").unwrap();
    for target in [&output, &input] {
        let out = cli(&["view", "--input", path(&input), "--output", path(target)]);
        assert_eq!(out.status.code(), Some(4));
        assert_eq!(parse(&out.stderr)["code"], "E-0003");
    }
    assert_eq!(fs::read(&input).unwrap(), original);
    assert_eq!(fs::read(&output).unwrap(), b"keep existing viewer");
    let missing_parent = temp.0.join("missing").join("viewer.html");
    let out = cli(&[
        "view",
        "--input",
        path(&input),
        "--output",
        path(&missing_parent),
    ]);
    assert_eq!(out.status.code(), Some(4));
    assert!(!missing_parent.parent().unwrap().exists());
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn dangling_output_symlink_is_preserved() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let input = temp.0.join("input.json");
    let output = temp.0.join("viewer.html");
    fs::write(&input, serde_json::to_vec(&fixture()).unwrap()).unwrap();
    symlink("missing target", &output).unwrap();
    let out = cli(&["view", "--input", path(&input), "--output", path(&output)]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(fs::read_link(&output).unwrap(), Path::new("missing target"));
}

#[test]
fn relative_output_reports_an_absolute_path() {
    let temp = Temp::new();
    fs::write(
        temp.0.join("results.json"),
        serde_json::to_vec(&fixture()).unwrap(),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dir-simulator"))
        .args(["view", "--input", "results.json", "--output", "viewer.html"])
        .current_dir(&temp.0)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        parse(&out.stdout)["viewer_path"],
        temp.0.join("viewer.html").to_str().unwrap()
    );
}
