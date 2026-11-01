//! Runtime-owned MIDI scheduling metadata for one actual section loop. Original
//! source identity, factories, note banks and device-resource custody remain in
//! the immutable revision. Only timing admission, span/index arrays and work
//! envelopes are rebuilt on control, then borrowed by callback queries.
use super::*;
use std::{mem::size_of, ops::Deref};

struct Entry {
    track: TrackId,
    clip: ClipId,
    instrument_id: Option<hex_project::DeviceId>,
    instrument_enabled: bool,
    mutable_limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    events: usize,
    source: PreparedMidiSourceTemplate,
    instrument: PreparedMidiInstrumentTemplate,
    work: SessionWorkEnvelope,
}

pub(crate) struct PreparedSessionMidiAdmission {
    entries: Vec<Entry>,
    tracks: Vec<(TrackId, SessionWorkEnvelope)>,
    work: SessionWorkEnvelope,
}

#[derive(Clone, Copy)]
pub(crate) struct EffectiveSessionMidi<'a> {
    base: &'a PreparedSessionInventory,
    admission: Option<&'a PreparedSessionMidiAdmission>,
}

#[derive(Clone, Copy)]
pub(crate) struct EffectiveSessionMidiClip<'a> {
    base: &'a PreparedSessionMidiClip,
    admitted: Option<&'a Entry>,
}
impl Deref for EffectiveSessionMidiClip<'_> {
    type Target = PreparedSessionMidiClip;
    fn deref(&self) -> &Self::Target {
        self.base
    }
}
impl<'a> EffectiveSessionMidiClip<'a> {
    pub(crate) fn source_template(self) -> &'a PreparedMidiSourceTemplate {
        self.admitted
            .map_or_else(|| self.base.source_template(), |entry| &entry.source)
    }
    pub(crate) fn instrument_template(self) -> &'a PreparedMidiInstrumentTemplate {
        self.admitted.map_or_else(
            || self.base.instrument_template(),
            |entry| &entry.instrument,
        )
    }
    pub(crate) fn work(self) -> SessionWorkEnvelope {
        self.admitted.map_or(self.base.work, |entry| entry.work)
    }
}

impl<'a> EffectiveSessionMidi<'a> {
    pub(crate) fn new(
        base: &'a PreparedSessionInventory,
        admission: Option<&'a PreparedSessionMidiAdmission>,
    ) -> Self {
        Self { base, admission }
    }
    fn wrap(self, base: &'a PreparedSessionMidiClip) -> EffectiveSessionMidiClip<'a> {
        let admitted = self.admission.and_then(|admission| {
            admission
                .entries
                .binary_search_by_key(&(base.id, base.track), |entry| (entry.clip, entry.track))
                .ok()
                .and_then(|index| admission.entries.get(index))
        });
        EffectiveSessionMidiClip { base, admitted }
    }
    pub(crate) fn midi_clip(self, id: ClipId) -> Option<EffectiveSessionMidiClip<'a>> {
        self.base.midi_clip(id).map(|clip| self.wrap(clip))
    }
    pub(crate) fn midi_clip_for_track(
        self,
        id: ClipId,
        track: TrackId,
    ) -> Option<EffectiveSessionMidiClip<'a>> {
        self.base
            .midi_clip_for_track(id, track)
            .map(|clip| self.wrap(clip))
    }
    pub(crate) fn activity_midi_clips(self) -> impl Iterator<Item = EffectiveSessionMidiClip<'a>> {
        self.base
            .activity_midi_clips()
            .map(move |clip| self.wrap(clip))
    }
    pub(crate) fn work(self) -> SessionWorkEnvelope {
        self.admission
            .map_or_else(|| self.base.work(), |admission| admission.work)
    }
}

impl PreparedSessionInventory {
    pub(crate) fn section_admission_quote(
        &self,
        max_micro_bpm: u64,
    ) -> Result<(usize, usize), crate::Invalid> {
        if max_micro_bpm <= self.source_admission_max_micro_bpm {
            return Ok((0, 0));
        }
        let clips = self.activity_midi_clips().count();
        if clips == 0 {
            return Ok((0, 0));
        }
        let mut retained = clips
            .checked_mul(size_of::<Entry>())
            .and_then(|n| {
                n.checked_add(
                    self.tracks
                        .len()
                        .checked_mul(size_of::<(TrackId, SessionWorkEnvelope)>())?,
                )
            })
            .ok_or(crate::Invalid("section midi admission quote overflow"))?;
        let mut scratch = 0;
        for clip in self.activity_midi_clips() {
            let source = clip
                .source
                .readmission_quote()
                .map_err(|_| crate::Invalid("section midi source quote failed"))?;
            let instrument = clip
                .instrument
                .readmission_quote()
                .map_err(|_| crate::Invalid("section midi instrument quote failed"))?;
            retained = retained
                .checked_add(
                    source
                        .0
                        .saturating_sub(size_of::<PreparedMidiSourceTemplate>()),
                )
                .and_then(|n| {
                    n.checked_add(
                        instrument
                            .0
                            .saturating_sub(size_of::<PreparedMidiInstrumentTemplate>()),
                    )
                })
                .ok_or(crate::Invalid("section midi admission quote overflow"))?;
            scratch = scratch.max(source.1).max(instrument.1);
        }
        Ok((retained, scratch))
    }

