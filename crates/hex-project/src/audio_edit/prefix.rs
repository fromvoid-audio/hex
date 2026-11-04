//! Silent output before a retained frame source; source reads and fades stay at song time.
//! A prefix shifts destination events and the container fade, never their raw
//! source reads. Repeated extension reuses this sequence without nested roots.
//! Unwarped pitch and source gain read a fixed frame map before event composition:
//! increasing its output length leaves its original prefix unchanged. Musical,
//! directed and looping roots retain their existing trim policy.
use super::{AudioClipEdit, AudioClipTiming, AudioDomainClock, MAX_AUDIO_DOMAIN_FRAMES};
use crate::{AudioEvent, AudioEvents, Clip, ClipFade, ClipGain, Invalid, MAX_AUDIO_EVENTS};
use std::sync::Arc;

fn can_prepend_silence(clip: &Clip) -> bool {
    !clip.looping()
        && clip.audio_edit().is_none_or(|edit| {
            matches!(edit.domain.clock, AudioDomainClock::Frames(_))
                && matches!(edit.timing, AudioClipTiming::Frames)
                && edit.content_view.is_none()
                && edit.placement.is_none()
                && edit.processing_projection.is_none()
                && edit.domain.fade_projection.is_none()
                && !edit.domain.looping
                && !edit.domain.warp_enabled
                && edit.domain.source_phase == 0
        })
}

/// Return a checked frame-root prefix only when the requested edge precedes
/// its retained origin. This does not mutate a project or admit new resources.
pub(crate) fn prepend_silence(clip: &Clip, frame: u64) -> Result<Option<Clip>, Invalid> {
    if !can_prepend_silence(clip) {
        return Ok(None);
    }
    let (length, start, end) = clip
        .audio_edit()
        .map_or((clip.duration(), 0, clip.duration()), |edit| {
            (edit.domain.clock.length(), edit.view_start, edit.view_end)
        });
    let origin = i128::from(clip.start()) - i128::from(start);
    if i128::from(frame) >= origin {
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
