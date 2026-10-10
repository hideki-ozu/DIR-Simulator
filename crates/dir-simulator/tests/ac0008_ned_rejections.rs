//! Stable AC0008-NED-NEG cases. These assertions cover rejection and source,
//! not the full DIR-TEST-0061 reason/target/callback contract.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = std::env::temp_dir().join(format!(
            "dir-ac0008-ned-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(directory.join("models/demo")).unwrap();
        fs::copy(root.join("docs/verification/fixtures/can/models/demo/Main.ned"), directory.join("models/demo/Main.ned")).unwrap();
        fs::write(directory.join("scenario.ini"), "[General]\nnetwork = demo.Main\nned-path = \"models\"\nsim-time-limit = 1ms\nMain.bus.bitrate = 500kbps\n").unwrap();
        Self(directory)
    }
    fn reject(&self, mutation: impl FnOnce(String) -> String) {
        let path = self.0.join("models/demo/Main.ned");
        let input = fs::read_to_string(&path).unwrap();
        let changed = mutation(input.clone());
        assert_ne!(changed, input, "mutation must actually change the fixture");
        fs::write(&path, changed).unwrap();
        let diagnostic = match dir_simulator::prepare(&self.0.join("scenario.ini")) {
            Ok(_) => panic!("unsupported or invalid NED was accepted"),
            Err(diagnostic) => diagnostic,
        };
        assert_eq!(diagnostic.code, "E-0001");
        assert!(!diagnostic.reason.is_empty());
        assert!(diagnostic.source.as_ref().unwrap().ends_with("Main.ned"));
        assert!(diagnostic.line.is_some());
        assert!(diagnostic.column.is_some());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}
#[test]
fn import_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(|s| s.replacen("package demo;", "package demo;\nimport demo.*;", 1));
}
#[test]
fn inheritance_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(|s| s.replacen("network Main", "network Main extends demo.Base", 1));
}
#[test]
fn vector_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(|s| s.replacen("a: demo.Controller", "a[2]: demo.Controller", 1));
}
#[test]
fn unqualified_child_type_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(|s| s.replacen("a: demo.Controller", "a: Controller", 1));
}
#[test]
fn inout_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(|s| s.replacen("output tx;", "inout tx;", 1));
}
#[test]
fn allowunconnected_is_explicitly_rejected_with_ned_source() {
    Fixture::new().reject(|s| s.replacen("connections:", "connections allowunconnected:", 1));
}
