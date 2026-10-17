//! Exact transactional integer metrics. Frozen views never mutate live integration state.
use crate::registry::{MetricAggregation as A, MetricSampling as S, *};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

mod decimal128 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        let n: u128 = s.parse().map_err(serde::de::Error::custom)?;
        if s != n.to_string() {
            return Err(serde::de::Error::custom("noncanonical integer"));
        }
        Ok(n)
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Totals {
    #[serde(with = "decimal128")]
    pub count: u128,
    #[serde(with = "decimal128")]
    pub sum: u128,
    pub min: Option<u64>,
    pub max: Option<u64>,
    #[serde(with = "decimal128")]
    pub area: u128,
}
fn overflow() -> ModelError {
    ModelError("Metric V2 arithmetic overflow".into())
}
fn add(a: u128, b: u128) -> ModelResult<u128> {
    a.checked_add(b).ok_or_else(overflow)
}
impl Totals {
    fn observe(&mut self, value: u64) -> ModelResult {
        self.count = add(self.count, 1)?;
        self.sum = add(self.sum, u128::from(value))?;
        self.min = Some(self.min.map_or(value, |v| v.min(value)));
        self.max = Some(self.max.map_or(value, |v| v.max(value)));
        Ok(())
    }
    fn merge(&mut self, other: &Self) -> ModelResult {
        self.count = add(self.count, other.count)?;
        self.sum = add(self.sum, other.sum)?;
        self.min = match (self.min, other.min) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.max = match (self.max, other.max) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accumulator {
    pub metric: String,
    pub target: String,
    pub total: Totals,
    pub last_time: u64,
    pub last_value: u64,
    pub cursor: u64,
    pub windows: BTreeMap<u64, Totals>,
    pub bucket_time: Option<u64>,
    pub bucket: Totals,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricRecordV2 {
    pub seq: u64,
    pub event_seq: Option<u64>,
    pub effect_seq: u64,
    pub time_ps: u64,
    pub metric: String,
    pub target: String,
    pub value: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricStateV2 {
    pub next_record_sequence: u64,
    pub accumulators: Vec<Accumulator>,
    pub records: Vec<MetricRecordV2>,
}
pub(crate) struct MetricDelta {
    pub next_record_sequence: u64,
    pub accumulators: Vec<Accumulator>,
    pub records: Vec<MetricRecordV2>,
}
impl MetricStateV2 {
    pub(crate) fn new(registry: &Registry, profile: &str) -> Self {
        let mut accumulators = Vec::new();
        for name in &registry.profiles[profile].metrics {
            if let Some(m) = registry.metrics_v2.get(name) {
                for target in &m.targets {
                    accumulators.push(Accumulator {
                        metric: name.clone(),
                        target: target.clone(),
                        total: Totals::default(),
                        last_time: 0,
                        last_value: *m.initial_gauges.get(target).unwrap_or(&0),
                        cursor: 0,
                        windows: BTreeMap::new(),
                        bucket_time: None,
                        bucket: Totals::default(),
                    });
                }
            }
        }
        Self {
            next_record_sequence: 0,
            accumulators,
            records: Vec::new(),
        }
    }
    pub(crate) fn preflight(
        &self,
        registry: &Registry,
        window: u64,
        time: u64,
        event_seq: Option<u64>,
        effects: &[Effect],
    ) -> ModelResult<MetricDelta> {
        let mut accumulators = self.accumulators.clone();
        let count = effects
            .iter()
            .filter(|e| matches!(e, Effect::MetricV2 { .. }))
            .count();
        if count > registry.metric_v2_limits.max_observations_per_batch as usize {
            return Err("Metric V2 observation limit exceeded".into());
        }
        for acc in &mut accumulators {
            if time < acc.cursor || acc.bucket_time.is_some_and(|t| t > time) {
                return Err("Metric V2 time regressed".into());
            }
            if acc.bucket_time.is_some_and(|t| t < time) {
                let bucket_time = acc.bucket_time.unwrap();
                acc.windows
                    .entry(bucket_time / window * window)
                    .or_default()
                    .merge(&acc.bucket)?;
                acc.bucket = Totals::default();
                acc.bucket_time = None;
            }
            if registry.metrics_v2[&acc.metric].descriptor.sampling == S::Gauge {
                integrate_windows(acc, time, window)?;
            }
        }
        let mut records = Vec::with_capacity(count);
        let mut next_record_sequence = self.next_record_sequence;
        for (effect_seq, effect) in effects.iter().enumerate() {
            if matches!(effect, Effect::Observe { .. }) {
                next_record_sequence = next_record_sequence.checked_add(1).ok_or_else(overflow)?;
            }
            if let Effect::MetricV2 {
                metric,
                target,
                value,
            } = effect
            {
                let m = registry
                    .metrics_v2
                    .get(metric)
                    .ok_or_else(|| ModelError("unknown Metric V2".into()))?;
                let acc = accumulators
                    .iter_mut()
                    .find(|a| a.metric == *metric && a.target == *target)
                    .ok_or_else(|| ModelError("unknown Metric V2 target".into()))?;
                if m.descriptor.sampling == S::Gauge {
                    let width = time.checked_sub(acc.last_time).ok_or_else(overflow)?;
                    acc.total.area = add(
                        acc.total.area,
                        u128::from(acc.last_value)
                            .checked_mul(u128::from(width))
                            .ok_or_else(overflow)?,
                    )?;
                    acc.last_time = time;
                    acc.last_value = *value;
                    acc.total.count = add(acc.total.count, 1)?;
                    acc.bucket.count = add(acc.bucket.count, 1)?;
                } else {
                    acc.total.observe(*value)?;
                    acc.bucket.observe(*value)?;
                }
                acc.bucket_time = Some(time);
                let seq = next_record_sequence;
                next_record_sequence = next_record_sequence.checked_add(1).ok_or_else(overflow)?;
                records.push(MetricRecordV2 {
                    seq,
                    event_seq,
                    effect_seq: effect_seq as u64,
                    time_ps: time,
                    metric: metric.clone(),
                    target: target.clone(),
                    value: *value,
                });
            }
        }
        Ok(MetricDelta {
            next_record_sequence,
            accumulators,
            records,
        })
    }
    pub(crate) fn apply(&mut self, delta: MetricDelta) {
        self.next_record_sequence = delta.next_record_sequence;
        self.accumulators = delta.accumulators;
        self.records.extend(delta.records);
    }
    pub fn summary(
        &self,
        registry: &Registry,
        horizon: u64,
        window: u64,
    ) -> ModelResult<Vec<Value>> {
        let mut result = Vec::new();
        for original in &self.accumulators {
            let m = &registry.metrics_v2[&original.metric];
            if m.descriptor.aggregation == A::Identity {
                continue;
            }
            let mut acc = original.clone();
            if m.descriptor.sampling == S::Gauge {
                let width = horizon.checked_sub(acc.last_time).ok_or_else(overflow)?;
                acc.total.area = add(
                    acc.total.area,
                    u128::from(acc.last_value)
                        .checked_mul(u128::from(width))
                        .ok_or_else(overflow)?,
                )?;
                integrate_windows(&mut acc, horizon, window)?;
            }
            if acc.bucket_time.is_some_and(|t| t < horizon) {
                let time = acc.bucket_time.unwrap();
                acc.windows
                    .entry(time / window * window)
                    .or_default()
                    .merge(&acc.bucket)?;
            }
            result.push(summary_row(
                m,
                &acc.target,
                &acc.total,
                None,
                None,
                horizon,
            )?);
            let mut start = 0;
            while start < horizon {
                let end = start.saturating_add(window).min(horizon);
                let totals = acc.windows.get(&start).cloned().unwrap_or_default();
                result.push(summary_row(
                    m,
                    &acc.target,
                    &totals,
                    Some(start),
                    Some(end),
                    end - start,
                )?);
                start = end;
            }
        }
        Ok(result)
    }
    pub fn record_values(&self, registry: &Registry, _offset: usize) -> Vec<Value> {
        self.records.iter().map(|r|{let d=&registry.metrics_v2[&r.metric].descriptor;json!({"seq":r.seq.to_string(),"event_seq":r.event_seq.map(|x|x.to_string()),"effect_seq":r.effect_seq.to_string(),"time_ps":r.time_ps.to_string(),"metric":r.metric,"target":r.target,"unit":d.unit,"value_kind":"integer","value":r.value.to_string(),"descriptor_version":d.version.to_string(),"start_ps":null,"end_ps":null,"request_id":null,"receiver":null,"reason":null,"sample_count":null})}).collect()
    }
}
fn integrate_windows(acc: &mut Accumulator, time: u64, window: u64) -> ModelResult {
    if window == 0 {
        return Err("invalid metric window".into());
    }
    while acc.cursor < time {
        let start = acc.cursor / window * window;
        let end = start.saturating_add(window).min(time);
        let area = u128::from(acc.last_value)
            .checked_mul(u128::from(end - acc.cursor))
            .ok_or_else(overflow)?;
        let total = acc.windows.entry(start).or_default();
        total.area = add(total.area, area)?;
        acc.cursor = end;
    }
    Ok(())
}
fn summary_row(
    m: &MetricRegistrationV2,
    target: &str,
    t: &Totals,
    start: Option<u64>,
    end: Option<u64>,
    duration: u64,
) -> ModelResult<Value> {
    use crate::output::aggregate::{MetricValue, ratio};
    let d = &m.descriptor;
    let value = match d.aggregation {
        A::Sum => json!(t.sum.to_string()),
        A::Count => json!(t.count.to_string()),
        A::Min => json!(t.min.map(|v| v.to_string())),
        A::Max => json!(t.max.map(|v| v.to_string())),
        A::Mean | A::TimeMean => {
            let (n, den) = if d.aggregation == A::Mean {
                (t.sum, t.count)
            } else {
                (t.area, u128::from(duration))
            };
            match ratio(n, den).map_err(|e| ModelError(e.message))? {
                MetricValue::Number(v) => json!(v),
                MetricValue::Null => Value::Null,
                MetricValue::Integer(v) => json!(v.to_string()),
            }
        }
        A::Identity => Value::Null,
    };
    Ok(
        json!({"metric":d.name,"target":target,"aggregation":d.aggregation,"unit":if d.aggregation==A::Count{"count"}else{&d.unit},"value_kind":if matches!(d.aggregation,A::Mean|A::TimeMean){"number"}else{"integer"},"value":value,"sample_count":t.count.to_string(),"start_ps":start.map(|v|v.to_string()),"end_ps":end.map(|v|v.to_string())}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn registry(sampling: S, aggregation: A) -> Registry {
        let mut r = Registry::empty();
        r.register_metric_v2(
            MetricDescriptorV2 {
                name: "test.metric".into(),
                unit: "items".into(),
                version: 1,
                value_kind: "integer".into(),
                sampling,
                aggregation,
            },
            vec!["a".into()],
            if sampling == S::Gauge {
                BTreeMap::from([("a".into(), 2)])
            } else {
                BTreeMap::new()
            },
        )
        .unwrap();
        r.register_profile(ProfileDescriptor {
            name: "test.profile".into(),
            implementation_version: "1".into(),
            modules: vec![],
            channels: vec![],
            events: vec![],
            metrics: vec!["test.metric".into()],
            model_records: vec![],
            output_schema_version: 3,
            validate: |_, _| Ok(()),
        })
        .unwrap();
        r
    }
    fn effect(value: u64) -> Effect {
        Effect::MetricV2 {
            metric: "test.metric".into(),
            target: "a".into(),
            value,
        }
    }
    fn publish(state: &mut MetricStateV2, r: &Registry, time: u64, values: &[u64]) {
        let delta = state
            .preflight(
                r,
                4,
                time,
                Some(time),
                &values.iter().map(|v| effect(*v)).collect::<Vec<_>>(),
            )
            .unwrap();
        state.apply(delta);
    }
    #[test]
    fn point_selected_aggregates_and_empty_targets() {
        for (aggregation, expected) in [
            (A::Sum, json!("10")),
            (A::Count, json!("3")),
            (A::Min, json!("1")),
            (A::Max, json!("7")),
            (A::Mean, json!(10.0 / 3.0)),
        ] {
            let r = registry(S::Point, aggregation);
            let mut state = MetricStateV2::new(&r, "test.profile");
            let empty = state.summary(&r, 0, 4).unwrap();
            assert_eq!(
                empty[0]["value"],
                if matches!(aggregation, A::Sum | A::Count) {
                    json!("0")
                } else {
                    Value::Null
                }
            );
            publish(&mut state, &r, 1, &[1, 2, 7]);
            assert_eq!(state.summary(&r, 4, 4).unwrap()[0]["value"], expected);
        }
        let r = registry(S::Point, A::Identity);
        let mut state = MetricStateV2::new(&r, "test.profile");
        publish(&mut state, &r, 1, &[1]);
        assert!(state.summary(&r, 4, 4).unwrap().is_empty());
        assert_eq!(state.records.len(), 1);
    }
    #[test]
    fn gauge_independent_area_and_non_destructive_horizon() {
        let r = registry(S::Gauge, A::TimeMean);
        let mut state = MetricStateV2::new(&r, "test.profile");
        publish(&mut state, &r, 3, &[4]);
        publish(&mut state, &r, 7, &[9, 1]);
        assert_eq!(state.accumulators[0].total.area, 22);
        let before = state.clone();
        for _ in 0..2 {
            let view = state.summary(&r, 7, 4).unwrap();
            assert_eq!(view[0]["value"], json!(22.0 / 7.0));
            assert_eq!(view[1]["value"], 2.5);
            assert_eq!(view[2]["value"], 4.0);
        }
        assert_eq!(state, before);
        let view = state.summary(&r, 10, 4).unwrap();
        assert_eq!(
            view.iter()
                .map(|v| v["value"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [2.5, 2.5, 3.25, 1.0]
        );
    }
    #[test]
    fn failed_preflight_does_not_flush_h_bucket_or_publish_overflow() {
        let r = registry(S::Point, A::Min);
        let mut state = MetricStateV2::new(&r, "test.profile");
        publish(&mut state, &r, 3, &[5]);
        publish(&mut state, &r, 7, &[1]);
        let view = state.summary(&r, 7, 4).unwrap();
        assert_eq!(view[0]["value"], "1");
        assert_eq!(view[1]["value"], "5");
        assert!(view[2]["value"].is_null());
        state.accumulators[0].total.sum = u128::MAX;
        let before = state.clone();
        assert!(state.preflight(&r, 4, 8, Some(8), &[effect(1)]).is_err());
        assert_eq!(state, before);
        assert_eq!(state.accumulators[0].bucket_time, Some(7));
    }
    #[test]
    fn invalid_descriptor_combinations_and_cross_version_names_rejected() {
        let mut r = Registry::empty();
        for (sampling, aggregation) in [(S::Gauge, A::Max), (S::Point, A::TimeMean)] {
            assert!(
                r.register_metric_v2(
                    MetricDescriptorV2 {
                        name: "bad.metric".into(),
                        unit: "x".into(),
                        version: 1,
                        value_kind: "integer".into(),
                        sampling,
                        aggregation
                    },
                    vec!["a".into()],
                    BTreeMap::new()
                )
                .is_err()
            );
        }
        let mut r = registry(S::Point, A::Sum);
        assert!(
            r.register_metric(MetricDescriptor {
                name: "test.metric".into(),
                unit: "x".into()
            })
            .is_err()
        );
        assert!(
            r.set_metric_v2_limits(MetricV2Limits {
                max_metric_targets: 0,
                max_observations_per_batch: 1
            })
            .is_err()
        );
    }
}
