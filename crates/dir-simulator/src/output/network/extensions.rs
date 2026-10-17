//! New profile summaries are reconstructed from committed records and occupied intervals.
use super::super::aggregate::{MetricValue, ratio};
use super::*;
use crate::types::network::PreparedNetwork;
pub(super) fn metadata(n: &PreparedNetwork, m: &mut Value) {
    let transfer = if n
        .extensions
        .as_ref()
        .is_some_and(|e| e.preemption.is_some())
    {
        "dir.preemption.transfer"
    } else {
        "ethernet.dynamic.transfer"
    };
    let mut references = vec![
        json!({"source_schema":"ethernet.dynamic.reception","field":"transfer_id","allowed_target_schemas":[transfer]}),
        json!({"source_schema":transfer,"field":"frame_id","allowed_target_schemas":["ethernet.dynamic.frame"]}),
        json!({"source_schema":transfer,"field":"parent_transfer_id","allowed_target_schemas":[transfer]}),
    ];
    let mut add = |source: &str, field: &str, targets: &[&str]| {
        references
            .push(json!({"source_schema":source,"field":field,"allowed_target_schemas":targets}))
    };
    if n.tsn.is_some() {
        add("ethernet.tsn.decision", "transfer_id", &[transfer]);
        add(
            "ethernet.tsn.policing",
            "reception_id",
            if n.bridge.is_some() {
                &["dir.composite.ingress", "ethernet.dynamic.reception"]
            } else {
                &["ethernet.dynamic.reception"]
            },
        );
    }
    if n.bridge.is_some() && n.dynamic.is_some() {
        for (source, field, target) in [
            (
                "dir.composite.ingress",
                "conversion_id",
                "dir.can_ethernet.conversion",
            ),
            (
                "dir.composite.ingress",
                "branch_id",
                "dir.can_ethernet.branch",
            ),
            (
                "dir.can_ethernet.conversion",
                "branch_ids[]",
                "dir.can_ethernet.branch",
            ),
            (
                "dir.can_ethernet.branch",
                "conversion_id",
                "dir.can_ethernet.conversion",
            ),
        ] {
            add(source, field, &[target]);
        }
        add(
            "dir.can_ethernet.conversion",
            "ingress_record",
            &["can.receiver", "ethernet.dynamic.reception"],
        );
        add(
            "dir.can_ethernet.branch",
            "child_id",
            &["can.request", "ethernet.dynamic.frame"],
        );
        add(
            "dir.can_ethernet.segment",
            "source_record_id",
            &["can.request", "ethernet.dynamic.frame"],
        );
        add(
            "dir.can_ethernet.segment",
            "targets[].completion_reception_id",
            &["can.receiver", "ethernet.dynamic.reception"],
        );
    }
    if n.extensions
        .as_ref()
        .is_some_and(|e| e.replication.is_some())
    {
        for (source, field, target) in [
            (
                "dir.redundancy.branch",
                "origin_id",
                "dir.redundancy.origin",
            ),
            (
                "dir.redundancy.branch",
                "recovery_id",
                "dir.redundancy.recovery",
            ),
            ("dir.redundancy.branch", "current_transfer_id", transfer),
            (
                "dir.redundancy.origin",
                "source_frame_id",
                "ethernet.dynamic.frame",
            ),
            (
                "dir.redundancy.origin",
                "branch_ids[]",
                "dir.redundancy.branch",
            ),
            (
                "dir.redundancy.origin",
                "delivered_branch_id",
                "dir.redundancy.branch",
            ),
            (
                "dir.redundancy.recovery",
                "input_branch",
                "dir.redundancy.branch",
            ),
            (
                "dir.redundancy.recovery",
                "origin_id",
                "dir.redundancy.origin",
            ),
        ] {
            add(source, field, &[target]);
        }
    }
    if n.extensions
        .as_ref()
        .is_some_and(|e| e.preemption.is_some())
    {
        for (source, field, target) in [
            (
                "dir.preemption.transfer",
                "fragment_ids[]",
                "dir.preemption.fragment",
            ),
            ("dir.preemption.transfer", "gap_ids[]", "dir.preemption.gap"),
            ("dir.preemption.fragment", "transfer_id", transfer),
            ("dir.preemption.gap", "transfer_id", transfer),
            (
                "dir.preemption.delivery",
                "frame_id",
                "ethernet.dynamic.frame",
            ),
            ("dir.preemption.delivery", "source_transfer_id", transfer),
            (
                "dir.preemption.delivery",
                "targets[].completion_reception_id",
                "ethernet.dynamic.reception",
            ),
            ("dir.preemption.decision", "active_transfer_id", transfer),
            ("dir.preemption.decision", "suspended_transfer_id", transfer),
        ] {
            add(source, field, &[target]);
        }
    }
    if n.extensions.is_some() {
        m["reference_catalog"] = json!(references);
    }

    if let Some(e) = &n.extensions {
        m["network_limits"] = json!({"max_pending_branches":e.limits.max_pending_branches,"max_control_events":e.limits.max_control_events});
        if let Some(r) = &e.replication {
            m["ethernet_redundancy"] = json!({"routing_rule":"static-circuit-no-fdb-no-stp","initial_sequence":"0","initial_history":"empty","shared_ports":r.shared_ports,"streams":r.streams.iter().map(|s|json!({"stream_id":s.stream_id,"source_generator":s.source_generator,"replicate_at":s.replicate_at,"eliminate_at":s.eliminate_at,"history_capacity":s.history_capacity.to_string(),"egress_paths":s.paths.iter().enumerate().map(|(i,p)|json!({"id":p.id,"index":i.to_string(),"ports":p.ports})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
        }
        if let Some(p) = &e.preemption {
            m["ethernet_preemption"] = json!({"fragment_unit":"serialized-mac-byte","fragment_overhead":"all-additional-occupancy","express_rule":"existing-preamble-ifg","gap_rule":"one-after-express-group","outputs":p.outputs.iter().map(|o|json!({"port":o.port,"express_classes":(0..8).filter(|p|o.express_mask&(1<<p)!=0).collect::<Vec<_>>(),"preemptable_classes":(0..8).filter(|p|o.preemptable_mask&(1<<p)!=0).collect::<Vec<_>>(),"fragment_payload_bytes":o.fragment_payload_bytes.to_string(),"fragment_overhead_bytes":o.fragment_overhead_bytes.to_string(),"resume_gap_ps":o.resume_gap_ps.to_string()})).collect::<Vec<_>>()});
            m["transfer_reference_schema"] = json!("dir.preemption.transfer/1");
        }
    }
}
fn number(v: &Value) -> Result<u128, Diagnostic> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Diagnostic::output("invalid network extension integer"))
}
fn total<'a>(mut rows: impl Iterator<Item = &'a Value>, field: &str) -> Result<u128, Diagnostic> {
    rows.try_fold(0u128, |n, v| {
        n.checked_add(number(&v[field])?)
            .ok_or_else(|| Diagnostic::output("network extension sum overflow"))
    })
}
fn interval(v: &Value, start: &str, end: &str, h: u64) -> Result<u128, Diagnostic> {
    let s = number(&v[start])?;
    let e = if v[end].is_null() {
        u128::from(h)
    } else {
        number(&v[end])?.min(u128::from(h))
    };
    Ok(e.saturating_sub(s))
}
pub(super) fn summary(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
    summary: &mut Vec<Record>,
) -> Result<(), Diagnostic> {
    let n = prepared
        .registered
        .as_ref()
        .unwrap()
        .network
        .as_ref()
        .unwrap();
    let Some(e) = &n.extensions else {
        return Ok(());
    };
    let h = snapshot.common.end_ps;
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    let schema = |name: &str| {
        records
            .values()
            .filter(|r| r.schema.name == name)
            .map(|r| &r.data)
            .collect::<Vec<_>>()
    };
    summary.retain(|r| r.metric != "ethernet.dropped");
    if let Some(ethernet) = &snapshot.ethernet {
        let targets = std::iter::once("$all")
            .chain(n.ethernet.directions.iter().map(|d| d.from_port.as_str()));
        for target in targets {
            let mut reasons = BTreeMap::<&str, u128>::new();
            for transfer in ethernet
                .transfers
                .iter()
                .filter(|t| t.status == "dropped" && (target == "$all" || t.from_port == target))
            {
                *reasons
                    .entry(transfer.drop_reason.as_deref().unwrap_or("queue_full"))
                    .or_default() += 1;
            }
            if reasons.is_empty() {
                reasons.insert("queue_full", 0);
            }
            for (reason, count) in reasons {
                let mut row = Record::aggregate(
                    target,
                    "ethernet.dropped",
                    MetricValue::Integer(count),
                    0,
                    h,
                );
                row.reason = Some(reason.into());
                summary.push(row);
            }
        }
    }
    let mut metrics = Vec::<(String, &str, u128)>::new();
    if let Some(replication) = &e.replication {
        let origins = schema("dir.redundancy.origin");
        let branches = schema("dir.redundancy.branch");
        let recoveries = schema("dir.redundancy.recovery");
        for s in &replication.streams {
            let mine = origins
                .iter()
                .copied()
                .filter(|r| r["stream_id"] == s.stream_id)
                .collect::<Vec<_>>();
            let target = total(mine.iter().copied(), "target_count")?;
            let completed = total(mine.iter().copied(), "completed_count")?;
            metrics.extend([
                (
                    s.stream_id.clone(),
                    "redundancy.origin.target_count",
                    target,
                ),
                (
                    s.stream_id.clone(),
                    "redundancy.origin.completed",
                    completed,
                ),
                (
                    s.stream_id.clone(),
                    "redundancy.branch.created",
                    branches
                        .iter()
                        .filter(|r| r["stream_id"] == s.stream_id)
                        .count() as u128,
                ),
                (
                    s.stream_id.clone(),
                    "redundancy.branch.dropped",
                    branches
                        .iter()
                        .filter(|r| r["stream_id"] == s.stream_id && r["status"] == "dropped")
                        .count() as u128,
                ),
            ]);
            for (decision, metric) in [
                ("accept_", "redundancy.recovery.accepted"),
                ("duplicate", "redundancy.recovery.duplicate"),
                ("stale", "redundancy.recovery.stale"),
            ] {
                metrics.push((
                    s.stream_id.clone(),
                    metric,
                    recoveries
                        .iter()
                        .filter(|r| {
                            r["stream_id"] == s.stream_id
                                && r["decision"]
                                    .as_str()
                                    .is_some_and(|s| s.starts_with(decision))
                        })
                        .count() as u128,
                ));
            }
            let source_ids = mine
                .iter()
                .filter_map(|r| r["source_frame_id"].as_str())
                .collect::<Vec<_>>();
            let mut mac = 0u128;
            let mut busy = 0u128;
            for t in records.values().filter(|r| {
                r.schema.name == "ethernet.dynamic.transfer"
                    && r.data["sof_ps"].is_string()
                    && r.data["frame_id"]
                        .as_str()
                        .is_some_and(|id| source_ids.contains(&id))
            }) {
                let m = number(&t.data["wire"]["mac_bytes"])?;
                mac = mac
                    .checked_add(m)
                    .ok_or_else(|| Diagnostic::output("branch bytes overflow"))?;
                busy = busy
                    .checked_add(interval(&t.data, "sof_ps", "release_ps", h)?)
                    .ok_or_else(|| Diagnostic::output("branch busy overflow"))?;
            }
            metrics.extend([
                (s.stream_id.clone(), "redundancy.branch.mac_bytes", mac),
                (s.stream_id.clone(), "redundancy.branch.busy_ps", busy),
            ]);
            summary.push(Record::aggregate(
                &s.stream_id,
                "redundancy.origin.completion_ratio",
                ratio(completed, target)?,
                0,
                h,
            ));
        }
    }
    if let Some(p) = &e.preemption {
        let fragments = schema("dir.preemption.fragment");
        let gaps = schema("dir.preemption.gap");
        let transfers = schema("dir.preemption.transfer");
        let deliveries = schema("dir.preemption.delivery");
        // Existing all-frame occupancy calculation includes suspension. Replace those
        // medium totals with exact fragment and express intervals for this profile.
        summary.retain(|r| {
            !matches!(
                r.metric.as_str(),
                "ethernet.busy_ps"
                    | "ethernet.link_utilization"
                    | "ethernet.mac_bits"
                    | "ethernet.wire_bits"
                    | "ethernet.occupied_bits"
            )
        });
        for o in &p.outputs {
            let fs = fragments
                .iter()
                .copied()
                .filter(|r| r["port"] == o.port)
                .collect::<Vec<_>>();
            let gs = gaps
                .iter()
                .copied()
                .filter(|r| r["port"] == o.port)
                .collect::<Vec<_>>();
            let ts = transfers
                .iter()
                .copied()
                .filter(|r| r["from_port"] == o.port)
                .collect::<Vec<_>>();
            let completed = fs
                .iter()
                .copied()
                .filter(|r| r["end_ps"].is_string())
                .collect::<Vec<_>>();
            let mut busy = 0u128;
            for f in &fs {
                busy = busy
                    .checked_add(interval(f, "start_ps", "end_ps", h)?)
                    .ok_or_else(|| Diagnostic::output("busy overflow"))?;
            }
            let mut gap = 0u128;
            for g in &gs {
                gap = gap
                    .checked_add(interval(g, "start_ps", "end_ps", h)?)
                    .ok_or_else(|| Diagnostic::output("gap overflow"))?;
            }
            for t in ts
                .iter()
                .filter(|t| t["kind"] == "express" && t["sof_ps"].is_string())
            {
                busy = busy
                    .checked_add(interval(t, "sof_ps", "release_ps", h)?)
                    .ok_or_else(|| Diagnostic::output("express busy overflow"))?;
            }
            let mut mac_bits = 0u128;
            let mut wire_bits = 0u128;
            let mut occupied_bits = 0u128;
            for t in &ts {
                let mac = number(&t["mac_bytes"])?;
                if t["eof_ps"].is_string() {
                    mac_bits = mac_bits
                        .checked_add(mac * 8)
                        .ok_or_else(|| Diagnostic::output("MAC bits overflow"))?;
                    if t["kind"] == "express" {
                        wire_bits = wire_bits
                            .checked_add((mac + 8) * 8)
                            .ok_or_else(|| Diagnostic::output("wire bits overflow"))?;
                    }
                }
                if t["kind"] == "express" && t["release_ps"].is_string() {
                    occupied_bits = occupied_bits
                        .checked_add((mac + 20) * 8)
                        .ok_or_else(|| Diagnostic::output("occupied bits overflow"))?;
                }
            }
            for f in &completed {
                let bits = (number(&f["payload_bytes"])? + number(&f["overhead_bytes"])?) * 8;
                wire_bits = wire_bits
                    .checked_add(bits)
                    .ok_or_else(|| Diagnostic::output("fragment bits overflow"))?;
                occupied_bits = occupied_bits
                    .checked_add(bits)
                    .ok_or_else(|| Diagnostic::output("fragment bits overflow"))?;
            }
            metrics.extend([
                (o.port.clone(), "ethernet.mac_bits", mac_bits),
                (o.port.clone(), "ethernet.wire_bits", wire_bits),
                (o.port.clone(), "ethernet.occupied_bits", occupied_bits),
            ]);
            metrics.extend([
                (
                    o.port.clone(),
                    "preemption.fragments.started",
                    fs.len() as u128,
                ),
                (
                    o.port.clone(),
                    "preemption.fragments.completed",
                    completed.len() as u128,
                ),
                (
                    o.port.clone(),
                    "preemption.frames.completed",
                    ts.iter().filter(|t| t["eof_ps"].is_string()).count() as u128,
                ),
                (
                    o.port.clone(),
                    "preemption.mac_bytes.completed",
                    total(ts.iter().copied(), "completed_mac_bytes")?,
                ),
                (
                    o.port.clone(),
                    "preemption.fragment_overhead_bytes",
                    total(fs.iter().copied(), "overhead_bytes")?,
                ),
                (o.port.clone(), "preemption.port.busy_ps", busy),
                (o.port.clone(), "preemption.port.resume_gap_ps", gap),
            ]);
            summary.push(Record::aggregate(
                &o.port,
                "ethernet.link_utilization",
                ratio(busy, u128::from(h))?,
                0,
                h,
            ));
        }
        metrics.extend([
            (
                "@network".into(),
                "preemption.delivery.target_count",
                total(deliveries.iter().copied(), "target_count")?,
            ),
            (
                "@network".into(),
                "preemption.delivery.completed",
                total(deliveries.iter().copied(), "completed_count")?,
            ),
        ]);
    }
    for (target, metric, n) in metrics {
        summary.push(Record::aggregate(
            &target,
            metric,
            MetricValue::Integer(n),
            0,
            h,
        ));
    }
    Ok(())
}

/// Legacy flow aggregates describe application deliveries. Preserve every raw
/// physical reception in registered output while selecting only accepted circuits here.
pub(super) fn aggregate_view(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<Option<Snapshot>, Diagnostic> {
    let network = prepared
        .registered
        .as_ref()
        .and_then(|r| r.network.as_ref());
    if network
        .and_then(|n| n.extensions.as_ref())
        .is_none_or(|e| e.replication.is_none())
    {
        return Ok(None);
    }
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    let mut view = Snapshot::empty(prepared);
    view.common.end_ps = snapshot.common.end_ps;
    view.common.partial = snapshot.common.partial;
    view.common.termination = snapshot.common.termination.clone();
    for point in snapshot.common.iter_points()? {
        view.common.points.push(point?);
    }
    view.registered = snapshot.registered.clone();
    view.network = snapshot.network.clone();
    view.ethernet = snapshot.ethernet.clone();
    let state = view.ethernet.as_mut().unwrap();
    let transfers = state
        .transfers
        .iter()
        .map(|t| (t.transfer_id.as_str(), t))
        .collect::<BTreeMap<_, _>>();
    state.receptions.retain(|r| {
        if r.status != "received" {
            return true;
        }
        let Some(origin) = records.values().find(|o| {
            o.schema.name == "dir.redundancy.origin" && o.data["source_frame_id"] == r.frame_id
        }) else {
            return true;
        };
        let Some(branch) = origin.data["delivered_branch_id"].as_str().and_then(|id| {
            records
                .values()
                .find(|b| b.schema.name == "dir.redundancy.branch" && b.id == id)
        }) else {
            return false;
        };
        let mut path = Vec::new();
        let mut current = Some(r.transfer_id.as_str());
        let mut visited = std::collections::BTreeSet::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return false;
            }
            let Some(t) = transfers.get(id) else {
                return false;
            };
            path.push(t.from_port.clone());
            current = t.parent_transfer_id.as_deref();
        }
        path.reverse();
        branch.data["path_ports"] == json!(path)
    });
    Ok(Some(view))
}
