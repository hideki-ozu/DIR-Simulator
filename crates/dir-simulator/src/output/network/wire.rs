//! Frozen physical wire accounting; protocol projection remains owned by the reducer.
use super::super::aggregate::{MetricValue, ratio};
use super::*;
fn number(v: &Value) -> u128 {
    v.as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
}
fn overlap(start: u128, end: u128, from: u128, to: u128) -> u128 {
    end.min(to).saturating_sub(start.max(from))
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
    if n.wire.is_none() {
        return Ok(());
    }
    let Some(wire) = snapshot
        .network
        .as_ref()
        .and_then(|n| n.get("wire"))
        .filter(|v| !v.is_null())
    else {
        return Ok(());
    };
    let h = snapshot.common.end_ps;
    let tx = wire["tx"]
        .as_array()
        .ok_or_else(|| Diagnostic::output("wire transfer snapshot missing"))?;
    let boundaries = wire["boundaries"]
        .as_array()
        .ok_or_else(|| Diagnostic::output("wire boundary snapshot missing"))?;
    let records = &snapshot.registered.as_ref().unwrap().model_records;
    for record in records.values().filter(|r| {
        r.schema.name.starts_with("ethernet.wire.")
            || r.schema.name.starts_with("ethernet.control.wire.")
    }) {
        let mut data = record.data.clone();
        if record.schema.name.starts_with("ethernet.control.wire.") {
            // The network carrier owns one canonical ordering field; the domain
            // payload validator accepts only its own closed field set.
            let sequence = data
                .as_object_mut()
                .and_then(|fields| fields.remove("effect_seq"))
                .ok_or_else(|| Diagnostic::output("wire carrier effect_seq missing"))?;
            let text = sequence.as_str().ok_or_else(|| {
                Diagnostic::output("wire carrier effect_seq must be a decimal string")
            })?;
            let parsed = text
                .parse::<u64>()
                .map_err(|_| Diagnostic::output("wire carrier effect_seq is outside u64"))?;
            if text != parsed.to_string() {
                return Err(Diagnostic::output(
                    "wire carrier effect_seq is noncanonical",
                ));
            }
        }
        crate::output::wire::validate_record(&record.schema.name, &data)?;
    }
    let events = records
        .values()
        .filter(|r| r.schema.name == "ethernet.control.wire.event")
        .map(|r| &r.data)
        .collect::<Vec<_>>();
    for (name, count) in [
        (
            "ethernet.wire.accepted",
            events
                .iter()
                .filter(|e| e["event_kind"] == "receive" && e["reason"].is_null())
                .count(),
        ),
        (
            "ethernet.wire.dropped",
            events
                .iter()
                .filter(|e| e["outcome"] == "control_drop")
                .count(),
        ),
        (
            "ethernet.wire.transmitted",
            tx.iter().filter(|t| t["eof_ps"].is_string()).count(),
        ),
    ] {
        summary.push(Record::aggregate(
            "@network",
            name,
            MetricValue::Integer(count as u128),
            0,
            h,
        ));
    }
    for direction in &n.ethernet.directions {
        let mine = tx
            .iter()
            .filter(|t| t["port"] == direction.from_port)
            .collect::<Vec<_>>();
        let incoming = boundaries
            .iter()
            .filter(|b| b["ingress_port"] == direction.to_port)
            .collect::<Vec<_>>();
        let metrics = [
            (
                "ethernet.wire.tx_started",
                mine.iter().filter(|t| t["sof_ps"].is_string()).count() as u128,
            ),
            (
                "ethernet.wire.tx_completed",
                mine.iter().filter(|t| t["eof_ps"].is_string()).count() as u128,
            ),
            (
                "ethernet.wire.tx_mac_bytes",
                mine.iter()
                    .filter(|t| t["eof_ps"].is_string())
                    .map(|t| number(&t["mac_bytes"]))
                    .sum(),
            ),
            (
                "ethernet.wire.tx_busy_ps",
                mine.iter().map(|t| number(&t["busy_ps"])).sum(),
            ),
            (
                "ethernet.wire.rx_wire_ps",
                incoming.iter().map(|b| number(&b["rx_wire_ps"])).sum(),
            ),
            (
                "ethernet.wire.rx_ifg_ps",
                incoming.iter().map(|b| number(&b["rx_ifg_ps"])).sum(),
            ),
            (
                "ethernet.wire.rx_completed",
                incoming
                    .iter()
                    .map(|b| number(&b["completed_frames"]))
                    .sum(),
            ),
            (
                "ethernet.wire.rx_mac_bytes",
                incoming
                    .iter()
                    .map(|b| number(&b["completed_mac_bytes"]))
                    .sum(),
            ),
            (
                "ethernet.wire.rx_bits",
                incoming.iter().map(|b| number(&b["completed_bits"])).sum(),
            ),
        ];
        for (name, count) in metrics {
            summary.push(Record::aggregate(
                &direction.from_port,
                name,
                MetricValue::Integer(count),
                0,
                h,
            ));
        }
        for row in summary
            .iter_mut()
            .filter(|r| r.target == direction.from_port)
        {
            if row.metric == "ethernet.link_utilization" {
                let start = u128::from(row.start_ps.unwrap_or(0));
                let end = u128::from(row.end_ps.unwrap_or(h));
                let mut busy = 0u128;
                if let Some(ethernet) = &snapshot.ethernet {
                    for t in ethernet
                        .transfers
                        .iter()
                        .filter(|t| t.from_port == direction.from_port)
                    {
                        if let Some(sof) = t.sof_ps {
                            busy = busy
                                .checked_add(overlap(
                                    sof.into(),
                                    t.release_ps.unwrap_or(h).into(),
                                    start,
                                    end,
                                ))
                                .ok_or_else(|| Diagnostic::output("wire busy sum overflow"))?;
                        }
                    }
                }
                for t in &mine {
                    if t["sof_ps"].is_string() {
                        busy = busy
                            .checked_add(overlap(
                                number(&t["sof_ps"]),
                                if t["release_ps"].is_null() {
                                    u128::from(h)
                                } else {
                                    number(&t["release_ps"])
                                },
                                start,
                                end,
                            ))
                            .ok_or_else(|| Diagnostic::output("control busy sum overflow"))?;
                    }
                }
                for packet in n
                    .wire
                    .as_ref()
                    .unwrap()
                    .packets
                    .iter()
                    .filter(|p| p.ingress_port == direction.to_port)
                {
                    if let Some(plan) = &packet.boundary {
                        if incoming.iter().any(|b| {
                            b["packet_id"] == packet.id && b["actual_start_ps"].is_string()
                        }) {
                            busy = busy
                                .checked_add(overlap(
                                    plan.start_ps.into(),
                                    u128::from(h).min(plan.release_ps.into()),
                                    start,
                                    end,
                                ))
                                .ok_or_else(|| Diagnostic::output("boundary busy sum overflow"))?;
                        }
                    }
                }
                row.value = ratio(busy, end.saturating_sub(start))?;
            }
        }
    }
    Ok(())
}
