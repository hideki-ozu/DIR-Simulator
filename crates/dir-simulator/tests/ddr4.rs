use dir_simulator::runtime::ddr4::*;
use std::collections::BTreeMap;
fn config() -> Config {
    Config {
        mode_id: MODE.into(),
        source_sha256: SOURCE_SHA256.into(),
        base: 0,
        size: CAPACITY,
        queue_capacity: 8,
    }
}
fn ready() -> Device {
    let mut service = Service::new(config()).unwrap();
    let mut deadlines = 0;
    while !service.device.is_ready() {
        let next = service.next_due_ps().unwrap();
        service.advance(next).unwrap();
        deadlines += 1;
        assert!(deadlines < 20, "initialization must jump to real deadlines");
    }
    service.validate().unwrap();
    service.device
}
fn tick(d: &mut Device, at: u64) {
    d.advance(at).unwrap();
}
fn command(d: &mut Device, at: u64, c: Command) {
    tick(d, at);
    d.issue(c).unwrap();
}
fn rd(bank: u8) -> Command {
    Command::Read {
        bank,
        column: 0,
        auto_precharge: false,
        burst: format!("r{bank}"),
        request: "read".into(),
        logical_offset: std::array::from_fn(Some),
    }
}
fn wr(bank: u8) -> Command {
    Command::Write {
        bank,
        column: 0,
        auto_precharge: false,
        burst: format!("w{bank}"),
        request: "write".into(),
        data: [1, 2, 3, 4, 5, 6, 7, 8],
        byte_enables: [true; 8],
        logical_offset: std::array::from_fn(Some),
    }
}
fn boundary(mut d: Device, start: u64, cycles: u64, c: Command, rule: &str) {
    tick(&mut d, start + (cycles - 1) * TCK);
    let before = d.clone();
    let e = d.issue(c.clone()).expect_err(rule);
    assert_eq!(e.rule, rule);
    assert_eq!(d, before, "refused command must be atomic");
    tick(&mut d, start + cycles * TCK);
    d.issue(c).unwrap();
}
#[test]
fn full_initialization_has_real_commands_and_no_cycle_busy_loop() {
    let mut s = Service::new(config()).unwrap();
    let mut effects = Vec::new();
    let mut steps = 0;
    while !s.device.is_ready() {
        let at = s.next_due_ps().unwrap();
        effects.extend(s.advance(at).unwrap());
        s.validate().unwrap();
        steps += 1;
        assert!(steps < 20);
    }
    assert!(s.now_ps() >= 700_000_000);
    let names: Vec<_> = effects
        .iter()
        .filter_map(|e| {
            if let Effect::Command { command, .. } = e {
                Some(command.name())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        names,
        [
            "RESET_RELEASE",
            "CKE_HIGH",
            "MRS",
            "MRS",
            "MRS",
            "MRS",
            "MRS",
            "MRS",
            "MRS",
            "ZQCL"
        ]
    );
    let mrs: Vec<_> = effects
        .iter()
        .filter_map(|e| {
            if let Effect::Command {
                at_ps,
                command: Command::Mrs { index, value },
            } = e
            {
                Some((*at_ps, *index, *value))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        mrs.iter().map(|m| m.1).collect::<Vec<_>>(),
        [3, 6, 5, 4, 2, 1, 0]
    );
    assert_eq!(mrs[6].2, 0x0934);
    for pair in mrs.windows(2) {
        assert_eq!(pair[1].0 - pair[0].0, 8 * 834);
    }
    assert!(effects.iter().any(|e| matches!(e, Effect::Ready { .. })));
    assert_eq!(s.device.mr.map(Option::unwrap), MR);
}
#[test]
fn independent_bank_and_bank_group_minima() {
    let d = ready();
    let t = d.now_ps;
    let mut active = d.clone();
    command(&mut active, t, Command::Act { bank: 0, row: 0 });
    boundary(active.clone(), t, 16, rd(0), "ACT_to_RD_WR");
    boundary(
        active.clone(),
        t,
        39,
        Command::Pre { bank: Some(0) },
        "ACT_to_PRE",
    );
    boundary(
        active.clone(),
        t,
        6,
        Command::Act { bank: 1, row: 0 },
        "ACT_same_BG",
    );
    boundary(
        active.clone(),
        t,
        4,
        Command::Act { bank: 4, row: 0 },
        "ACT_diff_BG",
    );
    let mut row = active;
    command(&mut row, t + 39 * TCK, Command::Pre { bank: Some(0) });
    boundary(
        row,
        t + 39 * TCK,
        16,
        Command::Act { bank: 0, row: 1 },
        "PRE_to_ACT_REF",
    );
    let mut faw = d;
    for (cycle, bank) in [(0, 0), (4, 4), (8, 8), (12, 12)] {
        command(&mut faw, t + cycle * TCK, Command::Act { bank, row: 0 });
    }
    boundary(
        faw,
        t,
        26,
        Command::Act { bank: 1, row: 0 },
        "ACT_fifth_window",
    );
}
#[test]
fn cas_direction_and_group_turnarounds() {
    for (write1, write2, bank2, cycles, rule) in [
        (false, false, 1, 6, "CAS_same_BG"),
        (false, false, 4, 4, "CAS_diff_BG"),
        (true, true, 1, 6, "CAS_same_BG"),
        (true, true, 4, 4, "CAS_diff_BG"),
        (false, true, 4, 10, "RD_to_WR"),
        (true, false, 1, 25, "WR_to_RD_same_BG"),
        (true, false, 4, 19, "WR_to_RD_diff_BG"),
    ] {
        let mut d = ready();
        let t = d.now_ps;
        command(&mut d, t, Command::Act { bank: 0, row: 0 });
        command(
            &mut d,
            t + 6 * TCK,
            Command::Act {
                bank: bank2,
                row: 0,
            },
        );
        let c = t + 22 * TCK;
        command(&mut d, c, if write1 { wr(0) } else { rd(0) });
        boundary(
            d,
            c,
            cycles,
            if write2 { wr(bank2) } else { rd(bank2) },
            rule,
        );
    }
}
#[test]
fn precharge_read_write_and_automatic_precharge() {
    for (write, cycles, rule) in [(false, 9, "RD_to_PRE"), (true, 34, "WR_to_PRE")] {
        let mut d = ready();
        let t = d.now_ps;
        command(&mut d, t, Command::Act { bank: 0, row: 0 });
        let c = t + 40 * TCK;
        command(&mut d, c, if write { wr(0) } else { rd(0) });
        boundary(d, c, cycles, Command::Pre { bank: Some(0) }, rule);
    }
    for (write, precycles) in [(false, 9), (true, 34)] {
        let mut d = ready();
        let t = d.now_ps;
        command(&mut d, t, Command::Act { bank: 0, row: 0 });
        let c = t + 40 * TCK;
        let mut op = if write { wr(0) } else { rd(0) };
        match &mut op {
            Command::Read { auto_precharge, .. } | Command::Write { auto_precharge, .. } => {
                *auto_precharge = true
            }
            _ => unreachable!(),
        };
        command(&mut d, c, op);
        tick(&mut d, c + TCK);
        assert_eq!(d.issue(rd(0)).unwrap_err().rule, "bank_command_state");
        tick(&mut d, c + precycles * TCK);
        assert!(d.banks[0].row.is_none());
        boundary(
            d,
            c + precycles * TCK,
            16,
            Command::Act { bank: 0, row: 1 },
            "PRE_to_ACT_REF",
        );
    }
}
#[test]
fn physical_read_write_windows_dm_and_checkpoint_continuation() {
    let mut s = Service::new(config()).unwrap();
    s.enqueue(Request {
        id: "write".into(),
        address: 3,
        operation: Operation::Write {
            data: (20..34).collect(),
            byte_enables: (0..14).map(|i| i % 3 != 1).collect(),
        },
    })
    .unwrap();
    s.enqueue(Request {
        id: "read".into(),
        address: 3,
        operation: Operation::Read { length: 14 },
    })
    .unwrap();
    let mut memory = BTreeMap::new();
    let mut got = BTreeMap::new();
    let mut journal = Vec::new();
    let mut restored = false;
    let mut snapshots = 0;
    while !s.is_empty() {
        let at = s.next_due_ps().unwrap();
        let effects = s.advance(at).unwrap();
        for e in &effects {
            match e {
                Effect::WriteByte {
                    address,
                    value,
                    byte_enable: true,
                    ..
                } => {
                    memory.insert(*address, *value);
                }
                Effect::ReadByte {
                    request,
                    logical_offset: Some(i),
                    address,
                    ..
                } if request == "read" => {
                    got.insert(*i, *memory.get(address).unwrap_or(&0));
                }
                _ => {}
            }
        }
        if effects
            .iter()
            .any(|e| matches!(e, Effect::WriteByte { .. }))
            && !restored
        {
            let blob = serde_json::to_vec(&s).unwrap();
            let round: Service = serde_json::from_slice(&blob).unwrap();
            round.validate().unwrap();
            let mut a = s.clone();
            let mut b = round;
            let next = a.next_due_ps().unwrap();
            assert_eq!(a.advance(next).unwrap(), b.advance(next).unwrap());
            restored = true;
            // Compare a checkpoint without consuming a byte in the real run.
            s = serde_json::from_slice(&blob).unwrap();
        }
        s.validate().unwrap();
        journal.extend(effects);
        snapshots += 1;
        assert!(snapshots < 200);
    }
    assert!(restored);
    assert_eq!(
        got.values().copied().collect::<Vec<_>>(),
        (0..14)
            .map(|i| if i % 3 == 1 { 0 } else { 20 + i as u8 })
            .collect::<Vec<_>>()
    );
    for e in &journal {
        if let Effect::Command { at_ps, command } = e {
            let (burst, write) = match command {
                Command::Read { burst, .. } => (burst, false),
                Command::Write { burst, .. } => (burst, true),
                _ => continue,
            };
            let first = journal
                .iter()
                .filter_map(|x| match x {
                    Effect::ReadByte {
                        at_ps, burst: b, ..
                    }
                    | Effect::WriteByte {
                        at_ps, burst: b, ..
                    } if b == burst => Some(*at_ps),
                    _ => None,
                })
                .min()
                .unwrap();
            assert_eq!(first - *at_ps, if write { 12 * 834 } else { 16 * 834 });
        }
    }
    assert_eq!(
        journal
            .iter()
            .filter(|e| matches!(e, Effect::RequestComplete { .. }))
            .count(),
        2
    );
    // A complete, valid-hash-style serialized checkpoint with a moved data beat
    // is rejected even if its JSON shape remains valid.
    let mut in_flight = Service::new(config()).unwrap();
    in_flight
        .enqueue(Request {
            id: "x".into(),
            address: 0,
            operation: Operation::Read { length: 8 },
        })
        .unwrap();
    loop {
        let e = in_flight.advance(in_flight.next_due_ps().unwrap()).unwrap();
        if e.iter().any(|e| {
            matches!(
                e,
                Effect::Command {
                    command: Command::Read { .. },
                    ..
                }
            )
        }) {
            break;
        }
    }
    let mut value = serde_json::to_value(&in_flight).unwrap();
    let pending = value["device"]["pending"].as_array_mut().unwrap();
    let beat = pending
        .iter_mut()
        .find(|p| p["kind"]["Output"]["kind"] == "read_byte")
        .unwrap();
    beat["kind"]["Output"]["address"] = serde_json::json!(7);
    let corrupted: Service = serde_json::from_value(value).unwrap();
    assert_eq!(
        corrupted.validate().unwrap_err().rule,
        "pending_byte_bijection"
    );
}
#[test]
fn refresh_priority_blocks_transport_until_real_rfc() {
    let mut s = Service::new(config()).unwrap();
    while !s.device.is_ready() {
        s.advance(s.next_due_ps().unwrap()).unwrap();
    }
    let due = s.device.refresh_due_ps().unwrap();
    let ref_at = ((due + 833) / 834) * 834;
    let effects = s.advance(ref_at).unwrap();
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Command {
            command: Command::Refresh,
            ..
        }
    )));
    s.enqueue(Request {
        id: "after_refresh".into(),
        address: 0,
        operation: Operation::Read { length: 8 },
    })
    .unwrap();
    let early = s.advance(ref_at + 311 * 834).unwrap();
    assert!(!early.iter().any(|e| matches!(
        e,
        Effect::Command {
            command: Command::Act { .. },
            ..
        }
    )));
    let boundary = s.advance(ref_at + 312 * 834).unwrap();
    assert!(boundary.iter().any(|e| matches!(
        e,
        Effect::Command {
            command: Command::Act { .. },
            ..
        }
    )));
    assert_eq!(s.len(), 1);
    s.validate().unwrap();
}
#[test]
fn strict_configuration_queue_bounds_and_refused_commands() {
    let mut c = config();
    c.mode_id = "generic-ddr".into();
    assert!(Service::new(c).is_err());
    let mut c = config();
    c.queue_capacity = 1;
    let mut s = Service::new(c).unwrap();
    s.enqueue(Request {
        id: "one".into(),
        address: 0,
        operation: Operation::Read { length: 1 },
    })
    .unwrap();
    let before = s.clone();
    assert_eq!(
        s.enqueue(Request {
            id: "two".into(),
            address: 1,
            operation: Operation::Read { length: 1 }
        })
        .unwrap_err()
        .rule,
        "queue_full"
    );
    assert_eq!(s, before);
    let mut d = ready();
    let now = d.now_ps;
    assert_eq!(d.issue(rd(0)).unwrap_err().rule, "bank_command_state");
    command(&mut d, now, Command::Act { bank: 0, row: 0 });
    let old = d.clone();
    assert!(d.issue(Command::Act { bank: 1, row: 0 }).is_err());
    assert_eq!(d, old);
    tick(&mut d, now + 40 * 834);
    let mut bad = rd(0);
    if let Command::Read { column, .. } = &mut bad {
        *column = 1;
    }
    assert_eq!(d.issue(bad).unwrap_err().rule, "burst_address");
    assert_eq!(
        d.issue(Command::Mrs {
            index: 1,
            value: 0x0041
        })
        .unwrap_err()
        .rule,
        "MRS_idle"
    );
}

#[test]
fn intervening_bank_groups_do_not_erase_write_to_read_guards() {
    for middle_read in [false, true] {
        let mut d = ready();
        let t = d.now_ps;
        command(&mut d, t, Command::Act { bank: 0, row: 0 });
        command(&mut d, t + 4 * TCK, Command::Act { bank: 4, row: 0 });
        let first = t + 20 * TCK;
        command(&mut d, first, wr(0));
        let middle = if middle_read { 19 } else { 4 };
        command(
            &mut d,
            first + middle * TCK,
            if middle_read { rd(4) } else { wr(4) },
        );
        boundary(d, first, 25, rd(0), "WR_to_RD_same_BG");
    }
}

#[test]
fn reset_and_mode_initialization_boundaries_are_atomic() {
    let mut d = Device::new(config()).unwrap();
    tick(&mut d, 200_000_000 - 1);
    let before = d.clone();
    assert_eq!(
        d.issue(Command::Reset { asserted: false })
            .unwrap_err()
            .rule,
        "RESET_powerup_low"
    );
    assert_eq!(d, before);
    let reset_at = ((200_000_000 + 833) / 834) * 834;
    command(&mut d, reset_at, Command::Reset { asserted: false });
    let cke_at = ((reset_at + 500_000_000 + 833) / 834) * 834;
    tick(&mut d, cke_at - TCK);
    assert_eq!(
        d.issue(Command::Cke { high: true }).unwrap_err().rule,
        "RESET_to_CKE"
    );
    command(&mut d, cke_at, Command::Cke { high: true });
    boundary(
        d.clone(),
        cke_at,
        324,
        Command::Mrs {
            index: 3,
            value: MR[3],
        },
        "rank_busy",
    );
    tick(&mut d, cke_at + 324 * TCK);
    assert_eq!(
        d.issue(Command::Mrs {
            index: 0,
            value: 0x0934
        })
        .unwrap_err()
        .rule,
        "initialization_sequence"
    );
    command(
        &mut d,
        cke_at + 324 * TCK,
        Command::Mrs {
            index: 3,
            value: MR[3],
        },
    );
    boundary(
        d,
        cke_at + 324 * TCK,
        8,
        Command::Mrs {
            index: 6,
            value: MR[6],
        },
        "MRS_to_MRS",
    );
    let mut d = ready();
    let at = d.now_ps;
    command(
        &mut d,
        at,
        Command::Mrs {
            index: 2,
            value: MR[2],
        },
    );
    boundary(
        d.clone(),
        at,
        8,
        Command::Mrs {
            index: 3,
            value: MR[3],
        },
        "MRS_to_MRS",
    );
    boundary(d, at, 24, Command::Act { bank: 0, row: 0 }, "MRS_to_nonMRS");
    let mut d = ready();
    let at = d.now_ps;
    command(&mut d, at, Command::Reset { asserted: true });
    assert!(matches!(d.phase, Phase::ResetLow { powerup: false, .. }));
    tick(&mut d, at + 999_999);
    assert_eq!(
        d.issue(Command::Reset { asserted: false })
            .unwrap_err()
            .rule,
        "RESET_stable_low"
    );
    // Warm reset is destructive even when no queued physical beat completes.
    assert!(d.banks.iter().all(|b| b.row.is_none()));
    assert!(d.advance(at + 1_000_000).unwrap().is_empty());
}
#[test]
fn refresh_zq_and_power_state_boundaries() {
    for (long, cycles) in [(false, 128), (true, 512)] {
        let mut d = ready();
        let t = d.now_ps;
        command(&mut d, t, Command::Zq { long });
        boundary(d, t, cycles, Command::Act { bank: 0, row: 0 }, "rank_busy");
    }
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Refresh);
    boundary(d, t, 312, Command::Act { bank: 0, row: 0 }, "rank_busy");
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Cke { high: false });
    tick(&mut d, t + TCK);
    assert_eq!(
        d.issue(Command::Act { bank: 0, row: 0 }).unwrap_err().rule,
        "command_disable"
    );
    boundary(d.clone(), t, 6, Command::Cke { high: true }, "CKE_pulse");
    command(&mut d, t + 6 * TCK, Command::Cke { high: true });
    boundary(
        d,
        t + 6 * TCK,
        8,
        Command::Act { bank: 0, row: 0 },
        "rank_busy",
    );
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Act { bank: 0, row: 0 });
    boundary(d, t, 2, Command::Cke { high: false }, "ACT_to_PDE");
    for (write, auto, cycles, rule) in [
        (false, false, 21, "RD_to_PDE"),
        (true, false, 34, "WR_to_PDE"),
        (true, true, 35, "WRA_to_PDE"),
    ] {
        let mut d = ready();
        let t = d.now_ps;
        command(&mut d, t, Command::Act { bank: 0, row: 0 });
        let c = t + 40 * TCK;
        let mut op = if write { wr(0) } else { rd(0) };
        if let Command::Write { auto_precharge, .. } = &mut op {
            *auto_precharge = auto;
        }
        command(&mut d, c, op);
        boundary(d, c, cycles, Command::Cke { high: false }, rule);
    }
}
#[test]
fn self_refresh_has_distinct_exit_guards() {
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::SelfRefresh);
    boundary(d.clone(), t, 7, Command::Cke { high: true }, "SRE_low");
    let x = t + 7 * TCK;
    command(&mut d, x, Command::Cke { high: true });
    boundary(
        d.clone(),
        x,
        144,
        Command::Zq { long: false },
        "SRX_fast_exceptions",
    );
    boundary(
        d.clone(),
        x,
        324,
        Command::Act { bank: 0, row: 0 },
        "SRX_nonDLL",
    );
    tick(&mut d, x + 324 * TCK);
    assert_eq!(d.issue(Command::SelfRefresh).unwrap_err().rule, "SRE_idle");
    command(&mut d, x + 324 * TCK, Command::Refresh);
    command(&mut d, x + 636 * TCK, Command::Act { bank: 0, row: 0 });
    boundary(d, x, 768, rd(0), "SRX_DLL");
}
#[test]
fn vref_entry_bits_are_ignored_and_later_steps_settle() {
    let mut d = ready();
    let t = d.now_ps;
    command(
        &mut d,
        t,
        Command::Mrs {
            index: 6,
            value: 0x08b1,
        },
    ); // entry code49 is don't-care
    assert_eq!(d.mr[6], Some(0x0890)); // underlying programmedcode16 preserved
    tick(&mut d, t + 180 * TCK);
    let mut dummy = d.clone();
    dummy.issue(Command::Act { bank: 0, row: 0 }).unwrap();
    command(
        &mut d,
        t + 180 * TCK,
        Command::Mrs {
            index: 6,
            value: 0x08b2,
        },
    );
    let programmed = t + 180 * TCK;
    // Previous entrybits49 cannot make the first actual50 a single-step.
    tick(&mut d, programmed + 72 * TCK);
    assert_eq!(
        d.issue(Command::Act { bank: 0, row: 0 }).unwrap_err().rule,
        "VREF_enter"
    );
    tick(&mut d, programmed + 180 * TCK);
    command(
        &mut d,
        programmed + 180 * TCK,
        Command::Mrs {
            index: 6,
            value: 0x08b1,
        },
    );
    let step = programmed + 180 * TCK;
    boundary(
        d.clone(),
        step,
        72,
        Command::Act { bank: 0, row: 0 },
        "VREF_enter",
    );
    tick(&mut d, step + 72 * TCK);
    assert_eq!(
        d.issue(Command::Mrs {
            index: 6,
            value: 0x08f1
        })
        .unwrap_err()
        .rule,
        "VREF_enter"
    ); // range change midtraining
    assert_eq!(
        d.issue(Command::Mrs {
            index: 6,
            value: 0x0830
        })
        .unwrap_err()
        .rule,
        "VREF_exit"
    ); // exit cannot changecode
    command(
        &mut d,
        step + 72 * TCK,
        Command::Mrs {
            index: 6,
            value: 0x0831,
        },
    );
    boundary(
        d,
        step + 72 * TCK,
        180,
        Command::Act { bank: 0, row: 0 },
        "rank_busy",
    );
}
#[test]
fn write_leveling_qoff_feedback_and_exit_mrd_are_distinct() {
    let mut d = ready();
    let t = d.now_ps;
    command(
        &mut d,
        t,
        Command::Mrs {
            index: 1,
            value: 0x0081,
        },
    );
    tick(&mut d, t + 24 * TCK);
    assert_eq!(
        d.issue(Command::Dqs { high: false }).unwrap_err().rule,
        "WLEV_drive"
    );
    command(&mut d, t + 25 * TCK, Command::Dqs { high: false });
    tick(&mut d, t + 39 * TCK);
    assert_eq!(
        d.issue(Command::Dqs { high: true }).unwrap_err().rule,
        "WLEV_first_edge"
    );
    let rising = t + 40 * TCK + 200;
    command(&mut d, rising, Command::Dqs { high: true });
    assert!(
        !d.advance(rising + 9499)
            .unwrap()
            .iter()
            .any(|e| matches!(e, Effect::LevelFeedback { .. }))
    );
    let feedback = d.advance(rising + 9500).unwrap();
    assert!(feedback.iter().any(|e| matches!(
        e,
        Effect::LevelFeedback {
            bits: Some([true, true, true, true, true, true, true, true]),
            ..
        }
    )));
    let low = ((rising + 10_000 + 833) / 834) * 834;
    command(&mut d, low, Command::Dqs { high: false });
    let qoff = low + TCK;
    command(
        &mut d,
        qoff,
        Command::Mrs {
            index: 1,
            value: 0x1081,
        },
    );
    command(&mut d, qoff + 24 * TCK + 200, Command::Dqs { high: true });
    assert!(
        !d.advance(qoff + 24 * TCK + 200 + 9500)
            .unwrap()
            .iter()
            .any(|e| matches!(e, Effect::LevelFeedback { .. }))
    );
    let exit = ((d.now_ps + 833) / 834) * 834;
    command(
        &mut d,
        exit,
        Command::Mrs {
            index: 1,
            value: MR[1],
        },
    );
    boundary(
        d.clone(),
        exit,
        8,
        Command::Mrs {
            index: 3,
            value: MR[3],
        },
        "MRS_to_MRS",
    );
    boundary(
        d,
        exit,
        24,
        Command::Act { bank: 0, row: 0 },
        "MRS_to_nonMRS",
    );
}
#[test]
fn checkpoint_rejects_removed_dq_history_and_changed_mode() {
    let mut s = Service::new(config()).unwrap();
    s.enqueue(Request {
        id: "x".into(),
        address: 0,
        operation: Operation::Write {
            data: vec![1; 8],
            byte_enables: vec![true; 8],
        },
    })
    .unwrap();
    loop {
        let effects = s.advance(s.next_due_ps().unwrap()).unwrap();
        if effects.iter().any(|e| {
            matches!(
                e,
                Effect::Command {
                    command: Command::Write { .. },
                    ..
                }
            )
        }) {
            break;
        }
    }
    for (field, new, rule) in [
        ("dq", serde_json::json!([]), "DQ_bijection"),
        (
            "write_groups",
            serde_json::json!([null, null, null, null]),
            "write_group_history",
        ),
        ("last_cas", serde_json::Value::Null, "CAS_history"),
        (
            "mr",
            serde_json::json!([0, 1, 24, 512, 0, 1024, 2064]),
            "mode_checkpoint",
        ),
    ] {
        let mut value = serde_json::to_value(&s).unwrap();
        value["device"][field] = new;
        let bad: Service = serde_json::from_value(value).unwrap();
        assert_eq!(bad.validate().unwrap_err().rule, rule, "{field}");
    }
}

