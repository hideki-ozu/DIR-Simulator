//! Frozen DIR-TEST-0105..0108 acceptance inputs; expected times and bytes are independent constants.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Output(PathBuf);
impl Output {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "dir-acceptance-bridge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/acceptance-2026-10-08/bridge")
}
fn product(case: &str, limit: Option<u64>) -> Value {
    let mut prepared =
        prepare(&fixtures().join(case).join("run.ini")).unwrap_or_else(|e| panic!("{case}: {e:?}"));
    if let Some(limit) = limit {
        prepared.common.time_limit_ps = limit;
    }
    let out = Output::new();
    run(prepared, &out.0).unwrap_or_else(|e| panic!("{case}: {e:?}"));
    serde_json::from_slice(&fs::read(out.0.join("results.json")).unwrap()).unwrap()
}
fn rows<'a>(r: &'a Value, schema: &str) -> Vec<&'a Value> {
    r["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["schema_name"] == schema)
        .collect()
}
fn n(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}
fn metric(r: &Value, name: &str) -> u64 {
    n(&r["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["metric"] == name)
        .unwrap()["value"])
}
fn conserved(r: &Value) {
    let conversions = rows(r, "dir.can_ethernet.conversion");
    let rejected = conversions
        .iter()
        .filter(|x| x["data"]["status"] == "rejected")
        .count();
    let processing = conversions
        .iter()
        .filter(|x| x["data"]["status"] == "processing")
        .count();
    let waiting = conversions
        .iter()
        .filter(|x| x["data"]["status"] == "waiting_tx")
        .count();
    let released = conversions
        .iter()
        .filter(|x| x["data"]["status"] == "released")
        .count();
    assert_eq!(
        conversions.len(),
        rejected + processing + waiting + released
    );
    let summary = r["simulation"]["summary"].as_array().unwrap();
    let total = |name: &str| {
        summary
            .iter()
            .filter(|x| x["metric"] == name)
            .map(|x| n(&x["value"]))
            .sum::<u64>()
    };
    assert_eq!(
        total("gateway.conversion.attempted"),
        conversions.len() as u64
    );
    assert_eq!(
        total("gateway.conversion.accepted"),
        (processing + waiting + released) as u64
    );
    assert_eq!(total("gateway.conversion.rejected"), rejected as u64);
    assert_eq!(total("gateway.rx.occupancy"), (processing + waiting) as u64);
    let mut targets = 0;
    let mut completed = 0;
    for s in rows(r, "dir.can_ethernet.segment") {
        let ts = s["data"]["targets"].as_array().unwrap();
        assert_eq!(n(&s["data"]["target_count"]), ts.len() as u64);
        targets += ts.len() as u64;
        completed += ts.iter().filter(|t| t["completed_ps"].is_string()).count() as u64;
    }
    assert_eq!(metric(r, "gateway.end_to_end.target_count"), targets);
    assert_eq!(metric(r, "gateway.end_to_end.completed"), completed);
    assert!(completed <= targets);
}
fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
        }
    }
    !crc
}
#[test]
fn dir_test_0105_c01_c04_c03ef_fixed_wire_vectors_and_independent_crc() {
    for (case, codec, pad) in [
        ("zero", "4449524301000000000000", 35),
        ("small", "4449524301000000012302aabb", 33),
        ("max-standard", "444952430100000007ff08ffffffffffffffff", 27),
        ("max-extended", "4449524301011fffffff08ffffffffffffffff", 27),
        ("extended-zero", "4449524301010000000000", 35),
        ("standard-2047", "444952430100000007ff00", 35),
        ("extended-2047", "444952430101000007ff00", 35),
        ("extended-2048", "4449524301010000080000", 35),
    ] {
        let r = product(&format!("codec-valid-{case}"), None);
        let frames = rows(&r, "ethernet.frame");
        assert_eq!(frames.len(), 1);
        let f = &frames[0]["data"];
        assert_eq!(f["data_hex"], codec, "{case}");
        assert_eq!(n(&f["pad_bytes"]), pad);
        assert_eq!(f["mac_bytes"], "64");
        let bytes = hex(f["mac_hex"].as_str().unwrap());
        assert_eq!(
            &bytes[bytes.len() - 4..],
            &crc32(&bytes[..bytes.len() - 4]).to_le_bytes()
        );
        conserved(&r);
    }
    let tagged = product("r01-tagged", None);
    let frames = rows(&tagged, "ethernet.frame");
    let f = &frames[0]["data"];
    let bytes = hex(f["mac_hex"].as_str().unwrap());
    assert_eq!(&bytes[12..18], &hex("8100600a88b5"));
    assert_eq!(f["mac_bytes"], "68");
    assert_eq!(f["pad_bytes"], "35");
    assert_eq!(&bytes[64..], &crc32(&bytes[..64]).to_le_bytes());
}
#[test]
fn dir_test_0105_c03g_c05_c06_format_rewrite_and_pcp_reclassification() {
    let r = product("codec-extended-to-standard", None);
    let b = rows(&r, "dir.can_ethernet.branch");
    assert_eq!(b[0]["data"]["input_format"], "extended");
    assert_eq!(b[0]["data"]["input_can_id"], "291");
    assert_eq!(b[0]["data"]["output_format"], "standard");
    assert_eq!(b[0]["data"]["output_can_id"], "1");
    let requests = rows(&r, "can.request");
    assert_eq!(requests[0]["data"]["serialized_bits"], "47");
    let r = product("codec-zero-to-one", None);
    assert_eq!(rows(&r, "can.request")[0]["data"]["serialized_bits"], "47");
    assert_eq!(
        rows(&r, "dir.can_ethernet.branch")[0]["data"]["input_can_id"],
        "0"
    );
    assert_eq!(
        rows(&r, "dir.can_ethernet.branch")[0]["data"]["output_can_id"],
        "1"
    );
    let r = product("pcp-reclassification", None);
    let c = rows(&r, "dir.can_ethernet.conversion");
    assert_eq!(c.len(), 2);
    assert!(
        c.iter()
            .any(|x| x["data"]["gateway"] == "Loop.g2" && x["data"]["reason"] == "no_rule")
    );
    assert_eq!(rows(&r, "can.request").len(), 1);
}
#[test]
fn dir_test_0106_n01_n01f_n03_n07_receive_rejection_precedence() {
    for case in [
        "magic",
        "version",
        "flags",
        "dlc",
        "standard-id",
        "extended-id",
        "truncated",
        "padding",
    ] {
        let r = product(&format!("codec-invalid-{case}"), None);
        let c = rows(&r, "dir.can_ethernet.conversion");
        assert_eq!(c.len(), 1, "{case}");
        assert_eq!(c[0]["data"]["reason"], "invalid_codec", "{case}");
        assert!(rows(&r, "can.request").is_empty());
        conserved(&r);
    }
    for (case, reason) in [
        ("other-ethertype", "invalid_codec"),
        ("no-rule", "no_rule"),
        ("format-mismatch", "no_rule"),
        ("rx-zero", "rx_full"),
    ] {
        let r = product(&format!("receive-{case}"), None);
        let c = rows(&r, "dir.can_ethernet.conversion");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["data"]["reason"], reason);
        assert!(rows(&r, "can.request").is_empty());
        conserved(&r);
    }
    for case in ["other-mac", "other-vid", "unsubscribed"] {
        let r = product(&format!("receive-{case}"), None);
        assert!(rows(&r, "dir.can_ethernet.conversion").is_empty(), "{case}");
        assert!(rows(&r, "can.request").is_empty());
        conserved(&r);
    }
}
#[test]
fn dir_test_0106_n04_n05_n06_strict_prepare_failures_do_not_start_run() {
    for case in [
        "bool-pcp",
        "pcp8",
        "vid0",
        "vid4095",
        "overflow",
        "duplicate-rule",
        "unknown-key",
        "missing-format",
        "duplicate-egress",
        "duplicate-owner",
        "unconnected-egress",
        "fd",
        "rtr",
        "half-duplex",
        "zero-dst",
        "reserved-dst",
        "invalid-dst",
    ] {
        let path = fixtures()
            .join(format!("prepare-invalid-{case}"))
            .join("run.ini");
        let e = prepare(&path).unwrap_err();
        assert_eq!(e.stage, "prepare", "{case}");
        assert!(e.source.is_some(), "{case}");
    }
}
fn visible_at(data: &Value, field: &str, time: u64, limit: u64) {
    if time < limit {
        assert_eq!(data[field], time.to_string(), "{field} at {limit}");
    } else {
        assert!(
            data[field].is_null(),
            "{field} prematurely visible at {limit}: {}",
            data[field]
        );
    }
}
#[test]
fn dir_test_0107_r01_r02_and_0108_p03_p06_every_milestone_half_open_boundary() {
    for (case, observed, ready, offer, eof, release, arrival, completed) in [
        (
            "r01",
            100_000_000,
            103_000_000,
            106_000_000,
            106_576_000,
            106_672_000,
            110_576_000,
            115_576_000,
        ),
        (
            "r02",
            1_608_000,
            6_608_000,
            10_608_000,
            110_608_000,
            116_608_000,
            110_608_000,
            115_608_000,
        ),
    ] {
        let mut milestones: Vec<u64> =
            vec![0, observed, ready, offer, eof, release, arrival, completed];
        if case == "r01" {
            milestones.extend([101_000_000, 106_000_000]);
        } else {
            milestones.extend([608_000, 704_000, 3_608_000]);
        }
        milestones.sort_unstable();
        milestones.dedup();
        for boundary in milestones {
            for limit in [boundary.saturating_sub(1), boundary, boundary + 1] {
                let r = product(case, Some(limit));
                conserved(&r);
                let conversions = rows(&r, "dir.can_ethernet.conversion");
                if limit <= observed {
                    assert!(conversions.is_empty(), "{case}@{limit}");
                } else {
                    assert_eq!(conversions.len(), 1);
                    let c = &conversions[0]["data"];
                    visible_at(c, "ready_ps", ready, limit);
                    visible_at(c, "released_ps", offer, limit);
                    assert_eq!(c["planned_ready_ps"], ready.to_string());
                }
                if case == "r01" {
                    let t = rows(&r, "ethernet.transfer");
                    if limit > offer {
                        assert_eq!(t.len(), 1);
                        let d = &t[0]["data"];
                        visible_at(d, "sof_ps", offer, limit);
                        visible_at(d, "eof_ps", eof, limit);
                        visible_at(d, "release_ps", release, limit);
                        visible_at(d, "arrival_ps", arrival, limit);
                    }
                } else {
                    let req = rows(&r, "can.request");
                    if limit > offer {
                        assert_eq!(req.len(), 1);
                        let d = &req[0]["data"];
                        visible_at(d, "sof_ps", offer, limit);
                        visible_at(d, "eof_ps", eof, limit);
                        visible_at(&d["model_fields"], "release_ps", release, limit);
                    }
                }
                assert_eq!(
                    metric(&r, "gateway.end_to_end.completed"),
                    u64::from(completed < limit),
                    "{case}@{limit}"
                );
            }
        }
    }
}
#[test]
fn dir_test_0107_r01_tag_and_each_one_ps_delay_changes_only_causal_milestones() {
    for (case, ready, sof, eof, arrival, completed) in [
        (
            "r01-tagged",
            103_000_000,
            106_000_000,
            106_608_000,
            110_608_000,
            115_608_000,
        ),
        (
            "r01-plus-can-rx",
            103_000_001,
            106_000_001,
            106_576_001,
            110_576_001,
            115_576_001,
        ),
        (
            "r01-plus-conversion",
            103_000_001,
            106_000_001,
            106_576_001,
            110_576_001,
            115_576_001,
        ),
        (
            "r01-plus-eth-tx",
            103_000_000,
            106_000_001,
            106_576_001,
            110_576_001,
            115_576_001,
        ),
        (
            "r01-plus-link",
            103_000_000,
            106_000_000,
            106_576_000,
            110_576_001,
            115_576_001,
        ),
        (
            "r01-plus-sink-rx",
            103_000_000,
            106_000_000,
            106_576_000,
            110_576_000,
            115_576_001,
        ),
    ] {
        let r = product(case, None);
        let c = rows(&r, "dir.can_ethernet.conversion");
        assert_eq!(c[0]["data"]["observed_ps"], "100000000");
        assert_eq!(c[0]["data"]["ready_ps"], ready.to_string());
        let t = rows(&r, "ethernet.transfer");
        for (key, time) in [("sof_ps", sof), ("eof_ps", eof), ("arrival_ps", arrival)] {
            assert_eq!(t[0]["data"][key], time.to_string(), "{case}:{key}");
        }
        let seg = rows(&r, "dir.can_ethernet.segment");
        let completed = completed.to_string();
        assert!(
            seg.iter()
                .flat_map(|s| s["data"]["targets"].as_array().unwrap())
                .any(|x| x["completed_ps"] == completed)
        );
        assert_eq!(rows(&r, "can.request")[0]["data"]["eof_ps"], "100000000");
        conserved(&r);
    }
}
#[test]
fn dir_test_0105_c07_0107_r08_r11_0108_p01_two_gateway_wire_loop_and_multicast() {
    let r = product("loop-multicast", None);
    conserved(&r);
    let conversions = rows(&r, "dir.can_ethernet.conversion");
    assert_eq!(conversions.len(), 3);
    let rejected = conversions
        .iter()
        .find(|x| x["data"]["reason"] == "loop_prevented")
        .unwrap();
    assert_eq!(
        rejected["data"]["visited_gateways"],
        serde_json::json!(["Loop.g1", "Loop.g2"])
    );
    assert_eq!(rejected["data"]["observed_ps"], "203218000");
    assert_eq!(rows(&r, "can.request").len(), 2);
    assert_eq!(rows(&r, "ethernet.frame").len(), 1);
    let recipients = rows(&r, "ethernet.reception");
    let terminals = recipients
        .iter()
        .filter(|x| {
            ["Loop.g2.eth.rx", "Loop.a.rx", "Loop.b.rx"]
                .contains(&x["data"]["ingress"].as_str().unwrap())
        })
        .count();
    assert_eq!(terminals, 3);
    assert!(
        !recipients
            .iter()
            .any(|x| x["data"]["ingress"] == "Loop.x.rx" && x["data"]["status"] == "received")
    );
    assert_eq!(metric(&r, "gateway.end_to_end.target_count"), 4);
    assert_eq!(metric(&r, "gateway.end_to_end.completed"), 4);
}
#[test]
fn dir_test_0108_p02_hop_limit_in_second_gateway_is_finite() {
    let r = product("loop-hop-limit", None);
    let c = rows(&r, "dir.can_ethernet.conversion");
    assert_eq!(c.len(), 2);
    assert_eq!(
        c.iter()
            .filter(|x| x["data"]["reason"] == "hop_limit")
            .count(),
        1
    );
    assert_eq!(rows(&r, "can.request").len(), 1);
    conserved(&r);
}
#[test]
fn dir_test_0107_r10_r11_r12_0108_p12e_two_terminal_tuples_and_partial_completion() {
    for (case, suffix, first, last) in [
        ("loop-broadcast", "eth.tx", 103_218_000, 108_218_000),
        ("loop-broadcast", "can.tx", 203_218_000, 208_218_000),
        ("native-can-terminals", "", 94_000_000, 99_000_000),
        ("native-ethernet-terminals", "", 3_218_000, 8_218_000),
    ] {
        for limit in [first, first + 1, last, last + 1] {
            let r = product(case, Some(limit));
            conserved(&r);
            let seg = rows(&r, "dir.can_ethernet.segment");
            let s = seg
                .iter()
                .find(|s| {
                    let ts = s["data"]["targets"].as_array().unwrap();
                    ts.len() == 2
                        && (suffix.is_empty()
                            || s["data"]["branch_lineage"].as_array().unwrap().len()
                                == if suffix == "eth.tx" { 1 } else { 2 })
                })
                .unwrap();
            let ts = s["data"]["targets"].as_array().unwrap();
            assert_eq!(
                ts.iter().filter(|x| x["completed_ps"].is_string()).count(),
                usize::from(first < limit) + usize::from(last < limit),
                "{case}@{limit}"
            );
            if case.starts_with("native") {
                assert!(rows(&r, "dir.can_ethernet.conversion").is_empty());
                assert!(s["data"]["branch_lineage"].as_array().unwrap().is_empty());
            }
        }
    }
}
#[test]
fn dir_test_0108_p10_reversed_gateway_rule_port_queue_arrays_preserve_records_and_metrics() {
    let a = product("loop-multicast", None);
    let b = product("loop-multicast-reversed", None);
    assert_eq!(
        a["simulation"]["model_records"],
        b["simulation"]["model_records"]
    );
    assert_eq!(a["simulation"]["summary"], b["simulation"]["summary"]);
}

