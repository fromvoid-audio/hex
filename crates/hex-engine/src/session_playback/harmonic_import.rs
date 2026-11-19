//! Borrowed, checked analytic-to-owned-history adoption at one committed cut.
use super::*;
use crate::midi_note_bank::PreparedMidiNote;
use crate::mutable_instrument::{
    ImportedLiveOccurrence, MutableOccurrence, MutableOccurrenceKey, MutableOccurrenceSignature,
};
use hex_lattice::{
    AnalyticInstrumentHistory, NativeInstrumentAnalyticSeed, PreparedSharedAnalyticNote,
};
const CAPACITY: usize = crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS;
pub(super) struct Import {
    context: u16,
    pub voices: [NativeInstrumentAnalyticSeed; CAPACITY],
    pub occurrences: [Option<MutableOccurrence>; CAPACITY],
    live_origins: [Option<ImportedLiveOccurrence>; CAPACITY],
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
        loop_pass: u64,
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
        gain_override: Option<f64>,
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
        let index = previous
            .routes
            .iter()
            .position(|r| r.id == route.id)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let (gain, gain_frozen) = match gain_override {
            Some(gain) => (gain, true),
            None => super::clip_gain::at_cut(
                previous,
                index,
                route.owner,
                clip,
                previous.arrangement_frame,
                previous.performance_frame,
            )?,
        };
        self.occurrences[self.len] = Some(MutableOccurrence {
            signature: MutableOccurrenceSignature::from_parts(
                MutableOccurrenceKey::new(route.id, clip, note, iteration, loop_pass),
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
            quiet_deadline_age: None,
            note_off_sent: gate <= cut,
            gain_at_end_bits: gain.to_bits(),
            gain_frozen,
            contributor_fade: None,
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
    let context = previous
        .revision
        .analytic_context_for_track(previous.routes[index].id)
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
        live_origins: [None; CAPACITY],
        len: 0,
    };
    let routing = previous.revision.source_routing();
    if routing.is_shared(index) {
        for &contributor in routing.contributors(index) {
            let contributor = usize::from(contributor);
            if previous.revision.arrangement().tracks()[contributor].source_input_enabled() {
                append_track(&mut out, previous, contributor, spans)?;
            }
        }
    } else {
        append_track(&mut out, previous, index, spans)?;
    }
    if let Some(keyboard) = previous.computer_keyboard.as_ref().filter(|keyboard| {
        keyboard.has_active_analytic_source_voices()
            && previous
                .routes
                .iter()
                .position(|route| route.id == keyboard.track())
                .is_some_and(|keyboard_index| {
                    routing.source(keyboard_index) == routing.source(index)
                })
    }) {
        let (voices, len) = keyboard
            .analytic_source_voices()
            .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
        for voice in voices[..len].iter().flatten() {
            if out.len >= CAPACITY {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let id = (out.len + 1) as u64;
            out.voices[out.len] = NativeInstrumentAnalyticSeed {
                occurrence_id: id,
                note: voice.note,
                age_frames: voice.age_frames,
                gate_frame: voice.gate_frame,
            };
            out.live_origins[out.len] = Some(ImportedLiveOccurrence {
                input: voice.input,
                token: voice.token,
                id,
            });
            out.len += 1;
        }
    }
    Ok(out)
}
fn append_track(
    out: &mut Import,
    previous: &SessionPlayback,
    index: usize,
    spans: &mut MidiSpanBuffer,
) -> Result<(), SessionPlaybackError> {
    let route = effective_midi_route(&previous.revision, &previous.routes, index)?;
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
                    if previous
                        .historical_floor_for(index)
                        .is_some_and(|floor| span.onset_beat < floor)
                    {
                        return;
                    }
                    if previous.arrangement_loop_pass > 0
                        && previous.loop_region().is_some_and(|region| {
                            region.enabled()
                                && span.start < region.start()
                                && span.gate_end <= region.start()
                        })
                    {
                        return;
                    }
                    failure = out
                        .push(
                            previous,
                            route,
                            previous.arrangement_loop_pass,
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
                            None,
                        )
                        .err();
                })
                .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
            if let Some(error) = failure {
                return Err(error);
            }
        }
        RouteOwner::Session { clip, origin } => {
            let clip = crate::session_inventory::EffectiveSessionMidi::new(
                previous.revision.session(),
                previous.midi_admission.as_ref(),
            )
            .midi_clip_for_track(clip, route.id)
            .ok_or(SessionPlaybackError::UnknownClip)?;
            if clip.track != route.id {
                return Err(SessionPlaybackError::ClipTrackMismatch);
            }
            let clock = timing_view_for(
                previous.revision.session(),
                previous.periodic_tempo(),
                previous.anchor,
            );
            let frame = previous.performance_frame;
            let end = frame
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            if let Some(floor) = previous.historical_floor_for(index) {
                clip.instrument_template()
                    .query_after_onset_with_origin(origin, &clock, frame, end, spans, floor)
            } else {
                clip.instrument_template()
                    .query_with_origin(origin, &clock, frame, end, spans)
            }
            .map_err(SessionPlaybackError::Midi)?;
            for span in spans.spans() {
                let sampler = span
                    .voice
                    .ok_or(SessionPlaybackError::SharedAnalyticContext)?;
                out.push(
                    previous,
                    route,
                    if previous.revision.source_routing().source(index).is_some() {
                        route.current_incarnation
                    } else {
                        0
                    },
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
                    None,
                )?;
            }
        }
        RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
    }
    // Current-owner bridges have already been suppressed in the route query.
    // Import them once; detached historical generations keep their original law.
    for entry in &previous.releases {
        if entry.track_id != route.id || !imports_release(previous, index, entry) {
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
            match route.owner {
                RouteOwner::Arrangement => previous.arrangement_loop_pass,
                _ if previous.revision.source_routing().source(index).is_some() => {
                    route.current_incarnation
                }
                _ => 0,
            },
            entry.voice.sampler(),
            entry.voice.start_frame(),
            entry.voice.gate_end_frame(),
            entry.voice.end_frame(),
            entry.sample_frame(previous.performance_frame)?,
            c.key.clip,
            c.key.note,
            c.key.iteration,
            c.onset_beat,
            c.gate_end_beat,
            c.pitch,
            c.channel,
            c.velocity,
            false,
            Some(entry.clip_gain),
        )?;
    }
    Ok(())
}
pub(super) fn imports_release(
    previous: &SessionPlayback,
    index: usize,
    entry: &ReleaseEntry,
) -> bool {
    let routing = previous.revision.source_routing();
    let belongs = entry.track_id == previous.routes[index].id
        || (routing.is_shared(index)
            && entry
                .source_origin
                .is_some_and(|origin| Some(origin.source) == routing.source(index)));
    belongs
        && entry.location.tail_group().is_none()
        && entry.voice.sampler().is_current()
        && entry.voice.context_ref()
            == previous
                .revision
                .analytic_context_for_track(entry.track_id)
                .map(crate::midi_note_bank::SharedContextRef::Revision)
        && entry.end_frame() > previous.performance_frame
}

#[inline(never)]
pub(super) fn has_active_history(
    previous: &SessionPlayback,
    index: usize,
    spans: &mut MidiSpanBuffer,
) -> Result<bool, SessionPlaybackError> {
    Ok(at_cut(previous, index, spans)?.len != 0)
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
    let supported = target.can_import_harmonics_with_live(
        &imported.history(previous)?,
        &imported.occurrences[..imported.len],
        &imported.live_origins[..imported.len],
    );
    if !supported && imported.len != 0 && previous.revision.source_routing().is_shared(index) {
        // The legacy per-track fallback cannot prove a complete shared-source
        // history. Refuse before it could admit only the receiver's voices.
        return Err(SessionPlaybackError::SourceHistoryUnavailable);
    }
    Ok(supported)
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
        .import_harmonics_with_live(
            &imported.history(previous)?,
            &imported.occurrences[..imported.len],
            &imported.live_origins[..imported.len],
        )
        .map_err(|_| SessionPlaybackError::DeviceRuntime)
}
