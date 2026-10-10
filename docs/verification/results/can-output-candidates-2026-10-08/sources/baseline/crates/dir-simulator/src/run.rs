//! Reserve output, execute a prepared run, and export committed results.
use crate::input::{FsInputSource, InputDirEntry, InputMetadata, InputSource};
use crate::output::reservation::reserve_output;
use crate::types::InputSnapshot;
use crate::{Diagnostic, PreparedSimulation, output, runtime};
use serde::Serialize;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// An unsuccessful run, including diagnostics committed before the final failure.
#[derive(Debug)]
pub struct RunFailure {
    /// The final error that determines the CLI exit status.
    pub diagnostic: Diagnostic,
    /// Earlier execution diagnostics, in their original order.
    pub prior_diagnostics: Vec<Diagnostic>,
    /// The simulation's termination before result publication failed.
    pub original_termination: Option<String>,
}

impl From<Diagnostic> for RunFailure {
    fn from(diagnostic: Diagnostic) -> Self {
        Self {
            diagnostic,
            prior_diagnostics: Vec::new(),
            original_termination: None,
        }
    }
}

impl std::fmt::Display for RunFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for diagnostic in &self.prior_diagnostics {
            write!(f, "{diagnostic}; ")?;
        }
        self.diagnostic.fmt(f)
    }
}

impl std::error::Error for RunFailure {}

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub schema_version: u32,
    pub termination: String,
    pub exit_code: u8,
    pub partial: bool,
    pub output_path: PathBuf,
    pub manifest_path: PathBuf,
    pub primary_diagnostic: Option<Diagnostic>,
    pub committed_events: String,
    pub finish_ps: String,
    pub last_event_time_ps: Option<String>,
    /// Wall time spent in the runtime's event dispatch loop, excluding preparation and export.
    pub event_processing_wall_seconds: Option<f64>,
    #[serde(skip)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Run once and publish a hash-checked result set in a new or empty directory.
/// For separately structured diagnostics on publication failure, use
/// [`run_with_diagnostics`]. This compatibility entry point retains preceding
/// errors in the final diagnostic's message.
pub fn run(prepared: PreparedSimulation, output_path: &Path) -> Result<RunReport, Diagnostic> {
    run_with_diagnostics(prepared, output_path).map_err(|failure| {
        let message = failure.to_string();
        let mut diagnostic = failure.diagnostic;
        if !failure.prior_diagnostics.is_empty() {
            diagnostic.message = message;
        }
        diagnostic
    })
}

/// Run once, preserving execution diagnostics if result publication also fails.
pub fn run_with_diagnostics(
    prepared: PreparedSimulation,
    output_path: &Path,
) -> Result<RunReport, Box<RunFailure>> {
    run_with_export(prepared, output_path, output::export)
}

fn run_with_export(
    mut prepared: PreparedSimulation,
    output_path: &Path,
    export: impl FnOnce(
        &PreparedSimulation,
        &crate::snapshot::Snapshot,
        &Path,
    ) -> Result<(), Diagnostic>,
) -> Result<RunReport, Box<RunFailure>> {
    if prepared.common.run_identity.is_none() {
        prepared.common.run_identity = Some(output::new_run_identity().map_err(RunFailure::from)?);
    }
    let reservation = reserve_output(output_path, &prepared).map_err(RunFailure::from)?;
    let (simulation, event_processing_wall) = runtime::timing::measure(|| {
        runtime::spool::with_spooling(&reservation.path, || runtime::simulate(&prepared))
    });
    let mut snapshot = simulation.map_err(RunFailure::from)?;
    if let Some(diagnostic) = snapshot.common.spool_error.take() {
        normalize_diagnostics(&mut snapshot.common.diagnostics);
        let seq = snapshot.common.diagnostics.len() as u64;
        return Err(Box::new(RunFailure {
            diagnostic: diagnostic.normalized(seq, seq == 0),
            prior_diagnostics: snapshot.common.diagnostics,
            original_termination: Some(snapshot.common.termination),
        }));
    }
    normalize_diagnostics(&mut snapshot.common.diagnostics);
    if let Err(diagnostic) = export(&prepared, &snapshot, &reservation.path) {
        let seq = snapshot.common.diagnostics.len() as u64;
        return Err(Box::new(RunFailure {
            diagnostic: diagnostic.normalized(seq, seq == 0),
            prior_diagnostics: snapshot.common.diagnostics,
            original_termination: Some(snapshot.common.termination),
        }));
    }
    Ok(RunReport {
        schema_version: 1,
        termination: snapshot.common.termination.clone(),
        exit_code: if snapshot.common.termination == "prep_failed" {
            2
        } else if snapshot.common.partial {
            3
        } else {
            0
        },
        partial: snapshot.common.partial,
        manifest_path: reservation.path.join("manifest.json"),
        output_path: reservation.path.clone(),
        primary_diagnostic: snapshot.common.diagnostics.first().cloned(),
        committed_events: snapshot.common.committed_events.to_string(),
        finish_ps: snapshot.common.end_ps.to_string(),
        last_event_time_ps: snapshot.common.last_event_time_ps.map(|t| t.to_string()),
        event_processing_wall_seconds: Some(event_processing_wall.as_secs_f64()),
        diagnostics: snapshot.common.diagnostics,
    })
}

