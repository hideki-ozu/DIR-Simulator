//! Common metric aggregation and schema-1/schema-2 manifest-last publication.
use crate::snapshot::Snapshot;
use crate::types::{Diagnostic, PreparedSimulation};
use std::path::Path;

mod aggregate;
mod axi;
mod canfd;
mod csv;
mod disk_sort;
mod ethernet;
mod json;
mod memory_ipc;
mod metadata;
mod model_records;
mod network;
mod preparation;
mod publish;
mod registered;
pub(crate) mod reservation;
mod soc;
mod stream;
pub(crate) use preparation::export_preparation_failure;
pub(crate) use preparation::profile_catalog;

const PROFILE: &str = "can.cc.ideal.v1";

/// Canonical diagnostic wire object shared by stderr and the persisted JSONL.
pub fn diagnostic_json(diagnostic: &Diagnostic) -> String {
    json::canonical(&diagnostic.wire_value())
}

pub(crate) fn new_run_identity() -> Result<crate::input::RunIdentity, Diagnostic> {
    Ok(crate::input::RunIdentity {
        run_id: publish::run_id()?,
        started_at_utc: metadata::utc_now(),
    })
}

/// Write the five required files within an already reserved empty output directory.
/// Completed data remains for diagnosis on publication failure; manifest is published last.
pub fn export(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    output: &Path,
) -> Result<(), Diagnostic> {
    let identity = prepared
        .common
        .run_identity
        .clone()
        .map(Ok)
        .unwrap_or_else(new_run_identity)?;
    let run_id = identity.run_id;
    let timestamp = identity.started_at_utc;
    if prepared.registered.as_ref().is_some_and(|r| r.is_generic()) {
        let version = prepared
            .registered
            .as_ref()
            .unwrap()
            .registry
            .profile(&prepared.common.profile)
            .unwrap()
            .output_schema_version;
        return publish::stream(output, &run_id, version, snapshot, |stage| {
            stage.write("results.json", |writer| {
                registered::write_result(writer, prepared, snapshot, &run_id, &timestamp)
            })?;
            stage.write("events.csv", |writer| {
                registered::write_csv(writer, prepared, snapshot, &run_id, version)
            })?;
            stage.write("summary.csv", |writer| {
                let summary = if prepared
                    .registered
                    .as_ref()
                    .is_some_and(|registered| registered.network.is_some())
                {
                    network::summary(prepared, snapshot)?
                } else {
                    Vec::new()
                };
                csv::write(writer, &summary, &run_id, version)
            })?;
            stage.write("diagnostics.jsonl", |writer| {
                writer
                    .write_all(&json::diagnostics(snapshot))
                    .map_err(|e| Diagnostic::output(format!("Cannot write diagnostics: {e}")))?;
                Ok(())
            })
        });
    }
    if snapshot.can.archive.is_some()
        && prepared.ethernet.is_none()
        && prepared.canfd.is_none()
        && prepared.axi.is_none()
        && prepared.soc.is_none()
        && prepared.memory_ipc.is_none()
    {
        let streamed = stream::prepare(prepared, snapshot, output)?;
        let version = if prepared.common.profile == PROFILE {
            1
        } else {
            2
        };
        return publish::stream(output, &run_id, version, snapshot, |stage| {
            stage.write("results.json", |writer| {
                json::write_stream_result(
                    writer, prepared, snapshot, &run_id, &timestamp, &streamed,
                )
            })?;
            stage.write("events.csv", |writer| {
                csv::write_stream(
                    writer,
                    streamed.records.iter()?.enumerate().map(|(i, row)| {
                        row.map(|row| {
                            let mut value = row.value;
                            value["seq"] = serde_json::json!(i.to_string());
                            value
                        })
                    }),
                    &run_id,
                    version,
                )
            })?;
            stage.write("summary.csv", |writer| {
                csv::write(writer, &streamed.summary, &run_id, version)
            })?;
            stage.write("diagnostics.jsonl", |writer| {
                writer
                    .write_all(&json::diagnostics(snapshot))
                    .map_err(|e| Diagnostic::output(format!("Cannot write diagnostics: {e}")))?;
                Ok(())
            })
        });
    }
    let (records, summary) = aggregate::records(prepared, snapshot)?;
    let version = if prepared.common.profile == PROFILE {
        1
    } else {
        2
    };
    publish::stream(output, &run_id, version, snapshot, |stage| {
        stage.write("results.json", |writer| {
            json::write_result(
                writer, prepared, snapshot, &run_id, &timestamp, &records, &summary,
            )
        })?;
        stage.write("events.csv", |writer| {
            csv::write(writer, &records, &run_id, version)
        })?;
        stage.write("summary.csv", |writer| {
            csv::write(writer, &summary, &run_id, version)
        })?;
        stage.write("diagnostics.jsonl", |writer| {
            writer
                .write_all(&json::diagnostics(snapshot))
                .map_err(|e| Diagnostic::output(format!("Cannot write diagnostics: {e}")))?;
            Ok(())
        })
    })
}

#[cfg(test)]
mod tests;
