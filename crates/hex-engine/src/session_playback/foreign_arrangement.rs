//! Authored and physical-key gate ownership for prepared VST3 generators. The engine owns
//! the bounded note ledger. The audio host owns the worker and accepts or
//! revokes staged events with the complete physical callback.
use super::{
    MAX_TRACKS, MidiSpanBuffer, PreparedRevision, RouteOwner, SessionPlaybackError, TrackRoute,
};
use crate::computer_keyboard::{
    COMPUTER_KEY_COUNT, ComputerKeyboardInput, KeyboardActivationClock,
};
use crate::mutable_instrument::{
    MutableInstrumentRuntimeError, MutableNoteOnset, MutableOccurrence, MutableOccurrenceKey,
    MutableOccurrenceSignature, MutableOnsetBuffer,
    authored_gate::{AuthoredGate, AuthoredGateContext, MAX_FOREIGN_NOTES, stage_authored_gates},
};
use crate::{Frame, MidiTiming, Track, session_inventory::EffectiveSessionMidi};
use hex_lattice::{
    ForeignNote, ForeignProcessorNode, ForeignProcessors, NativeInstrumentClock,
    PreparedForeignProcessor, PreparedProcessorFactory,
};
use hex_project::TrackId;
use std::{mem::size_of, num::NonZeroU32, sync::Arc};

mod hardware;
mod live_keys;
use live_keys::Input;

const MAX_AUTHORED_EVENTS_AT_CUT: usize = 2 * MAX_FOREIGN_NOTES;
const MAX_RENDER_EVENTS: usize =
    MAX_AUTHORED_EVENTS_AT_CUT + 2 * COMPUTER_KEY_COUNT + hardware::NOTE_BUDGET;

fn next_note_id(next: &mut u64) -> Result<NonZeroU32, MutableInstrumentRuntimeError> {
    let id = next
        .checked_add(1)
        .and_then(|id| i32::try_from(id).ok())
        .and_then(|id| NonZeroU32::new(id as u32))
        .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
    *next = u64::from(id.get());
    Ok(id)
}

#[derive(Default)]
struct Ledger {
    notes: Vec<MutableOccurrence>,
    next_id: u64,
    loop_pass: Option<u64>,
    route: Option<RouteOwner>,
    cut: bool,
    live: live_keys::Gates,
    hardware: hardware::Gates,
    quantum_offset: u16,
}

impl Ledger {
    fn has_held_notes(&self) -> bool {
        self.notes.iter().any(|note| !note.note_off_sent)
    }

    fn has_any_held_notes(&self) -> bool {
        self.has_held_notes() || self.live.held() || self.hardware.held()
    }
}

struct Generator {
    track: TrackId,
    processor: PreparedForeignProcessor,
    has_authored_automation: bool,
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
        let node = track
            .foreign_instrument
            .as_ref()
            .ok_or(SessionPlaybackError::DeviceRuntime)?;
        let Some(PreparedProcessorFactory::Foreign(processor)) = node.definition.as_ref() else {
            return Err(SessionPlaybackError::DeviceRuntime);
        };
        if processor.binding().is_none() {
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        Ok(Self {
            track: track_id,
            processor: *processor,
            has_authored_automation: node.automation.foreign().is_some(),
            committed: ledger()?,
            scratch: ledger()?,
        })
    }
}

/// Index mapping only. No borrowed or owned endpoint crosses this boundary.
pub(super) struct Handoff {
    old_indices: [Option<usize>; MAX_TRACKS],
    pub(super) requires_clock_continuity: bool,
    pub(super) keeps_first_pass_loop_clock: bool,
    pub(super) keeps_unautomated_loop_clock: bool,
}

pub(super) const INCOMPATIBLE_HANDOFF: SessionPlaybackError =
    SessionPlaybackError::MutableInstrumentDiagnostic(
        "vst3 instrument graph change requires stopped playback and released notes",
    );

