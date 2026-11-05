//! Read-only, whole-clip event placement from the canonical transaction owner.
//! The display candidate carries the validated expanded root and unchanged clip
//! window. Cached processed peaks may be borrowed only through the mappings below:
//! source identity/window, activation, event gain/fade remain equal, and the
//! frame container has no position-dependent processing. This owns metadata only;
//! no media, decoder, graph, history revision or global identity is allocated here.
use crate::{AudioClipTiming, AudioDomainClock, Clip, ClipId, Invalid, Project};

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
        let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
        original.audio_event_placement_limit().ok_or(Invalid(
            "free editor placement needs a fixed frame timeline",
        ))?;
        let candidate = crate::audio_event_timeline::prepare(self, clip, event, start)?;
        let root = original.audio_event_root();
        let mut waveform = Vec::new();
        if matches!(root.domain.clock, AudioDomainClock::Frames(_))
            && matches!(root.timing, AudioClipTiming::Frames)
            && root.placement.is_none()
            && root.content_view.is_none()
            && root.processing_projection.is_none()
            && !root.domain.warp_enabled
            && root.domain.pitch_cents == 0
            && root.domain.source_gain.is_empty()
            && root.domain.fade.is_neutral()
            && root.domain.source_phase == 0
        {
            let before = original.editable_audio_events()?;
            let after = candidate.audio_events();
            let converted = before.sources.is_empty() && !after.sources.is_empty();
            for target in &after.events {
                let Some(source) = before.events.iter().find(|source| {
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
                }) else {
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

#[cfg(test)]
mod tests;
