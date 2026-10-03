//! Snapshot-based input orchestration, file loading and common INI/JSON/time utilities.
//! NED resolution and model-specific validation are delegated to the input adapters.
mod can;
mod gateway;
mod ned;

use crate::types::{Diagnostic, InputSnapshot, PreparedSimulation};
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
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
        let profile = can::profile(&ini.general)?;
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
        let resolved = can::resolve(&declarations, network, &overrides, &ini.channels, &profile)?;
        let mut model_config_path = None;
        let gateways = if profile == "can.cc.multibus.v1" {
            let path = string_literal(ini.required("model-config")?)?;
            valid_file_path(&path)?;
            let path = absolute(Path::new(&path), base);
            let content = read_file(&path, &mut snapshots)?;
            model_config_path = Some(path.clone());
            gateway::parse(
                &content,
                &resolved.controllers,
                &resolved.controller_buses,
                &resolved.module_paths,
            )
            .map_err(|e| error(format!("{}: {}", path.display(), e.message)))?
        } else {
            Vec::new()
        };
        let mut workload_path = None;
        let generators = if let Some(path) = ini.general.get("workload") {
            let path = string_literal(path)?;
            valid_file_path(&path)?;
            let path = absolute(Path::new(&path), base);
            let content = read_file(&path, &mut snapshots)?;
            workload_path = Some(path.clone());
            can::workload(
                &content,
                &resolved.controllers,
                &resolved.controller_buses,
                &profile,
            )
            .map_err(|e| error(format!("{}: {}", path.display(), e.message)))?
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
            common: crate::types::PreparedCommon {
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
#[cfg(test)]
mod tests;
