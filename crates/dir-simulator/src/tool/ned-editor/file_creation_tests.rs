use super::super::{
    analysis,
    input::SnapshotInputSource,
    layout::{LayoutDocument, Position, TypeLayout},
    output, template,
};
use super::*;
use crate::input::InputKind;
use serde_json::json;
use std::path::PathBuf;

fn model(kind: &str) -> EditorSession {
    let cwd = std::env::temp_dir().join(random_id("create-file-test-").unwrap());
    EditorSession::new(template::new_project(kind, "Demo", &cwd).unwrap())
}
fn ned(path: &str, name: &str) -> Value {
    json!({"role":"ned","root_id":"ned-root-0","relative_path":path,"template_id":"ned.module_empty","declaration":{"kind":"module","local_name":name},"format":{"newline":"LF","bom":false}})
}
fn json_payload(role: &str, path: &str) -> Value {
    json!({"role":role,"relative_path":path,"template_id":if role == "workload" {"json.workload_empty"} else {"json.gateway_empty"},"format":{"newline":"LF","bom":false}})
}
fn apply(m: &mut EditorSession, p: &Value) -> CapturedFile {
    validate_create_payload(p).unwrap();
    let candidate = plan_create_file(m, p).unwrap();
    let file = candidate.added[0].clone();
    assert!(
        m.commit_with_files(candidate.text, vec![], candidate.added)
            .unwrap()
    );
    file
}
fn state(m: &EditorSession) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}|{}|{}|{}",
        m.project.files,
        m.project.metadata,
        m.project.directories,
        m.project.layouts,
        m.project.parsed,
        m.parsed_hashes,
        m.syntax,
        m.diagnostics,
        m.project.id,
        m.revision,
        m.input_revision,
        m.dirty(),
        m.can_undo(),
        m.can_redo()
    )
}
fn rejected(m: &mut EditorSession, p: &Value) {
    let before = state(m);
    assert_eq!(plan_create_file(m, p).err().unwrap().status, 422, "{p}");
    assert_eq!(state(m), before);
}
fn remove_json(m: &mut EditorSession, role: FileRole) {
    let f = m
        .project
        .files
        .values()
        .find(|f| f.roles.contains(&role))
        .unwrap()
        .clone();
    m.project.files.remove(&f.id);
    m.project.metadata.remove(&f.path);
    m.project
        .directories
        .get_mut(f.path.parent().unwrap())
        .unwrap()
        .retain(|(n, _)| n != f.path.file_name().unwrap());
    let key = if role == FileRole::Workload {
        "workload"
    } else {
        "model-config"
    };
    let config = m
        .project
        .file_by_path(&m.project.config)
        .unwrap()
        .id
        .clone();
    let raw: String = m.project.files[&config]
        .text
        .split_inclusive('\n')
        .filter(|line| {
            !line
                .trim_start_matches('\u{feff}')
                .trim_start()
                .starts_with(&format!("{key} ="))
        })
        .collect();
    let f = m.project.files.get_mut(&config).unwrap();
    f.text = raw.into();
    f.hash = hash(f.text.as_bytes());
    m.project.refresh_inputs();
    *m = EditorSession::new(m.project.clone());
}
fn sync(m: &mut EditorSession) {
    let batch = analysis::parse_project(&m.project);
    m.adopt_analysis(batch, m.context_epoch);
}

