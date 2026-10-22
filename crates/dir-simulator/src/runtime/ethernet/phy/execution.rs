//! Registered common-FES adapter for sampled Ethernet PHY contracts.
use super::{medium::*, *};
use crate::registry::*;
use crate::types::Diagnostic;
use serde_json::json;
pub const PROFILE: &str = "ethernet.phy.v1";
pub const IMPLEMENTATION: &str = "ethernet.SampledPhy";
fn tick_schema() -> Schema {
    Schema::new("dir.ethernet.PhySample", 1)
}
fn record_schema() -> Schema {
    Schema::new("dir.ethernet.PhyRecord", 1)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub schema: String,
    pub version: u32,
    pub medium: MediumConfig,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub schema: String,
    pub version: u32,
    pub requests: Vec<MacTx>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tick {
    sample_index: u64,
}
struct Execution {
    config: Configuration,
    workload: Workload,
    medium: SampledMedium,
    sequence: u64,
    initialized: bool,
    timer: Option<TimerToken>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    config: Configuration,
    workload: Workload,
    medium: Vec<u8>,
    sequence: u64,
    initialized: bool,
    timer: Option<TimerReference>,
}
fn parse(input: &ProfileInput) -> ModelResult<(Configuration, Workload, SampledMedium)> {
    let config: Configuration = serde_json::from_value(
        input
            .model_config
            .clone()
            .ok_or("sampled PHY requires model-config")?,
    )
    .map_err(|e| ModelError(format!("PHY config: {e}")))?;
    let workload: Workload = serde_json::from_value(
        input
            .workload
            .clone()
            .ok_or("sampled PHY requires workload")?,
    )
    .map_err(|e| ModelError(format!("PHY workload: {e}")))?;
    if config.schema != "dir.ethernet-phy.model"
        || config.version != 1
        || workload.schema != "dir.ethernet-phy.workload"
        || workload.version != 1
        || workload.requests.len() > 4096
    {
        return Err("unsupported PHY model/workload schema/version/bounds".into());
    }
    let mut medium = SampledMedium::new(config.medium.clone()).map_err(ModelError)?;
    let mut last = 0;
    for (i, request) in workload.requests.iter().enumerate() {
        if i > 0 && request.at_ps < last {
            return Err("PHY workload must be ordered by admission time".into());
        }
        let limit = config
            .medium
            .channel
            .grid
            .time(config.medium.maximum_samples)
            .map_err(ModelError)?;
        if request.at_ps as u128 * limit.denominator >= limit.numerator_ps {
            return Err("PHY admission exceeds declared sample budget".into());
        }
        medium.admit(request.clone()).map_err(ModelError)?;
        last = request.at_ps;
    }
    Ok((config, workload, medium))
}
fn validate_input(input: &ProfileInput) -> ModelResult {
    parse(input).map(|_| ())
}
fn factory(c: &ModelConfig) -> ModelResult<Box<dyn Model>> {
    let (config, workload, medium) = parse(&c.profile_input)?;
    Ok(Box::new(Execution {
        config,
        workload,
        medium,
        sequence: 0,
        initialized: false,
        timer: None,
    }))
}
fn payload(index: u64) -> ModelResult<Vec<u8>> {
    serde_json::to_vec(&Tick {
        sample_index: index,
    })
    .map_err(|e| ModelError(e.to_string()))
}
impl Model for Execution {
    fn initialize(&mut self, c: &mut Context<'_>) -> ModelResult {
        if self.initialized {
            return Err("PHY initialized twice".into());
        }
        let timer = if self.medium.has_pending() {
            Some(c.schedule_at(
                self.medium.next_event_ps().map_err(ModelError)?,
                tick_schema(),
                payload(self.medium.next_sample_index())?,
            )?)
        } else {
            None
        };
        self.timer = timer;
        self.initialized = true;
        Ok(())
    }
    fn on_event(&mut self, event: &Envelope, c: &mut Context<'_>) -> ModelResult {
        if !self.initialized
            || event.schema != tick_schema()
            || c.current_timer_token() != self.timer.as_ref()
            || c.now() != self.medium.next_event_ps().map_err(ModelError)?
            || event.payload != payload(self.medium.next_sample_index())?
        {
            return Err("invalid sampled PHY live timer boundary/payload/ownership".into());
        }
        let mut next = self.medium.clone();
        let effects = next.advance_one_sample().map_err(ModelError)?;
        let mut sequence = self.sequence;
        for effect in effects {
            let mut data = serde_json::to_value(effect).map_err(|e| ModelError(e.to_string()))?;
            data.as_object_mut()
                .ok_or("PHY record must be object")?
                .insert("effect_seq".into(), json!(sequence.to_string()));
            c.upsert_model_record(ModelRecord {
                schema: record_schema(),
                id: format!("phy:{sequence}"),
                subject: c.subject().into(),
                time_ps: c.now(),
                data,
            })?;
            sequence = sequence
                .checked_add(1)
                .ok_or("PHY record sequence overflow")?;
        }
        let timer = if next.has_pending() {
            Some(c.schedule_at(
                next.next_event_ps().map_err(ModelError)?,
                tick_schema(),
                payload(next.next_sample_index())?,
            )?)
        } else {
            None
        };
        self.medium = next;
        self.sequence = sequence;
        self.timer = timer;
        Ok(())
    }
    fn checkpoint(&self, c: &CheckpointEncodeContext<'_>) -> ModelResult<Vec<u8>> {
        serde_json::to_vec(&Saved {
            version: 1,
            config: self.config.clone(),
            workload: self.workload.clone(),
            medium: self.medium.checkpoint().map_err(ModelError)?,
            sequence: self.sequence,
            initialized: self.initialized,
            timer: self
                .timer
                .as_ref()
                .map(|t| c.encode_timer_ref(t))
                .transpose()?,
        })
        .map_err(|e| ModelError(e.to_string()))
    }
}
fn restore(
    c: &ModelConfig,
    version: u32,
    bytes: &[u8],
    context: &CheckpointDecodeContext<'_>,
) -> ModelResult<Box<dyn Model>> {
    if version != 1 {
        return Err("unsupported sampled PHY checkpoint codec".into());
    }
    let saved: Saved =
        serde_json::from_slice(bytes).map_err(|e| ModelError(format!("PHY checkpoint: {e}")))?;
    let (config, workload, _) = parse(&c.profile_input)?;
    if saved.version != 1 || saved.config != config || saved.workload != workload {
        return Err("PHY checkpoint immutable input mismatch".into());
    }
    let medium = SampledMedium::restore(&config.medium, &saved.medium).map_err(ModelError)?;
    medium
        .validate_admissions(&workload.requests)
        .map_err(ModelError)?;
    let timer = saved
        .timer
        .as_ref()
        .map(|t| context.decode_timer_ref(t))
        .transpose()?;
    if (saved.initialized && medium.has_pending()) != timer.is_some() {
        return Err("PHY checkpoint timer liveness mismatch".into());
    }
    if !saved.initialized && (medium.next_sample_index() != 0 || saved.sequence != 0) {
        return Err("PHY checkpoint callbacks precede initialization".into());
    }
    if let Some(timer) = &timer {
        let event = context.timer_envelope(timer)?;
        if event.time_ps != medium.next_event_ps().map_err(ModelError)?
            || event.schema != tick_schema()
            || event.payload != payload(medium.next_sample_index())?
            || event.source != timer.owner
            || event.destination != timer.owner
            || event.input_port.is_some()
        {
            return Err("PHY checkpoint actual FES boundary/payload mismatch".into());
        }
    }
    Ok(Box::new(Execution {
        config,
        workload,
        medium,
        sequence: saved.sequence,
        initialized: saved.initialized,
        timer,
    }))
}
pub fn register(r: &mut Registry) -> std::result::Result<(), Diagnostic> {
    r.register_event(EventDescriptor {
        schema: tick_schema(),
        phase: 1,
        validate: |bytes| {
            serde_json::from_slice::<Tick>(bytes)
                .map(|_| ())
                .map_err(|e| ModelError(e.to_string()))
        },
    })?;
    r.register_module(
        IMPLEMENTATION,
        factory,
        ModuleDescriptor {
            implementation_key: IMPLEMENTATION.into(),
            implementation_version: "1".into(),
            parameters: vec![],
            ports: vec![],
            events: vec![tick_schema()],
            resources: vec![],
        },
    )?;
    r.register_model_record(ModelRecordDescriptor {
        schema: record_schema(),
        validate: |data| {
            let mut data = data.clone();
            data.as_object_mut()
                .ok_or("invalid PHY record object")?
                .remove("effect_seq");
            serde_json::from_value::<PhyEffect>(data)
                .map(|_| ())
                .map_err(|e| ModelError(e.to_string()))
        },
    })?;
    r.register_profile(ProfileDescriptor {
        name: PROFILE.into(),
        implementation_version: "1".into(),
        modules: vec![IMPLEMENTATION.into()],
        channels: vec![],
        events: vec![tick_schema()],
        metrics: vec![],
        model_records: vec![record_schema()],
        output_schema_version: 3,
        validate: |models, channels| {
            if models.len() == 1 && channels.is_empty() {
                Ok(())
            } else {
                Err(
                    "sampled PHY needs exactly one medium owner and no external NED channels"
                        .into(),
                )
            }
        },
    })?;
    r.register_profile_input_validator(PROFILE, validate_input)?;
    r.register_execution_capabilities(
        PROFILE,
        ExecutionCapabilities {
            cooperative_control: true,
            checkpoint: true,
            metrics_v2: false,
        },
    )?;
    r.register_model_checkpoint_codec(
        CheckpointCodecDescriptor {
            implementation_key: IMPLEMENTATION.into(),
            implementation_version: "1".into(),
            codec_name: "ethernet.phy.sampled.state".into(),
            codec_version: 1,
            external_state: "none".into(),
            max_bytes: 67_108_864,
        },
        restore,
    )?;
    Ok(())
}
