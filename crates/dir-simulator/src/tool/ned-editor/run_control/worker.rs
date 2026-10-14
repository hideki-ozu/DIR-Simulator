//! Same-binary private worker; bounded frames, no command line paths or secrets.
use super::{storage::Directory, *};
use crate::input::RunIdentity;
use crate::output::{ResultInputEvidence, ResultReportEvidence, result_input_evidence};
use rustix::process::{self as process, Resource, Rlimit, Signal};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub(crate) const FRAME_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub schema_version: u32,
    pub probe: bool,
    pub parent_pid: u32,
    pub run_id: String,
    pub started_at_utc: String,
    pub run_root: PathBuf,
    pub bundle_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Message {
    Ready,
    Running {
        run_id: String,
        started_at_utc: String,
        inputs: Vec<ResultInputEvidence>,
    },
    PreparedFailed {
        diagnostic: Value,
    },
    Report {
        run_id: String,
        started_at_utc: String,
        report: Value,
        evidence: ResultReportEvidence,
        inputs: Vec<ResultInputEvidence>,
    },
    Failed {
        diagnostic: Value,
        prior_diagnostics: Vec<Value>,
        original_termination: Option<String>,
    },
    InfrastructureFailed {
        message: String,
    },
}
pub(crate) fn write_frame(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| storage::invalid(e.to_string()))?;
    if bytes.len() > FRAME_LIMIT {
        return Err(storage::invalid("Worker response exceeds 8 MiB"));
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .and_then(|()| writer.write_all(&bytes))
        .and_then(|()| writer.flush())
        .map_err(|e| EditorError::new("E-EDITOR-RUN-PROTOCOL", e.to_string(), 500))
}
pub(crate) fn read_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0; 4];
    reader
        .read_exact(&mut length)
        .map_err(|e| EditorError::new("E-EDITOR-RUN-PROTOCOL", e.to_string(), 500))?;
    let n = u32::from_be_bytes(length) as usize;
    if n == 0 || n > FRAME_LIMIT {
        return Err(storage::invalid("Invalid worker frame size"));
    }
    let mut bytes = vec![0; n];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| storage::invalid(e.to_string()))?;
    // Duplicate object keys are invalid control data as well as invalid requests.
    let value = super::super::controller::strict_json(&bytes)?;
    serde_json::from_value(value).map_err(|e| storage::invalid(e.to_string()))
}
pub(crate) fn enforce() -> Result<()> {
    process::umask(rustix::fs::Mode::from_bits_truncate(0o077));
    process::set_parent_process_death_signal(Some(Signal::Kill))
        .map_err(|e| storage::invalid(format!("Parent-death protection unavailable: {e}")))?;
    process::setpgid(None, None)
        .map_err(|e| storage::invalid(format!("Worker process group unavailable: {e}")))?;
    for (resource, limit) in [
        (Resource::As, 4 * 1024 * 1024 * 1024u64),
        (Resource::Fsize, 1024 * 1024 * 1024u64),
    ] {
        process::setrlimit(
            resource,
            Rlimit {
                current: Some(limit),
                maximum: Some(limit),
            },
        )
        .map_err(|e| storage::invalid(format!("Worker hard resource limit unavailable: {e}")))?;
        if process::getrlimit(resource)
            != (Rlimit {
                current: Some(limit),
                maximum: Some(limit),
            })
        {
            return Err(storage::invalid("Worker resource limit not applied"));
        }
    }
    Ok(())
}
fn execute(request: Request, stdout: &mut impl Write) -> Result<()> {
    if request.schema_version != 1 {
        return Err(storage::invalid("Unsupported worker protocol"));
    }
    // Setting pdeathsig before checking PPID closes the parent's startup-death race.
    enforce()?;
    if process::getppid().map(|p| p.as_raw_nonzero().get() as u32) != Some(request.parent_pid) {
        return Err(storage::invalid("Worker parent identity changed"));
    }
    write_frame(stdout, &Message::Ready)?;
    if request.probe {
        return Ok(());
    }
    valid_id(&request.run_id)?;
    let root = Directory::open(&request.run_root)?;
    if request.run_root.file_name().and_then(|s| s.to_str()) != Some(&request.run_id) {
        return Err(storage::invalid("Worker run path/identity mismatch"));
    }
    let project = super::super::run_snapshot::restore(
        &request.run_root.join("input"),
        &request.bundle_digest,
    )?;
    #[cfg(test)]
    crate::acceptance_hooks::hit("worker.before_prepare");
    let mut prepared = match super::super::analysis::prepare(&project) {
        Ok(p) => p,
        Err(d) => {
            let m = Message::PreparedFailed {
                diagnostic: d.wire_value(),
            };
            publish_response(&root.child("control")?, stdout, &m)?;
            return Ok(());
        }
    };
    let inputs = result_input_evidence(&prepared);
    prepared.common.run_identity = Some(RunIdentity {
        run_id: request.run_id.clone(),
        started_at_utc: request.started_at_utc.clone(),
    });
    root.verify()?;
    write_frame(
        stdout,
        &Message::Running {
            run_id: request.run_id.clone(),
            started_at_utc: request.started_at_utc.clone(),
            inputs: inputs.clone(),
        },
    )?;
    #[cfg(test)]
    crate::acceptance_hooks::hit("worker.before_execute");
    let message = match crate::run_with_diagnostics(prepared, &request.run_root.join("result")) {
        Ok(report) => {
            let evidence = ResultReportEvidence::from(&report);
            let mut value =
                serde_json::to_value(&report).map_err(|e| storage::invalid(e.to_string()))?;
            value["diagnostics"] = serde_json::to_value(&report.diagnostics)
                .map_err(|e| storage::invalid(e.to_string()))?;
            Message::Report {
                run_id: request.run_id.clone(),
                started_at_utc: request.started_at_utc.clone(),
                report: value,
                evidence,
                inputs,
            }
        }
        Err(f) => Message::Failed {
            diagnostic: f.diagnostic.wire_value(),
            prior_diagnostics: f
                .prior_diagnostics
                .iter()
                .map(crate::Diagnostic::wire_value)
                .collect(),
            original_termination: f.original_termination,
        },
    };
    // A complete response can survive loss of the pipe or coordinator.
    publish_response(&root.child("control")?, stdout, &message)
}
pub(crate) const DIAGNOSTIC_SUMMARY_LIMIT: usize = 32 * 1024;
pub(crate) const LARGE_RESPONSE_ARTIFACT: &str = "large-worker-response";
pub(crate) const LARGE_RESPONSE_FILE: &str = "large-worker-response.json";

