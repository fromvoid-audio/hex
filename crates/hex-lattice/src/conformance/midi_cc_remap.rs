//! Small deterministic MIDI Control Change remapper fixture for `hex-lattice`
//! conformance, engine PCM oracles, and callback allocation probes. Never a
//! browser device.
//!
//! Remaps incoming `ControlChange { cc: source_cc, value }` events to
//! `ControlChange { cc: target_cc, value }` while preserving exact wire order,
//! same-offset note/CC ordering, occurrence-owned NoteOn/NoteOff tracking, and
//! strict `0..=127` payload validation without callback allocation.

use crate::{
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorError, PreparedNativeMidiProcessor,
};
use std::any::Any;

pub const CC_REMAP_RUNTIME_VERSION: u32 = 1;
pub const CC_REMAP_MAX_INPUT_EVENTS: usize = 259;
pub const CC_REMAP_MAX_OUTPUT_EVENTS: usize = 259;
pub const CC_REMAP_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const CC_REMAP_MAX_WORK_UNITS: usize = CC_REMAP_MAX_INPUT_EVENTS * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedCcRemapMidiProcessor {
    source_cc: u8,
    target_cc: u8,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedCcRemapMidiProcessor {
    pub fn new(source_cc: u8, target_cc: u8) -> Result<Self, NativeMidiProcessorError> {
        Self::with_caps(
            source_cc,
            target_cc,
            CC_REMAP_MAX_INPUT_EVENTS,
            CC_REMAP_MAX_OUTPUT_EVENTS,
            CC_REMAP_MAX_ACTIVE_OCCURRENCES,
            CC_REMAP_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        source_cc: u8,
        target_cc: u8,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, NativeMidiProcessorError> {
        if source_cc > 127 || target_cc > 127 {
            return Err(NativeMidiProcessorError::InvalidEventPayload);
        }
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > CC_REMAP_MAX_ACTIVE_OCCURRENCES
            || max_work_units == 0
        {
            return Err(NativeMidiProcessorError::RuntimeBudgetExceeded);
        }
        Ok(Self {
            source_cc,
            target_cc,
            max_input_events,
            max_output_events,
            max_active_occurrences,
            max_work_units,
        })
    }

    pub const fn source_cc(self) -> u8 {
        self.source_cc
    }

    pub const fn target_cc(self) -> u8 {
        self.target_cc
    }
}

impl PreparedNativeMidiProcessor for PreparedCcRemapMidiProcessor {
    fn runtime_version(&self) -> u32 {
        CC_REMAP_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<CcRemapRuntime>()
    }

    fn max_input_events_per_block(&self) -> usize {
        self.max_input_events
    }

    fn max_output_events_per_block(&self) -> usize {
        self.max_output_events
    }

    fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    fn max_work_units_per_block(&self) -> usize {
        self.max_work_units
    }

    fn create_runtime(&self) -> Box<dyn MutableMidiProcessorRuntime> {
        Box::new(CcRemapRuntime {
            config: *self,
            active: [None; CC_REMAP_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
struct CcRemapRuntime {
    config: PreparedCcRemapMidiProcessor,
    active: [Option<u64>; CC_REMAP_MAX_ACTIVE_OCCURRENCES],
}

impl CcRemapRuntime {
    fn find_slot(slots: &[Option<u64>], occurrence_id: u64) -> Option<usize> {
        slots.iter().position(|slot| *slot == Some(occurrence_id))
    }

    fn find_free_slot(slots: &[Option<u64>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_active(slots: &[Option<u64>]) -> usize {
        slots.iter().flatten().count()
    }
}

impl MutableMidiProcessorRuntime for CcRemapRuntime {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        _frame_count: usize,
        output_capacity: usize,
        _clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        if input.len() > self.config.max_input_events {
            return Err(NativeMidiProcessorError::InputEventCapacityExceeded);
        }
        let mut simulated = self.active;
        let active_cap = self.config.max_active_occurrences;
        let mut output_count = 0_usize;
        let mut work_units = 0_usize;

        for &event in input {
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if Self::find_slot(&simulated[..active_cap], occurrence_id).is_some() {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_slot(&simulated[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    simulated[free] = Some(occurrence_id);
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = Self::find_slot(&simulated[..active_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    simulated[slot] = None;
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_slot(&simulated[..active_cap], occurrence_id.get()).is_none() {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    }
                }
                NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                    if cc > 127 || value > 127 {
                        return Err(NativeMidiProcessorError::InvalidEventPayload);
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {}
            }
            output_count = output_count
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            if output_count > output_capacity || output_count > self.config.max_output_events {
                return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
            }
            if work_units > self.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }
        Ok(output_count)
    }

    fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        _frame_count: usize,
        output: &mut MidiProcessorOutput<'_>,
        _clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let active_cap = self.config.max_active_occurrences;
        for &event in input {
            let _ = output.charge_work(1);
            let out_event = match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if let Some(free) = Self::find_free_slot(&self.active[..active_cap]) {
                        self.active[free] = Some(occurrence_id);
                    }
                    event
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_slot(&self.active[..active_cap], occurrence_id) {
                        self.active[slot] = None;
                    }
                    event
                }
                NativeInstrumentEvent::ControlChange {
                    frame_offset,
                    cc,
                    value,
                } => NativeInstrumentEvent::ControlChange {
                    frame_offset,
                    cc: if cc == self.config.source_cc {
                        self.config.target_cc
                    } else {
                        cc
                    },
                    value,
                },
                other => other,
            };
            let _ = output.charge_work(1);
            let _ = output.push(out_event);
        }
    }

    fn preflight_flush_active_note_offs(
        &self,
        frame_offset: usize,
        frame_count: usize,
        output_capacity: usize,
    ) -> Result<usize, NativeMidiProcessorError> {
        if frame_count == 0 || frame_offset >= frame_count {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        let active = Self::count_active(&self.active[..self.config.max_active_occurrences]);
        if active > output_capacity || active > self.config.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        if active > self.config.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
        }
        Ok(active)
    }

    fn flush_active_note_offs(
        &mut self,
        frame_offset: usize,
        output: &mut MidiProcessorOutput<'_>,
    ) {
        let active_cap = self.config.max_active_occurrences;
        let mut ids = [0_u64; CC_REMAP_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active[..active_cap] {
            if let Some(occurrence_id) = slot.take() {
                ids[len] = occurrence_id;
                len += 1;
            }
        }
        ids[..len].sort_unstable();
        for &occurrence_id in &ids[..len] {
            let _ = output.charge_work(1);
            let _ = output.push(NativeInstrumentEvent::NoteOff {
                frame_offset,
                occurrence_id,
            });
        }
    }

    fn active_occurrence_count(&self) -> usize {
        Self::count_active(&self.active[..self.config.max_active_occurrences])
    }

    fn reset(&mut self) {
        self.active.fill(None);
    }

    fn can_copy_history_from(&self, previous: &dyn MutableMidiProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| {
                other.active_occurrence_count() <= self.config.max_active_occurrences
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.active.fill(None);
        let active_cap = self.config.max_active_occurrences;
        for (write, &occurrence_id) in other.active[..other.config.max_active_occurrences]
            .iter()
            .flatten()
            .enumerate()
        {
            if write >= active_cap {
                break;
            }
            self.active[write] = Some(occurrence_id);
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
