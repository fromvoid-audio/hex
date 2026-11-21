//! Cached source peaks and their accepted clock for live musical event bends.
use super::waveform_frame::{coarse_peaks_from, merge};
use hex_engine::PreparedAudioRepeat;
use hex_engine::audio_transform::{
    DirectedMusicalDestinationFade, MusicalEventFadePreview, PreparedMusicalEventSourceClock,
};
use hex_project::{
    AudioDomainClock, AudioEvent, AudioEvents, Clip, ClipFade, ClipGain, Project, TempoMap,
};
use hex_session::{
    CachedPeak, CachedWaveform,
    service::{AudioClipWaveform, AudioEventOccurrenceWaveforms, AudioEventSourceWaveforms},
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
    sources: Option<EventSources>,
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

struct EventSources(Arc<AudioEventSourceWaveforms>);

impl std::fmt::Debug for EventSources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("EventSources")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for EventSources {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for EventSources {}

impl Projection {
    pub(super) fn for_display(
        project: &Project,
        clip: &Clip,
        prepared: Option<&AudioClipWaveform>,
        events: Option<AudioEvents>,
        gain: Option<ClipGain>,
        fade: Option<ClipFade>,
    ) -> Option<(Arc<CachedWaveform>, Arc<Self>)> {
        let edit = clip.audio_edit()?;
        let prepared = prepared?;
        let source = prepared.event_source.as_ref()?;
        let source_clock = source.musical_clock.as_ref()?;
        if project.clip(clip.id())? != clip
            || !matches!(edit.domain.clock, AudioDomainClock::Beats(_))
        {
            return None;
        }
        let events = events
            .map(Arc::new)
            .or_else(|| edit.domain.events.clone())
            .unwrap_or_else(|| Arc::new(AudioEvents::identity(edit.domain.clock.length())));
        let placed = edit.content_view.is_none() && edit.placement.is_some();
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
            repeat: source.repeat,
            occurrences: source
                .occurrences
                .as_ref()
                .map(|table| Occurrences(Arc::clone(table))),
            sources: prepared
                .event_sources
                .as_ref()
                .map(|table| EventSources(Arc::clone(table))),
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
        Some((Arc::clone(&source.waveform), Arc::new(projection)))
    }

    pub(super) fn query(&self, clock: &Clock) -> Option<Query<'_>> {
        if !self.events.sources.is_empty() {
            return None;
        }
        if matches!(clock, Clock::Arrangement)
            && let Some(occurrences) = &self.occurrences
        {
            let table = &occurrences.0;
            let directed_fade = table
                .directed()
                .map(|_| {
                    DirectedMusicalDestinationFade::new(&self.clip, &self.tempo, &self.source_clock)
                        .map(Box::new)
                })
                .transpose()
                .ok()?;
            return Some(Query::Placement {
                projection: self,
                table,
                current: None,
                directed_fade,
            });
        }
        if let Clock::Event(event) = clock
            && (self.occurrences.is_some() || self.clip.audio_edit()?.content_view.is_some())
        {
            return Some(Query::Shared {
                projection: self,
                clock: *clock,
                preview: MusicalEventFadePreview::for_editor(
                    &self.clip,
                    &self.tempo,
                    &self.source_clock,
                    event,
                    self.gain,
                )
                .ok()??,
            });
        }
        let events = match clock {
            Clock::Arrangement => &self.events.events,
            Clock::Event(event) => std::slice::from_ref(event),
        };
        Some(Query::Shared {
            projection: self,
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

    fn waveform<'a>(
        &'a self,
        original: &'a CachedWaveform,
        clock: &PreparedMusicalEventSourceClock,
        source_id: u64,
    ) -> Option<&'a CachedWaveform> {
        if source_id == 0 {
            return Some(original);
        }
        let table = &self.sources.as_ref()?.0;
        if table.musical_clock(source_id)? != clock {
            return None;
        }
        table.waveform(source_id).map(Arc::as_ref)
    }
}

pub(super) enum Query<'a> {
    Shared {
        projection: &'a Projection,
        clock: Clock,
        preview: MusicalEventFadePreview<'a>,
    },
    Placement {
        projection: &'a Projection,
        table: &'a AudioEventOccurrenceWaveforms,
        current: Option<(usize, MusicalEventFadePreview<'a>)>,
        directed_fade: Option<Box<DirectedMusicalDestinationFade<'a>>>,
    },
}

pub(super) fn peaks(
    query: &mut Query<'_>,
    waveform: &CachedWaveform,
    range: Range<u64>,
    remaining: &mut usize,
) -> Option<[CachedPeak; 2]> {
    match query {
        Query::Shared {
            projection,
            clock,
            preview,
        } => coarse_peaks_from(
            |source| projection.waveform(waveform, &projection.source_clock, source),
            |visit| match clock {
                Clock::Arrangement => preview.visit_arrangement(range, remaining, visit),
                Clock::Event(event) => preview.visit_event(event.id, range, remaining, visit),
            },
        ),
        Query::Placement {
            projection,
            table,
            current,
            directed_fade,
        } => {
            let Some(maps) = table.directed() else {
                return placement_peaks(projection, table, current, range, remaining);
            };
            let fade = directed_fade.as_mut()?;
            let mut combined = None;
            let mut cursor = range.start;
            for mapped in maps.source_ranges(range.clone()) {
                *remaining = remaining.checked_sub(1)?;
                let (destination, base) = mapped.ok()?;
                if cursor < destination.start {
                    merge(&mut combined, SILENCE);
                }
                let midpoint = destination.start + (destination.end - destination.start - 1) / 2;
                let gain = fade.gain_at(midpoint).ok()?;
                let peaks = if base.is_empty() || gain == 0.0 {
                    SILENCE
                } else {
                    placement_peaks(projection, table, current, base, remaining)?.map(|peak| {
                        CachedPeak {
                            min: peak.min * gain,
                            max: peak.max * gain,
                        }
                    })
                };
                merge(&mut combined, peaks);
                cursor = destination.end;
            }
            if cursor < range.end {
                merge(&mut combined, SILENCE);
            }
            combined
        }
    }
}

const SILENCE: [CachedPeak; 2] = [CachedPeak { min: 0.0, max: 0.0 }; 2];

fn placement_peaks<'a>(
    projection: &'a Projection,
    table: &'a AudioEventOccurrenceWaveforms,
    current: &mut Option<(usize, MusicalEventFadePreview<'a>)>,
    range: Range<u64>,
    remaining: &mut usize,
) -> Option<[CachedPeak; 2]> {
    let reads = table.reads();
    let first = reads.partition_point(|read| read.placement.destination_end <= range.start);
    let mut combined = None;
    let mut cursor = range.start;
    for (offset, read) in reads[first..]
        .iter()
        .enumerate()
        .take_while(|(_, read)| read.placement.destination_start < range.end)
    {
        let index = first + offset;
        if current.as_ref().is_none_or(|(last, _)| *last != index) {
            // A wide column can intersect many roots. Charge the visible
            // query budget for both occurrence setup and event visits.
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
            merge(&mut combined, SILENCE);
        }
        let original = table.waveform(index)?;
        let clock = read.source.musical_clock()?;
        let peaks = coarse_peaks_from(
            |source| projection.waveform(original, clock, source),
            |visit| preview.visit_arrangement(start..end, remaining, visit),
        )?;
        merge(&mut combined, peaks);
        cursor = end;
    }
    if cursor < range.end {
        merge(&mut combined, SILENCE);
    }
    combined
}
