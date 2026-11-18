//! Control-side custody of one canonical, verified granular source.
use crate::{
    CompletedGranularPage, GranularPageError, GranularPageProgress, StreamFailureKind,
    VerifiedGranularBuffers, VerifiedGranularPages, VerifiedGranularStepError,
    reference::validate_source, scan_verified_granular_region_on_control,
};
use hex_engine::{Frame, StreamedSourceMetadata, audio_transform::StreamedGranularRoot};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::SequentialGranularConfig;

mod preparation;
pub use preparation::GranularSourcePreparationLimits;

/// Caller-owned, fixed-size buffers moved into the control-side source owner.
pub struct GranularSourceBuffers {
    pub source: Box<[Frame]>,
    pub cache: Box<[Frame]>,
    pub grain: Box<[[f64; 2]]>,
}
impl GranularSourceBuffers {
    fn bytes(&self) -> Option<usize> {
        self.source
            .len()
            .checked_mul(std::mem::size_of::<Frame>())?
            .checked_add(self.cache.len().checked_mul(std::mem::size_of::<Frame>())?)?
            .checked_add(
                self.grain
                    .len()
                    .checked_mul(std::mem::size_of::<[f64; 2]>())?,
            )
    }
}

/// Inputs move together into the source owner; they do not grant a page lease.
pub struct GranularSourcePreparation {
    pub raw: StreamedSourceMetadata,
    pub root: StreamedGranularRoot,
    pub reader: VerifiedWavReader,
    pub plan: ResamplePlan,
    pub buffers: GranularSourceBuffers,
    /// Cap for the three supplied buffer payloads, no larger than 32 MiB.
    pub scratch_budget: usize,
    pub checkpoint_entries: usize,
    pub checkpoint_budget: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularSourceError {
    InvalidSource,
    ScratchLimit,
    Source(StreamFailureKind),
    Step(VerifiedGranularStepError),
    Page(GranularPageError),
}

/// Reader, conversion plan, source-bound renderer and scratch owned together.
///
/// Preparation and all methods may block; use only on a dedicated control/worker
/// thread. Before preparation, the owning runtime must admit reader/manifest,
/// conversion kernel, path, inline objects, construction peaks, checkpoint and
/// processing budgets, page pool, queues and thread costs. `reserved_processing_bytes`
/// covers only renderer, region, checkpoint reservation and supplied buffers;
/// it is not an aggregate worker quote. The caller also binds the asset/generation
/// to the reader and invalidates this owner on source replacement. This does not
/// establish a whole-file snapshot or a catalog/playback lease.
pub struct VerifiedGranularSource {
    reader: VerifiedWavReader,
    plan: ResamplePlan,
    buffers: GranularSourceBuffers,
    pages: VerifiedGranularPages,
    reserved_processing_bytes: usize,
}
pub(crate) struct GranularSourceData {
    pub processing_reserved_bytes: usize,
    pub reader_heap_bytes: usize,
    pub kernel_reserved_bytes: usize,
}
impl VerifiedGranularSource {
    pub(crate) fn validate_for_catalog(
        &mut self,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<(), StreamFailureKind> {
        // Decode the complete float source, including data outside the chosen
        // granular region, before granting catalog eligibility.
        if self.reader.encoding() == 3 {
            crate::worker::validate_finite_source(
                &mut self.reader,
                &mut self.buffers.source,
                stop,
            )?;
        } else {
            // A fresh verified read also checks file metadata/digest after the
            // earlier preparation, without claiming a whole-file snapshot.
            let count = self
                .buffers
                .source
                .len()
                .min(self.reader.max_window_frames())
                .min(1);
            if count == 0 {
                return Err(StreamFailureKind::DecodeFailed);
            }
            self.reader
                .read_frames_into_with_cancel(0, &mut self.buffers.source[..count], || {
                    stop.load(std::sync::atomic::Ordering::Acquire)
                })
                .map_err(crate::worker::map_import_error)?;
        }
        Ok(())
    }
    pub(crate) fn holds_page(&self) -> bool {
        self.pages.holds_page()
    }
    pub(crate) fn admission_data(&self) -> Option<GranularSourceData> {
        let reader = self.reader.control_data_layout();
        let kernel = self.plan.control_kernel_data_layout();
        Some(GranularSourceData {
            processing_reserved_bytes: self.reserved_processing_bytes,
            reader_heap_bytes: reader
                .path_capacity
                .checked_add(reader.manifest_owner_payload_bytes)?
                .checked_add(
                    reader
                        .manifest_capacity
                        .checked_mul(reader.manifest_element_bytes)?,
                )?,
            kernel_reserved_bytes: kernel.owner_payload_bytes.checked_add(
                kernel
                    .weight_capacity
                    .checked_mul(kernel.weight_element_bytes)?,
            )?,
        })
    }
    /// Validate canonical identity, scan the verified region and prepare a
    /// renderer. Inputs may be dropped on failure, always on the calling thread.
    pub fn prepare(
        input: GranularSourcePreparation,
        cancelled: impl FnMut() -> bool,
    ) -> Result<Self, GranularSourceError> {
        Self::prepare_inner(input, None, cancelled)
    }
    fn prepare_inner(
        input: GranularSourcePreparation,
        automatic_processing_limit: Option<usize>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, GranularSourceError> {
        let GranularSourcePreparation {
            raw,
            root,
            mut reader,
            plan,
            mut buffers,
            scratch_budget,
            checkpoint_entries,
            checkpoint_budget,
        } = input;
        let mut scratch = buffers.bytes().ok_or(GranularSourceError::ScratchLimit)?;
        if scratch_budget > hex_warp::MAX_SCRATCH_BYTES
            || scratch > scratch_budget
            || scratch
                .checked_add(checkpoint_budget)
                .is_none_or(|n| n > hex_warp::MAX_SCRATCH_BYTES)
        {
            return Err(GranularSourceError::ScratchLimit);
        }
        validate_source(&raw, &reader, &plan).map_err(GranularSourceError::Source)?;
        let (expected, region, map, mode, pitch_cents, law) = root.into_parts();
        let derived = raw
            .with_granular(&region, &map, mode, pitch_cents, law)
            .map_err(|_| GranularSourceError::InvalidSource)?;
        if expected != derived {
            return Err(GranularSourceError::InvalidSource);
        }
        let reference = scan_verified_granular_region_on_control(
            &raw,
            region,
            &mut reader,
            &plan,
            &mut buffers.source,
            &mut buffers.cache,
            &mut cancelled,
        )
        .map_err(GranularSourceError::Step)?;
        let channel = reference.channel();
        let pages = VerifiedGranularPages::new(
            reference,
            SequentialGranularConfig {
                map,
                sample_rate: raw.project_rate(),
                mode,
                pitch_cents,
                resampler: law,
                reference_channel: channel,
            },
            checkpoint_entries,
            checkpoint_budget,
            &mut cancelled,
        )
        .map_err(GranularSourceError::Page)?;
        if pages.metadata() != &expected {
            return Err(GranularSourceError::InvalidSource);
        }
        if let Some(limit) = automatic_processing_limit {
            if !pages.checkpoint_recording_fits() {
                return Err(GranularSourceError::ScratchLimit);
            }
            let grain_bytes = pages
                .max_source_frames()
                .checked_mul(std::mem::size_of::<[f64; 2]>())
                .ok_or(GranularSourceError::ScratchLimit)?;
            let retained = pages
                .reserved_processing_bytes()
                .and_then(|bytes| bytes.checked_add(scratch))
                .and_then(|bytes| bytes.checked_add(grain_bytes))
                .ok_or(GranularSourceError::ScratchLimit)?;
            if retained > limit {
                return Err(GranularSourceError::ScratchLimit);
            }
            buffers.grain = preparation::zeroed(pages.max_source_frames(), [0.0; 2])?;
            scratch = buffers.bytes().ok_or(GranularSourceError::ScratchLimit)?;
        }
        if buffers.grain.len() < pages.max_source_frames() {
            return Err(GranularSourceError::ScratchLimit);
        }
        let reserved_processing_bytes = pages
            .reserved_processing_bytes()
            .and_then(|n| n.checked_add(scratch))
            .ok_or(GranularSourceError::ScratchLimit)?;
        if reserved_processing_bytes > hex_warp::MAX_SCRATCH_BYTES {
            return Err(GranularSourceError::ScratchLimit);
        }
        if cancelled() {
            return Err(GranularSourceError::Step(
                VerifiedGranularStepError::Source(StreamFailureKind::Cancelled),
            ));
        }
        Ok(Self {
            reader,
            plan,
            buffers,
            pages,
            reserved_processing_bytes,
        })
    }
    pub fn metadata(&self) -> &StreamedSourceMetadata {
        self.pages.metadata()
    }
    pub fn reserved_processing_bytes(&self) -> usize {
        self.reserved_processing_bytes
    }
    pub fn begin_page(
        &mut self,
        index: u64,
        frames: Box<[Frame]>,
        max_preroll_frames: u64,
        cancelled: impl FnMut() -> bool,
    ) -> Result<(), (GranularPageError, Box<[Frame]>)> {
        self.pages.begin_page(
            index,
            frames,
            max_preroll_frames,
            &mut self.reader,
            &self.plan,
            cancelled,
        )
    }
    pub fn begin_page_replaying(
        &mut self,
        index: u64,
        frames: Box<[Frame]>,
        segment_frames: u64,
        cancelled: impl FnMut() -> bool,
    ) -> Result<(), (GranularPageError, Box<[Frame]>)> {
        self.pages.begin_page_replaying(
            index,
            frames,
            segment_frames,
            &mut self.reader,
            &self.plan,
            cancelled,
        )
    }
    pub fn step(
        &mut self,
        cancelled: impl FnMut() -> bool,
    ) -> Result<GranularPageProgress, GranularPageError> {
        self.pages.step(
            &mut self.reader,
            &self.plan,
            VerifiedGranularBuffers {
                source: &mut self.buffers.source,
                cache: &mut self.buffers.cache,
                grain: &mut self.buffers.grain,
            },
            cancelled,
        )
    }
    pub fn take_complete(&mut self) -> Option<CompletedGranularPage> {
        self.pages.take_complete()
    }
    /// Reclaim an incomplete, failed or unwanted page without publishing it.
    pub fn reclaim(&mut self) -> Option<Box<[Frame]>> {
        self.pages.reclaim()
    }
}