#[test]
fn file_creation_seven_templates_and_encoding_use_common_parser() {
    for (id, kind) in [
        ("ned.module_empty", "module"),
        ("ned.network_empty", "network"),
        ("ned.controller", "simple"),
        ("ned.bus", "simple"),
        ("ned.channel_fixed_delay", "channel"),
    ] {
        for profile in ["can", "multibus"] {
            let mut m = model(profile);
            let before = m.project.input_digest();
            let prepared_before = analysis::prepare(&m.project).unwrap();
            let mut p = ned("pkg/deep/New.ned", "New");
            p["template_id"] = json!(id);
            p["declaration"]["kind"] = json!(kind);
            p["format"] = json!({"newline":"CRLF","bom":true});
            let candidate = plan_create_file(&m, &p).unwrap();
            assert_eq!(m.project.input_digest(), before);
            assert!(!m.can_undo());
            let f = candidate.added[0].clone();
            assert!(f.text.starts_with("\u{feff}package pkg.deep;\r\n"));
            assert!(!f.text.replace("\r\n", "").contains(['\r', '\n']));
            m.commit_with_files(candidate.text, vec![], candidate.added)
                .unwrap();
            sync(&mut m);
            let prepared_after = analysis::prepare(&m.project).unwrap();
            assert_eq!(
                format!("{:?}", prepared_before.can),
                format!("{:?}", prepared_after.can)
            );
            assert_eq!(
                format!("{:?}", prepared_before.gateway),
                format!("{:?}", prepared_after.gateway)
            );
            let shape = analysis::shapes(m.current_parse(&f.id).unwrap()).remove(0);
            assert_eq!(shape.name, "pkg.deep.New");
            assert_eq!(shape.kind, kind);
            if id == "ned.controller" || id == "ned.bus" {
                assert_eq!(
                    shape.implementation.unwrap(),
                    format!(
                        "dir.can.{}{}",
                        if profile == "multibus" {
                            "Multibus"
                        } else {
                            ""
                        },
                        if id == "ned.bus" { "Bus" } else { "Controller" }
                    )
                );
            }
            let mut without = m.project.clone();
            without.files.remove(&f.id);
            assert_eq!(without.input_digest(), before);
        }
    }
}

#[test]
fn file_creation_missing_json_and_ini_are_one_reversible_transaction() {
    for role in [FileRole::Workload, FileRole::ModelConfig] {
        let mut m = model("multibus");
        remove_json(&mut m, role.clone());
        let config = m
            .project
            .file_by_path(&m.project.config)
            .unwrap()
            .id
            .clone();
        let before_raw = format!(
            "\u{feff}{}# 日本語\n# mixed\r\n",
            m.project.files[&config].text.replace('\n', "\r\n")
        );
        {
            let f = m.project.files.get_mut(&config).unwrap();
            f.text = before_raw.clone().into();
            f.hash = hash(f.text.as_bytes());
        }
        m.project.refresh_inputs();
        m = EditorSession::new(m.project);
        let before = m.project.input_digest();
        let dirs = m.project.directories.clone();
        let meta = m.project.metadata.clone();
        let p = json_payload(
            if role == FileRole::Workload {
                "workload"
            } else {
                "model_config"
            },
            "extra/nested/new.json",
        );
        let f = apply(&mut m, &p);
        assert!(!f.path.exists());
        let edited = m.project.files[&config].text.clone();
        let key = if role == FileRole::Workload {
            "workload"
        } else {
            "model-config"
        };
        let unchanged: String = edited
            .split_inclusive('\n')
            .filter(|line| !line.starts_with(&format!("{key} =")))
            .collect();
        assert_eq!(unchanged, before_raw);
        analysis::prepare(&m.project).unwrap();
        m.undo().unwrap();
        assert_eq!(m.project.input_digest(), before);
        assert_eq!(m.project.directories, dirs);
        assert_eq!(m.project.metadata, meta);
        assert!(!m.can_undo());
        m.redo().unwrap();
        assert_eq!(m.project.files[&f.id].text, f.text);
        assert_eq!(m.project.files[&config].text, edited);
        analysis::prepare(&m.project).unwrap();
    }
}

