//! Direct condition-level evidence for NE-XC01/04/06/07/21.
use super::analysis;
use super::file_creation::plan_create_file;
use super::input::{CapturedFile, FileRole, ProjectSnapshot, load_project};
use super::model::EditorSession;
use super::output::{self, SaveOutcome, TargetRegistry};
use super::refactor::{EditChanges, EditPlan, PLAN_BYTES, PLAN_TTL};
use super::{hash, random_id};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(random_id("ned-file-acceptance-").unwrap());
        fs::create_dir_all(root.join("input/models/demo")).unwrap();
        fs::create_dir_all(root.join("input/vendor/lib")).unwrap();
        fs::create_dir_all(root.join("exports")).unwrap();
        fs::write(
            root.join("input/models/demo/Main.ned"),
            include_str!("../../../../../docs/verification/fixtures/can/models/demo/Main.ned"),
        )
        .unwrap();
        fs::write(
            root.join("input/vendor/lib/Unused.ned"),
            "package lib; module Unused {}\n",
        )
        .unwrap();
        fs::write(
            root.join("input/project.ini"),
            "\u{feff}[General]\r\n# preserved 日本語\r\nnetwork = demo.Main\r\nned-path = \"models\";\"vendor\"\r\nsim-time-limit = 1ms\r\nMain.bus.bitrate = 500kbps\r\n",
        )
        .unwrap();
        Self { root }
    }
    fn load(&self) -> ProjectSnapshot {
        load_project(&self.root.join("input/project.ini"), &self.root).unwrap()
    }
    fn registry(&self) -> TargetRegistry {
        TargetRegistry::open(&self.root.join("exports"), &self.root.join("state")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

// Include directory entries and file bytes as well as independent SHA-256s.
// This detects deletions, additions, and writes anywhere in the measured tree.
fn disk_tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, String)> {
    fn visit(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, (Vec<u8>, String)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.file_type().is_symlink() {
            fs::read_link(path)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        } else if metadata.is_dir() {
            Vec::new()
        } else {
            fs::read(path).unwrap()
        };
        out.insert(
            path.strip_prefix(base).unwrap().into(),
            (bytes.clone(), hash(&bytes)),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(base, &entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn session_state(m: &EditorSession) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{}|{}|{}",
        m.project,
        m.syntax,
        m.parsed_hashes,
        m.diagnostics,
        m.prepared,
        m.analysis,
        m.last_output,
        m.never_exported,
        m.project_instance_id,
        m.revision,
        m.input_revision,
        m.context_epoch,
        m.dirty(),
        m.differs_from_origin(),
        m.can_undo(),
        m.can_redo()
    )
}

fn payload(relative: &str) -> Value {
    json!({"role":"ned","root_id":"ned-root-1","relative_path":relative,
        "template_id":"ned.module_empty","declaration":{"kind":"module","local_name":"New"},
        "format":{"newline":"LF","bom":false}})
}

fn create(m: &mut EditorSession) -> CapturedFile {
    let plan = EditPlan::build(
        m,
        &json!({"operation":"create_file","operation_payload":payload("pkg/New.ned")}),
    )
    .unwrap();
    let file = plan.changes.added[0].clone();
    *m = plan.apply(m).unwrap();
    file
}

fn semantic_snapshot(project: &ProjectSnapshot) -> String {
    let p = analysis::prepare(project).unwrap();
    // Provenance/input lists intentionally change when adding an unused type;
    // every active-network runtime model and its controls must remain equal.
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        json!({"profile":p.common.profile,"network":p.common.network,
            "module_paths":p.common.module_paths,"time_limit_ps":p.common.time_limit_ps,
            "metrics_window_ps":p.common.metrics_window_ps,"max_events":p.common.max_events,
            "max_delta_cycles":p.common.max_delta_cycles,"channel_count":p.common.channel_count}),
        p.can,
        p.gateway,
        p.ethernet,
        p.canfd,
        p.axi,
        p.soc,
        p.memory_ipc,
        p.registered
    )
}

#[test]
fn xc01_second_root_creation_preserves_first_root_disk_and_network_semantics() {
    let f = Fixture::new();
    let disk = disk_tree(&f.root.join("input"));
    let mut m = EditorSession::new(f.load());
    let original = m.project.clone();
    let semantics = semantic_snapshot(&m.project);
    let file = create(&mut m);
    assert_eq!(file.path, m.project.header.roots[1].join("pkg/New.ned"));
    assert_eq!(file.text.as_ref(), "package pkg;\n\nmodule New {}\n");
    assert_eq!(
        analysis::shapes(m.current_parse(&file.id).unwrap())[0].name,
        "pkg.New"
    );
    for original_file in original.files.values() {
        let current = &m.project.files[&original_file.id];
        assert_eq!(current.text, original_file.text);
        assert_eq!(current.hash, original_file.hash);
        assert_eq!(current.path, original_file.path);
    }
    assert_eq!(semantic_snapshot(&m.project), semantics);
    assert!(m.dirty());
    assert_eq!(m.revision, 2);
    assert_eq!(m.input_revision, 2);
    assert!(m.undo().unwrap());
    assert!(
        !m.undo().unwrap(),
        "one creation must retain exactly one history entry"
    );
    assert_eq!(m.project.input_digest(), original.input_digest());
    assert!(!m.dirty());
    assert_eq!(disk_tree(&f.root.join("input")), disk);
}

#[test]
fn xc04_real_captured_casefold_sidecar_paths_roles_and_empty_ned_reject_atomically() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("input/vendor/PKG")).unwrap();
    fs::create_dir_all(f.root.join("input/vendor/sidecar")).unwrap();
    fs::write(
        f.root.join("input/vendor/sidecar/New.ned.layout.json"),
        "reserved",
    )
    .unwrap();
    let mut m = EditorSession::new(f.load());
    let disk = disk_tree(&f.root.join("input"));
    let before = session_state(&m);
    for path in [
        "New.ned",
        "/tmp/New.ned",
        "pkg/../New.ned",
        "pkg/./New.ned",
        "pkg//New.ned",
        "pkg/New.ned",
        "sidecar/New.ned",
        "lib/Unused.ned",
    ] {
        let error = plan_create_file(&m, &payload(path)).err().unwrap();
        assert_eq!(error.status, 422, "{path}: {error:?}");
        assert_eq!(session_state(&m), before, "{path}");
    }
    // Empty sources are rejected at the native transaction boundary, even
    // though registered create templates never generate them.
    let mut valid = payload("other/New.ned");
    valid["root_id"] = json!("ned-root-0");
    let mut empty = plan_create_file(&m, &valid).unwrap().added.remove(0);
    empty.text = "".into();
    empty.origin_text = empty.text.clone();
    empty.hash = hash(b"");
    empty.origin_hash = empty.hash.clone();
    assert_eq!(
        m.commit_with_files(vec![], vec![], vec![empty])
            .unwrap_err()
            .status,
        422
    );
    assert_eq!(session_state(&m), before);
    for role in ["workload", "model_config"] {
        let new = super::template::new_project("multibus", "Demo", &f.root).unwrap();
        // Both singleton roles exist in the normal multibus project.
        assert!(
            new.files
                .values()
                .any(|file| file.roles.contains(&if role == "workload" {
                    FileRole::Workload
                } else {
                    FileRole::ModelConfig
                }))
        );
        let session = EditorSession::new(new.clone());
        let initial = session_state(&session);
        let p = json!({"role":role,"relative_path":"new.json","template_id":if role == "workload" {"json.workload_empty"} else {"json.gateway_empty"},"format":{"newline":"LF","bom":false}});
        assert_eq!(plan_create_file(&session, &p).err().unwrap().status, 422);
        assert_eq!(session_state(&session), initial);
    }
    assert_eq!(disk_tree(&f.root.join("input")), disk);
}

