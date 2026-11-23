//! One original-to-result context transfer after retained source trimming.
//! Reanchors the clip's Outer fade strictly to the resulting clip's corners so
//! trimmed and split clips never carry out-of-range offsets, oversized spans,
//! or mid-clip endpoints.
use super::{AudioClipFadeClock, AudioDomainClock};
use crate::{Clip, Invalid, TempoMap};
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
