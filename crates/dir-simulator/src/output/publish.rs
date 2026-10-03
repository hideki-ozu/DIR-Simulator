//! File hashes and no-clobber, manifest-last publication.
use super::json::canonical;
use crate::snapshot::Snapshot;
use crate::types::Diagnostic;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn run_id() -> Result<String, Diagnostic> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| Diagnostic::output(format!("Cannot obtain UUID entropy: {e}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").expect("writing to String");
    }
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Stage completed files, then publish each without replacing existing paths.
pub(super) fn publish(
    output: &Path,
    run_id: &str,
    version: u32,
    snapshot: &Snapshot,
    files: BTreeMap<&str, Vec<u8>>,
) -> Result<(), Diagnostic> {
    let manifest_files: Vec<_> = files.iter().map(|(name,bytes)| json!({"name":name,"sha256":digest(bytes),"bytes":bytes.len().to_string()})).collect();
    let manifest = json!({"schema_version":version,"run_id":run_id,"status":"complete","termination":snapshot.common.termination,"partial":snapshot.common.partial,"metadata_ref":"results.json#/metadata","files":manifest_files});
    let temporary = output.join(format!(".tmp-{run_id}"));
    fs::create_dir(&temporary)
        .map_err(|e| Diagnostic::output(format!("Cannot create output staging directory: {e}")))?;
    let publish = || -> std::io::Result<()> {
        for (name, bytes) in &files {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(temporary.join(name))?;
            file.write_all(bytes)?;
            file.flush()?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary.join("manifest.json"))?;
        file.write_all(format!("{}\n", canonical(&manifest)).as_bytes())?;
        file.flush()?;
        drop(file);
        // Hard-link publication supplies the no-replace property missing from std::fs::rename.
        // Each link exposes only a fully written file, within the same filesystem.
        for name in files
            .keys()
            .copied()
            .chain(std::iter::once("manifest.json"))
        {
            fs::hard_link(temporary.join(name), output.join(name))?;
            fs::remove_file(temporary.join(name))?;
        }
        Ok(())
    }();
    let _ = fs::remove_dir_all(&temporary);
    publish.map_err(|e| Diagnostic::output(format!("Cannot publish result files: {e}")))
}
