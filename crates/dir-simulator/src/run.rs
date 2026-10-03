//! Reserve output, execute a prepared run, and export committed results.
use crate::output::reservation::reserve_output;
use crate::{Diagnostic, PreparedSimulation, output, runtime};
use serde::Serialize;
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
    prepared: PreparedSimulation,
    output_path: &Path,
    export: impl FnOnce(
        &PreparedSimulation,
        &crate::snapshot::Snapshot,
        &Path,
    ) -> Result<(), Diagnostic>,
) -> Result<RunReport, Box<RunFailure>> {
    let reservation = reserve_output(output_path, &prepared).map_err(RunFailure::from)?;
    let snapshot = runtime::simulate(&prepared).map_err(RunFailure::from)?;
    if let Err(diagnostic) = export(&prepared, &snapshot, &reservation.path) {
        return Err(Box::new(RunFailure {
            diagnostic,
            prior_diagnostics: snapshot.common.diagnostics,
            original_termination: Some(snapshot.common.termination),
        }));
    }
    Ok(RunReport {
        schema_version: 1,
        termination: snapshot.common.termination.clone(),
        exit_code: if snapshot.common.partial { 3 } else { 0 },
        partial: snapshot.common.partial,
        manifest_path: reservation.path.join("manifest.json"),
        output_path: reservation.path.clone(),
        diagnostics: snapshot.common.diagnostics,
    })
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
