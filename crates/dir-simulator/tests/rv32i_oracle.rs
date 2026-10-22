use dir_simulator::runtime::soc_execution::cpu::{Core, Effect, evaluate, load_value};
use serde_json::Value;
use std::path::PathBuf;

const RV32I_BASE_OPCODES: &[&str] = &[
    "LUI", "AUIPC", "JAL", "JALR", "BEQ", "BNE", "BLT", "BGE", "BLTU", "BGEU", "LB", "LH", "LW",
    "LBU", "LHU", "SB", "SH", "SW", "ADDI", "SLTI", "SLTIU", "XORI", "ORI", "ANDI", "SLLI", "SRLI",
    "SRAI", "ADD", "SUB", "SLL", "SLT", "SLTU", "XOR", "SRL", "SRA", "OR", "AND", "FENCE", "ECALL",
    "EBREAK",
];

fn fixture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rv32i_spike_oracle/oracle.json");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("read pinned Spike fixture {}: {error}", path.display()));
    serde_json::from_slice(&bytes).expect("valid pinned Spike oracle JSON")
}

fn u32_array(value: &Value) -> [u32; 32] {
    let values = value.as_array().expect("register state array");
    assert_eq!(values.len(), 32);
    let mut regs = [0; 32];
    for (index, value) in values.iter().enumerate() {
        regs[index] = value.as_u64().expect("register value") as u32;
    }
    regs
}

fn writes(value: &Value) -> Vec<(u8, u32)> {
    value
        .as_array()
        .expect("oracle register writes")
        .iter()
        .map(|write| {
            (
                write["rd"].as_u64().expect("rd") as u8,
                write["value"].as_u64().expect("value") as u32,
            )
        })
        .collect()
}

fn check_record(label: &str, record: &Value) {
    let pc = record["pc"].as_u64().expect("pc") as u32;
    let word = record["word"].as_u64().expect("instruction word") as u32;
    let mut core = Core::new(pc, 0);
    core.regs = u32_array(&record["registers_before"]);
    core.regs[0] = 0;
    let actual = evaluate(&core, word)
        .unwrap_or_else(|trap| panic!("{label} at {pc:#010x}: unexpected {trap:?}"));
    let expected_next = record["next_pc"].as_u64().expect("next pc") as u32;
    let expected_writes = writes(&record["register_writes"]);
    let memory_reads: Vec<u32> = record["memory_reads"]
        .as_array()
        .expect("memory read log")
        .iter()
        .map(|address| address.as_u64().expect("load address") as u32)
        .collect();
    let memory_writes = record["memory_writes"]
        .as_array()
        .expect("memory write log");

    match &actual.effect {
        Effect::Retire { rd, next_pc } => {
            assert_eq!(*next_pc, expected_next, "{label} next PC at {pc:#010x}");
            assert!(memory_reads.is_empty(), "{label} unexpected load trace");
            assert!(memory_writes.is_empty(), "{label} unexpected store trace");
            match rd {
                Some((0, _)) => assert!(expected_writes.is_empty(), "x0 is omitted by Spike"),
                Some((rd, value)) => {
                    assert_eq!(expected_writes.len(), 1, "{label} rd write count");
                    assert_eq!(expected_writes[0], (*rd, *value), "{label} register result");
                }
                None => assert!(
                    expected_writes.is_empty(),
                    "{label} unexpected register write"
                ),
            }
        }
        Effect::Load {
            address,
            bytes,
            signed,
            rd,
            next_pc,
        } => {
            assert_eq!(*next_pc, expected_next, "{label} next PC");
            assert_eq!(memory_reads, vec![*address], "{label} memory read address");
            assert!(memory_writes.is_empty(), "{label} unexpected store trace");
            assert_eq!(
                expected_writes.len(),
                usize::from(*rd != 0),
                "{label} load rd count"
            );
            if *rd != 0 {
                assert_eq!(expected_writes[0].0, *rd, "{label} load rd");
                let memory_bytes = record["memory_bytes"]
                    .as_array()
                    .expect("loaded bytes from Spike test image");
                assert_eq!(memory_bytes.len(), *bytes, "{label} input memory width");
                let memory_bytes: Vec<u8> = memory_bytes
                    .iter()
                    .map(|byte| byte.as_u64().expect("memory byte") as u8)
                    .collect();
                assert_eq!(
                    expected_writes[0].1,
                    load_value(&memory_bytes, *signed),
                    "{label} loaded value"
                );
            }
        }
        Effect::Store {
            address,
            bytes,
            value,
            next_pc,
        } => {
            assert_eq!(*next_pc, expected_next, "{label} next PC");
            assert!(memory_reads.is_empty(), "{label} unexpected load trace");
            assert_eq!(memory_writes.len(), 1, "{label} store count");
            assert_eq!(
                memory_writes[0]["address"].as_u64().unwrap() as u32,
                *address
            );
            let store_mask = if *bytes == 4 {
                u32::MAX
            } else {
                (1u32 << (*bytes * 8)) - 1
            };
            assert_eq!(
                memory_writes[0]["value"].as_u64().unwrap() as u32,
                *value & store_mask
            );
            assert_eq!(memory_writes[0]["bytes"].as_u64().unwrap() as usize, *bytes);
            assert!(expected_writes.is_empty(), "{label} store wrote a register");
        }
        Effect::Fence { next_pc } => {
            assert_eq!(*next_pc, expected_next, "{label} next PC");
            assert!(expected_writes.is_empty(), "{label} fence wrote a register");
            assert!(memory_reads.is_empty(), "{label} fence read memory");
            assert!(memory_writes.is_empty(), "{label} fence wrote memory");
        }
    }
}

