use super::super::input::load_project;
use super::super::layout::LayoutDocument;
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static TEMP: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    project: ProjectSnapshot,
    registry: TargetRegistry,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ned-output-{}-{}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("input/models/demo/empty")).unwrap();
        std::fs::create_dir(root.join("exports")).unwrap();
        std::fs::write(
            root.join("input/models/demo/Main.ned"),
            include_str!("../../../../../docs/verification/fixtures/can/models/demo/Main.ned"),
        )
        .unwrap();
        std::fs::write(root.join("input/project.ini"),"\u{feff}[General]\r\n# keep\r\nnetwork = demo.Main\r\nned-path \t= \"models\" \t\r\nsim-time-limit = 1ms\r\nMain.bus.bitrate = 500kbps\r\n").unwrap();
        let project = load_project(&root.join("input/project.ini"), &root).unwrap();
        let mut registry =
            TargetRegistry::open(&root.join("exports"), &root.join("state")).unwrap();
        registry.register_source(&project).unwrap();
        Self {
            root,
            project,
            registry,
        }
    }
    fn destination(&mut self, name: &str) -> String {
        self.registry.register_destination(name).unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn plan(&self, id: &str) -> SavePlan {
        build_plan(&self.project, 1, 1, id, &self.registry).unwrap()
    }
    fn source_id(&self) -> String {
        self.registry
            .data
            .targets
            .values()
            .find(|t| t.kind == TargetKind::SourceProject)
            .unwrap()
            .id
            .clone()
    }
    fn edit(&mut self) {
        let file = self
            .project
            .files
            .values_mut()
            .find(|f| f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })))
            .unwrap();
        file.text = file.text.replace("default(64)", "default(65)").into();
        file.hash = hash(file.text.as_bytes());
    }
    fn restart(&self) -> TargetRegistry {
        TargetRegistry::open(&self.root.join("exports"), &self.root.join("state")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        FAILURE.with(|f| *f.borrow_mut() = None);
        PUBLICATION_COLLISION.with(|f| *f.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn inject(name: &str, count: usize) {
    FAILURE.with(|f| *f.borrow_mut() = Some((name.into(), count)));
}

#[test]
fn export_uses_snapshot_after_source_deleted_and_plan_digest_is_deterministic() {
    let mut f = Fixture::new();
    let id = f.destination("new");
    let first = f.plan(&id);
    let second = f.plan(&id);
    assert_ne!(first.id, second.id);
    assert_eq!(first.digest, second.digest);
    std::fs::remove_dir_all(f.root.join("input")).unwrap();
    let report = save_plan(first, &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    assert!(f.root.join("exports/new/ned/0001/demo/empty").is_dir());
    let ini = std::fs::read_to_string(f.root.join("exports/new/project.ini")).unwrap();
    assert!(ini.contains("# keep\r\n"));
    assert!(ini.contains("ned-path \t= \"ned/0001\" \t\r\n"));
    crate::input::prepare(&f.root.join("exports/new/project.ini")).unwrap();
    assert!(f.restart().pending_recoveries().unwrap().is_empty());
}
#[test]
fn source_overwrite_requires_exact_plan_confirmation_and_preserves_ini() {
    let mut f = Fixture::new();
    let id = f.source_id();
    let original = std::fs::read(f.root.join("input/project.ini")).unwrap();
    f.edit();
    let plan = f.plan(&id);
    let digest = plan.digest.clone();
    assert_eq!(
        save_plan(plan.clone(), &mut f.registry, None, false).state,
        SaveOutcome::Rejected
    );
    let report = save_plan(plan, &mut f.registry, Some(&digest), false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    assert_eq!(
        std::fs::read(f.root.join("input/project.ini")).unwrap(),
        original
    );
    assert!(
        std::fs::read_to_string(f.root.join("input/models/demo/Main.ned"))
            .unwrap()
            .contains("default(65)")
    );
}
#[test]
fn semantic_invalid_output_creates_no_output_or_journal() {
    let mut f = Fixture::new();
    let id = f.destination("invalid");
    let file = f
        .project
        .files
        .values_mut()
        .find(|v| v.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })))
        .unwrap();
    file.text = file.text.replace("default(64)", "default(-1)").into();
    let report = save_plan(f.plan(&id), &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::Rejected);
    assert_eq!(report.error.unwrap().code, "E-EDITOR-SAVE-VALIDATION");
    assert!(!f.root.join("exports/invalid").exists());
    assert!(
        Directory::open(&f.root.join("state/recovery"))
            .unwrap()
            .names()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn new_target_foreign_entry_and_existing_external_changes_are_preserved() {
    let mut f = Fixture::new();
    let id = f.destination("occupied");
    let plan = f.plan(&id);
    std::fs::create_dir(f.root.join("exports/occupied")).unwrap();
    std::fs::write(f.root.join("exports/occupied/foreign"), "keep").unwrap();
    assert_eq!(
        save_plan(plan, &mut f.registry, None, false).state,
        SaveOutcome::Rejected
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("exports/occupied/foreign")).unwrap(),
        "keep"
    );
    let source = f.source_id();
    f.edit();
    let plan = f.plan(&source);
    let digest = plan.digest.clone();
    std::fs::write(f.root.join("input/models/demo/Main.ned"), "foreign").unwrap();
    assert_eq!(
        save_plan(plan, &mut f.registry, Some(&digest), false).state,
        SaveOutcome::Rejected
    );
    assert_eq!(
        std::fs::read_to_string(f.root.join("input/models/demo/Main.ned")).unwrap(),
        "foreign"
    );
}
#[test]
fn invalid_layout_needs_explicit_replacement_confirmation() {
    let mut f = Fixture::new();
    let path = f.root.join("input/models/demo/Main.ned.layout.json");
    std::fs::write(&path, "broken layout").unwrap();
    f.project = load_project(&f.root.join("input/project.ini"), &f.root).unwrap();
    let source = f.registry.register_source(&f.project).unwrap();
    let layout = f.project.layouts.values_mut().next().unwrap();
    layout.adopted = Some(LayoutDocument::empty("Main.ned".into()));
    let plan = f.plan(&source);
    let digest = plan.digest.clone();
    assert!(plan.requires_layout_confirmation);
    assert_eq!(
        save_plan(plan.clone(), &mut f.registry, Some(&digest), false).state,
        SaveOutcome::Rejected
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken layout");
    let report = save_plan(plan, &mut f.registry, Some(&digest), true);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    LayoutDocument::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
}
#[test]
fn restart_completes_partial_no_clobber_publication_using_hash_truth() {
    let mut f = Fixture::new();
    let id = f.destination("partial");
    inject("after_publish", 0);
    let report = save_plan(f.plan(&id), &mut f.registry, None, false);
    assert_eq!(
        report.state,
        SaveOutcome::RecoveryRequired,
        "{:?}",
        report.error
    );
    let recovery = report.recovery_id.unwrap();
    let mut registry = f.restart();
    assert_eq!(registry.pending_recoveries().unwrap().len(), 1);
    let report = recover(&mut registry, &recovery, "complete");
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    crate::input::prepare(&f.root.join("exports/partial/project.ini")).unwrap();
}
#[test]
fn restore_partial_source_overwrite_returns_exact_before_bytes() {
    let mut f = Fixture::new();
    let original = std::fs::read(f.root.join("input/models/demo/Main.ned")).unwrap();
    f.edit();
    let source = f.source_id();
    let plan = f.plan(&source);
    let digest = plan.digest.clone();
    inject("after_publish", 0);
    let report = save_plan(plan, &mut f.registry, Some(&digest), false);
    assert_eq!(
        report.state,
        SaveOutcome::RecoveryRequired,
        "{:?}",
        report.error
    );
    let mut registry = f.restart();
    let report = recover(
        &mut registry,
        report.recovery_id.as_ref().unwrap(),
        "restore",
    );
    assert_eq!(report.state, SaveOutcome::Restored, "{:?}", report.error);
    assert_eq!(
        std::fs::read(f.root.join("input/models/demo/Main.ned")).unwrap(),
        original
    );
}
#[test]
fn recovery_third_party_change_stops_before_any_other_target_changes() {
    let mut f = Fixture::new();
    let id = f.destination("changed");
    inject("after_publish", 0);
    let report = save_plan(f.plan(&id), &mut f.registry, None, false);
    let recovery = report.recovery_id.unwrap();
    let path = f.root.join("exports/changed/ned/0001/demo/Main.ned");
    std::fs::write(&path, "third party").unwrap();
    let mut registry = f.restart();
    let result = recover(&mut registry, &recovery, "restore");
    assert_eq!(result.state, SaveOutcome::RecoveryRequired);
    assert!(!f.root.join("exports/changed/project.ini").exists());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "third party");
}
#[test]
fn completed_manifest_repairs_registry_after_crash_gap() {
    let mut f = Fixture::new();
    let id = f.destination("checkpoint");
    inject("after_completed_manifest", 0);
    let report = save_plan(f.plan(&id), &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::RecoveryRequired);
    let registry = f.restart();
    assert_eq!(registry.data.targets[&id].kind, TargetKind::ManagedExport);
    assert!(registry.pending_recoveries().unwrap().is_empty());
}

#[test]
fn completed_checkpoint_failure_can_be_finalized_without_restarting() {
    for point in ["after_completed_manifest", "after_registry"] {
        let mut f = Fixture::new();
        let id = f.destination("checkpoint");
        inject(point, 0);
        let report = save_plan(f.plan(&id), &mut f.registry, None, false);
        assert_eq!(report.state, SaveOutcome::RecoveryRequired);
        let recovery = report.recovery_id.unwrap();
        let pending = f.registry.pending_recoveries().unwrap();
        assert_eq!(pending.len(), 1, "{point}: {pending:?}");
        assert_eq!(pending[0]["id"], recovery);
        assert_eq!(pending[0]["state"], "finalizing");
        let report = recover(&mut f.registry, &recovery, "complete");
        assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
        assert!(f.registry.pending_recoveries().unwrap().is_empty());
        assert_eq!(f.registry.data.targets[&id].kind, TargetKind::ManagedExport);
        crate::input::prepare(&f.root.join("exports/checkpoint/project.ini")).unwrap();
    }
}

#[test]
fn recovery_checkpoint_failure_keeps_finalization_available_for_retry() {
    let mut f = Fixture::new();
    let target = f.destination("recovery-checkpoint");
    inject("after_publish", 0);
    let partial = save_plan(f.plan(&target), &mut f.registry, None, false);
    let recovery = partial.recovery_id.unwrap();
    inject("after_completed_manifest", 0);
    let report = recover(&mut f.registry, &recovery, "complete");
    assert_eq!(report.state, SaveOutcome::RecoveryRequired);
    let pending = f.registry.pending_recoveries().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["state"], "finalizing");
    assert_eq!(
        recover(&mut f.registry, &recovery, "complete").state,
        SaveOutcome::Complete
    );
    assert!(f.registry.pending_recoveries().unwrap().is_empty());
}

#[test]
fn successful_recovery_clears_marker_when_completion_journal_was_not_persisted() {
    let mut f = Fixture::new();
    let target = f.destination("unpersisted-completion");
    inject("after_publish", 0);
    let partial = save_plan(f.plan(&target), &mut f.registry, None, false);
    let recovery = partial.recovery_id.unwrap();
    // A failed completed-manifest write can leave the previous journal on disk
    // even though the in-memory operation registered a finalization retry.
    f.registry.pending_finalizations.insert(recovery.clone());
    assert_eq!(
        recover(&mut f.registry, &recovery, "complete").state,
        SaveOutcome::Complete
    );
    assert!(f.registry.pending_finalizations.is_empty());
    assert!(f.registry.pending_recoveries().unwrap().is_empty());
}

#[test]
fn invalid_journal_is_protected_without_hiding_other_recoveries_or_blocking_startup() {
    for corruption in ["malformed_json", "missing_completion", "stale_registration"] {
        let mut f = Fixture::new();
        let bad_target = f.destination("bad");
        inject("after_publish", 0);
        let bad = save_plan(f.plan(&bad_target), &mut f.registry, None, false);
        let bad_id = bad.recovery_id.unwrap();
        let good_target = f.destination("good");
        inject("after_publish", 0);
        let good = save_plan(f.plan(&good_target), &mut f.registry, None, false);
        let good_id = good.recovery_id.unwrap();
        let path = manifest_path(&f.registry, &bad_id);
        let original = std::fs::read(&path).unwrap();
        let bytes = if corruption == "malformed_json" {
            b"{invalid".to_vec()
        } else if corruption == "stale_registration" {
            f.registry.data.targets.remove(&bad_target);
            f.registry.persist().unwrap();
            original
        } else {
            let mut value: Value = serde_json::from_slice(&original).unwrap();
            value["state"] = json!("completed");
            value["completed_target"] = Value::Null;
            serde_json::to_vec(&value).unwrap()
        };
        std::fs::write(&path, &bytes).unwrap();
        let mut registry = f.restart();
        let pending = registry.pending_recoveries().unwrap();
        assert_eq!(pending.len(), 2, "{corruption}: {pending:?}");
        let invalid = pending
            .iter()
            .find(|record| record["id"] == bad_id)
            .unwrap();
        assert_eq!(invalid["state"], "invalid");
        assert!(
            invalid["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
        assert_eq!(
            recover(&mut registry, &bad_id, "restore").state,
            SaveOutcome::Rejected
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let recovered = recover(&mut registry, &good_id, "complete");
        assert_eq!(
            recovered.state,
            SaveOutcome::Complete,
            "{:?}",
            recovered.error
        );
        assert_eq!(registry.pending_recoveries().unwrap().len(), 1);
        crate::input::prepare(&f.root.join("exports/good/project.ini")).unwrap();
    }
}

#[test]
fn export_copies_workload_valid_layout_and_multiple_ordered_ned_roots() {
    let mut f = Fixture::new();
    std::fs::create_dir_all(f.root.join("vendor/libpkg/empty")).unwrap();
    std::fs::write(
        f.root.join("vendor/libpkg/Unused.ned"),
        "package libpkg; module Unused {}\n",
    )
    .unwrap();
    let ini = std::fs::read_to_string(f.root.join("input/project.ini"))
        .unwrap()
        .replace(
            "\"models\"",
            &format!(
                "\"models\";{}",
                quoted(f.root.join("vendor").to_str().unwrap())
            ),
        );
    std::fs::write(
        f.root.join("input/project.ini"),
        format!("{ini}workload = \"workload.json\"\r\n"),
    )
    .unwrap();
    std::fs::write(
        f.root.join("input/workload.json"),
        "{\"schema_version\":1,\"generators\":[]}\n",
    )
    .unwrap();
    let layout = LayoutDocument::empty("Main.ned".into());
    std::fs::write(
        f.root.join("input/models/demo/Main.ned.layout.json"),
        layout.bytes().unwrap(),
    )
    .unwrap();
    f.project = load_project(&f.root.join("input/project.ini"), &f.root).unwrap();
    f.registry.register_source(&f.project).unwrap();
    let id = f.destination("relocated");
    let plan = f.plan(&id);
    std::fs::remove_dir_all(f.root.join("input")).unwrap();
    std::fs::remove_dir_all(f.root.join("vendor")).unwrap();
    let report = save_plan(plan, &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    assert!(
        f.root
            .join("exports/relocated/ned/0002/libpkg/empty")
            .is_dir()
    );
    assert!(
        f.root
            .join("exports/relocated/ned/0001/demo/Main.ned.layout.json")
            .is_file()
    );
    assert!(
        f.root
            .join("exports/relocated/data/workload.json")
            .is_file()
    );
    crate::input::prepare(&f.root.join("exports/relocated/project.ini")).unwrap();
}
#[test]
fn same_json_input_is_planned_in_both_reference_roles() {
    let mut f = Fixture::new();
    let ini = std::fs::read_to_string(f.root.join("input/project.ini")).unwrap();
    std::fs::write(
        f.root.join("input/project.ini"),
        format!("{ini}workload = \"shared.json\"\r\nmodel-config = \"shared.json\"\r\n"),
    )
    .unwrap();
    std::fs::write(
        f.root.join("input/shared.json"),
        "{\"schema_version\":1,\"generators\":[]}\n",
    )
    .unwrap();
    f.project = load_project(&f.root.join("input/project.ini"), &f.root).unwrap();
    f.registry.register_source(&f.project).unwrap();
    let id = f.destination("duplicate-role");
    let plan = f.plan(&id);
    let workload = plan
        .files
        .iter()
        .find(|p| p.path.ends_with("data/workload.json"))
        .unwrap();
    let config = plan
        .files
        .iter()
        .find(|p| p.path.ends_with("data/model-config.json"))
        .unwrap();
    assert_eq!(workload.bytes, config.bytes);
    // Current ideal profile rejects model-config; the common validator remains authoritative.
    assert_eq!(
        save_plan(plan, &mut f.registry, None, false).state,
        SaveOutcome::Rejected
    );
}
#[test]
fn managed_save_allows_reserved_layout_creation_and_undo_to_empty_layout() {
    let mut f = Fixture::new();
    let id = f.destination("managed");
    assert_eq!(
        save_plan(f.plan(&id), &mut f.registry, None, false).state,
        SaveOutcome::Complete
    );
    let layout = f.project.layouts.values_mut().next().unwrap();
    let mut document = LayoutDocument::empty("Main.ned".into());
    let mut ty = super::super::layout::TypeLayout::default();
    ty.nodes
        .insert("a".into(), super::super::layout::Position::new(120.0, 80.0));
    document.types.insert("demo.Main".into(), ty);
    layout.adopted = Some(document);
    let plan = f.plan(&id);
    let digest = plan.digest.clone();
    let report = save_plan(plan, &mut f.registry, Some(&digest), false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    let output = f
        .root
        .join("exports/managed/ned/0001/demo/Main.ned.layout.json");
    assert!(
        !LayoutDocument::parse(&std::fs::read_to_string(&output).unwrap())
            .unwrap()
            .types
            .is_empty()
    );
    f.project.layouts.values_mut().next().unwrap().adopted = None;
    let plan = f.plan(&id);
    let digest = plan.digest.clone();
    let report = save_plan(plan, &mut f.registry, Some(&digest), false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    assert!(
        LayoutDocument::parse(&std::fs::read_to_string(&output).unwrap())
            .unwrap()
            .types
            .is_empty()
    );
    std::fs::write(
        f.root.join("input/models/demo/Extra.ned"),
        "package demo; module Extra {}\n",
    )
    .unwrap();
    f.project = load_project(&f.root.join("input/project.ini"), &f.root).unwrap();
    f.registry.register_source(&f.project).unwrap();
    let error = build_plan(&f.project, 2, 2, &id, &f.registry)
        .err()
        .unwrap();
    assert_eq!(error.code, "E-TARGET-SHAPE");
}

#[test]
fn nonreplace_publication_collision_never_clobbers_external_inode() {
    let mut f = Fixture::new();
    let id = f.destination("no-clobber");
    let plan = f.plan(&id);
    // Stop immediately after the durable journal, before output directory creation.
    inject("journal_prepared", 0);
    let report = save_plan(plan, &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::RecoveryRequired);
    let recovery = report.recovery_id.unwrap();
    let mut m = f.registry.load_manifest(&recovery).unwrap();
    for d in &mut m.directories {
        if stat_optional(&d.path).unwrap().is_none() {
            d.created = Some(mkdir_owned(&d.path, 0o777).unwrap());
        }
    }
    save_manifest(&f.registry, &m).unwrap();
    let index = m.files.iter().position(|f| f.rank == 0).unwrap();
    stage(&f.registry, &mut m, index).unwrap();
    let target = m.files[index].path.clone();
    // Create the foreign inode after all prechecks, immediately before linkat.
    PUBLICATION_COLLISION.with(|collision| *collision.borrow_mut() = Some(target.clone()));
    assert!(publish(&f.registry, &mut m, index).is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "foreign");
}
#[test]
fn source_symlink_hardlink_and_permission_changes_are_conflicts() {
    for change in ["symlink", "hardlink", "mode"] {
        let mut f = Fixture::new();
        f.edit();
        let source = f.source_id();
        let plan = f.plan(&source);
        let digest = plan.digest.clone();
        let target = f.root.join("input/models/demo/Main.ned");
        match change {
            "symlink" => {
                std::fs::rename(&target, target.with_extension("original")).unwrap();
                std::os::unix::fs::symlink(target.with_extension("original"), &target).unwrap();
            }
            "hardlink" => std::fs::hard_link(&target, target.with_extension("linked")).unwrap(),
            _ => {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
        assert_eq!(
            save_plan(plan, &mut f.registry, Some(&digest), false).state,
            SaveOutcome::Rejected,
            "{change}"
        );
    }
}
#[test]
fn recovery_action_cannot_flip_and_untrusted_manifest_cannot_authorize_new_paths() {
    let mut f = Fixture::new();
    let id = f.destination("bound");
    inject("after_publish", 0);
    let report = save_plan(f.plan(&id), &mut f.registry, None, false);
    let recovery = report.recovery_id.unwrap();
    inject("after_restore", 0);
    assert_eq!(
        recover(&mut f.registry, &recovery, "restore").state,
        SaveOutcome::RecoveryRequired
    );
    let result = recover(&mut f.registry, &recovery, "complete");
    assert_eq!(result.error.unwrap().code, "E-EDITOR-SAVE-CONFLICT");
    let path = manifest_path(&f.registry, &recovery);
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["files"][0]["path"] = json!(f.root.join("outside"));
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let result = recover(&mut f.registry, &recovery, "restore");
    assert_eq!(result.state, SaveOutcome::Rejected);
    assert_eq!(result.error.unwrap().code, "E-EDITOR-RECOVERY-INVALID");
    assert!(!f.root.join("outside").exists());
}
#[test]
fn restore_allows_semantically_invalid_before_content_and_preserves_invalid_utf8_layout() {
    let mut f = Fixture::new();
    let ned = f.root.join("input/models/demo/Main.ned");
    let bad = std::fs::read_to_string(&ned)
        .unwrap()
        .replace("default(64)", "default(-1)");
    std::fs::write(&ned, &bad).unwrap();
    f.project = load_project(&f.root.join("input/project.ini"), &f.root).unwrap();
    let source = f.registry.register_source(&f.project).unwrap();
    let file = f
        .project
        .files
        .values_mut()
        .find(|f| f.roles.iter().any(|r| matches!(r, FileRole::Ned { .. })))
        .unwrap();
    file.text = file.text.replace("default(-1)", "default(64)").into();
    let plan = f.plan(&source);
    let digest = plan.digest.clone();
    inject("after_publish", 0);
    let report = save_plan(plan, &mut f.registry, Some(&digest), false);
    let recovery = report.recovery_id.unwrap();
    let result = recover(&mut f.registry, &recovery, "restore");
    assert_eq!(result.state, SaveOutcome::Restored, "{:?}", result.error);
    assert_eq!(std::fs::read_to_string(&ned).unwrap(), bad);
    // Layout hash/stat alone are sufficient to protect a non-UTF-8 sidecar.
    let layout_path = f.root.join("input/models/demo/Main.ned.layout.json");
    std::fs::write(&layout_path, [0xff, 0xfe]).unwrap();
    let layout = f.project.layouts.values_mut().next().unwrap();
    layout.raw = None;
    layout.hash = Some(hash(&[0xff, 0xfe]));
    layout.stat = Some(super::super::input::CapturedStat::from_metadata(
        &std::fs::metadata(&layout_path).unwrap(),
    ));
    layout.warning = Some("Invalid UTF-8".into());
    f.registry.register_source(&f.project).unwrap();
    let registered = &f.registry.data.targets[&source];
    assert!(registered.protected_layouts.contains(&layout_path));
    assert!(matches!(
        registered.baseline[&layout_path],
        Before::Present { .. }
    ));
}

#[test]
fn stage_identity_journal_gap_can_restart_complete_or_restore_without_unknown_cleanup() {
    for action in ["complete", "restore"] {
        let mut f = Fixture::new();
        let id = f.destination(action);
        inject("after_stage_file_sync", 0);
        let report = save_plan(f.plan(&id), &mut f.registry, None, false);
        assert_eq!(
            report.state,
            SaveOutcome::RecoveryRequired,
            "{:?}",
            report.error
        );
        let recovery = report.recovery_id.unwrap();
        let journal = f.registry.load_manifest(&recovery).unwrap();
        let file = journal
            .files
            .iter()
            .find(|file| {
                stat_optional(&file.path.with_file_name(&file.temp_name))
                    .unwrap()
                    .is_some()
            })
            .unwrap();
        assert!(file.temp_identity.is_none());
        let orphan = file.path.with_file_name(&file.temp_name);
        let original = std::fs::read(&orphan).unwrap();
        let mut registry = f.restart();
        let report = recover(&mut registry, &recovery, action);
        assert_eq!(
            report.state,
            if action == "complete" {
                SaveOutcome::Complete
            } else {
                SaveOutcome::Restored
            },
            "{:?}",
            report.error
        );
        assert_eq!(std::fs::read(&orphan).unwrap(), original);
        assert!(
            report
                .files
                .iter()
                .any(|file| file.path == orphan && file.state == "preserved_temporary")
        );
        if action == "complete" {
            crate::input::prepare(&f.root.join(format!("exports/{action}/project.ini"))).unwrap();
        } else {
            assert!(
                !f.root
                    .join(format!("exports/{action}/project.ini"))
                    .exists()
            );
        }
    }
}
#[test]
fn directory_identity_journal_gap_can_restart_without_unknown_directory_cleanup() {
    for action in ["complete", "restore"] {
        let mut f = Fixture::new();
        let id = f.destination(action);
        inject("after_mkdir", 0);
        let report = save_plan(f.plan(&id), &mut f.registry, None, false);
        assert_eq!(report.state, SaveOutcome::RecoveryRequired);
        let recovery = report.recovery_id.unwrap();
        let journal = f.registry.load_manifest(&recovery).unwrap();
        let unknown = journal
            .directories
            .iter()
            .find(|directory| stat_optional(&directory.path).unwrap().is_some())
            .unwrap();
        assert!(unknown.before.is_none() && unknown.created.is_none());
        let retained = unknown.path.clone();
        let identity = Directory::open(&retained).unwrap().identity();
        let mut registry = f.restart();
        let report = recover(&mut registry, &recovery, action);
        assert_eq!(
            report.state,
            if action == "complete" {
                SaveOutcome::Complete
            } else {
                SaveOutcome::Restored
            },
            "{:?}",
            report.error
        );
        assert_eq!(Directory::open(&retained).unwrap().identity(), identity);
        if action == "complete" {
            crate::input::prepare(&retained.join("project.ini")).unwrap();
        } else {
            assert!(
                Directory::open(&retained)
                    .unwrap()
                    .names()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
#[test]
fn replay_of_old_completed_save_does_not_roll_back_newer_baseline() {
    let mut f = Fixture::new();
    let id = f.destination("generations");
    inject("after_gc_registry", 0);
    let first = save_plan(f.plan(&id), &mut f.registry, None, false);
    assert_eq!(first.state, SaveOutcome::RecoveryRequired);
    f.edit();
    let second = f.plan(&id);
    let digest = second.digest.clone();
    assert_eq!(
        save_plan(second, &mut f.registry, Some(&digest), false).state,
        SaveOutcome::Complete
    );
    let before = f.registry.data.targets[&id].clone();
    let report = recover(&mut f.registry, &first.save_id, "complete");
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    assert_eq!(f.registry.data.targets[&id].generation, before.generation);
    assert_eq!(f.registry.data.targets[&id].baseline, before.baseline);
    let file = f
        .project
        .files
        .values_mut()
        .find(|file| {
            file.roles
                .iter()
                .any(|role| matches!(role, FileRole::Ned { .. }))
        })
        .unwrap();
    file.text = file.text.replace("default(65)", "default(66)").into();
    let third = f.plan(&id);
    let digest = third.digest.clone();
    let report = save_plan(third, &mut f.registry, Some(&digest), false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
}
#[test]
fn cached_registry_clients_preserve_each_others_registered_destinations() {
    let mut f = Fixture::new();
    let mut other = f.registry.clone();
    let first = f.destination("first-client");
    let second = other.register_destination("second-client").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        save_plan(f.plan(&first), &mut f.registry, None, false).state,
        SaveOutcome::Complete
    );
    let plan = build_plan(&f.project, 1, 1, &second, &other).unwrap();
    assert_eq!(
        save_plan(plan, &mut other, None, false).state,
        SaveOutcome::Complete
    );
    let registry = f.restart();
    assert!(
        registry.data.targets.contains_key(&first) && registry.data.targets.contains_key(&second)
    );
}

#[test]
fn successful_saves_reclaim_all_private_backups_immediately() {
    let mut f = Fixture::new();
    let id = f.source_id();
    for _ in 0..3 {
        f.edit();
        let plan = f.plan(&id);
        let digest = plan.digest.clone();
        let report = save_plan(plan, &mut f.registry, Some(&digest), false);
        assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
        assert!(!f.root.join("state/recovery").join(&report.save_id).exists());
        assert!(f.registry.data.garbage.is_empty());
    }
    assert!(
        std::fs::read_dir(f.root.join("state/recovery"))
            .unwrap()
            .next()
            .is_none()
    );
    crate::input::prepare(&f.root.join("input/project.ini")).unwrap();
}

#[test]
fn retirement_and_each_reclamation_boundary_resume_after_restart_and_retry() {
    for point in [
        "before_gc_registry",
        "after_gc_registry",
        "before_gc_retire",
        "after_gc_retire",
        "after_gc_retire_sync",
        "before_gc_unlink",
        "after_gc_unlink",
        "after_gc_sync",
        "before_gc_forget",
    ] {
        for restart in [false, true] {
            // Source saves have both before and after blobs, exercising every deletion.
            let mut f = Fixture::new();
            let target = f.source_id();
            let plan = f.plan(&target);
            let count = plan.files.len() * 2 + 4;
            let iterations = if point.contains("unlink") || point == "after_gc_sync" {
                count
            } else {
                1
            };
            for nth in 0..iterations {
                let plan = f.plan(&target);
                let digest = plan.digest.clone();
                inject(point, nth);
                let report = save_plan(plan, &mut f.registry, Some(&digest), false);
                assert_eq!(
                    report.state,
                    SaveOutcome::RecoveryRequired,
                    "{point}/{nth}/{restart}: {:?}",
                    report.error
                );
                let id = report.recovery_id.unwrap();
                // Repeat the interrupted boundary on startup or in the same process.
                inject("before_gc_registry", 0);
                if restart {
                    f.registry = f.restart();
                    assert_eq!(
                        f.registry.pending_recoveries().unwrap()[0]["state"],
                        "finalizing"
                    );
                } else {
                    let retry = recover(&mut f.registry, &id, "complete");
                    assert_eq!(
                        retry.state,
                        SaveOutcome::RecoveryRequired,
                        "{point}/{nth}: {:?}",
                        retry.error
                    );
                }
                FAILURE.with(|v| *v.borrow_mut() = None);
                if !f.registry.data.garbage.is_empty() || manifest_path(&f.registry, &id).exists() {
                    let retry = recover(&mut f.registry, &id, "complete");
                    assert_eq!(
                        retry.state,
                        SaveOutcome::Complete,
                        "{point}/{nth}/{restart}: {:?}",
                        retry.error
                    );
                }
                assert!(!f.root.join("state/recovery").join(&id).exists());
                assert!(f.registry.data.garbage.is_empty());
                assert!(f.registry.pending_recoveries().unwrap().is_empty());
                crate::input::prepare(&f.root.join("input/project.ini")).unwrap();
                f.registry = f.restart();
                assert!(f.registry.data.garbage.is_empty());
            }
        }
    }
}

#[test]
fn reclamation_preserves_unknown_identities_links_contents_and_unrecovered_journals() {
    use std::os::unix::fs::symlink;
    for change in [
        "blob_identity",
        "blob_content",
        "manifest_content",
        "symlink",
        "hardlink",
        "directory",
        "unknown",
        "retired_marker",
    ] {
        let mut f = Fixture::new();
        let target = f.source_id();
        let plan = f.plan(&target);
        let digest = plan.digest.clone();
        inject("after_gc_registry", 0);
        let report = save_plan(plan, &mut f.registry, Some(&digest), false);
        let id = report.recovery_id.unwrap();
        let root = f.root.join("state/recovery").join(&id);
        let record = f.registry.data.garbage[&id].clone();
        let blob_name = record
            .nodes
            .keys()
            .find(|n| n.starts_with("after/"))
            .unwrap();
        let blob = root.join(blob_name);
        let original = std::fs::read(&blob).unwrap();
        match change {
            "blob_identity" => {
                std::fs::rename(&blob, root.join("old-blob")).unwrap();
                std::fs::write(&blob, &original).unwrap();
            }
            "blob_content" => std::fs::write(&blob, "modified").unwrap(),
            "manifest_content" => {
                let path = root.join("manifest.json");
                let mut bytes = std::fs::read(&path).unwrap();
                bytes.push(b' ');
                std::fs::write(path, bytes).unwrap();
            }
            "symlink" => {
                std::fs::remove_file(&blob).unwrap();
                symlink(f.root.join("input/project.ini"), &blob).unwrap();
            }
            "hardlink" => std::fs::hard_link(&blob, f.root.join("linked-blob")).unwrap(),
            "directory" => {
                std::fs::rename(root.join("after"), root.join("old-after")).unwrap();
                std::fs::create_dir(root.join("after")).unwrap();
            }
            "unknown" => std::fs::write(root.join("unknown"), "preserve").unwrap(),
            "retired_marker" => std::fs::write(root.join("retired.json"), "preserve").unwrap(),
            _ => unreachable!(),
        }
        let before = std::fs::read_dir(root.join("before")).unwrap().count();
        let retry = recover(&mut f.registry, &id, "complete");
        assert_eq!(
            retry.state,
            SaveOutcome::RecoveryRequired,
            "{change}: {:?}",
            retry.error
        );
        assert!(root.join("manifest.json").exists(), "{change}");
        assert_eq!(
            std::fs::read_dir(root.join("before")).unwrap().count(),
            before
        );
        f.registry = f.restart();
        assert!(root.exists());
        assert_eq!(
            f.registry.pending_recoveries().unwrap()[0]["state"],
            "invalid",
            "{change}: {:?}",
            f.registry.collection_errors
        );
        assert!(f.root.join("input/project.ini").is_file());
    }
    let mut f = Fixture::new();
    let target = f.destination("unfinished");
    inject("after_publish", 0);
    let report = save_plan(f.plan(&target), &mut f.registry, None, false);
    let id = report.recovery_id.unwrap();
    let root = f.root.join("state/recovery").join(id);
    let bytes = std::fs::read(root.join("manifest.json")).unwrap();
    f.registry = f.restart();
    assert_eq!(std::fs::read(root.join("manifest.json")).unwrap(), bytes);
    assert!(root.join("after").is_dir());
    assert!(f.registry.data.garbage.is_empty());
}

#[test]
fn malformed_retirement_allowlist_cannot_escape_private_journal() {
    let mut f = Fixture::new();
    let target = f.destination("gc-safe");
    inject("after_gc_registry", 0);
    let report = save_plan(f.plan(&target), &mut f.registry, None, false);
    let id = report.recovery_id.unwrap();
    let mut record = f.registry.data.garbage[&id].clone();
    let node = record.nodes["manifest.json"].clone();
    record.nodes.insert("../../input/project.ini".into(), node);
    f.registry.data.garbage.insert(id.clone(), record);
    assert!(f.registry.data.garbage[&id].validate(&id).is_err());
    assert!(f.registry.collect_retired(&id).is_err());
    assert!(manifest_path(&f.registry, &id).exists());
    // Refresh restores the independently durable authority rather than trusting cache corruption.
    assert_eq!(
        recover(&mut f.registry, &id, "complete").state,
        SaveOutcome::Complete
    );
    assert!(f.root.join("input/project.ini").is_file());
}

#[test]
fn restored_journal_with_unknown_entry_remains_visible_and_protected() {
    let mut f = Fixture::new();
    let target = f.destination("restore-protected");
    inject("after_publish", 0);
    let report = save_plan(f.plan(&target), &mut f.registry, None, false);
    let id = report.recovery_id.unwrap();
    let root = f.root.join("state/recovery").join(&id);
    std::fs::write(root.join("unknown"), "preserve").unwrap();
    let report = recover(&mut f.registry, &id, "restore");
    assert_eq!(report.state, SaveOutcome::RecoveryRequired);
    assert_eq!(
        f.registry.pending_recoveries().unwrap()[0]["state"],
        "invalid"
    );
    assert!(root.join("manifest.json").is_file());
    f.registry = f.restart();
    assert_eq!(
        f.registry.pending_recoveries().unwrap()[0]["state"],
        "invalid"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("unknown")).unwrap(),
        "preserve"
    );
    assert!(f.registry.data.garbage.is_empty());
}

#[test]
fn unknown_orphan_and_symlinked_journal_roots_are_never_collected() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new();
    let unknown = f
        .root
        .join("state/recovery")
        .join(format!("save-{}", "a".repeat(64)));
    std::fs::create_dir(&unknown).unwrap();
    std::fs::write(unknown.join("orphan"), "unknown").unwrap();
    let target = f.destination("symlink-protected");
    inject("after_gc_registry", 0);
    let report = save_plan(f.plan(&target), &mut f.registry, None, false);
    let id = report.recovery_id.unwrap();
    let root = f.root.join("state/recovery").join(&id);
    let preserved = f.root.join("saved-journal");
    std::fs::rename(&root, &preserved).unwrap();
    symlink(&preserved, &root).unwrap();
    f.registry = f.restart();
    assert!(
        std::fs::symlink_metadata(&root)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(preserved.join("manifest.json").exists());
    assert!(unknown.join("orphan").exists());
    assert!(
        f.root
            .join("exports/symlink-protected/project.ini")
            .is_file()
    );
    assert_eq!(
        f.registry.pending_recoveries().unwrap()[0]["state"],
        "invalid"
    );
}
