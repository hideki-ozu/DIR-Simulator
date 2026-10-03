//! Reserve output, execute a prepared run, and export committed results.
use crate::output::reservation::reserve_output;
use crate::{Diagnostic, PreparedSimulation, output, runtime};
use serde::Serialize;
use std::path::{Path, PathBuf};

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
pub fn run(prepared: PreparedSimulation, output_path: &Path) -> Result<RunReport, Diagnostic> {
    let reservation = reserve_output(output_path, &prepared)?;
    let snapshot = runtime::simulate(&prepared)?;
    output::export(&prepared, &snapshot, &reservation.path)?;
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
