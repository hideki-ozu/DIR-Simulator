//! Network observations enter the existing transactional Metric V2 owner.
use dir_simulator::runtime::network_metrics::{self, DELIVERY_DELAY, QUEUE_BYTES, QUEUE_FRAMES};
use dir_simulator::{
    CheckpointLimits, ControlCommand, ControlStatus, RunOptions, RunSession, RunState, prepare,
    restore_checkpoint, runtime,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
const CONFIGS: [&str; 7] = [
    "examples/ethernet/dynamic/unicast.ini",
    "examples/ethernet/tsn/tas.ini",
    "examples/can-ethernet/r01.ini",
    "examples/network-extensions/composite.ini",
    "examples/network-extensions/redundancy.ini",
    "examples/network-extensions/preemption.ini",
    "examples/ethernet-wire/active.ini",
];
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dir-network-metric-v2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn config(&self, index: usize, horizon: Option<u64>, version: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(CONFIGS[index])
            .canonicalize()
            .unwrap();
        let mut text = fs::read_to_string(&source).unwrap();
        for key in ["ned-path", "model-config", "workload"] {
            let line = text
                .lines()
                .find(|line| line.starts_with(key))
                .unwrap()
                .to_string();
            let value = line.split_once('=').unwrap().1.trim().trim_matches('"');
            let absolute = source.parent().unwrap().join(value).canonicalize().unwrap();
            text = text.replace(&line, &format!("{key} = \"{}\"", absolute.display()));
        }
        if let Some(horizon) = horizon {
            let line = text
                .lines()
                .find(|line| line.starts_with("sim-time-limit"))
                .unwrap()
                .to_string();
            text = text.replace(&line, &format!("sim-time-limit = {horizon}ps"));
        }
        text.push_str(&format!("\nmetrics-version = {version}\n"));
        let path = self.0.join(format!("case-{index}-{version}.ini"));
        fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn all_network_profiles_commit_exact_v1_observations_into_v2() {
    for (index, config) in CONFIGS.iter().enumerate() {
        let temp = Temp::new();
        let path = temp.config(
            index,
            Some(if index == 6 {
                1_000_000_000
            } else {
                120_000_000
            }),
            "2",
        );
        let p = prepare(&path).unwrap();
        let registry = &p.registered.as_ref().unwrap().registry;
        assert!(
            registry
                .execution_capabilities(&p.common.profile)
                .metrics_v2
        );
        assert_eq!(
            registry
                .profile(&p.common.profile)
                .unwrap()
                .output_schema_version,
            3
        );
        let snapshot = runtime::simulate(&p).unwrap();
        assert!(
            !snapshot.common.partial,
            "{}: {:?}",
            config, snapshot.common.diagnostics
        );
        let metrics = &snapshot.registered.as_ref().unwrap().metrics_v2;
        assert!(!metrics.records.is_empty(), "{}", config);
        for accumulator in &metrics.accumulators {
            let v1 = match accumulator.metric.as_str() {
                QUEUE_FRAMES => "queue_length",
                QUEUE_BYTES => "queue_bytes",
                DELIVERY_DELAY => "ethernet.delivery_ps",
                _ => panic!("unknown metric"),
            };
            let reference = snapshot
                .common
                .points
                .iter()
                .filter(|point| point.target == accumulator.target && point.metric == v1)
                .collect::<Vec<_>>();
            let records = metrics
                .records
                .iter()
                .filter(|point| {
                    point.target == accumulator.target && point.metric == accumulator.metric
                })
                .collect::<Vec<_>>();
            assert_eq!(
                records.len(),
                reference.len(),
                "{} {} {}",
                config,
                accumulator.metric,
                accumulator.target
            );
            assert!(
                records
                    .iter()
                    .zip(&reference)
                    .all(|(a, b)| a.time_ps == b.time_ps && a.value == b.value)
            );
            if v1.starts_with("queue_") {
                let (mut last, mut value, mut area) = (0, 0, 0u128);
                for point in reference {
                    area += u128::from(point.time_ps - last) * u128::from(value);
                    last = point.time_ps;
                    value = point.value;
                }
                area += u128::from(snapshot.common.end_ps - last) * u128::from(value);
                let actual = accumulator.total.area
                    + u128::from(snapshot.common.end_ps - accumulator.last_time)
                        * u128::from(accumulator.last_value);
                assert_eq!(
                    actual, area,
                    "{} {} {}",
                    config, accumulator.metric, accumulator.target
                );
            }
        }
    }
}

#[test]
fn zero_horizon_retains_initial_gauges_without_points_and_defaults_remain_v1() {
    let temp = Temp::new();
    let p = prepare(&temp.config(0, Some(0), "2")).unwrap();
    let snapshot = runtime::simulate(&p).unwrap();
    let metrics = &snapshot.registered.as_ref().unwrap().metrics_v2;
    assert!(!metrics.accumulators.is_empty());
    assert!(metrics.records.is_empty());
    assert!(snapshot.common.points.is_empty());
    assert!(
        metrics
            .accumulators
            .iter()
            .all(|a| a.last_value == 0 && a.total.area == 0)
    );
    let p = prepare(&temp.config(0, None, "1")).unwrap();
    assert_eq!(
        p.registered
            .as_ref()
            .unwrap()
            .registry
            .profile(&p.common.profile)
            .unwrap()
            .output_schema_version,
        2
    );
    for bad in ["0", "3", "\"2\"", "02"] {
        assert!(prepare(&temp.config(0, None, bad)).is_err());
    }
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/canfd/iso.ini");
    let mut p = prepare(&original).unwrap();
    let profile = p.common.profile.clone();
    assert!(network_metrics::enable(&mut p).is_err());
    assert_eq!(p.common.profile, profile);
}

#[test]
fn paused_export_and_checkpoint_preserve_exact_gauge_integrals_without_double_observation() {
    let temp = Temp::new();
    let p = prepare(&temp.config(1, None, "2")).unwrap();
    let registry = (*p.registered.as_ref().unwrap().registry).clone();
    let (mut session, controller) = RunSession::new(
        p,
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    for _ in 0..8 {
        assert_eq!(session.advance_one().unwrap(), RunState::Running);
    }
    controller.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let before = session
        .snapshot()
        .registered
        .as_ref()
        .unwrap()
        .metrics_v2
        .clone();
    for name in ["export-a", "export-b"] {
        assert_eq!(
            session.export(&temp.0.join(name)).unwrap().status,
            ControlStatus::Applied
        );
    }
    assert_eq!(
        before,
        session.snapshot().registered.as_ref().unwrap().metrics_v2
    );
    let a: Value =
        serde_json::from_slice(&fs::read(temp.0.join("export-a/results.json")).unwrap()).unwrap();
    let b: Value =
        serde_json::from_slice(&fs::read(temp.0.join("export-b/results.json")).unwrap()).unwrap();
    assert_eq!(a["simulation"], b["simulation"]);
    assert_eq!(
        session
            .checkpoint(&temp.0.join("bundle"), CheckpointLimits::default())
            .unwrap()
            .status,
        ControlStatus::Applied
    );
    let (mut restored, token) = restore_checkpoint(
        &temp.0.join("bundle"),
        registry,
        CheckpointLimits::default(),
    )
    .unwrap();
    restored.resume(token).unwrap();
    session.resume(session.pause_token().unwrap()).unwrap();
    assert_eq!(session.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(restored.run_until_yield().unwrap(), RunState::Completed);
    assert_eq!(
        session.snapshot().registered.as_ref().unwrap().metrics_v2,
        restored.snapshot().registered.as_ref().unwrap().metrics_v2
    );
}

#[test]
fn bounded_catalog_and_observation_failures_preserve_the_committed_prefix() {
    use dir_simulator::registry::MetricV2Limits;
    use std::sync::Arc;
    let temp = Temp::new();
    let mut p = prepare(&temp.config(0, None, "1")).unwrap();
    Arc::make_mut(&mut p.registered.as_mut().unwrap().registry)
        .set_metric_v2_limits(MetricV2Limits {
            max_metric_targets: 1,
            ..Default::default()
        })
        .unwrap();
    assert!(network_metrics::enable(&mut p).is_err());
    assert_eq!(
        p.registered
            .as_ref()
            .unwrap()
            .registry
            .profile(&p.common.profile)
            .unwrap()
            .output_schema_version,
        2
    );
    let mut p = prepare(&temp.config(0, None, "2")).unwrap();
    Arc::make_mut(&mut p.registered.as_mut().unwrap().registry)
        .set_metric_v2_limits(MetricV2Limits {
            max_observations_per_batch: 1,
            ..Default::default()
        })
        .unwrap();
    let snapshot = runtime::simulate(&p).unwrap();
    assert!(snapshot.common.partial);
    assert_eq!(snapshot.common.committed_events, 0);
    assert!(snapshot.common.points.is_empty());
    assert!(
        snapshot
            .registered
            .as_ref()
            .unwrap()
            .metrics_v2
            .records
            .is_empty()
    );
}
