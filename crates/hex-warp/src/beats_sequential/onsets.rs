//! Bounded streaming physical onset scanner matching `beats/analysis.rs` + `beats/onsets.rs`.
//!
//! Runs the energy tracker slightly ahead of the 4-band stereo spectral flux + phase-error
//! tracker so `insert` deduplication matches the resident two-pass detector exactly while
//! never buffering the full audio file or exceeding bounded per-step work.
use crate::{
    Error, GranularSource, MAX_MAP_SEGMENTS, buffer,
    fft::{self, Complex},
};

pub(super) const MAX_DETECTED_WINDOW: usize = MAX_MAP_SEGMENTS;
pub(super) const MAX_CACHED_LOOP_REGION: u64 = 262_144;
const ENERGY_CHUNK_FRAMES: u64 = 4_096;
const MAX_SPECTRAL_HOPS_PER_STEP: usize = 8;

fn fft_length(rate: u32) -> usize {
    ((rate as usize * 16 / 1000).next_power_of_two()).clamp(128, 4096)
}

#[derive(Clone)]
pub(super) struct OnsetScannerCheckpoint {
    pub(super) boundary_attack: Option<bool>,
    pub(super) energy_warmup_done: bool,
    pub(super) energy_slow: f64,
    pub(super) energy_last: Option<u64>,
    pub(super) energy_pos: u64,
    pub(super) spec_previous: Vec<[Complex; 2]>,
    pub(super) spec_older: Vec<[Complex; 2]>,
    pub(super) spec_mean: [f64; 4],
    pub(super) spec_deviation: [f64; 4],
    pub(super) spec_last_score: f64,
    pub(super) spec_before_score: f64,
    pub(super) spec_center: u64,
    pub(super) spec_count: u64,
    pub(super) onsets: Vec<u64>,
    pub(super) region_complete: bool,
}

impl OnsetScannerCheckpoint {
    pub(super) fn backing_bytes(&self) -> Option<usize> {
        let complex_pair = std::mem::size_of::<[Complex; 2]>();
        self.spec_previous
            .capacity()
            .checked_mul(complex_pair)?
            .checked_add(self.spec_older.capacity().checked_mul(complex_pair)?)?
            .checked_add(
                self.onsets
                    .capacity()
                    .checked_mul(std::mem::size_of::<u64>())?,
            )
    }
}

pub(super) struct OnsetScanner {
    sample_rate: u32,
    region_length: u64,
    looping: bool,
    separation: u64,
    n: usize,
    half: usize,
    bins: usize,
    hop: usize,
    window: Vec<f64>,
    fft_buf: Vec<Complex>,
    boundary_attack: Option<bool>,
    energy_warmup_done: bool,
    energy_slow: f64,
    energy_last: Option<u64>,
    energy_pos: u64,
    spec_previous: Vec<[Complex; 2]>,
    spec_older: Vec<[Complex; 2]>,
    spec_mean: [f64; 4],
    spec_deviation: [f64; 4],
    spec_last_score: f64,
    spec_before_score: f64,
    spec_center: u64,
    spec_count: u64,
    onsets: Vec<u64>,
    region_complete: bool,
}

impl OnsetScanner {
    pub(super) fn new(sample_rate: u32, region_length: u64, looping: bool) -> Result<Self, Error> {
        let n = fft_length(sample_rate);
        let half = n / 2;
        let bins = half + 1;
        let hop = n / 4;
        let separation = u64::from((sample_rate / 1000).max(1));
        let mut window = buffer(n, 0.0_f64)?;
        for (i, value) in window.iter_mut().enumerate() {
            *value = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
        }
        let mut onsets = Vec::new();
        onsets
            .try_reserve_exact(64)
            .map_err(|_| Error::Allocation)?;
        onsets.push(0);
        Ok(Self {
            sample_rate,
            region_length,
            looping,
            separation,
            n,
            half,
            bins,
            hop,
            window,
            fft_buf: buffer(n, Complex::default())?,
            boundary_attack: None,
            energy_warmup_done: false,
            energy_slow: 0.0,
            energy_last: None,
            energy_pos: 0,
            spec_previous: buffer(bins, [Complex::default(); 2])?,
            spec_older: buffer(bins, [Complex::default(); 2])?,
            spec_mean: [0.0; 4],
            spec_deviation: [0.0; 4],
            spec_last_score: 0.0,
            spec_before_score: 0.0,
            spec_center: 0,
            spec_count: 0,
            onsets,
            region_complete: false,
        })
    }

