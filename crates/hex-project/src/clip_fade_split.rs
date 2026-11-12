//! Arrangement cuts that keep every clip fade handle inside its own part.
//!
//! An ordinary one-shot clip whose local fade the cut does not cross splits into
//! two parts whose fades are reanchored to each part: the left keeps the fade
//! in, the right keeps the fade out. When the cut passes through a ramp, the
//! clip is captured as an audio event root first; the crossed ramp moves onto
//! the root's single event fade in the original root clock, which both parts
//! crop, and each part keeps only its uncrossed ramp as its clip fade. Ramps are
//! never duplicated, so the rendered product is unchanged.
//!
//! Looped, processed (warped/musical/event) clips, cuts inside a one-shot's
//! silent tail, already split fades whose
//! domain extends beyond the clip, and roots longer than the audio domain limit
//! keep the retained-domain split law from [`crate::edit::split_clip_at_tempo`].
//! Their fades also sound unchanged; only the handle may lie outside the part.
use crate::{
    AudioClipEdit, AudioEventFade, AudioEvents, Clip, ClipFade, ClipId, Invalid, TempoMap,
    audio_edit::{MAX_AUDIO_DOMAIN_FRAMES, project_clip, set_clip_fade_envelope},
    edit::{cuts_silent_tail, split_clip_at, split_clip_at_tempo},
};
use std::sync::Arc;

pub(crate) fn split(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
    tempo: &TempoMap,
) -> Result<(Clip, Clip), Invalid> {
    let fade = original.fade;
    let duration = original.duration;
    if original.edit.is_some()
        || original.looping
        || fade.is_neutral()
        || fade.offset() != 0
        || fade.span() != duration
        || duration > MAX_AUDIO_DOMAIN_FRAMES
        || cuts_silent_tail(original, frame)
    {
        return split_clip_at_tempo(original, frame, right_id, tempo);
    }
    // The ordinary split validates the cursor and source before any capture.
    let (mut left, mut right) = split_clip_at(original, frame, right_id)?;
    let cut = left.duration;
    let in_end = fade.in_start() + fade.fade_in();
    let out_start = duration - fade.out_end_offset() - fade.fade_out();
    // A ramp, with its silent endpoint, is local to one part only when the cut
    // leaves it whole and the part keeps at least one audible-domain frame.
    let in_cut = in_end != 0 && (in_end > cut || fade.in_start() >= cut);
    let out_cut =
        out_start != duration && (out_start < cut || duration - fade.out_end_offset() <= cut);
    if !in_cut && !out_cut {
        left.fade = ramps(fade, cut, true, false)?;
        right.fade = ramps(fade, duration - cut, false, true)?;
        return Ok((left, right));
    }
    let mut edit = AudioClipEdit::capture(original);
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade = ClipFade::neutral(duration)?;
    let mut events = AudioEvents::identity(duration);
    for event in &mut events.events {
        event.fade = Some(AudioEventFade::new(
            0,
            ramps(fade, duration, in_cut, out_cut)?,
        )?);
    }
    domain.events = Some(Arc::new(events));
    let mut root = original.clone();
    root.edit = Some(Arc::new(edit));
    project_clip(&mut root, tempo)?;
    let (mut left, mut right) = split_clip_at_tempo(&root, frame, right_id, tempo)?;
    let left_fade = ramps(fade, left.duration, !in_cut, false)?;
    set_clip_fade_envelope(&mut left, left_fade, tempo)?;
    let right_fade = ramps(fade, right.duration, false, !out_cut)?;
    set_clip_fade_envelope(&mut right, right_fade, tempo)?;
    Ok((left, right))
}

/// `fade`'s selected ramps, each with its silent endpoint, curve and position,
/// anchored to a domain of `span` frames measured from the matching edge.
fn ramps(fade: ClipFade, span: u64, fade_in: bool, fade_out: bool) -> Result<ClipFade, Invalid> {
    let pick = |keep: bool, value| if keep { value } else { 0 };
    let [in_law, out_law] = fade.curve_laws();
    Ok(ClipFade::new(
        span,
        0,
        pick(fade_in, fade.fade_in()),
        pick(fade_out, fade.fade_out()),
    )?
    .with_shape(
        pick(fade_in, fade.in_start()),
        pick(fade_out, fade.out_end_offset()),
        if fade_in { fade.in_curve() } else { 0 },
        if fade_out { fade.out_curve() } else { 0 },
    )?
    .with_curve_positions(
        if fade_in { fade.in_position() } else { 500 },
        if fade_out { fade.out_position() } else { 500 },
    )?
    .with_curve_laws([
        if fade_in {
            in_law
        } else {
            crate::FadeCurveLaw::Linear
        },
        if fade_out {
            out_law
        } else {
            crate::FadeCurveLaw::Linear
        },
    ]))
}

#[cfg(test)]
#[path = "clip_fade_split_tests.rs"]
mod tests;
