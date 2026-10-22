//! Source-certified sampled transport. No observations are promoted to a
//! future delay bound: the immutable envelope must cover the live certificate.
use super::*;
use crate::types::network::configuration::{RawMergeDependency, RawSourceCertificate};
use sha2::{Digest, Sha256};

impl NetworkState {
    fn qcc_raw_certificates(
        &self,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<BTreeMap<usize, RawSourceCertificate>> {
        let mut result = BTreeMap::new();
        let Some(p) = &self.prepared.qcc else {
            return Ok(result);
        };
        let attachments: BTreeSet<_> = p
            .streams
            .iter()
            .flat_map(|s| &s.demands)
            .filter_map(|d| d.merge_control.as_ref()?.raw_attachment)
            .collect();
        let Some(merge) = d.mac_merge.as_ref().or(self.mac_merge.as_ref()) else {
            return Ok(result);
        };
        for index in attachments {
            let a = &self.prepared.phy_attachments[index];
            let Some(envelope) = a.config.medium.raw_timing_envelope() else {
                continue;
            };
            let Some(physical) = d.phy.as_ref().unwrap_or(&self.phy)[index]
                .raw_timing_certificate()
                .map_err(error)?
            else {
                continue;
            };
            let config_hash = |bytes: Vec<u8>| format!("{:x}", Sha256::digest(bytes));
            if physical.medium_config_sha256
                != config_hash(
                    serde_json::to_vec(&a.config.medium).map_err(|e| error(e.to_string()))?,
                )
                || physical.attachment_config_sha256.as_ref()
                    != Some(&config_hash(
                        serde_json::to_vec(&a.config).map_err(|e| error(e.to_string()))?,
                    ))
                || physical.attachment_origin_ps != a.config.origin_ps
                || physical.grid != envelope.grid
                || !physical.bilateral_ready
                || !physical.initial_idle_verified
                || physical.directions.len() != 2
                || physical.physical_epoch_start_samples.len() != 2
                || physical.endpoints
                    != a.config
                        .medium
                        .endpoints
                        .iter()
                        .map(|e| e.id.clone())
                        .collect::<Vec<_>>()
                || physical.directions.iter().any(|direction| {
                    envelope
                        .directions
                        .iter()
                        .find(|bound| {
                            bound.source_endpoint == direction.source_endpoint
                                && bound.receiver_endpoint == direction.receiver_endpoint
                        })
                        .is_none_or(|bound| {
                            direction.pmax_samples > bound.pmax_samples_upper_bound
                                || direction.callback_rounding_samples
                                    > bound.callback_rounding_samples
                        })
                })
            {
                continue;
            }
            let mut dependencies = Vec::new();
            for &direction in &a.directions {
                let port = &merge.ports[direction];
                let Some(peer) = &port.verify.peer else {
                    continue;
                };
                let Some(expires) = port.capability_expires.filter(|expiry| *expiry > now) else {
                    continue;
                };
                let Some(packet) = &port.capability_packet else {
                    continue;
                };
                let Some(verify_packet) = &port.last_verify_packet else {
                    continue;
                };
                let Some(respond_packet) = &port.last_response_packet else {
                    continue;
                };
                let reverse = merge.identity.reverse[direction];
                let Some(raw) = &packet.raw else {
                    continue;
                };
                let reverse_port = &self.prepared.ethernet.directions[reverse].from_port;
                let Some(endpoint) = a
                    .config
                    .physical_ports
                    .iter()
                    .find(|p| &p.tx_port == reverse_port)
                else {
                    continue;
                };
                let control_valid = |packet: &mac_merge::integration::Packet, kind| {
                    matches!(packet.origin, mac_merge::integration::Origin::Control(k) if k == kind)
                        && packet.direction == reverse
                        && packet.epoch == merge.ports[reverse].tx.epoch
                        && packet.arrival_done
                        && packet.received.is_some()
                        && packet.invalidated_ps.is_none()
                        && packet.raw.as_ref().is_some_and(|raw| {
                            raw.attachment == index
                                && raw.source_endpoint == endpoint.endpoint
                                && raw.decoded_at_sample.is_some()
                                && raw.receive_cut_sample.is_some()
                        })
                };
                if !control_valid(verify_packet, mac_merge::PacketKind::Verify)
                    || !control_valid(respond_packet, mac_merge::PacketKind::Respond)
                    || !port.link_up
                    || !port.verify.p_active()
                    || !peer.support
                    || !peer.enabled
                    || merge.identity.ports[reverse]
                        .as_ref()
                        .is_none_or(|cfg| cfg.source_mac != peer.source_mac)
                    || peer.port_id != [vec![7], reverse_port.as_bytes().to_vec()].concat()
                    || peer
                        .received_at
                        .checked_add(u64::from(merge.identity.lldp_ttl_seconds) * 1_000_000_000_000)
                        != Some(expires)
                    || peer.ingress != direction
                    || peer.link_epoch != port.tx.epoch
                    || port.verify.epoch != port.tx.epoch
                    || packet.direction != reverse
                    || packet.epoch != merge.ports[reverse].tx.epoch
                    || packet.received.is_none()
                    || !packet.arrival_done
                    || packet.invalidated_ps.is_some()
                    || raw.attachment != index
                    || raw.source_endpoint != endpoint.endpoint
                    || peer.lldp_evidence_id.is_empty()
                {
                    continue;
                }
                dependencies.push(RawMergeDependency {
                    direction,
                    epoch: port.tx.epoch,
                    control_generation: port.verify.control_generation,
                    source_mac: peer.source_mac,
                    port_id: peer.port_id.clone(),
                    add_frag_size: peer.add_frag_size,
                    capability_evidence: peer.lldp_evidence_id.clone(),
                    received_verify_packet: verify_packet.id,
                    received_respond_packet: respond_packet.id,
                    capability_generation: port.capability_generation,
                    capability_expires_ps: expires,
                    enabled: port.verify.p_enable() && peer.enabled,
                    verified: port.verify.verified,
                });
            }
            dependencies.sort_by_key(|d| d.direction);
            let certificate = RawSourceCertificate {
                attachment: index,
                physical,
                merge: dependencies,
            };
            if certificate.valid_at(now) {
                result.insert(index, certificate);
            }
        }
        Ok(result)
    }

    pub(in crate::runtime::network) fn qcc_refresh_raw_sources(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(p) = &self.prepared.qcc else {
            return Ok(());
        };
        let certificates = self.qcc_raw_certificates(d, c.now())?;
        let mut state = self.qcc_state(d).unwrap().clone();
        let effects = state
            .raw_sources(p, c.now(), certificates)
            .map_err(|e| error(format!("Qcc raw source dependency: {e:?}")))?;
        d.qcc = Some(state);
        self.qcc_effects(effects, d, c)
    }

    pub(super) fn qcc_raw_plan_live(
        &self,
        plan: &ConfigurationPlan,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<bool> {
        let required: BTreeSet<_> = plan
            .streams
            .iter()
            .flat_map(|i| &self.prepared.qcc.as_ref().unwrap().streams[*i].demands)
            .filter_map(|d| d.merge_control.as_ref()?.raw_attachment)
            .collect();
        if required != plan.raw_certificates.keys().copied().collect() {
            return Ok(false);
        }
        if required.is_empty() {
            return Ok(true);
        }
        let current = self.qcc_raw_certificates(d, now)?;
        Ok(plan.raw_certificates.iter().all(|(a, certificate)| {
            current
                .get(a)
                .is_some_and(|live| certificate.same_epoch(live) && live.valid_at(now))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "actual sampled train and private fault injection; use release"]
    fn qcc_actual_idle_phy_loss_cancels_pending_and_withdraws_installed_proofs() {
        let p = crate::prepare(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/network-composite/qcc-raw/model.ini"),
        )
        .unwrap();
        let mut engine =
            crate::runtime::registered::Engine::new(std::sync::Arc::new(p.clone())).unwrap();
        engine.initialize().unwrap();
        let evidence = std::env::var_os("DIR_QCC_EVIDENCE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("raw-qcc-idle-loss-{}", std::process::id()));
        std::fs::create_dir_all(&evidence).unwrap();
        for (label, cut) in [("pending", 6_040_000_000), ("installed", 6_106_000_000)] {
            while engine.snapshot.common.last_event_time_ps.unwrap_or(0) < cut {
                assert!(engine.step(), "actual source prefix remains executable");
            }
            engine.with_network_state_for_test(|state| {
            let before = state.checkpoint();
            std::fs::write(evidence.join(format!("{label}-before.json")), serde_json::to_vec_pretty(&before).unwrap()).unwrap();
            if label == "pending" {
                assert!(!before["qcc"]["pending"]["plan"]["raw_certificates"]["0"].is_null());
            } else {
                assert_eq!(before["qcc"]["active_valid"], true);
                assert!(!before["qcc"]["active"]["raw_certificates"]["0"].is_null());
            }
            assert!(
                state
                    .mac_merge
                    .as_ref()
                    .unwrap()
                    .ports
                    .iter()
                    .all(|port| port.busy.is_none()),
                "fault must occur while idle"
            );
            assert!(state.mac_merge.as_ref().unwrap().packets.values().all(|packet|packet.arrival_done), "all packets must drain before idle loss");
            let old_epochs: Vec<_> = state
                .mac_merge
                .as_ref()
                .unwrap()
                .ports
                .iter()
                .map(|port| port.tx.epoch)
                .collect();
            let mut d = state.delta();
            d.phy = Some(state.phy.clone());
            let phy = &mut d.phy.as_mut().unwrap()[0];
            phy.set_gigabit_mdi_disconnected(true).unwrap();
            let mut physical_effects = Vec::new();
            for _ in 0..1000 {
                physical_effects.extend(phy.advance_samples(256).unwrap());
                if physical_effects.iter().any(|effect| {
                    matches!(
                        effect,
                        crate::runtime::ethernet::phy::attachment::AttachmentEffect::Physical {
                            effect: crate::runtime::ethernet::phy::medium::PhyEffect::TxReady {
                                ready: false,
                                ..
                            }
                        }
                    )
                }) {
                    break;
                }
            }
            assert!(
                physical_effects.iter().any(|effect| matches!(
                    effect,
                    crate::runtime::ethernet::phy::attachment::AttachmentEffect::Physical {
                        effect: crate::runtime::ethernet::phy::medium::PhyEffect::TxReady {
                            ready: false,
                            ..
                        }
                    }
                )),
                "actual idle voltage loss must withdraw readiness"
            );
            let sample = phy.next_sample_index().saturating_sub(1);
            let now = phy.sample_time_ps(sample).unwrap();
            let reg = p.registered.as_ref().unwrap();
            let channels = BTreeMap::new();
            let live = BTreeSet::new();
            let mut next = 100_000;
            let mut batch = crate::registry::EffectBatch::default();
            let mut c = Context {
                now,
                config: reg.models.iter().find(|m| m.id == "@network").unwrap(),
                registry: &reg.registry,
                profile: reg.registry.profile(&p.common.profile).unwrap(),
                channels: &channels,
                channel_configs: &[],
                live_timers: &live,
                executing_timer: None,
                next_id: &mut next,
                batch: &mut batch,
            };
            state
                .phy_effects(0, sample, physical_effects, &mut d, &mut c)
                .unwrap();
            let after = json!({"qcc":state.qcc_state(&d),"mac_merge":d.mac_merge.as_ref().or(state.mac_merge.as_ref()),"effect_count":batch.effects.len()});
            assert_eq!(after["qcc"]["active_valid"], false);
            assert!(after["qcc"]["pending"].is_null());
            assert_eq!(after["qcc"]["dependency_raw"], json!({}));
            let ports = &d.mac_merge.as_ref().unwrap().ports;
            for (i, port) in ports.iter().enumerate() {
                assert_eq!(
                    port.tx.epoch,
                    old_epochs[i] + 1,
                    "one bilateral reset per physical withdrawal"
                );
                assert!(port.capability_packet.is_none() && port.capability_expires.is_none());
                assert!(!port.verify.p_active());
            }
            d.phy.as_mut().unwrap()[0].set_gigabit_mdi_disconnected(false).unwrap();
            assert!(d.phy.as_ref().unwrap()[0].raw_timing_certificate().unwrap().is_none());
            assert!(
                state
                    .qcc_raw_certificates(&d, now)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                !state.qcc_state(&d).unwrap().active_valid,
                "reconnect cannot resurrect the old guarantee"
            );
            std::fs::write(
                evidence.join(format!("{label}-after.json")),
                serde_json::to_vec_pretty(&after).unwrap(),
            )
            .unwrap();
            }).expect("actual committed network owner");
        }
        if std::env::var_os("DIR_QCC_EVIDENCE").is_none() {
            std::fs::remove_dir_all(evidence).unwrap();
        }
    }
}
