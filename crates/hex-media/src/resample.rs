//! Bounded, deterministic sample-rate conversion for control-side work.
use crate::{
    ImportError, ResampleKernel, ResampleVersion,
    resample_kernel::{PHASE_COUNT, V2_PHASE_COUNT, validate_rates},
};
use hex_engine::Audio;
use hex_project::converted_frame_count;
use std::sync::atomic::AtomicBool;

/// Converted frames rendered between cooperative cancellation checks.
const CANCEL_CHECK_OUTPUT_FRAMES: usize = 64 * 1024;

/// Caller-owned bounds for one resampling operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResampleLimits {
    pub max_source_frames: usize,
    pub max_output_frames: usize,
}

/// Convert validated stereo PCM into the target rate using the v1 windowed-sinc kernel.
/// The input is only read and shared unchanged for identity-rate conversions.
pub fn resample(
    source: &Audio,
    target_rate: u32,
    limits: ResampleLimits,
) -> Result<Audio, ImportError> {
    resample_with_version(source, target_rate, limits, ResampleVersion::V1)
}

/// Convert validated stereo PCM into the target rate with an explicit
/// conversion law. Identity-rate conversions share the input unchanged.
pub fn resample_with_version(
    source: &Audio,
    target_rate: u32,
    limits: ResampleLimits,
    version: ResampleVersion,
) -> Result<Audio, ImportError> {
    resample_cancellable(source, target_rate, limits, version, &crate::NOT_CANCELLED)
}

/// [`resample_with_version`] with cooperative cancellation for superseded
/// control work. `cancel` is polled before allocation and before each window
/// of 65,536 converted frames; once observed, conversion stops and reports
/// [`ImportError::Cancelled`]. Windows share one global phase, so the output
/// is identical to an uninterrupted conversion.
pub fn resample_cancellable(
    source: &Audio,
    target_rate: u32,
    limits: ResampleLimits,
    version: ResampleVersion,
    cancel: &AtomicBool,
) -> Result<Audio, ImportError> {
    crate::cancelled(cancel)?;
    let source_frames = u64::try_from(source.frames().len())
        .map_err(|_| ImportError::Invalid("source frame count exceeds project range"))?;
    let max_source_frames = u64::try_from(limits.max_source_frames)
        .map_err(|_| ImportError::Invalid("source frame limit exceeds project range"))?;
    let max_output_frames = u64::try_from(limits.max_output_frames)
        .map_err(|_| ImportError::Invalid("output frame limit exceeds project range"))?;
    let plan = ResamplePlan::with_kernel(
        ResampleKernel::with_version(source.sample_rate(), target_rate, version)?,
        source_frames,
        max_source_frames,
        max_output_frames,
        usize::MAX,
    )?;
    let output_capacity = usize::try_from(plan.output_frames())
        .map_err(|_| ImportError::Invalid("converted frame count exceeds addressable range"))?;
    if source.sample_rate() == target_rate {
        return Ok(source.clone());
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_capacity)
        .map_err(|_| ImportError::AllocationFailed)?;
    output.resize(output_capacity, [0.0; 2]);
    let mut window_start = 0_u64;
    for window in output.chunks_mut(CANCEL_CHECK_OUTPUT_FRAMES) {
        crate::cancelled(cancel)?;
        plan.render_window(0, source.frames(), window_start, window)?;
        let count = u64::try_from(window.len())
            .map_err(|_| ImportError::Invalid("output window frame count overflow"))?;
        window_start = window_start
            .checked_add(count)
            .ok_or(ImportError::Invalid("output window range overflow"))?;
    }
    Audio::new(target_rate, output)
        .map_err(|_| ImportError::Invalid("resampler produced invalid audio"))
}

/// Immutable global-phase conversion configuration for bounded output windows.
pub struct ResamplePlan {
    kernel: ResampleKernel,
    source_frames: u64,
    output_frames: u64,
    max_output_window_frames: usize,
}

#[derive(Clone, Copy)]
struct ReaderMetadata {
    sample_rate: u32,
    source_frames: u64,
    max_window_frames: usize,
}

