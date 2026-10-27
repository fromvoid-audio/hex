//! Preview the accepted immutable peaks through the prospective musical map.
//! This is a bounded visual projection, never audio preparation on pointer motion.
use super::waveform::{
    SourceRanges, WaveformGrid, WaveformPreview, push_source_range, waveform_canvas,
};
use super::waveform_geometry::{WaveformGeometryCache, WaveformMapping};
use gpui::Div;
use hex_project::{Clip, ClipGain, TempoMap, TrimEdge, preview_audio_clip_stretch};
use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc};

/// Retained, value-comparable metadata makes cache identity cover both maps and
/// the tempo map. It cannot become stale through pointer reuse or root replacement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StretchProjection {
    original: Clip,
    preview: Clip,
    tempo: TempoMap,
    prepared: bool,
}

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
    let asset_frames = asset.map_or(source_end, |source| source.source_frames as u64);
    let expected = preview_audio_clip_stretch(original, edge, frame, tempo, asset_frames).ok()?;
    if &expected != preview {
        return None;
    }
    let source = if let Some(old) = original.audio_edit() {
        let (_, length, offset, _) = old.projected_domain(tempo).ok()?;
        let accepted = prepared?;
        if accepted.offset != offset
            || accepted.length != original.duration()
            || accepted.waveform.source_frames as u64 != length
        {
            return None;
        }
        Arc::clone(&accepted.waveform)
    } else {
        let source = asset?;
        if (source.source_frames as u64) < asset_frames {
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
    ))
}

impl StretchProjection {
    pub(super) fn gain(&self) -> ClipGain {
        self.preview.gain()
    }

    /// Inverse prospective frame -> accepted root/source frame, with Q32 source
    /// interpolation shared with audio preparation. Work is per visible column.
    fn position(&self, timeline: u64) -> Option<u64> {
        let edit = self.preview.audio_edit()?;
        let (origin, _, _, _) = edit.projected_domain(&self.tempo).ok()?;
        let root_frame = u64::try_from(i128::from(timeline) - origin).ok()?;
        let beat = edit.root_position_at(root_frame, &self.tempo).ok()?;
        accepted_root_frame(
            &self.original,
            &self.preview,
            &self.tempo,
            self.prepared,
            beat,
        )
    }

    pub(super) fn ranges(&self, visible: Range<u64>, available: usize) -> SourceRanges {
        let mut ranges = SourceRanges::default();
        let start = visible.start.max(self.preview.start());
        let end = visible
            .end
            .min(self.preview.start().saturating_add(self.preview.duration()));
        if start >= end {
            return ranges;
        }
        let Some(a) = self.position(start) else {
            return ranges;
        };
        let Some(b) = self.position(end) else {
            return ranges;
        };
        // Rounding may collapse a sub-frame column. Include its real source frame.
        let b = b.max(a.saturating_add(1));
        if self.prepared {
            push_source_range(
                &mut ranges,
                a.min(available as u64),
                b.min(available as u64),
                available,
            );
        } else {
            let offset = self.original.source_offset();
            let length = self.original.source_length();
            if length == 0 {
                return ranges;
            }
            if !self.original.looping() {
                push_source_range(
                    &mut ranges,
                    offset + a.min(length),
                    offset + b.min(length),
                    available,
                );
            } else if b - a >= length {
                push_source_range(&mut ranges, offset, offset + length, available);
            } else {
                let first = a % length;
                let span = b - a;
                let count = span.min(length - first);
                push_source_range(
                    &mut ranges,
                    offset + first,
                    offset + first + count,
                    available,
                );
                if count < span {
                    push_source_range(&mut ranges, offset, offset + span - count, available);
                }
            }
        }
        ranges
    }
}

/// Invert only the canonical stretch's root-clock retiming. Placement wrap and
/// visibility belong to AudioViewProjection, not this source-coordinate law.
pub(super) fn accepted_root_frame(
    original: &Clip,
    preview: &Clip,
    tempo: &TempoMap,
    prepared: bool,
    beat: u64,
) -> Option<u64> {
    let edit = preview.audio_edit()?;
    if let Some(old) = original.audio_edit().filter(|e| e.domain.warp_enabled) {
        // The stretch operation retimes all existing beat coordinates together.
        // Preserve the accepted processed signal, including its transients,
        // pitch, source envelope and loop history, rather than using raw PCM.
        let old_position = u64::try_from(
            u128::from(beat) * u128::from(old.domain.clock.length())
                / u128::from(edit.domain.clock.length()),
        )
        .ok()?;
        return old.root_frame_at(old_position, tempo).ok();
    }
    let phase = edit.domain.warp_phase_beat().ok()?;
    let source_q32 = edit
        .domain
        .warp_source_position_q32(u128::from(beat) + u128::from(phase))
        .ok()?;
    let source = u64::try_from(source_q32 >> 32).ok()?;
    if prepared {
        // A warp-off processed root advances through source frames in order.
        // Its accepted peaks already contain pitch/envelope processing. Do
        // not substitute raw peaks and thereby erase that processing.
        Some(source.saturating_sub(edit.domain.source_phase))
    } else {
        Some(source)
    }
}

#[cfg(test)]
mod tests;
