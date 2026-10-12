//! Explicit cleanup authenticates every descendant before publishing a tombstone.
use super::*;
use storage::OwnedEntry;
fn read_inventory(dir: &Directory, name: &str) -> Result<Vec<OwnedEntry>> {
    serde_json::from_slice(&dir.read(name, CONTROL_LIMIT)?).map_err(|e| invalid(e.to_string()))
}
pub(super) fn reserve_viewer(shared: &Shared, record: &RunRecord, name: &str) -> Result<()> {
    let _state = shared.state.lock().unwrap();
    let control = verify_owner(shared, record)?.child("control")?;
    let mut names: BTreeMap<String, Option<Identity>> = if control
        .names()?
        .iter()
        .any(|n| n == "viewer-inventory.json")
    {
        serde_json::from_slice(&control.read("viewer-inventory.json", CONTROL_LIMIT)?)
            .map_err(|e| invalid(e.to_string()))?
    } else {
        BTreeMap::new()
    };
    if names.contains_key(name) {
        return Err(invalid("Viewer allocation already exists"));
    }
    names.insert(name.into(), None);
    control.replace("viewer-inventory.json", &encode(&names)?)
}
pub(super) fn finish_viewer(
    shared: &Shared,
    record: &RunRecord,
    name: &str,
    file: &File,
) -> Result<()> {
    let _state = shared.state.lock().unwrap();
    let control = verify_owner(shared, record)?.child("control")?;
    let mut names: BTreeMap<String, Option<Identity>> =
        serde_json::from_slice(&control.read("viewer-inventory.json", CONTROL_LIMIT)?)
            .map_err(|e| invalid(e.to_string()))?;
    if !names.contains_key(name) {
        return Err(invalid("Unknown Viewer allocation"));
    }
    let stat = rustix::fs::fstat(file).map_err(|e| EditorError::io(&shared.root, e))?;
    if names
        .get(name)
        .and_then(Option::as_ref)
        .is_some_and(|i| i.dev != stat.st_dev || i.ino != stat.st_ino)
    {
        return Err(invalid("Viewer allocation identity changed"));
    }
    names.insert(
        name.into(),
        Some(Identity {
            dev: stat.st_dev,
            ino: stat.st_ino,
        }),
    );
    control.replace("viewer-inventory.json", &encode(&names)?)
}
fn decimal_tail(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn random_tail(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn result_name(path: &str, id: &str, pid: Option<&str>) -> bool {
    let mut parts = path.split('/');
    let Some(first) = parts.next() else {
        return false;
    };
    let rest: Vec<_> = parts.collect();
    let files = [
        "results.json",
        "events.csv",
        "summary.csv",
        "diagnostics.jsonl",
        "manifest.json",
    ];
    if rest.is_empty() && (files.contains(&first) || first == ".dir-simulator.lock") {
        return true;
    }
    if first == format!(".tmp-{id}") {
        return rest.is_empty() || (rest.len() == 1 && files.contains(&rest[0]));
    }
    let Some(pid) = pid else {
        return false;
    };
    if let Some(tail) = first.strip_prefix(&format!(".dir-observations-{pid}-")) {
        return rest.is_empty() && decimal_tail(tail);
    }
    for prefix in [
        ".dir-result-sort-",
        ".dir-result-ordered-",
        ".dir-contribution-sort-",
    ] {
        if let Some(tail) = first.strip_prefix(&format!("{prefix}{pid}-")) {
            return decimal_tail(tail)
                && (rest.is_empty()
                    || (rest.len() == 1
                        && (rest[0] == "rows"
                            || rest[0].strip_prefix("run-").is_some_and(decimal_tail))));
        }
    }
    false
}
pub(super) fn approved(shared: &Shared, record: &RunRecord) -> Result<Vec<OwnedEntry>> {
    let dir = verify_owner(shared, record)?;
    let control = dir.child("control")?;
    let inputs = read_inventory(&control, "input-inventory.json")?;
    let viewers: BTreeMap<String, Option<Identity>> = if control
        .names()?
        .iter()
        .any(|n| n == "viewer-inventory.json")
    {
        serde_json::from_slice(&control.read("viewer-inventory.json", CONTROL_LIMIT)?)
            .map_err(|e| invalid(e.to_string()))?
    } else {
        BTreeMap::new()
    };
    let current = dir.inventory()?;
    for entry in &current {
        let allowed = match entry.path.split_once('/') {
            None => {
                (entry.path == "owner.json" && !entry.directory)
                    || (["input", "control", "result", "viewer"].contains(&entry.path.as_str())
                        && entry.directory)
            }
            Some(("input", path)) => inputs.iter().any(|known| {
                known.path == path
                    && known.identity == entry.identity
                    && known.directory == entry.directory
            }),
            Some(("control", name)) => {
                !entry.directory
                    && !name.contains('/')
                    && ([
                        "record.json",
                        "provenance.json",
                        "references.lock",
                        "input-inventory.json",
                        "viewer-inventory.json",
                        "worker-response.json",
                        "large-worker-response.json",
                        "stderr.log",
                        "stdout.log",
                    ]
                    .contains(&name)
                        || name.strip_prefix(".write-").is_some_and(random_tail))
            }
            Some(("viewer", name)) => {
                !entry.directory
                    && viewers.get(name).is_some_and(|expected| {
                        expected.as_ref().is_some_and(|i| i == &entry.identity)
                    })
            }
            Some(("result", path)) => {
                result_name(path, &record.run_id, record.worker_pid.as_deref())
            }
            _ => false,
        };
        if !allowed {
            return Err(invalid(format!(
                "Unknown owned cleanup descendant: {}",
                entry.path
            )));
        }
    }
    Ok(current)
}
pub(super) fn approved_stage(dir: &Directory, id: &str) -> Result<Vec<OwnedEntry>> {
    let current = dir.inventory()?;
    for entry in &current {
        let permitted = match entry.path.split_once('/') {
            None => {
                (entry.path == "owner.json" && !entry.directory)
                    || (["input", "control", "result", "viewer"].contains(&entry.path.as_str())
                        && entry.directory)
            }
            Some(("control", name)) => {
                !entry.directory
                    && !name.contains('/')
                    && ([
                        "record.json",
                        "provenance.json",
                        "references.lock",
                        "input-inventory.json",
                    ]
                    .contains(&name)
                        || name.strip_prefix(".write-").is_some_and(random_tail))
            }
            Some(("input", name)) => {
                name == "blobs"
                    || ["snapshot.json", "snapshot.commit.json"].contains(&name)
                    || name.strip_prefix(".write-").is_some_and(random_tail)
                    || name.strip_prefix("blobs/blob-").is_some_and(random_tail)
            }
            _ => false,
        };
        if !permitted {
            return Err(invalid(format!(
                "Unknown staged cleanup descendant for {id}: {}",
                entry.path
            )));
        }
    }
    Ok(current)
}
