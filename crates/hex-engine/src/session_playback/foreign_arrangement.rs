//! Arrangement gate ownership for prepared VST3 generators. The engine owns
//! the bounded note ledger. The audio host owns the worker and accepts or
//! revokes staged events with the complete physical callback.
use super::{MAX_TRACKS, PreparedRevision, SessionPlaybackError};
use crate::mutable_instrument::{
    MutableInstrumentRuntimeError, MutableNoteOnset, MutableOccurrence, MutableOccurrenceKey,
    MutableOccurrenceSignature, MutableOnsetBuffer,
    authored_gate::{AuthoredGate, AuthoredGateContext, MAX_FOREIGN_NOTES, stage_authored_gates},
};
use crate::{Frame, Track};
use hex_lattice::{
    ForeignNote, ForeignProcessorNode, ForeignProcessors, NativeInstrumentClock,
    PreparedForeignProcessor, PreparedProcessorFactory,
};
use hex_project::TrackId;
use std::mem::size_of;

#[derive(Default)]
struct Ledger {
    notes: Vec<MutableOccurrence>,
    next_id: u64,
    loop_pass: Option<u64>,
    cut: bool,
}

impl Ledger {
    fn has_held_notes(&self) -> bool {
        self.notes.iter().any(|note| !note.note_off_sent)
    }
}

struct Generator {
    track: TrackId,
    processor: PreparedForeignProcessor,
    committed: Ledger,
    scratch: Ledger,
}

impl Generator {
    fn prepare(track: &Track) -> Result<Self, SessionPlaybackError> {
        fn ledger() -> Result<Ledger, SessionPlaybackError> {
            let mut ledger = Ledger::default();
            ledger
                .notes
                .try_reserve_exact(MAX_FOREIGN_NOTES)
                .map_err(|_| SessionPlaybackError::Allocation)?;
            Ok(ledger)
        }
        let track_id = track
            .canonical_track_id()
            .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
        let Some(PreparedProcessorFactory::Foreign(processor)) = track
            .foreign_instrument
            .as_ref()
            .and_then(|node| node.definition.as_ref())
        else {
            return Err(SessionPlaybackError::DeviceRuntime);
        };
        if processor.binding().is_none() {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        Ok(Self {
            track: track_id,
            processor: *processor,
            committed: ledger()?,
            scratch: ledger()?,
        })
    }
}

/// Index mapping only. No borrowed or owned endpoint crosses this boundary.
pub(super) struct Handoff {
    old_indices: [Option<usize>; MAX_TRACKS],
    pub(super) requires_clock_continuity: bool,
}

pub(super) const INCOMPATIBLE_HANDOFF: SessionPlaybackError =
    SessionPlaybackError::MutableInstrumentDiagnostic(
        "vst3 instrument graph change requires stopped playback and released notes",
    );

#[derive(Default)]
pub(super) struct ForeignArrangement {
    generators: Vec<Option<Generator>>,
    events: Vec<ForeignNote>,
}

fn has_generator(track: &Track) -> bool {
    track
        .foreign_instrument
        .as_ref()
        .is_some_and(|node| !node.bypassed)
}

impl ForeignArrangement {
    pub(super) fn prepare(revision: &PreparedRevision) -> Result<Self, SessionPlaybackError> {
        let tracks = revision.arrangement().tracks();
        let mut generators = Vec::new();
        let mut events = Vec::new();
        if tracks.iter().any(has_generator) {
            events
                .try_reserve_exact(MAX_FOREIGN_NOTES)
                .map_err(|_| SessionPlaybackError::Allocation)?;
            generators
                .try_reserve_exact(tracks.len())
                .map_err(|_| SessionPlaybackError::Allocation)?;
            for (index, track) in tracks.iter().enumerate() {
                if has_generator(track) {
                    if revision.source_routing().is_shared(index)
                        || track.midi_source_index.is_some()
                    {
                        return Err(SessionPlaybackError::MutableInstrumentDiagnostic(
                            "routed vst3 instrument sources are unavailable",
                        ));
                    }
                    generators.push(Some(Generator::prepare(track)?));
                } else {
                    generators.push(None);
                }
            }
        }
        Ok(Self { generators, events })
    }

    pub(super) fn preflight_bytes(revision: &PreparedRevision) -> Option<usize> {
        let tracks = revision.arrangement().tracks();
        let count = tracks.iter().filter(|track| has_generator(track)).count();
        if count == 0 {
            return Some(0);
        }
        tracks
            .len()
            .checked_mul(size_of::<Option<Generator>>())?
            .checked_add(
                count.checked_mul(2 * MAX_FOREIGN_NOTES * size_of::<MutableOccurrence>())?,
            )?
            .checked_add(MAX_FOREIGN_NOTES * size_of::<ForeignNote>())
    }