impl ResamplePlan {
    /// Control-only identity check for the actual captured kernel owner. No
    /// owner/Arc is returned and no source value or content infers identity.
    pub fn uses_kernel_allocation(&self, other: &ResampleKernel) -> bool {
        let Self {
            kernel,
            source_frames: _,
            output_frames: _,
            max_output_window_frames: _,
        } = self;
        kernel.shares_allocation_with(other)
    }

    /// Source halo on each side of an output center for this plan's kernel.
    pub fn kernel_support_radius(&self) -> usize {
        self.kernel.support_radius()
    }

    pub fn new(
        source_rate: u32,
        target_rate: u32,
        source_frames: u64,
        max_source_frames: u64,
        max_output_frames: u64,
        max_output_window_frames: usize,
    ) -> Result<Self, ImportError> {
        let output_frames = validate_plan_dimensions(
            source_rate,
            target_rate,
            source_frames,
            max_source_frames,
            max_output_frames,
        )?;
        let kernel = ResampleKernel::new(source_rate, target_rate)?;
        Ok(Self {
            kernel,
            source_frames,
            output_frames,
            max_output_window_frames,
        })
    }

    /// Creates a per-source plan that shares a caller-prepared rate-pair kernel.
    pub fn with_kernel(
        kernel: ResampleKernel,
        source_frames: u64,
        max_source_frames: u64,
        max_output_frames: u64,
        max_output_window_frames: usize,
    ) -> Result<Self, ImportError> {
        let output_frames = validate_plan_dimensions(
            kernel.source_rate(),
            kernel.target_rate(),
            source_frames,
            max_source_frames,
            max_output_frames,
        )?;
        Ok(Self {
            kernel,
            source_frames,
            output_frames,
            max_output_window_frames,
        })
    }

    pub fn source_rate(&self) -> u32 {
        self.kernel.source_rate()
    }

    pub fn target_rate(&self) -> u32 {
        self.kernel.target_rate()
    }

    pub const fn source_frames(&self) -> u64 {
        self.source_frames
    }

    pub const fn output_frames(&self) -> u64 {
        self.output_frames
    }

    pub const fn max_output_window_frames(&self) -> usize {
        self.max_output_window_frames
    }

    /// Returns the whole-source half-open span required for a global output window.
    pub fn required_source_range(
        &self,
        global_output_start: u64,
        output_count: usize,
    ) -> Result<(u64, u64), ImportError> {
        self.check_output_range(global_output_start, output_count)?;
        if output_count == 0 {
            return Ok((0, 0));
        }
        let last_output = global_output_start
            .checked_add(
                u64::try_from(output_count - 1)
                    .map_err(|_| ImportError::Invalid("output window frame count overflow"))?,
            )
            .ok_or(ImportError::Invalid("output window range overflow"))?;
        let (first_center, _, _) = self.position(global_output_start)?;
        let (last_center, _, _) = self.position(last_output)?;
        let radius = u64::try_from(self.kernel.inner.radius)
            .map_err(|_| ImportError::Invalid("resampling support exceeds project range"))?;
        let start = first_center.saturating_sub(radius);
        let end = last_center
            .saturating_add(radius)
            .saturating_add(1)
            .min(self.source_frames);
        Ok((start.min(self.source_frames), end))
    }

    /// Converts one global output window from a whole-source indexed input slice.
    /// The supplied slice must include every real source frame in the queried halo.
    /// A failed call clears all of `output`.
    pub fn render_window(
        &self,
        source_start: u64,
        source: &[[f32; 2]],
        global_output_start: u64,
        output: &mut [[f32; 2]],
    ) -> Result<(), ImportError> {
        let result = self.render_window_inner(source_start, source, global_output_start, output);
        if result.is_err() {
            output.fill([0.0; 2]);
        }
        result
    }

    /// Reads exactly the required source halo from a bounded WAV reader, then
    /// converts without allocating. Both scratch capacities and metadata are
    /// checked before the reader performs payload I/O.
    pub fn render_wav_window(
        &self,
        reader: &mut crate::WavFrameReader,
        global_output_start: u64,
        output: &mut [[f32; 2]],
        source_scratch: &mut [[f32; 2]],
    ) -> Result<(), ImportError> {
        self.render_reader_window(
            ReaderMetadata {
                sample_rate: reader.sample_rate(),
                source_frames: reader.source_frames(),
                max_window_frames: reader.max_window_frames(),
            },
            global_output_start,
            output,
            source_scratch,
            |source_start, scratch| {
                reader.read_frames_into(source_start, scratch)?;
                Ok(())
            },
        )
    }

