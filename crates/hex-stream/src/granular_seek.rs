//! Bounded control-side replay from verified granular checkpoints.
use crate::{
    StreamFailureKind, VerifiedGranularBuffers, VerifiedGranularStepError,
    VerifiedReferenceChannel, advance_verified_granular_on_control, reference::validate_source,
    worker::map_import_error,
};
use hex_engine::Frame;
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{Error, GranularCheckpoint, SequentialGranular};

/// Largest permitted distance between a checkpoint and its target.
pub const MAX_GRANULAR_PREROLL_FRAMES: u64 = 1_048_576;
/// Maximum newly analyzed grains in one replay call.
pub const MAX_GRANULAR_SEEK_STEPS: usize = 4;
const MAX_DISCARD_FRAMES: usize = 4096;

/// Replay never exposes discarded audio as a completed seek.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularSeekProgress {
    Pending { position: u64 },
    Reached { position: u64 },
}

/// A renderer bound to one immutable verified reference decision.
///
/// The caller establishes asset/generation custody, owns checkpoints, scratch,
/// reader and scheduling, and must invalidate this cursor on source replacement.
/// All operations are control-side; they may block and must not run on an audio
/// callback or the sole worker supplying active playback pages.
pub struct VerifiedGranularCursor {
    renderer: SequentialGranular,
    reference: VerifiedReferenceChannel,
    seeking: Option<u64>,
}
impl VerifiedGranularCursor {
    /// Bind a fresh renderer to its matching project-rate reference channel.
    pub fn new(
        renderer: SequentialGranular,
        reference: VerifiedReferenceChannel,
    ) -> Result<Self, Error> {
        if !renderer.is_pristine()
            || renderer.source_parameters()
                != (reference.metadata().project_rate(), reference.channel())
        {
            return Err(Error::Invalid("incompatible granular cursor source"));
        }
        Ok(Self {
            renderer,
            reference,
            seeking: None,
        })
    }
    pub fn position(&self) -> u64 {
        self.renderer.position()
    }
    pub fn available(&self) -> usize {
        if self.seeking.is_some() {
            0
        } else {
            self.renderer.available()
        }
    }
    pub fn max_source_frames(&self) -> usize {
        self.renderer.max_source_frames()
    }
    pub fn is_poisoned(&self) -> bool {
        self.renderer.is_poisoned()
    }
    pub fn seek_target(&self) -> Option<u64> {
        self.seeking
    }

    pub(crate) fn owns_checkpoint(&self, checkpoint: &GranularCheckpoint) -> bool {
        self.renderer.owns_checkpoint(checkpoint)
    }
    pub(crate) fn is_pristine(&self) -> bool {
        self.seeking.is_none() && self.renderer.is_pristine()
    }
    pub(crate) fn checkpoint_storage_limit(&self) -> Option<usize> {
        let used = self
            .renderer
            .backing_bytes()?
            .checked_add(
                self.renderer
                    .max_source_frames()
                    .checked_mul(std::mem::size_of::<[f64; 2]>())?,
            )?
            .checked_add(
                self.reference
                    .region()
                    .map_or(Some(0), |region| region.backing_bytes())?,
            )?;
        hex_warp::MAX_SCRATCH_BYTES.checked_sub(used)
    }
    pub(crate) fn refresh_checkpoint(
        &self,
        checkpoint: &mut GranularCheckpoint,
    ) -> Result<(), Error> {
        if self.seeking.is_some() {
            return Err(Error::Invalid("granular seek is pending"));
        }
        self.renderer.refresh_checkpoint(checkpoint)
    }

    /// Capture at a normal output boundary; checkpoint storage is caller-owned.
    pub fn checkpoint(&self, budget: usize) -> Result<GranularCheckpoint, Error> {
        if self.seeking.is_some() {
            return Err(Error::Invalid("granular seek is pending"));
        }
        self.renderer.checkpoint(budget)
    }
    /// Advance one verified grain during ordinary sequential output.
    pub fn advance(
        &mut self,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        buffers: VerifiedGranularBuffers<'_>,
        cancelled: impl FnMut() -> bool,
    ) -> Result<bool, VerifiedGranularStepError> {
        if self.seeking.is_some() {
            return Err(Error::Invalid("granular seek is pending").into());
        }
        advance_verified_granular_on_control(
            &mut self.renderer,
            &self.reference,
            reader,
            plan,
            buffers,
            cancelled,
        )
    }
    /// Drain completed ordinary output, never intermediate seek audio.
    pub fn drain(&mut self, output: &mut [Frame]) -> Result<(), Error> {
        if self.seeking.is_some() {
            return Err(Error::Invalid("granular seek is pending"));
        }
        self.renderer.drain(output)
    }