#[test]
fn dir_test_0108_p09_event_limit_every_callback_keeps_successful_journal_prefix() {
    let original = prepare(&fixtures().join("r01/run.ini")).unwrap();
    let full = dir_simulator::runtime::simulate(&original).unwrap();
    // Positive conversion completion schedules a distinct phase-1 notification.
    assert_eq!(full.common.committed_events, 18);
    let final_records = &full.registered.as_ref().unwrap().model_records;
    let mut prior_ids = std::collections::BTreeSet::new();
    for budget in 0..=18 {
        let mut prepared = original.clone();
        prepared.common.max_events = budget;
        let snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
        assert_eq!(snapshot.common.committed_events, budget);
        if budget < 18 {
            assert_eq!(snapshot.common.termination, "execution_failed");
            assert!(snapshot.common.pending_events > 0);
            assert_eq!(snapshot.common.diagnostics[0].reason, "event_limit");
        } else {
            assert_eq!(snapshot.common.termination, "events_exhausted");
        }
        let records = &snapshot.registered.as_ref().unwrap().model_records;
        for id in &prior_ids {
            assert!(
                records.contains_key(id),
                "previous committed row disappeared at budget {budget}"
            );
        }
        for (id, record) in records {
            assert!(final_records.contains_key(id));
            assert!(
                snapshot
                    .common
                    .last_event_time_ps
                    .is_some_and(|last| record.time_ps <= last)
            );
            if record.schema.name == "dir.can_ethernet.conversion"
                && record.data["status"] == "processing"
            {
                assert!(record.data["ready_ps"].is_null());
                assert!(record.data["released_ps"].is_null());
            }
        }
        prior_ids = records.keys().cloned().collect();
    }
}

