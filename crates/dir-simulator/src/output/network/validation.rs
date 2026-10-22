//! Profile-local typed reference closure and occupied-interval validation.
use super::*;
use std::collections::BTreeSet;
mod services;
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
fn transfer_reference<'a>(
    n: &crate::types::network::PreparedNetwork,
    records: &'a BTreeMap<String, crate::registry::ModelRecord>,
    id: &Value,
) -> Result<&'a crate::registry::ModelRecord, Diagnostic> {
    let id = id
        .as_str()
        .ok_or_else(|| fail("missing transfer reference ID"))?;
    let mut matches = records.values().filter(|r| {
        r.id == id
            && matches!(
                r.schema.name.as_str(),
                "ethernet.dynamic.transfer" | "dir.preemption.transfer"
            )
    });
    let record = matches
        .next()
        .ok_or_else(|| fail(format!("unresolved transfer reference {id}")))?;
    if matches.next().is_some() {
        return Err(fail("ambiguous transfer reference"));
    }
    let direction = n
        .ethernet
        .directions
        .iter()
        .position(|d| Some(d.from_port.as_str()) == record.data["from_port"].as_str())
        .ok_or_else(|| fail("transfer reference output missing"))?;
    let preemption = n
        .extensions
        .as_ref()
        .and_then(|e| e.preemption.as_ref())
        .is_some_and(|p| p.participating[direction]);
    if (record.schema.name == "dir.preemption.transfer") != preemption {
        return Err(fail("transfer schema differs from its MAC owner"));
    }
    Ok(record)
}
fn ids(value: &Value) -> Result<&Vec<Value>, Diagnostic> {
    value
        .as_array()
        .ok_or_else(|| fail("expected reference array"))
}
const BRANCH: &str = "branch_id origin_id stream_id sequence path_id path_index path_ports hop_index current_transfer_id created_ps first_sof_ps completed_ps status reason recovery_id effect_seq";
const ORIGIN: &str = "origin_id source_frame_id stream_id sequence created_ps branch_ids target_count completed_count target first_sof_ps completed_ps delivered_branch_id effect_seq";
const RECOVERY: &str = "recovery_id input_branch origin_id stream_id sequence eliminate_at decision previous_watermark new_watermark history_capacity bitmap_before_hex bitmap_after_hex delivered effect_seq";
const DISTRIBUTED_ORIGIN: &str = "origin_id placement source_frame_id source_frame_index stream_id sequence created_ps first_sof_ps prefix_reception_id winner_reception_id continuation_created continuation_ps suffix_root delivery_reception_id completed_ps completed_count target_count target prefix_ports branch_ports suffix_ports dropped prefix branches suffix effect_seq";
const DISTRIBUTED_RECOVERY: &str = "recovery_id placement input_branch reception_id origin_id stream_id sequence eliminate_at decision window_decision continuation_created suppression_reason previous_watermark new_watermark history_capacity bitmap_before_hex bitmap_after_hex delivered effect_seq";
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
    services::validate(n, snapshot)?;
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
            "dir.redundancy.origin" if data["placement"] == "bridge_pair_v1" => DISTRIBUTED_ORIGIN,
            "dir.redundancy.origin" => ORIGIN,
            "dir.redundancy.recovery" if data["placement"] == "bridge_pair_v1" => {
                DISTRIBUTED_RECOVERY
            }
            "dir.redundancy.recovery" => RECOVERY,
            "dir.preemption.transfer" => TRANSFER,
            "dir.preemption.fragment" => FRAGMENT,
            "dir.preemption.gap" => GAP,
            "dir.preemption.delivery" => DELIVERY,
            "dir.preemption.decision" => DECISION,
            "dir.composite.ingress" => INGRESS,
            _ => continue,
        };
        let normative = n
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .is_some_and(|r| {
                r.streams.iter().any(|stream| {
                    stream.stream_id == data["stream_id"]
                        && stream.recovery_algorithm
                            == crate::types::network::ReplicationRecoveryAlgorithm::Vector8021cb2017
                })
            });
        let fields = if normative
            && record.schema.name == "dir.redundancy.origin"
            && data["placement"] == "bridge_pair_v1"
        {
            format!("{fields} continuation_rounds accepted_count")
        } else if normative && record.schema.name == "dir.redundancy.origin" {
            format!("{fields} delivery_indications")
        } else if normative
            && record.schema.name == "dir.redundancy.recovery"
            && data["placement"] != "bridge_pair_v1"
        {
            format!("{fields} reception_id")
        } else {
            fields.into()
        };
        let mut fields = fields;
        let additions: &[&str] = match record.schema.name.as_str() {
            "dir.preemption.transfer" => &["partial_mac_bytes", "reset_ps"],
            "dir.preemption.fragment" => &["emitted_payload_bytes"],
            _ => &[],
        };
        for field in additions {
            if data.get(field).is_some() {
                fields.push(' ');
                fields.push_str(field);
            }
        }
        if record.schema.name == "dir.preemption.decision" {
            for field in ["wire_control_id", "active_transfer_index"] {
                if data.get(field).is_some() {
                    fields.push(' ');
                    fields.push_str(field);
                }
            }
        }
        exact(data, &fields)?;
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
            "dir.redundancy.origin" if data["placement"] == "bridge_pair_v1" => {
                reference(records, "ethernet.dynamic.frame", &data["source_frame_id"])?;
                let stream = n
                    .extensions
                    .as_ref()
                    .and_then(|e| e.replication.as_ref())
                    .and_then(|r| r.streams.iter().find(|s| s.stream_id == data["stream_id"]))
                    .ok_or_else(|| fail("distributed stream missing"))?;
                let crate::types::network::ReplicationPlacement::BridgePair(pair) =
                    &stream.placement
                else {
                    return Err(fail("distributed placement missing"));
                };
                if data["prefix_ports"] != json!(pair.prefix.ports)
                    || data["suffix_ports"] != json!(pair.suffix.ports)
                    || data["branch_ports"]
                        != json!(stream.paths.iter().map(|p| &p.ports).collect::<Vec<_>>())
                {
                    return Err(fail("distributed resolved paths differ"));
                }
                for key in [
                    "prefix_reception_id",
                    "winner_reception_id",
                    "delivery_reception_id",
                ] {
                    if !data[key].is_null() {
                        reference(records, "ethernet.dynamic.reception", &data[key])?;
                    }
                }
                let delivered = !data["delivery_reception_id"].is_null();
                if (!normative && number(&data["completed_count"])? != u64::from(delivered))
                    || data["continuation_created"].as_bool()
                        != Some(!data["winner_reception_id"].is_null())
                {
                    return Err(fail("distributed continuation/delivery marker"));
                }
                if delivered && !normative {
                    let rx = reference(
                        records,
                        "ethernet.dynamic.reception",
                        &data["delivery_reception_id"],
                    )?;
                    if !n.ethernet.directions.iter().any(|d| {
                        d.destination == pair.destination_device && d.to_port == rx.data["ingress"]
                    }) || rx.data["status"] != "received"
                        || rx.data["ready_ps"] != data["completed_ps"]
                    {
                        return Err(fail("distributed actual destination evidence"));
                    }
                }
                if normative {
                    let rounds = ids(&data["continuation_rounds"])?;
                    let accepted = records
                        .values()
                        .filter(|r| {
                            r.schema.name == "dir.redundancy.recovery"
                                && r.data["origin_id"] == record.id
                                && r.data["continuation_created"] == true
                        })
                        .collect::<Vec<_>>();
                    if rounds.len() != accepted.len()
                        || number(&data["accepted_count"])? != rounds.len() as u64
                        || number(&data["completed_count"])?
                            != rounds
                                .iter()
                                .filter(|r| r["delivery_reception_id"].is_string())
                                .count() as u64
                    {
                        return Err(fail("vector continuation counts"));
                    }
                    let mut seen = BTreeSet::new();
                    for round in rounds {
                        let id = round["id"].as_str().ok_or_else(|| fail("round ID"))?;
                        if !seen.insert(id) {
                            return Err(fail("duplicate continuation ID"));
                        }
                        let input = reference(
                            records,
                            "ethernet.dynamic.reception",
                            &round["input_reception_id"],
                        )?;
                        let root = reference(
                            records,
                            "ethernet.dynamic.transfer",
                            &round["suffix_root_transfer_id"],
                        )?;
                        if accepted
                            .iter()
                            .filter(|r| r.data["reception_id"] == input.id)
                            .count()
                            != 1
                            || root.data["parent_transfer_id"] != input.data["transfer_id"]
                            || root.data["queued_ps"] != round["created_ps"]
                        {
                            return Err(fail("round input/suffix evidence"));
                        }
                        if !round["delivery_reception_id"].is_null() {
                            let rx = reference(
                                records,
                                "ethernet.dynamic.reception",
                                &round["delivery_reception_id"],
                            )?;
                            if rx.data["status"] != "received"
                                || rx.data["ready_ps"] != round["suffix"]["terminal_ps"]
                                || !n.ethernet.directions.iter().any(|d| {
                                    d.destination == pair.destination_device
                                        && d.to_port == rx.data["ingress"]
                                })
                            {
                                return Err(fail("round actual delivery"));
                            }
                        }
                    }
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
                    || (!normative && completed > target)
                    || (target == 0) != data["target"].is_null()
                    || (completed == 0) != data["delivered_branch_id"].is_null()
                {
                    return Err(fail("origin opportunity mismatch"));
                }
                if normative {
                    let delivered = records
                        .values()
                        .filter(|r| {
                            r.schema.name == "dir.redundancy.recovery"
                                && r.data["origin_id"] == record.id
                                && r.data["delivered"] == true
                        })
                        .count() as u64;
                    if completed != delivered
                        || ids(&data["delivery_indications"])?.len() as u64 != completed
                    {
                        return Err(fail("endpoint vector delivery count"));
                    }
                    for indication in ids(&data["delivery_indications"])? {
                        let recovery = reference(
                            records,
                            "dir.redundancy.recovery",
                            &indication["recovery_id"],
                        )?;
                        let rx = reference(
                            records,
                            "ethernet.dynamic.reception",
                            &indication["reception_id"],
                        )?;
                        if recovery.data["origin_id"] != record.id
                            || recovery.data["delivered"] != true
                            || recovery.data["reception_id"] != indication["reception_id"]
                            || rx.data["ready_ps"] != indication["delivered_ps"]
                        {
                            return Err(fail("endpoint indication physical evidence"));
                        }
                    }
                }
                if completed >= 1 {
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
                if data["placement"] == "bridge_pair_v1" {
                    let rx =
                        reference(records, "ethernet.dynamic.reception", &data["reception_id"])?;
                    let origin = reference(records, "dir.redundancy.origin", &data["origin_id"])?;
                    let continuation = data["continuation_created"]
                        .as_bool()
                        .ok_or_else(|| fail("distributed continuation boolean"))?;
                    if data["delivered"] != false
                        || data["window_decision"] != data["decision"]
                        || continuation
                            && (!accepted
                                || !normative
                                    && origin.data["winner_reception_id"] != data["reception_id"])
                        || rx.data["ready_ps"].is_null()
                    {
                        return Err(fail("distributed recovery physical evidence"));
                    }
                } else if data["delivered"].as_bool() != Some(accepted) {
                    return Err(fail("recovery delivery mismatch"));
                }
                if normative && data["placement"] != "bridge_pair_v1" {
                    let rx =
                        reference(records, "ethernet.dynamic.reception", &data["reception_id"])?;
                    if rx.data["status"] != "received" || rx.data["ready_ps"].is_null() {
                        return Err(fail("endpoint vector actual reception"));
                    }
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
                    transfer_reference(n, records, &data["parent_transfer_id"])?;
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
                let completed = number(&data["completed_mac_bytes"])?;
                let mac_bytes = number(&data["mac_bytes"])?;
                let partial = data
                    .get("partial_mac_bytes")
                    .map(number)
                    .transpose()?
                    .unwrap_or(0);
                if completed
                    .checked_add(partial)
                    .is_none_or(|total| total > mac_bytes)
                {
                    return Err(fail("emitted bytes exceed MAC"));
                }
                let reset = if data["reset_ps"].is_null() {
                    None
                } else {
                    let reset = number(&data["reset_ps"])?;
                    if reset > h || number(&data["sof_ps"])? > reset {
                        return Err(fail("serializer reset is outside actual transmission"));
                    }
                    for field in ["eof_ps", "release_ps", "arrival_ps"] {
                        if !data[field].is_null() && number(&data[field])? > reset {
                            return Err(fail("reset transfer contains a later actual boundary"));
                        }
                    }
                    if data["arrival_ps"].is_null()
                        && (data["status"] != "dropped"
                            || data["drop_reason"] != "link_epoch_reset")
                    {
                        return Err(fail("retired transmission lacks its reset outcome"));
                    }
                    Some(reset)
                };
                if data["kind"] == "express" && data["sof_ps"].is_string() {
                    let start = number(&data["sof_ps"])?;
                    let planned = number(&data["planned_release_ps"])?;
                    let end = if data["release_ps"].is_null() {
                        h.min(planned)
                    } else {
                        number(&data["release_ps"])?
                    };
                    let end = reset.map_or(end, |reset| end.min(reset));
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
                    if end > h
                        || end < start
                        || (if data["status"] == "aborted" {
                            end > planned
                        } else {
                            end != planned
                        })
                    {
                        return Err(fail("actual/planned interval mismatch"));
                    }
                    if data["status"] == "aborted" && t.data["reset_ps"] != data["end_ps"] {
                        return Err(fail("aborted interval lacks its actual serializer reset"));
                    }
                    end
                };
                let gap = record.schema.name == "dir.preemption.gap";
                if !gap {
                    let offset = number(&data["offset_bytes"])?;
                    let payload = number(&data["payload_bytes"])?;
                    if let Some(emitted) = data.get("emitted_payload_bytes") {
                        let emitted = number(emitted)?;
                        if emitted > payload
                            || (data["status"] == "completed" && emitted != payload)
                        {
                            return Err(fail(
                                "fragment emitted payload exceeds its actual boundary",
                            ));
                        }
                        if data["status"] == "aborted" {
                            let rate = number(&data["bitrate_bps"])?;
                            let overhead = number(&data["overhead_bytes"])?;
                            let actual_octets =
                                u128::from(end - start) * u128::from(rate) / 8_000_000_000_000u128;
                            let expected = actual_octets
                                .saturating_sub(u128::from(overhead))
                                .min(u128::from(payload));
                            if u128::from(emitted) != expected {
                                return Err(fail(
                                    "aborted fragment payload differs from elapsed serialization",
                                ));
                            }
                        }
                    }
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
                    let source = transfer_reference(n, records, &data["source_transfer_id"])?;
                    if source.data["frame_id"] != data["frame_id"]
                        || source.data["sof_ps"] != data["first_sof_ps"]
                    {
                        return Err(fail("delivery source frame or actual SOF mismatch"));
                    }
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
                if let Some(index) = data.get("active_transfer_index") {
                    if index.is_null() != data["active_transfer_id"].is_null() {
                        return Err(fail(
                            "abstract serializer source index lacks active transfer",
                        ));
                    }
                    if !index.is_null() {
                        number(index)?;
                    }
                }
                if !data["wire_control_id"].is_null() {
                    let id = data["wire_control_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| fail("invalid Wire serializer identity"))?;
                    let sequence = number(&data["effect_seq"])?;
                    if !data["active_transfer_id"].is_null()
                        || !records.values().any(|r| {
                            (r.time_ps < record.time_ps
                                || r.time_ps == record.time_ps
                                    && number(&r.data["effect_seq"])
                                        .is_ok_and(|prior| prior < sequence))
                                && r.data["port"] == data["port"]
                                && ((r.schema.name == "ethernet.control.wire.event"
                                    && r.data["packet_id"] == id
                                    && r.data["event_kind"] == "tx_started")
                                    || (r.schema.name == "dir.network.mac_control"
                                        && r.data["kind"] == "tx_started"
                                        && r.data["evidence"]["packet_id"] == id))
                        })
                    {
                        return Err(fail("Wire serializer decision lacks actual source TX"));
                    }
                }
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
