//! Additive MAC Merge signal: physical occupation and CBS transmission differ.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Segment {
    OriginalMac,
    AddedMcrc,
    AddedContinuationHeader,
    AddedInterruptionIpg,
    FinalOriginalIpg,
    SampledPhysicalTail,
    Control,
    Idle,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MergeShaperView {
    pub medium_busy_until: Option<u64>,
    pub transmit_class: Option<u8>,
    pub next_boundary: Option<u64>,
    pub segment: Segment,
}
/// A read-only view of the existing MAC owner. Unknown release is busy=true
/// with release_ps=None; it is never represented by a fabricated deadline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MacServiceView {
    /// Stable identity supplied by the owner, for causal replay validation.
    pub active_work: Option<String>,
    pub retained_backlog: [bool; 8],
    pub transmit_class: Option<u8>,
    pub busy: bool,
    pub release_ps: Option<u64>,
    pub next_transition_ps: Option<u64>,
    pub segment: Segment,
    pub accounting: ServiceAccounting,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ServiceAccounting {
    /// Preserve the shipped ordinary/abstract configured credit bounds.
    LegacyCapped,
    /// Actual original-frame transmission; additional overhead is nonsending.
    Causal,
    /// Explicit project accounting of sampled wire occupation, including PCS.
    SampledWire,
}
/// Projected operation only: the MAC owner retains HOLD latch, cursor and
/// reservation lifecycle. Advances are physical durations, never local ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateControlProjection {
    pub at: u64,
    pub target_global_ps: u64,
    pub local_target: Option<crate::runtime::network::clock::LocalTime>,
    pub hold: bool,
    pub schedule_generation: u64,
    pub clock_generation: Option<u64>,
}
impl TsnState {
    /// Read-only forward reconstruction. Never invert credit at a later cut.
    pub(crate) fn credit_nonnegative_forward_clock_in(
        &self,
        d: &TsnDelta,
        port: &str,
        at: u64,
        clock: Option<&NodeClock>,
    ) -> Result<Option<u8>> {
        let i = self.index(port)?;
        let mut p = d
            .ports
            .iter()
            .find(|(j, _)| *j == i)
            .map_or(&self.ports[i], |(_, p)| p)
            .clone();
        if p.last_ps > at || p.cbs.iter().flatten().any(|s| s.last_ps > at) {
            return Ok(None);
        }
        // A new clock anchor cannot be extrapolated backward to prove a cut.
        if clock.is_some_and(|c| c.local_at(p.last_ps).is_err() || c.local_at(at).is_err()) {
            return Ok(None);
        }
        self.advance_port(i, &mut p, at, clock)?;
        Ok(Some((0..8).fold(0, |mask, c| {
            if p.cbs[c].as_ref().is_none_or(|s| !s.credit.negative) {
                mask | (1 << c)
            } else {
                mask
            }
        })))
    }
    /// Source-derived last start cut for a retained PHY claim. The physical
    /// owner keeps carrier/backoff; this permission never interrupts a frame.
    pub(crate) fn retained_start_permission_clock_in(
        &self,
        d: &TsnDelta,
        port: &str,
        priority: u8,
        now: u64,
        occupancy: u64,
        clock: Option<&NodeClock>,
    ) -> Result<Option<u64>> {
        let i = self.index(port)?;
        if priority >= 8 || occupancy == 0 {
            return Err(error(port, "invalid retained PHY class or occupancy"));
        }
        let p = d
            .ports
            .iter()
            .find(|(j, _)| *j == i)
            .map_or(&self.ports[i], |(_, p)| p);
        if p.last_ps != now {
            return Err(error(
                port,
                "PHY permission requires current credit settlement",
            ));
        }
        if p.cbs[priority as usize]
            .as_ref()
            .is_some_and(|s| s.credit.negative)
        {
            return Ok(None);
        }
        let clip = self.next_update(port, d.update_cursor.unwrap_or(self.update_cursor));
        let mut end = clip.map(u128::from).unwrap_or(u128::from(u64::MAX));
        if let Some(schedule) = p.oper.as_ref() {
            if clock_projection::fit(schedule, priority, now, occupancy, clip, clock)?
                != Some(u128::from(now))
            {
                return Ok(None);
            }
            if let Some(boundary) = clock_projection::boundary(schedule, priority, now, clock)? {
                end = end.min(boundary);
            }
        }
        let Some(last) = end.checked_sub(u128::from(occupancy)) else {
            return Ok(None);
        };
        Ok(u64::try_from(last).ok().filter(|last| *last >= now))
    }
    /// A continuation remains the same original CBS frame. Recheck physical
    /// gate fit without applying the fresh-frame nonnegative-credit rule.
    pub(crate) fn retained_gate_fit_clock_in(
        &self,
        d: &TsnDelta,
        port: &str,
        priority: u8,
        now: u64,
        occupancy: u64,
        clock: Option<&NodeClock>,
    ) -> Result<bool> {
        let i = self.index(port)?;
        if priority >= 8 || occupancy == 0 {
            return Err(error(port, "invalid continuation class or occupancy"));
        }
        let p = d
            .ports
            .iter()
            .find(|(j, _)| *j == i)
            .map_or(&self.ports[i], |(_, p)| p);
        let clip = self.next_update(port, d.update_cursor.unwrap_or(self.update_cursor));
        if let Some(schedule) = &p.oper {
            Ok(
                clock_projection::fit(schedule, priority, now, occupancy, clip, clock)?
                    == Some(u128::from(now)),
            )
        } else {
            Ok(clip.is_none_or(|end| u128::from(now) + u128::from(occupancy) <= u128::from(end)))
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn next_gate_operation_clock_in(
        &self,
        d: &TsnDelta,
        port: &str,
        now: u64,
        hold_advance_ps: u64,
        release_advance_ps: u64,
        clock: Option<&NodeClock>,
    ) -> Result<Option<GateControlProjection>> {
        use crate::runtime::network::clock::{ClockError, LocalTime};
        let (schedule_generation, schedule) = self.merge_schedule(d, port)?;
        let Some(s) = schedule else { return Ok(None) };
        let mut best: Option<GateControlProjection> = None;
        for (j, entry) in s.entries.iter().enumerate() {
            let (hold, advance) = match entry.operation {
                GateOperation::SetGateStates => continue,
                GateOperation::SetAndHoldMac => (true, hold_advance_ps),
                GateOperation::SetAndReleaseMac => (false, release_advance_ps),
            };
            // Existing contract selects the first advance event STRICTLY after now.
            let Some(first) = now.checked_add(advance).and_then(|t| t.checked_add(1)) else {
                continue;
            };
            let local_first = clock.map_or(Ok(i128::from(first)), |clock| {
                clock
                    .local_at(first - 1)
                    .and_then(|t| t.floor_ps())
                    .and_then(|t| t.checked_add(1).ok_or(ClockError::TimeOverflow))
                    .map_err(|e| error(port, e.to_string()))
            })?;
            let base = u128::from(s.base_ps) + u128::from(s.prefix_ps[j]);
            let k = u128::try_from(local_first)
                .ok()
                .filter(|t| *t > base)
                .map_or(0, |t| ceil(t - base, u128::from(s.cycle_ps)));
            let target = k
                .checked_mul(u128::from(s.cycle_ps))
                .and_then(|t| t.checked_add(base))
                .ok_or_else(|| error(port, "gate operation target overflow"))?;
            let (global, local_target) = if let Some(clock) = clock {
                let Ok(target) = i128::try_from(target) else {
                    continue;
                };
                let local = LocalTime::integer(target);
                let wake = match clock.deadline(now, local.clone()) {
                    Ok(wake) => wake,
                    Err(ClockError::TimeOverflow) => continue,
                    Err(e) => return Err(error(port, e.to_string())),
                };
                (wake.global_ps, Some(local))
            } else {
                let Ok(global) = u64::try_from(target) else {
                    continue;
                };
                (global, None)
            };
            let Some(at) = global.checked_sub(advance).filter(|at| *at > now) else {
                continue;
            };
            let candidate = GateControlProjection {
                at,
                target_global_ps: global,
                local_target,
                hold,
                schedule_generation,
                clock_generation: clock.map(NodeClock::generation),
            };
            if best.as_ref().is_none_or(|old| candidate.at < old.at) {
                best = Some(candidate);
            }
        }
        Ok(best)
    }
}
impl CbsState {
    pub(super) fn advance_merge(
        &mut self,
        cfg: &CbsConfig,
        schedule: Option<&Schedule>,
        now: u64,
        target: &str,
        clock: Option<&NodeClock>,
    ) -> Result<()> {
        let dt = now
            .checked_sub(self.last_ps)
            .ok_or_else(|| error(target, "credit time reversal"))?;
        let elapsed = if self.sending {
            dt
        } else {
            schedule.map_or(Ok(dt), |s| {
                clock_projection::open_elapsed(s, cfg.priority, self.last_ps, now, clock)
            })?
        };
        let negative = self.sending;
        let rate = if negative {
            cfg.link_bps - cfg.idle_bps
        } else {
            cfg.idle_bps
        };
        let amount = u128::from(rate)
            .checked_mul(u128::from(elapsed))
            .ok_or_else(|| error(target, "Merge credit product overflow"))?;
        if negative == self.credit.negative {
            self.credit = Credit::new(
                negative,
                self.credit
                    .magnitude
                    .checked_add(amount)
                    .ok_or_else(|| error(target, "Merge credit overflow"))?,
            );
        } else if amount > self.credit.magnitude {
            self.credit = Credit::new(negative, amount - self.credit.magnitude);
        } else {
            self.credit = Credit::new(self.credit.negative, self.credit.magnitude - amount);
        }
        // Credit is reset only while the gate is open, the queue is empty and
        // there is no retained selected frame. The caller includes retained backlog.
        let open = schedule.map_or(Ok(true), |s| {
            clock_projection::open(s, cfg.priority, now, clock)
        })?;
        if !self.sending && !self.backlog && !self.credit.negative && (open || elapsed > 0) {
            self.credit = Credit::default();
        }
        self.last_ps = now;
        Ok(())
    }
    pub(super) fn derive_merge(&mut self, open: bool) {
        if !self.sending && !self.backlog && !self.credit.negative && open {
            self.credit = Credit::default();
        }
        self.mode = if self.sending {
            CreditMode::Sending
        } else if !open {
            CreditMode::Frozen
        } else if !self.backlog && !self.credit.negative {
            CreditMode::Zero
        } else if self.backlog {
            CreditMode::Accumulating
        } else {
            CreditMode::Recovering
        };
    }
}
impl TsnState {
    pub(crate) fn plan_merge_transition_clock_in(
        &self,
        d: TsnDelta,
        port: &str,
        now: u64,
        input: &PortInput,
        clock: Option<&NodeClock>,
    ) -> Result<TsnDelta> {
        if input.merge.is_none() || input.sending.is_some() {
            return Err(error(port, "missing explicit Merge shaper signal"));
        }
        self.plan_port_clock_in(d, port, now, input, clock)
    }
    pub(crate) fn validate_merge_view(&self, port: &str, input: &PortInput) -> Result<()> {
        let i = self.index(port)?;
        let p = &self.ports[i];
        if p.merge != input.merge {
            return Err(error(
                port,
                "Merge shaper signal differs from actual packet segment",
            ));
        }
        for c in 0..8 {
            if let Some(s) = &p.cbs[c] {
                if s.backlog != input.backlog[c]
                    || s.sending
                        != input
                            .merge
                            .is_some_and(|v| v.transmit_class == Some(c as u8))
                {
                    return Err(error(
                        port,
                        "Merge credit mode differs from actual segment/backlog",
                    ));
                }
            }
        }
        Ok(())
    }
}
impl TsnState {
    pub(crate) fn merge_control_due(&self, now: u64) -> bool {
        self.prepared
            .updates
            .get(self.update_cursor)
            .is_some_and(|u| u.effective_at_ps == now)
    }
    pub(crate) fn merge_schedule(
        &self,
        d: &TsnDelta,
        port: &str,
    ) -> Result<(u64, Option<Arc<Schedule>>)> {
        let i = self.index(port)?;
        let p = d
            .ports
            .iter()
            .find(|(j, _)| *j == i)
            .map_or(&self.ports[i], |(_, p)| p);
        Ok((p.schedule_generation, p.oper.clone()))
    }
}

impl TsnState {
    pub(crate) fn merge_staged_time(&self, d: &TsnDelta, port: &str) -> Result<u64> {
        let i = self.index(port)?;
        Ok(d.ports
            .iter()
            .find(|(j, _)| *j == i)
            .map_or(&self.ports[i], |(_, p)| p)
            .last_ps)
    }
    pub(crate) fn validate_merge_replayed_port(&self, replayed: &Self, port: &str) -> Result<()> {
        let i = self.index(port)?;
        if serde_json::to_value(&self.ports[i]).map_err(|e| error(port, e.to_string()))?
            != serde_json::to_value(&replayed.ports[i]).map_err(|e| error(port, e.to_string()))?
        {
            let a = serde_json::to_value(&self.ports[i]).unwrap();
            let b = serde_json::to_value(&replayed.ports[i]).unwrap();
            let keys = a
                .as_object()
                .unwrap()
                .keys()
                .filter(|k| a[*k] != b[*k])
                .map(|k| format!("{k}: saved={} replay={}", a[k], b[k]))
                .collect::<Vec<_>>();
            return Err(error(
                port,
                format!(
                    "historical MAC Merge credit/residual/time/mode/wakeup replay differs: {}",
                    keys.join("; ")
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod fractional_tests {
    use super::*;
    #[test]
    fn forward_floor_credit_handles_zero_caps_slopes_crossings_and_no_backwards() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/mac-merge/model.ini"),
        )
        .unwrap();
        let mut cfg = p
            .registered
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .tsn
            .clone()
            .unwrap();
        let name = cfg.outputs[0].port.clone();
        cfg.outputs[0].cbs[0] = Some(CbsConfig {
            priority: 0,
            idle_bps: 250_000_000,
            link_bps: 1_000_000_000,
            hi: 0,
            lo: 1_000_000_000_000,
        });
        for causal in [false, true] {
            let mut tsn = TsnState::new(&cfg);
            let state = &mut tsn.ports[0];
            state.cbs[0].as_mut().unwrap().backlog = true;
            if causal {
                state.merge = Some(MergeShaperView {
                    medium_busy_until: None,
                    transmit_class: None,
                    next_boundary: None,
                    segment: Segment::Idle,
                });
            }
            assert_eq!(
                tsn.credit_nonnegative_forward_clock_in(&TsnDelta::default(), &name, 1, None)
                    .unwrap()
                    .unwrap()
                    & 1,
                1
            );
            tsn.ports[0].cbs[0].as_mut().unwrap().credit = Credit::new(true, 250_000_001);
            let before = serde_json::to_value(&tsn.ports[0]).unwrap();
            assert_eq!(
                tsn.credit_nonnegative_forward_clock_in(&TsnDelta::default(), &name, 1, None)
                    .unwrap()
                    .unwrap()
                    & 1,
                0
            );
            assert_eq!(serde_json::to_value(&tsn.ports[0]).unwrap(), before);
            tsn.ports[0].last_ps = 2;
            tsn.ports[0].cbs[0].as_mut().unwrap().last_ps = 2;
            assert_eq!(
                tsn.credit_nonnegative_forward_clock_in(&TsnDelta::default(), &name, 1, None)
                    .unwrap(),
                None
            );
        }
        cfg.outputs[0].cbs[0].as_mut().unwrap().idle_bps = 0;
        let tsn = TsnState::new(&cfg);
        assert_eq!(
            tsn.credit_nonnegative_forward_clock_in(&TsnDelta::default(), &name, 1, None)
                .unwrap()
                .unwrap()
                & 1,
            1
        );
    }
}
