//! Resumable assembly of one verified granular output page on a dedicated worker.
use crate::{
    CheckpointCacheError, GranularSeekProgress, StreamFailureKind, VerifiedGranularBuffers,
    VerifiedGranularCheckpointCache, VerifiedGranularCursor, VerifiedGranularStepError,
    VerifiedReferenceChannel, reference::validate_source, worker::map_import_error,
};
use hex_engine::{Frame, STREAM_PAGE_FRAMES, StreamedSourceMetadata};
use hex_media::{ResamplePlan, VerifiedWavReader};
use hex_warp::{Error, SequentialGranular, SequentialGranularConfig};

/// A failure retains the pending page buffer until it is explicitly reclaimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularPageError {
    InvalidRequest,
    Cache(CheckpointCacheError),
    Step(VerifiedGranularStepError),
    Warp(Error),
}
impl From<Error> for GranularPageError {
    fn from(value: Error) -> Self {
        Self::Warp(value)
    }
}
impl From<CheckpointCacheError> for GranularPageError {
    fn from(value: CheckpointCacheError) -> Self {
        Self::Cache(value)
    }
}
impl From<VerifiedGranularStepError> for GranularPageError {
    fn from(value: VerifiedGranularStepError) -> Self {
        Self::Step(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GranularPageProgress {
    Pending,
    Complete,
}

/// A complete page. Only `frames[..valid_frames]` belongs to the source extent.
pub struct CompletedGranularPage {
    pub metadata: StreamedSourceMetadata,
    pub page_index: u64,
    pub frames: Box<[Frame]>,
    pub valid_frames: usize,
}

enum Phase {
    Seeking,
    Rendering,
    Complete,
    Failed,
}
#[derive(Clone, Copy)]
enum SeekPolicy {
    Strict(u64),
    Segmented(u64),
}
struct PendingPage {
    index: u64,
    target: u64,
    policy: SeekPolicy,
    frames: Box<[Frame]>,
    valid: usize,
    written: usize,
    phase: Phase,
}

/// One cursor and checkpoint cache bound to an exact region and configuration.
///
/// Caller owns reader, conversion, scratch, source-generation invalidation and
/// aggregate memory admission. The cache budget excludes page/reader/scratch.
/// This is blocking control-side work: never run it on the audio callback or
/// the sole worker supplying active playback pages. One step performs at most
/// one grain advance and drains/discards at most one page; completing a page may
/// also record one bounded checkpoint and begin the next replay segment. Strict
/// seeks may refuse distant targets; segmented seeks can take substantial time.
/// Neither mode establishes a whole-file snapshot. On failure reclaim the buffer
/// and restart from an admitted checkpoint after resolving the source failure.
pub struct VerifiedGranularPages {
    metadata: StreamedSourceMetadata,
    cursor: VerifiedGranularCursor,
    cache: VerifiedGranularCheckpointCache,
    pending: Option<PendingPage>,
}
impl VerifiedGranularPages {
    pub fn new(
        reference: VerifiedReferenceChannel,
        config: SequentialGranularConfig,
        cache_entries: usize,
        cache_budget: usize,
        cancel: &mut dyn FnMut() -> bool,
    ) -> Result<Self, GranularPageError> {
        let region = reference
            .region()
            .ok_or(GranularPageError::InvalidRequest)?;
        if config.sample_rate != reference.metadata().project_rate()
            || config.reference_channel != reference.channel()
        {
            return Err(GranularPageError::InvalidRequest);
        }
        let metadata = reference
            .metadata()
            .with_granular(
                region,
                &config.map,
                config.mode,
                config.pitch_cents,
                config.resampler,
            )
            .map_err(|_| GranularPageError::InvalidRequest)?;
        let renderer = SequentialGranular::new(config, cancel)?;
        let cursor = VerifiedGranularCursor::new(renderer, reference)?;
        let cache = VerifiedGranularCheckpointCache::new(&cursor, cache_entries, cache_budget)?;
        Ok(Self {
            metadata,
            cursor,
            cache,
            pending: None,
        })
    }
    pub(crate) fn checkpoint_recording_fits(&self) -> bool {
        self.cache
            .minimum_recording_budget()
            .is_some_and(|bytes| bytes <= self.cache.reserved_budget())
    }
    pub(crate) fn reserved_processing_bytes(&self) -> Option<usize> {
        self.cursor
            .backing_bytes()?
            .checked_add(self.cache.reserved_budget())
    }
    pub(crate) fn holds_page(&self) -> bool {
        self.pending.is_some()
    }
    pub fn metadata(&self) -> &StreamedSourceMetadata {
        &self.metadata
    }
    pub fn max_source_frames(&self) -> usize {
        self.cursor.max_source_frames()
    }
    /// Begin one page within a strict total preroll. Refusal returns the buffer.
    pub fn begin_page(
        &mut self,
        index: u64,
        frames: Box<[Frame]>,
        max_preroll_frames: u64,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        cancelled: impl FnMut() -> bool,
    ) -> Result<(), (GranularPageError, Box<[Frame]>)> {
        self.begin_page_with_policy(
            index,
            frames,
            SeekPolicy::Strict(max_preroll_frames),
            reader,
            plan,
            cancelled,
        )
    }
    /// Begin a page with successive bounded replay segments. Total replay time
    /// may be large; the caller must schedule it away from the playback worker.
    /// A positive segment size no greater than `MAX_GRANULAR_PREROLL_FRAMES` is
    /// required. Refusal returns the unchanged buffer. Intermediate checkpoints
    /// consume the existing cache budget and can evict other noninitial entries.
    /// Distant replay needs budget for the pinned initial and one additional
    /// checkpoint; insufficient admission is reported before publishing a page.
    pub fn begin_page_replaying(
        &mut self,
        index: u64,
        frames: Box<[Frame]>,
        segment_frames: u64,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        cancelled: impl FnMut() -> bool,
    ) -> Result<(), (GranularPageError, Box<[Frame]>)> {
        self.begin_page_with_policy(
            index,
            frames,
            SeekPolicy::Segmented(segment_frames),
            reader,
            plan,
            cancelled,
        )
    }
    fn begin_page_with_policy(
        &mut self,
        index: u64,
        mut frames: Box<[Frame]>,
        policy: SeekPolicy,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        cancelled: impl FnMut() -> bool,
    ) -> Result<(), (GranularPageError, Box<[Frame]>)> {
        let Some(start) = index.checked_mul(STREAM_PAGE_FRAMES as u64) else {
            return Err((GranularPageError::InvalidRequest, frames));
        };
        if self.pending.is_some()
            || frames.len() != STREAM_PAGE_FRAMES
            || start >= self.metadata.playback_frames()
        {
            return Err((GranularPageError::InvalidRequest, frames));
        }
        let valid =
            (self.metadata.playback_frames() - start).min(STREAM_PAGE_FRAMES as u64) as usize;
        let seek = match policy {
            SeekPolicy::Strict(max_preroll_frames) => self.cache.begin_seek(
                &mut self.cursor,
                start,
                max_preroll_frames,
                reader,
                plan,
                cancelled,
            ),
            SeekPolicy::Segmented(segment_frames) => self.cache.begin_seek_toward(
                &mut self.cursor,
                start,
                segment_frames,
                reader,
                plan,
                cancelled,
            ),
        };
        let phase = match seek {
            Ok(GranularSeekProgress::Pending { .. }) => Phase::Seeking,
            Ok(GranularSeekProgress::Reached { .. }) => Phase::Rendering,
            Err(error) => return Err((error.into(), frames)),
        };
        frames.fill([0.0; 2]);
        self.pending = Some(PendingPage {
            index,
            target: start,
            policy,
            frames,
            valid,
            written: 0,
            phase,
        });
        Ok(())
    }
    /// Advance one bounded unit. Partial audio stays private until completion.
    pub fn step(
        &mut self,
        reader: &mut VerifiedWavReader,
        plan: &ResamplePlan,
        buffers: VerifiedGranularBuffers<'_>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<GranularPageProgress, GranularPageError> {
        let pending = self
            .pending
            .as_mut()
            .ok_or(GranularPageError::InvalidRequest)?;
        if matches!(pending.phase, Phase::Complete) {
            return Ok(GranularPageProgress::Complete);
        }
        if matches!(pending.phase, Phase::Failed) {
            return Err(GranularPageError::InvalidRequest);
        }
        if cancelled() {
            pending.phase = Phase::Failed;
            return Err(GranularPageError::Step(VerifiedGranularStepError::Source(
                StreamFailureKind::Cancelled,
            )));
        }
        let result = (|| -> Result<GranularPageProgress, GranularPageError> {
            if matches!(pending.phase, Phase::Seeking) {
                let mut discard = [[0.0; 2]; STREAM_PAGE_FRAMES];
                if let GranularSeekProgress::Reached { position } = self.cursor.continue_seek(
                    reader,
                    plan,
                    buffers,
                    &mut discard,
                    1,
                    &mut cancelled,
                )? {
                    if position == pending.target {
                        pending.phase = Phase::Rendering;
                    } else if let SeekPolicy::Segmented(segment_frames) = pending.policy {
                        if position > pending.target {
                            return Err(GranularPageError::InvalidRequest);
                        }
                        self.cache.record(&self.cursor)?;
                        match self.cache.begin_seek_toward(
                            &mut self.cursor,
                            pending.target,
                            segment_frames,
                            reader,
                            plan,
                            &mut cancelled,
                        )? {
                            GranularSeekProgress::Pending { .. } => {}
                            GranularSeekProgress::Reached { position }
                                if position == pending.target =>
                            {
                                pending.phase = Phase::Rendering;
                            }
                            GranularSeekProgress::Reached { .. } => {
                                return Err(GranularPageError::InvalidRequest);
                            }
                        }
                    } else {
                        return Err(GranularPageError::InvalidRequest);
                    }
                }
                return Ok(GranularPageProgress::Pending);
            }
            if self.cursor.available() == 0 {
                if !self.cursor.advance(reader, plan, buffers, &mut cancelled)? {
                    return Err(GranularPageError::InvalidRequest);
                }
                if self.cursor.available() == 0 {
                    return Ok(GranularPageProgress::Pending);
                }
            }
            let count = self.cursor.available().min(pending.valid - pending.written);
            if count == 0 {
                return Err(GranularPageError::InvalidRequest);
            }
            self.cursor
                .drain(&mut pending.frames[pending.written..pending.written + count])?;
            pending.written += count;
            if pending.written == pending.valid {
                validate_source(self.cursor.source_metadata(), reader, plan).map_err(|error| {
                    GranularPageError::Step(VerifiedGranularStepError::Source(error))
                })?;
                let mut probe = [[0.0; 2]; 1];
                let read = reader
                    .read_frames_into_with_cancel(0, &mut probe, &mut cancelled)
                    .map_err(map_import_error)
                    .map_err(|error| {
                        GranularPageError::Step(VerifiedGranularStepError::Source(error))
                    })?;
                if read != 1 {
                    return Err(GranularPageError::Step(VerifiedGranularStepError::Source(
                        StreamFailureKind::SourceChanged,
                    )));
                }
                self.cache.record(&self.cursor)?;
                pending.phase = Phase::Complete;
                Ok(GranularPageProgress::Complete)
            } else {
                Ok(GranularPageProgress::Pending)
            }
        })();
        if result.is_err() {
            pending.phase = Phase::Failed;
        }
        result
    }
    /// Return only a completed page. Incomplete or failed jobs remain owned here.
    pub fn take_complete(&mut self) -> Option<CompletedGranularPage> {
        if !matches!(self.pending.as_ref()?.phase, Phase::Complete) {
            return None;
        }
        let page = self.pending.take()?;
        Some(CompletedGranularPage {
            metadata: self.metadata.clone(),
            page_index: page.index,
            frames: page.frames,
            valid_frames: page.valid,
        })
    }
    /// Cancel or abandon a job and reclaim its buffer; its contents are not ready audio.
    pub fn reclaim(&mut self) -> Option<Box<[Frame]>> {
        self.pending.take().map(|page| page.frames)
    }
}
