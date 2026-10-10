//! Snapshot-based input orchestration, file loading and common INI/JSON/time utilities.
//! NED resolution and model-specific validation are delegated to the input adapters.
mod axi;
mod can;
pub(crate) mod can_ethernet;
mod canfd;
pub(crate) mod ethernet;
mod gateway;
mod json_diagnostics;
mod memory_ipc;
mod network;
mod provenance;
use json_diagnostics::JsonDocument;
pub(crate) use json_diagnostics::parse_json;
pub(crate) use provenance::effective_value_provenance;
pub use provenance::{PreparedProvenance, RunIdentity, capture_provenance};
pub(crate) mod ned;
mod soc;

use crate::types::{Diagnostic, InputSnapshot, PreparedSimulation, SourceSpan};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Filesystem operations used by the shared input pipeline.
/// Paths are normalized absolute logical paths. Directory names are immediate children.
pub trait InputSource {
    fn metadata(&self, path: &Path) -> io::Result<InputMetadata>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<InputDirEntry>>;
    fn read_utf8(&self, path: &Path) -> io::Result<String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputKind {
    File,
    Directory,
    Symlink,
    Other,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputMetadata {
    pub kind: InputKind,
}
#[derive(Debug)]
pub struct InputDirEntry {
    pub name: OsString,
    pub kind: io::Result<InputKind>,
}

/// The current on-disk input implementation; metadata never follows a symlink.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsInputSource;
fn input_kind(kind: fs::FileType) -> InputKind {
    if kind.is_symlink() {
        InputKind::Symlink
    } else if kind.is_dir() {
        InputKind::Directory
    } else if kind.is_file() {
        InputKind::File
    } else {
        InputKind::Other
    }
}
impl InputSource for FsInputSource {
    fn metadata(&self, path: &Path) -> io::Result<InputMetadata> {
        Ok(InputMetadata {
            kind: input_kind(fs::symlink_metadata(path)?.file_type()),
        })
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<InputDirEntry>> {
        // Keep per-entry type errors deferred until the shared sorted traversal.
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok(InputDirEntry {
                    name: entry.file_name(),
                    kind: entry.file_type().map(input_kind),
                })
            })
            .collect()
    }
    fn read_utf8(&self, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }
}

/// Structural INI inspection without execution/profile/quantity validation.
#[derive(Clone, Debug)]
pub struct ProjectHeader {
    pub config: PathBuf,
    pub cwd: PathBuf,
    pub network: Option<String>,
    pub profile: Option<String>,
    pub roots: Vec<PathBuf>,
    pub workload: Option<PathBuf>,
    pub model_config: Option<PathBuf>,
    pub general: BTreeMap<String, String>,
    pub channels: BTreeMap<String, BTreeMap<String, String>>,
}

pub use ned::{Attribute, AttributeOwner, Connection, Declaration, Parameter};

/// Owned syntax result. Declaration data is available only through immutable getters.
#[derive(Clone, Debug)]
pub struct ParsedNed {
    declarations: Vec<Declaration>,
}
impl ParsedNed {
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }
}
pub fn parse_ned(text: &str, path: &Path, expected_package: &str) -> Result<ParsedNed> {
    ned::parse(text, path, expected_package).map(|declarations| ParsedNed { declarations })
}
pub(crate) fn validate_parameter_literal(
    declaration: &Declaration,
    name: &str,
    value: &str,
    profile: &str,
) -> Result<()> {
    if matches!(
        profile,
        "ethernet.l2.store-forward.v1"
            | "ethernet.l2.qos.v1"
            | "ethernet.l2.vlan.v1"
            | "ethernet.l2.dynamic.v1"
            | "ethernet.tsn.v1"
            | "ethernet.l2.store-forward.v2"
            | "ethernet.l2.100base-t1.v1"
    ) {
        ethernet::validate_parameter_literal(declaration, name, value)
    } else if profile == "can.ethernet.gateway.v1" {
        network::topology::validate_parameter_literal(declaration, name, value)
    } else if profile == "can.fd.precomputed.v1" {
        canfd::validate_parameter_literal(declaration, name, value)
    } else if profile == "axi4.transaction.v1" {
        axi::validate_parameter_literal(declaration, name, value)
    } else if matches!(
        profile,
        "soc.shared.v1" | "ahb.transaction.v1" | "noc.xy.v1"
    ) {
        soc::validate_parameter_literal(declaration, name, value, profile)
    } else if profile == "memory.ipc.transaction.v1" {
        memory_ipc::validate_parameter_literal(declaration, name, value)
    } else {
        can::validate_parameter_literal(declaration, name, value, profile)
    }
}

type Result<T> = std::result::Result<T, Diagnostic>;
fn error(message: impl Into<String>) -> Diagnostic {
    json_diagnostics::contextual(Diagnostic::prepare(message))
}

