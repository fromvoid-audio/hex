//! Control-worker WAV windows for incremental browser audition.
//!
//! The reader pins one descriptor and retains the resampler's global phase. Each
//! call reads at most one bounded output window and its bounded source halo;
//! cancellation is checked before and after that work. No PCM is published to
//! the callback here. File length and modification time are checked against the
//! opened descriptor and path, but are not a fingerprint or atomic snapshot.
use super::{BROWSER_PREVIEW_FILE_LIMIT, BROWSER_PREVIEW_FRAME_LIMIT, PreviewFailure};
use hex_engine::Frame;
use hex_media::{ImportError, ResamplePlan, SourceFileIdentity, WavFrameReader, WavReaderLimits};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Maximum output frames returned in a single window.
pub const BROWSER_PREVIEW_WINDOW_FRAMES: usize = 4_096;
// Bound both retained scratch and the amount of uncancellable work per call.
const MAX_SOURCE_WINDOW_FRAMES: usize = 1_048_576;

/// A single open fixed-frame WAV and its output-rate conversion plan.
///
/// This is a worker-side source, not a playback lease. The caller owns request
/// generation and must discard windows when its request is superseded. `open`
/// returns an error for unsupported files so the existing resident fallback can
/// be selected by the caller. The source must not be used on an audio callback.
pub struct PreviewWindowSource {
    reader: WavFrameReader,
    plan: ResamplePlan,
    scratch: Vec<Frame>,
}

impl PreviewWindowSource {
    /// Open a fixed-frame WAV within the existing browser preview limits.
    pub fn open(
        path: impl AsRef<Path>,
        output_rate: u32,
        cancel: &AtomicBool,
    ) -> Result<Self, PreviewFailure> {
        check_cancel(cancel)?;
        let reader = WavFrameReader::open(
            path,
            WavReaderLimits::new(
                BROWSER_PREVIEW_FILE_LIMIT as u64,
                BROWSER_PREVIEW_FRAME_LIMIT as u64,
                32,
                MAX_SOURCE_WINDOW_FRAMES,
            ),
        )
        .map_err(PreviewFailure::from_import)?;
        let plan = ResamplePlan::new(
            reader.sample_rate(),
            output_rate,
            reader.source_frames(),
            BROWSER_PREVIEW_FRAME_LIMIT as u64,
            BROWSER_PREVIEW_FRAME_LIMIT as u64,
            BROWSER_PREVIEW_WINDOW_FRAMES,
        )
        .map_err(PreviewFailure::from_import)?;
        check_cancel(cancel)?;
        let source = Self {
            reader,
            plan,
            scratch: Vec::new(),
        };
        source.check_path()?;
        Ok(source)
    }

    /// Total output-rate frames, including a potentially short final window.
    pub fn output_frames(&self) -> u64 {
        self.plan.output_frames()
    }

    /// Output sample rate used by every returned window.
    pub fn output_rate(&self) -> u32 {
        self.plan.target_rate()
    }

    /// Read exactly `count` output frames at a global output-frame offset.
    ///
    /// `count` must be nonzero and at most [`BROWSER_PREVIEW_WINDOW_FRAMES`];
    /// a range beyond the output or a halo over the source bound is refused.
    /// Errors and cancellation return no partial PCM.
    pub fn read_window(
        &mut self,
        start: u64,
        count: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<Frame>, PreviewFailure> {
        check_cancel(cancel)?;
        self.check_path()?;
        if count == 0 || count > BROWSER_PREVIEW_WINDOW_FRAMES {
            return Err(PreviewFailure::from_import(ImportError::LimitExceeded(
                "output window frames",
            )));
        }
        let (first, end) = self
            .plan
            .required_source_range(start, count)
            .map_err(PreviewFailure::from_import)?;
        let span = end.checked_sub(first).ok_or_else(|| {
            PreviewFailure::from_import(ImportError::Invalid("source window range"))
        })?;
        let required = usize::try_from(span).map_err(|_| {
            PreviewFailure::from_import(ImportError::LimitExceeded("source window frames"))
        })?;
        if required > MAX_SOURCE_WINDOW_FRAMES {
            return Err(PreviewFailure::from_import(ImportError::LimitExceeded(
                "source window frames",
            )));
        }
        if required > self.scratch.len() {
            self.scratch
                .try_reserve_exact(required - self.scratch.len())
                .map_err(|_| PreviewFailure::from_import(ImportError::AllocationFailed))?;
            self.scratch.resize(required, [0.0; 2]);
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(count)
            .map_err(|_| PreviewFailure::from_import(ImportError::AllocationFailed))?;
        output.resize(count, [0.0; 2]);
        self.plan
            .render_wav_window(
                &mut self.reader,
                start,
                &mut output,
                &mut self.scratch[..required],
            )
            .map_err(PreviewFailure::from_import)?;
        check_cancel(cancel)?;
        self.check_path()?;
        Ok(output)
    }

    fn check_path(&self) -> Result<(), PreviewFailure> {
        let identity =
            SourceFileIdentity::observe(self.reader.path()).map_err(PreviewFailure::from_import)?;
        if !self.reader.matches_file_identity(identity) {
            return Err(PreviewFailure::SourceChanged);
        }
        Ok(())
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), PreviewFailure> {
    if cancel.load(Ordering::Relaxed) {
        Err(PreviewFailure::Cancelled)
    } else {
        Ok(())
    }
}
