//! Explicit opt-in export post-processing: deterministic target sample-rate
//! conversion (reusing `hex-media`'s windowed-sinc V2 resampler) and peak or
//! ITU-R BS.1770-4 K-weighted integrated loudness normalization.
//!
//! Normalization is strictly opt-in (`ExportNormalization::Off` by default)
//! and always reports the exact linear and dB gain applied to the exported
//! signal. Digital silence never divides by zero and always applies unity gain
//! (`1.0` linear, `0.0 dB`).

use crate::{ExportControl, ExportError};
use hex_engine::Frame;
use hex_media::{ResampleKernel, ResamplePlan, ResampleVersion};
use std::f64::consts::PI;

/// Supported target sample-rate bounds matching `hex-media`.
const MIN_TARGET_SAMPLE_RATE: u32 = 8_000;
const MAX_TARGET_SAMPLE_RATE: u32 = 192_000;
/// Bounded maximum frame count for post-processed (SRC / normalized) exports.
/// Covers > 2 hours at 48 kHz while preventing runaway allocations.
pub(crate) const MAX_POST_PROCESSED_FRAMES: usize = 360_000_000;
/// Converted frames rendered between cooperative cancellation polls during SRC.
const SRC_CANCEL_CHUNK_FRAMES: usize = 16_384;

/// Explicit opt-in normalization mode for an export job.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ExportNormalization {
    /// No gain adjustment (default). Reports unity gain (`1.0`, `0.0 dB`).
    #[default]
    Off,
    /// Normalize sample peak to `target_dbfs` (finite, `<= 0.0`).
    Peak { target_dbfs: f64 },
    /// Normalize ITU-R BS.1770-4 K-weighted integrated loudness to `target_lufs`
    /// (finite, `-70.0..=0.0`), optionally clamping linear gain so 4x oversampled
    /// true peak does not exceed `true_peak_ceiling_dbfs` (finite, `<= 0.0`).
    Loudness {
        target_lufs: f64,
        true_peak_ceiling_dbfs: Option<f64>,
    },
}