pub(crate) fn identifier(s: &str) -> bool {
    let mut chars = s.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
pub(crate) fn reserved(s: &str) -> bool {
    matches!(
        s,
        "package"
            | "simple"
            | "module"
            | "network"
            | "channel"
            | "parameters"
            | "gates"
            | "submodules"
            | "connections"
            | "int"
            | "double"
            | "bool"
            | "string"
            | "input"
            | "output"
            | "default"
            | "true"
            | "false"
            | "import"
            | "extends"
            | "like"
            | "interfaces"
            | "types"
            | "inout"
            | "allowunconnected"
            | "for"
            | "if"
            | "moduleinterface"
            | "channelinterface"
            | "volatile"
            | "xml"
            | "object"
    )
}
fn path_name(s: &str) -> bool {
    s.split('.').all(|part| identifier(part) && !reserved(part))
}

/// Parse exact decimal time without binary floating point or rounding.
pub fn parse_time(value: &str) -> Result<u64> {
    quantity(value, "s")
}

fn decimal_parts(value: &str, negative_allowed: bool) -> Result<(&str, &str, bool, &str)> {
    let value = value.trim_matches([' ', '\t']);
    let (number, negative) = if let Some(rest) = value.strip_prefix('-') {
        (rest, true)
    } else {
        (value, false)
    };
    if negative && !negative_allowed {
        return Err(error(format!("negative quantity: {value}"))
            .with_reason("invalid_range")
            .with_detail("actual", value)
            .with_detail("expected", "representable positive integer or quantity"));
    }
    let integer_end = number.bytes().take_while(u8::is_ascii_digit).count();
    let integer = &number[..integer_end];
    if integer.is_empty() || (integer.len() > 1 && integer.starts_with('0')) {
        return Err(error(format!("invalid decimal literal: {value}"))
            .with_reason("invalid_type")
            .with_detail("actual", value)
            .with_detail("expected", "declared scalar type"));
    }
    let mut remainder = &number[integer_end..];
    let fraction = if let Some(rest) = remainder.strip_prefix('.') {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return Err(error(format!("invalid decimal literal: {value}"))
                .with_reason("invalid_type")
                .with_detail("actual", value)
                .with_detail("expected", "declared scalar type"));
        }
        remainder = &rest[n..];
        &rest[..n]
    } else {
        ""
    };
    if negative && integer.bytes().chain(fraction.bytes()).all(|c| c == b'0') {
        return Err(error(format!(
            "negative zero is not a double literal: {value}"
        )));
    }
    Ok((
        integer,
        fraction,
        negative,
        remainder.trim_start_matches([' ', '\t']),
    ))
}

pub(crate) fn quantity(value: &str, unit: &str) -> Result<u64> {
    let (integer, fraction, _, suffix) = decimal_parts(value, false)?;
    let coefficient: u64 = match (unit, suffix) {
        ("s", "ps") => 1,
        ("s", "ns") => 1_000,
        ("s", "us") => 1_000_000,
        ("s", "ms") => 1_000_000_000,
        ("s", "s") => 1_000_000_000_000,
        ("bps", "bps") => 1,
        ("bps", "kbps") => 1_000,
        ("bps", "Mbps") => 1_000_000,
        ("bps", "Gbps") => 1_000_000_000,
        ("B", "B") => 1,
        ("B", "kB") => 1_000,
        ("B", "MB") => 1_000_000,
        ("B", "GB") => 1_000_000_000,
        ("B", "KiB") => 1_024,
        ("B", "MiB") => 1_048_576,
        ("B", "GiB") => 1_073_741_824,
        _ => {
            return Err(error(format!("expected a {unit} quantity, got {value}"))
                .with_reason("invalid_unit")
                .with_detail("actual", value)
                .with_detail("expected", format!("{unit} quantity")));
        }
    };
    // Multiplication of a decimal digit string avoids intermediate range limits.
    let mut digits: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes())
        .map(|c| c - b'0')
        .collect();
    let mut carry = 0u64;
    for digit in digits.iter_mut().rev() {
        let n = *digit as u64 * coefficient + carry;
        *digit = (n % 10) as u8;
        carry = n / 10;
    }
    let mut prefix = Vec::new();
    while carry > 0 {
        prefix.push((carry % 10) as u8);
        carry /= 10;
    }
    prefix.reverse();
    prefix.extend(digits);
    let split = prefix.len() - fraction.len();
    if prefix[split..].iter().any(|&digit| digit != 0) {
        return Err(error(format!("quantity requires rounding: {value}"))
            .with_reason("invalid_range")
            .with_detail("actual", value)
            .with_detail("expected", "representable positive integer or quantity"));
    }
    prefix[..split].iter().try_fold(0u64, |n, &digit| {
        n.checked_mul(10)
            .and_then(|n| n.checked_add(digit as u64))
            .ok_or_else(|| {
                error(format!("quantity exceeds u64: {value}"))
                    .with_reason("invalid_range")
                    .with_detail("actual", value)
                    .with_detail("expected", "representable positive integer or quantity")
            })
    })
}

pub(crate) fn unsigned(value: &str, positive: bool) -> Result<u64> {
    if value.is_empty()
        || !value.bytes().all(|c| c.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(error(format!("expected decimal integer: {value}"))
            .with_reason("invalid_type")
            .with_detail("actual", value)
            .with_detail("expected", "declared scalar type"));
    }
    let n: u64 = value.parse().map_err(|_| {
        error(format!("integer exceeds u64: {value}"))
            .with_reason("invalid_range")
            .with_detail("actual", value)
            .with_detail("expected", "representable positive integer or quantity")
    })?;
    if positive && n == 0 {
        return Err(error(format!("expected positive integer: {value}"))
            .with_reason("invalid_range")
            .with_detail("actual", value)
            .with_detail("expected", "representable positive integer or quantity"));
    }
    Ok(n)
}

pub(crate) fn string_literal(value: &str) -> Result<String> {
    let mut chars = value.chars();
    if chars.next() != Some('"') {
        return Err(error(format!("expected quoted string: {value}"))
            .with_reason("invalid_type")
            .with_detail("actual", value)
            .with_detail("expected", "declared scalar type"));
    }
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if chars.next().is_some() {
                    return Err(error(format!("trailing string input: {value}"))
                        .with_reason("invalid_type")
                        .with_detail("actual", value)
                        .with_detail("expected", "declared scalar type"));
                }
                return Ok(out);
            }
            '\\' => out.push(match chars.next() {
                Some('"') => '"',
                Some('\\') => '\\',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                _ => return Err(error("invalid string escape").with_reason("invalid_type")),
            }),
            c if c <= '\u{1f}' || c == '\u{7f}' => {
                return Err(
                    error("unescaped control character in string").with_reason("invalid_type")
                );
            }
            c => out.push(c),
        }
    }
    Err(error("unterminated quoted string").with_reason("syntax_error"))
}

