//! # Purpose
//! Own pure control-side stereo sample preparation and warp kernels.
//!
//! # Dependencies
//! Uses only the standard library; no project, UI or device dependency.
//!
//! # Does not own
//! Does not own files, threads, canonical edits or realtime callback processing. Callers validate
//! resource/work bounds and retain original source identity.
mod beats;
pub mod fft;
mod granular;
pub use granular::{GrainAnalysis, GrainDecision, GrainSynthesis, analyze_grain};
mod granular_overlap;
mod granular_sequential;
pub use granular_overlap::GranularOverlap;
pub use granular_sequential::{
    GranularCheckpoint, GranularSource, SequentialGranular, SequentialGranularConfig,
};
mod resample;
pub use resample::{PitchWindow, RepitchWindow, Resampler, V2_MAX_TAPS};
mod source_region;
mod spectral;
pub use source_region::SourceRegion;
use std::fmt;

pub type Frame = [f32; 2];
pub const MAX_OUTPUT_FRAMES: usize = 16_777_216;
/// Maximum metadata extent of a streamed map; whole-buffer rendering remains capped separately.
pub const MAX_STREAMED_OUTPUT_FRAMES: u64 = 1_u64 << 36;
pub const MAX_SCRATCH_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_MAP_SEGMENTS: usize = 8_192;
pub const MAX_POINTS: usize = 1_024;
pub const MAX_WORK_UNITS: u64 = 1_000_000_000;
pub const ALGORITHM_VERSION: u32 = 11;
const Q32: i128 = 1_i128 << 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Allocation,
    Cancelled,
    WorkLimit,
    ScratchLimit,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::Allocation => f.write_str("audio preparation allocation failed"),
            Self::Cancelled => f.write_str("audio preparation cancelled"),
            Self::WorkLimit => f.write_str("audio preparation exceeds its bounded analysis work"),
            Self::ScratchLimit => f.write_str("audio preparation exceeds its 32 mib scratch limit"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GainPoint {
    pub source_frame: u64,
    pub gain_q16: u32,
}

/// Incremental, bounded stereo reference selection used by granular warp.
///
/// Request each region-relative offset in order, resolve phase, looping and any
/// source gain in the caller, then submit the resulting stereo sample. The scan
/// owns no source, performs no IO, and visits at most 4096 samples regardless of
/// region length. Floating-point accumulation and left-on-tie selection match
/// the resident renderer; samples are not sanitized or reordered here.
#[derive(Debug)]
pub struct ReferenceChannelScan {
    count: u64,
    stride: u64,
    next: u64,
    energy: [f64; 2],
}

impl ReferenceChannelScan {
    /// Start a scan over a nonempty logical source region.
    pub fn new(region_length: u64) -> Result<Self, Error> {
        if region_length == 0 {
            return Err(Error::Invalid("reference scan needs a nonempty region"));
        }
        let count = region_length.min(4096);
        Ok(Self {
            count,
            stride: (region_length / count).max(1),
            next: 0,
            energy: [0.0; 2],
        })
    }

    /// The next region-relative source offset, or `None` after all samples.
    pub fn next_offset(&self) -> Option<u64> {
        // next < count and stride = floor(length / count), so this product is
        // strictly less than the original nonzero region length.
        (self.next < self.count).then(|| self.next * self.stride)
    }

    /// Submit exactly the requested offset; rejected submissions do not advance.
    pub fn observe(&mut self, offset: u64, frame: [f64; 2]) -> Result<(), Error> {
        if self.next_offset() != Some(offset) {
            return Err(Error::Invalid("reference scan sample is out of order"));
        }
        for (energy, sample) in self.energy.iter_mut().zip(frame) {
            *energy += sample * sample;
        }
        self.next += 1;
        Ok(())
    }

    /// Return the selected channel only after all requested samples arrive.
    pub fn finish(&self) -> Result<usize, Error> {
        if self.next != self.count {
            return Err(Error::Invalid("reference scan is incomplete"));
        }
        Ok(usize::from(self.energy[1] > self.energy[0]))
    }
}
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub frames: &'a [Frame],
    pub sample_rate: u32,
    pub region_start: u64,
    pub region_length: u64,
    pub phase: u64,
    pub looping: bool,
    pub envelope: &'a [GainPoint],
}
impl Input<'_> {
    fn validate(self) -> Result<(), Error> {
        if !(8_000..=192_000).contains(&self.sample_rate)
            || self.region_length == 0
            || self
                .region_start
                .checked_add(self.region_length)
                .is_none_or(|end| end > self.frames.len() as u64)
            || (self.looping && self.phase >= self.region_length)
            || (!self.looping && self.phase != 0)
        {
            return Err(Error::Invalid(
                "invalid audio preparation source interval, phase or rate",
            ));
        }
        if self.envelope.len() > MAX_POINTS
            || self
                .envelope
                .iter()
                .any(|p| p.source_frame > self.frames.len() as u64 || p.gain_q16 > 4 * 65_536)
            || self
                .envelope
                .windows(2)
                .any(|p| p[0].source_frame >= p[1].source_frame)
        {
            return Err(Error::Invalid("invalid source gain envelope"));
        }
        Ok(())
    }
    fn frame(self, relative: i128) -> [f64; 2] {
        let position = relative + i128::from(self.phase);
        let local = if self.looping {
            position.rem_euclid(i128::from(self.region_length))
        } else if position < 0 || position >= i128::from(self.region_length) {
            return [0.0; 2];
        } else {
            position
        };
        let index = self.region_start + local as u64;
        let Some(frame) = self.frames.get(index as usize) else {
            return [0.0; 2];
        };
        let gain = self.gain(index);
        [f64::from(frame[0]) * gain, f64::from(frame[1]) * gain]
    }
    fn gain(self, frame: u64) -> f64 {
        source_gain(self.envelope, frame)
    }
    fn reference_channel(self) -> Result<usize, Error> {
        let mut scan = ReferenceChannelScan::new(self.region_length)?;
        while let Some(offset) = scan.next_offset() {
            scan.observe(offset, self.frame(i128::from(offset)))?;
        }
        scan.finish()
    }
}

