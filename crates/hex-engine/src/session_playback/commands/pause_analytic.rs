//! Transactional analytic Pause capture; musical time and authored data stay fixed.
use super::*;
use std::sync::Arc;
impl SessionPlayback {
    pub(super) fn pause_analytic_sources(&mut self) -> Result<(), SessionPlaybackError> {
        let revision = Arc::clone(&self.revision);
        self.route_scratch.copy_from_slice(&self.routes);
        self.release_scratch.clear();
        self.release_scratch.extend(
            self.releases
                .iter()
                .copied()
                .filter(|entry| entry.end_frame() > self.performance_frame),
        );
        self.release_order_scratch = self.release_order;
        self.suppression_scratch.clear();
        self.suppression_scratch
            .extend_from_slice(&self.suppressions);
        let performance = self.performance_frame;
        // Historical onsets may already live in the retained release bank
        // after tempo/source handoff. They cannot be queried again under the
        // new clock, but their held gates still belong to clips and Pause must
        // release them at the existing source age.
        let mut index = 0;
        while index < self.release_scratch.len() {
            let entry = &mut self.release_scratch[index];
            let cut = entry.sample_frame(performance)?;
            if let Some(voice) = entry
                .voice
                .reconcile_at(cut, None)
                .map_err(|_| SessionPlaybackError::ArithmeticOverflow)?
            {
                entry.voice = voice;
                index += 1;
            } else {
                self.release_scratch.remove(index);
            }
        }
        let end = performance
            .checked_add(1)
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
        for index in 0..self.routes.len() {
            let route = effective_midi_route(&revision, &self.routes, index)?;
            let track = &revision.arrangement().tracks()[index];
            if !track.source_input_enabled() {
                continue;
            }
            let analytic = match route.owner {
                RouteOwner::Arrangement => track
                    .instrument
                    .as_ref()
                    .is_some_and(|instrument| !instrument.mutable),
                RouteOwner::Session { clip, .. } => {
                    crate::session_inventory::EffectiveSessionMidi::new(
                        revision.session(),
                        self.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, route.id)
                    .is_some_and(|clip| {
                        (clip.instrument_enabled()
                            || revision
                                .source_routing()
                                .receiver_index(index)
                                .and_then(|receiver| revision.arrangement().tracks().get(receiver))
                                .is_some_and(crate::Track::instrument_slot_enabled))
                            && !clip.instrument_template().is_event_source()
                    })
                }
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => false,
            };
            if !analytic {
                continue;
            }
            self.capture_outgoing_with_source_binding(
                index,
                route.owner,
                self.arrangement_frame,
                performance,
                true,
            )?;
            match route.owner {
                RouteOwner::Arrangement => {
                    let mut failure = None;
                    // Arrangement repeats reuse clip/note identities. Expire
                    // this pass's suppression exactly at the next loop cut.
                    let pass_end = self
                        .active_loop_region()
                        .filter(|region| region.enabled())
                        .map(|region| {
                            region
                                .end()
                                .checked_sub(self.arrangement_frame)
                                .and_then(|frames| performance.checked_add(frames))
                                .ok_or(SessionPlaybackError::ArithmeticOverflow)
                        })
                        .transpose()?;
                    track
                        .for_each_active_midi_span(self.arrangement_frame, |span, _| {
                            if span.start >= self.arrangement_frame {
                                return;
                            }
                            let Some(until) = span
                                .end
                                .checked_sub(self.arrangement_frame)
                                .and_then(|frames| performance.checked_add(frames))
                            else {
                                failure = Some(SessionPlaybackError::ArithmeticOverflow);
                                return;
                            };
                            let until = pass_end.map_or(until, |pass_end| until.min(pass_end));
                            if let Err(error) = pause_suppress(
                                &mut self.suppression_scratch,
                                self.release_capacity,
                                route.id,
                                span.clip,
                                span.note,
                                span.iteration,
                                until,
                            ) {
                                failure = Some(error);
                            }
                        })
                        .map_err(|error| SessionPlaybackError::Arrangement(error.0))?;
                    if let Some(error) = failure {
                        return Err(error);
                    }
                }
                RouteOwner::Session { clip, origin } => {
                    let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                        revision.session(),
                        self.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, route.id)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                    let timing =
                        timing_view_for(revision.session(), self.periodic_tempo(), self.anchor);
                    let template = prepared.instrument_template();
                    if let Some(floor) = self.historical_floor_for(index) {
                        template.query_after_onset_with_origin(
                            origin,
                            &timing,
                            performance,
                            end,
                            &mut self.spans,
                            floor,
                        )
                    } else {
                        template.query_with_origin(
                            origin,
                            &timing,
                            performance,
                            end,
                            &mut self.spans,
                        )
                    }
                    .map_err(SessionPlaybackError::Midi)?;
                    for span in self.spans.spans() {
                        if span.start_frame < performance {
                            pause_suppress(
                                &mut self.suppression_scratch,
                                self.release_capacity,
                                route.id,
                                span.clip,
                                span.note,
                                span.iteration,
                                span.end_frame,
                            )?;
                        }
                    }
                }
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
            }
        }
        self.check_release_reservations(&self.route_scratch, self.release_scratch.len())?;
        self.release_scratch.sort_unstable_by_key(|entry| {
            (
                entry.location.track_index(),
                entry.location.tail_group(),
                entry.order,
            )
        });
        self.suppression_scratch.sort_unstable_by_key(|entry| {
            (
                entry.track,
                entry.clip,
                entry.note,
                entry.iteration,
                entry.loop_pass,
                entry.mutable_signature(),
            )
        });
        std::mem::swap(&mut self.releases, &mut self.release_scratch);
        std::mem::swap(&mut self.suppressions, &mut self.suppression_scratch);
        self.release_order = self.release_order_scratch;
        Ok(())
    }
}
fn pause_suppress(
    entries: &mut Vec<MidiOccurrenceSuppression>,
    capacity: usize,
    track: TrackId,
    clip: ClipId,
    note: NoteId,
    iteration: u64,
    until: u64,
) -> Result<(), SessionPlaybackError> {
    if let Some(existing) = entries.iter_mut().find(|entry| {
        (
            entry.track,
            entry.clip,
            entry.note,
            entry.iteration,
            entry.loop_pass,
        ) == (track, clip, note, iteration, 0)
    }) {
        existing.until_frame = existing.until_frame.max(until);
        return Ok(());
    }
    if entries.len() >= capacity || entries.len() >= entries.capacity() {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    entries.push(MidiOccurrenceSuppression::analytic(
        crate::mutable_instrument::MutableOccurrenceKey::new(track, clip, note, iteration, 0),
        until,
    ));
    Ok(())
}
