use super::*;
use crate::input::InputSource;
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    root: PathBuf,
    input: PathBuf,
    project: ProjectSnapshot,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(random_id("dir-snapshot-test-").unwrap());
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let source = root.join("source");
        fs::create_dir_all(source.join("models/demo")).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (from, to) in [
            ("examples/can/baseline.ini", "project.ini"),
            ("examples/can/baseline.json", "baseline.json"),
            ("examples/can/models/demo/Main.ned", "models/demo/Main.ned"),
        ] {
            fs::copy(repo.join(from), source.join(to)).unwrap();
        }
        let project =
            super::super::input::load_project(&source.join("project.ini"), &source).unwrap();
        let input = root.join("input");
        fs::create_dir(&input).unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            input,
            project,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn roundtrip_preserves_memory_bytes_header_metadata_origins_and_layout() {
    let mut f = Fixture::new();
    f.project.origin = ProjectOrigin::New {
        template: "can".into(),
        name: "Untitled".into(),
    };
    let file_id = f.project.ned_files().next().unwrap().id.clone();
    let file = f.project.files.get_mut(&file_id).unwrap();
    file.text = Arc::from(format!(
        "\u{feff}// 非ASCII\r\n{}",
        file.text.replace('\n', "\r\n")
    ));
    file.hash = hash(file.text.as_bytes());
    let raw: Arc<str> = Arc::from("\u{feff}{ \"invalid_layout\":true }\r\n");
    let layout = f.project.layouts.get_mut(&file_id).unwrap();
    layout.raw = Some(raw.clone());
    layout.hash = Some(hash(raw.as_bytes()));
    layout.adopted = Some(LayoutDocument::empty("Main.ned".into()));
    layout.warning = Some("Captured invalid original layout".into());
    f.project.header.roots.push(f.root.join("unreadable-root"));
    let unusual = OsString::from_vec(vec![b'x', 0xff]);
    f.project
        .directories
        .entry(f.project.cwd.clone())
        .or_default()
        .push((unusual, InputKind::Other));
    f.project
        .metadata
        .insert(f.root.join("symlink"), InputKind::Symlink);
    let evidence = freeze(&f.project, &f.input).unwrap();
    assert_eq!(size(&f.project).unwrap(), evidence.bytes);
    let restored = restore(&f.input, &evidence.bundle_digest).unwrap();
    assert_eq!(restored.id, f.project.id);
    assert_eq!(restored.input_digest(), f.project.input_digest());
    assert_eq!(restored.header.roots, f.project.header.roots);
    assert_eq!(restored.header.general, f.project.header.general);
    assert_eq!(restored.config, f.project.config);
    assert_eq!(restored.cwd, f.project.cwd);
    assert!(matches!(restored.origin, ProjectOrigin::New { ref name, .. } if name == "Untitled"));
    assert_eq!(restored.directories, f.project.directories);
    assert_eq!(restored.metadata, f.project.metadata);
    for (id, original) in &f.project.files {
        let actual = &restored.files[id];
        assert_eq!(actual.text.as_bytes(), original.text.as_bytes());
        assert_eq!(actual.origin_text, original.origin_text);
        assert_eq!(actual.origin_hash, original.origin_hash);
        assert_eq!(actual.stat, original.stat);
        assert_eq!(actual.roles, original.roles);
    }
    assert_eq!(restored.layouts[&file_id].raw, Some(raw));
    assert_eq!(
        restored.layouts[&file_id].adopted,
        f.project.layouts[&file_id].adopted
    );
    assert_eq!(
        inspect(&f.input, &evidence.bundle_digest).unwrap().bytes,
        evidence.bytes
    );
}