    pub(super) fn backing_bytes(&self) -> Option<usize> {
        let complex_pair = std::mem::size_of::<[Complex; 2]>();
        self.window
            .capacity()
            .checked_mul(std::mem::size_of::<f64>())?
            .checked_add(
                self.fft_buf
                    .capacity()
                    .checked_mul(std::mem::size_of::<Complex>())?,
            )?
            .checked_add(self.spec_previous.capacity().checked_mul(complex_pair)?)?
            .checked_add(self.spec_older.capacity().checked_mul(complex_pair)?)?
            .checked_add(
                self.onsets
                    .capacity()
                    .checked_mul(std::mem::size_of::<u64>())?,
            )
    }

    pub(super) fn checkpoint(&self, budget: usize) -> Result<OnsetScannerCheckpoint, Error> {
        let complex_pair = std::mem::size_of::<[Complex; 2]>();
        let needed = self
            .bins
            .checked_mul(complex_pair)
            .and_then(|v| v.checked_mul(2))
            .and_then(|v| {
                self.onsets
                    .len()
                    .checked_mul(std::mem::size_of::<u64>())
                    .and_then(|o| v.checked_add(o))
            })
            .ok_or(Error::ScratchLimit)?;
        if needed > budget {
            return Err(Error::ScratchLimit);
        }
        let mut spec_previous = buffer(self.bins, [Complex::default(); 2])?;
        let mut spec_older = buffer(self.bins, [Complex::default(); 2])?;
        spec_previous.copy_from_slice(&self.spec_previous);
        spec_older.copy_from_slice(&self.spec_older);
        let mut onsets = Vec::new();
        onsets
            .try_reserve_exact(self.onsets.len())
            .map_err(|_| Error::Allocation)?;
        onsets.extend_from_slice(&self.onsets);
        let cp = OnsetScannerCheckpoint {
            boundary_attack: self.boundary_attack,
            energy_warmup_done: self.energy_warmup_done,
            energy_slow: self.energy_slow,
            energy_last: self.energy_last,
            energy_pos: self.energy_pos,
            spec_previous,
            spec_older,
            spec_mean: self.spec_mean,
            spec_deviation: self.spec_deviation,
            spec_last_score: self.spec_last_score,
            spec_before_score: self.spec_before_score,
            spec_center: self.spec_center,
            spec_count: self.spec_count,
            onsets,
            region_complete: self.region_complete,
        };
        if cp.backing_bytes().is_none_or(|b| b > budget) {
            return Err(Error::ScratchLimit);
        }
        Ok(cp)
    }

    pub(super) fn refresh_checkpoint(&self, cp: &mut OnsetScannerCheckpoint) -> Result<(), Error> {
        if cp.spec_previous.len() != self.bins || cp.spec_older.len() != self.bins {
            return Err(Error::Invalid("incompatible beats onset checkpoint"));
        }
        cp.boundary_attack = self.boundary_attack;
        cp.energy_warmup_done = self.energy_warmup_done;
        cp.energy_slow = self.energy_slow;
        cp.energy_last = self.energy_last;
        cp.energy_pos = self.energy_pos;
        cp.spec_previous.copy_from_slice(&self.spec_previous);
        cp.spec_older.copy_from_slice(&self.spec_older);
        cp.spec_mean = self.spec_mean;
        cp.spec_deviation = self.spec_deviation;
        cp.spec_last_score = self.spec_last_score;
        cp.spec_before_score = self.spec_before_score;
        cp.spec_center = self.spec_center;
        cp.spec_count = self.spec_count;
        cp.onsets.clear();
        if cp.onsets.capacity() < self.onsets.len() {
            cp.onsets
                .try_reserve(self.onsets.len() - cp.onsets.capacity())
                .map_err(|_| Error::Allocation)?;
        }
        cp.onsets.extend_from_slice(&self.onsets);
        cp.region_complete = self.region_complete;
        Ok(())
    }