#[test]
fn canonical_rv32i_instructions_match_pinned_spike_commits() {
    let fixture = fixture();
    assert_eq!(fixture["schema"], "dir.rv32i.spike-oracle.v1");
    assert_eq!(
        fixture["oracle"]["commit"],
        "b6a84e1f3a51800d8354489b6493723742abff9a"
    );
    assert_eq!(
        fixture["oracle"]["softfloat_commit"],
        "73e3f397e84d6d2a34bded52b0a77af3f4062ac2"
    );
    assert_eq!(fixture["summary"]["base_opcode_count"], 40);
    let covered = fixture["summary"]["covered_base_opcodes"]
        .as_array()
        .expect("covered opcode names");
    assert_eq!(covered.len(), RV32I_BASE_OPCODES.len());
    for (actual, expected) in covered.iter().zip(RV32I_BASE_OPCODES) {
        assert_eq!(actual.as_str().unwrap(), *expected);
    }
    let cases = fixture["canonical_instruction_cases"]
        .as_array()
        .expect("canonical cases");
    assert_eq!(cases.len(), 38);
    for (case, expected_name) in cases.iter().zip(&RV32I_BASE_OPCODES[..38]) {
        assert_eq!(case["name"].as_str().unwrap(), *expected_name);
        check_record(case["name"].as_str().unwrap(), case);
    }
}

#[test]
fn seeded_multi_operation_sequence_matches_pinned_spike_commits() {
    let fixture = fixture();
    let trace = fixture["seeded_trace"]["cases"]
        .as_array()
        .expect("seeded trace cases");
    assert_eq!(trace.len(), 256);
    for (index, record) in trace.iter().enumerate() {
        check_record(&format!("seeded #{index}"), record);
    }
}

#[test]
fn real_checksum_program_trace_matches_pinned_spike_commits() {
    let fixture = fixture();
    let trace = fixture["real_program_trace"]["cases"]
        .as_array()
        .expect("real program trace cases");
    assert_eq!(trace.len(), 24);
    for (index, record) in trace.iter().enumerate() {
        check_record(&format!("checksum program #{index}"), record);
    }
    assert_eq!(fixture["real_program_trace"]["result"], 6);
}

