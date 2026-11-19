//! Model-owned lower-handle resize. Shares the ordinary trim's source mapping;
//! only the envelope is explicitly refitted after the source window/crop changes.
use crate::{Clip, ClipFade, Invalid, TempoMap, TrimEdge};

pub(crate) fn preview(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_frames: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    if clip
        .source_offset
        .checked_add(clip.source_length)
        .is_none_or(|end| end > asset_frames)
    {
        return Err(Invalid("clip source interval exceeds media"));
    }
    let end = clip
        .start
        .checked_add(clip.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let original_edge = match edge {
        TrimEdge::Left => clip.start,
        TrimEdge::Right => end,
    };
    // Handle edits already fit inherited/musical fades to local clip frames.
    // Preserve that local edit upper sample, not a prior retained projection.
    let fade = clip.retained_fade().reanchor(clip.duration)?;
    let endpoint_aligned = match edge {
        TrimEdge::Left => fade.in_start() == 0,
        TrimEdge::Right => fade.out_end_offset() == 0,
    };
    if frame == original_edge && endpoint_aligned {
        return Ok(clip.clone());
    }
    let in_top = clip.start + fade.in_start() + fade.fade_in().saturating_sub(1);
    let out_top = end - fade.out_end_offset() - fade.fade_out().max(1);
    // Zero ramps remain neutral and can resize in either direction. Authored
    // ramps retain at least their upper sample; the lower handle cannot cross it.
    let frame = match edge {
        TrimEdge::Left if fade.fade_in() > 0 => frame.min(in_top),
        TrimEdge::Right if fade.fade_out() > 0 => frame.max(out_top + 1),
        _ => frame,
    };
    let frame = if let Some(edit) = clip.audio_edit().filter(|edit| edit.placement.is_none()) {
        let (_, full, start, _) = edit.projected_domain(tempo)?;
        let origin = i128::from(clip.start) - i128::from(start);
        let first = origin.max(0) as u64;
        let last = (origin + i128::from(full)).clamp(0, i128::from(u64::MAX)) as u64;
        match edge {
            TrimEdge::Left => frame.clamp(first, end - 1),
            TrimEdge::Right => frame.clamp(clip.start + 1, last),
        }
    } else {
        frame
    };
    let mut trimmed = clip.preview_edge_trim_with_tempo(edge, frame, asset_frames, tempo)?;
    if trimmed.start == clip.start && trimmed.duration == clip.duration && endpoint_aligned {
        return Ok(clip.clone());
    }
    let fitted = fade.reanchor(trimmed.duration)?;
    let (fade_in, fade_out, in_start, out_end_offset) = match edge {
        TrimEdge::Left => (
            if fade.fade_in() == 0 {
                0
            } else {
                in_top
                    .saturating_sub(trimmed.start)
                    .saturating_add(1)
                    .min(trimmed.duration)
            },
            fitted.fade_out(),
            0,
            fitted.out_end_offset(),
        ),
        TrimEdge::Right => (
            fitted.fade_in(),
            if fade.fade_out() == 0 {
                0
            } else {
                (trimmed.start + trimmed.duration)
                    .saturating_sub(out_top)
                    .min(trimmed.duration)
            },
            fitted.in_start(),
            0,
        ),
    };
    let next = ClipFade::new(trimmed.duration, 0, fade_in, fade_out)?
        .with_shape(in_start, out_end_offset, fade.in_curve(), fade.out_curve())?
        .with_curve_positions(fade.in_position(), fade.out_position())?
        .with_curve_laws(fade.curve_laws());
    if trimmed.audio_edit().is_some() {
        crate::audio_edit::set_clip_fade_envelope(&mut trimmed, next, tempo)?;
    } else {
        trimmed.fade = next;
    }
    if let Some(edit) = trimmed.audio_edit() {
        edit.validate(&trimmed, tempo, asset_frames)?;
    }
    Ok(trimmed)
}