// Measure the emitted JSON string, including escaping. Search only a bounded
// prefix so huge names and paths cannot allocate another complete message.
fn bounded_string(value: &str, budget: usize) -> String {
    const MARKER: &str = "... [truncated]";
    if value.len() <= budget
        && serde_json::to_vec(value).expect("String serializes").len() <= budget
    {
        return value.to_owned();
    }
    let mut low = 0;
    let mut high = value.len().min(budget);
    let mut best = 0;
    while low <= high {
        let middle = low + (high - low) / 2;
        let mut end = middle;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        let candidate = format!("{}{MARKER}", &value[..end]);
        if serde_json::to_vec(&candidate)
            .expect("String serializes")
            .len()
            <= budget
        {
            best = end;
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    format!("{}{MARKER}", &value[..best])
}
fn bounded_field(value: &Value, budget: usize) -> Value {
    match value {
        Value::String(s) => json!(bounded_string(s, budget)),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        // Diagnostics are produced by the common wire serializer. Unexpected
        // compound metadata is retained in the full artifact, never expanded.
        _ => Value::Null,
    }
}
fn prepare_summary(diagnostic: &Value, full_bytes: &[u8]) -> Result<Value> {
    let mut summary = serde_json::Map::new();
    for key in [
        "schema_version",
        "seq",
        "severity",
        "primary",
        "code",
        "phase",
        "reason",
        "line",
        "column",
        "end_line",
        "end_column",
        "time_ps",
        "event_seq",
    ] {
        if let Some(value) = diagnostic.get(key) {
            summary.insert(key.into(), bounded_field(value, 256));
        }
    }
    for key in ["source", "target"] {
        if let Some(value) = diagnostic.get(key) {
            summary.insert(key.into(), bounded_field(value, 4096));
        }
    }
    let prefix = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .map(|message| bounded_string(message, 8192))
        .unwrap_or_else(|| "Input preparation failed".into());
    summary.insert("message".into(), json!(format!("{prefix}\nDiagnostic summary only. Full original data is retained; retrieve the large-worker-response artifact.")));
    summary.insert("diagnostic_summary".into(), json!(true));
    summary.insert("details_artifact".into(), json!(LARGE_RESPONSE_ARTIFACT));
    summary.insert("details_sha256".into(), json!(hash(full_bytes)));
    summary.insert("details_bytes".into(), json!(full_bytes.len().to_string()));
    summary.insert("details".into(), json!({"notice":"Summary only; full original diagnostic retained in the named artifact", "details_artifact":LARGE_RESPONSE_ARTIFACT,"details_sha256":hash(full_bytes),"details_bytes":full_bytes.len().to_string()}));
    let summary = Value::Object(summary);
    if serde_json::to_vec(&summary)
        .map_err(|e| storage::invalid(e.to_string()))?
        .len()
        > DIAGNOSTIC_SUMMARY_LIMIT
    {
        return Err(storage::invalid(
            "Prepare diagnostic summary exceeds 32 KiB",
        ));
    }
    Ok(summary)
}

pub(super) fn publish_response(
    control: &Directory,
    stdout: &mut impl Write,
    message: &Message,
) -> Result<()> {
    let bytes = serde_json::to_vec(message).map_err(|e| storage::invalid(e.to_string()))?;
    if bytes.len() > FRAME_LIMIT {
        // Full original wire data is durable before publishing a bounded reply.
        control.replace(LARGE_RESPONSE_FILE, &bytes)?;
        let small = match message {
            Message::PreparedFailed { diagnostic } => Message::PreparedFailed {
                diagnostic: prepare_summary(diagnostic, &bytes)?,
            },
            Message::Failed {
                diagnostic,
                prior_diagnostics,
                original_termination,
            } => {
                let mut final_summary = prepare_summary(diagnostic, &bytes)?;
                let mut prior = Vec::new();
                let mut summary_bytes = 0usize;
                for diagnostic in prior_diagnostics {
                    let summary = prepare_summary(diagnostic, &bytes)?;
                    let size = serde_json::to_vec(&summary)
                        .map_err(|e| storage::invalid(e.to_string()))?
                        .len();
                    if summary_bytes.saturating_add(size) > 1024 * 1024 {
                        break;
                    }
                    summary_bytes += size;
                    prior.push(summary);
                }
                final_summary["details"]["original_prior_diagnostics_count"] =
                    json!(prior_diagnostics.len().to_string());
                final_summary["details"]["prior_diagnostics_summary_truncated"] =
                    json!(prior.len() != prior_diagnostics.len());
                Message::Failed {
                    diagnostic: final_summary,
                    prior_diagnostics: prior,
                    original_termination: original_termination.clone(),
                }
            }
            _ => Message::InfrastructureFailed {
                message:
                    "Worker response exceeds 8 MiB; preserved in control/large-worker-response.json"
                        .into(),
            },
        };
        control.replace(
            "worker-response.json",
            &serde_json::to_vec(&small).map_err(|e| storage::invalid(e.to_string()))?,
        )?;
        return write_frame(stdout, &small);
    }
    control.replace("worker-response.json", &bytes)?;
    #[cfg(test)]
    crate::acceptance_hooks::hit("worker.after_response");
    #[cfg(test)]
    crate::acceptance_hooks::hit("worker.before_frame");
    write_frame(stdout, message)
}
#[cfg(test)]
pub(super) fn acceptance_execute(request: Request, output: &mut impl Write) -> Result<()> {
    execute(request, output)
}
/// Private startup hook. This does not expose an editor HTTP operation.
pub fn hidden_worker() -> std::result::Result<(), crate::Diagnostic> {
    let mut stdout = std::io::stdout().lock();
    let result =
        read_frame::<Request>(&mut std::io::stdin().lock()).and_then(|r| execute(r, &mut stdout));
    if let Err(e) = result {
        let _ = write_frame(
            &mut stdout,
            &Message::InfrastructureFailed {
                message: e.to_string(),
            },
        );
        return Err(crate::Diagnostic::prepare(e.to_string()));
    }
    Ok(())
}
