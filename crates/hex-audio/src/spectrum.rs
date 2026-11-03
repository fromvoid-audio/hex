//! Bounded, offline stereo spectrum analysis primitives shared by every
//! spectrum view (EQ backgrounds and the project spectrum).
//!
//! The analysis is multi-resolution. A full-rate [`FFT_SIZE`] stage resolves
//! the mids and highs with a short window; a [`BASS_FFT_SIZE`] stage, fed by a
//! rate-dependent cascade of 2:1 decimators ([`SpectrumWindow`]), resolves the
//! bass with a longer window. Display points blend the stages across a
//! crossover chosen from the sample rate.
//!
//! Each display point reports a sine-calibrated level in dBFS:
//! - a fractional-octave power average of the Hann power spectrum, integrated
//!   exactly over a piecewise-linear interpolation between bins, so sparse bass
//!   bins produce a continuous curve rather than stair steps;
//! - or, where a tonal peak stands well above its local noise reference
//!   (a cell-averaging detector with guard bins), that peak's level after
//!   log-parabolic interpolation, which removes Hann scalloping loss.
//!
//! A configurable slope tilts the result around 1 kHz. Time-domain display
//! behavior (attack, release, peak hold) is owned by [`SpectrumBallistics`].
//!
//! This module does not own rolling-window cadence, gaps, freshness or
//! threads. Construction allocates fixed workspaces on the control side;
//! [`SpectrumKernel::analyze`] and [`SpectrumWindow::push`] never allocate.

use std::{error::Error, fmt};

mod ballistics;
mod window;
pub use ballistics::{BallisticsSettings, SpectrumBallistics};
pub use window::SpectrumWindow;

/// Full-rate stage transform length, in frames.
pub const FFT_SIZE: usize = 4096;
/// Decimated bass stage transform length, in decimated frames.
pub const BASS_FFT_SIZE: usize = 2048;
/// Logarithmically spaced display points per frame.
pub const SPECTRUM_BANDS: usize = 192;
pub const MIN_FREQUENCY_HZ: f64 = 20.0;
pub const MAX_FREQUENCY_HZ: f64 = 20_000.0;
pub const FLOOR_DBFS: f32 = -120.0;

const MIN_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_SAMPLE_RATE_HZ: u32 = 192_000;
/// The bass stage runs at the highest rate of the form `rate / 2^n`, n <= 3,
/// that stays at or above this rate.
const MIN_BASS_RATE_HZ: u32 = 4_000;
const SLOPE_PIVOT_HZ: f64 = 1_000.0;
/// Bass weight is 1 below `CROSSOVER_BINS` full-rate bins and 0 above twice
/// that, blended log-linearly in between.
const CROSSOVER_BINS: f64 = 32.0;
/// Hann main lobe half-width plus one bin excluded around a peak candidate.
const GUARD_BINS: usize = 3;
/// Reference bins averaged on each side of the guard.
const REFERENCE_BINS: usize = 8;
/// A candidate must exceed its local reference mean by 15 dB to be tonal.
const TONAL_RATIO: f64 = 31.622_776_601_683_79;
const SILENT_POWER: f64 = 1.0e-12;
/// Smoothed (non-tonal) power is expressed per this bandwidth, the finest
/// bass-stage resolution at any supported rate (4000 Hz / 2048). Broadband
/// material therefore reads the same through both stages and at every sample
/// rate, while detected tones keep their calibrated sine level.
const REFERENCE_BANDWIDTH_HZ: f64 = MIN_BASS_RATE_HZ as f64 / BASS_FFT_SIZE as f64;

/// Frequency-domain display options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumSettings {
    /// Tilt added around 1 kHz, in dB per octave. Valid range -12..=12.
    pub slope_db_per_octave: f32,
    /// Power-averaging width, in octaves. Valid range 0..=1; zero disables.
    pub smoothing_octaves: f32,
}

impl SpectrumSettings {
    pub const DEFAULT: Self = Self {
        slope_db_per_octave: 4.5,
        smoothing_octaves: 1.0 / 12.0,
    };

    fn is_valid(self) -> bool {
        self.slope_db_per_octave.is_finite()
            && (-12.0..=12.0).contains(&self.slope_db_per_octave)
            && self.smoothing_octaves.is_finite()
            && (0.0..=1.0).contains(&self.smoothing_octaves)
    }
}