fn source_gain(envelope: &[GainPoint], frame: u64) -> f64 {
    if envelope.is_empty() {
        return 1.0;
    }
    let next = envelope.partition_point(|point| point.source_frame <= frame);
    let value = match (
        next.checked_sub(1).and_then(|i| envelope.get(i)),
        envelope.get(next),
    ) {
        (Some(a), Some(b)) => {
            let span = b.source_frame - a.source_frame;
            let offset = frame - a.source_frame;
            (u128::from(a.gain_q16) * u128::from(span - offset)
                + u128::from(b.gain_q16) * u128::from(offset)) as f64
                / span as f64
        }
        (Some(a), None) => f64::from(a.gain_q16),
        (None, Some(b)) => f64::from(b.gain_q16),
        (None, None) => 65_536.0,
    };
    value / 65_536.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapSegment {
    pub output_start: u64,
    pub output_end: u64,
    /// Unwrapped region-relative source frames in signed Q64.32; phase is separate.
    pub source_start_q32: i128,
    pub source_end_q32: i128,
}
/// Validate one continuous output/source duration ratio before integer-frame
/// quantization, using the same pitch and algorithm limits as prepared maps.
/// Callers retain ownership of source validity, extents, resources and rendering.
pub fn validate_stretch_ratio(ratio: f64, mode: Mode, pitch_cents: i16) -> Result<(), Error> {
    if !(-4_800..=4_800).contains(&pitch_cents) || !ratio.is_finite() || ratio <= 0.0 {
        return Err(Error::Invalid("invalid audio transform extent or pitch"));
    }
    let pitch = 2.0_f64.powf(f64::from(pitch_cents) / 1_200.0);
    let compensated = if matches!(mode, Mode::RePitch) {
        ratio
    } else {
        ratio * pitch
    };
    if !matches!(mode, Mode::Resample) && !(0.125 - 1e-9..=8.0 + 1e-9).contains(&compensated) {
        return Err(Error::Invalid(
            "warp stretch including pitch compensation must be within 1/8..8",
        ));
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeMap {
    segments: Vec<MapSegment>,
}
impl TimeMap {
    pub fn new(segments: Vec<MapSegment>, output_frames: usize) -> Result<Self, Error> {
        if output_frames > MAX_OUTPUT_FRAMES {
            return Err(Error::Invalid(
                "audio map needs bounded explicit output endpoints",
            ));
        }
        Self::validate(segments, output_frames as u64)
    }
    /// Construct metadata for bounded streamed rendering without allocating output PCM.
    /// Whole-buffer rendering still enforces [`MAX_OUTPUT_FRAMES`].
    pub fn new_streamed(segments: Vec<MapSegment>, output_frames: u64) -> Result<Self, Error> {
        if output_frames > MAX_STREAMED_OUTPUT_FRAMES || segments.capacity() > MAX_MAP_SEGMENTS {
            return Err(Error::Invalid(
                "audio map needs bounded explicit output endpoints",
            ));
        }
        Self::validate(segments, output_frames)
    }
    fn validate(segments: Vec<MapSegment>, output_frames: u64) -> Result<Self, Error> {
        if output_frames == 0
            || segments.is_empty()
            || segments.len() > MAX_MAP_SEGMENTS
            || segments.first().is_none_or(|s| s.output_start != 0)
            || segments
                .last()
                .is_none_or(|s| s.output_end != output_frames)
        {
            return Err(Error::Invalid(
                "audio map needs bounded explicit output endpoints",
            ));
        }
        for segment in &segments {
            if segment.output_start >= segment.output_end
                || segment.source_start_q32 >= segment.source_end_q32
            {
                return Err(Error::Invalid(
                    "audio map must increase in source and output time",
                ));
            }
            if segment.source_start_q32.unsigned_abs() > (1_u128 << 40) << 32
                || segment.source_end_q32.unsigned_abs() > (1_u128 << 40) << 32
            {
                return Err(Error::Invalid(
                    "audio map source position exceeds its fixed-point range",
                ));
            }
        }
        if segments.windows(2).any(|pair| {
            pair[0].output_end != pair[1].output_start
                || pair[0].source_end_q32 != pair[1].source_start_q32
        }) {
            return Err(Error::Invalid("audio map segments must be contiguous"));
        }
        Ok(Self { segments })
    }
    /// Validate the renderer's exact per-segment stretch and pitch capability.
    /// Metadata only; the existing map constructor owns extent/segment validity.
    /// This does not admit PCM, algorithm settings, scratch or render work.
    pub fn validate_stretch(&self, mode: Mode, pitch_cents: i16) -> Result<(), Error> {
        for segment in self.segments() {
            let ratio = (segment.output_end - segment.output_start) as f64 * Q32 as f64
                / (segment.source_end_q32 - segment.source_start_q32) as f64;
            validate_stretch_ratio(ratio, mode, pitch_cents)?;
        }
        Ok(())
    }
    /// Retained allocation for worker memory accounting.
    pub fn backing_bytes(&self) -> Option<usize> {
        self.segments
            .capacity()
            .checked_mul(std::mem::size_of::<MapSegment>())
    }
    pub fn segments(&self) -> &[MapSegment] {
        &self.segments
    }
    pub fn position_q32(&self, frame: i128) -> Result<i128, Error> {
        let index = self
            .segments
            .partition_point(|s| i128::from(s.output_end) <= frame)
            .min(self.segments.len() - 1);
        let segment = &self.segments[index];
        let delta = frame
            .checked_sub(i128::from(segment.output_start))
            .ok_or(Error::Invalid("audio map position overflow"))?;
        let scaled = delta
            .checked_mul(segment.source_end_q32 - segment.source_start_q32)
            .ok_or(Error::Invalid("audio map position overflow"))?
            / i128::from(segment.output_end - segment.output_start);
        segment
            .source_start_q32
            .checked_add(scaled)
            .ok_or(Error::Invalid("audio map position overflow"))
    }
    fn position(&self, frame: i128) -> Result<f64, Error> {
        Ok(self.position_q32(frame)? as f64 / Q32 as f64)
    }
    fn rate_at(&self, frame: u64) -> f64 {
        let index = self
            .segments
            .partition_point(|s| s.output_end <= frame)
            .min(self.segments.len() - 1);
        let s = &self.segments[index];
        (s.source_end_q32 - s.source_start_q32) as f64
            / Q32 as f64
            / (s.output_end - s.output_start) as f64
    }
    pub fn output_for_source(&self, source_q32: i128) -> Option<u64> {
        let index = self
            .segments
            .partition_point(|s| s.source_end_q32 < source_q32);
        let s = self.segments.get(index)?;
        if source_q32 < s.source_start_q32 {
            return None;
        }
        let numerator =
            (source_q32 - s.source_start_q32) * i128::from(s.output_end - s.output_start);
        u64::try_from(
            i128::from(s.output_start) + numerator / (s.source_end_q32 - s.source_start_q32),
        )
        .ok()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    Off,
    Forward,
    Alternating,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Unwarped pitch resampling: `map` advances source at unity before transpose.
    Resample,
    RePitch,
    Beats {
        division_frames: Option<u64>,
        tail: Tail,
        decay: u8,
    },
    Tones {
        grain_ms: u16,
    },
    Texture {
        grain_ms: u16,
        fluctuation: u8,
        seed: u64,
    },
    Complex,
    ComplexPro {
        formants: u8,
        envelope: u16,
    },
}
pub struct Request<'a> {
    pub attacks: &'a [u64],
    pub input: Input<'a>,
    pub map: &'a TimeMap,
    pub output_frames: usize,
    pub pitch_cents: i16,
    pub mode: Mode,
}
pub struct Rendered {
    pub frames: Vec<Frame>,
    pub work_units: u64,
    pub peak_scratch_bytes: usize,
}
struct Work<'a> {
    cancel: &'a mut dyn FnMut() -> bool,
    used: u64,
    limit: u64,
    scratch: usize,
    base_scratch: usize,
    resampler: Resampler,
}
impl Work<'_> {
    fn check(&mut self, amount: u64) -> Result<(), Error> {
        self.used = self.used.checked_add(amount).ok_or(Error::WorkLimit)?;
        if self.used > self.limit {
            return Err(Error::WorkLimit);
        }
        if (self.cancel)() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }
    fn scratch(&mut self, bytes: usize) -> Result<(), Error> {
        let bytes = bytes
            .checked_add(self.base_scratch)
            .ok_or(Error::ScratchLimit)?;
        if bytes > MAX_SCRATCH_BYTES {
            return Err(Error::ScratchLimit);
        }
        self.scratch = self.scratch.max(bytes);
        Ok(())
    }
}
fn buffer<T: Clone>(length: usize, value: T) -> Result<Vec<T>, Error> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| Error::Allocation)?;
    result.resize(length, value);
    Ok(result)
}
fn finite(frame: [f64; 2]) -> Result<Frame, Error> {
    let frame = frame.map(|s| s as f32);
    if frame.iter().any(|v| !v.is_finite()) {
        return Err(Error::Invalid(
            "audio transform produced a nonfinite sample",
        ));
    }
    Ok(frame)
}
/// Render with the frozen v1 reconstruction law.
pub fn render(request: &Request<'_>, cancel: &mut dyn FnMut() -> bool) -> Result<Rendered, Error> {
    render_with_resampler(request, Resampler::V1, cancel)
}

