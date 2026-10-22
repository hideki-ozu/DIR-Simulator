//! Independent expected channel ordering, byte lanes and checkpoint continuation.
use dir_simulator::output::transaction_fabric::channel_records;
use dir_simulator::registry::Registry;
use dir_simulator::runtime::{self, session::*};
use dir_simulator::{prepare, types::transaction_fabric::*};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(config: Value, requests: Vec<Value>, horizon: u64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-fabric-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/fabric")).unwrap();
        std::fs::write(root.join("ned/fabric/model.ned"), "package fabric; simple Fabric { parameters: @class(\"fabric.Concurrent\"); } network Demo { submodules: fabric: fabric.Fabric; }").unwrap();
        std::fs::write(
            root.join("model.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("workload.json"), serde_json::to_vec(&json!({"schema":"dir.transaction-fabric.workload","version":1,"requests":requests})).unwrap()).unwrap();
        std::fs::write(root.join("scenario.ini"), format!("[General]\nnetwork=fabric.Demo\nned-path=\"ned\"\nmodel-profile=\"axi4.concurrent.v1\"\nmodel-config=\"model.json\"\nworkload=\"workload.json\"\nsim-time-limit={horizon}ps\nmetrics-window=1ps\n")).unwrap();
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
fn config(width: usize) -> Value {
    json!({"schema":"dir.transaction-fabric.model","version":1,"cycle_ps":1,"data_bytes":width,
    "managers":[{"id":"m0","routes":{"slow":["x0"],"fast":["x1"]}},{"id":"m1","routes":{"slow":["x0"],"fast":["x1"]}}],
    "interconnects":["x0","x1"],"targets":[
        {"id":"slow","base":0,"size":256,"kind":"ddr","read_latency_cycles":8,"write_response_cycles":3,"initial":[1,2,3,4,5,6,7,8]},
        {"id":"fast","base":4096,"size":256,"kind":"shared","read_latency_cycles":1,"write_response_cycles":1,"initial":[9,10,11,12]}
    ]})
}
fn read(name: &str, source: &str, id: u32, address: u64, at: u64, beats: usize) -> Value {
    json!({"request_id":name,"source":source,"id":id,"at_ps":at,"operation":"read","address":address,"size_bytes":4,"beats":beats,"burst":"INCR"})
}
fn write(name: &str, source: &str, id: u32, address: u64, at: u64, data: Vec<Vec<u8>>) -> Value {
    let beats: Vec<_> = data
        .iter()
        .map(|d| json!({"data":d,"strobe":vec![true;d.len()]}))
        .collect();
    json!({"request_id":name,"source":source,"id":id,"at_ps":at,"operation":"write","address":address,"size_bytes":4,"beats":beats.len(),"burst":"INCR","write_data":beats})
}
fn events(snapshot: &dir_simulator::snapshot::Snapshot) -> Vec<(u64, Value)> {
    channel_records(snapshot.registered.as_ref().unwrap())
        .into_iter()
        .map(|r| (r.time_ps, r.data.clone()))
        .collect()
}
fn channel<'a>(events: &'a [(u64, Value)], request: &str, ch: &str) -> Vec<&'a (u64, Value)> {
    events
        .iter()
        .filter(|(_, v)| v["request_id"] == request && v["channel"] == ch)
        .collect()
}
#[test]
fn independent_channels_cross_id_out_of_order_same_id_order_and_shared_dma_visibility() {
    let mut buffered = write(
        "buffer",
        "m1",
        7,
        4100,
        0,
        vec![vec![20, 21, 22, 23], vec![24, 25, 26, 27]],
    );
    buffered["address_delay_cycles"] = json!(3);
    let mut dma = read("dma", "m1", 8, 4100, 10, 2);
    dma["transport"] = json!("dma");
    let f = Fixture::new(
        config(4),
        vec![
            read("slow", "m0", 0, 0, 0, 2),
            read("fast", "m0", 1, 4096, 0, 1),
            read("same", "m0", 0, 4100, 0, 1),
            buffered,
            dma,
        ],
        100,
    );
    let s = f.run();
    assert_eq!(s.common.termination, "events_exhausted");
    assert!(!s.common.partial);
    let e = events(&s);
    assert!(channel(&e, "fast", "R")[0].0 < channel(&e, "slow", "R")[0].0);
    assert!(channel(&e, "same", "R")[0].0 >= channel(&e, "slow", "R")[1].0);
    assert_eq!(channel(&e, "buffer", "W")[0].0, 0);
    assert_eq!(channel(&e, "buffer", "AW")[0].0, 3);
    assert_eq!(channel(&e, "buffer", "COMMIT")[0].0, 3);
    assert!(channel(&e, "buffer", "B")[0].0 > channel(&e, "buffer", "COMMIT")[1].0);
    assert_eq!(
        channel(&e, "dma", "DMA_R")[0].1["detail"]["data"],
        json!([20, 21, 22, 23])
    );
    assert_eq!(
        channel(&e, "dma", "DMA_R")[1].1["detail"]["data"],
        json!([24, 25, 26, 27])
    );
    // Independent paths accept two addresses in the same cycle on the same channel.
    let f = Fixture::new(
        config(4),
        vec![read("a", "m0", 0, 0, 0, 1), read("b", "m1", 1, 4096, 0, 1)],
        30,
    );
    let e = events(&f.run());
    assert_eq!(channel(&e, "a", "AR")[0].0, 0);
    assert_eq!(channel(&e, "b", "AR")[0].0, 0);
}
#[test]
fn wrap_fixed_narrow_unaligned_and_byte_strobes_preserve_other_lanes() {
    let mut wrap = write(
        "wrap",
        "m0",
        0,
        12,
        0,
        vec![vec![11; 4], vec![22; 4], vec![33; 4], vec![44; 4]],
    );
    wrap["burst"] = json!("WRAP");
    let mut fixed = write("fixed", "m0", 1, 4096, 0, vec![vec![55; 4], vec![66; 4]]);
    fixed["burst"] = json!("FIXED");
    let f = Fixture::new(
        config(4),
        vec![
            wrap,
            fixed,
            read("wread", "m1", 2, 0, 20, 4),
            read("fread", "m1", 3, 4096, 20, 1),
        ],
        60,
    );
    let e = events(&f.run());
    assert_eq!(
        channel(&e, "wrap", "COMMIT")
            .iter()
            .map(|(_, v)| v["detail"]["address"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap())
            .collect::<Vec<_>>(),
        [12, 0, 4, 8]
    );
    assert_eq!(
        channel(&e, "wread", "R")
            .iter()
            .map(|(_, v)| v["detail"]["data"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!(vec![22; 4]),
            json!(vec![33; 4]),
            json!(vec![44; 4]),
            json!(vec![11; 4])
        ]
    );
    assert_eq!(
        channel(&e, "fread", "R")[0].1["detail"]["data"],
        json!(vec![66; 4])
    );
    let narrow = json!({"request_id":"unaligned","source":"m0","id":4,"at_ps":0,"operation":"write","address":3,"size_bytes":4,"beats":3,"burst":"INCR","write_data":[
        {"data":[0,0,0,91,0,0,0,0],"strobe":[false,false,false,true,false,false,false,false]},
        {"data":[0,0,0,0,92,93,94,95],"strobe":[false,false,false,false,true,false,true,true]},
        {"data":[96,97,98,99,0,0,0,0],"strobe":[true,true,true,true,false,false,false,false]}]});
    let mut r = read("narrow-read", "m1", 0, 3, 10, 3);
    r["size_bytes"] = json!(4);
    let f = Fixture::new(config(8), vec![narrow, r], 40);
    let s = f.run();
    let e = events(&s);
    assert_eq!(
        channel(&e, "narrow-read", "R")[0].1["detail"]["data"],
        json!([0, 0, 0, 91, 0, 0, 0, 0])
    );
    assert_eq!(
        channel(&e, "narrow-read", "R")[1].1["detail"]["data"],
        json!([0, 0, 0, 0, 92, 6, 94, 95])
    );
    assert_eq!(
        channel(&e, "narrow-read", "R")[2].1["detail"]["data"],
        json!([96, 97, 98, 99, 0, 0, 0, 0])
    );
}
#[test]
fn exclusives_invalidated_by_dma_and_successful_exclusive_is_visible() {
    let mut er = read("er", "m0", 9, 4096, 0, 1);
    er["exclusive"] = json!(true);
    let mut interference = write("interference", "m1", 3, 4096, 4, vec![vec![77; 4]]);
    interference["transport"] = json!("dma");
    let mut failed = write("failed", "m0", 9, 4096, 8, vec![vec![88; 4]]);
    failed["exclusive"] = json!(true);
    let mut er2 = read("er2", "m0", 9, 4096, 12, 1);
    er2["exclusive"] = json!(true);
    let mut success = write("success", "m0", 9, 4096, 16, vec![vec![99; 4]]);
    success["exclusive"] = json!(true);
    let f = Fixture::new(
        config(4),
        vec![
            er,
            interference,
            failed,
            read("between", "m1", 4, 4096, 10, 1),
            er2,
            success,
            read("after", "m1", 4, 4096, 20, 1),
        ],
        40,
    );
    let e = events(&f.run());
    assert_eq!(channel(&e, "er", "R")[0].1["detail"]["response"], "EXOKAY");
    assert_eq!(
        channel(&e, "failed", "B")[0].1["detail"]["response"],
        "OKAY"
    );
    assert_eq!(
        channel(&e, "failed", "COMMIT")[0].1["detail"]["written"],
        false
    );
    assert_eq!(
        channel(&e, "between", "R")[0].1["detail"]["data"],
        json!(vec![77; 4])
    );
    assert_eq!(
        channel(&e, "success", "B")[0].1["detail"]["response"],
        "EXOKAY"
    );
    assert_eq!(
        channel(&e, "after", "R")[0].1["detail"]["data"],
        json!(vec![99; 4])
    );
}
#[test]
fn decoder_errors_and_invalid_boundary_strobes_routes_are_rejected() {
    let f = Fixture::new(config(4), vec![read("unmapped", "m0", 0, 2048, 0, 1)], 20);
    assert_eq!(
        channel(&events(&f.run()), "unmapped", "R")[0].1["detail"]["response"],
        "DECERR"
    );
    for request in [
        read("cross", "m0", 0, 4092, 0, 2),
        json!({"request_id":"bad","source":"m0","id":0,"at_ps":0,"operation":"write","address":1,"size_bytes":1,"beats":1,"burst":"INCR","write_data":[{"data":[1,2,3,4],"strobe":[true,false,false,false]}]}),
        {
            let mut r = read("wrap", "m0", 0, 1, 0, 4);
            r["burst"] = json!("WRAP");
            r
        },
    ] {
        let f = Fixture::new(config(4), vec![request], 20);
        assert!(prepare(&f.0.join("scenario.ini")).is_err());
    }
    let mut c = config(4);
    c["targets"][1]["base"] = json!(128);
    let f = Fixture::new(c, vec![], 20);
    assert!(prepare(&f.0.join("scenario.ini")).is_err());
    let mut c = config(4);
    c["managers"][0]["routes"]["slow"] = json!(["unknown"]);
    let f = Fixture::new(c, vec![], 20);
    assert!(prepare(&f.0.join("scenario.ini")).is_err());
}
#[test]
fn midburst_checkpoint_restores_buffered_channels_and_byte_memory() {
    let mut w = write("long", "m0", 0, 4096, 0, vec![vec![21; 4]; 8]);
    w["address_delay_cycles"] = json!(3);
    let f = Fixture::new(
        config(4),
        vec![
            w,
            {
                let mut r = read("parallel", "m1", 1, 0, 0, 5);
                r["response_stall_cycles"] = json!(3);
                r
            },
            {
                let mut r = read("exclusive-checkpoint", "m0", 9, 4128, 0, 1);
                r["exclusive"] = json!(true);
                r
            },
            {
                let mut w = write("exclusive-after", "m0", 9, 4128, 20, vec![vec![43; 4]]);
                w["exclusive"] = json!(true);
                w
            },
            read("after", "m1", 2, 4096, 20, 8),
        ],
        80,
    );
    let baseline = f.run();
    for steps in [2, 5, 10] {
        let (mut session, controller) = RunSession::new(
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
        controller.submit(ControlCommand::Pause).unwrap();
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
        assert_eq!(events(resumed.snapshot()), events(&baseline));
        let records = |s: &dir_simulator::snapshot::Snapshot| {
            serde_json::to_value(&s.registered.as_ref().unwrap().model_records).unwrap()
        };
        assert_eq!(records(resumed.snapshot()), records(&baseline));
    }
}
#[test]
fn horizon_observes_real_memory_commits_without_inventing_b_response() {
    let f = Fixture::new(
        config(4),
        vec![write("cut", "m0", 0, 0, 0, vec![vec![31; 4]; 5])],
        2,
    );
    let s = f.run();
    let e = events(&s);
    assert_eq!(s.common.termination, "time_limit");
    assert_eq!(channel(&e, "cut", "COMMIT").len(), 2);
    assert!(channel(&e, "cut", "B").is_empty());
    let memory =
        &s.registered.as_ref().unwrap().model_records["fabric.Record:memory-slow"].data["bytes"];
    assert_eq!(
        &memory.as_array().unwrap()[..12],
        &vec![
            json!(31),
            json!(31),
            json!(31),
            json!(31),
            json!(31),
            json!(31),
            json!(31),
            json!(31),
            json!(0),
            json!(0),
            json!(0),
            json!(0)
        ]
    );
    let f = Fixture::new(
        config(4),
        vec![write("zero", "m0", 0, 0, 0, vec![vec![31; 4]])],
        0,
    );
    assert!(events(&f.run()).is_empty());
}
#[test]
fn shared_memory_write_preflight_has_no_partial_mutation() {
    use dir_simulator::runtime::transaction_fabric::memory::*;
    let c: FabricConfig = serde_json::from_value(config(4)).unwrap();
    let mut memory = MemoryService::new(&c.targets);
    let before = memory.clone();
    assert!(
        memory
            .write(&MemoryRequest {
                source: "dma",
                request_id: "bad",
                id: 0,
                address: 254,
                data: &[1, 2, 3, 4],
                byte_enables: &[true; 4]
            })
            .is_err()
    );
    assert_eq!(memory, before);
}

#[test]
fn finite_w_buffer_response_payload_holds_under_backpressure_and_b_can_reorder() {
    let mut c = config(4);
    c["write_buffer_beats"] = json!(1);
    let mut first = write("first", "m0", 0, 4096, 0, vec![vec![1; 4]; 3]);
    first["address_delay_cycles"] = json!(4);
    let second = write("second", "m0", 1, 0, 0, vec![vec![2; 4]; 2]);
    let f = Fixture::new(c, vec![first, second], 40);
    let e = events(&f.run());
    let first_w = channel(&e, "first", "W");
    assert_eq!(first_w[0].0, 0);
    assert_eq!(first_w[1].0, 5);
    assert!(channel(&e, "second", "W")[0].0 > first_w[2].0);
    // R data is captured when VALID is asserted, not changed by later writers while READY is low.
    let mut stalled = read("stalled", "m0", 0, 4096, 0, 1);
    stalled["response_stall_cycles"] = json!(5);
    let f = Fixture::new(
        config(4),
        vec![
            stalled,
            write("mutate", "m1", 0, 4096, 3, vec![vec![88; 4]]),
            read("bypass", "m0", 1, 4100, 2, 1),
        ],
        30,
    );
    let e = events(&f.run());
    let held = channel(&e, "stalled", "R")[0];
    assert_eq!(held.0, 6);
    assert_eq!(held.1["detail"]["valid_since_ps"], "1");
    assert_eq!(held.1["detail"]["data"], json!([9, 10, 11, 12]));
    assert!(channel(&e, "bypass", "R")[0].0 > held.0);
    let f = Fixture::new(
        config(4),
        vec![
            write("slow-b", "m0", 0, 0, 0, vec![vec![1; 4]]),
            write("fast-b", "m0", 1, 4096, 0, vec![vec![2; 4]]),
        ],
        30,
    );
    let e = events(&f.run());
    assert!(channel(&e, "fast-b", "B")[0].0 < channel(&e, "slow-b", "B")[0].0);
}
#[test]
fn exclusive_read_grant_waits_for_last_r_and_intervening_write_cancels_candidate() {
    let mut exclusive = read("exclusive", "m0", 7, 4096, 0, 2);
    exclusive["exclusive"] = json!(true);
    exclusive["response_stall_cycles"] = json!(2);
    let mut interference = write("interfere", "m1", 0, 4096, 2, vec![vec![66; 4]]);
    interference["transport"] = json!("dma");
    let mut attempted = write("attempt", "m0", 7, 4096, 10, vec![vec![77; 4]; 2]);
    attempted["exclusive"] = json!(true);
    let f = Fixture::new(config(4), vec![exclusive, interference, attempted], 30);
    let e = events(&f.run());
    assert_eq!(
        channel(&e, "attempt", "B")[0].1["detail"]["response"],
        "OKAY"
    );
    assert!(
        channel(&e, "attempt", "COMMIT")
            .iter()
            .all(|(_, v)| v["detail"]["written"] == false)
    );
}
#[test]
fn shared_slave_keeps_aw_and_noninterleaved_w_order_while_first_aw_is_delayed() {
    let mut c = config(4);
    c["write_buffer_beats"] = json!(1);
    let mut first = write("first", "m0", 0, 0, 0, vec![vec![1; 4]; 4]);
    first["address_delay_cycles"] = json!(6);
    let f = Fixture::new(
        c,
        vec![first, write("second", "m1", 1, 0, 1, vec![vec![2; 4]; 2])],
        50,
    );
    let e = events(&f.run());
    let first_w = channel(&e, "first", "W");
    let second_w = channel(&e, "second", "W");
    assert_eq!(first_w.len(), 4);
    assert_eq!(second_w.len(), 2);
    assert!(second_w[0].0 > first_w[3].0);
    assert!(channel(&e, "second", "AW")[0].0 > channel(&e, "first", "AW")[0].0);
}
