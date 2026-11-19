//! Bounded float WAV source bounce with source loops and create-new publication.
use super::*;

/// Maximum release tail after the selected clip span, in seconds. Session
/// preparation reserves the same allowance before rendering or adopting media.
/// Unfinished tails are truncated at this cap; exact-range bounces add no tail.
pub const MAX_ARRANGEMENT_BOUNCE_TAIL_SECONDS: u64 = 30;

// -80 dBFS peak per channel is a conservative offline trimming policy, not a
// runtime quiescence claim or a universal threshold of human hearing.
const BOUNCE_TAIL_SILENCE_PEAK: f32 = 0.0001;

#[cfg(all(test, feature = "instrument-test-support"))]
#[path = "bounce/partition_tests.rs"]
mod partition_tests;

/// Render an isolated source revision to lossless float32 PCM. Preroll chases
/// mutable instrument history at nonzero placements; no mix clamp/quantization.
/// Source loops retain phase; transport wrapping and Session commands are disabled.
/// Output is limited to `hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES`.
pub fn render_bounce_new(
    path: &Path,
    revision: Arc<PreparedRevision>,
    start: u64,
    frames: u64,
    source_gain: f32,
) -> Result<ExportReport, ExportError> {
    render_bounce_using(path, revision, None, start, frames, source_gain)
}

/// Float32 source bounce through the ordinary scoped renderer and bounded preroll.
/// The caller supplies an isolated admitted revision and pins its exact resource set. No monitor
/// inputs are captured, and source gain remains a final unclamped float multiplication.
pub fn render_bounce_new_with_device_resources(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: &PreparedDeviceResources,
    start: u64,
    frames: u64,
    source_gain: f32,
) -> Result<ExportReport, ExportError> {
    render_bounce_using(path, revision, Some(resources), start, frames, source_gain)
}
fn render_bounce_using(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    source_gain: f32,
) -> Result<ExportReport, ExportError> {
    if !source_gain.is_finite() || source_gain <= 0.0 {
        return Err(ExportError::Invalid("invalid bounce source gain"));
    }
    let rate = revision.arrangement().sample_rate();
    publish_encoded(
        path,
        rate,
        start,
        frames,
        &Publication::CreateNew,
        ExportFormat::Float32,
        || {
            let control = ExportControl::default();
            let playback = controlled::prepare_bounce_runtime(
                revision, resources, start, frames, &control, None,
            )?;
            Ok(BounceRenderer(playback, source_gain))
        },
    )
}
struct BounceRenderer(SessionPlayback, f32);
impl BlockRenderer for BounceRenderer {
    fn render_block(&mut self, frames: &mut [hex_engine::Frame]) -> Result<(), ExportError> {
        self.0.render(frames).map_err(ExportError::SessionRender)?;
        for frame in frames {
            for sample in frame {
                *sample *= self.1;
            }
        }
        Ok(())
    }
}

/// Selected-track bounce, post-device and before the retained mixer. Exact
/// ranges discard causal preroll then write exactly `frames`; clip mode keeps
/// rendering actual releases until the isolated runtime is quiescent, up to
/// `MAX_ARRANGEMENT_BOUNCE_TAIL_SECONDS` after the selected span. A tail still
/// sounding at that cap is truncated and published successfully without a fade.
/// Final tail length ignores peaks at/below -80 dBFS in both channels, keeping
/// up to 50 ms after the last louder tail frame. The selected span and retained PCM
/// are unchanged. Render through quiescence or the cap before trimming, so a
/// quiet gap cannot hide a later audible echo; noise can still cost full rendering.
/// Discard the isolated output path's declared latency before capturing the
/// selected span. Tail completion also drains any pending compensation delay.
/// The control may cancel rendering/publication; the session seals it only at
/// the final model-adoption boundary, after all files are prepared.
#[allow(clippy::too_many_arguments)]
pub fn render_arrangement_bounce_new(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    include_tails: bool,
    control: &ExportControl,
) -> Result<ExportReport, ExportError> {
    render_arrangement_bounce_with_pages(
        path,
        revision,
        resources,
        start,
        frames,
        include_tails,
        control,
        None,
    )
}

