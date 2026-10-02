//! DIR Simulator 0.1: deterministic ideal Classical CAN simulation.
//!
//! ```no_run
//! let prepared = dir_simulator::prepare(std::path::Path::new("scenario.ini"))?;
//! let report = dir_simulator::run(prepared, std::path::Path::new("results"))?;
//! assert_eq!(report.exit_code, 0);
//! # Ok::<(), dir_simulator::types::Diagnostic>(())
//! ```
pub mod can;
pub mod input;
pub mod output;
pub mod runtime;
pub mod snapshot;
pub mod types;
pub mod viewer;

pub use input::prepare;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
pub use types::{Diagnostic, PreparedSimulation};

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

struct OutputReservation {
    path: PathBuf,
    lock: PathBuf,
}
impl Drop for OutputReservation {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock);
    }
}
fn reserve_output(
    path: &Path,
    prepared: &PreparedSimulation,
) -> Result<OutputReservation, Diagnostic> {
    let io_error = |e: std::io::Error| Diagnostic::output(format!("{}: {e}", path.display()));
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_err(io_error)?.join(path)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| Diagnostic::output("output must have an existing parent directory"))?
        .canonicalize()
        .map_err(io_error)?;
    let name = absolute
        .file_name()
        .ok_or_else(|| Diagnostic::output("output directory name is missing"))?;
    let target = parent.join(name);
    if prepared
        .inputs
        .iter()
        .any(|input| input.path.starts_with(&target))
    {
        return Err(Diagnostic::prepare(
            "output directory contains an input file",
        ));
    }
    match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(Diagnostic::output("output must be a real directory"));
            }
            if fs::read_dir(&target).map_err(io_error)?.next().is_some() {
                return Err(Diagnostic::output(
                    "output directory is not empty; existing results were preserved",
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&target).map_err(io_error)?
        }
        Err(e) => return Err(io_error(e)),
    }
    let lock = target.join(".dir-simulator.lock");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .map_err(io_error)?;
    Ok(OutputReservation { path: target, lock })
}

/// Run once and publish a hash-checked result set in a new or empty directory.
pub fn run(prepared: PreparedSimulation, output_path: &Path) -> Result<RunReport, Diagnostic> {
    let reservation = reserve_output(output_path, &prepared)?;
    let snapshot = runtime::simulate(&prepared)?;
    output::export(&prepared, &snapshot, &reservation.path)?;
    Ok(RunReport {
        schema_version: 1,
        termination: snapshot.termination.clone(),
        exit_code: if snapshot.partial { 3 } else { 0 },
        partial: snapshot.partial,
        manifest_path: reservation.path.join("manifest.json"),
        output_path: reservation.path.clone(),
        diagnostics: snapshot.diagnostics,
    })
}