impl Default for SpectrumSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectrumError {
    InvalidSampleRate(u32),
    InvalidSettings,
    WindowNotReady,
    SampleRateMismatch { kernel: u32, window: u32 },
    NonFiniteSample { frame: usize, channel: usize },
}

impl fmt::Display for SpectrumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate(rate) => {
                write!(
                    formatter,
                    "spectrum sample rate {rate} is outside 8000..=192000 hz"
                )
            }
            Self::InvalidSettings => formatter.write_str("spectrum settings are out of range"),
            Self::WindowNotReady => {
                write!(
                    formatter,
                    "spectrum window holds fewer than {FFT_SIZE} frames"
                )
            }
            Self::SampleRateMismatch { kernel, window } => write!(
                formatter,
                "spectrum window rate {window} hz does not match kernel rate {kernel} hz"
            ),
            Self::NonFiniteSample { frame, channel } => {
                write!(
                    formatter,
                    "spectrum input frame {frame} channel {channel} is not finite"
                )
            }
        }
    }
}

impl Error for SpectrumError {}

#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumFrame {
    pub sample_rate_hz: u32,
    /// Displayed level per point, in dBFS with a -120 dBFS floor. A full-scale
    /// sine reads 0 dBFS at the slope pivot. Raw kernel output until a
    /// [`SpectrumBallistics`] update smooths it.
    pub bins_dbfs: [f32; SPECTRUM_BANDS],
    /// Held peak per point, in the same scale. Equal to `bins_dbfs` in raw
    /// kernel output; [`SpectrumBallistics`] holds and slowly releases it.
    pub peak_dbfs: [f32; SPECTRUM_BANDS],
}

impl SpectrumFrame {
    pub fn silent(sample_rate_hz: u32) -> Self {
        Self {
            sample_rate_hz,
            bins_dbfs: [FLOOR_DBFS; SPECTRUM_BANDS],
            peak_dbfs: [FLOOR_DBFS; SPECTRUM_BANDS],
        }
    }
}

/// One FFT stage's fixed workspace.
struct Stage {
    size: usize,
    resolution_hz: f64,
    /// `REFERENCE_BANDWIDTH_HZ / resolution_hz`, never above one.
    noise_scale: f64,
    window: Box<[f64]>,
    twiddles: Box<[(f64, f64)]>,
    real: Box<[f64]>,
    imaginary: Box<[f64]>,
    /// One-sided, window-normalized stereo-mean power per bin.
    power: Box<[f64]>,
    /// Exact integral of the piecewise-linear power density up to each bin.
    linear_integral: Box<[f64]>,
    /// Plain power prefix sums for detector reference means.
    prefix: Box<[f64]>,
    /// Inverse decimator power response per bin (1 at full rate).
    correction: Box<[f64]>,
    /// Highest point-level tonal power detected in this stage's last analysis.
    tonal: Box<[f64; SPECTRUM_BANDS]>,
}