#[test]
fn dir_test_0107_r08_r13_native_multicast_two_gateways_and_distinct_lineages() {
    let r = product("native-multicast-lineages", None);
    conserved(&r);
    let conversions = rows(&r, "dir.can_ethernet.conversion");
    for gateway in ["Loop.g1", "Loop.g2"] {
        assert!(conversions.iter().any(|c| c["data"]["gateway"] == gateway
            && c["data"]["observed_ps"] == "3218000"
            && c["data"]["status"] == "released"));
    }
    let receptions = rows(&r, "ethernet.reception");
    let initial = receptions
        .iter()
        .filter(|x| {
            x["data"]["frame_id"] == "native:0"
                && x["data"]["status"] == "received"
                && ["Loop.g1.eth.rx", "Loop.g2.eth.rx", "Loop.a.rx"]
                    .contains(&x["data"]["ingress"].as_str().unwrap())
        })
        .count();
    assert_eq!(initial, 3);
    let segments = rows(&r, "dir.can_ethernet.segment");
    let a_targets = segments
        .iter()
        .flat_map(|s| s["data"]["targets"].as_array().unwrap())
        .filter(|t| t["terminal_id"] == "Loop.a")
        .collect::<Vec<_>>();
    assert_eq!(a_targets.len(), 2);
    assert_ne!(a_targets[0]["segment_id"], a_targets[1]["segment_id"]);
    assert_ne!(
        a_targets[0]["branch_lineage"],
        a_targets[1]["branch_lineage"]
    );
    assert!(
        a_targets
            .iter()
            .all(|t| t["origin_id"] == "ethernet:native:0" && t["completed_ps"].is_string())
    );
}