/// As [`render_arrangement_bounce_new`], with an optional caller-owned offline
/// page provider for file-backed (streamed) sources. Each block is prepared
/// before any runtime mutation and completed after it, so a page miss fails the
/// job instead of rendering silence. `finish` runs after the last block and before
/// any publication; the provider's resources are its own and stay with the caller.
/// Without a provider, a streamed source refuses as before.
#[allow(clippy::too_many_arguments)] // Same bounce contract plus one borrowed provider.
pub fn render_arrangement_bounce_with_pages(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    include_tails: bool,
    control: &ExportControl,
    pages: Option<&mut dyn PreparedExportPages>,
) -> Result<ExportReport, ExportError> {
    render_arrangement_bounce_partitioned(
        path,
        revision,
        resources,
        start,
        frames,
        include_tails,
        control,
        RenderInputs {
            pages,
            foreign: None,
        },
        FRAMES_PER_BLOCK,
    )
}

/// A private, independently prepared foreign processor set for one bounce.
/// The host owns its instances and confirms each complete render block. An
/// error aborts the job before its WAV or model edit can be published.
pub trait ForeignBounceRenderer {
    fn max_block_frames(&self) -> usize;
    /// True only when the processor's declared finite tail allowance has
    /// elapsed since the selected span ended and the engine released its
    /// source gates. Unknown tails retain the existing finite render cap.
    fn tail_is_drained(&self, _quiescent_source_frames: u64) -> bool {
        false
    }
    fn render_block(
        &mut self,
        playback: &mut SessionPlayback,
        output: &mut [hex_engine::Frame],
        pages: Option<&mut dyn hex_engine::MediaPages>,
        control: &ExportControl,
    ) -> Result<(), ExportError>;
    /// Complete the provider before WAV publication. Drop must also clean
    /// its private instance on cancellation or any earlier refusal.
    fn finish(&mut self, control: &ExportControl) -> Result<(), ExportError>;
}

/// Run the same finite bounce with a caller-owned offline processor set.
#[allow(clippy::too_many_arguments)]
pub fn render_arrangement_bounce_with_foreign(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    include_tails: bool,
    control: &ExportControl,
    pages: Option<&mut dyn PreparedExportPages>,
    foreign: &mut dyn ForeignBounceRenderer,
) -> Result<ExportReport, ExportError> {
    let block_frames = foreign.max_block_frames().min(FRAMES_PER_BLOCK);
    render_arrangement_bounce_partitioned(
        path,
        revision,
        resources,
        start,
        frames,
        include_tails,
        control,
        RenderInputs {
            pages,
            foreign: Some(foreign),
        },
        block_frames,
    )
}

pub(super) struct RenderInputs<'p, 'pages, 'f> {
    pub(super) pages: Option<&'p mut (dyn PreparedExportPages + 'pages)>,
    pub(super) foreign: Option<&'f mut dyn ForeignBounceRenderer>,
}
impl RenderInputs<'_, '_, '_> {
    pub(super) fn render(
        &mut self,
        playback: &mut SessionPlayback,
        output: &mut [hex_engine::Frame],
        control: &ExportControl,
    ) -> Result<(), ExportError> {
        if let Some(pages) = self.pages.as_deref_mut() {
            pages.prepare_chunk(playback, output.len(), control)?;
            let mut view = crate::pages::PageView(pages);
            if let Some(foreign) = self.foreign.as_deref_mut() {
                foreign.render_block(playback, output, Some(&mut view), control)?;
            } else {
                playback
                    .render_with_pages(output, &mut view)
                    .map_err(ExportError::SessionRender)?;
            }
            pages.complete_chunk()
        } else if let Some(foreign) = self.foreign.as_deref_mut() {
            foreign.render_block(playback, output, None, control)
        } else {
            playback.render(output).map_err(ExportError::SessionRender)
        }
    }
}