fn quoted_paths(value: &str) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    let mut rest = value.trim_matches([' ', '\t']);
    loop {
        if !rest.starts_with('"') {
            return Err(
                error("ned-path requires quoted nonempty paths").with_reason("invalid_type")
            );
        }
        let mut escaped = false;
        let mut end = None;
        for (i, c) in rest.char_indices().skip(1) {
            if !escaped && c == '"' {
                end = Some(i + 1);
                break;
            }
            escaped = !escaped && c == '\\';
        }
        let end =
            end.ok_or_else(|| error("unterminated ned-path string").with_reason("syntax_error"))?;
        let path = string_literal(&rest[..end])?;
        valid_file_path(&path)?;
        paths.push(path);
        rest = rest[end..].trim_matches([' ', '\t']);
        if rest.is_empty() {
            return Ok(paths);
        }
        rest = rest
            .strip_prefix(';')
            .ok_or_else(|| error("invalid ned-path separator").with_reason("syntax_error"))?
            .trim_start_matches([' ', '\t']);
    }
}
fn valid_file_path(path: &str) -> Result<()> {
    if path.is_empty() || path.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(
            error("path is empty or contains control characters").with_reason("invalid_type")
        );
    }
    Ok(())
}
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}
pub(crate) fn absolute(path: &Path, base: &Path) -> PathBuf {
    normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}
pub(crate) fn no_symlinks(path: &Path, source: &dyn InputSource) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = source.metadata(&current).map_err(|e| {
            error(format!("{}: {e}", current.display()))
                .with_reason("input_unreadable")
                .with_source(path)
                .with_detail("operation", "metadata")
                .with_detail("os_error", e)
        })?;
        if metadata.kind == InputKind::Symlink {
            return Err(error(format!(
                "symlink input is unsupported: {}",
                current.display()
            )));
        }
    }
    Ok(())
}
pub(crate) fn read_file(
    path: &Path,
    snapshots: &mut Vec<InputSnapshot>,
    source: &dyn InputSource,
) -> Result<String> {
    no_symlinks(path, source)?;
    if source
        .metadata(path)
        .map_err(|e| {
            error(format!("{}: {e}", path.display()))
                .with_reason("input_unreadable")
                .with_source(path)
                .with_detail("operation", "metadata")
                .with_detail("os_error", e)
        })?
        .kind
        != InputKind::File
    {
        return Err(error(format!(
            "input is not a regular file: {}",
            path.display()
        )));
    }
    let content = source.read_utf8(path).map_err(|e| {
        error(format!("{}: {e}", path.display()))
            .with_reason(if e.kind() == io::ErrorKind::InvalidData {
                "invalid_utf8"
            } else {
                "input_unreadable"
            })
            .with_source(path)
            .with_detail("operation", "read")
            .with_detail("os_error", e)
    })?;
    snapshots.push(InputSnapshot {
        path: path.to_path_buf(),
        content: content.clone(),
    });
    Ok(content)
}

