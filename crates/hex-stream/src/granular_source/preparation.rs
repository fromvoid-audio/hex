//! Automatic processing scratch on the already-admitted reader's control owner.
use super::*;

/// Listed processing payload limits, not a reader/kernel/bank/runtime grant.
/// The caller admits the already-owned reader, manifest/path and conversion
/// kernel, this inline value, allocator/Arc metadata and aggregate source bank
/// separately. All allocation, I/O and final destruction happen on control.
#[derive(Clone, Copy, Debug)]
pub struct GranularSourcePreparationLimits {
    /// Original-rate stereo frames, independent of the file's total duration.
    pub source_scratch_frames: usize,
    pub checkpoint_entries: usize,
    pub checkpoint_budget: usize,
    /// Complete retained renderer/region/checkpoint/buffer payload, at most 32 MiB.
    pub max_processing_bytes: usize,
    /// Processing construction payload, including temporary Vec-to-Box backing.
    pub max_construction_peak_bytes: usize,
}
impl GranularSourcePreparationLimits {
    /// Conservative bound before creating any scratch. The existing renderer
    /// and checkpoint owners each enforce their combined 32 MiB processing
    /// ceiling. Two such ceilings plus twice the source/cache payload cover
    /// construction and temporary boxed-slice conversion without DSP formulas.
    /// This excludes already-admitted reader/kernel backing and opaque costs.
    pub fn construction_processing_bound(
        self,
        project_rate: u32,
    ) -> Result<usize, GranularSourceError> {
        let (_, prefix) = self.prefix(project_rate)?;
        hex_warp::MAX_SCRATCH_BYTES
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(prefix.checked_mul(2)?))
            .ok_or(GranularSourceError::ScratchLimit)
    }
    fn prefix(self, rate: u32) -> Result<(usize, usize), GranularSourceError> {
        if !(8_000..=192_000).contains(&rate)
            || !(1..=262_144).contains(&self.source_scratch_frames)
            || !(2..=crate::granular_checkpoint_cache::MAX_ENTRIES)
                .contains(&self.checkpoint_entries)
            || self.checkpoint_budget == 0
            || self.max_processing_bytes == 0
            || self.max_processing_bytes > hex_warp::MAX_SCRATCH_BYTES
        {
            return Err(GranularSourceError::ScratchLimit);
        }
        let cache = crate::granular::region_min_slot(rate)
            .checked_mul(4)
            .ok_or(GranularSourceError::ScratchLimit)?;
        let bytes = self
            .source_scratch_frames
            .checked_add(cache)
            .and_then(|frames| frames.checked_mul(std::mem::size_of::<Frame>()))
            .ok_or(GranularSourceError::ScratchLimit)?;
        if bytes
            .checked_add(self.checkpoint_budget)
            .is_none_or(|bytes| bytes > self.max_processing_bytes)
        {
            return Err(GranularSourceError::ScratchLimit);
        }
        Ok((cache, bytes))
    }
}
impl VerifiedGranularSource {
    /// Allocate bounded scratch around an already-admitted reader and kernel.
    ///
    /// This is blocking control/analysis work, never callback work or work for
    /// the sole active page-service worker. Reference selection, rendering and
    /// checkpoints reuse the manual-buffer constructor. Grain scratch comes
    /// from the actual renderer; no source-duration PCM buffer is allocated.
    /// Processing/peak refusal occurs before scratch allocation where the bound
    /// is known; actual renderer size is checked before allocating its grain
    /// buffer. Failure drops every input on this calling control thread.
    /// Success is a source owner, not aggregate bank admission or a page lease.
    pub fn prepare_from_reader_on_control(
        raw: StreamedSourceMetadata,
        root: StreamedGranularRoot,
        reader: VerifiedWavReader,
        plan: ResamplePlan,
        limits: GranularSourcePreparationLimits,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, GranularSourceError> {
        if cancelled() {
            return Err(GranularSourceError::Source(StreamFailureKind::Cancelled));
        }
        let (cache_frames, _) = limits.prefix(raw.project_rate())?;
        let peak = limits.construction_processing_bound(raw.project_rate())?;
        if peak > limits.max_construction_peak_bytes
            || plan.max_output_window_frames()
                < crate::granular::region_min_slot(raw.project_rate())
        {
            return Err(GranularSourceError::ScratchLimit);
        }
        validate_source(&raw, &reader, &plan).map_err(GranularSourceError::Source)?;
        if !root.metadata().is_granular_of(&raw) {
            return Err(GranularSourceError::InvalidSource);
        }
        let buffers = GranularSourceBuffers {
            source: zeroed(limits.source_scratch_frames, [0.0; 2])?,
            cache: zeroed(cache_frames, [0.0; 2])?,
            grain: Vec::new().into_boxed_slice(),
        };
        Self::prepare_inner(
            GranularSourcePreparation {
                raw,
                root,
                reader,
                plan,
                buffers,
                scratch_budget: limits.max_processing_bytes,
                checkpoint_entries: limits.checkpoint_entries,
                checkpoint_budget: limits.checkpoint_budget,
            },
            Some(limits.max_processing_bytes),
            cancelled,
        )
    }
}
pub(super) fn zeroed<T: Clone>(length: usize, value: T) -> Result<Box<[T]>, GranularSourceError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| GranularSourceError::ScratchLimit)?;
    values.resize(length, value);
    Ok(values.into_boxed_slice())
}