impl Stage {
    fn new(size: usize, rate_hz: f64, decimation_input_rates: &[f64]) -> Self {
        let half = size / 2;
        let window: Box<[f64]> = (0..size)
            .map(|index| {
                let phase = std::f64::consts::TAU * index as f64 / size as f64;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        let resolution_hz = rate_hz / size as f64;
        let correction = (0..=half)
            .map(|bin| {
                let frequency = bin as f64 * resolution_hz;
                let response: f64 = decimation_input_rates
                    .iter()
                    .map(|input| (std::f64::consts::PI * frequency / input).cos().powi(8))
                    .product();
                (1.0 / response.max(1.0e-6)).min(1.0e6)
            })
            .collect();
        Self {
            size,
            resolution_hz,
            noise_scale: (REFERENCE_BANDWIDTH_HZ / resolution_hz).min(1.0),
            window,
            twiddles: (0..half)
                .map(|index| {
                    let angle = -std::f64::consts::TAU * index as f64 / size as f64;
                    (angle.cos(), angle.sin())
                })
                .collect(),
            real: vec![0.0; size].into_boxed_slice(),
            imaginary: vec![0.0; size].into_boxed_slice(),
            power: vec![0.0; half + 1].into_boxed_slice(),
            linear_integral: vec![0.0; half + 1].into_boxed_slice(),
            prefix: vec![0.0; half + 2].into_boxed_slice(),
            correction,
            tonal: Box::new([0.0; SPECTRUM_BANDS]),
        }
    }

    fn storage_quote(size: usize) -> Option<usize> {
        let half = size / 2;
        let f64s = size
            .checked_mul(3)?
            .checked_add((half + 1).checked_mul(3)?)?
            .checked_add(half + 2)?;
        f64s.checked_mul(std::mem::size_of::<f64>())?
            .checked_add(half.checked_mul(std::mem::size_of::<(f64, f64)>())?)?
            .checked_add(std::mem::size_of::<[f64; SPECTRUM_BANDS]>())
    }

    fn data_bytes(&self) -> Option<usize> {
        [
            std::mem::size_of_val(self.window.as_ref()),
            std::mem::size_of_val(self.twiddles.as_ref()),
            std::mem::size_of_val(self.real.as_ref()),
            std::mem::size_of_val(self.imaginary.as_ref()),
            std::mem::size_of_val(self.power.as_ref()),
            std::mem::size_of_val(self.linear_integral.as_ref()),
            std::mem::size_of_val(self.prefix.as_ref()),
            std::mem::size_of_val(self.correction.as_ref()),
            std::mem::size_of_val(self.tonal.as_ref()),
        ]
        .into_iter()
        .try_fold(0usize, usize::checked_add)
    }

    fn reset(&mut self) {
        self.real.fill(0.0);
        self.imaginary.fill(0.0);
        self.power.fill(0.0);
        self.linear_integral.fill(0.0);
        self.prefix.fill(0.0);
        self.tonal.fill(0.0);
    }

    /// Transform one ring (oldest frame at `oldest`) and derive power tables.
    /// Left and right are packed as one complex sequence; for real channels
    /// `|L(k)|^2 + |R(k)|^2 = (|Z(k)|^2 + |Z(N - k)|^2) / 2`.
    fn transform(&mut self, ring: &[[f32; 2]], oldest: usize) {
        let size = self.size;
        let mut window_sum = 0.0;
        for index in 0..size {
            let frame = ring[(oldest + index) % size];
            let weight = self.window[index];
            window_sum += weight;
            self.real[index] = f64::from(frame[0]) * weight;
            self.imaginary[index] = f64::from(frame[1]) * weight;
        }
        fft(&mut self.real, &mut self.imaginary, &self.twiddles);
        let half = size / 2;
        let mut linear = 0.0;
        let mut prefix = 0.0;
        for bin in 0..=half {
            let mirror = (size - bin) % size;
            let packed = self.real[bin].mul_add(self.real[bin], self.imaginary[bin].powi(2))
                + self.real[mirror].mul_add(self.real[mirror], self.imaginary[mirror].powi(2));
            let one_sided = if bin == 0 || bin == half { 1.0 } else { 2.0 } / window_sum;
            // Stereo mean of per-channel one-sided power.
            let power = packed * 0.25 * one_sided * one_sided * self.correction[bin];
            if bin > 0 {
                linear += 0.5 * (self.power[bin - 1] + power);
            }
            self.power[bin] = power;
            self.linear_integral[bin] = linear;
            self.prefix[bin] = prefix;
            prefix += power;
        }
        self.prefix[half + 1] = prefix;
    }

    /// Integral of the piecewise-linear density from bin 0 to `position`.
    fn integral(&self, position: f64) -> f64 {
        let half = self.size / 2;
        let position = position.clamp(0.0, half as f64);
        let bin = (position.floor() as usize).min(half - 1);
        let t = position - bin as f64;
        let low = self.power[bin];
        let high = self.power[bin + 1];
        self.linear_integral[bin] + low * t + (high - low) * t * t * 0.5
    }

    fn density(&self, position: f64) -> f64 {
        let half = self.size / 2;
        let position = position.clamp(0.0, half as f64);
        let bin = (position.floor() as usize).min(half - 1);
        let t = position - bin as f64;
        self.power[bin] + (self.power[bin + 1] - self.power[bin]) * t
    }

    /// Mean density over a band, in Hz, per reference bandwidth. Narrow
    /// bands converge on interpolation.
    fn smoothed(&self, low_hz: f64, high_hz: f64) -> f64 {
        let half = self.size as f64 / 2.0;
        let low = (low_hz / self.resolution_hz).clamp(1.0, half);
        let high = (high_hz / self.resolution_hz).clamp(1.0, half);
        let mean = if high - low < 1.0e-6 {
            self.density((low + high) * 0.5)
        } else {
            ((self.integral(high) - self.integral(low)) / (high - low)).max(0.0)
        };
        mean * self.noise_scale
    }

    /// Detect tonal peaks up to `limit_hz` and record each one's corrected
    /// level at the display point containing its interpolated frequency.
    fn detect_tonal(&mut self, sample_rate_hz: u32, limit_hz: f64) {
        self.tonal.fill(0.0);
        let half = self.size / 2;
        let last = ((limit_hz / self.resolution_hz).ceil() as usize).min(half - 1);
        for bin in 2..=last {
            let peak = self.power[bin];
            if peak <= SILENT_POWER || peak <= self.power[bin - 1] || peak < self.power[bin + 1] {
                continue;
            }
            let left_end = bin.saturating_sub(GUARD_BINS);
            let left_start = bin.saturating_sub(GUARD_BINS + REFERENCE_BINS).max(1);
            let right_start = (bin + GUARD_BINS + 1).min(half + 1);
            let right_end = (bin + GUARD_BINS + 1 + REFERENCE_BINS).min(half + 1);
            let left_count = left_end.saturating_sub(left_start);
            let right_count = right_end.saturating_sub(right_start);
            let count = left_count + right_count;
            if count < REFERENCE_BINS {
                continue;
            }
            let reference = (self.prefix[left_end.max(left_start)] - self.prefix[left_start]
                + self.prefix[right_end]
                - self.prefix[right_start])
                / count as f64;
            if peak < reference * TONAL_RATIO {
                continue;
            }
            let (offset, level) = hann_peak(self.power[bin - 1], peak, self.power[bin + 1]);
            let frequency = (bin as f64 + offset) * self.resolution_hz;
            if let Some(point) = point_index(sample_rate_hz, frequency) {
                self.tonal[point] = self.tonal[point].max(level);
            }
        }
    }
}

/// Interpolate a Hann-windowed tone from its peak bin and larger neighbor.
///
/// For a Hann window the neighbor/peak magnitude ratio `a` gives the exact
/// fractional offset `d = (2a - 1) / (a + 1)`, and the peak bin sits at
/// `W(d) = sin(pi d) / (pi d (1 - d^2))` of the tone's amplitude. Returns the
/// signed bin offset and the corrected tone power.
fn hann_peak(left: f64, center: f64, right: f64) -> (f64, f64) {
    let (neighbor, direction) = if right >= left {
        (right, 1.0)
    } else {
        (left, -1.0)
    };
    let ratio = (neighbor / center).sqrt();
    let offset = ((2.0 * ratio - 1.0) / (ratio + 1.0)).clamp(0.0, 0.5);
    let response = if offset < 1.0e-9 {
        1.0
    } else {
        let phase = std::f64::consts::PI * offset;
        phase.sin() / (phase * (1.0 - offset * offset))
    };
    (direction * offset, center / (response * response))
}

#[derive(Clone, Copy)]
struct Point {
    center_hz: f64,
    bass_weight: f64,
    slope_db: f32,
}

/// Multi-resolution FFT, smoothing and tonal-peak reducer for one sample rate.
pub struct SpectrumKernel {
    sample_rate_hz: u32,
    settings: SpectrumSettings,
    smoothing_ratio: f64,
    bass_limit_hz: f64,
    points: Box<[Point; SPECTRUM_BANDS]>,
    high: Stage,
    bass: Stage,
}

impl SpectrumKernel {
    pub(crate) fn heap_storage_quote() -> Option<usize> {
        Stage::storage_quote(FFT_SIZE)?
            .checked_add(Stage::storage_quote(BASS_FFT_SIZE)?)?
            .checked_add(std::mem::size_of::<[Point; SPECTRUM_BANDS]>())
    }

    pub(crate) fn heap_data_bytes(&self) -> Option<usize> {
        self.high
            .data_bytes()?
            .checked_add(self.bass.data_bytes()?)?
            .checked_add(std::mem::size_of_val(self.points.as_ref()))
    }

    /// Construct with [`SpectrumSettings::DEFAULT`].
    pub fn new(sample_rate_hz: u32) -> Result<Self, SpectrumError> {
        Self::with_settings(sample_rate_hz, SpectrumSettings::DEFAULT)
    }

    /// Construct reusable fixed storage for a supported audio sample rate.
    pub fn with_settings(
        sample_rate_hz: u32,
        settings: SpectrumSettings,
    ) -> Result<Self, SpectrumError> {
        validate_rate(sample_rate_hz)?;
        if !settings.is_valid() {
            return Err(SpectrumError::InvalidSettings);
        }
        let rate = f64::from(sample_rate_hz);
        let stages = bass_decimation_stages(sample_rate_hz);
        let input_rates: Vec<f64> = (0..stages)
            .map(|stage| rate / f64::from(1u32 << stage))
            .collect();
        let bass_rate = rate / f64::from(1u32 << stages);
        let high = Stage::new(FFT_SIZE, rate, &[]);
        let bass = Stage::new(BASS_FFT_SIZE, bass_rate, &input_rates);
        let crossover_low = high.resolution_hz * CROSSOVER_BINS;
        let crossover_high = crossover_low * 2.0;
        let slope = f64::from(settings.slope_db_per_octave);
        let points = Box::new(std::array::from_fn(|index| {
            let (low_hz, high_hz) = band_bounds(sample_rate_hz, index);
            let center_hz = (low_hz * high_hz).sqrt();
            let bass_weight = if center_hz <= crossover_low {
                1.0
            } else if center_hz >= crossover_high {
                0.0
            } else {
                1.0 - (center_hz / crossover_low).log2()
            };
            Point {
                center_hz,
                bass_weight,
                slope_db: (slope * (center_hz / SLOPE_PIVOT_HZ).log2()) as f32,
            }
        }));
        Ok(Self {
            sample_rate_hz,
            settings,
            smoothing_ratio: 2f64.powf(f64::from(settings.smoothing_octaves) * 0.5),
            bass_limit_hz: crossover_high * 2f64.powf(1.0 / 6.0),
            points,
            high,
            bass,
        })
    }

    pub fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }

    pub fn settings(&self) -> SpectrumSettings {
        self.settings
    }

    /// Analyze the newest complete window without modifying it.
    ///
    /// Stereo power is averaged between channels, so opposite-polarity
    /// material does not cancel. Inputs above full scale may produce positive
    /// dBFS values. The returned peak curve equals the level curve.
    pub fn analyze(&mut self, window: &SpectrumWindow) -> Result<SpectrumFrame, SpectrumError> {
        if window.sample_rate_hz() != self.sample_rate_hz {
            return Err(SpectrumError::SampleRateMismatch {
                kernel: self.sample_rate_hz,
                window: window.sample_rate_hz(),
            });
        }
        if !window.is_ready() {
            return Err(SpectrumError::WindowNotReady);
        }
        let nyquist = f64::from(self.sample_rate_hz) * 0.5;
        self.high.transform(&window.high, window.high_write);
        self.high.detect_tonal(self.sample_rate_hz, nyquist);
        let bass_ready = window.bass_ready();
        if bass_ready {
            self.bass.transform(&window.bass, window.bass_write);
            self.bass
                .detect_tonal(self.sample_rate_hz, self.bass_limit_hz);
        }

        let mut bins_dbfs = [FLOOR_DBFS; SPECTRUM_BANDS];
        for (index, point) in self.points.iter().enumerate() {
            let low = point.center_hz / self.smoothing_ratio;
            let high = point.center_hz * self.smoothing_ratio;
            let bass_weight = if bass_ready { point.bass_weight } else { 0.0 };
            let mut power = 0.0;
            if bass_weight < 1.0 {
                let level = self.high.smoothed(low, high).max(self.high.tonal[index]);
                power += (1.0 - bass_weight) * level;
            }
            if bass_weight > 0.0 {
                let level = self.bass.smoothed(low, high).max(self.bass.tonal[index]);
                power += bass_weight * level;
            }
            if power > SILENT_POWER {
                bins_dbfs[index] = ((10.0 * power.log10()) as f32 + point.slope_db).max(FLOOR_DBFS);
            }
        }
        Ok(SpectrumFrame {
            sample_rate_hz: self.sample_rate_hz,
            bins_dbfs,
            peak_dbfs: bins_dbfs,
        })
    }

    /// Clear transform workspaces after a caller-observed stream gap.
    pub fn reset(&mut self) {
        self.high.reset();
        self.bass.reset();
    }
}

fn validate_rate(sample_rate_hz: u32) -> Result<(), SpectrumError> {
    if (MIN_SAMPLE_RATE_HZ..=MAX_SAMPLE_RATE_HZ).contains(&sample_rate_hz) {
        Ok(())
    } else {
        Err(SpectrumError::InvalidSampleRate(sample_rate_hz))
    }
}

/// Number of 2:1 decimators feeding the bass stage at a supported rate.
fn bass_decimation_stages(sample_rate_hz: u32) -> usize {
    (1..=window::MAX_DECIMATION_STAGES)
        .rev()
        .find(|stages| sample_rate_hz >> stages >= MIN_BASS_RATE_HZ)
        .unwrap_or(1)
}

/// Return the logarithmic frequency bounds for a spectrum band.
pub fn band_bounds_hz(sample_rate_hz: u32, band: usize) -> Option<(f32, f32)> {
    if validate_rate(sample_rate_hz).is_err() || band >= SPECTRUM_BANDS {
        return None;
    }
    let (lower, upper) = band_bounds(sample_rate_hz, band);
    Some((lower as f32, upper as f32))
}

/// Return a band's geometric center frequency, where its point is plotted.
pub fn band_center_hz(sample_rate_hz: u32, band: usize) -> Option<f32> {
    if validate_rate(sample_rate_hz).is_err() || band >= SPECTRUM_BANDS {
        return None;
    }
    let (lower, upper) = band_bounds(sample_rate_hz, band);
    Some((lower * upper).sqrt() as f32)
}

/// Highest displayed frequency at a supported sample rate.
pub fn maximum_frequency_hz(sample_rate_hz: u32) -> Option<f32> {
    validate_rate(sample_rate_hz).ok()?;
    Some(maximum_frequency(sample_rate_hz) as f32)
}

fn band_bounds(sample_rate_hz: u32, band: usize) -> (f64, f64) {
    let ratio = maximum_frequency(sample_rate_hz) / MIN_FREQUENCY_HZ;
    let lower = MIN_FREQUENCY_HZ * ratio.powf(band as f64 / SPECTRUM_BANDS as f64);
    let upper = MIN_FREQUENCY_HZ * ratio.powf((band + 1) as f64 / SPECTRUM_BANDS as f64);
    (lower, upper)
}

fn point_index(sample_rate_hz: u32, frequency_hz: f64) -> Option<usize> {
    let maximum = maximum_frequency(sample_rate_hz);
    if !(MIN_FREQUENCY_HZ..maximum).contains(&frequency_hz) {
        return None;
    }
    let position = (frequency_hz / MIN_FREQUENCY_HZ).ln() / (maximum / MIN_FREQUENCY_HZ).ln();
    Some(((position * SPECTRUM_BANDS as f64) as usize).min(SPECTRUM_BANDS - 1))
}

fn maximum_frequency(sample_rate_hz: u32) -> f64 {
    MAX_FREQUENCY_HZ.min(f64::from(sample_rate_hz) * 0.49)
}

/// In-place radix-2 FFT. `twiddles[j] = exp(-2 pi i j / n)` for `j < n / 2`.
fn fft(real: &mut [f64], imaginary: &mut [f64], twiddles: &[(f64, f64)]) {
    let size = real.len();
    let mut reversed = 0usize;
    for index in 1..size {
        let mut bit = size >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            real.swap(index, reversed);
            imaginary.swap(index, reversed);
        }
    }

