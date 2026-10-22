//! Independent physical-tick oracles exercise the production TSN planners.
use super::*;
use crate::input::ethernet::tsn::tests::{config, gate, prep};
use merge::{MacServiceView, Segment, ServiceAccounting};
use serde_json::json;
const PORT: &str = "N.a.tx";
fn prepared(base: u64, cycle: u64, rows: &[(u64, bool)]) -> PreparedTsn {
    let mut v = config();
    v["tsn"]["outputs"][0]["tas"] = gate(base, cycle, rows);
    v["tsn"]["outputs"][0]["cbs"] = json!([{"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}]);
    prep(v)
}
fn queued() -> PortInput {
    let mut input = PortInput::default();
    input.heads[7] = Some(Head { occupancy_ps: 1 });
    input.backlog[7] = true;
    input
}
// Intentionally no NodeClock or Schedule calls: exact numerator inequalities.
fn oracle_open(
    t: u64,
    offset: i128,
    num: u64,
    den: u64,
    base: u64,
    width: u64,
    cycle: u64,
) -> bool {
    let local_num = offset * i128::from(den) + i128::from(t) * i128::from(num);
    let relative = local_num - i128::from(base) * i128::from(den);
    relative >= 0
        && relative % (i128::from(cycle) * i128::from(den)) < i128::from(width) * i128::from(den)
}
fn credit(state: &TsnState) -> Credit {
    state.ports[0].cbs[7].as_ref().unwrap().credit
}
fn service(segment: Segment, sending: bool, retained: bool, busy: bool) -> PortInput {
    let mut backlog = [false; 8];
    backlog[7] = retained;
    PortInput {
        service: Some(MacServiceView {
            active_work: busy.then(|| "data:17:attempt:2".into()),
            retained_backlog: backlog,
            transmit_class: sending.then_some(7),
            busy,
            release_ps: None,
            next_transition_ps: None,
            segment,
            accounting: ServiceAccounting::Causal,
        }),
        ..PortInput::default()
    }
}
#[test]
fn physical_interval_oracle_rates_offsets_base_and_integer_fit_equality() {
    for (num, den) in [(3, 2), (1, 3), (7, 2), (10, 1)] {
        for offset in [-5, 0, 3, 11] {
            for base in [0, 7] {
                let prepared = prepared(base, 10, &[(4, true), (6, false)]);
                let schedule = prepared.outputs[0].tas.as_ref().unwrap();
                let clock = NodeClock::new(0, offset, num, den).unwrap();
                for from in 0..30 {
                    for to in from..60 {
                        let expected = (from..to)
                            .filter(|t| oracle_open(*t, offset, num, den, base, 4, 10))
                            .count() as u64;
                        assert_eq!(
                            clock_projection::open_elapsed(schedule, 7, from, to, Some(&clock))
                                .unwrap(),
                            expected,
                            "rate={num}/{den} offset={offset} base={base} [{from},{to})"
                        );
                    }
                    for occupancy in 1..=9 {
                        let expected = (from..=150).find(|start| {
                            (*start..*start + occupancy)
                                .all(|t| oracle_open(t, offset, num, den, base, 4, 10))
                        });
                        let actual = clock_projection::fit(
                            schedule,
                            7,
                            from,
                            occupancy,
                            Some(160),
                            Some(&clock),
                        )
                        .unwrap();
                        assert_eq!(
                            actual,
                            expected.map(u128::from),
                            "fit rate={num}/{den} offset={offset} base={base} from={from} occupancy={occupancy}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn recovery_steps_settle_old_anchor_and_replace_stale_wakes_with_zero_area() {
    for (num, den) in [(3, 2), (1, 3)] {
        for step in [-13, 17] {
            let prepared = prepared(7, 10, &[(4, true), (6, false)]);
            let mut state = TsnState::new(&prepared);
            let mut clock = NodeClock::new(0, -2, num, den).unwrap();
            // A retained causal debt large enough that recovery never completes.
            state.ports[0].cbs[7].as_mut().unwrap().credit = Credit::new(true, 100 * Q);
            let input = queued();
            let d = state
                .plan_port_clock_in(TsnDelta::default(), PORT, 0, &input, Some(&clock))
                .unwrap();
            state.apply(d);
            let d = state
                .plan_settle_clock_in(TsnDelta::default(), PORT, 23, Some(&clock))
                .unwrap();
            let old = state.ports[0].next_wake;
            let area = (0..23)
                .filter(|t| oracle_open(*t, -2, num, den, 7, 4, 10))
                .count() as u128;
            assert_eq!(
                d.ports[0].1.cbs[7].as_ref().unwrap().credit,
                Credit::new(true, 100 * Q - area * 250_000_000)
            );
            let settled_credit = d.ports[0].1.cbs[7].as_ref().unwrap().credit;
            clock.step_by(23, step).unwrap();
            let d = state
                .plan_port_clock_in(d, PORT, 23, &input, Some(&clock))
                .unwrap();
            assert_eq!(d.ports[0].1.cbs[7].as_ref().unwrap().credit, settled_credit);
            assert!(d.ports[0].1.wake_generation > state.ports[0].wake_generation);
            state.apply(d);
            let new_anchor = clock.local_at(23).unwrap();
            let expected = (23..45)
                .filter(|t| {
                    let numerator = new_anchor.numerator() * i128::from(den)
                        + i128::from(*t - 23)
                            * i128::from(num)
                            * i128::from(new_anchor.denominator());
                    let denominator = i128::from(new_anchor.denominator()) * i128::from(den);
                    let relative = numerator - 7 * denominator;
                    relative >= 0 && relative % (10 * denominator) < 4 * denominator
                })
                .count() as u128;
            let d = state
                .plan_settle_clock_in(TsnDelta::default(), PORT, 45, Some(&clock))
                .unwrap();
            assert_eq!(
                d.ports[0].1.cbs[7].as_ref().unwrap().credit,
                Credit::new(true, settled_credit.magnitude - expected * 250_000_000)
            );
            let _ = old;
        }
    }
}
#[test]
fn actual_tx_steps_debit_physical_duration_and_retained_overhead_recovers() {
    let prepared = prepared(0, 10, &[(4, true), (6, false)]);
    let mut state = TsnState::new(&prepared);
    let mut clock = NodeClock::new(0, 0, 3, 2).unwrap();
    let tx = service(Segment::OriginalMac, true, true, true);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &tx, Some(&clock))
        .unwrap();
    state.apply(d);
    for (t, step) in [(3, 14), (8, -22)] {
        let d = state
            .plan_settle_clock_in(TsnDelta::default(), PORT, t, Some(&clock))
            .unwrap();
        let before = d.ports[0].1.cbs[7].as_ref().unwrap().credit;
        assert_eq!(before, Credit::new(true, u128::from(t) * 750_000_000));
        clock.step_by(t, step).unwrap();
        let d = state
            .plan_port_clock_in(d, PORT, t, &tx, Some(&clock))
            .unwrap();
        assert_eq!(d.ports[0].1.cbs[7].as_ref().unwrap().credit, before);
        state.apply(d);
    }
    let overhead = service(Segment::AddedMcrc, false, true, true);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 10, &overhead, Some(&clock))
        .unwrap();
    state.apply(d);
    let before = credit(&state);
    assert_eq!(before, Credit::new(true, 7_500_000_000));
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 20, &overhead, Some(&clock))
        .unwrap();
    let area = (10..20)
        .filter(|t| {
            // at t=8: old local=12+14-22=4; hence local=(3*t-16)/2.
            let relative = 3 * i128::from(*t) - 16;
            relative >= 0 && relative % 20 < 8
        })
        .count() as u128;
    assert_eq!(
        d.ports[0].1.cbs[7].as_ref().unwrap().credit,
        Credit::new(true, before.magnitude - area * 250_000_000)
    );
    assert!(d.selected.is_empty()); // physical busy with unknown release
    assert!(d.ports[0].1.cbs[7].as_ref().unwrap().backlog);
}
#[test]
fn empty_positive_credit_stays_frozen_until_open_and_retained_work_does_not_reset() {
    for causal in [false, true] {
        let prepared = prepared(0, 10, &[(4, true), (6, false)]);
        let mut state = TsnState::new(&prepared);
        let clock = NodeClock::new(0, 5, 1, 3).unwrap();
        state.ports[0].cbs[7].as_mut().unwrap().credit = Credit::new(false, 9);
        let empty = if causal {
            service(Segment::Idle, false, false, false)
        } else {
            PortInput::default()
        };
        let d = state
            .plan_port_clock_in(TsnDelta::default(), PORT, 0, &empty, Some(&clock))
            .unwrap();
        state.apply(d);
        assert_eq!(credit(&state), Credit::new(false, 9));
        let d = state
            .plan_port_clock_in(TsnDelta::default(), PORT, 14, &empty, Some(&clock))
            .unwrap();
        state.apply(d);
        assert_eq!(credit(&state), Credit::new(false, 9));
        let d = state
            .plan_port_clock_in(TsnDelta::default(), PORT, 15, &empty, Some(&clock))
            .unwrap();
        state.apply(d);
        assert_eq!(credit(&state), Credit::default());
        assert_eq!(
            state.ports[0].cbs[7].as_ref().unwrap().mode,
            CreditMode::Zero
        );
    }
    let prepared = prepared(0, 10, &[(10, true)]);
    let mut state = TsnState::new(&prepared);
    let retained = service(Segment::AddedInterruptionIpg, false, true, true);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &retained, None)
        .unwrap();
    state.apply(d);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 5, &retained, None)
        .unwrap();
    state.apply(d);
    assert_eq!(credit(&state), Credit::new(false, 1_250_000_000));
}
#[test]
fn clock_guard_overflow_and_old_generation_are_atomic() {
    let prepared = prepared(0, 10, &[(4, true), (6, false)]);
    let mut state = TsnState::new(&prepared);
    let mut clock = NodeClock::new(0, 0, 3, 2).unwrap();
    state.ports[0].cbs[7].as_mut().unwrap().credit = Credit::new(true, 10_000_000_000);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &queued(), Some(&clock))
        .unwrap();
    let old = d.wakes[0].clone();
    state.apply(d);
    let before = serde_json::to_value(&state.ports[0]).unwrap();
    clock.step_by(3, 10).unwrap();
    assert!(
        state
            .plan_port_clock_in(TsnDelta::default(), PORT, 4, &queued(), Some(&clock))
            .is_err()
    );
    assert_eq!(serde_json::to_value(&state.ports[0]).unwrap(), before);
    assert!(!state.wake_valid_clock_in(&TsnDelta::default(), &old, Some(&clock)));
    let clock = NodeClock::new(0, 0, 1, u64::MAX).unwrap();
    let schedule = prepared.outputs[0].tas.as_ref().unwrap();
    assert_eq!(
        clock_projection::boundary(schedule, 7, 0, Some(&clock)).unwrap(),
        None
    );
    state.ports[0].wake_generation = u64::MAX;
    assert!(
        state
            .plan_port_clock_in(TsnDelta::default(), PORT, 0, &queued(), None)
            .is_err()
    );
}
#[test]
fn global_gcl_cut_local_base_and_same_time_segment_transition() {
    let mut v = config();
    v["tsn"]["outputs"][0]["tas"] = gate(7, 10, &[(4, true), (6, false)]);
    v["tsn"]["outputs"][0]["cbs"] = json!([{"priority":7,"idle_slope_bps":"250000000","hi_credit_bits":"1000","lo_credit_bits":"1000"}]);
    let mut replacement = gate(30, 10, &[(6, true), (4, false)]);
    replacement["id"] = json!("replacement");
    v["tsn"]["gcl_updates"] = json!([{"id":"u1","submitted_at_ps":"0","effective_at_ps":"23","port":PORT,"schedule":replacement}]);
    let mut state = TsnState::new(&prep(v));
    let clock = NodeClock::new(0, -2, 3, 2).unwrap();
    let old = service(Segment::OriginalMac, true, true, true);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &old, Some(&clock))
        .unwrap();
    state.apply(d);
    let overhead = service(Segment::AddedMcrc, false, true, true);
    let clocks = BTreeMap::from([(PORT.into(), clock.clone())]);
    let inputs = BTreeMap::from([(PORT.into(), overhead.clone())]);
    let d = state
        .plan_control_clock_in(TsnDelta::default(), 23, &inputs, &clocks)
        .unwrap();
    assert_eq!(d.gcl_updates, 1);
    assert_eq!(
        d.ports[0].1.cbs[7].as_ref().unwrap().credit,
        Credit::new(true, 23 * 750_000_000)
    );
    assert_eq!(d.ports[0].1.schedule_generation, 1);
    state.apply(d);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 31, &overhead, Some(&clock))
        .unwrap();
    let area = (23..31)
        .filter(|t| oracle_open(*t, -2, 3, 2, 30, 6, 10))
        .count() as u128;
    assert_eq!(
        d.ports[0].1.cbs[7].as_ref().unwrap().credit,
        Credit::new(true, 23 * 750_000_000 - area * 250_000_000)
    );
}

#[test]
fn clock_edge_step_stale_wake_and_pending_target_mutants() {
    let prepared = prepared(7, 10, &[(4, true), (6, false)]);
    let mut state = TsnState::new(&prepared);
    let mut clock = NodeClock::new(0, -2, 3, 2).unwrap();
    let input = queued();
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &input, Some(&clock))
        .unwrap();
    let old = d.wakes[0].clone();
    assert_eq!(old.time_ps, 6); // local reaches base7 exactly
    assert_eq!(
        d.ports[0].1.local_wake.as_ref().unwrap().local_deadline,
        crate::runtime::network::clock::LocalTime::integer(7)
    );
    state.apply(d);
    state.validate_checkpoint(0).unwrap();
    state.validate_clock_dependency(PORT, Some(&clock)).unwrap();
    let mut mutant = TsnState::new(&prepared);
    mutant.ports = state.ports.clone();
    mutant.ports[0].local_wake.as_mut().unwrap().global_ps += 1;
    assert!(mutant.validate_checkpoint(0).is_err());
    let mut mutant = TsnState::new(&prepared);
    mutant.ports = state.ports.clone();
    mutant.ports[0].local_wake.as_mut().unwrap().local_deadline =
        crate::runtime::network::clock::LocalTime::integer(9);
    assert!(mutant.validate_checkpoint(0).is_err());
    let mut mutant = TsnState::new(&prepared);
    mutant.ports = state.ports.clone();
    mutant.ports[0].local_wake = None;
    assert!(mutant.validate_checkpoint(0).is_err());
    let mut changed_clock = clock.clone();
    changed_clock.step_by(0, 1).unwrap();
    assert!(
        state
            .validate_clock_dependency(PORT, Some(&changed_clock))
            .is_err()
    );
    let d = state
        .plan_settle_clock_in(TsnDelta::default(), PORT, 6, Some(&clock))
        .unwrap();
    assert_eq!(
        d.ports[0].1.cbs[7].as_ref().unwrap().credit,
        Credit::default()
    );
    // Step directly from the open edge to the closed edge; zero credit area.
    clock.step_by(6, 4).unwrap();
    let d = state
        .plan_port_clock_in(d, PORT, 6, &input, Some(&clock))
        .unwrap();
    assert!(d.selected.is_empty());
    assert_eq!(d.ports[0].1.gate_mask.unwrap() & 128, 0);
    assert!(!state.wake_valid_clock_in(&d, &old, Some(&clock)));
    assert_eq!(
        d.ports[0].1.cbs[7].as_ref().unwrap().credit,
        Credit::default()
    );
    state.apply(d);
    state.validate_checkpoint(6).unwrap();
}

