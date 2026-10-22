#[path = "support/can_compound.rs"]
mod support;
use serde_json::json;
use support::*;
#[test]
fn actual_cc_and_fd_receivers_ack_and_decode_all_supported_lengths() {
    for frame in [
        cc(0x123),
        fd(0x123, false, 0),
        fd(0x123, true, 2),
        fd(0x123, true, 64),
    ] {
        let s = Fixture::new(
            config(),
            workload(frame.clone()),
            "can.mixed.protocol.v1",
            "2ms",
        )
        .run();
        assert!(
            rows(&s, "request")
                .iter()
                .any(|r| r["request_id"] == "g:0" && r["state"] == "completed"),
            "{frame}: {:?}",
            rows(&s, "error")
        );
        let received = rows(&s, "reception");
        assert_eq!(received.len(), 1);
        assert_eq!(received[0]["frame"]["data"], frame["data"]);
        assert!(
            rows(&s, "sample")
                .iter()
                .any(|r| r["controller"] == "a" && r["field"] == "ack" && r["sample"] == 0)
        );
    }
}
#[test]
fn no_receiver_means_actual_missing_ack_despite_ideal_codec_ack_bit() {
    let mut c = config();
    c["buses"][0]["controllers"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let s = Fixture::new(
        c,
        workload(fd(0x123, true, 2)),
        "can.mixed.protocol.v1",
        "300us",
    )
    .run();
    assert!(
        !rows(&s, "request")
            .iter()
            .any(|r| r["state"] == "completed")
    );
    assert!(rows(&s, "error").iter().any(|r| r["error"] == "ack"));
    assert!(rows(&s, "reception").is_empty());
}
#[test]
fn cc_only_samples_stay_nominal_during_fd_brs_and_reserved_fdf_is_not_immediate_form_error() {
    let mut c = config();
    c["buses"][0]["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c","capability":"cc_only"}));
    let s = Fixture::new(
        c,
        workload(fd(0x123, true, 64)),
        "can.mixed.protocol.v1",
        "300us",
    )
    .run();
    let samples = rows(&s, "sample");
    let local: Vec<_> = samples.iter().filter(|r| r["controller"] == "c").collect();
    assert!(!local.is_empty());
    assert!(local.iter().all(|r| r["rate"] == "nominal"));
    assert!(
        samples
            .iter()
            .any(|r| r["controller"] == "a" && r["rate"] == "data")
    );
    assert!(local.iter().any(|r| r["field"] == "fdf_reserved"));
}

