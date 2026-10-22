//! Explicit physical bridge-pair forwarding, owned by the network transaction.
use super::replication::{
    ContinuationRound, DistributedCursor, DistributedFlow, DistributedLeg, DistributedPhaseState,
    DistributedRecovery, SequenceAllocation, identifier,
};
use super::*;
use crate::types::network::{
    PreparedBridgePair, ReplicationPlacement, ReplicationRecoveryAlgorithm,
};

impl NetworkState {
    /// Receiver-local filtering must not retire the shared transmitter's
    /// circuit when another actual receiving MAC is the selected logical hop.
    pub(super) fn replication_receiver_is_intended(
        &self,
        transfer: &Transfer,
        receiver: Option<usize>,
        d: &NetworkDelta,
    ) -> Result<bool> {
        let Some(cursor) = &transfer.circuit else {
            return Ok(true);
        };
        let state = d
            .replication
            .as_ref()
            .or(self.replication.as_ref())
            .ok_or_else(|| error("replication receiver state"))?;
        let config = self
            .prepared
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .ok_or_else(|| error("replication receiver configuration"))?;
        let intended = if let Some(dc) = &cursor.distributed {
            let flow = state
                .distributed
                .get(&dc.flow)
                .ok_or_else(|| error("distributed receiver flow"))?;
            let stream = &config.streams[flow.stream];
            match &stream.placement {
                ReplicationPlacement::BridgePair(pair) => match dc.leg {
                    DistributedLeg::Prefix => pair.prefix.endpoints.get(cursor.hop),
                    DistributedLeg::Branch { path } => stream.paths[path].endpoints.get(cursor.hop),
                    DistributedLeg::Suffix | DistributedLeg::SuffixRound { .. } => {
                        pair.suffix.endpoints.get(cursor.hop)
                    }
                },
                _ => return Err(error("distributed receiver placement")),
            }
        } else {
            let branch = state
                .branches
                .get(&cursor.branch)
                .ok_or_else(|| error("endpoint receiver branch"))?;
            config.streams[branch.stream].paths[branch.path]
                .endpoints
                .get(cursor.hop)
        }
        .map(|(_, destination)| *destination)
        .ok_or_else(|| error("replication receiver hop"))?;
        Ok(self
            .effective_link(transfer.direction, receiver)?
            .destination
            == intended)
    }