#[test]
fn startup_checkpoint_deadlines_are_bijective() {
    for stage in 0..11 {
        let mut s = Service::new(config()).unwrap();
        for _ in 0..stage {
            s.advance(s.next_due_ps().unwrap()).unwrap();
        }
        s.validate().unwrap();
        let value = serde_json::to_value(&s).unwrap();
        if stage < 10 {
            let mut bad = value.clone();
            bad["next_action"] = serde_json::Value::Null;
            let decoded: Service = serde_json::from_value(bad).unwrap();
            assert_eq!(
                decoded.validate().unwrap_err().rule,
                "initialization_controller_deadline"
            );
        } else {
            for duplicate in [false, true] {
                let mut bad = value.clone();
                let pending = bad["device"]["pending"].as_array_mut().unwrap();
                if duplicate {
                    let mut copy = pending[0].clone();
                    copy["sequence"] = serde_json::json!(1);
                    pending.push(copy);
                    bad["device"]["sequence"] = serde_json::json!(2);
                } else {
                    pending.clear();
                }
                let decoded: Service = serde_json::from_value(bad).unwrap();
                assert_eq!(
                    decoded.validate().unwrap_err().rule,
                    "initialization_deadline_bijection"
                );
            }
        }
    }
}
#[test]
fn delayed_level_feedback_cannot_survive_qoff_or_exit() {
    for exit in [false, true] {
        let mut d = ready();
        let t = d.now_ps;
        command(
            &mut d,
            t,
            Command::Mrs {
                index: 1,
                value: 0x0081,
            },
        );
        command(&mut d, t + 25 * TCK, Command::Dqs { high: false });
        let edge = t + 40 * TCK + 200;
        command(&mut d, edge, Command::Dqs { high: true });
        let mrs = t + 41 * TCK;
        command(
            &mut d,
            mrs,
            Command::Mrs {
                index: 1,
                value: if exit { MR[1] } else { 0x1081 },
            },
        );
        assert!(
            !d.advance(edge + 9500)
                .unwrap()
                .iter()
                .any(|e| matches!(e, Effect::LevelFeedback { .. }))
        );
    }
}