#[derive(Default)]
struct Ini {
    general: BTreeMap<String, String>,
    channels: BTreeMap<String, BTreeMap<String, String>>,
    spans: BTreeMap<String, (SourceSpan, SourceSpan)>,
    section_spans: BTreeMap<String, SourceSpan>,
    eof: Option<SourceSpan>,
}
impl Ini {
    fn parse(content: &str, path: &Path) -> Result<Self> {
        let mut ini = Self {
            eof: Some(SourceSpan::new(path, content, content.len(), content.len())),
            ..Self::default()
        };
        let mut section: Option<String> = None;
        let mut saw_general = false;
        let bom = if content.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let mut offset = bom;
        for original_line in content[bom..].split_inclusive('\n') {
            let original = original_line
                .strip_suffix('\n')
                .map(|s| s.strip_suffix('\r').unwrap_or(s))
                .unwrap_or(original_line);
            let line = original.trim_matches([' ', '\t']);
            let start = offset + original.len() - original.trim_start_matches([' ', '\t']).len();
            offset += original_line.len();
            let fail = |message: String, reason: &str, token_start: usize, token_end: usize| {
                error(format!("{}: {message}", path.display()))
                    .with_reason(reason)
                    .with_span(path, content, token_start, token_end)
            };
            if let Some(position) = line.find('\r') {
                return Err(fail(
                    "bare carriage return".into(),
                    "syntax_error",
                    start + position,
                    start + position + 1,
                ));
            }
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if line.starts_with('[') {
                if line == "[General]" && !saw_general && section.is_none() {
                    saw_general = true;
                    section = Some(String::new());
                } else if saw_general && line.starts_with("[Channel ") && line.ends_with(']') {
                    let id = &line[9..line.len() - 1];
                    let valid = id.split_once("::").is_some_and(|(parent, endpoint)| {
                        path_name(parent) && path_name(endpoint) && endpoint.split('.').count() <= 2
                    });
                    if !valid {
                        return Err(fail(
                            format!("invalid channel identifier: {id}"),
                            "invalid_connection",
                            start + 9,
                            start + line.len() - 1,
                        )
                        .with_target(id));
                    }
                    if ini.channels.insert(id.into(), BTreeMap::new()).is_some() {
                        return Err(fail(
                            format!("duplicate channel section: {id}"),
                            "duplicate_definition",
                            start + 9,
                            start + line.len() - 1,
                        )
                        .with_target(id));
                    }
                    ini.section_spans.insert(
                        id.into(),
                        SourceSpan::new(path, content, start + 9, start + line.len() - 1),
                    );
                    section = Some(id.into());
                } else {
                    return Err(fail(
                        format!("unknown or duplicate section: {line}"),
                        "syntax_error",
                        start,
                        start + line.len(),
                    ));
                }
                continue;
            }
            let section = section.as_ref().ok_or_else(|| {
                fail(
                    "key before [General]".into(),
                    "syntax_error",
                    start,
                    start + line.len(),
                )
            })?;
            let (raw_key, raw_value) = line.split_once('=').ok_or_else(|| {
                fail(
                    "expected key = value".into(),
                    "syntax_error",
                    start,
                    start + line.len(),
                )
            })?;
            let key = raw_key.trim_matches([' ', '\t']);
            let value = raw_value.trim_matches([' ', '\t']);
            let key_start = start + raw_key.len() - raw_key.trim_start_matches([' ', '\t']).len();
            let value_start = start + raw_key.len() + 1 + raw_value.len()
                - raw_value.trim_start_matches([' ', '\t']).len();
            let target = if section.is_empty() {
                key.to_owned()
            } else {
                format!("{section}.{key}")
            };
            if value.is_empty() {
                return Err(fail(
                    format!("empty value for {key}"),
                    "missing_value",
                    value_start,
                    value_start,
                )
                .with_target(target));
            }
            let execution = matches!(
                key,
                "network"
                    | "ned-path"
                    | "sim-time-limit"
                    | "metrics-window"
                    | "max-events"
                    | "max-delta-cycles"
                    | "workload"
                    | "model-profile"
                    | "model-config"
            );
            if (section.is_empty() && !execution && !(key.contains('.') && path_name(key)))
                || (!section.is_empty() && (!identifier(key) || reserved(key)))
            {
                return Err(fail(
                    format!("unknown or invalid key: {key}"),
                    "unknown_parameter",
                    key_start,
                    key_start + key.len(),
                )
                .with_target(target)
                .with_detail("actual", key)
                .with_detail("expected", "registered parameter"));
            }
            let entries = if section.is_empty() {
                &mut ini.general
            } else {
                ini.channels.get_mut(section).unwrap()
            };
            if entries.insert(key.into(), value.into()).is_some() {
                let mut diagnostic = fail(
                    format!("duplicate key: {key}"),
                    "duplicate_definition",
                    key_start,
                    key_start + key.len(),
                )
                .with_target(&target);
                if let Some((previous, _)) = ini.spans.get(&target) {
                    diagnostic = diagnostic
                        .with_detail("related_source", &previous.source)
                        .with_detail("related_line", previous.line)
                        .with_detail("related_column", previous.column);
                }
                return Err(diagnostic);
            }
            ini.spans.insert(
                target,
                (
                    SourceSpan::new(path, content, key_start, key_start + key.len()),
                    SourceSpan::new(path, content, value_start, value_start + value.len()),
                ),
            );
        }
        if !saw_general {
            return Err(ini.eof.as_ref().unwrap().apply(
                error(format!("{}: missing [General]", path.display()))
                    .with_reason("missing_value"),
            ));
        }
        Ok(ini)
    }
    fn annotate(&self, key: &str, diagnostic: Diagnostic) -> Diagnostic {
        let key_span = matches!(
            diagnostic.reason.as_str(),
            "unknown_parameter" | "unknown_instance" | "duplicate_definition"
        );
        let diagnostic = diagnostic.with_target(key);
        if let Some((key, value)) = self.spans.get(key) {
            (if key_span { key } else { value }).apply(diagnostic)
        } else if let Some(span) = self.section_spans.get(key) {
            span.apply(diagnostic)
        } else {
            self.eof.as_ref().unwrap().apply(diagnostic)
        }
    }
    fn value<T>(&self, key: &str, result: Result<T>) -> Result<T> {
        result.map_err(|diagnostic| self.annotate(key, diagnostic))
    }
    fn required(&self, key: &str) -> Result<&str> {
        self.general.get(key).map(String::as_str).ok_or_else(|| {
            self.annotate(
                key,
                error(format!("missing General key: {key}"))
                    .with_reason("missing_value")
                    .with_detail("actual", "missing")
                    .with_detail("expected", "General value"),
            )
        })
    }
}

/// Attach a known configuration key using the same validated INI parser as preparation.
pub(crate) fn annotate_config_key(
    text: &str,
    path: &Path,
    key: &str,
    diagnostic: Diagnostic,
) -> Diagnostic {
    match Ini::parse(text, path) {
        Ok(ini) => ini.annotate(key, diagnostic),
        Err(_) => diagnostic.with_source(path),
    }
}

/// Inspect only INI structure and reference path literals. No disk is accessed.
pub fn inspect_config(text: &str, path: &Path, cwd: &Path) -> Result<ProjectHeader> {
    let config = absolute(path, cwd);
    let base = config
        .parent()
        .ok_or_else(|| error("config has no parent"))?;
    let ini = Ini::parse(text, &config)?;
    let roots = ini
        .value("ned-path", quoted_paths(ini.required("ned-path")?))?
        .into_iter()
        .map(|path| absolute(Path::new(&path), base))
        .collect();
    let reference = |key: &str| -> Result<Option<PathBuf>> {
        ini.general
            .get(key)
            .map(|value| {
                let value = string_literal(value)?;
                valid_file_path(&value)?;
                Ok(absolute(Path::new(&value), base))
            })
            .transpose()
            .map_err(|diagnostic| ini.annotate(key, diagnostic))
    };
    let workload = reference("workload")?;
    let model_config = reference("model-config")?;
    let profile = ini
        .general
        .get("model-profile")
        .map(|value| string_literal(value))
        .transpose()
        .map_err(|diagnostic| ini.annotate("model-profile", diagnostic))?;
    Ok(ProjectHeader {
        config,
        cwd: normalize(cwd),
        network: ini.general.get("network").cloned(),
        profile,
        roots,
        workload,
        model_config,
        general: ini.general,
        channels: ini.channels,
    })
}

