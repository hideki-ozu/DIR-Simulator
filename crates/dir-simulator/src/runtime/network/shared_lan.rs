//! One ordered MAC service reservation for all carrier ports of a declared LAN.
use super::*;
use crate::types::ethernet::EthernetDirection;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(super) enum SegmentOwner {
    Data(usize),
    Control(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SegmentReservation {
    pub direction: usize,
    pub owner: SegmentOwner,
    pub sof_ps: u64,
    pub eof_ps: u64,
    pub release_ps: u64,
}

fn paired(port: &str) -> Result<String> {
    let (device, gate) = port
        .rsplit_once('.')
        .ok_or_else(|| error("shared LAN port name"))?;
    let input = if gate == "tx" {
        "rx".to_string()
    } else if let Some(suffix) = gate.strip_prefix("tx_") {
        format!("rx_{suffix}")
    } else {
        return Err(error("shared LAN requires an Ethernet output gate"));
    };
    Ok(format!("{device}.{input}"))
}

impl NetworkState {
    pub(super) fn segment_for(&self, direction: usize) -> Option<usize> {
        if let Some(bindings) = &self.prepared.bindings {
            let port = bindings.ports.get(direction)?;
            return match bindings.domains.get(port.physical_domain)?.owner {
                crate::types::network::bindings::PhysicalOwner::IdealShared { segment } => {
                    Some(segment)
                }
                _ => None,
            };
        }
        self.prepared
            .shared_lans
            .iter()
            .position(|s| s.members.contains(&direction))
    }
    pub(super) fn segment_receivers(&self, direction: usize) -> Vec<usize> {
        self.segment_for(direction).map_or_else(Vec::new, |i| {
            self.prepared.shared_lans[i]
                .members
                .iter()
                .copied()
                .filter(|receiver| *receiver != direction)
                .collect()
        })
    }
    pub(super) fn segment_busy(&self, direction: usize, d: &NetworkDelta) -> bool {
        self.segment_for(direction)
            .is_some_and(|i| d.segment_active.as_ref().unwrap_or(&self.segment_active)[i].is_some())
    }
    pub(super) fn segment_reserve(
        &self,
        direction: usize,
        owner: SegmentOwner,
        eof: u64,
        release: u64,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(i) = self.segment_for(direction) else {
            return Ok(());
        };
        let slots = d
            .segment_active
            .get_or_insert_with(|| self.segment_active.clone());
        if slots[i].is_some() || eof <= c.now() || release < eof {
            return Err(error("shared LAN already busy or timing invalid"));
        }
        slots[i] = Some(SegmentReservation {
            direction,
            owner: owner.clone(),
            sof_ps: c.now(),
            eof_ps: eof,
            release_ps: release,
        });
        self.segment_record(i, &owner, "tx_started", direction, None, d, c)
    }
    pub(super) fn segment_release(
        &self,
        direction: usize,
        owner: SegmentOwner,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(i) = self.segment_for(direction) else {
            return Ok(());
        };
        let slots = d
            .segment_active
            .get_or_insert_with(|| self.segment_active.clone());
        let active = slots[i]
            .take()
            .ok_or_else(|| error("shared LAN release without reservation"))?;
        if active.direction != direction || active.owner != owner || active.release_ps != c.now() {
            return Err(error("shared LAN release identity/time mismatch"));
        }
        self.segment_record(i, &owner, "tx_released", direction, None, d, c)
    }
    #[allow(clippy::too_many_arguments)] // Preserve the established owner transaction interface.
    pub(super) fn segment_record(
        &self,
        i: usize,
        owner: &SegmentOwner,
        kind: &str,
        direction: usize,
        receiver: Option<usize>,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let segment = &self.prepared.shared_lans[i];
        let source = &self.prepared.ethernet.directions[direction].from_port;
        let recipient = receiver
            .map(|r| paired(&self.prepared.ethernet.directions[r].from_port))
            .transpose()?;
        let id = format!("segment:{}:{}", segment.id, d.record_sequence);
        Self::record(
            c,
            d,
            "dir.network.segment",
            id,
            json!({
                "segment_id":segment.id,"kind":kind,"owner":owner,
                "source_port":source,"receiver_port":recipient,
                "bitrate_bps":segment.bitrate_bps.to_string(),"delay_ps":segment.delay_ps.to_string(),
                "receivers":segment.members.len()-1,
            }),
        )
    }
    pub(super) fn effective_link(
        &self,
        direction: usize,
        receiver: Option<usize>,
    ) -> Result<EthernetDirection> {
        let mut link = self.prepared.ethernet.directions[direction].clone();
        if let Some(receiver) = receiver {
            let ideal_member = self
                .segment_for(direction)
                .is_some_and(|i| self.prepared.shared_lans[i].members.contains(&receiver));
            let physical = self.phy_receiver_port(direction, receiver);
            if receiver == direction || (!ideal_member && physical.is_none()) {
                return Err(error("shared LAN receiver mismatch"));
            }
            let target = &self.prepared.ethernet.directions[receiver];
            link.destination = target.source;
            link.to_port = match physical {
                Some(port) => port.rx_port.clone(),
                None => paired(&target.from_port)?,
            };
        }
        Ok(link)
    }
}