#[test]
fn source_deletion_does_not_change_prepare_and_unknown_paths_never_fall_back() {
    let f = Fixture::new();
    let before = super::super::analysis::prepare(&f.project).unwrap();
    let evidence = freeze(&f.project, &f.input).unwrap();
    fs::remove_dir_all(f.root.join("source")).unwrap();
    let restored = restore(&f.input, &evidence.bundle_digest).unwrap();
    let after = super::super::analysis::prepare(&restored).unwrap();
    assert_eq!(after.common.inputs.len(), before.common.inputs.len());
    for (actual, original) in after.common.inputs.iter().zip(before.common.inputs.iter()) {
        assert_eq!(actual.path, original.path);
        assert_eq!(
            hash(actual.content.as_bytes()),
            hash(original.content.as_bytes())
        );
    }
    let unknown = f.root.join("existing-on-disk.ini");
    fs::write(&unknown, "[General]\n").unwrap();
    let source = restored.source();
    assert!(source.read_utf8(&unknown).is_err());
    assert!(source.metadata(&unknown).is_err());
    assert!(source.read_dir(&f.root).is_err());
}

#[test]
fn syntactically_invalid_edited_source_is_frozen_and_restored_for_prepare() {
    let mut f = Fixture::new();
    let id = f.project.ned_files().next().unwrap().id.clone();
    let file = f.project.files.get_mut(&id).unwrap();
    file.text = Arc::from("\u{feff}this is not NED\r\n");
    file.hash = hash(file.text.as_bytes());
    let evidence = freeze(&f.project, &f.input).unwrap();
    let restored = restore(&f.input, &evidence.bundle_digest).unwrap();
    assert_eq!(restored.files[&id].text, f.project.files[&id].text);
    assert!(!restored.parsed.contains_key(&id));
    assert!(super::super::analysis::prepare(&restored).is_err());
}

#[test]
fn blob_manifest_commit_and_expected_identity_tampering_are_rejected() {
    let f = Fixture::new();
    let evidence = freeze(&f.project, &f.input).unwrap();
    assert!(restore(&f.input, &"0".repeat(64)).is_err());
    let blob = f
        .input
        .join("blobs")
        .join(&evidence.files[0].bundle_blob_id);
    let original_blob = fs::read(&blob).unwrap();
    fs::write(&blob, b"tampered").unwrap();
    assert!(inspect(&f.input, &evidence.bundle_digest).is_err());
    fs::write(&blob, original_blob).unwrap();
    let manifest = f.input.join(MANIFEST);
    let original_manifest = fs::read(&manifest).unwrap();
    let mut changed = original_manifest.clone();
    changed.push(b' ');
    fs::write(&manifest, changed).unwrap();
    assert!(inspect(&f.input, &evidence.bundle_digest).is_err());
    fs::write(&manifest, original_manifest).unwrap();
    fs::remove_file(f.input.join(COMMIT)).unwrap();
    assert!(inspect(&f.input, &evidence.bundle_digest).is_err());
}

