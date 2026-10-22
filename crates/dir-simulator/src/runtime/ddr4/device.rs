//! Digital pins/command state. Bytes remain in the caller's MemoryService.
use super::types::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bank {
    pub row: Option<u16>,
    pub activated: Option<u64>,
    pub precharge_until: u64,
    pub explicit_precharge: Option<u64>,
    pub last_read: Option<u64>,
    pub last_write: Option<u64>,
    pub last_write_auto: bool,
    pub auto_precharge: Option<u64>,
    pub generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    ResetLow { since: u64, powerup: bool },
    ResetReleased { at: u64 },
    Initializing,
    Ready,
    PowerDown { entered: u64 },
    SelfRefresh { entered: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Vref {
    range: bool,
    code: u8,
    training: bool,
    programmed: bool,
    ready: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Level {
    entered: u64,
    first_low: Option<u64>,
    last_edge: Option<(u64, bool)>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum PendingKind {
    Output(Effect),
    AutoPrecharge { bank: u8, generation: u64 },
    InitializationReady,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    at: u64,
    sequence: u64,
    kind: PendingKind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub config: Config,
    pub now_ps: u64,
    pub phase: Phase,
    pub banks: Vec<Bank>,
    pub cke: bool,
    cke_changed: u64,
    last_command: Option<u64>,
    pub mr: [Option<u16>; 7],
    pub initialization_step: usize,
    rank_busy: u64,
    mrs_ready: u64,
    non_mrs_ready: u64,
    dll_ready: u64,
    zq_initialized: bool,
    acts: VecDeque<(u64, u8)>,
    last_cas: Option<(u64, u8, bool)>,
    write_groups: [Option<u64>; 4],
    dq: Vec<(u64, u64)>,
    refresh_origin: Option<u64>,
    refresh_count: u64,
    refresh_history: VecDeque<u64>,
    last_refresh: Option<u64>,
    last_internal_refresh: Option<u64>,
    xs_ready: u64,
    xs_fast_ready: u64,
    sr_reentry_needs_ref: bool,
    sr_exit: Option<u64>,
    vref: Vref,
    level: Option<Level>,
    pending: Vec<Pending>,
    sequence: u64,
}
impl Device {
    pub fn new(config: Config) -> Result<Self, Violation> {
        config
            .validate()
            .map_err(|_| invalid("selected_clock_latency_limits"))?;
        Ok(Self {
            config,
            now_ps: 0,
            phase: Phase::ResetLow {
                since: 0,
                powerup: true,
            },
            banks: vec![Bank::default(); 16],
            cke: false,
            cke_changed: 0,
            last_command: None,
            mr: [None; 7],
            initialization_step: 0,
            rank_busy: 0,
            mrs_ready: 0,
            non_mrs_ready: 0,
            dll_ready: 0,
            zq_initialized: false,
            acts: VecDeque::new(),
            last_cas: None,
            write_groups: [None; 4],
            dq: vec![],
            refresh_origin: None,
            refresh_count: 0,
            refresh_history: VecDeque::new(),
            last_refresh: None,
            last_internal_refresh: None,
            xs_ready: 0,
            xs_fast_ready: 0,
            sr_reentry_needs_ref: false,
            sr_exit: None,
            vref: Vref {
                range: false,
                code: 16,
                training: false,
                programmed: true,
                ready: 0,
            },
            level: None,
            pending: vec![],
            sequence: 0,
        })
    }
    fn pending(&mut self, at: u64, kind: PendingKind) -> Result<(), Violation> {
        let seq = self.sequence;
        self.sequence = seq
            .checked_add(1)
            .ok_or_else(|| invalid("sequence_overflow"))?;
        self.pending.push(Pending {
            at,
            sequence: seq,
            kind,
        });
        Ok(())
    }
    fn output(&mut self, effect: Effect) -> Result<(), Violation> {
        self.pending(effect.at_ps(), PendingKind::Output(effect))
    }
    pub fn pending_time(&self) -> Option<u64> {
        self.pending.iter().map(|p| p.at).min()
    }
    pub fn refresh_due_ps(&self) -> Option<u64> {
        self.refresh_origin.and_then(|o| {
            self.refresh_count
                .checked_add(1)
                .and_then(|n| n.checked_mul(TREFI))
                .and_then(|n| o.checked_add(n))
        })
    }
    pub fn rank_ready_ps(&self) -> u64 {
        self.rank_busy.max(self.xs_ready).max(self.non_mrs_ready)
    }
    pub fn mrs_ready_ps(&self) -> u64 {
        self.rank_busy.max(self.xs_ready).max(self.mrs_ready)
    }
    pub fn next_command_ps(&self) -> Result<u64, Violation> {
        edge(
            self.last_command
                .map_or(self.now_ps, |at| self.now_ps.max(at.saturating_add(TCK))),
        )
    }
    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }
    pub fn refresh_debt(&self, at: u64) -> i128 {
        self.refresh_origin.map_or(0, |origin| {
            ((at.saturating_sub(origin)) / TREFI) as i128 - self.refresh_count as i128
        })
    }
    fn check_maxima(&self, at: u64) -> Result<(), Violation> {
        for bank in &self.banks {
            if bank.row.is_some()
                && bank
                    .activated
                    .is_some_and(|a| at.saturating_sub(a) > 9 * TREFI)
            {
                return Err(invalid("ACT_open_max"));
            }
        }
        if let Phase::PowerDown { entered } = self.phase {
            if at.saturating_sub(entered) > 9 * TREFI {
                return Err(invalid("PD_duration_max"));
            }
        }
        if matches!(self.phase, Phase::Ready | Phase::PowerDown { .. })
            && self.refresh_origin.is_some()
        {
            if self.refresh_debt(at) > 8 || self.refresh_debt(at) < -8 {
                return Err(invalid("refresh_postpone_bound"));
            }
            let last = self
                .last_refresh
                .or(self.last_internal_refresh)
                .or(self.refresh_origin)
                .unwrap();
            if at.saturating_sub(last) > 9 * TREFI {
                return Err(invalid("refresh_postpone_bound"));
            }
        }
        Ok(())
    }
    /// Advance only physical, already-issued operations. No reads/writes occur here.
    pub fn advance(&mut self, at: u64) -> Result<Vec<Effect>, Violation> {
        if at < self.now_ps {
            return Err(invalid("past_time"));
        }
        let mut next = self.clone();
        let mut effects = vec![];
        loop {
            let Some((index, _)) = next
                .pending
                .iter()
                .enumerate()
                .filter(|(_, p)| p.at <= at)
                .min_by_key(|(_, p)| (p.at, p.sequence))
            else {
                break;
            };
            let event = next.pending.remove(index);
            next.check_maxima(event.at)?;
            next.now_ps = event.at;
            match event.kind {
                PendingKind::Output(effect) => effects.push(effect),
                PendingKind::AutoPrecharge { bank, generation } => {
                    let b = &mut next.banks[bank as usize];
                    if b.generation == generation && b.auto_precharge == Some(event.at) {
                        b.row = None;
                        b.auto_precharge = None;
                        b.precharge_until = cycle_after(event.at, 16)?;
                        effects.push(Effect::State {
                            at_ps: event.at,
                            state: format!("bank-{bank}-auto-precharge"),
                        });
                    }
                }
                PendingKind::InitializationReady => {
                    if next.initialization_step != 8 || !next.zq_initialized {
                        return Err(invalid("initialization_sequence"));
                    }
                    next.phase = Phase::Ready;
                    next.refresh_origin = Some(event.at);
                    next.refresh_count = 0;
                    next.last_internal_refresh = Some(event.at);
                    effects.push(Effect::Ready { at_ps: event.at });
                }
            }
        }
        if let Phase::SelfRefresh { entered } = next.phase {
            let last = next.last_internal_refresh.unwrap_or(entered);
            let count = at.saturating_sub(last) / TREFI;
            if count > 0 {
                next.last_internal_refresh = Some(plus(
                    last,
                    count
                        .checked_mul(TREFI)
                        .ok_or_else(|| invalid("timestamp_overflow"))?,
                )?);
            }
        }
        next.check_maxima(at)?;
        next.now_ps = at;
        next.dq.retain(|(_, end)| *end > at);
        *self = next;
        Ok(effects)
    }
    fn idle(&self) -> Result<(), Violation> {
        if self
            .banks
            .iter()
            .any(|b| b.row.is_some() || b.auto_precharge.is_some())
        {
            return Err(invalid("bank_command_state"));
        }
        for b in &self.banks {
            wait(self.now_ps, b.precharge_until, "PRE_to_ACT_REF")?;
        }
        Ok(())
    }
    fn no_data(&self, rule: &str) -> Result<(), Violation> {
        if let Some((_, end)) = self
            .dq
            .iter()
            .filter(|(_, end)| *end > self.now_ps)
            .min_by_key(|(_, end)| *end)
        {
            wait(self.now_ps, *end, rule)?;
        }
        Ok(())
    }
    fn precharge_ready(&self, bank: usize) -> Result<u64, Violation> {
        let b = &self.banks[bank];
        let mut at = 0;
        if let Some(a) = b.activated {
            at = at.max(cycle_after(a, 39)?);
        }
        if let Some(r) = b.last_read {
            at = at.max(cycle_after(r, 9)?);
        }
        if let Some(w) = b.last_write {
            at = at.max(cycle_after(w, 34)?);
        }
        Ok(at)
    }
    fn valid_bank(bank: u8) -> Result<usize, Violation> {
        if bank < 16 {
            Ok(bank as usize)
        } else {
            Err(invalid("bank_address"))
        }
    }
    fn guard_phase(&self, command: &Command) -> Result<(), Violation> {
        if matches!(
            command,
            Command::Reset { .. } | Command::Cke { .. } | Command::Des
        ) {
            return Ok(());
        }
        if !self.cke {
            return Err(invalid("command_disable"));
        }
        if !matches!(self.phase, Phase::Ready | Phase::Initializing) {
            return Err(invalid("initialization_sequence"));
        }
        let fast = matches!(
            command,
            Command::Zq { .. }
                | Command::Mrs {
                    index: 0,
                    value: 0x0834
                }
                | Command::Mrs {
                    index: 2,
                    value: 0x0018
                }
        );
        wait(
            self.now_ps,
            if fast {
                self.xs_fast_ready
            } else {
                self.xs_ready
            },
            if fast {
                "SRX_fast_exceptions"
            } else {
                "SRX_nonDLL"
            },
        )?;
        if self.level.is_some()
            && !matches!(command, Command::Dqs { .. } | Command::Mrs { index: 1, .. })
        {
            return Err(invalid("WLEV_allowed"));
        }
        if self.vref.training
            && !matches!(
                command,
                Command::Act { .. }
                    | Command::Read { .. }
                    | Command::Write { .. }
                    | Command::Pre { .. }
                    | Command::Mrs { index: 6, .. }
            )
        {
            return Err(invalid("VREF_enter"));
        }
        wait(self.now_ps, self.rank_busy, "rank_busy")?;
        if matches!(command, Command::Mrs { .. }) {
            wait(self.now_ps, self.mrs_ready, "MRS_to_MRS")?;
        } else {
            wait(self.now_ps, self.non_mrs_ready, "MRS_to_nonMRS")?;
        }
        if self.vref.training && !matches!(command, Command::Mrs { index: 6, .. }) {
            wait(self.now_ps, self.vref.ready, "VREF_enter")?;
        }
        if !matches!(
            command,
            Command::Mrs { .. } | Command::Zq { .. } | Command::Dqs { .. }
        ) && !matches!(self.phase, Phase::Ready)
        {
            return Err(invalid("initialization_sequence"));
        }
        Ok(())
    }
    /// Refused commands leave device state, reservations and pending beats unchanged.
    pub fn issue(&mut self, command: Command) -> Result<Vec<Effect>, Violation> {
        let mut next = self.clone();
        let effects = next.apply(command)?;
        *self = next;
        Ok(effects)
    }
    fn apply(&mut self, command: Command) -> Result<Vec<Effect>, Violation> {
        let now = self.now_ps;
        self.check_maxima(now)?;
        let bus = !matches!(command, Command::Reset { .. } | Command::Dqs { .. });
        if bus {
            if now % TCK != 0 {
                return Err(invalid("command_bus"));
            }
            if let Some(last) = self.last_command {
                wait(now, cycle_after(last, 1)?, "command_bus")?;
            }
        }
        self.guard_phase(&command)?;
        let mut out = vec![Effect::Command {
            at_ps: now,
            command: command.clone(),
        }];
        match &command {
            Command::Reset { asserted: true } => {
                let powerup = matches!(self.phase, Phase::ResetLow { powerup: true, .. });
                let cke = self.cke;
                let cke_changed = self.cke_changed;
                let config = self.config.clone();
                *self = Self::new(config)?;
                self.now_ps = now;
                self.cke = cke;
                self.cke_changed = cke_changed;
                self.phase = Phase::ResetLow {
                    since: now,
                    powerup,
                };
                out.push(Effect::ResetInvalidation { at_ps: now });
            }
            Command::Reset { asserted: false } => {
                let Phase::ResetLow { since, powerup } = self.phase else {
                    return Err(invalid("RESET_stable_low"));
                };
                wait(
                    now,
                    plus(since, if powerup { 200_000_000 } else { 1_000_000 })?,
                    if powerup {
                        "RESET_powerup_low"
                    } else {
                        "RESET_stable_low"
                    },
                )?;
                if self.cke {
                    return Err(invalid("RESET_CKE_setup"));
                }
                wait(now, plus(self.cke_changed, 10_000)?, "RESET_CKE_setup")?;
                self.phase = Phase::ResetReleased { at: now };
            }
            Command::Cke { high } => self.cke(*high, &mut out)?,
            Command::Des => {}
            Command::Act { bank, row } => {
                let i = Self::valid_bank(*bank)?;
                if *row >= 32768 {
                    return Err(invalid("row_address"));
                }
                let b = &self.banks[i];
                if b.row.is_some() || b.auto_precharge.is_some() {
                    return Err(invalid("bank_command_state"));
                }
                wait(now, b.precharge_until, "PRE_to_ACT_REF")?;
                if let Some(a) = b.activated {
                    wait(now, cycle_after(a, 55)?, "ACT_to_ACT")?;
                }
                for (at, bg) in &self.acts {
                    wait(
                        now,
                        cycle_after(*at, if *bg == *bank / 4 { 6 } else { 4 })?,
                        if *bg == *bank / 4 {
                            "ACT_same_BG"
                        } else {
                            "ACT_diff_BG"
                        },
                    )?;
                }
                if self.acts.len() >= 4 {
                    wait(
                        now,
                        cycle_after(self.acts[self.acts.len() - 4].0, 26)?,
                        "ACT_fifth_window",
                    )?;
                }
                let b = &mut self.banks[i];
                b.row = Some(*row);
                b.activated = Some(now);
                b.last_read = None;
                b.last_write = None;
                self.acts.push_back((now, *bank / 4));
                while self.acts.len() > 4 {
                    self.acts.pop_front();
                }
            }
            Command::Pre { bank } => {
                let indices = if let Some(bank) = bank {
                    vec![Self::valid_bank(*bank)?]
                } else {
                    (0..16).collect()
                };
                for i in &indices {
                    let b = &self.banks[*i];
                    if let Some(a) = b.activated {
                        wait(now, cycle_after(a, 39)?, "ACT_to_PRE")?;
                    }
                    if let Some(r) = b.last_read {
                        wait(now, cycle_after(r, 9)?, "RD_to_PRE")?;
                    }
                    if let Some(w) = b.last_write {
                        wait(now, cycle_after(w, 34)?, "WR_to_PRE")?;
                    }
                }
                for i in indices {
                    let b = &mut self.banks[i];
                    b.row = None;
                    b.auto_precharge = None;
                    b.generation = b
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| invalid("generation_overflow"))?;
                    b.precharge_until = cycle_after(now, 16)?;
                    b.explicit_precharge = Some(now);
                }
            }
            Command::Read {
                bank,
                column,
                auto_precharge,
                burst,
                request,
                logical_offset,
            } => {
                self.cas(*bank, *column, false)?;
                wait(
                    now,
                    self.dll_ready,
                    if self.sr_exit.is_some() {
                        "SRX_DLL"
                    } else {
                        "DLL_lock"
                    },
                )?;
                let row = self.banks[*bank as usize].row.expect("CAS validated row");
                let start = cycle_after(now, 16)?;
                let end = cycle_after(now, 20)?;
                self.dq_window(start, end)?;
                for (i, logical_offset) in logical_offset.iter().enumerate() {
                    self.output(Effect::ReadByte {
                        at_ps: plus(start, i as u64 * HALF)?,
                        request: request.clone(),
                        burst: burst.clone(),
                        address: plus(self.config.base, offset(*bank, row, *column) + i as u64)?,
                        logical_offset: *logical_offset,
                    })?;
                }
                self.output(Effect::BurstComplete {
                    at_ps: end,
                    request: request.clone(),
                    burst: burst.clone(),
                })?;
                self.banks[*bank as usize].last_read = Some(now);
                if *auto_precharge {
                    self.auto(*bank)?;
                }
            }
            Command::Write {
                bank,
                column,
                auto_precharge,
                burst,
                request,
                data,
                byte_enables,
                logical_offset,
            } => {
                self.cas(*bank, *column, true)?;
                let row = self.banks[*bank as usize].row.expect("CAS validated row");
                let start = cycle_after(now, 12)?;
                let end = cycle_after(now, 16)?;
                self.dq_window(start, end)?;
                for i in 0..8 {
                    self.output(Effect::WriteByte {
                        at_ps: plus(start, i as u64 * HALF)?,
                        request: request.clone(),
                        burst: burst.clone(),
                        address: plus(self.config.base, offset(*bank, row, *column) + i as u64)?,
                        value: data[i],
                        byte_enable: byte_enables[i],
                        logical_offset: logical_offset[i],
                    })?;
                }
                self.output(Effect::BurstComplete {
                    at_ps: end,
                    request: request.clone(),
                    burst: burst.clone(),
                })?;
                self.banks[*bank as usize].last_write = Some(now);
                self.banks[*bank as usize].last_write_auto = *auto_precharge;
                if *auto_precharge {
                    self.auto(*bank)?;
                }
            }
            Command::Refresh => {
                self.idle().map_err(|mut e| {
                    if e.earliest_ps.is_none() {
                        e.rule = "REF_idle".into();
                    }
                    e
                })?;
                self.no_data("REF_idle")?;
                if self.refresh_debt(now) <= -8 {
                    return Err(invalid("refresh_postpone_bound"));
                }
                let cutoff = now.saturating_sub(2 * TREFI);
                while self.refresh_history.front().is_some_and(|t| *t <= cutoff) {
                    self.refresh_history.pop_front();
                }
                if self.refresh_history.len() >= 16 {
                    return Err(invalid("refresh_postpone_bound"));
                }
                self.refresh_history.push_back(now);
                self.refresh_count = self
                    .refresh_count
                    .checked_add(1)
                    .ok_or_else(|| invalid("refresh_count_overflow"))?;
                self.last_refresh = Some(now);
                self.rank_busy = cycle_after(now, 312)?;
                self.sr_reentry_needs_ref = false;
            }
            Command::Mrs { index, value } => self.mrs(*index, *value)?,
            Command::Zq { long } => {
                self.idle().map_err(|mut e| {
                    if e.earliest_ps.is_none() {
                        e.rule = "ZQ_idle".into();
                    }
                    e
                })?;
                self.no_data("ZQ_idle")?;
                let cycles = if !self.zq_initialized {
                    if !*long || self.initialization_step != 7 {
                        return Err(invalid("initialization_sequence"));
                    }
                    1024
                } else if *long {
                    512
                } else {
                    128
                };
                self.rank_busy = cycle_after(now, cycles)?;
                if !self.zq_initialized {
                    self.zq_initialized = true;
                    self.initialization_step = 8;
                    self.pending(
                        self.rank_busy.max(self.dll_ready),
                        PendingKind::InitializationReady,
                    )?;
                }
            }
            Command::SelfRefresh => {
                self.idle().map_err(|_| invalid("SRE_idle"))?;
                self.no_data("SRE_idle")?;
                if self.vref.training
                    || self.level.is_some()
                    || self.refresh_debt(now) > 0
                    || self.sr_reentry_needs_ref
                {
                    return Err(invalid("SRE_idle"));
                }
                wait(now, cycle_after(self.cke_changed, 6)?, "CKE_pulse")?;
                self.cke = false;
                self.cke_changed = now;
                self.phase = Phase::SelfRefresh { entered: now };
                self.last_internal_refresh = Some(now);
            }
            Command::Dqs { high } => self.dqs(*high)?,
        }
        if bus {
            self.last_command = Some(now);
        }
        Ok(out)
    }
    fn cas(&mut self, bank: u8, column: u16, write: bool) -> Result<(), Violation> {
        let i = Self::valid_bank(bank)?;
        let b = &self.banks[i];
        if column >= 1024 || column % 8 != 0 {
            return Err(invalid("burst_address"));
        }
        if b.row.is_none() || b.auto_precharge.is_some() {
            return Err(invalid("bank_command_state"));
        }
        if offset(bank, b.row.unwrap(), column) + 8 > self.config.size {
            return Err(invalid("mapped_address"));
        }
        wait(
            self.now_ps,
            cycle_after(
                b.activated.ok_or_else(|| invalid("bank_command_state"))?,
                16,
            )?,
            "ACT_to_RD_WR",
        )?;
        if let Some((at, bg, prior_write)) = self.last_cas {
            let (cycles, rule) = if prior_write != write {
                if write {
                    (10, "RD_to_WR")
                } else if bg == bank / 4 {
                    (25, "WR_to_RD_same_BG")
                } else {
                    (19, "WR_to_RD_diff_BG")
                }
            } else if bg == bank / 4 {
                (6, "CAS_same_BG")
            } else {
                (4, "CAS_diff_BG")
            };
            wait(self.now_ps, cycle_after(at, cycles)?, rule)?;
        }
        if !write {
            for (bg, last) in self.write_groups.iter().enumerate() {
                if let Some(at) = last {
                    wait(
                        self.now_ps,
                        cycle_after(*at, if bg == bank as usize / 4 { 25 } else { 19 })?,
                        if bg == bank as usize / 4 {
                            "WR_to_RD_same_BG"
                        } else {
                            "WR_to_RD_diff_BG"
                        },
                    )?;
                }
            }
        } else {
            self.write_groups[bank as usize / 4] = Some(self.now_ps);
        }
        self.last_cas = Some((self.now_ps, bank / 4, write));
        Ok(())
    }
    fn dq_window(&mut self, start: u64, end: u64) -> Result<(), Violation> {
        if let Some((_, prior_end)) = self
            .dq
            .iter()
            .filter(|(a, b)| start < *b && *a < end)
            .min_by_key(|(_, b)| *b)
        {
            return Err(Violation {
                rule: "burst_no_interruption".into(),
                earliest_ps: Some(plus(self.now_ps, prior_end - start)?),
            });
        }
        self.dq.push((start, end));
        Ok(())
    }
    fn auto(&mut self, bank: u8) -> Result<(), Violation> {
        let at = self.precharge_ready(bank as usize)?;
        let b = &mut self.banks[bank as usize];
        b.auto_precharge = Some(at);
        b.generation = b
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("generation_overflow"))?;
        let generation = b.generation;
        self.pending(at, PendingKind::AutoPrecharge { bank, generation })
    }
    fn cke(&mut self, high: bool, out: &mut Vec<Effect>) -> Result<(), Violation> {
        let now = self.now_ps;
        if high == self.cke {
            return Ok(());
        }
        wait(now, cycle_after(self.cke_changed, 6)?, "CKE_pulse")?;
        match (high, self.phase.clone()) {
            (false, Phase::ResetLow { .. } | Phase::ResetReleased { .. }) => {}
            (true, Phase::ResetReleased { at }) => {
                wait(now, plus(at, 500_000_000)?, "RESET_to_CKE")?;
                wait(now, 10_000, "clock_before_CKE")?;
                self.phase = Phase::Initializing;
                self.rank_busy = cycle_after(now, 324)?;
            }
            (false, Phase::Ready) => {
                if self.vref.training || self.level.is_some() || self.sr_reentry_needs_ref {
                    return Err(invalid("PDE_command"));
                }
                self.no_data("PDE_command")?;
                wait(now, self.non_mrs_ready, "MRS_to_PDE")?;
                if self.sr_exit.is_none() {
                    wait(now, self.dll_ready, "DLL_lock")?;
                }
                if self.sr_exit.is_some() {
                    wait(now, self.rank_busy, "SRX_reentry")?;
                }
                for b in &self.banks {
                    if let Some(a) = b.activated {
                        wait(now, cycle_after(a, 2)?, "ACT_to_PDE")?;
                    }
                    if let Some(pre) = b.explicit_precharge {
                        wait(now, cycle_after(pre, 2)?, "PRE_to_PDE")?;
                    }
                    if let Some(r) = b.last_read {
                        wait(now, cycle_after(r, 21)?, "RD_to_PDE")?;
                    }
                    if let Some(w) = b.last_write {
                        wait(
                            now,
                            cycle_after(w, if b.last_write_auto { 35 } else { 34 })?,
                            if b.last_write_auto {
                                "WRA_to_PDE"
                            } else {
                                "WR_to_PDE"
                            },
                        )?;
                    }
                }
                if let Some(r) = self.last_refresh {
                    wait(now, cycle_after(r, 2)?, "REF_to_PDE")?;
                }
                // rank_busy may be a running refresh; its first two clocks are
                // checked above. Other command blocks cannot be bypassed.
                if self
                    .last_refresh
                    .is_none_or(|r| self.rank_busy != cycle_after(r, 312).unwrap_or(u64::MAX))
                {
                    wait(now, self.rank_busy, "MRS_to_PDE")?;
                }
                self.phase = Phase::PowerDown { entered: now };
            }
            (true, Phase::PowerDown { entered }) => {
                wait(now, cycle_after(entered, 6)?, "PD_duration_min")?;
                self.phase = Phase::Ready;
                self.rank_busy = self.rank_busy.max(cycle_after(now, 8)?);
            }
            (true, Phase::SelfRefresh { entered }) => {
                wait(now, cycle_after(entered, 7)?, "SRE_low")?;
                wait(now, 10_000, "SRX_clock_setup")?;
                self.phase = Phase::Ready;
                self.xs_ready = cycle_after(now, 324)?;
                self.xs_fast_ready = cycle_after(now, 144)?;
                self.dll_ready = cycle_after(now, 768)?;
                self.sr_exit = Some(now);
                self.sr_reentry_needs_ref = true;
                self.refresh_origin = self.last_internal_refresh;
                self.refresh_count = 0;
                self.last_refresh = None;
                self.refresh_history.clear();
            }
            _ => return Err(invalid("CKE_state")),
        }
        self.cke = high;
        self.cke_changed = now;
        out.push(Effect::State {
            at_ps: now,
            state: format!("{:?}", self.phase),
        });
        Ok(())
    }
    fn mrs(&mut self, index: u8, value: u16) -> Result<(), Violation> {
        let now = self.now_ps;
        if index > 7 {
            return Err(invalid("mode_encoding"));
        }
        if index == 7 {
            if !matches!(self.phase, Phase::Ready) {
                return Err(invalid("initialization_sequence"));
            }
            self.mrs_ready = cycle_after(now, 8)?;
            self.non_mrs_ready = cycle_after(now, 24)?;
            return Ok(());
        }
        self.idle().map_err(|mut e| {
            if e.earliest_ps.is_none() {
                e.rule = "MRS_idle".into();
            }
            e
        })?;
        self.no_data("MRS_idle")?;
        let selected = MR[index as usize];
        let valid = match index {
            0 => value == selected || value == selected | 0x100,
            1 => {
                value == selected
                    || value == selected | 0x80
                    || self.level.is_some() && value == selected | 0x1080
            }
            6 => value & !0xff == selected & !0xff && (value & 63) <= 50,
            _ => value == selected,
        };
        if !valid {
            return Err(invalid("mode_encoding"));
        }
        if matches!(self.phase, Phase::Initializing) {
            let order = [3, 6, 5, 4, 2, 1, 0];
            if self.initialization_step >= 7
                || index != order[self.initialization_step]
                || index == 0 && value != 0x0934
                || index != 0 && value != selected
            {
                return Err(invalid("initialization_sequence"));
            }
            self.initialization_step += 1;
        }
        if index == 6 && matches!(self.phase, Phase::Ready) {
            let training = value & 0x80 != 0;
            let range = value & 0x40 != 0;
            let code = (value & 63) as u8;
            if !self.vref.training
                && !training
                && (range != self.vref.range || code != self.vref.code)
            {
                return Err(invalid("mode_encoding"));
            }
            if training {
                if self.vref.training && range != self.vref.range {
                    return Err(invalid("VREF_enter"));
                }
                wait(now, self.vref.ready, "VREF_single_step")?;
                if !self.vref.training {
                    // Entry code bits are don't-care (ISSI p86); retain the
                    // previous programmed value until the following MR6.
                    self.vref.range = range;
                    self.vref.training = true;
                    self.vref.programmed = false;
                    self.vref.ready = cycle_after(now, 180)?;
                } else {
                    let cycles = if self.vref.programmed && code.abs_diff(self.vref.code) <= 1 {
                        72
                    } else {
                        180
                    };
                    self.vref.code = code;
                    self.vref.programmed = true;
                    self.vref.ready = cycle_after(now, cycles)?;
                }
            } else if self.vref.training {
                wait(now, self.vref.ready, "VREF_exit")?;
                if !self.vref.programmed || range != self.vref.range || code != self.vref.code {
                    return Err(invalid("VREF_exit"));
                }
                self.vref.training = false;
                self.vref.ready = cycle_after(now, 180)?;
                self.rank_busy = self.vref.ready;
            }
        }
        if index == 1 {
            if value & 0x1000 != 0 || value & 0x80 == 0 {
                self.pending.retain(|p| {
                    !matches!(p.kind, PendingKind::Output(Effect::LevelFeedback { .. }))
                });
            }
            let training = value & 0x80 != 0;
            if training && self.level.is_none() {
                self.level = Some(Level {
                    entered: now,
                    first_low: None,
                    last_edge: None,
                });
            } else if !training && self.level.is_some() {
                self.level = None;
            }
        }
        if index == 0 && value & 0x100 != 0 {
            if self.mr[1] != Some(MR[1]) {
                return Err(invalid("initialization_sequence"));
            }
            self.dll_ready = cycle_after(now, 768)?;
        }
        self.mr[index as usize] = Some(if index == 0 {
            value & !0x100
        } else if index == 6 {
            (MR[6] & !0xff)
                | (u16::from(self.vref.range) * 0x40)
                | (u16::from(self.vref.training) * 0x80)
                | self.vref.code as u16
        } else {
            value
        });
        // Ordinary MRS permits the next MRS after8clocks, all other commands
        // after24. A separate field below avoids holding MRS for tMOD.
        self.mrs_ready = cycle_after(now, 8)?;
        self.non_mrs_ready = self.non_mrs_ready.max(cycle_after(now, 24)?);
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Violation> {
        self.config
            .validate()
            .map_err(|_| invalid("selected_clock_latency_limits"))?;
        if self.banks.len() != 16
            || self.initialization_step > 8
            || self.cke_changed > self.now_ps
            || self
                .last_command
                .is_some_and(|t| t > self.now_ps || t % TCK != 0)
        {
            return Err(invalid("device_checkpoint"));
        }
        if matches!(
            self.phase,
            Phase::Ready | Phase::PowerDown { .. } | Phase::SelfRefresh { .. }
        ) && (!self.zq_initialized
            || self.initialization_step != 8
            || self.mr.iter().any(Option::is_none))
        {
            return Err(invalid("initialization_sequence"));
        }
        if matches!(self.phase, Phase::Ready | Phase::Initializing) && !self.cke
            || matches!(
                self.phase,
                Phase::PowerDown { .. } | Phase::SelfRefresh { .. }
            ) && self.cke
        {
            return Err(invalid("CKE_state"));
        }
        if self.acts.len() > 4
            || self
                .acts
                .iter()
                .any(|(at, bg)| *at > self.now_ps || at % TCK != 0 || *bg > 3)
            || !self
                .acts
                .iter()
                .zip(self.acts.iter().skip(1))
                .all(|(a, b)| a.0 < b.0)
        {
            return Err(invalid("ACT_history"));
        }
        if self
            .last_cas
            .is_some_and(|(at, bg, _)| at > self.now_ps || at % TCK != 0 || bg > 3)
        {
            return Err(invalid("CAS_history"));
        }
        if self
            .write_groups
            .iter()
            .flatten()
            .any(|at| *at > self.now_ps || at % TCK != 0)
        {
            return Err(invalid("write_group_history"));
        }
        for (index, b) in self.banks.iter().enumerate() {
            if let Some(write) = b.last_write {
                if self.write_groups[index / 4].is_none_or(|at| at < write) {
                    return Err(invalid("write_group_history"));
                }
            }
            for at in [b.last_read, b.last_write].into_iter().flatten() {
                if self.last_cas.is_none_or(|(last, _, _)| last < at) {
                    return Err(invalid("CAS_history"));
                }
            }
        }
        for (index, word) in self.mr.iter().enumerate() {
            if let Some(value) = word {
                let expected = if index == 1 {
                    if self.level.is_some() {
                        MR[1] | 0x80
                    } else {
                        MR[1]
                    }
                } else if index == 6 {
                    (MR[6] & !0xff)
                        | (u16::from(self.vref.range) * 0x40)
                        | (u16::from(self.vref.training) * 0x80)
                        | self.vref.code as u16
                } else {
                    MR[index]
                };
                if *value != expected
                    && !(index == 1 && self.level.is_some() && *value == expected | 0x1000)
                {
                    return Err(invalid("mode_checkpoint"));
                }
            }
        }
        if self.vref.code > 50
            || self.level.as_ref().is_some_and(|l| {
                l.entered > self.now_ps
                    || l.first_low.is_some_and(|t| t > self.now_ps)
                    || l.last_edge.is_some_and(|(t, _)| t > self.now_ps)
            })
        {
            return Err(invalid("training_checkpoint"));
        }
        let mut sequences = std::collections::BTreeSet::new();
        let ready_count = self
            .pending
            .iter()
            .filter(|p| matches!(p.kind, PendingKind::InitializationReady))
            .count();
        if ready_count
            != usize::from(
                matches!(self.phase, Phase::Initializing) && self.initialization_step == 8,
            )
        {
            return Err(invalid("initialization_deadline_bijection"));
        }
        for pending in &self.pending {
            if pending.at <= self.now_ps
                || pending.sequence >= self.sequence
                || !sequences.insert(pending.sequence)
            {
                return Err(invalid("pending_time"));
            }
            match &pending.kind {
                PendingKind::Output(e) => {
                    if pending.at != e.at_ps() {
                        return Err(invalid("pending_time"));
                    }
                    if let Effect::ReadByte { address, .. } | Effect::WriteByte { address, .. } = e
                    {
                        if *address < self.config.base
                            || *address >= self.config.base + self.config.size
                        {
                            return Err(invalid("mapped_address"));
                        }
                    }
                }
                PendingKind::AutoPrecharge { bank, generation } => {
                    let Some(b) = self.banks.get(*bank as usize) else {
                        return Err(invalid("bank_address"));
                    };
                    if b.generation == *generation && b.auto_precharge != Some(pending.at) {
                        return Err(invalid("auto_precharge_checkpoint"));
                    }
                }
                PendingKind::InitializationReady => {
                    if !matches!(self.phase, Phase::Initializing)
                        || self.initialization_step != 8
                        || !self.zq_initialized
                        || pending.at != self.rank_busy.max(self.dll_ready)
                    {
                        return Err(invalid("initialization_sequence"));
                    }
                }
            }
        }
        for (index, b) in self.banks.iter().enumerate() {
            if b.row.is_some_and(|r| r >= 32768)
                || b.activated.is_some_and(|t| t > self.now_ps || t % TCK != 0)
                || b.last_read.is_some_and(|t| t > self.now_ps || t % TCK != 0)
                || b.last_write
                    .is_some_and(|t| t > self.now_ps || t % TCK != 0)
                || b.row.is_some() && b.activated.is_none()
            {
                return Err(invalid("bank_checkpoint"));
            }
            if let Some(at) = b.auto_precharge {
                if b.row.is_none()||at<=self.now_ps||!self.pending.iter().any(|p|p.at==at&&matches!(p.kind,PendingKind::AutoPrecharge{bank,generation}if bank as usize==index&&generation==b.generation)){return Err(invalid("auto_precharge_checkpoint"));}
            }
        }
        if self
            .dq
            .iter()
            .any(|(start, end)| start >= end || *end <= self.now_ps)
            || self
                .dq
                .iter()
                .enumerate()
                .any(|(i, (a, b))| self.dq.iter().skip(i + 1).any(|(c, d)| a < d && c < b))
        {
            return Err(invalid("DQ_checkpoint"));
        }
        self.check_maxima(self.now_ps)
    }
    pub(crate) fn required_initialization_action(&self) -> Result<Option<u64>, Violation> {
        match self.phase {
            Phase::ResetLow { .. } if self.cke => Ok(Some(edge(
                self.now_ps.max(cycle_after(self.cke_changed, 6)?),
            )?)),
            Phase::ResetLow { since, powerup } => Ok(Some(edge(plus(
                since,
                if powerup { 200_000_000 } else { 1_000_000 },
            )?)?)),
            Phase::ResetReleased { at } => Ok(Some(edge(plus(at, 500_000_000)?)?)),
            Phase::Initializing if self.initialization_step == 0 => Ok(Some(self.rank_busy)),
            Phase::Initializing if self.initialization_step < 7 => Ok(Some(self.mrs_ready)),
            Phase::Initializing if self.initialization_step == 7 => Ok(Some(self.non_mrs_ready)),
            _ => Ok(None),
        }
    }
    pub(crate) fn dq_windows(&self) -> Vec<(u64, u64)> {
        self.dq.clone()
    }
    pub(crate) fn pending_byte_effects(&self) -> Vec<Effect> {
        self.pending
            .iter()
            .filter_map(|p| match &p.kind {
                PendingKind::Output(e @ Effect::ReadByte { .. })
                | PendingKind::Output(e @ Effect::WriteByte { .. })
                | PendingKind::Output(e @ Effect::BurstComplete { .. }) => Some(e.clone()),
                _ => None,
            })
            .collect()
    }
    fn dqs(&mut self, high: bool) -> Result<(), Violation> {
        let now = self.now_ps;
        let level = self.level.as_mut().ok_or_else(|| invalid("WLEV_allowed"))?;
        if let Some((at, last)) = level.last_edge {
            if high == last {
                return Err(invalid("WLEV_pulse"));
            }
            wait(now, plus(at, 384)?, "WLEV_pulse")?;
        }
        if !high && level.first_low.is_none() {
            wait(now, cycle_after(level.entered, 25)?, "WLEV_drive")?;
            level.first_low = Some(now);
        }
        if high {
            if level.first_low.is_none() {
                return Err(invalid("WLEV_first_edge"));
            }
            wait(now, cycle_after(level.entered, 40)?, "WLEV_first_edge")?;
        }
        level.last_edge = Some((now, high));
        if high && self.mr[1].is_none_or(|value| value & 0x1000 == 0) {
            let phase = now % TCK;
            let distance = phase.min(TCK - phase).min(phase.abs_diff(HALF));
            let bits = if distance < 109 {
                None
            } else {
                Some([phase < HALF; 8])
            };
            self.output(Effect::LevelFeedback {
                at_ps: plus(now, 9_500)?,
                bits,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn isolated() -> Device {
        let mut d = Device::new(Config {
            mode_id: MODE.into(),
            source_sha256: SOURCE_SHA256.into(),
            base: 0,
            size: CAPACITY,
            queue_capacity: 1,
        })
        .unwrap();
        d.phase = Phase::Ready;
        d.cke = true;
        d.zq_initialized = true;
        d.initialization_step = 8;
        d.mr = MR.map(Some);
        d
    }
    #[test]
    fn independently_isolated_maximum_guards_use_late_inverse_boundaries() {
        // These fixtures isolate each guard; production schedules must satisfy
        // the intersection (the refresh guard can require PRE before tRASmax).
        let mut d = isolated();
        d.banks[0].row = Some(0);
        d.banks[0].activated = Some(0);
        assert!(d.check_maxima(70_200_000).is_ok());
        assert_eq!(d.check_maxima(70_200_001).unwrap_err().rule, "ACT_open_max");
        let mut d = isolated();
        d.phase = Phase::PowerDown { entered: 0 };
        assert!(d.check_maxima(70_200_000).is_ok());
        assert_eq!(
            d.check_maxima(70_200_001).unwrap_err().rule,
            "PD_duration_max"
        );
        let mut d = isolated();
        d.refresh_origin = Some(0);
        d.last_refresh = Some(0);
        assert!(d.check_maxima(9 * 7_800_000 - 1).is_ok());
        assert_eq!(
            d.check_maxima(9 * 7_800_000).unwrap_err().rule,
            "refresh_postpone_bound"
        );
        d.refresh_count = 8;
        assert!(d.check_maxima(0).is_ok());
        d.refresh_count = 9;
        assert_eq!(
            d.check_maxima(0).unwrap_err().rule,
            "refresh_postpone_bound"
        );
    }
    #[test]
    fn clock_hold_setup_are_fixed_continuous_clock_invariants() {
        assert_eq!(TCK, 834);
        assert_eq!(HALF, 417);
        assert_eq!(HALF * 2, TCK);
        let d = isolated();
        assert!(d.config.validate().is_ok());
        // This contract supplies a clock from0 and never stops it; the typed
        // command API provides no input that could silently stop its clock.
        assert!(d.clone().issue(Command::Des).is_ok());
    }
}
