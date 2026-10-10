//! Flow statistics use completed receptions and their actual ancestor path.
use super::{Diagnostic, MetricValue, PreparedSimulation, Record, Snapshot, checked_add, ratio};
use crate::snapshot::Point;
use crate::snapshot::ethernet::{EthernetFrameRecord, EthernetReceptionRecord};
use std::collections::{BTreeMap, BTreeSet};

type Observations = (Vec<(usize, Point)>, Vec<Record>);
struct Delivery<'a> {
    frame: &'a EthernetFrameRecord,
    reception: &'a EthernetReceptionRecord,
    delay: u64,
    components: [u64; 4],
}
fn invalid(message: &str) -> Diagnostic {
    Diagnostic::output(format!("Ethernet flow invariant: {message}"))
}
fn elapsed(end: Option<u64>, start: u64) -> Result<u64, Diagnostic> {
    end.and_then(|end| end.checked_sub(start))
        .ok_or_else(|| invalid("missing or reversed actual milestone"))
}

pub(super) fn observations(
    prepared: &PreparedSimulation,
    snapshot: &Snapshot,
) -> Result<Observations, Diagnostic> {
    let ethernet = prepared.ethernet.as_ref().expect("Ethernet input");
    let state = snapshot.ethernet.as_ref().expect("Ethernet snapshot");
    let h = snapshot.common.end_ps;
    let frames: BTreeMap<_, _> = state
        .frames
        .iter()
        .map(|frame| (frame.frame_id.as_str(), frame))
        .collect();
    let transfers: BTreeMap<_, _> = state
        .transfers
        .iter()
        .map(|transfer| (transfer.transfer_id.as_str(), transfer))
        .collect();
    let receptions: BTreeMap<_, _> = state
        .receptions
        .iter()
        .map(|reception| (reception.transfer_id.as_str(), reception))
        .collect();
    let mut deliveries = Vec::new();
    let mut points = Vec::new();
    let mut delivery_points = Vec::new();
    let mut event_effects = BTreeMap::new();
    for (order, point) in snapshot.common.iter_points()?.enumerate() {
        let point = point?;
        if let Some(effect) = point.effect_seq {
            event_effects
                .entry(point.event_seq)
                .and_modify(|max: &mut u64| *max = (*max).max(effect))
                .or_insert(effect);
        }
        if point.metric == "ethernet.delivery_ps" {
            delivery_points.push((order, point));
        }
    }
    for reception in state
        .receptions
        .iter()
        .filter(|reception| reception.status == "received")
    {
        let frame = *frames
            .get(reception.frame_id.as_str())
            .ok_or_else(|| invalid("unknown frame"))?;
        let delay = elapsed(reception.ready_ps, frame.generated_ps)?;
        let mut components = [
            0u128,
            0,
            0,
            elapsed(frame.ready_ps, frame.generated_ps)? as u128,
        ];
        let mut current = Some(reception.transfer_id.as_str());
        let mut visited = BTreeSet::new();
        while let Some(id) = current {
            if !visited.insert(id) {
                return Err(invalid("cyclic transfer ancestry"));
            }
            let transfer = transfers
                .get(id)
                .ok_or_else(|| invalid("unknown parent transfer"))?;
            if transfer.frame_id != frame.frame_id {
                return Err(invalid("parent belongs to a different original frame"));
            }
            let processing = receptions
                .get(id)
                .ok_or_else(|| invalid("missing actual parent reception"))?;
            components[0] = checked_add(
                components[0],
                elapsed(transfer.sof_ps, transfer.queued_ps)?.into(),
            )?;
            components[1] = checked_add(
                components[1],
                elapsed(
                    transfer.eof_ps,
                    transfer.sof_ps.ok_or_else(|| invalid("missing SOF"))?,
                )?
                .into(),
            )?;
            components[2] = checked_add(
                components[2],
                elapsed(
                    transfer.arrival_ps,
                    transfer.eof_ps.ok_or_else(|| invalid("missing EOF"))?,
                )?
                .into(),
            )?;
            components[3] = checked_add(
                components[3],
                elapsed(processing.ready_ps, processing.observed_ps)?.into(),
            )?;
            current = transfer.parent_transfer_id.as_deref();
        }
        let total = components
            .iter()
            .try_fold(0u128, |sum, component| checked_add(sum, *component))?;
        if total != delay as u128 {
            return Err(invalid("path components do not sum to delivery latency"));
        }
        let components = components
            .map(|value| u64::try_from(value).expect("component bounded by checked u64 latency"));
        let (order, observation) = delivery_points
            .iter()
            .find(|(_, point)| {
                point.metric == "ethernet.delivery_ps"
                    && point.request_id.as_deref() == Some(frame.frame_id.as_str())
                    && point.receiver.as_deref() == Some(reception.device.as_str())
                    && Some(point.time_ps) == reception.ready_ps
            })
            .ok_or_else(|| invalid("missing committed delivery observation"))?;
        let effect_base = event_effects
            .get(&observation.event_seq)
            .copied()
            .unwrap_or(0);
        for (index, metric) in [
            "ethernet.queue_wait_ps",
            "ethernet.serialization_ps",
            "ethernet.propagation_ps",
            "ethernet.processing_ps",
        ]
        .iter()
        .enumerate()
        {
            points.push((
                *order,
                Point {
                    event_seq: observation.event_seq,
                    effect_seq: Some(
                        effect_base
                            .checked_add(index as u64 + 1)
                            .ok_or_else(|| invalid("effect sequence overflow"))?,
                    ),
                    time_ps: observation.time_ps,
                    target: reception.device.clone(),
                    metric: (*metric).into(),
                    value: components[index],
                    request_id: Some(frame.frame_id.clone()),
                    receiver: Some(reception.device.clone()),
                    reason: None,
                },
            ));
        }
        deliveries.push(Delivery {
            frame,
            reception,
            delay,
            components,
        });
    }
    let flows: BTreeSet<_> = ethernet
        .generators
        .iter()
        .filter_map(|generator| generator.flow_id.as_deref())
        .collect();
    let endpoints: Vec<_> = ethernet
        .devices
        .iter()
        .filter(|device| device.kind == "endpoint")
        .map(|device| device.id.as_str())
        .collect();
    let mut summary = Vec::new();
    for flow in flows {
        let frame_matches = |frame: &EthernetFrameRecord| frame.flow_id.as_deref() == Some(flow);
        let row_matches = |id: &str| frames.get(id).is_some_and(|frame| frame_matches(frame));
        for receiver in std::iter::once(None).chain(endpoints.iter().copied().map(Some)) {
            let target = match receiver {
                None => format!("@flow:{flow}"),
                Some(receiver) => format!("@flow:{flow}:{receiver}"),
            };
            let count_row = |metric: &str, value: usize| {
                Record::aggregate(&target, metric, MetricValue::Integer(value as u128), 0, h)
            };
            if receiver.is_none() {
                summary.push(count_row(
                    "ethernet.flow.generated",
                    state
                        .frames
                        .iter()
                        .filter(|frame| frame_matches(frame))
                        .count(),
                ));
                summary.push(count_row(
                    "ethernet.flow.source_processing",
                    state
                        .frames
                        .iter()
                        .filter(|frame| frame_matches(frame) && frame.ready_ps.is_none())
                        .count(),
                ));
                summary.push(count_row(
                    "ethernet.flow.copy_dropped",
                    state
                        .transfers
                        .iter()
                        .filter(|transfer| {
                            row_matches(&transfer.frame_id) && transfer.status == "dropped"
                        })
                        .count(),
                ));
                summary.push(count_row(
                    "ethernet.flow.unfinished_copies",
                    state
                        .transfers
                        .iter()
                        .filter(|transfer| {
                            row_matches(&transfer.frame_id)
                                && transfer.status != "dropped"
                                && transfer.arrival_ps.is_none()
                        })
                        .count(),
                ));
            }
            for status in ["received", "filtered", "processing"] {
                summary.push(count_row(
                    &format!("ethernet.flow.{status}"),
                    state
                        .receptions
                        .iter()
                        .filter(|reception| {
                            row_matches(&reception.frame_id)
                                && receiver.is_none_or(|receiver| reception.device == receiver)
                                && reception.status == status
                        })
                        .count(),
                ));
            }
            let samples: Vec<_> = deliveries
                .iter()
                .filter(|delivery| {
                    frame_matches(delivery.frame)
                        && receiver.is_none_or(|receiver| delivery.reception.device == receiver)
                })
                .collect();
            let deadlines: Vec<_> = samples
                .iter()
                .filter(|sample| sample.frame.deadline_ps.is_some())
                .collect();
            let missed = deadlines
                .iter()
                .filter(|sample| sample.delay > sample.frame.deadline_ps.unwrap())
                .count();
            summary.push(count_row(
                "ethernet.flow.deadline_sample_count",
                deadlines.len(),
            ));
            summary.push(count_row("ethernet.flow.deadline_missed", missed));
            let mut miss_ratio = Record::aggregate(
                &target,
                "ethernet.flow.deadline_miss_ratio",
                ratio(missed as u128, deadlines.len() as u128)?,
                0,
                h,
            );
            miss_ratio.sample_count = Some(deadlines.len() as u128);
            summary.push(miss_ratio);
            for (component, metric) in [
                (None, "ethernet.flow.delivery_mean_ps"),
                (Some(0), "ethernet.flow.queue_wait_mean_ps"),
                (Some(1), "ethernet.flow.serialization_mean_ps"),
                (Some(2), "ethernet.flow.propagation_mean_ps"),
                (Some(3), "ethernet.flow.processing_mean_ps"),
            ] {
                let total = samples.iter().try_fold(0u128, |sum, sample| {
                    checked_add(
                        sum,
                        component.map_or(sample.delay, |index| sample.components[index]) as u128,
                    )
                })?;
                let mut row =
                    Record::aggregate(&target, metric, ratio(total, samples.len() as u128)?, 0, h);
                row.sample_count = Some(samples.len() as u128);
                summary.push(row);
            }
            let mut sorted: Vec<_> = samples.iter().map(|sample| sample.delay).collect();
            sorted.sort_unstable();
            let mut statistic = |metric: &str, value: Option<u64>| {
                let mut row = Record::aggregate(
                    &target,
                    metric,
                    value.map_or(MetricValue::Null, |value| {
                        MetricValue::Integer(value.into())
                    }),
                    0,
                    h,
                );
                row.sample_count = Some(sorted.len() as u128);
                summary.push(row);
            };
            statistic("ethernet.flow.delivery_max_ps", sorted.last().copied());
            statistic(
                "ethernet.flow.delivery_jitter_ps",
                sorted
                    .first()
                    .zip(sorted.last())
                    .map(|(first, last)| last - first),
            );
            for (metric, percentage) in [
                ("ethernet.flow.delivery_p50_ps", 50u128),
                ("ethernet.flow.delivery_p95_ps", 95),
                ("ethernet.flow.delivery_p99_ps", 99),
            ] {
                let value = if sorted.is_empty() {
                    None
                } else {
                    Some(sorted[((sorted.len() as u128 * percentage).div_ceil(100) - 1) as usize])
                };
                statistic(metric, value);
            }
        }
    }
    Ok((points, summary))
}
