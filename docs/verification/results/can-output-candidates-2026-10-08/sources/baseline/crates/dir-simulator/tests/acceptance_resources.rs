//! DIR-TEST-0082 exercises real finite queues with synthetic integer-ps wires.
use dir_simulator::{
    prepare, runtime,
    types::{Frame, Generator, Schedule},
};
use std::path::Path;

fn prepared() -> dir_simulator::PreparedSimulation {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/gw/independent-only.ini");
    let mut p = prepare(&path).unwrap();
    // A 50-bit empty ID0 frame plus 3-bit release interval takes exactly 10ps.
    // This is a scheduler/resource fixture, not a physical CAN bitrate claim.
    for bus in &mut p.can.buses {
        bus.bitrate = 5_300_000_000_000;
    }
    p.can.bitrate = 5_300_000_000_000;
    for c in &mut p.can.controllers {
        c.queue_capacity = 1;
        c.tx_processing_ps = 0;
        c.rx_processing_ps = 0;
        c.tx_channel_ps = 0;
        c.rx_channel_ps = 0;
    }
    let source = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Main.src")
        .unwrap();
    let independent = p
        .can
        .controllers
        .iter()
        .position(|c| c.id == "Main.sink")
        .unwrap();
    p.can.generators = [
        ("A", source, 0, 0),
        ("B", source, 2, 7),
        ("C", source, 3, 1),
        ("D", source, 10, 0),
        ("S", independent, 3, 0),
    ]
    .into_iter()
    .map(|(id, source, at, frame_id)| Generator {
        id: id.into(),
        source,
        frame: Frame {
            format: "standard".into(),
            id: frame_id,
            data: String::new(),
        },
        schedule: Schedule::Explicit(vec![at]),
    })
    .collect();
    p.common.time_limit_ps = 40;
    p
}

#[test]
fn finite_waiting_capacity_release_arrival_and_independent_resource() {
    let mut p = prepared();
    for limit in [3, 4, 10, 11, 40] {
        p.common.time_limit_ps = limit;
        let s = runtime::simulate(&p).unwrap();
        assert!(!s.common.partial, "{:?}", s.common.diagnostics);
        let a = s
            .can
            .requests
            .iter()
            .find(|r| r.request_id == "A:0")
            .unwrap();
        let b = s
            .can
            .requests
            .iter()
            .find(|r| r.request_id == "B:0")
            .unwrap();
        assert_eq!(a.sof_ps, Some(0));
        assert_eq!(a.planned_release_ps, Some(10));
        if limit <= 10 {
            assert_eq!(a.status, "in_flight");
            assert_eq!(b.status, "pending");
            assert_eq!(b.sof_ps, None);
        } else {
            assert_eq!(b.sof_ps, Some(10));
            let wait = s
                .common
                .points
                .iter()
                .find(|v| {
                    v.request_id.as_deref() == Some("B:0") && v.metric == "arbitration_wait_ps"
                })
                .unwrap();
            assert_eq!(wait.value, 8);
            assert_eq!(
                s.can
                    .requests
                    .iter()
                    .find(|r| r.request_id == "D:0")
                    .unwrap()
                    .status,
                "dropped"
            );
        }
        if limit > 3 {
            assert_eq!(
                s.can
                    .requests
                    .iter()
                    .find(|r| r.request_id == "C:0")
                    .unwrap()
                    .status,
                "dropped"
            );
            assert_eq!(
                s.can
                    .requests
                    .iter()
                    .find(|r| r.request_id == "S:0")
                    .unwrap()
                    .sof_ps,
                Some(3)
            );
        }
        let lengths: Vec<_> = s
            .common
            .points
            .iter()
            .filter(|v| v.target == "Main.src.txQueue" && v.metric == "queue_length")
            .collect();
        assert!(!lengths.is_empty());
        assert!(lengths.iter().all(|v| v.value <= 1));
    }
    p.common.time_limit_ps = 40;
    p.can
        .controllers
        .iter_mut()
        .find(|c| c.id == "Main.src")
        .unwrap()
        .queue_capacity = 0;
    let s = runtime::simulate(&p).unwrap();
    assert!(
        s.can
            .requests
            .iter()
            .filter(|r| r.source == "Main.src")
            .all(|r| r.status == "dropped" && r.sof_ps.is_none())
    );
    assert!(
        s.common
            .points
            .iter()
            .filter(|v| v.target == "Main.src.txQueue" && v.metric == "queue_length")
            .all(|v| v.value == 0)
    );
}

#[test]
fn failed_winner_time_reservation_keeps_waiting_request_and_queue() {
    let mut p = prepared();
    p.can.generators.truncate(2);
    p.can.generators[0].schedule = Schedule::Explicit(vec![u64::MAX - 15]);
    p.can.generators[1].schedule = Schedule::Explicit(vec![u64::MAX - 13]);
    p.common.time_limit_ps = u64::MAX;
    let s = runtime::simulate(&p).unwrap();
    assert_eq!(s.common.termination, "execution_failed");
    assert!(s.common.partial);
    assert_eq!(s.common.diagnostics[0].code, "E-0004");
    let a = s
        .can
        .requests
        .iter()
        .find(|r| r.request_id == "A:0")
        .unwrap();
    let b = s
        .can
        .requests
        .iter()
        .find(|r| r.request_id == "B:0")
        .unwrap();
    assert_eq!(a.release_ps, Some(u64::MAX - 5));
    assert_eq!(b.status, "pending");
    assert_eq!(b.sof_ps, None);
    assert_eq!(b.planned_eof_ps, None);
    assert_eq!(
        s.common
            .points
            .iter()
            .rev()
            .find(|v| v.target == "Main.src.txQueue" && v.metric == "queue_length")
            .unwrap()
            .value,
        1
    );
}
