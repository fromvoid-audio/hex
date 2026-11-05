//! Free audio event placement on a frame-fixed timeline.
//!
//! A grouped container's events read independently retained roots by finite
//! output frame, so its frame clock is only a placement extent. Placing an event
//! past the end grows that clock; placing it before the origin shifts every
//! event, the clip window and the container fade start later by the same lead,
//! so project time, PCM and the visible window are unchanged.
//!
//! A plain one-shot can use a grouped retained source. Raw frame source
//! loops and unwarped processing keep their source clock: source samples at a
//! fixed input coordinate do not depend on the destination extent. Finite and
//! directed frame views translate their crop coordinates with the events;
//! their duration, direction and cycle phase remain fixed. Musical roots
//! require a separate processing-clock proof. Work stays bounded by
//! [`MAX_AUDIO_DOMAIN_FRAMES`].
use crate::{
    AudioClipEdit, AudioClipTiming, AudioDomainClock, AudioEventEdit, AudioEvents, Clip, ClipFade,
    ClipGain, ClipId, Invalid, Project,
    audio_edit::{MAX_AUDIO_DOMAIN_FRAMES, project_clip},
};
use std::sync::Arc;

/// Prepare the exact canonical candidate without touching the live project.
pub(crate) fn prepare(
    project: &Project,
    clip: ClipId,
    operation: &AudioEventEdit,
) -> Result<Clip, Invalid> {
    let original = project.clip(clip).ok_or(Invalid("clip no longer exists"))?;
    let (mut edit, mut events) = placement_timeline(project, original)?;
    let length = edit.domain.clock.length();
    let (old_start, span, start) = match *operation {
        AudioEventEdit::Place { event, start } => {
            let moved = events
                .events
                .iter()
                .find(|e| e.id == event)
                .ok_or(Invalid("audio event no longer exists"))?;
            (moved.start, moved.length(), start)
        }
        AudioEventEdit::PlaceRange {
            start,
            end,
            destination,
        } if start < end && end <= length => (start, end - start, destination),
        _ => return Err(Invalid("invalid audio event placement range")),
    };
    if i128::from(start) == i128::from(old_start) {
        return Ok(original.clone());
    }
    let lead = if start < 0 { start.unsigned_abs() } else { 0 };
    let destination = u64::try_from(i128::from(start) + i128::from(lead))
        .map_err(|_| Invalid("audio event placement overflow"))?;
    let end = destination
        .checked_add(span)
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
        if let Some(content) = edit.content_view.as_mut() {
            content.base_view_start += lead;
            content.base_view_end += lead;
        }
    }
    let domain = Arc::make_mut(&mut edit.domain);
    domain.fade_start += lead;
    domain.clock = AudioDomainClock::Frames(timeline);
    let next = match *operation {
        AudioEventEdit::Place { event, .. } => events.edited(
            AudioEventEdit::Move {
                event,
                start: destination,
            },
            timeline,
        )?,
        _ => events.place_range(
            old_start + lead,
            old_start + lead + span,
            destination,
            timeline,
        )?,
    };
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
    Ok(candidate)
}

// The editable (view-carved) events keep their original source clock.
fn placement_timeline(
    project: &Project,
    clip: &Clip,
) -> Result<(AudioClipEdit, AudioEvents), Invalid> {
    let mut edit = clip.audio_event_root();
    let mut events = clip.editable_audio_events()?;
    check_timeline(&edit)?;
    if !events.sources.is_empty()
        || edit.domain.looping
        || edit.domain.pitch_cents != 0
        || !edit.domain.source_gain.is_empty()
        || edit.placement.is_some()
        || edit.content_view.is_some()
    {
        return Ok((edit, events));
    }
    let domain = &edit.domain;
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

// Shared admission for canonical writes, whole-clip previews and navigation.
fn check_timeline(edit: &AudioClipEdit) -> Result<(), Invalid> {
    if !matches!(edit.domain.clock, AudioDomainClock::Frames(_))
        || !matches!(edit.timing, AudioClipTiming::Frames)
        || edit.processing_projection.is_some()
        || edit.domain.warp_enabled
    {
        return Err(Invalid(
            "free audio event placement needs an unwarped frame clip",
        ));
    }
    Ok(())
}
impl Clip {
    /// Maximum proved frame extent for free event placement in the editor. Navigation may
    /// reveal this finite space without allocating or extending the source.
    pub fn audio_event_placement_limit(&self) -> Option<u64> {
        let root = self.audio_event_root();
        check_timeline(&root).ok()?;
        // Legacy unprocessed clips can exceed the derived-root budget. Keep
        // their existing camera and bounded edit path rather than truncating
        // navigation to a smaller free-placement envelope.
        if root.domain.clock.length() > MAX_AUDIO_DOMAIN_FRAMES {
            return None;
        }
        Some(MAX_AUDIO_DOMAIN_FRAMES)
    }
}

#[cfg(test)]
#[path = "audio_event_timeline_tests.rs"]
mod tests;
