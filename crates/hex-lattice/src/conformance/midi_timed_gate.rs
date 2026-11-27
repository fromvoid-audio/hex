//! Real bounded timed-gate MIDI processor test consumer for `#3490`.
//!
//! Uses [`BoundedFutureMidiQueue`] and the host's borrowed
//! [`NativeInstrumentClock`] to schedule delayed generated `NoteOn` and paired
//! `NoteOff` events in either musical beat (`BeatQ32`) or performance-frame
//! (`PerformanceFrame`) coordinates with deterministic equal-offset ordering,
//! reserved `NoteOff` custody, and zero callback allocations. Never a browser
//! device.

use crate::{
    BoundedFutureMidiQueue, DueMidiEvent, FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
    FUTURE_MIDI_MAX_HORIZON_BEATS_Q32, FUTURE_MIDI_MAX_HORIZON_FRAMES,
    FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS, FutureMidiTime, GENERATED_OCCURRENCE_ID_BASE,
    MidiProcessorOutput, MutableMidiProcessorRuntime, NativeInstrumentClock, NativeInstrumentEvent,
    NativeMidiProcessorError, PreparedNativeMidiProcessor, ScheduledMidiKind,
};
use std::any::Any;

pub const TIMED_GATE_RUNTIME_VERSION: u32 = 1;
pub const TIMED_GATE_MAX_INPUT_EVENTS: usize = 259;
pub const TIMED_GATE_MAX_OUTPUT_EVENTS: usize = 259;
pub const TIMED_GATE_MAX_ACTIVE_OCCURRENCES: usize = 64;
pub const TIMED_GATE_MAX_WORK_UNITS: usize = TIMED_GATE_MAX_INPUT_EVENTS * 4;