    pub(super) fn restore(&mut self, cp: &OnsetScannerCheckpoint) -> Result<(), Error> {
        if cp.spec_previous.len() != self.bins || cp.spec_older.len() != self.bins {
            return Err(Error::Invalid("incompatible beats onset checkpoint"));
        }
        self.boundary_attack = cp.boundary_attack;
        self.energy_warmup_done = cp.energy_warmup_done;
        self.energy_slow = cp.energy_slow;
        self.energy_last = cp.energy_last;
        self.energy_pos = cp.energy_pos;
        self.spec_previous.copy_from_slice(&cp.spec_previous);
        self.spec_older.copy_from_slice(&cp.spec_older);
        self.spec_mean = cp.spec_mean;
        self.spec_deviation = cp.spec_deviation;
        self.spec_last_score = cp.spec_last_score;
        self.spec_before_score = cp.spec_before_score;
        self.spec_center = cp.spec_center;
        self.spec_count = cp.spec_count;
        self.onsets.clear();
        if self.onsets.capacity() < cp.onsets.len() {
            self.onsets
                .try_reserve(cp.onsets.len() - self.onsets.capacity())
                .map_err(|_| Error::Allocation)?;
        }
        self.onsets.extend_from_slice(&cp.onsets);
        self.region_complete = cp.region_complete;
        Ok(())
    }

    pub(super) fn boundary_attack(&self) -> bool {
        self.boundary_attack.unwrap_or(!self.looping)
    }

    pub(super) fn onsets(&self) -> &[u64] {
        &self.onsets
    }

    /// Prune onsets strictly below `floor` when scanning a non-cached long region.
    pub(super) fn prune_before(&mut self, floor: u64) {
        if self.looping && self.region_length <= MAX_CACHED_LOOP_REGION {
            return;
        }
        let keep_from = floor.saturating_sub(self.separation + self.n as u64);
        let idx = self.onsets.partition_point(|&x| x < keep_from);
        if idx > 0 {
            self.onsets.drain(..idx);
        }
    }

    /// Highest region-relative frame through which all onsets are finalized.
    pub(super) fn finalized_through(&self) -> u64 {
        if self.region_complete {
            return self.region_length;
        }
        let spec_safe = self
            .spec_center
            .saturating_sub(self.hop as u64 + self.half as u64);
        spec_safe.min(self.energy_pos.saturating_sub(self.separation))
    }

