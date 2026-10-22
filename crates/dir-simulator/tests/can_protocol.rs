use dir_simulator::registry::{RegisteredSnapshot, Registry};
use dir_simulator::runtime::can_protocol::{
    codec, confinement::Counters, random::Cursor, register_can_protocol, types::*,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn frame(remote: bool, extended: bool) -> Frame {
    Frame {
        id: if extended { 0x1abcde } else { 0x123 },
        extended,
        remote,
        dlc: 2,
        data: if remote { vec![] } else { vec![0x12, 0x34] },
    }
}
fn registry() -> Registry {
    let mut r = Registry::empty();
    register_can_protocol(&mut r).unwrap();
    r
}
struct Fixture(PathBuf);
impl Fixture {
    fn new(config: Value, workload: Value, horizon: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-can-protocol-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/demo")).unwrap();
        std::fs::write(root.join("ned/demo/protocol.ned"),"package demo; simple Protocol { parameters: @class(\"dir.can.ProtocolV1\"); } network Demo { submodules: bus: demo.Protocol; }").unwrap();
        std::fs::write(root.join("config.json"), config.to_string()).unwrap();
        std::fs::write(root.join("workload.json"), workload.to_string()).unwrap();
        std::fs::write(root.join("protocol.ini"),format!("[General]\nnetwork=demo.Demo\nned-path=\"ned\"\nmodel-profile=\"can.cc.protocol.v1\"\nmodel-config=\"config.json\"\nworkload=\"workload.json\"\nsim-time-limit={horizon}\n")).unwrap();
        Self(root)
    }
    fn prepared(&self) -> dir_simulator::PreparedSimulation {
        dir_simulator::prepare_with_registry(&self.0.join("protocol.ini"), registry()).unwrap()
    }
    fn run(&self) -> RegisteredSnapshot {
        dir_simulator::runtime::simulate(&self.prepared())
            .unwrap()
            .registered
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn config() -> Value {
    json!({"schema_version":1,"bitrate":500000,"controllers":[{"id":"a"},{"id":"b"}]})
}
fn workload(f: Frame) -> Value {
    json!({"schema_version":1,"generators":[{"id":"g","controller":"a","frame":f,"schedule":{"kind":"explicit.v1","times_ps":[0]}}]})
}
fn records(s: &RegisteredSnapshot, kind: &str) -> Vec<Value> {
    s.model_records
        .values()
        .filter(|r| r.data["kind"] == kind)
        .map(|r| r.data.clone())
        .collect()
}
#[test]
fn codec_data_remote_both_formats_and_existing_vectors() {
    for remote in [false, true] {
        for ext in [false, true] {
            let f = frame(remote, ext);
            let w = codec::encode(&f).unwrap();
            assert_eq!(codec::decode(&w.bits).unwrap(), f);
            if !remote {
                let legacy = dir_simulator::can::serialize(&dir_simulator::types::Frame {
                    format: if ext { "extended" } else { "standard" }.into(),
                    id: f.id,
                    data: "1234".into(),
                })
                .unwrap();
                assert_eq!(w.bits, legacy.frame);
                assert_eq!(w.crc, legacy.crc15);
            }
        }
    }
}
#[test]
fn errors_derive_from_sampled_wire() {
    let w = codec::encode(&frame(false, false)).unwrap();
    let mut stuff = w.bits.clone();
    let at = w.stuff_positions[0];
    stuff[at] ^= 1;
    assert_eq!(codec::decode(&stuff).unwrap_err().kind, "stuff");
    let mut form = w.bits.clone();
    form[w.crc_delimiter] = 0;
    assert_eq!(codec::decode(&form).unwrap_err().kind, "form");
    let mut crc = w.bits.clone();
    crc[w.crc_delimiter - 1] ^= 1;
    assert!(codec::decode(&crc).is_err());
}
#[test]
fn remote_response_keeps_lineage() {
    let mut c = config();
    c["controllers"][1]["responders"] = json!([frame(false, false)]);
    let s = Fixture::new(c, workload(frame(true, false)), "2ms").run();
    let requests = records(&s, "request");
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r["state"] == "completed"));
    assert!(requests.iter().any(|r| r["parent_request_id"] == "g:0"));
    assert_eq!(records(&s, "reception").len(), 2);
}
#[test]
fn ack_failure_consumes_time_retries_same_request() {
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    let a = records(&s, "attempt");
    assert_eq!(a.len(), 2);
    let failed = a.iter().find(|r| r["state"] == "failed").unwrap();
    let success = a.iter().find(|r| r["state"] == "completed").unwrap();
    assert_eq!(success["request_id"], failed["request_id"]);
    assert!(
        success["sof_ps"].as_str().unwrap().parse::<u64>().unwrap()
            >= failed["release_ps"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
    assert_eq!(records(&s, "reception").len(), 1);
    let nodes = records(&s, "controller");
    assert_eq!(
        nodes.iter().find(|r| r["controller"] == "a").unwrap()["tec"],
        7
    );
}
#[test]
fn passive_ack_exception_does_not_bus_off() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(128);
    c["controllers"][1]["listen_only"] = json!(true);
    let s = Fixture::new(c, workload(frame(false, false)), "1ms").run();
    let nodes = records(&s, "controller");
    assert_eq!(
        nodes.iter().find(|r| r["controller"] == "a").unwrap()["tec"],
        128
    );
    assert!(records(&s, "attempt").len() > 1);
    assert!(records(&s, "reception").is_empty());
}
#[test]
fn handwritten_confinement_boundaries_and_exceptions() {
    let mut n = Counters { tec: 120, rec: 0 };
    n.tx_error(false, false);
    assert_eq!(n.state(), "error_passive");
    n.tx_error(true, false);
    assert_eq!(n.tec, 128);
    n.tx_error(false, true);
    assert_eq!(n.tec, 128);
    n.tec = 248;
    n.tx_error(false, false);
    assert_eq!(n.state(), "bus_off");
    n.tec = 128;
    n.tx_success();
    assert_eq!(n.state(), "error_active");
    n.rec = 128;
    n.rx_success();
    assert_eq!(n.rec, 120);
    n.rec = 127;
    n.rx_error(false);
    assert_eq!(n.state(), "error_passive");
    n.rx_error(true);
    assert_eq!(n.rec, 128);
    n.dominant_after_flag();
    assert_eq!(n.rec, 136);
    n.flag_bit_error(false);
    assert_eq!(n.rec, 144);
    n.dominant_extension(false);
    assert_eq!(n.rec, 152);
}
#[test]
fn seeded_schedules_are_online_and_reproducible() {
    let schedules = [
        Schedule::Uniform {
            seed: 42,
            start_ps: 10,
            count: 5,
            min_ps: 2,
            max_ps: 8,
        },
        Schedule::Bernoulli {
            seed: 42,
            start_ps: 0,
            count: 10,
            tick_ps: 10,
            numerator: 1,
            denominator: 2,
        },
        Schedule::Geometric {
            seed: 42,
            start_ps: 0,
            count: 5,
            tick_ps: 10,
            numerator: 1,
            denominator: 2,
        },
    ];
    let mut got = vec![];
    for s in schedules {
        let mut c = Cursor::new(&s);
        let mut times = vec![];
        while let Some(t) = c.next(&s).unwrap() {
            times.push(t);
        }
        got.push(times);
    }
    assert_eq!(
        got,
        vec![
            vec![10, 17, 24, 26, 30],
            vec![20, 30, 40, 50, 70, 90],
            vec![30, 40, 50, 60, 80]
        ]
    );
}
#[test]
fn no_completed_reception_before_eof() {
    let s = Fixture::new(config(), workload(frame(false, false)), "10us").run();
    assert!(records(&s, "reception").is_empty());
    assert!(
        records(&s, "request")
            .iter()
            .all(|r| r["state"] == "transmitting")
    );
}
#[test]
fn ack_error_overlap_has_independent_physical_time() {
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    let failed = records(&s, "attempt")
        .into_iter()
        .find(|r| r["state"] == "failed")
        .unwrap();
    let wire = codec::encode(&frame(false, false)).unwrap();
    let expected_bits = wire.ack + 1 + 7 + 8 + 3;
    assert_eq!(failed["wire_bits"].as_array().unwrap().len(), expected_bits);
    assert_eq!(
        failed["release_ps"],
        (expected_bits as u64 * 2_000_000).to_string()
    );
    let errors = failed["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0]["flag_start_bit"], wire.ack + 1);
    assert_eq!(errors[1]["flag_start_bit"], wire.ack + 2);
}
#[test]
fn same_id_data_arbitrates_before_remote_in_both_formats() {
    for extended in [false, true] {
        let f = frame(true, extended);
        let w = json!({"schema_version":1,"generators":[{"id":"remote","controller":"a","frame":f,"schedule":{"kind":"explicit.v1","times_ps":[0]}},{"id":"data","controller":"b","frame":frame(false,extended),"schedule":{"kind":"explicit.v1","times_ps":[0]}}]});
        let s = Fixture::new(config(), w, "2ms").run();
        let attempts = records(&s, "attempt");
        assert_eq!(
            attempts
                .iter()
                .filter(|a| a["state"] == "completed")
                .count(),
            2
        );
        assert!(
            attempts
                .iter()
                .any(|a| a["state"] == "arbitration_lost" && a["request_id"] == "remote:0")
        );
        assert_eq!(
            attempts
                .iter()
                .find(|r| r["request_id"] == "data:0")
                .unwrap()["sof_ps"],
            "0"
        );
    }
}
#[test]
fn overload_delays_arbitration_without_data_delivery() {
    let mut c = config();
    c["overloads"] = json!([{"request_id":"g:0","count":2}]);
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"]["times_ps"] = json!([0, 0]);
    let s = Fixture::new(c, w, "2ms").run();
    let a = records(&s, "attempt");
    let first = a.iter().find(|r| r["request_id"] == "g:0").unwrap();
    let next = a.iter().find(|r| r["request_id"] == "g:1").unwrap();
    let eof = first["eof_ps"].as_str().unwrap().parse::<u64>().unwrap();
    assert_eq!(next["sof_ps"], (eof + 33 * 2_000_000).to_string());
    assert_eq!(records(&s, "overload").len(), 2);
    assert_eq!(records(&s, "reception").len(), 2);
}
#[test]
fn bus_off_retains_request_and_recovers_from_observed_idle() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(248);
    c["controllers"][0]["automatic_recovery"] = json!(true);
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":25}]);
    let s = Fixture::new(c, workload(frame(false, false)), "10ms").run();
    let req = records(&s, "request");
    assert_eq!(req[0]["state"], "completed");
    assert_eq!(req[0]["attempt"], 2);
    let a = records(&s, "attempt");
    let retry = a.iter().find(|r| r["state"] == "completed").unwrap();
    assert!(retry["sof_ps"].as_str().unwrap().parse::<u64>().unwrap() >= 128 * 11 * 2_000_000);
    assert_eq!(
        records(&s, "controller")
            .iter()
            .find(|r| r["controller"] == "a")
            .unwrap()["state"],
        "error_active"
    );
}
#[test]
fn manual_bus_off_requires_explicit_recovery_then_idle_count() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(256);
    let blocked = Fixture::new(c.clone(), workload(frame(false, false)), "10ms").run();
    assert!(records(&blocked, "attempt").is_empty());
    c["controllers"][0]["recover_at_ps"] = json!(100_000_000);
    let recovered = Fixture::new(c, workload(frame(false, false)), "10ms").run();
    let a = records(&recovered, "attempt");
    assert_eq!(a.len(), 1);
    assert_eq!(
        a[0]["sof_ps"],
        (100_000_000 + 128 * 11 * 2_000_000u64).to_string()
    );
}
#[test]
fn checkpoint_resumes_live_attempt_and_rng_without_replay() {
    use dir_simulator::runtime::session::*;
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"] = json!({"kind":"uniform.splitmix64.v1","seed":42,"start_ps":0,"count":3,"min_ps":1000,"max_ps":8000});
    let f = Fixture::new(c, w, "10ms");
    let expected = f.run();
    for boundary in [7, 65, 110] {
        let (mut s, ctl) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..boundary {
            s.advance_one().unwrap();
        }
        ctl.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        let path = f.0.join(format!("checkpoint-{boundary}"));
        let ack = s.checkpoint(&path, CheckpointLimits::default()).unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let (mut restored, token) =
            restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
        let actual = restored.snapshot().registered.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&actual.model_records).unwrap(),
            serde_json::to_value(&expected.model_records).unwrap()
        );
    }
}
#[test]
fn scheduled_wire_errors_emit_actual_receiver_error_class_and_retry() {
    // Independent Python bit-serial CRC-15 oracle: 0x123/1234 has CRC 0x36b7,
    // one stuff bit at physical bit 17 and CRC delimiter at physical bit 51.
    for (kind, bit) in [("stuff", 17), ("form", 51), ("crc", 49)] {
        let mut c = config();
        c["faults"] =
            json!([{"request_id":"g:0","attempt":1,"kind":kind,"bit":bit,"receiver":"b"}]);
        let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
        assert!(
            records(&s, "error")
                .iter()
                .any(|r| r["detector"] == "b" && r["error"] == kind),
            "{kind}: {:?}",
            records(&s, "error")
        );
        assert_eq!(records(&s, "attempt").len(), 2);
        assert_eq!(records(&s, "reception").len(), 1);
    }
}
#[test]
fn ordinary_finite_application_rx_drops_do_not_erase_bus_reception() {
    let mut c = config();
    c["controllers"][1]["application_rx"] = json!({"capacity":1,"service_ps":1_000_000_000});
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"]["times_ps"] = json!([0, 0, 0]);
    let s = Fixture::new(c, w, "3ms").run();
    assert_eq!(records(&s, "reception").len(), 3);
    let app = records(&s, "application");
    assert_eq!(app.len(), 3);
    assert_eq!(app.iter().filter(|r| r["state"] == "completed").count(), 1);
    assert_eq!(
        app.iter().filter(|r| r["reason"] == "rx_fifo_full").count(),
        2
    );
}
#[test]
fn pause_mid_application_service_retains_fifo_and_completion_timer() {
    use dir_simulator::runtime::session::*;
    let mut c = config();
    c["controllers"][1]["application_rx"] = json!({"capacity":2,"service_ps":1_000_000_000});
    let f = Fixture::new(c, workload(frame(false, false)), "3ms");
    let expected = f.run();
    let (mut s, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    while records(s.snapshot().registered.as_ref().unwrap(), "application").is_empty() {
        s.advance_one().unwrap();
    }
    ctl.submit(ControlCommand::Pause).unwrap();
    s.poll_control().unwrap();
    let path = f.0.join("application-checkpoint");
    assert_eq!(
        s.checkpoint(&path, CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let (mut restored, token) =
        restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    restored.run_until_yield().unwrap();
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
        serde_json::to_value(&expected.model_records).unwrap()
    );
}
#[test]
fn generator_array_order_does_not_change_requests_or_rng() {
    let c = config();
    let mut w = workload(frame(false, false));
    let mut other = w["generators"][0].clone();
    other["id"] = json!("z");
    other["controller"] = json!("b");
    other["frame"]["id"] = json!(2);
    other["schedule"] = json!({"kind":"geometric.splitmix64.v1","seed":17,"start_ps":0,"count":3,"tick_ps":1000,"numerator":1,"denominator":2});
    w["generators"].as_array_mut().unwrap().push(other);
    let a = Fixture::new(c.clone(), w.clone(), "5ms").run();
    w["generators"].as_array_mut().unwrap().reverse();
    let b = Fixture::new(c, w, "5ms").run();
    assert_eq!(
        serde_json::to_value(a.model_records).unwrap(),
        serde_json::to_value(b.model_records).unwrap()
    );
}
#[test]
fn initial_auto_bus_off_recovers_before_later_request_and_idle_timer_does_not_duplicate_bits() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(256);
    c["controllers"][0]["automatic_recovery"] = json!(true);
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"]["times_ps"] = json!([4_000_000_000u64]);
    let s = Fixture::new(c.clone(), w, "10ms").run();
    assert_eq!(records(&s, "attempt")[0]["sof_ps"], "4000000000");
    let mut during = workload(frame(false, false));
    during["generators"][0]["controller"] = json!("b");
    during["generators"][0]["schedule"]["times_ps"] = json!([1000]);
    c["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c"}));
    let s = Fixture::new(c, during, "10ms").run();
    let attempt = records(&s, "attempt")
        .into_iter()
        .find(|a| a["state"] == "completed")
        .unwrap();
    let sof = attempt["sof_ps"].as_str().unwrap().parse::<u64>().unwrap();
    let eof = attempt["eof_ps"].as_str().unwrap().parse::<u64>().unwrap();
    assert_eq!(
        eof - sof,
        codec::encode(&frame(false, false)).unwrap().bits.len() as u64 * 2_000_000
    );
}
#[test]
fn strict_input_rejects_unknown_fields_invalid_rtr_and_zero_probability() {
    let mut bad = workload(frame(true, false));
    bad["generators"][0]["frame"]["data"] = json!([1]);
    let f = Fixture::new(config(), bad, "1ms");
    assert!(dir_simulator::prepare_with_registry(&f.0.join("protocol.ini"), registry()).is_err());
    let mut c = config();
    c["unexpected"] = json!(true);
    let f = Fixture::new(c, workload(frame(false, false)), "1ms");
    assert!(dir_simulator::prepare_with_registry(&f.0.join("protocol.ini"), registry()).is_err());
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"] = json!({"kind":"geometric.splitmix64.v1","seed":1,"start_ps":0,"count":1,"tick_ps":1,"numerator":0,"denominator":2});
    let f = Fixture::new(config(), w, "1ms");
    assert!(dir_simulator::prepare_with_registry(&f.0.join("protocol.ini"), registry()).is_err());
}
#[test]
fn error_delimiter_form_error_resignals_but_last_bit_overloads_without_counter_penalty() {
    // Bosch Part B 3.2.3/3.2.4/7.2: ACK at52, active flags53..58/54..59,
    // first recessive60, delimiter60..67. A dominant62 restarts error63;
    // dominant67 instead starts overload68. Values are handwritten, no encoder oracle.
    for (fault_bit, expected_length, expected_tec, expected_errors, overloads) in
        [(62, 80, 15, 4, 0), (67, 85, 7, 2, 1)]
    {
        let mut c = config();
        c["faults"] = json!([
            {"request_id":"g:0","attempt":1,"kind":"ack"},
            {"request_id":"g:0","attempt":1,"kind":"bit","bit":fault_bit}
        ]);
        let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
        let failed = records(&s, "attempt")
            .into_iter()
            .find(|a| a["state"] == "failed")
            .unwrap();
        assert_eq!(
            failed["wire_bits"].as_array().unwrap().len(),
            expected_length
        );
        assert_eq!(records(&s, "error").len(), expected_errors);
        assert_eq!(records(&s, "overload").len(), overloads);
        assert_eq!(
            records(&s, "controller")
                .iter()
                .find(|n| n["controller"] == "a")
                .unwrap()["tec"],
            expected_tec
        );
        assert_eq!(records(&s, "reception").len(), 1);
    }
}
#[test]
fn dominant_intermission_first_second_overload_third_is_sof() {
    for (fault_bit, expected_sof, overloads) in [
        (61, 158_000_000u64, 1),
        (62, 160_000_000, 1),
        (63, 126_000_000, 0),
    ] {
        let mut c = config();
        c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":fault_bit}]);
        let mut w = workload(frame(false, false));
        w["generators"][0]["schedule"]["times_ps"] = json!([0, 0]);
        let s = Fixture::new(c, w, "2ms").run();
        let next = records(&s, "attempt")
            .into_iter()
            .find(|a| a["request_id"] == "g:1")
            .unwrap();
        assert_eq!(
            next["sof_ps"],
            expected_sof.to_string(),
            "fault bit {fault_bit}"
        );
        assert_eq!(next["wire_bits"].as_array().unwrap().len(), 61);
        assert_eq!(records(&s, "overload").len(), overloads);
        assert_eq!(records(&s, "error").len(), 0);
        assert_eq!(records(&s, "reception").len(), 2);
    }
}
#[test]
fn overload_flag_and_delimiter_faults_use_same_sampling_and_error_rules() {
    // Internal receiver starts61, joining transmitter62, first recessive68.
    // Fault62 violates both transmitted dominant flags: rule4/5 add8, not RX+1.
    // Fault70 is delimiter form error. Fault75 (last delimiter) is overload.
    for (fault_bit, errors, overloads) in [(62, 2, 1), (70, 2, 1), (75, 0, 2)] {
        let mut c = config();
        c["overloads"] = json!([{"request_id":"g:0","count":1}]);
        c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":fault_bit}]);
        let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
        assert_eq!(records(&s, "error").len(), errors, "bit{fault_bit}");
        assert_eq!(records(&s, "overload").len(), overloads, "bit{fault_bit}");
        let nodes = records(&s, "controller");
        let a = nodes.iter().find(|n| n["controller"] == "a").unwrap();
        let b = nodes.iter().find(|n| n["controller"] == "b").unwrap();
        assert_eq!(a["tec"], if errors == 0 { 0 } else { 8 });
        assert_eq!(
            b["rec"],
            match fault_bit {
                62 => 8,
                70 => 1,
                _ => 0,
            }
        );
        assert_eq!(records(&s, "reception").len(), 1);
        assert_eq!(records(&s, "attempt").len(), 1);
    }
}
#[test]
fn identical_arbitration_is_real_concurrent_transmission() {
    let mut c = config();
    c["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c"}));
    let f = frame(false, false);
    let w = json!({"schema_version":1,"generators":[
        {"id":"left","controller":"a","frame":f,"schedule":{"kind":"explicit.v1","times_ps":[0]}},
        {"id":"right","controller":"b","frame":f,"schedule":{"kind":"explicit.v1","times_ps":[0]}}
    ]});
    let s = Fixture::new(c.clone(), w.clone(), "2ms").run();
    let attempts = records(&s, "attempt");
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts
            .iter()
            .all(|a| a["state"] == "completed" && a["sof_ps"] == "0")
    );
    assert_eq!(attempts[0]["wire_bits"], attempts[1]["wire_bits"]);
    assert_eq!(records(&s, "reception").len(), 1);
    let mut conflicting = w;
    conflicting["generators"][1]["frame"]["data"] = json!([0x92, 0x34]);
    let s = Fixture::new(c, conflicting, "1ms").run();
    let first: Vec<_> = records(&s, "attempt")
        .into_iter()
        .filter(|a| a["attempt"] == 1)
        .collect();
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .all(|a| a["state"] == "failed" && a["sof_ps"] == "0")
    );
    assert!(
        records(&s, "error")
            .iter()
            .any(|e| e["error"] == "bit" && e["detector"] == "b")
    );
    assert!(records(&s, "reception").is_empty());
}
#[test]
fn confinement_every_counter_rule_has_a_boundary_or_exception_vector() {
    // Bosch Part B 8(1)..(11); rule12 is exercised by observed-bus recovery tests.
    let mut n = Counters { tec: 0, rec: 0 };
    n.rx_error(false);
    assert_eq!(n.rec, 1); // 1
    n.rx_error(true);
    assert_eq!(n.rec, 1); // 1 flag-bit exception
    n.dominant_after_flag();
    assert_eq!(n.rec, 9); // 2
    n.tx_error(false, false);
    assert_eq!(n.tec, 8); // 3
    n.tec = 128;
    n.tx_error(true, false);
    assert_eq!(n.tec, 128); // 3 exception1
    n.tx_error(false, true);
    assert_eq!(n.tec, 128); // 3 exception2
    n.flag_bit_error(true);
    assert_eq!(n.tec, 136); // 4
    n.flag_bit_error(false);
    assert_eq!(n.rec, 17); // 5
    n.dominant_extension(true);
    n.dominant_extension(false);
    assert_eq!((n.tec, n.rec), (144, 25)); // 6
    n.tec = 0;
    n.tx_success();
    assert_eq!(n.tec, 0); // 7 zero
    n.tec = 1;
    n.tx_success();
    assert_eq!(n.tec, 0); // 7 nonzero
    for (before, after) in [(0, 0), (1, 0), (127, 126), (128, 120), (255, 120)] {
        n.rec = before;
        n.rx_success();
        assert_eq!(n.rec, after); // 8
    }
    n.tec = 120;
    n.rec = 0;
    n.tx_error(false, false);
    assert_eq!(n.state(), "error_passive"); // 9
    n.tec = 248;
    n.tx_error(false, false);
    assert_eq!(n.state(), "bus_off"); // 10
    n.tec = 128;
    n.rec = 127;
    n.tx_success();
    assert_eq!(n.state(), "error_active"); // 11
    n.rec = 128;
    assert_eq!(n.state(), "error_passive");
}
#[test]
fn passive_ack_exception_is_revoked_by_dominant_in_own_passive_flag() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(128);
    c["controllers"][1]["listen_only"] = json!(true);
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":53}]);
    let s = Fixture::new(c, workload(frame(false, false)), "500us").run();
    let nodes = records(&s, "controller");
    assert_eq!(
        nodes.iter().find(|n| n["controller"] == "a").unwrap()["tec"],
        136
    );
}
#[test]
fn dominant_extension_thresholds_follow_active_and_passive_flag_rules() {
    let mut c = config();
    let mut faults = vec![json!({"request_id":"g:0","attempt":1,"kind":"ack"})];
    faults.extend(
        (60..=75).map(|bit| json!({"request_id":"g:0","attempt":1,"kind":"bit","bit":bit})),
    );
    c["faults"] = json!(faults);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    let nodes = records(&s, "controller");
    assert_eq!(
        nodes.iter().find(|n| n["controller"] == "a").unwrap()["tec"],
        23
    );
    assert_eq!(
        nodes.iter().find(|n| n["controller"] == "b").unwrap()["rec"],
        24
    );
    let failed = records(&s, "attempt")
        .into_iter()
        .find(|a| a["state"] == "failed")
        .unwrap();
    assert_eq!(failed["wire_bits"].as_array().unwrap().len(), 87);
    for (last_bit, expected_tec) in [(65, 136), (66, 144)] {
        let mut c = config();
        c["controllers"][0]["tec"] = json!(128);
        c["controllers"][1]["listen_only"] = json!(true);
        c["faults"] = json!(
            (53..=last_bit)
                .map(|bit| json!({"request_id":"g:0","attempt":1,"kind":"bit","bit":bit}))
                .collect::<Vec<_>>()
        );
        let s = Fixture::new(c, workload(frame(false, false)), "500us").run();
        assert_eq!(
            records(&s, "controller")
                .iter()
                .find(|n| n["controller"] == "a")
                .unwrap()["tec"],
            expected_tec,
            "last dominant{last_bit}"
        );
    }
}
#[test]
fn busy_bus_recovery_counts_observed_sequences_and_local_fault_resets_partial_sequence() {
    for (local_fault, frame_count) in [(false, 128u64), (true, 129)] {
        let mut c = config();
        c["controllers"][0]["tec"] = json!(256);
        c["controllers"][0]["automatic_recovery"] = json!(true);
        c["controllers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c"}));
        if local_fault {
            c["faults"] =
                json!([{"request_id":"z:127","attempt":1,"kind":"bit","bit":60,"receiver":"a"}]);
        }
        let f = frame(false, false);
        let w = json!({"schema_version":1,"generators":[
            {"id":"g","controller":"a","frame":f,"schedule":{"kind":"explicit.v1","times_ps":[0]}},
            {"id":"z","controller":"b","frame":f,"schedule":{"kind":"explicit.v1","times_ps":vec![0;130]}}
        ]});
        let s = Fixture::new(c, w, "25ms").run();
        let a = records(&s, "attempt")
            .into_iter()
            .find(|a| a["request_id"] == "g:0")
            .unwrap();
        assert_eq!(a["sof_ps"], (frame_count * 64 * 2_000_000).to_string());
        assert_eq!(a["state"], "completed");
    }
}
#[test]
fn checkpoint_continues_during_resignalling_overload_and_concurrent_transmitters() {
    use dir_simulator::runtime::session::*;
    let mut c = config();
    c["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c"}));
    c["faults"] = json!([
        {"request_id":"g:0","attempt":1,"kind":"ack"},
        {"request_id":"g:0","attempt":1,"kind":"bit","bit":67}
    ]);
    c["overloads"] = json!([{"request_id":"g:0","count":1}]);
    let mut w = workload(frame(false, false));
    let mut peer = w["generators"][0].clone();
    peer["id"] = json!("p");
    peer["controller"] = json!("b");
    w["generators"].as_array_mut().unwrap().push(peer);
    let f = Fixture::new(c, w, "2ms");
    let expected = f.run();
    for boundary in [60, 70, 80, 150, 160] {
        let (mut s, ctl) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..boundary {
            s.advance_one().unwrap();
        }
        ctl.submit(ControlCommand::Pause).unwrap();
        s.poll_control().unwrap();
        let path = f.0.join(format!("protocol-phase-{boundary}"));
        let ack = s.checkpoint(&path, CheckpointLimits::default()).unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
        let (mut restored, token) =
            restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
        restored.resume(token).unwrap();
        assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
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
            serde_json::to_value(&expected.model_records).unwrap()
        );
    }
}
#[test]
fn arbitration_stuff_exception_and_error_passive_entry_drive_active_flags() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(120);
    let mut f = frame(false, false);
    f.id = 0;
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"stuff","bit":5}]);
    let s = Fixture::new(c, workload(f), "2ms").run();
    let errors = records(&s, "error");
    assert!(
        errors
            .iter()
            .any(|e| e["detector"] == "a" && e["error"] == "stuff" && e["flag"] == "active")
    );
    assert_eq!(
        records(&s, "controller")
            .iter()
            .find(|n| n["controller"] == "a")
            .unwrap()["tec"],
        119
    );
    for receiver in [false, true] {
        let mut c = config();
        c["controllers"][if receiver { 1 } else { 0 }][if receiver { "rec" } else { "tec" }] =
            json!(if receiver { 127 } else { 120 });
        c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":if receiver {51}else{25},"receiver":if receiver {json!("b")}else{Value::Null}}]);
        let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
        assert!(records(&s,"error").iter().any(|e|e["detector"]==if receiver {"b"}else{"a"} && e["flag"]=="active"));
    }
}
#[test]
fn acknowledgement_driver_monitors_corruption_and_does_not_decrement_rec() {
    let mut c = config();
    c["controllers"][1]["rec"] = json!(10);
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":52,"receiver":"b"}]);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    assert!(
        records(&s, "error")
            .iter()
            .any(|e| e["detector"] == "b" && e["error"] == "bit" && e["detected_bit"] == 52)
    );
    assert_eq!(records(&s, "attempt").len(), 2);
    assert_eq!(
        records(&s, "controller")
            .iter()
            .find(|n| n["controller"] == "b")
            .unwrap()["rec"],
        18
    );
}
#[test]
fn scheduled_cancellation_removes_queued_and_defers_future_request_acknowledgement() {
    let mut c = config();
    c["cancellations"] = json!([
        {"request_id":"g:1","at_ps":10_000_000},
        {"request_id":"g:2","at_ps":20_000_000}
    ]);
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"]["times_ps"] = json!([0, 0, 300_000_000]);
    let s = Fixture::new(c, w, "2ms").run();
    assert_eq!(records(&s, "attempt").len(), 1);
    let requests = records(&s, "request");
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["state"] == "cancelled")
            .count(),
        2
    );
    let cancellations = records(&s, "cancellation");
    assert!(
        cancellations
            .iter()
            .all(|r| r["state"] == "acknowledged" && r["physical_outcome"] == "not_started")
    );
    assert_eq!(
        cancellations
            .iter()
            .find(|r| r["request_id"] == "g:2")
            .unwrap()["observed_ps"],
        "300000000"
    );
}
#[test]
fn active_cancellation_preserves_wire_success_or_failure_and_only_suppresses_retry() {
    for fail in [false, true] {
        let mut c = config();
        c["cancellations"] = json!([{"request_id":"g:0","at_ps":10_000_000}]);
        if fail {
            c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
        }
        let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
        let a = records(&s, "attempt");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0]["state"], if fail { "failed" } else { "completed" });
        assert_eq!(
            a[0]["wire_bits"].as_array().unwrap().len(),
            if fail { 71 } else { 61 }
        );
        let req = records(&s, "request");
        assert_eq!(
            req[0]["state"],
            if fail { "cancelled" } else { "completed" }
        );
        assert_eq!(req[0]["cancel_requested"], true);
        assert_eq!(records(&s, "reception").len(), usize::from(!fail));
        let cancel = records(&s, "cancellation");
        assert_eq!(cancel[0]["state"], "acknowledged");
        assert_eq!(
            cancel[0]["physical_outcome"],
            if fail { "failed" } else { "completed" }
        );
    }
}
#[test]
fn cancellation_equal_generation_and_eof_times_and_too_late_outcome_are_explicit() {
    for (time, state, attempts) in [
        (0, "cancelled", 0),
        (122_000_000, "completed", 1),
        (130_000_000, "completed", 1),
    ] {
        let mut c = config();
        c["cancellations"] = json!([{"request_id":"g:0","at_ps":time}]);
        let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
        assert_eq!(records(&s, "request")[0]["state"], state);
        assert_eq!(records(&s, "attempt").len(), attempts);
        assert_eq!(
            records(&s, "cancellation")[0]["state"],
            if time == 130_000_000 {
                "too_late"
            } else {
                "acknowledged"
            }
        );
    }
    for id in ["missing:0", "g:1", "g:00", "response:g:0:1"] {
        let mut c = config();
        c["cancellations"] = json!([{"request_id":id,"at_ps":0}]);
        let f = Fixture::new(c, workload(frame(false, false)), "2ms");
        assert!(
            dir_simulator::prepare_with_registry(&f.0.join("protocol.ini"), registry()).is_err(),
            "{id}"
        );
    }
}
#[test]
fn checkpoint_before_cancellation_keeps_order_and_real_failed_attempt() {
    use dir_simulator::runtime::session::*;
    let mut c = config();
    c["cancellations"] = json!([{"request_id":"g:0","at_ps":100_000_000}]);
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
    let f = Fixture::new(c, workload(frame(false, false)), "2ms");
    let expected = f.run();
    let (mut s, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..10 {
        s.advance_one().unwrap();
    }
    ctl.submit(ControlCommand::Pause).unwrap();
    s.poll_control().unwrap();
    let path = f.0.join("before-cancel");
    let ack = s.checkpoint(&path, CheckpointLimits::default()).unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    let (mut restored, token) =
        restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
    restored.resume(token).unwrap();
    restored.run_until_yield().unwrap();
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
        serde_json::to_value(&expected.model_records).unwrap()
    );
}
#[test]
fn strict_restore_rejects_semantic_tampering_after_valid_bundle_hashes_are_rebuilt() {
    use dir_simulator::runtime::{
        checkpoint_bundle::{read_bundle, write_bundle},
        session::*,
    };
    use sha2::{Digest, Sha256};
    let f = Fixture::new(config(), workload(frame(false, false)), "2ms");
    let (mut s, ctl) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..10 {
        s.advance_one().unwrap();
    }
    ctl.submit(ControlCommand::Pause).unwrap();
    s.poll_control().unwrap();
    let original = f.0.join("semantic-source");
    let limits = CheckpointLimits::default();
    assert_eq!(
        s.checkpoint(&original, limits.clone()).unwrap().status,
        ControlStatus::Applied
    );
    let (manifest, files) = read_bundle(&original, &limits).unwrap();
    let engine: Value = serde_json::from_slice(&files["state.json"]).unwrap();
    let old = engine["codec_refs"][0]["blob_path"].as_str().unwrap();
    let raw: Value = serde_json::from_slice(&files[old]).unwrap();
    type Mutation = fn(&mut Value);
    let cases: [(&str, Mutation); 9] = [
        ("position", |v| v["active"]["position"] = json!(0)),
        ("samples", |v| v["active"]["samples"][0] = json!([])),
        ("wire", |v| v["active"]["wire"]["crc"] = json!(0)),
        ("timer_missing", |v| v["scheduled"] = json!([])),
        ("timer_deadline", |v| v["scheduled"][0]["at"] = json!(0)),
        ("timer_owner", |v| {
            v["scheduled"][0]["token"]["owner"] = json!("other")
        }),
        ("counter", |v| v["nodes"][0]["counters"]["tec"] = json!(257)),
        ("request_source", |v| {
            v["active"]["request"]["id"] = json!("unknown:0")
        }),
        ("unknown", |v| v["unknown"] = json!(true)),
    ];
    for (name, mutate) in cases {
        let mut raw = raw.clone();
        mutate(&mut raw);
        let bytes = serde_json::to_vec(&raw).unwrap();
        let new = format!("objects/{:x}.bin", Sha256::digest(&bytes));
        let mut changed = files.clone();
        changed.remove(old);
        changed.insert(new.clone(), bytes);
        let mut engine = engine.clone();
        engine["codec_refs"][0]["blob_path"] = json!(new);
        changed.insert("state.json".into(), serde_json::to_vec(&engine).unwrap());
        let path = f.0.join(format!("tampered-{name}"));
        let mut updated_manifest = manifest.clone();
        updated_manifest["codecs"] = engine["codec_refs"].clone();
        write_bundle(&path, updated_manifest, changed, &limits, 0).unwrap();
        let before = std::fs::read(path.join("manifest.json")).unwrap();
        let error = restore_checkpoint(&path, registry(), limits.clone())
            .err()
            .unwrap();
        assert_eq!(error.code, "RestoreFailed", "{name}: {error:?}");
        assert_eq!(std::fs::read(path.join("manifest.json")).unwrap(), before);
    }
    assert!(restore_checkpoint(&original, registry(), limits).is_ok());
}
#[test]
fn arbitration_withdraws_on_sampled_identifier_and_loser_acknowledges_winner() {
    let mut low = frame(false, false);
    low.id = 0;
    let mut high = frame(false, false);
    high.id = 0x400;
    let w = json!({"schema_version":1,"generators":[
        {"id":"low","controller":"a","frame":low,"schedule":{"kind":"explicit.v1","times_ps":[0]}},
        {"id":"high","controller":"b","frame":high,"schedule":{"kind":"explicit.v1","times_ps":[0]}}
    ]});
    let s = Fixture::new(config(), w.clone(), "2ms").run();
    let attempts = records(&s, "attempt");
    let loser = attempts
        .iter()
        .find(|a| a["state"] == "arbitration_lost")
        .unwrap();
    assert_eq!(loser["request_id"], "high:0");
    assert_eq!(loser["lost_bit"], 1);
    assert!(
        records(&s, "reception")
            .iter()
            .any(|r| r["receiver"] == "b" && r["request_id"] == "low:0")
    );
    assert!(records(&s, "controller").iter().all(|r| r["tec"] == 0));
    // Local corruption hides the winner's dominant ID bit from b: b continues,
    // detects later contention as an ordinary bit error, rather than being
    // preselected out by the nominal identifiers.
    let mut c = config();
    c["faults"] = json!([{"request_id":"low:0","attempt":1,"kind":"bit","bit":1,"receiver":"b"}]);
    let s = Fixture::new(c, w, "2ms").run();
    assert!(records(&s, "error").iter().any(|e| e["detector"] == "b"));
    assert!(
        records(&s, "attempt")
            .iter()
            .any(|a| a["state"] == "failed" && a["request_id"] == "high:0" && a["attempt"] == 1)
    );
}
#[test]
fn cancelling_one_concurrent_sender_preserves_other_sender_and_real_receiver_delivery() {
    for fail in [false, true] {
        let mut c = config();
        c["controllers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c"}));
        c["cancellations"] = json!([{"request_id":"g:0","at_ps":10_000_000}]);
        if fail {
            c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
        }
        let mut w = workload(frame(false, false));
        let mut peer = w["generators"][0].clone();
        peer["id"] = json!("p");
        peer["controller"] = json!("b");
        w["generators"].as_array_mut().unwrap().push(peer);
        let s = Fixture::new(c, w, "2ms").run();
        let requests = records(&s, "request");
        let cancelled = requests.iter().find(|r| r["request_id"] == "g:0").unwrap();
        let peer = requests.iter().find(|r| r["request_id"] == "p:0").unwrap();
        assert_eq!(
            cancelled["state"],
            if fail { "cancelled" } else { "completed" }
        );
        assert_eq!(peer["state"], "completed");
        assert_eq!(peer["attempt"], if fail { 2 } else { 1 });
        assert_eq!(records(&s, "reception").len(), if fail { 2 } else { 1 });
    }
}
#[test]
fn receiver_ack_eligibility_is_parsed_from_complete_crc_checked_prefix() {
    let f = frame(false, false);
    let w = codec::encode(&f).unwrap();
    assert_eq!(w.crc, 0x36b7);
    assert_eq!(w.stuff_positions, vec![17]);
    assert_eq!(w.ack, 52);
    for length in 0..=w.bits.len() {
        assert_eq!(
            codec::acknowledgement_ready(&w.bits[..length]),
            length == 52,
            "prefix{length}"
        );
    }
    let mut wrong_dlc = w.bits[..52].to_vec();
    wrong_dlc[15] ^= 1;
    assert!(!codec::acknowledgement_ready(&wrong_dlc));
}

