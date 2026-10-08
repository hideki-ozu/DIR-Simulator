//! Cross-model committed-state invariants, including every partial-run boundary.
use super::simulate;
use crate::snapshot::Snapshot;
use std::collections::BTreeSet;
use std::path::Path;

fn assert_lineage(snapshot: &Snapshot) {
    let requests: BTreeSet<_> = snapshot
        .can
        .requests
        .iter()
        .map(|r| r.request_id.as_str())
        .collect();
    assert_eq!(requests.len(), snapshot.can.requests.len());
    assert_eq!(
        requests,
        snapshot
            .gateway
            .request_lineage
            .keys()
            .map(String::as_str)
            .collect()
    );
    for (id, lineage) in &snapshot.gateway.request_lineage {
        assert!(requests.contains(lineage.origin_request_id.as_str()));
        if let Some(parent) = &lineage.parent_request_id {
            let parent_lineage = &snapshot.gateway.request_lineage[parent];
            assert_eq!(lineage.origin_request_id, parent_lineage.origin_request_id);
            assert_eq!(lineage.gw_hops, parent_lineage.gw_hops + 1);
        } else {
            assert_eq!(lineage.origin_request_id, *id);
            assert_eq!(lineage.gw_hops, 0);
        }
    }
    for forward in &snapshot.gateway.forwards {
        if let Some(child) = &forward.child_request_id {
            let lineage = &snapshot.gateway.request_lineage[child];
            assert_eq!(
                lineage.parent_request_id.as_deref(),
                Some(
                    snapshot.can.requests[forward.parent_request]
                        .request_id
                        .as_str()
                )
            );
        } else {
            assert!(!requests.contains(forward.forward_id.as_str()));
        }
    }
    for (index, buffer) in snapshot.gateway.rx_buffers.iter().enumerate() {
        let outstanding: BTreeSet<_> = snapshot
            .gateway
            .forwards
            .iter()
            .filter(|f| f.rx_buffer == index)
            .filter(|f| {
                f.status == "processing"
                    || f.child_request_id.as_ref().is_some_and(|id| {
                        let request = snapshot
                            .can
                            .requests
                            .iter()
                            .find(|r| &r.request_id == id)
                            .unwrap();
                        request.tx_enqueued_ps.is_none() && request.status != "dropped"
                    })
            })
            .filter_map(|f| f.egress)
            .collect();
        assert_eq!(buffer.remaining_egress, outstanding);
        if buffer.status == "holding" {
            assert!(buffer.released_ps.is_none());
            assert!(
                snapshot
                    .gateway
                    .rx_buffers
                    .iter()
                    .filter(|b| b.ingress == buffer.ingress && b.status == "holding")
                    .count() as u64
                    <= buffer.capacity
            );
        } else {
            assert!(buffer.remaining_egress.is_empty());
        }
    }
}

#[test]
fn lineage_tracks_only_committed_requests_across_gateway_boundaries() {
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/verification/fixtures/gw");
    for name in [
        "independent-only.ini",
        "multicast.ini",
        "multicast-drop.ini",
        "hop.ini",
        "capacity-zero.ini",
        "queue.ini",
    ] {
        let mut prepared = crate::prepare(&fixtures.join(name)).unwrap();
        let complete = simulate(&prepared).unwrap();
        assert!(!complete.common.partial, "{name}");
        assert_lineage(&complete);
        for limit in 0..complete.common.committed_events {
            prepared.common.max_events = limit;
            let partial = simulate(&prepared).unwrap();
            assert!(partial.common.partial, "{name}: {limit}");
            assert_eq!(partial.common.committed_events, limit);
            assert_lineage(&partial);
        }
    }
}

#[test]
fn failed_gateway_preflight_does_not_register_children() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/gw/multicast.ini");
    let mut prepared = crate::prepare(&config).unwrap();
    prepared.gateway.gateways[0].processing_delay_ps = u64::MAX;
    let snapshot = simulate(&prepared).unwrap();
    assert!(snapshot.common.partial);
    assert!(snapshot.gateway.forwards.is_empty());
    assert!(snapshot.gateway.rx_buffers.is_empty());
    assert_eq!(snapshot.can.requests.len(), 1);
    assert_lineage(&snapshot);
}

