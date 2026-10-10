//! Product acceptance evidence for DIR-TEST-0109..0116. Expectations are independent
//! integer arithmetic and declared policies, not values returned by the serializer.
use dir_simulator::{prepare, run};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/acceptance-2026-10-08/dynamic-tsn")
}
fn expected() -> Value {
    serde_json::from_slice(&fs::read(fixtures().join("expected.json")).unwrap()).unwrap()
}
fn product(name: &str, horizon: Option<u64>) -> Value {
    let output = std::env::temp_dir().join(format!(
        "dir-acceptance-dynamic-tsn-runs/{}-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        horizon.unwrap_or(200_000_000_000)
    ));
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    let mut p = prepare(&fixtures().join(format!("{name}.ini")))
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    if let Some(h) = horizon {
        p.common.time_limit_ps = h;
    }
    run(p, &output).unwrap_or_else(|e| panic!("{name}@{horizon:?}: {e:?}"));
    let value: Value =
        serde_json::from_slice(&fs::read(output.join("results.json")).unwrap()).unwrap();
    assert_output_projection(&output, &value);
    let dir = std::env::temp_dir().join("dir-acceptance-dynamic-tsn-cases");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{name}-{}.json",horizon.unwrap_or(200_000_000_000))),serde_json::to_vec_pretty(&json!({"fixture":name,"horizon":horizon,"result":output.join("results.json"),"termination":value["simulation"]["termination"]})).unwrap()).unwrap();
    value
}
fn rows<'a>(r: &'a Value, s: &str) -> Vec<&'a Value> {
    r["simulation"]["model_records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["schema_name"] == s)
        .collect()
}
fn number(v: &Value) -> u64 {
    v.as_str().unwrap().parse().unwrap()
}
fn transfer<'a>(r: &'a Value, port: &str) -> Vec<&'a Value> {
    rows(r, "ethernet.dynamic.transfer")
        .into_iter()
        .filter(|x| x["data"]["from_port"] == port)
        .collect()
}
fn sorted_times(r: &Value, port: &str, key: &str) -> Vec<u64> {
    let mut v = transfer(r, port)
        .iter()
        .filter(|x| x["data"][key].is_string())
        .map(|x| number(&x["data"][key]))
        .collect::<Vec<_>>();
    v.sort_unstable();
    v
}
fn data_without_effect(r: &Value) -> Value {
    let mut r = r.clone();
    if let Some(o) = r.as_object_mut() {
        o.remove("effect_seq");
    }
    r
}
#[test]
fn fixtures_prepare_and_execute_all_positive_cases() {
    for case in expected()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if name != "registration-capacity-error" {
            product(name, None);
        }
    }
}
fn audit(r: &Value, kind: &str) -> Vec<u64> {
    let mut a = rows(r, "ethernet.dynamic.control")
        .into_iter()
        .filter(|x| x["data"]["kind"] == kind)
        .map(|x| number(&x["time_ps"]))
        .collect::<Vec<_>>();
    a.sort_unstable();
    a
}
fn destination_copies(r: &Value, frame: &str) -> Vec<String> {
    let mut v = rows(r, "ethernet.dynamic.transfer")
        .into_iter()
        .filter(|x| {
            x["data"]["frame_id"] == frame
                && x["data"]["from_port"]
                    .as_str()
                    .unwrap()
                    .starts_with("Net.s.tx_")
        })
        .map(|x| x["data"]["from_port"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    v.sort();
    v
}
fn prefix(r: &Value, h: u64) -> Vec<Value> {
    r["simulation"]["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| number(&x["time_ps"]) < h)
        .cloned()
        .collect()
}
fn assert_prefix(full: &Value, part: &Value, h: u64) {
    assert_eq!(
        prefix(full, h),
        prefix(part, h),
        "metric/event/effect prefix at {h}"
    );
    let partial_rows = part["simulation"]["model_records"].as_array().unwrap();
    for schema in [
        "ethernet.dynamic.control",
        "ethernet.dynamic.policy",
        "ethernet.tsn.gate",
        "ethernet.tsn.credit",
        "ethernet.tsn.policing",
        "ethernet.tsn.decision",
    ] {
        let expected = rows(full, schema)
            .into_iter()
            .filter(|x| {
                number(&x["time_ps"]) < h
                    || schema == "ethernet.dynamic.policy" && x["data"]["initial"] == true
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            expected,
            rows(part, schema).into_iter().cloned().collect::<Vec<_>>(),
            "{schema} prefix at {h}"
        );
    }
    for x in rows(full, "ethernet.dynamic.transfer") {
        if number(&x["data"]["queued_ps"]) >= h {
            continue;
        }
        let p = partial_rows
            .iter()
            .find(|p| p["schema_name"] == x["schema_name"] && p["record_id"] == x["record_id"])
            .unwrap();
        for key in ["sof_ps", "eof_ps", "release_ps", "arrival_ps"] {
            let actual = &x["data"][key];
            let want = if actual.is_string() && number(actual) < h {
                actual.clone()
            } else {
                Value::Null
            };
            assert_eq!(p["data"][key], want, "{}:{key}@{h}", x["record_id"]);
        }
    }
    for x in rows(part, "ethernet.dynamic.reception") {
        assert!(
            number(&x["data"]["observed_ps"]) < h,
            "unobserved reception@{h}"
        );
    }
}
#[test]
fn dynamic_wire_control_and_expiry_t_minus_one_t_t_plus_one_prefix() {
    let full = product("wire-boundaries", None);
    let e = &expected()["cases"][0]["expected"];
    for hop in e["hops"].as_array().unwrap() {
        let t = transfer(&full, hop[0].as_str().unwrap());
        assert_eq!(t.len(), 1);
        for (i, key) in ["sof_ps", "eof_ps", "release_ps", "arrival_ps"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(number(&t[0]["data"][key]), hop[i + 1].as_u64().unwrap());
        }
    }
    assert_eq!(audit(&full, "registration_expire"), [2100]);
    assert_eq!(audit(&full, "mac_expire"), [1609000, 2220000]);
    let mut boundaries = e["wire_boundaries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(e["control_expiry_boundaries"].as_array().unwrap())
        .map(|x| x.as_u64().unwrap())
        .collect::<Vec<_>>();
    boundaries.sort_unstable();
    boundaries.dedup();
    for t in boundaries {
        for h in [t.saturating_sub(1), t, t + 1] {
            let part = product("wire-boundaries", Some(h));
            if h > 0 {
                assert_prefix(&full, &part, h);
            } else {
                assert!(rows(&part, "ethernet.dynamic.frame").is_empty());
            }
        }
    }
}
#[test]
fn dynamic_wire_down_and_up_receptions_use_arrival_policy() {
    for (name, reason, b_sof) in [
        ("down-onwire", Some("link_down"), None),
        ("up-before-arrival", None, Some(1222000)),
        ("up-after-arrival", Some("stp_discarding"), None),
    ] {
        let r = product(name, None);
        let t = transfer(&r, "Net.s1.tx_b");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0]["data"]["sof_ps"], "611000");
        assert_eq!(t[0]["data"]["eof_ps"], "1219000");
        assert_eq!(t[0]["data"]["release_ps"], "1315000");
        let reception = rows(&r, "ethernet.dynamic.reception")
            .into_iter()
            .find(|x| x["data"]["ingress"] == "Net.s2.rx_b")
            .unwrap();
        assert_eq!(reception["data"]["observed_ps"], "1220000");
        assert_eq!(
            reception["data"]["reason"],
            reason.map(Value::from).unwrap_or(Value::Null)
        );
        assert_eq!(
            sorted_times(&r, "Net.s2.tx_a", "sof_ps"),
            b_sof.into_iter().collect::<Vec<_>>()
        );
        if name == "up-after-arrival" {
            assert_eq!(audit(&r, "tree_publish"), [1900000]);
        }
    }
    let r = product("down-at-sof", None);
    assert!(
        transfer(&r, "Net.s1.tx_b")
            .iter()
            .all(|x| x["data"]["sof_ps"].is_null())
    );
    assert!(
        rows(&r, "ethernet.dynamic.reception")
            .iter()
            .all(|x| x["data"]["ingress"] != "Net.s2.rx_b")
    );
}
#[test]
fn dynamic_revisit_has_unique_copy_parent_dag_and_visit_limit() {
    let r = product("actual-revisit", None);
    let receptions = rows(&r, "ethernet.dynamic.reception");
    let mut observed = receptions
        .iter()
        .map(|x| {
            (
                x["data"]["ingress"]
                    .as_str()
                    .unwrap()
                    .rsplit_once(".")
                    .unwrap()
                    .0
                    .to_string(),
                number(&x["data"]["observed_ps"]),
            )
        })
        .collect::<Vec<_>>();
    observed.sort_by_key(|x| x.1);
    assert_eq!(
        observed,
        [
            ("Net.s1", 609000),
            ("Net.s3", 1220000),
            ("Net.s2", 1929000),
            ("Net.s1", 2540000),
            ("Net.s3", 3151000)
        ]
        .map(|(n, t)| (n.to_string(), t))
    );
    assert_eq!(sorted_times(&r, "Net.s1.tx_c", "sof_ps"), [611000, 2542000]);
    let transfers = rows(&r, "ethernet.dynamic.transfer");
    let index = transfers
        .iter()
        .map(|x| (x["record_id"].as_str().unwrap(), *x))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(index.len(), transfers.len());
    for x in &transfers {
        let mut current = *x;
        let mut seen = std::collections::BTreeSet::new();
        while let Some(parent) = current["data"]["parent_transfer_id"].as_str() {
            assert!(seen.insert(parent));
            current = index[parent];
            assert!(number(&current["data"]["queued_ps"]) <= number(&x["data"]["queued_ps"]));
        }
    }
    let limited = product("visit-limit", None);
    let third = rows(&limited, "ethernet.dynamic.reception")
        .into_iter()
        .find(|x| x["data"]["observed_ps"] == "1929000")
        .unwrap();
    assert_eq!(third["data"]["reason"], "visit_limit");
    assert!(transfer(&limited, "Net.s2.tx_b").is_empty());
}
#[test]
fn dynamic_learning_capacity_and_cancelled_expiry_remain_bounded() {
    let r = product("learning-capacity-refresh", None);
    for (kind, times) in [
        ("mac_learn", vec![608000]),
        ("mac_capacity", vec![1608000]),
        ("mac_refresh", vec![2608000]),
        ("mac_expire", vec![6608000]),
    ] {
        assert_eq!(audit(&r, kind), times, "{kind}");
    }
    let r = product("refresh-storm", Some(501));
    let d = &r["metadata"]["network_runtime"]["dynamic"];
    assert_eq!(d["registration"].as_array().unwrap().len(), 1);
    assert_eq!(d["registration"][0]["lease"]["generation"], "501");
    assert_eq!(d["registration"][0]["lease"]["expires_at"], "10000500");
    assert_eq!(r["simulation"]["pending_events"], "1");
}
#[test]
fn dynamic_ipv4_ipv6_source_filters_static_union_and_endpoint_subscription() {
    for family in ["ipv4", "ipv6"] {
        for (case, x, y) in [
            ("filters", vec!["b", "d"], vec!["c", "d"]),
            ("static-union", vec!["b", "d"], vec!["b", "c", "d"]),
            ("unsubscribed", vec!["b", "d"], vec!["c", "d"]),
            ("include-empty", vec!["d"], vec!["c", "d"]),
            ("exclude-empty", vec!["b", "c", "d"], vec!["c", "d"]),
            ("known-mismatch", vec!["b"], vec![]),
            (
                "same-mac-other-group",
                vec!["b", "c", "d"],
                vec!["b", "c", "d"],
            ),
            ("l2-null", vec!["b", "c", "d"], vec!["b", "c", "d"]),
        ] {
            let r = product(&format!("{family}-{case}"), None);
            for (source, want) in [("x", x), ("y", y)] {
                assert_eq!(
                    destination_copies(&r, &format!("{source}:0")),
                    want.into_iter()
                        .map(|p| format!("Net.s.tx_{p}"))
                        .collect::<Vec<_>>(),
                    "{family}-{case}:{source}"
                );
            }
            if case == "unsubscribed" {
                let b = rows(&r, "ethernet.dynamic.reception")
                    .into_iter()
                    .find(|x| x["data"]["ingress"] == "Net.b.rx")
                    .unwrap();
                assert_eq!(b["data"]["reason"], "multicast_not_subscribed");
            }
            if case == "known-mismatch" {
                assert!(
                    rows(&r, "ethernet.dynamic.reception")
                        .iter()
                        .any(|x| x["data"]["reason"] == "multicast_filtered")
                );
            }
        }
    }
}
#[test]
fn tas_all_ten_declared_vectors_execute_real_wire() {
    for case in expected()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["name"].as_str().unwrap().starts_with("tas-"))
    {
        let name = case["name"].as_str().unwrap();
        let r = product(name, None);
        let expected = &case["expected"];
        let ts = transfer(&r, "Net.a.tx");
        assert_eq!(
            ts[0]["data"]["sof_ps"],
            expected["sof_ps"]
                .as_u64()
                .map(|v| Value::from(v.to_string()))
                .unwrap_or(Value::Null),
            "{name}"
        );
        if let Some(sof) = expected["sof_ps"].as_u64() {
            assert_eq!(
                number(&ts[0]["data"]["eof_ps"]),
                sof + expected["wire_ps"].as_u64().unwrap()
            );
            assert_eq!(
                number(&ts[0]["data"]["release_ps"]),
                sof + expected["occupied_ps"].as_u64().unwrap()
            );
        }
        if name == "tas-10" {
            assert_eq!(sorted_times(&r, "Net.a.tx", "sof_ps"), [0, 672000]);
        }
        if name.starts_with("tas-07") {
            assert_eq!(r["simulation"]["termination"], "events_exhausted");
            assert!(
                rows(&r, "ethernet.tsn.decision")
                    .iter()
                    .any(|x| x["data"]["reason"] == "never_eligible")
            );
        }
    }
}
#[test]
fn cbs_wire_gate_and_recovery_t_minus_one_t_t_plus_one_prefix() {
    for (name, sofs, boundaries) in [
        (
            "cbs-boundaries",
            vec![0, 2688000],
            vec![576000, 672000, 2688000, 3264000, 3360000, 5376000],
        ),
        (
            "cbs-gated-boundaries",
            vec![0, 3688000],
            vec![576000, 672000, 1000000, 2000000, 2688000, 3688000, 4360000],
        ),
    ] {
        let r = product(name, None);
        assert_eq!(sorted_times(&r, "Net.a.tx", "sof_ps"), sofs);
        for t in boundaries {
            for h in [t - 1, t, t + 1] {
                let p = product(name, Some(h));
                assert_prefix(&r, &p, h);
                let credit = &p["metadata"]["network_runtime"]["tsn"]["ports"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|x| x["port"] == "Net.a.tx")
                    .unwrap()["credits"][0];
                let wanted = independent_cbs_credit(name, h);
                assert_eq!(
                    credit["credit"]["magnitude"],
                    wanted.unsigned_abs().to_string(),
                    "{name}@{h}"
                );
                assert_eq!(credit["credit"]["negative"], wanted < 0);
            }
        }
        assert!(
            rows(&r, "ethernet.tsn.credit")
                .iter()
                .any(|x| x["time_ps"] == "672000"
                    && x["data"]["sign"] == "negative"
                    && x["data"]["magnitude"] == "504000000000000")
        );
    }
}
#[test]
fn psfp_actual_arrival_refill_gate_and_sdu_before_meter() {
    const Q: u128 = 1_000_000_000_000;
    let r = product("psfp-five", None);
    let mut p = rows(&r, "ethernet.tsn.policing");
    p.sort_by_key(|x| number(&x["time_ps"]));
    let arrivals = [576000, 1248000, 1920000, 32576000, 96576000];
    let colors = ["green", "yellow", "red", "yellow", "green"];
    let mut c = 512 * Q;
    let mut peak = 1024 * Q;
    let mut last = arrivals[0];
    for (i, row) in p.iter().enumerate() {
        assert_eq!(number(&row["time_ps"]), arrivals[i]);
        let dt = (arrivals[i] - last) as u128;
        c = (c + 8_000_000 * dt).min(512 * Q);
        peak = (peak + 16_000_000 * dt).min(1024 * Q);
        assert_eq!(row["data"]["committed_before"], c.to_string());
        assert_eq!(row["data"]["peak_before"], peak.to_string());
        let consumed = match colors[i] {
            "green" => {
                c -= 512 * Q;
                peak -= 512 * Q;
                1024
            }
            "yellow" => {
                peak -= 512 * Q;
                512
            }
            _ => 0,
        };
        assert_eq!(row["data"]["color"], colors[i]);
        assert_eq!(row["data"]["consumed_bits"], consumed.to_string());
        assert_eq!(row["data"]["committed_after"], c.to_string());
        assert_eq!(row["data"]["peak_after"], peak.to_string());
        last = arrivals[i];
    }
    for (name, at, verdict) in [
        ("psfp-gate-999999", 999999, "pass"),
        ("psfp-gate-1000000", 1000000, "psfp_gate_closed"),
        ("psfp-max-sdu", 608000, "psfp_max_sdu"),
    ] {
        let r = product(name, None);
        let p = rows(&r, "ethernet.tsn.policing");
        assert_eq!(p.len(), 1);
        assert_eq!(number(&p[0]["time_ps"]), at);
        assert_eq!(p[0]["data"]["verdict"], verdict);
        if verdict != "pass" {
            assert_eq!(p[0]["data"]["committed_before"], Value::Null);
            assert_eq!(p[0]["data"]["consumed_bits"], "0");
        }
    }
}
#[test]
fn psfp_three_egress_partial_fanout_consumes_once_without_refund() {
    let r = product("psfp-three-egress-queue-full", None);
    let p = rows(&r, "ethernet.tsn.policing")
        .into_iter()
        .filter(|x| x["data"]["stream_id"] == "fanout-meter")
        .collect::<Vec<_>>();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0]["time_ps"], "608000");
    assert_eq!(p[0]["data"]["color"], "green");
    assert_eq!(p[0]["data"]["consumed_bits"], "1088");
    assert_eq!(p[0]["data"]["committed_after"], "544000000000000");
    assert_eq!(p[0]["data"]["peak_after"], "1632000000000000");
    assert_eq!(
        destination_copies(&r, "fanout:0"),
        ["Net.s.tx_b", "Net.s.tx_c", "Net.s.tx_d"]
    );
    let b = transfer(&r, "Net.s.tx_b");
    assert_eq!(b[0]["data"]["drop_reason"], "queue_full");
    assert!(b[0]["data"]["sof_ps"].is_null());
    for port in ["Net.s.tx_c", "Net.s.tx_d"] {
        assert_eq!(sorted_times(&r, port, "sof_ps"), [608000]);
    }
}
#[test]
fn psfp_queued_vid_expiry_preserves_tokens_and_resets_positive_credit() {
    let r = product("psfp-queued-vid-expiry", None);
    let p = rows(&r, "ethernet.tsn.policing")
        .into_iter()
        .find(|x| x["data"]["stream_id"] == "expiry-meter")
        .unwrap();
    assert_eq!(p["time_ps"], "1312000");
    assert_eq!(p["data"]["consumed_bits"], "1088");
    let dropped = transfer(&r, "Net.s.tx_b")
        .into_iter()
        .find(|x| x["data"]["frame_id"] == "b_metered:0")
        .unwrap();
    assert_eq!(dropped["data"]["drop_reason"], "vlan_unregistered");
    assert_eq!(dropped["time_ps"], "3000000");
    assert!(dropped["data"]["sof_ps"].is_null());
    let meter = &r["metadata"]["network_runtime"]["tsn"]["meters"][0];
    assert_eq!(meter["last_evaluated_ps"], "1312000");
    assert_eq!(meter["committed"], p["data"]["committed_after"]);
    assert_eq!(meter["peak"], p["data"]["peak_after"]);
    let credits = rows(&r, "ethernet.tsn.credit")
        .into_iter()
        .filter(|x| {
            x["data"]["port"] == "Net.s.tx_b"
                && x["data"]["priority"] == "7"
                && x["time_ps"] == "3000000"
        })
        .collect::<Vec<_>>();
    assert!(
        credits
            .iter()
            .any(|x| x["data"]["magnitude"] == "4220000000000")
    );
    assert!(credits.iter().any(|x| x["data"]["magnitude"] == "0"));
    for h in [2999999, 3000000, 3000001] {
        let part = product("psfp-queued-vid-expiry", Some(h));
        assert_prefix(&r, &part, h);
        let credit = &part["metadata"]["network_runtime"]["tsn"]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["port"] == "Net.s.tx_b")
            .unwrap()["credits"][0];
        let wanted = if h <= 3_000_000 {
            2_500_000u128 * u128::from(h - 1_312_000)
        } else {
            0
        };
        assert_eq!(credit["credit"]["magnitude"], wanted.to_string());
        assert_eq!(credit["backlog"], h <= 3_000_000);
    }
}
#[test]
fn same_time_wire_policy_aging_registration_gcl_and_arrival_order() {
    let r = product("same-time-domains", Some(1219001));
    let at = 1219000;
    let t = transfer(&r, "Net.s1.tx_b");
    assert_eq!(t[0]["data"]["eof_ps"], at.to_string());
    let eof = number(&t[0]["data"]["effect_seq"]);
    let controls = rows(&r, "ethernet.dynamic.control")
        .into_iter()
        .filter(|x| number(&x["time_ps"]) == at && x["data"]["kind"] != "mac_learn")
        .collect::<Vec<_>>();
    assert!(controls.iter().any(|x| x["data"]["kind"] == "link_set"));
    assert!(
        controls
            .iter()
            .any(|x| x["data"]["kind"] == "registration_expire")
    );
    assert!(controls.iter().any(|x| x["data"]["kind"] == "mac_flush"));
    assert!(!controls.iter().any(|x| x["data"]["kind"] == "mac_expire"));
    let gate = rows(&r, "ethernet.tsn.gate")
        .into_iter()
        .find(|x| {
            number(&x["time_ps"]) == at
                && x["data"]["port"] == "Net.a.tx"
                && x["data"]["schedule_id"] == "g1"
        })
        .unwrap();
    let gate_seq = number(&gate["data"]["effect_seq"]);
    for c in &controls {
        assert!(eof < number(&c["data"]["effect_seq"]));
        assert!(number(&c["data"]["effect_seq"]) < gate_seq);
    }
    let arrival = rows(&r, "ethernet.dynamic.reception")
        .into_iter()
        .find(|x| x["data"]["ingress"] == "Net.s3.rx_a" && number(&x["data"]["observed_ps"]) == at)
        .unwrap();
    assert!(gate_seq < number(&arrival["data"]["effect_seq"]));
    assert_eq!(
        arrival["data"]["policy_epoch_ingress"],
        controls
            .iter()
            .find(|x| x["data"]["kind"] == "link_set")
            .unwrap()["data"]["epoch_after"]
    );
}
#[test]
fn canonical_input_order_and_disabled_tsn_preserve_dynamic_media() {
    let a = product("canonical-order", None);
    let b = product("permuted-order", None);
    assert_eq!(
        a["simulation"]["model_records"],
        b["simulation"]["model_records"]
    );
    assert_eq!(a["simulation"]["records"], b["simulation"]["records"]);
    assert_eq!(
        a["metadata"]["network_runtime"],
        b["metadata"]["network_runtime"]
    );
    let a = product("disabled-dynamic", None);
    let b = product("disabled-tsn", None);
    for schema in [
        "ethernet.dynamic.frame",
        "ethernet.dynamic.transfer",
        "ethernet.dynamic.reception",
    ] {
        let a = rows(&a, schema)
            .into_iter()
            .map(|x| {
                let mut x = x.clone();
                x["data"] = data_without_effect(&x["data"]);
                x
            })
            .collect::<Vec<_>>();
        let b = rows(&b, schema)
            .into_iter()
            .map(|x| {
                let mut x = x.clone();
                x["data"] = data_without_effect(&x["data"]);
                x
            })
            .collect::<Vec<_>>();
        assert_eq!(a, b, "{schema}");
    }
    assert_eq!(
        a["metadata"]["network_runtime"]["dynamic"],
        b["metadata"]["network_runtime"]["dynamic"]
    );
}
fn patch(value: &mut Value, pointer: &str, replacement: Value, remove: bool) {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let parent = value.pointer_mut(parent).unwrap();
    if let Some(o) = parent.as_object_mut() {
        if remove {
            o.remove(key);
        } else {
            o.insert(key.to_string(), replacement);
        }
    } else {
        let a = parent.as_array_mut().unwrap();
        let i: usize = key.parse().unwrap();
        if remove {
            a.remove(i);
        } else {
            a[i] = replacement;
        }
    }
}
#[test]
fn strict_dynamic_and_tsn_negatives_are_rejected_even_beyond_horizon() {
    let catalog: Value =
        serde_json::from_slice(&fs::read(fixtures().join("strict-negatives.json")).unwrap())
            .unwrap();
    let mut diagnostics = vec![];
    for case in catalog["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let base = case["base"].as_str().unwrap();
        let mut model: Value = serde_json::from_slice(
            &fs::read(fixtures().join(format!("{base}.model.json"))).unwrap(),
        )
        .unwrap();
        let mut workload: Value = serde_json::from_slice(
            &fs::read(fixtures().join(format!("{base}.workload.json"))).unwrap(),
        )
        .unwrap();
        if case["future_inputs"] == true {
            for g in workload["generators"].as_array_mut().unwrap() {
                g["times_ps"] = json!(["2"]);
            }
            for control in workload["controls"].as_array_mut().unwrap() {
                control["at_ps"] = json!("2");
            }
        }
        if case["empty_generators"] == true {
            workload["generators"] = json!([]);
        }
        let target = if case["target"] == "model" {
            &mut model
        } else {
            &mut workload
        };
        if case["duplicate_json_key"] != true {
            patch(
                target,
                case["pointer"].as_str().unwrap(),
                case["value"].clone(),
                case["remove"] == true,
            );
        }
        if let Some(extra) = case["extra"].as_array() {
            for p in extra {
                let target = if p["target"] == "model" {
                    &mut model
                } else {
                    &mut workload
                };
                patch(
                    target,
                    p["pointer"].as_str().unwrap(),
                    p["value"].clone(),
                    false,
                );
            }
        }
        let dir = std::env::temp_dir().join(format!(
            "dir-acceptance-dynamic-tsn-negatives/{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut model_json = serde_json::to_string(&model).unwrap();
        if case["duplicate_json_key"] == true {
            model_json = format!(
                "{{\"schema_version\":{},{}",
                model["schema_version"],
                &model_json[1..]
            );
        }
        fs::write(dir.join("model.json"), model_json).unwrap();
        fs::write(
            dir.join("workload.json"),
            serde_json::to_vec(&workload).unwrap(),
        )
        .unwrap();
        let ini = fs::read_to_string(fixtures().join(format!("{base}.ini")))
            .unwrap()
            .replace("\"models/", &format!("\"{}/models/", fixtures().display()))
            .replace(&format!("{base}.model.json"), "model.json")
            .replace(&format!("{base}.workload.json"), "workload.json")
            .replace("200ms", "1ps");
        fs::write(dir.join("run.ini"), ini).unwrap();
        let error = prepare(&dir.join("run.ini")).expect_err(name);
        assert_eq!(
            error.code,
            case["expected_code"].as_str().unwrap(),
            "{name}: {error:?}"
        );
        diagnostics.push(
            json!({"case":name,"diagnostic":error,"callback_count":0,"config":dir.join("run.ini")}),
        );
    }
    fs::write(
        std::env::temp_dir().join("dir-acceptance-dynamic-tsn-strict-negatives.json"),
        serde_json::to_vec_pretty(&diagnostics).unwrap(),
    )
    .unwrap();
}
#[test]
fn registration_capacity_error_preserves_successful_control_prefix() {
    let r = product("registration-capacity-error", None);
    assert_eq!(r["simulation"]["termination"], "execution_failed");
    let d = &r["metadata"]["network_runtime"]["dynamic"];
    assert_eq!(d["registration"].as_array().unwrap().len(), 1);
    assert_eq!(d["policy"]["policy_epoch"], "1");
    assert_eq!(rows(&r, "ethernet.dynamic.control").len(), 1);
    assert_eq!(rows(&r, "ethernet.dynamic.control")[0]["time_ps"], "100");
    assert_eq!(d["registration"][0]["key"]["port"], "Net.s.tx_b");
}
#[test]
fn membership_expiry_unknown_policy_and_queued_copy_target_remain_distinct() {
    for (name, want) in [
        (
            "membership-expiry-flood",
            vec!["Net.s.tx_b", "Net.s.tx_c", "Net.s.tx_d"],
        ),
        ("membership-expiry-drop", vec![]),
    ] {
        let r = product(name, None);
        assert_eq!(audit(&r, "membership_expire"), [1000000]);
        assert_eq!(destination_copies(&r, "y:0"), want);
        for h in [999999, 1000000, 1000001] {
            let p = product(name, Some(h));
            assert_prefix(&r, &p, h);
        }
    }
    let r = product("psfp-queued-membership-leave", None);
    let b = transfer(&r, "Net.s.tx_b")
        .into_iter()
        .find(|x| x["data"]["frame_id"] == "b_metered:0")
        .unwrap();
    assert_eq!(b["data"]["sof_ps"], "71008000");
    assert_eq!(b["data"]["drop_reason"], Value::Null);
    assert_eq!(
        destination_copies(&r, "b_metered:0"),
        ["Net.s.tx_b", "Net.s.tx_c", "Net.s.tx_d"]
    );
    assert_eq!(audit(&r, "membership_leave"), [3000000]);
    let meter = rows(&r, "ethernet.tsn.policing")
        .into_iter()
        .filter(|x| x["data"]["stream_id"] == "expiry-meter")
        .collect::<Vec<_>>();
    assert_eq!(meter.len(), 1);
    assert_eq!(meter[0]["data"]["consumed_bits"], "1088");
    let r = product("registration-unregister-reregister", None);
    assert_eq!(audit(&r, "registration_expire"), [1500]);
    for h in [
        99, 100, 101, 599, 600, 601, 999, 1000, 1001, 1499, 1500, 1501, 1600, 1601,
    ] {
        let p = product("registration-unregister-reregister", Some(h));
        assert_prefix(&r, &p, h);
    }
}
#[test]
fn gcl_update_t_minus_one_t_t_plus_one_prefix_and_permutation() {
    for (name, t) in [("tas-08", 2000000), ("tas-09", 500000), ("tas-10", 672000)] {
        let r = product(name, None);
        for h in [t - 1, t, t + 1] {
            let p = product(name, Some(h));
            assert_prefix(&r, &p, h);
        }
    }
    let a = product("tas-08", None);
    let b = product("tas-order-permuted", None);
    assert_eq!(
        a["simulation"]["model_records"],
        b["simulation"]["model_records"]
    );
    assert_eq!(a["simulation"]["records"], b["simulation"]["records"]);
}
fn assert_output_projection(output: &Path, result: &Value) {
    let manifest: Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], 2);
    assert_eq!(manifest["run_id"], result["run_id"]);
    assert_eq!(manifest["files"].as_array().unwrap().len(), 4);
    let mut files = fs::read_dir(output)
        .unwrap()
        .map(|x| x.unwrap().file_name().to_str().unwrap().to_string())
        .collect::<Vec<_>>();
    files.sort();
    assert_eq!(
        files,
        [
            "diagnostics.jsonl",
            "events.csv",
            "manifest.json",
            "results.json",
            "summary.csv"
        ]
    );
    for f in manifest["files"].as_array().unwrap() {
        let bytes = fs::read(output.join(f["name"].as_str().unwrap())).unwrap();
        assert_eq!(number(&f["bytes"]), bytes.len() as u64);
        assert_eq!(f["sha256"], format!("{:x}", Sha256::digest(&bytes)));
    }
    for (file, key) in [("events.csv", "records"), ("summary.csv", "summary")] {
        let csv = fs::read_to_string(output.join(file)).unwrap();
        let mut lines = csv.lines();
        let headers = lines.next().unwrap().split(',').collect::<Vec<_>>();
        let actual = lines.collect::<Vec<_>>();
        let expected = result["simulation"][key].as_array().unwrap();
        assert_eq!(actual.len(), expected.len());
        for (line, row) in actual.iter().zip(expected) {
            let columns = line.split(',').collect::<Vec<_>>();
            assert_eq!(
                columns.len(),
                headers.len(),
                "acceptance fixture unexpectedly needs quoted CSV parser"
            );
            for (header, cell) in headers.iter().zip(columns) {
                let v = match *header {
                    "schema_version" => &result["schema_version"],
                    "run_id" => &result["run_id"],
                    _ => &row[*header],
                };
                if v.is_number() {
                    assert_eq!(
                        cell.parse::<f64>().unwrap(),
                        v.as_f64().unwrap(),
                        "{file}:{header}"
                    );
                } else {
                    assert_eq!(cell, v.as_str().unwrap_or(""), "{file}:{header}");
                }
            }
        }
    }
    let schemas = result["metadata"]["model_schemas"].as_array().unwrap();
    let tsn = result["metadata"]["model_profile"] == "ethernet.tsn.v1";
    assert_eq!(schemas.len(), if tsn { 9 } else { 5 });
    for row in result["simulation"]["model_records"].as_array().unwrap() {
        assert_eq!(row["schema_version"], 1);
        assert!(
            schemas
                .iter()
                .any(|x| x["schema_name"] == row["schema_name"]
                    && x["schema_version"] == row["schema_version"])
        );
    }
}
#[test]
fn queued_linkdown_at_release_preserves_completed_wire_and_drops_waiting_copy() {
    let r = product("queued-linkdown-at-release", None);
    let t = transfer(&r, "Net.a.tx");
    assert_eq!(t.len(), 2);
    let first = t.iter().find(|x| x["data"]["frame_id"] == "cbs:0").unwrap();
    let second = t.iter().find(|x| x["data"]["frame_id"] == "cbs:1").unwrap();
    assert_eq!(first["data"]["sof_ps"], "0");
    assert_eq!(first["data"]["eof_ps"], "576000");
    assert_eq!(first["data"]["release_ps"], "672000");
    assert_eq!(first["data"]["arrival_ps"], "576000");
    assert_eq!(second["data"]["queued_ps"], "100000");
    assert_eq!(second["data"]["sof_ps"], Value::Null);
    assert_eq!(second["data"]["drop_reason"], "link_down");
    assert_eq!(second["time_ps"], "672000");
    let down = rows(&r, "ethernet.dynamic.control")
        .into_iter()
        .find(|x| x["record_id"] == "down")
        .unwrap();
    assert!(number(&first["data"]["effect_seq"]) < number(&down["data"]["effect_seq"]));
    assert!(number(&down["data"]["effect_seq"]) < number(&second["data"]["effect_seq"]));
    assert_eq!(rows(&r, "ethernet.dynamic.reception").len(), 1);
    for h in [671999, 672000, 672001] {
        let p = product("queued-linkdown-at-release", Some(h));
        assert_prefix(&r, &p, h);
    }
}
fn independent_cbs_credit(name: &str, t: u64) -> i128 {
    const SEND: i128 = 750_000_000;
    const IDLE: i128 = 250_000_000;
    const NEGATIVE_RELEASE: i128 = -504_000_000_000_000;
    let second = if name == "cbs-boundaries" {
        2688000
    } else {
        3688000
    };
    if t <= 672000 {
        -SEND * i128::from(t)
    } else if name == "cbs-gated-boundaries" && t <= second {
        if t <= 1000000 {
            NEGATIVE_RELEASE + IDLE * i128::from(t - 672000)
        } else if t <= 2000000 {
            -422_000_000_000_000
        } else {
            -422_000_000_000_000 + IDLE * i128::from(t - 2000000)
        }
    } else if t <= second {
        NEGATIVE_RELEASE + IDLE * i128::from(t - 672000)
    } else if t <= second + 672000 {
        -SEND * i128::from(t - second)
    } else {
        (NEGATIVE_RELEASE + IDLE * i128::from(t - second - 672000)).min(0)
    }
}
#[test]
fn psfp_untagged_tuple_uses_receiver_pvid_and_default_priority() {
    let r = product("psfp-untagged-receiver-classification", None);
    let frame = rows(&r, "ethernet.dynamic.frame");
    assert_eq!(frame[0]["data"]["source_vlan_id"], "10");
    assert_eq!(frame[0]["data"]["priority"], "7");
    assert_eq!(frame[0]["data"]["tag"], Value::Null);
    let arrival = rows(&r, "ethernet.dynamic.reception");
    assert_eq!(arrival[0]["data"]["vlan_id"], "20");
    assert_eq!(arrival[0]["data"]["priority"], "3");
    let policing = rows(&r, "ethernet.tsn.policing");
    assert_eq!(policing[0]["data"]["stream_id"], "s0");
    assert_eq!(policing[0]["data"]["verdict"], "pass");
    assert_eq!(policing[0]["data"]["consumed_bits"], "1024");
}
