//! Opt-in integer Metric V2 observations for the owned network coordinator.
//! Default network profiles keep their schema-2 output contracts.
use crate::registry::{Effect, EffectBatch, MetricAggregation, MetricDescriptorV2, MetricSampling};
use crate::{Diagnostic, PreparedSimulation};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const QUEUE_FRAMES: &str = "network.queue.frames";
pub const QUEUE_BYTES: &str = "network.queue.bytes";
pub const DELIVERY_DELAY: &str = "network.delivery.delay_ps";

/// Select schema 3 and exact queue gauges/delivery delay means before starting
/// execution. Targets come exclusively from validated topology, not user strings.
/// Supply the resulting registry clone to `restore_checkpoint` when restoring.
pub fn enable(prepared: &mut PreparedSimulation) -> Result<(), Diagnostic> {
    let mut candidate = prepared.clone();
    let registered = candidate.registered.as_mut().ok_or_else(|| {
        Diagnostic::prepare("network Metric V2 requires a composed network profile")
    })?;
    let network = registered.network.as_ref().ok_or_else(|| {
        Diagnostic::prepare("network Metric V2 requires a composed network profile")
    })?;
    let mut lengths = BTreeSet::new();
    let mut bytes = BTreeSet::new();
    for link in &network.ethernet.directions {
        lengths.insert(format!("{}.queue", link.from_port));
        for priority in 0..8 {
            let target = format!("{}.queue.{priority}", link.from_port);
            lengths.insert(target.clone());
            bytes.insert(target);
        }
    }
    if let Some(bridge) = &network.bridge {
        lengths.extend(
            bridge
                .can
                .controllers
                .iter()
                .map(|c| format!("{}.txQueue", c.id)),
        );
    }
    let receivers = network
        .ethernet
        .devices
        .iter()
        .filter(|d| d.kind == "endpoint")
        .map(|d| d.id.clone())
        .collect::<BTreeSet<_>>();
    let definitions = [
        (
            QUEUE_FRAMES,
            "frames",
            MetricSampling::Gauge,
            MetricAggregation::TimeMean,
            lengths,
        ),
        (
            QUEUE_BYTES,
            "bytes",
            MetricSampling::Gauge,
            MetricAggregation::TimeMean,
            bytes,
        ),
        (
            DELIVERY_DELAY,
            "ps",
            MetricSampling::Point,
            MetricAggregation::Mean,
            receivers,
        ),
    ];
    let mut registry = (*registered.registry).clone();
    let mut names = Vec::new();
    for (name, unit, sampling, aggregation, targets) in definitions {
        if targets.is_empty() {
            continue;
        }
        let descriptor = MetricDescriptorV2 {
            name: name.into(),
            unit: unit.into(),
            version: 1,
            value_kind: "integer".into(),
            sampling,
            aggregation,
        };
        let targets = targets.into_iter().collect::<Vec<_>>();
        let initial_gauges = if sampling == MetricSampling::Gauge {
            targets.iter().cloned().map(|target| (target, 0)).collect()
        } else {
            BTreeMap::new()
        };
        if let Some(existing) = registry.metric_v2(name) {
            if existing.descriptor != descriptor
                || existing.targets != targets
                || existing.initial_gauges != initial_gauges
            {
                return Err(Diagnostic::prepare(
                    "network Metric V2 catalog conflicts with existing definition",
                ));
            }
        } else {
            registry.register_metric_v2(descriptor, targets, initial_gauges)?;
        }
        names.push(name.to_string());
    }
    let profile = registry
        .profiles
        .get_mut(&candidate.common.profile)
        .ok_or_else(|| Diagnostic::prepare("network profile missing"))?;
    profile.metrics.extend(names);
    profile.metrics.sort();
    profile.metrics.dedup();
    profile.output_schema_version = 3;
    let total = profile.metrics.iter().try_fold(0usize, |count, name| {
        count
            .checked_add(
                registry
                    .metrics_v2
                    .get(name)
                    .map_or(0, |metric| metric.targets.len()),
            )
            .ok_or_else(|| Diagnostic::prepare("network metric target overflow"))
    })?;
    if total > registry.metric_v2_limits.max_metric_targets as usize {
        return Err(Diagnostic::prepare(
            "network Metric V2 target limit exceeded",
        ));
    }
    registry
        .execution_capabilities
        .get_mut(&candidate.common.profile)
        .ok_or_else(|| Diagnostic::prepare("network capabilities missing"))?
        .metrics_v2 = true;
    registry.validate(&candidate.common.profile)?;
    registered.registry = Arc::new(registry);
    candidate.common.provenance = crate::input::capture_provenance(&candidate)?;
    *prepared = candidate;
    Ok(())
}

/// Called only by the private network owner, after its delta is prepared and
/// before either observations or model state commit. Generic model ownership
/// validation remains unchanged: these targets are the owner's prepared queues.
pub(crate) fn stage(
    profile: &str,
    points: &[crate::snapshot::Point],
    batch: &mut EffectBatch,
    registry: &crate::registry::Registry,
) -> crate::registry::ModelResult {
    if !registry.execution_capabilities(profile).metrics_v2 {
        return Ok(());
    }
    batch
        .effects
        .try_reserve(points.len())
        .map_err(|_| crate::registry::ModelError("network metric allocation failed".into()))?;
    for point in points {
        let name = match point.metric.as_str() {
            "queue_length" => QUEUE_FRAMES,
            "queue_bytes" => QUEUE_BYTES,
            "ethernet.delivery_ps" => DELIVERY_DELAY,
            _ => continue,
        };
        let Some(metric) = registry.metric_v2(name) else {
            continue;
        };
        if !registry.profiles[profile]
            .metrics
            .iter()
            .any(|metric| metric == name)
        {
            continue;
        }
        if metric.targets.binary_search(&point.target).is_err() {
            return Err(crate::registry::ModelError(
                "network metric target is outside prepared ownership".into(),
            ));
        }
        batch.effects.push(Effect::MetricV2 {
            metric: name.into(),
            target: point.target.clone(),
            value: point.value,
        });
    }
    Ok(())
}
