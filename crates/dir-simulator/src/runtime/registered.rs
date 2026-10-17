//! Model-independent owned arena and transactional callback scheduler.
use crate::allocation::{copy_string, reserve_vec};
use crate::registry::*;
use crate::snapshot::{Point, Snapshot};
use crate::types::{Diagnostic, PreparedSimulation};
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

type Key = (u64, u64, u8, u64);
#[derive(Clone)]
pub(crate) enum Work {
    Delivery {
        envelope: Envelope,
        token: Option<TimerToken>,
    },
    Arbitration {
        owner: String,
        resource: String,
    },
}
pub(crate) struct Engine {
    pub(crate) prepared: Arc<PreparedSimulation>,
    pub(crate) registered: PreparedRegistered,
    pub(crate) models: BTreeMap<String, Box<dyn Model>>,
    pub(crate) initialized: BTreeSet<String>,
    pub(crate) preparation_operation: &'static str,
    pub(crate) preparation_target: Option<String>,
    pub(crate) channels: BTreeMap<String, Box<dyn Channel>>,
    pub(crate) queue: BTreeMap<Key, Work>,
    pub(crate) dirty: BTreeSet<(u64, u64, String, String)>,
    pub(crate) live: BTreeSet<TimerToken>,
    pub(crate) sequence: u64,
    pub(crate) event_id: u64,
    pub(crate) last_committed_key: Option<Key>,
    pub(crate) snapshot: Snapshot,
    network_execution: Option<NetworkExecution>,
}
fn invoke<T>(callback: impl FnOnce() -> ModelResult<T>) -> ModelResult<T> {
    catch_unwind(AssertUnwindSafe(callback))
        .unwrap_or_else(|_| Err("model callback panicked".into()))
}
fn runtime_reason(error: &ModelError) -> &'static str {
    match error.0.as_str() {
        "allocation failed committing effects" | "allocation failed sealing arbitration" => {
            "allocation_failed"
        }
        "event ID overflow"
        | "delta overflow"
        | "sequence overflow"
        | "sequence overflow sealing arbitration"
        | "time overflow"
        | "Metric V2 arithmetic overflow" => "arithmetic_overflow",
        "event scheduled in the past" => "past_event",
        "model callback panicked" => "model_panic",
        "unknown destination"
        | "record identity changed or time regressed"
        | "model record data must be an object"
        | "channel is outside this model's connections"
        | "unknown channel"
        | "unregistered channel capability schema/version"
        | "event is outside selected profile"
        | "unknown event schema/version"
        | "event is outside module timer schemas"
        | "unconnected or unowned output port"
        | "send payload does not match port contract"
        | "unowned output handle"
        | "unowned or unknown resource"
        | "unknown metric"
        | "observation target is not owned by callback"
        | "invalid record subject, ID or time"
        | "record is outside selected profile"
        | "unregistered model record schema" => "invalid_event",
        _ => "model_failed",
    }
}
fn record_key(record: &ModelRecord) -> ModelResult<String> {
    if !record.data.is_object() {
        return Err("model record data must be an object".into());
    }
    let capacity = record
        .schema
        .name
        .len()
        .checked_add(1)
        .and_then(|n| n.checked_add(record.id.len()))
        .ok_or_else(|| ModelError("allocation failed committing effects".into()))?;
    let mut key = String::new();
    key.try_reserve(capacity)
        .map_err(|_| ModelError("allocation failed committing effects".into()))?;
    key.push_str(&record.schema.name);
    key.push(':');
    key.push_str(&record.id);
    Ok(key)
}
impl Engine {
    pub(crate) fn new(prepared: Arc<PreparedSimulation>) -> Result<Self, Diagnostic> {
        let registered = prepared
            .registered
            .clone()
            .ok_or_else(|| Diagnostic::prepare("missing registered engine"))?;
        let mut snapshot = Snapshot::empty(&prepared);
        snapshot.registered = Some(RegisteredSnapshot {
            metrics_v2: super::metric_v2::MetricStateV2::new(
                &registered.registry,
                &prepared.common.profile,
            ),
            ..Default::default()
        });
        Ok(Self {
            prepared,
            registered,
            models: BTreeMap::new(),
            initialized: BTreeSet::new(),
            preparation_operation: "allocate",
            preparation_target: None,
            channels: BTreeMap::new(),
            queue: BTreeMap::new(),
            dirty: BTreeSet::new(),
            live: BTreeSet::new(),
            sequence: 0,
            event_id: 0,
            last_committed_key: None,
            snapshot,
            network_execution: None,
        })
    }
    pub(crate) fn fail(
        &mut self,
        error: ModelError,
        key: Option<Key>,
        target: Option<&str>,
        prepare: bool,
        reason: &str,
    ) {
        let mut diagnostic = Diagnostic::execution(error.0)
            .with_runtime(
                if prepare { "prepare" } else { "run" },
                key.map(|k| k.0),
                key.map(|k| k.3),
                target,
            )
            .with_reason(reason);
        if prepare {
            diagnostic.code = "E-0001".into();
            diagnostic.stage = "prepare".into();
            diagnostic.reason = if self.preparation_operation == "allocate" {
                "allocation_failed"
            } else {
                "initialize_failed"
            }
            .into();
        } else if diagnostic.message == "model callback panicked" {
            diagnostic.reason = "model_panic".into();
        }
        if !prepare
            && matches!(
                reason,
                "event_limit" | "delta_cycle_limit" | "arithmetic_overflow" | "allocation_limit"
            )
        {
            diagnostic.code = "E-0004".into();
        }
        diagnostic.details = Some(
            serde_json::json!({ "reason": diagnostic.reason, "operation": if prepare {self.preparation_operation} else {"callback"}, "time_ps": key.map(|k| k.0.to_string()), "event_seq": key.map(|k| k.3.to_string()), "target": target }),
        );
        self.snapshot.common.diagnostics.push(diagnostic);
        self.snapshot.common.termination = if prepare {
            "prep_failed"
        } else {
            "execution_failed"
        }
        .into();
        self.snapshot.common.partial = true;
        self.snapshot.common.end_ps = if self.has_metric_v2() {
            self.snapshot.common.last_event_time_ps.unwrap_or(0)
        } else {
            key.map(|k| k.0).unwrap_or(0)
        };
    }
    fn has_metric_v2(&self) -> bool {
        self.snapshot
            .registered
            .as_ref()
            .is_some_and(|snapshot| !snapshot.metrics_v2.accumulators.is_empty())
    }
    fn commit(
        &mut self,
        owner: &str,
        key: Key,
        batch: EffectBatch,
        next_id: u64,
        initial: bool,
    ) -> ModelResult {
        // Validate every fallible operation before mutating any shared state.
        let mut schedules = Vec::new();
        let schedule_count = batch
            .effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Schedule { .. }))
            .count();
        reserve_vec(&mut schedules, schedule_count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        let mut live_tokens: Vec<Option<TimerToken>> = Vec::new();
        reserve_vec(&mut live_tokens, schedule_count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        let arbitration_count = batch
            .effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Arbitration(_)))
            .count();
        let mut arbitration_owners: Vec<String> = Vec::new();
        reserve_vec(&mut arbitration_owners, arbitration_count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        let observation_count = batch
            .effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Observe { .. }))
            .count();
        let record_count = batch
            .effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Record(_)))
            .count();
        let mut record_ids: Vec<(String, usize)> = Vec::new();
        reserve_vec(&mut record_ids, record_count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        let mut sequence = self.sequence;
        for (effect_index, effect) in batch.effects.iter().enumerate() {
            match effect {
                Effect::Schedule {
                    envelope,
                    token,
                    phase,
                } => {
                    let delta = if envelope.time_ps == key.0 {
                        key.1
                            .checked_add(u64::from(*phase < key.2))
                            .ok_or_else(|| ModelError("delta overflow".into()))?
                    } else {
                        0
                    };
                    if !self.models.contains_key(&envelope.destination) {
                        return Err("unknown destination".into());
                    }
                    schedules.push((envelope.time_ps, delta, *phase, sequence));
                    live_tokens.push(
                        token
                            .as_ref()
                            .map(|token| {
                                copy_string(&token.owner, "commit_effects").map(|owner| {
                                    TimerToken {
                                        owner,
                                        id: token.id,
                                    }
                                })
                            })
                            .transpose()
                            .map_err(|_| {
                                ModelError("allocation failed committing effects".into())
                            })?,
                    );
                    sequence = sequence
                        .checked_add(1)
                        .ok_or_else(|| ModelError("sequence overflow".into()))?;
                }
                Effect::Arbitration(_) => {
                    key.1
                        .checked_add(u64::from(key.2 == 2))
                        .ok_or_else(|| ModelError("delta overflow".into()))?;
                    arbitration_owners.push(
                        copy_string(owner, "commit_effects").map_err(|_| {
                            ModelError("allocation failed committing effects".into())
                        })?,
                    );
                }
                Effect::Record(record) => {
                    let journal_key = record_key(record)?;
                    let existing = record_ids
                        .iter()
                        .rev()
                        .find_map(|(key, index)| {
                            if key != &journal_key {
                                return None;
                            }
                            let Effect::Record(previous) = &batch.effects[*index] else {
                                unreachable!("record index refers to record effect")
                            };
                            Some((
                                &previous.schema,
                                previous.subject.as_str(),
                                previous.time_ps,
                            ))
                        })
                        .or_else(|| {
                            self.snapshot
                                .registered
                                .as_ref()
                                .unwrap()
                                .model_records
                                .get(&journal_key)
                                .map(|r| (&r.schema, r.subject.as_str(), r.time_ps))
                        });
                    if existing.is_some_and(|(schema, subject, time)| {
                        schema != &record.schema
                            || subject != record.subject
                            || time > record.time_ps
                    }) {
                        return Err("record identity changed or time regressed".into());
                    }
                    record_ids.push((journal_key, effect_index));
                }
                _ => {}
            }
        }
        // These are the vector allocations performed by publication and by the
        // successful current-event delivery that follows it. Reserve all three
        // before any committed queue, dirty or journal state is changed.
        self.snapshot
            .common
            .points
            .try_reserve(observation_count)
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        if !initial {
            self.snapshot
                .registered
                .as_mut()
                .unwrap()
                .deliveries
                .try_reserve(1)
                .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        }
        let metrics = self
            .snapshot
            .registered
            .as_ref()
            .unwrap()
            .metrics_v2
            .preflight(
                &self.registered.registry,
                self.prepared.common.metrics_window_ps,
                key.0,
                (!initial).then_some(key.3),
                &batch.effects,
            )?;
        self.snapshot
            .registered
            .as_mut()
            .unwrap()
            .metrics_v2
            .records
            .try_reserve(metrics.records.len())
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        let mut schedules = schedules.into_iter();
        let mut live_tokens = live_tokens.into_iter();
        let mut arbitration_owners = arbitration_owners.into_iter();
        let mut record_keys = record_ids.into_iter().map(|(key, _)| key);
        let mut observation_seq = 0;
        for effect in batch.effects {
            match effect {
                Effect::Schedule {
                    envelope, token, ..
                } => {
                    if let Some(live_token) = live_tokens.next().unwrap() {
                        self.live.insert(live_token);
                    }
                    self.queue.insert(
                        schedules.next().unwrap(),
                        Work::Delivery { envelope, token },
                    );
                }
                Effect::Cancel(token) => {
                    self.live.remove(&token);
                }
                Effect::Arbitration(resource) => {
                    self.dirty.insert((
                        key.0,
                        key.1 + u64::from(key.2 == 2),
                        arbitration_owners.next().unwrap(),
                        resource,
                    ));
                }
                Effect::Observe {
                    metric,
                    target,
                    value,
                } => {
                    self.snapshot.common.points.push(Point {
                        event_seq: (!initial).then_some(key.3),
                        effect_seq: (!initial).then_some(observation_seq),
                        time_ps: key.0,
                        target,
                        metric,
                        value,
                        request_id: None,
                        receiver: None,
                        reason: None,
                    });
                    observation_seq += 1;
                }
                Effect::MetricV2 { .. } => {}
                Effect::Record(record) => {
                    self.snapshot
                        .registered
                        .as_mut()
                        .unwrap()
                        .model_records
                        .insert(record_keys.next().unwrap(), record);
                }
            }
        }
        self.snapshot
            .registered
            .as_mut()
            .unwrap()
            .metrics_v2
            .apply(metrics);
        self.sequence = sequence;
        self.event_id = next_id;
        Ok(())
    }
    pub(crate) fn initialize(&mut self) -> ModelResult {
        if let Some(prepared) = self.registered.network.clone() {
            self.network_execution = Some(NetworkExecution {
                state: super::network::NetworkState::new(prepared)
                    .map_err(|e| ModelError(e.message))?,
                arena: NetworkArena::default(),
            });
            self.network_step(true);
            if self.snapshot.common.partial {
                return Err("network initialization failed".into());
            }
            return Ok(());
        }
        let registered = self.registered.clone();
        for config in &registered.channels {
            self.preparation_target = Some(config.id.clone());
            let channel = invoke(|| {
                (self.registered.registry.channels[&config.implementation_key].factory)(config)
            })?;
            self.channels.insert(config.id.clone(), channel);
        }
        // Complete the arena before initialize, so all resolved destinations exist.
        for config in &registered.models {
            self.preparation_target = Some(config.id.clone());
            let model = invoke(|| {
                (self.registered.registry.modules[&config.implementation_key].factory)(config)
            })?;
            self.models.insert(config.id.clone(), model);
        }
        self.preparation_operation = "initialize";
        let mut batches = Vec::new();
        let mut next_id = self.event_id;
        for config in &registered.models {
            self.preparation_target = Some(config.id.clone());
            let mut batch = EffectBatch::default();
            let model = self.models.get_mut(&config.id).unwrap();
            let mut context = Context {
                now: 0,
                config,
                registry: &self.registered.registry,
                profile: &self.registered.registry.profiles[&self.prepared.common.profile],
                channels: &self.channels,
                channel_configs: &self.registered.channels,
                live_timers: &self.live,
                executing_timer: None,
                next_id: &mut next_id,
                batch: &mut batch,
            };
            invoke(|| model.initialize(&mut context))?;
            self.initialized.insert(config.id.clone());
            batches.push((config.id.clone(), batch));
        }
        // If any init batch is invalid, clear all provisional initialization output.
        for (owner, batch) in batches {
            self.preparation_target = Some(owner.clone());
            if let Err(e) = self.commit(&owner, (0, 0, 0, 0), batch, next_id, true) {
                self.queue.clear();
                self.dirty.clear();
                self.live.clear();
                self.snapshot.common.points.clear();
                self.snapshot.registered = Some(RegisteredSnapshot {
                    metrics_v2: super::metric_v2::MetricStateV2::new(
                        &self.registered.registry,
                        &self.prepared.common.profile,
                    ),
                    ..Default::default()
                });
                return Err(e);
            }
        }
        self.remove_cancelled();
        Ok(())
    }
    fn seal_arbitration(&mut self) -> ModelResult {
        let Some((time, delta, _, _)) = self.dirty.first() else {
            return Ok(());
        };
        let group = (*time, *delta);
        if self
            .queue
            .first_key_value()
            .is_some_and(|(key, _)| (key.0, key.1, key.2) < (group.0, group.1, 2))
        {
            return Ok(());
        }
        let count = self
            .dirty
            .iter()
            .take_while(|(t, d, _, _)| (*t, *d) == group)
            .count();
        let end = self
            .sequence
            .checked_add(count as u64)
            .ok_or_else(|| ModelError("sequence overflow sealing arbitration".into()))?;
        let mut selected = Vec::new();
        reserve_vec(&mut selected, count, "seal_arbitration")
            .map_err(|_| ModelError("allocation failed sealing arbitration".into()))?;
        for (time, delta, owner, resource) in self
            .dirty
            .iter()
            .take_while(|(t, d, _, _)| (*t, *d) == group)
        {
            let owner = copy_string(owner, "seal_arbitration")
                .map_err(|_| ModelError("allocation failed sealing arbitration".into()))?;
            let resource = copy_string(resource, "seal_arbitration")
                .map_err(|_| ModelError("allocation failed sealing arbitration".into()))?;
            selected.push((*time, *delta, owner, resource));
        }
        for (time, delta, owner, resource) in selected {
            self.dirty
                .remove(&(time, delta, owner.clone(), resource.clone()));
            self.queue.insert(
                (time, delta, 2, self.sequence),
                Work::Arbitration { owner, resource },
            );
            self.sequence += 1;
        }
        self.sequence = end;
        Ok(())
    }
    fn remove_cancelled(&mut self) {
        self.queue.retain(|_, work| !matches!(work, Work::Delivery { token: Some(token), .. } if !self.live.contains(token)));
    }
    fn remove_cancelled_head(&mut self) {
        while self.queue.first_key_value().is_some_and(|(_, work)| {
            matches!(work, Work::Delivery { token: Some(token), .. } if !self.live.contains(token))
        }) {
            self.queue.pop_first();
        }
    }
    fn execute(&mut self) {
        while self.snapshot.common.spool_error.is_none() && self.step() {}
    }
    /// Execute exactly one callback transaction, retaining the next candidate.
    pub(crate) fn step(&mut self) -> bool {
        if self.network_execution.is_some() {
            return self.network_step(false);
        }
        let mut committed = false;
        let _event_loop = super::timing::event_loop();
        if self.prepared.common.time_limit_ps == 0 {
            self.remove_cancelled();
            self.snapshot.common.termination = "time_limit".into();
            self.snapshot.common.pending_events = (self.queue.len() + self.dirty.len()) as u64;
            return false;
        }
        'transaction: {
            self.remove_cancelled_head();
            if let Err(e) = self.seal_arbitration() {
                let reason = runtime_reason(&e);
                self.fail(e, None, None, false, reason);
                let time = self.dirty.first().map(|entry| entry.0).unwrap_or(0);
                if !self.has_metric_v2() {
                    self.snapshot.common.end_ps = time;
                }
                let diagnostic = self.snapshot.common.diagnostics.last_mut().unwrap();
                diagnostic.time_ps = Some(time);
                diagnostic.details = Some(serde_json::json!({
                    "reason": reason, "operation": "seal_arbitration",
                    "time_ps": time.to_string(), "event_seq": null, "target": null
                }));
                break 'transaction;
            }
            let Some((&key, work)) = self.queue.first_key_value() else {
                break 'transaction;
            };
            if key.0 >= self.prepared.common.time_limit_ps {
                self.snapshot.common.termination = "time_limit".into();
                break 'transaction;
            }
            let work = work.clone();
            let owner = match &work {
                Work::Delivery { envelope, .. } => envelope.destination.clone(),
                Work::Arbitration { owner, .. } => owner.clone(),
            };
            if self.snapshot.common.committed_events >= self.prepared.common.max_events
                || key.1 >= self.prepared.common.max_delta_cycles
            {
                self.fail(
                    "execution resource limit exceeded".into(),
                    Some(key),
                    Some(&owner),
                    false,
                    if self.snapshot.common.committed_events >= self.prepared.common.max_events {
                        "event_limit"
                    } else {
                        "delta_cycle_limit"
                    },
                );
                break 'transaction;
            }
            let executing_timer = match &work {
                Work::Delivery { token, .. } => token.as_ref(),
                Work::Arbitration { .. } => None,
            };
            let config = self
                .registered
                .models
                .iter()
                .find(|c| c.id == owner)
                .unwrap()
                .clone();
            let mut batch = EffectBatch::default();
            let mut next_id = self.event_id;
            let mut context = Context {
                now: key.0,
                config: &config,
                registry: &self.registered.registry,
                profile: &self.registered.registry.profiles[&self.prepared.common.profile],
                channels: &self.channels,
                channel_configs: &self.registered.channels,
                live_timers: &self.live,
                executing_timer,
                next_id: &mut next_id,
                batch: &mut batch,
            };
            let model = self.models.get_mut(&owner).unwrap();
            let result = invoke(|| match &work {
                Work::Delivery { envelope, .. } => model.on_event(envelope, &mut context),
                Work::Arbitration { resource, .. } => model.on_arbitration(resource, &mut context),
            });
            if let Err(error) =
                result.and_then(|()| self.commit(&owner, key, batch, next_id, false))
            {
                let reason = runtime_reason(&error);
                self.fail(error, Some(key), Some(&owner), false, reason);
                break 'transaction;
            }
            // A failing callback leaves the current event in the committed pending prefix.
            self.queue.remove(&key);
            if let Work::Delivery { envelope, token } = work {
                if let Some(token) = token {
                    self.live.remove(&token);
                }
                self.snapshot
                    .registered
                    .as_mut()
                    .unwrap()
                    .deliveries
                    .push(envelope);
            }
            self.snapshot.common.committed_events += 1;
            self.snapshot.common.last_event_time_ps = Some(key.0);
            self.last_committed_key = Some(key);
            committed = true;
            self.snapshot.common.checkpoint();
            break 'transaction;
        }
        self.remove_cancelled();
        self.snapshot.common.pending_events = (self.queue.len() + self.dirty.len()) as u64;
        committed
    }
    pub(crate) fn finish(&mut self) {
        // Freeze the result observed by every cleanup callback, including on failures.
        let mut failures = Vec::new();
        let context = FinishContext {
            snapshot: &self.snapshot,
        };
        for config in self.registered.models.iter().rev() {
            let id = &config.id;
            if !self.initialized.contains(id) {
                continue;
            }
            let model = self.models.get_mut(id).unwrap();
            if let Err(e) = invoke(|| model.finish(&context)) {
                failures.push((id.clone(), e));
            }
        }
        for (id, channel) in self.channels.iter_mut().rev() {
            if let Err(e) = invoke(|| channel.finish(&context)) {
                failures.push((id.clone(), e));
            }
        }
        for config in self.registered.models.iter().rev() {
            self.models.remove(&config.id);
        }
        while self.channels.pop_last().is_some() {}
        for (target, error) in failures {
            let mut diagnostic = Diagnostic::execution(format!("finish {target}: {error}"))
                .with_reason("finish_failed")
                .with_runtime(
                    "finish",
                    Some(self.snapshot.common.end_ps),
                    None,
                    Some(&target),
                );
            diagnostic.details = Some(
                serde_json::json!({"phase":"finish","target":target,"reason":"finish_failed"}),
            );
            self.snapshot.common.diagnostics.push(diagnostic);
            if !matches!(
                self.snapshot.common.termination.as_str(),
                "prep_failed" | "execution_failed"
            ) {
                self.snapshot.common.termination = "execution_failed".into();
                self.snapshot.common.partial = true;
            }
        }
    }
}
/// Reservable storage for the private staged profiles. All fallible preparation
/// precedes publication; insertion only moves already owned values within capacity.
struct NetworkExecution {
    state: super::network::NetworkState,
    arena: NetworkArena,
}
#[derive(Default)]
struct NetworkArena {
    queue: Vec<(Key, Work)>,
    dirty: Vec<(u64, u64)>,
    records: Vec<(String, ModelRecord)>,
}
struct StagedNetworkRecord {
    key: String,
    ordinal: usize,
    record: ModelRecord,
}
struct PreparedNetworkEffects {
    work: Vec<(Key, Work)>,
    dirty: Vec<(u64, u64)>,
    cancel: Vec<TimerToken>,
    records: Vec<StagedNetworkRecord>,
    record_merge: Option<Vec<(String, ModelRecord)>>,
    points: Vec<Point>,
    sequence: u64,
    event_id: u64,
}
impl NetworkArena {
    fn prepare(
        &mut self,
        key: Key,
        batch: EffectBatch,
        next_id: u64,
        sequence: u64,
        snapshot: &mut Snapshot,
        initial: bool,
    ) -> ModelResult<PreparedNetworkEffects> {
        let owner = "@network";
        let count = batch.effects.len();
        let mut out = PreparedNetworkEffects {
            work: Vec::new(),
            dirty: Vec::new(),
            cancel: Vec::new(),
            records: Vec::new(),
            record_merge: None,
            points: Vec::new(),
            sequence,
            event_id: next_id,
        };
        reserve_vec(&mut out.work, count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        reserve_vec(&mut out.dirty, count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        reserve_vec(&mut out.cancel, count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        reserve_vec(&mut out.records, count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        reserve_vec(&mut out.points, count, "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        for (index, effect) in batch.effects.into_iter().enumerate() {
            match effect {
                Effect::Schedule {
                    envelope,
                    token,
                    phase,
                } => {
                    if envelope.destination != owner {
                        return Err("unknown destination".into());
                    }
                    if envelope.time_ps < key.0 {
                        return Err("event scheduled in the past".into());
                    }
                    let delta = if envelope.time_ps == key.0 {
                        key.1
                            .checked_add(u64::from(phase < key.2))
                            .ok_or_else(|| ModelError("delta overflow".into()))?
                    } else {
                        0
                    };
                    let at = (envelope.time_ps, delta, phase, out.sequence);
                    out.sequence = out
                        .sequence
                        .checked_add(1)
                        .ok_or_else(|| ModelError("sequence overflow".into()))?;
                    out.work.push((at, Work::Delivery { envelope, token }));
                }
                Effect::MetricV2 { .. } => {}
                Effect::Cancel(token) => {
                    if token.owner != owner {
                        return Err("unowned timer cancellation".into());
                    }
                    out.cancel.push(token);
                }
                Effect::Arbitration(resource) => {
                    if resource != "network" {
                        return Err("unowned or unknown resource".into());
                    }
                    let delta = key
                        .1
                        .checked_add(u64::from(key.2 == 2))
                        .ok_or_else(|| ModelError("delta overflow".into()))?;
                    let group = (key.0, delta);
                    if !out.dirty.contains(&group) {
                        out.dirty.push(group);
                    }
                }
                Effect::Record(record) => {
                    let key = record_key(&record)?;
                    out.records.push(StagedNetworkRecord {
                        key,
                        ordinal: index,
                        record,
                    });
                }
                Effect::Observe {
                    metric,
                    target,
                    value,
                } => out.points.push(Point {
                    event_seq: (!initial).then_some(key.3),
                    effect_seq: (!initial).then_some(index as u64),
                    time_ps: key.0,
                    target,
                    metric,
                    value,
                    request_id: None,
                    receiver: None,
                    reason: None,
                }),
            }
        }
        // Validate each key's complete effect history before folding its final
        // upsert. Unstable sort allocates no scratch storage; the ordinal keeps
        // same-key callback order deterministic without a quadratic batch scan.
        out.records.sort_unstable_by(|left, right| {
            left.key
                .cmp(&right.key)
                .then(left.ordinal.cmp(&right.ordinal))
        });
        for index in 0..out.records.len() {
            let staged = &out.records[index];
            let old = if index > 0 && out.records[index - 1].key == staged.key {
                Some(&out.records[index - 1].record)
            } else {
                self.records
                    .binary_search_by(|(name, _)| name.cmp(&staged.key))
                    .ok()
                    .map(|position| &self.records[position].1)
            };
            if old.is_some_and(|record| {
                record.schema != staged.record.schema
                    || record.subject != staged.record.subject
                    || record.time_ps > staged.record.time_ps
            }) {
                return Err("record identity changed or time regressed".into());
            }
        }
        out.records.dedup_by(|later, earlier| {
            if later.key == earlier.key {
                std::mem::swap(later, earlier);
                true
            } else {
                false
            }
        });
        let new_records = out
            .records
            .iter()
            .filter(|record| {
                self.records
                    .binary_search_by(|(name, _)| name.cmp(&record.key))
                    .is_err()
            })
            .count();
        if new_records > 1 {
            // Bulk insertion merges by moving owned records at commit. Reserve
            // its complete final capacity here, avoiding repeated Vec shifts.
            let length = self
                .records
                .len()
                .checked_add(new_records)
                .ok_or_else(|| ModelError("allocation failed committing effects".into()))?;
            let mut merged = Vec::new();
            reserve_vec(&mut merged, length, "commit_record_merge")
                .map_err(|_| ModelError("allocation failed committing effects".into()))?;
            out.record_merge = Some(merged);
        } else {
            reserve_vec(&mut self.records, new_records, "commit_effects")
                .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        }
        reserve_vec(&mut self.queue, out.work.len(), "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        reserve_vec(&mut self.dirty, out.dirty.len(), "commit_effects")
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        reserve_vec(
            &mut snapshot.common.points,
            out.points.len(),
            "commit_effects",
        )
        .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        if !initial {
            reserve_vec(
                &mut snapshot.registered.as_mut().unwrap().deliveries,
                1,
                "commit_effects",
            )
            .map_err(|_| ModelError("allocation failed committing effects".into()))?;
        }
        Ok(out)
    }
    fn commit(&mut self, effects: PreparedNetworkEffects, snapshot: &mut Snapshot) {
        for row in effects.work {
            let i = self
                .queue
                .binary_search_by_key(&row.0, |(k, _)| *k)
                .unwrap_err();
            self.queue.insert(i, row);
        }
        for token in effects.cancel {
            self.queue
                .retain(|(_, work)| !matches!(work,Work::Delivery{token:Some(t),..} if *t==token));
        }
        for group in effects.dirty {
            if let Err(i) = self.dirty.binary_search(&group) {
                self.dirty.insert(i, group);
            }
        }
        self.commit_records(effects.records, effects.record_merge);
        snapshot.common.points.extend(effects.points);
    }
    fn commit_records(
        &mut self,
        records: Vec<StagedNetworkRecord>,
        merged: Option<Vec<(String, ModelRecord)>>,
    ) {
        if let Some(mut merged) = merged {
            let mut existing = std::mem::take(&mut self.records).into_iter().peekable();
            let mut staged = records.into_iter().peekable();
            while let (Some(old), Some(new)) = (existing.peek(), staged.peek()) {
                match old.0.cmp(&new.key) {
                    std::cmp::Ordering::Less => merged.push(existing.next().unwrap()),
                    std::cmp::Ordering::Equal => {
                        existing.next();
                        let new = staged.next().unwrap();
                        merged.push((new.key, new.record));
                    }
                    std::cmp::Ordering::Greater => {
                        let new = staged.next().unwrap();
                        merged.push((new.key, new.record));
                    }
                }
            }
            for record in existing {
                merged.push(record);
            }
            for record in staged {
                merged.push((record.key, record.record));
            }
            self.records = merged;
        } else {
            for record in records {
                match self
                    .records
                    .binary_search_by(|(name, _)| name.cmp(&record.key))
                {
                    Ok(position) => self.records[position].1 = record.record,
                    Err(position) => self.records.insert(position, (record.key, record.record)),
                }
            }
        }
    }
    fn seal(&mut self, sequence: &mut u64) -> ModelResult {
        let Some(&(time, delta)) = self.dirty.first() else {
            return Ok(());
        };
        if self
            .queue
            .first()
            .is_some_and(|(k, _)| (k.0, k.1, k.2) < (time, delta, 2))
        {
            return Ok(());
        }
        let next = sequence
            .checked_add(1)
            .ok_or_else(|| ModelError("sequence overflow sealing arbitration".into()))?;
        reserve_vec(&mut self.queue, 1, "seal_arbitration")
            .map_err(|_| ModelError("allocation failed sealing arbitration".into()))?;
        let work = Work::Arbitration {
            owner: copy_string("@network", "seal_arbitration")
                .map_err(|_| ModelError("allocation failed sealing arbitration".into()))?,
            resource: copy_string("network", "seal_arbitration")
                .map_err(|_| ModelError("allocation failed sealing arbitration".into()))?,
        };
        let key = (time, delta, 2, *sequence);
        let i = self
            .queue
            .binary_search_by_key(&key, |(k, _)| *k)
            .unwrap_err();
        self.queue.insert(i, (key, work));
        self.dirty.remove(0);
        *sequence = next;
        Ok(())
    }
}
impl Engine {
    fn execute_network(&mut self) -> Result<(), Diagnostic> {
        let prepared = self.registered.network.as_ref().unwrap().clone();
        self.network_execution = Some(NetworkExecution {
            state: super::network::NetworkState::new(prepared)?,
            arena: NetworkArena::default(),
        });
        self.network_step(true);
        while !self.snapshot.common.partial
            && self.snapshot.common.spool_error.is_none()
            && self.network_step(false)
        {}
        if self.snapshot.common.partial
            && matches!(
                self.prepared.common.profile.as_str(),
                "can.ethernet.tsn.v1"
                    | "ethernet.redundancy.abstract.v1"
                    | "ethernet.preemption.abstract.v1"
                    | "ethernet.control.wire.experimental.v1"
            )
        {
            // New profiles publish only the last successful virtual boundary.
            // The rejected candidate time remains in its diagnostic.
            self.snapshot.common.end_ps = self.snapshot.common.last_event_time_ps.unwrap_or(0);
        }
        self.refresh_view(self.snapshot.common.end_ps)?;
        Ok(())
    }
    pub(crate) fn refresh_view(&mut self, horizon: u64) -> Result<(), Diagnostic> {
        if let Some(execution) = &self.network_execution {
            if let Some(can) = execution.state.can_snapshot() {
                self.snapshot.can = can;
            }
            self.snapshot.ethernet = Some(execution.state.snapshot());
            self.snapshot.network = Some(execution.state.runtime_snapshot(horizon)?);
            self.snapshot.registered.as_mut().unwrap().model_records =
                execution.arena.records.iter().cloned().collect();
            execution.state.project_prefix_records(
                horizon,
                &mut self.snapshot.registered.as_mut().unwrap().model_records,
            );
            self.snapshot.common.pending_events =
                (execution.arena.queue.len() + execution.arena.dirty.len()) as u64
                    + u64::from(execution.state.future_generation());
        }
        Ok(())
    }
    fn network_step(&mut self, initial: bool) -> bool {
        let mut execution = self
            .network_execution
            .take()
            .expect("initialized network backend");
        let state = &mut execution.state;
        let arena = &mut execution.arena;
        let config = self
            .registered
            .models
            .iter()
            .find(|v| v.id == "@network")
            .expect("prepared network coordinator")
            .clone();
        let first = initial;
        let mut committed = false;

        'transaction: {
            if !first {
                if let Err(error) = arena.seal(&mut self.sequence) {
                    let time = arena.dirty.first().map_or(0, |v| v.0);
                    let reason = runtime_reason(&error);
                    self.fail(
                        error,
                        Some((time, 0, 2, self.sequence)),
                        Some("@network"),
                        false,
                        reason,
                    );
                    break 'transaction;
                }
            }
            let (key, work) = if first {
                ((0, 0, 0, 0), None)
            } else {
                let Some((key, work)) = arena.queue.first() else {
                    break 'transaction;
                };
                if key.0 >= self.prepared.common.time_limit_ps {
                    break 'transaction;
                }
                if self.snapshot.common.committed_events >= self.prepared.common.max_events
                    || key.1 >= self.prepared.common.max_delta_cycles
                {
                    self.fail(
                        "execution resource limit exceeded".into(),
                        Some(*key),
                        Some("@network"),
                        false,
                        if self.snapshot.common.committed_events >= self.prepared.common.max_events
                        {
                            "event_limit"
                        } else {
                            "delta_cycle_limit"
                        },
                    );
                    break 'transaction;
                }
                (*key, Some(work.clone()))
            };
            let mut batch = EffectBatch::default();
            let mut next_id = self.event_id;
            let token = match &work {
                Some(Work::Delivery { token, .. }) => token.as_ref(),
                _ => None,
            };
            let mut context = Context {
                now: key.0,
                config: &config,
                registry: &self.registered.registry,
                profile: &self.registered.registry.profiles[&self.prepared.common.profile],
                channels: &self.channels,
                channel_configs: &self.registered.channels,
                live_timers: &self.live,
                executing_timer: token,
                next_id: &mut next_id,
                batch: &mut batch,
            };
            let planned = if first {
                state.initialize(&mut context, self.prepared.common.time_limit_ps)
            } else {
                match work.as_ref().unwrap() {
                    Work::Delivery { envelope, .. } => {
                        state.event(envelope, &mut context, self.prepared.common.time_limit_ps)
                    }
                    Work::Arbitration { .. } => state.arbitrate(&mut context),
                }
            };
            let prepared = planned.and_then(|delta| {
                state.reserve(&delta)?;
                reserve_vec(
                    &mut self.snapshot.common.points,
                    delta.points.len() + batch.effects.len(),
                    "commit_effects",
                )
                .map_err(|_| Diagnostic::execution("allocation failed committing effects"))?;
                let mut metrics = self
                    .snapshot
                    .registered
                    .as_ref()
                    .unwrap()
                    .metrics_v2
                    .preflight(
                        &self.registered.registry,
                        self.prepared.common.metrics_window_ps,
                        key.0,
                        (!first).then_some(key.3),
                        &batch.effects,
                    )
                    .map_err(|e| Diagnostic::execution(e.0))?;
                metrics.next_record_sequence = metrics
                    .next_record_sequence
                    .checked_add(delta.points.len() as u64)
                    .ok_or_else(|| Diagnostic::execution("Metric V2 arithmetic overflow"))?;
                self.snapshot
                    .registered
                    .as_mut()
                    .unwrap()
                    .metrics_v2
                    .records
                    .try_reserve(metrics.records.len())
                    .map_err(|_| Diagnostic::execution("allocation failed committing effects"))?;
                let effects = arena
                    .prepare(
                        key,
                        batch,
                        next_id,
                        self.sequence,
                        &mut self.snapshot,
                        first,
                    )
                    .map_err(|e| Diagnostic::execution(e.0))?;
                Ok((delta, effects, metrics))
            });
            match prepared {
                Err(error) => {
                    let reason = if error.message.contains("allocation") {
                        "allocation_failed"
                    } else if error.code == "E-0004" || error.message.contains("overflow") {
                        "arithmetic_overflow"
                    } else {
                        "model_failed"
                    };
                    let target = error.target.as_deref().unwrap_or("@network");
                    self.fail(
                        ModelError(error.message),
                        Some(key),
                        Some(target),
                        first,
                        reason,
                    );
                    if let Some(serde_json::Value::Object(details)) = error.details {
                        if let Some(diagnostic) = self.snapshot.common.diagnostics.last_mut() {
                            if let Some(serde_json::Value::Object(context)) =
                                &mut diagnostic.details
                            {
                                for (key, value) in details {
                                    context.entry(key).or_insert(value);
                                }
                            }
                        }
                    }
                    break 'transaction;
                }
                Ok((mut delta, effects, metrics)) => {
                    // No validation/allocation remains after this point.
                    self.sequence = effects.sequence;
                    self.event_id = effects.event_id;
                    for (_, work) in &effects.work {
                        if let Work::Delivery {
                            token: Some(token), ..
                        } = work
                        {
                            self.live.insert(token.clone());
                        }
                    }
                    for token in &effects.cancel {
                        self.live.remove(token);
                    }
                    arena.commit(effects, &mut self.snapshot);
                    self.snapshot
                        .registered
                        .as_mut()
                        .unwrap()
                        .metrics_v2
                        .apply(metrics);
                    for (i, mut point) in delta.points.drain(..).enumerate() {
                        point.event_seq = (!first).then_some(key.3);
                        point.effect_seq = (!first).then_some(i as u64);
                        self.snapshot.common.points.push(point);
                    }
                    state.apply(delta);
                    #[cfg(test)]
                    state.wire_vector_committed();
                }
            }
            if first {
                break 'transaction;
            }
            if let Ok(i) = arena.queue.binary_search_by_key(&key, |(k, _)| *k) {
                arena.queue.remove(i);
            }
            if let Some(Work::Delivery { envelope, token }) = work {
                if let Some(token) = token {
                    self.live.remove(&token);
                }
                self.snapshot
                    .registered
                    .as_mut()
                    .unwrap()
                    .deliveries
                    .push(envelope);
            }
            self.snapshot.common.committed_events += 1;
            self.snapshot.common.last_event_time_ps = Some(key.0);
            self.last_committed_key = Some(key);
            committed = true;
            self.snapshot.common.checkpoint();
            break 'transaction;
        }

        self.snapshot.common.pending_events =
            (arena.queue.len() + arena.dirty.len()) as u64 + u64::from(state.future_generation());
        if !self.snapshot.common.partial {
            self.snapshot.common.termination = if self.prepared.common.time_limit_ps == 0
                || self.snapshot.common.pending_events > 0
            {
                "time_limit"
            } else {
                "events_exhausted"
            }
            .into();
        }
        if let Some(can) = state.can_snapshot() {
            self.snapshot.can = can;
        }
        self.snapshot.ethernet = Some(state.snapshot());
        self.snapshot.network = state
            .runtime_snapshot(self.snapshot.common.last_event_time_ps.unwrap_or(0))
            .ok();
        self.snapshot.registered.as_mut().unwrap().model_records =
            arena.records.iter().cloned().collect();
        self.network_execution = Some(execution);
        committed
    }
}

pub(crate) fn simulate(prepared: &PreparedSimulation) -> Result<Snapshot, Diagnostic> {
    let registered = prepared
        .registered
        .as_ref()
        .ok_or_else(|| Diagnostic::execution("missing registered model input"))?;
    if registered.adapter.is_some() {
        return super::simulate_builtin(prepared);
    }
    let mut engine = Engine::new(Arc::new(prepared.clone()))?;
    if registered.network.is_some() {
        engine.execute_network()?;
        return Ok(engine.snapshot);
    }
    match engine.initialize() {
        Ok(()) => engine.execute(),
        Err(error) => {
            let target = engine.preparation_target.clone();
            engine.fail(error, None, target.as_deref(), true, "model_failed");
        }
    }
    // Freeze the journal before handing it to cleanup callbacks, which may
    // retain read-only spool handles beyond this run.
    if engine.snapshot.common.spool_error.is_none() {
        if let Err(diagnostic) = engine.snapshot.common.spool_checkpoint(true) {
            engine.snapshot.common.spool_error = Some(diagnostic);
        }
    }
    engine.finish();
    Ok(engine.snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn failed_arbitration_reservation_keeps_dirty_resources_and_sequence() {
        let config = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/gateway/fanout.ini");
        let mut prepared = crate::prepare(&config).unwrap();
        prepared.registered = Some(PreparedRegistered {
            registry: Arc::new(Registry::new()),
            models: Vec::new(),
            channels: Vec::new(),
            adapter: None,
            builtin_node_count: 0,
            network: None,
        });
        let registered = prepared.registered.as_ref().unwrap();
        let mut snapshot = Snapshot::empty(&prepared);
        snapshot.registered = Some(RegisteredSnapshot::default());
        let mut engine = Engine {
            prepared: Arc::new(prepared.clone()),
            registered: registered.clone(),
            models: BTreeMap::new(),
            initialized: BTreeSet::new(),
            preparation_operation: "initialize",
            preparation_target: None,
            channels: BTreeMap::new(),
            queue: BTreeMap::new(),
            dirty: BTreeSet::from([(4, 0, "owner".into(), "resource".into())]),
            live: BTreeSet::new(),
            sequence: 7,
            event_id: 0,
            last_committed_key: None,
            snapshot,
            network_execution: None,
        };
        crate::allocation::fail_next_reservation("seal_arbitration");
        let error = engine.seal_arbitration().unwrap_err();
        assert_eq!(runtime_reason(&error), "allocation_failed");
        assert_eq!(engine.dirty.len(), 1);
        assert!(engine.queue.is_empty());
        assert_eq!(engine.sequence, 7);
    }
    fn network_prepared() -> PreparedSimulation {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet/dynamic/unicast.ini");
        crate::prepare(&path).unwrap()
    }
    #[test]
    fn staged_effect_validation_keeps_frame_queue_and_failed_event_uncommitted() {
        let mut prepared = network_prepared();
        let registry = Arc::make_mut(&mut prepared.registered.as_mut().unwrap().registry);
        registry
            .profiles
            .get_mut("ethernet.l2.dynamic.v1")
            .unwrap()
            .model_records
            .retain(|s| s.name != "ethernet.dynamic.transfer");
        let snapshot = simulate(&prepared).unwrap();
        assert_eq!(snapshot.common.termination, "execution_failed");
        assert_eq!(snapshot.common.committed_events, 1);
        assert_eq!(snapshot.common.pending_events, 1);
        let eth = snapshot.ethernet.unwrap();
        assert!(eth.frames.is_empty());
        assert!(eth.transfers.is_empty());
        assert!(eth.receptions.is_empty());
        assert_eq!(snapshot.registered.unwrap().model_records.len(), 1);
    }
    #[test]
    fn staged_sof_overflow_preserves_admitted_copy_without_planned_transmission() {
        let mut prepared = network_prepared();
        prepared.common.time_limit_ps = u64::MAX;
        let network = Arc::make_mut(
            prepared
                .registered
                .as_mut()
                .unwrap()
                .network
                .as_mut()
                .unwrap(),
        );
        network.ethernet.generators[0].times_ps = vec![u64::MAX - 1];
        let source = network.ethernet.generators[0].source;
        network.ethernet.devices[source].tx_processing_delay_ps = 0;
        let snapshot = simulate(&prepared).unwrap();
        assert_eq!(snapshot.common.termination, "execution_failed");
        assert_eq!(snapshot.common.committed_events, 2);
        assert_eq!(snapshot.common.pending_events, 1);
        let eth = snapshot.ethernet.unwrap();
        assert_eq!(eth.frames.len(), 1);
        assert_eq!(eth.transfers.len(), 1);
        assert_eq!(eth.transfers[0].status, "queued");
        assert_eq!(eth.transfers[0].sof_ps, None);
        assert_eq!(eth.transfers[0].planned_eof_ps, None);
        assert!(eth.receptions.is_empty());
    }
    #[test]
    fn staged_publication_reservation_failure_keeps_fes_and_journal_prefix() {
        let prepared = network_prepared();
        let mut snapshot = Snapshot::empty(&prepared);
        snapshot.registered = Some(RegisteredSnapshot::default());
        let work = Work::Arbitration {
            owner: "@network".into(),
            resource: "network".into(),
        };
        let mut arena = NetworkArena {
            queue: vec![((7, 0, 2, 3), work)],
            dirty: vec![(8, 0)],
            records: vec![],
        };
        let record = ModelRecord {
            schema: Schema::new("ethernet.dynamic.policy", 1),
            id: "test".into(),
            subject: "@network".into(),
            time_ps: 7,
            data: serde_json::json!({}),
        };
        let batch = EffectBatch {
            effects: vec![
                Effect::Record(record),
                Effect::Arbitration("network".into()),
            ],
        };
        crate::allocation::fail_next_reservation("commit_effects");
        assert!(
            arena
                .prepare((7, 0, 2, 3), batch, 11, 12, &mut snapshot, false)
                .is_err()
        );
        assert_eq!(arena.queue.len(), 1);
        assert_eq!(arena.queue[0].0, (7, 0, 2, 3));
        assert_eq!(arena.dirty, [(8, 0)]);
        assert!(arena.records.is_empty());
        assert!(snapshot.registered.unwrap().deliveries.is_empty());
    }
    #[test]
    fn staged_non_object_record_preserves_committed_journal_and_reservations() {
        let prepared = network_prepared();
        let mut snapshot = Snapshot::empty(&prepared);
        snapshot.registered = Some(RegisteredSnapshot::default());
        let prefix = ModelRecord {
            schema: Schema::new("ethernet.dynamic.policy", 1),
            id: "test".into(),
            subject: "@network".into(),
            time_ps: 6,
            data: serde_json::json!({"state":"committed"}),
        };
        let name = record_key(&prefix).unwrap();
        snapshot
            .registered
            .as_mut()
            .unwrap()
            .model_records
            .insert(name.clone(), prefix.clone());
        let mut arena = NetworkArena {
            queue: vec![(
                (7, 0, 2, 3),
                Work::Arbitration {
                    owner: "@network".into(),
                    resource: "network".into(),
                },
            )],
            dirty: vec![(8, 0)],
            records: vec![(name, prefix.clone())],
        };
        let mut invalid = prefix.clone();
        invalid.time_ps = 7;
        invalid.data = serde_json::json!(5);
        let batch = EffectBatch {
            effects: vec![
                Effect::Arbitration("network".into()),
                Effect::Record(invalid),
            ],
        };
        let error = arena
            .prepare((7, 0, 2, 3), batch, 11, 12, &mut snapshot, false)
            .err()
            .unwrap();
        assert_eq!(runtime_reason(&error), "invalid_event");
        assert_eq!(arena.queue.len(), 1);
        assert_eq!(arena.queue[0].0, (7, 0, 2, 3));
        assert_eq!(arena.dirty, [(8, 0)]);
        assert_eq!(arena.records.len(), 1);
        assert_eq!(arena.records[0].1.data, prefix.data);
        assert!(snapshot.common.points.is_empty());
        assert_eq!(
            snapshot
                .registered
                .unwrap()
                .model_record("ethernet.dynamic.policy", "test")
                .unwrap()
                .data,
            prefix.data
        );
    }
    #[test]
    fn staged_never_eligible_retains_queue_without_infinite_gate_ticks() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet/tsn/tas.ini");
        let mut prepared = crate::prepare(&path).unwrap();
        let network = Arc::make_mut(
            prepared
                .registered
                .as_mut()
                .unwrap()
                .network
                .as_mut()
                .unwrap(),
        );
        let tsn = network.tsn.as_mut().unwrap();
        tsn.updates.clear();
        for output in &mut tsn.outputs {
            if let Some(schedule) = &mut output.tas {
                let schedule = Arc::make_mut(schedule);
                for entry in &mut schedule.entries {
                    entry.open_mask = 0;
                }
                schedule.open_total_ps = [0; 8];
                schedule.class_open_runs = std::array::from_fn(|_| vec![]);
            }
        }
        let snapshot = simulate(&prepared).unwrap();
        assert_eq!(snapshot.common.termination, "events_exhausted");
        assert!(!snapshot.common.partial);
        assert_eq!(snapshot.common.pending_events, 0);
        assert!(snapshot.common.committed_events < 10);
        let ethernet = snapshot.ethernet.unwrap();
        assert!(!ethernet.transfers.is_empty());
        assert!(
            ethernet
                .transfers
                .iter()
                .all(|t| t.status == "queued" && t.sof_ps.is_none())
        );
        assert!(ethernet.receptions.is_empty());
        assert!(
            snapshot
                .registered
                .unwrap()
                .model_records
                .values()
                .any(|r| r.schema.name == "ethernet.tsn.decision"
                    && r.data["state"] == "never_eligible")
        );
    }
    #[test]
    fn staged_duplicate_time_generation_obeys_generator_id_then_ordinal() {
        let mut prepared = network_prepared();
        let network = Arc::make_mut(
            prepared
                .registered
                .as_mut()
                .unwrap()
                .network
                .as_mut()
                .unwrap(),
        );
        let mut first = network.ethernet.generators[0].clone();
        first.id = "a".into();
        first.times_ps = vec![0, 0];
        let mut second = first.clone();
        second.id = "b".into();
        second.times_ps = vec![0];
        network.ethernet.generators = vec![first, second];
        let snapshot = simulate(&prepared).unwrap();
        assert_eq!(snapshot.common.termination, "events_exhausted");
        assert_eq!(
            snapshot
                .ethernet
                .unwrap()
                .frames
                .into_iter()
                .map(|f| f.frame_id)
                .collect::<Vec<_>>(),
            ["a:0", "a:1", "b:0"]
        );
    }
    #[test]
    fn staged_filtered_receptions_do_not_reuse_visit_ids() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet/dynamic/membership.ini");
        let prepared = crate::prepare(&path).unwrap();
        let snapshot = simulate(&prepared).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        let mut filtered = 0;
        for row in snapshot
            .registered
            .unwrap()
            .model_records
            .values()
            .filter(|r| r.schema.name == "ethernet.dynamic.reception")
        {
            assert!(seen.insert(row.data["visit_id"].as_str().unwrap().to_owned()));
            if row.data["status"] == "filtered" {
                filtered += 1;
            }
        }
        assert!(filtered > 0);
        assert!(seen.len() > filtered);
    }
    fn bulk_record(id: usize, time: u64, value: u64) -> ModelRecord {
        ModelRecord {
            schema: Schema::new("test.bulk", 1),
            id: format!("id-{id:05}"),
            subject: "@network".into(),
            time_ps: time,
            data: serde_json::json!({"value":value.to_string()}),
        }
    }
    #[test]
    fn staged_large_record_batch_sorts_validates_folds_and_merges_without_commit_allocation() {
        let prepared = network_prepared();
        let mut snapshot = Snapshot::empty(&prepared);
        snapshot.registered = Some(RegisteredSnapshot::default());
        let old = bulk_record(1, 1, 100);
        let untouched = bulk_record(9999, 1, 99);
        let mut arena = NetworkArena {
            records: vec![
                (record_key(&old).unwrap(), old),
                (record_key(&untouched).unwrap(), untouched),
            ],
            ..Default::default()
        };
        let mut effects = (0..4094)
            .rev()
            .map(|id| Effect::Record(bulk_record(id, 7, id as u64)))
            .collect::<Vec<_>>();
        effects.push(Effect::Record(bulk_record(500, 8, 5000)));
        effects.push(Effect::Record(bulk_record(500, 9, 5001)));
        let staged = arena
            .prepare(
                (9, 0, 1, 0),
                EffectBatch { effects },
                0,
                0,
                &mut snapshot,
                false,
            )
            .unwrap();
        assert_eq!(staged.records.len(), 4094);
        assert_eq!(arena.records.len(), 2);
        let merge = staged.record_merge.as_ref().unwrap();
        assert!(merge.capacity() >= 4095);
        let merge_buffer = merge.as_ptr();
        arena.commit(staged, &mut snapshot);
        assert_eq!(arena.records.as_ptr(), merge_buffer);
        assert_eq!(arena.records.len(), 4095);
        assert!(arena.records.windows(2).all(|rows| rows[0].0 < rows[1].0));
        let record = &arena.records[arena
            .records
            .binary_search_by(|(key, _)| key.as_str().cmp("test.bulk:id-00500"))
            .unwrap()]
        .1;
        assert_eq!(record.time_ps, 9);
        assert_eq!(record.data["value"], "5001");
        assert_eq!(arena.records.last().unwrap().1.data["value"], "99");
    }
    #[test]
    fn staged_record_batch_rejects_invalid_intermediate_upsert_and_merge_allocation_failure() {
        let prepared = network_prepared();
        let mut snapshot = Snapshot::empty(&prepared);
        snapshot.registered = Some(RegisteredSnapshot::default());
        let old = bulk_record(1, 1, 100);
        let mut arena = NetworkArena {
            records: vec![(record_key(&old).unwrap(), old)],
            ..Default::default()
        };
        let invalid = EffectBatch {
            effects: vec![
                Effect::Record(bulk_record(1, 9, 9)),
                Effect::Record(bulk_record(1, 8, 8)),
                Effect::Record(bulk_record(1, 10, 10)),
            ],
        };
        assert!(
            arena
                .prepare((10, 0, 1, 0), invalid, 0, 0, &mut snapshot, false)
                .err()
                .unwrap()
                .0
                .contains("time regressed")
        );
        assert_eq!(arena.records[0].1.time_ps, 1);
        crate::allocation::fail_next_reservation("commit_record_merge");
        let effects = (0..4094)
            .map(|id| Effect::Record(bulk_record(id, 7, id as u64)))
            .collect();
        let error = arena
            .prepare(
                (7, 0, 1, 0),
                EffectBatch { effects },
                0,
                0,
                &mut snapshot,
                false,
            )
            .err()
            .unwrap();
        assert_eq!(runtime_reason(&error), "allocation_failed");
        assert_eq!(arena.records.len(), 1);
        assert_eq!(arena.records[0].1.time_ps, 1);
        assert!(arena.queue.is_empty());
        assert!(snapshot.common.points.is_empty());
    }
}

#[cfg(test)]
crate::runtime::network::network_vector_tests::vector_network_arena_adapter!();

/// Test adapter for a seeded wire control callback through actual publication.
#[cfg(test)]
pub(crate) fn vector_wire_successful_callback(
    prepared: PreparedSimulation,
    state: crate::runtime::network::NetworkState,
    event: Envelope,
) -> (crate::runtime::network::NetworkState, serde_json::Value) {
    let mut engine = Engine::new(Arc::new(prepared)).unwrap();
    let mut arena = NetworkArena::default();
    let phase = if event.schema.name == "dir.network.control" {
        0
    } else {
        1
    };
    let key = (event.time_ps, 0, phase, event.event_id);
    arena.queue.push((
        key,
        Work::Delivery {
            envelope: event,
            token: None,
        },
    ));
    engine.network_execution = Some(NetworkExecution { state, arena });
    assert!(
        engine.network_step(false),
        "{:?}",
        engine.snapshot.common.diagnostics
    );
    assert!(
        !engine.snapshot.common.partial,
        "{:?}",
        engine.snapshot.common.diagnostics
    );
    let execution = engine.network_execution.take().unwrap();
    let publication = serde_json::json!({
        "committed_events": engine.snapshot.common.committed_events,
        "records": execution.arena.records,
    });
    (execution.state, publication)
}
