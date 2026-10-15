//! Borrowed, checked analytic-to-owned-history adoption at one committed cut.
use super::*;
use crate::midi_note_bank::PreparedMidiNote;
use crate::mutable_instrument::{
    MutableOccurrence, MutableOccurrenceKey, MutableOccurrenceSignature,
};
use hex_lattice::{
    AnalyticInstrumentHistory, NativeInstrumentAnalyticSeed, PreparedSharedAnalyticNote,
};
const CAPACITY: usize = crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS;
pub(super) struct Import {
    context: u16,
    pub voices: [NativeInstrumentAnalyticSeed; CAPACITY],
    pub occurrences: [Option<MutableOccurrence>; CAPACITY],
    pub len: usize,
}
impl Import {
    pub fn history<'a>(
        &'a self,
        previous: &'a SessionPlayback,
    ) -> Result<AnalyticInstrumentHistory<'a>, SessionPlaybackError> {
        let source = previous
            .revision
            .arrangement()
            .shared_contexts()
            .owner(self.context)
            .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
        Ok(AnalyticInstrumentHistory {
            source: source.context(),
            source_id: source
                .continuation_id()
                .map_err(|_| SessionPlaybackError::SharedAnalyticContext)?,
            transition: previous
                .analytic_continuations
                .get(usize::from(self.context))
                .copied()
                .flatten(),
            performance_frame: previous.performance_frame,
            voices: &self.voices[..self.len],
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        previous: &SessionPlayback,
        route: TrackRoute,
        sampler: PreparedMidiNote,
        start: u64,
        gate: u64,
        end: u64,
        cut: u64,
        clip: hex_project::ClipId,
        note: hex_project::NoteId,
        iteration: u64,
        onset_beat: hex_project::BeatTime,
        gate_end_beat: hex_project::BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
        honor_suppression: bool,
    ) -> Result<(), SessionPlaybackError> {
        if start >= cut
            || end <= cut
            || (honor_suppression
                && previous.suppressions.iter().any(|entry| {
                    entry.track == route.id
                        && entry.clip == clip
                        && entry.note == note
                        && entry.iteration == iteration
                        && entry.until_frame > previous.performance_frame
                }))
        {
            return Ok(());
        }
        let PreparedMidiNote::Shared {
            context: crate::midi_note_bank::SharedContextRef::Revision(context),
            note: payload,
            id,
            ..
        } = sampler
        else {
            return Err(SessionPlaybackError::SharedAnalyticContext);
        };
        let owner = previous
            .revision
            .arrangement()
            .shared_contexts()
            .owner(self.context)
            .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
        if context != self.context || owner.id() != id || self.len >= CAPACITY {
            return Err(SessionPlaybackError::SharedAnalyticContext);
        }
        let age = cut
            .checked_sub(start)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let gate_age = gate
            .checked_sub(start)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let id = (self.len + 1) as u64;
        self.voices[self.len] = NativeInstrumentAnalyticSeed {
            occurrence_id: id,
            note: payload,
            age_frames: age,
            gate_frame: (gate <= cut).then_some(gate_age),
        };
        let gate_end_frame = if gate >= cut {
            previous
                .performance_frame
                .checked_add(gate - cut)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?
        } else {
            previous.performance_frame.saturating_sub(cut - gate)
        };
        let quiet_deadline_frame = previous
            .performance_frame
            .checked_add(end - cut)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        self.occurrences[self.len] = Some(MutableOccurrence {
            signature: MutableOccurrenceSignature::from_parts(
                MutableOccurrenceKey::new(
                    route.id,
                    clip,
                    note,
                    iteration,
                    previous.arrangement_loop_pass,
                ),
                onset_beat,
                gate_end_beat,
                pitch,
                channel,
                velocity,
            ),
            id,
            onset_frame: previous.performance_frame.saturating_sub(age),
            gate_end_frame,
            quiet_deadline_frame,
            note_off_sent: gate <= cut,
        });
        self.len += 1;
        Ok(())
    }
}
pub(super) fn at_cut(
    previous: &SessionPlayback,
    index: usize,
    spans: &mut MidiSpanBuffer,
) -> Result<Import, SessionPlaybackError> {
    let route = *previous
        .routes
        .get(index)
        .ok_or(SessionPlaybackError::UnknownTrack)?;
    let context = previous
        .revision
        .arrangement()
        .shared_contexts()
        .index_for_track(route.id)
        .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
    let mut out = Import {
        context,
        voices: [NativeInstrumentAnalyticSeed {
            occurrence_id: 0,
            note: PreparedSharedAnalyticNote::new([0; 8], 0),
            age_frames: 0,
            gate_frame: None,
        }; CAPACITY],
        occurrences: [None; CAPACITY],
        len: 0,
    };
    match route.owner {
        RouteOwner::Arrangement => {
            let track = previous
                .revision
                .arrangement()
                .tracks()
                .get(index)
                .ok_or(SessionPlaybackError::UnknownTrack)?;
            let mut failure = None;
            track
                .for_each_active_midi_span(previous.arrangement_frame, |span, sampler| {
                    if failure.is_some() {
                        return;
                    }
                    failure = out
                        .push(
                            previous,
                            route,
                            sampler,
                            span.start,
                            span.gate_end,
                            span.end,
                            previous.arrangement_frame,
                            span.clip,
                            span.note,
                            span.iteration,
                            span.onset_beat,
                            span.gate_end_beat,
                            span.pitch,
                            span.channel,
                            span.velocity,
                            true,
                        )
                        .err();
                })
                .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        RouteOwner::Session { clip, launch } => {
            let clip = previous
                .revision
                .session()
                .midi_clip(clip)
                .ok_or(SessionPlaybackError::UnknownClip)?;
            let clock = timing_view_for(
                previous.revision.session(),
                previous.periodic_tempo,
                previous.anchor,
            );
            let frame = previous.performance_frame;
            let end = frame
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            if let Some(floor) = previous.historical_floor_for(index) {
                clip.instrument_template()
                    .query_after_onset(launch, &clock, frame, end, spans, floor)
            } else {
                clip.instrument_template()
                    .query(launch, &clock, frame, end, spans)
            }
            .map_err(SessionPlaybackError::Midi)?;
            for span in spans.spans() {
                let sampler = span
                    .voice
                    .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
                out.push(
                    previous,
                    route,
                    sampler,
                    span.start_frame,
                    span.gate_end_frame,
                    span.end_frame,
                    frame,
                    span.clip,
                    span.note,
                    span.iteration,
                    span.onset_beat,
                    span.gate_end_beat,
                    span.pitch,
                    span.channel,
                    span.velocity,
                    true,
                )?;
            }
        }
        RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
    }
    // Current-owner bridges have already been suppressed in the route query.
    // Import them once; detached historical generations keep their original law.
    for entry in &previous.releases {
        if !imports_release(previous, index, entry) {
            continue;
        }
        let continuation = entry
            .continuation
            .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
        let c = continuation;
        // The matching suppression was intentionally honored above. This bridge
        // now supplies the actual age, gate and sampler instead of a fresh chase.
        out.push(
            previous,
            route,
            entry.voice.sampler(),
            entry.voice.start_frame(),
            entry.voice.gate_end_frame(),
            entry.voice.end_frame(),
            previous.performance_frame,
            c.key.clip,
            c.key.note,
            c.key.iteration,
            c.onset_beat,
            c.gate_end_beat,
            c.pitch,
            c.channel,
            c.velocity,
            false,
        )?;
    }
    Ok(out)
}
pub(super) fn imports_release(
    previous: &SessionPlayback,
    index: usize,
    entry: &ReleaseEntry,
) -> bool {
    entry.track_id == previous.routes[index].id
        && entry.location.tail_group().is_none()
        && entry.voice.sampler().is_current()
        && entry.voice.context_ref()
            == previous
                .revision
                .arrangement()
                .shared_contexts()
                .index_for_track(entry.track_id)
                .map(crate::midi_note_bank::SharedContextRef::Revision)
        && entry.voice.end_frame() > previous.performance_frame
}

// Keep fixed voice batches out of the already large host handoff stack frame.
#[inline(never)]
pub(super) fn preflight(
    previous: &SessionPlayback,
    index: usize,
    spans: &mut MidiSpanBuffer,
    target: &crate::mutable_instrument::MutableInstrumentTrackRuntime,
) -> Result<bool, SessionPlaybackError> {
    let imported = at_cut(previous, index, spans)?;
    Ok(target.can_import_harmonics(
        &imported.history(previous)?,
        &imported.occurrences[..imported.len],
    ))
}
#[inline(never)]
pub(super) fn adopt(
    previous: &SessionPlayback,
    index: usize,
    spans: &mut MidiSpanBuffer,
    target: &mut crate::mutable_instrument::MutableInstrumentTrackRuntime,
) -> Result<(), SessionPlaybackError> {
    let imported = at_cut(previous, index, spans)?;
    target
        .import_harmonics(
            &imported.history(previous)?,
            &imported.occurrences[..imported.len],
        )
        .map_err(|_| SessionPlaybackError::DeviceRuntime)
}
