//! Original source queries through bounded scalar coefficients and PCM borrows.
use super::*;
use hex_lattice::DeviceResourcePcm;

/// Copyable scalar-only voice coefficients. Gate duration and age use output frames. Releasing
/// a gated voice freezes its ADS level at note-off and ramps to zero; source
/// playback (and a Classic loop) continues during release. Age queries may be
/// made in any order, enabling seek/block-independent original render fixtures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceCoefficients {
    pub(super) expected_frames: usize,
    pub(super) channels: SampleChannels,
    pub(super) region: Region,
    pub(super) loop_offsets: Option<(usize, usize)>,
    /// Effective seam crossfade in source frames (offset order). Zero is a
    /// hard seam; otherwise it never exceeds the loop length or its pre-roll.
    pub(super) loop_crossfade: usize,
    pub(super) reverse: bool,
    pub(super) step: u64,
    /// Band-limited reconstruction above unity rate; derived from `step`.
    pub(super) kernel: Option<super::interpolation::Kernel>,
    pub(super) channel_gain: [f64; 2],
    pub(super) gate: GateMode,
    pub(super) envelope: envelope::PreparedEnvelope,
}

impl VoiceCoefficients {
    /// Zero fixed buffering/lookahead; source resampling and envelope timing
    /// do not introduce a block of host latency.
    pub const fn algorithmic_latency_frames(&self) -> u32 {
        0
    }

    pub const fn release_frames(&self) -> u64 {
        match self.gate {
            GateMode::Gate => self.envelope.release_frames(),
            GateMode::Trigger => 0,
        }
    }

    /// Length in output frames for a nonlooping source, excluding envelope
    /// truncation. A loop is unbounded until its gate/release ends. Trigger
    /// voices must be retained to this natural end even after MIDI note-off.
    pub fn natural_frames(&self) -> Option<u64> {
        if self.loop_offsets.is_some() {
            return None;
        }
        let length = (self.region.end - self.region.start) as u128 * ONE;
        u64::try_from(length.div_ceil(u128::from(self.step))).ok()
    }

    /// Exclusive onset-relative lifetime bound for this voice and gate duration.
    /// Trigger ignores note-off; gated playback ends at the earlier of source
    /// exhaustion and envelope release. Loops require a representable gate plus
    /// release. This is not a silence detector or authority to retain resources;
    /// the host must separately check onset addition, cancellation and leases.
    pub fn end_age_frames(&self, gate_frames: u64) -> Result<u64, Invalid> {
        let natural = self.natural_frames();
        if self.gate == GateMode::Trigger {
            return natural.ok_or(Invalid("trigger sample needs a finite natural end"));
        }
        match (natural, gate_frames.checked_add(self.release_frames())) {
            (Some(source), Some(release)) => Ok(source.min(release)),
            (Some(source), None) => Ok(source),
            (None, Some(release)) => Ok(release),
            (None, None) => Err(Invalid("sample gate and release end age overflow")),
        }
    }

    #[inline]
    pub fn sample(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
    ) -> StereoFrame {
        self.sample_inner::<true>(pcm, age_frames, gate_frames)
    }

