//! Bounded pre-instrument MIDI processor stage owned by `MutableInstrumentTrackRuntime`.
//!
//! Control constructs and validates the processor pair and preallocates the
//! fixed output event scratch buffer. Callback processing transforms the
//! engine's canonical `NativeInstrumentEvent` slice in place (via the
//! preallocated scratch buffer) with zero allocation, locks, I/O, or final drops.
//!
//! Host bypass is tracked transactionally inside this stage so toggling bypass
//! while notes are held never leaves stuck notes or orphaned note-offs:
//! - Notes started while active remain owned by `pair` until their `NoteOff`
//!   releases them through `pair` (preserving suppression of dropped notes and
//!   paired fanout release).
//! - Notes started while bypassed are tracked in `bypassed_primary`/`bypassed_scratch`
//!   and pass straight through untransformed, even if bypass is later turned off
//!   before those notes release.

use super::{
    MAX_MUTABLE_EVENTS_PER_QUERY, MAX_SIMULTANEOUS_VOICES_PER_TRACK, MutableInstrumentRuntimeError,
};
use hex_lattice::{
    NativeInstrumentClock, NativeInstrumentEvent, NativeMidiProcessorError,
    NativeMidiProcessorLimits, NativeMidiProcessorRuntimePair, PreparedNativeMidiProcessor,
};
use std::num::NonZeroU64;

/// Finite per-stage pair storage ceiling for one native MIDI processor.
pub(crate) const MAX_MIDI_PROCESSOR_PAIR_BYTES: usize = 256 * 1024;
/// Finite per-block work ceiling for one native MIDI processor stage.
pub(crate) const MAX_MIDI_PROCESSOR_WORK_UNITS_PER_BLOCK: usize = MAX_MUTABLE_EVENTS_PER_QUERY * 4;

pub(crate) fn midi_processor_limits() -> NativeMidiProcessorLimits {
    NativeMidiProcessorLimits {
        max_pair_bytes: MAX_MIDI_PROCESSOR_PAIR_BYTES,
        max_input_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
        max_output_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
        max_active_occurrences: MAX_SIMULTANEOUS_VOICES_PER_TRACK,
        max_work_units_per_block: MAX_MIDI_PROCESSOR_WORK_UNITS_PER_BLOCK,
    }
}

/// Preallocated pre-instrument MIDI processor stage with transactional pair,
/// transactional host bypass ledger, and fixed output/partition scratch storage.
pub(crate) struct MidiProcessorStage {
    identity: NonZeroU64,
    bypassed: bool,
    pair: NativeMidiProcessorRuntimePair,
    bypassed_primary: [Option<u64>; MAX_SIMULTANEOUS_VOICES_PER_TRACK],
    bypassed_scratch: [Option<u64>; MAX_SIMULTANEOUS_VOICES_PER_TRACK],
    pair_input: [NativeInstrumentEvent; MAX_MUTABLE_EVENTS_PER_QUERY],
    bypass_output: [NativeInstrumentEvent; MAX_MUTABLE_EVENTS_PER_QUERY],
    output_events: [NativeInstrumentEvent; MAX_MUTABLE_EVENTS_PER_QUERY],
}