    pub(crate) fn prepare_section_admission(
        &self,
        max_micro_bpm: u64,
        storage_limit: usize,
    ) -> Result<Option<PreparedSessionMidiAdmission>, crate::Invalid> {
        let quote = self.section_admission_quote(max_micro_bpm)?;
        if quote == (0, 0) {
            return Ok(None);
        }
        if quote
            .0
            .checked_add(quote.1)
            .is_none_or(|bytes| bytes > storage_limit)
        {
            return Err(crate::Invalid(
                "section midi admission exceeds its runtime storage budget",
            ));
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(self.activity_midi_clips().count())
            .map_err(|_| crate::Invalid("could not reserve section midi admission"))?;
        let mut tracks = Vec::new();
        tracks
            .try_reserve_exact(self.tracks.len())
            .map_err(|_| crate::Invalid("could not reserve section midi work"))?;
        tracks.extend(
            self.tracks
                .iter()
                .map(|track| (track.id, SessionWorkEnvelope::default())),
        );
        for clip in self.activity_midi_clips() {
            let source = clip
                .source
                .reprepare_for_timing_on_control(max_micro_bpm)
                .map_err(|error| crate::Invalid(error.diagnostic_message()))?;
            let instrument = clip
                .instrument
                .reprepare_for_source_on_control(&source, clip.mutable_limits)
                .map_err(|error| crate::Invalid(error.diagnostic_message()))?;
            let work = if clip.instrument_enabled {
                work_for(&source, &instrument)?
            } else {
                SessionWorkEnvelope::default()
            };
            let track = tracks
                .iter_mut()
                .find(|(id, _)| *id == clip.track)
                .ok_or(crate::Invalid("section midi track is unavailable"))?;
            track.1 = track.1.component_max(work);
            entries.push(Entry {
                track: clip.track,
                clip: clip.id,
                instrument_id: clip.instrument_id,
                instrument_enabled: clip.instrument_enabled,
                mutable_limits: clip.mutable_limits,
                events: source.max_query_edges(),
                source,
                instrument,
                work,
            });
        }
        validate_shared_source_admission(&mut entries)?;
        entries.sort_unstable_by_key(|entry| (entry.clip, entry.track));
        let work = tracks
            .iter()
            .try_fold(SessionWorkEnvelope::default(), |work, (_, track)| {
                work.checked_sum(*track)
            })?;
        let result = PreparedSessionMidiAdmission {
            entries,
            tracks,
            work,
        };
        if result.retained_bytes().is_none_or(|bytes| bytes > quote.0) {
            return Err(crate::Invalid(
                "section midi admission exceeds its storage quote",
            ));
        }
        Ok(Some(result))
    }
}

impl PreparedSessionMidiAdmission {
    pub(crate) fn track_voice_envelope(&self, track: TrackId) -> usize {
        self.tracks
            .iter()
            .find(|(id, _)| *id == track)
            .map_or(0, |(_, work)| work.active_voices)
    }

    pub(crate) fn retained_bytes(&self) -> Option<usize> {
        let mut bytes = self
            .entries
            .capacity()
            .checked_mul(size_of::<Entry>())?
            .checked_add(
                self.tracks
                    .capacity()
                    .checked_mul(size_of::<(TrackId, SessionWorkEnvelope)>())?,
            )?;
        for entry in &self.entries {
            bytes = bytes
                .checked_add(
                    entry
                        .source
                        .retained_readmission_bytes()?
                        .checked_sub(size_of::<PreparedMidiSourceTemplate>())?,
                )?
                .checked_add(
                    entry
                        .instrument
                        .retained_readmission_bytes()?
                        .checked_sub(size_of::<PreparedMidiInstrumentTemplate>())?,
                )?;
        }
        Some(bytes)
    }
}

#[derive(Default)]
struct SharedSourceEnvelope {
    work: SessionWorkEnvelope,
    events: usize,
    contributors: usize,
    limits: Option<crate::midi_compile::MutableInstrumentLimits>,
}

impl SharedSourceEnvelope {
    fn add(
        &mut self,
        work: SessionWorkEnvelope,
        events: usize,
        limits: Option<crate::midi_compile::MutableInstrumentLimits>,
    ) -> Result<(), crate::Invalid> {
        self.work = self.work.checked_sum(work)?;
        self.events = self.events.checked_add(events).ok_or(crate::Invalid(
            "shared session source event capacity overflow",
        ))?;
        self.contributors = self
            .contributors
            .checked_add(1)
            .ok_or(crate::Invalid("shared session source contributor overflow"))?;
        self.limits = self.limits.or(limits);
        Ok(())
    }