    #[inline]
    fn sample_inner<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
    ) -> StereoFrame {
        // u64 * u64 fits u128 exactly, even at u64::MAX age. Stateless
        // public queries retain their original age-relative source law.
        let position = u128::from(age_frames) * u128::from(self.step);
        self.sample_position_inner::<CHECK_FINITE>(pcm, age_frames, gate_frames, position)
    }

    /// Mutable gated voices carry actual source distance through tuning changes;
    /// envelope age and gate remain independent output-frame clocks.
    #[inline]
    pub(super) fn sample_at_position(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
        position: u128,
    ) -> StereoFrame {
        self.sample_position_inner::<true>(pcm, age_frames, gate_frames, position)
    }

    #[inline]
    fn sample_position_inner<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        age_frames: u64,
        gate_frames: u64,
        position: u128,
    ) -> StereoFrame {
        let channels = match pcm {
            DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
            DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
        };
        if pcm.frames() != self.expected_frames || channels != self.channels {
            return [0.0; 2];
        }
        let envelope = match self.gate {
            GateMode::Gate => self.envelope.sample(age_frames, gate_frames),
            GateMode::Trigger => self.envelope.held(age_frames),
        };
        if envelope == 0.0 {
            return [0.0; 2];
        }
        let Some(value) = self.source_value::<CHECK_FINITE>(pcm, position) else {
            return [0.0; 2];
        };
        let Some(value) = self.loop_crossfade_value::<CHECK_FINITE>(pcm, position, value) else {
            return [0.0; 2];
        };
        // Gain in f64 so finite f32 sources do not overflow. Saturation at
        // the representation boundary avoids NaN/Inf from otherwise valid
        // finite PCM and the bounded gain domain.
        std::array::from_fn(|channel| {
            let value = value[channel] * self.channel_gain[channel] * envelope;
            value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
        })
    }

    /// Reconstructed source at a Q32 playback position, before gain and
    /// envelope. `None` means silence (past a nonlooping end, a missing frame
    /// or, when checked, non-finite PCM).
    #[inline]
    pub(super) fn source_value<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        mut position: u128,
    ) -> Option<[f64; 2]> {
        let length = self.region.end - self.region.start;
        // Whether playback has wrapped at least once: the loop end then
        // precedes the loop start in playback history.
        let mut wrapped = false;
        // Integer modulo preserves loop phase without converting the carried
        // source cursor (or a stateless age-derived position) to f64.
        if let Some((start, end)) = self.loop_offsets {
            let end = end as u128 * ONE;
            let start = start as u128 * ONE;
            if position >= end {
                position = start + (position - end) % (end - start);
                // With a seam crossfade, playback reached the loop start
                // through the faded-in pre-roll, so band-limited taps before
                // the loop start read that pre-roll, not the loop end.
                wrapped = self.loop_crossfade == 0;
            }
        } else if position >= length as u128 * ONE {
            return None;
        }
        let index = (position >> FRACTION_BITS) as usize;
        let fraction = (position % ONE) as f64 / ONE as f64;
        let source_index = |offset: usize| {
            if self.reverse {
                self.region.end - 1 - offset
            } else {
                self.region.start + offset
            }
        };
        if let Some(kernel) = self.kernel {
            return self.band_limited::<CHECK_FINITE>(pcm, kernel, index, fraction, wrapped);
        }
        let mut next = index + 1;
        if let Some((start, end)) = self.loop_offsets {
            if next == end {
                next = start;
            }
        }
        next = next.min(length - 1);
        let (Some(a), Some(b)) = (
            pcm.frame(source_index(index)),
            pcm.frame(source_index(next)),
        ) else {
            return None;
        };
        if CHECK_FINITE && a.iter().chain(&b).any(|value| !value.is_finite()) {
            return None;
        }
        // Interpolate in f64 so finite f32 endpoints do not overflow the
        // subtraction.
        Some(std::array::from_fn(|channel| {
            f64::from(a[channel]) * (1.0 - fraction) + f64::from(b[channel]) * fraction
        }))
    }

    /// Classic loop seam crossfade (#4088). In the last `loop_crossfade`
    /// frames of each loop pass, the reconstructed tail blends linearly into
    /// the same-length pre-roll ending at the loop start, reconstructed by
    /// the same linear or band-limited law at its own (unwrapped) position, so
    /// the blend reaches the post-wrap source exactly at the seam. Preparation
    /// bounds the length by the pre-roll, so the earlier position never
    /// precedes the region. Off, or outside the fade, returns `direct` as is.
    #[inline]
    fn loop_crossfade_value<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        position: u128,
        direct: [f64; 2],
    ) -> Option<[f64; 2]> {
        let Some((start, end)) = self.loop_offsets else {
            return Some(direct);
        };
        if self.loop_crossfade == 0 {
            return Some(direct);
        }
        let (start, end) = (start as u128 * ONE, end as u128 * ONE);
        let phase = if position >= end {
            start + (position - end) % (end - start)
        } else {
            position
        };
        let fade_start = end - self.loop_crossfade as u128 * ONE;
        if phase < fade_start {
            return Some(direct);
        }
        let pre_roll = self.source_value::<CHECK_FINITE>(pcm, phase - (end - start))?;
        let weight = (phase - fade_start) as f64 / (self.loop_crossfade as u128 * ONE) as f64;
        Some(std::array::from_fn(|channel| {
            direct[channel] * (1.0 - weight) + pre_roll[channel] * weight
        }))
    }

    /// Pitched-up path; see `interpolation` for the law. Taps follow
    /// playback history: past a Classic loop end they continue at its start;
    /// before the loop start of a wrapped pass they come from the loop end
    /// (from the faded-in pre-roll when a seam crossfade is on).
    /// Other taps outside the region read silence.
    #[inline]
    fn band_limited<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        kernel: super::interpolation::Kernel,
        index: usize,
        fraction: f64,
        wrapped: bool,
    ) -> Option<[f64; 2]> {
        let taps = kernel.taps(fraction)?;
        if !taps.total.is_normal() {
            return Some([0.0; 2]);
        }
        let count = taps.lower.len();
        let blend = f64::from(taps.blend);
        let total = f64::from(taps.total);
        let length = self.region.end - self.region.start;
        let (loop_start, contiguous_end) = match self.loop_offsets {
            Some((start, end)) => (if wrapped { start } else { 0 }, end),
            None => (0, length),
        };
        let first = i64::try_from(index).ok()? + taps.first;
        // Fast path: every tap reads one contiguous run of the region in
        // playback order. Two f32 dot products (one per phase row) over
        // fixed-width chunks vectorize; a non-finite result (non-finite PCM
        // or f32 overflow) falls back to the checked f64 path below, which
        // keeps the original guarantees.
        if let Ok(low) = usize::try_from(first) {
            let high = low + count;
            if low >= loop_start && high <= contiguous_end {
                let (from, to) = if self.reverse {
                    (self.region.end - high, self.region.end - low)
                } else {
                    (self.region.start + low, self.region.start + high)
                };
                let sums = match pcm {
                    DeviceResourcePcm::Mono(samples) => {
                        let run = samples.get(from..to)?;
                        let [lower, upper] = if self.reverse {
                            dot_rows_reversed(run, taps.lower, taps.upper)
                        } else {
                            dot_rows(run, taps.lower, taps.upper)
                        };
                        [[lower; 2], [upper; 2]]
                    }
                    DeviceResourcePcm::Stereo(frames) => {
                        let run = frames.get(from..to)?;
                        if self.reverse {
                            dot_stereo_rows_reversed(run, taps.lower, taps.upper)
                        } else {
                            dot_stereo_rows(run, taps.lower, taps.upper)
                        }
                    }
                };
                let value: [f64; 2] = std::array::from_fn(|channel| {
                    let lower = f64::from(sums[0][channel]);
                    let upper = f64::from(sums[1][channel]);
                    (lower + (upper - lower) * blend) / total
                });
                if value.iter().all(|value| value.is_finite()) {
                    return Some(value);
                }
            }
        }
        let mut sum = [0.0_f64; 2];
        for (tap, (lower, upper)) in taps.lower.iter().zip(taps.upper).enumerate() {
            let weight = f64::from(*lower) + (f64::from(*upper) - f64::from(*lower)) * blend;
            let offset = first + tap as i64;
            let offset = match self.loop_offsets {
                Some((start, end)) => {
                    let (start, end) = (start as i64, end as i64);
                    let span = end - start;
                    if offset >= end {
                        start + (offset - end) % span
                    } else if wrapped && offset < start {
                        end - 1 - (start - 1 - offset) % span
                    } else {
                        offset
                    }
                }
                None => offset,
            };
            let Some(offset) = usize::try_from(offset)
                .ok()
                .filter(|offset| *offset < length)
            else {
                continue;
            };
            let source = if self.reverse {
                self.region.end - 1 - offset
            } else {
                self.region.start + offset
            };
            let Some(value) = pcm.frame(source) else {
                continue;
            };
            if CHECK_FINITE && value.iter().any(|sample| !sample.is_finite()) {
                return None;
            }
            sum[0] += f64::from(value[0]) * weight;
            sum[1] += f64::from(value[1]) * weight;
        }
        Some([sum[0] / total, sum[1] / total])
    }

    /// Fill caller-owned output. Overflowed ages are defined silence; they do
    /// not wrap to frame zero. No cursor, heap, ownership or block-size state.
    pub fn render(
        &self,
        pcm: DeviceResourcePcm<'_>,
        start_age: u64,
        gate_frames: u64,
        output: &mut [StereoFrame],
    ) {
        self.render_inner::<true>(pcm, start_age, gate_frames, output);
    }

    fn render_inner<const CHECK_FINITE: bool>(
        &self,
        pcm: DeviceResourcePcm<'_>,
        start_age: u64,
        gate_frames: u64,
        output: &mut [StereoFrame],
    ) {
        for (offset, frame) in output.iter_mut().enumerate() {
            *frame = u64::try_from(offset)
                .ok()
                .and_then(|offset| start_age.checked_add(offset))
                .map_or([0.0; 2], |age| {
                    self.sample_inner::<CHECK_FINITE>(pcm, age, gate_frames)
                });
        }
    }
}