#[allow(clippy::too_many_arguments)] // Internal partition used to verify block-boundary invariance.
fn render_arrangement_bounce_partitioned(
    path: &Path,
    revision: Arc<PreparedRevision>,
    resources: Option<&PreparedDeviceResources>,
    start: u64,
    frames: u64,
    include_tails: bool,
    control: &ExportControl,
    mut inputs: RenderInputs<'_, '_, '_>,
    block_frames: usize,
) -> Result<ExportReport, ExportError> {
    if block_frames == 0 || block_frames > FRAMES_PER_BLOCK {
        return Err(ExportError::Invalid("invalid bounce block size"));
    }
    control.check()?;
    let latency = u64::from(revision.arrangement().main_latency_frames());
    if frames == 0 || frames > hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES {
        return Err(ExportError::Invalid("bounce exceeds its frame budget"));
    }
    let rate = revision.arrangement().sample_rate();
    let tail_budget = if include_tails {
        u64::from(rate)
            .checked_mul(MAX_ARRANGEMENT_BOUNCE_TAIL_SECONDS)
            .ok_or(ExportError::Invalid("bounce tail budget overflows"))?
    } else {
        0
    };
    let maximum = frames
        .checked_add(tail_budget)
        .filter(|n| *n <= hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES)
        .ok_or(ExportError::Invalid(
            "bounce including tail allowance exceeds its frame budget",
        ))?;
    let rendered_maximum = maximum
        .checked_add(latency)
        .filter(|n| *n <= hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES)
        .ok_or(ExportError::Invalid(
            "compensated bounce exceeds its frame budget",
        ))?;
    start
        .checked_add(rendered_maximum)
        .ok_or(ExportError::Invalid("compensated bounce range overflows"))?;
    let mut playback = controlled::prepare_bounce_runtime_with_inputs(
        revision,
        resources,
        start,
        rendered_maximum,
        control,
        &mut inputs,
        block_frames,
    )?;
    let mut audio = Vec::new();
    audio
        .try_reserve_exact(
            usize::try_from(maximum)
                .map_err(|_| ExportError::Invalid("bounce allocation overflows"))?,
        )
        .map_err(|_| ExportError::Invalid("could not reserve bounce samples"))?;
    let mut block = [[0.0; 2]; FRAMES_PER_BLOCK];
    let mut remaining_latency = latency;
    while remaining_latency != 0 {
        control.check()?;
        let count = remaining_latency.min(block_frames as u64) as usize;
        inputs.render(&mut playback, &mut block[..count], control)?;
        if block[..count].iter().flatten().any(|s| !s.is_finite()) {
            return Err(ExportError::Invalid("bounce produced a non-finite sample"));
        }
        remaining_latency -= count as u64;
        control.advance(count);
    }
    let mut rendered = 0;
    let mut quiet_frames = 0_u64;
    let mut tail_quiescent = false;
    let mut quiet_source_at = None;
    let mut quiet_start = None;
    let mut last_audible_block_end = frames;
    let stopped_quiescent = loop {
        control.check()?;
        let boundary = if rendered < frames { frames } else { maximum };
        let count = (boundary - rendered).min(block_frames as u64) as usize;
        if count != 0 {
            inputs.render(&mut playback, &mut block[..count], control)?;
            if block[..count].iter().flatten().any(|s| !s.is_finite()) {
                return Err(ExportError::Invalid("bounce produced a non-finite sample"));
            }
            audio.extend_from_slice(&block[..count]);
            rendered += count as u64;
            if rendered > frames
                && block[..count]
                    .iter()
                    .flatten()
                    .any(|s| s.abs() > BOUNCE_TAIL_SILENCE_PEAK)
            {
                last_audible_block_end = rendered;
            }
            control.advance(count);
        }
        if rendered >= frames && include_tails {
            let was_quiescent = tail_quiescent;
            let source_quiescent = playback.arrangement_bounce_is_quiescent();
            if !source_quiescent {
                quiet_source_at = None;
            }
            tail_quiescent = source_quiescent
                && inputs.foreign.as_ref().is_none_or(|foreign| {
                    let since = rendered - *quiet_source_at.get_or_insert(rendered);
                    foreign.tail_is_drained(since)
                        && block[..count]
                            .iter()
                            .flatten()
                            .all(|sample| sample.abs() <= BOUNCE_TAIL_SILENCE_PEAK)
                });
            if !tail_quiescent {
                quiet_start = None;
            } else if !was_quiescent {
                quiet_start = Some(rendered);
            }
            // Compensation edges can still hold samples after processors first
            // become idle. Drain a complete path latency after that observation.
            quiet_frames = if tail_quiescent && was_quiescent {
                quiet_frames.saturating_add(count as u64)
            } else {
                0
            };
        }
        let stopped_quiescent = include_tails && tail_quiescent && quiet_frames >= latency;
        if rendered == maximum || (rendered >= frames && (!include_tails || stopped_quiescent)) {
            break stopped_quiescent;
        }
    };
    drop(playback);
    // Verify and release the provider before anything is published.
    if let Some(foreign) = inputs.foreign {
        foreign.finish(control)?;
    }
    if let Some(pages) = inputs.pages {
        pages.finish(control)?;
    }
    if include_tails {
        // Extra latency-drain blocks only verify that hidden compensation
        // buffers are empty. Do not turn their silence into additional padding.
        if stopped_quiescent && let Some(start) = quiet_start {
            let limit = start.max(last_audible_block_end).min(rendered);
            audio.truncate(
                usize::try_from(limit)
                    .map_err(|_| ExportError::Invalid("bounce sample offset overflows"))?,
            );
        }
        let selected = usize::try_from(frames)
            .map_err(|_| ExportError::Invalid("bounce sample offset overflows"))?;
        let tail = audio
            .get(selected..)
            .ok_or(ExportError::Invalid("bounce samples are incomplete"))?;
        let last_audible = tail.iter().rposition(|frame| {
            frame
                .iter()
                .any(|sample| sample.abs() > BOUNCE_TAIL_SILENCE_PEAK)
        });
        let end = if let Some(last) = last_audible {
            // Round the 50 ms padding up to a whole frame, bounded by audio
            // already rendered. This never extends the original 30-second cap.
            let padding = usize::try_from(rate.div_ceil(20))
                .map_err(|_| ExportError::Invalid("bounce tail padding overflows"))?;
            (selected + last + 1)
                .saturating_add(padding)
                .min(audio.len())
        } else {
            selected
        };
        audio.truncate(end);
        rendered =
            u64::try_from(end).map_err(|_| ExportError::Invalid("bounce frame count overflows"))?;
    }
    let mut cursor = 0;
    struct Samples<'a> {
        audio: &'a [hex_engine::Frame],
        cursor: &'a mut usize,
        control: &'a ExportControl,
    }
    impl BlockRenderer for Samples<'_> {
        fn render_block(&mut self, output: &mut [hex_engine::Frame]) -> Result<(), ExportError> {
            self.control.check()?;
            let end = self
                .cursor
                .checked_add(output.len())
                .ok_or(ExportError::Invalid("bounce sample offset overflows"))?;
            output.copy_from_slice(
                self.audio
                    .get(*self.cursor..end)
                    .ok_or(ExportError::Invalid("bounce samples are incomplete"))?,
            );
            *self.cursor = end;
            Ok(())
        }
        fn before_publication(&mut self) -> Result<(), ExportError> {
            self.control.check()
        }
    }
    publish_encoded(
        path,
        rate,
        start,
        rendered,
        &Publication::CreateNew,
        ExportFormat::Float32,
        || {
            Ok(Samples {
                audio: &audio,
                cursor: &mut cursor,
                control,
            })
        },
    )
}
