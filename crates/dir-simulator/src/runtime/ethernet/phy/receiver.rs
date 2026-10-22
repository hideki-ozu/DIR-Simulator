//! Causal front-end/ADC, trained timing acquisition and normalized-LMS equalizer.
//! Training labels are known PHY training symbols; payload decisions use samples only.
use super::ieee::t1s::{self, ReceivedControlReference};
use super::*;
use std::{collections::VecDeque, sync::Arc};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverConfig {
    pub grid: SampleGrid,
    pub peak_uv: i64,
    pub frontend_taps_q30: Vec<i64>,
    pub adc_bits: u8,
    pub adc_full_scale_uv: i64,
    /// Symbol-spaced feed-forward equalizer history at each recovered sample phase.
    pub ffe_taps: usize,
    /// Symbol-spaced known-local history. Fractional-UI echo is approximated by
    /// this basis; the declared accuracy envelope must test the chosen path.
    pub echo_taps: usize,
    pub adaptation_q30: i64,
    pub minimum_training_symbols: u32,
    /// RMS acquisition error relative to peak, in millionths.
    pub lock_error_ppm: u32,
    /// Explicit receiver oscillator offset; fractional cursor survives every batch.
    pub clock_ppm: i32,
    /// Supported unknown partner drift relative to this oscillator. Zero disables tracking.
    pub max_tracking_ppm: u32,
    /// Declared signalling envelope: a longer transition-free run loses timing lock.
    pub max_no_transition_symbols: u32,
    pub loss_error_ppm: u32,
}
impl ReceiverConfig {
    pub fn validate(&self) -> Result<()> {
        self.grid.validate()?;
        validate_taps(&self.frontend_taps_q30)?;
        if self.peak_uv <= 0
            || self.peak_uv > 10_000_000
            || !(4..=24).contains(&self.adc_bits)
            || self.adc_full_scale_uv < self.peak_uv
            || self.adc_full_scale_uv > 1_000_000_000
            || !(1..=16).contains(&self.ffe_taps)
            || self.echo_taps > 16
            || !(0..=Q30 / 2).contains(&self.adaptation_q30)
            || !(16..=4096).contains(&self.minimum_training_symbols)
            || self.lock_error_ppm > 500_000
            || self.clock_ppm.unsigned_abs() > 10_000
            || self.max_tracking_ppm > 10_000
            || self.max_tracking_ppm > 0 && self.grid.samples_per_ui % 2 != 0
            || !(8..=4096).contains(&self.max_no_transition_symbols)
            || !(1..=1_000_000).contains(&self.loss_error_ppm)
            || self.loss_error_ppm < self.lock_error_ppm
        {
            return Err("invalid numerical RX ADC/equalizer/training/clock config".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Acquisition {
    Training,
    Locked,
    Lost(LossReason),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LossReason {
    TimingRange,
    TransitionTimeout,
    DecisionError,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameToken {
    pub frame_id: String,
    pub epoch: u64,
    pub serial: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ReceiverEvent {
    TrainingStarted {
        epoch: u64,
        sample_index: u64,
    },
    LockAcquired {
        epoch: u64,
        sample_index: u64,
    },
    LockLost {
        epoch: u64,
        sample_index: u64,
        reason: LossReason,
    },
    FrameAborted {
        token: FrameToken,
        sample_index: u64,
        reason: LossReason,
    },
    FrameCompleted {
        token: FrameToken,
        sample_index: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolDecision {
    pub ordinal: u64,
    pub epoch: u64,
    pub sample_index: u64,
    pub symbol: i8,
    pub voltage_uv: i64,
    pub training: bool,
    pub acquisition: Acquisition,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    ffe: Vec<i64>,
    echo: Vec<i64>,
    rx: VecDeque<i64>,
    local: VecDeque<i64>,
    errors: VecDeque<u128>,
}
impl Candidate {
    fn new(config: &ReceiverConfig) -> Self {
        let mut ffe = vec![0; config.ffe_taps];
        ffe[0] = Q30;
        Self {
            ffe,
            echo: vec![0; config.echo_taps],
            rx: VecDeque::new(),
            local: VecDeque::new(),
            errors: VecDeque::new(),
        }
    }
    fn advance(&mut self, rx: i64, local: i64) -> Result<i64> {
        self.rx.push_front(rx);
        self.rx.truncate(self.ffe.len());
        self.local.push_front(local);
        self.local.truncate(self.echo.len());
        self.output_with(&self.ffe, &self.echo)
    }
    fn output_with(&self, ffe: &[i64], echo: &[i64]) -> Result<i64> {
        let mut sum = 0i128;
        for (&x, &weight) in self.rx.iter().zip(ffe) {
            sum = sum
                .checked_add(x as i128 * weight as i128)
                .ok_or("RX equalizer sum overflow")?;
        }
        for (&x, &weight) in self.local.iter().zip(echo) {
            sum = sum
                .checked_sub(x as i128 * weight as i128)
                .ok_or("RX echo sum overflow")?;
        }
        rounded_q30(sum)
    }
    fn train(&mut self, target: i64, observed: i64, config: &ReceiverConfig) -> Result<()> {
        let error = target
            .checked_sub(observed)
            .ok_or("RX training error overflow")?;
        let square = (error as i128)
            .checked_mul(error as i128)
            .ok_or("RX squared error overflow")? as u128;
        self.errors.push_back(square);
        if self.errors.len() > 64 {
            self.errors.pop_front();
        }
        let floor = (config.peak_uv as i128 * config.peak_uv as i128 / 64).max(1);
        let power = self
            .rx
            .iter()
            .chain(&self.local)
            .try_fold(floor, |sum, &x| {
                sum.checked_add(x as i128 * x as i128)
                    .ok_or("RX adaptation energy overflow")
            })?;
        for (weight, &x) in self.ffe.iter_mut().zip(&self.rx) {
            let delta = config.adaptation_q30 as i128 * error as i128 * x as i128 / power;
            *weight = (*weight as i128 + delta).clamp(-16 * Q30 as i128, 16 * Q30 as i128) as i64;
        }
        for (weight, &x) in self.echo.iter_mut().zip(&self.local) {
            let delta = config.adaptation_q30 as i128 * error as i128 * x as i128 / power;
            *weight = (*weight as i128 - delta).clamp(-16 * Q30 as i128, 16 * Q30 as i128) as i64;
        }
        Ok(())
    }
    fn score(&self) -> u128 {
        if self.errors.is_empty() {
            u128::MAX
        } else {
            self.errors.iter().sum::<u128>() / self.errors.len() as u128
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    index: u64,
    rx: i64,
    local: i64,
    #[serde(default)]
    clipped: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlWindow {
    ordinal: u64,
    phases: Vec<Observation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlAcquisition {
    epoch: u64,
    #[serde(default)]
    recent: VecDeque<(u64, usize, Observation)>,
    #[serde(with = "control_windows")]
    windows: Arc<Vec<ControlWindow>>,
    reference: Option<ReceivedControlReference>,
    prefix_published: bool,
    prefix_pending: Vec<i8>,
}
mod control_windows {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Arc<Vec<ControlWindow>>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        value.as_ref().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Arc<Vec<ControlWindow>>, D::Error> {
        Ok(Arc::new(Vec::<ControlWindow>::deserialize(deserializer)?))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receiver {
    config: ReceiverConfig,
    next_index: u64,
    ordinal: u64,
    phase: i64,
    frequency_q30: i64,
    previous_values: Option<Vec<i64>>,
    previous_symbol: Option<i8>,
    no_transition: u32,
    range_hits: u8,
    decision_errors: VecDeque<u128>,
    epoch: u64,
    frame_serial: u64,
    active_frame: Option<FrameToken>,
    events: VecDeque<ReceiverEvent>,
    frontend: VecDeque<i64>,
    window: Vec<Observation>,
    candidates: Vec<Candidate>,
    training: VecDeque<i8>,
    trained: u32,
    selected_phase: usize,
    acquisition: Acquisition,
    clipped_samples: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_acquisition: Option<ControlAcquisition>,
}
impl Receiver {
    pub fn new(config: ReceiverConfig) -> Result<Self> {
        config.validate()?;
        let n = config.grid.samples_per_ui as usize;
        let candidates = (0..n).map(|_| Candidate::new(&config)).collect();
        Ok(Self {
            config,
            next_index: 0,
            ordinal: 0,
            phase: 0,
            frequency_q30: 0,
            previous_values: None,
            previous_symbol: None,
            no_transition: 0,
            range_hits: 0,
            decision_errors: VecDeque::new(),
            epoch: 0,
            frame_serial: 0,
            active_frame: None,
            events: VecDeque::new(),
            frontend: VecDeque::new(),
            window: vec![],
            candidates,
            training: VecDeque::new(),
            trained: 0,
            selected_phase: n / 2,
            acquisition: Acquisition::Training,
            clipped_samples: 0,
            control_acquisition: None,
        })
    }
    /// Reset only acquisition. Physical oscillator, cursor, partial sampling
    /// window and analog frontend remain continuous across carrier epochs.
    pub(crate) fn begin_control_acquisition(&mut self) -> Result<()> {
        if self.active_frame.is_some() {
            return Err("abort active frame before control acquisition".into());
        }
        self.epoch = self.epoch.checked_add(1).ok_or("RX epoch overflow")?;
        self.ordinal = 0;
        self.trained = 0;
        self.candidates = (0..self.config.grid.samples_per_ui)
            .map(|_| Candidate::new(&self.config))
            .collect();
        self.training.clear();
        self.decision_errors.clear();
        self.no_transition = 0;
        self.acquisition = Acquisition::Training;
        self.control_acquisition = Some(ControlAcquisition {
            epoch: self.epoch,
            recent: VecDeque::new(),
            windows: Arc::new(vec![]),
            reference: None,
            prefix_published: false,
            prefix_pending: vec![],
        });
        self.emit(ReceiverEvent::TrainingStarted {
            epoch: self.epoch,
            sample_index: self.next_index,
        })
    }
    pub(crate) fn take_control_prefix(&mut self) -> Vec<i8> {
        self.control_acquisition
            .as_mut()
            .map(|state| std::mem::take(&mut state.prefix_pending))
            .unwrap_or_default()
    }
    /// Every later control group must still match this receiver's own recent
    /// ADC evidence. Earlier acquisition confidence cannot label a new signal.
    pub(crate) fn control_group_valid(&self, group: u8) -> bool {
        let Some(state) = &self.control_acquisition else {
            return false;
        };
        if state.reference.as_ref().is_some_and(|r| {
            r.groups.last() == Some(&group)
                && r.sample_indices.last() == state.recent.back().map(|(_, _, s)| &s.index)
        }) {
            return true;
        }
        if state.recent.len() < 10 {
            return false;
        }
        let recent = state.recent.iter().rev().take(10).collect::<Vec<_>>();
        if recent.iter().any(|(_, phase, s)| {
            *phase != self.selected_phase || s.clipped || s.rx.abs() < self.config.peak_uv / 2
        }) {
            return false;
        }
        let levels = recent
            .iter()
            .rev()
            .map(|(_, _, s)| if s.rx > 0 { 1 } else { -1 })
            .collect::<Vec<_>>();
        let Ok((decoded, _)) = t1s::decode_aligned_chips(&levels, -levels[0]) else {
            return false;
        };
        if decoded != group {
            return false;
        }
        let energy = recent
            .iter()
            .map(|(_, _, s)| {
                let residual = s.rx.abs() as i128 - self.config.peak_uv as i128;
                (residual * residual) as u128
            })
            .sum::<u128>();
        let limit = self.config.peak_uv as u128 * self.config.lock_error_ppm as u128;
        energy / 10 <= limit * limit / 1_000_000_000_000
    }
    fn control_reference(&self, state: &ControlAcquisition) -> Option<ReceivedControlReference> {
        let mut accepted: Vec<(ReceivedControlReference, u128)> = vec![];
        let limit = self.config.peak_uv as u128 * self.config.lock_error_ppm as u128;
        let allowed = limit * limit / 1_000_000_000_000;
        for phase in 0..self.config.grid.samples_per_ui as usize {
            let chips = state
                .windows
                .iter()
                .map(|window| {
                    let sample = &window.phases[phase];
                    let chip = if sample.clipped || sample.rx.abs() < self.config.peak_uv / 2 {
                        0
                    } else if sample.rx > 0 {
                        1
                    } else {
                        -1
                    };
                    (window.ordinal, sample.index, chip)
                })
                .collect::<Vec<_>>();
            let Some(reference) = t1s::received_control_reference(state.epoch, phase, &chips)
            else {
                continue;
            };
            let first = state
                .windows
                .iter()
                .position(|w| w.ordinal == reference.first_ordinal)?;
            let energy = state.windows[first..first + reference.chips.len()]
                .iter()
                .zip(&reference.chips)
                .map(|(w, chip)| {
                    let residual =
                        w.phases[phase].rx as i128 - *chip as i128 * self.config.peak_uv as i128;
                    (residual * residual) as u128
                })
                .sum::<u128>();
            if energy / reference.chips.len() as u128 <= allowed {
                accepted.push((reference, energy));
            }
        }
        let earliest = accepted.iter().map(|(r, _)| r.first_ordinal).min()?;
        accepted.retain(|(r, _)| r.first_ordinal == earliest);
        let (first, _) = accepted.first()?;
        if accepted.iter().any(|(other, _)| {
            let count = other.chips.len().min(first.chips.len());
            other.chips[..count] != first.chips[..count]
        }) {
            return None;
        }
        let longest = accepted.iter().map(|(r, _)| r.chips.len()).max()?;
        accepted.retain(|(r, _)| r.chips.len() == longest);
        accepted
            .into_iter()
            .min_by_key(|(r, energy)| {
                (
                    *energy,
                    r.phase
                        .abs_diff(self.config.grid.samples_per_ui as usize / 2),
                    r.phase,
                )
            })
            .map(|(r, _)| r)
    }
    fn replay_control_candidates(
        &self,
        state: &ControlAcquisition,
        reference: &ReceivedControlReference,
    ) -> Result<Vec<Candidate>> {
        let mut candidates = (0..self.config.grid.samples_per_ui)
            .map(|_| Candidate::new(&self.config))
            .collect::<Vec<_>>();
        for window in state.windows.iter() {
            let label = window
                .ordinal
                .checked_sub(reference.first_ordinal)
                .and_then(|offset| reference.chips.get(offset as usize));
            for (phase, candidate) in candidates.iter_mut().enumerate() {
                let sample = &window.phases[phase];
                let value = candidate.advance(sample.rx, sample.local)?;
                if let Some(chip) = label {
                    candidate.train(*chip as i64 * self.config.peak_uv, value, &self.config)?;
                }
            }
        }
        Ok(candidates)
    }
    fn advance_control_acquisition(&mut self) -> Result<()> {
        let Some(mut state) = self.control_acquisition.take() else {
            return Ok(());
        };
        if state
            .reference
            .as_ref()
            .is_some_and(|r| r.chips.len() >= self.config.minimum_training_symbols as usize)
        {
            self.control_acquisition = Some(state);
            return Ok(());
        }
        let n = self.config.grid.samples_per_ui as usize;
        Arc::make_mut(&mut state.windows).push(ControlWindow {
            ordinal: self.ordinal,
            phases: (0..n)
                .map(|phase| {
                    self.window[(phase * self.window.len() / n).min(self.window.len() - 1)].clone()
                })
                .collect(),
        });
        if state.windows.len() > 64 {
            // An incomplete candidate is discarded; no unbounded training history.
            Arc::make_mut(&mut state.windows).clear();
            state.reference = None;
            state.prefix_published = false;
            state.prefix_pending.clear();
            self.trained = 0;
            for candidate in &mut self.candidates {
                candidate.errors.clear();
            }
        }
        if let Some(reference) = self.control_reference(&state) {
            self.candidates = self.replay_control_candidates(&state, &reference)?;
            self.selected_phase = reference.phase;
            self.trained = reference.chips.len() as u32;
            if !state.prefix_published {
                state.prefix_pending = reference.chips[..20].to_vec();
                state.prefix_published = true;
            }
            if self.trained >= self.config.minimum_training_symbols {
                self.acquisition = Acquisition::Locked;
                self.emit(ReceiverEvent::LockAcquired {
                    epoch: self.epoch,
                    sample_index: self.next_index,
                })?;
            }
            state.reference = Some(reference);
        }
        self.control_acquisition = Some(state);
        Ok(())
    }
    fn validate_control_acquisition(&self) -> Result<()> {
        let Some(state) = &self.control_acquisition else {
            return Ok(());
        };
        let n = self.config.grid.samples_per_ui as usize;
        if state.epoch != self.epoch
            || state.recent.len() > 64
            || state.recent.iter().any(|(ordinal, phase, s)| {
                *ordinal >= self.ordinal
                    || *phase >= n
                    || s.index >= self.next_index
                    || s.rx.unsigned_abs() > self.config.adc_full_scale_uv as u64
            })
            || state
                .recent
                .iter()
                .zip(state.recent.iter().skip(1))
                .any(|((a, _, s), (b, _, t))| a.checked_add(1) != Some(*b) || s.index >= t.index)
            || state.windows.len() > 64
            || state.windows.iter().any(|w| {
                w.phases.len() != n
                    || w.phases.iter().any(|s| {
                        s.index >= self.next_index
                            || s.rx.unsigned_abs() > self.config.adc_full_scale_uv as u64
                    })
            })
            || state
                .windows
                .windows(2)
                .any(|w| w[0].ordinal + 1 != w[1].ordinal)
        {
            return Err("invalid received-control ADC history".into());
        }
        let reconstructed = self.control_reference(state);
        if reconstructed != state.reference
            || state.prefix_published != state.reference.is_some()
            || !state.prefix_pending.is_empty()
                && state
                    .reference
                    .as_ref()
                    .is_none_or(|r| state.prefix_pending != r.chips[..20])
            || state.reference.as_ref().is_some_and(|r| {
                r.phase != self.selected_phase
                    || r.chips.len() != self.trained as usize
                    || (self.acquisition == Acquisition::Locked)
                        != (r.chips.len() >= self.config.minimum_training_symbols as usize)
            })
        {
            return Err(
                "received-control reference/phase/count/lock differs from ADC evidence".into(),
            );
        }
        if let Some(reference) = &state.reference {
            let replay = self.replay_control_candidates(state, reference)?;
            if self
                .candidates
                .iter()
                .zip(replay)
                .any(|(current, expected)| {
                    current.ffe != expected.ffe
                        || current.echo != expected.echo
                        || current.errors != expected.errors
                })
            {
                return Err(
                    "received-control adaptation differs from chronological evidence".into(),
                );
            }
        }
        Ok(())
    }
    pub fn queue_training(&mut self, symbols: &[i8]) -> Result<()> {
        if self.training.len() + symbols.len() > MAX_BLOCK_SAMPLES
            || symbols.iter().any(|s| !(-1..=1).contains(s))
        {
            return Err("invalid/bounded RX training sequence".into());
        }
        if matches!(self.acquisition, Acquisition::Lost(_)) {
            return Err("lost RX requires start_training epoch".into());
        }
        self.training.extend(symbols);
        Ok(())
    }
    /// Starts a new observable training epoch at the next input sample. Codec
    /// and frame assemblers must discard the old epoch on the returned events.
    pub fn start_training(&mut self, symbols: &[i8]) -> Result<()> {
        if symbols.len() > MAX_BLOCK_SAMPLES || symbols.iter().any(|s| !(-1..=1).contains(s)) {
            return Err("invalid retraining sequence".into());
        }
        if self.active_frame.is_some() {
            return Err("explicitly abort active frame before administrative retraining".into());
        }
        let mut candidate = Self::new(self.config.clone())?;
        candidate.next_index = self.next_index;
        candidate.frontend = self.frontend.clone();
        candidate.clipped_samples = self.clipped_samples;
        candidate.epoch = self.epoch.checked_add(1).ok_or("RX epoch overflow")?;
        candidate.frame_serial = self.frame_serial;
        candidate.events = self.events.clone();
        candidate.emit(ReceiverEvent::TrainingStarted {
            epoch: candidate.epoch,
            sample_index: self.next_index,
        })?;
        candidate.training.extend(symbols);
        *self = candidate;
        Ok(())
    }
    pub fn begin_frame(&mut self, frame_id: &str) -> Result<FrameToken> {
        if self.acquisition != Acquisition::Locked
            || self.active_frame.is_some()
            || frame_id.is_empty()
            || frame_id.len() > 1024
        {
            return Err("frame requires locked RX and no active frame".into());
        }
        self.frame_serial = self
            .frame_serial
            .checked_add(1)
            .ok_or("RX frame serial overflow")?;
        let token = FrameToken {
            frame_id: frame_id.into(),
            epoch: self.epoch,
            serial: self.frame_serial,
        };
        self.active_frame = Some(token.clone());
        Ok(token)
    }
    pub fn finish_frame(&mut self, token: &FrameToken) -> Result<()> {
        if self.acquisition != Acquisition::Locked
            || self.active_frame.as_ref() != Some(token)
            || token.epoch != self.epoch
        {
            return Err("stale/unlocked RX frame completion".into());
        }
        self.emit(ReceiverEvent::FrameCompleted {
            token: token.clone(),
            sample_index: self.next_index,
        })?;
        self.active_frame = None;
        Ok(())
    }
    /// A codec failure aborts its active frame without asserting loss of timing lock.
    pub fn abort_frame(&mut self, token: &FrameToken) -> Result<()> {
        if self.active_frame.as_ref() != Some(token) {
            return Err("stale RX frame abort".into());
        }
        self.emit(ReceiverEvent::FrameAborted {
            token: token.clone(),
            sample_index: self.next_index,
            reason: LossReason::DecisionError,
        })?;
        self.active_frame = None;
        Ok(())
    }
    pub fn take_events(&mut self) -> Vec<ReceiverEvent> {
        self.events.drain(..).collect()
    }
    pub fn estimated_partner_offset_ppm(&self) -> i64 {
        self.frequency_q30 / Q30
    }
    /// The production adapter can split at this exact next possible state-change boundary.
    pub fn samples_until_decision(&self) -> usize {
        let ui = self.config.grid.samples_per_ui as i64 * 1_000_000;
        let step = 1_000_000 + self.config.clock_ppm as i64 + self.frequency_q30 / Q30;
        ((ui - self.phase + step - 1) / step) as usize
    }
    fn emit(&mut self, event: ReceiverEvent) -> Result<()> {
        if self.events.len() >= 1024 {
            return Err("RX event queue must be drained at observable boundaries".into());
        }
        self.events.push_back(event);
        Ok(())
    }
    fn lose_lock(&mut self, reason: LossReason, index: u64) -> Result<()> {
        if matches!(self.acquisition, Acquisition::Lost(_)) {
            return Ok(());
        }
        if let Some(token) = self.active_frame.take() {
            self.emit(ReceiverEvent::FrameAborted {
                token,
                sample_index: index,
                reason: reason.clone(),
            })?;
        }
        self.emit(ReceiverEvent::LockLost {
            epoch: self.epoch,
            sample_index: index,
            reason: reason.clone(),
        })?;
        self.epoch = self.epoch.checked_add(1).ok_or("RX loss epoch overflow")?;
        self.acquisition = Acquisition::Lost(reason);
        self.control_acquisition = None;
        self.training.clear();
        Ok(())
    }
    fn track_timing(&mut self, values: &[(u64, i64)], index: u64) -> Result<()> {
        if self.config.max_tracking_ppm == 0
            || self.trained < 32
            || matches!(self.acquisition, Acquisition::Lost(_))
        {
            return Ok(());
        }
        if let Some(previous) = &self.previous_values {
            let n = values.len();
            let p = self.selected_phase;
            let midpoint = if p >= n / 2 {
                values[p - n / 2].1
            } else {
                previous[p + n / 2]
            };
            let old = previous[p];
            let current = values[p].1;
            let power = self.config.peak_uv as i128 * self.config.peak_uv as i128;
            let error = (midpoint as i128 * (old as i128 - current as i128) * Q30 as i128 / power)
                .clamp(-Q30 as i128, Q30 as i128) as i64;
            // Gardner e=mid*(previous-current). A late midpoint gives negative e;
            // advance the sampling cursor and increase its rate in that case.
            let proposed = self
                .frequency_q30
                .checked_sub(error * 16)
                .ok_or("RX timing integrator overflow")?;
            let limit = self.config.max_tracking_ppm as i64 * Q30;
            if proposed.abs() > limit {
                self.range_hits = self.range_hits.saturating_add(1);
            } else {
                self.range_hits = 0;
            }
            self.frequency_q30 = proposed.clamp(-limit, limit);
            let ui = self.config.grid.samples_per_ui as i128 * 1_000_000;
            self.phase = self
                .phase
                .checked_add((-error as i128 * ui / (Q30 as i128 * 64)) as i64)
                .ok_or("RX timing phase overflow")?;
            if self.range_hits >= 8 {
                self.lose_lock(LossReason::TimingRange, index)?;
            }
        }
        Ok(())
    }
    pub fn acquisition(&self) -> Acquisition {
        self.acquisition.clone()
    }
    pub fn selected_phase(&self) -> usize {
        self.selected_phase
    }
    pub fn clipped_samples(&self) -> u64 {
        self.clipped_samples
    }
    pub fn trained_symbols(&self) -> u32 {
        self.trained
    }
    pub fn coefficients(&self) -> (&[i64], &[i64]) {
        let c = &self.candidates[self.selected_phase];
        (&c.ffe, &c.echo)
    }
    pub fn next_index(&self) -> u64 {
        self.next_index
    }
    pub fn active_frame_token(&self) -> Option<&FrameToken> {
        self.active_frame.as_ref()
    }
    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        self.validate_control_acquisition()?;
        let n = self.config.grid.samples_per_ui as usize;
        if self.candidates.len() != n
            || self.selected_phase >= n
            || self.training.len() > MAX_BLOCK_SAMPLES
            || self.training.iter().any(|s| !(-1..=1).contains(s))
            || self.phase < -(n as i64 * 1_000_000 / 32)
            || self.phase >= n as i64 * 1_000_000
            || self.frequency_q30.unsigned_abs() > self.config.max_tracking_ppm as u64 * Q30 as u64
            || self.previous_values.as_ref().is_some_and(|values| {
                values.len() != n
                    || values
                        .iter()
                        .any(|sample| sample.unsigned_abs() > 1_000_000_000_000)
            })
            || self.previous_symbol.is_some_and(|s| !(-1..=1).contains(&s))
            || self.events.len() > 1024
            || self.decision_errors.len() > 64
            || self
                .decision_errors
                .iter()
                .any(|&square| square > 1_000_000_000_000_000_000_000_000)
            || self.active_frame.as_ref().is_some_and(|token| {
                token.epoch != self.epoch
                    || token.serial > self.frame_serial
                    || self.acquisition != Acquisition::Locked
            })
            || self.window.len() > n + 8
            || self.trained as u64 > self.ordinal
            || self.clipped_samples > self.next_index
            || self
                .frontend
                .iter()
                .any(|sample| sample.unsigned_abs() > 1_000_000_000)
            || self.window.iter().any(|sample| {
                sample.rx.unsigned_abs() > self.config.adc_full_scale_uv as u64
                    || sample.local.unsigned_abs() > 1_000_000_000
            })
            || self.frontend.len()
                != self
                    .config
                    .frontend_taps_q30
                    .len()
                    .min(self.next_index as usize)
            || self
                .window
                .windows(2)
                .any(|w| w[0].index.checked_add(1) != Some(w[1].index))
            || self
                .window
                .last()
                .is_some_and(|last| last.index.checked_add(1) != Some(self.next_index))
            || self.candidates.iter().any(|c| {
                c.ffe.len() != self.config.ffe_taps
                    || c.echo.len() != self.config.echo_taps
                    || c.rx.len() != self.config.ffe_taps.min(self.ordinal as usize)
                    || c.local.len() != self.config.echo_taps.min(self.ordinal as usize)
                    || c.errors.len() != 64usize.min(self.trained as usize)
                    || c.errors
                        .iter()
                        .any(|&error| error > 1_000_000_000_000_000_000_000_000)
                    || c.rx
                        .iter()
                        .any(|sample| sample.unsigned_abs() > self.config.adc_full_scale_uv as u64)
                    || c.local
                        .iter()
                        .any(|sample| sample.unsigned_abs() > 1_000_000_000)
                    || c.ffe
                        .iter()
                        .chain(&c.echo)
                        .any(|w| w.unsigned_abs() > 16 * Q30 as u64)
            })
            || self.acquisition == Acquisition::Locked
                && self.trained < self.config.minimum_training_symbols
        {
            return Err("invalid saved numerical RX histories/phase/training".into());
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
    pub fn restore(config: &ReceiverConfig, bytes: &[u8]) -> Result<Self> {
        let state: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if &state.config != config {
            return Err("saved RX immutable config differs".into());
        }
        state.validate()?;
        Ok(state)
    }
    /// Consumes one endpoint's actual received waveform and its known local TX
    /// waveform. Local TX is subtracted only by learned echo coefficients.
    pub fn process(
        &mut self,
        start_index: u64,
        received: &[i64],
        local_tx: &[i64],
    ) -> Result<Vec<SymbolDecision>> {
        if start_index != self.next_index
            || received.len() != local_tx.len()
            || received.len() > MAX_BLOCK_SAMPLES
            || received
                .iter()
                .chain(local_tx)
                .any(|v| v.unsigned_abs() > 1_000_000_000)
        {
            return Err("RX waveform index/dimensions/units mismatch".into());
        }
        let mut candidate = self.clone();
        let output = candidate.process_inner(received, local_tx)?;
        *self = candidate;
        Ok(output)
    }
    /// One exact receive sample under an enclosing `SampledMedium` clone.
    pub(crate) fn process_one_in_transaction(
        &mut self,
        index: u64,
        received: i64,
        local_tx: i64,
    ) -> Result<Vec<SymbolDecision>> {
        if index != self.next_index
            || received.unsigned_abs() > 1_000_000_000
            || local_tx.unsigned_abs() > 1_000_000_000
        {
            return Err("RX one-sample index/units mismatch".into());
        }
        self.process_inner(&[received], &[local_tx])
    }
    fn process_inner(&mut self, received: &[i64], local_tx: &[i64]) -> Result<Vec<SymbolDecision>> {
        let mut decisions = vec![];
        let ui = self.config.grid.samples_per_ui as i64 * 1_000_000;
        for (&voltage, &local) in received.iter().zip(local_tx) {
            self.frontend.push_front(voltage);
            self.frontend.truncate(self.config.frontend_taps_q30.len());
            let sum = self
                .frontend
                .iter()
                .zip(&self.config.frontend_taps_q30)
                .try_fold(0i128, |sum, (&x, &tap)| {
                    sum.checked_add(x as i128 * tap as i128)
                        .ok_or("RX front-end overflow")
                })?;
            let filtered = rounded_q30(sum)?;
            let limit = self.config.adc_full_scale_uv;
            let clipped = filtered.clamp(-limit, limit);
            self.clipped_samples = self
                .clipped_samples
                .checked_add(u64::from(clipped != filtered))
                .ok_or("RX clipping counter overflow")?;
            let levels = (1i64 << (self.config.adc_bits - 1)) - 1;
            let magnitude = (clipped as i128).abs() * levels as i128;
            let code = (magnitude + limit as i128 / 2) / limit as i128;
            let quantized =
                (code * limit as i128 / levels as i128) as i64 * if clipped < 0 { -1 } else { 1 };
            self.window.push(Observation {
                index: self.next_index,
                rx: quantized,
                local,
                clipped: clipped != filtered,
            });
            self.next_index = self
                .next_index
                .checked_add(1)
                .ok_or("RX sample cursor overflow")?;
            let step = 1_000_000 + self.config.clock_ppm as i64 + self.frequency_q30 / Q30;
            self.phase = self
                .phase
                .checked_add(step)
                .ok_or("RX clock accumulator overflow")?;
            if self.phase < ui {
                continue;
            }
            self.phase -= ui;
            let target = self.training.pop_front();
            let mut values = Vec::with_capacity(self.candidates.len());
            let candidate_count = self.candidates.len();
            for (phase, candidate) in self.candidates.iter_mut().enumerate() {
                let index =
                    (phase * self.window.len() / candidate_count).min(self.window.len() - 1);
                let sample = &self.window[index];
                let value = candidate.advance(sample.rx, sample.local)?;
                if let Some(symbol) = target {
                    candidate.train(symbol as i64 * self.config.peak_uv, value, &self.config)?;
                }
                values.push((sample.index, value));
            }
            if target.is_some() {
                self.trained = self
                    .trained
                    .checked_add(1)
                    .ok_or("RX training counter overflow")?;
                if self.trained <= 32 {
                    self.selected_phase = (0..self.candidates.len())
                        .min_by_key(|&phase| {
                            (
                                self.candidates[phase].score(),
                                phase.abs_diff(self.candidates.len() / 2),
                                phase,
                            )
                        })
                        .unwrap();
                }
                let limit = self.config.peak_uv as u128 * self.config.lock_error_ppm as u128;
                let allowed = limit * limit / 1_000_000_000_000;
                let was_locked = self.acquisition == Acquisition::Locked;
                let score = self.candidates[self.selected_phase].score();
                let lock_valid =
                    self.trained >= self.config.minimum_training_symbols && score <= allowed;
                let loss_limit = self.config.peak_uv as u128 * self.config.loss_error_ppm as u128;
                let loss_allowed = loss_limit * loss_limit / 1_000_000_000_000;
                if was_locked && score > loss_allowed {
                    self.lose_lock(LossReason::DecisionError, self.next_index)?;
                } else {
                    self.acquisition = if was_locked || lock_valid {
                        Acquisition::Locked
                    } else {
                        Acquisition::Training
                    };
                }
                if !was_locked && self.acquisition == Acquisition::Locked {
                    self.emit(ReceiverEvent::LockAcquired {
                        epoch: self.epoch,
                        sample_index: self.next_index,
                    })?;
                }
            }
            if self.control_acquisition.is_some() {
                self.advance_control_acquisition()?;
            }
            if let Some(state) = &mut self.control_acquisition {
                let phase = self.selected_phase;
                let sample = self.window[(phase * self.window.len() / self.candidates.len())
                    .min(self.window.len() - 1)]
                .clone();
                state.recent.push_back((self.ordinal, phase, sample));
                if state.recent.len() > 64 {
                    state.recent.pop_front();
                }
            }
            let (index, value) = if self.control_acquisition.is_some() {
                let phase = self.selected_phase;
                let sample = &self.window[(phase * self.window.len() / self.candidates.len())
                    .min(self.window.len() - 1)];
                (
                    sample.index,
                    self.candidates[phase]
                        .output_with(&self.candidates[phase].ffe, &self.candidates[phase].echo)?,
                )
            } else {
                values[self.selected_phase]
            };
            let symbol = if value > self.config.peak_uv / 2 {
                1
            } else if value < -self.config.peak_uv / 2 {
                -1
            } else {
                0
            };
            // Timing uses the SAME trained filter at midpoint and both centers.
            // Training each phase independently must not synthesize the midpoint.
            let selected = &self.candidates[self.selected_phase];
            let common_values: Vec<_> = self
                .candidates
                .iter()
                .zip(&values)
                .map(|(candidate, (index, _))| {
                    candidate
                        .output_with(&selected.ffe, &selected.echo)
                        .map(|value| (*index, value))
                })
                .collect::<Result<_>>()?;
            self.track_timing(&common_values, self.next_index)?;
            if self.acquisition == Acquisition::Locked {
                self.no_transition = if self.previous_symbol == Some(symbol) {
                    self.no_transition.saturating_add(1)
                } else {
                    0
                };
                let residual = value as i128 - symbol as i128 * self.config.peak_uv as i128;
                self.decision_errors
                    .push_back((residual * residual) as u128);
                if self.decision_errors.len() > 64 {
                    self.decision_errors.pop_front();
                }
                let limit = self.config.peak_uv as u128 * self.config.loss_error_ppm as u128;
                let allowed = limit * limit / 1_000_000_000_000;
                if self.no_transition >= self.config.max_no_transition_symbols {
                    self.lose_lock(LossReason::TransitionTimeout, self.next_index)?;
                } else if self.decision_errors.len() == 64
                    && self.decision_errors.iter().sum::<u128>() / 64 > allowed
                {
                    self.lose_lock(LossReason::DecisionError, self.next_index)?;
                }
            }
            self.previous_symbol = Some(symbol);
            self.previous_values = Some(common_values.iter().map(|v| v.1).collect());
            decisions.push(SymbolDecision {
                ordinal: self.ordinal,
                epoch: self.epoch,
                sample_index: index,
                symbol,
                voltage_uv: value,
                training: target.is_some(),
                acquisition: self.acquisition.clone(),
            });
            self.ordinal = self
                .ordinal
                .checked_add(1)
                .ok_or("RX symbol ordinal overflow")?;
            self.window.clear();
        }
        Ok(decisions)
    }
}
/// Wilson95% interval, including a nonzero upper bound for zero observed errors.
pub fn ber_confidence(errors: u64, trials: u64) -> Result<(f64, f64)> {
    if trials == 0 || errors > trials {
        return Err("BER requires nonzero actual trials and errors<=trials".into());
    }
    let n = trials as f64;
    let p = errors as f64 / n;
    let z = 1.959963984540054;
    let d = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / d;
    let radius = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / d;
    Ok(((center - radius).max(0.0), (center + radius).min(1.0)))
}

#[cfg(test)]
mod control_tests {
    use super::*;
    fn config(n: u32) -> ReceiverConfig {
        ReceiverConfig {
            grid: SampleGrid {
                symbol_rate_hz: 25_000_000,
                samples_per_ui: n,
            },
            peak_uv: 1_000_000,
            frontend_taps_q30: vec![Q30],
            adc_bits: 16,
            adc_full_scale_uv: 4_000_000,
            ffe_taps: 1,
            echo_taps: 0,
            adaptation_q30: 0,
            minimum_training_symbols: 32,
            lock_error_ppm: 100_000,
            clock_ppm: 0,
            max_tracking_ppm: 0,
            max_no_transition_symbols: 128,
            loss_error_ppm: 500_000,
        }
    }
    #[test]
    fn actual_adc_control_sync_at_twenty_and_numerical_lock_at_forty_chips() {
        for n in [16, 32] {
            for polarity in [-1, 1] {
                for sync_groups in [2, 3, 4, 12] {
                    let cfg = config(n);
                    let mut rx = Receiver::new(cfg.clone()).unwrap();
                    rx.begin_control_acquisition().unwrap();
                    let mut tx = t1s::DmeTransmitter::default();
                    tx.resume_after_interruption(-polarity).unwrap();
                    let mut chips = vec![];
                    for _ in 0..sync_groups {
                        chips.extend(tx.group(t1s::SYNC_COMMIT).unwrap());
                    }
                    for _ in 0..2 {
                        chips.extend(tx.group(t1s::SSD).unwrap());
                    }
                    let mut sync_at = None;
                    let mut lock_at = None;
                    for (i, &chip) in chips.iter().enumerate() {
                        for _ in 0..n {
                            rx.process_one_in_transaction(
                                rx.next_index(),
                                chip as i64 * 1_000_000,
                                0,
                            )
                            .unwrap();
                        }
                        let prefix = rx.take_control_prefix();
                        if !prefix.is_empty() {
                            assert_eq!(prefix.len(), 20);
                            sync_at = Some(i + 1);
                        }
                        if rx.acquisition() == Acquisition::Locked && lock_at.is_none() {
                            lock_at = Some(i + 1);
                        }
                        let saved = rx.checkpoint().unwrap();
                        let restored = Receiver::restore(&cfg, &saved).unwrap();
                        assert_eq!(restored.checkpoint().unwrap(), saved);
                    }
                    assert_eq!(sync_at, Some(20));
                    assert_eq!(lock_at, Some(40));
                }
            }
        }
    }
    #[test]
    fn received_reference_mutations_and_weak_clipped_controls_are_rejected() {
        let cfg = config(16);
        for peak in [200_000, 5_000_000] {
            let mut rx = Receiver::new(cfg.clone()).unwrap();
            rx.begin_control_acquisition().unwrap();
            let mut tx = t1s::DmeTransmitter::default();
            for _ in 0..4 {
                for chip in tx.group(t1s::SYNC_COMMIT).unwrap() {
                    for _ in 0..16 {
                        rx.process_one_in_transaction(rx.next_index(), chip as i64 * peak, 0)
                            .unwrap();
                    }
                }
            }
            assert!(rx.take_control_prefix().is_empty());
            assert_ne!(rx.acquisition(), Acquisition::Locked);
        }
        let mut rx = Receiver::new(cfg.clone()).unwrap();
        rx.begin_control_acquisition().unwrap();
        let mut tx = t1s::DmeTransmitter::default();
        for _ in 0..4 {
            for chip in tx.group(t1s::SYNC_COMMIT).unwrap() {
                for _ in 0..16 {
                    rx.process_one_in_transaction(rx.next_index(), chip as i64 * 1_000_000, 0)
                        .unwrap();
                }
            }
        }
        rx.take_control_prefix();
        let original: serde_json::Value =
            serde_json::from_slice(&rx.checkpoint().unwrap()).unwrap();
        for field in [
            "epoch",
            "phase",
            "first_ordinal",
            "initial_level",
            "final_level",
            "sample_indices",
            "chips",
        ] {
            let mut value = original.clone();
            let reference = &mut value["control_acquisition"]["reference"];
            match field {
                "initial_level" | "final_level" => {
                    reference[field] = serde_json::json!(-reference[field].as_i64().unwrap())
                }
                "sample_indices" => {
                    reference[field][0] =
                        serde_json::json!(reference[field][0].as_u64().unwrap() + 1)
                }
                "chips" => {
                    reference[field][0] = serde_json::json!(-reference[field][0].as_i64().unwrap())
                }
                _ => reference[field] = serde_json::json!(reference[field].as_u64().unwrap() + 1),
            }
            assert!(
                Receiver::restore(&cfg, &serde_json::to_vec(&value).unwrap()).is_err(),
                "accepted {field}"
            );
        }
    }
}
