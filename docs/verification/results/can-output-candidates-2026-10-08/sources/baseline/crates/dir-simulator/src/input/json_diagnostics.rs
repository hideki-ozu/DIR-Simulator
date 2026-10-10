//! Positional JSON indexing. Semantic validators select explicit JSON pointers.
use super::{Result, error};
use crate::types::Diagnostic;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct JsonDocument<'a> {
    pub value: Value,
    text: &'a str,
    spans: BTreeMap<String, (usize, usize)>,
    keys: BTreeMap<String, (usize, usize)>,
}
impl<'a> JsonDocument<'a> {
    pub fn parse(text: &'a str) -> Result<Self> {
        let bom = if text.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let value = serde_json::from_str::<Value>(&text[bom..]).map_err(|e| {
            let prefix: usize = text[bom..]
                .split_inclusive('\n')
                .take(e.line().saturating_sub(1))
                .map(str::len)
                .sum();
            let line = text.get(bom + prefix..).unwrap_or("");
            let offset = bom + prefix + e.column().saturating_sub(1).min(line.len());
            // serde reports UTF-8 byte columns. Round the error byte to its scalar.
            let mut start = if e.is_eof() {
                text.len()
            } else {
                offset.min(text.len())
            };
            while !text.is_char_boundary(start) {
                start -= 1;
            }
            let end = start + text[start..].chars().next().map_or(0, char::len_utf8);
            error(e.to_string())
                .with_reason("syntax_error")
                .with_text_span(text, start, end)
                .with_detail("expected", "valid JSON")
        })?;
        let mut scanner = Scanner {
            text,
            cursor: bom,
            spans: BTreeMap::new(),
            keys: BTreeMap::new(),
        };
        scanner.value("")?;
        Ok(Self {
            value,
            text,
            spans: scanner.spans,
            keys: scanner.keys,
        })
    }
    pub fn span(&self, path: &std::path::Path, pointer: &str) -> Option<crate::types::SourceSpan> {
        self.spans
            .get(pointer)
            .map(|&(start, end)| crate::types::SourceSpan::new(path, self.text, start, end))
    }
    pub fn annotate(&self, pointer: &str, diagnostic: Diagnostic) -> Diagnostic {
        let spans = if diagnostic.reason == "unknown_parameter"
            || diagnostic.reason == "duplicate_definition"
        {
            &self.keys
        } else {
            &self.spans
        };
        let mut current = pointer;
        let range = loop {
            if let Some(range) = spans.get(current).or_else(|| self.spans.get(current)) {
                break Some(*range);
            }
            if let Some((parent, _)) = current.rsplit_once('/') {
                current = parent;
            } else {
                break None;
            }
        };
        let diagnostic = diagnostic.with_target(if pointer.is_empty() { "/" } else { pointer });
        if let Some((start, end)) = range {
            diagnostic.with_text_span(self.text, start, end)
        } else {
            diagnostic
        }
    }
    pub fn object<'v>(
        &self,
        pointer: &str,
        value: &'v Value,
        allowed: &[&str],
        target: &str,
    ) -> Result<&'v serde_json::Map<String, Value>> {
        let object = self.checked(pointer, super::object(value, allowed, target));
        match object {
            Err(diagnostic) if diagnostic.reason == "unknown_parameter" => {
                let key = value
                    .as_object()
                    .unwrap()
                    .keys()
                    .find(|key| !allowed.contains(&key.as_str()))
                    .unwrap();
                Err(self.annotate(
                    &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                    diagnostic,
                ))
            }
            result => result,
        }
    }
    pub fn string<'v>(
        &self,
        pointer: &str,
        object: &'v serde_json::Map<String, Value>,
        key: &str,
    ) -> Result<&'v str> {
        self.checked(
            &format!("{pointer}/{key}"),
            super::required_string(object, key),
        )
    }
    pub fn time(
        &self,
        pointer: &str,
        object: &serde_json::Map<String, Value>,
        key: &str,
    ) -> Result<u64> {
        self.checked(&format!("{pointer}/{key}"), super::json_time(object, key))
    }
    pub fn checked<T>(&self, pointer: &str, result: Result<T>) -> Result<T> {
        result.map_err(|diagnostic| self.annotate(pointer, diagnostic))
    }
}
struct Scanner<'a> {
    text: &'a str,
    cursor: usize,
    spans: BTreeMap<String, (usize, usize)>,
    keys: BTreeMap<String, (usize, usize)>,
}
impl Scanner<'_> {
    fn whitespace(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.cursor += 1;
        }
    }
    fn string(&mut self) -> (usize, usize) {
        let start = self.cursor;
        self.cursor += 1;
        let mut escaped = false;
        while let Some(&byte) = self.text.as_bytes().get(self.cursor) {
            self.cursor += 1;
            if byte == b'"' && !escaped {
                break;
            }
            escaped = byte == b'\\' && !escaped;
        }
        (start, self.cursor)
    }
    fn value(&mut self, pointer: &str) -> Result<()> {
        self.whitespace();
        let start = self.cursor;
        match self.text.as_bytes().get(self.cursor) {
            Some(b'{') => {
                self.cursor += 1;
                self.whitespace();
                let mut names = BTreeMap::new();
                while self.text.as_bytes().get(self.cursor) != Some(&b'}') {
                    self.whitespace();
                    let (start, end) = self.string();
                    let key: String =
                        serde_json::from_str(&self.text[start..end]).expect("validated JSON key");
                    let child = format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
                    if let Some((previous_start, _)) = names.insert(key.clone(), (start, end)) {
                        let previous = Diagnostic::prepare("").with_text_span(
                            self.text,
                            previous_start,
                            previous_start,
                        );
                        return Err(error(format!("duplicate JSON key: {key}"))
                            .with_reason("duplicate_definition")
                            .with_target(child)
                            .with_text_span(self.text, start, end)
                            .with_detail("actual", key)
                            .with_detail("expected", "unique object key")
                            .with_detail("related_line", previous.line.unwrap())
                            .with_detail("related_column", previous.column.unwrap()));
                    }
                    self.keys.insert(child.clone(), (start, end));
                    self.whitespace();
                    self.cursor += 1;
                    self.value(&child)?;
                    self.whitespace();
                    if self.text.as_bytes().get(self.cursor) == Some(&b',') {
                        self.cursor += 1;
                    } else {
                        break;
                    }
                }
                self.cursor += 1;
            }
            Some(b'[') => {
                self.cursor += 1;
                self.whitespace();
                let mut index = 0;
                while self.text.as_bytes().get(self.cursor) != Some(&b']') {
                    self.value(&format!("{pointer}/{index}"))?;
                    index += 1;
                    self.whitespace();
                    if self.text.as_bytes().get(self.cursor) == Some(&b',') {
                        self.cursor += 1;
                    } else {
                        break;
                    }
                }
                self.cursor += 1;
            }
            Some(b'"') => {
                self.string();
            }
            _ => {
                while self
                    .text
                    .as_bytes()
                    .get(self.cursor)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(b))
                {
                    self.cursor += 1;
                }
            }
        }
        self.spans.insert(pointer.into(), (start, self.cursor));
        Ok(())
    }
}