#[test]
fn dir_test_0107_r03_r05_0108_p10_fanout_and_egress_permutation_and_unadmittable_branches() {
    let a = product("fanout", None);
    let b = product("fanout-reversed", None);
    assert_eq!(
        a["simulation"]["model_records"],
        b["simulation"]["model_records"]
    );
    assert_eq!(a["simulation"]["summary"], b["simulation"]["summary"]);
    for (bus, eof, release) in [
        ("Fanout.bus_a", "110608000", "116608000"),
        ("Fanout.bus_b", "210608000", "222608000"),
    ] {
        let requests = rows(&a, "can.request");
        let req = requests.iter().find(|x| x["data"]["bus"] == bus).unwrap();
        assert_eq!(req["data"]["sof_ps"], "10608000");
        assert_eq!(req["data"]["eof_ps"], eof);
        assert_eq!(req["data"]["model_fields"]["release_ps"], release);
    }
    for case in ["fanout-unadmittable", "ethernet-unadmittable"] {
        let r = product(case, None);
        conserved(&r);
        let branches = rows(&r, "dir.can_ethernet.branch");
        assert_eq!(
            branches
                .iter()
                .filter(|x| x["data"]["reason"] == "tx_unadmittable")
                .count(),
            1
        );
        assert!(branches.iter().all(|x| x["data"]["status"] != "waiting"));
        assert!(
            rows(&r, "dir.can_ethernet.conversion")
                .iter()
                .all(|x| x["data"]["status"] == "released")
        );
        if case == "fanout-unadmittable" {
            assert_eq!(rows(&r, "can.request").len(), 1);
        } else {
            assert!(rows(&r, "ethernet.frame").is_empty());
        }
    }
}