impl ExportNormalization {
    pub fn validate(self) -> Result<(), ExportError> {
        match self {
            Self::Off => Ok(()),
            Self::Peak { target_dbfs } => {
                if !target_dbfs.is_finite() || !(-144.0..=0.0).contains(&target_dbfs) {
                    return Err(ExportError::Invalid(
                        "peak normalization target must be finite and within [-144, 0] dbfs",
                    ));
                }
                Ok(())
            }
            Self::Loudness {
                target_lufs,
                true_peak_ceiling_dbfs,
            } => {
                if !target_lufs.is_finite() || !(-70.0..=0.0).contains(&target_lufs) {
                    return Err(ExportError::Invalid(
                        "loudness normalization target must be finite and within [-70, 0] lufs",
                    ));
                }
                if let Some(ceiling) = true_peak_ceiling_dbfs
                    && (!ceiling.is_finite() || !(-144.0..=0.0).contains(&ceiling))
                {
                    return Err(ExportError::Invalid(
                        "loudness peak ceiling must be finite and within [-144, 0] dbfs",
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Optional target sample-rate conversion and normalization applied after
/// causal Arrangement rendering and PDC preroll discard.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExportOptions {
    /// Optional output sample rate in Hz (`8_000..=192_000`). `None` keeps the
    /// project's native sample rate without resampling.
    pub target_sample_rate: Option<u32>,
    /// Explicit opt-in normalization policy. Defaults to [`ExportNormalization::Off`].
    pub normalization: ExportNormalization,
}

impl ExportOptions {
    pub fn validate(self) -> Result<(), ExportError> {
        if let Some(rate) = self.target_sample_rate
            && !(MIN_TARGET_SAMPLE_RATE..=MAX_TARGET_SAMPLE_RATE).contains(&rate)
        {
            return Err(ExportError::Invalid(
                "target sample rate must be within 8000..=192000 hz",
            ));
        }
        self.normalization.validate()
    }

    pub fn is_identity(self, source_sample_rate: u32) -> bool {
        self.target_sample_rate
            .is_none_or(|rate| rate == source_sample_rate)
            && matches!(self.normalization, ExportNormalization::Off)
    }
}

/// Exact linear and dB gain applied by export normalization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedGain {
    pub linear: f64,
    pub db: f64,
}

impl Default for AppliedGain {
    fn default() -> Self {
        Self::UNITY
    }
}

impl AppliedGain {
    pub const UNITY: Self = Self {
        linear: 1.0,
        db: 0.0,
    };

    fn from_linear(linear: f64) -> Self {
        if !linear.is_finite() || linear <= 0.0 {
            Self::UNITY
        } else {
            Self {
                linear,
                db: 20.0 * linear.log10(),
            }
        }
    }
}

/// Resample rendered stereo PCM to `target_rate` using `hex-media`'s
/// deterministic windowed-sinc V2 kernel, polling live cooperative cancellation
/// per chunk, allocating the output buffer fallibly, and dropping the input
/// buffer immediately after conversion (avoiding a 3x RAM `.to_vec()` copy).
pub(crate) fn resample_export_frames(
    source_rate: u32,
    target_rate: u32,
    frames: Vec<Frame>,
    control: &ExportControl,
) -> Result<Vec<Frame>, ExportError> {
    resample_export_frames_with_hook(source_rate, target_rate, frames, control, |_| {})
}

fn resample_export_frames_with_hook(
    source_rate: u32,
    target_rate: u32,
    frames: Vec<Frame>,
    control: &ExportControl,
    mut after_chunk: impl FnMut(usize),
) -> Result<Vec<Frame>, ExportError> {
    control.check()?;
    if source_rate == target_rate || frames.is_empty() {
        return Ok(frames);
    }
    let source_frames = u64::try_from(frames.len())
        .map_err(|_| ExportError::Invalid("rendered export frame count exceeds range"))?;
    let max_frames = u64::try_from(MAX_POST_PROCESSED_FRAMES)
        .map_err(|_| ExportError::Invalid("post-processed export frame limit exceeds range"))?;
    let map_media = |err: hex_media::ImportError| match err {
        hex_media::ImportError::Cancelled => ExportError::Cancelled,
        hex_media::ImportError::AllocationFailed => {
            ExportError::Invalid("could not allocate resampled export buffer")
        }
        other => ExportError::Media(format!("target sample-rate conversion failed: {other}")),
    };
    let kernel = ResampleKernel::with_version(source_rate, target_rate, ResampleVersion::V2)
        .map_err(map_media)?;
    let plan = ResamplePlan::with_kernel(kernel, source_frames, max_frames, max_frames, usize::MAX)
        .map_err(map_media)?;
    let output_capacity = usize::try_from(plan.output_frames())
        .map_err(|_| ExportError::Invalid("resampled export frame count exceeds range"))?;
    control.check()?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_capacity)
        .map_err(|_| ExportError::Invalid("could not allocate resampled export buffer"))?;
    output.resize(output_capacity, [0.0; 2]);
    let mut window_start = 0_u64;
    for (chunk_index, window) in output.chunks_mut(SRC_CANCEL_CHUNK_FRAMES).enumerate() {
        control.check()?;
        plan.render_window(0, &frames, window_start, window)
            .map_err(map_media)?;
        let count = u64::try_from(window.len())
            .map_err(|_| ExportError::Invalid("resampled export window range overflows"))?;
        window_start = window_start.checked_add(count).ok_or(ExportError::Invalid(
            "resampled export window range overflows",
        ))?;
        after_chunk(chunk_index);
    }
    drop(frames);
    control.check()?;
    Ok(output)
}

/// Measure 4x oversampled inter-sample true peak using a 4-phase, 12-tap-per-phase
/// Blackman-windowed sinc polyphase FIR (phase 0 is the exact integer sample, so
/// the result is always `>= sample_peak`). Zero heap allocations.
fn measure_true_peak_4x(frames: &[Frame], sample_peak: f64) -> f64 {
    if frames.len() < 2 {
        return sample_peak;
    }
    let phases: [[f64; 12]; 3] = std::array::from_fn(|p| {
        let frac = (p + 1) as f64 * 0.25;
        let mut taps = [0.0_f64; 12];
        let mut sum = 0.0_f64;
        for (j, tap) in taps.iter_mut().enumerate() {
            let t = (j as i32 - 5) as f64 - frac;
            let u = (t + 6.0) / 12.0;
            let window = 0.42 - 0.5 * (2.0 * PI * u).cos() + 0.08 * (4.0 * PI * u).cos();
            let sinc = (PI * t).sin() / (PI * t);
            let value = sinc * window;
            *tap = value;
            sum += value;
        }
        if sum != 0.0 {
            for tap in &mut taps {
                *tap /= sum;
            }
        }
        taps
    });

    let len = frames.len();
    let mut true_peak = sample_peak;
    for i in 0..len {
        for taps in &phases {
            let mut acc_l = 0.0_f64;
            let mut acc_r = 0.0_f64;
            for (j, &coeff) in taps.iter().enumerate() {
                let offset = j as isize - 5;
                let idx = i as isize + offset;
                if idx >= 0 && (idx as usize) < len {
                    let [l, r] = frames[idx as usize];
                    acc_l += f64::from(l) * coeff;
                    acc_r += f64::from(r) * coeff;
                }
            }
            true_peak = true_peak.max(acc_l.abs()).max(acc_r.abs());
        }
    }
    true_peak
}

/// Compute and apply explicit opt-in normalization in-place, returning the
/// exact applied gain. Digital silence is left untouched with unity gain.
pub(crate) fn apply_normalization(
    frames: &mut [Frame],
    sample_rate: u32,
    mode: ExportNormalization,
) -> Result<AppliedGain, ExportError> {
    mode.validate()?;
    if frames.is_empty() || matches!(mode, ExportNormalization::Off) {
        return Ok(AppliedGain::UNITY);
    }

    let mut peak = 0.0_f64;
    for &[left, right] in frames.iter() {
        if !left.is_finite() || !right.is_finite() {
            return Err(ExportError::Invalid("render produced a non-finite sample"));
        }
        peak = peak.max(f64::from(left).abs()).max(f64::from(right).abs());
    }
    if peak == 0.0 {
        return Ok(AppliedGain::UNITY);
    }

    let gain = match mode {
        ExportNormalization::Off => 1.0,
        ExportNormalization::Peak { target_dbfs } => {
            let target_linear = db_to_linear(target_dbfs);
            target_linear / peak
        }
        ExportNormalization::Loudness {
            target_lufs,
            true_peak_ceiling_dbfs,
        } => {
            let Some(measured_lufs) = measure_integrated_lufs(frames, sample_rate) else {
                return Ok(AppliedGain::UNITY);
            };
            let mut linear = db_to_linear(target_lufs - measured_lufs);
            if let Some(ceiling_dbfs) = true_peak_ceiling_dbfs {
                let ceiling_linear = db_to_linear(ceiling_dbfs);
                let true_peak = measure_true_peak_4x(frames, peak);
                linear = linear.min(ceiling_linear / true_peak);
            }
            linear
        }
    };

    if !gain.is_finite() || gain <= 0.0 {
        return Ok(AppliedGain::UNITY);
    }
    let applied = AppliedGain::from_linear(gain);
    for frame in frames.iter_mut() {
        frame[0] = (f64::from(frame[0]) * applied.linear) as f32;
        frame[1] = (f64::from(frame[1]) * applied.linear) as f32;
    }
    Ok(applied)
}

fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

/// Direct Form II transposed biquad filter in `f64` for K-weighting.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// ITU-R BS.1770-4 Stage 1 (pre-filter high-shelf) and Stage 2 (RLB high-pass)
/// computed via exact bilinear transform for any sample rate.
fn k_weighting_filters(sample_rate: u32) -> (Biquad, Biquad) {
    let fs = f64::from(sample_rate);
    // Stage 1: +3.999843853973347 dB high-shelf at 1681.974450955533 Hz, Q = 0.7071752369554196
    let f0_shelf = 1_681.974_450_955_533_f64;
    let db_shelf = 3.999_843_853_973_347_f64;
    let q_shelf = 0.707_175_236_955_419_6_f64;
    let k = (PI * f0_shelf / fs).tan();
    let vh = 10.0_f64.powf(db_shelf / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q_shelf + k * k;
    let shelf = Biquad::new(
        (vh + vb * k / q_shelf + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q_shelf + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q_shelf + k * k) / a0,
    );

    // Stage 2: RLB high-pass at 38.13547087602444 Hz, Q = 0.5003270373238773
    let f0_hp = 38.135_470_876_024_44_f64;
    let q_hp = 0.500_327_037_323_877_3_f64;
    let k_hp = (PI * f0_hp / fs).tan();
    let a0_hp = 1.0 + k_hp / q_hp + k_hp * k_hp;
    let highpass = Biquad::new(
        1.0 / a0_hp,
        -2.0 / a0_hp,
        1.0 / a0_hp,
        2.0 * (k_hp * k_hp - 1.0) / a0_hp,
        (1.0 - k_hp / q_hp + k_hp * k_hp) / a0_hp,
    );
    (shelf, highpass)
}

/// Measure ITU-R BS.1770-4 K-weighted integrated loudness in LUFS.
/// Returns `None` for digital silence or below-gate signals.
pub fn measure_integrated_lufs(frames: &[Frame], sample_rate: u32) -> Option<f64> {
    if frames.is_empty() || sample_rate == 0 {
        return None;
    }
    let (mut shelf_l, mut hp_l) = k_weighting_filters(sample_rate);
    let (mut shelf_r, mut hp_r) = k_weighting_filters(sample_rate);

    let block_frames = ((u64::from(sample_rate) * 400) / 1000).max(1) as usize;
    let hop_frames = ((u64::from(sample_rate) * 100) / 1000).max(1) as usize;

    // Short-clip fallback when clip duration < 400 ms: measure K-weighted mean
    // power over the entire clip so short cues still normalize deterministically.
    if frames.len() < block_frames {
        let mut sum_sq = 0.0_f64;
        for &[l, r] in frames {
            let kl = hp_l.process(shelf_l.process(f64::from(l)));
            let kr = hp_r.process(shelf_r.process(f64::from(r)));
            sum_sq += kl * kl + kr * kr;
        }
        let mean_power = sum_sq / (frames.len() as f64);
        if mean_power <= 0.0 {
            return None;
        }
        let lufs = -0.691 + 10.0 * mean_power.log10();
        return (lufs > -70.0 && lufs.is_finite()).then_some(lufs);
    }

    // Stream K-weighted squared power in 100 ms sub-blocks so memory stays
    // bounded to O(duration / 100ms) instead of storing full-length f64 copies.
    let mut hop_sums: Vec<f64> = Vec::new();
    let mut current_sum = 0.0_f64;
    let mut in_hop = 0_usize;
    for &[l, r] in frames {
        let kl = hp_l.process(shelf_l.process(f64::from(l)));
        let kr = hp_r.process(shelf_r.process(f64::from(r)));
        current_sum += kl * kl + kr * kr;
        in_hop += 1;
        if in_hop == hop_frames {
            hop_sums.push(current_sum);
            current_sum = 0.0;
            in_hop = 0;
        }
    }

    if hop_sums.len() < 4 {
        return None;
    }
    let inv_block = 1.0 / (4 * hop_frames) as f64;
    let mut block_powers: Vec<f64> = Vec::with_capacity(hop_sums.len() - 3);
    for window in hop_sums.windows(4) {
        let power = (window[0] + window[1] + window[2] + window[3]) * inv_block;
        block_powers.push(power);
    }

    // Step 1: Absolute gate at -70 LUFS.
    let abs_threshold = 10.0_f64.powf((-70.0 + 0.691) / 10.0);
    let mut abs_sum = 0.0_f64;
    let mut abs_count = 0_usize;
    for &p in &block_powers {
        if p > abs_threshold {
            abs_sum += p;
            abs_count += 1;
        }
    }
    if abs_count == 0 {
        return None;
    }
    let ungated_lufs = -0.691 + 10.0 * (abs_sum / abs_count as f64).log10();

    // Step 2: Relative gate at ungated_lufs - 10 LU.
    let rel_threshold = 10.0_f64.powf((ungated_lufs - 10.0 + 0.691) / 10.0);
    let mut rel_sum = 0.0_f64;
    let mut rel_count = 0_usize;
    for &p in &block_powers {
        if p > abs_threshold && p > rel_threshold {
            rel_sum += p;
            rel_count += 1;
        }
    }
    if rel_count == 0 {
        return None;
    }
    let integrated = -0.691 + 10.0 * (rel_sum / rel_count as f64).log10();
    integrated.is_finite().then_some(integrated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_stereo(sample_rate: u32, frames: usize, hz: f64, amplitude: f32) -> Vec<Frame> {
        (0..frames)
            .map(|i| {
                let s =
                    ((2.0 * PI * hz * i as f64) / f64::from(sample_rate)).sin() as f32 * amplitude;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn peak_normalization_hits_target_and_reports_exact_gain() {
        let mut frames = sine_stereo(48_000, 4800, 1000.0, 0.25);
        let gain = apply_normalization(
            &mut frames,
            48_000,
            ExportNormalization::Peak { target_dbfs: -6.0 },
        )
        .unwrap();
        let expected_peak = db_to_linear(-6.0);
        let actual_peak = frames
            .iter()
            .flat_map(|f| [f[0].abs(), f[1].abs()])
            .fold(0.0_f32, f32::max);
        assert!((f64::from(actual_peak) - expected_peak).abs() < 1e-4);
        assert!((gain.db - (20.0 * gain.linear.log10())).abs() < 1e-12);
        assert!((gain.linear - expected_peak / 0.25).abs() < 1e-3);
    }

    #[test]
    fn loudness_normalization_hits_lufs_target_and_respects_peak_ceiling() {
        // ITU-R BS.1770-4 calibrates a stereo 1 kHz 0 dBFS sine to ~0.0 LUFS
        // (+0.65 dB K-weighting boost at 1 kHz cancels the -0.691 dB offset), so
        // amplitude 0.1 (-20 dBFS) measures ~ -20.04 LUFS.
        let mut frames = sine_stereo(48_000, 48_000, 1000.0, 0.1);
        let initial_lufs = measure_integrated_lufs(&frames, 48_000).unwrap();
        assert!((initial_lufs - (-20.0)).abs() < 0.15, "got {initial_lufs}");

        let gain = apply_normalization(
            &mut frames,
            48_000,
            ExportNormalization::Loudness {
                target_lufs: -14.0,
                true_peak_ceiling_dbfs: Some(-1.0),
            },
        )
        .unwrap();
        let after_lufs = measure_integrated_lufs(&frames, 48_000).unwrap();
        assert!((after_lufs - (-14.0)).abs() < 0.05, "got {after_lufs}");
        assert!((gain.db - (-14.0 - initial_lufs)).abs() < 1e-6);

        // Now push target to -1 LUFS with a -6 dBFS ceiling -> ceiling clamps gain.
        let mut clamped = sine_stereo(48_000, 48_000, 1000.0, 0.1);
        let clamped_gain = apply_normalization(
            &mut clamped,
            48_000,
            ExportNormalization::Loudness {
                target_lufs: -1.0,
                true_peak_ceiling_dbfs: Some(-6.0),
            },
        )
        .unwrap();
        let peak = clamped
            .iter()
            .flat_map(|f| [f[0].abs(), f[1].abs()])
            .fold(0.0_f32, f32::max);
        assert!((f64::from(peak) - db_to_linear(-6.0)).abs() < 1e-4);
        assert!(clamped_gain.db < 19.0);
    }

    #[test]
    fn silence_never_modifies_samples_and_reports_unity_gain() {
        let mut silent = vec![[0.0, 0.0]; 48_000];
        for mode in [
            ExportNormalization::Peak { target_dbfs: -1.0 },
            ExportNormalization::Loudness {
                target_lufs: -14.0,
                true_peak_ceiling_dbfs: Some(-1.0),
            },
        ] {
            let gain = apply_normalization(&mut silent, 48_000, mode).unwrap();
            assert_eq!(gain, AppliedGain::UNITY);
            assert!(silent.iter().all(|&f| f == [0.0, 0.0]));
        }
    }

    #[test]
    fn invalid_options_are_refused() {
        assert!(
            ExportOptions {
                target_sample_rate: Some(1_000),
                normalization: ExportNormalization::Off,
            }
            .validate()
            .is_err()
        );
        assert!(
            ExportNormalization::Peak { target_dbfs: 1.0 }
                .validate()
                .is_err()
        );
        assert!(
            ExportNormalization::Loudness {
                target_lufs: f64::NAN,
                true_peak_ceiling_dbfs: None,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn resample_export_frames_honors_live_cancellation_mid_conversion() {
        let frames = sine_stereo(48_000, 40_000, 440.0, 0.25);
        let control = ExportControl::default();
        let mut chunks_completed = 0_usize;
        let result =
            resample_export_frames_with_hook(48_000, 44_100, frames, &control, |chunk_index| {
                chunks_completed += 1;
                if chunk_index == 0 {
                    control.cancel();
                }
            });
        assert!(matches!(result, Err(ExportError::Cancelled)));
        assert_eq!(chunks_completed, 1);
    }

    #[test]
    fn loudness_true_peak_ceiling_clamps_intersample_peaks_above_sample_peak() {
        // Fs/4 alternating pairs [+0.5, +0.5, -0.5, -0.5] have sample peak 0.5
        // (-6.02 dBFS) but 4x oversampled true peak ~0.707 (-3.01 dBTP).
        let pattern = [0.5_f32, 0.5, -0.5, -0.5];
        let mut frames: Vec<Frame> = (0..48_000)
            .map(|i| {
                let s = pattern[i % 4];
                [s, s]
            })
            .collect();
        let initial_tp = measure_true_peak_4x(&frames, 0.5);
        assert!(
            initial_tp > 0.68,
            "expected ~0.707 true peak, got {initial_tp}"
        );

        apply_normalization(
            &mut frames,
            48_000,
            ExportNormalization::Loudness {
                target_lufs: -1.0,
                true_peak_ceiling_dbfs: Some(-6.0),
            },
        )
        .unwrap();
        let after_sample_peak = frames
            .iter()
            .flat_map(|f| [f[0].abs(), f[1].abs()])
            .fold(0.0_f64, |acc, s| acc.max(f64::from(s)));
        let after_true_peak = measure_true_peak_4x(&frames, after_sample_peak);
        let ceiling = db_to_linear(-6.0);
        assert!(
            (after_true_peak - ceiling).abs() < 1e-3,
            "expected true peak near {ceiling}, got {after_true_peak}"
        );
        assert!(
            after_sample_peak < ceiling - 0.1,
            "sample peak {after_sample_peak} should be well below true-peak ceiling {ceiling}"
        );
    }
}
