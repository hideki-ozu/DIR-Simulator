//! Profile-local typed reference closure and occupied-interval validation.
use super::*;
use std::collections::BTreeSet;
fn fail(message: impl Into<String>) -> Diagnostic {
    Diagnostic::output(format!("network extension invariant: {}", message.into()))
}
fn exact(v: &Value, fields: &str) -> Result<(), Diagnostic> {
    let keys = fields.split_whitespace().collect::<BTreeSet<_>>();
    let Some(object) = v.as_object() else {
        return Err(fail("expected object"));
    };
    if object.len() != keys.len() || object.keys().any(|k| !keys.contains(k.as_str())) {
        return Err(fail(format!("exact fields required: {fields}")));
    }
    Ok(())
}
fn number(v: &Value) -> Result<u64, Diagnostic> {
    let s = v.as_str().ok_or_else(|| fail("expected integer string"))?;
    if s.is_empty() || s.len() > 1 && s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(fail("noncanonical integer"));
    }
    s.parse().map_err(|_| fail("integer exceeds u64"))
}
fn reference<'a>(
    records: &'a BTreeMap<String, crate::registry::ModelRecord>,
    schema: &str,
    id: &Value,
) -> Result<&'a crate::registry::ModelRecord, Diagnostic> {
    let id = id.as_str().ok_or_else(|| fail("missing reference ID"))?;
    records
        .values()
        .find(|r| r.schema.name == schema && r.id == id)
        .ok_or_else(|| fail(format!("unresolved {schema} reference {id}")))
}
fn ids(value: &Value) -> Result<&Vec<Value>, Diagnostic> {
    value
        .as_array()
        .ok_or_else(|| fail("expected reference array"))
}
const BRANCH: &str = "branch_id origin_id stream_id sequence path_id path_index path_ports hop_index current_transfer_id created_ps first_sof_ps completed_ps status reason recovery_id effect_seq";
const ORIGIN: &str = "origin_id source_frame_id stream_id sequence created_ps branch_ids target_count completed_count target first_sof_ps completed_ps delivered_branch_id effect_seq";
const RECOVERY: &str = "recovery_id input_branch origin_id stream_id sequence eliminate_at decision previous_watermark new_watermark history_capacity bitmap_before_hex bitmap_after_hex delivered effect_seq";
const TRANSFER: &str = "transfer_id frame_id parent_transfer_id queue_id priority wire vlan_id from_port to_port queued_ps sof_ps eof_ps release_ps arrival_ps planned_eof_ps planned_release_ps planned_arrival_ps status drop_reason kind visit_id copy_id policy_epoch_offer policy_epoch_sof mac_bytes completed_mac_bytes fragment_ids gap_ids occupied_ps effect_seq";
const FRAGMENT: &str = "fragment_id transfer_id frame_id port ordinal offset_bytes payload_bytes overhead_bytes bitrate_bps start_ps planned_end_ps end_ps tx_epoch status effect_seq";
const GAP: &str =
    "gap_id transfer_id port start_ps planned_end_ps end_ps gap_epoch status effect_seq";
const DELIVERY: &str =
    "frame_id source_transfer_id first_sof_ps target_count targets completed_count effect_seq";