    /// Reads a globally phased source halo from a verified WAV descriptor and
    /// converts it into caller-owned output without allocating.
    pub fn render_verified_wav_window(
        &self,
        reader: &mut crate::VerifiedWavReader,
        global_output_start: u64,
        output: &mut [[f32; 2]],
        source_scratch: &mut [[f32; 2]],
    ) -> Result<(), ImportError> {
        self.render_reader_window(
            ReaderMetadata {
                sample_rate: reader.sample_rate(),
                source_frames: reader.source_frames(),
                max_window_frames: reader.max_window_frames(),
            },
            global_output_start,
            output,
            source_scratch,
            |source_start, scratch| {
                reader.read_frames_into(source_start, scratch)?;
                Ok(())
            },
        )
    }

    fn render_reader_window(
        &self,
        reader: ReaderMetadata,
        global_output_start: u64,
        output: &mut [[f32; 2]],
        source_scratch: &mut [[f32; 2]],
        read_source: impl FnOnce(u64, &mut [[f32; 2]]) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        output.fill([0.0; 2]);
        let result = (|| {
            if reader.sample_rate != self.source_rate()
                || reader.source_frames != self.source_frames
            {
                return Err(ImportError::Invalid(
                    "wav reader metadata does not match resample plan",
                ));
            }
            let (source_start, source_end) =
                self.required_source_range(global_output_start, output.len())?;
            let source_count = usize::try_from(source_end - source_start)
                .map_err(|_| ImportError::Invalid("source window exceeds addressable range"))?;
            if source_count > reader.max_window_frames {
                return Err(ImportError::LimitExceeded("wav reader window frames"));
            }
            if source_count > source_scratch.len() {
                return Err(ImportError::LimitExceeded("source scratch frames"));
            }
            let scratch = &mut source_scratch[..source_count];
            read_source(source_start, scratch)?;
            self.render_window(source_start, scratch, global_output_start, output)
        })();
        if result.is_err() {
            output.fill([0.0; 2]);
        }
        result
    }

    fn check_output_range(
        &self,
        global_output_start: u64,
        output_count: usize,
    ) -> Result<u64, ImportError> {
        if output_count > self.max_output_window_frames {
            return Err(ImportError::LimitExceeded("output window frames"));
        }
        let output_count = u64::try_from(output_count)
            .map_err(|_| ImportError::Invalid("output window frame count overflow"))?;
        let output_end = global_output_start
            .checked_add(output_count)
            .ok_or(ImportError::Invalid("output window range overflow"))?;
        if global_output_start > self.output_frames || output_end > self.output_frames {
            return Err(ImportError::Invalid("output window exceeds end"));
        }
        Ok(output_end)
    }