    /// Only the selected source-route adapter may bypass tree selection/learning.
    pub(super) fn explicit_replication_route(
        &self,
        cursor: Option<&Circuit>,
        d: &NetworkDelta,
    ) -> bool {
        let Some(cursor) = cursor else {
            return false;
        };
        let Some(state) = d.replication.as_ref().or(self.replication.as_ref()) else {
            return false;
        };
        let Some(cfg) = self
            .prepared
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
        else {
            return false;
        };
        if let Some(dc) = &cursor.distributed {
            state
                .distributed
                .get(&dc.flow)
                .and_then(|f| cfg.streams.get(f.stream))
                .is_some_and(|s| matches!(s.placement, ReplicationPlacement::BridgePair(_)))
        } else {
            state
                .branches
                .get(&cursor.branch)
                .and_then(|b| cfg.streams.get(b.stream))
                .is_some_and(|s| matches!(s.placement, ReplicationPlacement::LegacyEndpoint))
        }
    }
    fn distributed_config(&self, stream: usize) -> Result<&PreparedBridgePair> {
        match &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[stream]
            .placement
        {
            ReplicationPlacement::BridgePair(p) => Ok(p),
            _ => Err(error("distributed cursor on endpoint stream")),
        }
    }
    pub(super) fn distributed_route(
        &self,
        stream: usize,
        leg: &DistributedLeg,
    ) -> Result<&[usize]> {
        let pair = self.distributed_config(stream)?;
        Ok(match leg {
            DistributedLeg::Prefix => &pair.prefix.directions,
            DistributedLeg::Suffix | DistributedLeg::SuffixRound { .. } => &pair.suffix.directions,
            DistributedLeg::Branch { path } => {
                &self
                    .prepared
                    .extensions
                    .as_ref()
                    .unwrap()
                    .replication
                    .as_ref()
                    .unwrap()
                    .streams[stream]
                    .paths
                    .get(*path)
                    .ok_or_else(|| error("distributed branch index"))?
                    .directions
            }
        })
    }
    pub(super) fn distributed_source_offer(
        &self,
        frame: usize,
        stream: usize,
        class: l2::Classification,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let row = self.frame(d, frame).row.clone();
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[stream];
        let id = identifier(&[
            "ethernet.dynamic.frame",
            &row.frame_id,
            &config.stream_id,
            "bridge_pair_v1",
        ]);
        self.replication_mut(d)?.distributed.insert(
            id.clone(),
            DistributedFlow {
                frame,
                stream,
                created_ps: c.now(),
                sequence: None,
                prefix_reception: None,
                winner_reception: None,
                continued: false,
                suffix_root: None,
                delivery_reception: None,
                terminal_ps: None,
                dropped: false,
                prefix: DistributedPhaseState::default(),
                branches: Vec::new(),
                suffix: DistributedPhaseState::default(),
                recoveries: Vec::new(),
                rounds: Vec::new(),
            },
        );
        self.offer(
            CopyOrigin {
                circuit: Some(Circuit {
                    branch: id.clone(),
                    hop: 0,
                    distributed: Some(DistributedCursor {
                        flow: id.clone(),
                        leg: DistributedLeg::Prefix,
                    }),
                }),
                frame,
                parent: None,
                visit: 0,
            },
            &row.wire,
            class,
            &[self.distributed_route(stream, &DistributedLeg::Prefix)?[0]],
            d,
            c,
        )?;
        self.distributed_publish(&id, d)
    }
    fn distributed_publish(&self, id: &str, d: &mut NetworkDelta) -> Result<()> {
        let stream = self.replication_mut(d)?.distributed[id].stream;
        let normative = self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[stream]
            .recovery_algorithm
            == ReplicationRecoveryAlgorithm::Vector8021cb2017;
        if normative {
            let live = self.replication_mut(d)?.distributed.get_mut(id).unwrap();
            let all_done = live.prefix.terminal_ps.is_some()
                && (live.prefix.failure_reason.is_some()
                    || (!live.branches.is_empty()
                        && live.branches.iter().all(|p| p.terminal_ps.is_some())
                        && live.rounds.iter().all(|r| r.suffix.terminal_ps.is_some())));
            live.terminal_ps = if all_done {
                live.prefix
                    .terminal_ps
                    .into_iter()
                    .chain(live.branches.iter().filter_map(|p| p.terminal_ps))
                    .chain(live.rounds.iter().filter_map(|r| r.suffix.terminal_ps))
                    .max()
            } else {
                None
            };
            live.dropped = all_done && !live.rounds.iter().any(|r| r.delivery_reception.is_some());
        }
        let f = self.replication_mut(d)?.distributed[id].clone();
        let config = &self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap()
            .streams[f.stream];
        let pair = self.distributed_config(f.stream)?;
        let frame_id = self.frame(d, f.frame).row.frame_id.clone();
        let reception_id =
            |ri: Option<usize>| ri.map(|i| self.reception(d, i).row.reception_id.clone());
        let prefix_reception_id = reception_id(f.prefix_reception);
        let winner_reception_id = reception_id(f.winner_reception);
        let delivery_reception_id = reception_id(f.delivery_reception);
        let data = json!({"origin_id":id,"placement":"bridge_pair_v1","source_frame_id":frame_id,"source_frame_index":f.frame.to_string(),"stream_id":config.stream_id,"sequence":f.sequence.map(|s| s.to_string()),"created_ps":f.created_ps.to_string(),"first_sof_ps":f.prefix.first_sof_ps.map(|t|t.to_string()),"prefix_reception_id":prefix_reception_id,"winner_reception_id":winner_reception_id,"continuation_created":f.continued,"continuation_ps":f.recoveries.iter().find(|r|r.continuation).map(|r|r.time_ps.to_string()),"suffix_root":f.suffix_root,"delivery_reception_id":delivery_reception_id,"completed_ps":f.terminal_ps.map(|t| t.to_string()),"completed_count":if f.delivery_reception.is_some() {"1"} else {"0"},"target_count":if f.prefix.first_sof_ps.is_some() {"1"} else {"0"},"target":self.prepared.ethernet.devices[pair.destination_device].id,"prefix_ports":pair.prefix.ports,"branch_ports":config.paths.iter().map(|p|&p.ports).collect::<Vec<_>>(),"suffix_ports":pair.suffix.ports,"dropped":f.dropped,"prefix":f.prefix,"branches":f.branches,"suffix":f.suffix});
        let mut data = data;
        if normative {
            data["continuation_rounds"] = json!(
                f.rounds
                    .iter()
                    .map(|round| {
                        let mut row = decimal(json!(round));
                        row["input_reception_id"] =
                            json!(self.reception(d, round.input_reception).row.reception_id);
                        row["delivery_reception_id"] =
                            json!(reception_id(round.delivery_reception));
                        row["created_ps"] = json!(
                            f.recoveries
                                .iter()
                                .find(|r| r.order == round.recovery_order)
                                .unwrap()
                                .time_ps
                                .to_string()
                        );
                        row["suffix_root_transfer_id"] =
                            json!(self.transfer(d, round.suffix_root).row.transfer_id);
                        row
                    })
                    .collect::<Vec<_>>()
            );
            data["accepted_count"] = json!(
                f.recoveries
                    .iter()
                    .filter(|r| r.decision.starts_with("accept_"))
                    .count()
                    .to_string()
            );
            data["completed_count"] = json!(
                f.rounds
                    .iter()
                    .filter(|r| r.delivery_reception.is_some())
                    .count()
                    .to_string()
            );
        }
        self.replication_mut(d)?
            .records
            .push(("dir.redundancy.origin", id.into(), decimal(data)));
        for (path, phase) in f.branches.iter().enumerate() {
            let branch = identifier(&[id, &config.paths[path].id]);
            let recovery = f.recoveries.iter().find(|r| {
                self.transfer(d, self.reception(d, r.reception).transfer)
                    .circuit
                    .as_ref()
                    .and_then(|c| c.distributed.as_ref())
                    .is_some_and(|dc| dc.leg == (DistributedLeg::Branch { path }))
            });
            let recovery_id =
                recovery.map(|_| identifier(&[id, &config.paths[path].id, "recover"]));
            let status = if phase.failure_reason.is_some() {
                "dropped"
            } else if let Some(recovery) = recovery {
                recovery.decision.as_str()
            } else if phase.first_sof_ps.is_some() {
                "in_flight"
            } else {
                "queued"
            };
            let hop = phase
                .current_transfer
                .map(|t| self.transfer(d, t).circuit.as_ref().unwrap().hop)
                .unwrap_or(0);
            let current = phase
                .current_transfer
                .filter(|_| phase.terminal_ps.is_none())
                .map(|t| self.transfer(d, t).row.transfer_id.clone());
            let data = json!({"branch_id":branch,"origin_id":id,"stream_id":config.stream_id,"sequence":f.sequence.unwrap().to_string(),"path_id":config.paths[path].id,"path_index":path.to_string(),"path_ports":config.paths[path].ports,"hop_index":hop.to_string(),"current_transfer_id":current,"created_ps":f.prefix.terminal_ps.unwrap().to_string(),"first_sof_ps":phase.first_sof_ps.map(|t|t.to_string()),"completed_ps":phase.terminal_ps.map(|t|t.to_string()),"status":status,"reason":phase.failure_reason,"recovery_id":recovery_id});
            self.replication_mut(d)?
                .records
                .push(("dir.redundancy.branch", branch, data));
        }
        Ok(())
    }
    pub(super) fn distributed_status(
        &self,
        id: &str,
        leg: &DistributedLeg,
        t: usize,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        let transfer = self.transfer(d, t).clone();
        let failed_rx = d
            .receptions
            .iter()
            .find(|(_, r)| {
                r.transfer == t && matches!(r.row.status.as_str(), "dropped" | "filtered")
            })
            .map(|(_, r)| r.row.reason.clone().unwrap_or_else(|| "filtered".into()));
        let failure = transfer.row.drop_reason.clone().or(failed_rx);
        let flow = self
            .replication_mut(d)?
            .distributed
            .get_mut(id)
            .ok_or_else(|| error("unknown distributed flow"))?;
        let phase = match leg {
            DistributedLeg::Prefix => &mut flow.prefix,
            DistributedLeg::Suffix => &mut flow.suffix,
            DistributedLeg::SuffixRound { round } => {
                &mut flow
                    .rounds
                    .get_mut(*round)
                    .ok_or_else(|| error("unknown continuation round"))?
                    .suffix
            }
            DistributedLeg::Branch { path } => flow
                .branches
                .get_mut(*path)
                .ok_or_else(|| error("missing distributed branch lifecycle"))?,
        };
        phase.current_transfer = Some(t);
        if let Some(sof) = transfer.row.sof_ps {
            phase.first_sof_ps.get_or_insert(sof);
        }
        if let Some(reason) = failure {
            phase.terminal_ps = Some(now);
            phase.failure_reason = Some(reason);
        }
        if (phase.failure_reason.is_some() && !matches!(leg, DistributedLeg::Branch { .. }))
            || (matches!(leg, DistributedLeg::Branch { .. })
                && !flow.continued
                && flow.branches.iter().all(|b| b.terminal_ps.is_some()))
        {
            flow.dropped = true;
            flow.terminal_ps = Some(now);
        }
        self.distributed_publish(id, d)?;
        Ok(())
    }
    fn distributed_invalid_branch(
        &self,
        id: &str,
        leg: &DistributedLeg,
        now: u64,
        reason: &str,
        d: &mut NetworkDelta,
    ) -> Result<()> {
        if let DistributedLeg::Branch { path } = leg {
            let flow = self
                .replication_mut(d)?
                .distributed
                .get_mut(id)
                .ok_or_else(|| error("unknown distributed flow"))?;
            flow.branches[*path].terminal_ps = Some(now);
            flow.branches[*path].failure_reason = Some(reason.into());
            if !flow.continued && flow.branches.iter().all(|b| b.terminal_ps.is_some()) {
                flow.dropped = true;
                flow.terminal_ps = Some(now);
            }
        }
        self.distributed_publish(id, d)
    }
    #[allow(clippy::too_many_arguments)]
    fn distributed_offer(
        &self,
        reception: &Reception,
        origin: CopyOrigin,
        wire: &EthernetWireFrame,
        class: l2::Classification,
        direction: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<Vec<String>> {
        let incoming = self.transfer(d, reception.transfer).direction;
        let link = self.effective_link(incoming, reception.segment_receiver)?;
        let ingress = self
            .prepared
            .ethernet
            .port_policies
            .iter()
            .find(|policy| policy.ingress == link.to_port)
            .map(|policy| policy.port.as_str());
        if let Some(port) = ingress.filter(|port| {
            self.prepared
                .provider_l2
                .as_ref()
                .is_some_and(|topology| topology.ports.contains_key(*port))
        }) {
            let frame_id = self.transfer(d, reception.transfer).row.frame_id.clone();
            let Some((wire, class)) =
                self.provider_explicit_egress(port, wire, direction, d, &frame_id, c)?
            else {
                return Ok(Vec::new());
            };
            return self.offer(origin, &wire, class, &[direction], d, c);
        }
        self.offer(origin, wire, class, &[direction], d, c)
    }

    pub(super) fn distributed_complete(
        &self,
        reception: &mut Reception,
        transfer: &Transfer,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let cursor = transfer.circuit.as_ref().unwrap();
        let dc = cursor.distributed.as_ref().unwrap();
        let flow = self
            .replication_mut(d)?
            .distributed
            .get(&dc.flow)
            .ok_or_else(|| error("unknown distributed flow"))?
            .clone();
        let route = self.distributed_route(flow.stream, &dc.leg)?;
        if route.get(cursor.hop) != Some(&transfer.direction) {
            return Err(error("distributed physical cursor mismatch"));
        }
        let pair = self.distributed_config(flow.stream)?;
        let intended = route
            .get(cursor.hop + 1)
            .map(|next| self.prepared.ethernet.directions[*next].source)
            .unwrap_or(match dc.leg {
                DistributedLeg::Prefix => pair.replicate_device,
                DistributedLeg::Branch { .. } => pair.eliminate_device,
                DistributedLeg::Suffix | DistributedLeg::SuffixRound { .. } => {
                    pair.destination_device
                }
            });
        let received = self.effective_link(transfer.direction, reception.segment_receiver)?;
        if received.destination != intended {
            // A carrier fanout is one TX with independent MAC indications. An
            // uninvolved receiver must not allocate/recover this stream's state.
            reception.row.status = "filtered".into();
            reception.row.reason = Some("replication_unintended_receiver".into());
            return Ok(());
        }
        let wire = if matches!(dc.leg, DistributedLeg::Branch { .. }) {
            match frer_wire::strip(&transfer.row.wire) {
                Ok((wire, sequence)) if Some(sequence) == flow.sequence => wire,
                Ok(_) => {
                    reception.row.status = "dropped".into();
                    reception.row.reason = Some("sequence_mismatch".into());
                    self.distributed_invalid_branch(
                        &dc.flow,
                        &dc.leg,
                        c.now(),
                        "sequence_mismatch",
                        d,
                    )?;
                    return Ok(());
                }
                Err(reason) => {
                    reception.row.status = "dropped".into();
                    reception.row.reason = Some(reason.into());
                    self.distributed_invalid_branch(&dc.flow, &dc.leg, c.now(), reason, d)?;
                    return Ok(());
                }
            }
        } else {
            transfer.row.wire.clone()
        };
        let class = l2::Classification {
            vid: reception.row.vlan_id.unwrap(),
            priority: reception.row.priority,
            dei: wire.tag.map_or(0, |t| t.dei),
        };
        if cursor.hop + 1 < route.len() {
            reception.row.egress_transfer_ids = self.distributed_offer(
                reception,
                CopyOrigin {
                    circuit: Some(Circuit {
                        branch: cursor.branch.clone(),
                        hop: cursor.hop + 1,
                        distributed: Some(dc.clone()),
                    }),
                    frame: flow.frame,
                    parent: Some(reception.transfer),
                    visit: reception.visit,
                },
                &wire,
                class,
                route[cursor.hop + 1],
                d,
                c,
            )?;
            reception.row.status = "forwarded".into();
            return Ok(());
        }
        let ri = self.receptions.len()
            + d.receptions
                .iter()
                .filter(|(i, _)| *i >= self.receptions.len())
                .count();
        // Processed receptions already exist in the owner; resolve their stable index.
        let ri = (0..ri)
            .find(|&i| self.reception(d, i).row.reception_id == reception.row.reception_id)
            .ok_or_else(|| error("distributed reception owner"))?;
        match dc.leg {
            DistributedLeg::Prefix => {
                if flow.prefix_reception.is_some() {
                    return Err(error("distributed prefix completed twice"));
                }
                let config = &self
                    .prepared
                    .extensions
                    .as_ref()
                    .unwrap()
                    .replication
                    .as_ref()
                    .unwrap()
                    .streams[flow.stream];
                let state = self.replication_mut(d)?;
                let seq = state.streams[flow.stream].allocate()? as u16;
                if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
                    state.sequence_allocations.push(SequenceAllocation {
                        stream: flow.stream,
                        frame: flow.frame,
                        time_ps: c.now(),
                        sequence: seq,
                    });
                }
                let live = state.distributed.get_mut(&dc.flow).unwrap();
                live.sequence = Some(seq);
                live.prefix_reception = Some(ri);
                live.prefix.terminal_ps = Some(c.now());
                live.branches = vec![DistributedPhaseState::default(); config.paths.len()];
                for (path, config_path) in config.paths.iter().enumerate() {
                    if path == 1 {
                        crate::allocation::reservation_checkpoint("network_fanout_second_branch")?;
                    }
                    let ids = self.distributed_offer(
                        reception,
                        CopyOrigin {
                            circuit: Some(Circuit {
                                branch: identifier(&[&dc.flow, &config_path.id]),
                                hop: 0,
                                distributed: Some(DistributedCursor {
                                    flow: dc.flow.clone(),
                                    leg: DistributedLeg::Branch { path },
                                }),
                            }),
                            frame: flow.frame,
                            parent: Some(reception.transfer),
                            visit: reception.visit,
                        },
                        &wire,
                        class,
                        config_path.directions[0],
                        d,
                        c,
                    )?;
                    reception.row.egress_transfer_ids.extend(ids);
                }
                reception.row.status = "forwarded".into();
            }
            DistributedLeg::Branch { path } => {
                let config = &self
                    .prepared
                    .extensions
                    .as_ref()
                    .unwrap()
                    .replication
                    .as_ref()
                    .unwrap()
                    .streams[flow.stream];
                let normative =
                    config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017;
                let received_sequence =
                    u64::from(frer_wire::receive(&transfer.row.wire).map_err(error)?);
                let (decision, before, previous, after, high) =
                    self.replication_recover(flow.stream, received_sequence, d, c.now())?;
                let state = self.replication_mut(d)?;
                let continuation =
                    decision.starts_with("accept_") && (normative || !flow.continued);
                let recovery_id = identifier(&[&dc.flow, &config.paths[path].id, "recover"]);
                state.records.push(("dir.redundancy.recovery", recovery_id.clone(), json!({"recovery_id":recovery_id,"placement":"bridge_pair_v1","origin_id":dc.flow,"input_branch":cursor.branch,"reception_id":reception.row.reception_id,"stream_id":config.stream_id,"sequence":flow.sequence.unwrap().to_string(),"eliminate_at":config.eliminate_at,"decision":decision,"window_decision":decision,"continuation_created":continuation,"suppression_reason":if decision.starts_with("accept_") && flow.continued && !normative {Some("already_continued")} else {None},"previous_watermark":opt(previous),"new_watermark":opt(high),"history_capacity":config.history_capacity.to_string(),"bitmap_before_hex":before,"bitmap_after_hex":after,"delivered":false})));
                let order = state.endpoint_recoveries.len() as u64
                    + state
                        .distributed
                        .values()
                        .map(|f| f.recoveries.len() as u64)
                        .sum::<u64>();
                let live = state.distributed.get_mut(&dc.flow).unwrap();
                live.branches[path].terminal_ps = Some(c.now());
                live.recoveries.push(DistributedRecovery {
                    order,
                    reception: ri,
                    time_ps: c.now(),
                    decision: decision.into(),
                    continuation,
                });
                if !continuation
                    && !live.continued
                    && live.branches.iter().all(|b| b.terminal_ps.is_some())
                {
                    live.dropped = true;
                    live.terminal_ps = Some(c.now());
                }
                if continuation {
                    crate::allocation::reservation_checkpoint("network_recovery_deliver")?;
                    live.continued = true;
                    live.winner_reception.get_or_insert(ri);
                    let root = self.transfers.len()
                        + d.transfers
                            .iter()
                            .filter(|(i, _)| *i >= self.transfers.len())
                            .count();
                    let round = flow.rounds.len();
                    let leg = if normative {
                        DistributedLeg::SuffixRound { round }
                    } else {
                        DistributedLeg::Suffix
                    };
                    let live = self
                        .replication_mut(d)?
                        .distributed
                        .get_mut(&dc.flow)
                        .unwrap();
                    live.suffix_root.get_or_insert(root);
                    if normative {
                        live.rounds.push(ContinuationRound {
                            id: identifier(&[&dc.flow, &recovery_id, "continuation"]),
                            recovery_order: order,
                            input_reception: ri,
                            suffix_root: root,
                            suffix: DistributedPhaseState::default(),
                            delivery_reception: None,
                        });
                    }
                    reception.row.egress_transfer_ids = self.distributed_offer(
                        reception,
                        CopyOrigin {
                            circuit: Some(Circuit {
                                branch: dc.flow.clone(),
                                hop: 0,
                                distributed: Some(DistributedCursor {
                                    flow: dc.flow.clone(),
                                    leg: leg.clone(),
                                }),
                            }),
                            frame: flow.frame,
                            parent: Some(reception.transfer),
                            visit: reception.visit,
                        },
                        &wire,
                        class,
                        self.distributed_route(flow.stream, &leg)?[0],
                        d,
                        c,
                    )?;
                }
                reception.row.status = if continuation {
                    "forwarded"
                } else {
                    "received"
                }
                .into();
                reception.row.reason = None;
            }
            DistributedLeg::Suffix | DistributedLeg::SuffixRound { .. } => {
                let round = match dc.leg {
                    DistributedLeg::SuffixRound { round } => Some(round),
                    _ => None,
                };
                if round.map_or(flow.delivery_reception.is_some(), |r| {
                    flow.rounds[r].delivery_reception.is_some()
                }) {
                    return Err(error("distributed destination delivered twice"));
                }
                let live = self
                    .replication_mut(d)?
                    .distributed
                    .get_mut(&dc.flow)
                    .unwrap();
                live.delivery_reception.get_or_insert(ri);
                if let Some(round) = round {
                    live.rounds[round].delivery_reception = Some(ri);
                    live.rounds[round].suffix.terminal_ps = Some(c.now());
                } else {
                    live.terminal_ps = Some(c.now());
                    live.suffix.terminal_ps = Some(c.now());
                }
                let frame = self.frame(d, flow.frame);
                let destination = self.prepared.ethernet.devices
                    [self.distributed_config(flow.stream)?.destination_device]
                    .id
                    .clone();
                d.points.push(Point {
                    event_seq: None,
                    effect_seq: None,
                    time_ps: c.now(),
                    target: destination.clone(),
                    metric: "ethernet.delivery_ps".into(),
                    value: c
                        .now()
                        .checked_sub(frame.row.generated_ps)
                        .ok_or_else(|| error("negative distributed delivery"))?,
                    request_id: Some(frame.row.frame_id.clone()),
                    receiver: Some(destination),
                    reason: None,
                });
                reception.row.status = "received".into();
                reception.row.reason = None;
                self.qcc_delivered(transfer, d, c)?;
                self.notify_terminal_delivery(ri, reception, transfer, round, d, c)?;
            }
        }
        self.distributed_publish(&dc.flow, d)
    }
}

