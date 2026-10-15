//! Bounded opt-in import of actual sounding shared analytic occurrences.
use super::*;
use crate::{
    midi_note_bank::{PreparedMidiNote, SharedContextRef},
    mutable_instrument::{MutableOccurrence, MutableOccurrenceKey, MutableOccurrenceSignature},
};
use hex_lattice::NativeInstrumentAnalyticSeed;
const CAP: usize = crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS;
pub(super) struct ImportPlan {
    pub(super) seeds: [NativeInstrumentAnalyticSeed; CAP],
    pub(super) proofs: [Option<MutableOccurrence>; CAP],
    pub(super) len: usize,
}
impl SessionPlayback {
    pub(super) fn analytic_import_plan<T: crate::MidiTiming + ?Sized>(
        previous: &Self,
        old_index: usize,
        candidate: &PreparedRevision,
        new_index: usize,
        clock: &T,
        spans: &mut MidiSpanBuffer,
    ) -> Result<Option<ImportPlan>, SessionPlaybackError> {
        let old_route = previous
            .routes
            .get(old_index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        if previous
            .mutable_instruments
            .get(old_index)
            .is_some_and(Option::is_some)
            || old_route.pending.is_some()
            || previous.releases.iter().any(|r| r.track_id == old_route.id)
            || previous
                .suppressions
                .iter()
                .any(|s| s.track == old_route.id)
            || previous
                .tail_mutable_instruments
                .iter()
                .flatten()
                .any(|t| t.track_id == old_route.id)
        {
            return Ok(None);
        }
        let Some(factory) = candidate.session().mutable_instrument_factory(old_route.id) else {
            return Ok(None);
        };
        let old_track = previous
            .revision
            .arrangement()
            .tracks()
            .get(old_index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        let new_track = candidate
            .arrangement()
            .tracks()
            .get(new_index)
            .ok_or(SessionPlaybackError::UnknownTrack)?;
        if !old_track.instrument_slot_enabled() || !new_track.instrument_slot_enabled() {
            return Ok(None);
        }
        let sources_match = match old_route.owner {
            RouteOwner::Arrangement => previous
                .revision
                .mutable_arrangement_retarget_matches(candidate, old_route.id),
            RouteOwner::Session { clip, .. } => {
                previous
                    .revision
                    .mutable_track_retarget_topology_matches(candidate, old_route.id)
                    && previous
                        .revision
                        .session()
                        .midi_clip(clip)
                        .zip(candidate.session().midi_clip(clip))
                        .is_some_and(|(old, new)| {
                            old.source_content_matches(new) && old.track == new.track
                        })
            }
            _ => false,
        };
        if !sources_match {
            return Ok(None);
        }
        let mut plan = ImportPlan {
            seeds: [NativeInstrumentAnalyticSeed::default(); CAP],
            proofs: [None; CAP],
            len: 0,
        };
        let mut compatible = true;
        let mut append = |signature: MutableOccurrenceSignature,
                          start: u64,
                          gate: u64,
                          end: u64,
                          cut: u64,
                          sampler: PreparedMidiNote|
         -> Result<(), SessionPlaybackError> {
            if start >= cut {
                return Ok(());
            }
            let PreparedMidiNote::Shared {
                context: SharedContextRef::Revision(index),
                note,
            } = sampler
            else {
                compatible = false;
                return Ok(());
            };
            let context = previous
                .revision
                .session()
                .shared_contexts()
                .owner(index)
                .ok_or(SessionPlaybackError::DeviceRuntime)?;
            if !factory.can_import_analytic_context(context.context()) {
                compatible = false;
                return Ok(());
            }
            if plan.len >= CAP {
                return Err(SessionPlaybackError::ReleaseBankFull);
            }
            let age = cut
                .checked_sub(start)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let id = (plan.len + 1) as u64;
            plan.seeds[plan.len] = NativeInstrumentAnalyticSeed {
                occurrence_id: id,
                note,
                age_frames: age,
                gate_frame: (gate <= cut).then_some(gate.saturating_sub(start)),
            };
            plan.proofs[plan.len] = Some(MutableOccurrence {
                signature,
                id,
                onset_frame: previous.performance_frame,
                gate_end_frame: previous
                    .performance_frame
                    .checked_add(gate.saturating_sub(cut))
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                quiet_deadline_frame: previous
                    .performance_frame
                    .checked_add(
                        end.checked_sub(cut)
                            .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                    )
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
                note_off_sent: gate <= cut,
            });
            plan.len += 1;
            Ok(())
        };
        match old_route.owner {
            RouteOwner::Arrangement => {
                let cut = previous.arrangement_frame;
                let mut failure = None;
                old_track
                    .for_each_active_midi_span(cut, |span, sampler| {
                        if failure.is_some() {
                            return;
                        }
                        if previous
                            .historical_floor_for(old_index)
                            .is_some_and(|floor| span.onset_beat < floor)
                        {
                            return;
                        }
                        if previous.arrangement_loop_pass > 0
                            && previous
                                .revision
                                .arrangement()
                                .loop_region()
                                .is_some_and(|r| {
                                    r.enabled()
                                        && span.start < r.start()
                                        && span.gate_end <= r.start()
                                })
                        {
                            return;
                        }
                        let signature = MutableOccurrenceSignature::from_parts(
                            MutableOccurrenceKey::new(
                                old_route.id,
                                span.clip,
                                span.note,
                                span.iteration,
                                previous.arrangement_loop_pass,
                            ),
                            span.onset_beat,
                            span.gate_end_beat,
                            span.pitch,
                            span.channel,
                            span.velocity,
                        );
                        if let Err(e) =
                            append(signature, span.start, span.gate_end, span.end, cut, sampler)
                        {
                            failure = Some(e);
                        }
                    })
                    .map_err(|e| SessionPlaybackError::Arrangement(e.0))?;
                if let Some(e) = failure {
                    return Err(e);
                }
            }
            RouteOwner::Session { clip, launch } => {
                let prepared = previous
                    .revision
                    .session()
                    .midi_clip(clip)
                    .ok_or(SessionPlaybackError::UnknownClip)?;
                let end = previous
                    .performance_frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                if let Some(floor) = previous.historical_floor_for(old_index) {
                    prepared.instrument_template().query_after_onset(
                        launch,
                        clock,
                        previous.performance_frame,
                        end,
                        spans,
                        floor,
                    )
                } else {
                    prepared.instrument_template().query(
                        launch,
                        clock,
                        previous.performance_frame,
                        end,
                        spans,
                    )
                }
                .map_err(SessionPlaybackError::Midi)?;
                for span in spans.spans().copied() {
                    let Some(sampler) = span.prepared_voice() else {
                        return Ok(None);
                    };
                    let signature = MutableOccurrenceSignature::from_parts(
                        MutableOccurrenceKey::new(
                            old_route.id,
                            span.clip,
                            span.note,
                            span.iteration,
                            0,
                        ),
                        span.onset_beat,
                        span.gate_end_beat,
                        span.pitch,
                        span.channel,
                        span.velocity,
                    );
                    append(
                        signature,
                        span.start_frame,
                        span.gate_end_frame,
                        span.end_frame,
                        previous.performance_frame,
                        sampler,
                    )?;
                }
            }
            _ => return Ok(None),
        }
        if compatible { Ok(Some(plan)) } else { Ok(None) }
    }
}
