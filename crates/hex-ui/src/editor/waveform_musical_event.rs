//! Cached source peaks and their accepted clock for live musical event bends.
use super::waveform_frame::{coarse_peaks, merge};
use hex_engine::PreparedAudioRepeat;
use hex_engine::audio_transform::{MusicalEventFadePreview, PreparedMusicalEventSourceClock};
use hex_project::{
    AudioDomainClock, AudioEvent, AudioEvents, Clip, ClipFade, ClipGain, Project, TempoMap,
};
use hex_session::{
    CachedPeak, CachedWaveform,
    service::{AudioEventOccurrenceWaveforms, AudioEventSourceWaveform},
};
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
    occurrences: Option<Occurrences>,
    events: Arc<AudioEvents>,
    gain: ClipGain,
}

struct Occurrences(Arc<AudioEventOccurrenceWaveforms>);

impl std::fmt::Debug for Occurrences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Occurrences")
            .field(&self.0.reads().len())
            .finish()
    }
}

impl PartialEq for Occurrences {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Occurrences {}

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
        let placed = edit.placement.is_some();
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
            occurrences: prepared
                .occurrences
                .as_ref()
                .map(|table| Occurrences(Arc::clone(table))),
            events,
            gain: gain.unwrap_or_else(|| clip.gain()),
            clip,
        };
        // A placement with several independent roots must keep the established
        // destination waveform, including its held clip fade. Validate the
        // source and read geometry before selecting the event projection.
        if placed && projection.occurrences.is_none() {
            projection.project(&[])?;
        }
        Some((Arc::clone(&prepared.waveform), Arc::new(projection)))
    }

    pub(super) fn query(&self, clock: &Clock) -> Option<Query<'_>> {
        if !self.events.sources.is_empty() {
            return None;
        }
        if let Some(occurrences) = &self.occurrences {
            return match clock {
                Clock::Arrangement => Some(Query::Placement {
                    projection: self,
                    table: &occurrences.0,
                    current: None,
                }),
                Clock::Event(event) => Some(Query::Shared {
                    clock: *clock,
                    preview: MusicalEventFadePreview::for_editor(
                        &self.clip,
                        &self.tempo,
                        &self.source_clock,
                        event,
                        self.gain,
                    )
                    .ok()??,
                }),
            };
        }
        let events = match clock {
            Clock::Arrangement => &self.events.events,
            Clock::Event(event) => std::slice::from_ref(event),
        };
        Some(Query::Shared {
            clock: *clock,
            preview: self.project(events)?,
        })
    }

    fn project(&self, events: &[AudioEvent]) -> Option<MusicalEventFadePreview<'_>> {
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

pub(super) enum Query<'a> {
    Shared {
        clock: Clock,
        preview: MusicalEventFadePreview<'a>,
    },
    Placement {
        projection: &'a Projection,
        table: &'a AudioEventOccurrenceWaveforms,
        current: Option<(usize, MusicalEventFadePreview<'a>)>,
    },
}

pub(super) fn peaks(
    query: &mut Query<'_>,
    waveform: &CachedWaveform,
    range: Range<u64>,
    remaining: &mut usize,
) -> Option<[CachedPeak; 2]> {
    match query {
        Query::Shared { clock, preview } => coarse_peaks(waveform, |visit| match clock {
            Clock::Arrangement => preview.visit_arrangement(range, remaining, visit),
            Clock::Event(event) => preview.visit_event(event.id, range, remaining, visit),
        }),
        Query::Placement {
            projection,
            table,
            current,
        } => {
            let projection = *projection;
            let table = *table;
            let reads = table.reads();
            let first = reads.partition_point(|read| read.placement.destination_end <= range.start);
            let mut combined = None;
            let mut cursor = range.start;
            let silence = [CachedPeak { min: 0.0, max: 0.0 }; 2];
            for (offset, read) in reads[first..]
                .iter()
                .enumerate()
                .take_while(|(_, read)| read.placement.destination_start < range.end)
            {
                let index = first + offset;
                if current.as_ref().is_none_or(|(last, _)| *last != index) {
                    // Bound visible preparation and sample visits together, even
                    // when one very wide column intersects many different roots.
                    let events = &projection.events.events;
                    *remaining = remaining.checked_sub(events.len() + 1)?;
                    let preview = MusicalEventFadePreview::for_occurrence(
                        &projection.clip,
                        &projection.tempo,
                        read,
                        events,
                        projection.gain,
                    )
                    .ok()??;
                    *current = Some((index, preview));
                }
                let (_, preview) = current.as_ref()?;
                let start = range.start.max(read.placement.destination_start);
                let end = range.end.min(read.placement.destination_end);
                if cursor < start {
                    merge(&mut combined, silence);
                }
                let peaks = coarse_peaks(table.waveform(index)?, |visit| {
                    preview.visit_arrangement(start..end, remaining, visit)
                })?;
                merge(&mut combined, peaks);
                cursor = end;
            }
            if cursor < range.end {
                merge(&mut combined, silence);
            }
            combined
        }
    }
}
