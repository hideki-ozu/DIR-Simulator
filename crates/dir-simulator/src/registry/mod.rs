//! Static extension registration. A prepared run owns an immutable registry snapshot.
mod prepare;
use crate::types::Diagnostic;
pub use prepare::{prepare_with_registry, prepare_with_registry_and_source};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub type ModelResult<T = ()> = Result<T, ModelError>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelError(pub String);
impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for ModelError {}
impl From<&str> for ModelError {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for ModelError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Schema {
    pub name: String,
    pub version: u32,
}
impl Schema {
    pub fn new(name: impl Into<String>, version: u32) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Envelope {
    pub event_id: u64,
    pub source: String,
    pub destination: String,
    pub request_id: Option<String>,
    pub time_ps: u64,
    pub input_port: Option<String>,
    pub schema: Schema,
    pub payload: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ParameterValue {
    Integer(i64),
    Quantity(u64),
    Double(f64),
    Boolean(bool),
    String(String),
}
pub type Parameters = BTreeMap<String, ParameterValue>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    Dimensionless,
    Time,
    Bitrate,
    Data,
}
#[derive(Clone, Debug)]
pub struct ParameterDescriptor {
    pub name: String,
    pub ned_type: String,
    pub dimension: Dimension,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub required: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}
#[derive(Clone, Debug)]
pub struct PortDescriptor {
    pub name: String,
    pub direction: Direction,
    pub schema: Schema,
}
#[derive(Clone)]
pub struct EventDescriptor {
    pub schema: Schema,
    pub phase: u8,
    pub validate: fn(&[u8]) -> ModelResult,
}
#[derive(Clone, Debug)]
pub struct ModuleDescriptor {
    pub implementation_key: String,
    pub implementation_version: String,
    pub parameters: Vec<ParameterDescriptor>,
    pub ports: Vec<PortDescriptor>,
    pub events: Vec<Schema>,
    /// Resource names local to each instance. Their callbacks run at phase 2.
    pub resources: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct ChannelDescriptor {
    pub implementation_key: String,
    pub implementation_version: String,
    pub parameters: Vec<ParameterDescriptor>,
    pub capabilities: Vec<Schema>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ConnectionDescriptor {
    pub output_port: String,
    pub destination: String,
    pub input_port: String,
    pub schema: Schema,
    pub channels: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub profile_input: Arc<ProfileInput>,
    pub id: String,
    /// Normalized NED subject. Coordinators retain their separate execution ID.
    pub subject: String,
    pub implementation_key: String,
    pub parameters: Parameters,
    pub connections: Vec<ConnectionDescriptor>,
    /// Frozen output-handle to actual sending model bindings.
    pub output_sources: BTreeMap<String, String>,
}
#[derive(Clone, Debug)]
pub struct BorrowedOutput {
    pub handle: String,
    pub source: String,
    pub output_port: String,
}
#[derive(Clone, Debug)]
pub struct CoordinatorDescriptor {
    pub target: String,
    pub implementation_key: String,
    pub parameters: Parameters,
    pub outputs: Vec<BorrowedOutput>,
}
#[derive(Clone, Debug)]
pub struct ChannelConfig {
    pub id: String,
    pub implementation_key: String,
    pub parameters: Parameters,
}
pub trait Model: Send {
    fn initialize(&mut self, _context: &mut Context<'_>) -> ModelResult {
        Ok(())
    }
    fn on_event(&mut self, event: &Envelope, context: &mut Context<'_>) -> ModelResult;
    fn on_arbitration(&mut self, _resource: &str, _context: &mut Context<'_>) -> ModelResult {
        Err("arbitration callback is not implemented".into())
    }
    fn finish(&mut self, _context: &FinishContext<'_>) -> ModelResult {
        Ok(())
    }
}
struct NetworkPlaceholder;
impl Model for NetworkPlaceholder {
    fn on_event(&mut self, _: &Envelope, _: &mut Context<'_>) -> ModelResult {
        Err("private network coordinator dispatch missing".into())
    }
}
/// One instance is constructed per explicit NED connection. Capabilities are queried
/// by models; send_at never adds a channel delay a second time.
pub trait Channel: Send {
    fn capability(&self, name: &Schema, now_ps: u64, payload: &[u8])
    -> ModelResult<ParameterValue>;
    fn finish(&mut self, _context: &FinishContext<'_>) -> ModelResult {
        Ok(())
    }
}
#[derive(Clone, Debug, Default)]
pub struct ProfileInput {
    pub model_config: Option<Value>,
    pub workload: Option<Value>,
}
pub type ProfileInputValidator = fn(&ProfileInput) -> ModelResult;
pub type ModelFactory = fn(&ModelConfig) -> ModelResult<Box<dyn Model>>;
pub type ChannelFactory = fn(&ChannelConfig) -> ModelResult<Box<dyn Channel>>;
#[derive(Clone)]
pub struct ProfileDescriptor {
    pub name: String,
    pub implementation_version: String,
    pub modules: Vec<String>,
    pub channels: Vec<String>,
    pub events: Vec<Schema>,
    pub metrics: Vec<String>,
    pub model_records: Vec<Schema>,
    pub output_schema_version: u32,
    pub validate: fn(&[ModelConfig], &[ChannelConfig]) -> ModelResult,
}
#[derive(Clone, Debug)]
pub struct MetricDescriptor {
    pub name: String,
    pub unit: String,
}
#[derive(Clone)]
pub struct ModelRecordDescriptor {
    pub schema: Schema,
    pub validate: fn(&Value) -> ModelResult,
}
#[derive(Clone, Debug, Serialize)]
pub struct ModelRecord {
    pub schema: Schema,
    pub id: String,
    pub subject: String,
    pub time_ps: u64,
    pub data: Value,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct RegisteredSnapshot {
    pub model_records: BTreeMap<String, ModelRecord>,
    pub deliveries: Vec<Envelope>,
}
impl RegisteredSnapshot {
    /// Latest committed record, keyed by schema name and model-assigned record ID.
    pub fn model_record(&self, schema_name: &str, id: &str) -> Option<&ModelRecord> {
        self.model_records.get(&format!("{schema_name}:{id}"))
    }
}
pub struct FinishContext<'a> {
    pub snapshot: &'a crate::snapshot::Snapshot,
}

/// Executable strategy seams. Selection and queue rules remain model policies.
pub type ArbitrationStrategy = fn(&[Envelope]) -> ModelResult<Option<usize>>;
pub type QueueStrategy = fn(&[Envelope], &Envelope, usize) -> ModelResult<bool>;
pub type GeneratorStrategy = fn(&Parameters) -> ModelResult<Vec<(u64, Schema, Vec<u8>)>>;
pub type OutputStrategy = fn(&RegisteredSnapshot) -> ModelResult<Vec<u8>>;
#[derive(Clone)]
pub(crate) struct ModuleRegistration {
    pub descriptor: ModuleDescriptor,
    pub factory: ModelFactory,
}
#[derive(Clone)]
pub(crate) struct ChannelRegistration {
    pub descriptor: ChannelDescriptor,
    pub factory: ChannelFactory,
}
#[derive(Clone)]
pub struct Registry {
    pub(crate) builtin_profiles: BTreeSet<String>,
    pub(crate) modules: BTreeMap<String, ModuleRegistration>,
    pub(crate) channels: BTreeMap<String, ChannelRegistration>,
    pub(crate) events: BTreeMap<Schema, EventDescriptor>,
    pub(crate) profiles: BTreeMap<String, ProfileDescriptor>,
    pub(crate) metrics: BTreeMap<String, MetricDescriptor>,
    pub(crate) records: BTreeMap<Schema, ModelRecordDescriptor>,
    arbitration: BTreeMap<String, ArbitrationStrategy>,
    queues: BTreeMap<String, QueueStrategy>,
    generators: BTreeMap<String, GeneratorStrategy>,
    outputs: BTreeMap<String, OutputStrategy>,
    pub(crate) input_validators: BTreeMap<String, ProfileInputValidator>,
    pub(crate) coordinators: BTreeMap<(String, String), CoordinatorDescriptor>,
}
impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .field("profiles", &self.profiles.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}
fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(crate::input::identifier)
}
fn check_name(name: &str) -> Result<(), Diagnostic> {
    if name_valid(name) {
        Ok(())
    } else {
        Err(Diagnostic::prepare(format!(
            "invalid registration name: {name}"
        )))
    }
}
fn insert<K: Ord + std::fmt::Debug, V>(
    map: &mut BTreeMap<K, V>,
    key: K,
    value: V,
) -> Result<(), Diagnostic> {
    if map.contains_key(&key) {
        return Err(Diagnostic::prepare(format!(
            "duplicate registration: {key:?}"
        )));
    }
    map.insert(key, value);
    Ok(())
}
fn schema_valid(schema: &Schema) -> Result<(), Diagnostic> {
    check_name(&schema.name)?;
    if schema.version == 0 {
        return Err(Diagnostic::prepare("schema version must be positive"));
    }
    Ok(())
}
fn parameters_valid(parameters: &[ParameterDescriptor]) -> Result<(), Diagnostic> {
    let mut seen = BTreeSet::new();
    for p in parameters {
        if !crate::input::identifier(&p.name) || !seen.insert(&p.name) {
            return Err(Diagnostic::prepare("invalid or duplicate parameter name"));
        }
        if !matches!(p.ned_type.as_str(), "int" | "double" | "bool" | "string")
            || (p.dimension != Dimension::Dimensionless
                && !matches!(p.ned_type.as_str(), "int" | "double"))
        {
            return Err(Diagnostic::prepare("invalid parameter type/dimension"));
        }
        if p.minimum.is_some_and(|v| !v.is_finite())
            || p.maximum.is_some_and(|v| !v.is_finite())
            || matches!((p.minimum, p.maximum), (Some(a), Some(b)) if a > b)
        {
            return Err(Diagnostic::prepare("invalid parameter bounds"));
        }
    }
    Ok(())
}
impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    /// An empty registry, useful when only separately linked extensions are allowed.
    pub fn empty() -> Self {
        Self {
            builtin_profiles: BTreeSet::new(),
            modules: BTreeMap::new(),
            channels: BTreeMap::new(),
            events: BTreeMap::new(),
            profiles: BTreeMap::new(),
            metrics: BTreeMap::new(),
            records: BTreeMap::new(),
            arbitration: BTreeMap::new(),
            queues: BTreeMap::new(),
            generators: BTreeMap::new(),
            outputs: BTreeMap::new(),
            input_validators: BTreeMap::new(),
            coordinators: BTreeMap::new(),
        }
    }
    pub fn metric(&self, key: &str) -> Option<&MetricDescriptor> {
        self.metrics.get(key)
    }
    /// Register one profile-owned execution object for a normalized compound path.
    /// Borrowed outputs are resolved and frozen during preparation.
    pub fn register_coordinator(
        &mut self,
        profile: &str,
        descriptor: CoordinatorDescriptor,
    ) -> Result<(), Diagnostic> {
        check_name(profile)?;
        check_name(&descriptor.target)?;
        check_name(&descriptor.implementation_key)?;
        let mut handles = BTreeSet::new();
        for output in &descriptor.outputs {
            check_name(&output.source)?;
            if !crate::input::identifier(&output.handle)
                || !crate::input::identifier(&output.output_port)
                || !handles.insert(&output.handle)
            {
                return Err(Diagnostic::prepare(
                    "invalid or duplicate coordinator output handle",
                ));
            }
        }
        insert(
            &mut self.coordinators,
            (profile.into(), descriptor.target.clone()),
            descriptor,
        )
    }
    pub fn model_record(&self, schema: &Schema) -> Option<&ModelRecordDescriptor> {
        self.records.get(schema)
    }
    pub fn register_profile_input_validator(
        &mut self,
        profile: &str,
        validator: ProfileInputValidator,
    ) -> Result<(), Diagnostic> {
        check_name(profile)?;
        insert(&mut self.input_validators, profile.into(), validator)
    }
    pub fn register_module(
        &mut self,
        implementation_key: &str,
        factory: ModelFactory,
        descriptor: ModuleDescriptor,
    ) -> Result<(), Diagnostic> {
        check_name(implementation_key)?;
        if implementation_key != descriptor.implementation_key
            || descriptor.implementation_version.is_empty()
        {
            return Err(Diagnostic::prepare("module key/version mismatch"));
        }
        parameters_valid(&descriptor.parameters)?;
        let mut ports = BTreeSet::new();
        for p in &descriptor.ports {
            if !crate::input::identifier(&p.name) || !ports.insert(&p.name) {
                return Err(Diagnostic::prepare("invalid or duplicate port"));
            }
            schema_valid(&p.schema)?;
        }
        let mut resources = BTreeSet::new();
        for r in &descriptor.resources {
            check_name(r)?;
            if !resources.insert(r) {
                return Err(Diagnostic::prepare("duplicate resource"));
            }
        }
        for e in &descriptor.events {
            schema_valid(e)?;
        }
        insert(
            &mut self.modules,
            implementation_key.into(),
            ModuleRegistration {
                descriptor,
                factory,
            },
        )
    }
    pub fn register_channel(
        &mut self,
        implementation_key: &str,
        factory: ChannelFactory,
        descriptor: ChannelDescriptor,
    ) -> Result<(), Diagnostic> {
        check_name(implementation_key)?;
        if implementation_key != descriptor.implementation_key
            || descriptor.implementation_version.is_empty()
        {
            return Err(Diagnostic::prepare("channel key/version mismatch"));
        }
        parameters_valid(&descriptor.parameters)?;
        for c in &descriptor.capabilities {
            if c.version == 0
                || c.name.is_empty()
                || !c
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
            {
                return Err(Diagnostic::prepare(
                    "invalid channel capability name/version",
                ));
            }
        }
        insert(
            &mut self.channels,
            implementation_key.into(),
            ChannelRegistration {
                descriptor,
                factory,
            },
        )
    }
    pub fn register_event(&mut self, descriptor: EventDescriptor) -> Result<(), Diagnostic> {
        schema_valid(&descriptor.schema)?;
        if descriptor.phase > 1 {
            return Err(Diagnostic::prepare("event phase must be 0 or 1"));
        }
        insert(&mut self.events, descriptor.schema.clone(), descriptor)
    }
    pub fn register_profile(
        &mut self,
        mut descriptor: ProfileDescriptor,
    ) -> Result<(), Diagnostic> {
        // Profile identifiers contain version components such as v1, never bare digits.
        check_name(&descriptor.name)?;
        if descriptor.implementation_version.is_empty()
            || !matches!(descriptor.output_schema_version, 1 | 2)
        {
            return Err(Diagnostic::prepare(
                "invalid profile version or output schema",
            ));
        }
        descriptor.modules.sort();
        descriptor.channels.sort();
        descriptor.events.sort();
        descriptor.metrics.sort();
        descriptor.model_records.sort();
        if descriptor.modules.windows(2).any(|w| w[0] == w[1])
            || descriptor.channels.windows(2).any(|w| w[0] == w[1])
            || descriptor.events.windows(2).any(|w| w[0] == w[1])
            || descriptor.metrics.windows(2).any(|w| w[0] == w[1])
            || descriptor.model_records.windows(2).any(|w| w[0] == w[1])
        {
            return Err(Diagnostic::prepare("duplicate profile membership")
                .with_reason("duplicate_definition"));
        }
        insert(&mut self.profiles, descriptor.name.clone(), descriptor)
    }
    pub fn register_metric(&mut self, descriptor: MetricDescriptor) -> Result<(), Diagnostic> {
        check_name(&descriptor.name)?;
        insert(&mut self.metrics, descriptor.name.clone(), descriptor)
    }
    pub fn register_model_record(
        &mut self,
        descriptor: ModelRecordDescriptor,
    ) -> Result<(), Diagnostic> {
        schema_valid(&descriptor.schema)?;
        insert(&mut self.records, descriptor.schema.clone(), descriptor)
    }
    pub fn register_arbitration(
        &mut self,
        name: &str,
        strategy: ArbitrationStrategy,
    ) -> Result<(), Diagnostic> {
        check_name(name)?;
        insert(&mut self.arbitration, name.into(), strategy)
    }
    pub fn register_queue(
        &mut self,
        name: &str,
        strategy: QueueStrategy,
    ) -> Result<(), Diagnostic> {
        check_name(name)?;
        insert(&mut self.queues, name.into(), strategy)
    }
    pub fn register_generator(
        &mut self,
        name: &str,
        strategy: GeneratorStrategy,
    ) -> Result<(), Diagnostic> {
        check_name(name)?;
        insert(&mut self.generators, name.into(), strategy)
    }
    pub fn register_output(
        &mut self,
        name: &str,
        strategy: OutputStrategy,
    ) -> Result<(), Diagnostic> {
        check_name(name)?;
        insert(&mut self.outputs, name.into(), strategy)
    }
    pub fn module(&self, key: &str) -> Option<&ModuleDescriptor> {
        self.modules.get(key).map(|r| &r.descriptor)
    }
    pub fn event(&self, schema: &Schema) -> Option<&EventDescriptor> {
        self.events.get(schema)
    }
    pub fn profile(&self, key: &str) -> Option<&ProfileDescriptor> {
        self.profiles.get(key)
    }
    pub fn arbitrate(&self, name: &str, candidates: &[Envelope]) -> ModelResult<Option<usize>> {
        let selected = self
            .arbitration
            .get(name)
            .ok_or_else(|| ModelError(format!("unknown arbitration strategy: {name}")))?(
            candidates,
        )?;
        if selected.is_some_and(|i| i >= candidates.len()) {
            return Err("arbitration returned an invalid index".into());
        }
        Ok(selected)
    }
    pub fn queue_accepts(
        &self,
        name: &str,
        queue: &[Envelope],
        incoming: &Envelope,
        capacity: usize,
    ) -> ModelResult<bool> {
        self.queues
            .get(name)
            .ok_or_else(|| ModelError(format!("unknown queue strategy: {name}")))?(
            queue, incoming, capacity,
        )
    }
    pub fn generate(
        &self,
        name: &str,
        parameters: &Parameters,
    ) -> ModelResult<Vec<(u64, Schema, Vec<u8>)>> {
        self.generators
            .get(name)
            .ok_or_else(|| ModelError(format!("unknown generator: {name}")))?(parameters)
    }
    pub fn encode_output(&self, name: &str, snapshot: &RegisteredSnapshot) -> ModelResult<Vec<u8>> {
        self.outputs
            .get(name)
            .ok_or_else(|| ModelError(format!("unknown output strategy: {name}")))?(snapshot)
    }
    pub(crate) fn validate(&self, profile: &str) -> Result<(), Diagnostic> {
        let p = self
            .profiles
            .get(profile)
            .ok_or_else(|| Diagnostic::prepare(format!("unregistered profile: {profile}")))?;
        for key in &p.modules {
            let module = self
                .modules
                .get(key)
                .ok_or_else(|| Diagnostic::prepare(format!("unregistered module: {key}")))?;
            for schema in module
                .descriptor
                .events
                .iter()
                .chain(module.descriptor.ports.iter().map(|port| &port.schema))
            {
                if !p.events.contains(schema) {
                    return Err(Diagnostic::prepare(format!(
                        "module event excluded by profile: {key}: {schema:?}"
                    )));
                }
            }
        }
        for key in &p.channels {
            if !self.channels.contains_key(key) {
                return Err(Diagnostic::prepare(format!("unregistered channel: {key}")));
            }
        }
        for schema in &p.events {
            if !self.events.contains_key(schema) {
                return Err(Diagnostic::prepare(format!(
                    "unregistered event: {schema:?}"
                )));
            }
        }
        for key in &p.metrics {
            if !self.metrics.contains_key(key) {
                return Err(Diagnostic::prepare(format!("unregistered metric: {key}")));
            }
        }
        for schema in &p.model_records {
            if !self.records.contains_key(schema) {
                return Err(Diagnostic::prepare(format!(
                    "unregistered record: {schema:?}"
                )));
            }
        }
        for m in self.modules.values() {
            for schema in m
                .descriptor
                .events
                .iter()
                .chain(m.descriptor.ports.iter().map(|p| &p.schema))
            {
                if !self.events.contains_key(schema) {
                    return Err(Diagnostic::prepare(format!(
                        "unregistered module event: {schema:?}"
                    )));
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct PreparedRegistered {
    pub registry: Arc<Registry>,
    pub models: Vec<ModelConfig>,
    pub channels: Vec<ChannelConfig>,
    pub(crate) adapter: Option<BuiltinAdapter>,
    pub(crate) builtin_node_count: usize,
    pub(crate) network: Option<Arc<crate::types::network::PreparedNetwork>>,
}
#[derive(Clone, Debug)]
pub(crate) struct BuiltinAdapter;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimerToken {
    pub(crate) owner: String,
    pub(crate) id: u64,
}
#[derive(Clone, Debug)]
pub(crate) enum Effect {
    Schedule {
        token: Option<TimerToken>,
        envelope: Envelope,
        phase: u8,
    },
    Cancel(TimerToken),
    Arbitration(String),
    Observe {
        metric: String,
        target: String,
        value: u64,
    },
    Record(ModelRecord),
}
/// Effects belong to one callback and are published only after it succeeds.
#[derive(Default)]
pub(crate) struct EffectBatch {
    pub effects: Vec<Effect>,
}
pub struct Context<'a> {
    pub(crate) now: u64,
    pub(crate) config: &'a ModelConfig,
    pub(crate) registry: &'a Registry,
    pub(crate) profile: &'a ProfileDescriptor,
    pub(crate) channels: &'a BTreeMap<String, Box<dyn Channel>>,
    pub(crate) channel_configs: &'a [ChannelConfig],
    pub(crate) live_timers: &'a BTreeSet<TimerToken>,
    pub(crate) executing_timer: Option<&'a TimerToken>,
    pub(crate) next_id: &'a mut u64,
    pub(crate) batch: &'a mut EffectBatch,
}
impl Context<'_> {
    pub fn now(&self) -> u64 {
        self.now
    }
    pub fn self_id(&self) -> &str {
        &self.config.id
    }
    pub fn subject(&self) -> &str {
        &self.config.subject
    }
    pub fn registry(&self) -> &Registry {
        self.registry
    }
    pub fn connections(&self) -> &[ConnectionDescriptor] {
        &self.config.connections
    }
    pub fn channel_capability(
        &self,
        channel: &str,
        capability: &Schema,
        payload: &[u8],
    ) -> ModelResult<ParameterValue> {
        if !self
            .config
            .connections
            .iter()
            .any(|c| c.channels.iter().any(|id| id == channel))
        {
            return Err("channel is outside this model's connections".into());
        }
        let config = self
            .channel_configs
            .iter()
            .find(|config| config.id == channel)
            .ok_or_else(|| ModelError("unknown channel".into()))?;
        if !self.registry.channels[&config.implementation_key]
            .descriptor
            .capabilities
            .contains(capability)
        {
            return Err("unregistered channel capability schema/version".into());
        }
        self.channels
            .get(channel)
            .ok_or_else(|| ModelError("unknown channel".into()))?
            .capability(capability, self.now, payload)
    }
    fn allocate(&mut self) -> ModelResult<u64> {
        let id = *self.next_id;
        *self.next_id = id
            .checked_add(1)
            .ok_or_else(|| ModelError("event ID overflow".into()))?;
        Ok(id)
    }
    fn check_event(&self, at: u64, schema: &Schema, bytes: &[u8]) -> ModelResult<u8> {
        if at < self.now {
            return Err("event scheduled in the past".into());
        }
        if !self.profile.events.contains(schema) {
            return Err("event is outside selected profile".into());
        }
        let e = self
            .registry
            .events
            .get(schema)
            .ok_or_else(|| ModelError("unknown event schema/version".into()))?;
        (e.validate)(bytes)?;
        Ok(e.phase)
    }
    pub fn schedule_at(
        &mut self,
        at_ps: u64,
        schema: Schema,
        payload: Vec<u8>,
    ) -> ModelResult<TimerToken> {
        let phase = self.check_event(at_ps, &schema, &payload)?;
        if !self.registry.modules[&self.config.implementation_key]
            .descriptor
            .events
            .contains(&schema)
        {
            return Err("event is outside module timer schemas".into());
        }
        let id = self.allocate()?;
        let token = TimerToken {
            owner: self.config.id.clone(),
            id,
        };
        self.batch.effects.push(Effect::Schedule {
            token: Some(token.clone()),
            envelope: Envelope {
                event_id: id,
                source: self.config.id.clone(),
                destination: self.config.id.clone(),
                request_id: None,
                time_ps: at_ps,
                input_port: None,
                schema,
                payload,
            },
            phase,
        });
        Ok(token)
    }
    pub fn schedule_after(
        &mut self,
        delay_ps: u64,
        schema: Schema,
        payload: Vec<u8>,
    ) -> ModelResult<TimerToken> {
        self.schedule_at(
            self.now
                .checked_add(delay_ps)
                .ok_or_else(|| ModelError("time overflow".into()))?,
            schema,
            payload,
        )
    }
    pub fn cancel(&mut self, token: &TimerToken) -> bool {
        if token.owner != self.config.id
            || self.executing_timer == Some(token)
            || self
                .batch
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Cancel(t) if t == token))
        {
            return false;
        }
        let pending = self.live_timers.contains(token)
            || self
                .batch
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Schedule { token: Some(t), .. } if t == token));
        if pending {
            self.batch.effects.push(Effect::Cancel(token.clone()));
        }
        pending
    }
    pub fn send_at(
        &mut self,
        output_port: &str,
        at_ps: u64,
        schema: Schema,
        payload: Vec<u8>,
    ) -> ModelResult {
        self.send_request_at(output_port, at_ps, schema, payload, None)
    }
    pub fn send_request_at(
        &mut self,
        output_port: &str,
        at_ps: u64,
        schema: Schema,
        payload: Vec<u8>,
        request_id: Option<String>,
    ) -> ModelResult {
        self.check_event(at_ps, &schema, &payload)?;
        let route = self
            .config
            .connections
            .iter()
            .find(|c| c.output_port == output_port)
            .ok_or_else(|| ModelError("unconnected or unowned output port".into()))?;
        if route.schema != schema {
            return Err("send payload does not match port contract".into());
        }
        let destination = route.destination.clone();
        let input_port = route.input_port.clone();
        let source = self
            .config
            .output_sources
            .get(output_port)
            .ok_or_else(|| ModelError("unowned output handle".into()))?
            .clone();
        let id = self.allocate()?;
        self.batch.effects.push(Effect::Schedule {
            token: None,
            phase: 1,
            envelope: Envelope {
                event_id: id,
                source,
                destination,
                request_id,
                time_ps: at_ps,
                input_port: Some(input_port),
                schema,
                payload,
            },
        });
        Ok(())
    }
    pub fn request_arbitration(&mut self, resource: &str) -> ModelResult {
        if !self.registry.modules[&self.config.implementation_key]
            .descriptor
            .resources
            .iter()
            .any(|r| r == resource)
        {
            return Err("unowned or unknown resource".into());
        }
        self.batch
            .effects
            .push(Effect::Arbitration(resource.into()));
        Ok(())
    }
    pub fn observe(&mut self, metric: &str, target: &str, value: u64) -> ModelResult {
        if !self.profile.metrics.iter().any(|m| m == metric)
            || !self.registry.metrics.contains_key(metric)
        {
            return Err("unknown metric".into());
        }
        if target != self.subject() {
            return Err("observation target is not owned by callback".into());
        }
        self.batch.effects.push(Effect::Observe {
            metric: metric.into(),
            target: target.into(),
            value,
        });
        Ok(())
    }
    pub fn upsert_model_record(&mut self, record: ModelRecord) -> ModelResult {
        if record.subject != self.subject() || record.time_ps != self.now || record.id.is_empty() {
            return Err("invalid record subject, ID or time".into());
        }
        if !self.profile.model_records.contains(&record.schema) {
            return Err("record is outside selected profile".into());
        }
        let descriptor = self
            .registry
            .records
            .get(&record.schema)
            .ok_or_else(|| ModelError("unregistered model record schema".into()))?;
        (descriptor.validate)(&record.data)?;
        self.batch.effects.push(Effect::Record(record));
        Ok(())
    }
}
impl PreparedRegistered {
    pub(crate) fn network(
        network: crate::types::network::PreparedNetwork,
        profile: &str,
        registry: Registry,
    ) -> Self {
        let profile_input = Arc::new(ProfileInput {
            model_config: Some(network.config.clone()),
            workload: Some(network.workload.clone()),
        });
        let builtin_node_count = network.ethernet.devices.len()
            + network.bridge.as_ref().map_or(0, |bridge| {
                bridge.can.controllers.len() + bridge.can.buses.len()
            });
        let _ = profile;
        Self {
            registry: Arc::new(registry),
            models: vec![ModelConfig {
                profile_input,
                id: "@network".into(),
                subject: "@network".into(),
                implementation_key: "dir.network.Coordinator".into(),
                parameters: BTreeMap::new(),
                connections: Vec::new(),
                output_sources: BTreeMap::new(),
            }],
            channels: Vec::new(),
            adapter: None,
            builtin_node_count,
            network: Some(Arc::new(network)),
        }
    }
    pub fn node_count(&self) -> usize {
        if self.adapter.is_some() || self.network.is_some() {
            self.builtin_node_count
        } else {
            self.models
                .iter()
                .filter(|model| model.id == model.subject)
                .count()
        }
    }
    pub fn is_generic(&self) -> bool {
        self.adapter.is_none()
    }
}

impl Default for Registry {
    fn default() -> Self {
        let mut registry = Self::empty();
        registry
            .register_channel(
                "dir.link.FixedDelay",
                |config| {
                    let Some(ParameterValue::Quantity(delay)) = config.parameters.get("delay")
                    else {
                        return Err("FixedDelay requires normalized delay".into());
                    };
                    Ok(Box::new(FixedDelay(*delay)))
                },
                ChannelDescriptor {
                    implementation_key: "dir.link.FixedDelay".into(),
                    implementation_version: "1".into(),
                    parameters: vec![ParameterDescriptor {
                        name: "delay".into(),
                        ned_type: "double".into(),
                        dimension: Dimension::Time,
                        minimum: Some(0.0),
                        maximum: None,
                        required: true,
                    }],
                    capabilities: vec![Schema::new("propagation-delay", 1)],
                },
            )
            .expect("valid built-in channel descriptor");
        registry
            .register_module(
                "dir.network.Coordinator",
                |_| Ok(Box::new(NetworkPlaceholder)),
                ModuleDescriptor {
                    implementation_key: "dir.network.Coordinator".into(),
                    implementation_version: "1".into(),
                    parameters: Vec::new(),
                    ports: Vec::new(),
                    events: vec![
                        Schema::new("dir.network.event", 1),
                        Schema::new("dir.network.control", 1),
                    ],
                    resources: vec!["network".into()],
                },
            )
            .expect("valid private network coordinator");
        for (name, phase) in [("dir.network.event", 1), ("dir.network.control", 0)] {
            let schema = Schema::new(name, 1);
            registry.events.insert(
                schema.clone(),
                EventDescriptor {
                    schema,
                    phase,
                    validate: |payload| {
                        serde_json::from_slice::<Value>(payload)
                            .map(|_| ())
                            .map_err(|error| ModelError(error.to_string()))
                    },
                },
            );
        }
        for name in [
            "can.cc.ideal.v1",
            "can.cc.multibus.v1",
            "can.fd.precomputed.v1",
            "axi4.transaction.v1",
            "soc.shared.v1",
            "ahb.transaction.v1",
            "noc.xy.v1",
            "memory.ipc.transaction.v1",
            "ethernet.l2.store-forward.v1",
            "ethernet.l2.qos.v1",
            "ethernet.l2.vlan.v1",
            "ethernet.l2.store-forward.v2",
            "ethernet.l2.100base-t1.v1",
            "ethernet.l2.dynamic.v1",
            "ethernet.tsn.v1",
            "can.ethernet.gateway.v1",
        ] {
            let catalog = crate::output::profile_catalog(name).expect("built-in profile catalog");
            let metrics = catalog["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    let key = m["metric_id"].as_str().unwrap().to_string();
                    registry
                        .metrics
                        .entry(key.clone())
                        .or_insert_with(|| MetricDescriptor {
                            name: key.clone(),
                            unit: m["unit"].as_str().unwrap_or("").into(),
                        });
                    key
                })
                .collect();
            let records = catalog["model_schemas"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    let schema = Schema::new(
                        m["schema_name"].as_str().unwrap(),
                        m["schema_version"].as_u64().unwrap() as u32,
                    );
                    registry.records.entry(schema.clone()).or_insert_with(|| {
                        ModelRecordDescriptor {
                            schema: schema.clone(),
                            validate: |value| {
                                if value.is_object() {
                                    Ok(())
                                } else {
                                    Err("model record must be an object".into())
                                }
                            },
                        }
                    });
                    schema
                })
                .collect();
            let composed = matches!(
                name,
                "ethernet.l2.dynamic.v1" | "ethernet.tsn.v1" | "can.ethernet.gateway.v1"
            );
            // Built-in preparation is selected by registration provenance,
            // including composed profiles that execute through the generic runtime.
            registry.builtin_profiles.insert(name.into());
            registry.profiles.insert(
                name.into(),
                ProfileDescriptor {
                    name: name.into(),
                    implementation_version: "1".into(),
                    modules: if composed {
                        vec!["dir.network.Coordinator".into()]
                    } else {
                        Vec::new()
                    },
                    channels: Vec::new(),
                    events: if composed {
                        vec![
                            Schema::new("dir.network.event", 1),
                            Schema::new("dir.network.control", 1),
                        ]
                    } else {
                        Vec::new()
                    },
                    metrics,
                    model_records: records,
                    output_schema_version: if name == "can.cc.ideal.v1" { 1 } else { 2 },
                    validate: |_, _| Ok(()),
                },
            );
        }
        registry
    }
}

struct FixedDelay(u64);
impl Channel for FixedDelay {
    fn capability(
        &self,
        capability: &Schema,
        _now_ps: u64,
        _payload: &[u8],
    ) -> ModelResult<ParameterValue> {
        if capability != &Schema::new("propagation-delay", 1) {
            return Err("unsupported FixedDelay capability".into());
        }
        Ok(ParameterValue::Quantity(self.0))
    }
}