#[test]
fn clock_aware_install_suspend_and_merge_entrypoints_settle_old_interval() {
    let prepared = prepared(0, 10, &[(4, true), (6, false)]);
    let mut state = TsnState::new(&prepared);
    let clock = NodeClock::new(0, 11, 1, 3).unwrap();
    let input = queued();
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &input, Some(&clock))
        .unwrap();
    state.apply(d);
    let clocks = BTreeMap::from([(PORT.into(), clock.clone())]);
    let inputs = BTreeMap::from([(PORT.into(), input)]);
    let mut schedule = prepared.outputs[0].tas.as_ref().unwrap().as_ref().clone();
    // Local base14 was crossed by the time callback at global12; installation
    // is one-shot at the owner's due cut, not delayed to global base14.
    schedule.base_ps = 14;
    schedule.id = "managed".into();
    let schedules = BTreeMap::from([(PORT.into(), Arc::new(schedule))]);
    let d = state
        .plan_install_clock_in(TsnDelta::default(), 12, &schedules, &inputs, &clocks)
        .unwrap();
    let area = (0..12)
        .filter(|t| oracle_open(*t, 11, 1, 3, 0, 4, 10))
        .count() as u128;
    assert_eq!(
        d.ports[0].1.cbs[7].as_ref().unwrap().credit,
        Credit::new(false, area * 250_000_000)
    );
    state.apply(d);
    let mut closed = prepared.outputs[0].tas.as_ref().unwrap().as_ref().clone();
    closed.base_ps = 15;
    closed.id = "suspended".into();
    for entry in &mut closed.entries {
        entry.open_mask = 0;
    }
    closed.class_open_runs = std::array::from_fn(|_| vec![]);
    closed.open_total_ps = [0; 8];
    let closed = BTreeMap::from([(PORT.into(), Arc::new(closed))]);
    let d = state
        .plan_suspend_clock_in(TsnDelta::default(), 12, &closed, &inputs, 128, &clocks)
        .unwrap();
    assert_eq!(d.ports[0].1.gate_mask.unwrap() & 128, 0);
    state.apply(d);
    let mut merge_input = PortInput::default();
    merge_input.backlog[7] = true;
    merge_input.merge = Some(merge::MergeShaperView {
        medium_busy_until: Some(20),
        transmit_class: Some(7),
        next_boundary: Some(20),
        segment: Segment::OriginalMac,
    });
    let d = state
        .plan_merge_transition_clock_in(TsnDelta::default(), PORT, 12, &merge_input, Some(&clock))
        .unwrap();
    state.apply(d);
    let before = credit(&state);
    merge_input.merge.as_mut().unwrap().transmit_class = None;
    merge_input.merge.as_mut().unwrap().segment = Segment::AddedMcrc;
    let d = state
        .plan_merge_transition_clock_in(TsnDelta::default(), PORT, 16, &merge_input, Some(&clock))
        .unwrap();
    let expected_debit = 3_000_000_000u128;
    let expected = if before.magnitude >= expected_debit {
        Credit::new(false, before.magnitude - expected_debit)
    } else {
        Credit::new(true, expected_debit - before.magnitude)
    };
    assert_eq!(d.ports[0].1.cbs[7].as_ref().unwrap().credit, expected);
}

