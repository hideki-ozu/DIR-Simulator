//! Finite, open-page command controller with mandatory refresh priority.
use super::{device::Device, types::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Progress {
    Waiting,
    Issued { at_ps: u64 },
    Complete,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Burst {
    id: String,
    offset: u64,
    logical_offset: [Option<usize>; 8],
    progress: Progress,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    request: Request,
    bursts: Vec<Burst>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub device: Device,
    queue: VecDeque<Transfer>,
    next_action: Option<u64>,
    next_sequence: u64,
}
impl Service {
    pub fn new(config: Config) -> Result<Self, Violation> {
        Ok(Self {
            device: Device::new(config)?,
            queue: VecDeque::new(),
            next_action: Some(edge(200_000_000)?),
            next_sequence: 0,
        })
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn requests(&self) -> impl Iterator<Item = &Request> {
        self.queue.iter().map(|t| &t.request)
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub fn now_ps(&self) -> u64 {
        self.device.now_ps
    }
    pub fn next_due_ps(&self) -> Option<u64> {
        let refresh = if self.device.is_ready() {
            self.device
                .refresh_due_ps()
                .and_then(|due| edge(due).ok())
                .map(|due| {
                    if due <= self.now_ps() {
                        self.next_action.unwrap_or(self.now_ps())
                    } else {
                        due
                    }
                })
        } else {
            None
        };
        [self.next_action, self.device.pending_time(), refresh]
            .into_iter()
            .flatten()
            .min()
    }
    /// Inject an explicit maintenance/power/training pin operation at the current
    /// physical time. Data commands belong to the bounded request controller.
    pub fn issue_control(&mut self, command: Command) -> Result<Vec<Effect>, Violation> {
        if matches!(
            command,
            Command::Act { .. } | Command::Read { .. } | Command::Write { .. }
        ) {
            return Err(invalid("controller_owned_command"));
        }
        let mut candidate = self.clone();
        let reset = matches!(command, Command::Reset { asserted: true });
        let mut effects = candidate.device.issue(command)?;
        if reset {
            for transfer in candidate.queue.drain(..) {
                effects.push(Effect::RequestAborted {
                    at_ps: candidate.device.now_ps,
                    request: transfer.request.id,
                    reason: "reset_data_loss".into(),
                });
            }
        }
        candidate.next_action = candidate.device.required_initialization_action()?;
        if candidate.device.is_ready() && !candidate.queue.is_empty() {
            candidate.next_action = Some(candidate.device.next_command_ps()?);
        }
        *self = candidate;
        Ok(effects)
    }
    pub fn enqueue(&mut self, request: Request) -> Result<(), Violation> {
        let length = request.length();
        if self.queue.len() >= self.device.config.queue_capacity {
            return Err(invalid("queue_full"));
        }
        if request.id.is_empty()
            || request.id.len() > 256
            || self.queue.iter().any(|t| t.request.id == request.id)
        {
            return Err(invalid("request_identity"));
        }
        if length == 0
            || length > 65536
            || request.address < self.device.config.base
            || request
                .address
                .checked_add(length as u64)
                .is_none_or(|end| end > self.device.config.base + self.device.config.size)
        {
            return Err(invalid("request_address"));
        }
        if let Operation::Write { data, byte_enables } = &request.operation {
            if data.len() != byte_enables.len() {
                return Err(invalid("byte_enable_length"));
            }
        }
        let start = request.address - self.device.config.base;
        let end = plus(start, length as u64)?;
        let sequence = self.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| invalid("sequence_overflow"))?;
        let mut bursts = Vec::new();
        let mut at = start & !7;
        while at < end {
            let logical_offset = std::array::from_fn(|i| {
                let pos = at + i as u64;
                (pos >= start && pos < end).then(|| (pos - start) as usize)
            });
            bursts.push(Burst {
                id: format!("ddr-{sequence}-{}", bursts.len()),
                offset: at,
                logical_offset,
                progress: Progress::Waiting,
            });
            at = plus(at, 8)?;
        }
        let next_action = if self.device.is_ready() {
            Some(self.device.next_command_ps()?)
        } else {
            self.next_action
        };
        self.queue.push_back(Transfer { request, bursts });
        self.next_sequence = next_sequence;
        self.next_action = next_action;
        Ok(())
    }
    /// Process every device deadline through `at_ps`. Apply returned byte effects
    /// to the shared MemoryService in order; a transport completes only on
    /// RequestComplete. No array access is hidden in this controller.
    pub fn advance(&mut self, at_ps: u64) -> Result<Vec<Effect>, Violation> {
        if at_ps < self.now_ps() {
            return Err(invalid("past_time"));
        }
        let mut candidate = self.clone();
        let effects = candidate.advance_inner(at_ps)?;
        *self = candidate;
        Ok(effects)
    }
    fn advance_inner(&mut self, at_ps: u64) -> Result<Vec<Effect>, Violation> {
        let mut out = Vec::new();
        while let Some(next) = self.next_due_ps().filter(|next| *next <= at_ps) {
            if next < self.now_ps() {
                return Err(invalid("past_deadline"));
            }
            let physical = self.device.advance(next)?;
            for effect in physical {
                if let Effect::BurstComplete { request, burst, .. } = &effect {
                    let transfer = self
                        .queue
                        .iter_mut()
                        .find(|t| t.request.id == *request)
                        .ok_or_else(|| invalid("orphan_burst"))?;
                    let chunk = transfer
                        .bursts
                        .iter_mut()
                        .find(|b| b.id == *burst)
                        .ok_or_else(|| invalid("orphan_burst"))?;
                    if !matches!(chunk.progress, Progress::Issued { .. }) {
                        return Err(invalid("burst_state"));
                    }
                    chunk.progress = Progress::Complete;
                }
                out.push(effect);
            }
            let mut index = 0;
            while index < self.queue.len() {
                if self.queue[index]
                    .bursts
                    .iter()
                    .all(|b| b.progress == Progress::Complete)
                {
                    let transfer = self.queue.remove(index).expect("bounded index");
                    out.push(Effect::RequestComplete {
                        at_ps: next,
                        request: transfer.request.id,
                    });
                } else {
                    index += 1;
                }
            }
            let action_due = self.next_action.is_some_and(|at| at <= next);
            let refresh_due = self.device.refresh_due_ps().is_some_and(|at| at <= next);
            if action_due || refresh_due || self.device.is_ready() && !self.queue.is_empty() {
                self.next_action = None;
                self.schedule(&mut out)?;
            }
        }
        out.extend(self.device.advance(at_ps)?);
        Ok(out)
    }
    fn issue(&mut self, command: Command, out: &mut Vec<Effect>) -> Result<Option<u64>, Violation> {
        match self.device.issue(command) {
            Ok(effects) => {
                out.extend(effects);
                Ok(None)
            }
            Err(e) if e.earliest_ps.is_some() => Ok(Some(edge(
                e.earliest_ps.unwrap().max(
                    self.now_ps()
                        .checked_add(1)
                        .ok_or_else(|| invalid("timestamp_overflow"))?,
                ),
            )?)),
            Err(e) => Err(e),
        }
    }
    fn schedule(&mut self, out: &mut Vec<Effect>) -> Result<(), Violation> {
        let now = self.now_ps();
        if now % TCK != 0 {
            self.next_action = Some(edge(now)?);
            return Ok(());
        }
        use super::device::Phase;
        match self.device.phase.clone() {
            Phase::ResetLow { since, powerup } => {
                if self.device.cke {
                    self.next_action =
                        self.issue(Command::Cke { high: false }, out)?
                            .or(Some(edge(plus(
                                since,
                                if powerup { 200_000_000 } else { 1_000_000 },
                            )?)?));
                    return Ok(());
                }
                let at = edge(plus(since, if powerup { 200_000_000 } else { 1_000_000 })?)?;
                if now < at {
                    self.next_action = Some(at);
                    return Ok(());
                }
                if let Some(at) = self.issue(Command::Reset { asserted: false }, out)? {
                    self.next_action = Some(at);
                } else {
                    self.next_action = Some(edge(plus(now, 500_000_000)?)?);
                }
            }
            Phase::ResetReleased { at } => {
                let due = edge(plus(at, 500_000_000)?)?;
                if now < due {
                    self.next_action = Some(due);
                    return Ok(());
                }
                self.next_action = self
                    .issue(Command::Cke { high: true }, out)?
                    .or(Some(cycle_after(now, 324)?));
            }
            Phase::Initializing => {
                let step = self.device.initialization_step;
                match step {
                    0..=6 => {
                        let index = [3, 6, 5, 4, 2, 1, 0][step];
                        let command = Command::Mrs {
                            index,
                            value: if index == 0 {
                                MR[0] | 0x100
                            } else {
                                MR[index as usize]
                            },
                        };
                        self.next_action = self
                            .issue(command, out)?
                            .or(Some(cycle_after(now, if step == 6 { 24 } else { 8 })?));
                    }
                    7 => {
                        self.next_action = self.issue(Command::Zq { long: true }, out)?;
                    }
                    _ => {}
                }
            }
            Phase::Ready => {
                if self.device.refresh_due_ps().is_some_and(|at| at <= now) {
                    let command = if self
                        .device
                        .banks
                        .iter()
                        .any(|b| b.row.is_some() || b.auto_precharge.is_some())
                    {
                        Command::Pre { bank: None }
                    } else {
                        Command::Refresh
                    };
                    let is_refresh = matches!(command, Command::Refresh);
                    self.next_action = self.issue(command, out)?;
                    if self.next_action.is_none() && !is_refresh {
                        self.next_action = Some(self.device.next_command_ps()?);
                    }
                    if is_refresh && self.next_action.is_none() {
                        if !self.queue.is_empty() {
                            self.next_action = Some(edge(self.device.rank_ready_ps())?);
                        }
                        out.push(Effect::RefreshDeadline { at_ps: now });
                    }
                    return Ok(());
                }
                let mut deferred = None;
                for ti in 0..self.queue.len() {
                    // Older overlapping reads/writes retain memory order. Other
                    // requests may use independent banks while a row is busy.
                    if (0..ti)
                        .any(|older| overlaps(&self.queue[older].request, &self.queue[ti].request))
                    {
                        continue;
                    }
                    for bi in 0..self.queue[ti].bursts.len() {
                        let transfer = &self.queue[ti];
                        let burst = &transfer.bursts[bi];
                        if burst.progress != Progress::Waiting {
                            continue;
                        }
                        let (bank, row, column) = address(burst.offset);
                        let state = &self.device.banks[bank as usize];
                        if state.auto_precharge.is_some() {
                            continue;
                        }
                        let cas = state.row == Some(row);
                        let command = if cas {
                            match &transfer.request.operation {
                                Operation::Read { .. } => Command::Read {
                                    bank,
                                    column,
                                    auto_precharge: false,
                                    request: transfer.request.id.clone(),
                                    burst: burst.id.clone(),
                                    logical_offset: burst.logical_offset,
                                },
                                Operation::Write { data, byte_enables } => Command::Write {
                                    bank,
                                    column,
                                    auto_precharge: false,
                                    request: transfer.request.id.clone(),
                                    burst: burst.id.clone(),
                                    logical_offset: burst.logical_offset,
                                    data: std::array::from_fn(|i| {
                                        burst.logical_offset[i].map_or(0, |i| data[i])
                                    }),
                                    byte_enables: std::array::from_fn(|i| {
                                        burst.logical_offset[i].is_some_and(|i| byte_enables[i])
                                    }),
                                },
                            }
                        } else if state.row.is_some() {
                            Command::Pre { bank: Some(bank) }
                        } else {
                            Command::Act { bank, row }
                        };
                        match self.issue(command, out)? {
                            None => {
                                if cas {
                                    self.queue[ti].bursts[bi].progress =
                                        Progress::Issued { at_ps: now };
                                }
                                self.next_action = Some(self.device.next_command_ps()?);
                                return Ok(());
                            }
                            Some(at) => {
                                deferred = Some(deferred.map_or(at, |prior: u64| prior.min(at)))
                            }
                        }
                    }
                }
                self.next_action = deferred;
            }
            Phase::PowerDown { .. } | Phase::SelfRefresh { .. } => {}
        }
        Ok(())
    }
    /// Strict semantic checks used after decoding owned checkpoint state.
    pub fn validate(&self) -> Result<(), Violation> {
        self.device.validate()?;
        if self.queue.len() > self.device.config.queue_capacity
            || self
                .next_action
                .is_some_and(|at| at < self.now_ps() || at % TCK != 0)
        {
            return Err(invalid("controller_checkpoint"));
        }
        if let Some(required) = self.device.required_initialization_action()? {
            if self.next_action != Some(required) {
                return Err(invalid("initialization_controller_deadline"));
            }
        }
        let mut ids = BTreeSet::new();
        let mut bursts = BTreeSet::new();
        let mut expected = Vec::new();
        let mut expected_dq = Vec::new();
        for transfer in &self.queue {
            if !ids.insert(&transfer.request.id) || transfer.bursts.is_empty() {
                return Err(invalid("request_identity"));
            }
            let mut reconstructed = Self::new(self.device.config.clone())?;
            reconstructed.enqueue(transfer.request.clone())?;
            let shape = reconstructed.queue.front().unwrap();
            if shape.bursts.len() != transfer.bursts.len() {
                return Err(invalid("burst_shape"));
            }
            for (index, burst) in transfer.bursts.iter().enumerate() {
                if !bursts.insert(&burst.id)
                    || burst.offset != shape.bursts[index].offset
                    || burst.logical_offset != shape.bursts[index].logical_offset
                {
                    return Err(invalid("burst_shape"));
                }
                if let Progress::Issued { at_ps } = burst.progress {
                    if at_ps > self.now_ps() || at_ps % TCK != 0 {
                        return Err(invalid("burst_time"));
                    }
                    let first =
                        cycle_after(at_ps, if transfer.request.is_write() { 12 } else { 16 })?;
                    let end =
                        cycle_after(at_ps, if transfer.request.is_write() { 16 } else { 20 })?;
                    expected_dq.push((first, end));
                    if end <= self.now_ps() {
                        return Err(invalid("burst_time"));
                    }
                    for i in 0..8 {
                        let time = plus(first, i as u64 * HALF)?;
                        if time <= self.now_ps() {
                            continue;
                        }
                        let address = plus(self.device.config.base, burst.offset + i as u64)?;
                        let effect = match &transfer.request.operation {
                            Operation::Read { .. } => Effect::ReadByte {
                                at_ps: time,
                                request: transfer.request.id.clone(),
                                burst: burst.id.clone(),
                                address,
                                logical_offset: burst.logical_offset[i],
                            },
                            Operation::Write { data, byte_enables } => Effect::WriteByte {
                                at_ps: time,
                                request: transfer.request.id.clone(),
                                burst: burst.id.clone(),
                                address,
                                logical_offset: burst.logical_offset[i],
                                value: burst.logical_offset[i].map_or(0, |i| data[i]),
                                byte_enable: burst.logical_offset[i]
                                    .is_some_and(|i| byte_enables[i]),
                            },
                        };
                        expected.push(effect);
                    }
                    expected.push(Effect::BurstComplete {
                        at_ps: end,
                        request: transfer.request.id.clone(),
                        burst: burst.id.clone(),
                    });
                }
            }
        }
        let mut actual_dq = self.device.dq_windows();
        expected_dq.sort_unstable();
        actual_dq.sort_unstable();
        if expected_dq != actual_dq {
            return Err(invalid("DQ_bijection"));
        }
        let mut actual = self.device.pending_byte_effects();
        let key = |e: &Effect| {
            (
                e.at_ps(),
                serde_json::to_string(e).expect("serializable effect"),
            )
        };
        expected.sort_by_key(&key);
        actual.sort_by_key(key);
        if expected != actual {
            return Err(invalid("pending_byte_bijection"));
        }
        if !self.queue.is_empty() && self.device.is_ready() && self.next_due_ps().is_none() {
            return Err(invalid("missing_controller_deadline"));
        }
        Ok(())
    }
}
fn overlaps(a: &Request, b: &Request) -> bool {
    a.address < b.address + b.length() as u64 && b.address < a.address + a.length() as u64
}