pub(crate) fn parse_json(content: &str) -> Result<Value> {
    JsonDocument::parse(content).map(|document| document.value)
}

/// A validation scope stores address identities only; it never dereferences them.
/// The document owns all values throughout the scope and nested scopes restore the caller.
struct ValidationContext {
    text: String,
    spans: BTreeMap<String, (usize, usize)>,
    keys: BTreeMap<String, (usize, usize)>,
    values: BTreeMap<usize, String>,
    objects: BTreeMap<usize, String>,
    actuals: BTreeMap<String, String>,
    current_object: String,
    default_reason: &'static str,
}
thread_local! {
    static VALIDATION: std::cell::RefCell<Option<ValidationContext>> = const { std::cell::RefCell::new(None) };
}
pub(super) struct ValidationScope(Option<ValidationContext>);
impl Drop for ValidationScope {
    fn drop(&mut self) {
        VALIDATION.with(|slot| {
            *slot.borrow_mut() = self.0.take();
        });
    }
}
impl JsonDocument<'_> {
    pub fn enter(&self, default_reason: &'static str) -> ValidationScope {
        fn index(
            value: &Value,
            pointer: &str,
            values: &mut BTreeMap<usize, String>,
            objects: &mut BTreeMap<usize, String>,
        ) {
            values.insert(value as *const Value as usize, pointer.into());
            match value {
                Value::Object(object) => {
                    objects.insert(
                        object as *const serde_json::Map<String, Value> as usize,
                        pointer.into(),
                    );
                    for (key, value) in object {
                        index(
                            value,
                            &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                            values,
                            objects,
                        );
                    }
                }
                Value::Array(array) => {
                    for (i, value) in array.iter().enumerate() {
                        index(value, &format!("{pointer}/{i}"), values, objects);
                    }
                }
                _ => {}
            }
        }
        let mut values = BTreeMap::new();
        let mut objects = BTreeMap::new();
        index(&self.value, "", &mut values, &mut objects);
        fn actuals(value: &Value, pointer: &str, output: &mut BTreeMap<String, String>) {
            output.insert(pointer.into(), actual(value));
            match value {
                Value::Object(object) => {
                    for (key, value) in object {
                        actuals(
                            value,
                            &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                            output,
                        );
                    }
                }
                Value::Array(array) => {
                    for (i, value) in array.iter().enumerate() {
                        actuals(value, &format!("{pointer}/{i}"), output);
                    }
                }
                _ => {}
            }
        }
        let mut actual_values = BTreeMap::new();
        actuals(&self.value, "", &mut actual_values);
        let context = ValidationContext {
            actuals: actual_values,
            text: self.text.into(),
            spans: self.spans.clone(),
            keys: self.keys.clone(),
            values,
            objects,
            current_object: String::new(),
            default_reason,
        };
        ValidationScope(VALIDATION.with(|slot| slot.borrow_mut().replace(context)))
    }
}
impl ValidationContext {
    fn annotate(&self, pointer: &str, diagnostic: Diagnostic) -> Diagnostic {
        let document = JsonDocument {
            value: Value::Null,
            text: &self.text,
            spans: self.spans.clone(),
            keys: self.keys.clone(),
        };
        document.annotate(pointer, diagnostic)
    }
}
pub(super) fn contextual(diagnostic: Diagnostic) -> Diagnostic {
    VALIDATION.with(|slot| {
        let slot = slot.borrow();
        if let Some(context) = slot.as_ref() {
            context.annotate(
                &context.current_object,
                diagnostic.with_reason(context.default_reason),
            )
        } else {
            diagnostic
        }
    })
}
pub(super) fn select_object(value: &Value) {
    VALIDATION.with(|slot| {
        if let Some(context) = slot.borrow_mut().as_mut() {
            if let Some(pointer) = context.values.get(&(value as *const Value as usize)) {
                context.current_object = pointer.clone();
            }
        }
    });
}
pub(super) fn field_result<T>(
    object: &serde_json::Map<String, Value>,
    key: &str,
    result: Result<T>,
) -> Result<T> {
    result.map_err(|diagnostic| {
        VALIDATION.with(|slot| {
            let slot = slot.borrow();
            let Some(context) = slot.as_ref() else {
                return diagnostic;
            };
            let Some(pointer) = context
                .objects
                .get(&(object as *const serde_json::Map<String, Value> as usize))
            else {
                return diagnostic;
            };
            let pointer = format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
            let mut diagnostic = diagnostic;
            if matches!(
                diagnostic.reason.as_str(),
                "invalid_argument" | "model_config_invalid" | "workload_invalid"
            ) {
                diagnostic.reason = if object.get(key).is_some_and(Value::is_number) {
                    "invalid_range"
                } else if object.contains_key(key) {
                    "invalid_type"
                } else {
                    "missing_value"
                }
                .into();
            }
            if !diagnostic
                .details
                .as_ref()
                .and_then(Value::as_object)
                .is_some_and(|details| details.contains_key("actual"))
            {
                diagnostic = diagnostic.with_detail(
                    "actual",
                    object
                        .get(key)
                        .map(actual)
                        .unwrap_or_else(|| "missing".into()),
                );
            }
            if !diagnostic
                .details
                .as_ref()
                .and_then(Value::as_object)
                .is_some_and(|details| details.contains_key("expected"))
            {
                diagnostic = diagnostic.with_detail("expected", "declared field type and range");
            }
            context.annotate(&pointer, diagnostic)
        })
    })
}
pub(super) fn value_result<T>(value: &Value, result: Result<T>) -> Result<T> {
    result.map_err(|diagnostic| {
        VALIDATION.with(|slot| {
            let slot = slot.borrow();
            let Some(context) = slot.as_ref() else {
                return diagnostic;
            };
            let Some(pointer) = context.values.get(&(value as *const Value as usize)) else {
                return diagnostic;
            };
            let diagnostic = if matches!(
                diagnostic.reason.as_str(),
                "invalid_argument" | "model_config_invalid" | "workload_invalid"
            ) {
                diagnostic
                    .with_reason("invalid_type")
                    .with_detail("actual", value)
                    .with_detail("expected", "declared scalar type")
            } else {
                diagnostic
            };
            context.annotate(pointer, diagnostic)
        })
    })
}

