//! Read-only, whole-clip event placement from the canonical transaction owner.
//! The display candidate carries the validated expanded root and unchanged clip
//! window. Cached processed peaks may be borrowed only through the mappings below:
//! source identity/window, activation, event gain/fade remain equal, and the
//! frame container has no position-dependent processing. This owns metadata only;
//! no media, decoder, graph, history revision or global identity is allocated here.
use crate::{AudioClipTiming, AudioDomainClock, AudioEventEdit, Clip, ClipId, Invalid, Project};

#[derive(Clone, Debug)]
pub struct AudioEventPlacementPreview {
    clip: Clip,
    waveform: Vec<WaveformInterval>,
}
#[derive(Clone, Debug)]
struct WaveformInterval {
    id: u64,
    start: u64,
    end: u64,
    original_start: u64,
}
impl AudioEventPlacementPreview {
    pub fn clip(&self) -> &Clip {
        &self.clip
    }
    /// Translate a candidate event endpoint into the previous accepted root's
    /// processed frame clock. None means the old peaks are not evidence for this
    /// placement; wait for the worker's newly prepared summary instead.
    pub fn original_waveform_frame(&self, event: u64, position: u64) -> Option<u64> {
        let index = self.waveform.binary_search_by_key(&event, |m| m.id).ok()?;
        let mapping = self.waveform.get(index)?;
        if position < mapping.start || position > mapping.end {
            return None;
        }
        mapping.original_start.checked_add(position - mapping.start)
    }
}
impl Project {
    /// Validate a signed placement without changing the project, Undo/Redo,
    /// source media or identity allocator. The fixed-frame editor admission
    /// reuses `EditAudioEvents::Place` source restoration; unproved read laws refuse.
    pub fn preview_audio_event_placement(
        &self,
        clip: ClipId,
        event: u64,
        start: i64,
    ) -> Result<AudioEventPlacementPreview, Invalid> {
        self.preview_audio_event_edit(clip, &AudioEventEdit::Place { event, start })
    }

    /// Preview a signed single-event or range placement in its exact canonical root.
    pub fn preview_audio_event_edit(
        &self,
        clip: ClipId,
        operation: &AudioEventEdit,
    ) -> Result<AudioEventPlacementPreview, Invalid> {
        let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
        original.audio_event_placement_limit().ok_or(Invalid(
            "free editor placement needs a fixed frame timeline",
        ))?;
        let candidate = crate::audio_event_timeline::prepare(self, clip, operation)?;
        let root = original.audio_event_root();
        let mut waveform = Vec::new();
        if matches!(root.domain.clock, AudioDomainClock::Frames(_))
            && matches!(root.timing, AudioClipTiming::Frames)
            && root.processing_projection.is_none()
            && !root.domain.warp_enabled
            && root.domain.fade.is_neutral()
        {
            let before = original.editable_audio_events()?;
            let after = candidate.audio_events();
            let converted = before.sources.is_empty() && !after.sources.is_empty();
            // Place preserves IDs except a possible new right-hand victim
            // fragment. Look up retained identities first; only that fragment
            // needs the bounded source-window fallback. Repaints use the sorted
            // result below, rather than repeating a sequence-wide search.
            let mut identities = before.events.iter().collect::<Vec<_>>();
            identities.sort_unstable_by_key(|event| event.id);
            for target in &after.events {
                let matches_source = |source: &crate::AudioEvent| {
                    let id = if converted { 1 } else { source.source };
                    id == target.source
                        && source.active == target.active
                        && source.gain == target.gain
                        && source.fade == target.fade
                        && target.source_start >= source.source_start
                        && target
                            .source_start
                            .checked_add(target.length())
                            .is_some_and(|end| {
                                source
                                    .source_start
                                    .checked_add(source.length())
                                    .is_some_and(|last| end <= last)
                            })
                };
                let source = identities
                    .binary_search_by_key(&target.id, |event| event.id)
                    .ok()
                    .and_then(|index| identities.get(index).copied())
                    .filter(|source| matches_source(source))
                    .or_else(|| before.events.iter().find(|source| matches_source(source)));
                let Some(source) = source else {
                    continue;
                };
                let Some(original_start) = source
                    .start
                    .checked_add(target.source_start - source.source_start)
                else {
                    continue;
                };
                waveform.push(WaveformInterval {
                    id: target.id,
                    start: target.start,
                    end: target.end,
                    original_start,
                });
            }
            waveform.sort_unstable_by_key(|m| m.id);
        }
        Ok(AudioEventPlacementPreview {
            clip: candidate,
            waveform,
        })
    }
}

