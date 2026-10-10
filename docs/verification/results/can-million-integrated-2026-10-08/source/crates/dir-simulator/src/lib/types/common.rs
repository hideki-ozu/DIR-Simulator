//! Model-independent input, scheduling and diagnostic contracts.
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Schedule {
    Explicit(Vec<u64>),
    Periodic {
        start: u64,
        phase: u64,
        period: u64,
        end: Option<u64>,
        count: Option<u64>,
    },
}
impl Schedule {
    pub fn time(&self, ordinal: u64) -> Option<u128> {
        match self {
            Self::Explicit(times) => usize::try_from(ordinal)
                .ok()
                .and_then(|i| times.get(i))
                .map(|&t| t as u128),
            Self::Periodic {
                start,
                phase,
                period,
                end,
                count,
            } => {
                if count.is_some_and(|n| ordinal >= n) {
                    return None;
                }
                let time = *start as u128 + *phase as u128 + ordinal as u128 * *period as u128;
                if end.is_some_and(|n| time >= n as u128) {
                    None
                } else {
                    Some(time)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InputSnapshot {
    pub path: PathBuf,
    pub content: String,
}

/// Half-open input range. Byte offsets retain the original file encoding, including a BOM.
#[derive(Debug, Clone, Serialize)]
pub struct SourceSpan {
    pub source: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}
impl SourceSpan {
    pub fn new(path: &std::path::Path, text: &str, start_byte: usize, end_byte: usize) -> Self {
        let diagnostic = Diagnostic::prepare("").with_span(path, text, start_byte, end_byte);
        Self {
            source: diagnostic.source.unwrap(),
            line: diagnostic.line.unwrap(),
            column: diagnostic.column.unwrap(),
            end_line: diagnostic.end_line.unwrap(),
            end_column: diagnostic.end_column.unwrap(),
            start_byte,
            end_byte,
        }
    }
    pub fn apply(&self, mut diagnostic: Diagnostic) -> Diagnostic {
        diagnostic.source = Some(self.source.clone());
        diagnostic.line = Some(self.line);
        diagnostic.column = Some(self.column);
        diagnostic.end_line = Some(self.end_line);
        diagnostic.end_column = Some(self.end_column);
        diagnostic
    }
}

/// A diagnostic keeps legacy `stage`/`details` access while serializing the full wire contract.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub schema_version: u32,
    pub seq: u64,
    pub primary: bool,
    pub code: String,
    pub stage: String,
    pub reason: String,
    pub message: String,
    pub source: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub end_line: Option<usize>,
    pub end_column: Option<usize>,
    pub target: Option<String>,
    pub time_ps: Option<u64>,
    pub event_seq: Option<u64>,
    pub details: Option<serde_json::Value>,
}
impl Diagnostic {
    fn new(code: &str, phase: &str, reason: &str, message: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            seq: 0,
            primary: true,
            code: code.into(),
            stage: phase.into(),
            reason: reason.into(),
            message: message.into(),
            source: None,
            line: None,
            column: None,
            end_line: None,
            end_column: None,
            target: None,
            time_ps: None,
            event_seq: None,
            details: None,
        }
    }
    pub fn prepare(message: impl Into<String>) -> Self {
        Self::new("E-0001", "prepare", "invalid_argument", message)
    }
    pub fn execution(message: impl Into<String>) -> Self {
        Self::new("E-0002", "run", "model_failed", message)
    }
    pub fn output(message: impl Into<String>) -> Self {
        Self::new("E-0003", "output", "output_write_failed", message)
    }
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
    pub fn with_source(mut self, path: &std::path::Path) -> Self {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        };
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                component => normalized.push(component.as_os_str()),
            }
        }
        let source = normalized.to_string_lossy().into_owned();
        if self
            .details
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .is_some_and(|details| {
                details.contains_key("related_line") && !details.contains_key("related_source")
            })
        {
            self = self.with_detail("related_source", &source);
        }
        self.source = Some(source);
        self
    }
    /// Byte bounds are supplied by the parser; columns count Unicode scalars.
    pub fn with_span(self, path: &std::path::Path, text: &str, start: usize, end: usize) -> Self {
        self.with_source(path).with_text_span(text, start, end)
    }
    pub(crate) fn with_text_span(mut self, text: &str, start: usize, end: usize) -> Self {
        fn position(text: &str, byte: usize) -> (usize, usize) {
            let bom = if text.starts_with('\u{feff}') {
                '\u{feff}'.len_utf8()
            } else {
                0
            };
            let mut line = 1;
            let mut column = 1;
            let mut chars = text[bom..].char_indices().peekable();
            while let Some((offset, c)) = chars.next() {
                if offset + bom >= byte {
                    break;
                }
                if c == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                    chars.next();
                    line += 1;
                    column = 1;
                } else if c == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            (line, column)
        }
        let (line, column) = position(text, start.min(text.len()));
        let (end_line, end_column) = position(text, end.max(start).min(text.len()));
        self.line = Some(line);
        self.column = Some(column);
        self.end_line = Some(end_line);
        self.end_column = Some(end_column);
        self
    }
    pub fn with_runtime(
        mut self,
        phase: &str,
        time_ps: Option<u64>,
        event_seq: Option<u64>,
        target: Option<&str>,
    ) -> Self {
        self.stage = phase.into();
        self.time_ps = time_ps;
        self.event_seq = event_seq;
        if let Some(target) = target {
            self.target = Some(target.into());
        }
        self
    }
    pub fn with_detail(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        let details = self.details.get_or_insert_with(|| serde_json::json!({}));
        if !details.is_object() {
            *details = serde_json::json!({});
        }
        details
            .as_object_mut()
            .unwrap()
            .insert(key.into(), serde_json::Value::String(value.to_string()));
        self
    }
    pub fn normalize(&mut self, seq: u64, primary: bool) {
        self.seq = seq;
        self.primary = primary;
        if let Some(details) = self.details.as_ref().and_then(serde_json::Value::as_object) {
            if self.target.is_none() {
                self.target = details
                    .get("target")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
            }
            if let Some(reason) = details
                .get("reason")
                .or_else(|| details.get("kind"))
                .and_then(serde_json::Value::as_str)
            {
                self.reason = reason.into();
            }
        }
        if !matches!(
            self.reason.as_str(),
            "input_unreadable"
                | "invalid_utf8"
                | "syntax_error"
                | "unsupported_syntax"
                | "duplicate_definition"
                | "unknown_type"
                | "unknown_instance"
                | "unknown_parameter"
                | "missing_value"
                | "invalid_type"
                | "invalid_unit"
                | "invalid_range"
                | "invalid_connection"
                | "implementation_missing"
                | "allocation_failed"
                | "initialize_failed"
                | "invalid_argument"
                | "duplicate_metric"
                | "unsupported_profile"
                | "unsupported_profile_setting"
                | "model_config_invalid"
                | "workload_invalid"
                | "topology_invalid"
                | "model_failed"
                | "past_event"
                | "invalid_event"
                | "model_panic"
                | "finish_failed"
                | "output_not_empty"
                | "output_unwritable"
                | "output_create_failed"
                | "output_write_failed"
                | "output_flush_failed"
                | "output_close_failed"
                | "output_rename_failed"
                | "output_hash_failed"
                | "event_limit"
                | "delta_cycle_limit"
                | "arithmetic_overflow"
                | "allocation_limit"
        ) {
            let original_reason = std::mem::replace(&mut self.reason, "model_failed".into());
            self.code = "E-0002".into();
            let normalized = self.clone().with_detail("original_reason", original_reason);
            self.details = normalized.details;
        }
    }
    /// Clone normalization keeps stderr and persisted copies on the same sequence.
    pub fn normalized(&self, seq: u64, primary: bool) -> Self {
        let mut diagnostic = self.clone();
        diagnostic.normalize(seq, primary);
        diagnostic
    }
    pub fn wire_value(&self) -> serde_json::Value {
        self.normalized(self.seq, self.primary)
            .wire_value_normalized()
    }
    fn wire_value_normalized(&self) -> serde_json::Value {
        let mut details = std::collections::BTreeMap::<String, String>::new();
        if let Some(object) = self.details.as_ref().and_then(serde_json::Value::as_object) {
            for (key, value) in object {
                details.insert(
                    key.clone(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
        }
        serde_json::json!({"schema_version":self.schema_version,"seq":self.seq.to_string(),
            "severity":"error","primary":self.primary,"code":self.code,"phase":self.stage,
            "reason":self.reason,"message":self.message,"source":self.source,
            "line":self.line.map(|n| n.to_string()),"column":self.column.map(|n| n.to_string()),
            "end_line":self.end_line.map(|n| n.to_string()),"end_column":self.end_column.map(|n| n.to_string()),
            "target":self.target,"time_ps":self.time_ps.map(|n| n.to_string()),
            "event_seq":self.event_seq.map(|n| n.to_string()),"details":details})
    }
}
impl Serialize for Diagnostic {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire_value().serialize(serializer)
    }
}
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Diagnostic {}

#[derive(Debug, Clone)]
pub struct PreparedCommon {
    pub provenance: crate::input::PreparedProvenance,
    pub run_identity: Option<crate::input::RunIdentity>,
    pub profile: String,
    pub network: String,
    pub module_paths: Vec<String>,
    pub time_limit_ps: u64,
    pub metrics_window_ps: u64,
    pub max_events: u64,
    pub max_delta_cycles: u64,
    pub channel_count: usize,
    pub config_path: PathBuf,
    pub model_config_path: Option<PathBuf>,
    pub workload_path: Option<PathBuf>,
    pub inputs: Vec<InputSnapshot>,
}
