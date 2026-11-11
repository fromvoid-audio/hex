//! Preview the accepted immutable peaks through the prospective musical map.
//! This is a bounded visual projection, never audio preparation on pointer motion.
use super::waveform::{WaveformGrid, WaveformPreview, waveform_canvas};
use super::waveform_geometry::{WaveformGeometryCache, WaveformMapping};
use gpui::Div;
use hex_project::{TrimEdge, preview_audio_clip_stretch};
use std::{cell::RefCell, rc::Rc, sync::Arc};

mod projection;
pub(super) use projection::StretchProjection;

pub(super) fn stretch_waveform_view(
    request: WaveformPreview<'_>,
    visible_start: u64,
    visible_end: u64,
    cache: Rc<RefCell<WaveformGeometryCache>>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    if request
        .preview
        .audio_edit()
        .is_some_and(|edit| edit.placement.is_some())
    {
        return super::waveform_loop::loop_waveform_view(
            request,
            visible_start,
            visible_end,
            cache,
            grid,
        );
    }
    let WaveformPreview {
        original,
        preview,
        tempo,
        asset,
        prepared,
        ink,
    } = request;
    // Accept exactly the shared canonical stretch operation; changing pitch,
    // source, processing algorithm or arbitrary roots cannot borrow old peaks.
    let (edge, frame) = if original.start() == preview.start() {
        (
            TrimEdge::Right,
            preview.start().checked_add(preview.duration())?,
        )
    } else {
        (TrimEdge::Left, preview.start())
    };
    let source_end = original
        .source_offset()
        .checked_add(original.source_length())?;
    let asset_frames = asset.map_or(source_end, |source| source.source_frames() as u64);
    let expected = preview_audio_clip_stretch(original, edge, frame, tempo, asset_frames).ok()?;
    if &expected != preview {
        return None;
    }
    let source = if let Some(old) = original.audio_edit() {
        let (_, length, offset, _) = old.projected_domain(tempo).ok()?;
        let accepted = prepared?;
        if accepted.offset != offset
            || accepted.length != original.duration()
            || accepted.waveform.source_frames() as u64 != length
        {
            return None;
        }
        Arc::clone(&accepted.waveform)
    } else {
        let source = asset?;
        if (source.source_frames() as u64) < asset_frames {
            return None;
        }
        Arc::clone(source)
    };
    let projection = Arc::new(StretchProjection {
        original: original.clone(),
        preview: preview.clone(),
        tempo: tempo.clone(),
        prepared: original.audio_edit().is_some(),
    });
    Some(waveform_canvas(
        source,
        WaveformMapping::Stretch(projection),
        visible_start,
        visible_end,
        Some(cache),
        grid,
        ink,
    ))
}

mod accepted_root;
pub(super) use accepted_root::accepted_root_frame;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
