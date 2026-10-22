//! Prepared resource identities used by the existing network dispatch paths.
use crate::{
    prepare,
    types::network::{PreparedNetwork, bindings::*},
};
use std::{path::Path, sync::Arc};

fn network(relative: &str) -> Arc<PreparedNetwork> {
    prepare(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative),
    )
    .unwrap()
    .registered
    .unwrap()
    .network
    .unwrap()
}

#[test]
fn repeater_has_three_application_macs_in_one_six_endpoint_physical_domain() {
    let n = network("examples/network-composite/tenbase-t-repeater/model.ini");
    let bindings = n.bindings.as_ref().unwrap();
    assert_eq!(bindings.ports.len(), n.ethernet.directions.len());
    let (domain, physical) = bindings
        .domains
        .iter()
        .enumerate()
        .find(|(_, p)| matches!(p.owner, PhysicalOwner::Sampled { attachment: 0 }))
        .unwrap();
    assert_eq!(physical.duplex, Duplex::Half);
    assert!(!physical.point_to_point);
    assert_eq!(physical.mac_endpoints, 3);
    assert_eq!(physical.waveform_endpoints, 6);
    assert_eq!(physical.timestamps, TimestampSource::SampledMacMarkers);
    let mut endpoints = bindings
        .ports
        .iter()
        .filter(|p| p.physical_domain == domain)
        .map(|p| p.physical_endpoint.as_deref().unwrap())
        .collect::<Vec<_>>();
    endpoints.sort_unstable();
    assert_eq!(endpoints, ["m0", "m1", "m2"]);
    assert!(!physical.raw_merge_transport);
}

#[test]
fn gateway_frer_prefix_and_merge_branches_keep_distinct_mac_and_physical_owners() {
    let n = network("examples/network-composite/gateway-frer/model.ini");
    let bindings = n.bindings.as_ref().unwrap();
    let port = |name: &str| {
        bindings
            .ports
            .iter()
            .find(|p| n.ethernet.directions[p.direction].from_port == name)
            .unwrap()
    };
    assert_eq!(port("Net.gw.eth.tx").mac, MacOwner::Ordinary);
    for name in ["Net.r.tx_a", "Net.r.tx_b"] {
        let p = port(name);
        assert_eq!(p.mac, MacOwner::Clause99);
        let physical = &bindings.domains[p.physical_domain];
        assert!(physical.point_to_point && physical.raw_merge_transport);
        assert_eq!(physical.duplex, Duplex::Full);
        assert!(p.shaper.is_some());
    }
}

#[test]
fn gptp_clock_writer_is_bound_to_actual_node_and_port() {
    let n = network("examples/gptp/two-node.ini");
    let bindings = n.bindings.as_ref().unwrap();
    for (direction, gp) in n.gptp.as_ref().unwrap().ports.iter().enumerate() {
        let Some(gp) = gp else { continue };
        let p = &bindings.ports[direction];
        assert_eq!(p.clock_node, n.gptp.as_ref().unwrap().nodes[gp.node].device);
        assert_eq!(p.discipline, ClockDiscipline::Gptp);
        assert!(p.protocol_adapters.iter().any(|a| a == "gptp"));
        assert_eq!(bindings.domains[p.physical_domain].duplex, Duplex::Full);
    }
}