/// Original stereo borrowed-voice API. It delegates to the same scalar plan;
/// existing PCM, envelope, pitch and checked lifetime laws remain unchanged.
#[derive(Clone, Copy, Debug)]
pub struct Voice<'a> {
    pub(super) frames: &'a [StereoFrame],
    pub(super) coefficients: VoiceCoefficients,
}
impl Voice<'_> {
    pub fn coefficients(&self) -> VoiceCoefficients {
        self.coefficients
    }
    pub const fn algorithmic_latency_frames(&self) -> u32 {
        self.coefficients.algorithmic_latency_frames()
    }
    pub const fn release_frames(&self) -> u64 {
        self.coefficients.release_frames()
    }
    pub fn natural_frames(&self) -> Option<u64> {
        self.coefficients.natural_frames()
    }
    pub fn end_age_frames(&self, gate_frames: u64) -> Result<u64, Invalid> {
        self.coefficients.end_age_frames(gate_frames)
    }
    #[inline]
    pub fn sample(&self, age_frames: u64, gate_frames: u64) -> StereoFrame {
        // Sample::new already validated every immutable frame on control.
        // Keep that original path free of redundant per-frame finite checks;
        // public raw-PCM queries still use the checked specialization above.
        self.coefficients.sample_inner::<false>(
            DeviceResourcePcm::Stereo(self.frames),
            age_frames,
            gate_frames,
        )
    }
    pub fn render(&self, start_age: u64, gate_frames: u64, output: &mut [StereoFrame]) {
        self.coefficients.render_inner::<false>(
            DeviceResourcePcm::Stereo(self.frames),
            start_age,
            gate_frames,
            output,
        )
    }
}