fn rejected_before_dispatch(case: &str, pointer: Option<&str>, reason: &str) {
    let config = fixtures().join(case).join("run.ini");
    let diagnostic = prepare(&config).unwrap_err();
    assert_eq!(diagnostic.stage, "prepare", "{case}");
    assert_eq!(diagnostic.reason, reason, "{case}");
    if let Some(pointer) = pointer {
        assert_eq!(diagnostic.target.as_deref(), Some(pointer), "{case}");
        let source = if pointer.starts_with("/can/generators") {
            "workload.json"
        } else {
            "model.json"
        };
        assert_eq!(
            diagnostic.source.as_deref(),
            Some(
                config
                    .parent()
                    .unwrap()
                    .join(source)
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .as_ref()
            ),
            "{case}"
        );
        assert!(diagnostic.line.is_some(), "{case}");
        assert!(diagnostic.column.is_some(), "{case}");
    }
    let out = Output::new();
    let report = dir_simulator::run_config(&config, &out.0).unwrap();
    assert_eq!(report.exit_code, 2, "{case}");
    assert_eq!(report.termination, "prep_failed", "{case}");
    assert_eq!(report.committed_events, "0", "{case}");
    assert_eq!(report.last_event_time_ps, None, "{case}");
    assert_eq!(report.event_processing_wall_seconds, None, "{case}");
    let primary = report.primary_diagnostic.unwrap();
    assert_eq!(primary.target, diagnostic.target, "{case}");
    assert_eq!(primary.source, diagnostic.source, "{case}");
    assert_eq!(
        (
            primary.line,
            primary.column,
            primary.end_line,
            primary.end_column
        ),
        (
            diagnostic.line,
            diagnostic.column,
            diagnostic.end_line,
            diagnostic.end_column
        ),
        "{case}"
    );
}

