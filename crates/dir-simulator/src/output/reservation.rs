//! Output path validation and exclusive reservation, held through export.
use crate::types::{Diagnostic, PreparedSimulation};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct OutputReservation {
    pub(crate) path: PathBuf,
    _lock: fs::File,
}
pub(crate) fn reserve_output(
    path: &Path,
    prepared: &PreparedSimulation,
) -> Result<OutputReservation, Diagnostic> {
    reserve_output_for_inputs(path, &prepared.common.inputs)
}

pub(crate) fn reserve_output_for_inputs(
    path: &Path,
    inputs: &[crate::types::InputSnapshot],
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
    if inputs.iter().any(|input| input.path.starts_with(&target)) {
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
    // Lock the directory inode, not a directory entry which would remain in a
    // killed, otherwise complete bundle. Hold the descriptor through export.
    let lock = fs::File::open(&target).map_err(io_error)?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|e| io_error(e.into()))?;
    // Another writer may have published between the initial check and the lock.
    if fs::read_dir(&target).map_err(io_error)?.next().is_some() {
        return Err(Diagnostic::output(
            "output directory is not empty; existing results were preserved",
        ));
    }
    Ok(OutputReservation {
        path: target,
        _lock: lock,
    })
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn reservation_excludes_other_writers_without_a_bundle_entry() {
        let path =
            std::env::temp_dir().join(format!("dir-reservation-inode-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let first = reserve_output_for_inputs(&path, &[]).unwrap();
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        assert!(reserve_output_for_inputs(&path, &[]).is_err());
        drop(first);
        let second = reserve_output_for_inputs(&path, &[]).unwrap();
        drop(second);
        fs::remove_dir(path).unwrap();
    }
}