#[test]
fn xc04_real_symlink_is_rejected_during_capture_without_replacing_session() {
    let f = Fixture::new();
    let m = EditorSession::new(f.load());
    let before = session_state(&m);
    let outside = f.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("foreign"), b"preserve").unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("input/vendor/linked")).unwrap();
    let disk = disk_tree(&f.root.join("input"));
    let error = load_project(&f.root.join("input/project.ini"), &f.root)
        .err()
        .unwrap();
    assert_eq!(error.status, 422);
    assert_eq!(error.code, "E-EDITOR-INPUT-PATH");
    assert_eq!(session_state(&m), before);
    assert_eq!(disk_tree(&f.root.join("input")), disk);
    assert_eq!(fs::read(outside.join("foreign")).unwrap(), b"preserve");
}

#[test]
fn xc06_save_as_undo_shape_rejection_redo_preserve_all_disk_bytes_and_hashes() {
    let f = Fixture::new();
    let source = disk_tree(&f.root.join("input"));
    let mut m = EditorSession::new(f.load());
    let mut registry = f.registry();
    registry.register_source(&m.project).unwrap();
    let file = create(&mut m);
    let target = registry.register_destination("saved").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    let report = output::save_plan(plan, &mut registry, None, false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    m.mark_saved(f.root.join("exports/saved").display().to_string());
    assert!(!m.dirty());
    let output = disk_tree(&f.root.join("exports"));
    assert_eq!(
        fs::read(f.root.join("exports/saved/ned/0002/pkg/New.ned")).unwrap(),
        file.text.as_bytes()
    );
    assert!(m.undo().unwrap());
    assert!(m.dirty());
    let undone = session_state(&m);
    let error = output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry)
        .err()
        .unwrap();
    assert_eq!(error.code, "E-TARGET-SHAPE");
    assert_eq!(session_state(&m), undone);
    assert_eq!(disk_tree(&f.root.join("exports")), output);
    assert_eq!(disk_tree(&f.root.join("input")), source);
    assert!(m.redo().unwrap());
    assert!(!m.dirty());
    assert_eq!(m.project.files[&file.id].hash, file.hash);
    assert_eq!(m.project.files[&file.id].text, file.text);
    let repeat =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    let digest = repeat.digest.clone();
    assert_eq!(
        output::save_plan(repeat, &mut registry, Some(&digest), false).state,
        SaveOutcome::Complete
    );
    assert_eq!(disk_tree(&f.root.join("exports")), output);
    assert_eq!(disk_tree(&f.root.join("input")), source);
}