#[test]
fn local_second_intermission_overload_does_not_freeze_peer_third_sof() {
    assert_eq!(codec::encode(&frame(false, false)).unwrap().bits.len(), 61);
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":62,"receiver":"b"}]);
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"]["times_ps"] = json!([0, 0]);
    let s = Fixture::new(c, w, "2ms").run();
    let attempts = records(&s, "attempt");
    let next = attempts
        .iter()
        .find(|a| a["request_id"] == "g:1" && a["attempt"] == 1)
        .unwrap();
    assert_eq!(next["sof_ps"], "126000000");
    assert_eq!(
        next["state"], "arbitration_lost",
        "b's independent dominant overload must still drive after occupation replacement"
    );
    assert!(next["lost_bit"].as_u64().unwrap() <= 6);
}
#[test]
fn receiver_local_third_intermission_sof_exists_when_resolved_bus_is_recessive() {
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":63,"receiver":"b"}]);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    let sof = records(&s, "local_sof");
    assert!(sof.iter().any(|r| r["controller"] == "b"
        && r["resolved_level"] == 1
        && r["sample"] == 0
        && r["sof_ps"] == "126000000"));
    assert!(
        !sof.iter()
            .any(|r| r["controller"] == "a" && r["physical_bit"] == 63)
    );
    assert_eq!(
        records(&s, "attempt").len(),
        1,
        "local noise must not create a fictitious transmitter"
    );
}
#[test]
fn retry_is_settled_before_local_third_sof_and_occupation_excludes_that_sof() {
    let mut c = config();
    c["faults"] = json!([
        {"request_id":"g:0","attempt":1,"kind":"ack"},
        {"request_id":"g:0","attempt":1,"kind":"bit","bit":70}
    ]);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    let attempts = records(&s, "attempt");
    let first = attempts.iter().find(|a| a["attempt"] == 1).unwrap();
    let retry = attempts.iter().find(|a| a["attempt"] == 2).unwrap();
    assert_eq!(first["release_ps"], "140000000");
    assert_eq!(first["wire_bits"].as_array().unwrap().len(), 70);
    assert_eq!(retry["sof_ps"], "140000000");
    assert_eq!(retry["state"], "completed");
    assert_eq!(attempts.len(), 2);
}
#[test]
fn prior_passive_sender_receives_third_sof_while_arbitration_loser_joins() {
    let mut c = config();
    c["controllers"][0]["tec"] = json!(136);
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":63}]);
    let mut w = workload(frame(false, false));
    w["generators"][0]["schedule"]["times_ps"] = json!([0, 0]);
    let mut peer = w["generators"][0].clone();
    peer["id"] = json!("p");
    peer["controller"] = json!("b");
    peer["frame"]["id"] = json!(0x400);
    peer["schedule"]["times_ps"] = json!([0]);
    w["generators"].as_array_mut().unwrap().push(peer);
    let s = Fixture::new(c, w, "2ms").run();
    let attempts = records(&s, "attempt");
    let peer = attempts
        .iter()
        .find(|a| a["request_id"] == "p:0" && a["state"] == "completed")
        .unwrap();
    assert_eq!(peer["sof_ps"], "126000000");
    assert!(
        !attempts
            .iter()
            .any(|a| a["request_id"] == "g:1" && a["sof_ps"] == "126000000")
    );
    assert!(
        records(&s, "reception")
            .iter()
            .any(|r| r["receiver"] == "a" && r["request_id"] == "p:0")
    );
}

