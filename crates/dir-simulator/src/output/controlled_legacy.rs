//! Schema 4 wraps the existing typed result without retaining its row arrays.
use crate::{Diagnostic, PreparedSimulation};
use serde_json::{Value, json};
use std::io::Write;

pub(super) fn write(
    writer: &mut dyn Write,
    prepared: &PreparedSimulation,
    control: Option<&Value>,
    payload: impl FnOnce(&mut dyn Write) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    let Some(control) = control else {
        return payload(writer);
    };
    let version = if prepared.common.profile == "can.cc.ideal.v1" {
        1
    } else {
        2
    };
    let prefix = json!({
        "schema_name":"dir.controlled.legacy", "schema_version":4,
        "metadata":{"model_profile":prepared.common.profile,"payload_schema_version":version},
        "control":control,
    });
    let prefix = super::json::canonical(&prefix);
    writer
        .write_all(prefix.strip_suffix('}').expect("object prefix").as_bytes())
        .and_then(|()| writer.write_all(b",\"result\":"))
        .map_err(|e| Diagnostic::output(format!("Cannot write control envelope: {e}")))?;
    payload(writer)?;
    writer
        .write_all(b"}\n")
        .map_err(|e| Diagnostic::output(format!("Cannot close control envelope: {e}")))
}