#[test]
fn xc07_external_creation_after_preview_before_save_is_an_ordered_no_clobber_barrier() {
    let f = Fixture::new();
    let source = disk_tree(&f.root.join("input"));
    let mut m = EditorSession::new(f.load());
    create(&mut m);
    let before = session_state(&m);
    let mut registry = f.registry();
    let target = registry.register_destination("collision").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    // Join is a deterministic barrier: the external writer has closed its
    // file after preview and before B4 begins, with no timing-based sleep.
    let foreign = f.root.join("exports/collision/ned/0002/pkg/New.ned");
    std::thread::spawn(move || {
        fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        fs::write(foreign, b"external owner bytes\0\xff").unwrap();
    })
    .join()
    .unwrap();
    let occupied = disk_tree(&f.root.join("exports"));
    let report = output::save_plan(plan, &mut registry, None, false);
    assert_eq!(report.state, SaveOutcome::Rejected);
    assert!(report.error.is_some());
    assert_eq!(disk_tree(&f.root.join("exports")), occupied);
    assert_eq!(disk_tree(&f.root.join("input")), source);
    assert_eq!(session_state(&m), before);
    assert!(registry.pending_recoveries().unwrap().is_empty());
}

#[test]
fn xc21_history_exact_32mib_and_one_byte_over_are_distinct_atomic_boundaries() {
    const LIMIT: usize = 32 * 1024 * 1024;
    let f = Fixture::new();
    let mut m = EditorSession::new(f.load());
    let id = m.project.ned_files().next().unwrap().id.clone();
    let original = m.project.files[&id].text.clone();
    let baseline = m.project.clone();
    let before = session_state(&m);
    let below = format!("{original}\n//{}", "x".repeat(LIMIT - 4));
    assert!(m.commit(vec![(id.clone(), below)], vec![]).unwrap());
    assert_eq!(m.project.files[&id].text.len(), original.len() + LIMIT - 1);
    assert!(m.undo().unwrap());
    // Start a fresh history for the exact/+1 comparisons below.
    m = EditorSession::new(baseline);
    let too_large = format!("{original}\n//{}", "x".repeat(LIMIT - 2));
    let error = m.commit(vec![(id.clone(), too_large)], vec![]).unwrap_err();
    assert_eq!(
        (error.status, error.code.as_str()),
        (413, "E-EDITOR-HISTORY-LIMIT")
    );
    assert_eq!(session_state(&m), before);
    let exact = format!("{original}\n//{}", "x".repeat(LIMIT - 3));
    assert!(m.commit(vec![(id.clone(), exact)], vec![]).unwrap());
    assert!(m.dirty());
    assert_eq!(m.project.files[&id].text.len(), original.len() + LIMIT);
    assert!(m.undo().unwrap());
    assert_eq!(m.project.files[&id].text, original);
    assert!(!m.dirty());
    assert!(!m.undo().unwrap());
    assert!(m.redo().unwrap());
    assert_eq!(m.project.files[&id].text.len(), original.len() + LIMIT);
}