pub(crate) fn collect_ned(
    root: &Path,
    current: &Path,
    files: &mut Vec<PathBuf>,
    source: &dyn InputSource,
) -> Result<()> {
    let mut entries = source.read_dir(current).map_err(|e| {
        error(format!("{}: {e}", current.display()))
            .with_reason("input_unreadable")
            .with_source(current)
            .with_detail("operation", "read_dir")
            .with_detail("os_error", e)
    })?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        let path = current.join(entry.name);
        if path.to_str().is_none() {
            return Err(error(format!("non UTF-8 path under {}", root.display())));
        }
        let kind = entry
            .kind
            .map_err(|e| error(format!("{}: {e}", path.display())))?;
        if kind == InputKind::Symlink {
            return Err(error(format!("symlink in NED root: {}", path.display())));
        }
        if kind == InputKind::Directory {
            collect_ned(root, &path, files, source)?;
        } else if kind == InputKind::File && path.extension().is_some_and(|ext| ext == "ned") {
            files.push(path);
        }
    }
    Ok(())
}

pub fn prepare(config_path: &Path) -> Result<PreparedSimulation> {
    let cwd = std::env::current_dir().map_err(|e| error(e.to_string()))?;
    prepare_with_source(config_path, &cwd, &FsInputSource)
}