    /// Restore a supplied instance-bound checkpoint and begin bounded replay.
    /// The caller chooses/adopts checkpoints and supplies a maximum allowed
    /// preroll distance, capped by `MAX_GRANULAR_PREROLL_FRAMES`. Reader facts
    /// and one live verified source frame are checked before state mutation.
    /// This does not establish a whole-file snapshot or verify unread pages.
    pub fn begin_seek(
        &mut self,
        checkpoint: &GranularCheckpoint,
        target: u64,
        max_preroll_frames: u64,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<GranularSeekProgress, VerifiedGranularStepError> {
        let distance = target
            .checked_sub(checkpoint.position())
            .ok_or(Error::Invalid("granular target precedes checkpoint"))?;
        if target > self.renderer.total_frames()
            || max_preroll_frames > MAX_GRANULAR_PREROLL_FRAMES
            || distance > max_preroll_frames
        {
            return Err(Error::Invalid("granular seek exceeds bounded preroll").into());
        }
        validate_source(self.reference.metadata(), reader, plan)
            .map_err(VerifiedGranularStepError::Source)?;
        let mut probe = [[0.0; 2]; 1];
        let read = reader
            .read_frames_into_with_cancel(0, &mut probe, &mut cancelled)
            .map_err(map_import_error)
            .map_err(VerifiedGranularStepError::Source)?;
        if read != 1 {
            return Err(VerifiedGranularStepError::Source(
                StreamFailureKind::SourceChanged,
            ));
        }
        self.renderer.restore(checkpoint)?;
        if distance == 0 {
            self.seeking = None;
            Ok(GranularSeekProgress::Reached { position: target })
        } else {
            self.seeking = Some(target);
            Ok(GranularSeekProgress::Pending {
                position: self.renderer.position(),
            })
        }
    }

    /// Replay at most `max_steps` grains and discard at most 4096 frames.
    /// A pending or failed call never exposes replay PCM. Failure may leave
    /// progress or poison; callers can inspect position/poison and restart from
    /// an admitted checkpoint. Source scratch may be partially overwritten.
    pub fn continue_seek(
        &mut self,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        buffers: VerifiedGranularBuffers<'_>,
        discard: &mut [Frame],
        max_steps: usize,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<GranularSeekProgress, VerifiedGranularStepError> {
        let target = self
            .seeking
            .ok_or(Error::Invalid("no granular seek is pending"))?;
        if max_steps == 0 || max_steps > MAX_GRANULAR_SEEK_STEPS || discard.is_empty() {
            return Err(Error::Invalid("invalid granular seek work budget").into());
        }
        validate_source(self.reference.metadata(), reader, plan)
            .map_err(VerifiedGranularStepError::Source)?;
        let mut steps = 0;
        let mut discarded = 0;
        while self.renderer.position() < target {
            if cancelled() {
                return Err(VerifiedGranularStepError::Source(
                    StreamFailureKind::Cancelled,
                ));
            }
            if self.renderer.available() > 0 {
                let count = self
                    .renderer
                    .available()
                    .min(discard.len())
                    .min(MAX_DISCARD_FRAMES - discarded)
                    .min(usize::try_from(target - self.renderer.position()).unwrap_or(usize::MAX));
                if count == 0 {
                    break;
                }
                self.renderer.drain(&mut discard[..count])?;
                discarded += count;
            } else {
                if steps == max_steps {
                    break;
                }
                advance_verified_granular_on_control(
                    &mut self.renderer,
                    &self.reference,
                    reader,
                    plan,
                    VerifiedGranularBuffers {
                        source: &mut *buffers.source,
                        cache: &mut *buffers.cache,
                        grain: &mut *buffers.grain,
                    },
                    &mut cancelled,
                )?;
                steps += 1;
            }
        }
        let position = self.renderer.position();
        if position == target {
            self.seeking = None;
            Ok(GranularSeekProgress::Reached { position })
        } else {
            Ok(GranularSeekProgress::Pending { position })
        }
    }
}
