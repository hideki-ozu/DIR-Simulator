//! Executable immutable planners. Scheduling, queues, wire and dynamic policy belong to parent.
use crate::{
    snapshot::ethernet::tsn::*,
    types::{Diagnostic, ethernet::tsn::*},
};
use std::{collections::BTreeMap, sync::Arc};
type Result<T> = std::result::Result<T, Diagnostic>;
fn error(target: &str, message: impl Into<String>) -> Diagnostic {
    let mut d = Diagnostic::execution(message)
        .with_reason("arithmetic_overflow")
        .with_target(target);
    d.code = "E-0004".into();
    d
}
fn increment(n: u64, target: &str) -> Result<u64> {
    n.checked_add(1)
        .ok_or_else(|| error(target, "generation overflow"))
}
fn ceil(n: u128, d: u128) -> u128 {
    n / d + u128::from(n % d != 0)
}
fn min_some(a: Option<u128>, b: Option<u128>) -> Option<u128> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

impl Schedule {
    pub(crate) fn open(&self, priority: u8, time: u64) -> bool {
        if priority >= 8 || time < self.base_ps {
            return false;
        }
        let offset = (time - self.base_ps) % self.cycle_ps;
        let i = self.prefix_ps.partition_point(|p| *p <= offset) - 1;
        self.entries[i].open_mask & (1 << priority) != 0
    }
    fn cumulative_open(&self, priority: usize, time: u64) -> u128 {
        if time <= self.base_ps {
            return 0;
        }
        let dt = time - self.base_ps;
        let cycles = dt / self.cycle_ps;
        let rem = dt % self.cycle_ps;
        let partial = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.open_mask & (1 << priority) != 0)
            .map(|(i, _)| {
                rem.min(self.prefix_ps[i + 1])
                    .saturating_sub(self.prefix_ps[i]) as u128
            })
            .sum::<u128>();
        cycles as u128 * self.open_total_ps[priority] as u128 + partial
    }
    /// O(entries), independent of elapsed cycle count. Base-before elapsed is closed.
    pub(crate) fn open_elapsed(&self, priority: u8, from: u64, to: u64) -> Result<u64> {
        if priority >= 8 || to < from {
            return Err(error(&self.id, "invalid elapsed interval/class"));
        }
        u64::try_from(
            self.cumulative_open(priority as usize, to)
                - self.cumulative_open(priority as usize, from),
        )
        .map_err(|_| error(&self.id, "open elapsed overflow"))
    }
    /// Next actual open/closed transition, skipping adjacent entries with equal class state.
    pub(crate) fn next_boundary(&self, priority: u8, time: u64) -> Option<u128> {
        if priority >= 8 {
            return None;
        }
        if time < self.base_ps {
            return Some(self.base_ps as u128);
        }
        let total = self.open_total_ps[priority as usize];
        if total == 0 || total == self.cycle_ps {
            return None;
        }
        let dt = time - self.base_ps;
        let q = dt / self.cycle_ps;
        let rem = dt % self.cycle_ps;
        let mut best = None;
        for i in 0..self.entries.len() {
            let prev = if i == 0 {
                self.entries.len() - 1
            } else {
                i - 1
            };
            if (self.entries[prev].open_mask ^ self.entries[i].open_mask) & (1 << priority) == 0 {
                continue;
            }
            let offset = self.prefix_ps[i];
            let k = q as u128 + u128::from(offset <= rem);
            best = min_some(
                best,
                Some(self.base_ps as u128 + k * self.cycle_ps as u128 + offset as u128),
            );
        }
        best
    }
    /// Finds a fit with at most two occurrences of each merged run, without periodic ticks.
    pub(crate) fn next_fit(
        &self,
        priority: u8,
        time: u64,
        occupancy: u64,
        clip: Option<u64>,
    ) -> Option<u128> {
        if priority >= 8 {
            return None;
        }
        let c = priority as usize;
        let now = time.max(self.base_ps) as u128;
        let limit = clip.map(|t| t as u128);
        if limit.is_some_and(|t| now >= t) {
            return None;
        }
        if self.open_total_ps[c] == self.cycle_ps {
            return limit
                .is_none_or(|t| now + occupancy as u128 <= t)
                .then_some(now);
        }
        let q = (now - self.base_ps as u128) / self.cycle_ps as u128;
        let mut best = None;
        for run in &self.class_open_runs[c] {
            for k in [q, q + 1] {
                let origin = self.base_ps as u128 + k * self.cycle_ps as u128;
                let start = now.max(origin + run.start_ps as u128);
                let end = limit.map_or(origin + run.end_ps, |t| t.min(origin + run.end_ps));
                if start + occupancy as u128 <= end {
                    best = min_some(best, Some(start));
                }
            }
        }
        best
    }
}
/// Separate rounding from SOF for EOF and IFG-inclusive release.
pub(crate) fn wire_times(mac_bytes: u64, link_bps: u64) -> Result<(u64, u64)> {
    if link_bps == 0 {
        return Err(error("link_bps", "zero bitrate"));
    }
    let duration = |extra: u128| {
        u64::try_from(ceil((mac_bytes as u128 + extra) * 8 * Q, link_bps as u128))
            .map_err(|_| error("mac_bytes", "wire duration exceeds u64"))
    };
    Ok((duration(8)?, duration(20)?))
}
fn add_credit(c: Credit, amount: u128, cap: u128) -> Credit {
    if c.negative {
        if amount < c.magnitude {
            Credit::new(true, c.magnitude - amount)
        } else {
            Credit::new(false, (amount - c.magnitude).min(cap))
        }
    } else {
        Credit::new(
            false,
            c.magnitude.min(cap) + amount.min(cap.saturating_sub(c.magnitude)),
        )
    }
}
fn subtract_credit(c: Credit, amount: u128, cap: u128) -> Credit {
    if c.negative {
        Credit::new(
            true,
            c.magnitude.min(cap) + amount.min(cap.saturating_sub(c.magnitude)),
        )
    } else if amount <= c.magnitude {
        Credit::new(false, c.magnitude - amount)
    } else {
        Credit::new(true, (amount - c.magnitude).min(cap))
    }
}
#[derive(Debug, Clone)]
pub(crate) struct CbsState {
    pub credit: Credit,
    pub last_ps: u64,
    pub mode: CreditMode,
    pub backlog: bool,
    pub sending: bool,
}
impl CbsState {
    fn advance(
        &mut self,
        cfg: &CbsConfig,
        schedule: Option<&Schedule>,
        now: u64,
        target: &str,
    ) -> Result<()> {
        let dt = now
            .checked_sub(self.last_ps)
            .ok_or_else(|| error(target, "credit time reversal"))?;
        if self.sending {
            self.credit = subtract_credit(
                self.credit,
                (cfg.link_bps - cfg.idle_bps) as u128 * dt as u128,
                cfg.lo,
            );
        } else if !self.backlog && !self.credit.negative {
            self.credit = Credit::default();
        } else {
            let open = match schedule {
                Some(s) => s.open_elapsed(cfg.priority, self.last_ps, now)?,
                None => dt,
            };
            self.credit = add_credit(
                self.credit,
                cfg.idle_bps as u128 * open as u128,
                if self.backlog { cfg.hi } else { 0 },
            );
        }
        self.last_ps = now;
        Ok(())
    }
    fn derive(&mut self, open: bool) {
        if !self.sending && !self.backlog && !self.credit.negative {
            self.credit = Credit::default();
        }
        self.mode = if self.sending {
            CreditMode::Sending
        } else if !self.backlog && !self.credit.negative {
            CreditMode::Zero
        } else if !open {
            CreditMode::Frozen
        } else if self.backlog {
            CreditMode::Accumulating
        } else {
            CreditMode::Recovering
        };
    }
    fn slope(&self, cfg: &CbsConfig) -> Credit {
        match self.mode {
            CreditMode::Sending => Credit::new(true, (cfg.link_bps - cfg.idle_bps) as u128),
            CreditMode::Accumulating | CreditMode::Recovering => {
                Credit::new(false, cfg.idle_bps as u128)
            }
            _ => Credit::default(),
        }
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Head {
    pub occupancy_ps: u64,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sending {
    pub priority: u8,
    pub release_ps: u64,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct PortInput {
    pub heads: [Option<Head>; 8],
    /// Waiting backlog excludes on-wire frame. SOF planner prioritizes Sending over this.
    pub backlog: [bool; 8],
    pub sending: Option<Sending>,
    pub policy_epoch: u64,
}
#[derive(Debug, Clone)]
struct PortState {
    oper: Option<Arc<Schedule>>,
    schedule_generation: u64,
    wake_generation: u64,
    next_wake: Option<u128>,
    cbs: [Option<CbsState>; 8],
    last_ps: u64,
    gate_mask: Option<u8>,
    sending: Option<Sending>,
}
#[derive(Debug, Clone)]
struct MeterState {
    committed: u128,
    peak: u128,
    last_evaluated_ps: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WakeToken {
    pub port: String,
    pub time_ps: u64,
    pub wake_generation: u64,
    pub schedule_generation: u64,
}
#[derive(Debug)]
pub(crate) struct TsnState {
    prepared: Arc<PreparedTsn>,
    ports: Vec<PortState>,
    meters: Vec<Option<MeterState>>,
    update_cursor: usize,
}
#[derive(Debug, Default)]
pub(crate) struct TsnDelta {
    ports: Vec<(usize, PortState)>,
    meters: Vec<(usize, MeterState)>,
    update_cursor: Option<usize>,
    pub records: Vec<TsnRecord>,
    pub wakes: Vec<WakeToken>,
    /// One highest ready class per affected port, FIFO heads supplied by parent.
    pub selected: BTreeMap<String, u8>,
    pub psfp: Option<PolicingRecord>,
    pub gcl_updates: u64,
    pub guard_blocked: Vec<(String, u8)>,
}
impl TsnState {
    pub(crate) fn new(prepared: &PreparedTsn) -> Self {
        let ports = prepared
            .outputs
            .iter()
            .map(|o| PortState {
                oper: o.tas.clone(),
                schedule_generation: 0,
                wake_generation: 0,
                next_wake: None,
                last_ps: 0,
                gate_mask: None,
                sending: None,
                cbs: std::array::from_fn(|i| {
                    o.cbs[i].as_ref().map(|_| CbsState {
                        credit: Credit::default(),
                        last_ps: 0,
                        mode: CreditMode::Zero,
                        backlog: false,
                        sending: false,
                    })
                }),
            })
            .collect();
        let meters = prepared
            .streams
            .iter()
            .map(|s| {
                s.meter.as_ref().map(|m| MeterState {
                    committed: m.committed_cap,
                    peak: m.peak_cap,
                    last_evaluated_ps: 0,
                })
            })
            .collect();
        Self {
            prepared: Arc::new(prepared.clone()),
            ports,
            meters,
            update_cursor: 0,
        }
    }
    pub(crate) fn next_control(&self) -> Option<u64> {
        self.prepared
            .updates
            .get(self.update_cursor)
            .map(|u| u.effective_at_ps)
    }
    pub(crate) fn next_control_after(&self, delta: &TsnDelta) -> Option<u64> {
        match delta.update_cursor {
            Some(cursor) => self
                .prepared
                .updates
                .get(cursor)
                .map(|update| update.effective_at_ps),
            None => self.next_control(),
        }
    }
    fn index(&self, port: &str) -> Result<usize> {
        self.prepared
            .port_index
            .get(port)
            .copied()
            .ok_or_else(|| error(port, "unknown TSN output"))
    }
    fn next_update(&self, port: &str, cursor: usize) -> Option<u64> {
        self.prepared.updates[cursor..]
            .iter()
            .find(|u| u.port == port)
            .map(|u| u.effective_at_ps)
    }
    fn advance_port(&self, i: usize, p: &mut PortState, now: u64) -> Result<()> {
        let o = &self.prepared.outputs[i];
        if now < p.last_ps {
            return Err(error(&o.port, "port time reversal"));
        }
        for (c, state) in p.cbs.iter_mut().enumerate() {
            if let Some(state) = state {
                state.advance(o.cbs[c].as_ref().unwrap(), p.oper.as_deref(), now, &o.port)?;
            }
        }
        p.last_ps = now;
        Ok(())
    }
    /// Apply due GCL controls and evaluate supplied post-change queue states atomically.
    /// Caller must include a PortInput for every port with a due update.
    pub(crate) fn plan_control(
        &self,
        now: u64,
        inputs: &BTreeMap<String, PortInput>,
    ) -> Result<TsnDelta> {
        let mut d = TsnDelta::default();
        let mut staged = BTreeMap::new();
        let mut cursor = self.update_cursor;
        while let Some(u) = self
            .prepared
            .updates
            .get(cursor)
            .filter(|u| u.effective_at_ps <= now)
        {
            if u.effective_at_ps != now {
                return Err(error(&u.port, "missed GCL effective time"));
            }
            if !inputs.contains_key(&u.port) {
                return Err(error(&u.port, "missing post-control port input"));
            }
            let i = self.index(&u.port)?;
            let p = staged.entry(i).or_insert_with(|| self.ports[i].clone());
            self.advance_port(i, p, now)?;
            p.oper = Some(u.schedule.clone());
            p.schedule_generation = increment(p.schedule_generation, &u.port)?;
            p.gate_mask = None;
            d.gcl_updates = increment(d.gcl_updates, &u.port)?;
            cursor += 1;
        }
        for (port, input) in inputs {
            let i = self.index(port)?;
            let mut p = staged.remove(&i).unwrap_or_else(|| self.ports[i].clone());
            self.advance_port(i, &mut p, now)?;
            let changed = p.schedule_generation != self.ports[i].schedule_generation;
            self.finish_port(
                i,
                p,
                now,
                input,
                cursor,
                if changed { "update" } else { "boundary" },
                &mut d,
            )?;
        }
        if cursor != self.update_cursor {
            d.update_cursor = Some(cursor);
        }
        Ok(d)
    }
    #[cfg(test)]
    pub(crate) fn plan_port(&self, port: &str, now: u64, input: &PortInput) -> Result<TsnDelta> {
        self.plan_port_in(TsnDelta::default(), port, now, input)
    }
    fn staged_port(&self, d: &mut TsnDelta, i: usize) -> PortState {
        d.wakes.retain(|w| w.port != self.prepared.outputs[i].port);
        d.selected.remove(&self.prepared.outputs[i].port);
        if let Some(at) = d.ports.iter().position(|(index, _)| *index == i) {
            d.ports.swap_remove(at).1
        } else {
            self.ports[i].clone()
        }
    }
    /// Compose a queue change/selection into an owned delta, with no early state commit.
    pub(crate) fn plan_port_in(
        &self,
        mut d: TsnDelta,
        port: &str,
        now: u64,
        input: &PortInput,
    ) -> Result<TsnDelta> {
        let cursor = d.update_cursor.unwrap_or(self.update_cursor);
        if self
            .prepared
            .updates
            .get(cursor)
            .is_some_and(|u| u.effective_at_ps <= now)
        {
            return Err(error(
                port,
                "GCL control must be planned before port evaluation",
            ));
        }
        let i = self.index(port)?;
        let mut p = self.staged_port(&mut d, i);
        self.advance_port(i, &mut p, now)?;
        self.finish_port(i, p, now, input, cursor, "boundary", &mut d)?;
        Ok(d)
    }
    #[allow(clippy::too_many_arguments)]
    fn finish_port(
        &self,
        i: usize,
        mut p: PortState,
        now: u64,
        input: &PortInput,
        cursor: usize,
        cause: &str,
        d: &mut TsnDelta,
    ) -> Result<()> {
        let o = &self.prepared.outputs[i];
        let clip = self.next_update(&o.port, cursor);
        if input
            .sending
            .is_some_and(|s| s.priority >= 8 || s.release_ps <= now)
        {
            return Err(error(
                &o.port,
                "invalid sending state; release must be processed first",
            ));
        }
        let mask = (0..8).fold(0, |m, c| {
            m | if p.oper.as_ref().is_none_or(|s| s.open(c, now)) {
                1 << c
            } else {
                0
            }
        });
        let boundary = min_some(
            clip.map(|t| t as u128),
            (0..8).fold(None, |best, c| {
                min_some(best, p.oper.as_ref().and_then(|s| s.next_boundary(c, now)))
            }),
        );
        if p.gate_mask != Some(mask) || cause == "update" {
            d.records.push(TsnRecord::Gate(GateRecord {
                time_ps: now,
                port: o.port.clone(),
                schedule_id: p.oper.as_ref().map(|s| s.id.clone()),
                generation: p.schedule_generation,
                open_priorities: (0..8).filter(|c| mask & (1 << c) != 0).collect(),
                next_boundary_ps: boundary,
                cause: if p.gate_mask.is_none() && cause != "update" {
                    "initial"
                } else if cause == "update" {
                    "update"
                } else {
                    "boundary"
                }
                .into(),
            }));
        }
        p.gate_mask = Some(mask);
        p.sending = input.sending;
        let mut wake = clip.map(|t| t as u128);
        let mut decisions = Vec::new();
        if input.heads.iter().any(Option::is_some) {
            wake = min_some(
                wake,
                p.oper
                    .as_ref()
                    .filter(|g| now < g.base_ps)
                    .map(|g| g.base_ps as u128),
            );
        }
        if let Some(s) = input.sending {
            wake = min_some(wake, Some(s.release_ps as u128));
        }
        for c in 0..8 {
            let open = mask & (1 << c) != 0;
            if let Some(s) = &mut p.cbs[c] {
                let observed = d
                    .records
                    .iter()
                    .rev()
                    .find_map(|r| match r {
                        TsnRecord::Credit(r) if r.port == o.port && r.priority as usize == c => {
                            Some((r.credit, r.mode))
                        }
                        _ => None,
                    })
                    .or_else(|| self.ports[i].cbs[c].as_ref().map(|s| (s.credit, s.mode)));
                s.backlog = input.backlog[c];
                s.sending = input
                    .sending
                    .is_some_and(|send| send.priority as usize == c);
                s.derive(open);
                let cfg = o.cbs[c].as_ref().unwrap();
                if observed != Some((s.credit, s.mode)) || cause == "update" {
                    d.records.push(TsnRecord::Credit(CreditRecord {
                        time_ps: now,
                        port: o.port.clone(),
                        priority: c as u8,
                        credit: s.credit,
                        slope: s.slope(cfg),
                        mode: s.mode,
                        cause: cause.into(),
                    }));
                }
                // Sending ports wake at release; waiting negative credit wakes are recomputed there.
                if input.sending.is_none() && s.credit.negative {
                    let b = p.oper.as_ref().and_then(|g| g.next_boundary(c as u8, now));
                    if open {
                        wake = min_some(
                            wake,
                            min_some(
                                Some(now as u128 + ceil(s.credit.magnitude, cfg.idle_bps as u128)),
                                b,
                            ),
                        );
                    } else if s.backlog || p.oper.as_ref().is_some_and(|g| g.open_total_ps[c] != 0)
                    {
                        wake = min_some(wake, b);
                    }
                }
            }
            let Some(head) = input.heads[c] else {
                continue;
            };
            if head.occupancy_ps == 0 || !input.backlog[c] {
                return Err(error(
                    &o.port,
                    "invalid zero-duration or non-backlogged head",
                ));
            }
            let fit = match p.oper.as_ref() {
                Some(s) => s.next_fit(c as u8, now, head.occupancy_ps, clip),
                None => clip
                    .is_none_or(|t| now as u128 + head.occupancy_ps as u128 <= t as u128)
                    .then_some(now as u128),
            };
            let negative = p.cbs[c].as_ref().is_some_and(|s| s.credit.negative);
            let state = if input.sending.is_some() {
                "busy"
            } else if p.oper.as_ref().is_some_and(|s| now < s.base_ps) {
                "no_active_schedule"
            } else if p
                .oper
                .as_ref()
                .is_some_and(|s| s.next_fit(c as u8, now, head.occupancy_ps, None).is_none())
            {
                "never_eligible"
            } else if !open {
                "gate_closed"
            } else if fit != Some(now as u128) {
                "guard_blocked"
            } else if negative {
                "credit_negative"
            } else {
                "ready"
            };
            if state == "guard_blocked" && !d.guard_blocked.contains(&(o.port.clone(), c as u8)) {
                d.guard_blocked.push((o.port.clone(), c as u8));
            }
            if input.sending.is_none() {
                if let Some(f) = fit.filter(|f| *f > now as u128) {
                    wake = min_some(wake, Some(f));
                }
                if state == "ready" {
                    d.selected.insert(o.port.clone(), c as u8);
                }
            }
            decisions.push((c as u8, state));
        }
        // Never generate zero-delay timer loops; phase2 consumes ready selections directly.
        wake = wake.filter(|t| *t > now as u128);
        p.wake_generation = increment(p.wake_generation, &o.port)?;
        p.next_wake = wake;
        if let Some(time_ps) = wake.and_then(|n| u64::try_from(n).ok()) {
            d.wakes.push(WakeToken {
                port: o.port.clone(),
                time_ps,
                wake_generation: p.wake_generation,
                schedule_generation: p.schedule_generation,
            });
        }
        if decisions.is_empty() {
            decisions.push((
                0,
                if input.sending.is_some() {
                    "busy"
                } else {
                    "ready"
                },
            ));
        }
        for (c, state) in decisions {
            d.records.push(TsnRecord::Decision(DecisionRecord {
                time_ps: now,
                port: o.port.clone(),
                transfer_id: None,
                priority: input.heads[c as usize].map(|_| c),
                state: state.into(),
                policy_epoch: input.policy_epoch,
                schedule_generation: p.schedule_generation,
                next_wake_ps: wake,
                reason: state.into(),
            }));
        }
        d.ports.push((i, p));
        Ok(())
    }
    /// Parent supplies queues after removing the selected FIFO head, including other classes.
    #[cfg(test)]
    pub(crate) fn plan_start(
        &self,
        port: &str,
        priority: u8,
        now: u64,
        mac_bytes: u64,
        post: &PortInput,
    ) -> Result<(TsnDelta, u64, u64)> {
        self.plan_start_in(TsnDelta::default(), port, priority, now, mac_bytes, post)
    }
    /// Compose a SOF with prior control/selection changes, keeping only the final wake.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_start_in(
        &self,
        mut d: TsnDelta,
        port: &str,
        priority: u8,
        now: u64,
        mac_bytes: u64,
        post: &PortInput,
    ) -> Result<(TsnDelta, u64, u64)> {
        let cursor = d.update_cursor.unwrap_or(self.update_cursor);
        if priority >= 8 {
            return Err(error(port, "priority out of range"));
        }
        if self
            .prepared
            .updates
            .get(cursor)
            .is_some_and(|u| u.effective_at_ps <= now)
        {
            return Err(error(port, "GCL control must precede SOF"));
        }
        let i = self.index(port)?;
        let mut p = self.staged_port(&mut d, i);
        self.advance_port(i, &mut p, now)?;
        let (serialization, occupancy) = wire_times(mac_bytes, self.prepared.outputs[i].link_bps)?;
        let eof = now
            .checked_add(serialization)
            .ok_or_else(|| error(port, "SOF+serialization overflow"))?;
        let release = now
            .checked_add(occupancy)
            .ok_or_else(|| error(port, "SOF+occupancy overflow"))?;
        let clip = self.next_update(port, cursor);
        if p.sending.is_some()
            || p.cbs[priority as usize]
                .as_ref()
                .is_some_and(|s| s.credit.negative)
            || p.oper
                .as_ref()
                .is_some_and(|s| s.next_fit(priority, now, occupancy, clip) != Some(now as u128))
            || clip.is_some_and(|t| release > t)
        {
            return Err(error(port, "SOF is ineligible"));
        }
        let mut input = post.clone();
        input.sending = Some(Sending {
            priority,
            release_ps: release,
        });
        self.finish_port(i, p, now, &input, cursor, "sof", &mut d)?;
        Ok((d, eof, release))
    }
    pub(crate) fn wake_valid(&self, token: &WakeToken) -> bool {
        self.wake_valid_in(&TsnDelta::default(), token)
    }
    pub(crate) fn wake_valid_in(&self, d: &TsnDelta, token: &WakeToken) -> bool {
        self.prepared.port_index.get(&token.port).is_some_and(|i| {
            let p = d
                .ports
                .iter()
                .find(|(index, _)| index == i)
                .map_or(&self.ports[*i], |(_, p)| p);
            p.wake_generation == token.wake_generation
                && p.schedule_generation == token.schedule_generation
                && p.next_wake == Some(token.time_ps as u128)
        })
    }
    #[cfg(test)]
    pub(crate) fn plan_wake(&self, token: &WakeToken, input: &PortInput) -> Result<TsnDelta> {
        self.plan_wake_in(TsnDelta::default(), token, input)
    }
    #[cfg(test)]
    pub(crate) fn plan_wake_in(
        &self,
        d: TsnDelta,
        token: &WakeToken,
        input: &PortInput,
    ) -> Result<TsnDelta> {
        if !self.wake_valid_in(&d, token) {
            return Ok(d);
        }
        self.plan_port_in(d, &token.port, token.time_ps, input)
    }
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) fn plan_arrival(
        &self,
        ingress: &str,
        dst_mac: &str,
        vid: u16,
        priority: u8,
        mac_bytes: u64,
        reception_id: &str,
        now: u64,
    ) -> Result<TsnDelta> {
        self.plan_arrival_in(
            TsnDelta::default(),
            ingress,
            dst_mac,
            vid,
            priority,
            mac_bytes,
            reception_id,
            now,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_arrival_in(
        &self,
        mut d: TsnDelta,
        ingress: &str,
        dst_mac: &str,
        vid: u16,
        priority: u8,
        mac_bytes: u64,
        reception_id: &str,
        now: u64,
    ) -> Result<TsnDelta> {
        let key = StreamKey {
            ingress: ingress.into(),
            dst_mac: dst_mac.to_ascii_lowercase(),
            vid,
            priority,
        };
        let found = self.prepared.stream_index.get(&key).copied();
        let mut r = PolicingRecord {
            time_ps: now,
            stream_id: found.map(|i| self.prepared.streams[i].id.clone()),
            reception_id: reception_id.into(),
            ingress: ingress.into(),
            mac_bytes,
            verdict: if found.is_some() { "pass" } else { "bypass" }.into(),
            reason: if found.is_some() { "pass" } else { "bypass" }.into(),
            color: None,
            committed_before: None,
            committed_after: None,
            peak_before: None,
            peak_after: None,
            consumed_bits: 0,
        };
        if let Some(i) = found {
            let cfg = &self.prepared.streams[i];
            if mac_bytes > cfg.max_sdu_bytes {
                r.verdict = "drop".into();
                r.reason = "psfp_max_sdu".into();
            } else if cfg.gate.as_ref().is_some_and(|g| !g.open(priority, now)) {
                r.verdict = "drop".into();
                r.reason = "psfp_gate_closed".into();
            } else if let Some(m) = &cfg.meter {
                let mut s = if let Some(at) = d.meters.iter().position(|(index, _)| *index == i) {
                    d.meters.swap_remove(at).1
                } else {
                    self.meters[i].as_ref().unwrap().clone()
                };
                let dt = now
                    .checked_sub(s.last_evaluated_ps)
                    .ok_or_else(|| error(&cfg.id, "meter time reversal"))?;
                s.committed += ((m.committed_rate_bps as u128) * dt as u128)
                    .min(m.committed_cap - s.committed);
                s.peak += ((m.peak_rate_bps as u128) * dt as u128).min(m.peak_cap - s.peak);
                r.committed_before = Some(s.committed);
                r.peak_before = Some(s.peak);
                let bits = mac_bytes as u128 * 8;
                let cost = bits * Q;
                if s.peak < cost {
                    r.color = Some("red".into());
                    r.verdict = "drop".into();
                    r.reason = "psfp_meter_red".into();
                } else if s.committed < cost {
                    r.color = Some("yellow".into());
                    s.peak -= cost;
                    r.consumed_bits = bits;
                    if m.yellow_drop {
                        r.verdict = "drop".into();
                        r.reason = "psfp_meter_yellow".into();
                    }
                } else {
                    r.color = Some("green".into());
                    s.committed -= cost;
                    s.peak -= cost;
                    r.consumed_bits = bits * 2;
                }
                r.committed_after = Some(s.committed);
                r.peak_after = Some(s.peak);
                s.last_evaluated_ps = now;
                d.meters.push((i, s));
            }
        }
        d.psfp = Some(r.clone());
        d.records.push(TsnRecord::Policing(r));
        Ok(d)
    }
    /// All delta storage is preallocated in planning; commit performs only indexed swaps.
    pub(crate) fn apply(&mut self, d: TsnDelta) {
        for (i, p) in d.ports {
            self.ports[i] = p;
        }
        for (i, m) in d.meters {
            self.meters[i] = Some(m);
        }
        if let Some(cursor) = d.update_cursor {
            self.update_cursor = cursor;
        }
    }
    /// Project credit to the left limit at H without applying controls/release at H.
    pub(crate) fn snapshot(&self, h: u64) -> Result<TsnSnapshot> {
        let mut ports = Vec::new();
        for (i, current) in self.ports.iter().enumerate() {
            let mut p = current.clone();
            self.advance_port(i, &mut p, h)?;
            for (c, s) in p.cbs.iter_mut().enumerate() {
                if let Some(s) = s {
                    s.derive(
                        p.oper
                            .as_ref()
                            .is_none_or(|g| g.open(c as u8, h.saturating_sub(1))),
                    );
                }
            }
            let credits = p
                .cbs
                .iter()
                .enumerate()
                .filter_map(|(c, s)| {
                    s.as_ref().map(|s| CreditSnapshot {
                        priority: c as u8,
                        credit: s.credit,
                        last_ps: s.last_ps,
                        mode: s.mode,
                        backlog: s.backlog,
                        sending: s.sending,
                    })
                })
                .collect();
            ports.push(PortTsnSnapshot {
                port: self.prepared.outputs[i].port.clone(),
                schedule_id: p.oper.as_ref().map(|s| s.id.clone()),
                schedule_generation: p.schedule_generation,
                wake_generation: p.wake_generation,
                next_wake_ps: p.next_wake,
                credits,
            });
        }
        let meters = self
            .meters
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.as_ref().map(|s| MeterSnapshot {
                    stream_id: self.prepared.streams[i].id.clone(),
                    committed: s.committed,
                    peak: s.peak,
                    last_evaluated_ps: s.last_evaluated_ps,
                })
            })
            .collect();
        Ok(TsnSnapshot {
            ports,
            meters,
            update_cursor: self.update_cursor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ethernet::tsn::tests::{config, gate, prep, stream};
    use serde_json::{Value, json};
    const PORT: &str = "N.a.tx";
    fn tas(base: u64, cycle: u64, rows: &[(u64, bool)]) -> Arc<Schedule> {
        let mut v = config();
        v["tsn"]["outputs"][0]["tas"] = gate(base, cycle, rows);
        prep(v).outputs[0].tas.clone().unwrap()
    }
    fn queued(c: u8, occupancy: u64) -> PortInput {
        let mut input = PortInput::default();
        input.heads[c as usize] = Some(Head {
            occupancy_ps: occupancy,
        });
        input.backlog[c as usize] = true;
        input
    }
    fn cbs_config() -> Value {
        let mut v = config();
        v["tsn"]["outputs"][0]["cbs"] = json!([{"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}]);
        v
    }
    fn amount(state: &TsnState, h: u64) -> Credit {
        state.snapshot(h).unwrap().ports[0].credits[0].credit
    }
    fn policing(state: &mut TsnState, t: u64, m: u64) -> PolicingRecord {
        let d = state
            .plan_arrival("N.b.rx", "01:00:5e:00:00:01", 1, 7, m, "rx", t)
            .unwrap();
        let r = d.psfp.clone().unwrap();
        state.apply(d);
        r
    }
    #[test]
    fn tas_01_to_05_ifg_exact_fit_adjacent_and_circular() {
        assert_eq!(wire_times(64, 1_000_000_000).unwrap(), (576_000, 672_000));
        assert_eq!(wire_times(68, 1_000_000_000).unwrap(), (608_000, 704_000));
        let g = tas(0, 2_000_000, &[(1_000_000, true), (1_000_000, false)]);
        for (offer, m, next) in [
            (0, 64, 0),
            (328000, 64, 328000),
            (328001, 64, 2000000),
            (296000, 68, 296000),
            (296001, 68, 2000000),
        ] {
            assert_eq!(
                g.next_fit(7, offer, wire_times(m, 1_000_000_000).unwrap().1, None),
                Some(next)
            );
        }
        let g = tas(
            0,
            2_000_000,
            &[(400000, true), (600000, true), (1000000, false)],
        );
        assert_eq!(g.next_fit(7, 0, 672000, None), Some(0));
        let g = tas(
            0,
            1_000_000,
            &[(400000, true), (300000, false), (300000, true)],
        );
        assert_eq!(g.next_fit(7, 700000, 672000, None), Some(700000));
        assert_eq!(g.next_fit(7, 700000, 700001, None), None);
    }
    #[test]
    fn initial_base_never_borrows_prior_cycle_and_elapsed_skips_cycles() {
        let g = tas(100, 10, &[(4, true), (3, false), (3, true)]);
        assert_eq!(g.next_fit(7, 100, 6, None), Some(107));
        assert_eq!(g.next_fit(7, 107, 6, None), Some(107));
        assert_eq!(g.next_fit(7, 110, 6, None), Some(117));
        assert_eq!(g.next_fit(7, 107, 6, Some(112)), None);
        assert_eq!(g.open_elapsed(7, 95, 135).unwrap(), 25);
        assert!(!g.open(7, 104));
        assert!(g.open(7, 107));
        assert_eq!(
            g.open_elapsed(7, 100, 10_000_000_100).unwrap(),
            7_000_000_000
        );
        assert!(g.open_elapsed(7, 105, 104).is_err());
    }
    #[test]
    fn tas_06_07_future_base_closed_and_oversized_no_periodic_tick() {
        for rows in [
            vec![(1_000_000, false)],
            vec![(600000, true), (400000, false)],
        ] {
            let mut v = config();
            v["tsn"]["outputs"][0]["tas"] = gate(0, 1_000_000, &rows);
            let state = TsnState::new(&prep(v));
            let d = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap();
            assert!(d.selected.is_empty());
            assert!(d.wakes.is_empty());
            assert!(
                d.records
                    .iter()
                    .any(|r| matches!(r,TsnRecord::Decision(r) if r.state=="never_eligible"))
            );
        }
        let mut v = config();
        v["tsn"]["outputs"][0]["tas"] = gate(
            1_000_000,
            2_000_000,
            &[(1_000_000, true), (1_000_000, false)],
        );
        let mut state = TsnState::new(&prep(v));
        let d = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap();
        assert_eq!(d.wakes[0].time_ps, 1_000_000);
        state.apply(d);
        let d = state
            .plan_port(PORT, 1_000_000, &queued(7, 672000))
            .unwrap();
        assert_eq!(d.selected[PORT], 7);
    }
    fn updated(mut v: Value, effective: u64, open: u64) -> Value {
        let mut s = gate(
            effective,
            2_000_000,
            &[(open, true), (2_000_000 - open, false)],
        );
        s["id"] = json!("g1");
        v["tsn"]["gcl_updates"] = json!([{"id":"u1","submitted_at_ps":"0","effective_at_ps":effective.to_string(),"port":PORT,"schedule":s}]);
        v
    }
    #[test]
    fn tas_08_09_10_control_clips_all_open_invalidates_old_wake() {
        let mut v = config();
        v["tsn"]["outputs"][0]["tas"] =
            gate(0, 2_000_000, &[(1_000_000, true), (1_000_000, false)]);
        let mut state = TsnState::new(&prep(updated(v, 2_000_000, 800000)));
        let input = queued(7, 672000);
        let d = state.plan_port(PORT, 1_900_000, &input).unwrap();
        let old = d.wakes[0].clone();
        state.apply(d);
        let d = state
            .plan_control(2_000_000, &BTreeMap::from([(PORT.into(), input)]))
            .unwrap();
        assert_eq!(d.gcl_updates, 1);
        assert_eq!(d.selected[PORT], 7);
        state.apply(d);
        assert!(!state.wake_valid(&old));
        let no = state.plan_wake(&old, &PortInput::default()).unwrap();
        assert!(no.records.is_empty());
        assert!(no.ports.is_empty());
        let (d, eof, release) = state
            .plan_start(PORT, 7, 2_000_000, 64, &PortInput::default())
            .unwrap();
        assert_eq!((eof, release), (2576000, 2672000));
        state.apply(d);
        let mut state = TsnState::new(&prep(updated(config(), 500000, 1000000)));
        let d = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap();
        assert!(d.selected.is_empty());
        assert_eq!(d.wakes[0].time_ps, 500000);
        assert_eq!(d.guard_blocked.len(), 1);
        state.apply(d);
        let d = state
            .plan_control(500000, &BTreeMap::from([(PORT.into(), queued(7, 672000))]))
            .unwrap();
        assert_eq!(d.selected[PORT], 7);
    }
    #[test]
    fn cbs_ifg_and_two_frame_independent_timeline() {
        let mut state = TsnState::new(&prep(cbs_config()));
        let (d, eof, release) = state
            .plan_start(PORT, 7, 0, 64, &queued(7, 672000))
            .unwrap();
        assert_eq!((eof, release), (576000, 672000));
        state.apply(d);
        assert_eq!(amount(&state, 576000), Credit::new(true, 432 * Q));
        let d = state.plan_port(PORT, 672000, &queued(7, 672000)).unwrap();
        assert!(d.selected.is_empty());
        assert_eq!(d.wakes[0].time_ps, 2688000);
        state.apply(d);
        assert_eq!(amount(&state, 672000), Credit::new(true, 504 * Q));
        let d = state.plan_port(PORT, 2688000, &queued(7, 672000)).unwrap();
        assert_eq!(d.selected[PORT], 7);
        state.apply(d);
        assert_eq!(amount(&state, 2688000), Credit::default());
        let (d, _, r) = state
            .plan_start(PORT, 7, 2688000, 64, &PortInput::default())
            .unwrap();
        assert_eq!(r, 3360000);
        state.apply(d);
        let d = state
            .plan_port(PORT, 3360000, &PortInput::default())
            .unwrap();
        assert_eq!(d.wakes[0].time_ps, 5376000);
        state.apply(d);
        assert_eq!(amount(&state, 3360000), Credit::new(true, 504 * Q));
        let d = state
            .plan_port(PORT, 5376000, &PortInput::default())
            .unwrap();
        assert!(d.wakes.is_empty());
        state.apply(d);
        assert_eq!(amount(&state, 5376000), Credit::default());
    }
    #[test]
    fn cbs_lazy_freeze_and_old_timer_noop() {
        let mut v = cbs_config();
        v["tsn"]["outputs"][0]["tas"] = gate(
            0,
            5_000_000,
            &[(1_000_000, true), (1_000_000, false), (3_000_000, true)],
        );
        let mut state = TsnState::new(&prep(v));
        let input = queued(7, 672000);
        let (d, _, _) = state.plan_start(PORT, 7, 0, 64, &input).unwrap();
        state.apply(d);
        let d = state.plan_port(PORT, 672000, &input).unwrap();
        assert_eq!(d.wakes[0].time_ps, 1_000_000);
        state.apply(d);
        let old = WakeToken {
            port: PORT.into(),
            time_ps: 2688000,
            wake_generation: state.ports[0].wake_generation,
            schedule_generation: 0,
        };
        let d = state.plan_port(PORT, 1_000_000, &input).unwrap();
        assert_eq!(d.wakes[0].time_ps, 2_000_000);
        state.apply(d);
        assert_eq!(amount(&state, 1_000_000), Credit::new(true, 422 * Q));
        assert_eq!(amount(&state, 2_000_000), Credit::new(true, 422 * Q));
        let d = state.plan_port(PORT, 2_000_000, &input).unwrap();
        assert_eq!(d.wakes[0].time_ps, 3688000);
        state.apply(d);
        assert!(state.plan_wake(&old, &input).unwrap().records.is_empty());
        // Skip all callbacks until 0 crossing: Frozen does not suppress intervening open elapsed.
        let d = state.plan_port(PORT, 3688000, &input).unwrap();
        assert_eq!(d.selected[PORT], 7);
        state.apply(d);
        assert_eq!(amount(&state, 3688000), Credit::default());
    }
    #[test]
    fn cbs_saturation_reset_fractional_recovery_and_full_u128() {
        let cfg = CbsConfig {
            priority: 7,
            idle_bps: 250000000,
            link_bps: 1000000000,
            hi: 100 * Q,
            lo: 100 * Q,
        };
        let mut s = CbsState {
            credit: Credit::default(),
            last_ps: 0,
            mode: CreditMode::Accumulating,
            backlog: true,
            sending: false,
        };
        s.advance(&cfg, None, 1_000_000, PORT).unwrap();
        assert_eq!(s.credit, Credit::new(false, 100 * Q));
        s.backlog = false;
        s.derive(false);
        assert_eq!(s.credit, Credit::default());
        s.sending = true;
        s.advance(&cfg, None, 1_672_000, PORT).unwrap();
        assert_eq!(s.credit, Credit::new(true, 100 * Q));
        assert_eq!(
            1_672_000 + ceil(s.credit.magnitude, cfg.idle_bps as u128),
            2_072_000
        );
        let mut cfg = cfg;
        cfg.idle_bps = 3;
        s.sending = false;
        s.backlog = true;
        s.credit = Credit::new(true, 1);
        s.last_ps = 0;
        assert_eq!(ceil(1, 3), 1);
        s.advance(&cfg, None, 1, PORT).unwrap();
        assert_eq!(s.credit, Credit::new(false, 2));
        s.credit = Credit::new(true, 1);
        s.last_ps = 0;
        s.backlog = false;
        s.advance(&cfg, None, 1, PORT).unwrap();
        assert_eq!(s.credit, Credit::default());
        assert_eq!(
            add_credit(Credit::new(false, u128::MAX - 1), u128::MAX, u128::MAX),
            Credit::new(false, u128::MAX)
        );
        assert_eq!(
            subtract_credit(Credit::new(true, u128::MAX - 1), u128::MAX, u128::MAX),
            Credit::new(true, u128::MAX)
        );
        cfg.hi = 0;
        cfg.lo = 0;
        s.credit = Credit::default();
        s.backlog = true;
        s.advance(&cfg, None, u64::MAX, PORT).unwrap();
        assert_eq!(s.credit, Credit::default());
    }
    #[test]
    fn lower_ready_class_can_send_and_fifo_head_never_bypassed() {
        let mut v = config();
        v["tsn"]["outputs"][0]["tas"] = gate(0, 1_000_000, &[(600000, true), (400000, false)]);
        let mut state = TsnState::new(&prep(v));
        let mut input = queued(7, 672000);
        input.heads[0] = Some(Head { occupancy_ps: 1 });
        input.backlog[0] = true;
        // class0 is permanently closed, so no selection; no hidden smaller class7 head exists in API.
        let d = state.plan_port(PORT, 0, &input).unwrap();
        assert!(d.selected.is_empty());
        let mut v = config();
        v["tsn"]["outputs"][0]["cbs"] = cbs_config()["tsn"]["outputs"][0]["cbs"].clone();
        state = TsnState::new(&prep(v));
        state.ports[0].cbs[7].as_mut().unwrap().credit = Credit::new(true, Q);
        let d = state.plan_port(PORT, 0, &input).unwrap();
        assert_eq!(d.selected[PORT], 0);
    }
    #[test]
    fn overflow_plans_are_atomic_future_wake_u128_and_no_busy_loop() {
        let mut state = TsnState::new(&prep(cbs_config()));
        state.ports[0].wake_generation = u64::MAX;
        let err = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap_err();
        assert_eq!(err.code, "E-0004");
        assert_eq!(state.ports[0].wake_generation, u64::MAX);
        state.ports[0].wake_generation = 0;
        assert_eq!(
            state
                .plan_start(PORT, 7, u64::MAX, 64, &PortInput::default())
                .unwrap_err()
                .code,
            "E-0004"
        );
        assert_eq!(state.ports[0].last_ps, 0);
        let mut cfg = cbs_config();
        cfg["tsn"]["outputs"][0]["cbs"][0]["idle_slope_bps"] = json!("1");
        let mut state = TsnState::new(&prep(cfg));
        let p = &mut state.ports[0];
        p.last_ps = u64::MAX - 1;
        let s = p.cbs[7].as_mut().unwrap();
        s.credit = Credit::new(true, 100 * Q);
        s.last_ps = u64::MAX - 1;
        s.backlog = true;
        let d = state
            .plan_port(PORT, u64::MAX - 1, &queued(7, 672000))
            .unwrap();
        assert!(d.wakes.is_empty());
        assert_eq!(d.ports[0].1.next_wake, Some(u64::MAX as u128 - 1 + 100 * Q));
        let mut state = TsnState::new(&prep(config()));
        let mut input = queued(0, 672000);
        input.sending = Some(Sending {
            priority: 7,
            release_ps: 1000000,
        });
        let d = state.plan_port(PORT, 0, &input).unwrap();
        assert_eq!(d.wakes.len(), 1);
        assert_eq!(d.wakes[0].time_ps, 1000000);
        state.apply(d);
        assert!(
            state
                .plan_start(PORT, 0, 1, 64, &PortInput::default())
                .is_err()
        );
    }
    #[test]
    fn psfp_five_arrival_independent_vector_and_yellow_drop() {
        let mut v = config();
        v["tsn"]["streams"] = json!([stream()]);
        let mut state = TsnState::new(&prep(v.clone()));
        for (t, color, cb, pb, ca, pa, bits, verdict) in [
            (0, "green", 64, 128, 0, 64, 1024, "pass"),
            (0, "yellow", 0, 64, 0, 0, 512, "pass"),
            (0, "red", 0, 0, 0, 0, 0, "drop"),
            (32000000, "yellow", 32, 64, 32, 0, 512, "pass"),
            (96000000, "green", 64, 128, 0, 64, 1024, "pass"),
        ] {
            let r = policing(&mut state, t, 64);
            assert_eq!(r.color.as_deref(), Some(color));
            assert_eq!(r.verdict, verdict);
            assert_eq!(r.consumed_bits, bits);
            assert_eq!(
                (
                    r.committed_before,
                    r.peak_before,
                    r.committed_after,
                    r.peak_after
                ),
                (
                    Some(cb * 8 * Q),
                    Some(pb * 8 * Q),
                    Some(ca * 8 * Q),
                    Some(pa * 8 * Q)
                )
            );
        }
        v["tsn"]["streams"][0]["meter"]["yellow_action"] = json!("drop");
        let mut state = TsnState::new(&prep(v));
        policing(&mut state, 0, 64);
        let r = policing(&mut state, 0, 64);
        assert_eq!(r.verdict, "drop");
        assert_eq!(r.peak_after, Some(0));
        assert_eq!(r.consumed_bits, 512);
    }
    #[test]
    fn psfp_sdu_and_gate_drops_preserve_meter_last_time_and_bypass_tuple() {
        let mut v = config();
        let mut s = stream();
        s["gate"] = json!({"base_time_ps":"0","cycle_time_ps":"2000000","entries":[{"duration_ps":"1000000","open":true},{"duration_ps":"1000000","open":false}]});
        v["tsn"]["streams"] = json!([s]);
        let mut state = TsnState::new(&prep(v));
        let r = policing(&mut state, 0, 64);
        assert_eq!(r.verdict, "pass");
        let r = policing(&mut state, 999999, 68);
        assert_eq!(r.reason, "psfp_max_sdu");
        assert_eq!(r.color, None);
        let r = policing(&mut state, 1000000, 64);
        assert_eq!(r.reason, "psfp_gate_closed");
        assert_eq!(state.meters[0].as_ref().unwrap().last_evaluated_ps, 0);
        let r = policing(&mut state, 32000000, 64);
        assert_eq!(r.committed_before, Some(32 * 8 * Q));
        for (ingress, mac, vid, priority) in [
            ("other.rx", "01:00:5e:00:00:01", 1, 7),
            ("N.b.rx", "01:00:5e:00:00:02", 1, 7),
            ("N.b.rx", "01:00:5e:00:00:01", 2, 7),
            ("N.b.rx", "01:00:5e:00:00:01", 1, 6),
        ] {
            let d = state
                .plan_arrival(ingress, mac, vid, priority, 64, "rx", 32000000)
                .unwrap();
            assert_eq!(d.psfp.unwrap().reason, "bypass");
            assert!(d.meters.is_empty());
        }
    }
    #[test]
    fn psfp_red_refill_saturation_fraction_no_refund_and_discarded_delta() {
        let mut v = config();
        let mut s = stream();
        s["max_sdu_bytes"] = json!("1000");
        v["tsn"]["streams"] = json!([s]);
        let mut state = TsnState::new(&prep(v.clone()));
        let r = policing(&mut state, 0, 129);
        assert_eq!(r.color.as_deref(), Some("red"));
        assert_eq!(r.consumed_bits, 0);
        policing(&mut state, 0, 64);
        policing(&mut state, 0, 64);
        let r = policing(&mut state, 1, 64);
        assert_eq!(r.color.as_deref(), Some("red"));
        assert_eq!(r.peak_after, Some(16000000));
        assert_eq!(state.meters[0].as_ref().unwrap().last_evaluated_ps, 1);
        let r = policing(&mut state, u64::MAX, 64);
        assert_eq!(r.committed_before, Some(64 * 8 * Q));
        assert_eq!(r.peak_before, Some(128 * 8 * Q));
        assert!(
            state
                .plan_arrival("N.b.rx", "01:00:5e:00:00:01", 1, 7, 64, "rx", 0)
                .is_err()
        );
        v["tsn"]["streams"][0]["meter"]["committed_rate_bps"] = json!("1");
        v["tsn"]["streams"][0]["meter"]["peak_rate_bps"] = json!("1");
        let mut state = TsnState::new(&prep(v));
        policing(&mut state, 0, 64);
        let planned = state
            .plan_arrival("N.b.rx", "01:00:5e:00:00:01", 1, 7, 64, "rx2", 1)
            .unwrap();
        assert_eq!(planned.psfp.as_ref().unwrap().committed_before, Some(1));
        // Failed parent reservation discards delta. Downstream successful queue_full applies it.
        assert_eq!(state.meters[0].as_ref().unwrap().committed, 0);
        drop(planned);
        let d = state
            .plan_arrival("N.b.rx", "01:00:5e:00:00:01", 1, 7, 64, "rx2", 1)
            .unwrap();
        state.apply(d);
        assert_eq!(state.meters[0].as_ref().unwrap().peak, 1);
    }
    #[test]
    fn partial_snapshot_keeps_unexecuted_boundary_and_records_decimal() {
        let mut state = TsnState::new(&prep(cbs_config()));
        let (d, _, _) = state
            .plan_start(PORT, 7, 0, 64, &queued(7, 672000))
            .unwrap();
        state.apply(d);
        for (h, bits) in [(576000, 432), (672000, 504)] {
            let s = state.snapshot(h).unwrap();
            assert_eq!(s.ports[0].credits[0].credit, Credit::new(true, bits * Q));
            assert!(s.ports[0].credits[0].sending);
        }
        let d = state.plan_port(PORT, 672000, &queued(7, 672000)).unwrap();
        let r = d
            .records
            .iter()
            .find(|r| matches!(r, TsnRecord::Credit(_)))
            .unwrap()
            .data(u64::MAX);
        assert_eq!(r["effect_seq"], json!(u64::MAX.to_string()));
        assert_eq!(r["magnitude"], json!((504 * Q).to_string()));
        state.apply(d);
        let s = state.snapshot(2688000).unwrap();
        assert_eq!(s.ports[0].next_wake_ps, Some(2688000));
        assert_eq!(s.ports[0].credits[0].credit, Credit::default());
        assert!(!s.ports[0].credits[0].sending);
    }

    #[test]
    fn composed_control_selection_sof_commits_once_and_stale_wake_stays_empty() {
        let mut state = TsnState::new(&prep(updated(cbs_config(), 500000, 1000000)));
        let input = queued(7, 672000);
        let d = state.plan_port(PORT, 0, &input).unwrap();
        let old = d.wakes[0].clone();
        state.apply(d);
        let d = state
            .plan_control(500000, &BTreeMap::from([(PORT.into(), input)]))
            .unwrap();
        assert!(!state.wake_valid_in(&d, &old));
        let n = d.records.len();
        let d = state.plan_wake_in(d, &old, &PortInput::default()).unwrap();
        assert_eq!(d.records.len(), n);
        let (d, eof, release) = state
            .plan_start_in(d, PORT, 7, 500000, 64, &PortInput::default())
            .unwrap();
        assert_eq!((eof, release), (1076000, 1172000));
        assert_eq!(d.ports.len(), 1);
        assert_eq!(d.wakes.len(), 1);
        assert_eq!(d.wakes[0].time_ps, release);
        assert_eq!(state.update_cursor, 0);
        assert_eq!(state.ports[0].schedule_generation, 0);
        let token = d.wakes[0].clone();
        state.apply(d);
        assert_eq!(state.update_cursor, 1);
        assert_eq!(state.ports[0].schedule_generation, 1);
        assert!(state.wake_valid(&token));
        let mut state = TsnState::new(&prep(config()));
        let d = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap();
        let (d, _, _) = state
            .plan_start_in(d, PORT, 7, 0, 64, &PortInput::default())
            .unwrap();
        assert_eq!(d.ports.len(), 1);
        assert!(d.selected.is_empty());
        state.apply(d);
    }
    #[test]
    fn composed_arrivals_share_staged_meter_but_never_mutate_before_apply() {
        let mut v = config();
        v["tsn"]["streams"] = json!([stream()]);
        let mut state = TsnState::new(&prep(v));
        let mut d = TsnDelta::default();
        for (id, color) in [("a", "green"), ("b", "yellow"), ("c", "red")] {
            d = state
                .plan_arrival_in(d, "N.b.rx", "01:00:5e:00:00:01", 1, 7, 64, id, 0)
                .unwrap();
            assert_eq!(d.psfp.as_ref().unwrap().color.as_deref(), Some(color));
        }
        assert_eq!(d.meters.len(), 1);
        assert_eq!(state.meters[0].as_ref().unwrap().committed, 512 * Q);
        state.apply(d);
        assert_eq!(state.meters[0].as_ref().unwrap().peak, 0);
    }
    #[test]
    fn every_independent_design_vector_runs_against_production_planners() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/ethernet_tsn/design-vectors.json"
        ))
        .unwrap();
        let decimal = |v: &Value| v.as_str().unwrap().parse::<u64>().unwrap();
        let mut tested = Vec::new();
        for row in fixture["vectors"].as_array().unwrap() {
            let input = &row["input"];
            let expected = &row["expected"];
            match row["id"].as_str().unwrap() {
                "ethernet.tsn.tas.exact-fit-and-circular-window" => {
                    let g = tas(
                        decimal(&input["tas_base_time_ps"]),
                        decimal(&input["tas_cycle_ps"]),
                        &[(1000000, true), (1000000, false)],
                    );
                    for (name, offer, bytes) in [
                        ("untagged_exact", "untagged_exact", 64),
                        ("untagged_plus_1_ps", "untagged_plus_1_ps", 64),
                        ("tagged_exact", "tagged_exact", 68),
                        ("tagged_plus_1_ps", "tagged_plus_1_ps", 68),
                    ] {
                        let t = decimal(&input["offer_ps"][offer]);
                        let (_, occ) =
                            wire_times(bytes, input["link_rate_bps"].as_u64().unwrap()).unwrap();
                        let sof = g.next_fit(7, t, occ, None).unwrap();
                        let exp = &expected[name];
                        assert_eq!(
                            sof,
                            decimal(if exp["sof_ps"].is_string() {
                                &exp["sof_ps"]
                            } else {
                                &exp["next_sof_ps"]
                            }) as u128
                        );
                        assert_eq!(
                            sof + occ as u128,
                            decimal(if exp["release_ps"].is_string() {
                                &exp["release_ps"]
                            } else {
                                &exp["next_release_ps"]
                            }) as u128
                        );
                    }
                    let circ = tas(
                        0,
                        1000000,
                        &[(400000, true), (300000, false), (300000, true)],
                    );
                    let sof = circ
                        .next_fit(
                            7,
                            decimal(&input["circular_case"]["offer_ps"]),
                            672000,
                            None,
                        )
                        .unwrap();
                    assert_eq!(
                        sof + 672000,
                        decimal(&expected["circular_release_ps"]) as u128
                    );
                }
                "ethernet.tsn.cbs.recovery-and-gate-freeze" => {
                    let mut state = TsnState::new(&prep(cbs_config()));
                    let (d, _, _) = state
                        .plan_start(PORT, 7, 0, 64, &queued(7, 672000))
                        .unwrap();
                    state.apply(d);
                    for (idx, e) in expected["continuous_open"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .enumerate()
                    {
                        let t = decimal(&e["time_ps"]);
                        assert_eq!(
                            amount(&state, t),
                            Credit::new(
                                e["credit_negative"].as_bool().unwrap(),
                                e["credit_magnitude_u128"]
                                    .as_str()
                                    .unwrap()
                                    .parse()
                                    .unwrap()
                            )
                        );
                        if idx == 1 {
                            let d = state.plan_port(PORT, t, &queued(7, 672000)).unwrap();
                            state.apply(d);
                        }
                        if idx == 2 {
                            let (d, _, _) = state
                                .plan_start(PORT, 7, t, 64, &PortInput::default())
                                .unwrap();
                            state.apply(d);
                        }
                        if idx == 3 {
                            let d = state.plan_port(PORT, t, &PortInput::default()).unwrap();
                            state.apply(d);
                        }
                    }
                    let mut v = cbs_config();
                    v["tsn"]["outputs"][0]["tas"] = gate(
                        0,
                        5000000,
                        &[(1000000, true), (1000000, false), (3000000, true)],
                    );
                    let mut state = TsnState::new(&prep(v));
                    let (d, _, _) = state
                        .plan_start(PORT, 7, 0, 64, &queued(7, 672000))
                        .unwrap();
                    state.apply(d);
                    let d = state.plan_port(PORT, 672000, &queued(7, 672000)).unwrap();
                    state.apply(d);
                    let frozen = &expected["gate_freeze"];
                    assert_eq!(
                        amount(&state, decimal(&frozen["credit_at_close_ps"])).magnitude,
                        frozen["credit_at_close_magnitude_u128"]
                            .as_str()
                            .unwrap()
                            .parse()
                            .unwrap()
                    );
                    let d = state.plan_port(PORT, 2000000, &queued(7, 672000)).unwrap();
                    assert_eq!(d.wakes[0].time_ps, decimal(&frozen["zero_wake_ps"]));
                }
                "ethernet.tsn.psfp.five-arrivals" => {
                    let mut v = config();
                    let mut s = stream();
                    for field in [
                        "committed_rate_bps",
                        "peak_rate_bps",
                        "committed_burst_bytes",
                        "peak_burst_bytes",
                    ] {
                        s["meter"][field] = json!(input[field].as_u64().unwrap().to_string());
                    }
                    v["tsn"]["streams"] = json!([s]);
                    let mut state = TsnState::new(&prep(v));
                    for (a, e) in input["arrivals"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .zip(expected.as_array().unwrap())
                    {
                        let r = policing(
                            &mut state,
                            decimal(&a["time_ps"]),
                            input["mac_frame_bytes"].as_u64().unwrap(),
                        );
                        assert_eq!(r.color.as_deref(), e["color"].as_str());
                        assert_eq!(r.verdict, e["verdict"].as_str().unwrap());
                        assert_eq!(
                            r.consumed_bits,
                            e["consumed_bits"].as_u64().unwrap() as u128
                        );
                        for (actual, field) in [
                            (r.committed_before, "refilled_committed_u128"),
                            (r.peak_before, "refilled_peak_u128"),
                            (r.committed_after, "after_committed_u128"),
                            (r.peak_after, "after_peak_u128"),
                        ] {
                            assert_eq!(actual.unwrap().to_string(), e[field].as_str().unwrap());
                        }
                    }
                }
                other => panic!("unexecuted independent TSN vector: {other}"),
            }
            tested.push(row["id"].as_str().unwrap());
        }
        assert_eq!(tested.len(), 3);
    }

    #[test]
    fn staged_return_to_committed_mode_emits_correcting_credit_observation() {
        let state = TsnState::new(&prep(cbs_config()));
        let d = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap();
        let d = state
            .plan_port_in(d, PORT, 0, &PortInput::default())
            .unwrap();
        let last = d.records.iter().rev().find_map(|r| {
            if let TsnRecord::Credit(r) = r {
                Some(r.mode)
            } else {
                None
            }
        });
        assert_eq!(last, Some(CreditMode::Zero));
        let mut v = cbs_config();
        v["tsn"]["outputs"][0]["cbs"][0]["hi_credit_bits"] = json!("0");
        v["tsn"]["outputs"][0]["cbs"][0]["lo_credit_bits"] = json!("0");
        let mut state = TsnState::new(&prep(v));
        let (d, _, _) = state
            .plan_start(PORT, 7, 0, 64, &queued(7, 672000))
            .unwrap();
        state.apply(d);
        let d = state.plan_port(PORT, 672000, &queued(7, 672000)).unwrap();
        let (d, _, _) = state
            .plan_start_in(d, PORT, 7, 672000, 64, &PortInput::default())
            .unwrap();
        let last = d.records.iter().rev().find_map(|r| {
            if let TsnRecord::Credit(r) = r {
                Some(r.mode)
            } else {
                None
            }
        });
        assert_eq!(last, Some(CreditMode::Sending));
    }

    #[test]
    fn schedule_generation_overflow_and_empty_control_noop_are_atomic() {
        let mut state = TsnState::new(&prep(updated(config(), 10, 1000000)));
        state.ports[0].schedule_generation = u64::MAX;
        let e = state
            .plan_control(10, &BTreeMap::from([(PORT.into(), queued(7, 672000))]))
            .unwrap_err();
        assert_eq!(e.code, "E-0004");
        assert_eq!(state.update_cursor, 0);
        assert!(state.ports[0].oper.is_none());
        let d = state.plan_control(0, &BTreeMap::new()).unwrap();
        assert!(d.records.is_empty());
        assert!(d.ports.is_empty());
        assert!(d.wakes.is_empty());
        assert!(
            state
                .plan_control(11, &BTreeMap::from([(PORT.into(), queued(7, 672000))]))
                .is_err()
        );
        let mut v = config();
        v["tsn"]["outputs"][0]["tas"] = gate(0, 1000000, &[(1000000, false)]);
        let state = TsnState::new(&prep(updated(v, 500000, 1000000)));
        let d = state.plan_port(PORT, 0, &queued(7, 672000)).unwrap();
        assert_eq!(d.wakes.len(), 1);
        assert_eq!(d.wakes[0].time_ps, 500000);
    }
    #[test]
    fn metadata_roundtrips_and_snapshot_json_stays_decimal() {
        let prepared = prep(updated(cbs_config(), 2000000, 800000));
        let metadata = prepared.metadata();
        let mut v = config();
        for key in ["clock", "outputs", "streams", "gcl_updates"] {
            v["tsn"][key] = metadata[key].clone();
        }
        assert_eq!(prep(v).metadata(), metadata);
        let state = TsnState::new(&prepared);
        let data = state.snapshot(0).unwrap().data();
        assert_eq!(data["ports"][0]["credits"][0]["magnitude"], json!("0"));
        assert_eq!(data["ports"][0]["schedule_generation"], json!("0"));
        assert_eq!(data["update_cursor"], json!("0"));
        assert_eq!(SCHEMAS.len(), 4);
    }
}