fn decimal(value: &serde_json::Value, key: &str) -> u128 {
    value[key].as_str().unwrap().parse().unwrap()
}
fn earlier(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
    (decimal(a, "exact_num") * decimal(b, "exact_den"))
        .cmp(&(decimal(b, "exact_num") * decimal(a, "exact_den")))
}
#[test]
fn data_stuff_and_fixed_form_errors_return_each_detector_to_its_own_nominal_boundary() {
    use dir_simulator::runtime::canfd::codec;
    let frame = codec::Frame {
        id: 0x123,
        extended: false,
        brs: true,
        esi: false,
        data: vec![0; 64],
    };
    let wire = codec::encode(&frame).unwrap();
    let dynamic = *wire
        .dynamic_stuff_positions
        .iter()
        .find(|&&bit| bit > wire.brs_index + 8)
        .unwrap();
    for (bit, kind) in [(dynamic, "stuff"), (wire.fixed_stuff_positions[0], "form")] {
        for passive in [false, true] {
            let mut c = config();
            if passive {
                c["buses"][0]["controllers"][1]["rec"] = json!(128);
            }
            c["buses"][0]["controllers"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":"c","capability":"fd_capable"}));
            c["faults"] =
                json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":bit,"receiver":"b"}]);
            let mut source = fd(0x123, true, 64);
            source["data"] = json!(vec![0; 64]);
            let s = Fixture::new(c, workload(source), "can.mixed.protocol.v1", "2ms").run();
            let errors = rows(&s, "error");
            let receiver = errors
                .iter()
                .filter(|r| r["detector"] == "b")
                .min_by(|a, b| earlier(a, b))
                .unwrap();
            assert_eq!(receiver["error"], kind);
            assert_eq!(receiver["local_index"], bit);
            assert_eq!(receiver["flag"], if passive { "passive" } else { "active" });
            let start = decimal(receiver, "flag_start_num");
            let start_den = decimal(receiver, "flag_start_den");
            let detection = decimal(receiver, "exact_num");
            let den = decimal(receiver, "exact_den");
            assert_eq!(
                start * den - detection * start_den,
                500000 * start_den * den,
                "nominal remainder after the actual data sample"
            );
            let request = rows(&s, "request")
                .into_iter()
                .find(|r| r["request_id"] == "g:0")
                .unwrap();
            assert_eq!(request["state"], "completed");
            assert_eq!(request["attempt"].as_u64().unwrap() == 1, passive);
            if !passive {
                assert!(
                    errors
                        .iter()
                        .any(|r| r["detector"] == "a" && earlier(receiver, r).is_lt())
                );
            }
        }
    }
}
#[test]
fn invalid_crc_waits_for_ack_delimiter_while_a_healthy_receiver_really_acks() {
    use dir_simulator::runtime::canfd::codec;
    let frame = codec::Frame {
        id: 0x123,
        extended: false,
        brs: true,
        esi: false,
        data: vec![0x5a; 12],
    };
    let wire = codec::encode(&frame).unwrap();
    let mut c = config();
    c["buses"][0]["controllers"][1]["rec"] = json!(128);
    c["buses"][0]["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c","capability":"fd_capable"}));
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":wire.crc_delimiter_index-1,"receiver":"b"}]);
    let s = Fixture::new(
        c,
        workload(fd(0x123, true, 12)),
        "can.mixed.protocol.v1",
        "2ms",
    )
    .run();
    let error = rows(&s, "error")
        .into_iter()
        .find(|r| r["detector"] == "b")
        .unwrap();
    assert_eq!(error["error"], "crc");
    assert_eq!(error["local_index"], wire.crc_delimiter_index + 2);
    assert!(
        rows(&s, "sample")
            .iter()
            .any(|r| r["controller"] == "a" && r["field"] == "ack" && r["sample"] == 0)
    );
    assert_eq!(rows(&s, "request")[0]["state"], "completed");
    assert_eq!(rows(&s, "request")[0]["attempt"], 1);
    assert!(rows(&s, "reception").iter().any(|r| r["receiver"] == "c"));
    assert!(!rows(&s, "reception").iter().any(|r| r["receiver"] == "b"));
}
#[test]
fn same_id_cc_fd_is_a_real_post_arbitration_bit_error_but_rtr_loses_arbitration() {
    for remote in [false, true] {
        let mut classic = cc(0x123);
        if remote {
            classic["remote"] = json!(true);
            classic["data"] = json!([]);
        }
        let mut w = workload(classic);
        w["generators"].as_array_mut().unwrap().push(json!({"id":"fd","bus":"bus","controller":"b","frame":fd(0x123,false,2),"schedule":{"kind":"explicit.v1","times_ps":[0]}}));
        let mut c = config();
        c["buses"][0]["controllers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c","capability":"fd_capable"}));
        let s = Fixture::new(c, w, "can.mixed.protocol.v1", "600us").run();
        if remote {
            assert!(
                rows(&s, "attempt")
                    .iter()
                    .any(|r| r["request_id"] == "g:0" && r["state"] == "arbitration_lost")
            );
        } else {
            assert!(
                rows(&s, "error")
                    .iter()
                    .any(|r| r["detector"] == "b" && r["error"] == "bit")
            );
            assert!(
                !rows(&s, "attempt")
                    .iter()
                    .any(|r| r["request_id"] == "fd:0" && r["state"] == "arbitration_lost")
            );
        }
    }
}
#[test]
fn passive_missing_ack_exception_is_revoked_by_late_dominant_in_its_own_flag() {
    use dir_simulator::runtime::canfd::codec;
    let wire = codec::encode(&codec::Frame {
        id: 0x123,
        extended: false,
        brs: true,
        esi: true,
        data: vec![0x5a; 2],
    })
    .unwrap();
    for dominant in [false, true] {
        let mut c = config();
        c["buses"][0]["controllers"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        c["buses"][0]["controllers"][0]["tec"] = json!(128);
        if dominant {
            c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":wire.crc_delimiter_index+4}]);
        }
        let s = Fixture::new(
            c,
            workload(fd(0x123, true, 2)),
            "can.mixed.protocol.v1",
            "300us",
        )
        .run();
        let controller = rows(&s, "controller").remove(0);
        assert_eq!(controller["tec"], if dominant { 136 } else { 128 });
    }
}
#[test]
fn compound_checkpoints_resume_at_brs_crc_pending_and_asynchronous_error_boundaries() {
    use dir_simulator::runtime::canfd::codec;
    use dir_simulator::runtime::session::*;
    let wire = codec::encode(&codec::Frame {
        id: 0x123,
        extended: false,
        brs: true,
        esi: false,
        data: vec![0; 64],
    })
    .unwrap();
    let dynamic = *wire
        .dynamic_stuff_positions
        .iter()
        .find(|&&bit| bit > wire.brs_index + 8)
        .unwrap();
    for scenario in ["brs", "crc_delimiter", "crc_pending", "asynchronous"] {
        let mut c = config();
        c["buses"][0]["controllers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c","capability":"fd_capable"}));
        if scenario == "crc_pending" {
            c["buses"][0]["controllers"][1]["rec"] = json!(128);
            c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":wire.crc_delimiter_index-1,"receiver":"b"}]);
        }
        if scenario == "asynchronous" {
            c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":dynamic,"receiver":"b"}]);
        }
        let mut source = fd(0x123, true, 64);
        source["data"] = json!(vec![0; 64]);
        let f = Fixture::new(c, workload(source), "can.mixed.protocol.v1", "2ms");
        let expected = f.run();
        let (mut session, controller) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut found = false;
        for _ in 0..5000 {
            assert_ne!(session.advance_one().unwrap(), RunState::Failed);
            let records = &session
                .snapshot()
                .registered
                .as_ref()
                .unwrap()
                .model_records;
            if scenario == "asynchronous" {
                found = records
                    .values()
                    .any(|r| r.data["kind"] == "error" && r.data["detector"] == "b")
                    && !records
                        .values()
                        .any(|r| r.data["kind"] == "error" && r.data["detector"] == "a");
            } else {
                let field = if scenario == "brs" {
                    "brs"
                } else {
                    "crc_delimiter"
                };
                found = records.values().any(|r| {
                    r.data["kind"] == "sample"
                        && r.data["controller"] == "b"
                        && r.data["field"] == field
                });
            }
            if found {
                break;
            }
        }
        assert!(found, "{scenario}");
        controller.submit(ControlCommand::Pause).unwrap();
        session.poll_control().unwrap();
        let path = f.0.join("checkpoint");
        let ack = session
            .checkpoint(&path, CheckpointLimits::default())
            .unwrap();
        assert_eq!(ack.status, ControlStatus::Applied, "{scenario}: {ack:?}");
        let (mut restored, _) =
            restore_checkpoint(&path, Fixture::registry(), CheckpointLimits::default()).unwrap();
        restored.resume(restored.pause_token().unwrap()).unwrap();
        while restored.state() == RunState::Running {
            restored.advance_one().unwrap();
        }
        assert_eq!(restored.state(), RunState::Completed, "{scenario}");
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
            "{scenario}"
        );
    }
}

