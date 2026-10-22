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
    render_bounce_using(path, revision, None, start, frames, source_gain)
}

/// Float32 bounce through the same ordinary scoped linear/preroll runtime as controlled export.
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
                controlled::prepare_runtime(revision, resources, start, frames, &control)?;
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
