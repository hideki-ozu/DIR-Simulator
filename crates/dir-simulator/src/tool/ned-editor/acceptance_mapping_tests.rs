//! Direct evidence for XC23's input identity/output correspondence. Normal
//! product saves use valid distinct roles. A separately labeled policy fixture
//! measures one captured ID with multiple roles without claiming valid capture
//! or joint schema support for workload and model-config JSON.
use super::analysis;
use super::input::{FileRole, load_project};
use super::model::EditorSession;
use super::output::{self, InputOutputMapping, RolePathMapping, SaveOutcome, TargetRegistry};
use super::{hash, random_id};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
    model: EditorSession,
    registry: TargetRegistry,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(random_id("ned-mapping-acceptance-").unwrap());
        fs::create_dir_all(root.join("input/models/demo")).unwrap();
        fs::create_dir(root.join("exports")).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for (source, destination) in [
            ("examples/can/baseline.json", "baseline.json"),
            ("examples/can/models/demo/Main.ned", "models/demo/Main.ned"),
        ] {
            fs::copy(repo.join(source), root.join("input").join(destination)).unwrap();
        }
        let ini = fs::read_to_string(repo.join("examples/can/baseline.ini")).unwrap();
        fs::write(
            root.join("input/project.ini"),
            format!("\u{feff}{}", ini.replace('\n', "\r\n")),
        )
        .unwrap();
        let project = load_project(Path::new("input/project.ini"), &root).unwrap();
        analysis::prepare(&project).unwrap();
        let mut registry =
            TargetRegistry::open(&root.join("exports"), &root.join("state")).unwrap();
        registry.register_source(&project).unwrap();
        Self {
            root,
            model: EditorSession::new(project),
            registry,
        }
    }
    fn destination(&mut self, name: &str) -> String {
        self.registry.register_destination(name).unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
    fn plan(&self, id: &str) -> output::SavePlan {
        output::build_plan(
            &self.model.project,
            self.model.revision,
            self.model.input_revision,
            id,
            &self.registry,
        )
        .unwrap()
    }
    fn restart(&self) -> TargetRegistry {
        TargetRegistry::open(&self.root.join("exports"), &self.root.join("state")).unwrap()
    }
    fn manifest(&self, id: &str) -> PathBuf {
        self.root
            .join("state/recovery")
            .join(id)
            .join("manifest.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn source_identity(m: &EditorSession) -> Value {
    json!({"snapshot_id":m.project.id,"config":m.project.config,"cwd":m.project.cwd,
        "roots":m.project.header.roots,"files":m.project.files.iter().map(|(id,f)|json!({"id":id,"path":f.path,"roles":f.roles,"hash":f.hash,"bytes":f.text.as_ref()})).collect::<Vec<_>>()})
}
fn expected_mapping(f: &Fixture, output: &Path) -> InputOutputMapping {
    f.model
        .project
        .files
        .iter()
        .map(|(id, file)| {
            let paths = file
                .roles
                .iter()
                .map(|role| RolePathMapping {
                    role: role.clone(),
                    output_path: match role {
                        FileRole::Config => output.join("project.ini"),
                        FileRole::Ned { .. } => output.join("ned/0001/demo/Main.ned"),
                        FileRole::Workload => output.join("data/workload.json"),
                        FileRole::ModelConfig => output.join("data/model-config.json"),
                    },
                })
                .collect();
            (id.clone(), paths)
        })
        .collect()
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, current: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !current.exists() {
            return;
        }
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(base).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn assert_published_bytes(f: &Fixture, root: &Path, mapping: &InputOutputMapping) {
    for (id, outputs) in mapping {
        let input = &f.model.project.files[id];
        for output in outputs {
            let expected = if output.role == FileRole::Config {
                input
                    .text
                    .replace("ned-path = \"models\"", "ned-path = \"ned/0001\"")
                    .replace(
                        "workload = \"baseline.json\"",
                        "workload = \"data/workload.json\"",
                    )
                    .replace(
                        "model-config = \"baseline.json\"",
                        "model-config = \"data/model-config.json\"",
                    )
                    .into_bytes()
            } else {
                input.text.as_bytes().to_vec()
            };
            assert_eq!(
                fs::read(&output.output_path).unwrap(),
                expected,
                "{:?}",
                output.role
            );
        }
    }
    let expected: BTreeSet<_> = mapping
        .values()
        .flatten()
        .map(|p| p.output_path.strip_prefix(root).unwrap().to_path_buf())
        .collect();
    assert_eq!(files(root).into_keys().collect::<BTreeSet<_>>(), expected);
}

#[test]
fn xc23_real_save_mapping_preserves_every_output_byte_and_snapshot_logical_path() {
    let mut f = Fixture::new();
    let shared = f
        .model
        .project
        .files
        .values()
        .find(|file| file.roles.contains(&FileRole::Workload))
        .unwrap();
    assert_eq!(shared.roles, vec![FileRole::Workload]);
    let shared_id = shared.id.clone();
    let original = source_identity(&f.model);
    let source_disk = files(&f.root.join("input"));
    let id = f.destination("mapped");
    let plan = f.plan(&id);
    let expected = expected_mapping(&f, &plan.output_path);
    assert_eq!(plan.mapping, expected);
    assert_eq!(plan.mapping[&shared_id].len(), 1);
    assert_eq!(plan.digest, f.plan(&id).digest);
    let output = plan.output_path.clone();
    let report = output::save_plan(plan, &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::Complete, "{:?}", report.error);
    assert_eq!(report.mapping, expected);
    assert_eq!(report.files.len(), 3);
    assert_published_bytes(&f, &output, &report.mapping);
    f.model.mark_saved(output.to_string_lossy().into_owned());
    assert_eq!(source_identity(&f.model), original);
    assert_eq!(files(&f.root.join("input")), source_disk);
    // The recovered identity is descriptive. Reloading a source file with a
    // different session-local ID must yield a newly bound map, not rename input.
    let mut reload = f.model.project.clone();
    let mut file = reload.files.remove(&shared_id).unwrap();
    file.id = "new-session-shared-json".into();
    reload.files.insert(file.id.clone(), file);
    let next = f.registry.register_destination("new-session").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let plan = output::build_plan(&reload, 1, 1, &next, &f.registry).unwrap();
    assert!(!plan.mapping.contains_key(&shared_id));
    assert_eq!(plan.mapping["new-session-shared-json"].len(), 1);
}

#[test]
fn xc23_multirole_mapping_policy_has_two_outputs_but_does_not_bypass_profile_validation() {
    let mut f = Fixture::new();
    let config_id = f
        .model
        .project
        .files
        .values()
        .find(|file| file.roles.contains(&FileRole::Config))
        .unwrap()
        .id
        .clone();
    let shared_id = f
        .model
        .project
        .files
        .values()
        .find(|file| file.roles.contains(&FileRole::Workload))
        .unwrap()
        .id
        .clone();
    // This is an explicit mapping-policy fixture, not a claim that a normal
    // profile accepts identical bytes for two incompatible JSON schemas.
    let config = f.model.project.files.get_mut(&config_id).unwrap();
    config.text = format!("{}model-config = \"baseline.json\"\r\n", config.text).into();
    config.hash = hash(config.text.as_bytes());
    f.model
        .project
        .files
        .get_mut(&shared_id)
        .unwrap()
        .roles
        .push(FileRole::ModelConfig);
    f.model.project.refresh_inputs();
    assert!(analysis::prepare(&f.model.project).is_err());
    let original = source_identity(&f.model);
    let id = f.destination("mapping-policy");
    let plan = f.plan(&id);
    let expected = expected_mapping(&f, &plan.output_path);
    assert_eq!(plan.mapping, expected);
    assert_eq!(plan.mapping[&shared_id].len(), 2);
    assert_ne!(
        plan.mapping[&shared_id][0].output_path,
        plan.mapping[&shared_id][1].output_path
    );
    assert_eq!(
        plan.mapping[&shared_id]
            .iter()
            .map(|m| m.role.clone())
            .collect::<Vec<_>>(),
        vec![FileRole::Workload, FileRole::ModelConfig]
    );
    let report = output::save_plan(plan, &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::Rejected);
    assert_eq!(report.error.unwrap().status, 422);
    assert_eq!(report.mapping, expected);
    assert!(files(&f.root.join("exports")).is_empty());
    assert_eq!(source_identity(&f.model), original);
}

#[test]
fn xc23_mapping_does_not_authorize_foreign_paths_or_duplicate_pairs() {
    for variant in ["foreign_path", "duplicate_pair", "missing_input"] {
        let mut f = Fixture::new();
        let id = f.destination("rejected");
        let mut plan = f.plan(&id);
        let shared = f
            .model
            .project
            .files
            .values()
            .find(|file| file.roles.contains(&FileRole::Workload))
            .unwrap()
            .id
            .clone();
        let sentinel = f.root.join("sentinel");
        fs::write(&sentinel, b"preserve").unwrap();
        match variant {
            "foreign_path" => {
                plan.mapping.get_mut(&shared).unwrap()[0].output_path = sentinel.clone()
            }
            "duplicate_pair" => {
                let duplicate = plan.mapping[&shared][0].clone();
                plan.mapping.get_mut(&shared).unwrap().push(duplicate);
            }
            _ => {
                plan.mapping.remove(&shared);
            }
        }
        let before = source_identity(&f.model);
        let report = output::save_plan(plan, &mut f.registry, None, false);
        assert_eq!(report.state, SaveOutcome::Rejected, "{variant}");
        assert_eq!(report.error.unwrap().status, 422);
        assert_eq!(fs::read(sentinel).unwrap(), b"preserve");
        assert!(files(&f.root.join("exports")).is_empty());
        assert_eq!(source_identity(&f.model), before);
    }
}

#[test]
fn xc23_recovery_keeps_mapping_and_legacy_absence_does_not_guess_input_ids() {
    for legacy in [false, true] {
        let mut f = Fixture::new();
        let id = f.destination("recover");
        let plan = f.plan(&id);
        let expected = plan.mapping.clone();
        let output = plan.output_path.clone();
        let original = source_identity(&f.model);
        output::inject_failure("after_publish");
        let interrupted = output::save_plan(plan, &mut f.registry, None, false);
        assert_eq!(
            interrupted.state,
            SaveOutcome::RecoveryRequired,
            "{:?}",
            interrupted.error
        );
        assert_eq!(interrupted.mapping, expected);
        let recovery = interrupted.recovery_id.unwrap();
        if legacy {
            let path = f.manifest(&recovery);
            let mut journal: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            journal.as_object_mut().unwrap().remove("mapping");
            // Actual legacy journals predate both provenance fields.
            journal.as_object_mut().unwrap().remove("plan_context");
            fs::write(path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
        }
        let mut registry = f.restart();
        let recovered = output::recover(&mut registry, &recovery, "complete");
        assert_eq!(
            recovered.state,
            SaveOutcome::Complete,
            "{:?}",
            recovered.error
        );
        if legacy {
            assert!(recovered.mapping.is_empty());
        } else {
            assert_eq!(recovered.mapping, expected);
        }
        assert_published_bytes(&f, &output, &expected);
        assert_eq!(source_identity(&f.model), original);
    }
}

#[test]
fn xc23_tampered_recovery_mapping_cannot_redirect_publication() {
    let mut f = Fixture::new();
    let id = f.destination("recover");
    let plan = f.plan(&id);
    output::inject_failure("after_publish");
    let report = output::save_plan(plan, &mut f.registry, None, false);
    assert_eq!(report.state, SaveOutcome::RecoveryRequired);
    let recovery = report.recovery_id.unwrap();
    let path = f.manifest(&recovery);
    let mut journal: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let first = journal["mapping"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    first[0]["output_path"] = json!(f.root.join("foreign"));
    fs::write(&path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    let before = files(&f.root.join("exports"));
    let mut registry = f.restart();
    let rejected = output::recover(&mut registry, &recovery, "complete");
    assert_eq!(rejected.state, SaveOutcome::Rejected);
    assert_eq!(rejected.error.unwrap().status, 422);
    assert_eq!(files(&f.root.join("exports")), before);
    assert!(!f.root.join("foreign").exists());
}
