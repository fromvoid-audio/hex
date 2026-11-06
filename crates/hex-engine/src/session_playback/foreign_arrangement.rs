//! Arrangement gate ownership for prepared VST3 generators. The engine owns
//! the bounded note ledger. The audio host owns the worker and accepts or
//! revokes staged events with the complete physical callback.
use super::{PreparedRevision, SessionPlaybackError};
use crate::mutable_instrument::{
    MutableInstrumentRuntimeError, MutableNoteOnset, MutableOccurrence, MutableOccurrenceKey,
    MutableOnsetBuffer,
    authored_gate::{AuthoredGate, AuthoredGateContext, MAX_FOREIGN_NOTES, stage_authored_gates},
};
use crate::{Frame, Track};
use hex_lattice::{
    ForeignNote, ForeignProcessorNode, ForeignProcessors, NativeInstrumentClock,
    PreparedProcessorFactory,
};
use std::mem::size_of;

#[derive(Default)]
struct Ledger {
    notes: Vec<MutableOccurrence>,
    next_id: u64,
    loop_pass: Option<u64>,
    cut: bool,
}

struct Generator {
    committed: Ledger,
    scratch: Ledger,
}

impl Generator {
    fn prepare() -> Result<Self, SessionPlaybackError> {
        fn ledger() -> Result<Ledger, SessionPlaybackError> {
            let mut ledger = Ledger::default();
            ledger
                .notes
                .try_reserve_exact(MAX_FOREIGN_NOTES)
                .map_err(|_| SessionPlaybackError::Allocation)?;
            Ok(ledger)
        }
        Ok(Self {
            committed: ledger()?,
            scratch: ledger()?,
        })
    }
}

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
                    generators.push(Some(Generator::prepare()?));
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

    pub(super) fn begin(&mut self) {
        for generator in self.generators.iter_mut().flatten() {
            let Generator { committed, scratch } = generator;
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

pub(super) struct FrameContext {
    pub(super) performance: u64,
    pub(super) arrangement: u64,
    pub(super) loop_pass: u64,
    pub(super) playing: bool,
    pub(super) chase: bool,
}
