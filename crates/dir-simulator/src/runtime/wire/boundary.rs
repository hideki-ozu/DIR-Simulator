//! Successful-prefix physical RX accounting for externally supplied IP packets.
use crate::snapshot::wire::WireBoundaryRecord;
use crate::types::wire::BoundaryPlan;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Start,
    Arrival,
    Release,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundaryState {
    pub actual_start_ps: Option<u64>,
    pub actual_arrival_ps: Option<u64>,
    pub actual_release_ps: Option<u64>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metrics {
    pub rx_wire_ps: u64,
    pub rx_ifg_ps: u64,
    pub completed_frames: u64,
    pub completed_mac_bytes: u64,
    pub completed_bits: u64,
}
impl BoundaryState {
    pub fn plan(
        &self,
        plan: &BoundaryPlan,
        packet_id: &str,
        ingress_port: &str,
        event: Event,
        now: u64,
        effect_seq: u64,
    ) -> Result<(Self, WireBoundaryRecord), String> {
        let mut next = self.clone();
        let (event, status) = match event {
            Event::Start if now == plan.start_ps && self.actual_start_ps.is_none() => {
                next.actual_start_ps = Some(now);
                ("start", "receiving")
            }
            Event::Arrival
                if now == plan.arrival_ps
                    && self.actual_start_ps == Some(plan.start_ps)
                    && self.actual_arrival_ps.is_none() =>
            {
                next.actual_arrival_ps = Some(now);
                ("arrival", "received")
            }
            Event::Release
                if now == plan.release_ps
                    && self.actual_arrival_ps == Some(plan.arrival_ps)
                    && self.actual_release_ps.is_none() =>
            {
                next.actual_release_ps = Some(now);
                ("release", "released")
            }
            _ => return Err("invalid boundary phase/time/prefix".into()),
        };
        let r = WireBoundaryRecord {
            effect_seq: effect_seq.to_string(),
            packet_id: packet_id.into(),
            ingress_port: ingress_port.into(),
            time_ps: now.to_string(),
            event: event.into(),
            status: status.into(),
            actual_start_ps: next.actual_start_ps.map(|n| n.to_string()),
            actual_arrival_ps: next.actual_arrival_ps.map(|n| n.to_string()),
            actual_release_ps: next.actual_release_ps.map(|n| n.to_string()),
        };
        Ok((next, r))
    }
    pub fn metrics(&self, plan: &BoundaryPlan, horizon: u64) -> Result<Metrics, String> {
        let mut m = Metrics::default();
        if self.actual_start_ps.is_some() {
            m.rx_wire_ps = horizon.min(plan.arrival_ps).saturating_sub(plan.start_ps);
        }
        if self.actual_arrival_ps.is_some() {
            m.rx_ifg_ps = horizon.min(plan.release_ps).saturating_sub(plan.arrival_ps);
            m.completed_frames = 1;
            m.completed_mac_bytes = plan.mac_bytes;
            m.completed_bits = plan
                .mac_bytes
                .checked_add(8)
                .and_then(|n| n.checked_mul(8))
                .ok_or("boundary completed bits overflow")?;
        }
        Ok(m)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_prefix_only() {
        let p = BoundaryPlan {
            start_ps: 10,
            arrival_ps: 20,
            release_ps: 22,
            mac_bytes: 68,
            bitrate_bps: 100_000_000,
        };
        let s = BoundaryState::default();
        assert_eq!(s.metrics(&p, 100).unwrap(), Metrics::default());
        let (s, _) = s.plan(&p, "p", "in", Event::Start, 10, 1).unwrap();
        let m = s.metrics(&p, 15).unwrap();
        assert_eq!((m.rx_wire_ps, m.completed_frames, m.rx_ifg_ps), (5, 0, 0));
        let (s, _) = s.plan(&p, "p", "in", Event::Arrival, 20, 2).unwrap();
        let m = s.metrics(&p, 21).unwrap();
        assert_eq!((m.rx_wire_ps, m.rx_ifg_ps, m.completed_bits), (10, 1, 608));
        assert_eq!(s.metrics(&p, 21).unwrap(), m);
    }
}
