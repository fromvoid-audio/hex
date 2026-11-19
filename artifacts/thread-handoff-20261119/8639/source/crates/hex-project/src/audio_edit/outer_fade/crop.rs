//! One original-to-result context transfer after retained source trimming.
//! This does not admit Outer, author new fade edges or alter source processing.
use super::*;
use std::sync::Arc;

pub(crate) fn retain_crop_context(
    original: &Clip,
    result: &mut Clip,
    tempo: &TempoMap,
) -> Result<(), Invalid> {
    let Some(projected) = original.projected_outer_fade(tempo)? else {
        return Ok(());
    };
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
    // The same inverse owns source-independent destination context for frame,
    // live/frozen musical and directed reads, including negative prefix age.
    let elapsed = projected.view.signed_elapsed_at_frame(result.start())?;
    let duration = result.duration();
    // Compute fallible context first. Failure cannot partly change the result.
    let mut context = old.domain.as_ref().clone();
    match old.domain.clock {
        AudioDomainClock::Frames(_) => {
            let before = i128::from(old.domain.fade.offset()) - i128::from(old.domain.fade_start);
            let next = before
                .checked_add(elapsed)
                .ok_or(Invalid("outer frame crop age overflow"))?;
            let distance = u64::try_from(next.unsigned_abs())
                .map_err(|_| Invalid("outer frame crop age exceeds clock range"))?;
            let (offset, prefix) = if next >= 0 {
                (distance, 0)
            } else {
                (0, distance)
            };
            context.fade = padded_frame_fade(old.domain.fade, offset, prefix, duration)?;
            context.fade_start = prefix;
        }
        AudioDomainClock::Beats(_) => context.shift_outer_fade_displacement(elapsed)?,
    }
    let edit = result
        .audio_edit_mut()
        .ok_or(Invalid("outer crop result lost its retained domain"))?;
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade = context.fade;
    domain.fade_start = context.fade_start;
    domain.fade_projection = context.fade_projection;
    Ok(())
}

fn padded_frame_fade(
    original: ClipFade,
    offset: u64,
    prefix: u64,
    duration: u64,
) -> Result<ClipFade, Invalid> {
    let visible = duration.saturating_sub(prefix).max(1);
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