const DECISION: &str = "port state wait_reason active_transfer_id suspended_transfer_id offset_bytes remaining_bytes next_wake_ps tx_epoch gap_epoch wake_epoch effect_seq";
const INGRESS: &str = "ingress_id conversion_id branch_id ingress child_planned_id stream_id vid priority mac_bytes offered_ps verdict reason effect_seq";
pub(super) fn validate(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<(), Diagnostic> {
    let n = prepared
        .registered
        .as_ref()
        .unwrap()
        .network
        .as_ref()
        .unwrap();
    if n.extensions.is_none() {
        return Ok(());
    }
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    let h = snapshot.common.end_ps;
    let mut intervals = BTreeMap::<String, Vec<(u64, u64, bool)>>::new();
    for record in records.values() {
        let data = &record.data;
        let fields = match record.schema.name.as_str() {
            "dir.redundancy.branch" => BRANCH,
            "dir.redundancy.origin" => ORIGIN,
            "dir.redundancy.recovery" => RECOVERY,
            "dir.preemption.transfer" => TRANSFER,
            "dir.preemption.fragment" => FRAGMENT,
            "dir.preemption.gap" => GAP,
            "dir.preemption.delivery" => DELIVERY,
            "dir.preemption.decision" => DECISION,
            "dir.composite.ingress" => INGRESS,
            _ => continue,
        };
        exact(data, fields)?;
        number(&data["effect_seq"])?;
        match record.schema.name.as_str() {
            "dir.redundancy.branch" => {
                reference(records, "dir.redundancy.origin", &data["origin_id"])?;
                if !data["current_transfer_id"].is_null() {
                    reference(
                        records,
                        "ethernet.dynamic.transfer",
                        &data["current_transfer_id"],
                    )?;
                }
                if !data["recovery_id"].is_null() {
                    reference(records, "dir.redundancy.recovery", &data["recovery_id"])?;
                }
                if number(&data["hop_index"])? as usize >= ids(&data["path_ports"])?.len() {
                    return Err(fail("branch hop out of path"));
                }
            }
            "dir.redundancy.origin" => {
                reference(records, "ethernet.dynamic.frame", &data["source_frame_id"])?;
                for id in ids(&data["branch_ids"])? {
                    let b = reference(records, "dir.redundancy.branch", id)?;
                    if b.data["origin_id"] != data["origin_id"] {
                        return Err(fail("branch origin mismatch"));
                    }
                }
                let target = number(&data["target_count"])?;
                let completed = number(&data["completed_count"])?;
                if target > 1
                    || completed > target
                    || (target == 0) != data["target"].is_null()
                    || (completed == 0) != data["delivered_branch_id"].is_null()
                {
                    return Err(fail("origin opportunity mismatch"));
                }
                if completed == 1 {
                    let b = reference(
                        records,
                        "dir.redundancy.branch",
                        &data["delivered_branch_id"],
                    )?;
                    if b.data["status"] != "delivered" {
                        return Err(fail("successful origin missing delivered branch"));
                    }
                }
            }
            "dir.redundancy.recovery" => {
                reference(records, "dir.redundancy.branch", &data["input_branch"])?;
                reference(records, "dir.redundancy.origin", &data["origin_id"])?;
                let accepted = data["decision"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("accept_"));
                if data["delivered"].as_bool() != Some(accepted) {
                    return Err(fail("recovery delivery mismatch"));
                }
                let capacity = number(&data["history_capacity"])?;
                for field in ["bitmap_before_hex", "bitmap_after_hex"] {
                    let text = data[field].as_str().ok_or_else(|| fail("missing bitmap"))?;
                    if text.len() != capacity.div_ceil(4) as usize
                        || !text
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    {
                        return Err(fail("bitmap width/encoding mismatch"));
                    }
                }
            }
            "dir.preemption.transfer" => {
                reference(records, "ethernet.dynamic.frame", &data["frame_id"])?;
                if !data["parent_transfer_id"].is_null() {
                    reference(
                        records,
                        "dir.preemption.transfer",
                        &data["parent_transfer_id"],
                    )?;
                }
                for id in ids(&data["fragment_ids"])? {
                    let r = reference(records, "dir.preemption.fragment", id)?;
                    if r.data["transfer_id"] != data["transfer_id"] {
                        return Err(fail("fragment owner mismatch"));
                    }
                }
                for id in ids(&data["gap_ids"])? {
                    reference(records, "dir.preemption.gap", id)?;
                }
                if number(&data["completed_mac_bytes"])? > number(&data["mac_bytes"])? {
                    return Err(fail("completed bytes exceed MAC"));
                }
                if data["kind"] == "express" && data["sof_ps"].is_string() {
                    let start = number(&data["sof_ps"])?;
                    let planned = number(&data["planned_release_ps"])?;
                    let end = if data["release_ps"].is_null() {
                        h.min(planned)
                    } else {
                        number(&data["release_ps"])?
                    };
                    intervals
                        .entry(data["from_port"].as_str().unwrap().into())
                        .or_default()
                        .push((start, end, false));
                }
            }
            "dir.preemption.fragment" | "dir.preemption.gap" => {
                let t = reference(records, "dir.preemption.transfer", &data["transfer_id"])?;
                if t.data["from_port"] != data["port"] {
                    return Err(fail("interval port mismatch"));
                }
                let start = number(&data["start_ps"])?;
                let planned = number(&data["planned_end_ps"])?;
                if planned < start {
                    return Err(fail("reversed interval"));
                }
                let end = if data["end_ps"].is_null() {
                    h.min(planned)
                } else {
                    let end = number(&data["end_ps"])?;
                    if end != planned || end > h {
                        return Err(fail("actual/planned interval mismatch"));
                    }
                    end
                };
                let gap = record.schema.name == "dir.preemption.gap";
                if !gap {
                    let offset = number(&data["offset_bytes"])?;
                    let payload = number(&data["payload_bytes"])?;
                    if payload == 0
                        || offset
                            .checked_add(payload)
                            .is_none_or(|n| n > number(&t.data["mac_bytes"]).unwrap_or(0))
                    {
                        return Err(fail("invalid MAC slice"));
                    }
                }
                intervals
                    .entry(data["port"].as_str().unwrap().into())
                    .or_default()
                    .push((start, end, gap));
            }
            "dir.preemption.delivery" => {
                reference(records, "ethernet.dynamic.frame", &data["frame_id"])?;
                if !data["source_transfer_id"].is_null() {
                    reference(
                        records,
                        "dir.preemption.transfer",
                        &data["source_transfer_id"],
                    )?;
                }
                let targets = ids(&data["targets"])?;
                if targets.len() as u64 != number(&data["target_count"])?
                    || targets
                        .iter()
                        .filter(|t| t["completed_ps"].is_string())
                        .count() as u64
                        != number(&data["completed_count"])?
                {
                    return Err(fail("delivery target mismatch"));
                }
                for target in targets {
                    exact(target, "terminal_id completed_ps completion_reception_id")?;
                    if !target["completion_reception_id"].is_null() {
                        let r = reference(
                            records,
                            "ethernet.dynamic.reception",
                            &target["completion_reception_id"],
                        )?;
                        if r.data["frame_id"] != data["frame_id"]
                            || r.data["ready_ps"] != target["completed_ps"]
                        {
                            return Err(fail("terminal reception mismatch"));
                        }
                    }
                }
            }
            "dir.preemption.decision" => {
                for field in ["active_transfer_id", "suspended_transfer_id"] {
                    if !data[field].is_null() {
                        reference(records, "dir.preemption.transfer", &data[field])?;
                    }
                }
            }
            "dir.composite.ingress" => {
                reference(
                    records,
                    "dir.can_ethernet.conversion",
                    &data["conversion_id"],
                )?;
                let b = reference(records, "dir.can_ethernet.branch", &data["branch_id"])?;
                if b.data["planned_child_id"] != data["child_planned_id"] {
                    return Err(fail("synthetic child mismatch"));
                }
            }
            _ => {}
        }
    }
    for (port, mut spans) in intervals {
        spans.retain(|(start, end, _)| end > start);
        spans.sort_unstable();
        for adjacent in spans.windows(2) {
            if adjacent[0].1 > adjacent[1].0 {
                return Err(fail(format!("overlapping busy/gap intervals on {port}")));
            }
        }
    }
    if n.tsn.is_some() {
        for r in records
            .values()
            .filter(|r| r.schema.name == "ethernet.tsn.policing")
        {
            let id = &r.data["reception_id"];
            let found = reference(records, "ethernet.dynamic.reception", id).is_ok()
                || n.bridge.is_some() && reference(records, "dir.composite.ingress", id).is_ok();
            if !found {
                return Err(fail("unresolved policing input"));
            }
        }
    }
    Ok(())
}