    pub(super) fn storage_bytes(&self) -> Option<usize> {
        self.generators.iter().flatten().try_fold(
            self.generators
                .capacity()
                .checked_mul(size_of::<Option<Generator>>())?
                .checked_add(
                    self.events
                        .capacity()
                        .checked_mul(size_of::<ForeignNote>())?,
                )?,
            |bytes, generator| {
                bytes.checked_add(
                    generator
                        .committed
                        .notes
                        .capacity()
                        .checked_add(generator.scratch.notes.capacity())?
                        .checked_mul(size_of::<MutableOccurrence>())?,
                )
            },
        )
    }

    pub(super) fn is_empty(&self) -> bool {
        self.generators.is_empty()
    }

    pub(super) fn has_started(&self) -> bool {
        self.generators
            .iter()
            .flatten()
            .any(|generator| generator.committed.loop_pass.is_some())
    }

    pub(super) fn plan_handoff(
        &self,
        old: &Self,
        tracks: &[Track],
        cut: FrameContext,
    ) -> Result<Handoff, SessionPlaybackError> {
        let mut plan = Handoff {
            old_indices: [None; MAX_TRACKS],
            requires_clock_continuity: false,
        };
        let mut retained = [false; MAX_TRACKS];
        for (index, candidate) in self.generators.iter().enumerate() {
            let Some(candidate) = candidate else { continue };
            let matched = old.generators.iter().enumerate().find_map(|(index, old)| {
                let old = old.as_ref()?;
                (old.track == candidate.track && old.processor == candidate.processor)
                    .then_some((index, &old.committed))
            });
            let Some((old_index, ledger)) = matched else {
                if cut.playing {
                    return Err(INCOMPATIBLE_HANDOFF);
                }
                continue;
            };
            *plan
                .old_indices
                .get_mut(index)
                .ok_or(SessionPlaybackError::RevisionTrackMismatch)? = Some(old_index);
            *retained
                .get_mut(old_index)
                .ok_or(SessionPlaybackError::RevisionTrackMismatch)? = true;
            plan.requires_clock_continuity |=
                ledger.loop_pass.is_some() && (cut.playing || ledger.has_held_notes());
            if !ledger.cut && ledger.loop_pass == Some(cut.loop_pass) {
                let track = tracks
                    .get(index)
                    .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
                validate_held_gates(track, candidate.track, ledger, &cut)?;
            }
        }
        for (index, generator) in old.generators.iter().enumerate() {
            if let Some(generator) = generator
                && !retained.get(index).copied().unwrap_or(false)
                && generator.committed.loop_pass.is_some()
                && (cut.playing || generator.committed.has_held_notes())
            {
                return Err(INCOMPATIBLE_HANDOFF);
            }
        }
        Ok(plan)
    }

    /// Commit only after the graph preflight. Both graphs keep their reserved
    /// buffers, and the host retains the exact worker on its existing binding.
    pub(super) fn inherit_handoff(&mut self, old: &mut Self, plan: Handoff, keep_clock: bool) {
        for (candidate, old_index) in self.generators.iter_mut().zip(plan.old_indices) {
            if let Some((candidate, donor)) = candidate.as_mut().zip(
                old_index
                    .and_then(|index| old.generators.get_mut(index))
                    .and_then(Option::as_mut),
            ) {
                if keep_clock {
                    std::mem::swap(&mut candidate.committed, &mut donor.committed);
                } else {
                    // A stopped reset has no held gates, but the retained
                    // worker still needs monotonically increasing note IDs.
                    candidate.committed.next_id = donor.committed.next_id;
                }
            }
        }
    }

    pub(super) fn begin(&mut self) {
        for generator in self.generators.iter_mut().flatten() {
            let Generator {
                committed, scratch, ..
            } = generator;
            scratch.notes.clear();
            scratch.notes.extend_from_slice(&committed.notes);
            scratch.next_id = committed.next_id;
            scratch.loop_pass = committed.loop_pass;
            scratch.cut = committed.cut;
        }
    }

    pub(super) fn commit(&mut self) {
        for generator in self.generators.iter_mut().flatten() {
            std::mem::swap(&mut generator.committed, &mut generator.scratch);
        }
    }

    pub(super) fn cut(&mut self) {
        for generator in self.generators.iter_mut().flatten() {
            generator.committed.cut = true;
        }
    }

