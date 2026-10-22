//! Project mailbox ABI v1. Control words share the existing IPC slot lifecycle.
//! Payload storage remains exclusively in MemoryService; this module owns no RAM.
use super::*;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResultCode {
    #[default]
    Okay,
    ForeignSource,
    StaleGeneration,
    InvalidPhase,
    IncompletePayload,
    Outstanding,
    InvalidLength,
    InvalidCommand,
}
impl ResultCode {
    fn word(self) -> u32 {
        match self {
            Self::Okay => 0,
            Self::ForeignSource => 1,
            Self::StaleGeneration => 2,
            Self::InvalidPhase => 3,
            Self::IncompletePayload => 4,
            Self::Outstanding => 5,
            Self::InvalidLength => 6,
            Self::InvalidCommand => 7,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stage {
    pub length: u32,
    pub expected: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Endpoint {
    pub stages: [Stage; 2],
    pub pending: Option<usize>,
    pub result: ResultCode,
    pub notification: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Access {
    pub endpoint: usize,
    pub native: Option<(Transport, String, usize)>,
    pub accepted: u64,
    pub ready: u64,
    pub sequence: u64,
    pub stage: Stage,
    pub applied: bool,
    pub applied_sequence: Option<u64>,
    pub data: Vec<u8>,
    pub result: ResultCode,
    pub generation: u64,
}
impl Soc {
    pub(super) fn mailbox_accesses_completed(&self, s: &State, i: usize) -> bool {
        s.mailbox_accesses.get(&i).is_some_and(|a| a.applied)
    }
    pub(super) fn mailbox_index(&self, address: u32, length: usize) -> Option<usize> {
        crate::input::soc_execution::mailbox(&self.config, address, length)
    }
    pub(super) fn mailbox_access_valid(&self, t: &Transaction) -> bool {
        let r = &t.request;
        let Some(e) = self.mailbox_index(r.address, r.length) else {
            return false;
        };
        let cfg = &self.config.mailboxes[e];
        let Some(offset) = r.address.checked_sub(cfg.base) else {
            return false;
        };
        r.address % 4 == 0
            && r.length == 4
            && r.atomic.is_none()
            && r.lease.is_none()
            && !matches!(t.client, Client::Fetch { .. })
            && (r.source == cfg.producer || r.source == cfg.consumer)
            && match r.operation {
                Operation::Read => matches!(offset, 0 | 4 | 8 | 12 | 16 | 20 | 28 | 32 | 36),
                Operation::Write => {
                    matches!(offset, 12 | 16 | 20 | 24)
                        && r.data.len() == 4
                        && (r.byte_enables.is_empty()
                            || r.byte_enables.len() == 4 && r.byte_enables.iter().all(|b| *b))
                }
                Operation::Atomic => false,
            }
    }
    pub(super) fn mailbox_available(&self, s: &State, e: usize) -> bool {
        s.mailboxes[e].pending.is_none()
    }
    pub(super) fn accept_mailbox(
        &self,
        s: &mut State,
        c: &mut Context<'_>,
        i: usize,
        native: Option<(Transport, String, usize)>,
    ) -> ModelResult {
        if !self.mailbox_access_valid(&s.transactions[i]) {
            return Err("invalid mailbox reached target acceptance".into());
        }
        let t = &s.transactions[i];
        let endpoint = self
            .mailbox_index(t.request.address, t.request.length)
            .ok_or("missing mailbox target")?;
        if s.mailboxes[endpoint]
            .pending
            .is_some_and(|owner| owner != i)
        {
            return Err("mailbox target accepted while response slot occupied".into());
        }
        if s.mailbox_accesses.contains_key(&i) {
            return Err("mailbox command replay".into());
        }
        let role = usize::from(t.request.source != self.config.mailboxes[endpoint].producer);
        let stage = s.mailboxes[endpoint].stages[role].clone();
        let ready = self.later(c.now(), self.config.mailboxes[endpoint].service_cycles)?;
        s.mailbox_accesses.insert(
            i,
            Access {
                endpoint,
                native,
                accepted: c.now(),
                ready,
                sequence: s.sequence,
                stage,
                applied: false,
                applied_sequence: None,
                data: vec![],
                result: ResultCode::Okay,
                generation: 0,
            },
        );
        s.mailboxes[endpoint].pending = Some(i);
        self.emit(
            s,
            c,
            "mailbox_accepted",
            Some(i),
            json!({"endpoint":self.config.mailboxes[endpoint].id,"ready_ps":ready}),
        )
    }
    pub(super) fn advance_mailboxes(&self, s: &mut State, c: &mut Context<'_>) -> ModelResult {
        let mut due: Vec<_> = s
            .mailbox_accesses
            .iter()
            .filter(|(_, a)| !a.applied && a.ready <= c.now())
            .map(|(i, a)| (*i, a.ready, a.sequence))
            .collect();
        due.sort_by_key(|(i, at, seq)| (*at, *seq, *i));
        for (i, _, _) in due {
            let applied_sequence = self.record_ipc(s, c, ipc::Origin::Mapped { transaction: i })?;
            let a = s.mailbox_accesses[&i].clone();
            let cfg = &self.config.mailboxes[a.endpoint];
            let t = s.transactions[i].clone();
            let slot = self
                .config
                .slots
                .iter()
                .position(|slot| slot.id == cfg.slot)
                .ok_or("mailbox slot absent")?;
            let offset = t.request.address - cfg.base;
            let active = |lease: &LeaseRef| {
                s.transactions
                    .iter()
                    .any(|t| t.phase != Phase::Done && t.request.lease.as_ref() == Some(lease))
            };
            let (data, code, published) = apply_access(
                &self.config,
                a.endpoint,
                &mut s.mailboxes[a.endpoint],
                &mut s.slots,
                &t.request,
                &a.stage,
                &active,
            )?;
            if let Some(lease) = published {
                self.emit_publication(s, c, &lease)?;
            }
            if let Some((transport, key, offset)) = &a.native {
                match transport {
                    Transport::Axi => s.axi.complete_target(key, *offset, &data, false)?,
                    Transport::Ahb => s.ahb.complete_target(key, *offset, &data, false)?,
                    _ => return Err("mailbox native transport mismatch".into()),
                }
            } else {
                s.transactions[i].result = if t.request.operation == Operation::Read {
                    data.clone()
                } else {
                    vec![]
                };
                s.transactions[i].wire_data = s.transactions[i].result.clone();
                s.transactions[i].offset = 4;
                s.transactions[i].phase = Phase::Response;
                s.transactions[i].response_ready = Some(c.now());
            }
            let saved = s
                .mailbox_accesses
                .get_mut(&i)
                .expect("retained mailbox access");
            saved.applied = true;
            saved.applied_sequence = Some(applied_sequence);
            saved.data = data;
            saved.result = code;
            saved.generation = s.slots[slot].generation;
            self.emit(s,c,"mailbox_completed",Some(i),json!({"endpoint":cfg.id,"register":offset,"result":code,"generation":s.slots[slot].generation}))?;
        }
        Ok(())
    }
}

impl Soc {
    pub(super) fn validate_mailboxes(&self, s: &State) -> ModelResult {
        if s.mailboxes.len() != self.config.mailboxes.len() {
            return Err("mailbox endpoint count mismatch".into());
        }
        let last = s.last_tick.unwrap_or(0);
        let mut ordered: Vec<_> = s.mailbox_accesses.iter().collect();
        ordered.sort_by_key(|(i, a)| (a.sequence, **i));
        let mut stages = vec![[Stage::default(), Stage::default()]; s.mailboxes.len()];
        let mut results = vec![ResultCode::Okay; s.mailboxes.len()];
        let mut live = BTreeMap::new();
        let mut native = BTreeSet::new();
        let mut previous_sequence = None;
        for (i, a) in &ordered {
            let t = s.transactions.get(**i).ok_or("orphan mailbox access")?;
            if !self.mailbox_access_valid(t)
                || self.mailbox_index(t.request.address, t.request.length) != Some(a.endpoint)
                || a.endpoint >= s.mailboxes.len()
                || a.accepted < t.admitted
                || a.accepted > last
                || a.accepted % self.config.cycle_ps != 0
                || a.ready
                    != self.later(a.accepted, self.config.mailboxes[a.endpoint].service_cycles)?
                || a.applied != (a.ready <= last)
                || a.sequence >= s.sequence
                || previous_sequence.is_some_and(|seq| seq >= a.sequence)
            {
                return Err("mailbox immutable request/acceptance/deadline mismatch".into());
            }
            previous_sequence = Some(a.sequence);
            let cfg = &self.config.mailboxes[a.endpoint];
            let role = usize::from(t.request.source != cfg.producer);
            if a.stage != stages[a.endpoint][role] {
                return Err("mailbox command staging snapshot mismatch".into());
            }
            if t.phase != Phase::Done && live.insert(a.endpoint, **i).is_some() {
                return Err("mailbox bounded response slot overcommitted".into());
            }
            if let Some((transport, key, offset)) = &a.native {
                if *transport != t.request.transport
                    || *offset != 0
                    || t.request.transport == Transport::Noc
                    || !native.insert((*transport as u8, key.clone(), *offset))
                {
                    return Err("mailbox native owner mismatch".into());
                }
            } else if t.request.transport != Transport::Noc {
                return Err("mailbox bypassed native bus".into());
            }
            if !a.applied {
                if !a.data.is_empty()
                    || a.result != ResultCode::Okay
                    || a.generation != 0
                    || t.phase
                        != if t.request.transport == Transport::Noc {
                            Phase::Peripheral
                        } else {
                            Phase::Native
                        }
                {
                    return Err("unapplied mailbox has side effect/completion".into());
                }
                continue;
            }
            if a.data.len() != 4 {
                return Err("mailbox completion width mismatch".into());
            }
            if t.request.operation == Operation::Write {
                if a.data != [0; 4] {
                    return Err("mailbox write target returned nonzero capture".into());
                }
                let value =
                    u32::from_le_bytes(t.request.data[..4].try_into().expect("validated word"));
                match t.request.address - cfg.base {
                    12 => stages[a.endpoint][role].length = value,
                    16 => {
                        stages[a.endpoint][role].expected = (stages[a.endpoint][role].expected
                            & 0xffff_ffff_0000_0000)
                            | value as u64
                    }
                    20 => {
                        stages[a.endpoint][role].expected = (stages[a.endpoint][role].expected
                            & 0xffff_ffff)
                            | ((value as u64) << 32)
                    }
                    24 => {
                        let unauthorized = (matches!(value, 1 | 2) && role != 0)
                            || (matches!(value, 3 | 4) && role != 1);
                        if unauthorized && a.result != ResultCode::ForeignSource
                            || !matches!(value, 1..=4) && a.result != ResultCode::InvalidCommand
                        {
                            return Err("mailbox command result contradicts role/opcode".into());
                        }
                        results[a.endpoint] = a.result;
                    }
                    _ => return Err("invalid mailbox journal register".into()),
                }
            }
            let slot = self
                .config
                .slots
                .iter()
                .position(|slot| slot.id == cfg.slot)
                .ok_or("missing mailbox slot")?;
            if a.generation > s.slots[slot].generation {
                return Err("mailbox generation exceeds slot".into());
            }
            if t.request.operation == Operation::Read
                && t.phase == Phase::Done
                && t.error.is_none()
                && t.result != a.data
            {
                return Err("mailbox read response/capture mismatch".into());
            }
        }
        for (e, endpoint) in s.mailboxes.iter().enumerate() {
            if endpoint.stages != stages[e] || endpoint.result != results[e] {
                return Err("mailbox staged words/result lack applied commands".into());
            }
            if let Some(i) = endpoint.pending {
                let t = s
                    .transactions
                    .get(i)
                    .ok_or("mailbox response slot has orphan transaction")?;
                if t.phase == Phase::Done
                    || !self.mailbox_access_valid(t)
                    || self.mailbox_index(t.request.address, t.request.length) != Some(e)
                    || live.get(&e).is_some_and(|owner| *owner != i)
                    || !s.mailbox_accesses.contains_key(&i) && t.phase != Phase::Native
                {
                    return Err("mailbox pending response owner mismatch".into());
                }
            } else if live.contains_key(&e) {
                return Err("live mailbox response lost reserved slot".into());
            }
        }
        for (transport, targets) in [
            (Transport::Axi, s.axi.target_states()),
            (Transport::Ahb, s.ahb.target_states()),
        ] {
            for (target, completed, data, error) in targets {
                if self
                    .mailbox_index(target.address as u32, target.data.len())
                    .is_none()
                {
                    continue;
                }
                let (i, a) = s
                    .mailbox_accesses
                    .iter()
                    .find(|(_, a)| {
                        a.native.as_ref() == Some(&(transport, target.key.clone(), target.offset))
                    })
                    .ok_or("mailbox native target lost accepted access")?;
                let t = &s.transactions[*i];
                if !native.contains(&(transport as u8, target.key, target.offset))
                    || target.address != t.request.address as u64
                    || target.read != (t.request.operation == Operation::Read)
                    || target.data.len() != 4
                    || !target.read
                        && (target.data != t.request.data || target.enables != vec![true; 4])
                    || completed != vec![a.applied; 4]
                    || error
                    || a.applied && data != a.data
                {
                    return Err("mailbox native completion/side-effect binding mismatch".into());
                }
            }
        }
        Ok(())
    }
}

/// The same transition is used by the live target and semantic checkpoint audit.
pub(super) fn apply_access(
    config: &Config,
    e: usize,
    endpoint: &mut Endpoint,
    slots: &mut [Slot],
    r: &Request,
    stage: &Stage,
    active: &dyn Fn(&LeaseRef) -> bool,
) -> ModelResult<(Vec<u8>, ResultCode, Option<LeaseRef>)> {
    let cfg = &config.mailboxes[e];
    let slot = config
        .slots
        .iter()
        .position(|s| s.id == cfg.slot)
        .ok_or("missing mailbox slot")?;
    let offset = r.address - cfg.base;
    let role = usize::from(r.source != cfg.producer);
    let mut code = ResultCode::Okay;
    let mut published = None;
    let value = if r.operation == Operation::Read {
        match offset {
            0 => {
                (match slots[slot].phase {
                    SlotPhase::Free => 0,
                    SlotPhase::Leased => 1,
                    SlotPhase::Filling => 2,
                    SlotPhase::Ready => 3,
                    SlotPhase::Reading => 4,
                }) | (u32::from(endpoint.result != ResultCode::Okay) << 9)
            }
            4 => slots[slot].generation as u32,
            8 => (slots[slot].generation >> 32) as u32,
            12 => endpoint.stages[role].length,
            16 => endpoint.stages[role].expected as u32,
            20 => (endpoint.stages[role].expected >> 32) as u32,
            28 => endpoint.result.word(),
            32 => endpoint.notification.unwrap_or(0) as u32,
            36 => (endpoint.notification.unwrap_or(0) >> 32) as u32,
            _ => return Err("invalid mailbox read offset".into()),
        }
    } else {
        let value = u32::from_le_bytes(
            r.data[..4]
                .try_into()
                .map_err(|_| ModelError("invalid mailbox write word".into()))?,
        );
        match offset {
            12 => endpoint.stages[role].length = value,
            16 => {
                endpoint.stages[role].expected =
                    (endpoint.stages[role].expected & 0xffff_ffff_0000_0000) | value as u64
            }
            20 => {
                endpoint.stages[role].expected =
                    (endpoint.stages[role].expected & 0xffff_ffff) | ((value as u64) << 32)
            }
            24 => {
                let lease = LeaseRef {
                    slot: cfg.slot.clone(),
                    generation: stage.expected,
                    owner: cfg.producer.clone(),
                };
                code = if (matches!(value, 1 | 2) && role != 0)
                    || (matches!(value, 3 | 4) && role != 1)
                {
                    ResultCode::ForeignSource
                } else {
                    let action = match value {
                        1 => Some(CommandOp::Lease {
                            slot: cfg.slot.clone(),
                            owner: cfg.producer.clone(),
                            length: stage.length as usize,
                        }),
                        2 => Some(CommandOp::Publish {
                            lease: lease.clone(),
                        }),
                        3 => Some(CommandOp::BeginRead {
                            lease: lease.clone(),
                            consumer: cfg.consumer.clone(),
                        }),
                        4 => Some(CommandOp::Release {
                            lease: lease.clone(),
                            consumer: cfg.consumer.clone(),
                        }),
                        _ => None,
                    };
                    match action {
                        Some(action) => ipc::apply_transition(config, slots, active, &action)?,
                        None => ResultCode::InvalidCommand,
                    }
                };
                endpoint.result = code;
                if code == ResultCode::Okay {
                    if value == 2 {
                        endpoint.notification = Some(slots[slot].generation);
                        published = Some(lease);
                    }
                    if value == 1 {
                        endpoint.notification = None;
                    }
                }
            }
            _ => return Err("invalid mailbox write offset".into()),
        }
        0
    };
    Ok((
        if r.operation == Operation::Read {
            value.to_le_bytes().to_vec()
        } else {
            vec![0; 4]
        },
        code,
        published,
    ))
}
pub(super) fn lifecycle_notification(
    config: &Config,
    slots: &[Slot],
    endpoints: &mut [Endpoint],
    action: &CommandOp,
    code: ResultCode,
) {
    if code != ResultCode::Okay {
        return;
    }
    let (slot, publish) = match action {
        CommandOp::Lease { slot, .. } => (slot, false),
        CommandOp::Publish { lease } => (&lease.slot, true),
        _ => return,
    };
    for (e, cfg) in endpoints.iter_mut().zip(&config.mailboxes) {
        if cfg.slot == *slot {
            e.notification = if publish {
                config
                    .slots
                    .iter()
                    .position(|s| s.id == *slot)
                    .map(|i| slots[i].generation)
            } else {
                None
            };
        }
    }
}