pub(crate) fn normalize_diagnostics(diagnostics: &mut [Diagnostic]) {
    for (seq, diagnostic) in diagnostics.iter_mut().enumerate() {
        diagnostic.normalize(seq as u64, seq == 0);
    }
}

struct CapturingSource {
    snapshots: RefCell<Vec<InputSnapshot>>,
}
impl InputSource for CapturingSource {
    fn metadata(&self, path: &Path) -> std::io::Result<InputMetadata> {
        FsInputSource.metadata(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<InputDirEntry>> {
        FsInputSource.read_dir(path)
    }
    fn read_utf8(&self, path: &Path) -> std::io::Result<String> {
        if let Some(snapshot) = self.snapshots.borrow().iter().find(|i| i.path == path) {
            return Ok(snapshot.content.clone());
        }
        let content = FsInputSource.read_utf8(path)?;
        self.snapshots.borrow_mut().push(InputSnapshot {
            path: path.to_path_buf(),
            content: content.clone(),
        });
        Ok(content)
    }
}

/// Prepare and run a configuration, publishing a diagnostic result on preparation failure.
/// Inputs are captured when first read; failure publication does not read them again.
pub fn run_config(config_path: &Path, output_path: &Path) -> Result<RunReport, Box<RunFailure>> {
    run_config_with_registry(
        config_path,
        output_path,
        crate::registry::Registry::default(),
    )
}

/// Prepare, execute and publish through a frozen statically linked registry.
pub fn run_config_with_registry(
    config_path: &Path,
    output_path: &Path,
    registry: crate::registry::Registry,
) -> Result<RunReport, Box<RunFailure>> {
    let identity = output::new_run_identity().map_err(RunFailure::from)?;
    let cwd = std::env::current_dir().map_err(|e| {
        RunFailure::from(Diagnostic::prepare(format!(
            "Cannot determine current directory: {e}"
        )))
    })?;
    let source = CapturingSource {
        snapshots: RefCell::new(Vec::new()),
    };
    match crate::registry::prepare_with_registry_and_source(
        config_path,
        &cwd,
        &source,
        registry.clone(),
    ) {
        Ok(mut prepared) => {
            prepared.common.run_identity = Some(identity);
            run_with_diagnostics(prepared, output_path)
        }
        Err(mut diagnostic) => {
            diagnostic.normalize(0, true);
            let inputs = source.snapshots.into_inner();
            let reservation =
                crate::output::reservation::reserve_output_for_inputs(output_path, &inputs)
                    .map_err(|d| {
                        Box::new(RunFailure {
                            diagnostic: d.normalized(1, false),
                            prior_diagnostics: vec![diagnostic.clone()],
                            original_termination: Some("prep_failed".into()),
                        })
                    })?;
            if let Err(d) = output::export_preparation_failure(
                config_path,
                &cwd,
                &inputs,
                &identity,
                &diagnostic,
                &reservation.path,
                &registry,
            ) {
                return Err(Box::new(RunFailure {
                    diagnostic: d.normalized(1, false),
                    prior_diagnostics: vec![diagnostic],
                    original_termination: Some("prep_failed".into()),
                }));
            }
            Ok(RunReport {
                schema_version: 1,
                termination: "prep_failed".into(),
                exit_code: 2,
                partial: true,
                output_path: reservation.path.clone(),
                manifest_path: reservation.path.join("manifest.json"),
                primary_diagnostic: Some(diagnostic.clone()),
                diagnostics: vec![diagnostic],
                committed_events: "0".into(),
                finish_ps: "0".into(),
                last_event_time_ps: None,
                event_processing_wall_seconds: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn publication_failure_preserves_execution_diagnostics_without_duplicates() {
        for max_events in [1, u64::MAX] {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let mut prepared =
                crate::prepare(&root.join("docs/verification/fixtures/can/competition.ini"))
                    .unwrap();
            prepared.common.max_events = max_events;
            let destination = std::env::temp_dir().join(format!(
                "dir-run-failure-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let failure = run_with_export(prepared, &destination, |_, _, _| {
                Err(Diagnostic::output("injected publication failure"))
            })
            .unwrap_err();
            assert_eq!(failure.diagnostic.code, "E-0003");
            if max_events == 1 {
                assert_eq!(failure.prior_diagnostics.len(), 1);
                assert_eq!(failure.prior_diagnostics[0].code, "E-0004");
                assert_eq!(failure.prior_diagnostics[0].message, "max-events exceeded");
                assert_eq!(
                    failure.original_termination.as_deref(),
                    Some("execution_failed")
                );
                assert!(failure.to_string().contains("E-0004: max-events exceeded"));
            } else {
                assert!(failure.prior_diagnostics.is_empty());
                assert_eq!(
                    failure.original_termination.as_deref(),
                    Some("events_exhausted")
                );
            }
            assert!(!destination.join("manifest.json").exists());
            assert!(!destination.join(".dir-simulator.lock").exists());
            fs::remove_dir_all(destination).unwrap();
        }
    }
}
