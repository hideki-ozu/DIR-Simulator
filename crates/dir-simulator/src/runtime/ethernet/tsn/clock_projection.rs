//! Exact conversion of local TAS windows to physical scheduler deadlines.
use super::{Result, Schedule, error};
use crate::runtime::network::clock::{ClockWake, LocalTime, NodeClock};

fn local_floor(clock: &NodeClock, now: u64, target: &str) -> Result<i128> {
    clock
        .local_at(now)
        .and_then(|t| t.floor_ps())
        .map_err(|e| error(target, e.to_string()))
}
fn global_deadline(clock: &NodeClock, now: u64, local: u128, target: &str) -> Result<Option<u128>> {
    let Ok(local) = i128::try_from(local) else {
        return Ok(None);
    };
    match clock.deadline(now, LocalTime::integer(local)) {
        Ok(wake) => Ok(Some(u128::from(wake.global_ps))),
        Err(crate::runtime::network::clock::ClockError::TimeOverflow) => Ok(None),
        Err(e) => Err(error(target, e.to_string())),
    }
}
pub(super) fn before_base(s: &Schedule, now: u64, clock: Option<&NodeClock>) -> Result<bool> {
    match clock {
        Some(clock) => Ok(local_floor(clock, now, &s.id)? < i128::from(s.base_ps)),
        None => Ok(now < s.base_ps),
    }
}
pub(super) fn open(
    s: &Schedule,
    priority: u8,
    now: u64,
    clock: Option<&NodeClock>,
) -> Result<bool> {
    let Some(clock) = clock else {
        return Ok(s.open(priority, now));
    };
    if priority >= 8 {
        return Ok(false);
    }
    let local = local_floor(clock, now, &s.id)?;
    if local < i128::from(s.base_ps) {
        return Ok(false);
    }
    // Local time may exceed the physical scheduler's u64 range. Only the
    // periodic phase is bounded by cycle_ps and safe to narrow.
    let phase = ((local - i128::from(s.base_ps)) as u128 % u128::from(s.cycle_ps)) as u64;
    let entry = s.prefix_ps.partition_point(|p| *p <= phase) - 1;
    Ok(s.entries[entry].open_mask & (1 << priority) != 0)
}
pub(super) fn boundary(
    s: &Schedule,
    priority: u8,
    now: u64,
    clock: Option<&NodeClock>,
) -> Result<Option<u128>> {
    let Some(clock) = clock else {
        return Ok(s.next_boundary(priority, now));
    };
    let Some(target) = local_boundary(s, priority, now, clock)? else {
        return Ok(None);
    };
    global_deadline(clock, now, target.numerator() as u128, &s.id)
}
/// The exact LOCAL gate target, retained alongside its physical wake.
pub(super) fn local_boundary(
    s: &Schedule,
    priority: u8,
    now: u64,
    clock: &NodeClock,
) -> Result<Option<LocalTime>> {
    if priority >= 8 {
        return Ok(None);
    }
    let local = local_floor(clock, now, &s.id)?;
    if local < i128::from(s.base_ps) {
        return Ok(Some(LocalTime::integer(i128::from(s.base_ps))));
    }
    let total = s.open_total_ps[priority as usize];
    if total == 0 || total == s.cycle_ps {
        return Ok(None);
    }
    let cycle = u128::from(s.cycle_ps);
    let phase = (local - i128::from(s.base_ps)) as u128 % cycle;
    let mut next = None;
    for i in 0..s.entries.len() {
        let previous = if i == 0 { s.entries.len() - 1 } else { i - 1 };
        if (s.entries[previous].open_mask ^ s.entries[i].open_mask) & (1 << priority) == 0 {
            continue;
        }
        let offset = u128::from(s.prefix_ps[i]);
        let delta = if offset > phase {
            offset - phase
        } else {
            cycle - phase + offset
        };
        let candidate = local as u128 + delta;
        next = Some(next.map_or(candidate, |old: u128| old.min(candidate)));
    }
    Ok(next
        .and_then(|t| i128::try_from(t).ok())
        .map(LocalTime::integer))
}
pub(super) fn base_wake(s: &Schedule, now: u64, clock: Option<&NodeClock>) -> Result<Option<u128>> {
    if !before_base(s, now, clock)? {
        return Ok(None);
    }
    clock.map_or(Ok(Some(u128::from(s.base_ps))), |clock| {
        global_deadline(clock, now, u128::from(s.base_ps), &s.id)
    })
}
/// Integrate the discrete FES gate state over physical integer intervals.
/// Several local edges may project onto one physical tick: only the state at
/// that tick owns [tick, tick + 1), never the skipped local duration.
pub(super) fn open_elapsed(
    s: &Schedule,
    priority: u8,
    from: u64,
    to: u64,
    clock: Option<&NodeClock>,
) -> Result<u64> {
    if to < from {
        return Err(error(&s.id, "gate elapsed time reversal"));
    }
    let Some(clock) = clock else {
        return s.open_elapsed(priority, from, to);
    };
    if priority >= 8 || s.open_total_ps[priority as usize] == 0 {
        return Ok(0);
    }
    let start = if before_base(s, from, Some(clock))? {
        base_wake(s, from, Some(clock))?
            .and_then(|t| u64::try_from(t).ok())
            .unwrap_or(to)
            .min(to)
    } else {
        from
    };
    if start >= to {
        return Ok(0);
    }
    if s.open_total_ps[priority as usize] == s.cycle_ps {
        return Ok(to - start);
    }
    let period = physical_period(s, clock);
    let repeats = u128::from(to - start) / period;
    if repeats > 0 {
        // T = cycle*rate_den/gcd(cycle,rate_num) is an INTEGER physical
        // period and local(t+T)=local(t)+k*cycle exactly, for integer k.
        // Thus every sampled gate state (including fractional-edge rounding)
        // repeats. Count ONE exact boundary-iterated period, never a rounded
        // local cycle, and multiply only that proven physical repetition.
        let period_end =
            start + u64::try_from(period).map_err(|_| error(&s.id, "gate period overflow"))?;
        let area = iterate_open(s, priority, start, period_end, clock)?;
        let tail_start = start
            + u64::try_from(repeats * period).map_err(|_| error(&s.id, "gate repeat overflow"))?;
        let elapsed = u128::from(area) * repeats
            + u128::from(iterate_open(s, priority, tail_start, to, clock)?);
        return u64::try_from(elapsed).map_err(|_| error(&s.id, "gate elapsed overflow"));
    }
    iterate_open(s, priority, start, to, clock)
}
fn physical_period(s: &Schedule, clock: &NodeClock) -> u128 {
    let (num, den) = clock.rate();
    let mut a = u128::from(s.cycle_ps);
    let mut b = u128::from(num);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    (u128::from(s.cycle_ps) / a) * u128::from(den)
}
fn iterate_open(s: &Schedule, priority: u8, from: u64, to: u64, clock: &NodeClock) -> Result<u64> {
    let mut cursor = from;
    let mut elapsed = 0u64;
    while cursor < to {
        let next = boundary(s, priority, cursor, Some(clock))?
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(to)
            .min(to);
        if next <= cursor {
            return Err(error(&s.id, "nonadvancing projected gate boundary"));
        }
        if open(s, priority, cursor, Some(clock))? {
            elapsed += next - cursor;
        }
        cursor = next;
    }
    Ok(elapsed)
}