#[test]
fn precise_cpu_and_memory_traps_match_spike_csrs() {
    let fixture = fixture();
    let probes = fixture["trap_probes"].as_array().expect("trap probes");
    assert_eq!(probes.len(), 10);
    for probe in probes {
        let name = probe["name"].as_str().unwrap();
        let pc = probe["pc"].as_u64().unwrap() as u32;
        let word = probe["word"].as_u64().unwrap() as u32;
        let mut core = Core::new(pc, 0);
        core.regs = u32_array(&probe["registers_before"]);
        core.regs[0] = 0;
        let expected_mepc = probe["expected_mepc"].as_u64().unwrap() as u32;
        let expected_tval = probe["expected_tval"].as_u64().unwrap() as u32;
        assert_eq!(
            probe["mepc"].as_u64().unwrap() as u32,
            expected_mepc,
            "{name} Spike mepc"
        );
        assert_eq!(
            probe["mtval"].as_u64().unwrap() as u32,
            expected_tval,
            "{name} Spike mtval"
        );
        assert_eq!(
            cause_from_mcause(probe["mcause"].as_u64().unwrap()),
            probe["cause"],
            "{name} Spike cause"
        );

        match probe["stage"].as_str().unwrap() {
            "cpu_decode" => {
                let trap = evaluate(&core, word).expect_err(name);
                assert_eq!(
                    serde_json::to_value(trap.cause).unwrap(),
                    probe["cause"],
                    "{name} cause"
                );
                assert_eq!(trap.pc, pc, "{name} trap PC");
                assert_eq!(trap.instruction, Some(word), "{name} trap instruction");
                assert_eq!(trap.tval, expected_tval, "{name} trap value");
            }
            "memory_access" => match evaluate(&core, word).expect(name).effect {
                Effect::Load {
                    address, next_pc, ..
                }
                | Effect::Store {
                    address, next_pc, ..
                } => {
                    assert_eq!(address, expected_tval, "{name} faulting memory address");
                    assert_eq!(next_pc, pc.wrapping_add(4), "{name} instruction next PC");
                }
                other => panic!("{name} expected a memory action, got {other:?}"),
            },
            "instruction_fetch" => match evaluate(&core, word).expect(name).effect {
                Effect::Retire { next_pc, .. } => {
                    assert_eq!(next_pc, expected_mepc, "{name} jump target")
                }
                other => panic!("{name} expected a jump action, got {other:?}"),
            },
            stage => panic!("{name}: unknown trap stage {stage}"),
        }
    }
}

#[test]
fn sequential_pc_wrap_matches_spike() {
    let probe = fixture()["pc_wrap_probe"].clone();
    let pc = probe["pc"].as_u64().unwrap() as u32;
    let word = probe["word"].as_u64().unwrap() as u32;
    let core = Core::new(pc, 0);
    let action = evaluate(&core, word).expect("ADDI retires at the top of RV32 address space");
    assert_eq!(probe["next_pc"].as_u64().unwrap() as u32, 0);
    assert_eq!(probe["rd"].as_u64().unwrap(), 3);
    assert_eq!(probe["value"].as_u64().unwrap(), 1);
    assert_eq!(
        action.effect,
        Effect::Retire {
            rd: Some((3, 1)),
            next_pc: 0,
        }
    );
}

fn cause_from_mcause(mcause: u64) -> Value {
    let name = match mcause {
        0 => "instruction_address_misaligned",
        1 => "instruction_access_fault",
        2 => "illegal_instruction",
        3 => "breakpoint",
        4 => "load_address_misaligned",
        5 => "load_access_fault",
        6 => "store_address_misaligned",
        7 => "store_access_fault",
        11 => "environment_call",
        _ => panic!("unexpected Spike mcause {mcause}"),
    };
    Value::String(name.to_owned())
}
