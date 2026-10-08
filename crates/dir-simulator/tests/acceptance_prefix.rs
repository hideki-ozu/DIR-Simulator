//! Earlier committed observations retain their EventKey when only T changes.
use dir_simulator::{prepare, runtime, types::Schedule};
use serde_json::Value;
use std::path::Path;

#[test]
fn native_can_and_ethernet_future_dispatch_preserve_exact_committed_prefix() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for config in [
        "docs/verification/fixtures/can/delay-filter.ini",
        "docs/verification/fixtures/ethernet/unicast.ini",
    ] {
        let mut p = prepare(&root.join(config)).unwrap();
        if let Some(ethernet) = &mut p.ethernet {
            let mut future = ethernet.generators[0].clone();
            future.id = "future".into();
            future.times_ps = vec![200_000_000];
            future.schedule = None;
            ethernet.generators.push(future);
        } else {
            let mut future = p.can.generators[0].clone();
            future.id = "future".into();
            future.schedule = Schedule::Explicit(vec![200_000_000]);
            p.can.generators.push(future);
        }
        p.common.time_limit_ps = 500_000_000;
        let full = runtime::simulate(&p).unwrap();
        assert!(!full.common.partial);
        for horizon in [
            0,
            1,
            100_000,
            3_000_001,
            103_000_001,
            115_000_001,
            200_000_000,
        ] {
            p.common.time_limit_ps = horizon;
            let part = runtime::simulate(&p).unwrap();
            assert!(!part.common.partial, "{config} H={horizon}");
            let earlier: Vec<Value> = full
                .common
                .points
                .iter()
                .filter(|point| point.time_ps < horizon && point.event_seq.is_some())
                .map(|point| serde_json::to_value(point).unwrap())
                .collect();
            let actual: Vec<Value> = part
                .common
                .points
                .iter()
                .filter(|point| point.event_seq.is_some())
                .map(|point| serde_json::to_value(point).unwrap())
                .collect();
            assert_eq!(actual, earlier, "{config} H={horizon}");
            assert!(part.common.pending_events > 0);
        }
    }
}
