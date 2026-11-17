//! Bounded metadata-only affine source retiming, shared by preview and commit.
use super::*;
use crate::{BeatsPreserve, BeatsTail, WarpMode};

fn scale(value: u64, numerator: u64, denominator: u64) -> Result<u64, Invalid> {
    if numerator == 0 || denominator == 0 {
        return Err(Invalid("stretch must retain a positive interval"));
    }
    u64::try_from(
        (u128::from(value) * u128::from(numerator) + u128::from(denominator / 2))
            / u128::from(denominator),
    )
    .map_err(|_| Invalid("stretch exceeds time range"))
}

pub(super) fn retime(
    edit: &mut AudioClipEdit,
    numerator: u64,
    denominator: u64,
    start: u64,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    if numerator == denominator && numerator != 0 {
        return Ok(());
    }
    if edit.domain.musical_event_source.is_some() {
        return Err(Invalid(
            "stretching a separated musical event source is unsupported",
        ));
    }
    let (_, old_frames, _, _) = edit.projected_domain(tempo)?;
    let old_fade = edit.domain.fade;
    let length = scale(edit.domain.clock.length(), numerator, denominator)?;
    if let Some(view) = &mut edit.content_view {
        view.base_view_start = scale(view.base_view_start, numerator, denominator)?;
        view.base_view_end = scale(view.base_view_end, numerator, denominator)?;
        let sign = view.base_start_delta.signum();
        let absolute = u64::try_from(view.base_start_delta.unsigned_abs())
            .map_err(|_| Invalid("audio content stretch displacement overflow"))?;
        view.base_start_delta = i128::from(scale(absolute, numerator, denominator)?) * sign;
        view.window = crate::ContentWindow::new(
            scale(view.window.start, numerator, denominator)?,
            scale(view.window.end, numerator, denominator)?,
            view.window.reversed,
        )?;
        if let Some(p) = &mut view.base_placement {
            p.outer_length = AudioDomainClock::Beats(BeatTime::from_raw(scale(
                p.outer_length.length(),
                numerator,
                denominator,
            )?));
            p.cycle_phase = scale(p.cycle_phase, numerator, denominator)?;
        }
    }
    edit.view_start = scale(edit.view_start, numerator, denominator)?;
    edit.view_end = scale(edit.view_end, numerator, denominator)?;
    if let Some(placement) = &mut edit.placement {
        placement.outer_length = AudioDomainClock::Beats(BeatTime::from_raw(scale(
            placement.outer_length.length(),
            numerator,
            denominator,
        )?));
        placement.cycle_phase = scale(placement.cycle_phase, numerator, denominator)?;
        if placement.mode == AudioViewMode::Repeat
            && placement.cycle_phase >= edit.view_end - edit.view_start
        {
            return Err(Invalid("stretched cycle phase exceeds selected interval"));
        }
    }
    edit.timing = AudioClipTiming::Musical(tempo.frame_to_beat(start)?);
    let domain = Arc::make_mut(&mut edit.domain);
    domain.clock = AudioDomainClock::Beats(BeatTime::from_raw(length));
    domain.fade_start = scale(domain.fade_start, numerator, denominator)?;
    if let Some(warp) = &mut domain.warp {
        for marker in &mut warp.markers {
            marker.beat = BeatTime::from_raw(scale(marker.beat.raw(), numerator, denominator)?);
        }
    }
    if let Some(events) = &mut domain.events {
        for event in &mut Arc::make_mut(events).events {
            event.start = scale(event.start, numerator, denominator)?;
            event.end = scale(event.end, numerator, denominator)?;
            event.source_start = scale(event.source_start, numerator, denominator)?;
            event.fade = event
                .fade
                .map(|fade| fade.reproject(|at| scale(at, numerator, denominator)))
                .transpose()?;
        }
    }
    edit.sync_content_length()?;
    let origin = edit.projection_origin()?;
    let (_, frames, _, _) = edit.projected_domain(tempo)?;
    // Callers project the completed clip and apply its source-specific extent
    // limit there, after the retimed fade and source metadata are in place.
    if frames == 0 {
        return Err(Invalid("stretch must retain a positive interval"));
    }
    let domain = Arc::make_mut(&mut edit.domain);
    let fade_scale = |value| scale(value, frames, old_frames);
    let span = fade_scale(old_fade.span())?.max(1);
    domain.fade = ClipFade::new(
        span,
        fade_scale(old_fade.offset())?.min(span - 1),
        fade_scale(old_fade.fade_in())?,
        fade_scale(old_fade.fade_out())?,
    )?
    .with_shape(
        fade_scale(old_fade.in_start())?,
        fade_scale(old_fade.out_end_offset())?,
        old_fade.in_curve(),
        old_fade.out_curve(),
    )?
    .with_curve_positions(old_fade.in_position(), old_fade.out_position())?
    .with_curve_laws(old_fade.curve_laws());
    domain.fade_projection = Some(AudioFadeProjection {
        tempo: tempo.clone(),
        origin_beat_raw: origin + i128::from(domain.fade_start),
        local_start_raw: domain.fade_start,
    });
    Ok(())
}

/// Stretch the visible interval by retiming its entire retained source map.
/// PCM, source bounds, phase, pitch and algorithm remain unchanged. The helper
/// performs bounded metadata work only, and is also used for the final command.
pub fn preview_audio_clip_stretch(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
    asset_frames: u64,
) -> Result<Clip, Invalid> {
    let end = clip
        .start
        .checked_add(clip.duration)
        .ok_or(Invalid("clip end overflow"))?;
    if matches!(edge, TrimEdge::Left) && frame >= end
        || matches!(edge, TrimEdge::Right) && frame <= clip.start
    {
        return Err(Invalid("stretch must retain a positive interval"));
    }
    if (edge == TrimEdge::Left && frame == clip.start) || (edge == TrimEdge::Right && frame == end)
    {
        return Ok(clip.clone());
    }
    let start = if edge == TrimEdge::Left {
        frame
    } else {
        clip.start
    };
    let end = if edge == TrimEdge::Right { frame } else { end };
    let mut result = clip.clone();
    let mut edit = clip
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(clip));
    edit.anchor(clip.start, tempo)?;
    if edit.domain.warp.is_none() {
        let settings = super::default_audio_warp(
            clip,
            tempo,
            WarpMode::Beats {
                preserve: BeatsPreserve::Transients,
                tail: BeatsTail::Forward,
                decay: 100,
            },
        )?;
        Arc::make_mut(&mut edit.domain).warp = Some(settings);
    }
    Arc::make_mut(&mut edit.domain).warp_enabled = true;
    let old_span = edit
        .placement
        .map_or(edit.view_end - edit.view_start, |placement| {
            placement.outer_length.length()
        });
    let new_span = tempo
        .frame_to_beat(end)?
        .raw()
        .checked_sub(tempo.frame_to_beat(start)?.raw())
        .ok_or(Invalid("stretch beat span invalid"))?;
    retime(&mut edit, new_span, old_span, start, tempo)?;
    result.edit = Some(Arc::new(edit));
    result.start = start;
    super::project_clip(&mut result, tempo)?;
    result
        .audio_edit()
        .ok_or(Invalid("stretch edit missing after projection"))?
        .validate(&result, tempo, asset_frames)?;
    Ok(result)
}
