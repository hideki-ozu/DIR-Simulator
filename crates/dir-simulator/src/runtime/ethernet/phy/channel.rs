//! Causal fixed-point N-endpoint convolution, propagation, local echo and noise.
use super::*;
use std::collections::VecDeque;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Path {
    pub delay_samples: usize,
    pub taps_q30: Vec<i64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelConfig {
    pub grid: SampleGrid,
    pub endpoints: Vec<String>,
    /// paths[receiver][transmitter]; None means no physical transfer path.
    /// Diagonal paths explicitly model local transmitter echo.
    pub paths: Vec<Vec<Option<Path>>>,
    pub noise_rms_uv: Vec<u32>,
    pub noise_seed: u64,
    pub clip_uv: i64,
}
impl ChannelConfig {
    pub fn validate(&self) -> Result<()> {
        self.grid.validate()?;
        let n = self.endpoints.len();
        let ids: std::collections::BTreeSet<_> = self.endpoints.iter().collect();
        if n == 0
            || n > MAX_ENDPOINTS
            || ids.len() != n
            || self.endpoints.iter().any(|id| id.is_empty())
            || self.paths.len() != n
            || self.paths.iter().any(|row| row.len() != n)
            || self.noise_rms_uv.len() != n
            || self.clip_uv <= 0
            || self.clip_uv > 1_000_000_000
            || self.noise_rms_uv.iter().any(|&rms| rms > 100_000_000)
        {
            return Err("invalid numerical PHY channel matrix/units/bounds".into());
        }
        for row in &self.paths {
            let mut total = 0u64;
            for path in row.iter().flatten() {
                validate_taps(&path.taps_q30)?;
                if path.delay_samples > 262_144 {
                    return Err("channel propagation history exceeds bound".into());
                }
                for tap in &path.taps_q30 {
                    total = total
                        .checked_add(tap.unsigned_abs())
                        .ok_or("channel gain overflow")?;
                }
            }
            if total > 16 * Q30 as u64 {
                return Err("channel receiver absolute matrix gain exceeds16".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Noise {
    rng: u64,
    spare_bits: Option<u64>,
}
impl Noise {
    fn draw(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9e3779b97f4a7c15);
        let mut x = self.rng;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^ (x >> 31)
    }
    fn gaussian(&mut self) -> f64 {
        if let Some(bits) = self.spare_bits.take() {
            return f64::from_bits(bits);
        }
        // SplitMix64 high53bits mapped into open(0,1); Box–Muller v1.
        // libm evaluation is part of the same-target numerical contract; no
        // cross-platform bitwise identity claim is made for transcendental math.
        let a = ((self.draw() >> 11) as f64 + 1.0) / 9_007_199_254_740_994.0;
        let b = ((self.draw() >> 11) as f64 + 1.0) / 9_007_199_254_740_994.0;
        let radius = (-2.0 * a.ln()).sqrt();
        let angle = std::f64::consts::TAU * b;
        self.spare_bits = Some((radius * angle.sin()).to_bits());
        radius * angle.cos()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixChannel {
    config: ChannelConfig,
    next_index: u64,
    history: Vec<VecDeque<i64>>,
    noise: Vec<Noise>,
    clipped_samples: Vec<u64>,
}
impl MatrixChannel {
    pub fn new(config: ChannelConfig) -> Result<Self> {
        config.validate()?;
        let n = config.endpoints.len();
        let noise = config
            .endpoints
            .iter()
            .map(|id| {
                // Identity-keyed FNV1a seed mixing makes noise independent of endpoint order.
                let hash = id.bytes().fold(0xcbf29ce484222325u64, |hash, b| {
                    (hash ^ (b as u64)).wrapping_mul(0x100000001b3)
                });
                Noise {
                    rng: config.noise_seed ^ hash,
                    spare_bits: None,
                }
            })
            .collect();
        Ok(Self {
            config,
            next_index: 0,
            history: vec![VecDeque::new(); n],
            noise,
            clipped_samples: vec![0; n],
        })
    }
    pub fn config(&self) -> &ChannelConfig {
        &self.config
    }
    pub fn next_index(&self) -> u64 {
        self.next_index
    }
    pub fn clipped_samples(&self) -> &[u64] {
        &self.clipped_samples
    }
    fn history_bound(&self, tx: usize) -> usize {
        self.config
            .paths
            .iter()
            .filter_map(|row| row[tx].as_ref())
            .map(|path| path.delay_samples + path.taps_q30.len())
            .max()
            .unwrap_or(1)
    }
    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        let n = self.config.endpoints.len();
        if self.history.len() != n
            || self.noise.len() != n
            || self.clipped_samples.len() != n
            || self
                .clipped_samples
                .iter()
                .any(|&count| count > self.next_index)
            || self.history.iter().enumerate().any(|(tx, h)| {
                h.len() != self.history_bound(tx).min(self.next_index as usize)
                    || h.iter().any(|sample| sample.unsigned_abs() > 1_000_000_000)
            })
            || self.noise.iter().any(|noise| {
                noise.spare_bits.is_some_and(|bits| {
                    !f64::from_bits(bits).is_finite() || f64::from_bits(bits).abs() > 16.0
                })
            })
        {
            return Err("invalid saved channel histories/noise state".into());
        }
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
    pub fn restore(config: &ChannelConfig, bytes: &[u8]) -> Result<Self> {
        let state: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if &state.config != config {
            return Err("saved channel immutable config differs".into());
        }
        state.validate()?;
        Ok(state)
    }
    pub fn process(&mut self, input: &SampleBlock) -> Result<SampleBlock> {
        input.validate()?;
        if input.grid != self.config.grid
            || input.start_index != self.next_index
            || input.voltages.len() != self.config.endpoints.len()
        {
            return Err("channel sample grid/index/endpoint mismatch".into());
        }
        // All fallible arithmetic works on a bounded transaction; no partial block commits.
        let mut candidate = self.clone();
        let result = candidate.process_inner(input)?;
        *self = candidate;
        Ok(result)
    }
    /// Single-sample channel step under an enclosing `SampledMedium`
    /// transaction.  The outer clone is the rollback boundary.
    pub(crate) fn process_one_in_transaction(&mut self, drives: &[i64]) -> Result<Vec<i64>> {
        if drives.len() != self.config.endpoints.len()
            || drives.iter().any(|v| v.unsigned_abs() > 1_000_000_000)
        {
            return Err("channel one-sample drives mismatch".into());
        }
        let input = SampleBlock {
            grid: self.config.grid,
            start_index: self.next_index,
            voltages: drives.iter().map(|&v| vec![v]).collect(),
        };
        let output = self.process_inner(&input)?;
        Ok(output.voltages.into_iter().map(|row| row[0]).collect())
    }
    fn process_inner(&mut self, input: &SampleBlock) -> Result<SampleBlock> {
        let n = self.config.endpoints.len();
        let bounds: Vec<_> = (0..n).map(|tx| self.history_bound(tx)).collect();
        let mut output = vec![Vec::with_capacity(input.len()); n];
        for index in 0..input.len() {
            for (tx, history) in self.history.iter_mut().enumerate() {
                history.push_front(input.voltages[tx][index]);
                history.truncate(bounds[tx]);
            }
            for (rx, received) in output.iter_mut().enumerate() {
                let mut sum = 0i128;
                for (tx, path) in self.config.paths[rx].iter().enumerate() {
                    if let Some(path) = path {
                        for (tap, &coefficient) in path.taps_q30.iter().enumerate() {
                            let sample = self.history[tx]
                                .get(path.delay_samples + tap)
                                .copied()
                                .unwrap_or(0);
                            sum = sum
                                .checked_add(
                                    (sample as i128)
                                        .checked_mul(coefficient as i128)
                                        .ok_or("channel product overflow")?,
                                )
                                .ok_or("channel sum overflow")?;
                        }
                    }
                }
                // Sum every path before rounding/clipping; endpoint ordering cannot alter it.
                let signal = rounded_q30(sum)?;
                let noise = if self.config.noise_rms_uv[rx] == 0 {
                    0
                } else {
                    (self.noise[rx].gaussian() * self.config.noise_rms_uv[rx] as f64).round() as i64
                };
                let value = signal
                    .checked_add(noise)
                    .ok_or("channel noisy voltage overflow")?;
                let clipped = value.clamp(-self.config.clip_uv, self.config.clip_uv);
                self.clipped_samples[rx] = self.clipped_samples[rx]
                    .checked_add(u64::from(value != clipped))
                    .ok_or("clipping count overflow")?;
                received.push(clipped);
            }
        }
        self.next_index = self
            .next_index
            .checked_add(input.len() as u64)
            .ok_or("channel sample index overflow")?;
        Ok(SampleBlock {
            grid: input.grid,
            start_index: input.start_index,
            voltages: output,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransmitterConfig {
    pub grid: SampleGrid,
    pub peak_uv: i64,
    /// Linear rise/fall duration in millionths of one UI;0 is rectangular.
    pub transition_ui_ppm: u32,
    pub clock_ppm: i32,
    pub filter_taps_q30: Vec<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transmitter {
    config: TransmitterConfig,
    pending: VecDeque<i8>,
    active: Option<i8>,
    phase: u64,
    next_index: u64,
    previous: i64,
    transition_from: i64,
    transition_elapsed: u64,
    history: VecDeque<i64>,
}
impl Transmitter {
    pub fn new(config: TransmitterConfig) -> Result<Self> {
        config.grid.validate()?;
        validate_taps(&config.filter_taps_q30)?;
        if config.peak_uv <= 0
            || config.peak_uv > 10_000_000
            || config.transition_ui_ppm > 1_000_000
            || config.clock_ppm.unsigned_abs() > 10_000
        {
            return Err("invalid transmitter voltage/pulse/clock range".into());
        }
        Ok(Self {
            config,
            pending: VecDeque::new(),
            active: None,
            phase: 0,
            next_index: 0,
            previous: 0,
            transition_from: 0,
            transition_elapsed: 0,
            history: VecDeque::new(),
        })
    }
    pub fn push_symbols(&mut self, symbols: &[i8]) -> Result<()> {
        if self.pending.len() + symbols.len() > MAX_BLOCK_SAMPLES
            || symbols.iter().any(|s| !(-1..=1).contains(s))
        {
            return Err("PAM3 source symbols/pending buffer out of range".into());
        }
        self.pending.extend(symbols);
        Ok(())
    }
    pub fn pending_symbols(&self) -> usize {
        self.pending.len()
    }
    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.pending.is_empty()
    }
    /// True immediately after the final sample of the active symbol. The
    /// following symbol may already be queued but has not driven the MDI.
    pub fn at_symbol_boundary(&self) -> bool {
        self.active.is_none()
    }
    /// Collision/admin control replaces only future chips; the active pulse and
    /// oscillator/filter histories remain causal through their current boundary.
    pub fn replace_pending_symbols(&mut self, symbols: &[i8]) -> Result<()> {
        if symbols.len() > MAX_BLOCK_SAMPLES || symbols.iter().any(|s| !(-1..=1).contains(s)) {
            return Err("invalid replacement TX chip queue".into());
        }
        self.pending.clear();
        self.pending.extend(symbols);
        Ok(())
    }
    pub fn next_index(&self) -> u64 {
        self.next_index
    }
    pub fn checkpoint(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }
    pub fn restore(config: &TransmitterConfig, bytes: &[u8]) -> Result<Self> {
        let state: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if &state.config != config {
            return Err("saved TX config differs".into());
        }
        state.validate()?;
        Ok(state)
    }
    fn validate(&self) -> Result<()> {
        Self::new(self.config.clone())?;
        if self.pending.len() > MAX_BLOCK_SAMPLES
            || self.pending.iter().any(|s| !(-1..=1).contains(s))
            || self.active.is_some_and(|s| !(-1..=1).contains(&s))
            || self.phase >= self.config.grid.samples_per_ui as u64 * 1_000_000
            || self.phase as u128
                != self.next_index as u128 * (1_000_000i64 + self.config.clock_ppm as i64) as u128
                    % (self.config.grid.samples_per_ui as u128 * 1_000_000)
            || self.history.len()
                != self
                    .config
                    .filter_taps_q30
                    .len()
                    .min(self.next_index as usize)
            || self
                .history
                .iter()
                .any(|sample| sample.unsigned_abs() > self.config.peak_uv as u64)
            || self.previous.unsigned_abs() > self.config.peak_uv as u64
            || self.transition_from.unsigned_abs() > self.config.peak_uv as u64
        {
            return Err("invalid saved TX state".into());
        }
        Ok(())
    }
    pub fn render(&mut self, maximum: usize) -> Result<Vec<i64>> {
        if maximum > MAX_BLOCK_SAMPLES {
            return Err("TX sample batch exceeds bound".into());
        }
        let mut candidate = self.clone();
        let output = candidate.render_inner(maximum)?;
        *self = candidate;
        Ok(output)
    }
    /// Single-sample TX step under an enclosing `SampledMedium` transaction.
    pub(crate) fn render_one_in_transaction(&mut self) -> Result<i64> {
        self.render_inner(1)?
            .into_iter()
            .next()
            .ok_or("TX has no queued active symbol".into())
    }
    fn render_inner(&mut self, maximum: usize) -> Result<Vec<i64>> {
        let mut output = Vec::with_capacity(maximum);
        let step = (1_000_000i64 + self.config.clock_ppm as i64) as u64;
        let ui = self.config.grid.samples_per_ui as u64 * 1_000_000;
        for _ in 0..maximum {
            if self.active.is_none() {
                let Some(symbol) = self.pending.pop_front() else {
                    break;
                };
                self.active = Some(symbol);
                self.transition_from = self.previous;
                self.transition_elapsed = 0;
            }
            let target = self.active.unwrap() as i64 * self.config.peak_uv;
            let duration =
                self.config.transition_ui_ppm as u64 * self.config.grid.samples_per_ui as u64;
            let voltage = if duration == 0 || self.transition_elapsed >= duration {
                target
            } else {
                self.transition_from
                    + ((target - self.transition_from) as i128 * self.transition_elapsed as i128
                        / duration as i128) as i64
            };
            self.previous = voltage;
            self.history.push_front(voltage);
            self.history.truncate(self.config.filter_taps_q30.len());
            let sum = self
                .history
                .iter()
                .zip(&self.config.filter_taps_q30)
                .try_fold(0i128, |sum, (&sample, &tap)| {
                    sum.checked_add(sample as i128 * tap as i128)
                        .ok_or("TX FIR overflow")
                })?;
            output.push(rounded_q30(sum)?);
            self.transition_elapsed = self.transition_elapsed.saturating_add(step);
            self.phase += step;
            if self.phase >= ui {
                self.phase -= ui;
                self.active = None;
            }
            self.next_index = self
                .next_index
                .checked_add(1)
                .ok_or("TX sample cursor overflow")?;
        }
        Ok(output)
    }
}
