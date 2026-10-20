//! Affine retiming shared with owner-free scoped previews.
use super::*;
pub(super) fn retime(
    edit: &mut AudioClipEdit,
    numerator: u64,
    denominator: u64,
    start: u64,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    crate::audio_preview::retime_edit(edit, numerator, denominator, start, tempo)
}
/// Materialize the exact plan used by scoped reads; canonical commands still
/// recompute against current metadata and admit the resulting source separately.
pub fn preview_audio_clip_stretch(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
    asset_frames: u64,
) -> Result<Clip, Invalid> {
    crate::audio_preview::materialize(
        clip,
        crate::AudioClipPreviewOperation::Stretch(edge),
        frame,
        asset_frames,
        tempo,
    )
}
