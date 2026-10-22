//! Bounded, typed priority-vector decision for one fixed physical tree.
use super::super::Result;
use super::integration::{gerr, quality};
use super::state::{self, InformationSource, Role};
use crate::types::network::gptp::PreparedGptp;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct PriorityVector {
    identity: [u8; 14],
    steps: u16,
    source_clock: [u8; 8],
    source_port: u16,
    receiving_port: u16,
}

impl PriorityVector {
    fn received(candidate: &state::Candidate, receiving_port: u16) -> Result<Self> {
        Ok(Self {
            identity: candidate.identity,
            steps: candidate
                .steps_removed
                .checked_add(1)
                .ok_or_else(|| gerr("received stepsRemoved overflow"))?,
            source_clock: candidate.source.clock,
            source_port: candidate.source.port,
            receiving_port,
        })
    }
    fn advertised(self, clock: [u8; 8], port: u16) -> Self {
        Self {
            source_clock: clock,
            source_port: port,
            receiving_port: 0,
            ..self
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Change {
    pub node: usize,
    pub old_parent: Option<usize>,
    pub parent: Option<usize>,
    pub grandmaster: [u8; 8],
    pub epoch: u64,
}

/// The only selector used by both live staging and checkpoint causal replay.
pub(super) fn apply(cfg: &PreparedGptp, state: &mut state::State, now: u64) -> Result<Vec<Change>> {
    let mut changes = Vec::new();
    for node in 0..cfg.nodes.len() {
        let own = &cfg.nodes[node];
        let mut best = (own.priority1 != 255).then_some((
            PriorityVector {
                identity: quality(own),
                steps: 0,
                source_clock: own.clock_identity,
                source_port: 0,
                receiving_port: 0,
            },
            None,
        ));
        for (direction, prepared) in cfg.ports.iter().enumerate() {
            let Some(prepared) = prepared.as_ref().filter(|p| p.node == node) else {
                continue;
            };
            let port = state.ports[direction].as_ref().unwrap();
            if !port.link_up || !port.as_capable {
                continue;
            }
            let Some(candidate) = port.candidate.as_ref().filter(|a| a.deadline_ps > now) else {
                continue;
            };
            let vector = PriorityVector::received(candidate, prepared.port_number)?;
            if best.as_ref().is_none_or(|(current, _)| vector < *current) {
                best = Some((vector, Some(direction)));
            }
        }
        let parent = best.as_ref().and_then(|(_, parent)| *parent);
        let grandmaster = parent
            .and_then(|direction| state.ports[direction].as_ref().unwrap().candidate.as_ref())
            .map_or(own.clock_identity, |candidate| candidate.grandmaster);
        let old = state.nodes[node].clone();
        if old.parent != parent || old.grandmaster != grandmaster {
            if state.selections.len() >= cfg.max_journal_entries {
                return Err(gerr("selection journal limit exceeded"));
            }
            let candidate_transfer = parent
                .and_then(|direction| state.ports[direction].as_ref().unwrap().candidate.as_ref())
                .map(|candidate| candidate.transfer);
            let candidate_link_epoch = candidate_transfer
                .and_then(|transfer| state.packets.get(&transfer))
                .map(|packet| packet.link_epoch);
            let selected = &mut state.nodes[node];
            selected.parent = parent;
            selected.grandmaster = grandmaster;
            selected.epoch = selected
                .epoch
                .checked_add(1)
                .ok_or_else(|| gerr("selection epoch overflow"))?;
            selected.synchronized = parent.is_none() && own.priority1 != 255;
            selected.sync_pair = None;
            selected.relay = None;
            selected.last_sync_ps = None;
            selected.sync_deadline_raw = None;
            selected.sync_deadline_ps = None;
            selected.sync_timeout_generation = selected
                .sync_timeout_generation
                .checked_add(1)
                .ok_or_else(|| gerr("Sync timeout generation overflow"))?;
            changes.push(Change {
                node,
                old_parent: old.parent,
                parent,
                grandmaster,
                epoch: selected.epoch,
            });
            state.selections.push(state::Selection {
                node,
                at_ps: now,
                epoch: selected.epoch,
                parent,
                grandmaster,
                candidate_transfer,
                candidate_link_epoch,
            });
        }
        for (direction, prepared) in cfg.ports.iter().enumerate() {
            let Some(prepared) = prepared.as_ref().filter(|p| p.node == node) else {
                continue;
            };
            let current = state.ports[direction].as_ref().unwrap();
            let eligible = current.link_up && current.as_capable;
            let received = current.candidate.as_ref().filter(|a| a.deadline_ps > now);
            let role = if !eligible {
                Role::Disabled
            } else if parent == Some(direction) {
                Role::Slave
            } else if let Some((winner, _)) = best {
                let received_vector = received
                    .map(|candidate| PriorityVector::received(candidate, prepared.port_number))
                    .transpose()?;
                if received_vector.is_none_or(|candidate| {
                    winner.advertised(own.clock_identity, prepared.port_number) < candidate
                }) {
                    Role::Master
                } else {
                    Role::Passive
                }
            } else {
                Role::Passive
            };
            let information_source = if !eligible {
                InformationSource::Disabled
            } else if role == Role::Master {
                InformationSource::Mine
            } else if received.is_some() {
                InformationSource::Received
            } else {
                InformationSource::Aged
            };
            let port = state.ports[direction].as_mut().unwrap();
            port.role = role;
            port.information_source = information_source;
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::super::codec::PortIdentity;
    use super::super::math::Rat;
    use super::*;
    use std::path::Path;

    fn fixture() -> PreparedGptp {
        let prepared = crate::prepare(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gptp/three-node.ini"),
        )
        .unwrap();
        prepared
            .registered
            .as_ref()
            .unwrap()
            .network
            .as_ref()
            .unwrap()
            .gptp
            .as_ref()
            .unwrap()
            .clone()
    }
    fn candidate(identity: [u8; 14], clock: [u8; 8], transfer: usize) -> state::Candidate {
        state::Candidate {
            source: PortIdentity { clock, port: 1 },
            grandmaster: clock,
            identity,
            steps_removed: 0,
            path: vec![clock],
            sequence: 1,
            transfer,
            receipt_ps: 1,
            deadline_raw: Rat::new(1_000_000, 1).unwrap(),
            deadline_ps: 1_000_000,
            generation: 1,
            time_source: 0xa0,
        }
    }
    #[test]
    fn competing_vectors_choose_the_same_root_in_both_receive_orders() {
        let cfg = fixture();
        let b = cfg
            .nodes
            .iter()
            .position(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 2])
            .unwrap();
        let ports = cfg
            .ports
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().filter(|p| p.node == b).map(|_| i))
            .collect::<Vec<_>>();
        assert_eq!(ports.len(), 2);
        let better = cfg
            .nodes
            .iter()
            .find(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 1])
            .unwrap();
        let worse = cfg
            .nodes
            .iter()
            .find(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 3])
            .unwrap();
        for order in [false, true] {
            let mut s = state::State::new(&cfg);
            for i in &ports {
                s.ports[*i].as_mut().unwrap().as_capable = true;
            }
            let assignments = if order {
                [(ports[1], worse), (ports[0], better)]
            } else {
                [(ports[0], better), (ports[1], worse)]
            };
            for (i, n) in assignments {
                s.ports[i].as_mut().unwrap().candidate =
                    Some(candidate(quality(n), n.clock_identity, i));
                apply(&cfg, &mut s, 2).unwrap();
            }
            assert_eq!(s.nodes[b].parent, Some(ports[0]));
            assert_eq!(s.nodes[b].grandmaster, better.clock_identity);
            assert_eq!(s.ports[ports[0]].as_ref().unwrap().role, Role::Slave);
            // Repeated source information refresh is allowed to deteriorate.
            let mut poorer_identity = quality(worse);
            poorer_identity[1] = 249;
            let mut deteriorated = candidate(poorer_identity, better.clock_identity, ports[0]);
            deteriorated.grandmaster = worse.clock_identity;
            s.ports[ports[0]].as_mut().unwrap().candidate = Some(deteriorated);
            apply(&cfg, &mut s, 3).unwrap();
            assert_eq!(s.nodes[b].parent, Some(ports[1]));
        }
    }
    #[test]
    fn equal_root_vector_uses_source_and_receiving_port_and_marks_passive() {
        let cfg = fixture();
        let b = cfg
            .nodes
            .iter()
            .position(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 2])
            .unwrap();
        let ports = cfg
            .ports
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().filter(|p| p.node == b).map(|_| i))
            .collect::<Vec<_>>();
        let root = cfg
            .nodes
            .iter()
            .find(|n| n.clock_identity == [0, 0, 0, 0, 0, 0, 0, 1])
            .unwrap();
        let (first, second) = if cfg.ports[ports[0]].as_ref().unwrap().port_number
            < cfg.ports[ports[1]].as_ref().unwrap().port_number
        {
            (ports[0], ports[1])
        } else {
            (ports[1], ports[0])
        };
        let mut s = state::State::new(&cfg);
        for i in ports {
            let p = s.ports[i].as_mut().unwrap();
            p.as_capable = true;
            p.candidate = Some(candidate(quality(root), root.clock_identity, i));
        }
        apply(&cfg, &mut s, 2).unwrap();
        assert_eq!(s.nodes[b].parent, Some(first));
        assert_eq!(s.ports[second].as_ref().unwrap().role, Role::Passive);
        assert_eq!(
            s.ports[second].as_ref().unwrap().information_source,
            InformationSource::Received
        );
    }
}
