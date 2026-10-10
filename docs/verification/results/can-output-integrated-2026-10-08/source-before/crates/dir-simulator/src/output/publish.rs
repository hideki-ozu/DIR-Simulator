//! File hashes and no-clobber, manifest-last publication.
use super::json::canonical;
use crate::snapshot::Snapshot;
use crate::types::Diagnostic;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
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

struct HashWriter {
    file: File,
    hash: Sha256,
    bytes: u64,
}
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.hash.update(&bytes[..n]);
        self.bytes = self
            .bytes
            .checked_add(n as u64)
            .ok_or_else(|| std::io::Error::other("output length overflow"))?;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
pub(super) struct Staging {
    directory: std::path::PathBuf,
    files: BTreeMap<String, ValueFile>,
}
struct ValueFile {
    sha256: String,
    bytes: u64,
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
impl Staging {
    pub(super) fn write(
        &mut self,
        name: &str,
        write: impl FnOnce(&mut dyn Write) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        if ![
            "results.json",
            "events.csv",
            "summary.csv",
            "diagnostics.jsonl",
        ]
        .contains(&name)
            || self.files.contains_key(name)
        {
            return Err(Diagnostic::output("unknown or duplicate output file"));
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(name))
            .map_err(|e| Diagnostic::output(format!("Cannot create {name}: {e}")))?;
        let mut writer = BufWriter::with_capacity(
            64 * 1024,
            HashWriter {
                file,
                hash: Sha256::new(),
                bytes: 0,
            },
        );
        write(&mut writer)?;
        writer
            .flush()
            .map_err(|e| Diagnostic::output(format!("Cannot flush {name}: {e}")))?;
        let writer = writer
            .into_inner()
            .map_err(|e| Diagnostic::output(format!("Cannot close {name}: {e}")))?;
        self.files.insert(
            name.into(),
            ValueFile {
                sha256: format!("{:x}", writer.hash.finalize()),
                bytes: writer.bytes,
            },
        );
        Ok(())
    }
}
/// Stream each file through a hashing writer, publishing the manifest last.
pub(super) fn stream(
    output: &Path,
    run_id: &str,
    version: u32,
    snapshot: &Snapshot,
    write: impl FnOnce(&mut Staging) -> Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    let directory = output.join(format!(".tmp-{run_id}"));
    fs::create_dir(&directory)
        .map_err(|e| Diagnostic::output(format!("Cannot create output staging directory: {e}")))?;
    let mut staging = Staging {
        directory,
        files: BTreeMap::new(),
    };
    write(&mut staging)?;
    if staging.files.len() != 4 {
        return Err(Diagnostic::output("incomplete result file set"));
    }
    let files: Vec<_> = staging
        .files
        .iter()
        .map(
            |(name, file)| json!({"name":name,"sha256":file.sha256,"bytes":file.bytes.to_string()}),
        )
        .collect();
    let manifest = json!({"schema_version":version,"run_id":run_id,"status":"complete","termination":snapshot.common.termination,"partial":snapshot.common.partial,"metadata_ref":"results.json#/metadata","files":files});
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging.directory.join("manifest.json"))
        .map_err(|e| Diagnostic::output(format!("Cannot stage manifest: {e}")))?;
    file.write_all(format!("{}\n", canonical(&manifest)).as_bytes())
        .and_then(|()| file.flush())
        .map_err(|e| Diagnostic::output(format!("Cannot write manifest: {e}")))?;
    drop(file);
    for name in staging
        .files
        .keys()
        .map(String::as_str)
        .chain(std::iter::once("manifest.json"))
    {
        fs::hard_link(staging.directory.join(name), output.join(name))
            .map_err(|e| Diagnostic::output(format!("Cannot publish {name}: {e}")))?;
    }
    Ok(())
}

/// Publish captured preparation-failure files without replacing existing paths.
pub(super) fn publish_files(
    output: &Path,
    run_id: &str,
    version: u32,
    termination: &str,
    partial: bool,
    files: BTreeMap<&str, Vec<u8>>,
) -> Result<(), Diagnostic> {
    let manifest_files: Vec<_> = files.iter().map(|(name,bytes)| json!({"name":name,"sha256":digest(bytes),"bytes":bytes.len().to_string()})).collect();
    let manifest = json!({"schema_version":version,"run_id":run_id,"status":"complete","termination":termination,"partial":partial,"metadata_ref":"results.json#/metadata","files":manifest_files});
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

#[cfg(test)]
mod acceptance_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// DIR-TEST-0081: fail real create/link boundaries and the supplied writer
    /// boundary; a failed stage must never publish a successful manifest.
    #[test]
    fn failed_data_and_manifest_publication_preserve_external_paths() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let p = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/verification/fixtures/can/delay-filter.ini"),
        )
        .unwrap();
        let snapshot = Snapshot::empty(&p);
        for site in [
            "data-create",
            "data-write",
            "data-publish",
            "manifest-stage",
            "manifest-publish",
        ] {
            let out = std::env::temp_dir().join(format!(
                "dir-acceptance-publish-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&out).unwrap();
            if site == "data-publish" {
                fs::write(out.join("events.csv"), b"external sentinel").unwrap();
            }
            if site == "manifest-publish" {
                fs::create_dir(out.join("manifest.json")).unwrap();
            }
            let failed = stream(&out, "acceptance", 1, &snapshot, |staging| {
                if site == "data-create" {
                    fs::create_dir(staging.directory.join("results.json")).unwrap();
                }
                for name in [
                    "results.json",
                    "events.csv",
                    "summary.csv",
                    "diagnostics.jsonl",
                ] {
                    staging.write(name, |writer| {
                        writer
                            .write_all(b"completed file\n")
                            .map_err(|e| Diagnostic::output(e.to_string()))?;
                        if site == "data-write" {
                            return Err(Diagnostic::output("injected writer boundary failure"));
                        }
                        Ok(())
                    })?;
                }
                if site == "manifest-stage" {
                    fs::create_dir(staging.directory.join("manifest.json")).unwrap();
                }
                Ok(())
            })
            .unwrap_err();
            assert_eq!(failed.code, "E-0003", "{site}");
            assert!(!out.join("manifest.json").is_file(), "{site}");
            assert!(!out.join(".tmp-acceptance").exists(), "{site}");
            if site == "data-publish" {
                assert_eq!(
                    fs::read(out.join("events.csv")).unwrap(),
                    b"external sentinel"
                );
            }
            if site == "manifest-publish" {
                assert!(out.join("manifest.json").is_dir());
            }
            fs::remove_dir_all(&out).unwrap();
        }
    }
}