/// Render with an explicit reconstruction law. Every mode's fractional reads
/// use it; analysis and grain/beat/spectral decisions are unchanged.
pub fn render_with_resampler(
    request: &Request<'_>,
    resampler: Resampler,
    cancel: &mut dyn FnMut() -> bool,
) -> Result<Rendered, Error> {
    request.input.validate()?;
    if request.attacks.len() > MAX_POINTS
        || (!request.attacks.is_empty() && request.attacks.first() != Some(&0))
        || request
            .attacks
            .iter()
            .any(|&frame| frame >= request.input.region_length)
        || request.attacks.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(Error::Invalid(
            "explicit beat attacks must be sorted within the source region and begin at zero",
        ));
    }

    match request.mode {
        Mode::Beats {
            division_frames,
            decay,
            ..
        } if decay > 100 || division_frames == Some(0) => {
            return Err(Error::Invalid("invalid transient division or decay"));
        }
        Mode::Tones { grain_ms } | Mode::Texture { grain_ms, .. }
            if !(8..=200).contains(&grain_ms) =>
        {
            return Err(Error::Invalid("grain size must be within 8..200 ms"));
        }
        Mode::Texture { fluctuation, .. } if fluctuation > 100 => {
            return Err(Error::Invalid("grain fluctuation must be within 0..100"));
        }
        Mode::ComplexPro { formants, envelope }
            if formants > 100 || !(32..=512).contains(&envelope) =>
        {
            return Err(Error::Invalid("invalid formant or envelope setting"));
        }
        _ => {}
    }
    if request
        .map
        .segments
        .last()
        .is_none_or(|segment| segment.output_end != request.output_frames as u64)
    {
        return Err(Error::Invalid(
            "audio map extent differs from requested output",
        ));
    }

    if request.output_frames == 0
        || request.output_frames > MAX_OUTPUT_FRAMES
        || !(-4_800..=4_800).contains(&request.pitch_cents)
    {
        return Err(Error::Invalid("invalid audio transform extent or pitch"));
    }
    request
        .map
        .validate_stretch(request.mode, request.pitch_cents)?;
    let pitch = 2.0_f64.powf(f64::from(request.pitch_cents) / 1_200.0);
    let base_scratch = request.map.segments.len() * std::mem::size_of::<MapSegment>()
        + std::mem::size_of_val(request.input.envelope)
        + std::mem::size_of_val(request.attacks);
    let mut work = Work {
        cancel,
        used: 0,
        limit: if matches!(request.mode, Mode::Beats { .. }) {
            beats::MAX_RENDER_WORK_UNITS
        } else {
            MAX_WORK_UNITS
        },
        scratch: base_scratch,
        base_scratch,
        resampler,
    };
    work.check(0)?;
    let frames = match request.mode {
        Mode::Resample | Mode::RePitch => resample::render(request, pitch, &mut work)?,
        Mode::Beats { .. } => beats::render(request, pitch, &mut work)?,
        Mode::Tones { .. } | Mode::Texture { .. } => granular::grains(request, pitch, &mut work)?,
        Mode::Complex | Mode::ComplexPro { .. } => spectral::render(request, pitch, &mut work)?,
    };
    Ok(Rendered {
        frames,
        work_units: work.used,
        peak_scratch_bytes: work.scratch,
    })
}