/// Canonical exact gate dependency for a selected global wake. Looking just
/// before that wake also covers a fit in a later opening, beyond the first edge.
pub(super) fn gate_wake(
    s: &Schedule,
    now: u64,
    wake: u64,
    clock: &NodeClock,
) -> Result<Option<ClockWake>> {
    for class in 0..8 {
        for cut in [now, wake.saturating_sub(1).max(now)] {
            if let Some(target) = local_boundary(s, class, cut, clock)? {
                if let Ok(projected) = clock.deadline(now, target) {
                    if projected.global_ps == wake {
                        return Ok(Some(projected));
                    }
                }
            }
        }
    }
    Ok(None)
}

pub(super) fn fit(
    s: &Schedule,
    priority: u8,
    now: u64,
    occupancy: u64,
    clip: Option<u64>,
    clock: Option<&NodeClock>,
) -> Result<Option<u128>> {
    let Some(clock) = clock else {
        return Ok(s.next_fit(priority, now, occupancy, clip));
    };
    if priority >= 8 || occupancy == 0 || s.open_total_ps[priority as usize] == 0 {
        return Ok(None);
    }
    let start = if before_base(s, now, Some(clock))? {
        let Some(start) = base_wake(s, now, Some(clock))? else {
            return Ok(None);
        };
        u64::try_from(start).map_err(|_| error(&s.id, "TAS base exceeds physical time"))?
    } else {
        now
    };
    let period = physical_period(s, clock);
    let limit = clip.unwrap_or(u64::MAX);
    let search_end = u128::from(start)
        .saturating_add(period.saturating_mul(2))
        .min(u128::from(limit));
    let mut cursor = start;
    let mut candidate = None;
    loop {
        if u128::from(cursor) >= search_end {
            return Ok(None);
        }
        if open(s, priority, cursor, Some(clock))? {
            candidate.get_or_insert(cursor);
        } else {
            candidate = None;
        }
        // If one full exact repetition contained no sampled closed interval,
        // the gate remains open forever on integer FES ticks.
        if candidate == Some(start) && u128::from(cursor) >= u128::from(start) + period {
            return Ok(start
                .checked_add(occupancy)
                .filter(|r| *r <= limit)
                .map(|_| u128::from(start)));
        }
        let next = boundary(s, priority, cursor, Some(clock))?
            .unwrap_or(u128::from(limit))
            .min(u128::from(limit));
        if let Some(candidate) = candidate {
            if let Some(release) = candidate.checked_add(occupancy) {
                if u128::from(release) <= next {
                    return Ok(Some(u128::from(candidate)));
                }
            }
        }
        if next <= u128::from(cursor) || next >= search_end {
            return Ok(None);
        }
        cursor = u64::try_from(next).map_err(|_| error(&s.id, "TAS boundary overflow"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ethernet::tsn::{GateEntry, OpenRun};

    fn periodic(base_ps: u64) -> Schedule {
        let mut runs: [Vec<OpenRun>; 8] = std::array::from_fn(|_| vec![]);
        runs[0].push(OpenRun {
            start_ps: 0,
            end_ps: 4,
        });
        let mut totals = [0; 8];
        totals[0] = 4;
        Schedule {
            id: "periodic".into(),
            base_ps,
            cycle_ps: 10,
            entries: vec![
                GateEntry {
                    operation: crate::types::ethernet::tsn::GateOperation::SetGateStates,
                    duration_ps: 4,
                    open_mask: 1,
                },
                GateEntry {
                    operation: crate::types::ethernet::tsn::GateOperation::SetGateStates,
                    duration_ps: 6,
                    open_mask: 0,
                },
            ],
            prefix_ps: vec![0, 4, 10],
            class_open_runs: runs,
            open_total_ps: totals,
        }
    }

    #[test]
    fn wide_local_time_keeps_phase_boundaries_and_physical_fit() {
        let schedule = periodic(7);
        let mut clock = NodeClock::new(0, 1i128 << 64, 1, 1).unwrap();
        assert!(!open(&schedule, 0, 0, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 0, 0, Some(&clock)).unwrap(), Some(1));
        assert!(open(&schedule, 0, 1, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 0, 1, Some(&clock)).unwrap(), Some(5));
        assert_eq!(
            fit(&schedule, 0, 0, 4, None, Some(&clock)).unwrap(),
            Some(1)
        );
        assert_eq!(fit(&schedule, 0, 0, 5, None, Some(&clock)).unwrap(), None);
        clock.step_by(0, -(1i128 << 64)).unwrap();
        assert!(!open(&schedule, 0, 0, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 0, 0, Some(&clock)).unwrap(), Some(7));

        let near_physical_limit = periodic(u64::MAX);
        let clock = NodeClock::new(0, 1i128 << 64, 1, 1).unwrap();
        assert!(open(&near_physical_limit, 0, 0, Some(&clock)).unwrap());
        assert_eq!(
            boundary(&near_physical_limit, 0, 0, Some(&clock)).unwrap(),
            Some(3)
        );
        assert!(!open(&schedule, 8, 0, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 8, 0, Some(&clock)).unwrap(), None);
    }

    #[test]
    fn slow_fractional_clock_changes_only_at_exact_local_boundaries() {
        let schedule = periodic(7);
        let clock = NodeClock::new(0, 0, 1, 3).unwrap();
        assert!(!open(&schedule, 0, 20, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 0, 20, Some(&clock)).unwrap(), Some(21));
        assert!(open(&schedule, 0, 32, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 0, 32, Some(&clock)).unwrap(), Some(33));
        assert!(!open(&schedule, 0, 33, Some(&clock)).unwrap());
        assert_eq!(boundary(&schedule, 0, 33, Some(&clock)).unwrap(), Some(51));
        assert_eq!(
            fit(&schedule, 0, 32, 1, None, Some(&clock)).unwrap(),
            Some(32)
        );
        assert_eq!(
            fit(&schedule, 0, 32, 2, None, Some(&clock)).unwrap(),
            Some(51)
        );
    }

    #[test]
    fn continuous_open_gate_does_not_end_at_a_cycle_boundary() {
        let mut runs: [Vec<OpenRun>; 8] = std::array::from_fn(|_| vec![]);
        runs[0].push(OpenRun {
            start_ps: 0,
            end_ps: 10,
        });
        let mut totals = [0; 8];
        totals[0] = 10;
        let schedule = Schedule {
            id: "always".into(),
            base_ps: 0,
            cycle_ps: 10,
            entries: vec![GateEntry {
                operation: crate::types::ethernet::tsn::GateOperation::SetGateStates,
                duration_ps: 10,
                open_mask: 1,
            }],
            prefix_ps: vec![0, 10],
            class_open_runs: runs,
            open_total_ps: totals,
        };
        let clock = NodeClock::new(0, 0, 3, 2).unwrap();
        assert_eq!(
            fit(&schedule, 0, 9, 30, None, Some(&clock)).unwrap(),
            Some(9)
        );
        assert_eq!(
            fit(&schedule, 0, 9, 30, Some(38), Some(&clock)).unwrap(),
            None
        );
        assert_eq!(
            fit(&schedule, 0, 9, 30, Some(39), Some(&clock)).unwrap(),
            Some(9)
        );
    }
}
