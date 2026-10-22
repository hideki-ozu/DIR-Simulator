//! Private, versioned snapshots of owned network state. Prepared inputs are rebound.
use super::*;
use crate::types::network::ReplicationRecoveryAlgorithm;
use serde::{Deserializer, Serializer, de::DeserializeOwned};

pub(crate) fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| error(format!("invalid network checkpoint: {e}")))
}

// Shared ordered-map codec preserves the existing checkpoint representation.
pub(crate) use crate::runtime::wire::codec::ordered_map as map;

macro_rules! fields {
    ($state:ty, $value:ident; $($field:ident),+; $($map:ident),* $(,)?) => {
        impl $state {
            pub(crate) fn checkpoint_fields(&self) -> serde_json::Value {
                serde_json::json!({$(stringify!($field): &self.$field),+, $(stringify!($map): self.$map.iter().collect::<Vec<_>>()),*})
            }
            pub(crate) fn restore_fields($value: &serde_json::Value, mut state: Self) -> std::result::Result<Self, $crate::types::Diagnostic> {
                let object = $value.as_object().ok_or_else(|| $crate::types::Diagnostic::execution("invalid network checkpoint object"))?;
                let fields = [$(stringify!($field)),+, $(stringify!($map)),*];
                if object.len() != fields.len() || fields.iter().any(|f| !object.contains_key(*f)) {
                    return Err($crate::types::Diagnostic::execution("network checkpoint fields differ"));
                }
                $(state.$field = $crate::runtime::network::checkpoint::decode($value[stringify!($field)].clone())?;)+
                $(state.$map = $crate::runtime::network::checkpoint::decode_map($value[stringify!($map)].clone())?;)*
                Ok(state)
            }
        }
    };
}
pub(crate) use fields;

