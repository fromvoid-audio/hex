//! Same-runtime CURRENT route cuts. Admission never mutates DSP or committed notes.
use super::*;
use crate::MidiTiming;
use crate::mutable_instrument::{
    MAX_MUTABLE_OCCURRENCE_PROOFS as PROOFS, MutableOccurrenceKey as Key,
};

struct Keys {
    values: [Option<Key>; PROOFS],
    len: usize,
}
impl Keys {
    fn new() -> Self {
        Self {
            values: [None; PROOFS],
            len: 0,
        }
    }
    fn insert(&mut self, key: Key) -> Result<(), SessionPlaybackError> {
        if self.values[..self.len].contains(&Some(key)) {
            return Ok(());
        }
        let slot = self
            .values
            .get_mut(self.len)
            .ok_or(SessionPlaybackError::ReleaseBankFull)?;
        *slot = Some(key);
        self.len += 1;
        Ok(())
    }
}
fn rebase(frame: u64, source: u64, performance: u64) -> Result<u64, SessionPlaybackError> {
    if frame >= source {
        performance.checked_add(frame - source)
    } else {
        performance.checked_sub(source - frame)
    }
    .ok_or(SessionPlaybackError::ArithmeticOverflow)
}
impl SessionPlayback {
    #[cfg(test)]
    pub(crate) fn test_current_sampler_route_proofs(
        &self,
        index: usize,
    ) -> Option<(u64, usize, usize)> {
        let route = self.routes.get(index)?;
        let proofs = self
            .mutable_instruments
            .get(index)?
            .as_ref()?
            .transition_proofs(false);
        Some((
            route.current_incarnation,
            proofs
                .iter()
                .filter(|p| !p.note_off_sent && p.gate_end_frame > self.performance_frame)
                .count(),
            proofs
                .iter()
                .filter(|p| p.note_off_sent && p.quiet_deadline_frame > self.performance_frame)
                .count(),
        ))
    }
    pub(super) fn current_sampler_route_track(&self, index: usize) -> bool {
        self.current_sampler_console
            && self.revision.resource_scope.is_some()
            && self
                .mutable_instruments
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|runtime| {
                    runtime.is_concrete_current_sampler()
                        && self
                            .revision
                            .arrangement()
                            .tracks()
                            .get(index)
                            .is_some_and(|t| {
                                t.instrument_id == runtime.expression_identity
                                    && t.midi_source_index.is_none_or(|i| i == index)
                            })
                })
    }
    /// Bound actual releases already owned plus outgoing onsets that can still
    /// survive at the future cut. An onset exactly at the cut belongs to incoming.
    pub(super) fn current_route_reservation(
        &mut self,
        index: usize,
        next: RouteOwner,
        now: u64,
        cut: u64,
        arrangement_cut: u64,
        speculative: bool,
    ) -> Result<CurrentRouteReservation, SessionPlaybackError> {
        if !self.current_sampler_route_track(index) {
            return Err(SessionPlaybackError::InvalidCommand);
        }
        let route = self
            .route_scratch
            .get(index)
            .copied()
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let incarnation = route
            .current_incarnation
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        if matches!(next, RouteOwner::Arrangement)
            && self.mutable_destination_has_active_occurrence(index, next, arrangement_cut)?
        {
            return Err(SessionPlaybackError::MutableInstrumentChase);
        }
        let runtime = self.mutable_instruments[index]
            .as_ref()
            .ok_or(SessionPlaybackError::MutableInstrumentRuntime)?;
        let tail = runtime.max_release_tail_frames();
        let cut_end = cut
            .checked_add(tail)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let voice_capacity = runtime.max_active_occurrences();
        let runtime_steals = runtime.steals_releasing_voices();
        let proof_capacity = runtime.transition_proof_capacity();
        let event_capacity = runtime
            .max_events_per_block()
            .min(crate::mutable_instrument::MAX_MUTABLE_NOTE_EVENTS_PER_QUERY)
            .min(self.mutable_events.capacity());
        let mut survivors = Keys::new();
        let mut offs = Keys::new();
        for proof in runtime.transition_proofs(speculative) {
            let key = proof.signature.key();
            let held = !proof.note_off_sent && proof.gate_end_frame >= cut;
            if held {
                offs.insert(key)?;
            }
            let deadline = if held {
                cut_end
            } else {
                proof.quiet_deadline_frame
            };
            if deadline > cut {
                survivors.insert(key)?;
            }
        }
        if cut > now {
            let start = cut
                .checked_sub(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let track_id = route.id;
            match route.owner {
                RouteOwner::Arrangement => {
                    let track = self
                        .revision
                        .arrangement()
                        .tracks()
                        .get(index)
                        .ok_or(SessionPlaybackError::UnknownTrack)?;
                    let source_start = arrangement_cut
                        .checked_sub(1)
                        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                    let mut failure = None;
                    track
                        .for_each_mutable_midi_span(source_start, arrangement_cut, |span| {
                            let result = (|| {
                                let onset = rebase(span.start, arrangement_cut, cut)?;
                                if onset < now || onset >= cut {
                                    return Ok(());
                                }
                                let gate = rebase(span.gate_end, arrangement_cut, cut)?;
                                let end = rebase(span.end, arrangement_cut, cut)?;
                                let key = Key::new(
                                    track_id,
                                    span.clip,
                                    span.note,
                                    span.iteration,
                                    self.arrangement_loop_pass,
                                );
                                if gate >= cut {
                                    offs.insert(key)?;
                                }
                                if (if gate >= cut { cut_end } else { end }) > cut {
                                    survivors.insert(key)?;
                                }
                                Ok(())
                            })();
                            if let Err(e) = result {
                                failure = Some(e)
                            }
                        })
                        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                    if let Some(e) = failure {
                        return Err(e);
                    }
                }
                RouteOwner::Session { clip, origin } => {
                    let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                        self.revision.session(),
                        self.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, track_id)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                    let timing = timing_view_for(
                        self.revision.session(),
                        self.periodic_tempo(),
                        self.anchor,
                    );
                    prepared
                        .instrument_template()
                        .query_with_origin(origin, &timing, start, cut, &mut self.spans)
                        .map_err(SessionPlaybackError::Midi)?;
                    for span in self.spans.spans() {
                        if span.start_frame < now || span.start_frame >= cut {
                            continue;
                        }
                        let key = Key::new(
                            track_id,
                            span.clip,
                            span.note,
                            span.iteration,
                            route.current_incarnation,
                        );
                        if span.gate_end_frame >= cut {
                            offs.insert(key)?;
                        }
                        if (if span.gate_end_frame >= cut {
                            cut_end
                        } else {
                            span.end_frame
                        }) > cut
                        {
                            survivors.insert(key)?;
                        }
                    }
                }
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
            }
        }
        let end = cut
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let (incoming_voices, incoming_onsets) = match next {
            RouteOwner::Session { clip, origin } => {
                let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                    self.revision.session(),
                    self.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, route.id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
                let timing =
                    timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor);
                prepared
                    .instrument_template()
                    .query_with_origin(origin, &timing, cut, end, &mut self.spans)
                    .map_err(SessionPlaybackError::Midi)?;
                (
                    // Held density when the instrument steals releases.
                    prepared.instrument_template().admitted_voices(),
                    self.spans.spans().filter(|s| s.start_frame == cut).count(),
                )
            }
            RouteOwner::Arrangement => {
                let track = self
                    .revision
                    .arrangement()
                    .tracks()
                    .get(index)
                    .ok_or(SessionPlaybackError::UnknownTrack)?;
                let source_end = arrangement_cut
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                let mut onsets = 0usize;
                track
                    .for_each_mutable_midi_span(arrangement_cut, source_end, |span| {
                        if span.start == arrangement_cut {
                            onsets = onsets.saturating_add(1)
                        }
                    })
                    .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                // Quiescent return is guaranteed; active chase is explicitly
                // refused. Future Arrangement notes retain their admitted factory cap.
                (voice_capacity, onsets)
            }
            RouteOwner::SessionAudio { .. } | RouteOwner::Silence => (0, 0),
        };
        let total = survivors
            .len
            .checked_add(incoming_voices)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        let events = offs
            .len
            .checked_add(incoming_onsets)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        // Every survivor is releasing by the cut (held ones receive NoteOff
        // there, before equal-offset NoteOns), so a stealing provider needs
        // voice slots for the incoming voices only. Proofs still bound all.
        let voices = if runtime_steals {
            incoming_voices
        } else {
            total
        };
        if total > proof_capacity
            || voices > voice_capacity
            || events > event_capacity
            || incoming_onsets > crate::mutable_instrument::MAX_MUTABLE_SPANS_PER_QUERY
        {
            return Err(SessionPlaybackError::ReleaseBankFull);
        }
        Ok(CurrentRouteReservation {
            incarnation,
            survivors: survivors.len,
            note_offs: offs.len,
            incoming_voices,
            incoming_onsets,
        })
    }
    /// Validate every due CURRENT destination before any speculative proof cut.
    pub(super) fn preflight_current_due_routes(
        &mut self,
        performance: u64,
        arrangement: u64,
    ) -> Result<(), SessionPlaybackError> {
        if !self.current_sampler_console {
            return Ok(());
        }
        self.current_route_cut_scratch.fill(None);
        for index in 0..self.route_scratch.len() {
            let proven_current = self.current_sampler_route_track(index);
            let Some(pending) = self.route_scratch[index]
                .pending
                .filter(|p| p.frame <= performance)
            else {
                continue;
            };
            let is_audio = matches!(pending.target,RouteTarget::Session(clip) if self.revision.session().audio_clip(clip).is_some());
            let beat = if matches!(pending.target, RouteTarget::Session(_)) && !is_audio {
                timing_view_for(self.revision.session(), self.periodic_tempo(), self.anchor)
                    .frame_to_beat(performance)
                    .map_err(|_| SessionPlaybackError::TempoConversion)?
            } else {
                pending.beat
            };
            let owner = self.owner_for_target(pending.target, beat, performance)?;
            if !proven_current {
                if pending.current_reservation.is_some() {
                    return Err(SessionPlaybackError::InvalidCommand);
                }
                let slot = self
                    .current_route_cut_scratch
                    .get_mut(index)
                    .ok_or(SessionPlaybackError::InvalidCommand)?;
                *slot = Some(CurrentPendingCut {
                    current_sampler_cut: false,
                    owner,
                    incarnation: self.route_scratch[index].current_incarnation,
                    frame: performance,
                });
                continue;
            }
            let Some(reserved) = pending.current_reservation else {
                if owner != self.route_scratch[index].owner {
                    return Err(SessionPlaybackError::InvalidCommand);
                }
                let slot = self
                    .current_route_cut_scratch
                    .get_mut(index)
                    .ok_or(SessionPlaybackError::InvalidCommand)?;
                *slot = Some(CurrentPendingCut {
                    current_sampler_cut: false,
                    owner,
                    incarnation: self.route_scratch[index].current_incarnation,
                    frame: performance,
                });
                continue;
            };
            let actual = self.current_route_reservation(
                index,
                owner,
                performance,
                performance,
                arrangement,
                true,
            )?;
            if actual.incarnation != reserved.incarnation
                || actual.survivors > reserved.survivors
                || actual.note_offs > reserved.note_offs
                || actual.incoming_voices != reserved.incoming_voices
                || actual.incoming_onsets > reserved.incoming_onsets
            {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let slot = self
                .current_route_cut_scratch
                .get_mut(index)
                .ok_or(SessionPlaybackError::InvalidCommand)?;
            *slot = Some(CurrentPendingCut {
                current_sampler_cut: true,
                owner,
                incarnation: reserved.incarnation,
                frame: performance,
            });
        }
        Ok(())
    }
}
