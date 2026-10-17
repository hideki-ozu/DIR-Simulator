//! Build a self-contained local viewer without modifying the result set.
use crate::types::Diagnostic;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HTML: &str = include_str!("viewer/assets/index.html");
const STYLE: &str = include_str!("viewer/assets/style.css");
const MODEL: &str = include_str!("viewer/assets/model.js");
const APP: &str = include_str!("viewer/assets/app.js");
const ETHERNET_MODEL: &str = include_str!("viewer/assets/ethernet-model.js");
const ETHERNET_APP: &str = include_str!("viewer/assets/ethernet-app.js");
const TRANSACTION_MODEL: &str = include_str!("viewer/assets/transaction-model.js");
const TRANSACTION_APP: &str = include_str!("viewer/assets/transaction-app.js");
const CONTROLLED_MODEL: &str = include_str!("viewer/assets/controlled-model.js");
const CONTROLLED_APP: &str = include_str!("viewer/assets/controlled-app.js");

/// A published standalone Viewer and hashes for its fixed inline assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerArtifact {
    pub path: PathBuf,
    /// Lowercase hexadecimal SHA-256 of the complete emitted HTML bytes.
    pub sha256: String,
    /// Quoted CSP source expressions, in executable script order. JSON is excluded.
    pub script_hashes: Vec<String>,
    /// Quoted CSP source expressions for fixed inline styles, excluding dynamic attributes.
    pub style_hashes: Vec<String>,
}

pub(crate) struct RenderedViewer {
    pub(crate) html: String,
    pub(crate) script_hashes: Vec<String>,
    pub(crate) style_hashes: Vec<String>,
}

fn csp_hash(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(44);
    for chunk in digest.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        encoded.push(char::from(ALPHABET[usize::from(first >> 2)]));
        encoded.push(char::from(
            ALPHABET[usize::from(((first & 3) << 4) | (second >> 4))],
        ));
        encoded.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from(((second & 15) << 2) | (third >> 6))])
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(third & 63)])
        } else {
            '='
        });
    }
    format!("'sha256-{encoded}'")
}

fn asset_tag(template: String, tag: &str, replacement: &str) -> Result<String, Diagnostic> {
    if template.matches(tag).count() != 1 {
        return Err(Diagnostic::output(format!(
            "Viewer template must contain exactly one {tag}"
        )));
    }
    Ok(template.replacen(tag, replacement, 1))
}

/// Protect raw-text HTML elements even if trusted assets contain a closing tag in a string.
fn escape_closing_tag(asset: &str, name: &str) -> String {
    let needle = format!("</{name}");
    let lower = asset.to_ascii_lowercase();
    let mut result = String::with_capacity(asset.len());
    let mut offset = 0;
    for (position, _) in lower.match_indices(&needle) {
        result.push_str(&asset[offset..position + 1]);
        result.push('\\');
        result.push_str(&asset[position + 1..position + needle.len()]);
        offset = position + needle.len();
    }
    result.push_str(&asset[offset..]);
    result
}

fn render(result: &Value) -> Result<RenderedViewer, Diagnostic> {
    // Escaping '<' prevents user-controlled JSON from closing its script element.
    // JSON escaping preserves strings exactly when the browser parses the payload.
    let embedded = serde_json::to_string(result)
        .map_err(|e| Diagnostic::prepare(format!("Cannot encode viewer input: {e}")))?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let style = escape_closing_tag(STYLE, "style");
    let style_hashes = vec![csp_hash(style.as_bytes())];
    let mut html = asset_tag(
        HTML.to_owned(),
        "<link rel=\"stylesheet\" href=\"style.css\">",
        &format!("<style>{style}</style>"),
    )?;
    let mut script_hashes = Vec::with_capacity(8);
    for (index, (tag, asset)) in [
        ("<script src=\"model.js\"></script>", MODEL),
        (
            "<script src=\"ethernet-model.js\"></script>",
            ETHERNET_MODEL,
        ),
        ("<script src=\"ethernet-app.js\"></script>", ETHERNET_APP),
        (
            "<script src=\"transaction-model.js\"></script>",
            TRANSACTION_MODEL,
        ),
        (
            "<script src=\"transaction-app.js\"></script>",
            TRANSACTION_APP,
        ),
        (
            "<script src=\"controlled-model.js\"></script>",
            CONTROLLED_MODEL,
        ),
        (
            "<script src=\"controlled-app.js\"></script>",
            CONTROLLED_APP,
        ),
        ("<script src=\"app.js\"></script>", APP),
    ]
    .into_iter()
    .enumerate()
    {
        let script = escape_closing_tag(asset, "script");
        script_hashes.push(csp_hash(script.as_bytes()));
        let mut replacement = String::new();
        if index == 0 {
            replacement.push_str(&format!(
                "<script id=\"embedded-results\" type=\"application/json\">{embedded}</script>\n"
            ));
        }
        replacement.push_str(&format!("<script>{script}</script>"));
        html = asset_tag(html, tag, &replacement)?;
    }
    Ok(RenderedViewer {
        html,
        script_hashes,
        style_hashes,
    })
}