#[test]
fn rational_split_clocks_match_absolute_analytic_samples_without_per_bit_rounding() {
    let mut c = config();
    c["buses"][0]["nominal_bps"] = json!(333333);
    c["buses"][0]["data_bps"] = json!(1700003);
    c["buses"][0]["nominal_sample"] = json!({"num":2,"den":3});
    c["buses"][0]["data_sample"] = json!({"num":3,"den":7});
    let s = Fixture::new(
        c,
        workload(fd(0x123, true, 64)),
        "can.mixed.protocol.v1",
        "3ms",
    )
    .run();
    assert_eq!(rows(&s, "request")[0]["state"], "completed");
    let mut samples: Vec<_> = rows(&s, "sample")
        .into_iter()
        .filter(|r| r["controller"] == "b")
        .collect();
    samples.sort_by(earlier);
    // Every actual sample satisfies its independently derived interval: nominal,
    // BRS -> data sample, data, CRC delimiter -> nominal sample, nominal.
    let nominal_num = 1_000_000_000_000u128;
    let nominal_den = 333333u128;
    let data_den = 1700003u128;
    for pair in samples.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        let delta_num = decimal(b, "exact_num") * decimal(a, "exact_den")
            - decimal(a, "exact_num") * decimal(b, "exact_den");
        let delta_den = decimal(b, "exact_den") * decimal(a, "exact_den");
        let (expected_num, expected_den) = if a["field"] == "brs" {
            (nominal_num, data_den)
        } else if a["field"] == "crc_delimiter" {
            (nominal_num, nominal_den)
        } else if a["rate"] == "data" {
            (nominal_num, data_den)
        } else {
            (nominal_num, nominal_den)
        };
        assert_eq!(
            delta_num * expected_den,
            expected_num * delta_den,
            "{a} -> {b}"
        );
    }
    let records = rows(&s, "sample");
    for local in records.iter().filter(|r| r["controller"] == "b") {
        let peers: Vec<_> = records
            .iter()
            .filter(|r| {
                r["exact_num"] == local["exact_num"] && r["exact_den"] == local["exact_den"]
            })
            .collect();
        assert!(
            peers
                .iter()
                .all(|r| r["resolved_level"] == local["resolved_level"])
        );
    }
}
#[test]
fn confinement_derives_esi_and_cancellation_retains_active_physical_truth() {
    for (at, expected, completed) in [
        (0, "future_suppressed", false),
        (1_000_000, "active_retry_suppressed", true),
        (500_000_000, "too_late", true),
    ] {
        let mut c = config();
        c["buses"][0]["controllers"][0]["tec"] = json!(128);
        c["cancellations"] = json!([{"request_id":"g:0","at_ps":at}]);
        let s = Fixture::new(
            c,
            workload(fd(0x123, true, 2)),
            "can.mixed.protocol.v1",
            "1ms",
        )
        .run();
        assert_eq!(rows(&s, "cancellation")[0]["state"], expected);
        let received = rows(&s, "reception");
        assert_eq!(!received.is_empty(), completed);
        if completed {
            assert_eq!(received[0]["frame"]["esi"], true);
            assert_eq!(rows(&s, "request")[0]["state"], "completed");
        } else {
            assert!(rows(&s, "attempt").is_empty());
        }
    }
}
#[test]
fn unsupported_capabilities_and_noncanonical_selectors_fail_preparation() {
    for change in [
        "fd_tolerant",
        "pxhd",
        "sample",
        "standard",
        "cancel",
        "duplicate_fault",
    ] {
        let mut c = config();
        match change {
            "fd_tolerant" => c["buses"][0]["controllers"][1]["capability"] = json!("fd_tolerant"),
            "pxhd" => c["buses"][0]["pxhd"] = json!(false),
            "sample" => c["buses"][0]["data_sample"] = json!({"num":0,"den":1}),
            "standard" => c["standard_edition"] = json!("all-ISO-editions"),
            "cancel" => c["cancellations"] = json!([{"request_id":"g:00","at_ps":1}]),
            _ => {
                c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":3},{"request_id":"g:0","attempt":1,"kind":"flip","bit":3}])
            }
        }
        let f = Fixture::new(c, workload(cc(0x123)), "can.mixed.protocol.v1", "1ms");
        assert!(
            dir_simulator::prepare_with_registry(&f.0.join("compound.ini"), Fixture::registry())
                .is_err(),
            "{change}"
        );
    }
}