pub(super) fn prepared_identity(p: &crate::types::network::PreparedReplication) -> Option<Value> {
    let rows = p.streams.iter().filter_map(|s| match &s.placement {
        ReplicationPlacement::LegacyEndpoint => None,
        ReplicationPlacement::BridgePair(pair) => { let mut row=json!({"stream_id":s.stream_id,"source_generator":s.source_generator,"placement":"bridge_pair_v1","forwarding":"explicit_physical_route_v1","replicate_device":pair.replicate_device,"eliminate_device":pair.eliminate_device,"destination_device":pair.destination_device,"prefix_ports":pair.prefix.ports,"prefix_directions":pair.prefix.directions,"prefix_endpoints":pair.prefix.endpoints,"branches":s.paths.iter().map(|p|json!({"id":p.id,"ports":p.ports,"directions":p.directions,"endpoints":p.endpoints})).collect::<Vec<_>>(),"suffix_ports":pair.suffix.ports,"suffix_directions":pair.suffix.directions,"suffix_endpoints":pair.suffix.endpoints,"wire_format":"rtag_8021cb_2017","modulus":65536,"history_capacity":s.history_capacity,"timeout_ps":s.recovery_timeout_ps,"composition":"physical_copy_tsn_v1"}); if matches!(s.source, ReplicationSource::GatewayEgress { .. }) { row.as_object_mut().unwrap().remove("source_generator"); row["source"] = json!(s.source); row["source_device"] = json!(s.source_device); row["source_port"] = json!(s.source_port); }
            if s.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 { row["recovery_algorithm"]=json!("vector_8021cb_2017");row["recovery_tick_ps"]=json!(replication::RECOVERY_TICK_PS); } Some(row) },
    }).collect::<Vec<_>>();
    (!rows.is_empty()).then(|| json!(rows))
}

