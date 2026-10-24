//! Bounded float WAV source bounce with source loops and create-new publication.
use super::*;

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
        WavEncoding::Float32,
        || {
            let control = ExportControl::default();
            let playback =
                controlled::prepare_bounce_runtime(revision, resources, start, frames, &control)?;
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
/// rendering actual releases until the isolated runtime is quiescent. A hard
/// ten-second release budget is a refusal, never a successful truncated bounce.
/// Nonzero-latency paths refuse until bounce has an origin-aligned capture tap.
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
    control.check()?;
    if revision.arrangement().main_latency_frames() != 0 {
        return Err(ExportError::Invalid(
            "bounce requires a zero-latency device path",
        ));
    }
    if frames == 0 || frames > hex_engine::session_playback::MAX_FINITE_BOUNCE_FRAMES {
        return Err(ExportError::Invalid("bounce exceeds its frame budget"));
    }
    let rate = revision.arrangement().sample_rate();
    let tail_budget = if include_tails {
        u64::from(rate)
            .checked_mul(10)
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
    let mut playback =
        controlled::prepare_bounce_runtime(revision, resources, start, maximum, control)?;
    let mut audio = Vec::new();
    audio
        .try_reserve_exact(
            usize::try_from(maximum)
                .map_err(|_| ExportError::Invalid("bounce allocation overflows"))?,
        )
        .map_err(|_| ExportError::Invalid("could not reserve bounce samples"))?;
    let mut block = [[0.0; 2]; FRAMES_PER_BLOCK];
    let mut rendered = 0;
    loop {
        control.check()?;
        let boundary = if rendered < frames { frames } else { maximum };
        let count = (boundary - rendered).min(FRAMES_PER_BLOCK as u64) as usize;
        if count != 0 {
            playback
                .render(&mut block[..count])
                .map_err(ExportError::SessionRender)?;
            if block[..count].iter().flatten().any(|s| !s.is_finite()) {
                return Err(ExportError::Invalid("bounce produced a non-finite sample"));
            }
            audio.extend_from_slice(&block[..count]);
            rendered += count as u64;
            control.advance(count);
        }
        if rendered >= frames && (!include_tails || playback.arrangement_bounce_is_quiescent()) {
            break;
        }
        if rendered == maximum {
            return Err(ExportError::Invalid(
                "bounce tail did not become quiescent within ten seconds · no edit applied",
            ));
        }
    }
    drop(playback);
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
        fn before_publication(&self) -> Result<(), ExportError> {
            self.control.check()
        }
    }
    publish_encoded(
        path,
        rate,
        start,
        rendered,
        &Publication::CreateNew,
        WavEncoding::Float32,
        || {
            Ok(Samples {
                audio: &audio,
                cursor: &mut cursor,
                control,
            })
        },
    )
}