    fn render_window_inner(
        &self,
        source_start: u64,
        source: &[[f32; 2]],
        global_output_start: u64,
        output: &mut [[f32; 2]],
    ) -> Result<(), ImportError> {
        output.fill([0.0; 2]);
        self.check_output_range(global_output_start, output.len())?;
        if output.is_empty() {
            return Ok(());
        }
        let source_count = u64::try_from(source.len())
            .map_err(|_| ImportError::Invalid("source window frame count overflow"))?;
        let source_end = source_start
            .checked_add(source_count)
            .ok_or(ImportError::Invalid("source window range overflow"))?;
        if source_end > self.source_frames {
            return Err(ImportError::Invalid("source window exceeds end"));
        }
        let (required_start, required_end) =
            self.required_source_range(global_output_start, output.len())?;
        if source_start > required_start || source_end < required_end {
            return Err(ImportError::Invalid(
                "source window is missing resampling halo",
            ));
        }
        let required_start_index = usize::try_from(required_start - source_start)
            .map_err(|_| ImportError::Invalid("source window offset exceeds addressable range"))?;
        let required_end_index = usize::try_from(required_end - source_start)
            .map_err(|_| ImportError::Invalid("source window end exceeds addressable range"))?;
        if source[required_start_index..required_end_index]
            .iter()
            .flatten()
            .any(|sample| !sample.is_finite())
        {
            return Err(ImportError::Invalid("source samples must be finite"));
        }
        if self.source_rate() == self.target_rate() {
            let source_offset =
                usize::try_from(global_output_start - source_start).map_err(|_| {
                    ImportError::Invalid("identity source offset exceeds addressable range")
                })?;
            let source_end = source_offset
                .checked_add(output.len())
                .ok_or(ImportError::Invalid("identity source range overflow"))?;
            let identity_source = source
                .get(source_offset..source_end)
                .ok_or(ImportError::Invalid("identity source window is incomplete"))?;
            output.copy_from_slice(identity_source);
            return Ok(());
        }

        let radius = u64::try_from(self.kernel.inner.radius)
            .map_err(|_| ImportError::Invalid("resampling radius exceeds project range"))?;
        for (output_offset, destination) in output.iter_mut().enumerate() {
            let global_index = global_output_start
                .checked_add(u64::try_from(output_offset).map_err(|_| {
                    ImportError::Invalid("output frame offset exceeds project range")
                })?)
                .ok_or(ImportError::Invalid("output frame index overflow"))?;
            let (center, phase_index, fraction) = self.position(global_index)?;
            let row_start = phase_index
                .checked_mul(self.kernel.inner.taps)
                .ok_or(ImportError::Invalid("resampling table index overflow"))?;
            let row_end = row_start
                .checked_add(self.kernel.inner.taps)
                .ok_or(ImportError::Invalid("resampling table index overflow"))?;
            let row =
                self.kernel
                    .inner
                    .weights
                    .get(row_start..row_end)
                    .ok_or(ImportError::Invalid(
                        "resampling kernel table is incomplete",
                    ))?;
            // The halo was validated for the whole output window above. Clip
            // each row once to real source frames, keeping zero padding at the
            // source edges and the original ascending tap accumulation order.
            let first_source = center.saturating_sub(radius);
            let end_source = center
                .saturating_add(radius)
                .saturating_add(1)
                .min(self.source_frames);
            let first_weight = usize::try_from(radius.saturating_sub(center)).map_err(|_| {
                ImportError::Invalid("resampling weight offset exceeds addressable range")
            })?;
            let count = usize::try_from(end_source.saturating_sub(first_source)).map_err(|_| {
                ImportError::Invalid("resampling tap count exceeds addressable range")
            })?;
            let end_weight = first_weight
                .checked_add(count)
                .ok_or(ImportError::Invalid("resampling weight range overflow"))?;
            let weights = row
                .get(first_weight..end_weight)
                .ok_or(ImportError::Invalid(
                    "resampling kernel table is incomplete",
                ))?;
            let source_offset = first_source
                .checked_sub(source_start)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or(ImportError::Invalid(
                    "source tap offset exceeds addressable range",
                ))?;
            let source_end = source_offset
                .checked_add(count)
                .ok_or(ImportError::Invalid("source tap range overflow"))?;
            let frames = source
                .get(source_offset..source_end)
                .ok_or(ImportError::Invalid(
                    "source window is missing a required tap",
                ))?;
            let mut left = 0.0_f64;
            let mut right = 0.0_f64;
            match self.kernel.version() {
                ResampleVersion::V1 => {
                    for (&weight, frame) in weights.iter().zip(frames) {
                        if weight == 0.0 {
                            continue;
                        }
                        left += f64::from(frame[0]) * weight;
                        right += f64::from(frame[1]) * weight;
                    }
                }
                ResampleVersion::V2 => {
                    // The closing row follows every interpolated row.
                    let next = self
                        .kernel
                        .inner
                        .weights
                        .get(row_end + first_weight..row_end + end_weight)
                        .ok_or(ImportError::Invalid(
                            "resampling kernel table is incomplete",
                        ))?;
                    for ((&low, &high), frame) in weights.iter().zip(next).zip(frames) {
                        let weight = low + (high - low) * fraction;
                        left += f64::from(frame[0]) * weight;
                        right += f64::from(frame[1]) * weight;
                    }
                }
            }
            *destination = [checked_sample(left)?, checked_sample(right)?];
        }
        Ok(())
    }
}