#[test]
fn xc21_added_file_64_65_and_rename_1000_1001_limits_are_separate() {
    let f = Fixture::new();
    let m = EditorSession::new(f.load());
    let before = session_state(&m);
    let mut added = Vec::new();
    for i in 0..65 {
        let mut request = payload(&format!("pkg/New{i}.ned"));
        request["declaration"]["local_name"] = json!(format!("New{i}"));
        added.push(plan_create_file(&m, &request).unwrap().added.remove(0));
    }
    let make = |files| {
        EditPlan::from_changes(
            &m,
            "create_file".into(),
            json!({}),
            EditChanges {
                added: files,
                ..EditChanges::default()
            },
        )
    };
    let error = make(added.clone()).err().unwrap();
    assert_eq!(
        (error.status, error.code.as_str()),
        (413, "E-EDITOR-PLAN-LIMIT")
    );
    added.pop();
    let below = make(added[..63].to_vec()).unwrap();
    assert_eq!(
        below.apply(&m).unwrap().project.files.len(),
        m.project.files.len() + 63
    );
    let plan = make(added).unwrap();
    assert_eq!(plan.changes.added.len(), 64);
    let candidate = plan.apply(&m).unwrap();
    assert_eq!(candidate.project.files.len(), m.project.files.len() + 64);
    assert_eq!(session_state(&m), before);
    for count in [999, 1000, 1001] {
        let entries: Vec<_> = (0..count).map(|i| json!({"target_symbol_id":format!("symbol-{i}"),"expected_old_name":"old","new_local_name":format!("New{i}")})).collect();
        let result = super::refactor_symbols::validate_rename_payload(&json!({"entries":entries}));
        if count <= 1000 {
            result.unwrap(); // payload admission; IDs resolve during planning
        } else {
            let error = result.unwrap_err();
            assert_eq!(
                (error.status, error.code.as_str()),
                (413, "E-EDITOR-PLAN-LIMIT")
            );
        }
    }
    assert_eq!(session_state(&m), before);
}

#[test]
fn xc21_plan_encoded_64mib_boundary_and_ten_minute_expiry_are_independent() {
    let f = Fixture::new();
    let m = EditorSession::new(f.load());
    let before = session_state(&m);
    let changes = EditChanges {
        added: plan_create_file(&m, &payload("pkg/New.ned")).unwrap().added,
        ..EditChanges::default()
    };
    let mut small =
        EditPlan::from_changes(&m, "create_file".into(), json!(""), changes.clone()).unwrap();
    // Measure the actual normalized wire representation of this simple
    // addition. Only normalized_payload grows; source and history stay tiny.
    let fingerprint = json!({"text":[],"added":changes.added.iter().map(|file|json!({"id":file.id,"path":file.path,"roles":file.roles,"hash":file.hash,"actual_hash":hash(file.text.as_bytes()),"origin_hash":file.origin_hash,"actual_origin_hash":hash(file.origin_text.as_bytes()),"stat":file.stat})).collect::<Vec<_>>(),"layouts":[],"symbol_remap":[],"instance_remap":[],"semantic_evidence":null});
    let encoded = serde_json::to_vec(
        &json!({"summary":small.summary,"impacts":small.impacts,"changes":fingerprint}),
    )
    .unwrap()
    .len();
    assert!(encoded < PLAN_BYTES);
    let payload_bytes = PLAN_BYTES - encoded;
    let below = EditPlan::from_changes(
        &m,
        "create_file".into(),
        json!("x".repeat(payload_bytes - 1)),
        changes.clone(),
    )
    .unwrap();
    assert_eq!(
        below.summary["normalized_payload"].as_str().unwrap().len() + encoded,
        PLAN_BYTES - 1
    );
    drop(below);
    let mut exact = EditPlan::from_changes(
        &m,
        "create_file".into(),
        json!("x".repeat(payload_bytes)),
        changes.clone(),
    )
    .unwrap();
    assert_eq!(
        exact.summary["normalized_payload"].as_str().unwrap().len() + encoded,
        PLAN_BYTES
    );
    // Expire without sleeping or changing system time. The 600-second edge
    // is inclusive and checked before the expensive digest recomputation.
    exact.created = std::time::Instant::now() - PLAN_TTL;
    let error = exact.apply(&m).err().unwrap();
    assert_eq!(
        (error.status, error.code.as_str()),
        (410, "E-EDITOR-EXPIRED")
    );
    drop(exact);
    let error = EditPlan::from_changes(
        &m,
        "create_file".into(),
        json!("x".repeat(payload_bytes + 1)),
        changes,
    )
    .err()
    .unwrap();
    assert_eq!(
        (error.status, error.code.as_str()),
        (413, "E-EDITOR-PLAN-LIMIT")
    );
    // A non-expired plan still applies; the larger payload is not needed.
    small.created = std::time::Instant::now() - std::time::Duration::from_secs(599);
    assert!(small.apply(&m).is_ok());
    assert_eq!(session_state(&m), before);
}
