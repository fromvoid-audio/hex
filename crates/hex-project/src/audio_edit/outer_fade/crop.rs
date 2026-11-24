//! Explicit edge edits anchor the destination fade at the new clip corners.
//! Retained intervals keep the original fade position across section and grouping edits.
use super::{AudioClipFadeClock, AudioDomainClock};
use crate::{Clip, ClipFade, Invalid, TempoMap};
use std::sync::Arc;

pub(crate) fn retain_outer_fade_crop_context(
    original: &Clip,
    result: &mut Clip,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    if original.projected_outer_fade(tempo)?.is_none() {
        return Ok(());
    }
    let old = original
        .audio_edit()
        .ok_or(Invalid("outer crop source domain is missing"))?;
    let next = result
        .audio_edit()
        .ok_or(Invalid("outer crop result lost its retained domain"))?;
    if original.asset() != result.asset()
        || next.domain.fade_clock != AudioClipFadeClock::Outer
        || matches!(old.domain.clock, AudioDomainClock::Frames(_))
            != matches!(next.domain.clock, AudioDomainClock::Frames(_))
    {
        return Err(Invalid("outer crop changed source or clock kind"));
    }
    let reanchored = old.domain.fade.reanchor_to_corners(result.duration())?;
    let authored = result.with_authored_outer_fade(tempo, reanchored)?;
    let authored_domain = &authored
        .audio_edit()
        .ok_or(Invalid("outer crop result lost its retained domain"))?
        .domain;
    let edit = result
        .audio_edit_mut()
        .ok_or(Invalid("outer crop result lost its retained domain"))?;
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade = authored_domain.fade;
    domain.fade_start = authored_domain.fade_start;
    domain.fade_projection = authored_domain.fade_projection.clone();
    result.fade = authored.fade;
    Ok(())
}

/// The crop is inside the validated source interval. Keep the authored fade
/// and advance only its local reading position, independent of source phase.
pub(in crate::audio_edit) fn retain_source_offset(
    original: &Clip,
    result: &mut Clip,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    let Some(projected) = original.projected_outer_fade(tempo)? else {
        return Ok(());
    };
    let elapsed = projected.view.elapsed_at_frame(result.start())?;
    let duration = result.duration();
    let edit = result
        .audio_edit_mut()
        .ok_or(Invalid("retained fade lost its audio domain"))?;
    let domain = Arc::make_mut(&mut edit.domain);
    match domain.clock {
        AudioDomainClock::Frames(_) => {
            let offset = domain
                .fade
                .offset()
                .checked_add(elapsed.saturating_sub(domain.fade_start))
                .ok_or(Invalid("retained fade offset overflow"))?;
            domain.fade_start = domain.fade_start.saturating_sub(elapsed);
            domain.fade = padded_frame_fade(domain.fade, offset, domain.fade_start, duration)?;
        }
        AudioDomainClock::Beats(_) => domain.shift_outer_fade_displacement(i128::from(elapsed))?,
    }
    Ok(())
}

fn padded_frame_fade(
    original: ClipFade,
    offset: u64,
    prefix: u64,
    duration: u64,
) -> Result<ClipFade, Invalid> {
    // Preserve a valid latent envelope even when the visible clip is all prefix.
    let visible = (duration - prefix.min(duration)).max(1);
    let required = offset
        .checked_add(visible)
        .ok_or(Invalid("outer fade crop span overflow"))?;
    let span = original.span().max(required);
    let padding = span - original.span();
    // Padding a neutral law must keep unity. For a nonneutral law, increasing
    // span and out-end offset equally keeps its original zero tail unchanged.
    let out_end = if original.is_neutral() {
        original.out_end_offset()
    } else {
        original
            .out_end_offset()
            .checked_add(padding)
            .ok_or(Invalid("outer fade silent padding overflow"))?
    };
    ClipFade::new(span, offset, original.fade_in(), original.fade_out())?
        .with_shape(
            original.in_start(),
            out_end,
            original.in_curve(),
            original.out_curve(),
        )?
        .with_curve_positions(original.in_position(), original.out_position())
        .map(|fade| fade.with_curve_laws(original.curve_laws()))
}
