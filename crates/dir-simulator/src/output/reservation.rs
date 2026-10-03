//! Output path validation and exclusive reservation, held through export.
use crate::types::{Diagnostic, PreparedSimulation};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct OutputReservation {
    pub(crate) path: PathBuf,
    lock: PathBuf,
}
impl Drop for OutputReservation {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock);
    }
}
pub(crate) fn reserve_output(
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
        .common
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
