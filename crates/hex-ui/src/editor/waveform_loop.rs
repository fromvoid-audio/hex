//! Bounded visual projection of accepted processing-root peaks through the
//! canonical placement query. This never prepares audio or owns a DSP clock.
use super::waveform::{WaveformGrid, WaveformPreview, waveform_canvas};
use super::waveform_geometry::{WaveformMapping, WaveformViewCache};
use gpui::Div;
use hex_project::{AudioViewResizeOperation, preview_audio_view_resize};
use std::{rc::Rc, sync::Arc};

mod accepted_root;
mod projection;
use accepted_root::{AcceptedLoopSourceRequest, accepted_loop_source};
pub(super) use projection::LoopProjection;

pub(super) fn loop_waveform_view(
    request: WaveformPreview<'_>,
    start: u64,
    end: u64,
    cache: Rc<WaveformViewCache>,
    grid: Option<WaveformGrid>,
) -> Option<Div> {
    request.preview.audio_edit()?.placement?;
    let same_root = request
        .original
        .audio_edit()
        .zip(request.preview.audio_edit())
        .is_some_and(|(a, b)| Arc::ptr_eq(&a.domain, &b.domain));
    let mut retimed = false;
    if !same_root {
        let endpoint = request
            .preview
            .start()
            .checked_add(request.preview.duration())?;
        let asset_frames = request.asset.map_or(
            request
                .original
                .source_offset()
                .checked_add(request.original.source_length())?,
            |source| source.source_frames() as u64,
        );
        let repeated = preview_audio_view_resize(
            request.original,
            request.tempo,
            asset_frames,
            AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: endpoint,
            },
        )
        .ok();
        if repeated.as_ref() != Some(request.preview) {
            let edge = if request.original.start() == request.preview.start() {
                hex_project::TrimEdge::Right
            } else {
                hex_project::TrimEdge::Left
            };
            let frame = if edge == hex_project::TrimEdge::Right {
                endpoint
            } else {
                request.preview.start()
            };
            let stretched = hex_project::preview_audio_clip_stretch(
                request.original,
                edge,
                frame,
                request.tempo,
                asset_frames,
            )
            .ok()?;
            if &stretched != request.preview {
                return None;
            }
            retimed = true;
        }
    }
    let source = accepted_loop_source(AcceptedLoopSourceRequest {
        original: request.original,
        preview: request.preview,
        tempo: request.tempo,
        asset: request.asset,
        prepared: request.prepared,
    })?;
    let projection = Arc::new(LoopProjection {
        original: request.original.clone(),
        preview: request.preview.clone(),
        tempo: request.tempo.clone(),
        prepared: source.prepared,
        accepted_origin: source.origin,
        retimed,
    });
    Some(waveform_canvas(
        source.waveform,
        WaveformMapping::Loop(projection),
        start,
        end,
        Some(cache),
        grid,
        request.ink,
    ))
}