pub(crate) mod decimal128 {
    use super::*;
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        let n: u128 = s.parse().map_err(serde::de::Error::custom)?;
        if s != n.to_string() {
            return Err(serde::de::Error::custom("noncanonical u128"));
        }
        Ok(n)
    }
}
pub(crate) mod optional128 {
    use super::*;
    pub fn serialize<S: Serializer>(
        v: &Option<u128>,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        v.map(|v| v.to_string()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Option<u128>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                let n: u128 = s.parse().map_err(serde::de::Error::custom)?;
                if s != n.to_string() {
                    return Err(serde::de::Error::custom("noncanonical u128"));
                }
                Ok(n)
            })
            .transpose()
    }
}
pub(crate) mod credit {
    use super::*;
    use crate::types::ethernet::tsn::Credit;
    pub fn serialize<S: Serializer>(v: &Credit, s: S) -> std::result::Result<S::Ok, S::Error> {
        (v.negative, v.magnitude.to_string()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Credit, D::Error> {
        let (negative, s) = <(bool, String)>::deserialize(d)?;
        let magnitude: u128 = s.parse().map_err(serde::de::Error::custom)?;
        if s != magnitude.to_string() || negative && magnitude == 0 {
            return Err(serde::de::Error::custom("noncanonical credit"));
        }
        Ok(Credit {
            negative,
            magnitude,
        })
    }
}
pub(crate) mod records {
    use super::*;
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Vec<(&'static str, String, Value)>, D::Error> {
        Vec::<(String, String, Value)>::deserialize(d)?
            .into_iter()
            .map(|(schema, id, value)| {
                let schema = match schema.as_str() {
                    "dir.redundancy.branch" => "dir.redundancy.branch",
                    "dir.redundancy.origin" => "dir.redundancy.origin",
                    "dir.redundancy.recovery" => "dir.redundancy.recovery",
                    "dir.redundancy.timeout" => "dir.redundancy.timeout",
                    "dir.preemption.fragment" => "dir.preemption.fragment",
                    "dir.preemption.gap" => "dir.preemption.gap",
                    "dir.preemption.delivery" => "dir.preemption.delivery",
                    "dir.preemption.decision" => "dir.preemption.decision",
                    _ => return Err(serde::de::Error::custom("unknown network record schema")),
                };
                Ok((schema, id, value))
            })
            .collect()
    }
}

pub(crate) fn decode_map<K: DeserializeOwned + Ord, V: DeserializeOwned>(
    value: Value,
) -> Result<BTreeMap<K, V>> {
    let entries: Vec<(K, V)> = decode(value)?;
    if entries.windows(2).any(|p| p[0].0 >= p[1].0) {
        return Err(error("unordered network map"));
    }
    Ok(entries.into_iter().collect())
}

fields!(NetworkState, value; cursors, frames, transfers, receptions, queues, tsn_wakes, due, control, control_generation, record_sequence, next_visit, future_generation, pending_direct; );
impl NetworkState {
    pub(crate) fn checkpoint(&self) -> Value {
        let component_wire =
            self.prepared.provider_l2.is_some()
                && self.prepared.wire.as_ref().is_some_and(|wire| {
                    wire.adapters.iter().any(|a| a.component_binding.is_some())
                });
        let mut value = json!({"version":1,"core":self.checkpoint_fields(),
            "dynamic":self.dynamic.as_ref().map(DynamicState::checkpoint_fields),
            "tsn":self.tsn.as_ref().map(TsnState::checkpoint_fields),
            "bridge":self.bridge.as_ref().map(BridgeState::checkpoint_fields),
            "replication":self.replication,"preemption":self.preemption,
            "wire":self.wire.as_ref().map(|host| host.checkpoint(component_wire))});
        if let Some(clocks) = &self.clocks {
            value["version"] = json!(2);
            value["clocks"] = json!(clocks);
            value["clock_cursor"] = json!(self.clock_cursor);
        }
        if !self.phy.is_empty() {
            value["version"] = json!(3);
            value["phy"] = json!(
                self.phy
                    .iter()
                    .map(|a| a.checkpoint().expect("validated live PHY attachment"))
                    .collect::<Vec<_>>()
            );
            value["phy_wakes"] = json!(self.phy_wakes);
            value["phy_arrivals"] = json!(self.phy_arrivals.iter().collect::<Vec<_>>());
            value["phy_admitted"] = json!(self.phy_admitted.iter().collect::<Vec<_>>());
        }
        if let Some(active) = &self.multicast_active {
            value["version"] = json!(4);
            value["multicast_active"] = json!(active);
            value["multicast_cursor"] = json!(self.multicast_cursor);
            value["multicast_wake"] = json!(self.multicast_wake);
            value["multicast_history"] = json!(self.multicast_history);
        }
        if let Some(qcc) = &self.qcc {
            value["version"] = json!(5);
            value["qcc"] = json!(qcc);
            value["qcc_sources"] = json!(self.qcc_sources.iter().collect::<Vec<_>>());
        }
        if !self.prepared.shared_lans.is_empty() {
            value["version"] = json!(6);
            value["segment_active"] = json!(self.segment_active);
            value["segment_arrivals"] = json!(self.segment_arrivals.iter().collect::<Vec<_>>());
        }
        if let Some(fdb) = &self.provider_fdb {
            value["version"] = json!(7);
            value["provider_fdb"] = json!(fdb);
        }
        if self.multicast_active.is_some() {
            value["version"] = json!(8);
        }
        if component_wire {
            value["version"] = json!(9);
        }
        if self
            .replication
            .as_ref()
            .is_some_and(|r| r.distributed_identity.is_some())
        {
            value["version"] = json!(10);
        }
        if let Some(state) = &self.mac_merge {
            value["version"] = json!(11);
            value["mac_merge"] = json!(state);
        }
        if self
            .prepared
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .is_some_and(|r| {
                r.streams
                    .iter()
                    .any(|s| s.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017)
            })
        {
            value["version"] = json!(if self.mac_merge.is_some() { 13 } else { 12 });
        }
        if let Some(state) = &self.gptp {
            value["version"] = json!(14);
            value["gptp"] = json!(state);
        }
        if self
            .prepared
            .phy_attachments
            .iter()
            .any(|p| !p.config.physical_ports.is_empty())
        {
            value["version"] = json!(15);
        }
        if self.prepared.has_gateway_replication() {
            value["version"] = json!(16);
        }
        if self
            .prepared
            .phy_attachments
            .iter()
            .any(|p| p.config.medium.physical_mode.is_named())
        {
            value["version"] = json!(17);
            value["phy_boundaries"] = json!(self.phy_boundaries);
        }
        if self.prepared.has_phy_mac_service() {
            value["version"] = json!(18);
            value["phy_services"] = json!(self.phy_services.iter().collect::<Vec<_>>());
        }
        if self.prepared.mac_control.is_some() || self.prepared.ethernet.media.is_some() {
            value["version"] = json!(19);
            value["configured_media"] = json!(self.configured_media);
            value["mac_control"] = json!(self.mac_control);
        }
        value
    }
    pub(crate) fn restore_checkpoint(
        p: Arc<PreparedNetwork>,
        value: &Value,
        horizon: u64,
    ) -> Result<Self> {
        // Raw ingress bytes/markers are authenticated by the existing frozen
        // Engine-prefix replay and adopted before this native ideal-MAC route.
        if p.phy_attachments
            .iter()
            .any(|a| a.config.medium.gigabit_raw_merge)
        {
            return Err(error(
                "raw Merge restore requires exact frozen-input Engine prefix",
            ));
        }
        let with_media_pause = p.mac_control.is_some() || p.ethernet.media.is_some();
        let with_clocks = p.clocks.is_some();
        let with_phy = !p.phy_attachments.is_empty();
        let with_active = p.multicast_active.is_some();
        let with_qcc = p.qcc.is_some();
        let with_segments = !p.shared_lans.is_empty();
        let with_provider = p.provider_l2.is_some();
        let with_component_wire = with_provider
            && p.wire
                .as_ref()
                .is_some_and(|wire| wire.adapters.iter().any(|a| a.component_binding.is_some()));
        let with_distributed = p
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .is_some_and(|r| distributed::prepared_identity(r).is_some());
        let with_mac_merge = p.extensions.as_ref().is_some_and(|e| e.mac_merge.is_some());
        let with_vector = p
            .extensions
            .as_ref()
            .and_then(|e| e.replication.as_ref())
            .is_some_and(|r| {
                r.streams
                    .iter()
                    .any(|s| s.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017)
            });
        let with_gptp = p.gptp.is_some();
        let with_phy_service = p.has_phy_mac_service();
        let with_named_phy = p
            .phy_attachments
            .iter()
            .any(|p| !p.config.physical_ports.is_empty());
        if value["version"]
            != if with_media_pause {
                19
            } else if with_phy_service {
                18
            } else if with_named_phy {
                17
            } else if p.has_gateway_replication() {
                16
            } else if with_gptp {
                14
            } else if with_vector && with_mac_merge {
                13
            } else if with_vector {
                12
            } else if with_mac_merge {
                11
            } else if with_distributed {
                10
            } else if with_component_wire {
                9
            } else if with_active {
                8
            } else if with_provider {
                7
            } else if with_segments {
                6
            } else if with_qcc {
                5
            } else if with_phy {
                3
            } else if with_clocks {
                2
            } else {
                1
            }
            || value.as_object().is_none_or(|o| {
                o.len()
                    != 8 + if with_media_pause { 2 } else { 0 }
                        + if with_gptp { 1 } else { 0 }
                        + if with_mac_merge { 1 } else { 0 }
                        + if with_clocks { 2 } else { 0 }
                        + if with_phy { 4 } else { 0 }
                        + if with_active { 4 } else { 0 }
                        + if with_qcc { 2 } else { 0 }
                        + if with_segments { 2 } else { 0 }
                        + if with_provider { 1 } else { 0 }
                        + if with_named_phy { 1 } else { 0 }
                        + if with_phy_service { 1 } else { 0 }
            })
        {
            return Err(error("unsupported network checkpoint version/fields"));
        }
        let mut state = Self::restore_fields(&value["core"], Self::new(p)?)?;
        if with_media_pause {
            state.configured_media = decode(value["configured_media"].clone())?;
            state.mac_control = decode(value["mac_control"].clone())?;
        }
        if with_phy_service {
            state.phy_services = decode_map(value["phy_services"].clone())?;
        }
        if with_gptp {
            state.gptp = Some(decode(value["gptp"].clone())?);
        }
        if with_mac_merge {
            state.mac_merge = Some(decode(value["mac_merge"].clone())?);
        }
        if with_clocks {
            state.clocks = Some(decode(value["clocks"].clone())?);
            state.clock_cursor = decode(value["clock_cursor"].clone())?;
        }
        if with_phy {
            let saved: Vec<Vec<u8>> = decode(value["phy"].clone())?;
            if saved.len() != state.prepared.phy_attachments.len() {
                return Err(error("network checkpoint PHY count differs"));
            }
            state.phy = state
                .prepared
                .phy_attachments
                .iter()
                .zip(saved)
                .map(|(p, bytes)| {
                    crate::runtime::ethernet::phy::attachment::AttachedMedium::restore(
                        &p.config, &bytes,
                    )
                    .map_err(error)
                })
                .collect::<Result<Vec<_>>>()?;
            state.phy_wakes = decode(value["phy_wakes"].clone())?;
            state.phy_arrivals = decode_map(value["phy_arrivals"].clone())?;
            state.phy_admitted = decode_map(value["phy_admitted"].clone())?;
        }
        if with_named_phy {
            state.phy_boundaries = decode(value["phy_boundaries"].clone())?;
        }
        if with_active {
            state.multicast_active = Some(decode(value["multicast_active"].clone())?);
            state.multicast_cursor = decode(value["multicast_cursor"].clone())?;
            state.multicast_wake = decode(value["multicast_wake"].clone())?;
            state.multicast_history = decode(value["multicast_history"].clone())?;
        }
        if with_qcc {
            state.qcc = Some(decode(value["qcc"].clone())?);
            state.qcc_sources = decode_map(value["qcc_sources"].clone())?;
        }
        if with_segments {
            state.segment_active = decode(value["segment_active"].clone())?;
            state.segment_arrivals = decode_map(value["segment_arrivals"].clone())?;
        }
        if with_provider {
            state.provider_fdb = Some(decode(value["provider_fdb"].clone())?);
        }
        macro_rules! restore {
            ($field:ident,$ty:ty) => {
                state.$field = match (state.$field.take(), value.get(stringify!($field))) {
                    (Some(old), Some(saved)) if !saved.is_null() => {
                        Some(<$ty>::restore_fields(saved, old)?)
                    }
                    (None, Some(saved)) if saved.is_null() => None,
                    _ => {
                        return Err(error(concat!(
                            "network checkpoint backend mismatch: ",
                            stringify!($field)
                        )))
                    }
                };
            };
        }
        if let Some(dynamic) = &state.dynamic {
            let frozen = dynamic.checkpoint_fields();
            for key in [
                "wire_enabled",
                "wire_tree_bridges",
                "wire_registration_ports",
            ] {
                if value["dynamic"][key] != frozen[key] {
                    return Err(error("network checkpoint wire policy ownership differs"));
                }
            }
        }
        if let Some(bridge) = &state.bridge {
            if value["bridge"]["synthetic_policing"]
                != bridge.checkpoint_fields()["synthetic_policing"]
            {
                return Err(error("network checkpoint policing mode differs"));
            }
        }
        restore!(dynamic, DynamicState);
        restore!(tsn, TsnState);
        restore!(bridge, BridgeState);
        if state.replication.is_some() == value["replication"].is_null()
            || state.preemption.is_some() == value["preemption"].is_null()
        {
            return Err(error("network checkpoint extension mismatch"));
        }
        state.replication = decode(value["replication"].clone())?;
        state.preemption = decode(value["preemption"].clone())?;
        state.wire = match (state.wire.take(), &value["wire"]) {
            (Some(old), saved) if !saved.is_null() => Some(WireHost::restore_checkpoint(
                saved,
                old,
                horizon,
                with_component_wire,
            )?),
            (None, saved) if saved.is_null() => None,
            _ => return Err(error("wire checkpoint backend mismatch")),
        };
        state.validate_checkpoint(horizon)?;
        Ok(state)
    }
    fn validate_checkpoint(&mut self, horizon: u64) -> Result<()> {
        use std::collections::BTreeSet;
        let p = &self.prepared;
        self.validate_media_pause_checkpoint(horizon)?;
        if let Some(topology) = &p.provider_l2 {
            let fdb = self
                .provider_fdb
                .as_ref()
                .ok_or_else(|| error("network checkpoint provider FDB missing"))?;
            let dynamic = p
                .dynamic
                .as_ref()
                .ok_or_else(|| error("network checkpoint provider dynamic limits missing"))?;
            if fdb.entries().len() > dynamic.limits.mac_entries {
                return Err(error("network checkpoint provider FDB exceeds capacity"));
            }
            for ((component, vid, mac), entry) in fdb.entries() {
                let bound = topology
                    .ports
                    .get(&entry.port)
                    .ok_or_else(|| error("network checkpoint provider FDB port unknown"))?;
                if bound.component != *component
                    || !bound.member_vids.contains(vid)
                    || mac.len() != 17
                    || mac.bytes().any(|b| !(b.is_ascii_hexdigit() || b == b':'))
                    || entry.learned_ps > horizon
                    || entry.expires_ps != entry.learned_ps.saturating_add(dynamic.mac_age_ps)
                {
                    return Err(error(
                        "network checkpoint provider FDB component/VLAN/MAC mismatch",
                    ));
                }
            }
        } else if self.provider_fdb.is_some() {
            return Err(error("network checkpoint unexpected provider FDB"));
        }
        if let Some(config) = &p.clocks {
            let Some(clocks) = &self.clocks else {
                return Err(error("network checkpoint clocks absent"));
            };
            if clocks.len() != p.ethernet.devices.len()
                || self.clock_cursor > config.adjustments.len()
            {
                return Err(error("network checkpoint clock dimensions/cursor"));
            }
            let mut expected = config
                .initial
                .iter()
                .map(|v| {
                    clock::NodeClock::new(0, v.offset_ps, v.rate_num, v.rate_den)
                        .map_err(|e| error(e.to_string()))
                })
                .collect::<Result<Vec<_>>>()?;
            for adjustment in config.adjustments.iter().take(self.clock_cursor) {
                if adjustment.at_ps > horizon {
                    return Err(error("network checkpoint future clock adjustment"));
                }
                match adjustment.action {
                    PreparedClockAction::StepBy { offset_ps } => {
                        expected[adjustment.node].step_by(adjustment.at_ps, offset_ps)
                    }
                    PreparedClockAction::Slew { rate_num, rate_den } => {
                        expected[adjustment.node].slew(adjustment.at_ps, rate_num, rate_den)
                    }
                }
                .map_err(|e| error(e.to_string()))?;
            }
            if self.prepared.gptp.is_none()
                && (*clocks != expected
                    || config
                        .adjustments
                        .get(self.clock_cursor)
                        .is_some_and(|next| next.at_ps < horizon))
            {
                return Err(error(
                    "network checkpoint clock state differs from adjustment prefix",
                ));
            }
            if self.prepared.gptp.is_some() {
                self.validate_gptp_checkpoint(horizon, &expected)?;
            }
        } else if self.clocks.is_some() || self.clock_cursor != 0 {
            return Err(error("network checkpoint unexpected clocks"));
        }
        if self.phy.len() != p.phy_attachments.len() || self.phy_wakes.len() != self.phy.len() {
            return Err(error("network checkpoint PHY attachment dimensions"));
        }
        self.validate_phy_boundaries(horizon)?;
        if let Some(config) = p.multicast_active.as_ref() {
            let actor = self
                .multicast_active
                .as_ref()
                .ok_or_else(|| error("network checkpoint active multicast missing"))?;
            actor.validate(config, horizon).map_err(error)?;
            actor
                .validate_change_prefix(config, self.multicast_cursor)
                .map_err(error)?;
            if self.multicast_cursor > config.changes.len()
                || config
                    .changes
                    .iter()
                    .take(self.multicast_cursor)
                    .any(|c| c.at_ps > horizon)
                || config
                    .changes
                    .get(self.multicast_cursor)
                    .is_some_and(|c| c.at_ps < horizon)
            {
                return Err(error("network checkpoint active multicast change cursor"));
            }
            let wakes: Vec<_> = self
                .due
                .iter()
                .filter(|d| matches!(d.kind, Completion::MulticastTimer))
                .map(|d| d.at)
                .collect();
            if wakes != self.multicast_wake.into_iter().collect::<Vec<_>>() {
                return Err(error(
                    "network checkpoint active multicast wake reservation",
                ));
            }
            if self.multicast_wake
                != actor.checkpoint_deadline(config, horizon, self.multicast_cursor)
            {
                return Err(error(
                    "network checkpoint active multicast causal wake differs",
                ));
            }
            self.validate_multicast_history(horizon)?;
        } else if self.multicast_active.is_some()
            || self.multicast_cursor != 0
            || self.multicast_wake.is_some()
            || !self.multicast_history.is_empty()
        {
            return Err(error("network checkpoint unexpected active multicast"));
        }
        for (index, medium) in self.phy.iter().enumerate() {
            medium.validate().map_err(error)?;
            let identities = self
                .phy_admitted
                .iter()
                .filter(|(_, owner)| **owner == index)
                .map(|(transfer, _)| {
                    let row = self
                        .transfers
                        .get(*transfer)
                        .ok_or_else(|| error("network checkpoint PHY transfer missing"))?;
                    let direction = p
                        .ethernet
                        .directions
                        .get(row.direction)
                        .ok_or_else(|| error("network checkpoint PHY direction missing"))?;
                    if !p.phy_attachments[index].directions.contains(&row.direction) {
                        return Err(error(
                            "network checkpoint PHY transfer bound to wrong attachment",
                        ));
                    }
                    crate::runtime::ethernet::phy::attachment::AttachedMedium::identity(
                        &row.row.transfer_id,
                        &direction.from_port,
                        &row.row.wire,
                        medium.config().medium.maximum_mac_bytes,
                    )
                    .map_err(error)
                })
                .collect::<Result<Vec<_>>>()?;
            medium
                .validate_external_admissions(&identities)
                .map_err(error)?;
            if !medium.config().medium.physical_mode.is_named()
                && (self.phy_wakes[index] != Some(medium.next_sample_index())
                    || medium.next_event_ps().map_err(error)? < horizon)
            {
                return Err(error("network checkpoint PHY sample wake differs"));
            }
        }
        self.validate_phy_ingress_checkpoint(horizon)?;
        for (transfer, at) in &self.phy_arrivals {
            if *at < horizon
                || !self.phy_admitted.contains_key(transfer)
                || self
                    .transfers
                    .get(*transfer)
                    .is_none_or(|t| t.row.arrival_ps.is_some())
            {
                return Err(error("network checkpoint PHY arrival reservation differs"));
            }
        }
        if self.segment_active.len() != p.shared_lans.len() {
            return Err(error(
                "network checkpoint shared LAN reservation dimensions",
            ));
        }
        for (segment, reservation) in self.segment_active.iter().enumerate() {
            let Some(reservation) = reservation else {
                continue;
            };
            if !p.shared_lans[segment]
                .members
                .contains(&reservation.direction)
                || reservation.sof_ps > horizon
                || reservation.release_ps < horizon
                || reservation.eof_ps <= reservation.sof_ps
                || reservation.release_ps < reservation.eof_ps
            {
                return Err(error("network checkpoint shared LAN active reservation"));
            }
            match &reservation.owner {
                SegmentOwner::Data(transfer) => {
                    let row = self
                        .transfers
                        .get(*transfer)
                        .ok_or_else(|| error("network checkpoint shared LAN transfer missing"))?;
                    if row.direction != reservation.direction
                        || row.row.sof_ps != Some(reservation.sof_ps)
                        || row.row.planned_eof_ps != Some(reservation.eof_ps)
                        || row.row.planned_release_ps != Some(reservation.release_ps)
                        || row.row.release_ps.is_some()
                        || self.queues[reservation.direction].active != Some(*transfer)
                    {
                        return Err(error(
                            "network checkpoint shared LAN transfer reservation differs",
                        ));
                    }
                }
                SegmentOwner::Control(id) => {
                    let host = self.wire.as_ref().ok_or_else(|| {
                        error("network checkpoint shared LAN control host missing")
                    })?;
                    let tx = host
                        .tx
                        .get(id)
                        .ok_or_else(|| error("network checkpoint shared LAN control TX missing"))?;
                    if host.active[reservation.direction].as_deref() != Some(id)
                        || tx.sof != Some(reservation.sof_ps)
                        || tx.planned_eof != Some(reservation.eof_ps)
                        || tx.planned_release != Some(reservation.release_ps)
                        || tx.release.is_some()
                    {
                        return Err(error(
                            "network checkpoint shared LAN control reservation differs",
                        ));
                    }
                }
            }
        }
        for ((transfer, receiver), at) in &self.segment_arrivals {
            let row = self
                .transfers
                .get(*transfer)
                .ok_or_else(|| error("network checkpoint shared LAN arrival transfer missing"))?;
            if self
                .segment_for(row.direction)
                .is_none_or(|segment| !p.shared_lans[segment].members.contains(receiver))
                || *receiver == row.direction
                || *at < horizon
                || row.row.eof_ps.is_none()
                || row.row.planned_arrival_ps != Some(*at)
            {
                return Err(error(
                    "network checkpoint shared LAN arrival reservation differs",
                ));
            }
        }
        if self.cursors.len() != p.ethernet.generators.len()
            || self.queues.len() != p.ethernet.directions.len()
            || self.tsn_wakes.len() != p.tsn.as_ref().map_or(0, |t| t.outputs.len())
            || self.next_visit == 0
        {
            return Err(error("network checkpoint dimensions/cursor"));
        }
        for (i, cursor) in self.cursors.iter().enumerate() {
            let generator = &p.ethernet.generators[i];
            if *cursor > 0
                && generator
                    .time(cursor - 1)
                    .is_none_or(|t| t > u128::from(horizon))
                || generator
                    .time(*cursor)
                    .is_some_and(|t| t < u128::from(horizon))
            {
                return Err(error("network checkpoint generation cursor"));
            }
        }
        let mut frame_ids = BTreeSet::new();
        for f in &self.frames {
            if f.generator
                .is_some_and(|i| i >= p.ethernet.generators.len())
                || f.row.priority > 7
                || f.row.generated_ps > horizon
                || !frame_ids.insert(&f.row.frame_id)
            {
                return Err(error("network checkpoint frame reference/time/identity"));
            }
        }
        let mut transfer_ids = BTreeSet::new();
        for t in &self.transfers {
            let f = self
                .frames
                .get(t.frame)
                .ok_or_else(|| error("network checkpoint transfer frame"))?;
            let link = p
                .ethernet
                .directions
                .get(t.direction)
                .ok_or_else(|| error("network checkpoint transfer direction"))?;
            if t.row.priority > 7
                || t.row.frame_id != f.row.frame_id
                || t.row.from_port != link.from_port
                || t.row.to_port != link.to_port
                || t.visit >= self.next_visit
                || !transfer_ids.insert(&t.row.transfer_id)
            {
                return Err(error("network checkpoint transfer owner/identity"));
            }
            if let Some(fragments) = &t.fragments {
                if fragments.completed > t.row.wire.mac_bytes
                    || fragments.partial_mac_bytes
                        > t.row.wire.mac_bytes.saturating_sub(fragments.completed)
                    || fragments.reset_ps.is_some_and(|v| {
                        v > horizon
                            || (t.row.status != "dropped" && t.row.arrival_ps.is_none())
                            || fragments.active_start.is_some()
                            || fragments.active_end.is_some()
                    })
                    || fragments.arrival_retired_ps.is_some_and(|v| {
                        Some(v) != t.row.planned_arrival_ps
                            || v > horizon
                            || fragments.reset_ps.is_none()
                            || t.row.eof_ps.is_none()
                    })
                    || fragments.active_start.is_some_and(|t| t > horizon)
                {
                    return Err(error("network checkpoint fragment offset/time"));
                }
            }
            if let Some(circuit) = &t.circuit {
                if circuit.distributed.is_some() {
                    continue;
                }
                let r = self
                    .replication
                    .as_ref()
                    .ok_or_else(|| error("network checkpoint circuit without replication"))?;
                let Some(branch) = r.branches.get(&circuit.branch) else {
                    if t.row.arrival_ps.is_some() || t.row.drop_reason.is_some() {
                        continue;
                    }
                    return Err(error("network checkpoint live circuit branch"));
                };
                let config = p
                    .extensions
                    .as_ref()
                    .and_then(|e| e.replication.as_ref())
                    .and_then(|r| r.streams.get(branch.stream))
                    .and_then(|s| s.paths.get(branch.path))
                    .ok_or_else(|| error("network checkpoint replication path"))?;
                if config.directions.get(circuit.hop) != Some(&t.direction) {
                    return Err(error("network checkpoint replication hop"));
                }
            }
        }
        let mut reception_ids = BTreeSet::new();
        for r in &self.receptions {
            let t = self
                .transfers
                .get(r.transfer)
                .ok_or_else(|| error("network checkpoint reception transfer"))?;
            if r.row.transfer_id != t.row.transfer_id
                || r.row.frame_id != t.row.frame_id
                || r.row.priority > 7
                || r.visit >= self.next_visit
                || !reception_ids.insert(&r.row.reception_id)
            {
                return Err(error("network checkpoint reception identity"));
            }
        }
        let mut queued = BTreeSet::new();
        for (direction, q) in self.queues.iter().enumerate() {
            for (priority, waiting) in q.waiting.iter().enumerate() {
                let mut bytes = 0u64;
                for i in waiting {
                    let t = self
                        .transfers
                        .get(*i)
                        .ok_or_else(|| error("network checkpoint queue transfer"))?;
                    if t.direction != direction
                        || usize::from(t.row.priority) != priority
                        || !queued.insert(*i)
                    {
                        return Err(error("network checkpoint queue ownership"));
                    }
                    bytes = bytes
                        .checked_add(t.row.wire.mac_bytes)
                        .ok_or_else(|| error("network checkpoint queue bytes overflow"))?;
                }
                if bytes != q.bytes[priority] {
                    return Err(error("network checkpoint queue byte accounting"));
                }
            }
            if let Some(i) = q.active {
                if self
                    .transfers
                    .get(i)
                    .is_none_or(|t| t.direction != direction)
                {
                    return Err(error("network checkpoint active transfer"));
                }
            }
        }
        for due in &self.due {
            if due.at < horizon {
                return Err(error("network checkpoint past completion"));
            }
            self.validate_completion(&due.kind)?;
        }
        if let Some(dynamic) = &self.dynamic {
            dynamic.validate_checkpoint(horizon)?;
            if let Some(wire) = &self.wire {
                dynamic.validate_wire_tree_projection(
                    &wire.protocol.tree_policies(),
                    wire.protocol.is_initialized(),
                )?;
            }
        }
        if let Some(qcc) = &self.qcc {
            self.validate_qcc_topology_history(horizon)?;
            if self.qcc_available(&self.delta())? != qcc.available() {
                return Err(error("Qcc checkpoint topology differs from dynamic policy"));
            }
            if qcc.topology_history().iter().any(|(at, _)| *at > horizon) {
                return Err(error("Qcc checkpoint future topology input"));
            }
        }
        if self.qcc.is_some() {
            self.validate_qcc_sources(horizon)?;
            self.validate_qcc_quiescence_history(horizon)?;
        }
        if let Some(tsn) = &mut self.tsn {
            if let Some(qcc) = &self.qcc {
                let prepared = p
                    .qcc
                    .as_ref()
                    .ok_or_else(|| error("Qcc checkpoint prepared input missing"))?;
                qcc.validate(prepared)
                    .map_err(|e| error(format!("Qcc checkpoint state: {e:?}")))?;
                let history = qcc
                    .installation_history(prepared)
                    .map_err(|e| error(format!("Qcc checkpoint installation history: {e:?}")))?;
                let mut managed = BTreeMap::new();
                for (at, version, plan) in history {
                    if at > horizon {
                        return Err(error("Qcc checkpoint future installation"));
                    }
                    for port in plan.ports {
                        let name = prepared.ports[port.direction].port.clone();
                        let schedule = configuration::schedule(prepared, &port, at, version)
                            .map_err(|e| error(format!("Qcc checkpoint schedule: {e:?}")))?;
                        let count = managed.get(&name).map_or(0u64, |(_, count)| *count);
                        managed.insert(name, (schedule, count + 1));
                    }
                }
                tsn.validate_checkpoint_with_managed(horizon, &managed)?;
            } else {
                tsn.validate_checkpoint(horizon)?;
            }
            tsn.validate_checkpoint_wakes(&self.tsn_wakes, horizon)?;
            // Application clocks have already been reconstructed above from
            // prepared adjustments or validated gPTP causes. The shaper's
            // cached dependency must name that same authoritative anchor.
            for output in &p.tsn.as_ref().unwrap().outputs {
                let source = p
                    .ethernet
                    .directions
                    .iter()
                    .find(|d| d.from_port == output.port)
                    .ok_or_else(|| error("saved shaper clock has unknown output"))?
                    .source;
                tsn.validate_clock_dependency(
                    &output.port,
                    self.clocks.as_ref().map(|clocks| &clocks[source]),
                )?;
            }
        }
        self.validate_mac_merge(horizon)?;
        self.validate_producer_express_sources(horizon)?;
        if let Some(bridge) = &self.bridge {
            bridge.validate_checkpoint(horizon)?;
        }
        self.validate_distributed_checkpoint(horizon)?;
        self.validate_gateway_composition_checkpoint(horizon)?;
        if let Some(replication) = &self.replication {
            let cfg = p
                .extensions
                .as_ref()
                .and_then(|e| e.replication.as_ref())
                .unwrap();
            if replication.streams.len() != cfg.streams.len() {
                return Err(error("network checkpoint replication stream count"));
            }
            for (index, (stream, c)) in replication.streams.iter().zip(&cfg.streams).enumerate() {
                if !stream
                    .history
                    .checkpoint_capacity(c.history_capacity, c.wire_format)
                    || stream.last_recovery_ps.is_some_and(|time| time > horizon)
                    || (c.wire_format == ReplicationWireFormat::Rtag8021cb2017
                        && (stream.next >= 65536 || stream.exhausted))
                {
                    return Err(error("network checkpoint replication history"));
                }
                let timers: Vec<_> = self
                    .due
                    .iter()
                    .filter_map(|due| match due.kind {
                        Completion::RecoveryTimeout { stream, generation } if stream == index => {
                            Some((due.at, generation))
                        }
                        _ => None,
                    })
                    .collect();
                if c.recovery_algorithm == ReplicationRecoveryAlgorithm::Vector8021cb2017 {
                    let expected = stream
                        .recovery_deadline_ps
                        .map(|at| (at, stream.timer_generation));
                    if timers.as_slice() != expected.as_slice()
                        || stream.reset_count == 0
                        || stream.reset_count.checked_sub(1)
                            != Some(stream.reset_events.len() as u64)
                        || stream
                            .reset_events
                            .iter()
                            .any(|reset| reset.time_ps > horizon)
                        || stream.reset_events.windows(2).any(|pair| {
                            pair[0].time_ps >= pair[1].time_ps
                                || pair[0].generation >= pair[1].generation
                        })
                        || expected.is_some_and(|(at, generation)| {
                            generation == 0
                                || at < horizon
                                || stream.last_recovery_ps.and_then(|last| {
                                    (last / replication::RECOVERY_TICK_PS
                                        * replication::RECOVERY_TICK_PS)
                                        .checked_add(c.recovery_timeout_ps.unwrap_or(0))
                                }) != Some(at)
                        })
                    {
                        return Err(error(
                            "network checkpoint recovery timer owner/deadline/reset mismatch",
                        ));
                    }
                } else if !timers.is_empty()
                    || stream.timer_generation != 0
                    || stream.recovery_deadline_ps.is_some()
                    || stream.reset_count != 0
                    || !stream.reset_events.is_empty()
                {
                    return Err(error("project recovery has normative timer state"));
                }
            }
            if !replication.records.is_empty() {
                return Err(error("network checkpoint uncommitted replication records"));
            }
            for (id, origin) in &replication.origins {
                let ids = origin["branch_ids"]
                    .as_array()
                    .ok_or_else(|| error("network checkpoint replication origin branches"))?;
                if !origin.is_object()
                    || ids.is_empty()
                    || ids.iter().any(|branch| {
                        branch.as_str().is_none_or(|b| {
                            replication.branches.get(b).is_none_or(|b| &b.origin != id)
                        })
                    })
                {
                    return Err(error("network checkpoint replication origin references"));
                }
            }
            for (id, branch) in &replication.branches {
                if !branch.data.is_object()
                    || branch.data["branch_id"] != *id
                    || !replication.origins.contains_key(&branch.origin)
                    || cfg
                        .streams
                        .get(branch.stream)
                        .is_none_or(|s| branch.path >= s.paths.len())
                {
                    return Err(error("network checkpoint replication branch"));
                }
            }
        }
        if let Some(preemption) = &self.preemption {
            if !preemption.records.is_empty()
                || preemption.ports.len() != p.ethernet.directions.len()
            {
                return Err(error("network checkpoint preemption port count"));
            }
            for (direction, port) in preemption.ports.iter().enumerate() {
                if let Some(preemption::Work::Wire(id)) = &port.active {
                    let host = self
                        .wire
                        .as_ref()
                        .ok_or_else(|| error("abstract Wire checkpoint host missing"))?;
                    let tx = host
                        .tx
                        .get(id)
                        .ok_or_else(|| error("abstract Wire checkpoint source missing"))?;
                    if host.active[direction].as_ref() != Some(id)
                        || tx.sof.is_none()
                        || tx.release.is_some()
                        || port.active_end != tx.planned_release
                        || self.queues[direction].active.is_some()
                    {
                        return Err(error("abstract Wire checkpoint active owner differs"));
                    }
                }
                for i in port
                    .active
                    .as_ref()
                    .and_then(preemption::Work::data)
                    .iter()
                    .chain(port.suspended.iter())
                {
                    if self
                        .transfers
                        .get(*i)
                        .is_none_or(|t| t.direction != direction || t.fragments.is_none())
                    {
                        return Err(error("network checkpoint fragment owner"));
                    }
                }
            }
        }
        if let Some(host) = &self.wire {
            if host.queues.len() != self.queues.len()
                || host.boundaries.len() != p.wire.as_ref().unwrap().packets.len()
                || host.logical.len()
                    > p.extensions
                        .as_ref()
                        .map_or(0, |e| e.limits.max_pending_branches as usize)
            {
                return Err(error("network checkpoint wire dimensions"));
            }
            for (i, queue) in host.queues.iter().enumerate() {
                for id in queue.iter().chain(host.active[i].iter()) {
                    if host.tx[id].intent.port != p.ethernet.directions[i].from_port {
                        return Err(error("network checkpoint wire media owner"));
                    }
                }
            }
        }
        Ok(())
    }
    fn validate_gateway_composition_checkpoint(&self, horizon: u64) -> Result<()> {
        if !self.prepared.has_gateway_replication() {
            if self
                .frames
                .iter()
                .any(|f| f.gateway_injection.is_some() || !f.terminal_indications.is_empty())
            {
                return Err(error(
                    "network checkpoint Gateway provenance without codec16",
                ));
            }
            return Ok(());
        }
        let bridge = self
            .bridge
            .as_ref()
            .ok_or_else(|| error("network checkpoint Gateway Bridge missing"))?;
        let mut bindings = BTreeMap::new();
        for (fi, frame) in self.frames.iter().enumerate() {
            let Some(injection) = &frame.gateway_injection else {
                if !frame.terminal_indications.is_empty() {
                    return Err(error("network checkpoint foreign terminal indications"));
                }
                continue;
            };
            if bindings
                .insert((injection.conversion_index, injection.branch_index), fi)
                .is_some()
            {
                return Err(error("network checkpoint duplicate Gateway child binding"));
            }
            let stream = self
                .replication_source_stream(frame)
                .ok_or_else(|| error("network checkpoint Gateway source selector"))?;
            self.validate_replication_frame_source(frame, stream, None)?;
            let evidence = bridge.gateway_source_evidence(
                injection.conversion_index,
                injection.branch_index,
                None,
            )?;
            let config = &self
                .prepared
                .extensions
                .as_ref()
                .unwrap()
                .replication
                .as_ref()
                .unwrap()
                .streams[stream];
            let crate::types::network::ReplicationPlacement::BridgePair(pair) = &config.placement
            else {
                return Err(error("network checkpoint Gateway placement"));
            };
            let flow = self
                .replication
                .as_ref()
                .unwrap()
                .distributed
                .values()
                .filter(|f| f.frame == fi && f.stream == stream)
                .collect::<Vec<_>>();
            if flow.len() != 1 || flow[0].created_ps != frame.row.generated_ps {
                return Err(error("network checkpoint Gateway flow binding"));
            }
            let roots = self
                .transfers
                .iter()
                .filter(|t| t.frame == fi && t.row.parent_transfer_id.is_none())
                .collect::<Vec<_>>();
            if roots.len() != 1
                || roots[0].direction != pair.prefix.directions[0]
                || roots[0].row.queued_ps != evidence.injected_ps
            {
                return Err(error("network checkpoint Gateway physical prefix"));
            }
            let sof = roots[0].row.sof_ps;
            if sof != evidence.sof_ps || sof.is_some_and(|t| t > horizon) {
                return Err(error("network checkpoint Gateway source SOF evidence"));
            }
            let expected = self
                .receptions
                .iter()
                .enumerate()
                .filter_map(|(ri, rx)| {
                    let t = &self.transfers[rx.transfer];
                    if t.frame != fi
                        || rx.row.status != "received"
                        || self.prepared.ethernet.directions[t.direction].destination
                            != pair.destination_device
                    {
                        return None;
                    }
                    let dc = t.circuit.as_ref()?.distributed.as_ref()?;
                    match dc.leg {
                        replication::DistributedLeg::SuffixRound { round } => {
                            Some(TerminalIndication {
                                reception: ri,
                                recovery_round: Some(round),
                            })
                        }
                        _ => None,
                    }
                })
                .collect::<Vec<_>>();
            if frame.terminal_indications != expected {
                return Err(error(
                    "network checkpoint Gateway terminal indication ledger",
                ));
            }
            for i in &expected {
                let rx = &self.receptions[i.reception];
                let round = i.recovery_round.unwrap();
                if rx.row.ready_ps.is_none_or(|t| t > horizon)
                    || flow[0]
                        .rounds
                        .get(round)
                        .is_none_or(|r| r.delivery_reception != Some(i.reception))
                {
                    return Err(error(
                        "network checkpoint Gateway unaccepted terminal round",
                    ));
                }
            }
            match (sof, bridge.source_segment(&evidence.lineage.segment_id)) {
                (None, None) => (),
                (Some(sof), Some(segment)) => {
                    let target = segment
                        .targets
                        .first()
                        .ok_or_else(|| error("network checkpoint Gateway frozen target missing"))?;
                    let first = expected.first().map(|i| &self.receptions[i.reception]);
                    if segment.sof_ps != sof
                        || segment.source_record_id != frame.row.frame_id
                        || segment.origin_id != evidence.lineage.origin_id
                        || segment.branch_lineage != evidence.lineage.branch_lineage
                        || segment.target_count != 1
                        || segment.targets.len() != 1
                        || target.terminal_id
                            != self.prepared.ethernet.devices[pair.destination_device].id
                        || target.origin_id != segment.origin_id
                        || target.segment_id != segment.segment_id
                        || target.branch_lineage != segment.branch_lineage
                        || target.completed_ps != first.and_then(|r| r.row.ready_ps)
                        || target.completion_reception_id.as_deref()
                            != first.map(|r| r.row.reception_id.as_str())
                    {
                        return Err(error(
                            "network checkpoint Gateway SOF/terminal summary causality",
                        ));
                    }
                }
                _ => {
                    return Err(error(
                        "network checkpoint Gateway premature/missing SOF segment",
                    ));
                }
            }
        }
        for (ci, bi) in bridge.gateway_injection_indices() {
            let e = bridge.gateway_source_evidence(ci, bi, None)?;
            if self
                .prepared
                .extensions
                .as_ref()
                .unwrap()
                .replication
                .as_ref()
                .unwrap()
                .streams
                .iter()
                .any(|s| s.source == e.selector)
                && !bindings.contains_key(&(ci, bi))
            {
                return Err(error("network checkpoint Gateway admitted child omitted"));
            }
        }
        Ok(())
    }
    fn validate_completion(&self, c: &Completion) -> Result<()> {
        let valid = match c {
            Completion::ConfiguredMedia { domain, generation } => self
                .configured_media
                .pairs
                .get(*domain)
                .is_some_and(|p| p.reservation.is_some_and(|(_, g)| g == *generation)),
            Completion::PauseAdmin(i) => self
                .prepared
                .mac_control
                .as_ref()
                .is_some_and(|p| *i < p.controls.len()),
            Completion::PauseExpire {
                direction,
                generation,
            } => self
                .mac_control
                .ports
                .get(*direction)
                .and_then(Option::as_ref)
                .is_some_and(|p| *generation <= p.generation),
            Completion::Eof(t) | Completion::Release(t) => *t < self.transfers.len(),
            Completion::Ready(f) => *f < self.frames.len(),
            Completion::Processed(r) => *r < self.receptions.len(),
            Completion::Fragment {
                direction,
                transfer,
                ..
            } => self
                .transfers
                .get(*transfer)
                .is_some_and(|t| t.direction == *direction),
            Completion::Gap { direction, .. } => *direction < self.queues.len(),
            Completion::Bridge(timer) => {
                return self
                    .bridge
                    .as_ref()
                    .ok_or_else(|| error("bridge checkpoint completion missing"))?
                    .validate_checkpoint_timer(timer);
            }
            Completion::Wire(event) => return self.validate_wire_event(event),
            Completion::MulticastTimer => self.multicast_active.is_some(),
            Completion::MacMerge(_) => self.mac_merge.is_some(),
            Completion::RecoveryTimeout { stream, generation } => self
                .replication
                .as_ref()
                .and_then(|r| r.streams.get(*stream))
                .is_some_and(|s| {
                    s.timer_generation == *generation && s.recovery_deadline_ps.is_some()
                }),
        };
        if valid {
            Ok(())
        } else {
            Err(error("network checkpoint completion reference"))
        }
    }
    fn validate_wire_event(&self, event: &HostEvent) -> Result<()> {
        let Some(host) = &self.wire else {
            return Err(error("wire checkpoint event without host"));
        };
        let valid = match event {
            HostEvent::Packet(i) => *i < self.prepared.wire.as_ref().unwrap().packets.len(),
            HostEvent::BoundaryStart(i) | HostEvent::BoundaryRelease(i) => self
                .prepared
                .wire
                .as_ref()
                .unwrap()
                .packets
                .get(*i)
                .is_some_and(|p| p.boundary.is_some()),
            HostEvent::Timer { adapter, .. } => host.protocol.checkpoint_adapter(adapter).is_some(),
            HostEvent::Logical(id) => host.logical.contains_key(id),
            HostEvent::Eof { direction, id }
            | HostEvent::Release { direction, id }
            | HostEvent::Arrival { direction, id } => self
                .prepared
                .ethernet
                .directions
                .get(*direction)
                .is_some_and(|d| {
                    host.tx
                        .get(id)
                        .is_some_and(|tx| tx.intent.port == d.from_port)
                }),
        };
        if valid {
            Ok(())
        } else {
            Err(error("wire checkpoint event reference"))
        }
    }
    pub(crate) fn validate_checkpoint_envelope(&self, envelope: &Envelope) -> Result<bool> {
        let event: Event =
            serde_json::from_slice(&envelope.payload).map_err(|e| error(e.to_string()))?;
        match &event {
            Event::PhyBoundary {
                attachment,
                generation,
                sample_cut,
            } => {
                let state = self
                    .phy_boundaries
                    .get(*attachment)
                    .ok_or_else(|| error("network checkpoint unknown physical boundary"))?;
                let pending = state
                    .pending
                    .as_ref()
                    .ok_or_else(|| error("network checkpoint physical boundary missing"))?;
                if state.generation != *generation
                    || pending.sample_cut != *sample_cut
                    || self.phy[*attachment]
                        .sample_time_ps(*sample_cut)
                        .map_err(error)?
                        != envelope.time_ps
                {
                    return Err(error("network checkpoint physical boundary differs"));
                }
            }
            Event::MacMergeTimer {
                direction, token, ..
            } => {
                if self
                    .mac_merge
                    .as_ref()
                    .is_none_or(|s| *direction >= s.ports.len() || token.due_ps != envelope.time_ps)
                {
                    return Err(error("MAC Merge timer reference"));
                }
            }
            Event::MacMergeArrival {
                direction,
                epoch,
                packet,
            } => self.validate_merge_arrival(*direction, *epoch, *packet, envelope.time_ps)?,
            Event::Dispatch => {}
            Event::Generate(i, ordinal) => {
                if self
                    .prepared
                    .ethernet
                    .generators
                    .get(*i)
                    .is_none_or(|g| g.time(*ordinal) != Some(u128::from(envelope.time_ps)))
                {
                    return Err(error("network checkpoint generator reference"));
                }
            }
            Event::Ready(f) => {
                if *f >= self.frames.len() {
                    return Err(error("network checkpoint ready reference"));
                }
            }
            Event::Arrival(t) => {
                if *t >= self.transfers.len()
                    || self.segment_for(self.transfers[*t].direction).is_some()
                {
                    return Err(error("network checkpoint transfer event"));
                }
            }
            Event::SegmentArrival { transfer, receiver } => {
                if self.segment_arrivals.get(&(*transfer, *receiver)) != Some(&envelope.time_ps) {
                    return Err(error("network checkpoint shared LAN arrival event"));
                }
            }
            Event::Complete(r) => {
                if *r >= self.receptions.len() {
                    return Err(error("network checkpoint completion reception"));
                }
            }
            Event::Bridge(timer) => self
                .bridge
                .as_ref()
                .ok_or_else(|| error("bridge checkpoint event missing"))?
                .validate_checkpoint_timer(timer)?,
            Event::Control(generation) => {
                if *generation != self.control_generation {
                    return Err(error("network checkpoint control generation"));
                }
            }
            Event::Wire(event) => self.validate_wire_event(event)?,
            Event::Clock(index) => {
                if *index != self.clock_cursor
                    || self
                        .prepared
                        .clocks
                        .as_ref()
                        .and_then(|c| c.adjustments.get(*index))
                        .is_none_or(|a| a.at_ps != envelope.time_ps)
                {
                    return Err(error("network checkpoint clock event reference"));
                }
            }
            Event::Gptp(event) => self.validate_gptp_event(event, envelope.time_ps)?,
            Event::PhySample {
                attachment,
                sample_index,
            } => {
                if self.phy_wakes.get(*attachment) != Some(&Some(*sample_index))
                    || self.phy.get(*attachment).is_none_or(|phy| {
                        phy.next_sample_index() != *sample_index
                            || phy.next_event_ps().ok() != Some(envelope.time_ps)
                    })
                {
                    return Err(error("network checkpoint PHY sample event reference"));
                }
            }
        }
        let control = matches!(event, Event::Control(_));
        if envelope.schema
            != Schema::new(
                if control {
                    "dir.network.control"
                } else {
                    "dir.network.event"
                },
                1,
            )
        {
            return Err(error("network checkpoint event schema"));
        }
        Ok(!control)
    }
    /// The sole network Control consumes every due item before committing.
    /// Other deliveries are phase one and arbitration is phase two; accepted
    /// recovery always arms a strictly future tick. Thus no active recovery
    /// deadline can remain at or before any committed event's timestamp.
    pub(crate) fn validate_recovery_checkpoint_cut(
        &self,
        cursor: Option<super::super::registered::Key>,
    ) -> Result<()> {
        if cursor.is_some_and(|last| {
            self.due.iter().any(|due| {
                matches!(due.kind, Completion::RecoveryTimeout { .. }) && due.at <= last.0
            })
        }) {
            return Err(error(
                "network checkpoint recovery deadline contradicts committed event cut",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_checkpoint_reservations(
        &self,
        queue: &[(
            super::super::registered::Key,
            super::super::registered::Work,
        )],
        live: &std::collections::BTreeSet<TimerToken>,
        limit: u64,
    ) -> Result<()> {
        let mut direct = 0;
        let mut controls = 0;
        let mut pending = Pending::default();
        let mut bridge_timers = Vec::new();
        let mut wire_events = Pending::default();
        let mut counts = vec![0u64; self.cursors.len()];
        let mut generate = Vec::new();
        let mut dispatch = Vec::new();
        let mut clock_events = Vec::new();
        for frame in &self.frames {
            if let Some(i) = frame.generator {
                counts[i] += 1;
            }
        }
        for due in &self.due {
            match &due.kind {
                Completion::Eof(t) => pending.add(key("eof", *t), due.at),
                Completion::Release(t) => pending.add(key("release", *t), due.at),
                Completion::Ready(f) => pending.add(key("ready", *f), due.at),
                Completion::Processed(r) => pending.add(key("processed", *r), due.at),
                Completion::Bridge(timer) => bridge_timers.push((due.at, timer.clone())),
                Completion::Wire(event) => wire_events.add(event_key(event)?, due.at),
                _ => {}
            }
        }
        let mut merge_events = Vec::new();
        let mut phy_events = Vec::new();
        let mut boundary_events = Vec::new();
        let mut gptp_events = Vec::new();
        for (key, work) in queue {
            if let super::super::registered::Work::Delivery { envelope, token } = work {
                if token.as_ref().is_some_and(|t| !live.contains(t)) {
                    continue;
                }
                if self.validate_checkpoint_envelope(envelope)? {
                    direct += 1;
                }
                let event: Event =
                    serde_json::from_slice(&envelope.payload).map_err(|e| error(e.to_string()))?;
                match event {
                    Event::PhyBoundary {
                        attachment,
                        generation,
                        sample_cut,
                    } => {
                        boundary_events.push((
                            attachment,
                            generation,
                            sample_cut,
                            key.0,
                            token.clone(),
                        ));
                    }
                    event @ (Event::MacMergeTimer { .. } | Event::MacMergeArrival { .. }) => {
                        merge_events.push((key.0, event))
                    }
                    Event::Control(_) => {
                        controls += 1;
                        if self.control.as_ref().is_none_or(|(time, held)| {
                            *time != key.0 || token.as_ref() != Some(held)
                        }) {
                            return Err(error(
                                "network checkpoint held control reservation differs",
                            ));
                        }
                    }
                    Event::Generate(i, ordinal) => generate.push((i, ordinal)),
                    Event::Dispatch => dispatch.push(key.0),
                    Event::Ready(f) => {
                        pending.add(crate::runtime::network::checkpoint::key("ready", f), key.0)
                    }
                    Event::Arrival(t) => pending.add(
                        crate::runtime::network::checkpoint::key("arrival", t),
                        key.0,
                    ),
                    Event::SegmentArrival { transfer, receiver } => {
                        pending.add(format!("segment_arrival:{transfer}:{receiver}"), key.0)
                    }
                    Event::Complete(r) => pending.add(
                        crate::runtime::network::checkpoint::key("processed", r),
                        key.0,
                    ),
                    Event::Bridge(timer) => bridge_timers.push((key.0, timer)),
                    Event::Wire(event) => wire_events.add(event_key(&event)?, key.0),
                    Event::Clock(index) => clock_events.push((index, key.0)),
                    Event::Gptp(event) => gptp_events.push((event, key.0)),
                    Event::PhySample {
                        attachment,
                        sample_index,
                    } => phy_events.push((attachment, sample_index, key.0)),
                }
            }
        }
        if direct != self.pending_direct || controls != usize::from(self.control.is_some()) {
            return Err(error("network checkpoint reservation count"));
        }
        self.validate_gptp_reservations(&gptp_events)?;
        for (i, ordinal) in generate {
            if ordinal != counts[i] {
                return Err(error("network checkpoint pending source ordinal"));
            }
            counts[i] += 1;
        }
        if counts != self.cursors {
            return Err(error(
                "network checkpoint cursor and generated frame ledger differ",
            ));
        }
        let next = self
            .next_generation(&self.cursors)
            .and_then(|v| u64::try_from(v).ok());
        if dispatch != next.into_iter().collect::<Vec<_>>() {
            return Err(error("network checkpoint source dispatcher reservation"));
        }
        let next_clock = self
            .prepared
            .clocks
            .as_ref()
            .and_then(|config| config.adjustments.get(self.clock_cursor))
            .map(|adjustment| (self.clock_cursor, adjustment.at_ps));
        if clock_events != next_clock.into_iter().collect::<Vec<_>>() {
            return Err(error("network checkpoint clock reservation"));
        }
        let expected_phy: Vec<_> = self
            .phy_wakes
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.phy[*i].config().medium.physical_mode.is_named())
            .map(|(i, wake)| {
                Ok((
                    i,
                    wake.ok_or_else(|| error("network checkpoint PHY wake missing"))?,
                    self.phy[i].next_event_ps().map_err(error)?,
                ))
            })
            .collect::<Result<_>>()?;
        phy_events.sort_unstable();
        if phy_events != expected_phy {
            return Err(error("network checkpoint PHY wake reservation"));
        }
        for (index, phy) in self.phy.iter().enumerate() {
            if phy.config().medium.physical_mode.is_named() && self.phy_wakes[index].is_some() {
                return Err(error("named PHY has a legacy scalar reservation"));
            }
        }
        let mut expected_boundaries = self
            .phy_boundaries
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.pending.as_ref().map(|w| (i, s.generation, w)))
            .map(|(i, generation, w)| {
                Ok((
                    i,
                    generation,
                    w.sample_cut,
                    self.phy[i].sample_time_ps(w.sample_cut).map_err(error)?,
                    Some(w.token.clone()),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        expected_boundaries.sort();
        boundary_events.sort();
        if boundary_events != expected_boundaries {
            return Err(error(
                "network checkpoint named PHY boundary reservation differs",
            ));
        }
        let deadline = self
            .due
            .iter()
            .map(|d| d.at)
            .chain(self.dynamic.as_ref().and_then(DynamicState::next_deadline))
            .chain(self.tsn.as_ref().and_then(TsnState::next_control))
            .chain(self.tsn_wakes.iter().flatten().map(|w| w.time_ps))
            .chain(
                self.prepared
                    .qcc
                    .as_ref()
                    .and_then(|p| self.qcc.as_ref().and_then(|s| s.next_deadline(p))),
            )
            .min();
        if self.control.as_ref().map(|(time, _)| *time) != deadline {
            return Err(error("network checkpoint control deadline ledger differs"));
        }
        if let Some((_, token)) = &self.control {
            if !live.contains(token) {
                return Err(error("network checkpoint control token is not live"));
            }
        }
        self.validate_lifecycle(&mut pending)?;
        if let Some(bridge) = &self.bridge {
            bridge.validate_checkpoint_pending(&bridge_timers, limit)?;
        } else if !bridge_timers.is_empty() {
            return Err(error("network checkpoint unexpected bridge timers"));
        }
        self.validate_wire_pending(&mut wire_events)?;
        self.validate_fragment_pending()?;
        self.validate_mac_merge_pending(&merge_events)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Serialize, Deserialize)]
    struct Exact {
        #[serde(with = "decimal128")]
        numerator: u128,
        #[serde(with = "optional128")]
        wake: Option<u128>,
        #[serde(with = "credit")]
        credit: crate::types::ethernet::tsn::Credit,
    }
    #[test]
    fn checkpoint_numerators_preserve_full_u128_and_reject_noncanonical_values() {
        let saved = serde_json::to_value(Exact {
            numerator: u128::MAX,
            wake: Some(u128::MAX),
            credit: crate::types::ethernet::tsn::Credit::new(true, u128::MAX),
        })
        .unwrap();
        let restored: Exact = decode(saved.clone()).unwrap();
        assert_eq!(restored.numerator, u128::MAX);
        assert_eq!(restored.wake, Some(u128::MAX));
        assert_eq!(restored.credit.magnitude, u128::MAX);
        let mut invalid = saved;
        invalid["numerator"] = json!("01");
        assert!(decode::<Exact>(invalid).is_err());
    }
}

#[derive(Default)]
struct Pending(BTreeMap<String, Vec<u64>>);
fn key(kind: &str, index: usize) -> String {
    format!("{kind}:{index}")
}
fn event_key(event: &HostEvent) -> Result<String> {
    serde_json::to_string(event).map_err(|e| error(e.to_string()))
}
impl Pending {
    fn add(&mut self, key: String, time: u64) {
        self.0.entry(key).or_default().push(time);
    }
    fn expect(&mut self, key: String, time: Option<u64>) -> Result<()> {
        if self.0.remove(&key).unwrap_or_default() != time.into_iter().collect::<Vec<_>>() {
            return Err(error(format!(
                "network checkpoint lifecycle reservation differs: {key}"
            )));
        }
        Ok(())
    }
    fn finish(&self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(error("network checkpoint unexpected lifecycle reservation"))
        }
    }
}
impl NetworkState {
    fn validate_lifecycle(&self, pending: &mut Pending) -> Result<()> {
        for (i, frame) in self.frames.iter().enumerate() {
            let expected = if frame.row.ready_ps.is_none() {
                let generator = frame
                    .generator
                    .and_then(|g| self.prepared.ethernet.generators.get(g))
                    .ok_or_else(|| error("unready checkpoint frame without generator"))?;
                let device = &self.prepared.ethernet.devices[generator.source];
                Some(
                    frame
                        .row
                        .generated_ps
                        .checked_add(device.tx_processing_delay_ps)
                        .ok_or_else(|| error("checkpoint source ready overflow"))?,
                )
            } else {
                None
            };
            pending.expect(key("ready", i), expected)?;
        }
        for (i, transfer) in self.transfers.iter().enumerate() {
            let row = &transfer.row;
            let attached = self.phy_attachment_for(transfer.direction).is_some();
            let merged = self.merge_direction(transfer.direction);
            let simple = !self.preemptable(transfer)
                && !attached
                && !merged
                && (self.preemption_direction(transfer.direction)
                    || self.configured_endpoint(transfer.direction).is_none());
            if simple {
                if let Some(sof) = row.sof_ps {
                    let link = &self.prepared.ethernet.directions[transfer.direction];
                    let (eof, release, arrival) = l2::timing(
                        sof,
                        row.wire.mac_bytes,
                        link.bitrate_bps,
                        self.physical_receive_delay(transfer.direction)?,
                    )?;
                    if (
                        row.planned_eof_ps,
                        row.planned_release_ps,
                        row.planned_arrival_ps,
                    ) != (Some(eof), Some(release), Some(arrival))
                    {
                        return Err(error("network checkpoint planned wire timing differs"));
                    }
                    if row.status != "dropped"
                        && transfer
                            .fragments
                            .as_ref()
                            .is_none_or(|f| f.reset_ps.is_none())
                        && row.release_ps.is_none()
                        && self.queues[transfer.direction].active != Some(i)
                    {
                        return Err(error(
                            "network checkpoint active transfer missing from port",
                        ));
                    }
                }
            }
            pending.expect(
                key("eof", i),
                if simple && row.status != "dropped" && row.sof_ps.is_some() && row.eof_ps.is_none()
                {
                    Some(
                        row.planned_eof_ps
                            .ok_or_else(|| error("checkpoint EOF not planned"))?,
                    )
                } else {
                    None
                },
            )?;
            pending.expect(
                key("release", i),
                if simple
                    && transfer
                        .fragments
                        .as_ref()
                        .is_none_or(|f| f.reset_ps.is_none())
                    && row.status != "dropped"
                    && row.sof_ps.is_some()
                    && row.release_ps.is_none()
                {
                    Some(
                        row.planned_release_ps
                            .ok_or_else(|| error("checkpoint release not planned"))?,
                    )
                } else {
                    None
                },
            )?;
            pending.expect(
                key("arrival", i),
                if merged {
                    None
                } else if attached {
                    self.phy_arrivals.get(&i).copied()
                } else if self.segment_for(transfer.direction).is_some() {
                    None
                } else if row.eof_ps.is_some()
                    && row.arrival_ps.is_none()
                    && transfer
                        .fragments
                        .as_ref()
                        .is_none_or(|f| f.arrival_retired_ps.is_none())
                {
                    Some(
                        row.planned_arrival_ps
                            .ok_or_else(|| error("checkpoint arrival not planned"))?,
                    )
                } else {
                    None
                },
            )?;
            if self.segment_for(transfer.direction).is_some() {
                for receiver in self.segment_receivers(transfer.direction) {
                    let observed = self
                        .receptions
                        .iter()
                        .any(|r| r.transfer == i && r.segment_receiver == Some(receiver));
                    let expected = if row.eof_ps.is_some() && !observed {
                        row.planned_arrival_ps
                    } else {
                        None
                    };
                    if self.segment_arrivals.get(&(i, receiver)).copied() != expected {
                        return Err(error(
                            "network checkpoint shared LAN reception ledger differs",
                        ));
                    }
                    pending.expect(format!("segment_arrival:{i}:{receiver}"), expected)?;
                }
            }
        }
        for (i, reception) in self.receptions.iter().enumerate() {
            pending.expect(
                key("processed", i),
                if reception.row.ready_ps.is_none() {
                    reception.row.planned_ready_ps
                } else {
                    None
                },
            )?;
        }
        pending.finish()
    }
    fn validate_wire_pending(&self, pending: &mut Pending) -> Result<()> {
        let Some(host) = &self.wire else {
            return pending.finish();
        };
        let expected = host.protocol.checkpoint_timers();
        if expected != host.timers {
            return Err(error("wire checkpoint active timer ledgers differ"));
        }
        for ((adapter, key), (generation, deadline)) in &host.timers {
            pending.expect(
                event_key(&HostEvent::Timer {
                    adapter: adapter.clone(),
                    key: key.clone(),
                    generation: *generation,
                    deadline: *deadline,
                })?,
                Some(*deadline),
            )?;
        }
        for (id, work) in &host.logical {
            pending.expect(
                event_key(&HostEvent::Logical(id.clone()))?,
                Some(work.at_ps),
            )?;
        }
        for (id, tx) in &host.tx {
            let direction = self
                .prepared
                .ethernet
                .directions
                .iter()
                .position(|d| d.from_port == tx.intent.port)
                .ok_or_else(|| error("checkpoint wire TX port missing"))?;
            let physical_owned = (!self.preemption_direction(direction)
                && self.configured_endpoint(direction).is_some())
                || self.merge_direction(direction)
                || tx.phy_transfer.is_some();
            for (event, required, time) in [
                (
                    HostEvent::Eof {
                        direction,
                        id: id.clone(),
                    },
                    !physical_owned && !tx.dropped && tx.sof.is_some() && tx.eof.is_none(),
                    tx.planned_eof,
                ),
                (
                    HostEvent::Release {
                        direction,
                        id: id.clone(),
                    },
                    !physical_owned && tx.sof.is_some() && tx.release.is_none(),
                    tx.planned_release,
                ),
                (
                    HostEvent::Arrival {
                        direction,
                        id: id.clone(),
                    },
                    !self.merge_direction(direction)
                        && tx.phy_transfer.is_none()
                        && tx.eof.is_some()
                        && tx.arrival.is_none()
                        && tx.arrival_retired_ps.is_none(),
                    tx.planned_arrival,
                ),
            ] {
                pending.expect(
                    event_key(&event)?,
                    if required {
                        Some(time.ok_or_else(|| error("wire checkpoint missing planned boundary"))?)
                    } else {
                        None
                    },
                )?;
            }
        }
        for (i, packet) in self
            .prepared
            .wire
            .as_ref()
            .unwrap()
            .packets
            .iter()
            .enumerate()
        {
            let state = &host.boundaries[i];
            if let Some(plan) = &packet.boundary {
                pending.expect(
                    event_key(&HostEvent::BoundaryStart(i))?,
                    state.actual_start_ps.is_none().then_some(plan.start_ps),
                )?;
                pending.expect(
                    event_key(&HostEvent::BoundaryRelease(i))?,
                    state.actual_release_ps.is_none().then_some(plan.release_ps),
                )?;
                pending.expect(
                    event_key(&HostEvent::Packet(i))?,
                    state.actual_arrival_ps.is_none().then_some(plan.arrival_ps),
                )?;
            } else {
                // Non-boundary injections have no retained physical RX row. Their
                // pending FES entry remains authoritative at the current time.
                pending.0.remove(&event_key(&HostEvent::Packet(i))?);
            }
        }
        pending.finish()
    }
    fn validate_fragment_pending(&self) -> Result<()> {
        let Some(state) = &self.preemption else {
            return Ok(());
        };
        for (direction, port) in state.ports.iter().enumerate() {
            if let Some(i) = port.active.as_ref().and_then(preemption::Work::data) {
                let transfer = &self.transfers[i];
                if self.preemptable(transfer) {
                    let f = transfer
                        .fragments
                        .as_ref()
                        .ok_or_else(|| error("checkpoint active fragment state missing"))?;
                    let id = f
                        .fragments
                        .last()
                        .ok_or_else(|| error("checkpoint active fragment id missing"))?;
                    let count=self.due.iter().filter(|due|matches!(&due.kind,Completion::Fragment{direction:d,transfer:t,epoch,id:fragment,..}if *d==direction&&*t==i&&*epoch==port.tx_epoch&&fragment==id)&&Some(due.at)==port.active_end).count();
                    if count != 1 || f.active_end != port.active_end {
                        return Err(error("checkpoint active fragment completion missing"));
                    }
                }
            }
            if let Some(at) = port.gap_due {
                let count=self.due.iter().filter(|due|due.at==at&&matches!(&due.kind,Completion::Gap{direction:d,epoch,id}if *d==direction&&*epoch==port.gap_epoch&&Some(id)==port.gap_id.as_ref())).count();
                if count != 1 {
                    return Err(error("checkpoint gap completion missing"));
                }
            }
        }
        Ok(())
    }
}
