//! Cached source peaks and their accepted clock for live musical event bends.
use super::waveform_frame::coarse_peaks;
use hex_engine::PreparedAudioRepeat;
use hex_engine::audio_transform::{MusicalEventFadePreview, PreparedMusicalEventSourceClock};
use hex_project::{
    AudioDomainClock, AudioEvent, AudioEvents, Clip, ClipFade, ClipGain, Project, TempoMap,
};
use hex_session::{CachedPeak, CachedWaveform, service::AudioEventSourceWaveform};
use std::{ops::Range, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Clock {
    Arrangement,
    Event(AudioEvent),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Projection {
    clip: Clip,
    tempo: TempoMap,
    source_clock: PreparedMusicalEventSourceClock,
    repeat: Option<PreparedAudioRepeat>,
    events: Arc<AudioEvents>,
    gain: ClipGain,
}

impl Projection {
    pub(super) fn for_display(
        project: &Project,
        clip: &Clip,
        prepared: Option<&AudioEventSourceWaveform>,
        events: Option<AudioEvents>,
        gain: Option<ClipGain>,
        fade: Option<ClipFade>,
    ) -> Option<(Arc<CachedWaveform>, Arc<Self>)> {
        let edit = clip.audio_edit()?;
        let prepared = prepared?;
        let source_clock = prepared.musical_clock.as_ref()?;
        if project.clip(clip.id())? != clip
            || !matches!(edit.domain.clock, AudioDomainClock::Beats(_))
            || edit.content_view.is_some()
        {
            return None;
        }
        let events = events
            .map(Arc::new)
            .or_else(|| edit.domain.events.clone())
            .unwrap_or_else(|| Arc::new(AudioEvents::identity(edit.domain.clock.length())));
        let clip = match fade {
            Some(fade) if project.arrangement_fade_edit_uses_outer(clip) => clip
                .with_authored_outer_fade(project.tempo_map(), fade)
                .ok()?,
            Some(_) => return None,
            None => clip.clone(),
        };
        let projection = Self {
            tempo: project.tempo_map().clone(),
            source_clock: source_clock.clone(),
            repeat: prepared.repeat,
            events,
            gain: gain.unwrap_or_else(|| clip.gain()),
            clip,
        };
        Some((Arc::clone(&prepared.waveform), Arc::new(projection)))
    }

    pub(super) fn query(&self, clock: &Clock) -> Option<MusicalEventFadePreview<'_>> {
        if !self.events.sources.is_empty() {
            return None;
        }
        let events = match clock {
            Clock::Arrangement => &self.events.events,
            Clock::Event(event) => std::slice::from_ref(event),
        };
        MusicalEventFadePreview::new(
            &self.clip,
            &self.tempo,
            &self.source_clock,
            events,
            self.gain,
            self.repeat,
        )
        .ok()?
    }
}

pub(super) fn peaks(
    query: &MusicalEventFadePreview<'_>,
    clock: Clock,
    waveform: &CachedWaveform,
    range: Range<u64>,
    remaining: &mut usize,
) -> Option<[CachedPeak; 2]> {
    coarse_peaks(waveform, |visit| match clock {
        Clock::Arrangement => query.visit_arrangement(range, remaining, visit),
        Clock::Event(event) => query.visit_event(event.id, range, remaining, visit),
    })
}
