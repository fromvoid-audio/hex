//! Free audio event placement on a frame-fixed grouped timeline.
//!
//! A grouped container's events read independently retained roots by finite
//! output frame, so its frame clock is only a placement extent. Placing an event
//! past the end grows that clock; placing it before the origin shifts every
//! event, the clip window and the container fade start later by the same lead,
//! so project time, PCM and the visible window are unchanged.
//!
//! An ungrouped one-shot frame root (no warp, pitch, source gain, loop, musical
//! placement or directed view) first becomes a grouped container whose single
//! retained root is that root's complete source read. Each event keeps its
//! identity, interval, source frame, fade, gain and activation. Other roots
//! refuse explicitly. The timeline stays within [`MAX_AUDIO_DOMAIN_FRAMES`].
use crate::{
    AudioClipEdit, AudioClipTiming, AudioDomainClock, AudioEventEdit, AudioEvents, Clip, ClipFade,
    ClipGain, ClipId, Invalid, Project,
    audio_edit::{MAX_AUDIO_DOMAIN_FRAMES, project_clip},
};
use std::sync::Arc;

pub(crate) fn place(
    project: &mut Project,
    clip: ClipId,
    event: u64,
    start: i64,
) -> Result<(), Invalid> {
    let original = project.clip(clip).ok_or(Invalid("clip no longer exists"))?;
    let (mut edit, mut events) = grouped_timeline(project, original)?;
    let length = edit.domain.clock.length();
    let moved = events
        .events
        .iter()
        .find(|e| e.id == event)
        .ok_or(Invalid("audio event no longer exists"))?;
    if i128::from(start) == i128::from(moved.start) {
        return Ok(());
    }
    let lead = if start < 0 { start.unsigned_abs() } else { 0 };
    let destination = u64::try_from(i128::from(start) + i128::from(lead))
        .map_err(|_| Invalid("audio event placement overflow"))?;
    let end = destination
        .checked_add(moved.length())
        .ok_or(Invalid("audio event placement overflow"))?;
    let timeline = length
        .checked_add(lead)
        .map(|shifted| shifted.max(end))
        .filter(|timeline| *timeline <= MAX_AUDIO_DOMAIN_FRAMES)
        .ok_or(Invalid(
            "audio event placement exceeds the 16777216 frame timeline limit",
        ))?;
    if lead > 0 {
        for event in &mut events.events {
            event.start += lead;
            event.end += lead;
        }
        edit.view_start += lead;
        edit.view_end += lead;
    }
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade_start += lead;
    domain.clock = AudioDomainClock::Frames(timeline);
    let next = events.edited(
        AudioEventEdit::Move {
            event,
            start: destination,
        },
        timeline,
    )?;
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(next));
    let mut candidate = original.clone();
    candidate.edit = Some(Arc::new(edit));
    project_clip(&mut candidate, project.tempo_map())?;
    let asset = project
        .asset(candidate.asset())
        .ok_or(Invalid("audio event source missing"))?;
    candidate
        .audio_edit()
        .ok_or(Invalid("audio domain missing"))?
        .validate(&candidate, project.tempo_map(), asset.playback_frames())?;
    if (candidate.start(), candidate.duration()) != (original.start(), original.duration()) {
        return Err(Invalid(
            "audio event placement must not move its clip window",
        ));
    }
    *project.clip_mut(clip)? = candidate;
    Ok(())
}

// The grouped edit and its editable (view-carved) events, converting an
// eligible ungrouped frame root without changing its rendered samples.
fn grouped_timeline(
    project: &Project,
    clip: &Clip,
) -> Result<(AudioClipEdit, AudioEvents), Invalid> {
    let mut edit = clip.audio_event_root();
    let mut events = clip.editable_audio_events()?;
    if !events.sources.is_empty() {
        if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
            || edit.content_view.is_some()
            || edit.placement.is_some()
        {
            return Err(Invalid(
                "free audio event placement needs a frame-fixed grouped clip",
            ));
        }
        return Ok((edit, events));
    }
    let domain = &edit.domain;
    if !matches!(domain.clock, AudioDomainClock::Frames(_))
        || !matches!(edit.timing, AudioClipTiming::Frames)
        || domain.warp_enabled
        || domain.pitch_cents != 0
        || !domain.source_gain.is_empty()
        || domain.looping
        || domain.source_phase != 0
        || edit.content_view.is_some()
        || edit.placement.is_some()
        || edit.processing_projection.is_some()
    {
        return Err(Invalid(
            "free audio event placement needs an unwarped, unpitched one-shot frame clip",
        ));
    }
    let length = domain.clock.length();
    let mut root = clip.clone();
    root.active = true;
    root.edit = None;
    root.duration = length;
    root.source_offset = domain.source_offset;
    root.source_length = domain.source_length;
    root.looping = false;
    root.source_phase = 0;
    root.gain = ClipGain::UNITY;
    root.fade = ClipFade::neutral(length)?;
    let root = project.restore_retained_audio_root(
        1,
        root.retained_snapshot(),
        Arc::new(project.tempo_map().clone()),
    )?;
    events.sources = vec![root];
    for event in &mut events.events {
        event.source = 1;
    }
    Arc::make_mut(&mut edit.domain).events = Some(Arc::new(events.clone()));
    Ok((edit, events))
}

#[cfg(test)]
#[path = "audio_event_timeline_tests.rs"]
mod tests;
