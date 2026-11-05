//! Silent output before a retained frame source; source reads and fades stay at song time.
//! A prefix shifts destination events and the container fade, never their raw
//! source reads. Repeated extension reuses this sequence without nested roots.
//! Unwarped pitch and source gain read a fixed frame map before event composition:
//! increasing its output length leaves its original prefix unchanged. A frame
//! Once placement also retains its finite gate and trailing silence. Musical,
//! directed and repeating roots retain their existing trim policy.
use super::{
    AudioClipEdit, AudioClipTiming, AudioDomainClock, AudioViewMode, MAX_AUDIO_DOMAIN_FRAMES,
};
use crate::{AudioEvent, AudioEvents, Clip, ClipFade, ClipGain, Invalid, MAX_AUDIO_EVENTS};
use std::sync::Arc;

fn can_extend_frame_left(clip: &Clip) -> bool {
    !clip.looping()
        && clip.audio_edit().is_none_or(|edit| {
            matches!(edit.domain.clock, AudioDomainClock::Frames(_))
                && matches!(edit.timing, AudioClipTiming::Frames)
                && edit.content_view.is_none()
                && edit.placement.is_none_or(|p| p.mode == AudioViewMode::Once)
                && edit.processing_projection.is_none()
                && edit.domain.fade_projection.is_none()
                && !edit.domain.looping
                && !edit.domain.warp_enabled
                && edit.domain.source_phase == 0
        })
}

/// Rewind a frame Once crop past its first marker, or add silence if the
/// requested edge precedes the retained root. This never admits resources.
pub(crate) fn extend_frame_left(clip: &Clip, frame: u64) -> Result<Option<Clip>, Invalid> {
    if !can_extend_frame_left(clip) {
        return Ok(None);
    }
    let (length, cursor, end) =
        clip.audio_edit()
            .map_or((clip.duration(), 0, clip.duration()), |edit| {
                (
                    edit.domain.clock.length(),
                    i128::from(edit.view_start)
                        + i128::from(edit.placement.map_or(0, |p| p.cycle_phase)),
                    edit.view_end,
                )
            });
    let origin = i128::from(clip.start()) - cursor;
    if i128::from(frame) >= origin {
        if let Some(edit) = clip.audio_edit()
            && let Some(mut placement) = edit.placement
            && i128::from(frame) < origin + i128::from(edit.view_start)
        {
            let duration = clip
                .start
                .checked_add(clip.duration)
                .and_then(|end| end.checked_sub(frame))
                .ok_or(Invalid("audio trim length overflow"))?;
            let mut result = clip.clone();
            let next = result
                .audio_edit_mut()
                .ok_or(Invalid("audio crop has no domain"))?;
            next.view_start = u64::try_from(i128::from(frame) - origin)
                .map_err(|_| Invalid("audio trim position overflow"))?;
            placement.cycle_phase = 0;
            placement.outer_length = AudioDomainClock::Frames(duration);
            next.placement = Some(placement);
            result.start = frame;
            result.duration = duration;
            result.fade = ClipFade::neutral(duration)?;
            return Ok(Some(result));
        }
        return Ok(None);
    }
    let lead =
        u64::try_from(origin - i128::from(frame)).map_err(|_| Invalid("audio prefix overflow"))?;
    let length = length
        .checked_add(lead)
        .filter(|n| *n <= MAX_AUDIO_DOMAIN_FRAMES)
        .ok_or(Invalid(
            "audio left extension exceeds the 16777216 frame processing limit",
        ))?;
    let end = end
        .checked_add(lead)
        .ok_or(Invalid("audio prefix end overflow"))?;
    let song_end = clip
        .start()
        .checked_add(clip.duration())
        .ok_or(Invalid("audio clip end overflow"))?;
    let duration = song_end
        .checked_sub(frame)
        .filter(|n| *n > 0)
        .ok_or(Invalid("audio prefix must keep a positive clip"))?;
    let old = clip
        .audio_edit()
        .and_then(|edit| edit.domain.events.as_deref());
    let count = old.map_or(1, |events| events.events.len());
    let sources = old.map_or(0, |events| events.sources.len());
    if count > MAX_AUDIO_EVENTS || sources > MAX_AUDIO_EVENTS {
        return Err(Invalid("audio event limit reached"));
    }
    let mut events = AudioEvents {
        sources: Vec::new(),
        next_id: old.map_or(2, |e| e.next_id),
        events: Vec::new(),
    };
    events
        .sources
        .try_reserve_exact(sources)
        .map_err(|_| Invalid("could not reserve audio prefix sources"))?;
    events
        .events
        .try_reserve_exact(count)
        .map_err(|_| Invalid("could not reserve audio prefix events"))?;
    if let Some(old) = old {
        events.sources.extend(old.sources.iter().cloned());
        events.events.extend_from_slice(&old.events);
    } else {
        events.events.push(AudioEvent {
            active: true,
            gain: ClipGain::UNITY,
            source: 0,
            id: 1,
            start: 0,
            end: clip
                .audio_edit()
                .map_or(clip.duration(), |edit| edit.domain.clock.length()),
            source_start: 0,
            fade: None,
        });
    }
    for event in &mut events.events {
        event.start = event
            .start
            .checked_add(lead)
            .ok_or(Invalid("audio event prefix overflow"))?;
        event.end = event
            .end
            .checked_add(lead)
            .ok_or(Invalid("audio event prefix overflow"))?;
    }
    events.validate(length)?;
    let mut edit = clip
        .audio_edit()
        .cloned()
        .unwrap_or_else(|| AudioClipEdit::capture(clip));
    let fade_start = edit
        .domain
        .fade_start
        .checked_add(lead)
        .ok_or(Invalid("audio fade prefix overflow"))?;
    let domain = Arc::make_mut(&mut edit.domain);
    domain.clock = AudioDomainClock::Frames(length);
    domain.fade_start = fade_start;
    domain.events = Some(Arc::new(events));
    edit.view_start = 0;
    edit.view_end = end;
    if let Some(placement) = &mut edit.placement {
        placement.cycle_phase = 0;
        placement.outer_length = AudioDomainClock::Frames(duration);
    }
    let mut result = clip.clone();
    result.start = frame;
    result.duration = duration;
    result.fade = ClipFade::neutral(duration)?;
    result.edit = Some(Arc::new(edit));
    Ok(Some(result))
}

#[cfg(test)]
#[path = "prefix_tests.rs"]
mod tests;