    fn validate(&self) -> Result<(), crate::Invalid> {
        if self.contributors <= 1 {
            return Ok(());
        }
        let max_voices = self.limits.map_or(
            crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK,
            |limits| limits.max_active_occurrences,
        );
        if self.work.active_voices > max_voices
            || self.work.query_onsets > crate::midi_compile::MAX_ONSETS_PER_WINDOW
            || self
                .limits
                .is_some_and(|limits| self.events > limits.max_events_per_query)
        {
            return Err(crate::Invalid(
                "shared session source exceeds its aggregate work capacity",
            ));
        }
        Ok(())
    }
}

/// Re-admitted Console clips can become denser at a section's faster tempo.
/// Keep their one-runtime source admission aggregate at the same boundary as
/// initial inventory preparation instead of validating each contributor alone.
fn validate_shared_source_admission(entries: &mut [Entry]) -> Result<(), crate::Invalid> {
    entries.sort_unstable_by_key(|entry| (entry.instrument_id, entry.track, entry.clip));
    let mut source_start = 0;
    while source_start < entries.len() {
        let Some(source_id) = entries[source_start].instrument_id else {
            source_start += 1;
            continue;
        };
        let mut source_end = source_start + 1;
        while entries
            .get(source_end)
            .is_some_and(|entry| entry.instrument_id == Some(source_id))
        {
            source_end += 1;
        }

        let mut source_envelope = SharedSourceEnvelope::default();
        let mut track_start = source_start;
        while track_start < source_end {
            let track_id = entries[track_start].track;
            let mut track_end = track_start + 1;
            while track_end < source_end && entries[track_end].track == track_id {
                track_end += 1;
            }
            let mut track_work = SessionWorkEnvelope::default();
            let mut track_events = 0usize;
            let mut has_events = false;
            let mut track_limits = None;
            for entry in &entries[track_start..track_end] {
                track_limits = track_limits.or(entry.mutable_limits);
                if entry.instrument_enabled {
                    track_work = track_work.component_max(entry.work);
                    track_events = track_events.max(entry.events);
                    has_events = true;
                }
            }
            if has_events {
                source_envelope.add(track_work, track_events, track_limits)?;
            }
            track_start = track_end;
        }
        source_envelope.validate()?;
        source_start = source_end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SharedSourceEnvelope;
    use crate::{Invalid, SessionWorkEnvelope, midi_compile::MutableInstrumentLimits};

    fn envelope(voices: usize, onsets: usize) -> SessionWorkEnvelope {
        SessionWorkEnvelope {
            active_voices: voices,
            query_onsets: onsets,
            ..SessionWorkEnvelope::default()
        }
    }

    fn limits(max_voices: usize, max_events: usize) -> MutableInstrumentLimits {
        MutableInstrumentLimits {
            max_events_per_query: max_events,
            max_active_occurrences: max_voices,
            max_release_tail_frames: 0,
            supports_clip_volume: false,
            steals_releasing_voices: false,
        }
    }

    #[test]
    fn section_readmission_rejects_aggregate_shared_source_voice_overflow() {
        let mut source = SharedSourceEnvelope::default();
        source
            .add(envelope(40, 12), 24, Some(limits(64, 256)))
            .unwrap();
        source
            .add(envelope(25, 12), 24, Some(limits(64, 256)))
            .unwrap();
        assert_eq!(
            source.validate(),
            Err(Invalid(
                "shared session source exceeds its aggregate work capacity"
            ))
        );
    }

    #[test]
    fn section_readmission_rejects_aggregate_shared_source_event_overflow() {
        let mut source = SharedSourceEnvelope::default();
        source
            .add(envelope(8, 10), 130, Some(limits(64, 256)))
            .unwrap();
        source
            .add(envelope(8, 10), 130, Some(limits(64, 256)))
            .unwrap();
        assert_eq!(
            source.validate(),
            Err(Invalid(
                "shared session source exceeds its aggregate work capacity"
            ))
        );
    }

    #[test]
    fn section_readmission_rejects_aggregate_shared_source_onset_overflow() {
        let mut source = SharedSourceEnvelope::default();
        source
            .add(envelope(8, 65), 20, Some(limits(64, 256)))
            .unwrap();
        source
            .add(envelope(8, 65), 20, Some(limits(64, 256)))
            .unwrap();
        assert_eq!(
            source.validate(),
            Err(Invalid(
                "shared session source exceeds its aggregate work capacity"
            ))
        );
    }
}