#[test]
fn file_creation_nested_edit_undo_twice_redo_twice_has_no_residue() {
    let mut m = model("multibus-empty");
    assert!(analysis::prepare(&m.project).is_err());
    let before = m.project.input_digest();
    let metadata = m.project.metadata.clone();
    let directories = m.project.directories.clone();
    let parsed = m.project.parsed.keys().cloned().collect::<Vec<_>>();
    let layouts = m.project.layouts.keys().cloned().collect::<Vec<_>>();
    // Preserve an already captured empty directory.
    let root = m.project.header.roots[0].clone();
    let keep = root.join("keep");
    let source = SnapshotInputSource::virtual_project(BTreeMap::new(), std::slice::from_ref(&keep));
    m.project.metadata.extend(source.metadata);
    for (p, entries) in source.directories {
        let d = m.project.directories.entry(p).or_default();
        for e in entries {
            if !d.contains(&e) {
                d.push(e);
            }
        }
        d.sort_by(|a, b| a.0.cmp(&b.0));
    }
    let f = apply(&mut m, &ned("keep/one/two/New.ned", "New"));
    assert_eq!(m.syntax[&f.id], "pending");
    assert!(m.project.parsed.contains_key(&f.id));
    sync(&mut m);
    assert!(m.current_parse(&f.id).is_ok());
    let edited = f.text.replace("{}", "{\n // 日本語\n}");
    let mut layout = LayoutDocument::empty("New.ned".into());
    layout.types.insert(
        "keep.one.two.New".into(),
        TypeLayout {
            nodes: BTreeMap::from([("dummy".into(), Position::new(12.0, 34.0))]),
            ..Default::default()
        },
    );
    m.commit(
        vec![(f.id.clone(), edited.clone())],
        vec![(f.id.clone(), Some(layout.clone()))],
    )
    .unwrap();
    sync(&mut m);
    m.undo().unwrap();
    assert_eq!(m.project.files[&f.id].text, f.text);
    assert!(m.project.layouts[&f.id].adopted.is_none());
    sync(&mut m);
    m.diagnostics
        .push(json!({"origin":"layout","file_id":f.id,"message":"test"}));
    m.undo().unwrap();
    assert_eq!(m.project.input_digest(), before);
    assert_eq!(m.project.parsed.keys().cloned().collect::<Vec<_>>(), parsed);
    assert_eq!(
        m.project.layouts.keys().cloned().collect::<Vec<_>>(),
        layouts
    );
    assert!(!m.syntax.contains_key(&f.id));
    assert!(!m.parsed_hashes.contains_key(&f.id));
    assert!(!m.diagnostics.iter().any(|d| d["file_id"] == f.id));
    assert!(m.project.directories.contains_key(&keep));
    assert!(!m.project.directories.contains_key(&keep.join("one")));
    assert!(!m.project.metadata.contains_key(&f.path));
    for (p, k) in metadata {
        assert_eq!(m.project.metadata.get(&p), Some(&k));
    }
    for (p, e) in directories {
        for item in e {
            assert!(m.project.directories[&p].contains(&item));
        }
    }
    m.redo().unwrap();
    assert_eq!(m.project.files[&f.id].text, f.text);
    assert_eq!(m.syntax[&f.id], "pending");
    m.redo().unwrap();
    assert_eq!(m.project.files[&f.id].text.as_ref(), edited);
    assert_eq!(m.project.layouts[&f.id].adopted.as_ref(), Some(&layout));
    sync(&mut m);
    assert!(m.current_parse(&f.id).is_ok());
    assert!(!f.path.exists());
    assert!(analysis::prepare(&m.project).is_err());
}