#[test]
fn maintenance_self_refresh_and_destructive_reset_use_same_live_target() {
    let mut s = Service::new(config()).unwrap();
    let mut bytes = BTreeMap::new();
    s.enqueue(Request {
        id: "first".into(),
        address: 0,
        operation: Operation::Write {
            data: vec![9; 8],
            byte_enables: vec![true; 8],
        },
    })
    .unwrap();
    while !s.is_empty() {
        for e in s.advance(s.next_due_ps().unwrap()).unwrap() {
            if let Effect::WriteByte {
                address,
                value,
                byte_enable: true,
                ..
            } = e
            {
                bytes.insert(address, value);
            }
        }
    }
    // Complete PRE recovery before entering selfrefresh via the same target API.
    let close = ((s.now_ps() + 40 * TCK + 833) / 834) * 834;
    s.advance(close).unwrap();
    s.issue_control(Command::Pre { bank: None }).unwrap();
    s.advance(close + 16 * TCK).unwrap();
    let at = s.now_ps();
    s.issue_control(Command::SelfRefresh).unwrap();
    assert!(s.next_due_ps().is_none());
    s.advance(at + 3 * TREFI).unwrap();
    assert_eq!(bytes.values().copied().collect::<Vec<_>>(), vec![9; 8]);
    let x = ((s.now_ps() + 833) / 834) * 834;
    s.advance(x).unwrap();
    s.issue_control(Command::Cke { high: true }).unwrap();
    s.enqueue(Request {
        id: "replace".into(),
        address: 0,
        operation: Operation::Write {
            data: vec![5; 8],
            byte_enables: vec![true; 8],
        },
    })
    .unwrap();
    s.enqueue(Request {
        id: "pending_read".into(),
        address: 0,
        operation: Operation::Read { length: 8 },
    })
    .unwrap();
    loop {
        let effects = s.advance(s.next_due_ps().unwrap()).unwrap();
        if effects
            .iter()
            .any(|e| matches!(e, Effect::WriteByte { .. }))
        {
            break;
        }
    }
    let reset_at = s.now_ps();
    let reset = s.issue_control(Command::Reset { asserted: true }).unwrap();
    for effect in &reset {
        if matches!(effect, Effect::ResetInvalidation { .. }) {
            bytes.clear();
        }
    }
    assert!(bytes.is_empty());
    assert!(s.is_empty());
    assert_eq!(
        reset
            .iter()
            .filter(|e| matches!(e, Effect::RequestAborted { .. }))
            .count(),
        2
    );
    s.validate().unwrap();
    let later = s.advance(reset_at + 100_000).unwrap();
    assert!(!later.iter().any(|e| matches!(
        e,
        Effect::WriteByte { .. } | Effect::ReadByte { .. } | Effect::RequestComplete { .. }
    )));
    while !s.device.is_ready() {
        s.advance(s.next_due_ps().unwrap()).unwrap();
        s.validate().unwrap();
    }
    assert!(
        bytes.is_empty(),
        "reinitialization cannot invent retained data"
    );
}
#[test]
fn long_idle_and_queued_bank_conflicts_refresh_without_starvation() {
    let mut s = Service::new(config()).unwrap();
    for i in 0..8 {
        s.enqueue(Request {
            id: format!("q{i}"),
            address: if i % 2 == 0 { 0 } else { 16384 },
            operation: Operation::Write {
                data: vec![i as u8; 64],
                byte_enables: vec![true; 64],
            },
        })
        .unwrap();
    }
    let all = s.advance(750_000_000).unwrap();
    assert!(s.is_empty());
    assert_eq!(
        all.iter()
            .filter(|e| matches!(e, Effect::RequestComplete { .. }))
            .count(),
        8
    );
    let refresh: Vec<_> = all
        .iter()
        .filter_map(|e| {
            if let Effect::Command {
                at_ps,
                command: Command::Refresh,
            } = e
            {
                Some(*at_ps)
            } else {
                None
            }
        })
        .collect();
    assert!(refresh.len() >= 5);
    for pair in refresh.windows(2) {
        assert!(pair[1] - pair[0] <= 7_800_000 + 834);
    }
    s.validate().unwrap();
}