#[test]
fn exact_repetition_uses_physical_period_at_wide_horizon() {
    for (num, den) in [(3, 2), (1, 3), (7, 2)] {
        let prepared = prepared(7, 10, &[(4, true), (6, false)]);
        let schedule = prepared.outputs[0].tas.as_ref().unwrap();
        let clock = NodeClock::new(0, -2, num, den).unwrap();
        // First physical tick reaching base, calculated without NodeClock.
        let start = (9 * den).div_ceil(num);
        // cycle10, num coprime with 10 in these three cases.
        let period = 10 * den;
        let count = (start..start + period)
            .filter(|t| oracle_open(*t, -2, num, den, 7, 4, 10))
            .count() as u64;
        let repeats = 1_000_000_000_000_000u64;
        let end = start + repeats * period + 7;
        let tail = (start..start + 7)
            .filter(|t| oracle_open(*t, -2, num, den, 7, 4, 10))
            .count() as u64;
        assert_eq!(
            clock_projection::open_elapsed(schedule, 7, 0, end, Some(&clock)).unwrap(),
            count * repeats + tail
        );
    }
}

#[test]
fn causal_credit_and_recovery_deadline_overflow_discard_entire_plan() {
    let mut p = prepared(0, 10, &[(10, true)]);
    p.outputs[0].cbs[7].as_mut().unwrap().idle_bps = 1;
    let mut state = TsnState::new(&p);
    let retained = service(Segment::Idle, false, true, false);
    state.ports[0].cbs[7].as_mut().unwrap().credit = Credit::new(false, u128::MAX - 1);
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &retained, None)
        .unwrap();
    state.apply(d);
    let before = serde_json::to_value(&state.ports[0]).unwrap();
    assert!(
        state
            .plan_port_clock_in(TsnDelta::default(), PORT, 2, &retained, None)
            .is_err()
    );
    assert_eq!(serde_json::to_value(&state.ports[0]).unwrap(), before);
    state.ports[0].last_ps = 1;
    state.ports[0].cbs[7].as_mut().unwrap().last_ps = 1;
    state.ports[0].cbs[7].as_mut().unwrap().credit = Credit::new(true, u128::MAX);
    let before = serde_json::to_value(&state.ports[0]).unwrap();
    assert!(
        state
            .plan_port_clock_in(TsnDelta::default(), PORT, 1, &retained, None)
            .is_err()
    );
    assert_eq!(serde_json::to_value(&state.ports[0]).unwrap(), before);
}

