use dir_simulator::output::soc_execution::execution_records;
use dir_simulator::registry::Registry;
use dir_simulator::runtime::session::*;
use dir_simulator::{prepare, runtime};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(
        config: Value,
        requests: Vec<Value>,
        dma: Vec<Value>,
        commands: Vec<Value>,
        limit: u64,
    ) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-soc-execution-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/soc")).unwrap();
        std::fs::write(root.join("ned/soc/model.ned"),"package soc; simple Execution { parameters: @class(\"dir.soc.Execution\"); } network Demo { submodules: soc: soc.Execution; }").unwrap();
        std::fs::write(
            root.join("model.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("workload.json"),serde_json::to_vec(&json!({"schema":"dir.soc-execution.workload","version":1,"requests":requests,"dma":dma,"commands":commands})).unwrap()).unwrap();
        std::fs::write(root.join("scenario.ini"),format!("[General]\nnetwork=soc.Demo\nned-path=\"ned\"\nmodel-profile=\"soc.execution.v1\"\nmodel-config=\"model.json\"\nworkload=\"workload.json\"\nsim-time-limit={limit}ps\nmetrics-window=1ps\n")).unwrap();
        Self(root)
    }
    fn prepared(&self) -> dir_simulator::PreparedSimulation {
        prepare(&self.0.join("scenario.ini")).unwrap()
    }
    fn run(&self) -> dir_simulator::snapshot::Snapshot {
        runtime::simulate(&self.prepared()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn config() -> Value {
    json!({"schema":"dir.soc-execution.model","version":1,"cycle_ps":1,"memory":[{"id":"ram","base":0,"size":1024,"readable":true,"writable":true,"executable":true}],"cores":[],"cache":{"line_bytes":16,"sets":4,"ways":2,"mshrs":2,"ack_cycles":3},"fabric":{"queue_capacity":4,"aging_cycles":2,"data_bytes":4,"axi_address_delay_cycles":1,"ahb_wait_cycles":2},"noc":{"schema":"dir.noc-flit.model","version":1,"topology":{"kind":"mesh","width":2,"height":2},"flit_bytes":4,"buffer_flits":1,"adaptive_vcs":1,"cycle_ps":1,"link_latency_cycles":2,"credit_latency_cycles":2,"escape_after_stalls":2,"ejection_capacity_flits":1,"ejection_service_cycles":1,"pending_packets":4},"memory_router":3,"dma_buffer_bytes":3,"slots":[]})
}
#[allow(clippy::too_many_arguments)] // Independent fixture parameters mirror the public request.
fn request(
    id: &str,
    source: &str,
    transport: &str,
    op: &str,
    at: u64,
    address: u32,
    length: usize,
    data: &[u8],
) -> Value {
    json!({"id":id,"source":source,"transport":transport,"operation":op,"at_ps":at,"address":address,"length":length,"data":data,"router":0})
}
fn events(s: &dir_simulator::snapshot::Snapshot) -> Vec<(u64, Value)> {
    execution_records(s.registered.as_ref().unwrap())
        .into_iter()
        .map(|r| (r.time_ps, r.data.clone()))
        .collect()
}
fn kind<'a>(e: &'a [(u64, Value)], k: &str) -> Vec<&'a (u64, Value)> {
    e.iter().filter(|(_, v)| v["kind"] == k).collect()
}
fn completed<'a>(e: &'a [(u64, Value)], id: &str) -> &'a Value {
    &kind(e, "request_completed")
        .into_iter()
        .find(|(_, v)| v["request_id"] == id)
        .unwrap()
        .1["detail"]
}
fn i(imm: i32, rs: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (((imm as u32) & 4095) << 20) | (rs << 15) | (f3 << 12) | (rd << 7) | op
}
fn store(imm: i32, rs2: u32, rs1: u32) -> u32 {
    let n = imm as u32 & 4095;
    ((n >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((n & 31) << 7) | 0x23
}
fn branch(imm: i32, rs2: u32, rs1: u32) -> u32 {
    let n = imm as u32 & 8191;
    (((n >> 12) & 1) << 31)
        | (((n >> 5) & 63) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (((n >> 1) & 15) << 8)
        | (((n >> 11) & 1) << 7)
        | 0x63
}
fn program(c: &mut Value, at: usize, words: &[u32]) {
    let mut bytes: Vec<u8> =
        serde_json::from_value(c["memory"][0]["initial"].clone()).unwrap_or_default();
    bytes.resize(bytes.len().max(at + words.len() * 4), 0);
    for (k, w) in words.iter().enumerate() {
        bytes[at + k * 4..at + k * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    c["memory"][0]["initial"] = json!(bytes);
}
fn core(id: &str, pc: u32, transport: &str, router: usize) -> Value {
    json!({"id":id,"reset_pc":pc,"initial_sp":1008,"instruction_budget":200,"cycle_cycles":1,"execute_cycles":1,"transport":transport,"router":router})
}
#[test]
fn three_real_adapters_share_one_byte_owner_and_preserve_errors() {
    for transport in ["axi", "ahb", "noc"] {
        let mut write = request(
            "write",
            "agent",
            transport,
            "write",
            0,
            128,
            6,
            &[1, 2, 3, 4, 5, 6],
        );
        write["byte_enables"] = json!([true, false, true, true, false, true]);
        let f = Fixture::new(
            config(),
            vec![
                write,
                request("read", "agent", transport, "read", 1, 128, 6, &[]),
                request("bad", "agent", transport, "read", 2, 1022, 4, &[]),
            ],
            vec![],
            vec![],
            1000,
        );
        let s = f.run();
        let e = events(&s);
        assert_eq!(s.common.termination, "events_exhausted");
        assert_eq!(completed(&e, "read")["data"], json!([1, 0, 3, 4, 0, 6]));
        assert_ne!(completed(&e, "bad")["status"], "okay");
        assert!(kind(&e, "memory_final").len() == 1);
        if transport == "noc" {
            assert!(!kind(&e, "flit_launched").is_empty());
            assert!(!kind(&e, "packet_delivered").is_empty());
        }
    }
}
#[test]
fn actual_rv32_program_two_core_handoff_fence_dirty_intervention_and_dma() {
    let mut c = config();
    c["cores"] = json!([core("cpu0", 0, "axi", 0), core("cpu1", 64, "noc", 1)]);
    program(
        &mut c,
        0,
        &[
            i(256, 0, 0, 1, 0x13),
            i(42, 0, 0, 2, 0x13),
            store(0, 2, 1),
            0x0ff0000f,
            0x73,
        ],
    );
    program(
        &mut c,
        64,
        &[
            i(256, 0, 0, 1, 0x13),
            i(0, 1, 2, 3, 0x03),
            branch(-4, 0, 3),
            i(1, 3, 0, 3, 0x13),
            store(4, 3, 1),
            0x73,
        ],
    );
    let dma = json!({"id":"copy","source":"dma0","at_ps":600,"source_address":256,"destination":512,"length":8,"transport":"ahb","router":2});
    let f = Fixture::new(
        c,
        vec![request(
            "observe",
            "observer",
            "axi",
            "read",
            900,
            512,
            8,
            &[],
        )],
        vec![dma],
        vec![],
        2000,
    );
    let s = f.run();
    let e = events(&s);
    assert_eq!(s.common.termination, "events_exhausted");
    assert_eq!(
        completed(&e, "observe")["data"],
        json!([42, 0, 0, 0, 43, 0, 0, 0])
    );
    assert_eq!(kind(&e, "cpu_trap").len(), 2);
    assert!(
        kind(&e, "cpu_trap")
            .iter()
            .all(|(_, v)| v["detail"]["trap"]["cause"] == "environment_call")
    );
    assert!(!kind(&e, "cpu_retired").is_empty());
    assert!(!kind(&e, "coherence_ack").is_empty());
    assert!(!kind(&e, "cache_writeback").is_empty());
    assert!(
        kind(&e, "coherence_ack")
            .iter()
            .any(|(_, v)| !v["detail"]["dirty_data"].is_null())
    );
}
#[test]
fn traps_do_not_commit_register_pc_or_faulting_memory() {
    let mut c = config();
    c["cores"] = json!([core("cpu", 0, "axi", 0)]);
    program(
        &mut c,
        0,
        &[
            i(129, 0, 0, 1, 0x13),
            i(9, 0, 0, 2, 0x13),
            store(0, 2, 1),
            0x73,
        ],
    );
    let f = Fixture::new(
        c,
        vec![request("read", "observer", "axi", "read", 100, 128, 4, &[])],
        vec![],
        vec![],
        200,
    );
    let e = events(&f.run());
    assert_eq!(kind(&e, "cpu_retired").len(), 2);
    let t = &kind(&e, "cpu_trap")[0].1["detail"]["trap"];
    assert_eq!(t["cause"], "store_address_misaligned");
    assert_eq!(t["pc"], 8);
    assert_eq!(completed(&e, "read")["data"], json!([0, 0, 0, 0]));
}
#[test]
fn axi5_all_operations_signed_edges_compare_layout_and_independent_responses() {
    let old = 0x80000000u32;
    let operand = 0x7fffffffu32;
    let expected = [
        old.wrapping_add(operand),
        old & !operand,
        old ^ operand,
        old | operand,
        operand,
        old,
        old,
        operand,
    ];
    for base in [0x10u8, 0x20] {
        for (op, want) in expected.iter().enumerate() {
            let mut c = config();
            let mut initial = vec![0; 132];
            initial[128..132].copy_from_slice(&old.to_le_bytes());
            c["memory"][0]["initial"] = json!(initial);
            let mut atomic = request(
                "atomic",
                "a",
                "axi",
                "atomic",
                0,
                128,
                4,
                &operand.to_le_bytes(),
            );
            atomic["atomic"] = json!({"awatop":base+op as u8,"burst":"incr"});
            atomic["r_stall_cycles"] = json!(7);
            atomic["b_stall_cycles"] = json!(1);
            let f = Fixture::new(
                c,
                vec![atomic, request("read", "a", "axi", "read", 1, 128, 4, &[])],
                vec![],
                vec![],
                100,
            );
            let e = events(&f.run());
            assert_eq!(completed(&e, "read")["data"], json!(want.to_le_bytes()));
            let r: Vec<_> = kind(&e, "axi_r")
                .into_iter()
                .filter(|(_, v)| v["request_id"] == "atomic")
                .collect();
            assert_eq!(r.len(), usize::from(base == 0x20));
            if base == 0x20 {
                assert_eq!(r[0].1["detail"]["data"], json!(old.to_le_bytes()));
                let b = kind(&e, "axi_b")
                    .into_iter()
                    .find(|(_, v)| v["request_id"] == "atomic")
                    .unwrap();
                assert!(b.0 < r[0].0);
            }
            assert_eq!(kind(&e, "atomic_linearized").len(), 1);
        }
    }
    for (address, match_compare) in [(128, true), (128, false), (132, true), (132, false)] {
        let mut c = config();
        let mut initial = vec![0; 136];
        initial[address..address + 4].copy_from_slice(&5u32.to_le_bytes());
        c["memory"][0]["initial"] = json!(initial);
        let compare = if match_compare { 5u32 } else { 6 };
        let mut data = if address % 8 == 0 {
            compare.to_le_bytes().to_vec()
        } else {
            9u32.to_le_bytes().to_vec()
        };
        data.extend_from_slice(&if address % 8 == 0 {
            9u32.to_le_bytes()
        } else {
            compare.to_le_bytes()
        });
        let mut a = request("cas", "a", "axi", "atomic", 0, address as u32, 4, &data);
        a["atomic"] = json!({"awatop":49,"burst":if address%8==0{"incr"}else{"wrap"}});
        let f = Fixture::new(
            c,
            vec![
                a,
                request("read", "a", "axi", "read", 1, address as u32, 4, &[]),
            ],
            vec![],
            vec![],
            100,
        );
        let e = events(&f.run());
        assert_eq!(completed(&e, "cas")["data"], json!([5, 0, 0, 0]));
        assert_eq!(
            completed(&e, "read")["data"],
            json!([if match_compare { 9 } else { 5 }, 0, 0, 0])
        );
    }
    let mut c = config();
    c["memory"][0]["initial"] = json!(vec![0; 132]);
    let mut swap = request("swap", "a", "axi", "atomic", 0, 128, 4, &7u32.to_le_bytes());
    swap["atomic"] = json!({"awatop":48,"burst":"incr"});
    swap["r_stall_cycles"] = json!(10);
    let f = Fixture::new(
        c,
        vec![
            swap,
            request(
                "interfere",
                "b",
                "axi",
                "write",
                1,
                128,
                4,
                &9u32.to_le_bytes(),
            ),
            request("read", "a", "axi", "read", 2, 128, 4, &[]),
        ],
        vec![],
        vec![],
        100,
    );
    let e = events(&f.run());
    assert_eq!(completed(&e, "swap")["data"], json!([0, 0, 0, 0]));
    assert_eq!(completed(&e, "read")["data"], json!([9, 0, 0, 0]));
    assert_eq!(kind(&e, "atomic_linearized").len(), 1);
}
fn lease() -> Value {
    json!({"slot":"slot","generation":1,"owner":"producer"})
}
#[test]
fn dma_leased_slot_publishes_actual_completed_bytes_and_rejects_stale_early_release() {
    let mut c = config();
    let mut initial = vec![0; 136];
    initial[128..136].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    c["memory"][0]["initial"] = json!(initial);
    c["slots"] = json!([{"id":"slot","base":512,"capacity":8}]);
    let mut read = request("consume", "consumer", "noc", "read", 100, 512, 8, &[]);
    read["lease"] = lease();
    let dma = json!({"id":"fill","source":"producer","at_ps":0,"source_address":128,"destination":512,"length":8,"transport":"axi","router":0,"lease":lease(),"publish":true});
    let commands = vec![
        json!({"id":"lease","at_ps":0,"operation":"lease","slot":"slot","owner":"producer","length":8}),
        json!({"id":"early","at_ps":3,"operation":"publish","lease":lease()}),
        json!({"id":"duplicate","at_ps":90,"operation":"publish","lease":lease()}),
        json!({"id":"begin","at_ps":100,"operation":"begin_read","lease":lease(),"consumer":"consumer"}),
        json!({"id":"early-release","at_ps":101,"operation":"release","lease":lease(),"consumer":"consumer"}),
        json!({"id":"release","at_ps":300,"operation":"release","lease":lease(),"consumer":"consumer"}),
        json!({"id":"reuse","at_ps":301,"operation":"lease","slot":"slot","owner":"producer","length":8}),
        json!({"id":"stale","at_ps":302,"operation":"publish","lease":lease()}),
    ];
    let f = Fixture::new(
        c.clone(),
        vec![read],
        vec![dma.clone()],
        commands.clone(),
        400,
    );
    let e = events(&f.run());
    assert_eq!(
        completed(&e, "consume")["data"],
        json!([1, 2, 3, 4, 5, 6, 7, 8])
    );
    assert_eq!(kind(&e, "slot_published").len(), 1);
    assert_eq!(kind(&e, "slot_notification").len(), 1);
    assert_eq!(kind(&e, "dma_destination_completed").len(), 3);
    assert_eq!(kind(&e, "slot_command_rejected").len(), 4);
    let last = kind(&e, "dma_destination_completed").last().unwrap().0;
    assert!(kind(&e, "slot_published")[0].0 >= last);
    let partial = Fixture::new(c, vec![], vec![dma], commands[..2].to_vec(), 14);
    let e = events(&partial.run());
    assert!(kind(&e, "slot_published").is_empty());
    assert_eq!(kind(&e, "dma_destination_completed").len(), 1);
}
#[test]
fn finite_mshr_conflicts_dirty_eviction_and_resume_transients() {
    let mut c = config();
    c["cache"]["sets"] = json!(1);
    c["cache"]["ways"] = json!(1);
    c["cache"]["mshrs"] = json!(1);
    c["fabric"]["queue_capacity"] = json!(2);
    c["cores"] = json!([core("cpu0", 0, "axi", 0), core("cpu1", 64, "ahb", 1)]);
    program(
        &mut c,
        0,
        &[
            i(256, 0, 0, 1, 0x13),
            i(11, 0, 0, 2, 0x13),
            store(0, 2, 1),
            i(0, 1, 2, 3, 0x03),
            0x73,
        ],
    );
    program(
        &mut c,
        64,
        &[
            i(256, 0, 0, 1, 0x13),
            i(22, 0, 0, 2, 0x13),
            store(0, 2, 1),
            i(0, 1, 2, 3, 0x03),
            0x73,
        ],
    );
    let f = Fixture::new(c, vec![], vec![], vec![], 1000);
    let baseline = f.run();
    let expected = events(&baseline);
    assert_eq!(kind(&expected, "cpu_trap").len(), 2);
    assert!(!kind(&expected, "cache_eviction_completed").is_empty());
    assert!(!kind(&expected, "cache_writeback").is_empty());
    for steps in [2, 8, 15, 30, 50, 80] {
        let (mut session, ctl) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..steps {
            if session.state() == RunState::Completed {
                break;
            }
            session.advance_one().unwrap();
        }
        if session.state() == RunState::Completed {
            continue;
        }
        ctl.submit(ControlCommand::Pause).unwrap();
        assert_eq!(session.poll_control().unwrap(), RunState::Paused);
        let path = f.0.join(format!("checkpoint-{steps}"));
        assert_eq!(
            session
                .checkpoint(&path, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut resumed, token) =
            restore_checkpoint(&path, Registry::default(), CheckpointLimits::default()).unwrap();
        assert_eq!(events(resumed.snapshot()), events(session.snapshot()));
        resumed.resume(token).unwrap();
        assert_eq!(resumed.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(events(resumed.snapshot()), expected);
    }
}
#[test]
fn qos_aging_and_admission_rotation_drain_finite_saturated_queue() {
    let mut c = config();
    c["fabric"]["queue_capacity"] = json!(2);
    c["cache"]["mshrs"] = json!(1);
    let mut requests = vec![request("low", "low", "axi", "read", 0, 128, 4, &[])];
    for n in 0..16 {
        let mut r = request(
            &format!("high{n:02}"),
            "high",
            "axi",
            "read",
            0,
            128,
            4,
            &[],
        );
        r["qos"] = json!(15);
        requests.push(r);
    }
    let f = Fixture::new(c, requests, vec![], vec![], 500);
    let e = events(&f.run());
    assert_eq!(kind(&e, "request_completed").len(), 17);
    let low = kind(&e, "request_admitted")
        .into_iter()
        .find(|(_, v)| v["request_id"] == "low")
        .unwrap();
    assert!(low.0 <= 1);
    assert!(
        kind(&e, "owner_grant")
            .iter()
            .all(|(_, v)| v["detail"]["effective_qos"].as_u64().unwrap() <= 15)
    );
    assert_eq!(completed(&e, "low")["status"], "okay");
}
fn mutate_checkpoint(
    source: &std::path::Path,
    destination: &std::path::Path,
    change: impl FnOnce(&mut Value),
) {
    use dir_simulator::runtime::checkpoint_bundle::{read_bundle, write_bundle};
    use sha2::{Digest, Sha256};
    let (mut manifest, mut files) = read_bundle(source, &CheckpointLimits::default()).unwrap();
    let mut state: Value = serde_json::from_slice(&files["state.json"]).unwrap();
    let reference = state["codec_refs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["codec_name"] == "dir.soc.execution.state")
        .unwrap();
    let old = reference["blob_path"].as_str().unwrap().to_string();
    let mut model: Value = serde_json::from_slice(&files[&old]).unwrap();
    change(&mut model);
    let bytes = serde_json::to_vec(&model).unwrap();
    let path = format!("objects/{:x}.bin", Sha256::digest(&bytes));
    reference["blob_path"] = json!(path);
    files.remove(&old);
    files.insert(path, bytes);
    files.insert("state.json".into(), serde_json::to_vec(&state).unwrap());
    manifest["codecs"] = state["codec_refs"].clone();
    let ordinal = manifest["checkpoint_ordinal"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    write_bundle(
        destination,
        manifest,
        files,
        &CheckpointLimits::default(),
        ordinal,
    )
    .unwrap();
}
#[test]
fn checkpoint_rejects_fetch_dma_ack_and_timer_tampering_without_panicking() {
    let mut c = config();
    c["cores"] = json!([core("cpu", 0, "axi", 0)]);
    program(
        &mut c,
        0,
        &[i(128, 0, 0, 1, 0x13), i(0, 1, 2, 2, 0x03), 0x73],
    );
    let dma = json!({"id":"copy","source":"dma","at_ps":0,"source_address":128,"destination":512,"length":8,"transport":"ahb","router":1});
    let f = Fixture::new(c, vec![], vec![dma], vec![], 500);
    let (mut session, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..8 {
        session.advance_one().unwrap();
    }
    ctl.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let source = f.0.join("good");
    session
        .checkpoint(&source, CheckpointLimits::default())
        .unwrap();
    type CheckpointMutation = (&'static str, fn(&mut Value));
    let checks: [CheckpointMutation; 5] = [
        ("fetch", |v| {
            let txn = v["state"]["cores"][0]["phase"]["txn"].as_u64().unwrap() as usize;
            v["state"]["transactions"][txn]["request"]["length"] = json!(1);
        }),
        ("dma", |v| {
            let txn = v["state"]["transactions"]
                .as_array()
                .unwrap()
                .iter()
                .position(|t| {
                    matches!(t["client"]["kind"].as_str(), Some("dma_read" | "dma_write"))
                })
                .unwrap();
            v["state"]["transactions"][txn]["request"]["address"] = json!(129);
        }),
        ("x0", |v| {
            v["state"]["cores"][0]["architectural"]["regs"][0] = json!(1)
        }),
        (
            "owner",
            |v| {
                v["state"]["directory"]["128"] =
                    json!({"owner":9,"sharers":[],"epoch":0,"transient":null})
            },
        ),
        ("timer", |v| v["state"]["next_tick"] = json!(100)),
    ];
    for (name, change) in checks {
        let path = f.0.join(name);
        mutate_checkpoint(&source, &path, change);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_checkpoint(&path, Registry::default(), CheckpointLimits::default())
        }));
        assert!(result.is_ok(), "panic for {name}");
        assert!(result.unwrap().is_err(), "accepted {name}");
    }
}
#[test]
fn overlapping_slots_and_illegal_atomic_shapes_reject_preparation() {
    let mut c = config();
    c["slots"] = json!([{"id":"a","base":512,"capacity":8},{"id":"b","base":516,"capacity":8}]);
    let f = Fixture::new(c, vec![], vec![], vec![], 10);
    assert!(prepare(&f.0.join("scenario.ini")).is_err());
    for (atop, address, burst) in [
        (0x28, 128, "incr"),
        (0x31, 132, "incr"),
        (0x31, 128, "wrap"),
        (0x30, 129, "incr"),
    ] {
        let mut a = request("a", "agent", "axi", "atomic", 0, address, 4, &[0; 8]);
        a["atomic"] = json!({"awatop":atop,"burst":burst});
        let f = Fixture::new(config(), vec![a], vec![], vec![], 20);
        assert!(prepare(&f.0.join("scenario.ini")).is_err());
    }
}
fn ddr_config() -> Value {
    let mut c = config();
    c["memory"].as_array_mut().unwrap().push(json!({"id":"ddr","base":4096,"size":1024,"readable":true,"writable":true,"executable":false,"backend":"ddr_strict"}));
    c["ddr"] = json!({"mode_id":runtime::ddr4::MODE,"source_sha256":runtime::ddr4::SOURCE_SHA256,"base":4096,"size":1024,"queue_capacity":4});
    c
}
#[test]
fn actual_ddr_bytes_gate_axi_ahb_noc_responses_without_initialization_polling() {
    for transport in ["axi", "ahb", "noc"] {
        let c = ddr_config();
        let at = 710_000_000;
        let requests = vec![
            request(
                "write",
                "writer",
                transport,
                "write",
                0,
                4096,
                4,
                &[11, 22, 33, 44],
            ),
            request("read", "reader", transport, "read", at, 4096, 4, &[]),
            request(
                "undefined",
                "reader2",
                transport,
                "read",
                at + 2_000_000,
                4128,
                4,
                &[],
            ),
        ];
        let f = Fixture::new(c, requests, vec![], vec![], 730_000_000);
        let (mut session, _) = RunSession::new(f.prepared(), RunOptions::default()).unwrap();
        let mut count = 0;
        while session.state() != RunState::Completed && count < 5000 {
            session.advance_one().unwrap();
            count += 1;
        }
        assert_eq!(
            session.state(),
            RunState::Completed,
            "{transport} polled initialization: {count}; common={:?}; tail={:?}",
            session.snapshot().common,
            events(session.snapshot())
                .into_iter()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
        );
        let snap = session.snapshot();
        assert_ne!(
            snap.common.termination, "execution_failed",
            "{transport}: {:?}",
            snap.common.diagnostics
        );
        let e = events(snap);
        assert_eq!(
            completed(&e, "read")["data"],
            json!([11, 22, 33, 44]),
            "{transport}"
        );
        assert_eq!(completed(&e, "read")["status"], "okay");
        assert_ne!(completed(&e, "undefined")["status"], "okay");
        let bytes: Vec<_> = kind(&e, "ddr_effect")
            .into_iter()
            .filter(|(_, v)| {
                v["detail"]["effect"]["kind"] == "write_byte"
                    && v["detail"]["effect"]["byte_enable"] == true
            })
            .collect();
        assert_eq!(bytes.len(), 4);
        let last = bytes.iter().map(|(at, _)| *at).max().unwrap();
        let completed_at = kind(&e, "request_completed")
            .into_iter()
            .find(|(_, v)| v["request_id"] == "write")
            .unwrap()
            .0;
        assert!(last <= completed_at);
        assert!(last > 700_000_000);
        assert!(count < 1000, "{transport} excess events {count}");
    }
}
#[test]
fn ddr_initialization_and_byte_transient_checkpoint_resume_and_tamper() {
    let c = ddr_config();
    let f = Fixture::new(
        c,
        vec![
            request(
                "write",
                "writer",
                "axi",
                "write",
                0,
                4096,
                8,
                &[1, 2, 3, 4, 5, 6, 7, 8],
            ),
            request("read", "reader", "ahb", "read", 712_000_000, 4096, 8, &[]),
        ],
        vec![],
        vec![],
        725_000_000,
    );
    let expected = events(&f.run());
    for steps in [4, 12, 22, 35] {
        let (mut session, ctl) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..steps {
            session.advance_one().unwrap();
        }
        ctl.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        let good = f.0.join(format!("good-ddr-{steps}"));
        session
            .checkpoint(&good, CheckpointLimits::default())
            .unwrap();
        let (mut resumed, token) =
            restore_checkpoint(&good, Registry::default(), CheckpointLimits::default()).unwrap();
        resumed.resume(token).unwrap();
        resumed.run_until_yield().unwrap();
        assert_eq!(events(resumed.snapshot()), expected, "boundary {steps}");
        let bad = f.0.join(format!("bad-ddr-{steps}"));
        mutate_checkpoint(&good, &bad, |v| {
            let old = v["state"]["ddr"]["valid"][0].as_bool().unwrap();
            v["state"]["ddr"]["valid"][0] = json!(!old);
        });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restore_checkpoint(&bad, Registry::default(), CheckpointLimits::default())
        }));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
}
#[test]
fn ddr_dirty_intervention_coherent_dma_and_atomic_share_physical_byte_owner() {
    let mut c = ddr_config();
    let mut cpu = core("cpu", 0, "axi", 0);
    cpu["cycle_cycles"] = json!(10_000_000);
    c["cores"] = json!([cpu]);
    program(
        &mut c,
        0,
        &[
            0x000010b7,
            i(42, 0, 0, 2, 0x13),
            store(0, 2, 1),
            i(0, 1, 2, 3, 0x03),
            0x0f,
            0x73,
        ],
    );
    let mut atomic = request(
        "atomic",
        "atomic",
        "axi",
        "atomic",
        760_000_000,
        4096,
        4,
        &1u32.to_le_bytes(),
    );
    atomic["atomic"] = json!({"awatop":0x20,"burst":"incr"});
    let dma = json!({"id":"copy","source":"dma","at_ps":720_000_000,"source_address":4096,"destination":512,"length":4,"transport":"noc","router":0});
    let f = Fixture::new(
        c,
        vec![
            request(
                "initialize",
                "writer",
                "axi",
                "write",
                0,
                4096,
                16,
                &[0; 16],
            ),
            atomic,
            request(
                "observe",
                "observer",
                "ahb",
                "read",
                780_000_000,
                4096,
                4,
                &[],
            ),
        ],
        vec![dma],
        vec![],
        820_000_000,
    );
    let snap = f.run();
    assert_ne!(
        snap.common.termination, "execution_failed",
        "{:?}",
        snap.common.diagnostics
    );
    let e = events(&snap);
    assert_eq!(completed(&e, "atomic")["data"], json!([42, 0, 0, 0]));
    assert_eq!(completed(&e, "observe")["data"], json!([43, 0, 0, 0]));
    assert_eq!(
        kind(&e, "dma_bytes_captured").last().unwrap().1["detail"]["data"],
        json!([0])
    );
    let captures: Vec<_> = kind(&e, "dma_bytes_captured")
        .into_iter()
        .flat_map(|(_, v)| {
            v["detail"]["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(captures, vec![42, 0, 0, 0]);
    assert!(!kind(&e, "cache_writeback").is_empty());
    assert_eq!(kind(&e, "cpu_trap").len(), 1);
    let writeback: Vec<_> = kind(&e, "ddr_effect")
        .into_iter()
        .filter(|(_, v)| {
            v["detail"]["effect"]["kind"] == "write_byte"
                && v["detail"]["effect"]["request"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("ack:") || s.starts_with("intervention:"))
        })
        .collect();
    assert!(
        writeback
            .iter()
            .any(|(_, v)| v["detail"]["effect"]["value"] == 42)
    );
}
#[test]
fn ddr_reset_invalidates_bytes_and_aborts_active_native_request() {
    for reset_at in [700_000_000, 710_000_000] {
        let reset_at = reset_at / runtime::ddr4::TCK * runtime::ddr4::TCK;
        let f = Fixture::new(
            ddr_config(),
            vec![
                request("write", "writer", "axi", "write", 0, 4096, 4, &[1, 2, 3, 4]),
                request(
                    "read",
                    "reader",
                    "ahb",
                    "read",
                    reset_at + 1_000_000,
                    4096,
                    4,
                    &[],
                ),
            ],
            vec![],
            vec![
                json!({"id":"reset","at_ps":reset_at,"operation":"ddr_control","command":{"kind":"reset","asserted":true}}),
            ],
            1_450_000_000,
        );
        let snap = f.run();
        assert_ne!(
            snap.common.termination, "execution_failed",
            "reset {reset_at}: {:?}",
            snap.common.diagnostics
        );
        let e = events(&snap);
        assert_ne!(completed(&e, "read")["status"], "okay");
        if reset_at < 705_000_000 {
            assert_ne!(completed(&e, "write")["status"], "okay");
        }
        assert!(
            kind(&e, "ddr_effect")
                .iter()
                .any(|(_, v)| v["detail"]["effect"]["kind"] == "reset_invalidation")
        );
    }
}
#[test]
fn coherent_writer_invalidating_a_queued_hit_forces_actual_refill() {
    for transport in ["axi", "ahb", "noc"] {
        let mut c = config();
        c["cores"] = json!([core("cpu", 0, transport, 0)]);
        program(
            &mut c,
            0,
            &[
                i(128, 0, 0, 1, 0x13),
                i(0, 1, 2, 2, 0x03),
                i(0, 1, 2, 3, 0x03),
                0x73,
            ],
        );
        let baseline = Fixture::new(c.clone(), vec![], vec![], vec![], 2000);
        let e = events(&baseline.run());
        let at = kind(&e, "request_admitted")
            .into_iter()
            .find(|(_, v)| v["request_id"] == "cpu:2:load")
            .unwrap()
            .0;
        let mut write = request(
            "interference",
            "writer",
            "axi",
            "write",
            at,
            128,
            4,
            &[99, 0, 0, 0],
        );
        write["qos"] = json!(15);
        let f = Fixture::new(c, vec![write], vec![], vec![], 2500);
        let snap = f.run();
        assert_ne!(
            snap.common.termination, "execution_failed",
            "{transport}: {:?}",
            snap.common.diagnostics
        );
        let e = events(&snap);
        assert_eq!(
            completed(&e, "cpu:2:load")["data"],
            json!([99, 0, 0, 0]),
            "{transport}"
        );
        assert!(
            kind(&e, "cache_fill")
                .into_iter()
                .any(|(_, v)| v["request_id"] == "cpu:2:load"),
            "{transport} omitted genuine refill"
        );
        assert!(
            !kind(&e, "cache_hit")
                .into_iter()
                .any(|(_, v)| v["request_id"] == "cpu:2:load"),
            "{transport} claimed stale hit"
        );
    }
}
fn assert_bad_restore(
    source: &std::path::Path,
    destination: &std::path::Path,
    change: impl FnOnce(&mut Value),
) {
    mutate_checkpoint(source, destination, change);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        restore_checkpoint(
            destination,
            Registry::default(),
            CheckpointLimits::default(),
        )
    }));
    assert!(
        result.is_ok(),
        "restore panicked: {}",
        destination.display()
    );
    assert!(
        result.unwrap().is_err(),
        "accepted corruption: {}",
        destination.display()
    );
}
#[test]
fn checkpoint_rejects_short_cache_response_and_fabric_capture_and_ack_corruption() {
    let mut c = config();
    c["cores"] = json!([core("cpu", 0, "axi", 0)]);
    program(&mut c, 0, &[i(1, 0, 0, 1, 0x13), i(2, 0, 0, 2, 0x13), 0x73]);
    let f = Fixture::new(c, vec![], vec![], vec![], 500);
    for cache_hit in [false, true] {
        let (mut session, ctl) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut found = false;
        for _ in 0..100 {
            session.advance_one().unwrap();
            let e = events(session.snapshot());
            found = if cache_hit {
                kind(&e, "cache_hit")
                    .into_iter()
                    .any(|(_, v)| v["request_id"] == "cpu:1:fetch")
            } else {
                e.iter()
                    .any(|(_, v)| v["channel"] == "R" && v["detail"]["last"] == true)
            };
            if found {
                break;
            }
        }
        assert!(found);
        ctl.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        let source = f.0.join(format!("good-response-{cache_hit}"));
        session
            .checkpoint(&source, CheckpointLimits::default())
            .unwrap();
        if cache_hit {
            assert_bad_restore(&source, &f.0.join("short-response"), |v| {
                let txn = v["state"]["cores"][0]["phase"]["txn"].as_u64().unwrap() as usize;
                assert_eq!(v["state"]["transactions"][txn]["phase"], "response");
                v["state"]["transactions"][txn]["result"] = json!([]);
            });
        } else {
            assert_bad_restore(&source, &f.0.join("injected-capture"), |v| {
                v["state"]["native_deliveries"][0][2][0] = json!(255);
            });
            assert_bad_restore(&source, &f.0.join("lost-native-job"), |v| {
                v["state"]["axi"]["transfers"] = json!([]);
            });
        }
    }
    let f = Fixture::new(
        config(),
        vec![request(
            "write",
            "writer",
            "axi",
            "write",
            0,
            128,
            4,
            &[1, 2, 3, 4],
        )],
        vec![],
        vec![],
        100,
    );
    let (mut session, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..3 {
        session.advance_one().unwrap();
    }
    ctl.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let source = f.0.join("good-write");
    session
        .checkpoint(&source, CheckpointLimits::default())
        .unwrap();
    assert_bad_restore(&source, &f.0.join("short-write"), |v| {
        v["state"]["transactions"][0]["request"]["data"] = json!([]);
    });
    let mut c = config();
    c["cores"] = json!([core("cpu0", 0, "axi", 0), core("cpu1", 0, "axi", 1)]);
    program(&mut c, 0, &[0x73]);
    let f = Fixture::new(c, vec![], vec![], vec![], 300);
    let (mut session, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut found = false;
    for _ in 0..100 {
        session.advance_one().unwrap();
        found = !kind(&events(session.snapshot()), "intervention_sent").is_empty();
        if found {
            break;
        }
    }
    assert!(found);
    ctl.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let source = f.0.join("good-ack");
    session
        .checkpoint(&source, CheckpointLimits::default())
        .unwrap();
    for duplicate in [false, true] {
        assert_bad_restore(&source, &f.0.join(format!("bad-ack-{duplicate}")), |v| {
            let txn = v["state"]["transactions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|t| !t["acknowledgments"].as_array().unwrap().is_empty())
                .unwrap();
            let a = txn["acknowledgments"].as_array_mut().unwrap();
            if duplicate {
                a.push(a[0].clone());
            } else {
                a[0]["epoch"] = json!(999);
            }
        });
    }
}
#[test]
fn ddr_reset_revokes_ready_slot_without_publishing_lost_bytes() {
    let mut c = ddr_config();
    c["slots"] = json!([{"id":"ddr-slot","base":4096,"capacity":4}]);
    let lease = json!({"slot":"ddr-slot","generation":1,"owner":"writer"});
    let mut write = request("write", "writer", "axi", "write", 0, 4096, 4, &[1, 2, 3, 4]);
    write["lease"] = lease.clone();
    let reset_at = 710_000_000 / runtime::ddr4::TCK * runtime::ddr4::TCK;
    let commands = vec![
        json!({"id":"lease","at_ps":0,"operation":"lease","slot":"ddr-slot","owner":"writer","length":4}),
        json!({"id":"publish","at_ps":705_000_000,"operation":"publish","lease":lease}),
        json!({"id":"reset","at_ps":reset_at,"operation":"ddr_control","command":{"kind":"reset","asserted":true}}),
        json!({"id":"begin","at_ps":711_000_000,"operation":"begin_read","lease":lease,"consumer":"reader"}),
    ];
    let f = Fixture::new(c, vec![write], vec![], commands, 720_000_000);
    let snap = f.run();
    assert_ne!(
        snap.common.termination, "execution_failed",
        "{:?}",
        snap.common.diagnostics
    );
    let e = events(&snap);
    assert_eq!(kind(&e, "slot_published").len(), 1);
    assert_eq!(kind(&e, "slot_invalidated").len(), 1);
    assert!(
        kind(&e, "slot_command_rejected")
            .into_iter()
            .any(|(_, v)| v["detail"]["command_id"] == "begin")
    );
}
#[test]
fn checkpoint_rejects_consistently_redirected_ddr_controller_and_coherence_request() {
    let f = Fixture::new(
        ddr_config(),
        vec![request("read", "reader", "noc", "read", 0, 4096, 4, &[])],
        vec![],
        vec![],
        720_000_000,
    );
    let (mut session, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut found = false;
    for _ in 0..100 {
        session.advance_one().unwrap();
        found = kind(&events(session.snapshot()), "owner_grant")
            .iter()
            .any(|(_, v)| v["request_id"] == "read");
        if found {
            break;
        }
    }
    assert!(found);
    session.advance_one().unwrap();
    ctl.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let good = f.0.join("ddr-logical-owner");
    session
        .checkpoint(&good, CheckpointLimits::default())
        .unwrap();
    assert!(restore_checkpoint(&good, Registry::default(), CheckpointLimits::default()).is_ok());
    let rewritten = f.0.join("ddr-model-noop-rehash");
    mutate_checkpoint(&good, &rewritten, |_| {});
    assert!(
        restore_checkpoint(&rewritten, Registry::default(), CheckpointLimits::default()).is_ok()
    );
    let redirected = f.0.join("ddr-redirected-controller");
    mutate_checkpoint(&good, &redirected, |v| {
        let bridge = &mut v["state"]["ddr"];
        assert_eq!(bridge["jobs"]["read:0:0"]["request"]["address"], 4096);
        bridge["jobs"]["read:0:0"]["request"]["address"] = json!(4128);
        let queued = bridge["service"]["queue"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|t| t["request"]["id"] == "read:0:0")
            .unwrap();
        queued["request"]["address"] = json!(4128);
        for burst in queued["bursts"].as_array_mut().unwrap() {
            assert_eq!(burst["progress"]["state"], "waiting");
            let offset = burst["offset"].as_u64().unwrap();
            burst["offset"] = json!(offset + 32);
        }
        let physical: runtime::ddr4::Service =
            serde_json::from_value(bridge["service"].clone()).unwrap();
        physical
            .validate()
            .expect("co-mutated physical request must itself remain semantically valid");
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        restore_checkpoint(
            &redirected,
            Registry::default(),
            CheckpointLimits::default(),
        )
    }));
    assert!(result.is_ok());
    let error = result
        .unwrap()
        .err()
        .expect("accepted redirected DDR logical owner");
    assert!(
        format!("{error:?}").contains("redirected from actual owner"),
        "{error:?}"
    );
    for key in ["read:0:00", "read:0:4", "read:0:0:extra"] {
        let destination = f.0.join(key.replace(':', "_"));
        assert_bad_restore(&good, &destination, |v| {
            let jobs = v["state"]["ddr"]["jobs"].as_object_mut().unwrap();
            let mut job = jobs.remove("read:0:0").unwrap();
            job["request"]["id"] = json!(key);
            jobs.insert(key.into(), job);
            let queued = &mut v["state"]["ddr"]["service"]["queue"][0];
            queued["request"]["id"] = json!(key);
        });
    }
}

fn mailbox_config() -> Value {
    let mut c = config();
    c["slots"] = json!([{"id":"slot","base":256,"capacity":16}]);
    c["mailboxes"] = json!([{"id":"ipc","base":4096,"slot":"slot","producer":"producer","consumer":"consumer","service_cycles":7}]);
    c["dma_buffer_bytes"] = json!(4);
    c
}
fn mmio(
    id: &str,
    source: &str,
    transport: &str,
    at: u64,
    offset: u32,
    value: Option<u32>,
) -> Value {
    request(
        id,
        source,
        transport,
        if value.is_some() { "write" } else { "read" },
        at,
        4096 + offset,
        4,
        &value.map(|v| v.to_le_bytes().to_vec()).unwrap_or_default(),
    )
}
fn mailbox_sequence(transport: &str) -> Vec<Value> {
    let mut payload = request(
        "payload",
        "producer",
        transport,
        "write",
        300,
        256,
        4,
        &[7, 8, 9, 10],
    );
    payload["lease"] = json!({"slot":"slot","generation":1,"owner":"producer"});
    let mut read = request(
        "payload-read",
        "consumer",
        transport,
        "read",
        1000,
        256,
        4,
        &[],
    );
    read["lease"] = payload["lease"].clone();
    let mut publish = mmio("publish", "producer", transport, 600, 24, Some(2));
    publish["b_stall_cycles"] = json!(30);
    vec![
        mmio("length", "producer", transport, 0, 12, Some(4)),
        mmio("lease", "producer", transport, 100, 24, Some(1)),
        mmio("generation", "producer", transport, 200, 4, None),
        mmio("expect-producer", "producer", transport, 250, 16, Some(1)),
        payload,
        publish,
        mmio("notification", "consumer", transport, 700, 32, None),
        mmio("expect-consumer", "consumer", transport, 750, 16, Some(1)),
        mmio("begin", "consumer", transport, 850, 24, Some(3)),
        read,
        mmio("release", "consumer", transport, 1150, 24, Some(4)),
        mmio("status", "consumer", transport, 1300, 0, None),
    ]
}
#[test]
fn mapped_mailbox_three_native_transports_share_payload_and_lifecycle() {
    for transport in ["axi", "ahb", "noc"] {
        let f = Fixture::new(
            mailbox_config(),
            mailbox_sequence(transport),
            vec![],
            vec![],
            3000,
        );
        let snapshot = f.run();
        let e = events(&snapshot);
        assert_eq!(
            snapshot.common.termination, "events_exhausted",
            "{transport}: {:?}",
            snapshot.common
        );
        assert_eq!(
            completed(&e, "generation")["data"],
            json!([1, 0, 0, 0]),
            "{transport}"
        );
        assert_eq!(
            completed(&e, "notification")["data"],
            json!([1, 0, 0, 0]),
            "{transport}"
        );
        assert_eq!(
            completed(&e, "payload-read")["data"],
            json!([7, 8, 9, 10]),
            "{transport}"
        );
        assert_eq!(
            completed(&e, "status")["data"],
            json!([0, 0, 0, 0]),
            "{transport}"
        );
        assert_eq!(kind(&e, "slot_published").len(), 1);
        assert_eq!(kind(&e, "slot_notification").len(), 1);
        assert_eq!(kind(&e, "mailbox_completed").len(), 10);
        let accepted = kind(&e, "mailbox_accepted");
        for completed in kind(&e, "mailbox_completed") {
            let start = accepted
                .iter()
                .find(|(_, v)| v["request_id"] == completed.1["request_id"])
                .unwrap();
            assert_eq!(completed.0, start.0 + 7);
        }
        if transport == "axi" {
            assert!(e.iter().any(|(_, v)| v["channel"] == "AW"
                && v["detail"]["address"] == "4108"
                && v["detail"]["beats"] == 1
                && v["detail"]["size_bytes"] == 4));
        }
    }
}
#[test]
fn mapped_mailbox_preflights_entire_access_and_typed_command_results() {
    for transport in ["axi", "ahb", "noc"] {
        let mut burst = request(
            "burst",
            "producer",
            transport,
            "write",
            0,
            4108,
            8,
            &[4, 0, 0, 0, 1, 0, 0, 0],
        );
        burst["b_stall_cycles"] = json!(10);
        let mut partial = mmio("partial", "producer", transport, 100, 12, Some(4));
        partial["byte_enables"] = json!([true, false, true, true]);
        let mut zero = mmio("zero", "producer", transport, 200, 12, Some(4));
        zero["byte_enables"] = json!([false, false, false, false]);
        let mut requests = vec![
            burst,
            partial,
            zero,
            request(
                "unaligned",
                "producer",
                transport,
                "write",
                300,
                4109,
                4,
                &[4, 0, 0, 0],
            ),
            request("narrow", "producer", transport, "write", 400, 4108, 1, &[4]),
            mmio("foreign", "stranger", transport, 500, 24, Some(1)),
            mmio("length-zero", "producer", transport, 600, 12, None),
            mmio("wrong-role", "consumer", transport, 700, 24, Some(1)),
            mmio("role-result", "consumer", transport, 800, 28, None),
            mmio("length", "producer", transport, 900, 12, Some(4)),
            mmio("lease", "producer", transport, 1000, 24, Some(1)),
            mmio("expect", "producer", transport, 1100, 16, Some(1)),
            mmio("incomplete", "producer", transport, 1200, 24, Some(2)),
            mmio("incomplete-result", "producer", transport, 1300, 28, None),
        ];
        if transport == "axi" {
            let mut atomic = mmio("atomic", "producer", transport, 1400, 24, Some(1));
            atomic["operation"] = json!("atomic");
            atomic["atomic"] = json!({"awatop":32,"burst":"incr"});
            requests.push(atomic);
        }
        let snapshot = Fixture::new(mailbox_config(), requests, vec![], vec![], 4000).run();
        assert_eq!(
            snapshot.common.termination, "events_exhausted",
            "{transport}: {:?}",
            snapshot.common
        );
        let e = events(&snapshot);
        for id in ["burst", "partial", "zero", "unaligned", "narrow", "foreign"] {
            assert_ne!(completed(&e, id)["status"], "okay", "{transport}:{id}");
        }
        assert_eq!(completed(&e, "length-zero")["data"], json!([0, 0, 0, 0]));
        assert_eq!(completed(&e, "role-result")["data"], json!([1, 0, 0, 0]));
        assert_eq!(
            completed(&e, "incomplete-result")["data"],
            json!([4, 0, 0, 0])
        );
        assert!(kind(&e, "slot_notification").is_empty());
    }
}
#[test]
fn cpu_fence_and_dma_register_access_use_uncached_mailbox_target() {
    let mut c = mailbox_config();
    c["mailboxes"][0]["producer"] = json!("cpu");
    c["cores"] = json!([core("cpu", 0, "axi", 0)]);
    program(
        &mut c,
        0,
        &[
            0x000010b7,
            i(4, 0, 0, 2, 0x13),
            store(12, 2, 1),
            0x0ff0000f,
            i(1, 0, 0, 2, 0x13),
            store(24, 2, 1),
            0x0ff0000f,
            i(4, 1, 2, 3, 0x03),
            0x73,
        ],
    );
    let e = events(&Fixture::new(c, vec![], vec![], vec![], 2000).run());
    let generation = kind(&e, "cpu_retired")
        .into_iter()
        .find(|(_, v)| v["detail"]["register"] == json!([3, 1]));
    assert!(generation.is_some());
    assert_eq!(kind(&e, "mailbox_completed").len(), 3);
    assert!(
        kind(&e, "cache_fill")
            .iter()
            .all(|(_, v)| v["detail"]["line"].as_u64().unwrap() < 4096)
    );
    assert!(kind(&e, "ddr_command").is_empty());
    for transport in ["axi", "ahb", "noc"] {
        let mut c = mailbox_config();
        c["memory"][0]["initial"] = json!([4, 0, 0, 0, 1, 0, 0, 0]);
        let dma = vec![
            json!({"id":"len","source":"producer","at_ps":0,"source_address":0,"destination":4108,"length":4,"transport":transport,"router":0}),
            json!({"id":"cmd","source":"producer","at_ps":100,"source_address":4,"destination":4120,"length":4,"transport":transport,"router":0}),
        ];
        let e = events(
            &Fixture::new(
                c,
                vec![mmio("generation", "consumer", transport, 400, 4, None)],
                dma,
                vec![],
                2000,
            )
            .run(),
        );
        assert_eq!(
            completed(&e, "generation")["data"],
            json!([1, 0, 0, 0]),
            "{transport}"
        );
        assert_eq!(kind(&e, "dma_destination_completed").len(), 2);
    }
}
#[test]
fn mapped_mailbox_checkpoint_accept_applied_and_stalled_response_resume() {
    for transport in ["axi", "ahb", "noc"] {
        let mut requests = mailbox_sequence(transport);
        requests[1]["b_stall_cycles"] = json!(80);
        let f = Fixture::new(mailbox_config(), requests, vec![], vec![], 3000);
        let expected = events(&f.run());
        for steps in [1, 4, 10, 22, 45, 80, 130, 200] {
            let (mut session, ctl) = RunSession::new(
                f.prepared(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            for _ in 0..steps {
                if session.state() == RunState::Completed {
                    break;
                }
                session.advance_one().unwrap();
            }
            if session.state() == RunState::Completed {
                continue;
            }
            ctl.submit(ControlCommand::Pause).unwrap();
            session.poll_control().unwrap();
            let path = f.0.join(format!("mailbox-{steps}"));
            session
                .checkpoint(&path, CheckpointLimits::default())
                .unwrap();
            let (mut resumed, token) =
                restore_checkpoint(&path, Registry::default(), CheckpointLimits::default())
                    .unwrap();
            resumed.resume(token).unwrap();
            resumed.run_until_yield().unwrap();
            assert_eq!(events(resumed.snapshot()), expected, "{transport}:{steps}");
        }
    }
}

#[test]
fn mapped_mailbox_same_id_atomic_cannot_deadlock_older_blocked_access() {
    let mut first = mmio("occupied", "consumer", "axi", 0, 12, Some(4));
    first["b_stall_cycles"] = json!(200);
    let mut atomic = mmio("atomic", "producer", "axi", 6, 24, Some(1));
    atomic["operation"] = json!("atomic");
    atomic["atomic"] = json!({"awatop":32,"burst":"incr"});
    let s = Fixture::new(
        mailbox_config(),
        vec![
            first,
            mmio("older", "producer", "axi", 5, 12, Some(4)),
            atomic,
            mmio("observe", "producer", "axi", 500, 12, None),
        ],
        vec![],
        vec![],
        2000,
    )
    .run();
    let e = events(&s);
    assert_eq!(s.common.termination, "events_exhausted", "{:?}", s.common);
    assert_eq!(completed(&e, "older")["status"], "okay");
    assert_ne!(completed(&e, "atomic")["status"], "okay");
    assert_eq!(completed(&e, "observe")["data"], json!([4, 0, 0, 0]));
}
#[test]
fn ddr_only_reset_does_not_abort_pending_sram_mailbox_native_target() {
    for transport in ["axi", "ahb"] {
        let mut c = ddr_config();
        let m = mailbox_config();
        c["slots"] = m["slots"].clone();
        c["mailboxes"] = m["mailboxes"].clone();
        c["mailboxes"][0]["base"] = json!(8192);
        c["mailboxes"][0]["service_cycles"] = json!(100);
        let mut requests = vec![
            mmio("length", "producer", transport, 0, 12, Some(4)),
            mmio("lease", "producer", transport, 800, 24, Some(1)),
            mmio("generation", "consumer", transport, 1500, 4, None),
        ];
        requests[1]["b_stall_cycles"] = json!(200);
        for r in &mut requests {
            r["address"] = json!(r["address"].as_u64().unwrap() + 4096);
        }
        let s=Fixture::new(c,requests,vec![],vec![json!({"id":"reset","at_ps":834,"operation":"ddr_control","command":{"kind":"reset","asserted":true}})],10000).run();
        assert_ne!(
            s.common.termination, "execution_failed",
            "{transport}: {:?}",
            s.common
        );
        let e = events(&s);
        assert_eq!(completed(&e, "generation")["data"], json!([1, 0, 0, 0]));
        assert_eq!(completed(&e, "lease")["status"], "okay");
        assert_eq!(
            kind(&e, "mailbox_completed")
                .iter()
                .filter(|(_, v)| v["request_id"] == "lease")
                .count(),
            1
        );
    }
}
fn pause_after(f: &Fixture, predicate: impl Fn(&[(u64, Value)]) -> bool) -> RunSession {
    let (mut session, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..2000 {
        if predicate(&events(session.snapshot())) {
            break;
        }
        assert_ne!(
            session.state(),
            RunState::Completed,
            "requested transient was not reached"
        );
        session.advance_one().unwrap();
    }
    assert!(predicate(&events(session.snapshot())));
    ctl.submit(ControlCommand::Pause).unwrap();
    assert_eq!(session.poll_control().unwrap(), RunState::Paused);
    session
}
fn assert_mailbox_mutant_rejected(
    source: &std::path::Path,
    destination: &std::path::Path,
    change: impl FnOnce(&mut Value),
    reason: &str,
) {
    mutate_checkpoint(source, destination, change);
    let result = restore_checkpoint(
        destination,
        Registry::default(),
        CheckpointLimits::default(),
    );
    match result {
        Ok(_) => panic!("mailbox semantic mutant restored"),
        Err(e) => assert!(e.to_string().contains(reason), "expected {reason}: {e}"),
    }
}
#[test]
fn mailbox_semantic_history_rejects_forged_ready_and_erased_notification() {
    let f = Fixture::new(
        mailbox_config(),
        mailbox_sequence("axi"),
        vec![],
        vec![],
        3000,
    );
    let mut before = pause_after(&f, |e| {
        kind(e, "request_completed")
            .iter()
            .any(|(_, v)| v["request_id"] == "payload")
    });
    let source = f.0.join("before-publish");
    before
        .checkpoint(&source, CheckpointLimits::default())
        .unwrap();
    let noop = f.0.join("noop-rehashed");
    mutate_checkpoint(&source, &noop, |_| {});
    assert!(restore_checkpoint(&noop, Registry::default(), CheckpointLimits::default()).is_ok());
    assert_mailbox_mutant_rejected(
        &source,
        &f.0.join("forged-ready"),
        |v| v["state"]["slots"][0]["phase"] = json!("ready"),
        "typed transition history",
    );
    assert_mailbox_mutant_rejected(
        &source,
        &f.0.join("vacuous-ready"),
        |v| {
            v["state"]["slots"][0]["phase"] = json!("ready");
            v["state"]["slots"][0]["length"] = json!(0);
            v["state"]["slots"][0]["completed"] = json!([]);
        },
        "typed transition history",
    );
    let mut after = pause_after(&f, |e| {
        kind(e, "mailbox_completed")
            .iter()
            .any(|(_, v)| v["request_id"] == "publish")
    });
    let published = f.0.join("published");
    after
        .checkpoint(&published, CheckpointLimits::default())
        .unwrap();
    assert_mailbox_mutant_rejected(
        &published,
        &f.0.join("erased-notification"),
        |v| v["state"]["mailboxes"][0]["notification"] = Value::Null,
        "notification differs from transition history",
    );
    assert_mailbox_mutant_rejected(
        &published,
        &f.0.join("removed-effect"),
        |v| {
            let list = v["state"]["ipc_events"].as_array_mut().unwrap();
            let index = list
                .iter()
                .rposition(|e| e["origin"]["kind"] == "mapped")
                .unwrap();
            list.remove(index);
        },
        "typed transition history",
    );
}
#[test]
fn mailbox_semantic_history_rejects_co_mutated_register_read_capture() {
    let f = Fixture::new(
        mailbox_config(),
        vec![
            mmio("write-length", "producer", "axi", 0, 12, Some(4)),
            mmio("read-length", "producer", "axi", 100, 12, None),
            mmio("future", "consumer", "axi", 1000, 0, None),
        ],
        vec![],
        vec![],
        2000,
    );
    let mut session = pause_after(&f, |e| {
        kind(e, "request_completed")
            .iter()
            .any(|(_, v)| v["request_id"] == "read-length")
    });
    let source = f.0.join("read-complete");
    session
        .checkpoint(&source, CheckpointLimits::default())
        .unwrap();
    let noop = f.0.join("read-noop");
    mutate_checkpoint(&source, &noop, |_| {});
    assert!(restore_checkpoint(&noop, Registry::default(), CheckpointLimits::default()).is_ok());
    assert_mailbox_mutant_rejected(
        &source,
        &f.0.join("co-mutated-read"),
        |v| {
            let s = &mut v["state"];
            let index = s["transactions"]
                .as_array()
                .unwrap()
                .iter()
                .position(|t| t["request"]["id"] == "read-length")
                .unwrap();
            let bytes = json!([9, 0, 0, 0]);
            s["transactions"][index]["result"] = bytes.clone();
            s["mailbox_accesses"][index.to_string()]["data"] = bytes.clone();
            s["axi"]["transfers"][index]["data"] = bytes.clone();
            s["axi"]["asynchronous"][format!("soc-{}", index * 4097)]["data"] = bytes;
        },
        "derived register/result/generation mismatch",
    );
}
#[test]
fn invalid_register_dma_descriptor_cannot_partially_stage_a_command() {
    for transport in ["axi", "ahb", "noc"] {
        let mut c = mailbox_config();
        c["memory"][0]["initial"] = json!([4, 0, 0, 0, 1, 0, 0, 0]);
        let d = json!({"id":"bad","source":"producer","at_ps":0,"source_address":0,"destination":4108,"length":8,"transport":transport,"router":0});
        let e = events(
            &Fixture::new(
                c,
                vec![mmio("length", "producer", transport, 100, 12, None)],
                vec![d],
                vec![],
                1000,
            )
            .run(),
        );
        assert_eq!(completed(&e, "length")["data"], json!([0, 0, 0, 0]));
        assert_eq!(kind(&e, "dma_failed").len(), 1);
        assert_eq!(kind(&e, "mailbox_completed").len(), 1);
    }
}

#[test]
fn released_ddr_mailbox_notification_clears_on_reset_and_rehashed_restore_matches() {
    let mut c = ddr_config();
    c["slots"] = json!([{"id":"ddr-slot","base":4096,"capacity":4},{"id":"ram-slot","base":256,"capacity":4}]);
    c["mailboxes"] = json!([{"id":"ddr_ipc","base":8192,"slot":"ddr-slot","producer":"producer","consumer":"consumer","service_cycles":7},
        {"id":"ram_ipc","base":8256,"slot":"ram-slot","producer":"ram_producer","consumer":"ram_consumer","service_cycles":7}]);
    let ddr_lease = json!({"slot":"ddr-slot","generation":1,"owner":"producer"});
    let ram_lease = json!({"slot":"ram-slot","generation":1,"owner":"ram_producer"});
    let mut write = request(
        "write-ddr",
        "producer",
        "axi",
        "write",
        0,
        4096,
        4,
        &[1, 2, 3, 4],
    );
    write["lease"] = ddr_lease.clone();
    let mut ram = request(
        "write-ram",
        "ram_producer",
        "ahb",
        "write",
        0,
        256,
        4,
        &[5, 6, 7, 8],
    );
    ram["lease"] = ram_lease.clone();
    let reset = 713_000_000 / runtime::ddr4::TCK * runtime::ddr4::TCK;
    let commands = vec![
        json!({"id":"lease-ddr","at_ps":0,"operation":"lease","slot":"ddr-slot","owner":"producer","length":4}),
        json!({"id":"lease-ram","at_ps":0,"operation":"lease","slot":"ram-slot","owner":"ram_producer","length":4}),
        json!({"id":"publish-ddr","at_ps":710_000_000,"operation":"publish","lease":ddr_lease}),
        json!({"id":"publish-ram","at_ps":710_000_000,"operation":"publish","lease":ram_lease}),
        json!({"id":"begin-ddr","at_ps":711_000_000,"operation":"begin_read","lease":ddr_lease,"consumer":"consumer"}),
        json!({"id":"begin-ram","at_ps":711_000_000,"operation":"begin_read","lease":ram_lease,"consumer":"ram_consumer"}),
        json!({"id":"release-ddr","at_ps":712_000_000,"operation":"release","lease":ddr_lease,"consumer":"consumer"}),
        json!({"id":"release-ram","at_ps":712_000_000,"operation":"release","lease":ram_lease,"consumer":"ram_consumer"}),
        json!({"id":"reset","at_ps":reset,"operation":"ddr_control","command":{"kind":"reset","asserted":true}}),
    ];
    let requests = vec![
        write,
        ram,
        request(
            "notify-before",
            "consumer",
            "axi",
            "read",
            712_500_000,
            8192 + 32,
            4,
            &[],
        ),
        request(
            "notify-after",
            "consumer",
            "axi",
            "read",
            714_000_000,
            8192 + 32,
            4,
            &[],
        ),
        request(
            "ram-notify-after",
            "ram_consumer",
            "ahb",
            "read",
            714_000_000,
            8256 + 32,
            4,
            &[],
        ),
        request(
            "future",
            "consumer",
            "axi",
            "read",
            715_000_000,
            8192,
            4,
            &[],
        ),
    ];
    let f = Fixture::new(c, requests, vec![], commands, 730_000_000);
    let uninterrupted = f.run();
    let expected = events(&uninterrupted);
    assert_ne!(
        uninterrupted.common.termination, "execution_failed",
        "{:?}",
        uninterrupted.common
    );
    assert_eq!(
        completed(&expected, "notify-before")["data"],
        json!([1, 0, 0, 0])
    );
    assert_eq!(
        completed(&expected, "notify-after")["data"],
        json!([0, 0, 0, 0])
    );
    assert_eq!(
        completed(&expected, "ram-notify-after")["data"],
        json!([1, 0, 0, 0])
    );
    let mut session = pause_after(&f, |e| {
        kind(e, "request_completed")
            .iter()
            .any(|(_, v)| v["request_id"] == "ram-notify-after")
            && kind(e, "request_completed")
                .iter()
                .any(|(_, v)| v["request_id"] == "notify-after")
    });
    let good = f.0.join("after-reset");
    session
        .checkpoint(&good, CheckpointLimits::default())
        .unwrap();
    let rehashed = f.0.join("after-reset-noop");
    mutate_checkpoint(&good, &rehashed, |_| {});
    let (mut resumed, token) =
        restore_checkpoint(&rehashed, Registry::default(), CheckpointLimits::default()).unwrap();
    resumed.resume(token).unwrap();
    resumed.run_until_yield().unwrap();
    assert_eq!(events(resumed.snapshot()), expected);
    assert_mailbox_mutant_rejected(
        &good,
        &f.0.join("resurrect-notification"),
        |v| v["state"]["mailboxes"][0]["notification"] = json!(1),
        "notification differs from transition history",
    );
}
