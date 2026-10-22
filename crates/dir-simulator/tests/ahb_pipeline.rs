use dir_simulator::registry::Registry;
use dir_simulator::runtime::session::*;
use dir_simulator::{prepare, runtime};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(config: Value, requests: Vec<Value>, horizon: u64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-ahb-pipeline-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/ahb")).unwrap();
        std::fs::write(root.join("ned/ahb/model.ned"),"package ahb; simple Pipeline { parameters: @class(\"ahb.Pipeline\"); } network Demo { submodules: bus: ahb.Pipeline; }").unwrap();
        std::fs::write(
            root.join("model.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("workload.json"),
            serde_json::to_vec(
                &json!({"schema":"dir.ahb-pipeline.workload","version":1,"requests":requests}),
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("scenario.ini"),format!("[General]\nnetwork=ahb.Demo\nned-path=\"ned\"\nmodel-profile=\"ahb.pipeline.v1\"\nmodel-config=\"model.json\"\nworkload=\"workload.json\"\nsim-time-limit={horizon}ps\nmetrics-window=1ps\n")).unwrap();
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
    json!({"schema":"dir.ahb-pipeline.model","version":1,"cycle_ps":1,"data_bytes":width,"managers":[{"id":"low","priority":1},{"id":"high","priority":0}],"targets":[{"id":"ram0","base":0,"size":1024,"initial":[1,2,3,4,5,6,7,8]},{"id":"ram1","base":4096,"size":1024}]})
}
fn read(id: &str, source: &str, address: u64, at: u64, beats: usize) -> Value {
    json!({"request_id":id,"source":source,"at_ps":at,"operation":"read","address":address,"size_bytes":4,"beats":beats,"burst":"INCR"})
}
fn write(id: &str, source: &str, address: u64, at: u64, values: Vec<Vec<u8>>) -> Value {
    json!({"request_id":id,"source":source,"at_ps":at,"operation":"write","address":address,"size_bytes":4,"beats":values.len(),"burst":"INCR","write_data":values})
}
fn rows(s: &dir_simulator::snapshot::Snapshot) -> Vec<(u64, Value)> {
    let mut rows: Vec<_> = s
        .registered
        .as_ref()
        .unwrap()
        .model_records
        .values()
        .filter(|r| r.data["kind"] == "cycle")
        .map(|r| (r.time_ps, r.data.clone()))
        .collect();
    rows.sort_by_key(|(t, v)| {
        (
            *t,
            v["effect_seq"].as_str().unwrap().parse::<u64>().unwrap(),
        )
    });
    rows
}
fn accepted<'a>(r: &'a [(u64, Value)], id: &str) -> Vec<&'a (u64, Value)> {
    r.iter()
        .filter(|(_, v)| v["address_accepted"] == true && v["address_phase"]["request_id"] == id)
        .collect()
}
fn completed<'a>(r: &'a [(u64, Value)], id: &str) -> Vec<&'a (u64, Value)> {
    r.iter()
        .filter(|(_, v)| {
            v["hready"] == true && v["hresp"] == "OKAY" && v["data_phase"]["request_id"] == id
        })
        .collect()
}
#[test]
fn address_and_data_overlap_with_distinct_owners_and_real_read_after_write() {
    let f = Fixture::new(
        config(4),
        vec![
            write(
                "burst",
                "low",
                0,
                0,
                vec![vec![11; 4], vec![22; 4], vec![33; 4], vec![44; 4]],
            ),
            read("preempt", "high", 4096, 2, 1),
            read("readback", "high", 0, 10, 4),
        ],
        40,
    );
    let s = f.run();
    assert_eq!(s.common.termination, "events_exhausted");
    let r = rows(&s);
    let one = r.iter().find(|(t, _)| *t == 1).unwrap();
    assert_eq!(one.1["address_phase"]["beat"], 1);
    assert_eq!(one.1["data_phase"]["beat"], 0);
    assert_eq!(one.1["data_phase"]["committed"], true);
    let two = r.iter().find(|(t, _)| *t == 2).unwrap();
    assert_eq!(two.1["address_phase"]["source"], "high");
    assert_eq!(two.1["data_phase"]["source"], "low");
    let resumed = accepted(&r, "burst")[2];
    assert_eq!(resumed.1["address_phase"]["htrans"], "NONSEQ");
    assert_eq!(resumed.1["address_phase"]["hburst"], "SINGLE");
    assert_eq!(
        completed(&r, "readback")
            .iter()
            .map(|(_, v)| v["data_phase"]["read_data"].clone())
            .collect::<Vec<_>>(),
        [
            json!(vec![11; 4]),
            json!(vec![22; 4]),
            json!(vec![33; 4]),
            json!(vec![44; 4])
        ]
    );
}
#[test]
fn hready_wait_holds_following_address_and_current_write_data() {
    let mut c = config(4);
    c["targets"][0]["wait_cycles"] = json!(2);
    let f = Fixture::new(
        c,
        vec![write("wait", "low", 0, 0, vec![vec![91; 4], vec![92; 4]])],
        20,
    );
    let r = rows(&f.run());
    for time in [1, 2, 3] {
        let row = r.iter().find(|(t, _)| *t == time).unwrap();
        assert_eq!(row.1["address_phase"]["beat"], 1);
        assert_eq!(row.1["data_phase"]["beat"], 0);
        assert_eq!(row.1["data_phase"]["write_data"], json!(vec![91; 4]));
        assert_eq!(row.1["hready"], time == 3);
        assert_eq!(row.1["data_phase"]["committed"], time == 3);
    }
    assert_eq!(
        completed(&r, "wait")
            .iter()
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        [3, 6]
    );
}
#[test]
fn every_wrap_length_and_narrow_hsize_transfer_uses_correct_byte_lanes() {
    for beats in [4, 8, 16] {
        let start = (beats * 4 - 4) as u64;
        let mut w = write(
            "wrap",
            "low",
            start,
            0,
            (0..beats).map(|b| vec![(b + 20) as u8; 4]).collect(),
        );
        w["burst"] = json!(format!("WRAP{beats}"));
        let f = Fixture::new(config(4), vec![w], 50);
        let r = rows(&f.run());
        let addresses: Vec<_> = accepted(&r, "wrap")
            .iter()
            .map(|(_, v)| {
                v["address_phase"]["address"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            })
            .collect();
        let expected: Vec<_> = std::iter::once(start)
            .chain((0..beats - 1).map(|b| (b * 4) as u64))
            .collect();
        assert_eq!(addresses, expected);
    }
    let mut w = write(
        "narrow",
        "low",
        2,
        0,
        vec![
            vec![0, 0, 80, 81, 0, 0, 0, 0],
            vec![0, 0, 0, 0, 82, 83, 0, 0],
        ],
    );
    w["size_bytes"] = json!(2);
    let mut r = read("read", "high", 0, 8, 1);
    r["size_bytes"] = json!(8);
    let f = Fixture::new(config(8), vec![w, r], 20);
    let r = rows(&f.run());
    assert_eq!(
        completed(&r, "read")[0].1["data_phase"]["read_data"],
        json!([1, 2, 80, 81, 82, 83, 7, 8])
    );
}
#[test]
fn locked_burst_excludes_higher_priority_contender_through_final_data() {
    let mut w = write("lock", "low", 0, 0, vec![vec![10; 4]; 4]);
    w["locked"] = json!(true);
    w["burst"] = json!("INCR4");
    let f = Fixture::new(
        config(4),
        vec![w, read("contender", "high", 4096, 2, 1)],
        30,
    );
    let r = rows(&f.run());
    assert!(r[0].1["address_phase"].is_null());
    assert_eq!(r[0].1["hlock"], "low");
    let last = completed(&r, "lock").last().unwrap().0;
    assert!(accepted(&r, "contender")[0].0 > last);
    assert!(r.iter().find(|(t, _)| *t == last).unwrap().1["address_phase"].is_null());
}
#[test]
fn retry_reissues_without_mask_and_split_allows_lower_until_actual_hsplit() {
    for response in ["RETRY", "SPLIT"] {
        let mut c = config(4);
        c["responses"] = json!([{"request_id":"first","beat":0,"attempt":0,"response":response,"split_release_cycles":if response=="SPLIT"{json!(5)}else{Value::Null}}]);
        let f = Fixture::new(
            c,
            vec![
                write("first", "high", 4096, 0, vec![vec![90; 4]]),
                read("other", "low", 0, 0, 1),
            ],
            30,
        );
        let r = rows(&f.run());
        let first = accepted(&r, "first");
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].1["address_phase"]["attempt"], 0);
        assert_eq!(first[1].1["address_phase"]["attempt"], 1);
        assert_eq!(completed(&r, "first").len(), 1);
        assert_eq!(completed(&r, "first")[0].1["data_phase"]["committed"], true);
        let response_rows: Vec<_> = r.iter().filter(|(_, v)| v["hresp"] == response).collect();
        assert_eq!(response_rows.len(), 2);
        assert_eq!(response_rows[0].1["hready"], false);
        assert_eq!(response_rows[1].1["hready"], true);
        assert!(response_rows[1].1["address_phase"].is_null());
        if response == "SPLIT" {
            let pulse = r
                .iter()
                .find(|(_, v)| v["hsplit"] == json!(["high"]))
                .unwrap();
            assert!(accepted(&r, "other")[0].0 < pulse.0);
            assert!(first[1].0 > pulse.0);
            assert!(
                r.iter()
                    .any(|(_, v)| v["masked_masters"] == json!(["high"]))
            );
        } else {
            assert!(r.iter().all(|(_, v)| v["masked_masters"] == json!([])));
            assert!(first[1].0 < accepted(&r, "other")[0].0);
        }
    }
}
#[test]
fn locked_split_grants_only_dummy_idle_and_locked_retry_retains_owner() {
    for response in ["SPLIT", "RETRY"] {
        let mut c = config(4);
        c["responses"] = json!([{"request_id":"lock","beat":0,"attempt":0,"response":response,"split_release_cycles":if response=="SPLIT"{json!(4)}else{Value::Null}}]);
        let mut w = write("lock", "low", 0, 0, vec![vec![71; 4]]);
        w["locked"] = json!(true);
        let f = Fixture::new(c, vec![w, read("contender", "high", 4096, 2, 1)], 30);
        let r = rows(&f.run());
        let done = completed(&r, "lock")[0].0;
        assert!(accepted(&r, "contender")[0].0 > done);
        if response == "SPLIT" {
            let masked: Vec<_> = r
                .iter()
                .filter(|(_, v)| v["masked_masters"] == json!(["low"]))
                .collect();
            assert!(!masked.is_empty());
            assert!(masked.iter().all(|(_, v)| v["address_phase"].is_null()));
        }
    }
}
#[test]
fn error_is_two_cycle_and_cancels_following_address_without_memory_mutation() {
    let mut c = config(4);
    c["responses"] =
        json!([{"request_id":"error","beat":0,"attempt":0,"response":"ERROR","wait_cycles":1}]);
    let f = Fixture::new(
        c,
        vec![
            write("error", "low", 0, 0, vec![vec![99; 4]; 4]),
            read("read", "high", 0, 8, 1),
        ],
        20,
    );
    let s = f.run();
    let r = rows(&s);
    assert_eq!(
        r.iter()
            .filter(|(_, v)| v["hresp"] == "ERROR")
            .map(|(t, v)| (*t, v["hready"].as_bool().unwrap()))
            .collect::<Vec<_>>(),
        [(2, false), (3, true)]
    );
    assert_eq!(accepted(&r, "error").len(), 1);
    assert!(completed(&r, "error").is_empty());
    assert_eq!(
        completed(&r, "read")[0].1["data_phase"]["read_data"],
        json!([1, 2, 3, 4])
    );
    assert!(r.iter().find(|(t, _)| *t == 2).unwrap().1["cancelled_address"] == true);
}
#[test]
fn rejects_bad_alignment_1k_boundary_burst_shape_and_unsafe_retry_target() {
    for request in [
        read("unaligned", "low", 1, 0, 1),
        read("cross", "low", 1020, 0, 2),
        {
            let mut r = read("length", "low", 0, 0, 3);
            r["burst"] = json!("WRAP4");
            r
        },
    ] {
        let f = Fixture::new(config(4), vec![request], 20);
        assert!(prepare(&f.0.join("scenario.ini")).is_err());
    }
    let mut c = config(4);
    c["responses"] = json!([{"request_id":"retry","beat":0,"attempt":0,"response":"RETRY"}]);
    let f = Fixture::new(
        c,
        vec![
            read("retry", "low", 0, 0, 1),
            read("unsafe", "high", 4, 10, 1),
        ],
        20,
    );
    assert!(
        prepare(&f.0.join("scenario.ini"))
            .unwrap_err()
            .message
            .contains("single-manager")
    );
    let f = Fixture::new(config(4), vec![read("unmapped", "low", 2048, 0, 1)], 10);
    assert_eq!(
        rows(&f.run())
            .iter()
            .filter(|(_, v)| v["hresp"] == "ERROR")
            .count(),
        2
    );
}
#[test]
fn mid_stall_and_split_checkpoint_resume_preserve_slots_bytes_masks_and_attempts() {
    let mut c = config(4);
    c["targets"][0]["wait_cycles"] = json!(2);
    c["responses"] = json!([{"request_id":"split","beat":0,"attempt":0,"response":"SPLIT","split_release_cycles":6}]);
    let f = Fixture::new(
        c,
        vec![
            write("write", "low", 0, 0, vec![vec![48; 4]; 4]),
            read("split", "high", 4096, 1, 2),
        ],
        50,
    );
    let baseline = f.run();
    for steps in [3, 6, 9] {
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
        let p = f.0.join(format!("checkpoint-{steps}"));
        assert_eq!(
            session
                .checkpoint(&p, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut restored, token) =
            restore_checkpoint(&p, Registry::default(), CheckpointLimits::default()).unwrap();
        assert_eq!(rows(restored.snapshot()), rows(session.snapshot()));
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(rows(restored.snapshot()), rows(&baseline));
        assert_eq!(
            serde_json::to_value(
                &restored
                    .snapshot()
                    .registered
                    .as_ref()
                    .unwrap()
                    .model_records
            )
            .unwrap(),
            serde_json::to_value(&baseline.registered.as_ref().unwrap().model_records).unwrap()
        );
    }
}
#[test]
fn horizon_and_stop_never_commit_pending_data_phase() {
    let mut c = config(4);
    c["targets"][0]["wait_cycles"] = json!(4);
    let f = Fixture::new(c, vec![write("wait", "low", 0, 0, vec![vec![88; 4]; 4])], 3);
    let s = f.run();
    assert_eq!(s.common.termination, "time_limit");
    assert!(completed(&rows(&s), "wait").is_empty());
    assert!(
        s.registered
            .as_ref()
            .unwrap()
            .model_records
            .values()
            .all(|r| r.data["kind"] != "memory")
    );
    let (mut s, c) = RunSession::new(f.prepared(), RunOptions::default()).unwrap();
    for _ in 0..3 {
        s.advance_one().unwrap();
    }
    c.submit(ControlCommand::Stop).unwrap();
    assert_eq!(s.poll_control().unwrap(), RunState::Stopped);
    assert!(completed(&rows(s.snapshot()), "wait").is_empty());
    let f = Fixture::new(config(4), vec![read("zero", "low", 0, 0, 1)], 0);
    assert!(rows(&f.run()).is_empty());
}