/// Run the existing full preparation with all I/O supplied by the caller.
pub fn prepare_with_source(
    config_path: &Path,
    cwd: &Path,
    source: &dyn InputSource,
) -> Result<PreparedSimulation> {
    let mut prepared = prepare_with_source_inner(config_path, cwd, source)?;
    prepared.common.provenance = capture_provenance(&prepared)?;
    Ok(prepared)
}
fn prepare_with_source_inner(
    config_path: &Path,
    cwd: &Path,
    source: &dyn InputSource,
) -> Result<PreparedSimulation> {
    let config = absolute(config_path, cwd);
    let base = config
        .parent()
        .ok_or_else(|| error("config has no parent"))?;
    let mut snapshots = Vec::new();
    let config_text = read_file(&config, &mut snapshots, source)?;
    let ini = Ini::parse(&config_text, &config)?;
    let result = (|| {
        let network = ini.required("network")?;
        if !network.contains('.') || !path_name(network) {
            return Err(ini.annotate(
                "network",
                error(format!("invalid fully qualified network: {network}"))
                    .with_reason("invalid_type")
                    .with_detail("actual", network)
                    .with_detail("expected", "fully qualified network type"),
            ));
        }
        let profile = ini.value("model-profile", can::profile(&ini.general))?;
        let time_limit_ps = ini.value(
            "sim-time-limit",
            parse_time(ini.required("sim-time-limit")?),
        )?;
        let metrics_window_ps = ini.value(
            "metrics-window",
            parse_time(
                ini.general
                    .get("metrics-window")
                    .map(String::as_str)
                    .unwrap_or("1ms"),
            ),
        )?;
        if metrics_window_ps == 0 {
            return Err(ini.annotate(
                "metrics-window",
                error("metrics-window must be positive")
                    .with_reason("invalid_range")
                    .with_detail("actual", "0")
                    .with_detail("expected", "positive duration"),
            ));
        }
        let max_events = ini.value(
            "max-events",
            unsigned(
                ini.general
                    .get("max-events")
                    .map(String::as_str)
                    .unwrap_or("100000000"),
                true,
            ),
        )?;
        let max_delta_cycles = ini.value(
            "max-delta-cycles",
            unsigned(
                ini.general
                    .get("max-delta-cycles")
                    .map(String::as_str)
                    .unwrap_or("1000000"),
                true,
            ),
        )?;
        let roots: Vec<PathBuf> = ini
            .value("ned-path", quoted_paths(ini.required("ned-path")?))?
            .into_iter()
            .map(|s| absolute(Path::new(&s), base))
            .collect();
        for (i, root) in roots.iter().enumerate() {
            no_symlinks(root, source)?;
            if !source
                .metadata(root)
                .is_ok_and(|metadata| metadata.kind == InputKind::Directory)
            {
                return Err(error(format!(
                    "NED root is not a directory: {}",
                    root.display()
                )));
            }
            if roots[..i]
                .iter()
                .any(|other| root.starts_with(other) || other.starts_with(root))
            {
                return Err(error(format!("overlapping NED root: {}", root.display())));
            }
        }
        let mut declarations = BTreeMap::new();
        for root in roots {
            let mut files = Vec::new();
            collect_ned(&root, &root, &mut files, source)?;
            files.sort();
            for file in files {
                let content = read_file(&file, &mut snapshots, source)?;
                let relative = file.strip_prefix(&root).unwrap();
                let package_components = relative
                    .parent()
                    .unwrap()
                    .components()
                    .map(|p| p.as_os_str().to_str().unwrap())
                    .collect::<Vec<_>>();
                if package_components
                    .iter()
                    .any(|part| !identifier(part) || reserved(part))
                {
                    return Err(error(format!(
                        "{}: NED package directories must each be an identifier",
                        file.display()
                    )));
                }
                let package = package_components.join(".");
                for declaration in ned::parse(&content, &file, &package)? {
                    let name = declaration.name().to_string();
                    if let Some(previous) = declarations.get(&name) {
                        let previous: &Declaration = previous;
                        return Err(declaration.name_span().apply(
                            error(format!("{}: duplicate NED type: {name}", file.display()))
                                .with_reason("duplicate_definition")
                                .with_target(&name)
                                .with_detail("actual", &name)
                                .with_detail("expected", "unique NED type")
                                .with_detail("related_source", &previous.name_span().source)
                                .with_detail("related_line", previous.name_span().line)
                                .with_detail("related_column", previous.name_span().column),
                        ));
                    }
                    declarations.insert(name, declaration);
                }
            }
        }
        let overrides = ini
            .general
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.as_str(),
                    "network"
                        | "ned-path"
                        | "sim-time-limit"
                        | "metrics-window"
                        | "max-events"
                        | "max-delta-cycles"
                        | "workload"
                        | "model-profile"
                        | "model-config"
                )
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if matches!(
            profile.as_str(),
            "axi4.transaction.v1"
                | "soc.shared.v1"
                | "ahb.transaction.v1"
                | "noc.xy.v1"
                | "memory.ipc.transaction.v1"
        ) {
            let mut axi_model = None;
            let mut soc_model = None;
            let mut memory_model = None;
            let (module_paths, channel_count) = match profile.as_str() {
                "axi4.transaction.v1" => {
                    let (model, paths, count) =
                        axi::resolve(&declarations, network, &overrides, &ini.channels)?;
                    axi_model = Some(model);
                    (paths, count)
                }
                "memory.ipc.transaction.v1" => {
                    let (model, paths, count) =
                        memory_ipc::resolve(&declarations, network, &overrides, &ini.channels)?;
                    memory_model = Some(model);
                    (paths, count)
                }
                _ => {
                    let (model, paths, count) =
                        soc::resolve(&declarations, network, &overrides, &ini.channels, &profile)?;
                    soc_model = Some(model);
                    (paths, count)
                }
            };
            let model_path = string_literal(ini.required("model-config")?)?;
            valid_file_path(&model_path)?;
            let model_path = absolute(Path::new(&model_path), base);
            let content = read_file(&model_path, &mut snapshots, source)?;
            let configured = if let Some(model) = &mut axi_model {
                axi::configure(&content, model, &profile)
            } else if let Some(model) = &mut soc_model {
                soc::configure(&content, model, &profile)
            } else {
                memory_ipc::configure(
                    &content,
                    memory_model.as_mut().expect("selected memory model"),
                    &profile,
                )
            };
            configured.map_err(|mut diagnostic| {
                diagnostic.message = format!("{}: {}", model_path.display(), diagnostic.message);
                diagnostic.with_source(&model_path)
            })?;
            let mut workload_path = None;
            if let Some(path) = ini.general.get("workload") {
                let path = string_literal(path)?;
                valid_file_path(&path)?;
                let path = absolute(Path::new(&path), base);
                let content = read_file(&path, &mut snapshots, source)?;
                let loaded = if let Some(model) = &mut axi_model {
                    axi::workload(&content, model, &profile)
                } else if let Some(model) = &mut soc_model {
                    soc::workload(&content, model, &profile)
                } else {
                    memory_ipc::workload(
                        &content,
                        memory_model.as_mut().expect("selected memory model"),
                        &profile,
                    )
                };
                loaded.map_err(|mut diagnostic| {
                    diagnostic.message = format!("{}: {}", path.display(), diagnostic.message);
                    diagnostic.with_source(&path)
                })?;
                workload_path = Some(path);
            }
            return Ok(PreparedSimulation {
                registered: None,
                common: crate::types::PreparedCommon {
                    provenance: Default::default(),
                    run_identity: None,
                    profile,
                    module_paths,
                    network: network.into(),
                    time_limit_ps,
                    metrics_window_ps,
                    max_events,
                    max_delta_cycles,
                    channel_count,
                    config_path: config.clone(),
                    model_config_path: Some(model_path),
                    workload_path,
                    inputs: snapshots,
                },
                can: crate::types::PreparedCan {
                    buses: Vec::new(),
                    controller_buses: Vec::new(),
                    bus_id: String::new(),
                    bitrate: 0,
                    controllers: Vec::new(),
                    generators: Vec::new(),
                },
                gateway: crate::types::PreparedGateway {
                    gateways: Vec::new(),
                    controller_gateways: Vec::new(),
                },
                ethernet: None,
                canfd: None,
                axi: axi_model,
                soc: soc_model,
                memory_ipc: memory_model,
            });
        }
        if profile == "can.fd.precomputed.v1" {
            let (mut canfd, module_paths, channel_count) =
                canfd::resolve(&declarations, network, &overrides, &ini.channels)?;
            let model_path = string_literal(ini.required("model-config")?)?;
            valid_file_path(&model_path)?;
            let model_path = absolute(Path::new(&model_path), base);
            let content = read_file(&model_path, &mut snapshots, source)?;
            canfd::configure(&content, &mut canfd, &profile).map_err(|mut e| {
                e.message = format!("{}: {}", model_path.display(), e.message);
                e.with_source(&model_path)
            })?;
            let mut workload_path = None;
            if let Some(path) = ini.general.get("workload") {
                let path = string_literal(path)?;
                valid_file_path(&path)?;
                let path = absolute(Path::new(&path), base);
                let content = read_file(&path, &mut snapshots, source)?;
                canfd::workload(&content, &mut canfd, &profile).map_err(|mut e| {
                    e.message = format!("{}: {}", path.display(), e.message);
                    e.with_source(&path)
                })?;
                workload_path = Some(path);
            }
            return Ok(PreparedSimulation {
                registered: None,
                common: crate::types::PreparedCommon {
                    provenance: Default::default(),
                    run_identity: None,
                    profile,
                    module_paths,
                    network: network.into(),
                    time_limit_ps,
                    metrics_window_ps,
                    max_events,
                    max_delta_cycles,
                    channel_count,
                    config_path: config.clone(),
                    model_config_path: Some(model_path),
                    workload_path,
                    inputs: snapshots,
                },
                can: crate::types::PreparedCan {
                    buses: Vec::new(),
                    controller_buses: Vec::new(),
                    bus_id: String::new(),
                    bitrate: 0,
                    controllers: Vec::new(),
                    generators: Vec::new(),
                },
                gateway: crate::types::PreparedGateway {
                    gateways: Vec::new(),
                    controller_gateways: Vec::new(),
                },
                ethernet: None,
                canfd: Some(canfd),
                axi: None,
                soc: None,
                memory_ipc: None,
            });
        }
        if profile == "can.ethernet.gateway.v1" {
            let (can_base, mut ethernet, module_paths, channel_count) =
                network::topology::resolve(&declarations, network, &overrides, &ini.channels)?;
            let model_path = string_literal(ini.required("model-config")?)?;
            valid_file_path(&model_path)?;
            let model_path = absolute(Path::new(&model_path), base);
            let content = read_file(&model_path, &mut snapshots, source)?;
            let mut bridge =
                can_ethernet::configure(&content, &can_base, &mut ethernet, &module_paths)
                    .map_err(|diagnostic| diagnostic.with_source(&model_path))?;
            let config_value = parse_json(&content)?;
            let mut workload_path = None;
            let mut workload_value = serde_json::json!({"schema_version":1,"can":{"schema_version":2,"generators":[]},"ethernet":{"schema_version":3,"generators":[]}});
            if let Some(path) = ini.general.get("workload") {
                let path = string_literal(path)?;
                valid_file_path(&path)?;
                let path = absolute(Path::new(&path), base);
                let content = read_file(&path, &mut snapshots, source)?;
                can_ethernet::workload(&content, &mut bridge)
                    .map_err(|diagnostic| diagnostic.with_source(&path))?;
                workload_value = parse_json(&content)?;
                workload_path = Some(path);
            }
            let can = bridge.can.clone();
            let ethernet = bridge.ethernet.clone();
            let payload = crate::types::network::PreparedNetwork {
                ethernet: ethernet.clone(),
                dynamic: None,
                tsn: None,
                bridge: Some(bridge),
                config: config_value,
                workload: workload_value,
            };
            return Ok(PreparedSimulation {
                registered: Some(crate::registry::PreparedRegistered::network(
                    payload,
                    &profile,
                    crate::registry::Registry::default(),
                )),
                common: crate::types::PreparedCommon {
                    provenance: Default::default(),
                    run_identity: None,
                    profile,
                    module_paths,
                    network: network.into(),
                    time_limit_ps,
                    metrics_window_ps,
                    max_events,
                    max_delta_cycles,
                    channel_count,
                    config_path: config.clone(),
                    model_config_path: Some(model_path),
                    workload_path,
                    inputs: snapshots,
                },
                can,
                gateway: crate::types::PreparedGateway {
                    gateways: Vec::new(),
                    controller_gateways: Vec::new(),
                },
                ethernet: Some(ethernet),
                canfd: None,
                axi: None,
                soc: None,
                memory_ipc: None,
            });
        }
        if matches!(
            profile.as_str(),
            "ethernet.l2.store-forward.v1"
                | "ethernet.l2.qos.v1"
                | "ethernet.l2.vlan.v1"
                | "ethernet.l2.dynamic.v1"
                | "ethernet.tsn.v1"
                | "ethernet.l2.store-forward.v2"
                | "ethernet.l2.100base-t1.v1"
        ) {
            let (mut ethernet, module_paths, channel_count) = ethernet::resolve_profile(
                &declarations,
                network,
                &overrides,
                &ini.channels,
                &profile,
            )?;
            let model_path = string_literal(ini.required("model-config")?)?;
            valid_file_path(&model_path)?;
            let model_path = absolute(Path::new(&model_path), base);
            let content = read_file(&model_path, &mut snapshots, source)?;
            let extension = matches!(
                profile.as_str(),
                "ethernet.l2.dynamic.v1" | "ethernet.tsn.v1"
            );
            let extension_config = extension
                .then(|| {
                    parse_json(&content).map_err(|diagnostic| diagnostic.with_source(&model_path))
                })
                .transpose()?;
            if !extension {
                ethernet::configure(&content, &mut ethernet, &profile).map_err(|mut e| {
                    e.message = format!("{}: {}", model_path.display(), e.message);
                    e.with_source(&model_path)
                })?;
            }
            let mut workload_path = None;
            let mut extension_workload = network::empty_workload();
            let mut extension_workload_content = None;
            if let Some(path) = ini.general.get("workload") {
                let path = string_literal(path)?;
                valid_file_path(&path)?;
                let path = absolute(Path::new(&path), base);
                let content = read_file(&path, &mut snapshots, source)?;
                if extension {
                    extension_workload =
                        parse_json(&content).map_err(|diagnostic| diagnostic.with_source(&path))?;
                    extension_workload_content = Some(content);
                } else {
                    ethernet::workload(&content, &mut ethernet, &profile).map_err(|mut e| {
                        e.message = format!("{}: {}", path.display(), e.message);
                        e.with_source(&path)
                    })?;
                }
                workload_path = Some(path);
            }
            let registered = if extension {
                let network = network::prepare(
                    ethernet,
                    extension_config.unwrap(),
                    extension_workload,
                    &profile,
                    (&content, &model_path),
                    extension_workload_content
                        .as_deref()
                        .zip(workload_path.as_deref()),
                )?;
                ethernet = network.ethernet.clone();
                Some(crate::registry::PreparedRegistered::network(
                    network,
                    &profile,
                    crate::registry::Registry::default(),
                ))
            } else {
                None
            };
            return Ok(PreparedSimulation {
                registered,
                common: crate::types::PreparedCommon {
                    provenance: Default::default(),
                    run_identity: None,
                    profile,
                    module_paths,
                    network: network.into(),
                    time_limit_ps,
                    metrics_window_ps,
                    max_events,
                    max_delta_cycles,
                    channel_count,
                    config_path: config.clone(),
                    model_config_path: Some(model_path),
                    workload_path,
                    inputs: snapshots,
                },
                can: crate::types::PreparedCan {
                    buses: Vec::new(),
                    controller_buses: Vec::new(),
                    bus_id: String::new(),
                    bitrate: 0,
                    controllers: Vec::new(),
                    generators: Vec::new(),
                },
                gateway: crate::types::PreparedGateway {
                    gateways: Vec::new(),
                    controller_gateways: Vec::new(),
                },
                ethernet: Some(ethernet),
                canfd: None,
                axi: None,
                soc: None,
                memory_ipc: None,
            });
        }
        let resolved = can::resolve(&declarations, network, &overrides, &ini.channels, &profile)?;
        let mut model_config_path = None;
        let gateways = if profile == "can.cc.multibus.v1" {
            let path = string_literal(ini.required("model-config")?)?;
            valid_file_path(&path)?;
            let path = absolute(Path::new(&path), base);
            let content = read_file(&path, &mut snapshots, source)?;
            model_config_path = Some(path.clone());
            gateway::parse(
                &content,
                &resolved.controllers,
                &resolved.controller_buses,
                &resolved.module_paths,
            )
            .map_err(|mut e| {
                e.message = format!("{}: {}", path.display(), e.message);
                e.with_source(&path)
            })?
        } else {
            Vec::new()
        };
        let mut workload_path = None;
        let generators = if let Some(path) = ini.general.get("workload") {
            let path = string_literal(path)?;
            valid_file_path(&path)?;
            let path = absolute(Path::new(&path), base);
            let content = read_file(&path, &mut snapshots, source)?;
            workload_path = Some(path.clone());
            can::workload(
                &content,
                &resolved.controllers,
                &resolved.controller_buses,
                &profile,
            )
            .map_err(|mut e| {
                e.message = format!("{}: {}", path.display(), e.message);
                e.with_source(&path)
            })?
        } else {
            Vec::new()
        };
        let controller_gateways = gateway::validate(
            &gateways,
            &generators,
            &resolved.controller_buses,
            &resolved.controllers,
            &resolved.buses,
        )?;
        Ok(PreparedSimulation {
            registered: None,
            ethernet: None,
            canfd: None,
            axi: None,
            soc: None,
            memory_ipc: None,
            common: crate::types::PreparedCommon {
                provenance: Default::default(),
                run_identity: None,
                profile,
                module_paths: resolved.module_paths,
                network: network.into(),
                time_limit_ps,
                metrics_window_ps,
                max_events,
                max_delta_cycles,
                channel_count: resolved.channel_count,
                config_path: config.clone(),
                model_config_path,
                workload_path,
                inputs: snapshots,
            },
            can: crate::types::PreparedCan {
                buses: resolved.buses,
                controller_buses: resolved.controller_buses,
                bus_id: resolved.bus_id,
                bitrate: resolved.bitrate,
                controllers: resolved.controllers,
                generators,
            },
            gateway: crate::types::PreparedGateway {
                gateways,
                controller_gateways,
            },
        })
    })();
    result.map_err(|mut diagnostic: Diagnostic| {
        if let Some(target) = diagnostic.target.clone() {
            if ini.spans.contains_key(&target) || ini.section_spans.contains_key(&target) {
                diagnostic = ini.annotate(&target, diagnostic);
            }
        }
        diagnostic.message = format!("{}: {}", config.display(), diagnostic.message);
        diagnostic
    })
}

