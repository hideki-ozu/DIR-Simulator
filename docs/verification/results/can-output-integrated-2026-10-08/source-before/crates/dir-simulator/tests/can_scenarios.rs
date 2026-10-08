use dir_simulator::snapshot::Snapshot;
use dir_simulator::{input, runtime};
use serde_json::{Value, json};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/can")
        .join(name)
}
fn project(s: &Snapshot) -> Value {
    let status_count = |status: &str| s.can.requests.iter().filter(|r| r.status == status).count();
    let receive_count = |status: &str| {
        s.can
            .receivers
            .iter()
            .filter(|r| r.status == status)
            .count()
    };
    let mut sent: Vec<_> = s
        .can
        .requests
        .iter()
        .filter(|r| r.sof_ps.is_some())
        .collect();
    sent.sort_by_key(|r| r.sof_ps);
    let mut result = json!({
        "termination":s.common.termination,"generated":s.can.requests.len(),"success":status_count("success"),
        "dropped":status_count("dropped"),"in_flight":status_count("in_flight"),"pending":status_count("pending"),
        "received":receive_count("received"),"filtered":receive_count("filtered"),"receiver_rows":s.can.receivers.len(),
        "bus_state":s.can.bus_state,"attempts":sent.len(),
        "dropped_ids":s.can.requests.iter().filter(|r| r.status=="dropped").map(|r| &r.request_id).collect::<Vec<_>>(),
        "requests":s.can.requests.iter().map(|r| json!({"id":r.request_id,"sof_ps":r.sof_ps,"eof_ps":r.eof_ps,"release_ps":r.release_ps})).collect::<Vec<_>>(),
        "sof_order":sent.iter().map(|r| &r.request_id).collect::<Vec<_>>(),
        "sof_times_ps":sent.iter().map(|r| r.sof_ps).collect::<Vec<_>>(),
        "generated_order":s.can.requests.iter().map(|r| &r.request_id).collect::<Vec<_>>(),
        "generated_times_ps":s.can.requests.iter().map(|r| r.generated_ps).collect::<Vec<_>>()
    });
    if let Some(r) = s.can.requests.first() {
        result["sof_ps"] = json!(r.sof_ps);
        result["eof_ps"] = json!(r.eof_ps);
        result["release_ps"] = json!(r.release_ps);
    }
    for r in &s.can.receivers {
        let name = r.receiver.rsplit('.').next().unwrap();
        result[format!("{name}_observed_ps")] = json!(r.observed_ps);
        result[format!("{name}_received_ps")] = json!(r.received_ps);
    }
    result
}
#[test]
fn all_documented_can_scenarios_match_analytic_expectations() {
    let fixtures: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("scenarios.json")).unwrap()).unwrap();
    for scenario in fixtures["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let prepared = input::prepare(&fixture(scenario["config"].as_str().unwrap())).unwrap();
        let snapshot = runtime::simulate(&prepared).unwrap();
        assert!(
            !snapshot.common.partial,
            "{name}: {:?}",
            snapshot.common.diagnostics
        );
        let actual = project(&snapshot);
        for (key, expected) in scenario["expected"].as_object().unwrap() {
            assert_eq!(&actual[key], expected, "{name}: {key}");
        }
        let sum: usize = ["processing", "pending", "in_flight", "success", "dropped"]
            .iter()
            .map(|state| {
                snapshot
                    .can
                    .requests
                    .iter()
                    .filter(|r| r.status == *state)
                    .count()
            })
            .sum();
        assert_eq!(
            sum,
            snapshot.can.requests.len(),
            "{name} request conservation"
        );
        assert_eq!(
            snapshot.can.receivers.len(),
            snapshot
                .can
                .requests
                .iter()
                .filter(|r| r.status == "success")
                .count()
                * (prepared.can.controllers.len() - 1)
        );
        assert_eq!(
            actual,
            project(&runtime::simulate(&prepared).unwrap()),
            "{name} deterministic repetition"
        );
    }
}

#[test]
fn event_limit_keeps_committed_prefix_and_can_finish_normally_at_exact_count() {
    let mut p = input::prepare(&fixture("competition.ini")).unwrap();
    let complete = runtime::simulate(&p).unwrap();
    p.common.max_events = complete.common.committed_events;
    assert_eq!(
        runtime::simulate(&p).unwrap().common.termination,
        "events_exhausted"
    );
    p.common.max_events = 3;
    let partial = runtime::simulate(&p).unwrap();
    assert_eq!(partial.common.termination, "execution_failed");
    assert_eq!(partial.common.committed_events, 3);
    assert_eq!(partial.common.diagnostics[0].code, "E-0004");
    assert!(partial.common.pending_events > 0);
    assert_eq!(partial.can.requests.len(), 2);
    assert!(
        partial
            .can
            .requests
            .iter()
            .all(|r| r.status == "processing" && r.ready_ps.is_none())
    );
}

#[test]
fn overflow_in_eof_delivery_keeps_request_in_flight_without_receivers() {
    let mut p = input::prepare(&fixture("competition.ini")).unwrap();
    p.can.controllers[0].tx_channel_ps = u64::MAX;
    let s = runtime::simulate(&p).unwrap();
    assert!(s.common.partial);
    assert!(s.can.receivers.is_empty());
    assert_eq!(s.can.requests[0].status, "in_flight");
    assert_eq!(s.can.requests[0].eof_ps, None);
    assert_eq!(s.can.bus_state, "transmitting");
}

#[test]
fn zero_horizon_and_future_only_generation_do_not_generate_requests() {
    let mut p = input::prepare(&fixture("competition.ini")).unwrap();
    p.common.time_limit_ps = 0;
    let s = runtime::simulate(&p).unwrap();
    assert!(s.common.points.is_empty());
    assert!(s.can.requests.is_empty());
    assert_eq!(s.common.termination, "time_limit");
    p.can.generators.clear();
    assert_eq!(
        runtime::simulate(&p).unwrap().common.termination,
        "time_limit"
    );
}

#[test]
fn mixed_identifier_formats_follow_wire_priority() {
    let mut p = input::prepare(&fixture("competition.ini")).unwrap();
    p.can.generators[0].frame.id = 0x123;
    p.can.generators[1].frame.format = "extended".into();
    p.can.generators[1].frame.id = 0x123;
    let s = runtime::simulate(&p).unwrap();
    assert_eq!(s.can.requests[1].sof_ps, Some(0));
    p.can.generators[1].frame.id = 0x048c0000;
    let s = runtime::simulate(&p).unwrap();
    assert_eq!(s.can.requests[0].sof_ps, Some(0));
}