    #[allow(clippy::too_many_arguments)] // Borrow the existing callback and its disjoint scratch.
    pub(super) fn render_frame(
        &mut self,
        index: usize,
        track: &Track,
        context: FrameContext,
        output: &mut [Frame; 1],
        onsets: &mut MutableOnsetBuffer,
        offset: usize,
        foreign: &mut dyn ForeignProcessors,
        clock: &dyn NativeInstrumentClock,
    ) -> Result<(), SessionPlaybackError> {
        let Some(runtime) = self.generators.get_mut(index).and_then(Option::as_mut) else {
            return Ok(());
        };
        let ledger = &mut runtime.scratch;
        if !context.playing && ledger.loop_pass.is_none() {
            return Ok(());
        }
        let node = track
            .foreign_instrument
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let Some(PreparedProcessorFactory::Foreign(prepared)) = node.definition else {
            return Err(SessionPlaybackError::DeviceRuntime);
        };
        let configuration = node
            .configuration
            .as_deref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let events = &mut self.events;
        events.clear();
        let mut emit = |gate| -> Result<(), MutableInstrumentRuntimeError> {
            if events.len() == MAX_FOREIGN_NOTES {
                return Err(MutableInstrumentRuntimeError::OccurrenceCapacity);
            }
            events.push(ForeignNote::try_from(gate)?);
            Ok(())
        };
        let wrapped = !ledger.cut
            && ledger
                .loop_pass
                .is_some_and(|pass| pass != context.loop_pass);
        if ledger.cut || wrapped {
            for occurrence in ledger.notes.drain(..).filter(|note| !note.note_off_sent) {
                emit(AuthoredGate::release_now(occurrence))
                    .map_err(|error| error.render_error())?;
            }
            ledger.cut = false;
        }
        if context.playing {
            let id = track
                .canonical_track_id()
                .ok_or(SessionPlaybackError::RevisionTrackMismatch)?;
            onsets.clear();
            let end = context
                .arrangement
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            let mut overflow = false;
            track
                .for_each_mutable_midi_span(context.arrangement, end, |span| {
                    if span.start < context.arrangement && !(context.chase || wrapped) {
                        return;
                    }
                    overflow |= onsets
                        .push(MutableNoteOnset {
                            key: MutableOccurrenceKey::new(
                                id,
                                span.clip,
                                span.note,
                                span.iteration,
                                context.loop_pass,
                            ),
                            onset_beat: span.onset_beat,
                            gate_end_beat: span.gate_end_beat,
                            source_start: span.start.max(context.arrangement),
                            source_gate_end: span.gate_end,
                            pitch: span.pitch,
                            channel: span.channel,
                            velocity: span.velocity,
                        })
                        .is_err();
                })
                .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
            if overflow {
                return Err(SessionPlaybackError::MutableInstrumentEvents);
            }
            stage_authored_gates(
                &mut ledger.notes,
                &mut ledger.next_id,
                onsets,
                AuthoredGateContext {
                    track: Some(id),
                    performance_start: context.performance,
                    source_start: context.arrangement,
                    width: 1,
                    source_age: None,
                    tail_frames: 0,
                    max_active: MAX_FOREIGN_NOTES,
                    steals_releasing: true,
                    live: 0,
                    suppressions: &[],
                },
                emit,
            )
            .map_err(|error| error.render_error())?;
        }
        ledger.loop_pass = Some(context.loop_pass);
        let node = ForeignProcessorNode {
            owner: node.owner,
            device_id: node.device_id,
            device_type: node.device_type,
            sample_rate: node.sample_rate,
            configuration,
            prepared,
            bypassed: false,
            // The compensated generator merge owns the instrument output gain.
            input_gain: 1.0,
            output_gain: 1.0,
        };
        output.fill([0.0; 2]);
        foreign
            .process_segment_with_notes(node, offset, output, &[], events, Some(clock))
            .map_err(|_| SessionPlaybackError::DeviceRuntime)
    }
}

fn validate_held_gates(
    track: &Track,
    track_id: TrackId,
    ledger: &Ledger,
    cut: &FrameContext,
) -> Result<(), SessionPlaybackError> {
    let mut unmatched = [false; MAX_FOREIGN_NOTES];
    for (flag, note) in unmatched.iter_mut().zip(&ledger.notes) {
        *flag = !note.note_off_sent && note.gate_end_frame > cut.performance;
    }
    if !unmatched.contains(&true) {
        return Ok(());
    }
    let end = cut
        .arrangement
        .checked_add(1)
        .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
    track
        .for_each_mutable_midi_span(cut.arrangement, end, |span| {
            let signature = MutableOccurrenceSignature::from_parts(
                MutableOccurrenceKey::new(
                    track_id,
                    span.clip,
                    span.note,
                    span.iteration,
                    cut.loop_pass,
                ),
                span.onset_beat,
                span.gate_end_beat,
                span.pitch,
                span.channel,
                span.velocity,
            );
            let deadline = span
                .gate_end
                .checked_sub(cut.arrangement)
                .and_then(|remaining| cut.performance.checked_add(remaining));
            for (flag, note) in unmatched.iter_mut().zip(&ledger.notes) {
                if *flag && note.signature == signature && Some(note.gate_end_frame) == deadline {
                    *flag = false;
                }
            }
        })
        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
    if unmatched.contains(&true) {
        return Err(INCOMPATIBLE_HANDOFF);
    }
    Ok(())
}

pub(super) struct FrameContext {
    pub(super) performance: u64,
    pub(super) arrangement: u64,
    pub(super) loop_pass: u64,
    pub(super) playing: bool,
    pub(super) chase: bool,
}