#[test]
fn reanchor_before_same_ps_global_control_keeps_old_list_until_its_ordered_event() {
    let mut v = config();
    v["tsn"]["outputs"][0]["tas"] = gate(0, 10, &[(4, true), (6, false)]);
    let mut replacement = gate(30, 10, &[(6, true), (4, false)]);
    replacement["id"] = json!("replacement");
    v["tsn"]["gcl_updates"] = json!([{"id":"u1","submitted_at_ps":"0","effective_at_ps":"23","port":PORT,"schedule":replacement}]);
    let mut state = TsnState::new(&prep(v));
    let mut clock = NodeClock::new(0, 0, 1, 1).unwrap();
    let input = queued();
    let d = state
        .plan_port_clock_in(TsnDelta::default(), PORT, 0, &input, Some(&clock))
        .unwrap();
    state.apply(d);
    let d = state
        .plan_settle_clock_in(TsnDelta::default(), PORT, 23, Some(&clock))
        .unwrap();
    clock.step_by(23, 8).unwrap();
    let d = state
        .plan_reanchor_clock_in(d, PORT, 23, &input, Some(&clock))
        .unwrap();
    assert_eq!(d.ports[0].1.schedule_generation, 0);
    assert!(d.update_cursor.is_none());
    assert_eq!(d.ports[0].1.oper.as_ref().unwrap().id, "g0");
    assert!(d.selected.is_empty()); // no claim across the pending global cut
    let clocks = BTreeMap::from([(PORT.into(), clock)]);
    let inputs = BTreeMap::from([(PORT.into(), input)]);
    let d = state
        .plan_control_clock_in(d, 23, &inputs, &clocks)
        .unwrap();
    assert_eq!(d.ports[0].1.schedule_generation, 1);
    assert_eq!(d.ports[0].1.oper.as_ref().unwrap().id, "replacement");
    assert_eq!(d.update_cursor, Some(1));
    assert_eq!(d.selected[PORT], 7);
}

