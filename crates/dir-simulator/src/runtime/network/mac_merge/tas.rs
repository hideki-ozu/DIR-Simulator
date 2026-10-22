//! Table 8-7 operations use real MM_CTL events before their GCL boundary.
use super::super::*;
use super::integration::MergeEvent;
use crate::types::ethernet::tsn::{GateOperation, Schedule};
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TasControl {
    pub at: u64,
    pub target: u64,
    pub hold: bool,
    pub schedule_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_generation: Option<u64>,
    pub generation: u64,
}
impl NetworkState {
    pub(super) fn merge_next_tas(
        &self,
        i: usize,
        d: &NetworkDelta,
        now: u64,
    ) -> Result<Option<TasControl>> {
        let Some(tsn) = &self.tsn else {
            return Ok(None);
        };
        let port = &self.prepared.ethernet.directions[i].from_port;
        let rate = self.prepared.ethernet.directions[i].bitrate_bps;
        let hold_advance =
            super::bit_duration(1240 + 512 * 3, rate).map_err(super::integration::err)?;
        let release_advance = super::bit_duration(96, rate).map_err(super::integration::err)?;
        Ok(tsn
            .next_gate_operation_clock_in(
                &d.tsn,
                port,
                now,
                hold_advance,
                release_advance,
                self.clock_for_port(d, port),
            )?
            .map(|candidate| TasControl {
                at: candidate.at,
                target: candidate.target_global_ps,
                hold: candidate.hold,
                schedule_generation: candidate.schedule_generation,
                clock_generation: candidate.clock_generation,
                generation: 0,
            }))
    }
    fn merge_tas_candidate(
        &self,
        i: usize,
        s: &Schedule,
        generation: u64,
        now: u64,
    ) -> Result<Option<TasControl>> {
        let rate = self.prepared.ethernet.directions[i].bitrate_bps;
        // Conservative maximum HRT for every negotiated addFragSize. Zero local
        // processing delay; releaseAdvance is the existing 96-bit serializer IPG.
        let hold_advance =
            super::bit_duration(1240 + 512 * 3, rate).map_err(super::integration::err)?;
        let release_advance = super::bit_duration(96, rate).map_err(super::integration::err)?;
        let mut best = None;
        for (j, e) in s.entries.iter().enumerate() {
            let (hold, advance) = match e.operation {
                GateOperation::SetGateStates => continue,
                GateOperation::SetAndHoldMac => (true, hold_advance),
                GateOperation::SetAndReleaseMac => (false, release_advance),
            };
            let base = u128::from(s.base_ps) + u128::from(s.prefix_ps[j]);
            let first = u128::from(now) + u128::from(advance) + 1;
            let k = if first > base {
                (first - base).div_ceil(u128::from(s.cycle_ps))
            } else {
                0
            };
            let target = base + k * u128::from(s.cycle_ps);
            let at = target - u128::from(advance);
            if let (Ok(at), Ok(target)) = (u64::try_from(at), u64::try_from(target)) {
                let candidate = TasControl {
                    at,
                    target,
                    hold,
                    schedule_generation: generation,
                    clock_generation: None,
                    generation: 0,
                };
                if best.is_none_or(|b: TasControl| candidate.at < b.at) {
                    best = Some(candidate);
                }
            }
        }
        Ok(best)
    }
    pub(super) fn merge_validate_tas_proof(
        &self,
        i: usize,
        control: TasControl,
        horizon: u64,
    ) -> Result<()> {
        if control.at > horizon || control.at == 0 {
            return Err(error("invalid historical TAS control time"));
        }
        let cfg = self
            .prepared
            .tsn
            .as_ref()
            .ok_or_else(|| error("TAS proof without shaper"))?;
        let name = &self.prepared.ethernet.directions[i].from_port;
        let mut source = cfg
            .outputs
            .iter()
            .find(|o| o.port == *name)
            .and_then(|o| o.tas.as_ref());
        let mut generation = 0;
        for update in cfg
            .updates
            .iter()
            .filter(|u| u.port == *name && u.effective_at_ps <= control.at)
        {
            source = Some(&update.schedule);
            generation += 1;
        }
        let mut expected = self
            .merge_tas_candidate(
                i,
                source.ok_or_else(|| error("TAS proof without GCL"))?,
                generation,
                control.at - 1,
            )?
            .ok_or_else(|| error("TAS proof without operation"))?;
        expected.generation = control.generation;
        if expected != control {
            return Err(error("TAS applied control not derived from GCL"));
        }
        Ok(())
    }
    pub(in crate::runtime::network) fn merge_plan_tas(
        &self,
        i: usize,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        let mut candidate = self.merge_next_tas(i, d, now)?;
        let old = self.merge_state(d).unwrap().ports[i].tas_control;
        if let Some(mut previous) = old {
            previous.generation = 0;
            if Some(previous) == candidate {
                return Ok(());
            }
        }
        self.due_mut(d).retain(|due|!matches!(&due.kind,Completion::MacMerge(MergeEvent::Tas{direction,..}) if *direction==i));
        let p = &mut self.merge_mut(d)?.ports[i];
        p.tas_generation = add(p.tas_generation, 1)?;
        p.tas_planned_at = now;
        if let Some(c) = &mut candidate {
            c.generation = p.tas_generation;
        }
        p.tas_control = candidate;
        if let Some(control) = candidate {
            self.merge_due(
                d,
                control.at,
                MergeEvent::Tas {
                    direction: i,
                    control,
                },
            );
        }
        Ok(())
    }
    pub(super) fn merge_tas_event(
        &self,
        i: usize,
        control: TasControl,
        d: &mut NetworkDelta,
        now: u64,
    ) -> Result<()> {
        if self.merge_state(d).unwrap().ports[i].tas_control != Some(control) {
            return Ok(());
        }
        let port_name = &self.prepared.ethernet.directions[i].from_port;
        if self.prepared.tsn.as_ref().is_some_and(|tsn| {
            tsn.updates
                .iter()
                .any(|u| u.port == *port_name && u.effective_at_ps == now)
        }) {
            self.merge_mut(d)?.ports[i].tas_control = None;
            return Ok(());
        }
        let active = self.merge_state(d).unwrap().ports[i].verify.p_active();
        if active {
            self.merge_mut(d)?.ports[i].tx.hold = control.hold;
            self.merge_mut(d)?.ports[i].applied_tas = Some(control);
        }
        self.merge_mut(d)?.ports[i].tas_control = None;
        self.merge_shaper_transition(i, d, now)?;
        self.merge_port_record(d, i, now, "tas_mm_ctl")?;
        let extra = json!({"request":if control.hold{"hold"}else{"release"},"applied":active,"target_ps":control.target.to_string(),"advance_ps":(control.target-control.at).to_string()});
        self.merge_mut(d)?
            .records
            .last_mut()
            .unwrap()
            .2
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.merge_plan_tas(i, d, now)
    }
}