#[test]
fn unsafe_blob_names_are_rejected_even_with_recomputed_bundle_digest() {
    let f = Fixture::new();
    freeze(&f.project, &f.input).unwrap();
    let raw = fs::read(f.input.join(MANIFEST)).unwrap();
    let mut manifest: Manifest = decode(&raw).unwrap();
    let mut blobs: BlobBytes = manifest
        .blobs
        .iter()
        .map(|blob| {
            (
                blob.id.clone(),
                fs::read(f.input.join("blobs").join(&blob.id)).unwrap(),
            )
        })
        .collect();
    let old_id = manifest.blobs[0].id.clone();
    manifest.blobs[0].id = "../../private".into();
    let renamed = blobs.remove(&old_id).unwrap();
    blobs.insert(manifest.blobs[0].id.clone(), renamed);
    let raw = encode(&manifest).unwrap();
    let digest = bundle_digest(&raw, &blobs);
    // A recomputed commit cannot authorize a logical name as a physical blob path.
    fs::write(f.input.join(MANIFEST), &raw).unwrap();
    fs::write(
        f.input.join(COMMIT),
        encode(&Commit {
            schema_version: CODEC_VERSION,
            bundle_digest: digest.clone(),
            bytes: (raw.len() + blobs.values().map(Vec::len).sum::<usize>()) as u64,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(restore(&f.input, &digest).is_err());
}

#[test]
fn bounds_fail_before_writing_and_length_boundary_is_inclusive() {
    let mut f = Fixture::new();
    let original = f.project.files.values().next().unwrap().clone();
    for i in 0..MAX_FILES {
        let mut file = original.clone();
        file.id = format!("extra-{i}");
        f.project.files.insert(file.id.clone(), file);
    }
    assert_eq!(freeze(&f.project, &f.input).unwrap_err().status, 413);
    assert_eq!(fs::read_dir(&f.input).unwrap().count(), 0);
    let mut blobs = BTreeMap::new();
    let mut total = MAX_BYTES - 1;
    assert!(add_blob(&mut blobs, &[1], &mut total).is_ok());
    assert_eq!(total, MAX_BYTES);
    assert_eq!(
        add_blob(&mut blobs, &[1], &mut total).unwrap_err().status,
        413
    );
    assert_eq!(blobs.len(), 1);
}

#[test]
fn corrupted_memory_hashes_fail_before_writing() {
    let mut f = Fixture::new();
    f.project.files.values_mut().next().unwrap().hash = "invalid".into();
    assert_eq!(
        freeze(&f.project, &f.input).unwrap_err().code,
        "E-EDITOR-RUN-INTEGRITY"
    );
    assert_eq!(fs::read_dir(&f.input).unwrap().count(), 0);
}

#[test]
fn unavailable_invalid_utf8_layout_retains_warning_and_hash_without_disk_read() {
    let mut f = Fixture::new();
    let id = f.project.ned_files().next().unwrap().id.clone();
    let layout = f.project.layouts.get_mut(&id).unwrap();
    layout.raw = None;
    layout.hash = Some(hash(&[0xff]));
    layout.warning = Some("Invalid layout UTF-8".into());
    let evidence = freeze(&f.project, &f.input).unwrap();
    let restored = restore(&f.input, &evidence.bundle_digest).unwrap();
    assert_eq!(restored.layouts[&id].hash, f.project.layouts[&id].hash);
    assert_eq!(
        restored.layouts[&id].warning,
        f.project.layouts[&id].warning
    );
    assert!(restored.layouts[&id].raw.is_none());
}

#[test]
fn unexpected_entries_are_preserved_and_rejected() {
    let f = Fixture::new();
    let unexpected = f.input.join("owned-but-unknown");
    fs::write(&unexpected, b"keep").unwrap();
    assert!(freeze(&f.project, &f.input).is_err());
    assert_eq!(fs::read(&unexpected).unwrap(), b"keep");
    fs::remove_file(unexpected).unwrap();
    let evidence = freeze(&f.project, &f.input).unwrap();
    fs::write(f.input.join("blobs").join("unknown"), b"keep").unwrap();
    assert!(inspect(&f.input, &evidence.bundle_digest).is_err());
}

#[test]
fn non_utf8_file_paths_and_roles_roundtrip_in_bundle_and_evidence() {
    let mut f = Fixture::new();
    let id = f.project.ned_files().next().unwrap().id.clone();
    let relative = PathBuf::from(OsString::from_vec(vec![b'x', 0xff, b'.', b'n', b'e', b'd']));
    let file = f.project.files.get_mut(&id).unwrap();
    file.path = f.project.header.roots[0].join(&relative);
    file.roles = vec![FileRole::Ned {
        root_index: 0,
        relative,
        package: "demo".into(),
    }];
    let evidence = freeze(&f.project, &f.input).unwrap();
    let restored = restore(&f.input, &evidence.bundle_digest).unwrap();
    assert_eq!(restored.files[&id].path, f.project.files[&id].path);
    assert_eq!(restored.files[&id].roles, f.project.files[&id].roles);
    let decoded: SnapshotEvidence = decode(&encode(&evidence).unwrap()).unwrap();
    let entry = decoded
        .files
        .iter()
        .find(|file| file.file_id == id)
        .unwrap();
    assert_eq!(entry.original_logical_path, f.project.files[&id].path);
    assert_eq!(entry.roles, f.project.files[&id].roles);
}