    /// Perform one bounded scan step toward finalizing onsets through `target_local`.
    /// Returns `Ok(true)` once all onsets up to `target_local.min(region_length)` are final.
    pub(super) fn step_toward<S: GranularSource>(
        &mut self,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        target_local: u64,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<bool, S::Error> {
        let clamped_target = target_local.min(self.region_length);
        if self.boundary_attack.is_none() {
            if cancel() {
                return Err(Error::Cancelled.into());
            }
            self.boundary_attack = Some(self.compute_cycle_boundary(source)?);
        }
        if self.region_complete || self.finalized_through() >= clamped_target {
            return Ok(true);
        }
        if !self.energy_warmup_done {
            let warmup = self
                .region_length
                .min(u64::from((self.sample_rate / 200).max(1)))
                .max(1);
            let mut slow = 0.0;
            for i in 0..warmup {
                if i % 1024 == 0 && cancel() {
                    return Err(Error::Cancelled.into());
                }
                let x = source.frame(i128::from(i))?;
                slow += (x[0] * x[0] + x[1] * x[1]) / warmup as f64;
            }
            self.energy_slow = slow;
            self.energy_warmup_done = true;
        }
        // Keep energy ahead of spectral by half + hop + separation so spectral
        // insert sees every energy onset within +-separation.
        let required_energy = clamped_target
            .saturating_add(self.half as u64 + self.hop as u64 + self.separation + 1)
            .min(self.region_length);
        if self.energy_pos < required_energy {
            let chunk_end = (self.energy_pos + ENERGY_CHUNK_FRAMES).min(required_energy);
            let coefficient = 1.0 - (-1.0 / (f64::from(self.sample_rate) * 0.020)).exp();
            let mut i = self.energy_pos;
            while i < chunk_end {
                if (i - self.energy_pos).is_multiple_of(1024) && cancel() {
                    return Err(Error::Cancelled.into());
                }
                let x = source.frame(i128::from(i))?;
                let energy = x[0] * x[0] + x[1] * x[1];
                if self
                    .energy_last
                    .is_none_or(|last| i >= last + self.separation)
                    && energy > 1e-12
                    && energy > self.energy_slow * 8.0
                {
                    if i != 0 {
                        self.insert_raw(i).map_err(S::Error::from)?;
                    }
                    self.energy_last = Some(i);
                }
                self.energy_slow += (energy - self.energy_slow) * coefficient;
                i += 1;
            }
            self.energy_pos = chunk_end;
            if self.energy_pos < required_energy {
                return Ok(false);
            }
        }

        // Advance spectral detector up to MAX_SPECTRAL_HOPS_PER_STEP hops.
        let mut hops_done = 0;
        let required_spec_center = clamped_target
            .saturating_add(self.hop as u64 + self.half as u64 + 1)
            .min(self.region_length);
        while self.spec_center < required_spec_center && hops_done < MAX_SPECTRAL_HOPS_PER_STEP {
            if cancel() {
                return Err(Error::Cancelled.into());
            }
            self.step_spectral_hop(source, scratch, cancel)?;
            hops_done += 1;
        }
        if self.spec_center >= self.region_length && self.energy_pos >= self.region_length {
            self.region_complete = true;
        }
        Ok(self.region_complete || self.finalized_through() >= clamped_target)
    }

    fn compute_cycle_boundary<S: GranularSource>(&self, source: &mut S) -> Result<bool, S::Error> {
        if !self.looping {
            return Ok(true);
        }
        let count = i128::from((self.sample_rate / 500).max(1));
        let mut difference = |at: i128| -> Result<f64, S::Error> {
            let x = source.frame(at)?;
            let y = source.frame(at - 1)?;
            Ok((0..2).map(|c| (x[c] - y[c]).powi(2)).sum::<f64>())
        };
        let seam = difference(0)?;
        let mut neighborhood = 0.0;
        for at in -count..=count {
            if at != 0 {
                neighborhood += difference(at)?;
            }
        }
        Ok(seam > 1e-12 && seam > neighborhood / (count * 2) as f64 * 8.0)
    }

    fn insert_raw(&mut self, at: u64) -> Result<(), Error> {
        let index = self.onsets.partition_point(|&x| x < at);
        if index < self.onsets.len() && self.onsets[index] == at {
            return Ok(());
        }
        if self.onsets.len() == MAX_DETECTED_WINDOW {
            if self.looping && self.region_length <= MAX_CACHED_LOOP_REGION {
                return Err(Error::Invalid("transients exceed bounded source plan"));
            }
            // Sliding long-file window: drop oldest entry outside active neighborhood.
            self.onsets.remove(0);
            let new_idx = self.onsets.partition_point(|&x| x < at);
            self.onsets.insert(new_idx, at);
            return Ok(());
        }
        self.onsets.try_reserve(1).map_err(|_| Error::Allocation)?;
        self.onsets.insert(index, at);
        Ok(())
    }

    fn insert_spectral(&mut self, at: u64) -> Result<(), Error> {
        let index = self.onsets.partition_point(|&x| x < at);
        if index > 0 && at - self.onsets[index - 1] < self.separation
            || index < self.onsets.len() && self.onsets[index] - at < self.separation
        {
            return Ok(());
        }
        self.insert_raw(at)
    }

    fn step_spectral_hop<S: GranularSource>(
        &mut self,
        source: &mut S,
        scratch: &mut [[f64; 2]],
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<(), S::Error> {
        let center = self.spec_center;
        let use_scratch = scratch.len() >= self.n;
        if use_scratch {
            let slice = &mut scratch[..self.n];
            for (i, slot) in slice.iter_mut().enumerate() {
                let at = i128::from(center) + i as i128 - self.half as i128;
                *slot = if at >= 0 && at < i128::from(self.region_length) {
                    source.frame(at)?
                } else {
                    [0.0; 2]
                };
            }
        }
        let mut increase = [0.0; 4];
        let mut magnitude = [0.0; 4];
        let mut old_magnitude = [0.0; 4];
        let mut phase_error = [0.0; 4];
        #[allow(clippy::needless_range_loop)]
        for channel in 0..2 {
            for (i, value) in self.fft_buf.iter_mut().enumerate() {
                let x = if use_scratch {
                    scratch[i][channel]
                } else {
                    let at = i128::from(center) + i as i128 - self.half as i128;
                    if at >= 0 && at < i128::from(self.region_length) {
                        source.frame(at)?[channel]
                    } else {
                        0.0
                    }
                };
                *value = Complex::new(x * self.window[i], 0.0);
            }
            fft::transform(&mut self.fft_buf, false).map_err(S::Error::from)?;
            for bin in 0..self.bins {
                let hz = bin as f64 * f64::from(self.sample_rate) / self.n as f64;
                let band = if hz < 200.0 {
                    0
                } else if hz < 2000.0 {
                    1
                } else if hz < 6000.0 {
                    2
                } else {
                    3
                };
                let now = self.fft_buf[bin];
                let old = self.spec_previous[bin][channel];
                let oldest = self.spec_older[bin][channel];
                let m = now.magnitude();
                let p = old.magnitude();
                increase[band] += (m - p).max(0.0);
                magnitude[band] += m;
                old_magnitude[band] += p;
                let denominator = p * oldest.magnitude();
                if denominator > 1e-20 {
                    let predicted = old.mul(old).mul(Complex::new(oldest.re, -oldest.im));
                    phase_error[band] += (now.re - predicted.re / denominator)
                        .hypot(now.im - predicted.im / denominator);
                }
                self.spec_older[bin][channel] = old;
                self.spec_previous[bin][channel] = now;
            }
        }
        let mut score = 0.0_f64;
        for band in 0..4 {
            let total = magnitude[band] + old_magnitude[band];
            let flux = if total > 1e-7 * self.n as f64 {
                increase[band] / total * (0.75 + 0.25 * (phase_error[band] / total).min(1.0))
            } else {
                0.0
            };
            let threshold =
                (0.15 + self.spec_mean[band] * 1.5 + self.spec_deviation[band] * 3.0).min(0.70);
            score = score.max(flux - threshold);
            let delta = flux - self.spec_mean[band];
            self.spec_mean[band] += 0.05 * delta;
            self.spec_deviation[band] += 0.05 * (delta.abs() - self.spec_deviation[band]);
        }
        if self.spec_count >= 3
            && self.spec_last_score > 0.0
            && self.spec_last_score >= self.spec_before_score
            && self.spec_last_score > score
            && let Some(at) = self.refine(source, center - self.hop as u64, cancel)?
        {
            self.insert_spectral(at).map_err(S::Error::from)?;
        }
        self.spec_before_score = self.spec_last_score;
        self.spec_last_score = score;
        self.spec_count += 1;
        self.spec_center = self
            .spec_center
            .checked_add(self.hop as u64)
            .ok_or_else(|| S::Error::from(Error::WorkLimit))?;
        Ok(())
    }

    fn refine<S: GranularSource>(
        &self,
        source: &mut S,
        center: u64,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Option<u64>, S::Error> {
        let half = self.half as u64;
        let start = center.saturating_sub(half);
        let end = center.saturating_add(half).min(self.region_length);
        let block = u64::from((self.sample_rate / 1000).max(1));
        let mut maximum = 0.0_f64;
        let mut sum = 0.0;
        for at in start..end {
            if (at - start).is_multiple_of(block) && cancel() {
                return Err(Error::Cancelled.into());
            }
            let now = source.frame(i128::from(at))?;
            let previous = source.frame(i128::from(at) - 1)?;
            let x: f64 = (0..2).map(|c| (now[c] - previous[c]).powi(2)).sum();
            maximum = maximum.max(x);
            sum += x;
        }
        let floor = sum / (end - start).max(1) as f64 * 4.0;
        if maximum <= floor.max(1e-14) {
            return Ok(None);
        }
        let threshold = (maximum * 0.10).max(floor).max(1e-14);
        for at in start..end {
            if (at - start).is_multiple_of(block) && cancel() {
                return Err(Error::Cancelled.into());
            }
            let now = source.frame(i128::from(at))?;
            let previous = source.frame(i128::from(at) - 1)?;
            let x: f64 = (0..2).map(|c| (now[c] - previous[c]).powi(2)).sum();
            if x >= threshold {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
}