impl ResamplePlan {
    /// Source center, phase row and the fraction toward the next row for one
    /// output frame. v1 rounds to the nearest of its rows (fraction 0); v2
    /// floors to its row and interpolates by the exact rational remainder.
    fn position(&self, output_index: u64) -> Result<(u64, usize, f64), ImportError> {
        match self.kernel.version() {
            ResampleVersion::V1 => {
                let (center, phase) =
                    source_position(output_index, self.source_rate(), self.target_rate())?;
                Ok((center, phase, 0.0))
            }
            ResampleVersion::V2 => {
                let target = u128::from(self.target_rate());
                let position = u128::from(output_index)
                    .checked_mul(u128::from(self.source_rate()))
                    .ok_or(ImportError::Invalid("resampling position overflow"))?;
                let center = u64::try_from(position / target).map_err(|_| {
                    ImportError::Invalid("resampling position exceeds project range")
                })?;
                let scaled = (position % target)
                    .checked_mul(V2_PHASE_COUNT as u128)
                    .ok_or(ImportError::Invalid("resampling phase overflow"))?;
                // remainder < target, so the row is below V2_PHASE_COUNT.
                let row = (scaled / target) as usize;
                let fraction = (scaled % target) as f64 / target as f64;
                Ok((center, row, fraction))
            }
        }
    }
}

fn source_position(
    output_index: u64,
    source_rate: u32,
    target_rate: u32,
) -> Result<(u64, usize), ImportError> {
    let target_rate_u128 = u128::from(target_rate);
    let source_rate_u128 = u128::from(source_rate);
    let source_position = u128::from(output_index)
        .checked_mul(source_rate_u128)
        .ok_or(ImportError::Invalid("resampling position overflow"))?;
    let mut center = u64::try_from(source_position / target_rate_u128)
        .map_err(|_| ImportError::Invalid("resampling position exceeds project range"))?;
    let remainder = source_position % target_rate_u128;
    let phase_scaled = remainder
        .checked_mul(PHASE_COUNT as u128)
        .and_then(|value| value.checked_add(target_rate_u128 / 2))
        .ok_or(ImportError::Invalid("resampling phase overflow"))?
        / target_rate_u128;
    let phase_index = if phase_scaled >= PHASE_COUNT as u128 {
        center = center
            .checked_add(1)
            .ok_or(ImportError::Invalid("resampling position overflow"))?;
        0
    } else {
        phase_scaled as usize
    };
    Ok((center, phase_index))
}

fn validate_plan_dimensions(
    source_rate: u32,
    target_rate: u32,
    source_frames: u64,
    max_source_frames: u64,
    max_output_frames: u64,
) -> Result<u64, ImportError> {
    validate_rates(source_rate, target_rate)?;
    if source_frames == 0 {
        return Err(ImportError::Invalid("source audio must contain frames"));
    }
    if source_frames > max_source_frames {
        return Err(ImportError::LimitExceeded("source frames"));
    }
    let output_frames = converted_frame_count(source_frames, source_rate, target_rate)
        .map_err(|error| ImportError::Invalid(error.0))?;
    if output_frames > max_output_frames {
        return Err(ImportError::LimitExceeded("output frames"));
    }
    Ok(output_frames)
}