#[test]
fn independent_bus_exact_instants_with_the_same_ceil_keep_their_private_order() {
    let mut c = config();
    let mut a = bus("bus");
    let mut b = bus("other");
    a["nominal_bps"] = json!(1000000);
    a["nominal_sample"] = json!({"num":1,"den":3});
    b["nominal_bps"] = json!(1000000);
    b["nominal_sample"] = json!({"num":333334,"den":1000000});
    c["buses"] = json!([a, b]);
    let mut w = workload(cc(0x123));
    w["generators"].as_array_mut().unwrap().push(json!({"id":"h","bus":"other","controller":"a","frame":cc(0x123),"schedule":{"kind":"explicit.v1","times_ps":[0]}}));
    let fixture = Fixture::new(c, w, "can.mixed.protocol.v1", "1ms");
    let s = fixture.run();
    assert!(
        rows(&s, "request")
            .iter()
            .all(|r| r["state"] == "completed")
    );
    let samples = rows(&s, "sample");
    let a = samples
        .iter()
        .find(|r| r["bus"] == "bus" && r["controller"] == "a" && r["local_index"] == 0)
        .unwrap();
    let b = samples
        .iter()
        .find(|r| r["bus"] == "other" && r["controller"] == "a" && r["local_index"] == 0)
        .unwrap();
    assert_eq!(decimal(a, "exact_num"), 1_000_000);
    assert_eq!(decimal(a, "exact_den"), 3);
    assert_eq!(decimal(b, "exact_num"), 333334);
    assert_eq!(decimal(b, "exact_den"), 1);
    assert!(earlier(a, b).is_lt());
    use dir_simulator::runtime::session::*;
    let (mut session, controller) = RunSession::new(
        fixture.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let mut separated = false;
    for _ in 0..20 {
        session.advance_one().unwrap();
        let actual = rows(session.snapshot().registered.as_ref().unwrap(), "sample");
        separated =
            actual.iter().any(|r| r["bus"] == "bus") && !actual.iter().any(|r| r["bus"] == "other");
        if separated {
            break;
        }
    }
    assert!(
        separated,
        "pause lies between distinct exact instants that share ceil"
    );
    controller.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let path = fixture.0.join("same-ceil-checkpoint");
    let ack = session
        .checkpoint(&path, CheckpointLimits::default())
        .unwrap();
    assert_eq!(ack.status, ControlStatus::Applied, "{:?}", ack.error);
    let (mut restored, token) =
        restore_checkpoint(&path, Fixture::registry(), CheckpointLimits::default()).unwrap();
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
        serde_json::to_value(&s.model_records).unwrap()
    );
}