#[derive(Default)]
pub(super) struct ForeignArrangement {
    generators: Vec<Option<Generator>>,
    events: Vec<ForeignNote>,
    keyboard: Option<live_keys::Input>,
    hardware_batch: hardware::Batch,
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
                .try_reserve_exact(MAX_RENDER_EVENTS)
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
        Ok(Self {
            generators,
            events,
            keyboard: None,
            hardware_batch: hardware::Batch::default(),
        })
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
            .checked_add(MAX_RENDER_EVENTS * size_of::<ForeignNote>())
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
                )?
                .checked_add(
                    self.keyboard
                        .as_ref()
                        .map_or(0, |input| input.root.control_storage_bytes()),
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

    pub(super) fn has_held_notes(&self) -> bool {
        self.generators
            .iter()
            .flatten()
            .any(|generator| generator.committed.has_any_held_notes())
    }

    pub(super) fn has_keyboard(&self) -> bool {
        self.keyboard
            .as_ref()
            .is_some_and(|input| !input.detached && !input.root.is_cancelled())
    }

    pub(super) fn attach_keyboard(
        &mut self,
        track: TrackId,
        root: Arc<ComputerKeyboardInput>,
    ) -> Result<(), crate::Invalid> {
        if self.keyboard.is_some()
            || root.is_cancelled()
            || !self
                .generators
                .iter()
                .flatten()
                .any(|generator| generator.track == track)
        {
            return Err(crate::Invalid(
                "vst3 computer keyboard target is unavailable",
            ));
        }
        if root.ordered_keys.is_some() {
            return Err(crate::Invalid("vst3 monitoring does not support recording"));
        }
        self.keyboard = Some(live_keys::Input {
            track,
            root,
            detached: false,
        });
        Ok(())
    }

    pub(super) fn remove_keyboard_on_control(&mut self) {
        self.keyboard = None;
    }

    pub(super) fn shares_keyboard(&self, previous: &Self) -> bool {
        self.keyboard
            .as_ref()
            .zip(previous.keyboard.as_ref())
            .is_some_and(|(next, old)| {
                !next.detached && !old.detached && Arc::ptr_eq(&next.root, &old.root)
            })
    }

    pub(super) fn detach_keyboard(&mut self) {
        if let Some(input) = self.keyboard.as_mut() {
            input.detached = true;
        }
    }

    pub(super) fn cancel_keyboard(&self, keep_clock: Option<&KeyboardActivationClock>) {
        if let Some(input) = &self.keyboard
            && !input.detached
            && keep_clock.is_none_or(|clock| !input.root.bound_to_clock(clock))
        {
            input.root.cancel();
        }
    }

    pub(super) fn plan_handoff(
        &self,
        old: &Self,
        tracks: &[Track],
        cut: FrameContext,
        fresh: Option<TrackId>,
        loops: [Option<hex_project::LoopRegion>; 2],
    ) -> Result<Handoff, SessionPlaybackError> {
        let mut plan = Handoff {
            old_indices: [None; MAX_TRACKS],
            requires_clock_continuity: false,
            keeps_first_pass_loop_clock: true,
            keeps_unautomated_loop_clock: true,
        };
        let mut retained = [false; MAX_TRACKS];
        for (index, candidate) in self.generators.iter().enumerate() {
            let Some(candidate) = candidate else { continue };
            let matched = old.generators.iter().enumerate().find_map(|(index, old)| {
                let old = old.as_ref()?;
                (old.track == candidate.track && old.processor == candidate.processor)
                    .then_some((index, old))
            });
            let Some((old_index, donor)) = matched else {
                if cut.playing && fresh != Some(candidate.track) {
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
            let ledger = &donor.committed;
            if ledger.loop_pass.is_some() && (cut.playing || ledger.has_any_held_notes()) {
                plan.requires_clock_continuity = true;
                // An Arrangement gate keeps its performance-frame deadline at
                // the same cursor. A future wrap releases it through the normal
                // renderer. Without an authored program, a changed loop does
                // not affect the worker's parameter-origin budget.
                plan.keeps_unautomated_loop_clock &= ledger.route == Some(RouteOwner::Arrangement)
                    && !candidate.has_authored_automation
                    && !donor.has_authored_automation;
                // These held gates are timed against the canonical first
                // pass. Keep their IDs and deadlines on the original worker.
                plan.keeps_first_pass_loop_clock &= cut.loop_pass == 0
                    && cut.performance == cut.arrangement
                    && ledger.route == Some(RouteOwner::Arrangement)
                    && !ledger.live.held()
                    && !ledger.hardware.held()
                    && loops
                        .into_iter()
                        .flatten()
                        .all(|region| !region.enabled() || cut.arrangement < region.end())
                    && crate::foreign_automation::QuantumProfile::keeps_instrument_epoch(
                        candidate.processor.quantum_frames(),
                        loops[0],
                        loops[1],
                    )
                    && candidate.has_authored_automation;
            }
            if !ledger.cut
                && matches!(ledger.route, Some(RouteOwner::Session { .. }))
                && ledger.has_held_notes()
            {
                return Err(INCOMPATIBLE_HANDOFF);
            }
            if !ledger.cut
                && ledger.route == Some(RouteOwner::Arrangement)
                && ledger.loop_pass == Some(cut.loop_pass)
            {
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
                && (cut.playing || generator.committed.has_any_held_notes())
            {
                return Err(INCOMPATIBLE_HANDOFF);
            }
        }
        Ok(plan)
    }

    /// Commit only after the graph preflight. Both graphs keep their reserved
    /// buffers, and the host retains the exact worker on its existing binding.
    pub(super) fn inherit_handoff(&mut self, old: &mut Self, plan: Handoff, keep_clock: bool) {
        let same_keyboard = self
            .keyboard
            .as_ref()
            .zip(old.keyboard.as_ref())
            .is_some_and(|(next, previous)| {
                next.track == previous.track && Arc::ptr_eq(&next.root, &previous.root)
            });
        for (candidate, old_index) in self.generators.iter_mut().zip(plan.old_indices) {
            if let Some((candidate, donor)) = candidate.as_mut().zip(
                old_index
                    .and_then(|index| old.generators.get_mut(index))
                    .and_then(Option::as_mut),
            ) {
                if keep_clock {
                    std::mem::swap(&mut candidate.committed, &mut donor.committed);
                    if !same_keyboard {
                        candidate.committed.live.disconnect();
                        candidate.committed.hardware.disconnect();
                    }
                } else {
                    // Restart arrangement history while retaining worker note IDs.
                    candidate.committed.next_id = donor.committed.next_id;
                    candidate.committed.quantum_offset = donor.committed.quantum_offset;
                    // The retained worker still owns monitor notes even when the
                    // arrangement clock restarts. Preserve them to release safely.
                    candidate.committed.live = donor.committed.live;
                    if !same_keyboard {
                        candidate.committed.live.disconnect();
                    }
                    candidate
                        .committed
                        .hardware
                        .inherit_budget(&donor.committed.hardware);
                }
            }
        }
    }

    pub(super) fn begin(&mut self) {
        self.hardware_batch.begin(self.keyboard.as_ref());
        for generator in self.generators.iter_mut().flatten() {
            let Generator {
                committed, scratch, ..
            } = generator;
            scratch.notes.clear();
            scratch.notes.extend_from_slice(&committed.notes);
            scratch.next_id = committed.next_id;
            scratch.loop_pass = committed.loop_pass;
            scratch.route = committed.route;
            scratch.cut = committed.cut;
            scratch.live = committed.live;
            scratch.hardware = committed.hardware;
            scratch.quantum_offset = committed.quantum_offset;
        }
    }

    pub(super) fn commit(&mut self) {
        self.hardware_batch.commit(self.keyboard.as_ref());
        for generator in self.generators.iter_mut().flatten() {
            std::mem::swap(&mut generator.committed, &mut generator.scratch);
            if let Some(input) = self
                .keyboard
                .as_ref()
                .filter(|input| !input.detached && input.track == generator.track)
            {
                generator.committed.live.commit_audition(input);
            }
        }
    }

    pub(super) fn cut(&mut self) {
        for generator in self.generators.iter_mut().flatten() {
            generator.committed.cut = true;
        }
    }

    /// A fresh Arrangement seek on the same revision keeps the worker but
    /// restarts its authored clock. Keep the physical quantum phase and note
    /// IDs, so the first destination frame releases old authored gates before
    /// chasing new ones. A rebound monitor input keeps its own held gates.
    pub(super) fn cut_retained_gates_from(&mut self, previous: &mut Self) {
        let shares_input = self.shares_keyboard(previous);
        let input_track = self.keyboard.as_ref().map(|input| input.track);
        for (next, old) in self.generators.iter_mut().zip(&mut previous.generators) {
            let (Some(next), Some(old)) = (next, old) else {
                continue;
            };
            if next.track != old.track || next.processor != old.processor {
                continue;
            }
            std::mem::swap(&mut next.committed, &mut old.committed);
            next.committed.cut = true;
            if !shares_input || input_track != Some(next.track) {
                next.committed.live.disconnect();
                next.committed.hardware.disconnect();
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // Borrow the existing callback and its disjoint scratch.
    pub(super) fn render_frame(
        &mut self,
        index: usize,
        track: &Track,
        context: FrameContext,
        session: SessionRoute<'_>,
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
        let keyboard = self
            .keyboard
            .as_ref()
            .filter(|input| !input.detached && input.track == runtime.track);
        if !context.playing
            && ledger.loop_pass.is_none()
            && keyboard.is_none_or(|input| input.root.is_cancelled())
        {
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
        let authored_capacity = if ledger.cut {
            MAX_AUTHORED_EVENTS_AT_CUT
        } else {
            MAX_FOREIGN_NOTES
        };
        let mut emit = |gate| -> Result<(), MutableInstrumentRuntimeError> {
            if events.len() == authored_capacity {
                return Err(MutableInstrumentRuntimeError::OccurrenceCapacity);
            }
            events.push(ForeignNote::try_from(gate)?);
            Ok(())
        };
        let route_changed = ledger.route.is_some_and(|old| old != session.route.owner);
        // A fresh generator can first appear at a running Arrangement cursor.
        // Chase its current span without changing any retained generator's gate.
        let first_route = ledger.route.is_none();
        // A same-clock audio restart can keep this ledger. Only chase an
        // interactive start if this generator also cut its authored gates.
        let chase = context.chase && ledger.cut;
        let wrapped = session.route.owner == RouteOwner::Arrangement
            && !route_changed
            && !ledger.cut
            && ledger
                .loop_pass
                .is_some_and(|pass| pass != context.loop_pass);
        if ledger.cut || wrapped || route_changed {
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
            let source_start = match session.route.owner {
                RouteOwner::Arrangement => context.arrangement,
                _ => context.performance,
            };
            let end = source_start
                .checked_add(1)
                .ok_or(SessionPlaybackError::ArithmeticOverflow)?;
            match session.route.owner {
                RouteOwner::Arrangement => {
                    let mut overflow = false;
                    track
                        .for_each_mutable_midi_span(context.arrangement, end, |span| {
                            if span.start < context.arrangement
                                && !(chase || wrapped || first_route)
                            {
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
                                    contributor_fade_in: false,
                                })
                                .is_err();
                        })
                        .map_err(|_| SessionPlaybackError::MutableInstrumentEvents)?;
                    if overflow {
                        return Err(SessionPlaybackError::MutableInstrumentEvents);
                    }
                }
                RouteOwner::Session { .. } => super::render::collect_console_midi_onsets(
                    &session.midi,
                    session.route,
                    session.timing,
                    source_start,
                    end,
                    session.spans,
                    onsets,
                )?,
                RouteOwner::SessionAudio { .. } | RouteOwner::Silence => {}
            }
            stage_authored_gates(
                &mut ledger.notes,
                &mut ledger.next_id,
                onsets,
                AuthoredGateContext {
                    track: Some(id),
                    performance_start: context.performance,
                    source_start,
                    width: 1,
                    source_age: None,
                    contributor_fade_rate: None,
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
        // One physical callback spans at most one quantum boundary. Preserve
        // headroom for this snapshot even when the authored slice is full.
        if ledger.quantum_offset == 0 && (keyboard.is_some() || ledger.live.held()) {
            ledger
                .live
                .stage(keyboard, &mut ledger.next_id, events)
                .map_err(|error| error.render_error())?;
        }
        ledger.live.advance_audition();
        if (self.hardware_batch.has_events() && keyboard.is_some()) || ledger.hardware.held() {
            ledger
                .hardware
                .stage(
                    keyboard,
                    &mut self.hardware_batch,
                    &mut ledger.next_id,
                    events,
                )
                .map_err(|error| error.render_error())?;
        }
        ledger.quantum_offset += 1;
        if ledger.quantum_offset == prepared.quantum_frames() {
            ledger.quantum_offset = 0;
            ledger.hardware.reset_budget();
        }
        ledger.loop_pass = Some(context.loop_pass);
        ledger.route = Some(session.route.owner);
        let node = ForeignProcessorNode {
            owner: node.owner,
            device_id: node.device_id,
            device_type: node.device_type,
            sample_rate: node.sample_rate,
            configuration,
            prepared,
            automation: node
                .automation
                .foreign()
                .map(hex_lattice::PreparedForeignAutomation::borrow),
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

pub(super) struct SessionRoute<'a> {
    pub(super) route: TrackRoute,
    pub(super) midi: EffectiveSessionMidi<'a>,
    pub(super) timing: &'a dyn MidiTiming,
    pub(super) spans: &'a mut MidiSpanBuffer,
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