impl NetworkState {
    pub(super) fn validate_distributed_checkpoint(&self, horizon: u64) -> Result<()> {
        let Some(state) = &self.replication else {
            return Ok(());
        };
        let cfg = self
            .prepared
            .extensions
            .as_ref()
            .unwrap()
            .replication
            .as_ref()
            .unwrap();
        if state.streams.len() != cfg.streams.len() {
            return Err(error("network checkpoint distributed stream count"));
        }
        if state.distributed_identity != prepared_identity(cfg) {
            return Err(error("network checkpoint distributed prepared identity"));
        }
        let mut evidence = Vec::new();
        for (id, flow) in &state.distributed {
            let frame = self
                .frames
                .get(flow.frame)
                .ok_or_else(|| error("network checkpoint distributed frame"))?;
            let config = cfg
                .streams
                .get(flow.stream)
                .ok_or_else(|| error("network checkpoint distributed stream"))?;
            self.validate_replication_frame_source(frame, flow.stream, None)?;
            let pair = self.distributed_config(flow.stream)?;
            if self.replication_source_stream(frame) != Some(flow.stream)
                || id
                    != &identifier(&[
                        "ethernet.dynamic.frame",
                        &frame.row.frame_id,
                        &config.stream_id,
                        "bridge_pair_v1",
                    ])
                || flow.created_ps > horizon
            {
                return Err(error("network checkpoint distributed flow identity"));
            }
            let validate_rx = |ri: usize, leg: &DistributedLeg| -> Result<&Reception> {
                let r = self
                    .receptions
                    .get(ri)
                    .ok_or_else(|| error("network checkpoint distributed reception"))?;
                let t = self
                    .transfers
                    .get(r.transfer)
                    .ok_or_else(|| error("network checkpoint distributed reception transfer"))?;
                let cursor = t
                    .circuit
                    .as_ref()
                    .and_then(|c| c.distributed.as_ref())
                    .ok_or_else(|| error("network checkpoint distributed reception cursor"))?;
                let route = self.distributed_route(flow.stream, leg)?;
                let actual = self.effective_link(t.direction, r.segment_receiver)?;
                let intended = match leg {
                    DistributedLeg::Prefix => pair.replicate_device,
                    DistributedLeg::Branch { .. } => pair.eliminate_device,
                    DistributedLeg::Suffix | DistributedLeg::SuffixRound { .. } => {
                        pair.destination_device
                    }
                };
                if actual.destination != intended
                    || cursor.flow != *id
                    || cursor.leg != *leg
                    || t.frame != flow.frame
                    || t.direction != *route.last().unwrap()
                    || r.row.ready_ps.is_none_or(|time| time > horizon)
                    || !matches!(r.row.status.as_str(), "forwarded" | "received")
                    || t.row
                        .arrival_ps
                        .is_none_or(|arrival| arrival > r.row.ready_ps.unwrap_or(0))
                {
                    return Err(error(
                        "network checkpoint distributed terminal reception evidence",
                    ));
                }
                Ok(r)
            };
            if flow.sequence.is_some() != flow.prefix_reception.is_some() {
                return Err(error("network checkpoint distributed premature sequence"));
            }
            if let Some(prefix) = flow.prefix_reception {
                validate_rx(prefix, &DistributedLeg::Prefix)?;
            }
            if flow.branches.len()
                != if flow.sequence.is_some() {
                    config.paths.len()
                } else {
                    0
                }
            {
                return Err(error(
                    "network checkpoint distributed branch lifecycle count",
                ));
            }
            let normative =
                config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017;
            let mut phases = vec![(DistributedLeg::Prefix, &flow.prefix)];
            if normative {
                for (round, r) in flow.rounds.iter().enumerate() {
                    phases.push((DistributedLeg::SuffixRound { round }, &r.suffix));
                }
            } else {
                phases.push((DistributedLeg::Suffix, &flow.suffix));
            }
            for (path, phase) in flow.branches.iter().enumerate() {
                phases.push((DistributedLeg::Branch { path }, phase));
            }
            for (leg, phase) in phases {
                let transfers = self
                    .transfers
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        t.circuit
                            .as_ref()
                            .and_then(|c| c.distributed.as_ref())
                            .is_some_and(|c| c.flow == *id && c.leg == leg)
                    })
                    .collect::<Vec<_>>();
                if transfers.last().map(|(i, _)| *i) != phase.current_transfer {
                    return Err(error(
                        "network checkpoint distributed lifecycle current transfer",
                    ));
                }
                let first_sof = transfers.iter().filter_map(|(_, t)| t.row.sof_ps).min();
                if first_sof != phase.first_sof_ps {
                    return Err(error("network checkpoint distributed lifecycle SOF"));
                }
                let expected_terminal = transfers.last().and_then(|(_, t)| {
                    if t.row.status == "dropped" {
                        Some((t.row.time_ps, t.row.drop_reason.clone()))
                    } else {
                        self.receptions
                            .iter()
                            .find(|r| r.transfer == transfers.last().unwrap().0)
                            .and_then(|r| {
                                if matches!(r.row.status.as_str(), "filtered" | "dropped") {
                                    Some((
                                        r.row.ready_ps.unwrap_or(r.row.observed_ps),
                                        r.row.reason.clone(),
                                    ))
                                } else if t.circuit.as_ref().unwrap().hop + 1
                                    == self.distributed_route(flow.stream, &leg).ok()?.len()
                                    && matches!(r.row.status.as_str(), "received" | "forwarded")
                                {
                                    Some((r.row.ready_ps?, None))
                                } else {
                                    None
                                }
                            })
                    }
                });
                if phase.terminal_ps != expected_terminal.as_ref().map(|(t, _)| *t)
                    || phase.failure_reason != expected_terminal.and_then(|(_, r)| r)
                {
                    return Err(error(
                        "network checkpoint distributed lifecycle terminal evidence",
                    ));
                }
            }
            let prefix_roots = self
                .transfers
                .iter()
                .filter(|t| {
                    t.circuit.as_ref().is_some_and(|c| {
                        c.hop == 0
                            && c.distributed.as_ref().is_some_and(|dc| {
                                dc.flow == *id && dc.leg == DistributedLeg::Prefix
                            })
                    })
                })
                .collect::<Vec<_>>();
            if prefix_roots.len() != 1 || prefix_roots[0].row.queued_ps != flow.created_ps {
                return Err(error("network checkpoint distributed prefix root"));
            }
            let expected_dropped = flow.prefix.failure_reason.is_some()
                || flow.suffix.failure_reason.is_some()
                || !flow.continued
                    && !flow.branches.is_empty()
                    && flow.branches.iter().all(|b| b.terminal_ps.is_some());
            let expected_terminal =
                if flow.delivery_reception.is_some() || flow.suffix.failure_reason.is_some() {
                    flow.suffix.terminal_ps
                } else if flow.prefix.failure_reason.is_some() {
                    flow.prefix.terminal_ps
                } else if expected_dropped {
                    flow.branches.iter().filter_map(|b| b.terminal_ps).max()
                } else {
                    None
                };
            if !normative
                && (flow.dropped != expected_dropped || flow.terminal_ps != expected_terminal)
            {
                return Err(error(
                    "network checkpoint distributed flow terminal evidence",
                ));
            }
            let mut continuations = 0;
            let mut recovery_inputs = std::collections::BTreeSet::new();
            for recovery in &flow.recoveries {
                if !recovery_inputs.insert(recovery.reception)
                    || recovery.continuation
                        != (recovery.decision.starts_with("accept_")
                            && (normative || continuations == 0))
                {
                    return Err(error(
                        "network checkpoint distributed recovery latch evidence",
                    ));
                }
                let r = self
                    .receptions
                    .get(recovery.reception)
                    .ok_or_else(|| error("network checkpoint distributed recovery reception"))?;
                let t = self
                    .transfers
                    .get(r.transfer)
                    .ok_or_else(|| error("network checkpoint distributed recovery transfer"))?;
                let leg = &t
                    .circuit
                    .as_ref()
                    .and_then(|c| c.distributed.as_ref())
                    .ok_or_else(|| error("network checkpoint distributed recovery cursor"))?
                    .leg;
                if !matches!(leg, DistributedLeg::Branch { .. }) {
                    return Err(error("network checkpoint distributed recovery phase"));
                }
                validate_rx(recovery.reception, leg)?;
                if r.row.ready_ps != Some(recovery.time_ps)
                    || frer_wire::receive(&t.row.wire).ok() != flow.sequence
                {
                    return Err(error("network checkpoint distributed recovery wire/time"));
                }
                if recovery.continuation {
                    continuations += 1;
                    if !normative && flow.winner_reception != Some(recovery.reception) {
                        return Err(error("network checkpoint distributed recovery winner"));
                    }
                }
                evidence.push((
                    recovery.order,
                    flow.stream,
                    frer_wire::receive(&t.row.wire).map_err(error)?,
                    recovery,
                ));
            }
            for (ri, r) in self.receptions.iter().enumerate() {
                let t = &self.transfers[r.transfer];
                if let Some(dc) = t.circuit.as_ref().and_then(|c| c.distributed.as_ref()) {
                    if dc.flow == *id
                        && matches!(dc.leg, DistributedLeg::Branch { .. })
                        && t.circuit.as_ref().unwrap().hop + 1
                            == self.distributed_route(flow.stream, &dc.leg)?.len()
                        && matches!(r.row.status.as_str(), "forwarded" | "received")
                        && !recovery_inputs.contains(&ri)
                    {
                        return Err(error(
                            "network checkpoint distributed omitted recovery input",
                        ));
                    }
                }
            }
            if !normative
                && (continuations > 1
                    || flow.continued != (continuations == 1)
                    || flow.continued != flow.suffix_root.is_some()
                    || flow.continued != flow.winner_reception.is_some())
            {
                return Err(error("network checkpoint distributed continuation latch"));
            }
            if normative {
                let accepted = flow
                    .recoveries
                    .iter()
                    .filter(|r| r.continuation)
                    .collect::<Vec<_>>();
                if flow.rounds.len() != accepted.len() || flow.continued == accepted.is_empty() {
                    return Err(error("network checkpoint vector continuation bijection"));
                }
                for (round, (r, recovery)) in flow.rounds.iter().zip(accepted).enumerate() {
                    let input = &self.receptions[recovery.reception];
                    let root = self
                        .transfers
                        .get(r.suffix_root)
                        .ok_or_else(|| error("network checkpoint vector suffix root"))?;
                    let path = match self.transfers[input.transfer]
                        .circuit
                        .as_ref()
                        .unwrap()
                        .distributed
                        .as_ref()
                        .unwrap()
                        .leg
                    {
                        DistributedLeg::Branch { path } => path,
                        _ => unreachable!(),
                    };
                    let recovery_id = identifier(&[id, &config.paths[path].id, "recover"]);
                    if r.id != identifier(&[id, &recovery_id, "continuation"])
                        || r.recovery_order != recovery.order
                        || r.input_reception != recovery.reception
                        || root.circuit.as_ref().is_none_or(|c| {
                            c.hop != 0
                                || c.distributed.as_ref().is_none_or(|dc| {
                                    dc.flow != *id
                                        || dc.leg != (DistributedLeg::SuffixRound { round })
                                })
                        })
                        || root.row.queued_ps != recovery.time_ps
                        || root.row.parent_transfer_id.as_ref()
                            != Some(&self.transfers[input.transfer].row.transfer_id)
                    {
                        return Err(error("network checkpoint vector round causality"));
                    }
                    if let Some(delivery) = r.delivery_reception {
                        let rx = validate_rx(delivery, &DistributedLeg::SuffixRound { round })?;
                        if r.suffix.terminal_ps != rx.row.ready_ps
                            || r.suffix.failure_reason.is_some()
                        {
                            return Err(error("network checkpoint vector round delivery"));
                        }
                    }
                }
                let roots = self
                    .transfers
                    .iter()
                    .filter(|t| {
                        t.circuit.as_ref().is_some_and(|c| {
                            c.hop == 0
                                && c.distributed.as_ref().is_some_and(|dc| {
                                    dc.flow == *id
                                        && matches!(dc.leg, DistributedLeg::SuffixRound { .. })
                                })
                        })
                    })
                    .count();
                if roots != flow.rounds.len() {
                    return Err(error("network checkpoint vector extra suffix root"));
                }
                let all_done = flow.prefix.terminal_ps.is_some()
                    && (flow.prefix.failure_reason.is_some()
                        || (!flow.branches.is_empty()
                            && flow.branches.iter().all(|b| b.terminal_ps.is_some())
                            && flow.rounds.iter().all(|r| r.suffix.terminal_ps.is_some())));
                let terminal = if all_done {
                    flow.prefix
                        .terminal_ps
                        .into_iter()
                        .chain(flow.branches.iter().filter_map(|b| b.terminal_ps))
                        .chain(flow.rounds.iter().filter_map(|r| r.suffix.terminal_ps))
                        .max()
                } else {
                    None
                };
                if flow.terminal_ps != terminal
                    || flow.dropped
                        != (all_done && !flow.rounds.iter().any(|r| r.delivery_reception.is_some()))
                    || flow.winner_reception != flow.rounds.first().map(|r| r.input_reception)
                    || flow.suffix_root != flow.rounds.first().map(|r| r.suffix_root)
                    || flow.delivery_reception
                        != flow
                            .rounds
                            .iter()
                            .filter_map(|r| r.delivery_reception)
                            .min_by_key(|ri| self.receptions[*ri].row.ready_ps)
                {
                    return Err(error(
                        "network checkpoint vector aggregate terminal evidence",
                    ));
                }
            }
            let suffix_roots = self
                .transfers
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.circuit.as_ref().is_some_and(|c| {
                        c.hop == 0
                            && c.distributed.as_ref().is_some_and(|dc| {
                                dc.flow == *id && dc.leg == DistributedLeg::Suffix
                            })
                    })
                })
                .collect::<Vec<_>>();
            if !normative
                && (suffix_roots.len() != usize::from(flow.continued)
                    || suffix_roots.first().map(|(i, _)| *i) != flow.suffix_root)
            {
                return Err(error("network checkpoint distributed suffix uniqueness"));
            }
            if let Some((_, root)) = suffix_roots.first() {
                let winner = self
                    .receptions
                    .get(flow.winner_reception.unwrap())
                    .ok_or_else(|| error("network checkpoint distributed suffix winner"))?;
                if root.row.parent_transfer_id.as_ref()
                    != Some(&self.transfers[winner.transfer].row.transfer_id)
                    || root.row.queued_ps != winner.row.ready_ps.unwrap()
                {
                    return Err(error("network checkpoint distributed suffix parent"));
                }
            }
            if let Some(delivery) = flow.delivery_reception.filter(|_| !normative) {
                let r = validate_rx(delivery, &DistributedLeg::Suffix)?;
                if flow.terminal_ps != r.row.ready_ps
                    || flow.dropped
                    || self
                        .effective_link(self.transfers[r.transfer].direction, r.segment_receiver)?
                        .destination
                        != pair.destination_device
                {
                    return Err(error("network checkpoint distributed delivery"));
                }
            }
        }
        for t in &self.transfers {
            let Some(cursor) = t.circuit.as_ref() else {
                continue;
            };
            let Some(dc) = &cursor.distributed else {
                continue;
            };
            let flow = state
                .distributed
                .get(&dc.flow)
                .ok_or_else(|| error("network checkpoint missing distributed flow"))?;
            let route = self.distributed_route(flow.stream, &dc.leg)?;
            if route.get(cursor.hop) != Some(&t.direction) || t.frame != flow.frame {
                return Err(error("network checkpoint distributed hop/direction"));
            }
            if let DistributedLeg::Branch { .. } = dc.leg {
                if frer_wire::receive(&t.row.wire).ok() != flow.sequence || flow.sequence.is_none()
                {
                    return Err(error("network checkpoint distributed branch sequence/FCS"));
                }
            } else if frer_wire::receive(&t.row.wire).is_ok() {
                return Err(error("network checkpoint unexpected R-TAG outside branch"));
            }
            if matches!(dc.leg, DistributedLeg::Prefix) && cursor.hop == 0 {
                if t.row.parent_transfer_id.is_some() {
                    return Err(error("network checkpoint distributed prefix parent"));
                }
            } else {
                let parent = t
                    .row
                    .parent_transfer_id
                    .as_ref()
                    .and_then(|id| self.transfers.iter().position(|p| &p.row.transfer_id == id))
                    .ok_or_else(|| error("network checkpoint distributed actual parent"))?;
                let rx = self
                    .receptions
                    .iter()
                    .find(|r| {
                        r.transfer == parent
                            && r.row.egress_transfer_ids.contains(&t.row.transfer_id)
                    })
                    .ok_or_else(|| error("network checkpoint distributed parent reception"))?;
                if rx.row.status != "forwarded"
                    || rx.row.ready_ps != Some(t.row.queued_ps)
                    || !rx.row.egress_transfer_ids.contains(&t.row.transfer_id)
                {
                    return Err(error("network checkpoint distributed parent causality"));
                }
                let parent_cursor = self.transfers[parent]
                    .circuit
                    .as_ref()
                    .and_then(|c| c.distributed.as_ref())
                    .ok_or_else(|| error("network checkpoint distributed parent phase"))?;
                if parent_cursor.flow != dc.flow {
                    return Err(error("network checkpoint distributed foreign parent"));
                }
                let incoming = &self.transfers[parent];
                let parent_link = self.effective_link(incoming.direction, rx.segment_receiver)?;
                let ingress = self
                    .prepared
                    .ethernet
                    .port_policies
                    .iter()
                    .find(|policy| policy.ingress == parent_link.to_port)
                    .map(|policy| policy.port.as_str());
                let parent_branch = matches!(parent_cursor.leg, DistributedLeg::Branch { .. });
                let base = if parent_branch {
                    frer_wire::strip(&incoming.row.wire).map_err(error)?.0
                } else {
                    incoming.row.wire.clone()
                };
                if let Some(ingress) = ingress.filter(|port| {
                    self.prepared
                        .provider_l2
                        .as_ref()
                        .is_some_and(|topology| topology.ports.contains_key(*port))
                }) {
                    let expected = self
                        .provider_explicit_wire(ingress, &base, t.direction)?
                        .ok_or_else(|| error("network checkpoint provider explicit egress"))?;
                    let expected = if matches!(dc.leg, DistributedLeg::Branch { .. }) {
                        frer_wire::transmit(
                            &expected,
                            flow.sequence.ok_or_else(|| error("distributed sequence"))?,
                        )
                        .map_err(error)?
                    } else {
                        expected
                    };
                    if expected.mac_hex != t.row.wire.mac_hex {
                        return Err(error("network checkpoint provider explicit wire bytes"));
                    }
                } else if matches!(dc.leg, DistributedLeg::SuffixRound { .. })
                    && cursor.hop == 0
                    && t.row.wire.mac_hex != base.mac_hex
                {
                    return Err(error("network checkpoint vector suffix wire bytes"));
                }
                if cursor.hop > 0 {
                    let parent_hop = self.transfers[parent].circuit.as_ref().unwrap().hop;
                    if parent_cursor.leg != dc.leg || parent_hop + 1 != cursor.hop {
                        return Err(error("network checkpoint distributed parent hop"));
                    }
                } else if matches!(dc.leg, DistributedLeg::Branch { .. })
                    && flow.prefix_reception
                        != self
                            .receptions
                            .iter()
                            .position(|r| r.row.reception_id == rx.row.reception_id)
                {
                    return Err(error("network checkpoint distributed branch parent"));
                }
            }
        }
        for rx in &self.receptions {
            let transfer = &self.transfers[rx.transfer];
            if transfer.circuit.is_some()
                && !self.replication_receiver_is_intended(
                    transfer,
                    rx.segment_receiver,
                    &self.delta(),
                )?
                && (matches!(rx.row.status.as_str(), "forwarded" | "received")
                    || !rx.row.egress_transfer_ids.is_empty())
            {
                return Err(error(
                    "network checkpoint unintended replication receiver advanced",
                ));
            }
        }
        let mut endpoint_inputs = std::collections::BTreeSet::new();
        for entry in &state.endpoint_recoveries {
            let recovery = &entry.recovery;
            let config = cfg
                .streams
                .get(entry.stream)
                .ok_or_else(|| error("network checkpoint endpoint recovery stream"))?;
            if config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
                || !matches!(config.placement, ReplicationPlacement::LegacyEndpoint)
                || !endpoint_inputs.insert(recovery.reception)
            {
                return Err(error("network checkpoint endpoint recovery identity"));
            }
            let rx = self
                .receptions
                .get(recovery.reception)
                .ok_or_else(|| error("network checkpoint endpoint recovery reception"))?;
            let transfer = &self.transfers[rx.transfer];
            let cursor = transfer
                .circuit
                .as_ref()
                .ok_or_else(|| error("network checkpoint endpoint recovery cursor"))?;
            let frame = &self.frames[transfer.frame];
            let generator = frame
                .generator
                .and_then(|i| self.prepared.ethernet.generators.get(i))
                .ok_or_else(|| error("network checkpoint endpoint generator"))?;
            let path = config
                .paths
                .iter()
                .find(|p| {
                    cursor.branch
                        == identifier(&[
                            &identifier(&["ethernet.dynamic.frame", &frame.row.frame_id]),
                            &config.stream_id,
                            &p.id,
                        ])
                })
                .ok_or_else(|| error("network checkpoint endpoint branch identity"))?;
            if cursor.distributed.is_some()
                || cursor.hop + 1 != path.directions.len()
                || path.directions.get(cursor.hop) != Some(&transfer.direction)
                || generator.id != config.source_generator
                || !self.replication_receiver_is_intended(
                    transfer,
                    rx.segment_receiver,
                    &self.delta(),
                )?
                || rx.row.ready_ps != Some(recovery.time_ps)
                || recovery.time_ps > horizon
                || rx.row.status != "received"
                || recovery.continuation != recovery.decision.starts_with("accept_")
            {
                return Err(error("network checkpoint endpoint recovery causality"));
            }
            let sequence = frer_wire::receive(&transfer.row.wire).map_err(error)?;
            evidence.push((recovery.order, entry.stream, sequence, recovery));
        }
        for (ri, rx) in self.receptions.iter().enumerate() {
            let t = &self.transfers[rx.transfer];
            let Some(cursor) = t.circuit.as_ref().filter(|c| c.distributed.is_none()) else {
                continue;
            };
            let Some(generator) = self.frames[t.frame]
                .generator
                .and_then(|i| self.prepared.ethernet.generators.get(i))
            else {
                continue;
            };
            let Some(config) = cfg.streams.iter().find(|s| {
                s.source_generator == generator.id
                    && s.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017
                    && matches!(s.placement, ReplicationPlacement::LegacyEndpoint)
            }) else {
                continue;
            };
            if rx.row.status == "received"
                && config.paths.iter().any(|p| {
                    p.directions.last() == Some(&t.direction)
                        && cursor.hop + 1 == p.directions.len()
                })
                && !endpoint_inputs.contains(&ri)
            {
                return Err(error("network checkpoint omitted endpoint recovery"));
            }
        }
        let mut allocated = std::collections::BTreeSet::new();
        let mut next = vec![0u16; cfg.streams.len()];
        for allocation in &state.sequence_allocations {
            let config = cfg
                .streams
                .get(allocation.stream)
                .ok_or_else(|| error("network checkpoint vector allocation stream"))?;
            let frame = self
                .frames
                .get(allocation.frame)
                .ok_or_else(|| error("network checkpoint vector allocation frame"))?;
            self.validate_replication_frame_source(frame, allocation.stream, None)?;
            if config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
                || self.replication_source_stream(frame) != Some(allocation.stream)
                || allocation.sequence != next[allocation.stream]
                || allocation.time_ps > horizon
                || !allocated.insert((allocation.stream, allocation.frame))
            {
                return Err(error("network checkpoint vector allocation sequence"));
            }
            next[allocation.stream] = next[allocation.stream].wrapping_add(1);
            let roots = self
                .transfers
                .iter()
                .filter(|t| {
                    t.frame == allocation.frame
                        && t.circuit.as_ref().is_some_and(|c| {
                            c.hop == 0
                                && match &c.distributed {
                                    None => matches!(
                                        config.placement,
                                        ReplicationPlacement::LegacyEndpoint
                                    ),
                                    Some(dc) => matches!(dc.leg, DistributedLeg::Branch { .. }),
                                }
                        })
                })
                .collect::<Vec<_>>();
            if roots.len() != config.paths.len()
                || roots.iter().any(|t| {
                    t.row.queued_ps != allocation.time_ps
                        || frer_wire::receive(&t.row.wire).ok() != Some(allocation.sequence)
                })
            {
                return Err(error("network checkpoint vector allocation physical roots"));
            }
            if let ReplicationPlacement::BridgePair(_) = config.placement {
                let flow = state
                    .distributed
                    .values()
                    .find(|f| f.frame == allocation.frame && f.stream == allocation.stream)
                    .ok_or_else(|| error("network checkpoint vector allocation flow"))?;
                if flow.sequence != Some(allocation.sequence)
                    || flow
                        .prefix_reception
                        .and_then(|r| self.receptions[r].row.ready_ps)
                        != Some(allocation.time_ps)
                {
                    return Err(error("network checkpoint vector allocation prefix"));
                }
            }
        }
        for (i, config) in cfg.streams.iter().enumerate() {
            if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
                if state.streams[i].next != u64::from(next[i]) {
                    return Err(error("network checkpoint vector generator state"));
                }
                for t in &self.transfers {
                    if t.circuit.as_ref().is_some_and(|c| {
                        c.hop == 0
                            && c.distributed
                                .as_ref()
                                .is_none_or(|dc| matches!(dc.leg, DistributedLeg::Branch { .. }))
                    }) && (self.replication_source_stream(&self.frames[t.frame]) == Some(i))
                        && !allocated.contains(&(i, t.frame))
                    {
                        return Err(error("network checkpoint omitted vector allocation"));
                    }
                }
            }
        }
        evidence.sort_by_key(|(order, _, _, _)| *order);
        if evidence
            .windows(2)
            .any(|pair| pair[0].3.time_ps > pair[1].3.time_ps)
        {
            return Err(error("network checkpoint recovery physical order"));
        }
        if state
            .sequence_allocations
            .windows(2)
            .any(|pair| pair[0].time_ps > pair[1].time_ps)
        {
            return Err(error("network checkpoint generator physical order"));
        }
        let mut replay = replication::ReplicationState::new(cfg);
        for (expected, (order, stream, sequence, recovery)) in evidence.into_iter().enumerate() {
            if order != expected as u64 {
                return Err(error("network checkpoint distributed recovery order"));
            }
            let s = &mut replay.streams[stream];
            let normative = cfg.streams[stream].recovery_algorithm
                == ReplicationRecoveryAlgorithm::Vector8021cb2017;
            let decision = if normative {
                if let Some(deadline) = s.recovery_deadline_ps.filter(|d| *d <= recovery.time_ps) {
                    s.timeout(deadline, s.timer_generation)?;
                }
                s.vector_recover(
                    u64::from(sequence),
                    recovery.time_ps,
                    cfg.streams[stream].recovery_timeout_ps.unwrap(),
                )?
            } else {
                let timeout = cfg.streams[stream]
                    .recovery_timeout_ps
                    .is_some_and(|timeout| {
                        s.last_recovery_ps
                            .is_some_and(|last| recovery.time_ps.saturating_sub(last) >= timeout)
                    });
                if timeout {
                    s.history.high = None;
                    s.history.words.fill(0);
                }
                let decision = s.history.recover(u64::from(sequence));
                s.last_recovery_ps = Some(recovery.time_ps);
                if timeout && decision == "accept_first" {
                    "accept_reset"
                } else {
                    decision
                }
            };
            if decision != recovery.decision {
                return Err(error("network checkpoint distributed recovery decision"));
            }
        }
        for (i, config) in cfg.streams.iter().enumerate() {
            if !matches!(config.placement, ReplicationPlacement::BridgePair(_))
                && config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
            {
                continue;
            }
            let a = &state.streams[i];
            let b = &mut replay.streams[i];
            if config.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
                if let Some(deadline) = b.recovery_deadline_ps {
                    let pending_at_cut = self.due.iter().any(|due| due.at == deadline && matches!(due.kind, Completion::RecoveryTimeout { stream, generation } if stream == i && generation == b.timer_generation));
                    if deadline < horizon || deadline == horizon && !pending_at_cut {
                        b.timeout(deadline, b.timer_generation)?;
                    }
                }
                if a.timer_generation != b.timer_generation
                    || a.recovery_deadline_ps != b.recovery_deadline_ps
                    || a.reset_count != b.reset_count
                    || a.reset_events != b.reset_events
                {
                    return Err(error(
                        "network checkpoint vector recovery timer/reset history",
                    ));
                }
            }
            if a.history.high != b.history.high
                || a.history.words != b.history.words
                || a.last_recovery_ps != b.last_recovery_ps
                || (config.recovery_algorithm != ReplicationRecoveryAlgorithm::Vector8021cb2017
                    && a.next
                        != state
                            .distributed
                            .values()
                            .filter(|f| f.stream == i && f.sequence.is_some())
                            .count() as u64
                            % 65536)
            {
                return Err(error(
                    "network checkpoint distributed recovery history/sequence",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn distributed_atomic_transition_faults_preserve_the_committed_prefix() {
        for fault in ["network_fanout_second_branch", "network_recovery_deliver"] {
            let mut p = crate::prepare(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../examples/frer-rtag/distributed/model.ini"),
            )
            .unwrap();
            crate::allocation::fail_next_reservation(fault);
            let failed = crate::runtime::simulate(&p).unwrap();
            assert_eq!(failed.common.termination, "execution_failed", "{fault}");
            p.common.time_limit_ps = failed.common.end_ps;
            let baseline = crate::runtime::simulate(&p).unwrap();
            assert_eq!(
                failed.network.as_ref().unwrap()["redundancy"],
                baseline.network.as_ref().unwrap()["redundancy"],
                "{fault}"
            );
            assert_eq!(
                failed.ethernet.as_ref().unwrap().transfers.len(),
                baseline.ethernet.as_ref().unwrap().transfers.len(),
                "{fault}"
            );
        }
    }
}

#[cfg(test)]
mod normative_physical_tests {
    use super::*;
    #[test]
    fn normative_reservation_faults_leave_timer_history_and_suffix_uncommitted() {
        for fault in [
            "network_fanout_second_branch",
            "network_recovery_timer",
            "network_recovery_deliver",
        ] {
            let mut p = crate::prepare(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../examples/frer-rtag/normative/model.ini"),
            )
            .unwrap();
            crate::allocation::fail_next_reservation(fault);
            let failed = crate::runtime::simulate(&p).unwrap();
            assert_eq!(failed.common.termination, "execution_failed", "{fault}");
            p.common.time_limit_ps = failed.common.end_ps;
            let baseline = crate::runtime::simulate(&p).unwrap();
            assert_eq!(
                failed.network.as_ref().unwrap()["redundancy"],
                baseline.network.as_ref().unwrap()["redundancy"],
                "{fault}"
            );
            assert_eq!(
                failed.ethernet.as_ref().unwrap().transfers.len(),
                baseline.ethernet.as_ref().unwrap().transfers.len(),
                "{fault}"
            );
        }
    }
    #[test]
    fn accepted_copy_after_timeout_creates_two_actual_suffix_deliveries() {
        let mut p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/frer-rtag/distributed/model.ini"),
        )
        .unwrap();
        p.common.time_limit_ps = 4_000_000_000;
        let n = std::sync::Arc::make_mut(p.registered.as_mut().unwrap().network.as_mut().unwrap());
        n.extensions
            .as_mut()
            .unwrap()
            .replication
            .as_mut()
            .unwrap()
            .streams[0]
            .recovery_algorithm = ReplicationRecoveryAlgorithm::Vector8021cb2017;
        n.ethernet
            .directions
            .iter_mut()
            .find(|d| d.from_port == "Net.b.tx_e")
            .unwrap()
            .delay_ps = 2_000_000_000;
        let snapshot = crate::runtime::simulate(&p).unwrap();
        assert_ne!(snapshot.common.termination, "execution_failed");
        let records = &snapshot.registered.as_ref().unwrap().model_records;
        let origin = records
            .values()
            .find(|r| r.schema.name == "dir.redundancy.origin")
            .unwrap();
        assert_eq!(origin.data["completed_count"], "2");
        assert_eq!(
            origin.data["continuation_rounds"].as_array().unwrap().len(),
            2
        );
        let rows = snapshot.ethernet.as_ref().unwrap();
        assert_eq!(
            rows.receptions
                .iter()
                .filter(|r| r.device == "Net.d" && r.status == "received")
                .count(),
            2
        );
        assert!(
            records
                .values()
                .filter(|r| r.schema.name == "dir.redundancy.timeout")
                .count()
                >= 2
        );
    }
}

#[cfg(test)]
#[path = "forwarding_composition_tests.rs"]
mod forwarding_composition_tests;
