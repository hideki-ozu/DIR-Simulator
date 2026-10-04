//! Build a self-contained local viewer without modifying the result set.
use crate::types::Diagnostic;
use serde_json::Value;
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

fn render(result: &Value) -> Result<String, Diagnostic> {
    // Escaping '<' prevents user-controlled JSON from closing its script element.
    // JSON escaping preserves strings exactly when the browser parses the payload.
    let embedded = serde_json::to_string(result)
        .map_err(|e| Diagnostic::prepare(format!("Cannot encode viewer input: {e}")))?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let html = asset_tag(
        HTML.to_owned(),
        "<link rel=\"stylesheet\" href=\"style.css\">",
        &format!("<style>{}</style>", escape_closing_tag(STYLE, "style")),
    )?;
    let html = asset_tag(
        html,
        "<script src=\"model.js\"></script>",
        &format!(
            "<script id=\"embedded-results\" type=\"application/json\">{embedded}</script>\n<script>{}</script>",
            escape_closing_tag(MODEL, "script")
        ),
    )?;
    let html = asset_tag(
        html,
        "<script src=\"ethernet-model.js\"></script>",
        &format!(
            "<script>{}</script>",
            escape_closing_tag(ETHERNET_MODEL, "script")
        ),
    )?;
    let html = asset_tag(
        html,
        "<script src=\"ethernet-app.js\"></script>",
        &format!(
            "<script>{}</script>",
            escape_closing_tag(ETHERNET_APP, "script")
        ),
    )?;
    asset_tag(
        html,
        "<script src=\"app.js\"></script>",
        &format!("<script>{}</script>", escape_closing_tag(APP, "script")),
    )
}

/// Read a schema-1/schema-2 result and atomically publish a new standalone HTML file.
/// The browser's DIRViewerModel validates records and ledgers needed for replay.
pub fn write(input: &Path, output: &Path) -> Result<PathBuf, Diagnostic> {
    let bytes = fs::read(input).map_err(|e| {
        Diagnostic::prepare(format!("Cannot read viewer input {}: {e}", input.display()))
    })?;
    let result: Value = serde_json::from_slice(&bytes).map_err(|e| {
        Diagnostic::prepare(format!("Invalid result JSON {}: {e}", input.display()))
    })?;
    if !matches!(result["schema_version"].as_u64(), Some(1 | 2)) {
        return Err(Diagnostic::prepare(
            "Viewer requires result schema_version 1 or 2",
        ));
    }
    if !result["simulation"].is_object() {
        return Err(Diagnostic::prepare("Result simulation must be an object"));
    }
    if result["schema_version"] == 2
        && (!matches!(
            result["metadata"]["model_profile"].as_str(),
            Some(
                "can.cc.multibus.v1"
                    | "ethernet.l2.store-forward.v1"
                    | "ethernet.l2.qos.v1"
                    | "ethernet.l2.vlan.v1"
            )
        ) || !result["simulation"]["model_records"].is_array())
    {
        return Err(Diagnostic::prepare(
            "Schema 2 viewer requires supported CAN or Ethernet model records",
        ));
    }
    let html = render(&result)?;
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
        file.write_all(html.as_bytes())?;
        file.flush()?;
        drop(file);
        // Linking a completed same-filesystem file is atomic and refuses replacement.
        fs::hard_link(&temporary, &target)
    })();
    let _ = fs::remove_file(&temporary);
    publish.map_err(io_error)?;
    Ok(target)
}

#[cfg(test)]
mod tests;