/// Width of the independent accumulator lanes in the dot products below.
const LANES: usize = 8;

/// Dot products of `samples` with two weight rows, in eight independent
/// lanes so the compiler can vectorize the reductions.
#[inline]
fn dot_rows(samples: &[f32], lower: &[f32], upper: &[f32]) -> [f32; 2] {
    let mut low = [0.0_f32; LANES];
    let mut high = [0.0_f32; LANES];
    let chunks = samples.len() / LANES;
    for chunk in 0..chunks {
        let range = chunk * LANES..(chunk + 1) * LANES;
        let (Some(x), Some(a), Some(b)) = (
            samples.get(range.clone()),
            lower.get(range.clone()),
            upper.get(range),
        ) else {
            break;
        };
        for lane in 0..LANES {
            low[lane] += x[lane] * a[lane];
            high[lane] += x[lane] * b[lane];
        }
    }
    let mut result = [low.iter().sum::<f32>(), high.iter().sum::<f32>()];
    for index in chunks * LANES..samples.len() {
        if let (Some(x), Some(a), Some(b)) =
            (samples.get(index), lower.get(index), upper.get(index))
        {
            result[0] += x * a;
            result[1] += x * b;
        }
    }
    result
}

#[inline]
fn dot_rows_reversed(samples: &[f32], lower: &[f32], upper: &[f32]) -> [f32; 2] {
    let mut result = [0.0_f32; 2];
    for ((x, a), b) in samples.iter().rev().zip(lower).zip(upper) {
        result[0] += x * a;
        result[1] += x * b;
    }
    result
}

/// Per row, per channel sums: `[[lower_left, lower_right], [upper_left, upper_right]]`.
#[inline]
fn dot_stereo_rows(frames: &[[f32; 2]], lower: &[f32], upper: &[f32]) -> [[f32; 2]; 2] {
    let mut lanes = [[[0.0_f32; 2]; 2]; LANES];
    let chunks = frames.len() / LANES;
    for chunk in 0..chunks {
        let range = chunk * LANES..(chunk + 1) * LANES;
        let (Some(x), Some(a), Some(b)) = (
            frames.get(range.clone()),
            lower.get(range.clone()),
            upper.get(range),
        ) else {
            break;
        };
        for lane in 0..LANES {
            lanes[lane][0][0] += x[lane][0] * a[lane];
            lanes[lane][0][1] += x[lane][1] * a[lane];
            lanes[lane][1][0] += x[lane][0] * b[lane];
            lanes[lane][1][1] += x[lane][1] * b[lane];
        }
    }
    let mut result = [[0.0_f32; 2]; 2];
    for lane in lanes {
        for row in 0..2 {
            result[row][0] += lane[row][0];
            result[row][1] += lane[row][1];
        }
    }
    for index in chunks * LANES..frames.len() {
        if let (Some(x), Some(a), Some(b)) = (frames.get(index), lower.get(index), upper.get(index))
        {
            result[0][0] += x[0] * a;
            result[0][1] += x[1] * a;
            result[1][0] += x[0] * b;
            result[1][1] += x[1] * b;
        }
    }
    result
}

#[inline]
fn dot_stereo_rows_reversed(frames: &[[f32; 2]], lower: &[f32], upper: &[f32]) -> [[f32; 2]; 2] {
    let mut result = [[0.0_f32; 2]; 2];
    for ((x, a), b) in frames.iter().rev().zip(lower).zip(upper) {
        result[0][0] += x[0] * a;
        result[0][1] += x[1] * a;
        result[1][0] += x[0] * b;
        result[1][1] += x[1] * b;
    }
    result
}

#[cfg(test)]
mod tests;
