//! Runtime-owned MIDI scheduling metadata for one actual section loop. Original
//! source identity, factories, note banks and device-resource custody remain in
//! the immutable revision. Only timing admission, span/index arrays and work
//! envelopes are rebuilt on control, then borrowed by callback queries.
use super::*;
use std::{mem::size_of, ops::Deref};

struct Entry {
    track: TrackId,
    clip: ClipId,
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
            let work = work_for(&source, &instrument)?;
            let track = tracks
                .iter_mut()
                .find(|(id, _)| *id == clip.track)
                .ok_or(crate::Invalid("section midi track is unavailable"))?;
            track.1 = track.1.component_max(work);
            entries.push(Entry {
                track: clip.track,
                clip: clip.id,
                source,
                instrument,
                work,
            });
        }
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