impl MidiProcessorStage {
    pub(crate) fn new_with_bypass(
        identity: NonZeroU64,
        prepared: &dyn PreparedNativeMidiProcessor,
        bypassed: bool,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let pair = NativeMidiProcessorRuntimePair::new(prepared, midi_processor_limits())
            .map_err(MutableInstrumentRuntimeError::MidiProcessor)?;
        let default_event = NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        };
        Ok(Self {
            identity,
            bypassed,
            pair,
            bypassed_primary: [None; MAX_SIMULTANEOUS_VOICES_PER_TRACK],
            bypassed_scratch: [None; MAX_SIMULTANEOUS_VOICES_PER_TRACK],
            pair_input: [default_event; MAX_MUTABLE_EVENTS_PER_QUERY],
            bypass_output: [default_event; MAX_MUTABLE_EVENTS_PER_QUERY],
            output_events: [default_event; MAX_MUTABLE_EVENTS_PER_QUERY],
        })
    }

    pub(crate) fn storage_bytes(&self) -> Option<usize> {
        std::mem::size_of::<Self>().checked_add(self.pair.fixed_storage_pair_bytes()?)
    }

    pub(crate) fn declared_pair_bytes(&self) -> usize {
        self.pair.declared_pair_bytes()
    }

    pub(crate) fn begin_block(&mut self) {
        self.pair.begin_block();
        self.bypassed_scratch = self.bypassed_primary;
    }

    fn find_bypassed(slots: &[Option<u64>], occurrence_id: u64) -> Option<usize> {
        slots.iter().position(|slot| *slot == Some(occurrence_id))
    }

    fn has_bypassed(slots: &[Option<u64>]) -> bool {
        slots.iter().any(Option::is_some)
    }

    /// Transform `events` in place through the preallocated scratch buffer.
    /// Fails before modifying `events` or downstream instrument state on any
    /// invalid batch, capacity overflow, or work budget refusal.
    pub(crate) fn process_block(
        &mut self,
        events: &mut Vec<NativeInstrumentEvent>,
        frame_count: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let capacity = events.capacity().min(self.output_events.len());
        if !self.bypassed && !Self::has_bypassed(&self.bypassed_scratch) {
            let written = self
                .pair
                .process_block(
                    events.as_slice(),
                    frame_count,
                    &mut self.output_events[..capacity],
                    clock,
                )
                .map_err(MutableInstrumentRuntimeError::MidiProcessor)?;
            events.clear();
            events.extend_from_slice(&self.output_events[..written]);
            return Ok(());
        }

        // Validate basic batch ordering and bounds up front before touching scratch.
        if events.len() > MAX_MUTABLE_EVENTS_PER_QUERY {
            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                NativeMidiProcessorError::InputEventCapacityExceeded,
            ));
        }
        let mut prev_offset = 0_usize;
        for &event in events.iter() {
            let offset = event.frame_offset();
            if frame_count == 0 || offset >= frame_count || offset < prev_offset {
                return Err(MutableInstrumentRuntimeError::MidiProcessor(
                    NativeMidiProcessorError::InvalidInputEventBatch,
                ));
            }
            prev_offset = offset;
        }

        let mut pair_in_len = 0_usize;
        let mut bypass_out_len = 0_usize;
        for &event in events.iter() {
            match event {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    velocity,
                    ..
                } => {
                    if occurrence_id == 0 || pitch > 127 || !(1..=127).contains(&velocity) {
                        return Err(MutableInstrumentRuntimeError::MidiProcessor(
                            NativeMidiProcessorError::InvalidInputEventBatch,
                        ));
                    }
                    if Self::find_bypassed(&self.bypassed_scratch, occurrence_id).is_some() {
                        return Err(MutableInstrumentRuntimeError::MidiProcessor(
                            NativeMidiProcessorError::DuplicateOccurrence,
                        ));
                    }
                    if self.bypassed {
                        if bypass_out_len >= capacity {
                            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::OutputEventCapacityExceeded,
                            ));
                        }
                        let free = self
                            .bypassed_scratch
                            .iter()
                            .position(Option::is_none)
                            .ok_or(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded,
                            ))?;
                        self.bypassed_scratch[free] = Some(occurrence_id);
                        self.bypass_output[bypass_out_len] = event;
                        bypass_out_len += 1;
                    } else {
                        self.pair_input[pair_in_len] = event;
                        pair_in_len += 1;
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if occurrence_id == 0 {
                        return Err(MutableInstrumentRuntimeError::MidiProcessor(
                            NativeMidiProcessorError::InvalidInputEventBatch,
                        ));
                    }
                    if let Some(slot) = Self::find_bypassed(&self.bypassed_scratch, occurrence_id) {
                        if bypass_out_len >= capacity {
                            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::OutputEventCapacityExceeded,
                            ));
                        }
                        self.bypassed_scratch[slot] = None;
                        self.bypass_output[bypass_out_len] = event;
                        bypass_out_len += 1;
                    } else {
                        self.pair_input[pair_in_len] = event;
                        pair_in_len += 1;
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_bypassed(&self.bypassed_scratch, occurrence_id.get()).is_some() {
                        if bypass_out_len >= capacity {
                            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::OutputEventCapacityExceeded,
                            ));
                        }
                        self.bypass_output[bypass_out_len] = event;
                        bypass_out_len += 1;
                    } else {
                        self.pair_input[pair_in_len] = event;
                        pair_in_len += 1;
                    }
                }
                NativeInstrumentEvent::Expression { expression, .. } => {
                    if !expression.is_valid() {
                        return Err(MutableInstrumentRuntimeError::MidiProcessor(
                            NativeMidiProcessorError::InvalidEventPayload,
                        ));
                    }
                    if self.bypassed {
                        if bypass_out_len >= capacity {
                            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::OutputEventCapacityExceeded,
                            ));
                        }
                        self.bypass_output[bypass_out_len] = event;
                        bypass_out_len += 1;
                    } else {
                        self.pair_input[pair_in_len] = event;
                        pair_in_len += 1;
                    }
                }
                NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                    if cc > 127 || value > 127 {
                        return Err(MutableInstrumentRuntimeError::MidiProcessor(
                            NativeMidiProcessorError::InvalidEventPayload,
                        ));
                    }
                    if self.bypassed {
                        if bypass_out_len >= capacity {
                            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::OutputEventCapacityExceeded,
                            ));
                        }
                        self.bypass_output[bypass_out_len] = event;
                        bypass_out_len += 1;
                    } else {
                        self.pair_input[pair_in_len] = event;
                        pair_in_len += 1;
                    }
                }
                NativeInstrumentEvent::PitchBend { value, .. } => {
                    if !(-8_192..=8_191).contains(&value) {
                        return Err(MutableInstrumentRuntimeError::MidiProcessor(
                            NativeMidiProcessorError::InvalidEventPayload,
                        ));
                    }
                    if self.bypassed {
                        if bypass_out_len >= capacity {
                            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                                NativeMidiProcessorError::OutputEventCapacityExceeded,
                            ));
                        }
                        self.bypass_output[bypass_out_len] = event;
                        bypass_out_len += 1;
                    } else {
                        self.pair_input[pair_in_len] = event;
                        pair_in_len += 1;
                    }
                }
            }
        }

        // Advance the underlying processor pair at most once per audio block so
        // clock-driven future events (e.g., pending generated NoteOffs) drain
        // at exact offsets without double-advancing during bypass transitions.
        let pair_out_len = if pair_in_len > 0 || !self.pair.speculative_is_quiescent() {
            let remaining = capacity.saturating_sub(bypass_out_len);
            self.pair
                .process_block(
                    &self.pair_input[..pair_in_len],
                    frame_count,
                    &mut self.output_events[..remaining],
                    clock,
                )
                .map_err(MutableInstrumentRuntimeError::MidiProcessor)?
        } else {
            0
        };
        let total_len = pair_out_len + bypass_out_len;
        if total_len > capacity {
            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                NativeMidiProcessorError::OutputEventCapacityExceeded,
            ));
        }
        self.output_events[pair_out_len..total_len]
            .copy_from_slice(&self.bypass_output[..bypass_out_len]);
        super::stable_sort_native_events(&mut self.output_events[..total_len]);
        events.clear();
        events.extend_from_slice(&self.output_events[..total_len]);
        Ok(())
    }

    pub(crate) fn commit_block(&mut self) {
        self.pair.commit_block();
        self.bypassed_primary = self.bypassed_scratch;
    }

    pub(crate) fn rollback_block(&mut self) {
        self.pair.rollback_block();
        self.bypassed_scratch = self.bypassed_primary;
    }

    pub(crate) fn reset(&mut self) {
        self.pair.reset();
        self.bypassed_primary.fill(None);
        self.bypassed_scratch.fill(None);
    }

    pub(crate) fn committed_is_quiescent(&self) -> bool {
        self.pair.committed_is_quiescent() && !Self::has_bypassed(&self.bypassed_primary)
    }

    pub(crate) fn speculative_is_quiescent(&self) -> bool {
        self.pair.speculative_is_quiescent() && !Self::has_bypassed(&self.bypassed_scratch)
    }

    pub(crate) fn can_inherit_from(&self, previous: &Self) -> bool {
        self.identity == previous.identity && self.pair.can_inherit_from(&previous.pair)
    }

    pub(crate) fn inherit_from(
        &mut self,
        previous: &Self,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if self.identity != previous.identity {
            return Err(MutableInstrumentRuntimeError::MidiProcessor(
                NativeMidiProcessorError::IncompatibleHistory,
            ));
        }
        self.pair
            .inherit_from(&previous.pair)
            .map_err(MutableInstrumentRuntimeError::MidiProcessor)?;
        self.bypassed_primary = previous.bypassed_primary;
        self.bypassed_scratch = previous.bypassed_primary;
        Ok(())
    }
}
