//! Versioned named-PHY synchronization at actual observable sample boundaries.
use super::*;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BoundaryState {
    pub generation: u64,
    pub pending: Option<BoundaryWake>,
    /// An optimization of an immutable physical prefix, never checkpoint state.
    #[serde(skip)]
    forecast: Option<Arc<CachedForecast>>,
}

#[derive(Clone, Debug)]
struct CachedForecast {
    base_sample: u64,
    prefix: crate::runtime::ethernet::phy::attachment::AttachmentForecast,
}
impl PartialEq for BoundaryState {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation && self.pending == other.pending
    }
}
impl Eq for BoundaryState {}
impl BoundaryState {
    pub(super) fn invalidate_forecast(&mut self) {
        self.forecast = None;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BoundaryWake {
    pub sample_cut: u64,
    pub effects_present: bool,
    pub exhausted: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uncertain: bool,
    pub token: TimerToken,
}

impl NetworkState {
    /// New queue/control demand can make an earlier GMII cut observable even
    /// when no PHY field changed. Reserve that cut before returning arbitration.
    pub(super) fn phy_replan_raw_demand(
        &self,
        index: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let Some(cut) = self.merge_phy_next_cut(index, d)? else {
            return Ok(());
        };
        let cursor = d.phy.as_ref().unwrap_or(&self.phy)[index].next_sample_index();
        let pending = &d.phy_boundaries.as_ref().unwrap_or(&self.phy_boundaries)[index].pending;
        if cut > cursor && pending.as_ref().is_none_or(|wake| cut < wake.sample_cut) {
            self.phy_plan_boundary(index, d, c)?;
        }
        Ok(())
    }
    pub(super) fn phy_plan_boundary(
        &self,
        index: usize,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
    ) -> Result<()> {
        let previous = d.phy_boundaries.as_ref().unwrap_or(&self.phy_boundaries)[index].clone();
        let medium = &d.phy.as_ref().unwrap_or(&self.phy)[index];
        let cfg = medium.config();
        let mut count = u64::from(cfg.lookahead_samples())
            .min(if cfg.medium.trace_samples {
                4096
            } else {
                1_000_000
            })
            .min(
                cfg.medium
                    .maximum_samples
                    .saturating_sub(medium.next_sample_index()),
            );
        if let Some(cut) = self.merge_phy_next_cut(index, d)? {
            if let Some(distance) = cut
                .checked_sub(medium.next_sample_index())
                .filter(|n| *n > 0)
            {
                count = count.min(distance);
            }
        }
        let (sample_cut, effects_present, exhausted, uncertain, forecast) = if count == 0 {
            // The final permitted sample commits; only the next attempt fails.
            (
                add(medium.next_sample_index(), 1)?,
                false,
                true,
                false,
                None,
            )
        } else {
            match medium.forecast_until_first_effect(count) {
                Ok(prediction) => {
                    if prediction.samples == 0
                        || prediction.end_sample <= medium.next_sample_index()
                    {
                        return Err(error("named PHY predicted an empty sample interval"));
                    }
                    (
                        prediction.end_sample,
                        !prediction.effects.is_empty(),
                        false,
                        false,
                        Some(Arc::new(CachedForecast {
                            base_sample: medium.next_sample_index(),
                            prefix: prediction,
                        })),
                    )
                }
                // A speculative failure is not an event at the current time.
                // Advance one real sample at a time until its actual failing cut.
                Err(_) => (
                    add(medium.next_sample_index(), 1)?,
                    false,
                    false,
                    true,
                    None,
                ),
            }
        };
        let at = medium.sample_time_ps(sample_cut).map_err(error)?;
        if at < c.now() {
            return Err(error("named PHY boundary reservation is not in the future"));
        }
        if let Some(old) = previous.pending {
            if c.cancel(&old.token) {
                d.pending_direct = d
                    .pending_direct
                    .checked_sub(1)
                    .ok_or_else(|| error("named PHY cancellation count underflow"))?;
            }
        }
        let generation = add(previous.generation, 1)?;
        let token = Self::schedule(
            c,
            d,
            at,
            Event::PhyBoundary {
                attachment: index,
                generation,
                sample_cut,
            },
        )?;
        d.phy_boundaries
            .get_or_insert_with(|| self.phy_boundaries.clone())[index] = BoundaryState {
            generation,
            forecast,
            pending: Some(BoundaryWake {
                sample_cut,
                effects_present,
                exhausted,
                uncertain,
                token,
            }),
        };
        Ok(())
    }

    /// Phase0 controls synchronize strictly before now; phase1 callbacks and
    /// arbitration synchronize through now. Both scalar-new and batched-new
    /// use this rule, independently of incidental reservation IDs.
    pub(super) fn phy_synchronize_boundaries(
        &self,
        d: &mut NetworkDelta,
        c: &mut Context<'_>,
        inclusive: bool,
    ) -> Result<()> {
        for index in 0..self.phy.len() {
            if !self.prepared.phy_attachments[index]
                .config
                .medium
                .physical_mode
                .is_named()
            {
                continue;
            }
            let state = d.phy_boundaries.as_ref().unwrap_or(&self.phy_boundaries)[index].clone();
            let Some(pending) = state.pending else {
                continue;
            };
            let medium = &d.phy.as_ref().unwrap_or(&self.phy)[index];
            let start = medium.next_sample_index();
            let at = medium.sample_time_ps(pending.sample_cut).map_err(error)?;
            if at < c.now() {
                return Err(error("named PHY observable boundary was missed"));
            }
            if pending.exhausted {
                if inclusive && at == c.now() {
                    return Err(error("named PHY sample budget exhausted"));
                }
                continue;
            }
            let (mut low, mut high) = (start, pending.sample_cut);
            while low < high {
                let mid = low + (high - low).div_ceil(2);
                let time = medium.sample_time_ps(mid).map_err(error)?;
                if time < c.now() || inclusive && time == c.now() {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            let cut = low;
            if cut == start {
                continue;
            }
            let effects = if cut == pending.sample_cut {
                if let Some(cached) = state.forecast.filter(|cached| cached.base_sample == start) {
                    let effects = cached.prefix.effects.clone();
                    *self.phy_mut(d, index)? = cached.prefix.end_state.clone();
                    effects
                } else {
                    self.phy_mut(d, index)?
                        .advance_samples(cut - start)
                        .map_err(error)?
                }
            } else {
                self.phy_mut(d, index)?
                    .advance_samples(cut - start)
                    .map_err(error)?
            };
            if !effects.is_empty() && (cut != pending.sample_cut || at != c.now() || !inclusive) {
                return Err(error(
                    "named PHY effect discovered outside its reserved boundary",
                ));
            }
            if cut == pending.sample_cut
                && !pending.uncertain
                && effects.is_empty() == pending.effects_present
            {
                return Err(error(
                    "named PHY prediction differs from exact sample execution",
                ));
            }
            if !effects.is_empty() {
                self.phy_effects(index, cut - 1, effects, d, c)?;
            }
            if cut == pending.sample_cut {
                self.phy_plan_boundary(index, d, c)?;
            }
        }
        Ok(())
    }

    pub(super) fn phy_boundary_event(
        &self,
        index: usize,
        generation: u64,
        cut: u64,
        d: &NetworkDelta,
        c: &Context<'_>,
    ) -> Result<()> {
        let state = self
            .phy_boundaries
            .get(index)
            .ok_or_else(|| error("unknown named PHY boundary attachment"))?;
        let pending = state
            .pending
            .as_ref()
            .ok_or_else(|| error("named PHY boundary has no reservation"))?;
        if state.generation != generation
            || pending.sample_cut != cut
            || c.current_timer_token() != Some(&pending.token)
            || self.phy[index].sample_time_ps(cut).map_err(error)? != c.now()
            || d.phy.as_ref().unwrap_or(&self.phy)[index].next_sample_index() != cut
        {
            return Err(error("named PHY boundary reservation/cut differs"));
        }
        Ok(())
    }

    pub(super) fn validate_phy_boundaries(&self, horizon: u64) -> Result<()> {
        if self.phy_boundaries.len() != self.phy.len() {
            return Err(error("named PHY boundary dimensions differ"));
        }
        for (index, state) in self.phy_boundaries.iter().enumerate() {
            let medium = &self.phy[index];
            if !medium.config().medium.physical_mode.is_named() {
                if state != &BoundaryState::default() {
                    return Err(error("legacy PHY has named boundary state"));
                }
                continue;
            }
            let pending = state
                .pending
                .as_ref()
                .ok_or_else(|| error("named PHY wake missing"))?;
            let cursor = medium.next_sample_index();
            if cursor > 0 && medium.sample_time_ps(cursor).map_err(error)? > horizon {
                return Err(error(
                    "named PHY authoritative cursor is beyond committed horizon",
                ));
            }
            let count = pending
                .sample_cut
                .checked_sub(cursor)
                .filter(|count| {
                    *count > 0 && *count <= u64::from(medium.config().lookahead_samples())
                })
                .ok_or_else(|| error("named PHY saved boundary range differs"))?;
            if state.generation == 0
                || medium.sample_time_ps(pending.sample_cut).map_err(error)? < horizon
            {
                return Err(error("named PHY saved boundary time/generation differs"));
            }
            if pending.exhausted {
                if cursor != medium.config().medium.maximum_samples
                    || pending.sample_cut != add(cursor, 1)?
                    || pending.effects_present
                    || pending.uncertain
                {
                    return Err(error(
                        "named PHY exhausted reservation differs from actual budget",
                    ));
                }
                continue;
            }
            if pending.uncertain {
                if count != 1
                    || pending.effects_present
                    || medium
                        .forecast_until_first_effect(
                            u64::from(medium.config().lookahead_samples()).min(
                                medium
                                    .config()
                                    .medium
                                    .maximum_samples
                                    .saturating_sub(cursor),
                            ),
                        )
                        .is_ok()
                {
                    return Err(error("named PHY speculative-failure reservation differs"));
                }
                continue;
            }
            let expected = medium.predict_until_first_effect(count).map_err(error)?;
            if expected.end_sample != pending.sample_cut
                || expected.effects_present != pending.effects_present
            {
                return Err(error(
                    "named PHY saved wake is not its first observable boundary",
                ));
            }
        }
        Ok(())
    }
}