#[test]
fn file_creation_roles_templates_paths_and_snapshot_collisions_are_atomic() {
    let mut m = model("can");
    for path in [
        "",
        "New.ned",
        "/pkg/New.ned",
        "pkg/../New.ned",
        "pkg/./New.ned",
        "pkg//New.ned",
        "pkg/New.json",
        "pkg/New.ned/",
        "pkg/\0New.ned",
        "C:/pkg/New.ned",
        "pkg\\New.ned",
        "network/New.ned",
    ] {
        rejected(&mut m, &ned(path, "New"));
    }
    for (key, v) in [
        ("role", json!("config")),
        ("root_id", json!("ned-root-999")),
        ("root_id", json!("ned-root-00")),
        ("template_id", json!("../../template")),
        ("unknown", json!(true)),
        ("format", json!({"newline":"LF","bom":false,"other":1})),
        ("format", json!({"newline":"other","bom":false})),
        ("declaration", json!({"kind":"network","local_name":"New"})),
        (
            "declaration",
            json!({"kind":"module","local_name":"network"}),
        ),
        (
            "declaration",
            json!({"kind":"module","local_name":"X; network Evil {}"}),
        ),
        (
            "declaration",
            json!({"kind":"module","local_name":"New","unknown":true}),
        ),
    ] {
        let mut p = ned("pkg/New.ned", "New");
        p[key] = v;
        rejected(&mut m, &p);
    }
    rejected(&mut m, &json_payload("workload", "new.json"));
    rejected(&mut m, &json_payload("model_config", "new.json"));
    for (path, kind) in [
        ("pkg", InputKind::Symlink),
        ("pkg", InputKind::File),
        ("pkg", InputKind::Other),
        ("pkg/New.ned", InputKind::Directory),
        ("pkg/New.ned.layout.json", InputKind::File),
        ("PKG", InputKind::Directory),
        ("pkg/NEW.NED", InputKind::File),
    ] {
        let mut s = model("can");
        s.project
            .metadata
            .insert(s.project.header.roots[0].join(path), kind);
        rejected(&mut s, &ned("pkg/New.ned", "New"));
    }
    let mut s = model("can");
    let id = s.project.ned_files().next().unwrap().id.clone();
    s.commit(vec![(id, "not NED".into())], vec![]).unwrap();
    rejected(&mut s, &ned("pkg/New.ned", "New"));
    let mut s = model("can");
    remove_json(&mut s, FileRole::Workload);
    rejected(&mut s, &json_payload("workload", "models/pkg/new.json"));
    let mut p = json_payload("workload", "new.json");
    p["root_id"] = Value::Null;
    rejected(&mut s, &p);
    let mut s = model("can");
    let config = s
        .project
        .file_by_path(&s.project.config)
        .unwrap()
        .id
        .clone();
    s.commit(
        vec![(config, "[General]\nunknown = broken\n".into())],
        vec![],
    )
    .unwrap();
    rejected(&mut s, &ned("pkg/New.ned", "New"));
}

#[test]
fn file_creation_distinct_roots_allow_same_relative_path_but_not_same_type() {
    let mut m = model("can");
    let config = m
        .project
        .file_by_path(&m.project.config)
        .unwrap()
        .id
        .clone();
    let raw = super::super::builtin::ini_values(
        &m.project.files[&config].text,
        &BTreeMap::from([("ned-path".into(), "\"models\";\"MODELS\"".into())]),
    )
    .unwrap();
    let f = m.project.files.get_mut(&config).unwrap();
    f.text = raw.into();
    f.hash = hash(f.text.as_bytes());
    m.project.refresh_inputs();
    let root0 = m.project.header.roots[0].clone();
    let bytes = m.project.ned_files().next().unwrap().text.clone();
    let first = apply(&mut m, &ned("pkg/New.ned", "First"));
    let mut p = ned("pkg/New.ned", "Second");
    p["root_id"] = json!("ned-root-1");
    let second = apply(&mut m, &p);
    assert_ne!(first.id, second.id);
    assert_eq!(first.path, root0.join("pkg/New.ned"));
    assert_eq!(second.path, m.project.header.roots[1].join("pkg/New.ned"));
    assert_eq!(
        m.project
            .ned_files()
            .find(|f| f.path.ends_with("Demo/Main.ned"))
            .unwrap()
            .text,
        bytes
    );
    let mut p = ned("pkg/Other.ned", "First");
    p["root_id"] = json!("ned-root-1");
    rejected(&mut m, &p);
    m.undo().unwrap();
    m.undo().unwrap();
    let fresh = plan_create_file(&m, &ned("pkg/New.ned", "First")).unwrap();
    assert_ne!(fresh.added[0].id, first.id);
}

#[test]
fn file_creation_commit_boundary_rejects_forged_roles_packages_and_partial_changes() {
    let mut m = model("can");
    let f = plan_create_file(&m, &ned("pkg/New.ned", "New"))
        .unwrap()
        .added
        .remove(0);
    for roles in [
        vec![FileRole::Config],
        vec![],
        vec![FileRole::Ned {
            root_index: 99,
            relative: PathBuf::from("pkg/New.ned"),
            package: "pkg".into(),
        }],
        vec![FileRole::Ned {
            root_index: 0,
            relative: PathBuf::from("pkg/New.ned"),
            package: "other".into(),
        }],
        vec![FileRole::Ned {
            root_index: 0,
            relative: PathBuf::from("../pkg/New.ned"),
            package: "pkg".into(),
        }],
        vec![FileRole::Workload],
        vec![
            FileRole::Ned {
                root_index: 0,
                relative: PathBuf::from("pkg/New.ned"),
                package: "pkg".into(),
            },
            FileRole::Workload,
        ],
    ] {
        let mut forged = f.clone();
        forged.roles = roles;
        let before = state(&m);
        assert!(m.commit_with_files(vec![], vec![], vec![forged]).is_err());
        assert_eq!(state(&m), before);
    }
    let before = state(&m);
    assert!(
        m.commit_with_files(vec![(f.id.clone(), "bad".into())], vec![], vec![f.clone()])
            .is_err()
    );
    assert_eq!(state(&m), before);
    let mut duplicate = f.clone();
    duplicate.id = random_id("file-").unwrap();
    assert!(
        m.commit_with_files(vec![], vec![], vec![f, duplicate])
            .is_err()
    );
    assert_eq!(state(&m), before);
}

