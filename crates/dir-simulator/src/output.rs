//! Common metric aggregation and schema-1/schema-2 manifest-last publication.
use crate::snapshot::Snapshot;
use crate::types::{Diagnostic, PreparedSimulation};
use std::collections::BTreeMap;
use std::path::Path;

mod aggregate;
mod axi;
mod canfd;
mod csv;
mod ethernet;
mod json;
mod memory_ipc;
mod metadata;
mod model_records;
mod publish;
pub(crate) mod reservation;
mod soc;

const PROFILE: &str = "can.cc.ideal.v1";

/// Write the five required files within an already reserved empty output directory.
/// Completed data remains for diagnosis on publication failure; manifest is published last.
pub fn export(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    output: &Path,
) -> Result<(), Diagnostic> {
    let run_id = publish::run_id()?;
    let timestamp = metadata::utc_now();
    let (records, summary) = aggregate::records(prepared, snapshot)?;
    let result = json::result(prepared, snapshot, &run_id, &timestamp, &records, &summary)?;
    let version = if prepared.common.profile == PROFILE {
        1
    } else {
        2
    };
    let diagnostics = json::diagnostics(snapshot);
    let files = BTreeMap::from([
        ("diagnostics.jsonl", diagnostics),
        ("events.csv", csv::encode(&records, &run_id, version)),
        (
            "results.json",
            format!("{}\n", json::canonical(&result)).into_bytes(),
        ),
        ("summary.csv", csv::encode(&summary, &run_id, version)),
    ]);
    publish::publish(output, &run_id, version, snapshot, files)
}

#[cfg(test)]
mod tests;
