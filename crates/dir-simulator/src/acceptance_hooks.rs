//! Private acknowledged fault barriers, present only in the unit-test executable.
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub(crate) const ENTRY: &str = "tool::ned_editor::run_control::acceptance_fault_tests::fault_child";
pub(crate) const WORKERS: &str = "DIR_ACCEPTANCE_WORKERS";

pub(crate) fn hit(site: &str) {
    hit_path(site, None);
}

pub(crate) fn hit_path(site: &str, path: Option<&Path>) {
    let Ok(selected) = std::env::var("DIR_ACCEPTANCE_SITE") else {
        return;
    };
    if selected != "*" && selected != site {
        return;
    }
    static COUNTS: OnceLock<Mutex<BTreeMap<String, usize>>> = OnceLock::new();
    let occurrence = {
        let mut counts = COUNTS.get_or_init(Default::default).lock().unwrap();
        let n = counts.entry(site.into()).or_default();
        let current = *n;
        *n += 1;
        current
    };
    if std::env::var("DIR_ACCEPTANCE_OCCURRENCE")
        .is_ok_and(|n| n != "*" && n.parse::<usize>().ok() != Some(occurrence))
    {
        return;
    }
    let socket = std::env::var_os("DIR_ACCEPTANCE_SOCKET").expect("barrier socket");
    let mut stream = UnixStream::connect(socket).expect("connect fault supervisor");
    stream
        .set_read_timeout(Some(Duration::from_secs(45)))
        .unwrap();
    let value = json!({"site":site,"occurrence":occurrence,"pid":std::process::id(),"path":path});
    writeln!(stream, "{value}").unwrap();
    let mut ack = [0];
    stream
        .read_exact(&mut ack)
        .expect("fault barrier requires acknowledged release");
    assert_eq!(ack[0], b'R');
}

pub(crate) fn propagate(command: &mut std::process::Command) {
    for key in [
        "DIR_ACCEPTANCE_SITE",
        "DIR_ACCEPTANCE_OCCURRENCE",
        "DIR_ACCEPTANCE_SOCKET",
        "DIR_ACCEPTANCE_ROOT",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}