impl Clip {
    /// Numerical root-frame limits for this captured event. This is a preview
    /// hint, not a processing reservation; the canonical command revalidates it.
    pub fn audio_event_placement_bounds(&self, event: u64) -> Result<(i64, i64), Invalid> {
        let events = self.editable_audio_events()?;
        let event = events
            .events
            .iter()
            .find(|e| e.id == event)
            .ok_or(Invalid("audio event no longer exists"))?;
        self.audio_event_range_placement_bounds(event.start, event.end)
    }

    /// Placement limits for a nonempty interval in the retained frame root.
    pub fn audio_event_range_placement_bounds(
        &self,
        start: u64,
        end: u64,
    ) -> Result<(i64, i64), Invalid> {
        let limit = self.audio_event_placement_limit().ok_or(Invalid(
            "free editor placement needs a fixed frame timeline",
        ))?;
        let length = self.audio_event_root().domain.clock.length();
        if start >= end || end > length {
            return Err(Invalid("invalid audio event placement range"));
        }
        let lead = limit
            .checked_sub(length)
            .ok_or(Invalid("audio event root exceeds placement limit"))?;
        let last = limit
            .checked_sub(end - start)
            .ok_or(Invalid("audio event exceeds placement limit"))?;
        Ok((
            -i64::try_from(lead).map_err(|_| Invalid("audio event placement overflow"))?,
            i64::try_from(last).map_err(|_| Invalid("audio event placement overflow"))?,
        ))
    }
    /// Prove a fixed-frame authoring-origin translation across Place or its
    /// history restoration. The Arrangement window and physical source window
    /// stay equal; both clock length and container-fade origin translate by the
    /// same amount. Pure crop, source replacement, processing and other clock
    /// changes supply no such proof. This fact moves display coordinates only.
    pub fn audio_event_coordinate_shift_from(&self, previous: &Self) -> Option<i64> {
        if self.id() != previous.id()
            || self.asset() != previous.asset()
            || (self.start(), self.duration()) != (previous.start(), previous.duration())
            || self.audio_event_placement_limit().is_none()
            || previous.audio_event_placement_limit().is_none()
        {
            return None;
        }
        let before = previous.audio_event_root();
        let after = self.audio_event_root();
        let shift =
            i128::from(after.domain.clock.length()) - i128::from(before.domain.clock.length());
        if shift != i128::from(after.domain.fade_start) - i128::from(before.domain.fade_start)
            || after.domain.fade != before.domain.fade
            || after.domain.source_offset != before.domain.source_offset
            || after.domain.source_length != before.domain.source_length
            || after.domain.looping != before.domain.looping
            || after.domain.source_phase != before.domain.source_phase
            || after.domain.pitch_cents != before.domain.pitch_cents
            || after.domain.source_gain != before.domain.source_gain
            || after.domain.warp_resampler != before.domain.warp_resampler
            || after.placement != before.placement
            || i128::from(after.view_start) - i128::from(before.view_start) != shift
            || i128::from(after.view_end) - i128::from(before.view_end) != shift
        {
            return None;
        }
        match (before.content_view, after.content_view) {
            (Some(mut previous), Some(current)) => {
                previous.base_view_start =
                    u64::try_from(i128::from(previous.base_view_start) + shift).ok()?;
                previous.base_view_end =
                    u64::try_from(i128::from(previous.base_view_end) + shift).ok()?;
                if previous != current {
                    return None;
                }
            }
            (None, None) => {}
            _ => return None,
        }
        i64::try_from(shift).ok()
    }
}

#[cfg(test)]
mod tests;
