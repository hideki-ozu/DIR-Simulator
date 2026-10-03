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
