//! Strict, snapshot-based loading for the supported Classical CAN profile.
mod ned;

use crate::types::{Diagnostic, Frame, Generator, InputSnapshot, PreparedSimulation, Schedule};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

type Result<T> = std::result::Result<T, Diagnostic>;
fn error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::prepare(message)
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
        return Err(error(format!("negative quantity: {value}")));
    }
    let integer_end = number.bytes().take_while(u8::is_ascii_digit).count();
    let integer = &number[..integer_end];
    if integer.is_empty() || (integer.len() > 1 && integer.starts_with('0')) {
        return Err(error(format!("invalid decimal literal: {value}")));
    }
    let mut remainder = &number[integer_end..];
    let fraction = if let Some(rest) = remainder.strip_prefix('.') {
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return Err(error(format!("invalid decimal literal: {value}")));
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
        _ => return Err(error(format!("expected a {unit} quantity, got {value}"))),
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
        return Err(error(format!("quantity requires rounding: {value}")));
    }
    prefix[..split].iter().try_fold(0u64, |n, &digit| {
        n.checked_mul(10)
            .and_then(|n| n.checked_add(digit as u64))
            .ok_or_else(|| error(format!("quantity exceeds u64: {value}")))
    })
}

fn unsigned(value: &str, positive: bool) -> Result<u64> {
    if value.is_empty()
        || !value.bytes().all(|c| c.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(error(format!("expected decimal integer: {value}")));
    }
    let n: u64 = value
        .parse()
        .map_err(|_| error(format!("integer exceeds u64: {value}")))?;
    if positive && n == 0 {
        return Err(error(format!("expected positive integer: {value}")));
    }
    Ok(n)
}

pub(crate) fn string_literal(value: &str) -> Result<String> {
    let mut chars = value.chars();
    if chars.next() != Some('"') {
        return Err(error(format!("expected quoted string: {value}")));
    }
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if chars.next().is_some() {
                    return Err(error(format!("trailing string input: {value}")));
                }
                return Ok(out);
            }
            '\\' => out.push(match chars.next() {
                Some('"') => '"',
                Some('\\') => '\\',
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                _ => return Err(error("invalid string escape")),
            }),
            c if c <= '\u{1f}' || c == '\u{7f}' => {
                return Err(error("unescaped control character in string"));
            }
            c => out.push(c),
        }
    }
    Err(error("unterminated quoted string"))
}

fn quoted_paths(value: &str) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    let mut rest = value.trim_matches([' ', '\t']);
    loop {
        if !rest.starts_with('"') {
            return Err(error("ned-path requires quoted nonempty paths"));
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
        let end = end.ok_or_else(|| error("unterminated ned-path string"))?;
        let path = string_literal(&rest[..end])?;
        valid_file_path(&path)?;
        paths.push(path);
        rest = rest[end..].trim_matches([' ', '\t']);
        if rest.is_empty() {
            return Ok(paths);
        }
        rest = rest
            .strip_prefix(';')
            .ok_or_else(|| error("invalid ned-path separator"))?
            .trim_start_matches([' ', '\t']);
    }
}
fn valid_file_path(path: &str) -> Result<()> {
    if path.is_empty() || path.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(error("path is empty or contains control characters"));
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
fn absolute(path: &Path, base: &Path) -> PathBuf {
    normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    })
}
fn no_symlinks(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|e| error(format!("{}: {e}", current.display())))?;
        if metadata.file_type().is_symlink() {
            return Err(error(format!(
                "symlink input is unsupported: {}",
                current.display()
            )));
        }
    }
    Ok(())
}
fn read_file(path: &Path, snapshots: &mut Vec<InputSnapshot>) -> Result<String> {
    no_symlinks(path)?;
    if !fs::metadata(path)
        .map_err(|e| error(format!("{}: {e}", path.display())))?
        .is_file()
    {
        return Err(error(format!(
            "input is not a regular file: {}",
            path.display()
        )));
    }
    let content =
        fs::read_to_string(path).map_err(|e| error(format!("{}: {e}", path.display())))?;
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
}
impl Ini {
    fn parse(content: &str, path: &Path) -> Result<Self> {
        let mut ini = Self::default();
        let mut section: Option<String> = None;
        let mut saw_general = false;
        for (line_number, original) in content
            .trim_start_matches('\u{feff}')
            .split_inclusive('\n')
            .enumerate()
        {
            let original = if let Some(rest) = original.strip_suffix('\n') {
                rest.strip_suffix('\r').unwrap_or(rest)
            } else {
                original
            };
            let line = original.trim_matches([' ', '\t']);
            let fail = |m: String| error(format!("{}:{}: {m}", path.display(), line_number + 1));
            if line.contains('\r') {
                return Err(fail("bare carriage return".into()));
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
                        return Err(fail(format!("invalid channel identifier: {id}")));
                    }
                    if ini
                        .channels
                        .insert(id.to_string(), BTreeMap::new())
                        .is_some()
                    {
                        return Err(fail(format!("duplicate channel section: {id}")));
                    }
                    section = Some(id.into());
                } else {
                    return Err(fail(format!("unknown or duplicate section: {line}")));
                }
                continue;
            }
            let section = section
                .as_ref()
                .ok_or_else(|| fail("key before [General]".into()))?;
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| fail("expected key = value".into()))?;
            let key = key.trim_matches([' ', '\t']);
            let value = value.trim_matches([' ', '\t']);
            if value.is_empty() {
                return Err(fail(format!("empty value for {key}")));
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
                return Err(fail(format!("unknown or invalid key: {key}")));
            }
            let entries = if section.is_empty() {
                &mut ini.general
            } else {
                ini.channels.get_mut(section).unwrap()
            };
            if entries.insert(key.into(), value.into()).is_some() {
                return Err(fail(format!("duplicate key: {key}")));
            }
        }
        if !saw_general {
            return Err(error(format!("{}: missing [General]", path.display())));
        }
        Ok(ini)
    }
    fn required(&self, key: &str) -> Result<&str> {
        self.general
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| error(format!("missing General key: {key}")))
    }
}

