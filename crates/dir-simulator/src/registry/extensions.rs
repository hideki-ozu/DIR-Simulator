//! Additive execution capabilities, integer metric catalog and explicit checkpoint codecs.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCapabilities {
    pub cooperative_control: bool,
    pub metrics_v2: bool,
    pub checkpoint: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricSampling {
    Point,
    Gauge,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricAggregation {
    Identity,
    Sum,
    Count,
    Min,
    Max,
    Mean,
    TimeMean,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricDescriptorV2 {
    pub name: String,
    pub unit: String,
    pub version: u32,
    pub value_kind: String,
    pub sampling: MetricSampling,
    pub aggregation: MetricAggregation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricV2Limits {
    pub max_metric_targets: u32,
    pub max_observations_per_batch: u32,
}
impl Default for MetricV2Limits {
    fn default() -> Self {
        Self {
            max_metric_targets: 1024,
            max_observations_per_batch: 65536,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricRegistrationV2 {
    pub descriptor: MetricDescriptorV2,
    pub targets: Vec<String>,
    pub initial_gauges: BTreeMap<String, u64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointCodecDescriptor {
    pub implementation_key: String,
    pub implementation_version: String,
    pub codec_name: String,
    pub codec_version: u32,
    pub external_state: String,
    pub max_bytes: u64,
}
pub type ModelRestoreFactory =
    fn(&ModelConfig, u32, &[u8], &CheckpointDecodeContext<'_>) -> ModelResult<Box<dyn Model>>;
pub type ChannelRestoreFactory =
    fn(&ChannelConfig, u32, &[u8], &CheckpointDecodeContext<'_>) -> ModelResult<Box<dyn Channel>>;
#[derive(Clone)]
pub(crate) struct ModelCodec {
    pub descriptor: CheckpointCodecDescriptor,
    pub restore: ModelRestoreFactory,
}
#[derive(Clone)]
pub(crate) struct ChannelCodec {
    pub descriptor: CheckpointCodecDescriptor,
    pub restore: ChannelRestoreFactory,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimerReference {
    pub version: u32,
    pub owner: String,
    pub id: String,
    pub state: String,
}
pub struct CheckpointEncodeContext<'a> {
    pub(crate) owner: &'a str,
    pub(crate) next_event_id: u64,
    pub(crate) live_timers: &'a BTreeSet<TimerToken>,
    pub(crate) reservations: &'a BTreeSet<TimerToken>,
}
pub struct CheckpointDecodeContext<'a> {
    pub(crate) owner: &'a str,
    pub(crate) next_event_id: u64,
    pub(crate) live_timers: &'a BTreeSet<TimerToken>,
    pub(crate) reservations: &'a BTreeSet<TimerToken>,
}
impl CheckpointEncodeContext<'_> {
    pub fn encode_timer_ref(&self, token: &TimerToken) -> ModelResult<TimerReference> {
        if token.owner != self.owner || token.id >= self.next_event_id {
            return Err("invalid checkpoint timer owner/id".into());
        }
        let live = self.live_timers.contains(token);
        if live != self.reservations.contains(token) {
            return Err("checkpoint timer reservation mismatch".into());
        }
        Ok(TimerReference {
            version: 1,
            owner: token.owner.clone(),
            id: token.id.to_string(),
            state: if live { "live" } else { "retired" }.into(),
        })
    }
}
impl CheckpointDecodeContext<'_> {
    pub fn decode_timer_ref(&self, reference: &TimerReference) -> ModelResult<TimerToken> {
        let id: u64 = reference
            .id
            .parse()
            .map_err(|_| ModelError("invalid checkpoint timer ID".into()))?;
        if reference.id != id.to_string()
            || reference.version != 1
            || reference.owner != self.owner
            || id >= self.next_event_id
            || !matches!(reference.state.as_str(), "live" | "retired")
        {
            return Err("invalid checkpoint timer reference".into());
        }
        let token = TimerToken {
            owner: reference.owner.clone(),
            id,
        };
        let live = reference.state == "live";
        if self.live_timers.contains(&token) != live || self.reservations.contains(&token) != live {
            return Err("checkpoint timer reservation mismatch".into());
        }
        Ok(token)
    }
}
impl Registry {
    pub fn register_execution_capabilities(
        &mut self,
        profile: &str,
        capabilities: ExecutionCapabilities,
    ) -> Result<(), Diagnostic> {
        check_name(profile)?;
        if capabilities.checkpoint && !capabilities.cooperative_control {
            return Err(Diagnostic::prepare(
                "checkpoint requires cooperative control",
            ));
        }
        insert(
            &mut self.execution_capabilities,
            profile.into(),
            capabilities,
        )
    }
    pub fn execution_capabilities(&self, profile: &str) -> ExecutionCapabilities {
        self.execution_capabilities
            .get(profile)
            .copied()
            .unwrap_or_default()
    }
    pub fn set_metric_v2_limits(&mut self, limits: MetricV2Limits) -> Result<(), Diagnostic> {
        if limits.max_metric_targets == 0 || limits.max_observations_per_batch == 0 {
            return Err(Diagnostic::prepare("Metric V2 limits must be positive"));
        }
        self.metric_v2_limits = limits;
        Ok(())
    }
    pub fn register_metric_v2(
        &mut self,
        descriptor: MetricDescriptorV2,
        mut targets: Vec<String>,
        initial_gauges: BTreeMap<String, u64>,
    ) -> Result<(), Diagnostic> {
        check_name(&descriptor.name)?;
        targets.sort();
        if descriptor.unit.is_empty()
            || descriptor.version == 0
            || descriptor.value_kind != "integer"
            || targets.is_empty()
            || targets.windows(2).any(|w| w[0] == w[1])
            || targets.iter().any(|s| s.is_empty())
            || self.metrics.contains_key(&descriptor.name)
        {
            return Err(Diagnostic::prepare(
                "invalid or duplicate Metric V2 definition",
            ));
        }
        let valid = match descriptor.sampling {
            MetricSampling::Point => {
                descriptor.aggregation != MetricAggregation::TimeMean && initial_gauges.is_empty()
            }
            MetricSampling::Gauge => {
                descriptor.aggregation == MetricAggregation::TimeMean
                    && initial_gauges.keys().eq(targets.iter())
            }
        };
        if !valid {
            return Err(Diagnostic::prepare(
                "invalid Metric V2 sampling/aggregation/initial gauges",
            ));
        }
        insert(
            &mut self.metrics_v2,
            descriptor.name.clone(),
            MetricRegistrationV2 {
                descriptor,
                targets,
                initial_gauges,
            },
        )
    }
    pub fn metric_v2(&self, name: &str) -> Option<&MetricRegistrationV2> {
        self.metrics_v2.get(name)
    }
    pub(crate) fn validate_metric_targets(
        &self,
        profile: &str,
        models: &[ModelConfig],
    ) -> Result<(), Diagnostic> {
        let mut total = 0usize;
        for name in &self.profiles[profile].metrics {
            if let Some(metric) = self.metrics_v2.get(name) {
                if !self.execution_capabilities(profile).metrics_v2
                    || self.profiles[profile].output_schema_version != 3
                {
                    return Err(Diagnostic::prepare(
                        "Metric V2 requires schema 3 and explicit capability",
                    ));
                }
                for target in &metric.targets {
                    if !models.iter().any(|m| &m.subject == target) {
                        return Err(Diagnostic::prepare(format!(
                            "unknown Metric V2 target: {target}"
                        )));
                    }
                }
                total = total
                    .checked_add(metric.targets.len())
                    .ok_or_else(|| Diagnostic::prepare("metric target overflow"))?;
            }
        }
        if total > self.metric_v2_limits.max_metric_targets as usize {
            return Err(Diagnostic::prepare("Metric V2 target limit exceeded"));
        }
        Ok(())
    }
    pub fn register_model_checkpoint_codec(
        &mut self,
        descriptor: CheckpointCodecDescriptor,
        restore: ModelRestoreFactory,
    ) -> Result<(), Diagnostic> {
        self.validate_codec(&descriptor, true)?;
        insert(
            &mut self.model_codecs,
            descriptor.implementation_key.clone(),
            ModelCodec {
                descriptor,
                restore,
            },
        )
    }
    pub fn register_channel_checkpoint_codec(
        &mut self,
        descriptor: CheckpointCodecDescriptor,
        restore: ChannelRestoreFactory,
    ) -> Result<(), Diagnostic> {
        self.validate_codec(&descriptor, false)?;
        insert(
            &mut self.channel_codecs,
            descriptor.implementation_key.clone(),
            ChannelCodec {
                descriptor,
                restore,
            },
        )
    }
    fn validate_codec(&self, d: &CheckpointCodecDescriptor, model: bool) -> Result<(), Diagnostic> {
        check_name(&d.implementation_key)?;
        check_name(&d.codec_name)?;
        let version = if model {
            self.modules
                .get(&d.implementation_key)
                .map(|m| &m.descriptor.implementation_version)
        } else {
            self.channels
                .get(&d.implementation_key)
                .map(|m| &m.descriptor.implementation_version)
        };
        if version != Some(&d.implementation_version)
            || d.codec_version == 0
            || d.max_bytes == 0
            || d.external_state != "none"
        {
            return Err(Diagnostic::prepare(
                "unsupported checkpoint codec declaration",
            ));
        }
        Ok(())
    }
}
impl Context<'_> {
    fn metric_v2_effect(
        &mut self,
        name: &str,
        target: &str,
        value: u64,
        sampling: MetricSampling,
    ) -> ModelResult {
        let metric = self
            .registry
            .metrics_v2
            .get(name)
            .ok_or_else(|| ModelError("unknown Metric V2".into()))?;
        if !self.profile.metrics.iter().any(|m| m == name)
            || metric.descriptor.sampling != sampling
            || !metric.targets.iter().any(|t| t == target)
            || self.subject() != target
        {
            return Err("invalid Metric V2 sampling or target ownership".into());
        }
        if self
            .batch
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::MetricV2 { .. }))
            .count()
            >= self.registry.metric_v2_limits.max_observations_per_batch as usize
        {
            return Err("Metric V2 observation limit exceeded".into());
        }
        self.batch.effects.push(Effect::MetricV2 {
            metric: name.into(),
            target: target.into(),
            value,
        });
        Ok(())
    }
    pub fn observe_point_u64(&mut self, name: &str, target: &str, value: u64) -> ModelResult {
        self.metric_v2_effect(name, target, value, MetricSampling::Point)
    }
    pub fn set_gauge_u64(&mut self, name: &str, target: &str, value: u64) -> ModelResult {
        self.metric_v2_effect(name, target, value, MetricSampling::Gauge)
    }
}