#[test]
fn file_creation_save_checkpoint_undo_shape_redo_and_external_collision() {
    let mut m = model("can");
    let root = m.project.cwd.clone();
    std::fs::create_dir_all(root.join("exports")).unwrap();
    let mut registry =
        output::TargetRegistry::open(&root.join("exports"), &root.join("state")).unwrap();
    let f = apply(&mut m, &ned("pkg/New.ned", "New"));
    let target = registry.register_destination("saved").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    let report = output::save_plan(plan, &mut registry, None, false);
    assert_eq!(
        report.state,
        output::SaveOutcome::Complete,
        "{:?}",
        report.error
    );
    m.mark_saved(root.join("exports/saved").display().to_string());
    assert!(!m.dirty());
    let path = root.join("exports/saved/ned/0001/pkg/New.ned");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), f.text.as_ref());
    m.undo().unwrap();
    assert!(m.dirty());
    assert!(path.exists());
    assert_eq!(
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry)
            .err()
            .unwrap()
            .code,
        "E-TARGET-SHAPE"
    );
    m.redo().unwrap();
    assert!(!m.dirty());
    assert!(
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).is_ok()
    );
    let target = registry.register_destination("collision").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    let external = root.join("exports/collision");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("existing"), "external").unwrap();
    let report = output::save_plan(plan, &mut registry, None, false);
    assert_eq!(report.state, output::SaveOutcome::Rejected);
    assert_eq!(
        std::fs::read_to_string(external.join("existing")).unwrap(),
        "external"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_creation_history_exact_limit_and_oversized_rejection_preserve_redo() {
    let mut m = model("can");
    let original = plan_create_file(&m, &ned("pkg/New.ned", "New"))
        .unwrap()
        .added
        .remove(0);
    for size in [32 * 1024 * 1024 + 1, 32 * 1024 * 1024] {
        let mut f = original.clone();
        let mut raw = f.text.to_string();
        raw.push_str("//");
        raw.push_str(&"x".repeat(size - raw.len()));
        f.text = raw.into();
        f.origin_text = f.text.clone();
        f.hash = hash(f.text.as_bytes());
        f.origin_hash = f.hash.clone();
        let before = state(&m);
        let result = m.commit_with_files(vec![], vec![], vec![f]);
        if size > 32 * 1024 * 1024 {
            assert_eq!(result.unwrap_err().status, 413);
            assert_eq!(state(&m), before);
        } else {
            assert!(result.unwrap());
            m.undo().unwrap();
            assert!(m.can_redo());
        }
    }
    let before = state(&m);
    let mut f = original;
    f.text = format!("{}//{}", f.text, "x".repeat(32 * 1024 * 1024)).into();
    f.origin_text = f.text.clone();
    f.hash = hash(f.text.as_bytes());
    f.origin_hash = f.hash.clone();
    assert_eq!(
        m.commit_with_files(vec![], vec![], vec![f])
            .unwrap_err()
            .status,
        413
    );
    assert_eq!(state(&m), before);
    assert!(m.can_redo());
}

#[test]
fn file_creation_initial_layout_delta_is_reversible_and_json_path_is_quoted() {
    let mut m = model("can");
    let candidate = plan_create_file(&m, &ned("pkg/deep/New.ned", "New")).unwrap();
    let file = candidate.added[0].clone();
    let layout = LayoutDocument::empty("New.ned".into());
    m.commit_with_files(
        candidate.text,
        vec![(file.id.clone(), Some(layout.clone()))],
        candidate.added,
    )
    .unwrap();
    assert_eq!(m.project.layouts[&file.id].adopted.as_ref(), Some(&layout));
    m.undo().unwrap();
    assert!(!m.project.layouts.contains_key(&file.id));
    m.redo().unwrap();
    assert_eq!(m.project.layouts[&file.id].adopted.as_ref(), Some(&layout));
    remove_json(&mut m, FileRole::Workload);
    let p = json_payload("workload", "extra/流量 \"quoted\".json");
    let f = apply(&mut m, &p);
    assert_eq!(m.project.header.workload.as_ref(), Some(&f.path));
    analysis::prepare(&m.project).unwrap();
    let mut s = model("can");
    let root = s.project.header.roots[0].clone();
    s.project
        .metadata
        .insert(root.join("pkg/STRASSE.ned"), InputKind::File);
    rejected(&mut s, &ned("pkg/Straße.ned", "New"));
}

#[test]
fn file_creation_preserves_captured_parent_metadata_and_incomplete_save_writes_nothing() {
    let root = std::env::temp_dir().join(random_id("create-disk-test-").unwrap());
    std::fs::create_dir_all(root.join("input/models/demo/empty")).unwrap();
    std::fs::write(
        root.join("input/models/demo/Main.ned"),
        include_str!("../../../../../docs/verification/fixtures/can/models/demo/Main.ned"),
    )
    .unwrap();
    std::fs::write(
        root.join("input/project.ini"),
        "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\n",
    )
    .unwrap();
    let project =
        super::super::input::load_project(&root.join("input/project.ini"), &root).unwrap();
    let metadata = project.metadata.clone();
    let directories = project.directories.clone();
    let mut m = EditorSession::new(project);
    apply(&mut m, &json_payload("workload", "deep/sub/new.json"));
    m.undo().unwrap();
    assert_eq!(m.project.metadata, metadata);
    for (path, entries) in directories {
        assert_eq!(m.project.directories[&path], entries);
    }
    assert!(!m.project.directories.contains_key(&root.join("input/deep")));
    assert_eq!(
        m.project.metadata[&root.join("input")],
        InputKind::Directory
    );
    std::fs::remove_dir_all(root).unwrap();
    let mut m = model("multibus-empty");
    let f = apply(&mut m, &ned("pkg/New.ned", "New"));
    let root = m.project.cwd.clone();
    std::fs::create_dir_all(root.join("exports")).unwrap();
    let mut registry =
        output::TargetRegistry::open(&root.join("exports"), &root.join("state")).unwrap();
    let target = registry.register_destination("incomplete").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    let report = output::save_plan(plan, &mut registry, None, false);
    assert_eq!(report.state, output::SaveOutcome::Rejected);
    assert_eq!(report.error.unwrap().code, "E-EDITOR-SAVE-VALIDATION");
    assert!(!root.join("exports/incomplete").exists());
    assert!(!f.path.exists());
    assert!(registry.pending_recoveries().unwrap().is_empty());
    let main_id = m
        .project
        .ned_files()
        .find(|file| file.path.ends_with("Demo/Main.ned"))
        .unwrap()
        .id
        .clone();
    let completed = template::new_project("multibus", "Demo", &root)
        .unwrap()
        .ned_files()
        .next()
        .unwrap()
        .text
        .to_string();
    m.commit(vec![(main_id, completed)], vec![]).unwrap();
    sync(&mut m);
    analysis::prepare(&m.project).unwrap();
    assert_eq!(m.project.files[&f.id].id, f.id);
    let plan =
        output::build_plan(&m.project, m.revision, m.input_revision, &target, &registry).unwrap();
    let report = output::save_plan(plan, &mut registry, None, false);
    assert_eq!(
        report.state,
        output::SaveOutcome::Complete,
        "{:?}",
        report.error
    );
    assert_eq!(
        std::fs::read_to_string(root.join("exports/incomplete/ned/0001/pkg/New.ned")).unwrap(),
        f.text.as_ref()
    );
    std::fs::remove_dir_all(root).unwrap();
}