fn collect_ned(root: &Path, current: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(current)
        .map_err(|e| error(format!("{}: {e}", current.display())))?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| error(format!("{}: {e}", current.display())))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.to_str().is_none() {
            return Err(error(format!("non UTF-8 path under {}", root.display())));
        }
        let kind = entry
            .file_type()
            .map_err(|e| error(format!("{}: {e}", path.display())))?;
        if kind.is_symlink() {
            return Err(error(format!("symlink in NED root: {}", path.display())));
        }
        if kind.is_dir() {
            collect_ned(root, &path, files)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "ned") {
            files.push(path);
        }
    }
    Ok(())
}

pub fn prepare(config_path: &Path) -> Result<PreparedSimulation> {
    let cwd = std::env::current_dir().map_err(|e| error(e.to_string()))?;
    let config = absolute(config_path, &cwd);
    let base = config
        .parent()
        .ok_or_else(|| error("config has no parent"))?;
    let mut snapshots = Vec::new();
    let config_text = read_file(&config, &mut snapshots)?;
    let result = (|| {
        let ini = Ini::parse(&config_text, &config)?;
        let network = ini.required("network")?;
        if !network.contains('.') || !path_name(network) {
            return Err(error(format!("invalid fully qualified network: {network}")));
        }
        if let Some(profile) = ini.general.get("model-profile") {
            if string_literal(profile)? != "can.cc.ideal.v1" {
                return Err(error(format!("unsupported model-profile: {profile}")));
            }
        }
        if ini.general.contains_key("model-config") {
            return Err(error("model-config is unsupported for can.cc.ideal.v1"));
        }
        let time_limit_ps = parse_time(ini.required("sim-time-limit")?)?;
        let metrics_window_ps = parse_time(
            ini.general
                .get("metrics-window")
                .map(String::as_str)
                .unwrap_or("1ms"),
        )?;
        if metrics_window_ps == 0 {
            return Err(error("metrics-window must be positive"));
        }
        let max_events = unsigned(
            ini.general
                .get("max-events")
                .map(String::as_str)
                .unwrap_or("100000000"),
            true,
        )?;
        let max_delta_cycles = unsigned(
            ini.general
                .get("max-delta-cycles")
                .map(String::as_str)
                .unwrap_or("1000000"),
            true,
        )?;
        let roots: Vec<PathBuf> = quoted_paths(ini.required("ned-path")?)?
            .into_iter()
            .map(|s| absolute(Path::new(&s), base))
            .collect();
        for (i, root) in roots.iter().enumerate() {
            no_symlinks(root)?;
            if !root.is_dir() {
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
            collect_ned(&root, &root, &mut files)?;
            files.sort();
            for file in files {
                let content = read_file(&file, &mut snapshots)?;
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
                    let name = declaration.name.clone();
                    if declarations.insert(name.clone(), declaration).is_some() {
                        return Err(error(format!(
                            "{}: duplicate NED type: {name}",
                            file.display()
                        )));
                    }
                }
            }
        }
        let resolved = ned::resolve(&declarations, network, &ini.general, &ini.channels)?;
        let generators = if let Some(path) = ini.general.get("workload") {
            let path = string_literal(path)?;
            valid_file_path(&path)?;
            let path = absolute(Path::new(&path), base);
            let content = read_file(&path, &mut snapshots)?;
            workload(&content, &resolved.controllers)
                .map_err(|e| error(format!("{}: {}", path.display(), e.message)))?
        } else {
            Vec::new()
        };
        Ok(PreparedSimulation {
            network: network.into(),
            bus_id: resolved.bus_id,
            bitrate: resolved.bitrate,
            controllers: resolved.controllers,
            generators,
            time_limit_ps,
            metrics_window_ps,
            max_events,
            max_delta_cycles,
            channel_count: resolved.channel_count,
            inputs: snapshots,
        })
    })();
    result.map_err(|e: Diagnostic| error(format!("{}: {}", config.display(), e.message)))
}

