//! Static source-routed replication in the shared network transaction.
use super::*;
use crate::types::network::PreparedReplication;
use crate::types::network::{ReplicationRecoveryAlgorithm, ReplicationWireFormat};

type ReplicationRecoveryResult = (&'static str, String, Option<u64>, String, Option<u64>);

pub(super) fn identifier(parts: &[&str]) -> String {
    use std::fmt::Write;
    let mut id = String::new();
    for part in parts {
        write!(&mut id, "{}:{part}", part.len()).expect("writing into String");
    }
    id
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Circuit {
    pub branch: String,
    pub hop: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distributed: Option<DistributedCursor>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DistributedCursor {
    pub flow: String,
    pub leg: DistributedLeg,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DistributedLeg {
    Prefix,
    Branch { path: usize },
    Suffix,
    SuffixRound { round: usize },
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DistributedFlow {
    pub frame: usize,
    pub stream: usize,
    pub created_ps: u64,
    pub sequence: Option<u16>,
    pub prefix_reception: Option<usize>,
    pub winner_reception: Option<usize>,
    pub continued: bool,
    pub suffix_root: Option<usize>,
    pub delivery_reception: Option<usize>,
    pub terminal_ps: Option<u64>,
    pub dropped: bool,
    pub prefix: DistributedPhaseState,
    pub branches: Vec<DistributedPhaseState>,
    pub suffix: DistributedPhaseState,
    pub recoveries: Vec<DistributedRecovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rounds: Vec<ContinuationRound>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContinuationRound {
    pub id: String,
    pub recovery_order: u64,
    pub input_reception: usize,
    pub suffix_root: usize,
    pub suffix: DistributedPhaseState,
    pub delivery_reception: Option<usize>,
}
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DistributedPhaseState {
    pub current_transfer: Option<usize>,
    pub first_sof_ps: Option<u64>,
    pub terminal_ps: Option<u64>,
    pub failure_reason: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DistributedRecovery {
    pub order: u64,
    pub reception: usize,
    pub time_ps: u64,
    pub decision: String,
    pub continuation: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SequenceWindow {
    pub high: Option<u64>,
    pub words: Vec<u64>,
    capacity: usize,
    #[serde(default)]
    modulus: Option<u64>,
}
impl SequenceWindow {
    #[cfg(test)]
    pub fn new(capacity: usize) -> Self {
        Self {
            high: None,
            words: vec![0; capacity.div_ceil(64)],
            capacity,
            modulus: None,
        }
    }
    pub fn hex(&self) -> String {
        (0..self.capacity.div_ceil(4))
            .rev()
            .map(|n| {
                char::from_digit(
                    ((self.words[(n * 4) / 64] >> ((n * 4) % 64)) & 15) as u32,
                    16,
                )
                .unwrap()
            })
            .collect()
    }
    /// Comparison precedes subtraction; large advances clear before shifting.
    pub fn recover(&mut self, sequence: u64) -> &'static str {
        if self.modulus.is_some_and(|modulus| sequence >= modulus) {
            return "invalid_sequence";
        }
        let Some(high) = self.high else {
            self.high = Some(sequence);
            self.words[0] = 1;
            return "accept_first";
        };
        let advance = if let Some(modulus) = self.modulus {
            let distance = (sequence + modulus - high) % modulus;
            if distance == modulus / 2 {
                return "ambiguous";
            }
            (distance > 0 && distance < modulus / 2).then_some(distance)
        } else {
            (sequence > high).then_some(sequence.saturating_sub(high))
        };
        if let Some(d) = advance {
            if d >= self.capacity as u64 {
                self.words.fill(0);
            } else {
                let word = (d / 64) as usize;
                let bits = (d % 64) as u32;
                for i in (0..self.words.len()).rev() {
                    self.words[i] = if i >= word {
                        (self.words[i - word] << bits)
                            | if bits != 0 && i > word {
                                self.words[i - word - 1] >> (64 - bits)
                            } else {
                                0
                            }
                    } else {
                        0
                    };
                }
            }
            self.high = Some(sequence);
            self.words[0] |= 1;
            let rem = self.capacity % 64;
            if rem != 0 {
                let last = self.words.len() - 1;
                self.words[last] &= (1u64 << rem) - 1;
            }
            "accept_advance"
        } else {
            let i = if let Some(modulus) = self.modulus {
                (high + modulus - sequence) % modulus
            } else {
                high - sequence
            };
            if i >= self.capacity as u64 {
                return "stale";
            }
            let word = i as usize / 64;
            let bit = 1u64 << (i % 64);
            if self.words[word] & bit != 0 {
                "duplicate"
            } else {
                self.words[word] |= bit;
                "accept_late"
            }
        }
    }
}

#[cfg(test)]
mod rtag_recovery_tests {
    use super::*;

    #[test]
    fn wraps_without_erasing_recent_duplicates_or_guessing_half_range() {
        let mut history = SequenceWindow {
            high: None,
            words: vec![0; 1],
            capacity: 8,
            modulus: Some(65536),
        };
        for sequence in [65534, 65535, 0, 2] {
            assert!(history.recover(sequence).starts_with("accept_"));
        }
        assert_eq!(history.recover(65535), "duplicate");
        assert_eq!(history.recover(1), "accept_late");
        assert_eq!(history.recover(65530), "stale");
        assert_eq!(history.recover(32770), "ambiguous");
        assert_eq!(history.high, Some(2));
    }
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StreamState {
    pub next: u64,
    pub exhausted: bool,
    pub history: SequenceWindow,
    #[serde(default)]
    pub last_recovery_ps: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub timer_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_deadline_ps: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reset_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reset_events: Vec<RecoveryReset>,
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryReset {
    pub time_ps: u64,
    pub generation: u64,
}
pub(super) const RECOVERY_TICK_PS: u64 = 1_000_000_000;
impl StreamState {
    /// Signed modulo-65536 Vector_Alg; H is strictly less than the half range.
    pub(super) fn vector_recover(
        &mut self,
        sequence: u64,
        now: u64,
        timeout: u64,
    ) -> Result<&'static str> {
        if sequence >= 65536 {
            return Ok("invalid_sequence");
        }
        let decision = if let Some(high) = self.history.high {
            let raw = (sequence as u16).wrapping_sub(high as u16);
            let delta = i32::from(raw as i16);
            let h = self.history.capacity as i32;
            if delta >= h || delta <= -h {
                "rogue"
            } else {
                self.history.recover(sequence)
            }
        } else {
            self.history.recover(sequence)
        };
        if !decision.starts_with("accept_") {
            return Ok(decision);
        }
        let generation = add(self.timer_generation, 1)?;
        let deadline = (now / RECOVERY_TICK_PS)
            .checked_mul(RECOVERY_TICK_PS)
            .and_then(|epoch| epoch.checked_add(timeout))
            .ok_or_else(|| error("recovery timer deadline overflow"))?;
        self.timer_generation = generation;
        self.recovery_deadline_ps = Some(deadline);
        self.last_recovery_ps = Some(now);
        Ok(if decision == "accept_first" && self.reset_count > 1 {
            "accept_reset"
        } else {
            decision
        })
    }
    pub(super) fn timeout(&mut self, now: u64, generation: u64) -> Result<bool> {
        if self.timer_generation != generation || self.recovery_deadline_ps != Some(now) {
            return Ok(false);
        }
        self.history.high = None;
        self.history.words.fill(0);
        self.recovery_deadline_ps = None;
        self.reset_count = add(self.reset_count, 1)?;
        self.reset_events.push(RecoveryReset {
            time_ps: now,
            generation,
        });
        Ok(true)
    }

    pub(super) fn allocate(&mut self) -> Result<u64> {
        if let Some(modulus) = self.history.modulus {
            let next = self.next;
            self.next = (next + 1) % modulus;
            return Ok(next);
        }
        if self.exhausted {
            let mut d =
                error("replication sequence exhausted (no wrap)").with_reason("sequence_exhausted");
            d.code = "E-0004".into();
            return Err(d);
        }
        let next = self.next;
        if next == u64::MAX {
            self.exhausted = true;
        } else {
            self.next = add(next, 1)?;
        }
        Ok(next)
    }
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Branch {
    pub stream: usize,
    pub path: usize,
    pub origin: String,
    pub sequence: u64,
    pub data: Value,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EndpointRecovery {
    pub stream: usize,
    pub recovery: DistributedRecovery,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SequenceAllocation {
    pub stream: usize,
    pub frame: usize,
    pub time_ps: u64,
    pub sequence: u16,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplicationState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoint_recoveries: Vec<EndpointRecovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequence_allocations: Vec<SequenceAllocation>,
    pub streams: Vec<StreamState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub distributed: BTreeMap<String, DistributedFlow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distributed_identity: Option<Value>,
    pub branches: BTreeMap<String, Branch>,
    pub origins: BTreeMap<String, Value>,
    #[serde(deserialize_with = "super::checkpoint::records::deserialize")]
    pub records: Vec<(&'static str, String, Value)>,
}
impl ReplicationState {
    pub fn new(p: &PreparedReplication) -> Self {
        Self {
            endpoint_recoveries: Vec::new(),
            sequence_allocations: Vec::new(),
            streams: p
                .streams
                .iter()
                .map(|s| StreamState {
                    next: 0,
                    exhausted: false,
                    last_recovery_ps: None,
                    timer_generation: 0,
                    recovery_deadline_ps: None,
                    reset_count: u64::from(
                        s.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017,
                    ),
                    reset_events: Vec::new(),
                    history: SequenceWindow {
                        high: None,
                        words: s.initial_history_words.clone(),
                        capacity: s.history_capacity,
                        modulus: (s.wire_format == ReplicationWireFormat::Rtag8021cb2017)
                            .then_some(65536),
                    },
                })
                .collect(),
            distributed: BTreeMap::new(),
            distributed_identity: distributed::prepared_identity(p),
            branches: BTreeMap::new(),
            origins: BTreeMap::new(),
            records: Vec::new(),
        }
    }
    fn publish_branch(&mut self, id: &str) {
        self.records.push((
            "dir.redundancy.branch",
            id.into(),
            self.branches[id].data.clone(),
        ));
    }
    fn publish_origin(&mut self, id: &str) {
        self.records
            .push(("dir.redundancy.origin", id.into(), self.origins[id].clone()));
    }
    fn prune_finished(&mut self) {
        let finished = self
            .origins
            .iter()
            .filter(|(_, o)| {
                o["branch_ids"].as_array().unwrap().iter().all(|id| {
                    self.branches
                        .get(id.as_str().unwrap())
                        .is_some_and(|b| b.data["completed_ps"].is_string())
                })
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in finished {
            if let Some(origin) = self.origins.remove(&id) {
                for branch in origin["branch_ids"].as_array().unwrap() {
                    self.branches.remove(branch.as_str().unwrap());
                }
            }
        }
    }
    pub fn snapshot(&self, now: u64) -> Value {
        let mut value = json!({"streams":self.streams.iter().map(|s|{ let mut row = json!({"next_sequence":s.next.to_string(),"exhausted":s.exhausted,"high":opt(s.history.high),"bitmap_hex":s.history.hex(),"last_recovery_ps":opt(s.last_recovery_ps)}); if s.reset_count != 0 { row["recovery_algorithm"] = json!("vector_8021cb_2017"); row["take_any"] = json!(s.history.high.is_none()); row["recov_seq_num"] = json!(s.history.high.unwrap_or(65535).to_string()); row["timer_generation"] = json!(s.timer_generation.to_string()); row["recovery_deadline_ps"] = opt(s.recovery_deadline_ps); row["remaining_ticks"] = json!(s.recovery_deadline_ps.map_or(0, |deadline| deadline.saturating_sub(now / RECOVERY_TICK_PS * RECOVERY_TICK_PS) / RECOVERY_TICK_PS).to_string()); row["reset_count"] = json!(s.reset_count.to_string()); row["timeout_events"] = decimal(json!(s.reset_events)); } row }).collect::<Vec<_>>(),"branches":self.branches.values().filter(|b|b.data["completed_ps"].is_null()).map(|b|b.data.clone()).collect::<Vec<_>>()});
        if !self.distributed.is_empty() {
            value["distributed_flows"] = decimal(json!(self.distributed));
        }
        value
    }
}
impl NetworkState {
    pub(super) fn replication_recover(
        &self,
        stream_index: usize,
        sequence: u64,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<ReplicationRecoveryResult> {
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[stream_index];
        let stream = &mut self.replication_mut(d)?.streams[stream_index];
        let mut before = stream.history.hex();
        let mut previous = stream.history.high;
        let decision =
            if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
                stream.vector_recover(sequence, now, config.recovery_timeout_ps.unwrap())?
            } else {
                let timed_out = config.recovery_timeout_ps.is_some_and(|timeout| {
                    stream
                        .last_recovery_ps
                        .is_some_and(|last| now.saturating_sub(last) >= timeout)
                });
                if timed_out {
                    stream.history.high = None;
                    stream.history.words.fill(0);
                    before = stream.history.hex();
                    previous = stream.history.high;
                }
                let decision = stream.history.recover(sequence);
                stream.last_recovery_ps = Some(now);
                if timed_out && decision == "accept_first" {
                    "accept_reset"
                } else {
                    decision
                }
            };
        let after = stream.history.hex();
        let high = stream.history.high;
        let timer = if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017
            && decision.starts_with("accept_")
        {
            Some((
                stream.recovery_deadline_ps.unwrap(),
                stream.timer_generation,
            ))
        } else {
            None
        };
        if let Some((at, generation)) = timer {
            crate::allocation::reservation_checkpoint("network_recovery_timer")?;
            let due = self.due_mut(d);
            due.retain(|item| !matches!(item.kind, Completion::RecoveryTimeout { stream, .. } if stream == stream_index));
            due.push(Due {
                at,
                kind: Completion::RecoveryTimeout {
                    stream: stream_index,
                    generation,
                },
            });
        }
        Ok((decision, before, previous, after, high))
    }
    pub(super) fn replication_timeout(
        &self,
        stream: usize,
        generation: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[stream];
        if config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017 {
            return Err(error("recovery timer on project stream"));
        }
        let state = self.replication_mut(d)?;
        if state.streams[stream].timeout(c.now(), generation)? {
            let id = identifier(&[&config.stream_id, "timeout", &generation.to_string()]);
            state.records.push(("dir.redundancy.timeout", id.clone(), json!({"timeout_id":id,"stream_id":config.stream_id,"generation":generation.to_string(),"time_ps":c.now().to_string(),"reset_count":state.streams[stream].reset_count.to_string()})));
        }
        Ok(())
    }
    pub(super) fn replication_mut<'a>(
        &self,
        d: &'a mut NetworkDelta,
    ) -> Result<&'a mut ReplicationState> {
        if d.replication.is_none() {
            d.replication = self.replication.clone();
        }
        d.replication
            .as_mut()
            .ok_or_else(|| error("missing replication state"))
    }
    pub(super) fn replication_source_stream(&self, frame: &Frame) -> Option<usize> {
        let p = self.prepared.extensions.as_ref()?.replication.as_ref()?;
        if let Some(g) = frame.generator {
            p.generator_stream
                .get(&self.prepared.ethernet.generators.get(g)?.id)
                .copied()
        } else {
            let source = &frame.gateway_injection.as_ref()?.selector;
            p.streams.iter().position(|s| &s.source == source)
        }
    }
    pub(super) fn validate_replication_frame_source(
        &self,
        frame: &Frame,
        stream: usize,
        delta: Option<&BridgeDelta>,
    ) -> Result<()> {
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[stream];
        match &config.source {
            crate::types::network::ReplicationSource::Generator { id } => {
                if frame.gateway_injection.is_some()
                    || frame
                        .generator
                        .and_then(|g| self.prepared.ethernet.generators.get(g))
                        .is_none_or(|g| g.id != *id)
                {
                    return Err(error("replication generator provenance mismatch"));
                }
            }
            selector @ crate::types::network::ReplicationSource::GatewayEgress { .. } => {
                let injection = frame
                    .gateway_injection
                    .as_ref()
                    .ok_or_else(|| error("Gateway replication provenance missing"))?;
                let evidence = self
                    .bridge
                    .as_ref()
                    .ok_or_else(|| error("Gateway replication Bridge missing"))?
                    .gateway_source_evidence(
                        injection.conversion_index,
                        injection.branch_index,
                        delta,
                    )?;
                if frame.generator.is_some()
                    || &injection.selector != selector
                    || &evidence.selector != selector
                    || injection.child_id != evidence.child_id
                    || frame.row.frame_id != evidence.child_id
                    || frame.branch_id.as_ref() != Some(&evidence.branch_id)
                    || frame.lineage.as_ref() != Some(&evidence.lineage)
                    || frame.row.source != self.prepared.ethernet.devices[evidence.source].id
                    || config.source_device != evidence.source
                    || frame.row.source_vlan_id != Some(evidence.vid)
                    || frame.row.priority != evidence.priority
                    || frame.row.generated_ps != evidence.injected_ps
                    || frame.row.ready_ps != Some(evidence.injected_ps)
                    || serde_json::to_value(&frame.row.wire).unwrap()
                        != serde_json::to_value(&evidence.wire).unwrap()
                {
                    return Err(error(
                        "Gateway frame source/child/bytes/VID/priority/lineage mismatch",
                    ));
                }
            }
        }
        Ok(())
    }
    pub(super) fn replication_offer(
        &self,
        f: usize,
        _source: usize,
        class: l2::Classification,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<bool> {
        let Some(p) = self
            .prepared
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
        else {
            return Ok(false);
        };
        let frame = self.frame(d, f).clone();
        let Some(stream) = self.replication_source_stream(&frame) else {
            return Ok(false);
        };
        self.validate_replication_frame_source(&frame, stream, d.bridge.as_ref())?;
        let config = &p.streams[stream];
        if matches!(
            config.placement,
            crate::types::network::ReplicationPlacement::BridgePair(_)
        ) {
            self.distributed_source_offer(f, stream, class, d, c)?;
            return Ok(true);
        }
        let state = self.replication_mut(d)?;
        let seq = state.streams[stream].allocate()?;
        if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
            state.sequence_allocations.push(SequenceAllocation {
                stream,
                frame: f,
                time_ps: c.now(),
                sequence: seq as u16,
            });
        }
        let origin = identifier(&["ethernet.dynamic.frame", &frame.row.frame_id]);
        let branches = config
            .paths
            .iter()
            .map(|path| identifier(&[&origin, &config.stream_id, &path.id]))
            .collect::<Vec<_>>();
        state.origins.insert(origin.clone(),json!({"origin_id":origin,"source_frame_id":frame.row.frame_id,"stream_id":config.stream_id,"sequence":seq.to_string(),"created_ps":c.now().to_string(),"branch_ids":branches,"target_count":"0","completed_count":"0","target":null,"first_sof_ps":null,"completed_ps":null,"delivered_branch_id":null}));
        if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
            state.origins.get_mut(&origin).unwrap()["delivery_indications"] = json!([]);
        }
        state.publish_origin(&origin);
        for (index, path) in config.paths.iter().enumerate() {
            let id = branches[index].clone();
            state.branches.insert(id.clone(),Branch {stream,path:index,origin:origin.clone(),sequence:seq,data:json!({"branch_id":id,"origin_id":origin,"stream_id":config.stream_id,"sequence":seq.to_string(),"path_id":path.id,"path_index":index.to_string(),"path_ports":path.ports,"hop_index":"0","current_transfer_id":null,"created_ps":c.now().to_string(),"first_sof_ps":null,"completed_ps":null,"status":"created","reason":null,"recovery_id":null})});
            state.publish_branch(&id);
        }
        for (index, (path, id)) in config.paths.iter().zip(branches).enumerate() {
            if index == 1 {
                crate::allocation::reservation_checkpoint("network_fanout_second_branch")?;
            }
            self.offer(
                CopyOrigin {
                    circuit: Some(Circuit {
                        branch: id,
                        hop: 0,
                        distributed: None,
                    }),
                    frame: f,
                    parent: None,
                    visit: 0,
                },
                &frame.row.wire,
                class,
                &[path.directions[0]],
                d,
                c,
            )?;
        }
        Ok(true)
    }
    pub(super) fn replication_offered(
        &self,
        t: usize,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        let transfer = self.transfer(d, t).clone();
        let Some(cursor) = transfer.circuit else {
            return Ok(());
        };
        if let Some(distributed) = &cursor.distributed {
            self.distributed_status(&distributed.flow, &distributed.leg, t, d, now)?;
            return Ok(());
        }
        let state = self.replication_mut(d)?;
        let branch = state
            .branches
            .get_mut(&cursor.branch)
            .ok_or_else(|| error("unknown circuit branch"))?;
        branch.data["hop_index"] = json!(cursor.hop.to_string());
        if transfer.row.status == "dropped" {
            branch.data["status"] = json!("dropped");
            branch.data["reason"] = json!(transfer.row.drop_reason);
            branch.data["completed_ps"] = json!(now.to_string());
            branch.data["current_transfer_id"] = Value::Null;
        } else {
            branch.data["status"] = json!("queued");
            branch.data["current_transfer_id"] = json!(transfer.row.transfer_id);
        }
        state.publish_branch(&cursor.branch);
        Ok(())
    }
    pub(super) fn replication_drop(
        &self,
        t: usize,
        d: &mut NetworkDelta,
        now: u64,
        fallback: &str,
    ) -> Result<()> {
        let transfer = self.transfer(d, t).clone();
        let Some(cursor) = transfer.circuit else {
            return Ok(());
        };
        if let Some(distributed) = &cursor.distributed {
            self.distributed_status(&distributed.flow, &distributed.leg, t, d, now)?;
            return Ok(());
        }
        let reason = d
            .receptions
            .iter()
            .rev()
            .find(|(_, r)| r.transfer == t)
            .and_then(|(_, r)| r.row.reason.clone())
            .or(transfer.row.drop_reason)
            .unwrap_or_else(|| fallback.into());
        let state = self.replication_mut(d)?;
        let branch = state
            .branches
            .get_mut(&cursor.branch)
            .ok_or_else(|| error("unknown circuit branch"))?;
        if branch.data["completed_ps"].is_null() {
            branch.data["status"] = json!("dropped");
            branch.data["reason"] = json!(reason);
            branch.data["completed_ps"] = json!(now.to_string());
            branch.data["current_transfer_id"] = Value::Null;
            state.publish_branch(&cursor.branch);
        }
        Ok(())
    }
    pub(super) fn replication_sof(&self, t: usize, d: &mut NetworkDelta, now: u64) -> Result<()> {
        let Some(cursor) = self.transfer(d, t).circuit.clone() else {
            return Ok(());
        };
        if let Some(distributed) = &cursor.distributed {
            self.distributed_status(&distributed.flow, &distributed.leg, t, d, now)?;
            return Ok(());
        }
        let state = self.replication_mut(d)?;
        let branch = state
            .branches
            .get_mut(&cursor.branch)
            .ok_or_else(|| error("unknown circuit branch"))?;
        branch.data["status"] = json!("in_flight");
        if branch.data["first_sof_ps"].is_null() {
            branch.data["first_sof_ps"] = json!(now.to_string());
        }
        let stream = branch.stream;
        let origin = branch.origin.clone();
        state.publish_branch(&cursor.branch);
        let row = state.origins.get_mut(&origin).unwrap();
        if row["first_sof_ps"].is_null() {
            row["first_sof_ps"] = json!(now.to_string());
            row["target_count"] = json!("1");
            row["target"] = json!(
                self.prepared
                    .extensions
                    .as_ref()
                    .unwrap()
                    .replication
                    .as_ref()
                    .unwrap()
                    .streams[stream]
                    .eliminate_at
            );
            state.publish_origin(&origin);
        }
        Ok(())
    }
    pub(super) fn replication_complete(
        &self,
        reception: &mut Reception,
        transfer: &Transfer,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let cursor = transfer.circuit.as_ref().unwrap();
        if cursor.distributed.is_some() {
            return self.distributed_complete(reception, transfer, d, c);
        }
        let branch = self.replication_mut(d)?.branches[&cursor.branch].clone();
        if !branch.data["completed_ps"].is_null() {
            return Ok(());
        }
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[branch.stream];
        let path = &config.paths[branch.path];
        let intended = path
            .endpoints
            .get(cursor.hop)
            .map(|(_, destination)| *destination)
            .ok_or_else(|| error("endpoint replication physical cursor mismatch"))?;
        let link = self.effective_link(transfer.direction, reception.segment_receiver)?;
        if link.destination != intended {
            reception.row.status = "filtered".into();
            reception.row.reason = Some("replication_unintended_receiver".into());
            return Ok(());
        }
        let wire_sequence = if config.wire_format == ReplicationWireFormat::Rtag8021cb2017 {
            match super::frer_wire::receive(&transfer.row.wire) {
                Ok(sequence) => u64::from(sequence),
                Err(reason) => {
                    reception.row.status = "dropped".into();
                    reception.row.reason = Some(reason.into());
                    self.replication_drop(reception.transfer, d, c.now(), reason)?;
                    return Ok(());
                }
            }
        } else {
            branch.sequence
        };
        if cursor.hop + 1 < path.directions.len() {
            let state = self.replication_mut(d)?;
            state.branches.get_mut(&cursor.branch).unwrap().data["status"] = json!("forwarding");
            state.publish_branch(&cursor.branch);
            let base = if config.wire_format == ReplicationWireFormat::Rtag8021cb2017 {
                frer_wire::strip(&transfer.row.wire).map_err(error)?.0
            } else {
                transfer.row.wire.clone()
            };
            let mut class = l2::Classification {
                vid: reception.row.vlan_id.unwrap(),
                priority: reception.row.priority,
                dei: base.tag.map_or(0, |t| t.dei),
            };
            let next = path.directions[cursor.hop + 1];
            let ingress = self
                .prepared
                .ethernet
                .port_policies
                .iter()
                .find(|policy| policy.ingress == link.to_port)
                .map(|policy| policy.port.as_str());
            let base = if let Some(port) = ingress.filter(|port| {
                self.prepared
                    .provider_l2
                    .as_ref()
                    .is_some_and(|topology| topology.ports.contains_key(*port))
            }) {
                let Some((wire, selected)) =
                    self.provider_explicit_egress(port, &base, next, d, &transfer.row.frame_id, c)?
                else {
                    reception.row.status = "filtered".into();
                    reception.row.reason = Some("provider_no_egress".into());
                    self.replication_drop(reception.transfer, d, c.now(), "provider_no_egress")?;
                    return Ok(());
                };
                class = selected;
                wire
            } else {
                base
            };
            reception.row.egress_transfer_ids = self.offer(
                CopyOrigin {
                    circuit: Some(Circuit {
                        branch: cursor.branch.clone(),
                        hop: cursor.hop + 1,
                        distributed: None,
                    }),
                    frame: transfer.frame,
                    parent: Some(reception.transfer),
                    visit: reception.visit,
                },
                &base,
                class,
                &[next],
                d,
                c,
            )?;
            reception.row.status = "forwarded".into();
            return Ok(());
        }
        let (decision, before, previous, after, high) =
            self.replication_recover(branch.stream, wire_sequence, d, c.now())?;
        let accepted = decision.starts_with("accept_");
        if accepted {
            crate::allocation::reservation_checkpoint("network_recovery_deliver")?;
        }
        let recovery = identifier(&[&cursor.branch, "recover"]);
        if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
            let ri = (0..self.receptions.len()
                + d.receptions
                    .iter()
                    .filter(|(i, _)| *i >= self.receptions.len())
                    .count())
                .find(|&i| self.reception(d, i).row.reception_id == reception.row.reception_id)
                .ok_or_else(|| error("endpoint recovery reception owner"))?;
            let state = self.replication_mut(d)?;
            let order = state.endpoint_recoveries.len() as u64
                + state
                    .distributed
                    .values()
                    .map(|f| f.recoveries.len() as u64)
                    .sum::<u64>();
            state.endpoint_recoveries.push(EndpointRecovery {
                stream: branch.stream,
                recovery: DistributedRecovery {
                    order,
                    reception: ri,
                    time_ps: c.now(),
                    decision: decision.into(),
                    continuation: accepted,
                },
            });
        }
        let state = self.replication_mut(d)?;
        state.records.push(("dir.redundancy.recovery",recovery.clone(),json!({"recovery_id":recovery,"input_branch":cursor.branch,"origin_id":branch.origin,"stream_id":config.stream_id,"sequence":wire_sequence.to_string(),"eliminate_at":config.eliminate_at,"decision":decision,"previous_watermark":opt(previous),"new_watermark":opt(high),"history_capacity":config.history_capacity.to_string(),"bitmap_before_hex":before,"bitmap_after_hex":after,"delivered":accepted})));
        if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
            state.records.last_mut().unwrap().2["reception_id"] = json!(reception.row.reception_id);
        }
        let row = &mut state.branches.get_mut(&cursor.branch).unwrap().data;
        row["status"] = json!(if accepted { "delivered" } else { decision });
        row["completed_ps"] = json!(c.now().to_string());
        row["current_transfer_id"] = Value::Null;
        row["recovery_id"] = json!(recovery);
        state.publish_branch(&cursor.branch);
        if accepted {
            let row = state.origins.get_mut(&branch.origin).unwrap();
            let count = row["completed_count"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .map_err(|_| error("delivery count"))?;
            if count != 0
                && config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
            {
                return Err(error("duplicate terminal delivery invariant"));
            }
            row["completed_count"] = json!(add(count, 1)?.to_string());
            if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
                row["delivery_indications"].as_array_mut().unwrap().push(json!({"recovery_id":recovery,"reception_id":reception.row.reception_id,"branch_id":cursor.branch,"delivered_ps":c.now().to_string()}));
            }
            if count == 0
                || config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
            {
                row["completed_ps"] = json!(c.now().to_string());
                row["delivered_branch_id"] = json!(cursor.branch);
            }
            state.publish_origin(&branch.origin);
            let frame = self.frame(d, transfer.frame);
            d.points.push(Point {
                event_seq: None,
                effect_seq: None,
                time_ps: c.now(),
                target: config.eliminate_at.clone(),
                metric: "ethernet.delivery_ps".into(),
                value: c
                    .now()
                    .checked_sub(frame.row.generated_ps)
                    .ok_or_else(|| error("negative delivery"))?,
                request_id: Some(frame.row.frame_id.clone()),
                receiver: Some(config.eliminate_at.clone()),
                reason: None,
            });
        }
        // Physical L2 reception succeeds for every recovery input. Application
        // delivery is represented exclusively by the origin/recovery records.
        if accepted {
            self.qcc_delivered(transfer, d, c)?;
        }
        reception.row.status = "received".into();
        reception.row.reason = None;
        Ok(())
    }
    pub(super) fn emit_extensions(&self, d: &mut NetworkDelta, c: &mut Context<'_>) -> Result<()> {
        if let Some(state) = d.replication.as_mut() {
            for (schema, id, data) in std::mem::take(&mut state.records) {
                Self::record(c, d, schema, id, data)?;
            }
        }
        if let Some(state) = d.replication.as_mut() {
            state.prune_finished();
        }
        self.emit_wire(d, c)?;
        self.emit_preemption(d, c)
    }
    pub(super) fn check_extension_limits(&self, d: &NetworkDelta) -> Result<()> {
        if let (Some(wire), Some(prepared)) = (self.wire_host(d), self.prepared.dynamic.as_ref()) {
            let inherited = self
                .dynamic
                .as_ref()
                .map_or(0, |state| state.semantic_pending_after(d.dynamic.as_ref()));
            if wire
                .protocol
                .pending_timers()
                .checked_add(inherited)
                .is_none_or(|n| n > prepared.limits.pending_timers)
            {
                return Err(error("shared wire pending_timers capacity exceeded"));
            }
        }
        let Some(e) = &self.prepared.extensions else {
            return Ok(());
        };
        let branches = d
            .replication
            .as_ref()
            .or(self.replication.as_ref())
            .map_or(0, |s| {
                s.branches
                    .values()
                    .filter(|b| b.data["completed_ps"].is_null())
                    .count()
                    + s.distributed
                        .values()
                        .map(|f| {
                            usize::from(f.prefix.terminal_ps.is_none())
                                + f.branches
                                    .iter()
                                    .filter(|b| b.terminal_ps.is_none())
                                    .count()
                                + usize::from(
                                    f.continued
                                        && f.rounds.is_empty()
                                        && f.suffix.terminal_ps.is_none(),
                                )
                                + f.rounds
                                    .iter()
                                    .filter(|r| r.suffix.terminal_ps.is_none())
                                    .count()
                        })
                        .sum::<usize>()
            });
        let transfers = if self.preemption.is_some() {
            (0..self.transfers.len()
                + d.transfers
                    .iter()
                    .filter(|(i, _)| *i >= self.transfers.len())
                    .count())
                .filter(|&i| {
                    let t = self.transfer(d, i);
                    t.row.status != "dropped"
                        && ((t.row.arrival_ps.is_none())
                            || !(0..self.receptions.len()
                                + d.receptions
                                    .iter()
                                    .filter(|(i, _)| *i >= self.receptions.len())
                                    .count())
                                .any(|r| {
                                    self.reception(d, r).transfer == i
                                        && self.reception(d, r).row.ready_ps.is_some()
                                }))
                })
                .count()
        } else {
            0
        };
        let composite = if let (Some(bridge), Some(delta)) = (&self.bridge, &d.bridge) {
            let can = bridge.can_snapshot_after(delta);
            let prepared = self.prepared.bridge.as_ref().unwrap();
            bridge
                .branch_rows_after(delta)
                .into_iter()
                .filter(|row| {
                    if row.status == "dropped" {
                        return false;
                    }
                    let Some(child) = row.child_id.as_ref() else {
                        return true;
                    };
                    if row.pcp.is_some() {
                        (0..self.transfers.len()
                            + d.transfers
                                .iter()
                                .filter(|(i, _)| *i >= self.transfers.len())
                                .count())
                            .any(|i| {
                                let t = self.transfer(d, i);
                                &t.row.frame_id == child
                                    && t.row.status != "dropped"
                                    && (t.row.arrival_ps.is_none()
                                        || !(0..self.receptions.len()
                                            + d.receptions
                                                .iter()
                                                .filter(|(i, _)| *i >= self.receptions.len())
                                                .count())
                                            .any(|r| {
                                                self.reception(d, r).transfer == i
                                                    && self.reception(d, r).row.ready_ps.is_some()
                                            }))
                            })
                    } else {
                        let Some(request) = can.requests.iter().find(|r| &r.request_id == child)
                        else {
                            return true;
                        };
                        if request.status == "dropped" {
                            return false;
                        }
                        if request.eof_ps.is_none() {
                            return true;
                        }
                        let Some(source) = prepared
                            .can
                            .controllers
                            .iter()
                            .position(|c| c.id == request.source)
                        else {
                            return true;
                        };
                        let bus = prepared.can.controller_buses[source];
                        let frame = crate::types::Frame {
                            format: row.output_format.name().into(),
                            id: row.output_can_id,
                            data: String::new(),
                        };
                        let expected = prepared
                            .can
                            .controllers
                            .iter()
                            .enumerate()
                            .filter(|(i, c)| {
                                *i != source
                                    && prepared.can.controller_buses[*i] == bus
                                    && crate::runtime::can::protocol::accepts(&c.rx_filter, &frame)
                            })
                            .count();
                        can.receivers
                            .iter()
                            .filter(|r| &r.request_id == child && r.received_ps.is_some())
                            .count()
                            < expected
                    }
                })
                .count()
        } else {
            0
        };
        if branches + transfers + composite > e.limits.max_pending_branches as usize {
            return Err(
                error("network pending branch limit exceeded").with_reason("max_pending_branches")
            );
        }
        let due = d.due.as_ref().unwrap_or(&self.due).len();
        let wakes = (0..self.tsn_wakes.len())
            .filter(|&i| d.tsn_wakes.get(&i).unwrap_or(&self.tsn_wakes[i]).is_some())
            .count();
        let updates = self
            .tsn
            .as_ref()
            .map_or(0, |s| s.pending_controls_after(&d.tsn));
        let dynamic = self
            .dynamic
            .as_ref()
            .map_or(0, |state| state.semantic_pending_after(d.dynamic.as_ref()));
        if due + wakes + updates + dynamic + d.pending_direct as usize
            > e.limits.max_control_events as usize
        {
            return Err(error("network control reservation limit exceeded")
                .with_reason("max_control_events"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_window_independent_vectors() {
        let mut w = SequenceWindow::new(4);
        let decisions = [5, 5, 3, 1, 6, 3].map(|n| w.recover(n));
        assert_eq!(
            decisions,
            [
                "accept_first",
                "duplicate",
                "accept_late",
                "stale",
                "accept_advance",
                "duplicate"
            ]
        );
        assert_eq!(w.high, Some(6));
        assert_eq!(w.hex(), "b");
        let mut w = SequenceWindow::new(1);
        assert_eq!(w.recover(u64::MAX), "accept_first");
        assert_eq!(w.recover(u64::MAX - 1), "stale");
        assert_eq!(w.recover(u64::MAX), "duplicate");
        let mut w = SequenceWindow::new(65536);
        assert_eq!(w.words.len(), 1024);
        w.recover(1);
        w.recover(65536);
        assert_eq!(w.recover(1), "duplicate");
        w.recover(65537);
        assert_eq!(w.recover(1), "stale");
        assert_eq!(w.hex().len(), 16384);
        let mut w = SequenceWindow::new(65);
        w.recover(0);
        w.recover(64);
        assert_eq!(w.words, [1, 1]);
        w.recover(129);
        assert_eq!(w.words, [1, 0]);
    }
}
#[cfg(test)]
mod design_tests {
    use super::*;
    #[test]
    fn every_designed_recovery_sequence_and_exhaustion() {
        let fixture: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../docs/proposals/fixtures/network-extension-v1.json"),
            )
            .unwrap(),
        )
        .unwrap();
        for v in fixture["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["kind"] == "recovery_window")
        {
            let input = &v["input"];
            let expected = &v["expected"];
            let mut window =
                SequenceWindow::new(input["history_capacity"].as_u64().unwrap() as usize);
            window.high = input["initial_high"].as_str().map(|s| s.parse().unwrap());
            window.words[0] =
                u64::from_str_radix(input["initial_bitmap_hex"].as_str().unwrap(), 16).unwrap();
            for (i, seq) in input["sequences"].as_array().unwrap().iter().enumerate() {
                let decision = window.recover(seq.as_str().unwrap().parse().unwrap());
                assert_eq!(
                    decision,
                    expected["decisions"][i].as_str().unwrap(),
                    "{} step {i}",
                    v["id"]
                );
                assert_eq!(opt(window.high), expected["watermarks"][i]);
                assert_eq!(window.hex(), expected["bitmaps_hex"][i].as_str().unwrap());
            }
        }
        let mut stream = StreamState {
            next: u64::MAX,
            exhausted: false,
            history: SequenceWindow::new(4),
            last_recovery_ps: None,
            timer_generation: 0,
            recovery_deadline_ps: None,
            reset_count: 0,
            reset_events: Vec::new(),
        };
        assert_eq!(stream.allocate().unwrap(), u64::MAX);
        assert!(stream.exhausted);
        assert_eq!(stream.allocate().unwrap_err().code, "E-0004");
        assert_eq!(stream.next, u64::MAX);
        let mut window = SequenceWindow::new(5);
        window.recover(0);
        assert_eq!(window.hex(), "01");
    }
}

#[cfg(test)]
mod atomic_tests {
    fn prepared(name: &str) -> crate::PreparedSimulation {
        crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../examples/network-extensions/{name}.ini")),
        )
        .unwrap()
    }
    #[test]
    fn second_branch_failure_keeps_sequence_history_and_queue_prefix() {
        let mut p = prepared("redundancy");
        p.common.time_limit_ps = 0;
        let baseline = crate::runtime::simulate(&p).unwrap();
        p.common.time_limit_ps = 10_000_000;
        crate::allocation::fail_next_reservation("network_fanout_second_branch");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        assert_eq!(
            failed.network.as_ref().unwrap()["redundancy"],
            baseline.network.as_ref().unwrap()["redundancy"]
        );
        assert!(
            failed
                .registered
                .as_ref()
                .unwrap()
                .model_records
                .values()
                .all(|r| !r.schema.name.starts_with("dir.redundancy."))
        );
        assert!(failed.ethernet.as_ref().unwrap().transfers.is_empty());
    }
    #[test]
    fn history_and_delivery_rollback_together() {
        let mut p = prepared("redundancy");
        p.common.time_limit_ps = 10_000_000;
        crate::allocation::fail_next_reservation("network_recovery_deliver");
        let failed = crate::runtime::simulate(&p).unwrap();
        assert_eq!(failed.common.termination, "execution_failed");
        p.common.time_limit_ps = failed.common.end_ps;
        let baseline = crate::runtime::simulate(&p).unwrap();
        assert_eq!(
            failed.network.as_ref().unwrap()["redundancy"],
            baseline.network.as_ref().unwrap()["redundancy"]
        );
        let records = &failed.registered.as_ref().unwrap().model_records;
        assert!(
            !records
                .values()
                .any(|r| r.schema.name == "dir.redundancy.recovery")
        );
        assert!(
            records
                .values()
                .filter(|r| r.schema.name == "dir.redundancy.origin")
                .all(|r| r.data["completed_count"] == "0")
        );
    }
}
impl SequenceWindow {
    pub(super) fn checkpoint_capacity(
        &self,
        capacity: usize,
        format: ReplicationWireFormat,
    ) -> bool {
        self.capacity == capacity
            && self.modulus == (format == ReplicationWireFormat::Rtag8021cb2017).then_some(65536)
            && self
                .high
                .is_none_or(|high| self.modulus.is_none_or(|modulus| high < modulus))
            && self.words.len() == capacity.div_ceil(64)
            && capacity > 0
            && (capacity % 64 == 0
                || self
                    .words
                    .last()
                    .is_some_and(|w| *w >> (capacity % 64) == 0))
    }
}

#[cfg(test)]
mod normative_vector_tests {
    use super::*;
    fn stream() -> StreamState {
        StreamState {
            next: 0,
            exhausted: false,
            history: SequenceWindow {
                high: None,
                words: vec![0],
                capacity: 4,
                modulus: Some(65536),
            },
            last_recovery_ps: None,
            timer_generation: 0,
            recovery_deadline_ps: None,
            reset_count: 1,
            reset_events: Vec::new(),
        }
    }
    #[test]
    fn literal_signed_vectors_and_independent_instances() {
        let mut s = stream();
        let other = stream();
        for (sequence, decision, high, bitmap) in [
            (65534, "accept_first", 65534, 1),
            (0, "accept_advance", 0, 5),
            (65535, "accept_late", 0, 7),
            (65535, "duplicate", 0, 7),
            (4, "rogue", 0, 7),
            (65532, "rogue", 0, 7),
            (32768, "rogue", 0, 7),
            (3, "accept_advance", 3, 9),
            (0, "duplicate", 3, 9),
            (1, "accept_late", 3, 13),
            (2, "accept_late", 3, 15),
            (65535, "rogue", 3, 15),
        ] {
            assert_eq!(
                s.vector_recover(sequence, 250_000_000, 3_000_000_000)
                    .unwrap(),
                decision
            );
            assert_eq!(s.history.high, Some(high));
            assert_eq!(s.history.words[0], bitmap);
            assert_eq!(other.history.high, None);
            assert_eq!(other.history.words[0], 0);
        }
    }
    #[test]
    fn regular_tick_timeout_and_accepted_only_refresh() {
        let mut s = stream();
        assert_eq!(
            s.vector_recover(7, 250_000_000, 3_000_000_000).unwrap(),
            "accept_first"
        );
        assert_eq!(s.recovery_deadline_ps, Some(3_000_000_000));
        assert_eq!(
            s.vector_recover(7, 1_250_000_000, 3_000_000_000).unwrap(),
            "duplicate"
        );
        assert_eq!(
            s.vector_recover(100, 2_750_000_000, 3_000_000_000).unwrap(),
            "rogue"
        );
        assert_eq!(s.timer_generation, 1);
        assert!(s.timeout(3_000_000_000, 1).unwrap());
        assert_eq!(s.history.high, None);
        assert_eq!(s.history.words[0], 0);
        assert_eq!(
            s.vector_recover(7, 3_000_000_000, 3_000_000_000).unwrap(),
            "accept_reset"
        );
        assert_eq!(s.recovery_deadline_ps, Some(6_000_000_000));
        assert_eq!(
            s.vector_recover(7, 3_000_000_000, 3_000_000_000).unwrap(),
            "duplicate"
        );
        assert!(s.timeout(6_000_000_000, 2).unwrap());
        assert_eq!(
            s.vector_recover(7, 6_000_000_000, 3_000_000_000).unwrap(),
            "accept_reset"
        );
    }
    #[test]
    fn late_refresh_cancels_old_generation_and_generator_wrap_is_independent() {
        let mut s = stream();
        for (seq, at, deadline) in [
            (10, 250_000_000, 3_000_000_000),
            (12, 1_250_000_000, 4_000_000_000),
            (11, 2_250_000_000, 5_000_000_000),
        ] {
            assert!(
                s.vector_recover(seq, at, 3_000_000_000)
                    .unwrap()
                    .starts_with("accept_")
            );
            assert_eq!(s.recovery_deadline_ps, Some(deadline));
        }
        assert!(!s.timeout(3_000_000_000, 1).unwrap());
        assert!(!s.timeout(4_000_000_000, 2).unwrap());
        assert!(s.timeout(5_000_000_000, 3).unwrap());
        s.next = 65534;
        for seq in [65534, 65535, 0, 1] {
            assert_eq!(s.allocate().unwrap(), seq);
        }
        assert_eq!(s.next, 2);
    }
}
