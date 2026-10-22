//! Independent observable expectations for actual flit/VC/credit execution.
use dir_simulator::output::noc_flit::flit_records;
use dir_simulator::registry::Registry;
use dir_simulator::runtime::{self, session::*};
use dir_simulator::{prepare, types::noc_flit::*};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(config: Value, packets: Vec<Value>, faults: Vec<Value>, horizon: u64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "dir-noc-flit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("ned/noc")).unwrap();
        std::fs::write(root.join("ned/noc/model.ned"),"package noc; simple Flit { parameters: @class(\"dir.noc.Flit\"); } network Demo { submodules: noc: noc.Flit; }").unwrap();
        std::fs::write(
            root.join("model.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("workload.json"),serde_json::to_vec(&json!({"schema":"dir.noc-flit.workload","version":1,"packets":packets,"faults":faults})).unwrap()).unwrap();
        std::fs::write(root.join("scenario.ini"),format!("[General]\nnetwork=noc.Demo\nned-path=\"ned\"\nmodel-profile=\"noc.flit.v1\"\nmodel-config=\"model.json\"\nworkload=\"workload.json\"\nsim-time-limit={horizon}ps\nmetrics-window=1ps\n")).unwrap();
        Self(root)
    }
    fn prepared(&self) -> dir_simulator::PreparedSimulation {
        prepare(&self.0.join("scenario.ini")).unwrap()
    }
    fn run(&self) -> dir_simulator::snapshot::Snapshot {
        runtime::simulate(&self.prepared()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn config(kind: &str, width: usize, height: usize) -> Value {
    json!({"schema":"dir.noc-flit.model","version":1,"topology":{"kind":kind,"width":width,"height":height},"flit_bytes":2,"buffer_flits":1,"adaptive_vcs":1,"cycle_ps":1,"link_latency_cycles":2,"credit_latency_cycles":3,"escape_after_stalls":2,"ejection_capacity_flits":1,"ejection_service_cycles":2,"pending_packets":8})
}
fn packet(id: &str, source: usize, destination: usize, at: u64, payload: &[u8]) -> Value {
    json!({"id":id,"source":source,"destination":destination,"at_ps":at,"payload":payload})
}
fn fault(id: &str, link: usize, at: u64, up: bool) -> Value {
    json!({"id":id,"link_id":link,"at_ps":at,"up":up})
}
fn events(s: &dir_simulator::snapshot::Snapshot) -> Vec<(u64, Value)> {
    flit_records(s.registered.as_ref().unwrap())
        .into_iter()
        .map(|r| (r.time_ps, r.data.clone()))
        .collect()
}
fn kind<'a>(e: &'a [(u64, Value)], k: &str) -> Vec<&'a (u64, Value)> {
    e.iter().filter(|(_, v)| v["kind"] == k).collect()
}
fn for_packet<'a>(e: &'a [(u64, Value)], p: &str, k: &str) -> Vec<&'a (u64, Value)> {
    kind(e, k)
        .into_iter()
        .filter(|(_, v)| v["packet_id"] == p)
        .collect()
}
#[test]
fn finite_credit_delay_tail_ownership_and_exact_payload() {
    let f = Fixture::new(
        config("mesh", 2, 1),
        vec![
            packet("a", 0, 1, 0, &[1, 2, 3, 4, 5]),
            packet("b", 0, 1, 0, &[8, 9]),
        ],
        vec![],
        100,
    );
    let s = f.run();
    assert_eq!(s.common.termination, "events_exhausted");
    let e = events(&s);
    let launches = for_packet(&e, "a", "flit_launched");
    assert_eq!(launches.len(), 3);
    assert_eq!(launches[1].0, launches[0].0 + 5); // Two transit cycles, destination pop, then three return cycles.
    assert!(
        kind(&e, "credit_returned")
            .iter()
            .any(|(t, v)| *t == launches[1].0
                && v["detail"]["destination_vc"] == launches[1].1["detail"]["destination_vc"])
    );
    let target = launches[0].1["detail"]["destination_vc"].clone();
    assert!(
        launches
            .iter()
            .all(|(_, v)| v["detail"]["destination_vc"] == target)
    );
    let last_pop = for_packet(&e, "a", "vc_released")
        .into_iter()
        .find(|(_, v)| v["detail"]["source_vc"] == target)
        .unwrap()
        .0;
    // A later head may use the dedicated escape VC, but cannot steal a's owned VC.
    assert!(
        for_packet(&e, "b", "vc_allocated")
            .iter()
            .all(|(t, v)| v["detail"]["destination_vc"] != target || *t >= last_pop)
    );
    assert_eq!(
        for_packet(&e, "a", "packet_delivered")[0].1["detail"]["payload"],
        json!([1, 2, 3, 4, 5])
    );
    assert_eq!(for_packet(&e, "b", "packet_delivered").len(), 1);
    assert_eq!(
        for_packet(&e, "a", "flit_ejected").last().unwrap().1["detail"]["valid_bytes"],
        1
    );
}
#[test]
fn concurrent_links_shared_link_fairness_and_control_flit() {
    let f = Fixture::new(
        config("mesh", 3, 1),
        vec![
            packet("east", 0, 1, 0, &[1, 2, 3, 4]),
            packet("west", 2, 1, 0, &[]),
            packet("later", 0, 2, 0, &[5]),
        ],
        vec![],
        150,
    );
    let e = events(&f.run());
    let launches = kind(&e, "flit_launched");
    assert_eq!(
        for_packet(&e, "east", "flit_launched")[0].0,
        for_packet(&e, "west", "flit_launched")[0].0
    );
    let mut seen = std::collections::BTreeSet::new();
    for (at, v) in launches {
        assert!(seen.insert((*at, v["detail"]["link_id"].as_u64().unwrap())));
    }
    assert_eq!(kind(&e, "packet_delivered").len(), 3);
    assert_eq!(for_packet(&e, "west", "flit_ejected").len(), 1);
    assert_eq!(
        for_packet(&e, "west", "packet_delivered")[0].1["detail"]["payload"],
        json!([])
    );
}
#[test]
fn adaptive_stalls_enter_permanent_escape_and_drain() {
    let mut c = config("torus", 3, 3);
    c["ejection_service_cycles"] = json!(5);
    c["pending_packets"] = json!(64);
    let packets = (0..9)
        .flat_map(|src| {
            (0..4).map(move |i| {
                packet(
                    &format!("p{src}-{i}"),
                    src,
                    (src + 4) % 9,
                    0,
                    &[1, 2, 3, 4, 5, 6],
                )
            })
        })
        .collect();
    let f = Fixture::new(c, packets, vec![], 3000);
    let s = f.run();
    let e = events(&s);
    assert_eq!(kind(&e, "packet_delivered").len(), 36);
    assert!(!kind(&e, "escape_entered").is_empty());
    for (_, entry) in kind(&e, "escape_entered") {
        let p = entry["packet_id"].as_str().unwrap();
        let mut entered = false;
        for (_, v) in for_packet(&e, p, "flit_launched")
            .into_iter()
            .filter(|(_, v)| v["flit_index"] == 0)
        {
            if v["detail"]["escape"] == true {
                entered = true;
            }
            if entered {
                assert_eq!(v["detail"]["escape"], true);
            }
        }
    }
}
#[test]
fn independent_escape_channel_dependency_graph_is_acyclic() {
    use dir_simulator::runtime::noc_flit::topology::Topology;
    for (kind, w, h) in [
        (TopologyKind::Mesh, 4, 3),
        (TopologyKind::Torus, 2, 2),
        (TopologyKind::Torus, 3, 4),
        (TopologyKind::Mesh, 1, 4),
    ] {
        let t = Topology::new(&TopologyConfig {
            kind,
            width: w,
            height: h,
        });
        let r = t.routing(&vec![true; t.links.len()]);
        let mut graph = vec![std::collections::BTreeSet::new(); t.links.len()];
        for src in 0..t.nodes {
            for dst in 0..t.nodes {
                let mut at = src;
                let mut prev: Option<usize> = None;
                let mut steps = 0;
                while at != dst {
                    let l = t.escape_next(&r, at, dst).unwrap();
                    assert!(r.tree_links[l]);
                    if let Some(p) = prev {
                        graph[p].insert(l);
                    }
                    prev = Some(l);
                    at = t.links[l].destination;
                    steps += 1;
                    assert!(steps < t.nodes);
                }
            }
        }
        fn visit(n: usize, g: &[std::collections::BTreeSet<usize>], color: &mut [u8]) {
            assert_ne!(color[n], 1, "escape dependency cycle");
            if color[n] == 2 {
                return;
            }
            color[n] = 1;
            for next in &g[n] {
                visit(*next, g, color);
            }
            color[n] = 2;
        }
        let mut color = vec![0; graph.len()];
        for i in 0..graph.len() {
            visit(i, &graph, &mut color);
        }
    }
}
#[test]
fn torus_wrap_parallel_ports_and_tree_detours() {
    let torus = Fixture::new(
        config("torus", 4, 2),
        vec![packet("wrap", 0, 3, 0, &[7])],
        vec![],
        100,
    );
    let mesh = Fixture::new(
        config("mesh", 4, 2),
        vec![packet("wrap", 0, 3, 0, &[7])],
        vec![],
        100,
    );
    assert_eq!(
        for_packet(&events(&torus.run()), "wrap", "flit_launched").len(),
        1
    );
    assert_eq!(
        for_packet(&events(&mesh.run()), "wrap", "flit_launched").len(),
        3
    );
    use dir_simulator::runtime::noc_flit::topology::Topology;
    let t = Topology::new(&TopologyConfig {
        kind: TopologyKind::Torus,
        width: 2,
        height: 2,
    });
    assert_eq!(t.links.len(), 16);
    assert_eq!(t.outgoing[0].len(), 4);
    assert_eq!(
        t.links[t.outgoing[0][0]].destination,
        t.links[t.outgoing[0][1]].destination
    );
    assert_ne!(
        t.links[t.outgoing[0][0]].reverse,
        t.links[t.outgoing[0][1]].reverse
    );
    let t = Topology::new(&TopologyConfig {
        kind: TopologyKind::Torus,
        width: 4,
        height: 3,
    });
    let r = t.routing(&vec![true; t.links.len()]);
    assert!((0..t.nodes).any(|src| (0..t.nodes).any(|dst| {
        let mut at = src;
        let mut hops = 0;
        while at != dst {
            let l = t.escape_next(&r, at, dst).unwrap();
            assert!(r.tree_links[l]);
            at = t.links[l].destination;
            hops += 1;
        }
        hops > r.distances[dst][src].unwrap()
    })));
}
#[test]
fn fault_at_arrival_aborts_once_reroutes_pending_partition_and_recovery() {
    let mut c = config("mesh", 2, 2);
    c["link_latency_cycles"] = json!(4);
    let f = Fixture::new(
        c,
        vec![
            packet("cut", 0, 1, 0, &[1, 2, 3, 4]),
            packet("pending", 0, 1, 0, &[99]),
            packet("around", 0, 1, 5, &[6]),
            packet("unaffected", 2, 3, 29, &[10, 11, 12, 13]),
            packet("partition", 0, 1, 31, &[7]),
            packet("recovered", 0, 1, 42, &[8]),
        ],
        vec![
            fault("break-east", 0, 4, false),
            fault("isolate-south", 1, 30, false),
            fault("recover-east", 0, 40, true),
        ],
        100,
    );
    let e = events(&f.run());
    assert_eq!(for_packet(&e, "cut", "topology_epoch_aborted").len(), 1);
    assert!(for_packet(&e, "cut", "flit_arrived").is_empty());
    assert!(for_packet(&e, "cut", "packet_delivered").is_empty());
    // A second epoch deliberately aborts all injected traffic, including an unaffected path.
    assert_eq!(
        for_packet(&e, "unaffected", "topology_epoch_aborted").len(),
        1
    );
    assert_eq!(for_packet(&e, "around", "packet_delivered").len(), 1);
    assert_eq!(for_packet(&e, "pending", "flit_injected")[0].0, 4);
    assert_eq!(for_packet(&e, "pending", "packet_delivered").len(), 1);
    assert!(
        for_packet(&e, "around", "flit_launched")
            .iter()
            .all(|(_, v)| v["detail"]["link_id"] != 0)
    );
    assert_eq!(for_packet(&e, "partition", "unreachable").len(), 1);
    assert_eq!(for_packet(&e, "recovered", "packet_delivered").len(), 1);
    assert_eq!(kind(&e, "topology_changed").len(), 3);
}
#[test]
fn finite_pending_admission_local_router_empty_and_future_fault() {
    let mut c = config("mesh", 1, 1);
    c["pending_packets"] = json!(0);
    let f = Fixture::new(
        c,
        vec![packet("a", 0, 0, 0, &[]), packet("b", 0, 0, 0, &[9])],
        vec![],
        100,
    );
    let e = events(&f.run());
    assert_eq!(kind(&e, "packet_delivered").len(), 1);
    assert_eq!(kind(&e, "injection_rejected").len(), 1);
    assert!(kind(&e, "flit_launched").is_empty());
    let f = Fixture::new(config("mesh", 1, 1), vec![], vec![], 100);
    assert_eq!(f.run().common.termination, "events_exhausted");
    let f = Fixture::new(
        config("mesh", 2, 1),
        vec![],
        vec![fault("future", 0, 30, false)],
        100,
    );
    let e = events(&f.run());
    assert_eq!(kind(&e, "topology_changed")[0].0, 30);
}
#[test]
fn full_checkpoint_resume_at_transit_credit_ejection_and_fault_boundaries() {
    let f = Fixture::new(
        config("mesh", 2, 2),
        vec![
            packet("a", 0, 3, 0, &[1, 2, 3, 4, 5]),
            packet("b", 1, 2, 0, &[]),
            packet("after", 0, 3, 30, &[8, 9]),
        ],
        vec![fault("fault", 0, 15, false), fault("up", 0, 25, true)],
        100,
    );
    let baseline = f.run();
    let expected = events(&baseline);
    for steps in [1, 2, 3, 5, 8, 12, 16, 20, 24, 28, 32] {
        let (mut s, control) = RunSession::new(
            f.prepared(),
            RunOptions {
                checkpoint_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..steps {
            if s.state() == RunState::Completed {
                break;
            }
            s.advance_one().unwrap();
        }
        if s.state() == RunState::Completed {
            continue;
        }
        control.submit(ControlCommand::Pause).unwrap();
        assert_eq!(s.poll_control().unwrap(), RunState::Paused);
        let p = f.0.join(format!("checkpoint-{steps}"));
        assert_eq!(
            s.checkpoint(&p, CheckpointLimits::default())
                .unwrap()
                .status,
            ControlStatus::Applied
        );
        let (mut resumed, token) =
            restore_checkpoint(&p, Registry::default(), CheckpointLimits::default()).unwrap();
        assert_eq!(events(resumed.snapshot()), events(s.snapshot()));
        resumed.resume(token).unwrap();
        assert_eq!(resumed.run_until_yield().unwrap(), RunState::Completed);
        assert_eq!(events(resumed.snapshot()), expected);
    }
    let partial = Fixture::new(
        config("mesh", 2, 1),
        vec![packet("cut", 0, 1, 0, &[1, 2, 3, 4])],
        vec![],
        1,
    );
    let e = events(&partial.run());
    assert!(kind(&e, "packet_delivered").is_empty());
    assert_eq!(kind(&e, "flit_launched").len(), 1);
}
#[test]
fn checkpoint_rejects_a_validly_reserved_tick_at_the_wrong_time() {
    use dir_simulator::runtime::checkpoint_bundle;
    let fixture = Fixture::new(
        config("mesh", 2, 1),
        vec![packet("a", 0, 1, 0, &[1, 2, 3, 4])],
        vec![],
        100,
    );
    let (mut session, control) = RunSession::new(
        fixture.prepared(),
        RunOptions {
            checkpoint_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    session.advance_one().unwrap();
    session.advance_one().unwrap();
    control.submit(ControlCommand::Pause).unwrap();
    session.poll_control().unwrap();
    let limits = CheckpointLimits::default();
    let source = fixture.0.join("valid-tick");
    assert_eq!(
        session.checkpoint(&source, limits.clone()).unwrap().status,
        ControlStatus::Applied
    );
    let (manifest, mut files) = checkpoint_bundle::read_bundle(&source, &limits).unwrap();
    let mut state: Value = serde_json::from_slice(&files["state.json"]).unwrap();
    let row = &mut state["fes"][0];
    let at = row["time_ps"].as_str().unwrap().parse::<u64>().unwrap() + 1;
    row["time_ps"] = json!(at.to_string());
    row["work"]["envelope"]["time_ps"] = json!(at.to_string());
    files.insert("state.json".into(), serde_json::to_vec(&state).unwrap());
    let changed = fixture.0.join("wrong-tick");
    checkpoint_bundle::write_bundle(&changed, manifest, files, &limits, 0).unwrap();
    let error = restore_checkpoint(&changed, Registry::default(), limits)
        .err()
        .expect("wrong tick must not restore");
    assert!(error.message.contains("tick envelope mismatch"), "{error}");
}
#[test]
fn strict_invalid_inputs_and_order_independence() {
    for mut c in [
        config("mesh", 0, 1),
        config("torus", 1, 2),
        config("mesh", 2, 1),
    ] {
        if c["topology"]["width"] == 2 {
            c["cycle_ps"] = json!(0);
        }
        let f = Fixture::new(c, vec![], vec![], 10);
        assert!(prepare(&f.0.join("scenario.ini")).is_err());
    }
    let f = Fixture::new(
        config("mesh", 2, 1),
        vec![],
        vec![fault("a", 0, 1, false), fault("b", 0, 1, true)],
        10,
    );
    assert!(prepare(&f.0.join("scenario.ini")).is_err());
    let a = packet("a", 0, 1, 0, &[1]);
    let b = packet("b", 0, 1, 0, &[2]);
    let f = Fixture::new(
        config("mesh", 2, 1),
        vec![a.clone(), b.clone()],
        vec![],
        100,
    );
    let g = Fixture::new(config("mesh", 2, 1), vec![b, a], vec![], 100);
    assert_eq!(events(&f.run()), events(&g.run()));
}