/// Ring-only WOLA normalization: output-sized weight/mapping arrays are forbidden.
struct Accumulator {
    output: Vec<Frame>,
    samples: Vec<[f64; 2]>,
    weights: Vec<f64>,
    cursor: usize,
}
impl Accumulator {
    // Read-only already-finalized context for a continuation's overlap search.
    // It borrows the same output storage; it neither rewrites past audio nor
    // allocates another history/PCM buffer.
    fn reference(&self, frame: i128) -> Option<[f64; 2]> {
        let frame = usize::try_from(frame).ok()?;
        if frame < self.cursor {
            self.output.get(frame).map(|x| x.map(f64::from))
        } else {
            self.preview(frame as i128)
        }
    }
    fn previous(&self) -> [f64; 2] {
        self.cursor
            .checked_sub(1)
            .and_then(|i| self.output.get(i))
            .map_or([0.0; 2], |x| x.map(f64::from))
    }
    fn preview(&self, frame: i128) -> Option<[f64; 2]> {
        let frame = usize::try_from(frame).ok()?;
        if frame < self.cursor
            || frame >= self.output.len()
            || frame - self.cursor >= self.samples.len()
        {
            return None;
        }
        let i = frame % self.samples.len();
        (self.weights[i] > 1e-12).then(|| self.samples[i].map(|x| x / self.weights[i]))
    }
    fn add_correlated(
        &mut self,
        frame: i128,
        sample: [f64; 2],
        weight: f64,
        correlation: f64,
        previous_mean: [f64; 2],
        next_mean: [f64; 2],
    ) -> Result<(), Error> {
        let Ok(at) = usize::try_from(frame) else {
            return Ok(());
        };
        if at >= self.output.len() {
            return Ok(());
        }
        if at < self.cursor || at - self.cursor >= self.samples.len() {
            return Err(Error::Invalid(
                "audio overlap exceeds bounded normalization ring",
            ));
        }
        let i = at % self.samples.len();
        let a = self.weights[i];
        let b = weight;
        let gain = if a > 1e-12 && b > 1e-12 {
            ((a + b) / (a * a + b * b + 2.0 * correlation.clamp(0.0, 1.0) * a * b).sqrt())
                .clamp(1.0, std::f64::consts::SQRT_2)
        } else {
            1.0
        };
        for c in 0..2 {
            let sum = self.samples[i][c] + sample[c] * b;
            let mean = previous_mean[c] * a + next_mean[c] * b;
            self.samples[i][c] = if gain == 1.0 {
                sum
            } else {
                (sum - mean) * gain + mean
            };
        }
        self.weights[i] += b;
        Ok(())
    }
    fn new(frames: usize, ring: usize, work: &mut Work<'_>, extra: usize) -> Result<Self, Error> {
        let bytes = ring
            .checked_mul(24)
            .and_then(|n| n.checked_add(extra))
            .ok_or(Error::ScratchLimit)?;
        work.scratch(bytes)?;
        Ok(Self {
            output: buffer(frames, [0.0; 2])?,
            samples: buffer(ring, [0.0; 2])?,
            weights: buffer(ring, 0.0)?,
            cursor: 0,
        })
    }
    fn add(&mut self, frame: i128, sample: [f64; 2], weight: f64) -> Result<(), Error> {
        let Ok(frame) = usize::try_from(frame) else {
            return Ok(());
        };
        if frame >= self.output.len() {
            return Ok(());
        }
        if frame < self.cursor || frame - self.cursor >= self.samples.len() {
            return Err(Error::Invalid(
                "audio overlap exceeds bounded normalization ring",
            ));
        }
        let index = frame % self.samples.len();
        self.samples[index][0] += sample[0] * weight;
        self.samples[index][1] += sample[1] * weight;
        self.weights[index] += weight;
        Ok(())
    }
    fn finish_until(&mut self, end: usize) -> Result<(), Error> {
        while self.cursor < end.min(self.output.len()) {
            let index = self.cursor % self.samples.len();
            let gain = if self.weights[index] > 1e-12 {
                1.0 / self.weights[index]
            } else {
                0.0
            };
            self.output[self.cursor] = finite(self.samples[index].map(|v| v * gain))?;
            self.samples[index] = [0.0; 2];
            self.weights[index] = 0.0;
            self.cursor += 1;
        }
        Ok(())
    }
    fn finish(mut self) -> Result<Vec<Frame>, Error> {
        self.finish_until(self.output.len())?;
        Ok(self.output)
    }
}