fn object<'a>(value: &'a Value, allowed: &[&str], target: &str) -> Result<&'a Map<String, Value>> {
    json_diagnostics::select_object(value);
    let object = value.as_object().ok_or_else(|| {
        error(format!("{target} must be an object"))
            .with_reason("invalid_type")
            .with_detail("actual", json_diagnostics::actual(value))
            .with_detail("expected", "object")
    })?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return json_diagnostics::field_result(
            object,
            key,
            Err(error(format!("unknown {target} field: {key}"))
                .with_reason("unknown_parameter")
                .with_detail("actual", key)
                .with_detail("expected", "registered field")),
        );
    }
    Ok(object)
}
fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    json_diagnostics::field_check(object, key, || {
        object.get(key).and_then(Value::as_str).ok_or_else(|| {
            error(format!("missing or non-string field: {key}"))
                .with_reason(if object.contains_key(key) {
                    "invalid_type"
                } else {
                    "missing_value"
                })
                .with_detail(
                    "actual",
                    object
                        .get(key)
                        .map(json_diagnostics::actual)
                        .unwrap_or_else(|| "missing".into()),
                )
                .with_detail("expected", "string")
        })
    })
}
fn json_time(object: &Map<String, Value>, key: &str) -> Result<u64> {
    json_diagnostics::field_check(object, key, || parse_time(required_string(object, key)?))
}
#[cfg(test)]
mod tests;

#[cfg(test)]
mod editor_api_tests;
