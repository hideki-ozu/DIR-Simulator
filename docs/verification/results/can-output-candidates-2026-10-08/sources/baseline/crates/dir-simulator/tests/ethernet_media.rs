//! Runtime projections compared with independently authored media fixtures.
use dir_simulator::{prepare, run};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/ethernet-media")
        .join(name)
}
fn product(name: &str) -> Value {
    let output = Temp(std::env::temp_dir().join(format!(
        "dir-media-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let report = run(prepare(&fixture(name)).unwrap(), &output.0).unwrap();
    assert_eq!(report.exit_code, 0, "{name}");
    serde_json::from_slice(&fs::read(output.0.join("results.json")).unwrap()).unwrap()
}
fn rows<'a>(r: &'a Value, name: &str) -> Vec<&'a Value> {
    r["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["schema_name"] == name)
        .collect()
}
fn decimal(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}
fn metric<'a>(r: &'a Value, target: &str, name: &str) -> &'a Value {
    r["simulation"]["summary"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["target"] == target && v["metric"] == name)
        .unwrap()
}
#[test]
fn collisions_jam_beb_retries_match_analytic_vectors() {
    let catalog: Value =
        serde_json::from_slice(&fs::read(fixture("scenarios.json")).unwrap()).unwrap();
    for name in ["collision", "late-start", "zero-propagation", "half-10"] {
        let golden = &catalog["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .unwrap()["expected"];
        let result = product(&format!("{name}.ini"));
        let attempts = rows(&result, "ethernet.attempt");
        let first: Vec<_> = attempts
            .iter()
            .filter(|v| v["data"]["number"] == "1")
            .collect();
        assert_eq!(first.len(), 2);
        for (field, key) in [
            ("collision_ps", "collision_ps"),
            ("planned_jam_start_ps", "planned_jam_start_ps"),
            ("jam_end_ps", "jam_end_ps"),
            ("backoff_slots", "backoff_slots"),
            ("backoff_until_ps", "backoff_until_ps"),
        ] {
            if let Some(expected) = golden[key].as_array() {
                for (row, expected) in first.iter().zip(expected) {
                    assert_eq!(
                        decimal(&row["data"][field]),
                        expected.as_u64().unwrap(),
                        "{name} {field}"
                    );
                }
            }
        }
        let retry: Vec<_> = attempts
            .iter()
            .filter(|v| v["data"]["number"] == "2")
            .collect();
        for (field, key) in [("sof_ps", "retry_sof_ps"), ("eof_ps", "retry_eof_ps")] {
            if let Some(expected) = golden[key].as_array() {
                for (row, expected) in retry.iter().zip(expected) {
                    assert_eq!(
                        decimal(&row["data"][field]),
                        expected.as_u64().unwrap(),
                        "{name} {field}"
                    );
                }
            }
        }
        assert!(first.iter().all(|a| a["data"]["eof_ps"].is_null()
            && a["data"]["arrival_ps"].is_null()
            && a["data"]["planned_mdi_eof_ps"].is_null()));
        assert_eq!(rows(&result, "ethernet.reception").len(), 2);
    }
}
#[test]
fn delayed_carrier_and_same_time_carrier_prevent_false_collisions() {
    for name in ["carrier", "arrival-tie"] {
        let result = product(&format!("{name}.ini"));
        let t = rows(&result, "ethernet.transfer");
        assert_eq!(t[0]["data"]["sof_ps"], "0");
        assert_eq!(t[1]["data"]["sof_ps"], "6820000");
        assert_eq!(
            metric(&result, "$all", "ethernet.media.collisions")["value"],
            "0"
        );
    }
}
#[test]
fn half_stop_preserves_incomplete_jam_and_backoff_without_delivery() {
    let result = product("stop-jam.ini");
    let a = rows(&result, "ethernet.attempt");
    assert_eq!(a.len(), 2);
    assert!(
        a.iter()
            .all(|a| a["data"]["status"] == "jamming" && a["data"]["jam_end_ps"].is_null())
    );
    assert!(rows(&result, "ethernet.reception").is_empty());
    for target in ["Main.a.tx", "Main.b.tx"] {
        assert_eq!(
            metric(&result, target, "ethernet.media.jam_ps")["value"],
            "160000"
        );
        assert_eq!(
            metric(&result, target, "ethernet.media.tx_utilization")["value"],
            1.0
        );
    }
    let result = product("stop-backoff.ini");
    let t = rows(&result, "ethernet.transfer");
    assert_eq!(t[0]["data"]["status"], "deferred");
    assert_eq!(t[1]["data"]["status"], "backoff");
    assert!(rows(&result, "ethernet.reception").is_empty());
}
#[test]
fn current_retry_is_excluded_from_fifo_capacity() {
    let r = product("queue-retry.ini");
    let t = rows(&r, "ethernet.transfer");
    let dropped = t
        .iter()
        .find(|t| t["record_id"] == "a:2@Main.a.tx")
        .unwrap();
    assert_eq!(dropped["data"]["drop_reason"], "queue_full");
    assert_eq!(dropped["data"]["attempt_count"], "0");
    assert_eq!(
        t.iter()
            .filter(|t| t["data"]["drop_reason"] == "queue_full")
            .count(),
        1
    );
}
#[test]
fn t1_duplex_and_pipeline_keep_mac_release_independent_of_phy() {
    let r = product("t1-duplex.ini");
    let t = rows(&r, "ethernet.transfer");
    for row in &t {
        assert_eq!(row["data"]["sof_ps"], "0");
        assert_eq!(row["data"]["eof_ps"], "576000");
        assert_eq!(row["data"]["release_ps"], "672000");
        assert_eq!(row["data"]["attempt_count"], "1");
    }
    assert_eq!(t[0]["data"]["arrival_ps"], "877000");
    assert_eq!(t[1]["data"]["arrival_ps"], "1277000");
    let r = product("t1-pipeline.ini");
    let t = rows(&r, "ethernet.transfer");
    assert_eq!(t[0]["data"]["arrival_ps"], "2577000");
    assert_eq!(t[1]["data"]["sof_ps"], "672000");
    assert_eq!(t[1]["data"]["arrival_ps"], "3249000");
    assert_eq!(rows(&r, "ethernet.reception").len(), 2);
}
#[test]
fn arrival_boundary_and_media_schema_metrics_are_exact() {
    let r = product("t1-boundary.ini");
    assert!(rows(&r, "ethernet.reception").is_empty());
    let t = rows(&r, "ethernet.transfer");
    assert!(t[0]["data"]["arrival_ps"].is_null());
    assert_eq!(t[0]["data"]["planned_arrival_ps"], "877000");
    let r = product("t1-after-boundary.ini");
    assert_eq!(rows(&r, "ethernet.reception").len(), 1);
    assert_eq!(r["metadata"]["model_schemas"].as_array().unwrap().len(), 5);
    assert_eq!(r["metadata"]["metrics"].as_array().unwrap().len(), 9);
    let schema = rows(&r, "ethernet.attempt");
    assert_eq!(schema[0]["data"].as_object().unwrap().len(), 23);
    assert_eq!(
        rows(&r, "ethernet.transfer")[0]["data"]
            .as_object()
            .unwrap()
            .len(),
        19
    );
    assert_eq!(
        rows(&r, "ethernet.phy_link")[0]["data"]
            .as_object()
            .unwrap()
            .len(),
        12
    );
    assert!(
        r["metadata"]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["metric_id"]
                .as_str()
                .unwrap()
                .starts_with("ethernet.media."))
    );
}
#[test]
fn strict_media_prepare_boundaries_report_specific_rules() {
    for (name, rule) in [
        ("invalid-slot.ini", "half_slot_bound"),
        ("invalid-t1-half.ini", "phy_duplex"),
        ("invalid-t1-roles.ini", "phy_roles"),
    ] {
        let error = prepare(&fixture(name)).unwrap_err();
        assert_eq!(error.code, "E-0001");
        assert_eq!(error.details.unwrap()["rule"], rule);
    }
    prepare(&fixture("slot-upper.ini")).unwrap();
}
#[test]
fn mixed_switch_pair_timelines_preserve_independent_transfers() {
    let r = product("mixed.ini");
    let catalog: Value =
        serde_json::from_slice(&fs::read(fixture("scenarios.json")).unwrap()).unwrap();
    let g = &catalog["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "mixed")
        .unwrap()["expected"];
    let t = rows(&r, "ethernet.transfer");
    for field in ["sof_ps", "arrival_ps"] {
        for (id, time) in g[field].as_object().unwrap() {
            let row = t.iter().find(|t| t["record_id"] == *id).unwrap();
            assert_eq!(
                decimal(&row["data"][field]),
                time.as_u64().unwrap(),
                "{id} {field}"
            );
        }
    }
    assert_eq!(
        metric(&r, "$all", "ethernet.media.collisions")["value"],
        "0"
    );
}

#[test]
fn repeat_collisions_and_max_frame_match_remaining_catalog_cases() {
    let r = product("collision-repeat.ini");
    let a = rows(&r, "ethernet.attempt");
    for side in ["a", "b"] {
        let find = |number: &str| {
            a.iter()
                .find(|v| v["record_id"] == format!("{side}:0@Main.{side}.tx#{number}"))
                .unwrap()
        };
        assert_eq!(find("1")["data"]["backoff_slots"], "1");
        assert_eq!(find("2")["data"]["sof_ps"], "6080000");
        assert_eq!(find("2")["data"]["collision_ps"], "6180000");
        assert_eq!(find("2")["data"]["jam_end_ps"], "7040000");
        assert_eq!(
            find("2")["data"]["backoff_slots"],
            if side == "a" { "3" } else { "0" }
        );
        assert_eq!(
            find("3")["data"]["sof_ps"],
            if side == "a" { "22400000" } else { "8100000" }
        );
    }
    let r = product("t1-max-frame.ini");
    let t = rows(&r, "ethernet.transfer");
    assert_eq!(t[0]["data"]["eof_ps"], "12208000");
    assert_eq!(t[0]["data"]["release_ps"], "12304000");
    assert_eq!(t[0]["data"]["arrival_ps"], "12509000");
}
#[test]
fn dedicated_100base_t1_uses_independent_policy_and_full_pipeline() {
    let path = fixture("../original-network/t1.ini");
    let p = prepare(&path).unwrap();
    let snapshot = dir_simulator::runtime::simulate(&p).unwrap();
    assert!(!snapshot.common.partial);
    let t = &snapshot.ethernet.as_ref().unwrap().transfers;
    for row in t {
        assert_eq!(row.sof_ps, Some(0));
        assert_eq!(row.eof_ps, Some(5_760_000));
        assert_eq!(row.release_ps, Some(6_720_000));
    }
    assert_eq!(t[0].arrival_ps, Some(6_061_000));
    assert_eq!(t[1].arrival_ps, Some(6_461_000));
}
#[test]
fn pair_start_overflow_preflights_both_directions_before_any_sof_commit() {
    let mut p = prepare(&fixture("t1-duplex.ini")).unwrap();
    p.ethernet
        .as_mut()
        .unwrap()
        .media
        .as_mut()
        .unwrap()
        .physical_links[0]
        .b_phy
        .tx_latency_ps = u64::MAX;
    let snapshot = dir_simulator::runtime::simulate(&p).unwrap();
    assert!(snapshot.common.partial);
    assert_eq!(snapshot.common.diagnostics[0].code, "E-0004");
    let eth = snapshot.ethernet.unwrap();
    assert!(eth.attempts.is_empty());
    assert!(
        eth.transfers
            .iter()
            .all(|t| t.sof_ps.is_none() && t.status == "queued")
    );
}

#[test]
fn canceled_pair_reservations_are_excluded_from_partial_pending_count() {
    let mut p = prepare(&fixture("collision.ini")).unwrap();
    let s = (1..30)
        .find_map(|limit| {
            p.common.max_events = limit;
            let s = dir_simulator::runtime::simulate(&p).unwrap();
            let eth = s.ethernet.as_ref().unwrap();
            (s.common.partial
                && eth.attempts.len() == 2
                && eth.attempts.iter().all(|a| a.status == "jamming"))
            .then_some(s)
        })
        .unwrap();
    assert_eq!(s.common.end_ps, 100_000);
    assert_eq!(
        s.common.pending_events, 2,
        "one pair timer and its pair arbitration, with canceled normal EOF removed"
    );
}

#[test]
fn public_media_examples_execute_with_the_selected_profiles() {
    for name in [
        "collision",
        "mixed",
        "t1-duplex",
        "t1-pipeline",
        "100base-t1",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/ethernet/media")
            .join(format!("{name}.ini"));
        let prepared = prepare(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let snapshot = dir_simulator::runtime::simulate(&prepared).unwrap();
        assert!(!snapshot.common.partial, "{name}");
        assert!(!snapshot.ethernet.unwrap().attempts.is_empty(), "{name}");
    }
}

#[test]
fn carrier_on_ifg_deadline_resets_continuous_idle_and_post_preamble_collision_jams_immediately() {
    let mut p = prepare(&fixture("carrier.ini")).unwrap();
    p.ethernet.as_mut().unwrap().generators[0].times_ps = vec![0, 1_000_000];
    let s = dir_simulator::runtime::simulate(&p).unwrap();
    assert!(!s.common.partial);
    let eth = s.ethernet.unwrap();
    let b = eth
        .transfers
        .iter()
        .find(|t| t.from_port == "Main.b.tx")
        .unwrap();
    assert_eq!(b.sof_ps, Some(13_540_000));
    assert!(eth.attempts.iter().all(|a| a.collision_ps.is_none()));
    let mut p = prepare(&fixture("collision.ini")).unwrap();
    for direction in &mut p.ethernet.as_mut().unwrap().directions {
        direction.delay_ps = 2_399_999;
    }
    let s = dir_simulator::runtime::simulate(&p).unwrap();
    assert!(!s.common.partial);
    for a in s
        .ethernet
        .unwrap()
        .attempts
        .iter()
        .filter(|a| a.number == 1)
    {
        assert_eq!(a.collision_ps, Some(2_399_999));
        assert_eq!(a.planned_jam_start_ps, a.collision_ps);
        assert_eq!(a.jam_end_ps, Some(2_719_999));
    }
}