#[test]
fn simultaneous_buses_reserve_arbitration_as_one_batch() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/gw/independent-only.ini");
    let prepared = crate::prepare(&config).unwrap();
    let snapshot = simulate(&prepared).unwrap();
    assert!(!snapshot.common.partial);
    let arbitrations: Vec<_> = snapshot
        .common
        .points
        .iter()
        .filter(|p| p.time_ps == 0 && p.metric == "arbitration_wait_ps")
        .map(|p| (p.target.as_str(), p.event_seq))
        .collect();
    // Dispatcher, two Generate and two Ready events precede the batch. Both
    // buses must receive sequence IDs before either reserves EOF/release.
    assert_eq!(
        arbitrations,
        vec![("Main.src", Some(5)), ("Main.sink", Some(6))]
    );
    let mut requests: Vec<_> = snapshot
        .can
        .requests
        .iter()
        .map(|r| {
            (
                r.request_id.as_str(),
                r.status.as_str(),
                r.sof_ps,
                r.eof_ps,
                r.release_ps,
            )
        })
        .collect();
    requests.sort_unstable();
    assert_eq!(
        requests,
        vec![
            (
                "other:0",
                "success",
                Some(0),
                Some(94_000_000),
                Some(100_000_000)
            ),
            (
                "source:0",
                "success",
                Some(0),
                Some(100_000_000),
                Some(106_000_000)
            )
        ]
    );
}

#[test]
fn failed_arbitration_batch_preserves_all_dirty_buses() {
    use super::engine::Engine;
    use std::collections::BinaryHeap;

    let config = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/verification/fixtures/gw/independent-only.ini");
    let mut prepared = crate::prepare(&config).unwrap();
    prepared.can.generators.clear();
    let snapshot = simulate(&prepared).unwrap();
    let initial_points = snapshot.common.points.len();
    let engine = Engine {
        prepared: &prepared,
        wires: Vec::new(),
        heap: BinaryHeap::new(),
        // One reservation would fit; the complete two-bus batch must fail
        // before consuming either dirty entry or committing a callback.
        next_sequence: u64::MAX - 1,
        now: (0, 0, 1, 0),
        dirty: BTreeSet::from([(0, 0, 0), (0, 0, 1)]),
        cursors: Vec::new(),
        future_generation: false,
        queues: vec![BTreeSet::new(); prepared.can.controllers.len()],
        requests: Default::default(),
        receivers: Default::default(),
        forwards: Default::default(),
        rx_buffers: Default::default(),
        request_generators: Default::default(),
        active: vec![None; prepared.can.buses.len()],
        request_sources: Default::default(),
        request_forwards: Default::default(),
        rx_used: vec![0; prepared.can.controllers.len()],
        tx_waiting: vec![BTreeSet::new(); prepared.can.controllers.len()],
        routed: BTreeSet::new(),
        request_events: Default::default(),
        request_receivers: Default::default(),
        retire_candidates: Default::default(),
        archive: None,
        snapshot,
    };
    let failed = engine.finish();
    assert!(failed.common.partial);
    assert_eq!(failed.common.termination, "execution_failed");
    assert_eq!(failed.common.committed_events, 0);
    assert_eq!(failed.common.last_event_time_ps, None);
    assert_eq!(failed.common.pending_events, 2);
    assert_eq!(failed.common.points.len(), initial_points);
    assert!(failed.can.requests.is_empty());
    assert!(failed.can.bus_states.iter().all(|state| state == "idle"));
    assert_eq!(failed.common.diagnostics.len(), 1);
    assert_eq!(failed.common.diagnostics[0].code, "E-0004");
    assert_eq!(failed.common.diagnostics[0].event_seq, None);
    assert_eq!(
        failed.common.diagnostics[0].details.as_ref().unwrap()["operation"],
        "seal_arbitration"
    );
    assert_eq!(
        failed.common.diagnostics[0].message,
        "event sequence overflow"
    );
}

