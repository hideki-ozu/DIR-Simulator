use super::*;

#[test]
#[ignore = "Requires freshly built same binary in DIR_NED_RUN_TEST_BINARY; run explicitly"]
fn xc26_real_worker_never_falls_back_to_an_uncaptured_existing_disk_file() {
    let binary =
        std::env::var_os("DIR_NED_RUN_TEST_BINARY").expect("Supply DIR_NED_RUN_TEST_BINARY");
    let mut f = Fixture::new();
    let disk = f.root.join("source/uncaptured.json");
    fs::copy(f.root.join("source/baseline.json"), &disk).unwrap();
    let disk_bytes = fs::read(&disk).unwrap();
    let ini = f
        .project
        .files
        .values_mut()
        .find(|file| file.path.ends_with("project.ini"))
        .unwrap();
    assert!(ini.text.contains("baseline.json"));
    let text = ini.text.replace("baseline.json", "uncaptured.json");
    ini.hash = hash(text.as_bytes());
    ini.text = std::sync::Arc::from(text);
    f.project.refresh_inputs();
    assert!(!f.project.files.values().any(|file| file.path == disk));
    // Disk preparation succeeds with the same changed INI; the captured provider
    // must still refuse the file, even though it exists and is otherwise valid.
    fs::write(
        f.root.join("source/project.ini"),
        f.project
            .files
            .values()
            .find(|file| file.path.ends_with("project.ini"))
            .unwrap()
            .text
            .as_bytes(),
    )
    .unwrap();
    crate::prepare(&f.root.join("source/project.ini")).unwrap();
    let expected = super::super::super::analysis::prepare(&f.project).unwrap_err();
    let manager = RunManager::open_impl(
        &f.root.join("state"),
        &f.project.input_regions(),
        Some(Path::new(&binary)),
    )
    .unwrap();
    let id = f.create(&manager, "uncaptured-disk-fallback");
    let record = await_real_worker_completion(&manager, &id);
    assert_eq!(record.phase, "prepare_failed", "{}", record.value());
    assert_eq!(record.artifact_state, "unpublished");
    assert!(record.report.is_none());
    assert_eq!(record.worker_exit.as_ref().unwrap()["reaped"], true);
    assert_eq!(record.diagnostics[0]["diagnostic"], expected.wire_value());
    assert_eq!(fs::read(&disk).unwrap(), disk_bytes);
    assert!(
        fs::read_dir(manager.shared.root.join(&id).join("result"))
            .unwrap()
            .next()
            .is_none()
    );
}