#[test]
fn explicit_precharge_refresh_and_mrs_each_have_power_entry_boundary() {
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Act { bank: 0, row: 0 });
    let pre = t + 39 * TCK;
    command(&mut d, pre, Command::Pre { bank: Some(0) });
    boundary(d, pre, 2, Command::Cke { high: false }, "PRE_to_PDE");
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Refresh);
    boundary(d, t, 2, Command::Cke { high: false }, "REF_to_PDE");
    let mut d = ready();
    let t = d.now_ps;
    command(
        &mut d,
        t,
        Command::Mrs {
            index: 3,
            value: MR[3],
        },
    );
    boundary(d, t, 24, Command::Cke { high: false }, "MRS_to_PDE");
}
#[test]
fn reset_cke_setup_and_selected_continuous_clock_contract() {
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Reset { asserted: true });
    let release = t + 1300 * TCK;
    command(&mut d, release - 12 * TCK, Command::Cke { high: false });
    tick(&mut d, release - TCK);
    let before = d.clone();
    assert_eq!(
        d.issue(Command::Reset { asserted: false })
            .unwrap_err()
            .rule,
        "RESET_CKE_setup"
    );
    assert_eq!(d, before);
    command(&mut d, release, Command::Reset { asserted: false });
    let mut value = serde_json::to_value(config()).unwrap();
    value["cycle_ps"] = serde_json::json!(833);
    assert!(serde_json::from_value::<Config>(value).is_err());
    for name in ["clock_stop", "clock_restart", "clock_rate"] {
        assert!(serde_json::from_value::<Command>(serde_json::json!({"kind":name})).is_err());
    }
}
#[test]
fn manual_dll_reset_zq_idle_and_power_down_burst_guards() {
    let mut d = ready();
    let t = d.now_ps;
    command(
        &mut d,
        t,
        Command::Mrs {
            index: 0,
            value: 0x0934,
        },
    );
    command(&mut d, t + 24 * TCK, Command::Act { bank: 0, row: 0 });
    boundary(d.clone(), t, 768, rd(0), "DLL_lock");
    tick(&mut d, t + 25 * TCK);
    assert_eq!(
        d.issue(Command::Cke { high: false }).unwrap_err().rule,
        "DLL_lock"
    );
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Act { bank: 0, row: 0 });
    tick(&mut d, t + 40 * TCK);
    assert_eq!(
        d.issue(Command::Zq { long: true }).unwrap_err().rule,
        "ZQ_idle"
    );
    command(&mut d, t + 40 * TCK, Command::Pre { bank: Some(0) });
    boundary(
        d,
        t + 40 * TCK,
        16,
        Command::Zq { long: false },
        "PRE_to_ACT_REF",
    );
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Act { bank: 0, row: 0 });
    command(&mut d, t + 16 * TCK, wr(0));
    tick(&mut d, t + 29 * TCK);
    let before = d.clone();
    assert_eq!(
        d.issue(Command::Cke { high: false }).unwrap_err().rule,
        "PDE_command"
    );
    assert_eq!(d, before);
}
#[test]
fn auto_precharge_with_tras_dominant_read_and_other_bank_progress() {
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::Act { bank: 0, row: 0 });
    let mut op = rd(0);
    if let Command::Read { auto_precharge, .. } = &mut op {
        *auto_precharge = true;
    }
    command(&mut d, t + 16 * TCK, op);
    command(&mut d, t + 17 * TCK, Command::Act { bank: 4, row: 0 });
    tick(&mut d, t + 38 * TCK);
    assert_eq!(d.banks[0].row, Some(0));
    tick(&mut d, t + 39 * TCK);
    assert_eq!(d.banks[0].row, None);
    boundary(
        d,
        t + 39 * TCK,
        16,
        Command::Act { bank: 0, row: 1 },
        "PRE_to_ACT_REF",
    );
}
#[test]
fn srx_reentry_requires_real_ref_and_full_rfc_for_pd_and_sr() {
    let mut d = ready();
    let t = d.now_ps;
    command(&mut d, t, Command::SelfRefresh);
    let exit = t + 7 * TCK;
    command(&mut d, exit, Command::Cke { high: true });
    tick(&mut d, exit + 324 * TCK);
    assert_eq!(
        d.issue(Command::Cke { high: false }).unwrap_err().rule,
        "PDE_command"
    );
    assert_eq!(d.issue(Command::SelfRefresh).unwrap_err().rule, "SRE_idle");
    command(&mut d, exit + 324 * TCK, Command::Refresh);
    boundary(
        d.clone(),
        exit + 324 * TCK,
        312,
        Command::Cke { high: false },
        "SRX_reentry",
    );
    boundary(d, exit + 324 * TCK, 312, Command::SelfRefresh, "rank_busy");
}
#[test]
fn selected_mode_reserved_bits_and_training_encoding_matrix() {
    let d = ready();
    for (index, selected) in MR.iter().copied().enumerate() {
        for bit in 0..16 {
            let value = selected ^ (1 << bit);
            let allowed =
                (index == 0 && bit == 8) || (index == 1 && bit == 7) || (index == 6 && bit == 7);
            let mut probe = d.clone();
            let before = probe.clone();
            let result = probe.issue(Command::Mrs {
                index: index as u8,
                value,
            });
            if allowed {
                result.unwrap();
            } else {
                assert_eq!(
                    result.unwrap_err().rule,
                    "mode_encoding",
                    "MR{index} bit{bit}"
                );
                assert_eq!(probe, before);
            }
        }
    }
    for code in 0..64 {
        let mut probe = d.clone();
        let result = probe.issue(Command::Mrs {
            index: 6,
            value: 0x0880 | code,
        });
        if code <= 50 {
            result.unwrap();
            assert_eq!(probe.mr[6], Some(0x0890));
        } else {
            assert_eq!(result.unwrap_err().rule, "mode_encoding");
        }
    }
    let mut ignored = d;
    let before = ignored.mr;
    ignored
        .issue(Command::Mrs {
            index: 7,
            value: u16::MAX,
        })
        .unwrap();
    assert_eq!(ignored.mr, before);
}
#[test]
fn later_vref_single_and_multi_code_steps_have_independent_boundaries() {
    let mut d = ready();
    let t = d.now_ps;
    command(
        &mut d,
        t,
        Command::Mrs {
            index: 6,
            value: 0x0890,
        },
    );
    command(
        &mut d,
        t + 180 * TCK,
        Command::Mrs {
            index: 6,
            value: 0x0890,
        },
    );
    let first = t + 360 * TCK;
    command(
        &mut d,
        first,
        Command::Mrs {
            index: 6,
            value: 0x0891,
        },
    );
    boundary(
        d.clone(),
        first,
        72,
        Command::Act { bank: 0, row: 0 },
        "VREF_enter",
    );
    command(
        &mut d,
        first + 72 * TCK,
        Command::Mrs {
            index: 6,
            value: 0x0898,
        },
    );
    boundary(
        d,
        first + 72 * TCK,
        180,
        Command::Act { bank: 0, row: 0 },
        "VREF_enter",
    );
}
#[test]
fn leveling_command_legality_pulse_and_setup_hold_aperture() {
    let mut d = ready();
    let t = d.now_ps;
    command(
        &mut d,
        t,
        Command::Mrs {
            index: 1,
            value: 0x0081,
        },
    );
    tick(&mut d, t + 25 * TCK);
    assert_eq!(
        d.issue(Command::Act { bank: 0, row: 0 }).unwrap_err().rule,
        "WLEV_allowed"
    );
    assert_eq!(
        d.issue(Command::Mrs {
            index: 2,
            value: MR[2]
        })
        .unwrap_err()
        .rule,
        "WLEV_allowed"
    );
    command(&mut d, t + 25 * TCK, Command::Dqs { high: false });
    let rising = t + 40 * TCK + 200;
    let mut pulse = d.clone();
    command(&mut pulse, rising, Command::Dqs { high: true });
    tick(&mut pulse, rising + 383);
    let before = pulse.clone();
    assert_eq!(
        pulse.issue(Command::Dqs { high: false }).unwrap_err().rule,
        "WLEV_pulse"
    );
    assert_eq!(pulse, before);
    command(&mut pulse, rising + 384, Command::Dqs { high: false });
    for (phase, expected) in [
        (0, None),
        (108, None),
        (109, Some(true)),
        (308, Some(true)),
        (309, None),
        (526, Some(false)),
    ] {
        let mut probe = d.clone();
        let edge = t + 40 * TCK + phase;
        command(&mut probe, edge, Command::Dqs { high: true });
        let effects = probe.advance(edge + 9500).unwrap();
        let bits = effects
            .iter()
            .find_map(|e| {
                if let Effect::LevelFeedback { bits, .. } = e {
                    Some(*bits)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(bits, expected.map(|b| [b; 8]), "phase{phase}");
    }
}

#[test]
fn queued_independent_banks_overlap_but_share_ca_and_dq_buses() {
    fn drain(service: &mut Service) -> Vec<Effect> {
        let mut effects = Vec::new();
        while !service.is_empty() {
            effects.extend(service.advance(service.next_due_ps().unwrap()).unwrap());
            service.validate().unwrap();
        }
        effects
    }
    fn request(id: &str, bank: u8) -> Request {
        Request {
            id: id.into(),
            address: offset(bank, 0, 0),
            operation: Operation::Read { length: 8 },
        }
    }
    let mut initial = Service::new(config()).unwrap();
    while !initial.device.is_ready() {
        initial.advance(initial.next_due_ps().unwrap()).unwrap();
    }
    let start = initial.now_ps();
    let mut parallel = initial.clone();
    parallel.enqueue(request("first", 0)).unwrap();
    parallel.enqueue(request("second", 4)).unwrap();
    let effects = drain(&mut parallel);
    let mut serial = initial;
    serial.enqueue(request("first", 0)).unwrap();
    drain(&mut serial);
    serial.enqueue(request("second", 4)).unwrap();
    drain(&mut serial);
    assert!(parallel.now_ps() - start < serial.now_ps() - start);
    let commands: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Command { at_ps, command } => Some((*at_ps, command)),
            _ => None,
        })
        .collect();
    assert!(
        commands.windows(2).all(|pair| pair[1].0 - pair[0].0 >= TCK),
        "one shared CA command per CK"
    );
    let acts: Vec<_> = commands
        .iter()
        .filter_map(|(at, c)| matches!(c, Command::Act { .. }).then_some(*at))
        .collect();
    assert_eq!(
        acts[1] - acts[0],
        4 * TCK,
        "independent bank group activation overlaps first bank latency"
    );
    let reads: Vec<_> = commands
        .iter()
        .filter_map(|(at, c)| matches!(c, Command::Read { .. }).then_some(*at))
        .collect();
    assert_eq!(
        reads[1] - reads[0],
        4 * TCK,
        "shared CAS/data bandwidth still limits independent banks"
    );
    let bytes: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::ReadByte { at_ps, request, .. } => Some((*at_ps, request.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(bytes.len(), 16);
    assert!(
        bytes
            .windows(2)
            .all(|pair| pair[1].0 - pair[0].0 >= TCK / 2),
        "shared DQ beats cannot overlap"
    );
    assert_eq!(bytes.iter().filter(|(_, id)| *id == "first").count(), 8);
    assert_eq!(bytes.iter().filter(|(_, id)| *id == "second").count(), 8);
    let first_complete = effects
        .iter()
        .find_map(|e| match e {
            Effect::RequestComplete { at_ps, request } if request == "first" => Some(*at_ps),
            _ => None,
        })
        .unwrap();
    assert!(
        reads[1] < first_complete,
        "second bank progresses before first request completes"
    );
}
