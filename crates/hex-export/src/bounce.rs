//! Float WAV bounce using the canonical linear runtime and create-new publication.
use super::*;

/// Render an isolated source revision to lossless float32 PCM. Preroll chases
/// mutable instrument history at nonzero placements; no mix clamp/quantization.
/// Callers bound the source range before invoking this control-worker operation.
pub fn render_bounce_new(
    path: &Path,
    revision: Arc<PreparedRevision>,
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
            let playback = controlled::prepare_runtime(revision, start, frames, &control)?;
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