fn checked_sample(value: f64) -> Result<f32, ImportError> {
    if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
        return Err(ImportError::Invalid(
            "resampled sample is non-finite or out of range",
        ));
    }
    let sample = value as f32;
    if !sample.is_finite() {
        return Err(ImportError::Invalid(
            "resampled sample is non-finite or out of range",
        ));
    }
    Ok(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: ResampleLimits = ResampleLimits {
        max_source_frames: 100_000,
        max_output_frames: 200_000,
    };

    fn audio(rate: u32, frames: Vec<[f32; 2]>) -> Audio {
        Audio::new(rate, frames).expect("valid source audio")
    }

    fn sine(rate: u32, frames: usize, frequency: f64, right: bool) -> Audio {
        let samples = (0..frames)
            .map(|index| {
                let value = (std::f64::consts::TAU * frequency * index as f64 / f64::from(rate))
                    .sin() as f32
                    * 0.8;
                if right { [0.0, value] } else { [value, 0.0] }
            })
            .collect();
        audio(rate, samples)
    }

    fn amplitude(frames: &[[f32; 2]], rate: u32, frequency: f64, channel: usize) -> f64 {
        let start = 256;
        let end = frames.len() - 256;
        let count = (end - start) as f64;
        let (mut sine, mut cosine) = (0.0, 0.0);
        for (index, frame) in frames.iter().enumerate().take(end).skip(start) {
            let phase = std::f64::consts::TAU * frequency * index as f64 / f64::from(rate);
            sine += f64::from(frame[channel]) * phase.sin();
            cosine += f64::from(frame[channel]) * phase.cos();
        }
        2.0 * sine.hypot(cosine) / count
    }

    #[test]
    fn windowed_cancellable_conversion_matches_one_window_and_honours_cancel() {
        use std::sync::atomic::AtomicBool;
        // Several cancellation windows; the global phase must be continuous.
        let source = sine(44_100, 3 * CANCEL_CHECK_OUTPUT_FRAMES / 2, 997.0, false);
        let limits = ResampleLimits {
            max_source_frames: 1 << 20,
            max_output_frames: 1 << 20,
        };
        let running = AtomicBool::new(false);
        let windowed =
            resample_cancellable(&source, 48_000, limits, ResampleVersion::V1, &running).unwrap();
        let plan = ResamplePlan::with_kernel(
            ResampleKernel::with_version(44_100, 48_000, ResampleVersion::V1).unwrap(),
            source.frames().len() as u64,
            u64::MAX,
            u64::MAX,
            usize::MAX,
        )
        .unwrap();
        let mut whole = vec![[0.0; 2]; plan.output_frames() as usize];
        plan.render_window(0, source.frames(), 0, &mut whole)
            .unwrap();
        assert!(whole.len() > CANCEL_CHECK_OUTPUT_FRAMES);
        assert_eq!(windowed.frames(), whole.as_slice());
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            resample_cancellable(&source, 48_000, limits, ResampleVersion::V1, &cancel),
            Err(ImportError::Cancelled)
        ));
    }

    #[test]
    fn kernel_table_stays_below_the_documented_cap_at_extreme_rate_ratio() {
        let kernel = ResampleKernel::new(192_000, 8_000).expect("extreme ratio kernel");
        assert!(kernel.table_payload_bytes() <= 13 * 1_024 * 1_024);
    }

    #[test]
    fn one_rate_pair_kernel_is_shared_across_many_source_plans() {
        let kernel = ResampleKernel::new(44_100, 48_000).expect("shared kernel");
        let kernel_bytes = kernel.table_payload_bytes();
        assert!(kernel_bytes > 0);
        let plans = (1..=100_u64)
            .map(|source_frames| {
                ResamplePlan::with_kernel(
                    kernel.clone(),
                    source_frames,
                    source_frames,
                    source_frames + 10,
                    usize::try_from(source_frames).expect("frame count fits usize") + 64,
                )
                .expect("bounded source plan")
            })
            .collect::<Vec<_>>();

        assert_eq!(plans.len(), 100);
        assert!(
            plans
                .iter()
                .all(|plan| std::sync::Arc::ptr_eq(&kernel.inner, &plan.kernel.inner))
        );
        assert_eq!(kernel.table_payload_bytes(), kernel_bytes);
        assert!(plans.iter().all(|plan| plan.source_rate() == 44_100));
        assert!(plans.iter().all(|plan| plan.target_rate() == 48_000));
    }

    #[test]
    fn shared_kernel_windows_match_resident_v1_conversion_bits() {
        let frames = (0..257)
            .map(|index| {
                [
                    ((index * 37 + 11) % 101) as f32 / 128.0 - 0.375,
                    ((index * 19 + 7) % 67) as f32 / 64.0 - 0.5,
                ]
            })
            .collect::<Vec<_>>();
        let source = audio(48_000, frames);
        let expected = resample(&source, 44_100, LIMITS).expect("resident conversion");
        let kernel = ResampleKernel::new(48_000, 44_100).expect("shared kernel");
        let plan = ResamplePlan::with_kernel(
            kernel,
            source.frames().len() as u64,
            100_000,
            200_000,
            200_000,
        )
        .expect("shared plan");
        let mut actual = vec![[0.0; 2]; expected.frames().len()];
        plan.render_window(0, source.frames(), 0, &mut actual)
            .expect("shared plan render");
        assert_eq!(actual.len(), expected.frames().len());
        for (actual, expected) in actual
            .iter()
            .flatten()
            .zip(expected.frames().iter().flatten())
        {
            assert_eq!(actual.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn shared_kernel_lives_until_the_last_plan_is_dropped() {
        let kernel = ResampleKernel::new(48_000, 44_100).expect("kernel");
        let weak = std::sync::Arc::downgrade(&kernel.inner);
        let plan =
            ResamplePlan::with_kernel(kernel.clone(), 48_000, 48_000, 44_100, 512).expect("plan");
        drop(kernel);
        assert!(weak.upgrade().is_some());
        drop(plan);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn identity_kernel_has_no_weight_payload_and_plan_rates_come_from_kernel() {
        let kernel = ResampleKernel::new(48_000, 48_000).expect("identity kernel");
        assert_eq!(kernel.table_payload_bytes(), 0);
        let plan = ResamplePlan::with_kernel(kernel.clone(), 16, 16, 16, 16).expect("plan");
        assert_eq!((plan.source_rate(), plan.target_rate()), (48_000, 48_000));
        assert!(std::sync::Arc::ptr_eq(&kernel.inner, &plan.kernel.inner));
    }

    #[test]
    fn shared_kernel_plan_rejects_source_and_output_budgets() {
        let kernel = ResampleKernel::new(44_100, 48_000).expect("kernel");
        assert!(matches!(
            ResamplePlan::with_kernel(kernel.clone(), 0, 100, 200, 128),
            Err(ImportError::Invalid("source audio must contain frames"))
        ));
        assert!(matches!(
            ResamplePlan::with_kernel(kernel.clone(), 101, 100, 200, 128),
            Err(ImportError::LimitExceeded("source frames"))
        ));
        assert!(matches!(
            ResamplePlan::with_kernel(kernel, 100, 100, 108, 128),
            Err(ImportError::LimitExceeded("output frames"))
        ));
        assert!(matches!(
            ResampleKernel::new(7_999, 48_000),
            Err(ImportError::Invalid(_))
        ));
    }

    #[test]
    fn identity_conversion_preserves_bits_and_shares_pcm_storage() {
        let source = audio(48_000, vec![[0.0, -0.0], [0.125, -0.75], [1.0, -1.0]]);
        let converted = resample(&source, 48_000, LIMITS).expect("identity conversion");
        assert_eq!(source.frames().as_ptr(), converted.frames().as_ptr());
        assert_eq!(source.frames().len(), converted.frames().len());
        for (source, converted) in source.frames().iter().zip(converted.frames()) {
            assert_eq!(source[0].to_bits(), converted[0].to_bits());
            assert_eq!(source[1].to_bits(), converted[1].to_bits());
        }
    }

    #[test]
    fn output_lengths_use_project_rounding_and_keep_tiny_sources_nonempty() {
        let source = audio(44_100, vec![[0.25, -0.25]; 441]);
        let converted = resample(&source, 48_000, LIMITS).expect("44.1 to 48 khz");
        assert_eq!(converted.frames().len(), 480);
        let tiny = audio(192_000, vec![[0.75, -0.75]]);
        let converted = resample(&tiny, 8_000, LIMITS).expect("tiny downsample");
        assert_eq!(converted.frames().len(), 1);
    }

    #[test]
    fn enforces_rate_and_source_output_frame_limits_before_filter_allocation() {
        let source = audio(48_000, vec![[0.0, 0.0]; 10]);
        assert!(matches!(
            resample(
                &source,
                48_000,
                ResampleLimits {
                    max_source_frames: 9,
                    max_output_frames: 10,
                }
            ),
            Err(ImportError::LimitExceeded("source frames"))
        ));
        assert!(matches!(
            resample(
                &source,
                96_000,
                ResampleLimits {
                    max_source_frames: 10,
                    max_output_frames: 19,
                }
            ),
            Err(ImportError::LimitExceeded("output frames"))
        ));
        let unsupported = audio(7_999, vec![[0.0, 0.0]; 2]);
        assert!(matches!(
            resample(&unsupported, 48_000, LIMITS),
            Err(ImportError::Invalid(
                "sample rates must be within 8000..192000 hz"
            ))
        ));
        assert!(matches!(
            resample(&source, 192_001, LIMITS),
            Err(ImportError::Invalid(
                "sample rates must be within 8000..192000 hz"
            ))
        ));
    }

    #[test]
    fn centered_impulse_keeps_its_expected_output_position() {
        let mut input = vec![[0.0, 0.0]; 256];
        input[128] = [1.0, -0.5];
        let converted = resample(&audio(48_000, input), 96_000, LIMITS).expect("upsample impulse");
        let peak = converted
            .frames()
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| left[0].abs().total_cmp(&right[0].abs()))
            .map(|(index, _)| index)
            .expect("peak");
        assert_eq!(peak, 256);
        let left = converted.frames()[256][0];
        let right = converted.frames()[256][1];
        assert!(left > 0.8);
        assert!((right / left + 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn normalized_kernel_preserves_interior_dc_and_zero_pads_edges() {
        let source = audio(48_000, vec![[1.0, 1.0]; 1_000]);
        let converted = resample(&source, 96_000, LIMITS).expect("upsample dc");
        assert!(converted.frames()[0][0] < 0.99);
        for frame in &converted.frames()[128..1_872] {
            assert!((frame[0] - 1.0).abs() < 1.0e-5);
            assert!((frame[1] - 1.0).abs() < 1.0e-5);
        }
    }

    #[test]
    fn stereo_channels_remain_isolated() {
        let source = sine(44_100, 1_000, 1_000.0, false);
        let converted = resample(&source, 48_000, LIMITS).expect("resample left-only signal");
        assert!(converted.frames().iter().any(|frame| frame[0].abs() > 0.1));
        assert!(converted.frames().iter().all(|frame| frame[1] == 0.0));
    }

    #[test]
    fn downsampling_preserves_passband_and_rejects_alias_band() {
        let passband = resample(&sine(48_000, 48_000, 1_000.0, false), 16_000, LIMITS)
            .expect("downsample passband signal");
        let stopband = resample(&sine(48_000, 48_000, 12_000.0, false), 16_000, LIMITS)
            .expect("downsample stopband signal");
        let passband_amplitude = amplitude(passband.frames(), 16_000, 1_000.0, 0);
        let alias_amplitude = amplitude(stopband.frames(), 16_000, 4_000.0, 0);
        assert!(
            passband_amplitude > 0.79,
            "passband amplitude {passband_amplitude}"
        );
        assert!(
            alias_amplitude <= 1.0e-3,
            "alias amplitude {alias_amplitude}"
        );
    }

    #[test]
    fn non_integer_and_extreme_rate_conversions_remain_finite() {
        let non_integer =
            resample(&sine(44_100, 4_410, 997.0, true), 48_000, LIMITS).expect("44.1 to 48 khz");
        let extreme =
            resample(&sine(192_000, 4_096, 997.0, true), 8_000, LIMITS).expect("192 to 8 khz");
        assert!(
            non_integer
                .frames()
                .iter()
                .flatten()
                .all(|sample| sample.is_finite())
        );
        assert!(
            extreme
                .frames()
                .iter()
                .flatten()
                .all(|sample| sample.is_finite())
        );
    }

    #[test]
    fn non_finite_or_out_of_range_accumulators_are_rejected() {
        assert!(checked_sample(f64::INFINITY).is_err());
        assert!(checked_sample(f64::NAN).is_err());
        assert!(checked_sample(f64::from(f32::MAX) * 1.01).is_err());
        assert!(checked_sample(-f64::from(f32::MAX) * 1.01).is_err());
    }
}