#[test]
fn genuine_production_v1_state_migrates_only_before_ambiguous_local_recovery() {
    let raw = include_bytes!("fixtures/can_protocol_state_v1.json");
    let migrated = dir_simulator::runtime::can_protocol::migrate_v1_state(raw).unwrap();
    let old: Value = serde_json::from_slice(raw).unwrap();
    let new: Value = serde_json::from_slice(&migrated).unwrap();
    assert_eq!(old["active"]["position"], new["active"]["position"]);
    assert_eq!(old["scheduled"], new["scheduled"]);
    assert_eq!(new["bit_tick"], 7);
    assert_eq!(new["nodes"][0]["phase"], json!({"phase":"Frame"}));
    let mut ambiguous = old;
    ambiguous["active"]["stage"] = json!("intermission");
    assert!(
        dir_simulator::runtime::can_protocol::migrate_v1_state(
            &serde_json::to_vec(&ambiguous).unwrap()
        )
        .is_err()
    );
}

#[test]
fn cancelling_settled_retry_during_recovery_retains_failed_physical_outcome() {
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"}]);
    c["cancellations"] = json!([{"request_id":"g:0","at_ps":120_000_000}]);
    let s = Fixture::new(c, workload(frame(false, false)), "2ms").run();
    assert_eq!(records(&s, "attempt").len(), 1);
    let request = records(&s, "request").remove(0);
    assert_eq!(request["state"], "cancelled");
    assert_eq!(request["physical_outcome"], "failed");
    assert_eq!(records(&s, "cancellation")[0]["physical_outcome"], "failed");
}
#[test]
fn receiver_validity_precedes_transmitter_eof_and_survives_last_eof_local_overload() {
    let wire = codec::encode(&frame(false, false)).unwrap();
    assert_eq!(wire.bits.len(), 61);
    assert!(codec::receiver_valid(&wire.bits[..59]).unwrap().is_none());
    assert!(codec::receiver_valid(&wire.bits[..60]).unwrap().is_some());
    let mut c = config();
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":60,"receiver":"b"}]);
    let f = Fixture::new(c, workload(frame(false, false)), "2ms");
    let s = f.run();
    assert_eq!(records(&s, "reception").len(), 1);
    assert_eq!(records(&s, "request")[0]["state"], "completed");
    assert_eq!(records(&s, "overload").len(), 1);
    assert!(!records(&s, "error").iter().any(|e| e["detector"] == "b"));
}