struct ArchiveTemp(std::path::PathBuf);
impl ArchiveTemp {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "dir-can-archive-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for ArchiveTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_archive_matches_materialized(prepared: &crate::PreparedSimulation) {
    let memory = simulate(prepared).unwrap();
    let temp = ArchiveTemp::new();
    let disk = super::spool::with_spooling(&temp.0, || simulate(prepared)).unwrap();
    assert!(
        disk.common.spool_error.is_none(),
        "{:?}",
        disk.common.spool_error
    );
    assert_eq!(memory.common.committed_events, disk.common.committed_events);
    assert_eq!(memory.common.pending_events, disk.common.pending_events);
    assert_eq!(memory.common.termination, disk.common.termination);
    assert!(disk.can.requests.is_empty());
    assert!(disk.can.receivers.is_empty());
    assert!(disk.gateway.request_lineage.is_empty());
    let archive = disk.can.archive.as_ref().unwrap();
    let mut actual: Vec<_> = archive
        .iter()
        .unwrap()
        .map(Result::unwrap)
        .map(|row| serde_json::to_value(row).unwrap())
        .collect();
    let mut expected = Vec::new();
    for (index, request) in memory.can.requests.iter().enumerate() {
        let mut forwards: Vec<_> = memory
            .gateway
            .forwards
            .iter()
            .filter(|f| f.parent_request == index)
            .cloned()
            .collect();
        for row in &mut forwards {
            row.parent_request = 0;
        }
        let mut rx_buffers: Vec<_> = memory
            .gateway
            .rx_buffers
            .iter()
            .filter(|b| b.parent_request == index)
            .cloned()
            .collect();
        for row in &mut rx_buffers {
            row.parent_request = 0;
        }
        expected.push(
            serde_json::to_value(super::can_archive::ArchivedRequest {
                request: request.clone(),
                lineage: memory.gateway.request_lineage[&request.request_id].clone(),
                receivers: memory
                    .can
                    .receivers
                    .iter()
                    .filter(|r| r.request_id == request.request_id)
                    .cloned()
                    .collect(),
                forwards,
                rx_buffers,
            })
            .unwrap(),
        );
    }
    let key = |row: &serde_json::Value| row["request"]["request_id"].as_str().unwrap().to_owned();
    actual.sort_by_key(key);
    expected.sort_by_key(key);
    assert_eq!(expected, actual);
    assert_eq!(
        archive.iter().unwrap().count(),
        expected.len(),
        "repeatable ledger reader"
    );
    drop(disk);
    assert_eq!(
        std::fs::read_dir(&temp.0).unwrap().count(),
        0,
        "owned journal cleanup"
    );
}

#[test]
fn archived_ledgers_preserve_gateway_holds_delayed_receivers_and_partial_prefixes() {
    for name in [
        "delay",
        "queue",
        "capacity-zero",
        "multicast",
        "multicast-drop",
        "hop",
        "no-route",
        "rx-filter",
        "disjoint-cycle",
        "eof-boundary",
        "forward-boundary",
    ] {
        let config = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/verification/fixtures/gw")
            .join(format!("{name}.ini"));
        let mut prepared = crate::prepare(&config).unwrap();
        assert_archive_matches_materialized(&prepared);
        let complete = simulate(&prepared).unwrap().common.committed_events;
        for limit in [0, 1, complete / 2, complete.saturating_sub(1)] {
            prepared.common.max_events = limit;
            assert_archive_matches_materialized(&prepared);
        }
        prepared.common.time_limit_ps = 0;
        assert_archive_matches_materialized(&prepared);
    }
}

#[test]
fn archived_request_storage_tracks_live_work_not_generated_history() {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/can/baseline.ini");
    let mut prepared = crate::prepare(&config).unwrap();
    prepared.can.generators.truncate(1);
    prepared.can.generators[0].schedule = crate::types::Schedule::Periodic {
        start: 0,
        phase: 0,
        period: 1_000_000_000,
        end: None,
        count: Some(10_000),
    };
    prepared.common.time_limit_ps = 10_000_000_000_000;
    let temp = ArchiveTemp::new();
    let snapshot = super::spool::with_spooling(&temp.0, || simulate(&prepared)).unwrap();
    assert!(snapshot.common.spool_error.is_none());
    let archive = snapshot.can.archive.as_ref().unwrap();
    assert_eq!(archive.len(), 10_000);
    assert!(
        archive.peak_live_requests() < 100,
        "{} live requests",
        archive.peak_live_requests()
    );
    assert!(
        archive.peak_live_receivers() < 100,
        "{} live receivers",
        archive.peak_live_receivers()
    );
}
