//! Authenticate manifest-last result bundles without reopening artifact paths.
use crate::types::{Diagnostic, PreparedSimulation};
use rustix::fs::{self as rfs, AtFlags, FileType, Mode, OFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const REQUIRED: [&str; 4] = [
    "diagnostics.jsonl",
    "events.csv",
    "results.json",
    "summary.csv",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ResultInputEvidence {
    pub path: PathBuf,
    pub sha256: String,
    pub logical_path: Option<String>,
}

pub(crate) fn result_input_evidence(prepared: &PreparedSimulation) -> Vec<ResultInputEvidence> {
    prepared
        .common
        .inputs
        .iter()
        .map(|input| ResultInputEvidence {
            path: input.path.clone(),
            sha256: super::publish::digest(input.content.as_bytes()),
            logical_path: Some(
                prepared
                    .common
                    .provenance
                    .logical_path(prepared, &input.path),
            ),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResultReportEvidence {
    /// RunReport envelope version, independent of result schema_version.
    pub schema_version: u32,
    pub termination: String,
    pub partial: bool,
    pub exit_code: u8,
    pub committed_events: String,
    pub finish_ps: String,
    pub last_event_time_ps: Option<String>,
}
impl From<&crate::RunReport> for ResultReportEvidence {
    fn from(report: &crate::RunReport) -> Self {
        Self {
            schema_version: report.schema_version,
            termination: report.termination.clone(),
            partial: report.partial,
            exit_code: report.exit_code,
            committed_events: report.committed_events.clone(),
            finish_ps: report.finish_ps.clone(),
            last_event_time_ps: report.last_event_time_ps.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResultBundleExpectation {
    pub run_id: String,
    pub started_at_utc: Option<String>,
    pub frozen_inputs: Vec<ResultInputEvidence>,
    pub acquired_inputs: Option<Vec<ResultInputEvidence>>,
    pub report: Option<ResultReportEvidence>,
    pub manifest_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtifactIdentity {
    pub dev: u64,
    pub ino: u64,
}
impl ArtifactIdentity {
    fn of(stat: &rfs::Stat) -> Self {
        Self {
            dev: stat.st_dev,
            ino: stat.st_ino,
        }
    }
}

/// The authenticated descriptor, positioned at zero. Consumers must use this
/// handle, rather than reopening the name in the mutable filesystem namespace.
#[derive(Debug)]
pub(crate) struct ValidatedArtifact {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub identity: ArtifactIdentity,
    pub file: File,
    observed: FileObservation,
}
#[derive(Debug, PartialEq, Eq)]
struct FileObservation {
    mode: u32,
    uid: u32,
    links: u64,
    modified: (u64, u64),
    changed: (u64, u64),
}
impl FileObservation {
    fn of(stat: &rfs::Stat) -> Self {
        Self {
            mode: stat.st_mode,
            uid: stat.st_uid,
            links: stat.st_nlink,
            modified: (stat.st_mtime, stat.st_mtime_nsec),
            changed: (stat.st_ctime, stat.st_ctime_nsec),
        }
    }
}
#[derive(Debug)]
pub(crate) struct ValidatedResultBundle {
    pub schema_version: u32,
    pub run_id: String,
    pub termination: String,
    pub partial: bool,
    pub exit_code: u8,
    pub committed_events: String,
    pub finish_ps: String,
    pub last_event_time_ps: Option<String>,
    pub manifest_sha256: String,
    pub inputs: Vec<ResultInputEvidence>,
    pub artifacts: BTreeMap<String, ValidatedArtifact>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    run_id: String,
    status: String,
    termination: String,
    partial: bool,
    metadata_ref: String,
    files: Vec<ManifestFile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    name: String,
    sha256: String,
    bytes: String,
}
// Serde skips the large records/model_records arrays without retaining them.
#[derive(Deserialize)]
struct ResultHeader {
    schema_version: u32,
    run_id: String,
    metadata: Metadata,
    simulation: Simulation,
}
#[derive(Deserialize)]
struct Metadata {
    started_at_utc: String,
    sources: Vec<Source>,
    input_sha256: String,
}
#[derive(Deserialize)]
struct Source {
    canonical_path: PathBuf,
    logical_path: String,
    sha256: String,
    content_utf8: String,
}
#[derive(Deserialize)]
struct Simulation {
    termination: String,
    partial: bool,
    committed_events: String,
    end_ps: String,
    #[serde(deserialize_with = "required_nullable")]
    last_event_time_ps: Option<String>,
}
fn required_nullable<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::output(format!("Invalid result bundle: {}", message.into()))
}
fn io(error: impl std::fmt::Display) -> Diagnostic {
    invalid(error.to_string())
}
fn decimal(text: &str) -> Result<u64, Diagnostic> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return Err(invalid("expected canonical unsigned decimal string"));
    }
    text.parse().map_err(io)
}
fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn stat_same(a: &rfs::Stat, b: &rfs::Stat) -> bool {
    ArtifactIdentity::of(a) == ArtifactIdentity::of(b)
        && a.st_size == b.st_size
        && a.st_mode == b.st_mode
        && a.st_uid == b.st_uid
        && a.st_nlink == b.st_nlink
        && a.st_mtime == b.st_mtime
        && a.st_mtime_nsec == b.st_mtime_nsec
        && a.st_ctime == b.st_ctime
        && a.st_ctime_nsec == b.st_ctime_nsec
}
struct Directory {
    chain: Vec<(File, PathBuf, ArtifactIdentity)>,
    uid: u32,
}
impl Directory {
    fn open(path: &Path) -> Result<Self, Diagnostic> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_err(io)?.join(path)
        };
        if absolute.to_str().is_none()
            || absolute.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::CurDir | Component::Prefix(_)
                )
            })
        {
            return Err(invalid(
                "result directory must have a normalized UTF-8 path",
            ));
        }
        let uid = std::fs::metadata("/proc/self").map_err(io)?.uid();
        let root: File = rfs::open(
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?
        .into();
        let identity = ArtifactIdentity::of(&rfs::fstat(&root).map_err(io)?);
        let mut directory = Self {
            chain: vec![(root, PathBuf::from("/"), identity)],
            uid,
        };
        for part in absolute.components() {
            if let Component::Normal(name) = part {
                let file: File = rfs::openat(
                    directory.fd(),
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(io)?
                .into();
                let stat = rfs::fstat(&file).map_err(io)?;
                let path = directory.chain.last().unwrap().1.join(name);
                directory
                    .chain
                    .push((file, path, ArtifactIdentity::of(&stat)));
            }
        }
        let stat = rfs::fstat(directory.fd()).map_err(io)?;
        if stat.st_uid != uid || stat.st_mode & 0o022 != 0 {
            return Err(invalid(
                "result directory ownership or permissions are unsafe",
            ));
        }
        directory.verify()?;
        Ok(directory)
    }
    fn fd(&self) -> &File {
        &self.chain.last().unwrap().0
    }
    fn verify(&self) -> Result<(), Diagnostic> {
        for pair in self.chain.windows(2) {
            let stat = rfs::statat(
                &pair[0].0,
                pair[1].1.file_name().unwrap(),
                AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(io)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
                || ArtifactIdentity::of(&stat) != pair[1].2
            {
                return Err(invalid("result directory identity changed"));
            }
        }
        let stat = rfs::fstat(self.fd()).map_err(io)?;
        if stat.st_uid != self.uid || stat.st_mode & 0o022 != 0 {
            return Err(invalid("result directory ownership or permissions changed"));
        }
        Ok(())
    }
    fn check_names(&self) -> Result<(), Diagnostic> {
        let mut stream = rfs::Dir::read_from(self.fd()).map_err(io)?;
        let mut names = BTreeSet::new();
        while let Some(entry) = stream.read() {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            let name = std::str::from_utf8(name).map_err(io)?;
            if name != "manifest.json" && !REQUIRED.contains(&name) {
                return Err(invalid("unknown result directory entry"));
            }
            names.insert(name.to_owned());
        }
        if names.len() != REQUIRED.len() + 1 {
            return Err(invalid("required result artifact is missing"));
        }
        Ok(())
    }
    fn artifact(
        &self,
        name: &str,
        expected: Option<(&str, u64)>,
    ) -> Result<ValidatedArtifact, Diagnostic> {
        if name != "manifest.json" && !REQUIRED.contains(&name) {
            return Err(invalid("artifact path is outside the common allowlist"));
        }
        let mut file: File = rfs::openat(
            self.fd(),
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io)?
        .into();
        let stat = rfs::fstat(&file).map_err(io)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_uid != self.uid
            || stat.st_mode & 0o022 != 0
        {
            return Err(invalid(
                "artifact is not an owned, singly linked regular file",
            ));
        }
        let bytes = u64::try_from(stat.st_size).map_err(io)?;
        if name == "manifest.json" && bytes > 1024 * 1024 {
            return Err(invalid("manifest exceeds size limit"));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut count = 0u64;
        loop {
            let read = file.read(&mut buffer).map_err(io)?;
            if read == 0 {
                break;
            }
            count = count
                .checked_add(read as u64)
                .ok_or_else(|| invalid("artifact size overflow"))?;
            if count > bytes {
                return Err(invalid("artifact changed while hashing"));
            }
            hash.update(&buffer[..read]);
        }
        let sha256 = format!("{:x}", hash.finalize());
        if count != bytes || expected.is_some_and(|(hash, size)| hash != sha256 || size != bytes) {
            return Err(invalid("artifact size or SHA-256 mismatch"));
        }
        let after = rfs::fstat(&file).map_err(io)?;
        let current = rfs::statat(self.fd(), name, AtFlags::SYMLINK_NOFOLLOW).map_err(io)?;
        if !stat_same(&stat, &after) || !stat_same(&after, &current) {
            return Err(invalid("artifact identity changed while observing it"));
        }
        file.seek(SeekFrom::Start(0)).map_err(io)?;
        Ok(ValidatedArtifact {
            name: name.into(),
            bytes,
            sha256,
            identity: ArtifactIdentity::of(&stat),
            file,
            observed: FileObservation::of(&stat),
        })
    }
}

/// Validate the existing common schema, frozen source evidence, and optional
/// worker report. A valid manifest alone can recover only persisted measurements.
pub(crate) fn validate_result_bundle(
    directory: &Path,
    expected: &ResultBundleExpectation,
) -> Result<ValidatedResultBundle, Diagnostic> {
    let directory = Directory::open(directory)?;
    directory.check_names()?;
    let mut manifest_artifact = directory.artifact("manifest.json", None)?;
    if expected
        .manifest_sha256
        .as_ref()
        .is_some_and(|hash| hash != &manifest_artifact.sha256)
    {
        return Err(invalid("manifest SHA-256 differs from durable provenance"));
    }
    let manifest: Manifest =
        serde_json::from_reader(BufReader::new(&mut manifest_artifact.file)).map_err(io)?;
    if ![1, 2, 3].contains(&manifest.schema_version)
        || manifest.run_id != expected.run_id
        || manifest.status != "complete"
        || manifest.metadata_ref != "results.json#/metadata"
        || manifest.termination.is_empty()
    {
        return Err(invalid(
            "manifest schema, identity, status or metadata reference mismatch",
        ));
    }
    let mut artifacts = BTreeMap::new();
    for item in manifest.files {
        if !REQUIRED.contains(&item.name.as_str())
            || artifacts.contains_key(&item.name)
            || !hash_valid(&item.sha256)
        {
            return Err(invalid("unknown, duplicate or malformed manifest artifact"));
        }
        let bytes = decimal(&item.bytes)?;
        let artifact = directory.artifact(&item.name, Some((&item.sha256, bytes)))?;
        artifacts.insert(item.name, artifact);
    }
    if artifacts.len() != REQUIRED.len() {
        return Err(invalid("manifest omits a required artifact"));
    }
    let results = artifacts.get_mut("results.json").unwrap();
    let header: ResultHeader =
        serde_json::from_reader(BufReader::new(&mut results.file)).map_err(io)?;
    if header.schema_version != manifest.schema_version
        || header.run_id != expected.run_id
        || header.simulation.termination != manifest.termination
        || header.simulation.partial != manifest.partial
    {
        return Err(invalid(
            "results and manifest identity or termination differ",
        ));
    }
    if expected
        .started_at_utc
        .as_ref()
        .is_some_and(|time| time != &header.metadata.started_at_utc)
    {
        return Err(invalid("result start identity differs"));
    }
    decimal(&header.simulation.committed_events)?;
    decimal(&header.simulation.end_ps)?;
    if let Some(time) = &header.simulation.last_event_time_ps {
        decimal(time)?;
    }
    let exit_code = if manifest.termination == "prep_failed" {
        2
    } else if manifest.partial {
        3
    } else {
        0
    };
    if let Some(report) = &expected.report {
        if report.schema_version != 1
            || report.termination != manifest.termination
            || report.partial != manifest.partial
            || report.exit_code != exit_code
            || report.committed_events != header.simulation.committed_events
            || report.finish_ps != header.simulation.end_ps
            || report.last_event_time_ps != header.simulation.last_event_time_ps
        {
            return Err(invalid("worker report disagrees with persisted result"));
        }
    }
    let frozen = evidence_map(&expected.frozen_inputs)?;
    let acquired = expected
        .acquired_inputs
        .as_deref()
        .map(evidence_map)
        .transpose()?;
    if let Some(acquired) = &acquired {
        for (path, input) in acquired {
            if frozen.get(path).is_none_or(|source| {
                source.sha256 != input.sha256
                    || source.logical_path.as_ref().is_some_and(|logical| {
                        input
                            .logical_path
                            .as_ref()
                            .is_some_and(|other| other != logical)
                    })
            }) {
                return Err(invalid(
                    "worker acquired input is absent from frozen bundle",
                ));
            }
        }
    }
    let mut inputs = Vec::new();
    let mut paths = BTreeSet::new();
    let mut previous: Option<String> = None;
    let mut hashes = Vec::new();
    for source in header.metadata.sources {
        if !paths.insert(source.canonical_path.clone())
            || previous.as_ref().is_some_and(|p| p >= &source.logical_path)
        {
            return Err(invalid("metadata sources are duplicate or unsorted"));
        }
        previous = Some(source.logical_path.clone());
        let evidence = acquired
            .as_ref()
            .unwrap_or(&frozen)
            .get(&source.canonical_path)
            .ok_or_else(|| invalid("result source is absent from expected inputs"))?;
        let frozen_source = frozen
            .get(&source.canonical_path)
            .ok_or_else(|| invalid("result source is absent from frozen bundle"))?;
        let logical = evidence
            .logical_path
            .as_ref()
            .or(frozen_source.logical_path.as_ref())
            .ok_or_else(|| invalid("source logical path lacks authenticated normalization"))?;
        if evidence.sha256 != source.sha256
            || frozen_source.sha256 != source.sha256
            || logical != &source.logical_path
            || frozen_source
                .logical_path
                .as_ref()
                .is_some_and(|expected| expected != &source.logical_path)
            || super::publish::digest(source.content_utf8.as_bytes()) != source.sha256
        {
            return Err(invalid(
                "source content, hash or normalized logical path differs",
            ));
        }
        hashes.push(serde_json::json!({"logical_path":source.logical_path,"sha256":source.sha256}));
        inputs.push(ResultInputEvidence {
            path: source.canonical_path,
            sha256: source.sha256,
            logical_path: Some(source.logical_path),
        });
    }
    if acquired.as_ref().is_some_and(|a| a.len() != inputs.len()) || inputs.is_empty() {
        return Err(invalid("result metadata omits acquired inputs"));
    }
    if super::publish::digest(super::json::canonical(&serde_json::json!(hashes)).as_bytes())
        != header.metadata.input_sha256
    {
        return Err(invalid("metadata aggregate input hash differs"));
    }
    // Rehash parsed files and recheck every entry after all observations. Reads
    // used the same descriptors; replacement paths cannot supply parsed content.
    for artifact in artifacts
        .values_mut()
        .chain(std::iter::once(&mut manifest_artifact))
    {
        let checked =
            directory.artifact(&artifact.name, Some((&artifact.sha256, artifact.bytes)))?;
        if checked.identity != artifact.identity || checked.observed != artifact.observed {
            return Err(invalid("artifact was replaced during bundle validation"));
        }
        artifact.file.seek(SeekFrom::Start(0)).map_err(io)?;
    }
    directory.check_names()?;
    directory.verify()?;
    let manifest_sha256 = manifest_artifact.sha256.clone();
    artifacts.insert("manifest.json".into(), manifest_artifact);
    Ok(ValidatedResultBundle {
        schema_version: manifest.schema_version,
        run_id: manifest.run_id,
        termination: manifest.termination,
        partial: manifest.partial,
        exit_code,
        committed_events: header.simulation.committed_events,
        finish_ps: header.simulation.end_ps,
        last_event_time_ps: header.simulation.last_event_time_ps,
        manifest_sha256,
        inputs: expected.acquired_inputs.clone().unwrap_or(inputs),
        artifacts,
    })
}
fn evidence_map(
    inputs: &[ResultInputEvidence],
) -> Result<BTreeMap<PathBuf, &ResultInputEvidence>, Diagnostic> {
    let mut result = BTreeMap::new();
    for input in inputs {
        if !hash_valid(&input.sha256)
            || !input.path.is_absolute()
            || input.path.to_str().is_none()
            || input.path.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::CurDir | Component::Prefix(_)
                )
            })
            || result.insert(input.path.clone(), input).is_some()
        {
            return Err(invalid("malformed or duplicate source evidence"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Bundle {
        root: PathBuf,
        expected: ResultBundleExpectation,
    }
    impl Drop for Bundle {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn bundle(partial: bool) -> Bundle {
        bundle_with_fixture(partial, "docs/verification/fixtures/can/delay-filter.ini")
    }
    fn bundle_with_fixture(partial: bool, fixture: &str) -> Bundle {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dir-result-validator-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let mut prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(fixture),
        )
        .unwrap();
        let identity = super::super::new_run_identity().unwrap();
        prepared.common.run_identity = Some(identity.clone());
        if partial {
            prepared.common.max_events = 1;
        }
        let inputs = result_input_evidence(&prepared);
        let report = crate::run(prepared, &root.join("result")).unwrap();
        Bundle {
            root,
            expected: ResultBundleExpectation {
                run_id: identity.run_id,
                started_at_utc: Some(identity.started_at_utc),
                frozen_inputs: inputs.clone(),
                acquired_inputs: Some(inputs),
                report: Some((&report).into()),
                manifest_sha256: None,
            },
        }
    }
    fn validate(bundle: &Bundle) -> Result<ValidatedResultBundle, Diagnostic> {
        validate_result_bundle(&bundle.root.join("result"), &bundle.expected)
    }
    fn mutate_manifest(bundle: &Bundle, mutate: impl FnOnce(&mut Value)) {
        let path = bundle.root.join("result/manifest.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutate(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn mutate_results(bundle: &Bundle, mutate: impl FnOnce(&mut Value)) {
        let path = bundle.root.join("result/results.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        mutate(&mut value);
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(path, &bytes).unwrap();
        mutate_manifest(bundle, |manifest| {
            let file = manifest["files"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|file| file["name"] == "results.json")
                .unwrap();
            file["bytes"] = json!(bytes.len().to_string());
            file["sha256"] = json!(super::super::publish::digest(&bytes));
        });
    }

    #[test]
    fn exported_complete_and_partial_bundles_validate_with_report_and_manifest_only() {
        for partial in [false, true] {
            let mut bundle = bundle(partial);
            let verified = validate(&bundle).unwrap();
            assert_eq!(verified.partial, partial);
            assert_eq!(verified.exit_code, if partial { 3 } else { 0 });
            assert_eq!(verified.artifacts.len(), 5);
            assert_eq!(
                verified.inputs,
                bundle.expected.acquired_inputs.as_ref().unwrap().clone()
            );
            bundle.expected.report = None;
            bundle.expected.acquired_inputs = None;
            bundle.expected.manifest_sha256 = Some(verified.manifest_sha256);
            assert_eq!(
                validate(&bundle).unwrap().committed_events,
                verified.committed_events
            );
        }
    }

    #[test]
    fn unused_frozen_blobs_are_not_fabricated_as_acquired_inputs() {
        let mut bundle = bundle(false);
        bundle.expected.frozen_inputs.push(ResultInputEvidence {
            path: bundle.root.join("unused.json"),
            sha256: super::super::publish::digest(b"auxiliary input"),
            logical_path: None,
        });
        let verified = validate(&bundle).unwrap();
        assert_eq!(
            verified.inputs,
            bundle.expected.acquired_inputs.as_ref().unwrap().clone()
        );
        bundle.expected.acquired_inputs.as_mut().unwrap()[0].logical_path =
            Some("spoofed-logical-path".into());
        assert!(validate(&bundle).is_err());
        bundle.expected.acquired_inputs = None;
        bundle.expected.report = None;
        assert!(validate(&bundle).is_ok());
        for evidence in &mut bundle.expected.frozen_inputs {
            evidence.logical_path = None;
        }
        assert!(validate(&bundle).is_err());
    }

    #[test]
    fn exported_schema_two_gateway_bundle_uses_the_same_validator() {
        let bundle = bundle_with_fixture(false, "examples/gateway/fanout.ini");
        let verified = validate(&bundle).unwrap();
        assert_eq!(verified.schema_version, 2);
        assert_eq!(verified.run_id, bundle.expected.run_id);
        assert_eq!(
            verified.termination,
            bundle.expected.report.as_ref().unwrap().termination
        );
        assert_eq!(
            verified.finish_ps,
            bundle.expected.report.as_ref().unwrap().finish_ps
        );
        assert_eq!(
            verified.last_event_time_ps,
            bundle.expected.report.as_ref().unwrap().last_event_time_ps
        );
    }

    #[test]
    fn rejects_artifact_tamper_sizes_hashes_and_missing_artifacts() {
        let bundle = bundle(false);
        fs::write(
            bundle.root.join("result/events.csv"),
            "secret-shaped altered content",
        )
        .unwrap();
        assert!(validate(&bundle).is_err());
        for field in ["bytes", "sha256"] {
            let bundle = self::bundle(false);
            mutate_manifest(&bundle, |m| {
                m["files"][0][field] = json!(if field == "bytes" { "00" } else { "FF" })
            });
            assert!(validate(&bundle).is_err());
        }
        let bundle = self::bundle(false);
        fs::remove_file(bundle.root.join("result/summary.csv")).unwrap();
        assert!(validate(&bundle).is_err());
    }

    #[test]
    fn rejects_internally_consistent_other_run_identity_and_report_mismatch() {
        let mut bundle = bundle(false);
        bundle.expected.run_id = "different-run".into();
        assert!(validate(&bundle).is_err());
        let mut bundle = self::bundle(false);
        bundle.expected.report.as_mut().unwrap().termination = "unknown_termination".into();
        assert!(validate(&bundle).is_err());
        let mut bundle = self::bundle(false);
        bundle.expected.started_at_utc = Some("other start".into());
        assert!(validate(&bundle).is_err());
        let mut bundle = self::bundle(false);
        bundle.expected.manifest_sha256 = Some("0".repeat(64));
        assert!(validate(&bundle).is_err());
    }

    #[test]
    fn rejects_result_identity_termination_and_frozen_provenance_tampering() {
        for field in [
            "identity",
            "termination",
            "partial",
            "source",
            "logical",
            "input_digest",
            "content",
            "source_order",
        ] {
            let bundle = bundle(false);
            mutate_results(&bundle, |r| match field {
                "identity" => r["run_id"] = json!("other"),
                "termination" => r["simulation"]["termination"] = json!("other"),
                "partial" => r["simulation"]["partial"] = json!(true),
                "source" => r["metadata"]["sources"][0]["canonical_path"] = json!("/outside"),
                "logical" => r["metadata"]["sources"][0]["logical_path"] = json!("outside"),
                "input_digest" => r["metadata"]["input_sha256"] = json!("0".repeat(64)),
                "content" => r["metadata"]["sources"][0]["content_utf8"] = json!("altered"),
                "source_order" => r["metadata"]["sources"].as_array_mut().unwrap().reverse(),
                _ => unreachable!(),
            });
            assert!(validate(&bundle).is_err(), "{field}");
        }
        let mut bundle = self::bundle(false);
        bundle.expected.frozen_inputs[0].sha256 = "0".repeat(64);
        assert!(validate(&bundle).is_err());
        let mut bundle = self::bundle(false);
        bundle.expected.acquired_inputs.as_mut().unwrap().pop();
        assert!(validate(&bundle).is_err());
    }

    #[test]
    fn rejects_absolute_traversal_duplicates_unknown_schema_and_unknown_entries() {
        for name in [
            "/etc/passwd",
            "../outside",
            "sub/results.json",
            "./results.json",
            "results.json/",
            "manifest.json",
        ] {
            let bundle = bundle(false);
            mutate_manifest(&bundle, |m| m["files"][0]["name"] = json!(name));
            assert!(validate(&bundle).is_err(), "{name}");
        }
        for change in ["schema", "duplicate", "missing", "unknown"] {
            let bundle = bundle(false);
            mutate_manifest(&bundle, |m| match change {
                "schema" => m["schema_version"] = json!(999),
                "duplicate" => m["files"][1] = m["files"][0].clone(),
                "missing" => {
                    m["files"].as_array_mut().unwrap().pop();
                }
                "unknown" => m["editor_schema"] = json!(1),
                _ => unreachable!(),
            });
            assert!(validate(&bundle).is_err(), "{change}");
        }
        let bundle = bundle(false);
        fs::write(bundle.root.join("result/unknown"), "preserve").unwrap();
        assert!(validate(&bundle).is_err());
        assert_eq!(
            fs::read(bundle.root.join("result/unknown")).unwrap(),
            b"preserve"
        );
        let bundle = self::bundle(false);
        let path = bundle.root.join("result/manifest.json");
        let text = fs::read_to_string(&path).unwrap();
        fs::write(path, text.replacen('{', "{\"run_id\":\"duplicate\",", 1)).unwrap();
        assert!(validate(&bundle).is_err());
    }

    #[test]
    fn rejects_symlinks_external_hardlinks_and_writable_result_directory() {
        let bundle = bundle(false);
        let result = bundle.root.join("result/results.json");
        fs::rename(&result, bundle.root.join("secret")).unwrap();
        symlink(bundle.root.join("secret"), result).unwrap();
        assert!(validate(&bundle).is_err());
        let bundle = self::bundle(false);
        fs::hard_link(
            bundle.root.join("result/results.json"),
            bundle.root.join("outside-link"),
        )
        .unwrap();
        assert!(validate(&bundle).is_err());
        let bundle = self::bundle(false);
        symlink(bundle.root.join("result"), bundle.root.join("alias")).unwrap();
        assert!(validate_result_bundle(&bundle.root.join("alias"), &bundle.expected).is_err());
        let bundle = self::bundle(false);
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            bundle.root.join("result"),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(validate(&bundle).is_err());
    }

    #[test]
    fn authenticated_descriptor_survives_path_replacement_without_following_it() {
        let bundle = bundle(false);
        let mut verified = validate(&bundle).unwrap();
        let artifact = verified.artifacts.get_mut("results.json").unwrap();
        let original = fs::read(bundle.root.join("result/results.json")).unwrap();
        fs::remove_file(bundle.root.join("result/results.json")).unwrap();
        fs::write(bundle.root.join("secret"), "must never be served").unwrap();
        symlink(
            bundle.root.join("secret"),
            bundle.root.join("result/results.json"),
        )
        .unwrap();
        let mut bytes = Vec::new();
        artifact.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, original);
    }
}
