//! One full validation per preview, not per waveform column. Only the bounded
//! event identity pass uses scratch: 1024 * (u64, usize) = 16 KiB on arm64.
use super::*;
use crate::{MAX_AUDIO_POINTS, MAX_SOURCE_GAIN};
pub(super) fn validate(
    plan: Plan,
    clip: &Clip,
    tempo: &TempoMap,
    asset_frames: u64,
) -> Result<(), Invalid> {
    validate_with_source(plan, clip, clip.audio_edit(), tempo, asset_frames)
}
pub(super) fn validate_with_source(
    plan: Plan,
    clip: &Clip,
    source: Option<&AudioClipEdit>,
    tempo: &TempoMap,
    asset_frames: u64,
) -> Result<(), Invalid> {
    let edit = plan.edit.ok_or(Invalid("audio domain missing"))?;
    if let Some(events) = source.and_then(|e| e.domain.events.as_ref()) {
        events.validate_transformed(edit.clock.length(), |event| {
            edit.retime.map_or(Ok(event), |s| s.event(event))
        })?;
    }
    let domain = source.map(|e| e.domain.as_ref());
    let offset = domain.map_or(clip.source_offset, |d| d.source_offset);
    let length = domain.map_or(clip.source_length, |d| d.source_length);
    let phase = domain.map_or(clip.source_phase, |d| d.source_phase);
    let looping = domain.map_or(clip.looping, |d| d.looping);
    let source_end = offset
        .checked_add(length)
        .ok_or(Invalid("audio domain source end overflow"))?;
    if length == 0
        || source_end > asset_frames
        || (looping && phase >= length)
        || (!looping && phase != 0)
    {
        return Err(Invalid("audio domain source interval or phase is invalid"));
    }
    if !(-4800..=4800).contains(&domain.map_or(0, |d| d.pitch_cents)) {
        return Err(Invalid("audio pitch must be within -4800..4800 cents"));
    }
    if edit.view_start >= edit.view_end || edit.view_end > edit.clock.length() {
        return Err(Invalid("audio crop must be inside its retained domain"));
    }
    if edit.fade_start >= edit.clock.length() {
        return Err(Invalid("audio fade starts outside its domain"));
    }
    if matches!(edit.clock, AudioDomainClock::Beats(_)) != edit.musical_start.is_some() {
        return Err(Invalid("audio clock and placement disagree"));
    }
    let gain = domain.map_or(&[][..], |d| d.source_gain.as_slice());
    if gain.len() > MAX_AUDIO_POINTS
        || gain
            .iter()
            .any(|p| p.frame > asset_frames || p.gain_q16 > MAX_SOURCE_GAIN)
        || gain.windows(2).any(|pair| pair[0].frame >= pair[1].frame)
    {
        return Err(Invalid(
            "source gain points must be bounded, sorted and within media",
        ));
    }
    let has_warp =
        matches!(edit.warp, WarpPlan::Default { .. }) || domain.is_some_and(|d| d.warp.is_some());
    if has_warp {
        if edit.warp_enabled && edit.musical_start.is_none() {
            return Err(Invalid("warp requires musical placement"));
        }
        let warp = edit.warp_read(source)?;
        warp.mode.validate()?;
        let mut invalid = !(2..=MAX_AUDIO_POINTS).contains(&warp.len());
        if !invalid {
            let first = warp.marker(0)?;
            let last = warp.marker(warp.len() - 1)?;
            invalid = first.source_frame != offset
                || first.beat != BeatTime::ZERO
                || last.source_frame != source_end
                || last.beat == BeatTime::ZERO;
            if !invalid {
                for index in 1..warp.len() {
                    let a = warp.marker(index - 1)?;
                    let b = warp.marker(index)?;
                    if a.source_frame >= b.source_frame || a.beat >= b.beat {
                        invalid = true;
                        break;
                    }
                }
            }
        }
        if invalid {
            return Err(Invalid(
                "warp markers need increasing source/beat positions and explicit region endpoints",
            ));
        }
    }
    if edit.warp_enabled && !has_warp {
        return Err(Invalid("active warp needs settings and markers"));
    }
    let (_, length, view_start, view_end) = edit.projected_domain(tempo)?;
    if length == 0 || length > MAX_AUDIO_DOMAIN_FRAMES || view_start >= view_end {
        return Err(Invalid(
            "full audio domain exceeds its 16777216 frame limit or has an empty crop",
        ));
    }
    if plan.facts.source_offset != offset
        || plan.facts.source_length != domain.map_or(clip.source_length, |d| d.source_length)
        || plan.facts.source_phase != phase
        || plan.facts.looping != looping
    {
        return Err(Invalid(
            "audio source cache differs from its canonical domain",
        ));
    }
    if edit.placement.is_some() {
        let value = AudioViewProjection::from_preview_parts(
            clip,
            tempo,
            plan.facts.start,
            edit.clock,
            edit.view_start,
            edit.view_end,
            edit.musical_start,
            edit.placement,
        )?;
        let (start, end) = value.outer_frame_bounds();
        if plan.facts.start != start || plan.facts.duration != end - start {
            return Err(Invalid("audio outer frame cache differs from placement"));
        }
    } else if let Some(start) = edit.musical_start {
        let end = start
            .raw()
            .checked_add(edit.view_end - edit.view_start)
            .ok_or(Invalid("musical audio end overflow"))?;
        let start_frame = tempo.beat_to_frame(start)?;
        let end_frame = tempo.beat_to_frame(BeatTime::from_raw(end))?;
        if plan.facts.start != start_frame
            || plan.facts.duration
                != end_frame
                    .checked_sub(start_frame)
                    .ok_or(Invalid("musical audio frame span invalid"))?
        {
            return Err(Invalid(
                "musical audio frame cache differs from beat endpoints",
            ));
        }
    } else if plan.facts.duration != view_end - view_start {
        return Err(Invalid("audio crop duration differs from frame placement"));
    }
    let projection = edit.projection.resolve(source, tempo);
    if matches!(edit.clock, AudioDomainClock::Beats(_)) != projection.is_some() {
        return Err(Invalid("audio fade projection and domain clock disagree"));
    }
    if let Some(projection) = projection {
        if projection.tempo.sample_rate() != tempo.sample_rate()
            || projection.local_start_raw >= edit.clock.length()
            || projection.local_start_raw != edit.fade_start
        {
            return Err(Invalid("audio fade projection is invalid"));
        }
        audio_projection_frame(projection.tempo, projection.origin_beat_raw)?;
    }
    Ok(())
}