/// Scheduling domain for [`PreparedTimedGateMidiProcessor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimedGateDomain {
    /// Musical performance beats in Q32.32 quarter-note units.
    BeatsQ32 {
        delay_beats_q32: u64,
        gate_beats_q32: u64,
    },
    /// Monotonic host performance frames.
    PerformanceFrames { delay_frames: u64, gate_frames: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedTimedGateMidiProcessor {
    domain: TimedGateDomain,
    queue_capacity: usize,
    reserved_note_offs: usize,
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
}

impl PreparedTimedGateMidiProcessor {
    pub fn new(domain: TimedGateDomain) -> Result<Self, NativeMidiProcessorError> {
        Self::with_caps(
            domain,
            FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
            FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS,
            TIMED_GATE_MAX_INPUT_EVENTS,
            TIMED_GATE_MAX_OUTPUT_EVENTS,
            TIMED_GATE_MAX_ACTIVE_OCCURRENCES,
            TIMED_GATE_MAX_WORK_UNITS,
        )
    }

    pub fn with_caps(
        domain: TimedGateDomain,
        queue_capacity: usize,
        reserved_note_offs: usize,
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, NativeMidiProcessorError> {
        match domain {
            TimedGateDomain::BeatsQ32 {
                delay_beats_q32,
                gate_beats_q32,
            } => {
                if gate_beats_q32 == 0
                    || delay_beats_q32
                        .checked_add(gate_beats_q32)
                        .is_none_or(|total| total > FUTURE_MIDI_MAX_HORIZON_BEATS_Q32)
                {
                    return Err(NativeMidiProcessorError::FutureEventHorizonExceeded);
                }
            }
            TimedGateDomain::PerformanceFrames {
                delay_frames,
                gate_frames,
            } => {
                if gate_frames == 0
                    || delay_frames
                        .checked_add(gate_frames)
                        .is_none_or(|total| total > FUTURE_MIDI_MAX_HORIZON_FRAMES)
                {
                    return Err(NativeMidiProcessorError::FutureEventHorizonExceeded);
                }
            }
        }
        let _ = BoundedFutureMidiQueue::new(queue_capacity, reserved_note_offs)?;
        if max_input_events == 0
            || max_output_events == 0
            || max_active_occurrences == 0
            || max_active_occurrences > TIMED_GATE_MAX_ACTIVE_OCCURRENCES
            || max_work_units == 0
        {
            return Err(NativeMidiProcessorError::RuntimeBudgetExceeded);
        }
        Ok(Self {
            domain,
            queue_capacity,
            reserved_note_offs,
            max_input_events,
            max_output_events,
            max_active_occurrences,
            max_work_units,
        })
    }

    pub const fn domain(self) -> TimedGateDomain {
        self.domain
    }
}

impl PreparedNativeMidiProcessor for PreparedTimedGateMidiProcessor {
    fn runtime_version(&self) -> u32 {
        TIMED_GATE_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<TimedGateRuntime>()
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
        let queue = BoundedFutureMidiQueue::new(self.queue_capacity, self.reserved_note_offs)
            .unwrap_or_default();
        Box::new(TimedGateRuntime {
            config: *self,
            queue,
            next_generated_id: GENERATED_OCCURRENCE_ID_BASE,
            active_inputs: [None; TIMED_GATE_MAX_ACTIVE_OCCURRENCES],
            active_generated: [None; TIMED_GATE_MAX_ACTIVE_OCCURRENCES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
struct TimedGateRuntime {
    config: PreparedTimedGateMidiProcessor,
    queue: BoundedFutureMidiQueue,
    next_generated_id: u64,
    active_inputs: [Option<u64>; TIMED_GATE_MAX_ACTIVE_OCCURRENCES],
    active_generated: [Option<u64>; TIMED_GATE_MAX_ACTIVE_OCCURRENCES],
}

impl TimedGateRuntime {
    fn find_slot(slots: &[Option<u64>], id: u64) -> Option<usize> {
        slots.iter().position(|slot| *slot == Some(id))
    }

    fn find_free_slot(slots: &[Option<u64>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_used(slots: &[Option<u64>]) -> usize {
        slots.iter().flatten().count()
    }

    fn schedule_pair(
        domain: TimedGateDomain,
        frame_offset: usize,
        pitch: u8,
        velocity: u8,
        clock: Option<&dyn NativeInstrumentClock>,
        queue: &mut BoundedFutureMidiQueue,
        next_generated_id: &mut u64,
    ) -> Result<(), NativeMidiProcessorError> {
        let Some(clock_ref) = clock else {
            return Err(NativeMidiProcessorError::ClockUnavailable);
        };
        let (on_due, off_due) = match domain {
            TimedGateDomain::BeatsQ32 {
                delay_beats_q32,
                gate_beats_q32,
            } => {
                let now = clock_ref
                    .beat_at_offset(frame_offset)
                    .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
                let on_t = now
                    .checked_add(delay_beats_q32)
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                let off_t = on_t
                    .checked_add(gate_beats_q32)
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                (
                    FutureMidiTime::BeatQ32(on_t),
                    FutureMidiTime::BeatQ32(off_t),
                )
            }
            TimedGateDomain::PerformanceFrames {
                delay_frames,
                gate_frames,
            } => {
                let now = clock_ref
                    .performance_frame_at_offset(frame_offset)
                    .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
                let on_t = now
                    .checked_add(delay_frames)
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                let off_t = on_t
                    .checked_add(gate_frames)
                    .ok_or(NativeMidiProcessorError::FutureEventHorizonExceeded)?;
                (
                    FutureMidiTime::PerformanceFrame(on_t),
                    FutureMidiTime::PerformanceFrame(off_t),
                )
            }
        };
        let generated_id = *next_generated_id;
        *next_generated_id = generated_id
            .checked_add(1)
            .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
        // Enqueue NoteOn first (checks non-terminal limit), then paired NoteOff (uses reserved capacity).
        queue.schedule(
            on_due,
            ScheduledMidiKind::NoteOn {
                occurrence_id: generated_id,
                pitch,
                velocity,
            },
            clock,
        )?;
        queue.schedule(
            off_due,
            ScheduledMidiKind::NoteOff {
                occurrence_id: generated_id,
            },
            clock,
        )?;
        Ok(())
    }

    fn simulate_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        emit: Option<&mut MidiProcessorOutput<'_>>,
    ) -> Result<(Self, usize), NativeMidiProcessorError> {
        if frame_count == 0 {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        if input.len() > self.config.max_input_events {
            return Err(NativeMidiProcessorError::InputEventCapacityExceeded);
        }
        let mut state = self.clone();
        let active_cap = state.config.max_active_occurrences;
        let mut work_units = 0_usize;

        let mut passthrough = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; TIMED_GATE_MAX_INPUT_EVENTS];
        let mut passthrough_len = 0_usize;

        for &event in input {
            work_units = work_units
                .checked_add(1)
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            match event {
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    pitch,
                    velocity,
                } => {
                    if Self::find_slot(&state.active_inputs[..active_cap], occurrence_id).is_some()
                    {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_slot(&state.active_inputs[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    state.active_inputs[free] = Some(occurrence_id);
                    Self::schedule_pair(
                        state.config.domain,
                        frame_offset,
                        pitch,
                        velocity,
                        clock,
                        &mut state.queue,
                        &mut state.next_generated_id,
                    )?;
                    work_units = work_units
                        .checked_add(2)
                        .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = Self::find_slot(&state.active_inputs[..active_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    state.active_inputs[slot] = None;
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_slot(&state.active_inputs[..active_cap], occurrence_id.get())
                        .is_none()
                    {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    passthrough[passthrough_len] = event;
                    passthrough_len += 1;
                }
            }
            if work_units > state.config.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }

        // Drain all scheduled future events due in this block.
        let mut due_buf = [DueMidiEvent {
            frame_offset: 0,
            sequence: 0,
            kind: ScheduledMidiKind::NoteOff { occurrence_id: 1 },
        }; FUTURE_MIDI_EVENT_QUEUE_CAPACITY];
        let due_len = if !state.queue.is_empty() {
            state.queue.drain_due(frame_count, clock, &mut due_buf)?
        } else {
            0
        };

        // Update active generated occurrences and merge with passthrough events in
        // deterministic `(frame_offset, rank)` order (CC/Expr < NoteOff < NoteOn).
        let total_out = passthrough_len
            .checked_add(due_len)
            .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
        if total_out > output_capacity || total_out > state.config.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        work_units = work_units
            .checked_add(total_out)
            .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
        if work_units > state.config.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
        }

        let mut merged = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; TIMED_GATE_MAX_OUTPUT_EVENTS];
        let mut merged_len = 0_usize;

        for &ev in &passthrough[..passthrough_len] {
            merged[merged_len] = ev;
            merged_len += 1;
        }
        for &due in &due_buf[..due_len] {
            match due.kind {
                ScheduledMidiKind::NoteOn { occurrence_id, .. } => {
                    let free = Self::find_free_slot(&state.active_generated[..active_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    state.active_generated[free] = Some(occurrence_id);
                }
                ScheduledMidiKind::NoteOff { occurrence_id } => {
                    let slot =
                        Self::find_slot(&state.active_generated[..active_cap], occurrence_id)
                            .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    state.active_generated[slot] = None;
                }
                ScheduledMidiKind::ControlChange { .. } => {}
            }
            merged[merged_len] = due.into_native_event();
            merged_len += 1;
        }

        // Stable insertion sort by `(frame_offset, rank)` so CC < NoteOff < NoteOn.
        let sort_key = |ev: NativeInstrumentEvent| -> (usize, u8) {
            let rank = match ev {
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => 0,
                NativeInstrumentEvent::NoteOff { .. } => 1,
                NativeInstrumentEvent::NoteOn { .. } => 2,
                _ => 3,
            };
            (ev.frame_offset(), rank)
        };
        for i in 1..merged_len {
            let cur = merged[i];
            let key = sort_key(cur);
            let mut j = i;
            while j > 0 && sort_key(merged[j - 1]) > key {
                merged[j] = merged[j - 1];
                j -= 1;
            }
            merged[j] = cur;
        }

        if let Some(sink) = emit {
            let _ = sink.charge_work(work_units);
            for &ev in &merged[..merged_len] {
                let _ = sink.push(ev);
            }
        }
        Ok((state, merged_len))
    }
}

impl MutableMidiProcessorRuntime for TimedGateRuntime {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        let (_next, written) =
            self.simulate_block(input, frame_count, output_capacity, clock, None)?;
        Ok(written)
    }

    fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output: &mut MidiProcessorOutput<'_>,
        clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let cap = output.remaining_capacity();
        if let Ok((next, _)) = self.simulate_block(input, frame_count, cap, clock, Some(output)) {
            *self = next;
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
        let active = Self::count_used(&self.active_generated[..self.config.max_active_occurrences]);
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
        let mut ids = [0_u64; TIMED_GATE_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for slot in &mut self.active_generated[..active_cap] {
            if let Some(id) = slot.take() {
                ids[len] = id;
                len += 1;
            }
        }
        self.active_inputs.fill(None);
        self.queue.reset();
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
        let active_cap = self.config.max_active_occurrences;
        Self::count_used(&self.active_inputs[..active_cap])
            .max(Self::count_used(&self.active_generated[..active_cap]))
    }

    fn is_quiescent(&self) -> bool {
        self.active_occurrence_count() == 0 && self.queue.is_empty()
    }

    fn reset(&mut self) {
        self.active_inputs.fill(None);
        self.active_generated.fill(None);
        self.queue.reset();
        self.next_generated_id = GENERATED_OCCURRENCE_ID_BASE;
    }

    fn can_copy_history_from(&self, previous: &dyn MutableMidiProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| {
                other.config.domain == self.config.domain
                    && other.active_occurrence_count() <= self.config.max_active_occurrences
                    && other.queue.pending_count() <= self.queue.capacity()
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.active_inputs = other.active_inputs;
        self.active_generated = other.active_generated;
        self.queue = other.queue;
        self.next_generated_id = other.next_generated_id;
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