#[test]
fn dir_test_0105_c03f_native_standard_2048_is_rejected_before_dispatch() {
    rejected_before_dispatch(
        "prepare-invalid-native-standard-2048",
        Some("/can/generators/0/frame/id"),
        "invalid_range",
    );
}

#[test]
fn dir_test_0106_n04_forbidden_models_reject_zero_count_and_future_sources() {
    for (kind, pointer, reason) in [
        ("fd", "/can/generators/0/frame/format", "invalid_type"),
        ("rtr", "/can/generators/0/frame/rtr", "unknown_parameter"),
        ("tsn", "/ethernet/tsn", "unknown_parameter"),
        ("dynamic", "/ethernet/dynamic", "unknown_parameter"),
        ("half-duplex", "/ethernet/media", "unknown_parameter"),
    ] {
        for timing in ["count-zero", "future"] {
            rejected_before_dispatch(
                &format!("prepare-invalid-{kind}-{timing}"),
                Some(pointer),
                reason,
            );
        }
    }
}

#[test]
fn dir_test_0106_n05_connected_switch_cycle_and_n06_duplicate_json_rule_key_reject() {
    let case = "prepare-invalid-connected-ethernet-cycle";
    let diagnostic = prepare(&fixtures().join(case).join("run.ini")).unwrap_err();
    assert!(
        diagnostic
            .message
            .contains("Ethernet topology must be a connected tree")
    );
    let source = fixtures()
        .join("topologies/2cdb997676bc/bridge/Main.ned")
        .canonicalize()
        .unwrap();
    assert_eq!(
        diagnostic.source.as_deref(),
        Some(source.to_string_lossy().as_ref())
    );
    assert_eq!(diagnostic.target.as_deref(), Some("Cycle"));
    assert_eq!(
        (
            diagnostic.line,
            diagnostic.column,
            diagnostic.end_line,
            diagnostic.end_column
        ),
        (Some(53), Some(1), Some(61), Some(2))
    );
    rejected_before_dispatch(case, None, "topology_invalid");
    rejected_before_dispatch(
        "prepare-invalid-duplicate-json-rule-key",
        Some("/gateways/0/rules/0/match/can_id"),
        "duplicate_definition",
    );
}