#[test]
fn compound_checkpoint_rejects_rehashed_semantic_clock_parser_source_and_timer_corruption() {
    use dir_simulator::runtime::{
        checkpoint_bundle::{read_bundle, write_bundle},
        session::*,
    };
    use sha2::{Digest, Sha256};
    let f = Fixture::new(
        config(),
        workload(fd(0x123, true, 64)),
        "can.mixed.protocol.v1",
        "2ms",
    );
    let (mut session, controller) = RunSession::new(
        f.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..10 {
        session.advance_one().unwrap();
    }
    controller.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let original = f.0.join("semantic-source");
    let limits = CheckpointLimits::default();
    assert_eq!(
        session
            .checkpoint(&original, limits.clone())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let (manifest, files) = read_bundle(&original, &limits).unwrap();
    let engine: serde_json::Value = serde_json::from_slice(&files["state.json"]).unwrap();
    let old = engine["codec_refs"][0]["blob_path"].as_str().unwrap();
    let original_raw: serde_json::Value = serde_json::from_slice(&files[old]).unwrap();
    type Mutation = fn(&mut serde_json::Value);
    let cases: [(&str, Mutation); 8] = [
        ("clock_rate", |v| {
            v["buses"][0]["nodes"][1]["clock"]["data"] = json!(true)
        }),
        ("clock_deadline", |v| {
            v["buses"][0]["nodes"][1]["clock"]["next"][0]["num"] = json!("1")
        }),
        ("parser", |v| {
            v["buses"][0]["nodes"][1]["parser"]["delivered"] = json!(true)
        }),
        ("request", |v| {
            v["buses"][0]["nodes"][0]["tx"]["request"]["id"] = json!("unknown:0")
        }),
        ("generation", |v| v["cursors"][0]["ordinal"] = json!(0)),
        ("timer_deadline", |v| {
            v["timer"]["tick"]["at"]["num"] = json!("1")
        }),
        ("timer_payload", |v| {
            v["timer"]["tick"]["generation"] = json!(999)
        }),
        ("epoch", |v| v["buses"][0]["epochs"] = json!({})),
    ];
    for (name, mutation) in cases {
        let mut raw = original_raw.clone();
        mutation(&mut raw);
        let bytes = serde_json::to_vec(&raw).unwrap();
        let new = format!("objects/{:x}.bin", Sha256::digest(&bytes));
        let mut changed = files.clone();
        changed.remove(old);
        changed.insert(new.clone(), bytes);
        let mut engine = engine.clone();
        engine["codec_refs"][0]["blob_path"] = json!(new);
        changed.insert("state.json".into(), serde_json::to_vec(&engine).unwrap());
        let mut manifest = manifest.clone();
        manifest["codecs"] = engine["codec_refs"].clone();
        let path = f.0.join(format!("tamper-{name}"));
        write_bundle(&path, manifest, changed, &limits, 0).unwrap();
        assert!(
            restore_checkpoint(&path, Fixture::registry(), limits.clone()).is_err(),
            "{name}"
        );
    }
    assert!(restore_checkpoint(&original, Fixture::registry(), limits).is_ok());
}

#[test]
fn passive_flag_dominant_extension_enters_bus_off_until_manual_recovery_observations() {
    let wire = dir_simulator::runtime::canfd::codec::encode(
        &dir_simulator::runtime::canfd::codec::Frame {
            id: 0x123,
            extended: false,
            brs: true,
            esi: true,
            data: vec![0x5a; 2],
        },
    )
    .unwrap();
    let ack = wire.crc_delimiter_index + 1;
    for recover in [false, true] {
        let mut c = config();
        c["buses"][0]["controllers"][0]["tec"] = json!(240);
        c["buses"][0]["controllers"][1]["capability"] = json!("monitor_only");
        if recover {
            c["buses"][0]["controllers"][0]["recover_at_ps"] = json!(3_000_000_000u64);
        }
        c["faults"] = json!(
            (ack + 3..ack + 22)
                .map(|bit| json!({"request_id":"g:0","attempt":1,"kind":"flip","bit":bit}))
                .collect::<Vec<_>>()
        );
        let s = Fixture::new(
            c,
            workload(fd(0x123, true, 2)),
            "can.mixed.protocol.v1",
            if recover { "6ms" } else { "1ms" },
        )
        .run();
        let attempts = rows(&s, "attempt");
        if recover {
            let retry = attempts
                .iter()
                .find(|r| r["request_id"] == "g:0" && r["attempt"] == 2)
                .unwrap();
            assert!(
                retry["sof_ps"].as_str().unwrap().parse::<u64>().unwrap()
                    >= 3_000_000_000 + 128 * 11 * 2_000_000
            );
        } else {
            assert_eq!(attempts.len(), 1);
            let controller = rows(&s, "controller")
                .into_iter()
                .find(|r| r["controller"] == "a")
                .unwrap();
            assert_eq!(controller["state"], "bus_off");
            assert_eq!(controller["phase"]["phase"], "BusOff");
        }
    }
}

#[test]
fn actual_count_invalidity_is_delayed_with_real_healthy_ack_and_transmitter_faults_return_nominal()
{
    let wire = dir_simulator::runtime::canfd::codec::encode(
        &dir_simulator::runtime::canfd::codec::Frame {
            id: 0x123,
            extended: false,
            brs: true,
            esi: false,
            data: vec![0x5a; 12],
        },
    )
    .unwrap();
    // The Gray/count bit is followed by fixed stuffing. Flip a count bit that
    // does not invalidate the next fixed complement, preserving the intended cause.
    let bit = wire.fixed_stuff_positions[0] + 1;
    let mut c = config();
    c["buses"][0]["controllers"][1]["rec"] = json!(128);
    c["buses"][0]["controllers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"c","capability":"fd_capable"}));
    c["faults"] = json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":bit,"receiver":"b"}]);
    let s = Fixture::new(
        c,
        workload(fd(0x123, true, 12)),
        "can.mixed.protocol.v1",
        "1ms",
    )
    .run();
    let error = rows(&s, "error")
        .into_iter()
        .find(|r| r["detector"] == "b")
        .unwrap();
    assert_eq!(error["error"], "stuff_count");
    assert_eq!(error["local_index"], wire.crc_delimiter_index + 2);
    assert_eq!(rows(&s, "request")[0]["attempt"], 1);
    assert!(
        rows(&s, "sample")
            .iter()
            .any(|r| r["controller"] == "a" && r["field"] == "ack" && r["sample"] == 0)
    );
    for passive in [false, true] {
        let mut c = config();
        if passive {
            c["buses"][0]["controllers"][0]["tec"] = json!(128);
        }
        let local_wire = dir_simulator::runtime::canfd::codec::encode(
            &dir_simulator::runtime::canfd::codec::Frame {
                id: 0x123,
                extended: false,
                brs: true,
                esi: passive,
                data: vec![0x5a; 12],
            },
        )
        .unwrap();
        let bit = local_wire.fixed_stuff_positions[0];
        c["faults"] =
            json!([{"request_id":"g:0","attempt":1,"kind":"flip","bit":bit,"receiver":"a"}]);
        let s = Fixture::new(
            c,
            workload(fd(0x123, true, 12)),
            "can.mixed.protocol.v1",
            "1ms",
        )
        .run();
        let error = rows(&s, "error")
            .into_iter()
            .filter(|r| r["detector"] == "a")
            .min_by(earlier)
            .unwrap();
        assert_eq!(error["local_index"], bit);
        assert_eq!(error["flag"], if passive { "passive" } else { "active" });
        assert_eq!(
            decimal(&error, "flag_start_num") * decimal(&error, "exact_den")
                - decimal(&error, "exact_num") * decimal(&error, "flag_start_den"),
            500000 * decimal(&error, "exact_den") * decimal(&error, "flag_start_den")
        );
    }
}
#[test]
fn equal_fd_arbitration_preserves_real_esi_and_payload_disagreement() {
    for mismatch in ["esi", "payload", "extended"] {
        let mut c = config();
        c["buses"][0]["controllers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"c","capability":"fd_capable"}));
        let mut w = workload(fd(0x123, true, 2));
        let mut peer = w["generators"][0].clone();
        peer["id"] = json!("p");
        peer["controller"] = json!("b");
        match mismatch {
            "esi" => c["buses"][0]["controllers"][1]["tec"] = json!(128),
            "payload" => peer["frame"]["data"] = json!([0xa5, 0x5a]),
            _ => {
                peer["frame"]["extended"] = json!(true);
                peer["frame"]["id"] = json!(0x123u32 << 18);
            }
        }
        w["generators"].as_array_mut().unwrap().push(peer);
        let s = Fixture::new(c, w, "can.mixed.protocol.v1", "1ms").run();
        let attempts = rows(&s, "attempt");
        if mismatch == "extended" {
            assert!(
                attempts
                    .iter()
                    .any(|r| r["request_id"] == "p:0" && r["state"] == "arbitration_lost")
            );
        } else {
            assert!(rows(&s, "error").iter().any(|r| r["error"] == "bit"));
            assert!(
                !attempts
                    .iter()
                    .any(|r| r["attempt"] == 1 && r["state"] == "arbitration_lost")
            );
        }
    }
}
