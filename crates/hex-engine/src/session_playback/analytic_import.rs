//! Bounded opt-in import of actual sounding shared analytic occurrences.
use super::*;
use crate::{
    midi_note_bank::{PreparedMidiNote, SharedContextRef},
    mutable_instrument::{MutableOccurrence, MutableOccurrenceKey, MutableOccurrenceSignature},
};
use hex_lattice::NativeInstrumentAnalyticSeed;
const CAP: usize = crate::mutable_instrument::MAX_MUTABLE_OCCURRENCE_PROOFS;
pub(super) struct ImportPlan {
    pub(super) seeds: Vec<NativeInstrumentAnalyticSeed>,
    pub(super) proofs: Vec<Option<MutableOccurrence>>,
    pub(super) len: usize,
}
impl ImportPlan {
    pub(super) fn visit_candidate_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self { seeds, proofs, len } = self;
        if *len != 0 || proofs.iter().any(Option::is_some) {
            return Err(crate::AudioStorageError::UnsupportedGraph);
        }
        ledger.vec(seeds)?;
        ledger.vec(proofs)
    }
    // Construct on control; handoff only overwrites these fixed-length buffers.
    pub(super) fn new() -> Result<Self, SessionPlaybackError> {
        let mut seeds = Vec::new();
        seeds
            .try_reserve_exact(CAP)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        let mut proofs = Vec::new();
        proofs
            .try_reserve_exact(CAP)
            .map_err(|_| SessionPlaybackError::Allocation)?;
        if seeds.capacity() != CAP || proofs.capacity() != CAP {
            return Err(SessionPlaybackError::Allocation);
        }
        seeds.resize(CAP, NativeInstrumentAnalyticSeed::default());
        proofs.resize(CAP, None);
        Ok(Self {
            seeds,
            proofs,
            len: 0,
        })
    }

    pub(super) fn backing_bytes(&self) -> Option<usize> {
        self.seeds
            .capacity()
            .checked_mul(std::mem::size_of::<NativeInstrumentAnalyticSeed>())?
            .checked_add(
                self.proofs
                    .capacity()
                    .checked_mul(std::mem::size_of::<Option<MutableOccurrence>>())?,
            )
    }

    pub(super) fn preflight_bytes() -> Option<usize> {
        CAP.checked_mul(std::mem::size_of::<NativeInstrumentAnalyticSeed>())?
            .checked_add(CAP.checked_mul(std::mem::size_of::<Option<MutableOccurrence>>())?)
    }
}
impl SessionPlayback {
    pub(super) fn analytic_import_plan<T: crate::MidiTiming + ?Sized>(
        previous: &Self,
        old_index: usize,
        candidate: &PreparedRevision,
        new_index: usize,
        clock: &T,
        spans: &mut MidiSpanBuffer,
        plan: &mut ImportPlan,
    ) -> Result<bool, SessionPlaybackError> {
        plan.len = 0;
        let old_route = effective_midi_route(&previous.revision, &previous.routes, old_index)?;
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
            return Ok(false);
        }
        let Some(factory) = candidate.session().mutable_instrument_factory(old_route.id) else {
            return Ok(false);
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
            return Ok(false);
        }
        let sources_match = match old_route.owner {
            RouteOwner::Arrangement => previous
                .revision
                .mutable_arrangement_retarget_matches(candidate, old_route.id),
            RouteOwner::Session { clip, .. } => {
                previous
                    .revision
                    .mutable_track_retarget_topology_matches(candidate, old_route.id)
                    && crate::session_inventory::EffectiveSessionMidi::new(
                        previous.revision.session(),
                        previous.midi_admission.as_ref(),
                    )
                    .midi_clip_for_track(clip, old_route.id)
                    .zip(candidate.session().midi_clip_for_track(clip, old_route.id))
                    .is_some_and(|(old, new)| {
                        old.source_content_matches(new) && old.track == new.track
                    })
            }
            _ => false,
        };
        if !sources_match {
            return Ok(false);
        }
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
                binding: crate::midi_note_bank::AnalyticBinding::Current,
                ..
            } = sampler
            else {
                compatible = false;
                return Ok(());
            };
            if previous
                .analytic_continuations
                .get(usize::from(index))
                .copied()
                .flatten()
                .is_some_and(|r| {
                    r.elapsed_at(previous.performance_frame)
                        .map_or(true, |n| n < r.duration())
                })
            {
                compatible = false;
                return Ok(());
            }
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
            let (gain, gain_frozen) = super::clip_gain::at_cut(
                previous,
                old_index,
                old_route.owner,
                signature.key().clip,
                previous.arrangement_frame,
                previous.performance_frame,
            )?;
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
                quiet_deadline_age: None,
                note_off_sent: gate <= cut,
                gain_at_end_bits: gain.to_bits(),
                gain_frozen,
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
                            && previous.loop_region().is_some_and(|r| {
                                r.enabled() && span.start < r.start() && span.gate_end <= r.start()
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
            RouteOwner::Session { clip, origin } => {
                let prepared = crate::session_inventory::EffectiveSessionMidi::new(
                    previous.revision.session(),
                    previous.midi_admission.as_ref(),
                )
                .midi_clip_for_track(clip, old_route.id)
                .ok_or(SessionPlaybackError::UnknownClip)?;
                let end = previous
                    .performance_frame
                    .checked_add(1)
                    .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
                if let Some(floor) = previous.historical_floor_for(old_index) {
                    prepared
                        .instrument_template()
                        .query_after_onset_with_origin(
                            origin,
                            clock,
                            previous.performance_frame,
                            end,
                            spans,
                            floor,
                        )
                } else {
                    prepared.instrument_template().query_with_origin(
                        origin,
                        clock,
                        previous.performance_frame,
                        end,
                        spans,
                    )
                }
                .map_err(SessionPlaybackError::Midi)?;
                for span in spans.spans().copied() {
                    let Some(sampler) = span.prepared_voice() else {
                        plan.len = 0;
                        return Ok(false);
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
            _ => {
                plan.len = 0;
                return Ok(false);
            }
        }
        if !compatible {
            plan.len = 0;
        }
        Ok(compatible)
    }
}