#[test]
fn positive_conversion_delay_offers_after_existing_same_time_ingress() {
    let input = Output::new();
    fs::create_dir_all(&input.0).unwrap();
    let mut model: Value =
        serde_json::from_slice(&fs::read(fixtures().join("r01/model.json")).unwrap()).unwrap();
    model["gateways"][0]["rx_capacity"] = serde_json::json!(1);
    fs::write(
        input.0.join("model.json"),
        serde_json::to_vec(&model).unwrap(),
    )
    .unwrap();
    let mut workload: Value =
        serde_json::from_slice(&fs::read(fixtures().join("r01/workload.json")).unwrap()).unwrap();
    workload["ethernet"]["generators"] = serde_json::json!([{
        "id": "native", "node": "R01.sink", "kind": "ethernet.explicit.v1",
        "times_ps": ["98424000"], "priority": 3, "flow_id": "native", "deadline_ps": null,
        "frame": {"dst_mac": "02:00:00:00:00:01", "ether_type": 34997, "data": "4449524301000000000000", "tag": null}
    }]);
    fs::write(
        input.0.join("workload.json"),
        serde_json::to_vec(&workload).unwrap(),
    )
    .unwrap();
    let ini = format!(
        "[General]\nnetwork = bridge.R01\nned-path = \"{}\"\nmodel-profile = \"can.ethernet.gateway.v1\"\nmodel-config = \"model.json\"\nworkload = \"workload.json\"\nsim-time-limit = 1ms\nR01.gw.can.rxProcessingDelay = 1us\nR01.gw.eth.txProcessingDelay = 0ps\n",
        fixtures().join("topologies/d2acd8e6f7f5").display()
    );
    fs::write(input.0.join("run.ini"), &ini).unwrap();
    let execute = |limit: Option<u64>| {
        let mut prepared = prepare(&input.0.join("run.ini")).unwrap();
        if let Some(limit) = limit {
            prepared.common.time_limit_ps = limit;
        }
        let output = Output::new();
        run(prepared, &output.0).unwrap();
        serde_json::from_slice::<Value>(&fs::read(output.0.join("results.json")).unwrap()).unwrap()
    };
    let result = execute(None);
    // CAN EOF 100 us + RX 1 us + conversion 2 us = 103 us.
    // Native Ethernet SOF 98.424 us + 576 ns serialization + 4 us link
    // also arrives at 103 us; that phase-1 arrival was reserved before
    // the conversion's phase-0 completion can reserve its phase-1 offer.
    let conversions = rows(&result, "dir.can_ethernet.conversion");
    let incoming = conversions
        .iter()
        .find(|c| c["data"]["origin_id"].as_str().unwrap().contains("native"))
        .unwrap();
    assert_eq!(n(&incoming["data"]["observed_ps"]), 103_000_000);
    assert_eq!(incoming["data"]["reason"], "rx_full");
    let branch = rows(&result, "dir.can_ethernet.branch")[0];
    assert_eq!(n(&branch["data"]["offer_ps"]), 103_000_000);
    assert_eq!(n(&branch["data"]["sof_ps"]), 103_000_000);
    assert_eq!(result["simulation"], execute(None)["simulation"]);
    let prefix = execute(Some(103_000_000));
    let branch = rows(&prefix, "dir.can_ethernet.branch")[0];
    assert!(branch["data"]["offer_ps"].is_null());
    assert!(branch["data"]["sof_ps"].is_null());
    conserved(&result);
    conserved(&prefix);

    // The zero-delay path still offers inside its phase-1 ingress callback.
    model["gateways"][0]["conversion_delay_ps"] = serde_json::json!("0");
    fs::write(
        input.0.join("model.json"),
        serde_json::to_vec(&model).unwrap(),
    )
    .unwrap();
    fs::write(
        input.0.join("run.ini"),
        ini.replace("rxProcessingDelay = 1us", "rxProcessingDelay = 0ps"),
    )
    .unwrap();
    let zero = execute(Some(100_000_001));
    let branch = rows(&zero, "dir.can_ethernet.branch")[0];
    assert_eq!(n(&branch["data"]["offer_ps"]), 100_000_000);
    assert_eq!(n(&branch["data"]["sof_ps"]), 100_000_000);
    conserved(&zero);
}
