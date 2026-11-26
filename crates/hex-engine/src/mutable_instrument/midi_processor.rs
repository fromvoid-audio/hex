//! Bounded pre-instrument MIDI processor stage owned by `MutableInstrumentTrackRuntime`.
//!
//! Control constructs and validates the processor pair and preallocates the
//! fixed output event scratch buffer. Callback processing transforms the
//! engine's canonical `NativeInstrumentEvent` slice in place (via the
//! preallocated scratch buffer) with zero allocation, locks, I/O, or final drops.

use super::{
    MAX_MUTABLE_EVENTS_PER_QUERY, MAX_SIMULTANEOUS_VOICES_PER_TRACK, MutableInstrumentRuntimeError,
};
use hex_lattice::{
    NativeInstrumentClock, NativeInstrumentEvent, NativeMidiProcessorLimits,
    NativeMidiProcessorRuntimePair, PreparedNativeMidiProcessor,
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

/// Preallocated pre-instrument MIDI processor stage with transactional pair
/// and fixed output scratch storage.
pub(crate) struct MidiProcessorStage {
    identity: NonZeroU64,
    pair: NativeMidiProcessorRuntimePair,
    output_events: [NativeInstrumentEvent; MAX_MUTABLE_EVENTS_PER_QUERY],
}

impl MidiProcessorStage {
    pub(crate) fn new(
        identity: NonZeroU64,
        prepared: &dyn PreparedNativeMidiProcessor,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let pair = NativeMidiProcessorRuntimePair::new(prepared, midi_processor_limits())
            .map_err(MutableInstrumentRuntimeError::MidiProcessor)?;
        Ok(Self {
            identity,
            pair,
            output_events: [NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            }; MAX_MUTABLE_EVENTS_PER_QUERY],
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
        Ok(())
    }

    pub(crate) fn commit_block(&mut self) {
        self.pair.commit_block();
    }

    pub(crate) fn rollback_block(&mut self) {
        self.pair.rollback_block();
    }

    pub(crate) fn reset(&mut self) {
        self.pair.reset();
    }

    pub(crate) fn committed_is_quiescent(&self) -> bool {
        self.pair.committed_is_quiescent()
    }

    pub(crate) fn speculative_is_quiescent(&self) -> bool {
        self.pair.speculative_is_quiescent()
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
                hex_lattice::NativeMidiProcessorError::IncompatibleHistory,
            ));
        }
        self.pair
            .inherit_from(&previous.pair)
            .map_err(MutableInstrumentRuntimeError::MidiProcessor)
    }
}