/// Read a schema-1/schema-2 result and atomically publish a new standalone HTML file.
/// The browser's DIRViewerModel validates records and ledgers needed for replay.
pub fn write(input: &Path, output: &Path) -> Result<PathBuf, Diagnostic> {
    write_with_policy(input, output).map(|artifact| artifact.path)
}

/// Publish a Viewer and return hashes of the emitted HTML and fixed inline assets.
/// CSP hashes include quotes and can be joined with spaces for a source list.
/// Styles created dynamically by the Viewer still require inline style permission.
pub fn write_with_policy(input: &Path, output: &Path) -> Result<ViewerArtifact, Diagnostic> {
    let rendered = render_input_with_policy(input)?;
    publish_rendered(rendered, output)
}

/// Common rendering entry for the editor's resource-limited, descriptor-based worker.
pub(crate) fn render_input_with_policy(input: &Path) -> Result<RenderedViewer, Diagnostic> {
    let bytes = fs::read(input).map_err(|e| {
        Diagnostic::prepare(format!("Cannot read viewer input {}: {e}", input.display()))
    })?;
    let result: Value = serde_json::from_slice(&bytes).map_err(|e| {
        Diagnostic::prepare(format!("Invalid result JSON {}: {e}", input.display()))
    })?;
    if !matches!(result["schema_version"].as_u64(), Some(1..=3)) {
        return Err(Diagnostic::prepare(
            "Viewer requires result schema_version 1, 2 or 3",
        ));
    }
    if !result["simulation"].is_object() {
        return Err(Diagnostic::prepare("Result simulation must be an object"));
    }
    if result["schema_version"] == 3
        && (!result["simulation"]["model_records"].is_array()
            || result["metadata"]["model_profile"]
                .as_str()
                .is_none_or(str::is_empty))
    {
        return Err(Diagnostic::prepare(
            "Schema 3 viewer requires declared profile and model records",
        ));
    }
    if result["schema_version"] == 2
        && (!matches!(
            result["metadata"]["model_profile"].as_str(),
            Some(
                "can.cc.multibus.v1"
                    | "ethernet.l2.store-forward.v1"
                    | "ethernet.l2.qos.v1"
                    | "ethernet.l2.vlan.v1"
                    | "ethernet.l2.dynamic.v1"
                    | "can.ethernet.gateway.v1"
                    | "ethernet.tsn.v1"
                    | "can.ethernet.tsn.v1"
                    | "ethernet.redundancy.abstract.v1"
                    | "ethernet.preemption.abstract.v1"
                    | "ethernet.control.wire.experimental.v1"
                    | "ethernet.l2.store-forward.v2"
                    | "ethernet.l2.100base-t1.v1"
                    | "can.fd.precomputed.v1"
                    | "axi4.transaction.v1"
                    | "soc.shared.v1"
                    | "ahb.transaction.v1"
                    | "noc.xy.v1"
                    | "memory.ipc.transaction.v1"
            )
        ) || !result["simulation"]["model_records"].is_array())
    {
        return Err(Diagnostic::prepare(
            "Schema 2 viewer requires supported model records",
        ));
    }
    render(&result)
}