// Deserialize recursively so duplicate keys are rejected before a JSON object can overwrite them.
struct StrictJson(Value);
impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON value with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::from(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::from(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::from(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(StrictJson(value)) = sequence.next_element()? {
                    out.push(value);
                }
                Ok(StrictJson(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON key: {key}")));
                    }
                    let StrictJson(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(StrictJson(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

fn object<'a>(value: &'a Value, allowed: &[&str], target: &str) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| error(format!("{target} must be an object")))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(error(format!("unknown {target} field: {key}")));
    }
    Ok(object)
}
fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error(format!("missing or non-string field: {key}")))
}
fn json_time(object: &Map<String, Value>, key: &str) -> Result<u64> {
    parse_time(required_string(object, key)?)
}
fn workload(content: &str, controllers: &[crate::types::Controller]) -> Result<Vec<Generator>> {
    let StrictJson(value) = serde_json::from_str(content.trim_start_matches('\u{feff}'))
        .map_err(|e| error(e.to_string()))?;
    let root = object(&value, &["schema_version", "generators"], "workload")?;
    if root.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err(error("workload schema_version must be integer 1"));
    }
    let generators = root
        .get("generators")
        .and_then(Value::as_array)
        .ok_or_else(|| error("generators must be an array"))?;
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    let mut owners = BTreeMap::new();
    for value in generators {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| error("missing generator kind"))?;
        let allowed: &[&str] = match kind {
            "can.explicit.v1" => &["id", "kind", "node", "frame", "times"],
            "can.periodic.v1" => &[
                "id", "kind", "node", "frame", "start", "phase", "period", "end", "count",
            ],
            _ => return Err(error(format!("unsupported generator kind: {kind}"))),
        };
        let generator = object(value, allowed, "generator")?;
        let id = required_string(generator, "id")?;
        if !identifier(id) || !ids.insert(id.to_string()) {
            return Err(error(format!("invalid or duplicate generator id: {id}")));
        }
        let node = required_string(generator, "node")?;
        let source = controllers
            .iter()
            .position(|c| c.id == node)
            .ok_or_else(|| error(format!("unknown Controller node: {node}")))?;
        let frame = generator
            .get("frame")
            .ok_or_else(|| error("missing frame"))?;
        object(frame, &["format", "id", "data"], "frame")?;
        let mut frame: Frame =
            serde_json::from_value(frame.clone()).map_err(|e| error(e.to_string()))?;
        let maximum = match frame.format.as_str() {
            "standard" => 2047,
            "extended" => 536_870_911,
            _ => return Err(error(format!("unknown frame format: {}", frame.format))),
        };
        if frame.id > maximum
            || frame.data.len() > 16
            || frame.data.len() % 2 != 0
            || !frame.data.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(error(format!("invalid frame for generator {id}")));
        }
        frame.data.make_ascii_lowercase();
        if owners
            .insert((frame.format.clone(), frame.id), source)
            .is_some_and(|previous| previous != source)
        {
            return Err(error(format!(
                "CAN frame ownership conflict: {} id {}",
                frame.format, frame.id
            )));
        }
        let schedule = if kind == "can.explicit.v1" {
            let times = generator
                .get("times")
                .and_then(Value::as_array)
                .ok_or_else(|| error("explicit times must be an array"))?;
            let times = times
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| error("explicit time must be a string"))
                        .and_then(parse_time)
                })
                .collect::<Result<Vec<_>>>()?;
            if times.windows(2).any(|pair| pair[0] > pair[1]) {
                return Err(error(format!("unsorted explicit times for {id}")));
            }
            Schedule::Explicit(times)
        } else {
            let start = json_time(generator, "start")?;
            let phase = if generator.contains_key("phase") {
                json_time(generator, "phase")?
            } else {
                0
            };
            let period = json_time(generator, "period")?;
            let end = generator
                .contains_key("end")
                .then(|| json_time(generator, "end"))
                .transpose()?;
            let count = generator
                .get("count")
                .map(|v| {
                    v.as_u64()
                        .ok_or_else(|| error("count must be a u64 JSON integer"))
                })
                .transpose()?;
            if period == 0 || phase >= period || end.is_some_and(|end| end < start) {
                return Err(error(format!("invalid periodic bounds for {id}")));
            }
            Schedule::Periodic {
                start,
                phase,
                period,
                end,
                count,
            }
        };
        out.push(Generator {
            id: id.into(),
            source,
            frame,
            schedule,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

pub(crate) fn validate_filter(value: &str) -> Result<()> {
    if matches!(value, "*" | "none") {
        return Ok(());
    }
    let mut ids = BTreeSet::new();
    for item in value.split(',') {
        let (kind, hex) = item
            .split_once(":0x")
            .ok_or_else(|| error(format!("invalid rxFilter: {value}")))?;
        let limit = match kind {
            "std" => 2047,
            "ext" => 536_870_911,
            _ => return Err(error(format!("invalid rxFilter: {value}"))),
        };
        if hex.is_empty() || hex.len() > 8 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(error(format!("invalid rxFilter: {value}")));
        }
        let id = u32::from_str_radix(hex, 16)
            .map_err(|_| error(format!("invalid rxFilter: {value}")))?;
        if id > limit || !ids.insert((kind, id)) {
            return Err(error(format!(
                "out of range or duplicate rxFilter: {value}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const NED: &str = include_str!("../../../docs/verification/fixtures/can/models/demo/Main.ned");
    const INI: &str = "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n";
    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(ned: &str, ini: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "dir-input-test-{}-{}",
                std::process::id(),
                TEMP_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("models/demo")).unwrap();
            fs::write(path.join("models/demo/Main.ned"), ned).unwrap();
            fs::write(path.join("scenario.ini"), ini).unwrap();
            Self(path)
        }
        fn prepare(&self) -> Result<PreparedSimulation> {
            prepare(&self.0.join("scenario.ini"))
        }
        fn workload(&self, text: &str) {
            fs::write(self.0.join("workload.json"), text).unwrap();
            fs::write(
                self.0.join("scenario.ini"),
                format!("{INI}workload = \"workload.json\"\n"),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn exact_units_and_extreme_decimal_precision() {
        for (literal, expected) in [
            ("0ps", 0),
            ("0.001ns", 1),
            ("1 ms", 1_000_000_000),
            ("18446744073709551615ps", u64::MAX),
            ("0.000000000001s", 1),
            ("1.00000000000000000000000000000000000000000000000ps", 1),
        ] {
            assert_eq!(parse_time(literal).unwrap(), expected, "{literal}");
        }
        for literal in [
            "0.1ps",
            "18446744073709551616ps",
            "-0ps",
            "-1ps",
            "01ps",
            ".1ps",
            "1.ps",
            "1e3ps",
            "1",
            "1Mbps",
            "1PS",
            "1ps#comment",
            "1\u{a0}ps",
        ] {
            assert!(parse_time(literal).is_err(), "{literal}");
        }
        assert_eq!(quantity("0.5Mbps", "bps").unwrap(), 500_000);
        assert_eq!(quantity("1KiB", "B").unwrap(), 1_024);
        assert!(quantity("0.5bps", "bps").is_err());
    }

    #[test]
    fn all_can_fixtures_load_and_preserve_snapshots() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/can");
        for name in [
            "competition",
            "queue-full",
            "capacity-zero",
            "eof-boundary",
            "after-eof",
            "delay-filter",
            "release-arrival",
            "mixed-generators",
        ] {
            let prepared = prepare(&root.join(format!("{name}.ini"))).unwrap();
            assert_eq!(prepared.bitrate, 500_000);
            assert_eq!(prepared.controllers.len(), 3);
            assert_eq!(prepared.channel_count, 6);
            assert_eq!(prepared.inputs.len(), 3);
            assert_eq!(prepared.controllers[0].id, "Main.a");
            assert_eq!(prepared.bus_id, "Main.bus");
        }
        let prepared = prepare(&root.join("delay-filter.ini")).unwrap();
        assert_eq!(prepared.controllers[0].tx_processing_ps, 3_000_000);
        assert_eq!(prepared.controllers[0].tx_channel_ps, 2_000_000);
        assert_eq!(prepared.controllers[1].rx_channel_ps, 3_000_000);
        assert_eq!(prepared.controllers[2].rx_filter, "none");
    }

    #[test]
    fn defaults_overrides_empty_workload_and_limits() {
        let fixture = Fixture::new(
            NED,
            &format!(
                "{INI}Main.a.queueCapacity = 32\nmax-events = 18446744073709551615\nsim-time-limit-unknown = 1ms\n"
            ),
        );
        assert!(fixture.prepare().is_err());
        fs::write(
            fixture.0.join("scenario.ini"),
            format!("{INI}Main.a.queueCapacity = 32\nmax-events = 18446744073709551615\n"),
        )
        .unwrap();
        let prepared = fixture.prepare().unwrap();
        assert_eq!(prepared.controllers[0].queue_capacity, 32);
        assert_eq!(prepared.controllers[1].queue_capacity, 64);
        assert_eq!(prepared.max_events, u64::MAX);
        assert_eq!(prepared.metrics_window_ps, 1_000_000_000);
        assert!(prepared.generators.is_empty());
        assert_eq!(prepared.inputs.len(), 2);
    }

    #[test]
    fn compound_boundary_channels_are_independent_and_sum() {
        let model = NED
            .replace("a: demo.Controller;", "a: demo.Box;")
            .replace("a.tx -->", "a.out -->")
            .replace("--> a.rx;", "--> a.in;")
            + "\nmodule Box { gates: input in; output out; submodules: c: demo.Controller; connections: in --> demo.Wire --> c.rx; c.tx --> demo.Wire --> out; }\n";
        let fixture = Fixture::new(
            &model,
            &format!(
                "{INI}[Channel Main::a.out]\ndelay = 2ns\n[Channel Main.a::c.tx]\ndelay = 3ns\n"
            ),
        );
        let prepared = fixture.prepare().unwrap();
        assert_eq!(prepared.controllers[0].id, "Main.a.c");
        assert_eq!(prepared.controllers[0].tx_channel_ps, 5_000);
        assert_eq!(prepared.controllers[1].tx_channel_ps, 0);
        assert_eq!(prepared.channel_count, 8);
        fs::write(fixture.0.join("scenario.ini"), format!("{INI}[Channel Main::a.out]\ndelay = 18446744073709551615ps\n[Channel Main.a::c.tx]\ndelay = 1ps\n")).unwrap();
        assert!(fixture.prepare().unwrap_err().message.contains("overflow"));
    }

    #[test]
    fn invalid_ned_declarations_and_wiring_are_rejected() {
        let variants = [
            NED.replace("package demo;", "package other;"),
            NED.replace("@class(\"dir.can.Controller\")", "@class(\"unknown\")"),
            NED.replace("int queueCapacity", "double queueCapacity"),
            NED.replace("default(64)", "default(-1)"),
            NED.replace("default(64)", "default(64.0)"),
            NED.replace("int queueCapacity", "int queueCapacity @unit(s)"),
            NED.replace("output tx; input rx;", "output tx; input rx; input extra;"),
            NED.replace(
                "a.tx --> demo.Wire --> bus.tx_a;",
                "a.rx --> demo.Wire --> bus.tx_a;",
            ),
            NED.replace(
                "a.tx --> demo.Wire --> bus.tx_a;",
                "a.tx --> demo.Wire --> bus.rx_a;",
            ),
            NED.replace(
                "a.tx --> demo.Wire --> bus.tx_a;",
                "a.tx --> demo.Wire --> bus.tx_a; a.tx --> demo.Wire --> bus.tx_a;",
            ),
            NED.replace("bus.rx_a --> demo.Wire --> a.rx;", ""),
            NED.replace(
                "bus.rx_a --> demo.Wire --> a.rx;",
                "bus.rx_a --> demo.Wire --> b.rx;",
            )
            .replace(
                "bus.rx_b --> demo.Wire --> b.rx;",
                "bus.rx_b --> demo.Wire --> a.rx;",
            ),
            NED.replace("a: demo.Controller;", "a: demo.Missing;"),
            NED.replace("network Main {", "network Main { gates: input in;"),
            format!("{NED}\nmodule Loop {{ submodules: x: demo.Loop; }}"),
            format!("{NED}\nnetwork Other {{ submodules: x: demo.Missing; }}"),
            format!("{NED}\nchannel Wire {{}}"),
            NED.replace("default(64)", "64"),
            NED.replace("a: demo.Controller;", "a: Controller;"),
        ];
        for model in variants {
            let fixture = Fixture::new(&model, INI);
            assert!(fixture.prepare().is_err(), "accepted {model}");
        }
        let fixture = Fixture::new(
            &NED.replace("default(64)", "default(-1)"),
            &format!(
                "{INI}Main.a.queueCapacity = 64\nMain.b.queueCapacity = 64\nMain.c.queueCapacity = 64\n"
            ),
        );
        assert!(
            fixture.prepare().is_err(),
            "INI must not hide invalid defaults"
        );
        let fixture = Fixture::new(
            &NED.replace("default(64)", "default(4294967296)"),
            &format!(
                "{INI}Main.a.queueCapacity = 64\nMain.b.queueCapacity = 64\nMain.c.queueCapacity = 64\n"
            ),
        );
        assert!(
            fixture.prepare().is_err(),
            "INI must not hide an excessive queue default"
        );
    }

    #[test]
    fn truncated_ned_returns_diagnostics_without_panicking() {
        for (end, _) in NED.char_indices() {
            let _ = ned::parse(&NED[..end], Path::new("models/demo/Main.ned"), "demo");
        }
    }

    #[test]
    fn ini_rejects_unknown_duplicate_and_malformed_inputs() {
        for addition in [
            "Main.unknown.queueCapacity = 1",
            "Main.a.unknown = 1",
            "Main.a.queueCapacity = \"32\"",
            "Main.a.queueCapacity = -1",
            "Main.a.queueCapacity = 4294967296",
            "Main.a.queueCapacity = 64.0",
            "Main.a.rxFilter = \"std:0x1,std:0x01\"",
            "Main.a.rxFilter = \"std:0x800\"",
            "Main.a.rxFilter = \"none\" # comment",
            "max-events = 0",
            "metrics-window = 0ps",
            "network = demo.Main",
            "[General]",
            "[Config Fast]",
            "[Channel Main::a.tx]\nunknown = 1ps",
            "[Channel Main::missing.tx]\ndelay = 1ps",
            "[Channel  Main::a.tx]\ndelay = 1ps",
            "workload = \"\"",
            "model-profile = \"can.fd.v1\"",
            "model-config = \"anything.json\"",
            "[Channel Main::a.tx]\ndelay = 1ps\n[Channel Main::a.tx]\ndelay = 1ps",
        ] {
            let fixture = Fixture::new(NED, &format!("{INI}{addition}\n"));
            assert!(fixture.prepare().is_err(), "accepted {addition}");
        }
        for ini in [
            "network = demo.Main\n",
            "[General]\r",
            "[General]\nnetwork = demo.Main\rsim-time-limit = 1ps",
        ] {
            assert!(Ini::parse(ini, Path::new("scenario.ini")).is_err());
        }
    }

    #[test]
    fn json_duplicate_unknown_fields_and_inactive_owner_conflicts() {
        let fixture = Fixture::new(NED, INI);
        for text in [
            "{\"schema_version\":1,\"schema_version\":1,\"generators\":[]}",
            "{\"schema_version\":1.0,\"generators\":[]}",
            "{\"schema_version\":1,\"generators\":[],\"seed\":0}",
            r#"{"schema_version":1,"generators":[{"id":"a","kind":"can.explicit.v1","node":"Main.a","times":[],"frame":{"format":"standard","id":0,"data":"","data":""}}]}"#,
            r#"{"schema_version":1,"generators":[{"id":"a","kind":"can.explicit.v1","node":"Main.a","times":[],"frame":{"format":"standard","id":0,"data":""}},{"id":"b","kind":"can.periodic.v1","node":"Main.b","start":"0ps","period":"1ms","count":0,"frame":{"format":"standard","id":0,"data":""}}]}"#,
        ] {
            fixture.workload(text);
            assert!(fixture.prepare().is_err(), "accepted {text}");
        }
    }

    #[test]
    fn workload_validates_schedule_and_frame_beyond_time_limit() {
        let fixture = Fixture::new(NED, INI);
        let base = serde_json::json!({"schema_version":1,"generators":[{"id":"z","kind":"can.explicit.v1","node":"Main.a","times":["0ps","0ps","2ms"],"frame":{"format":"standard","id":291,"data":"00FF"}}]});
        fixture.workload(&base.to_string());
        let prepared = fixture.prepare().unwrap();
        assert_eq!(prepared.generators[0].frame.data, "00ff");
        assert_eq!(prepared.generators[0].schedule.time(2), Some(2_000_000_000));
        for (field, value) in [
            ("times", serde_json::json!(["2ms", "1ms"])),
            ("times", serde_json::json!(["2ms", "-1ps"])),
            ("node", serde_json::json!("Main.bus")),
            ("id", serde_json::json!("bad-id")),
            (
                "frame",
                serde_json::json!({"format":"standard","id":2048,"data":""}),
            ),
            (
                "frame",
                serde_json::json!({"format":"standard","id":0,"data":"f"}),
            ),
            (
                "frame",
                serde_json::json!({"format":"standard","id":0,"data":"000000000000000000"}),
            ),
            (
                "frame",
                serde_json::json!({"format":"standard","id":0,"data":"","rtr":true}),
            ),
            ("count", serde_json::json!(0)),
        ] {
            let mut changed = base.clone();
            changed["generators"][0][field] = value;
            fixture.workload(&changed.to_string());
            assert!(fixture.prepare().is_err(), "accepted {changed}");
        }
        let periodic = serde_json::json!({"schema_version":1,"generators":[{"id":"p","kind":"can.periodic.v1","node":"Main.a","start":"1ms","period":"2ms","phase":"1ms","end":"8ms","count":3,"frame":{"format":"extended","id":291,"data":""}}]});
        fixture.workload(&periodic.to_string());
        let prepared = fixture.prepare().unwrap();
        assert_eq!(prepared.generators[0].schedule.time(0), Some(2_000_000_000));
        assert_eq!(prepared.generators[0].schedule.time(2), Some(6_000_000_000));
        assert_eq!(prepared.generators[0].schedule.time(3), None);
        for (field, value) in [
            ("period", serde_json::json!("0ps")),
            ("phase", serde_json::json!("2ms")),
            ("end", serde_json::json!("0ps")),
            ("count", serde_json::json!(0.5)),
        ] {
            let mut changed = periodic.clone();
            changed["generators"][0][field] = value;
            fixture.workload(&changed.to_string());
            assert!(fixture.prepare().is_err(), "accepted {changed}");
        }
    }

    #[test]
    fn root_overlap_missing_files_and_symlinks_fail() {
        let fixture = Fixture::new(
            NED,
            &INI.replace("\"models\"", "\"models\";\"models/demo\""),
        );
        assert!(
            fixture
                .prepare()
                .unwrap_err()
                .message
                .contains("overlapping")
        );
        fs::write(
            fixture.0.join("scenario.ini"),
            format!("{INI}workload = \"missing.json\"\n"),
        )
        .unwrap();
        assert!(fixture.prepare().is_err());
        #[cfg(unix)]
        {
            fs::write(fixture.0.join("scenario.ini"), INI).unwrap();
            std::os::unix::fs::symlink("Main.ned", fixture.0.join("models/demo/symlink.txt"))
                .unwrap();
            assert!(fixture.prepare().unwrap_err().message.contains("symlink"));
        }
    }
}