#[test]
fn checkpoint_preserves_divergent_local_overload_sof_retry_and_suspension_boundaries() {
    use dir_simulator::runtime::session::*;
    for scenario in ["local_overload", "local_sof", "failed_retry", "passive"] {
        let mut c = config();
        let mut w = workload(frame(false, false));
        let boundaries = if scenario == "failed_retry" {
            c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"ack"},{"request_id":"g:0","attempt":1,"kind":"bit","bit":70}]);
            vec![69, 70, 71, 72]
        } else {
            if scenario == "local_sof" {
                c["faults"] =
                    json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":63,"receiver":"b"}]);
            } else {
                w["generators"][0]["schedule"]["times_ps"] = json!([0, 0]);
                c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"bit","bit":if scenario=="passive"{63}else{62},"receiver":if scenario=="passive"{serde_json::Value::Null}else{json!("b")}}]);
                if scenario == "passive" {
                    c["controllers"][0]["tec"] = json!(136);
                    let mut peer = w["generators"][0].clone();
                    peer["id"] = json!("p");
                    peer["controller"] = json!("b");
                    peer["frame"]["id"] = json!(0x400);
                    peer["schedule"]["times_ps"] = json!([0]);
                    w["generators"].as_array_mut().unwrap().push(peer);
                }
            }
            vec![63, 64, 65, 66]
        };
        let f = Fixture::new(c, w, "2ms");
        let expected = f.run();
        for boundary in boundaries {
            let (mut s, controller) = RunSession::new(
                f.prepared(),
                RunOptions {
                    checkpoint_enabled: true,
                    ..Default::default()
                },
            )
            .unwrap();
            for _ in 0..boundary {
                s.advance_one().unwrap();
            }
            controller.submit(ControlCommand::Pause).unwrap();
            s.poll_control().unwrap();
            let path = f.0.join(format!("{scenario}-{boundary}"));
            let ack = s.checkpoint(&path, CheckpointLimits::default()).unwrap();
            assert_eq!(
                ack.status,
                ControlStatus::Applied,
                "{scenario}/{boundary}: {:?}",
                ack.error
            );
            let (mut restored, token) =
                restore_checkpoint(&path, registry(), CheckpointLimits::default()).unwrap();
            restored.resume(token).unwrap();
            assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
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
                serde_json::to_value(&expected.model_records).unwrap(),
                "{scenario}/{boundary}"
            );
        }
    }
}