fn publish_rendered(rendered: RenderedViewer, output: &Path) -> Result<ViewerArtifact, Diagnostic> {
    let sha256 = format!("{:x}", Sha256::digest(rendered.html.as_bytes()));
    let io_error = |e: std::io::Error| {
        Diagnostic::output(format!("Cannot write viewer {}: {e}", output.display()))
    };
    let absolute = if output.is_absolute() {
        output.to_owned()
    } else {
        std::env::current_dir().map_err(io_error)?.join(output)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| Diagnostic::output("Viewer output needs an existing parent directory"))?
        .canonicalize()
        .map_err(io_error)?;
    let filename = absolute
        .file_name()
        .ok_or_else(|| Diagnostic::output("Viewer output filename is missing"))?;
    let target = parent.join(filename);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            return Err(Diagnostic::output(
                "Viewer output already exists; existing file was preserved",
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(e)),
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut temporary = None;
    for attempt in 0..32 {
        let path = parent.join(format!(
            ".dir-viewer-{}-{nonce}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_error(e)),
        }
    }
    let (temporary, mut file) = temporary.ok_or_else(|| {
        Diagnostic::output("Cannot reserve temporary viewer file after 32 attempts")
    })?;
    let publish = (|| -> std::io::Result<()> {
        file.write_all(rendered.html.as_bytes())?;
        file.flush()?;
        drop(file);
        // Linking a completed same-filesystem file is atomic and refuses replacement.
        fs::hard_link(&temporary, &target)
    })();
    let _ = fs::remove_file(&temporary);
    publish.map_err(io_error)?;
    Ok(ViewerArtifact {
        path: target,
        sha256,
        script_hashes: rendered.script_hashes,
        style_hashes: rendered.style_hashes,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod policy_tests {
    use super::*;
    use serde_json::json;

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("dir-viewer-policy-{}-{nonce}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn bodies<'a>(html: &'a str, opening: &str, closing: &str) -> Vec<&'a str> {
        html.split(opening)
            .skip(1)
            .map(|rest| rest.split_once(closing).unwrap().0)
            .collect()
    }

    #[test]
    fn csp_hash_matches_known_sha256_base64_vectors() {
        assert_eq!(
            csp_hash(b""),
            "'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='"
        );
        assert_eq!(
            csp_hash(b"abc"),
            "'sha256-ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0='"
        );
    }

    #[test]
    fn emitted_asset_hashes_exclude_and_protect_json() {
        let result = json!({
            "schema_version": 1,
            "simulation": {},
            "label": "</SCRIPT><script>alert('x')</script><!--</style>日本語\u{2028}\u{2029}"
        });
        let rendered = render(&result).unwrap();
        let scripts = bodies(&rendered.html, "<script>", "</script>");
        let styles = bodies(&rendered.html, "<style>", "</style>");
        assert_eq!(scripts.len(), 8);
        assert_eq!(styles.len(), 1);
        assert_eq!(
            scripts
                .iter()
                .map(|body| csp_hash(body.as_bytes()))
                .collect::<Vec<_>>(),
            rendered.script_hashes
        );
        assert_eq!(
            styles
                .iter()
                .map(|body| csp_hash(body.as_bytes()))
                .collect::<Vec<_>>(),
            rendered.style_hashes
        );
        let embedded = bodies(
            &rendered.html,
            "<script id=\"embedded-results\" type=\"application/json\">",
            "</script>",
        );
        assert_eq!(embedded.len(), 1);
        assert!(!embedded[0].contains('<'));
        assert!(!embedded[0].contains(['\u{2028}', '\u{2029}']));
        assert_eq!(serde_json::from_str::<Value>(embedded[0]).unwrap(), result);
        assert_eq!(rendered.html.matches("</script>").count(), 9);
        let different = render(&json!({"schema_version": 2, "simulation": {}})).unwrap();
        assert_eq!(rendered.script_hashes, different.script_hashes);
        assert_eq!(rendered.style_hashes, different.style_hashes);
    }

    #[test]
    fn policy_publication_matches_legacy_and_preserves_existing_files() {
        let temp = Temp::new();
        let input = temp.0.join("results.json");
        let original = br#"{"schema_version":2,"metadata":{"model_profile":"can.cc.multibus.v1"},"simulation":{"model_records":[]}}"#;
        fs::write(&input, original).unwrap();
        let output = temp.0.join("policy.html");
        let artifact = write_with_policy(&input, &output).unwrap();
        assert_eq!(artifact.path, output);
        let html = fs::read(&artifact.path).unwrap();
        assert_eq!(artifact.sha256, format!("{:x}", Sha256::digest(&html)));
        let emitted = String::from_utf8(html.clone()).unwrap();
        assert_eq!(
            artifact.script_hashes,
            bodies(&emitted, "<script>", "</script>")
                .iter()
                .map(|body| csp_hash(body.as_bytes()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            artifact.style_hashes,
            bodies(&emitted, "<style>", "</style>")
                .iter()
                .map(|body| csp_hash(body.as_bytes()))
                .collect::<Vec<_>>()
        );
        let legacy = write(&input, &temp.0.join("legacy.html")).unwrap();
        assert_eq!(fs::read(legacy).unwrap(), html);
        assert!(write_with_policy(&input, &output).is_err());
        assert!(write(&input, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), html);
        assert_eq!(fs::read(&input).unwrap(), original);
    }
}
