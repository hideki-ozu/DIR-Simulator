//! File hashes and no-clobber, manifest-last publication.
use super::json::canonical;
use crate::snapshot::Snapshot;
use crate::types::Diagnostic;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::MetadataExt;
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
    owned: BTreeMap<String, (u64, u64)>,
    directory_removed: bool,
}
struct ValueFile {
    sha256: String,
    bytes: u64,
}
impl Drop for Staging {
    fn drop(&mut self) {
        if self.directory_removed {
            return;
        }
        // Never recursively remove unrecognized entries, including failed create
        // destinations. The reserved output directory must remain exclusively owned
        // by this writer; this is not a defense against concurrent namespace tampering.
        for (name, identity) in &self.owned {
            let path = self.directory.join(name);
            if fs::symlink_metadata(&path)
                .is_ok_and(|m| m.is_file() && (m.dev(), m.ino()) == *identity)
            {
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_dir(&self.directory);
    }
}
impl Staging {
    fn open(&mut self, name: &str) -> Result<BufWriter<HashWriter>, Diagnostic> {
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
        let metadata = file
            .metadata()
            .map_err(|e| Diagnostic::output(format!("Cannot inspect {name}: {e}")))?;
        self.owned
            .insert(name.into(), (metadata.dev(), metadata.ino()));
        Ok(BufWriter::with_capacity(
            64 * 1024,
            HashWriter {
                file,
                hash: Sha256::new(),
                bytes: 0,
            },
        ))
    }
    fn close(name: &str, mut writer: BufWriter<HashWriter>) -> Result<ValueFile, Diagnostic> {
        writer
            .flush()
            .map_err(|e| Diagnostic::output(format!("Cannot flush {name}: {e}")))?;
        let writer = writer
            .into_inner()
            .map_err(|e| Diagnostic::output(format!("Cannot close {name}: {e}")))?;
        Ok(ValueFile {
            sha256: format!("{:x}", writer.hash.finalize()),
            bytes: writer.bytes,
        })
    }
    pub(super) fn write(
        &mut self,
        name: &str,
        write: impl FnOnce(&mut dyn Write) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        let mut writer = self.open(name)?;
        write(&mut writer)?;
        let value = Self::close(name, writer)?;
        self.files.insert(name.into(), value);
        Ok(())
    }
    pub(super) fn write_pair(
        &mut self,
        left: &str,
        right: &str,
        write: impl FnOnce(&mut dyn Write, &mut dyn Write) -> Result<(), Diagnostic>,
    ) -> Result<(), Diagnostic> {
        if left == right {
            return Err(Diagnostic::output("duplicate output pair"));
        }
        let mut a = self.open(left)?;
        let mut b = self.open(right)?;
        write(&mut a, &mut b)?;
        let a = Self::close(left, a)?;
        let b = Self::close(right, b)?;
        self.files.insert(left.into(), a);
        self.files.insert(right.into(), b);
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
    #[cfg(test)]
    crate::acceptance_hooks::hit_path("output.export_entry", Some(output));
    let directory = output.join(format!(".tmp-{run_id}"));
    fs::create_dir(&directory)
        .map_err(|e| Diagnostic::output(format!("Cannot create output staging directory: {e}")))?;
    let mut staging = Staging {
        directory,
        files: BTreeMap::new(),
        owned: BTreeMap::new(),
        directory_removed: false,
    };
    write(&mut staging)?;
    // Journals remain readable through their descriptors for snapshot reuse.
    // Only their exact owned directory entries are detached before commit.
    if let Some(archive) = &snapshot.can.archive {
        archive.detach()?;
    }
    if let Some(spool) = &snapshot.common.point_spool {
        spool.detach()?;
    }
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
    let manifest = json!({"schema_version":version,"run_id":run_id,"status":"complete","termination":snapshot.common.termination,"partial":snapshot.common.partial,"metadata_ref":if version == 4 {"results.json#/result/metadata"} else {"results.json#/metadata"},"files":files});
    staging.publish(output, run_id, &manifest)
}

impl Staging {
    fn publish(
        &mut self,
        output: &Path,
        run_id: &str,
        manifest: &serde_json::Value,
    ) -> Result<(), Diagnostic> {
        let staged_manifest = self.directory.join("manifest.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_manifest)
            .map_err(|e| Diagnostic::output(format!("Cannot stage manifest: {e}")))?;
        let metadata = file
            .metadata()
            .map_err(|e| Diagnostic::output(format!("Cannot inspect manifest: {e}")))?;
        self.owned
            .insert("manifest.json".into(), (metadata.dev(), metadata.ino()));
        file.write_all(format!("{}\n", canonical(manifest)).as_bytes())
            .and_then(|()| file.flush())
            .map_err(|e| Diagnostic::output(format!("Cannot write manifest: {e}")))?;
        drop(file);
        for name in self.files.keys() {
            fs::hard_link(self.directory.join(name), output.join(name))
                .map_err(|e| Diagnostic::output(format!("Cannot publish {name}: {e}")))?;
            fs::remove_file(self.directory.join(name))
                .map_err(|e| Diagnostic::output(format!("Cannot unstage {name}: {e}")))?;
            self.owned.remove(name);
        }
        // Move the sole manifest link out before removing the empty directory.
        // Both renames must refuse every existing destination (including symlinks).
        // Failure before the final rename leaves an incomplete output for diagnosis;
        // no cleanup or unlink is needed after the manifest becomes observable.
        let pending = output.join(format!(".tmp-{run_id}-manifest"));
        rename_noreplace(&staged_manifest, &pending)
            .map_err(|e| Diagnostic::output(format!("Cannot unstage manifest: {e}")))?;
        self.owned.remove("manifest.json");
        fs::remove_dir(&self.directory).map_err(|e| {
            Diagnostic::output(format!("Cannot remove output staging directory: {e}"))
        })?;
        self.directory_removed = true;
        // Refuse publication if any unknown or retained transient entry remains.
        // Do not infer ownership from a scratch-looking filename or delete it.
        for entry in fs::read_dir(output)
            .map_err(|e| Diagnostic::output(format!("Cannot inspect output directory: {e}")))?
        {
            let entry = entry.map_err(|e| Diagnostic::output(e.to_string()))?;
            let name = entry.file_name();
            let expected = name.to_str().is_some_and(|n| self.files.contains_key(n))
                || entry.path() == pending;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|e| Diagnostic::output(e.to_string()))?;
            if !expected || !metadata.is_file() || metadata.nlink() != 1 {
                return Err(Diagnostic::output(
                    "unexpected output entry before manifest commit",
                ));
            }
        }
        #[cfg(test)]
        crate::acceptance_hooks::hit("output.before_manifest");
        rename_noreplace(&pending, &output.join("manifest.json"))
            .map_err(|e| Diagnostic::output(format!("Cannot publish manifest.json: {e}")))?;
        #[cfg(test)]
        crate::acceptance_hooks::hit("output.after_manifest");
        Ok(())
    }
}

fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(Into::into)
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
    let manifest = json!({"schema_version":version,"run_id":run_id,"status":"complete","termination":termination,"partial":partial,"metadata_ref":if version == 4 {"results.json#/result/metadata"} else {"results.json#/metadata"},"files":manifest_files});
    let temporary = output.join(format!(".tmp-{run_id}"));
    fs::create_dir(&temporary)
        .map_err(|e| Diagnostic::output(format!("Cannot create output staging directory: {e}")))?;
    let mut staging = Staging {
        directory: temporary,
        files: BTreeMap::new(),
        owned: BTreeMap::new(),
        directory_removed: false,
    };
    for (name, bytes) in files {
        staging.write(name, |writer| {
            writer
                .write_all(&bytes)
                .map_err(|e| Diagnostic::output(format!("Cannot write {name}: {e}")))
        })?;
    }
    staging.publish(output, run_id, &manifest)
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/can/baseline.ini")
    }

    #[test]
    fn publication_child() {
        let Some(root) = std::env::var_os("DIR_PUBLICATION_TEST_ROOT") else {
            return;
        };
        let prepared = crate::prepare(&fixture()).unwrap();
        crate::run(prepared, Path::new(&root)).unwrap();
    }

    #[test]
    fn killed_publication_has_strict_bundle_exactly_after_manifest() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        for site in ["output.before_manifest", "output.after_manifest"] {
            let root = std::env::temp_dir().join(format!(
                "dir-publish-kill-{}-{}",
                std::process::id(),
                site
            ));
            fs::create_dir(&root).unwrap();
            let output = root.join("result");
            let socket = root.join("barrier.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "output::publish::acceptance_tests::publication_child",
                    "--nocapture",
                ])
                .env("DIR_PUBLICATION_TEST_ROOT", &output)
                .env("DIR_ACCEPTANCE_SITE", site)
                .env("DIR_ACCEPTANCE_OCCURRENCE", "0")
                .env("DIR_ACCEPTANCE_SOCKET", &socket)
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
                            let _ = child.kill();
                            let _ = child.wait();
                            panic!("publication child did not reach {site}");
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("accept publication barrier: {e}");
                    }
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut message = String::new();
            let mut reader = BufReader::new(stream);
            reader.read_line(&mut message).unwrap();
            child.kill().unwrap();
            child.wait().unwrap();
            drop(reader);
            let barrier: serde_json::Value = serde_json::from_str(&message).unwrap();
            assert_eq!(barrier["site"], site);
            if site == "output.before_manifest" {
                assert!(!output.join("manifest.json").exists());
            } else {
                use crate::output::{
                    ResultBundleExpectation, result_input_evidence, validate_result_bundle,
                };
                let manifest: serde_json::Value =
                    serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap())
                        .unwrap();
                let prepared = crate::prepare(&fixture()).unwrap();
                let expected = ResultBundleExpectation {
                    run_id: manifest["run_id"].as_str().unwrap().into(),
                    started_at_utc: None,
                    frozen_inputs: result_input_evidence(&prepared),
                    acquired_inputs: None,
                    report: None,
                    manifest_sha256: None,
                };
                validate_result_bundle(&output, &expected).unwrap();
                assert_eq!(fs::read_dir(&output).unwrap().count(), 5);
                for entry in fs::read_dir(&output).unwrap() {
                    assert_eq!(entry.unwrap().metadata().unwrap().nlink(), 1);
                }
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn snapshot_journals_remain_reusable_after_strict_publication() {
        use crate::output::{
            ResultBundleExpectation, result_input_evidence, validate_result_bundle,
        };
        let root =
            std::env::temp_dir().join(format!("dir-detached-snapshot-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let prepared = crate::prepare(&fixture()).unwrap();
        let snapshot =
            crate::runtime::spool::with_spooling(&first, || crate::runtime::simulate(&prepared))
                .unwrap();
        let archive = snapshot.can.archive.as_ref().unwrap();
        assert!(snapshot.common.point_spool.is_some());
        let count = archive.len();
        assert!(count > 0);
        for output in [&first, &second] {
            crate::output::export(&prepared, &snapshot, output).unwrap();
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
            validate_result_bundle(
                output,
                &ResultBundleExpectation {
                    run_id: manifest["run_id"].as_str().unwrap().into(),
                    started_at_utc: None,
                    frozen_inputs: result_input_evidence(&prepared),
                    acquired_inputs: None,
                    report: None,
                    manifest_sha256: None,
                },
            )
            .unwrap();
            assert_eq!(fs::read_dir(output).unwrap().count(), 5);
            let mut a = archive.iter().unwrap();
            let mut b = archive.iter().unwrap();
            for _ in 0..count {
                a.next().unwrap().unwrap();
                b.next().unwrap().unwrap();
            }
            assert!(a.next().is_none());
            assert!(b.next().is_none());
        }
        drop(snapshot);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preparation_publication_leaves_only_single_link_outputs() {
        let out =
            std::env::temp_dir().join(format!("dir-preparation-publish-{}", std::process::id()));
        fs::create_dir(&out).unwrap();
        let files = [
            "results.json",
            "events.csv",
            "summary.csv",
            "diagnostics.jsonl",
        ]
        .into_iter()
        .map(|name| (name, b"data".to_vec()))
        .collect();
        publish_files(&out, "complete", 1, "prep_failed", true, files).unwrap();
        assert_eq!(fs::read_dir(&out).unwrap().count(), 5);
        for entry in fs::read_dir(&out).unwrap() {
            assert_eq!(entry.unwrap().metadata().unwrap().nlink(), 1);
        }
        fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn preparation_publication_refuses_all_existing_destination_types() {
        use std::os::unix::fs::symlink;
        for name in ["events.csv", ".tmp-collision-manifest", "manifest.json"] {
            for kind in ["file", "directory", "symlink"] {
                let out = std::env::temp_dir().join(format!(
                    "dir-publish-collision-{}-{name}-{kind}",
                    std::process::id()
                ));
                fs::create_dir(&out).unwrap();
                let path = out.join(name);
                match kind {
                    "file" => fs::write(&path, b"foreign sentinel").unwrap(),
                    "directory" => fs::create_dir(&path).unwrap(),
                    _ => symlink("missing-foreign-target", &path).unwrap(),
                }
                let before = fs::symlink_metadata(&path).unwrap();
                let files = [
                    "results.json",
                    "events.csv",
                    "summary.csv",
                    "diagnostics.jsonl",
                ]
                .into_iter()
                .map(|n| (n, b"data".to_vec()))
                .collect();
                assert!(publish_files(&out, "collision", 1, "prep_failed", true, files).is_err());
                let after = fs::symlink_metadata(&path).unwrap();
                assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
                match kind {
                    "file" => assert_eq!(fs::read(&path).unwrap(), b"foreign sentinel"),
                    "directory" => assert!(after.is_dir()),
                    _ => assert_eq!(
                        fs::read_link(&path).unwrap(),
                        Path::new("missing-foreign-target")
                    ),
                }
                if name != "manifest.json" {
                    assert!(!out.join("manifest.json").exists());
                }
                fs::remove_dir_all(out).unwrap();
            }
        }
    }

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
            "foreign-stage-entry",
            "foreign-output-entry",
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
                if site == "foreign-output-entry" {
                    fs::write(out.join(".dir-result-sort-foreign"), b"preserve me").unwrap();
                }
                if site == "foreign-stage-entry" {
                    fs::write(staging.directory.join("foreign"), b"preserve me").unwrap();
                }
                if site == "manifest-stage" {
                    fs::create_dir(staging.directory.join("manifest.json")).unwrap();
                }
                Ok(())
            })
            .unwrap_err();
            assert_eq!(failed.code, "E-0003", "{site}");
            assert!(!out.join("manifest.json").is_file(), "{site}");
            if matches!(
                site,
                "data-create" | "manifest-stage" | "foreign-stage-entry"
            ) {
                // A failed create never grants ownership of the foreign directory.
                assert!(out.join(".tmp-acceptance").is_dir(), "{site}");
            } else {
                assert!(!out.join(".tmp-acceptance").exists(), "{site}");
            }
            if site == "foreign-stage-entry" {
                assert_eq!(
                    fs::read(out.join(".tmp-acceptance/foreign")).unwrap(),
                    b"preserve me"
                );
            }
            if site == "foreign-output-entry" {
                assert_eq!(
                    fs::read(out.join(".dir-result-sort-foreign")).unwrap(),
                    b"preserve me"
                );
            }
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
    #[test]
    fn paired_writer_failure_never_publishes_either_output() {
        let prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/can/baseline.ini"),
        )
        .unwrap();
        let snapshot = Snapshot::empty(&prepared);
        for side in 0..3 {
            let out = std::env::temp_dir()
                .join(format!("dir-paired-failure-{}-{side}", std::process::id()));
            fs::create_dir(&out).unwrap();
            let error = stream(&out, "pair-failure", 1, &snapshot, |stage| {
                stage.write_pair("results.json", "events.csv", |left, right| {
                    if side == 0 {
                        return Err(Diagnostic::output("injected before left"));
                    }
                    left.write_all(b"left complete\n").unwrap();
                    if side == 1 {
                        return Err(Diagnostic::output("injected before right"));
                    }
                    right.write_all(b"right complete\n").unwrap();
                    Err(Diagnostic::output("injected after both writes"))
                })
            })
            .unwrap_err();
            assert_eq!(error.code, "E-0003");
            assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
            fs::remove_dir(out).unwrap();
        }
    }
}