    let mut stage = 2;
    while stage <= size {
        let half = stage / 2;
        let stride = size / stage;
        for start in (0..size).step_by(stage) {
            for offset in 0..half {
                let (twiddle_real, twiddle_imaginary) = twiddles[offset * stride];
                let even = start + offset;
                let odd = even + half;
                let odd_real = real[odd] * twiddle_real - imaginary[odd] * twiddle_imaginary;
                let odd_imaginary = real[odd] * twiddle_imaginary + imaginary[odd] * twiddle_real;
                let even_real = real[even];
                let even_imaginary = imaginary[even];
                real[even] = even_real + odd_real;
                imaginary[even] = even_imaginary + odd_imaginary;
                real[odd] = even_real - odd_real;
                imaginary[odd] = even_imaginary - odd_imaginary;
            }
        }
        stage *= 2;
    }
}

/// Remove [`SpectrumSettings::DEFAULT`]'s slope from a frame's level curve,
/// leaving floor values in place, for calibrated assertions in worker tests.
#[cfg(test)]
pub(crate) fn without_default_slope(frame: &SpectrumFrame) -> [f32; SPECTRUM_BANDS] {
    std::array::from_fn(|point| {
        let level = frame.bins_dbfs[point];
        if level <= FLOOR_DBFS {
            return level;
        }
        let (low, high) = band_bounds(frame.sample_rate_hz, point);
        let octaves = ((low * high).sqrt() / SLOPE_PIVOT_HZ).log2();
        level - (f64::from(SpectrumSettings::DEFAULT.slope_db_per_octave) * octaves) as f32
    })
}

#[cfg(test)]
mod tests;
