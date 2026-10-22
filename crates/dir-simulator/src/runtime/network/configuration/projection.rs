//! One physical calendar, conservatively projected into exact affine clocks.
use super::*;
use crate::runtime::network::clock::{LocalTime, NodeClock};

pub fn rounding_quantum(port: &ManagedPort) -> u64 {
    if port.clock_rate_num == port.clock_rate_den && !port.clock_fractional {
        0
    } else {
        port.clock_rate_den.div_ceil(port.clock_rate_num)
    }
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
fn supercycle(p: &PreparedQcc, own: usize, clock: &NodeClock) -> Result<u64, Rejection> {
    let c = u128::from(p.config.cycle_ps.0);
    let mut k = 1u128;
    for port in p
        .ports
        .iter()
        .filter(|port| port.writable_schedule && p.depends_on_direction(port.direction))
    {
        let (num, den) = if port.direction == own {
            clock.rate()
        } else {
            (port.clock_rate_num, port.clock_rate_den)
        };
        let term = u128::from(den)
            / gcd(
                c.checked_mul(u128::from(num))
                    .ok_or(Rejection::ArithmeticOverflow)?,
                u128::from(den),
            );
        k = (k / gcd(k, term))
            .checked_mul(term)
            .ok_or(Rejection::ArithmeticOverflow)?;
        if k > u128::from(p.config.search_budget) {
            return Err(Rejection::SearchLimit);
        }
    }
    u64::try_from(k).map_err(|_| Rejection::ArithmeticOverflow)
}
// Channel0..7: own class permit;8: background forbidden;9: foreign all forbidden.
fn interval(
    events: &mut BTreeMap<u64, Vec<(usize, i32)>>,
    start: i128,
    end: i128,
    period: u64,
    channel: usize,
) -> Result<(), Rejection> {
    if end <= start {
        return Ok(());
    }
    let period_i = i128::from(period);
    let mut insert = |a, b| {
        events.entry(a).or_default().push((channel, 1));
        events.entry(b).or_default().push((channel, -1));
    };
    if end - start >= period_i {
        insert(0, period);
        return Ok(());
    }
    let a = u64::try_from(start.rem_euclid(period_i)).map_err(|_| Rejection::ArithmeticOverflow)?;
    let length = u64::try_from(end - start).map_err(|_| Rejection::ArithmeticOverflow)?;
    let b = add(a, length)?;
    if b <= period {
        insert(a, b)
    } else {
        insert(a, period);
        insert(0, b - period);
    }
    Ok(())
}
pub fn compile_certificate(
    p: &PreparedQcc,
    plan: &PortPlan,
    activation: u64,
    clock: Option<&NodeClock>,
) -> Result<ScheduleCertificate, Rejection> {
    gate_entries(p, plan)?; // Validate the original physical half-open calendar.
    let clock = match clock {
        Some(clock) => clock.clone(),
        None => NodeClock::new(0, 0, 1, 1).map_err(|_| Rejection::InvalidState)?,
    };
    let k = supercycle(p, plan.direction, &clock)?;
    let c = p.config.cycle_ps.0;
    let global_period = multiply(k, c)?;
    let end_global = add(activation, global_period)?;
    let local_start = clock
        .local_at(activation)
        .map_err(|_| Rejection::ArithmeticOverflow)?;
    let base = local_start
        .floor_ps()
        .map_err(|_| Rejection::ArithmeticOverflow)?;
    let base_ps = u64::try_from(base)
        .map_err(|_| Rejection::NoProvenBound(UnprovenBound::ClockProjection))?;
    let (num, den) = clock.rate();
    let exact_period = u128::from(global_period)
        .checked_mul(u128::from(num))
        .ok_or(Rejection::ArithmeticOverflow)?;
    if exact_period % u128::from(den) != 0 {
        return Err(Rejection::InvalidState);
    }
    let period =
        u64::try_from(exact_period / u128::from(den)).map_err(|_| Rejection::ArithmeticOverflow)?;
    if period == 0 {
        return Err(Rejection::NoScheduleInSupportedModel);
    }
    clock
        .local_at(end_global)
        .map_err(|_| Rejection::ArithmeticOverflow)?;
    let windows = u64::try_from(plan.slots.len() + plan.closed_slots.len())
        .map_err(|_| Rejection::SearchLimit)?;
    let expanded = multiply(multiply(k, windows)?, 8)?;
    if add(expanded, 2)? > u64::from(p.config.search_budget) {
        return Err(Rejection::SearchLimit);
    }
    let local = |at, ceil: bool| -> Result<i128, Rejection> {
        let time = clock
            .local_at(at)
            .map_err(|_| Rejection::ArithmeticOverflow)?;
        (if ceil {
            time.ceil_ps()
        } else {
            time.floor_ps()
        })
        .map_err(|_| Rejection::ArithmeticOverflow)
    };
    let mut events = BTreeMap::<u64, Vec<(usize, i32)>>::new();
    events.entry(0).or_default();
    events.entry(period).or_default();
    let mut current_port = p.ports[plan.direction].clone();
    (current_port.clock_rate_num, current_port.clock_rate_den) = clock.rate();
    current_port.clock_fractional = local_start.denominator() != 1;
    let shrink = multiply(rounding_quantum(&current_port), 2)?;
    for repeat in 0..k {
        let origin = add(activation, multiply(repeat, c)?)?;
        for slot in &plan.slots {
            let start_global = add(origin, slot.start_ps)?;
            let end_global = add(origin, slot.end_ps)?;
            let inward_start = local(start_global, true)?;
            let inward_end = local(end_global, false)?;
            let required = (slot.end_ps - slot.start_ps).saturating_sub(shrink);
            let first = clock
                .deadline(start_global, LocalTime::integer(inward_start))
                .map_err(|_| Rejection::ArithmeticOverflow)?
                .global_ps;
            let last = clock
                .deadline(start_global, LocalTime::integer(inward_end))
                .map_err(|_| Rejection::ArithmeticOverflow)?
                .global_ps;
            if inward_end <= inward_start
                || last.checked_sub(first).is_none_or(|width| width < required)
            {
                return Err(Rejection::NoScheduleInSupportedModel);
            }
            interval(
                &mut events,
                inward_start - base,
                inward_end - base,
                period,
                slot.priority as usize,
            )?;
            interval(
                &mut events,
                local(start_global, false)? - base,
                local(end_global, true)? - base,
                period,
                8,
            )?;
        }
        for slot in &plan.closed_slots {
            let a = local(add(origin, slot.start_ps)?, false)? - base;
            let b = local(add(origin, slot.end_ps)?, true)? - base;
            interval(&mut events, a, b, period, 9)?;
        }
    }
    let mut counts = [0i32; 10];
    let mut previous = 0;
    let mut mask = 0u8;
    let background = !p.managed_priority_mask(plan.direction);
    let mut entries = Vec::<(u64, u8)>::new();
    for (cut, changes) in events {
        if cut > previous {
            if let Some(last) = entries.last_mut().filter(|entry| entry.1 == mask) {
                last.0 = add(last.0, cut - previous)?;
            } else {
                entries.push((cut - previous, mask));
            }
        }
        for (channel, delta) in changes {
            counts[channel] = counts[channel]
                .checked_add(delta)
                .ok_or(Rejection::ArithmeticOverflow)?;
            if counts[channel] < 0 {
                return Err(Rejection::InvalidState);
            }
        }
        mask = 0;
        if counts[9] == 0 {
            if counts[8] == 0 {
                mask |= background;
            }
            for (class, count) in counts[..8].iter().enumerate() {
                if *count > 0 {
                    mask |= 1 << class;
                }
            }
        }
        previous = cut;
    }
    if entries.is_empty()
        || entries.iter().map(|e| u128::from(e.0)).sum::<u128>() != u128::from(period)
    {
        return Err(Rejection::InvalidState);
    }
    Ok(ScheduleCertificate {
        direction: plan.direction,
        activation_ps: activation,
        base_ps,
        cycle_ps: period,
        entries,
        clock,
    })
}
pub fn compiled_schedule(
    certificate: &ScheduleCertificate,
    version: u64,
) -> Result<std::sync::Arc<crate::types::ethernet::tsn::Schedule>, Rejection> {
    super::schedule_entries(certificate, version)
}
