//! The lower-handle command materializes the same plan used by scoped reads.
use crate::{Clip, Invalid, TempoMap, TrimEdge};
pub(crate) fn preview(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_frames: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    crate::audio_preview::materialize(
        clip,
        crate::AudioClipPreviewOperation::FadeTrim(edge),
        frame,
        asset_frames,
        tempo,
    )
}