pub(super) struct ObjectScope(Option<String>);
impl Drop for ObjectScope {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            VALIDATION.with(|slot| {
                if let Some(context) = slot.borrow_mut().as_mut() {
                    context.current_object = previous;
                }
            });
        }
    }
}
pub(super) fn scoped_value(value: &Value) -> ObjectScope {
    let previous = VALIDATION.with(|slot| {
        let mut slot = slot.borrow_mut();
        let context = slot.as_mut()?;
        let pointer = context
            .values
            .get(&(value as *const Value as usize))?
            .clone();
        Some(std::mem::replace(&mut context.current_object, pointer))
    });
    ObjectScope(previous)
}

/// Select a field named explicitly by a validator's registered rule.
pub(super) fn current_field(key: &str, mut diagnostic: Diagnostic) -> Diagnostic {
    VALIDATION.with(|slot| {
        let slot = slot.borrow();
        let Some(context) = slot.as_ref() else {
            return diagnostic;
        };
        let pointer = format!(
            "{}/{}",
            context.current_object,
            key.replace('~', "~0").replace('/', "~1")
        );
        if !diagnostic
            .details
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|details| details.contains_key("actual"))
        {
            diagnostic = diagnostic.with_detail(
                "actual",
                context
                    .actuals
                    .get(&pointer)
                    .map(String::as_str)
                    .unwrap_or("missing"),
            );
        }
        if !diagnostic
            .details
            .as_ref()
            .and_then(Value::as_object)
            .is_some_and(|details| details.contains_key("expected"))
        {
            diagnostic = diagnostic.with_detail("expected", "declared field type and range");
        }
        context.annotate(&pointer, diagnostic)
    })
}

pub(super) fn actual(value: &Value) -> String {
    match value {
        Value::Array(_) => "array".into(),
        Value::Object(_) => "object".into(),
        _ => value.to_string(),
    }
}

pub(super) fn field_check<T>(
    object: &serde_json::Map<String, Value>,
    key: &str,
    check: impl FnOnce() -> Result<T>,
) -> Result<T> {
    field_result(object, key, check())
}
pub(super) fn value_check<T>(value: &Value, check: impl FnOnce() -> Result<T>) -> Result<T> {
    value_result(value, check())
}