#[test]
fn merge_operation_projection_subtracts_physical_advance_and_retains_local_target() {
    let mut v = config();
    v["tsn"]["outputs"][0]["tas"] = gate(7, 10, &[(4, true), (6, false)]);
    v["tsn"]["outputs"][0]["tas"]["entries"][0]["operation"] = json!("set_and_release_mac");
    v["tsn"]["outputs"][0]["tas"]["entries"][1]["operation"] = json!("set_and_hold_mac");
    let state = TsnState::new(&prep(v));
    for (num, den) in [(3, 2), (1, 3)] {
        for offset in [-5, 0, 11] {
            let clock = NodeClock::new(0, offset, num, den).unwrap();
            for now in 0..25 {
                let mut expected = None;
                for (target0, hold, advance) in [(7, false, 1), (7, true, 3)] {
                    // HOLD entry is at base+4, RELEASE at base.
                    let target0 = target0 + if hold { 4 } else { 0 };
                    for k in 0..100 {
                        let local = target0 + 10 * k;
                        let global = (0u64..400)
                            .find(|t| {
                                offset * i128::from(den) + i128::from(*t) * i128::from(num)
                                    >= i128::from(local) * i128::from(den)
                            })
                            .unwrap();
                        if let Some(at) = global.checked_sub(advance).filter(|t| *t > now) {
                            let candidate = (at, global, hold, local);
                            if expected.is_none_or(|old: (u64, u64, bool, u64)| at < old.0) {
                                expected = Some(candidate);
                            }
                            break;
                        }
                    }
                }
                let actual = state
                    .next_gate_operation_clock_in(
                        &TsnDelta::default(),
                        PORT,
                        now,
                        3,
                        1,
                        Some(&clock),
                    )
                    .unwrap()
                    .unwrap();
                let (at, global, hold, local) = expected.unwrap();
                assert_eq!(
                    (actual.at, actual.target_global_ps, actual.hold),
                    (at, global, hold)
                );
                assert_eq!(
                    actual.local_target,
                    Some(crate::runtime::network::clock::LocalTime::integer(
                        i128::from(local)
                    ))
                );
                assert_eq!(actual.clock_generation, Some(0));
            }
        }
    }
}
