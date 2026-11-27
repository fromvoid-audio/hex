//! Bounded per-source future MIDI event queue and host clock conversion.
//!
//! Timed native MIDI processors (such as arpeggiators and note-length
//! processors) own a [`BoundedFutureMidiQueue`] inside their transactional
//! primary/scratch runtime twins. This guarantees:
//! - Finite per-source event capacity and explicit finite beat/frame horizons.
//! - Reserved `NoteOff` capacity so already-started generated occurrences can
//!   always schedule their terminal release under queue pressure.
//! - Deterministic equal-offset ordering (`ControlChange` < `NoteOff` < `NoteOn`,
//!   then monotonic insertion sequence).
//! - Allocation-free host clock conversion (`NativeInstrumentClock`) across
//!   arbitrary block partitions and tempo changes without a second scheduler.
//! - Transactional commit/rollback, owner invalidation, and `Stop`/reset
//!   cleanup with zero callback allocation, locks, I/O, or final drops.

use super::{NativeInstrumentClock, NativeInstrumentEvent, NativeMidiProcessorError};

/// Maximum simultaneous scheduled future events per MIDI processor source.
pub const FUTURE_MIDI_EVENT_QUEUE_CAPACITY: usize = 128;

/// Reserved slots for terminal `NoteOff` events so active generated notes can
/// always enqueue their release even when non-terminal capacity is saturated.
pub const FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS: usize = 64;

/// Maximum beat-domain scheduling horizon ahead of the current block start
/// (64 quarter-note beats in Q32.32 fixed point).
pub const FUTURE_MIDI_MAX_HORIZON_BEATS_Q32: u64 = 64_u64 << 32;

/// Maximum frame-domain scheduling horizon ahead of the current block start
/// (60 seconds at 192 kHz).
pub const FUTURE_MIDI_MAX_HORIZON_FRAMES: u64 = 192_000 * 60;

/// Disjoint base for generated downstream occurrence IDs so timed generators
/// never collide with raw host occurrence IDs during bypass or live handoffs.
pub const GENERATED_OCCURRENCE_ID_BASE: u64 = 1_u64 << 48;

/// Explicit time coordinate for a scheduled future MIDI event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FutureMidiTime {
    /// Musical performance beat in Q32.32 quarter-note units (follows host tempo).
    BeatQ32(u64),
    /// Monotonic host performance frame.
    PerformanceFrame(u64),
}

/// Payload of one scheduled future MIDI event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduledMidiKind {
    ControlChange {
        cc: u8,
        value: u8,
    },
    NoteOff {
        occurrence_id: u64,
    },
    NoteOn {
        occurrence_id: u64,
        pitch: u8,
        velocity: u8,
    },
}

impl ScheduledMidiKind {
    const fn priority_rank(self) -> u8 {
        match self {
            Self::ControlChange { .. } => 0,
            Self::NoteOff { .. } => 1,
            Self::NoteOn { .. } => 2,
        }
    }

    const fn is_note_off(self) -> bool {
        matches!(self, Self::NoteOff { .. })
    }

    const fn is_valid(self) -> bool {
        match self {
            Self::ControlChange { cc, value } => cc <= 127 && value <= 127,
            Self::NoteOff { occurrence_id } => occurrence_id != 0,
            Self::NoteOn {
                occurrence_id,
                pitch,
                velocity,
            } => occurrence_id != 0 && pitch <= 127 && velocity >= 1 && velocity <= 127,
        }
    }

    pub const fn into_native_event(self, frame_offset: usize) -> NativeInstrumentEvent {
        match self {
            Self::ControlChange { cc, value } => NativeInstrumentEvent::ControlChange {
                frame_offset,
                cc,
                value,
            },
            Self::NoteOff { occurrence_id } => NativeInstrumentEvent::NoteOff {
                frame_offset,
                occurrence_id,
            },
            Self::NoteOn {
                occurrence_id,
                pitch,
                velocity,
            } => NativeInstrumentEvent::NoteOn {
                frame_offset,
                occurrence_id,
                pitch,
                velocity,
            },
        }
    }
}

/// One queued future MIDI event owned by a specific source owner generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduledMidiEvent {
    pub due: FutureMidiTime,
    pub owner_generation: u64,
    pub sequence: u64,
    pub kind: ScheduledMidiKind,
}

/// Resolved due event ready for emission inside the current audio block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DueMidiEvent {
    pub frame_offset: usize,
    pub sequence: u64,
    pub kind: ScheduledMidiKind,
}

impl DueMidiEvent {
    const fn sort_key(self) -> (usize, u8, u64) {
        (self.frame_offset, self.kind.priority_rank(), self.sequence)
    }

    pub const fn into_native_event(self) -> NativeInstrumentEvent {
        self.kind.into_native_event(self.frame_offset)
    }
}

/// Resolve whether `due` falls within the current block `[0, frame_count)`
/// under the host's borrowed [`NativeInstrumentClock`].
///
/// Uses allocation-free binary search for `BeatQ32` so resolution stays
/// sample-exact and block-partition invariant across arbitrary tempo changes.
/// Returns `Ok(Some(offset))` if due in this block (overdue events clamp to 0),
/// `Ok(None)` if strictly in the future beyond `frame_count`, or `Err` when
/// the required host clock domain is unavailable.
pub fn resolve_future_time_in_block(
    due: FutureMidiTime,
    frame_count: usize,
    clock: Option<&dyn NativeInstrumentClock>,
) -> Result<Option<usize>, NativeMidiProcessorError> {
    if frame_count == 0 {
        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
    }
    let Some(clock) = clock else {
        return Err(NativeMidiProcessorError::ClockUnavailable);
    };
    match due {
        FutureMidiTime::PerformanceFrame(target) => {
            let start = clock
                .performance_frame_at_offset(0)
                .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
            if target <= start {
                return Ok(Some(0));
            }
            let delta = target - start;
            if delta < frame_count as u64 {
                Ok(Some(delta as usize))
            } else {
                Ok(None)
            }
        }
        FutureMidiTime::BeatQ32(target_beat) => {
            let start_beat = clock
                .beat_at_offset(0)
                .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
            if target_beat <= start_beat {
                return Ok(Some(0));
            }
            let end_beat = clock
                .beat_at_offset(frame_count)
                .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
            if end_beat < start_beat {
                return Err(NativeMidiProcessorError::ClockUnavailable);
            }
            if target_beat >= end_beat {
                return Ok(None);
            }
            // Binary search for the smallest offset in `1..frame_count` whose
            // beat is >= `target_beat`.
            let mut lo = 1_usize;
            let mut hi = frame_count;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let mid_beat = clock
                    .beat_at_offset(mid)
                    .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
                if mid_beat < target_beat {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            if lo < frame_count {
                Ok(Some(lo))
            } else {
                Ok(None)
            }
        }
    }
}

/// Validate that `due` lies within the finite scheduling horizon relative to
/// the current block start on `clock`.
pub fn validate_future_horizon(
    due: FutureMidiTime,
    clock: Option<&dyn NativeInstrumentClock>,
) -> Result<(), NativeMidiProcessorError> {
    let Some(clock) = clock else {
        return Err(NativeMidiProcessorError::ClockUnavailable);
    };
    match due {
        FutureMidiTime::BeatQ32(target) => {
            let now = clock
                .beat_at_offset(0)
                .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
            if target.saturating_sub(now) > FUTURE_MIDI_MAX_HORIZON_BEATS_Q32 {
                return Err(NativeMidiProcessorError::FutureEventHorizonExceeded);
            }
            Ok(())
        }
        FutureMidiTime::PerformanceFrame(target) => {
            let now = clock
                .performance_frame_at_offset(0)
                .ok_or(NativeMidiProcessorError::ClockUnavailable)?;
            if target.saturating_sub(now) > FUTURE_MIDI_MAX_HORIZON_FRAMES {
                return Err(NativeMidiProcessorError::FutureEventHorizonExceeded);
            }
            Ok(())
        }
    }
}

/// Fixed-capacity, allocation-free per-source future MIDI event queue.
///
/// Embedded directly inside a timed processor's committed primary and
/// speculative scratch runtime twins so preflight, rollback, commit, reset,
/// and live history inheritance are automatically transactional.
#[derive(Clone, Copy, Debug)]
pub struct BoundedFutureMidiQueue {
    slots: [Option<ScheduledMidiEvent>; FUTURE_MIDI_EVENT_QUEUE_CAPACITY],
    capacity: usize,
    reserved_note_offs: usize,
    owner_generation: u64,
    next_sequence: u64,
}

impl Default for BoundedFutureMidiQueue {
    fn default() -> Self {
        Self::new(
            FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
            FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS,
        )
        .unwrap_or(Self {
            slots: [None; FUTURE_MIDI_EVENT_QUEUE_CAPACITY],
            capacity: FUTURE_MIDI_EVENT_QUEUE_CAPACITY,
            reserved_note_offs: FUTURE_MIDI_RESERVED_NOTE_OFF_SLOTS,
            owner_generation: 1,
            next_sequence: 1,
        })
    }
}

impl BoundedFutureMidiQueue {
    pub fn new(
        capacity: usize,
        reserved_note_offs: usize,
    ) -> Result<Self, NativeMidiProcessorError> {
        if capacity == 0
            || capacity > FUTURE_MIDI_EVENT_QUEUE_CAPACITY
            || reserved_note_offs >= capacity
        {
            return Err(NativeMidiProcessorError::RuntimeBudgetExceeded);
        }
        Ok(Self {
            slots: [None; FUTURE_MIDI_EVENT_QUEUE_CAPACITY],
            capacity,
            reserved_note_offs,
            owner_generation: 1,
            next_sequence: 1,
        })
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub const fn reserved_note_offs(&self) -> usize {
        self.reserved_note_offs
    }

    pub const fn owner_generation(&self) -> u64 {
        self.owner_generation
    }

    pub fn pending_count(&self) -> usize {
        self.slots[..self.capacity].iter().flatten().count()
    }

    pub fn pending_note_off_count(&self) -> usize {
        self.slots[..self.capacity]
            .iter()
            .flatten()
            .filter(|ev| ev.kind.is_note_off())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.slots[..self.capacity].iter().all(Option::is_none)
    }

    /// Clear all queued events and advance the owner generation (Stop / Seek / hard reset).
    pub fn reset(&mut self) {
        self.slots.fill(None);
        self.owner_generation = self.owner_generation.wrapping_add(1).max(1);
        self.next_sequence = 1;
    }

    /// Invalidate non-terminal future events (`NoteOn`, `ControlChange`) owned
    /// by prior source generations while preserving already-scheduled terminal
    /// `NoteOff`s so active downstream voices never stick.
    pub fn invalidate_non_terminal_events(&mut self) {
        self.owner_generation = self.owner_generation.wrapping_add(1).max(1);
        for slot in &mut self.slots[..self.capacity] {
            if let Some(ev) = *slot
                && !ev.kind.is_note_off()
            {
                *slot = None;
            }
        }
    }

    /// Enqueue one future event after validating payload, horizon, and
    /// reserved `NoteOff` capacity. Never allocates.
    pub fn schedule(
        &mut self,
        due: FutureMidiTime,
        kind: ScheduledMidiKind,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), NativeMidiProcessorError> {
        if !kind.is_valid() {
            return Err(NativeMidiProcessorError::InvalidEventPayload);
        }
        validate_future_horizon(due, clock)?;
        let pending = self.pending_count();
        if kind.is_note_off() {
            if pending >= self.capacity {
                return Err(NativeMidiProcessorError::FutureQueueCapacityExceeded);
            }
        } else {
            let non_terminal_limit = self.capacity.saturating_sub(self.reserved_note_offs);
            let non_terminal_used = pending.saturating_sub(self.pending_note_off_count());
            if pending >= self.capacity || non_terminal_used >= non_terminal_limit {
                return Err(NativeMidiProcessorError::FutureQueueCapacityExceeded);
            }
        }
        let free = self.slots[..self.capacity]
            .iter()
            .position(Option::is_none)
            .ok_or(NativeMidiProcessorError::FutureQueueCapacityExceeded)?;
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(NativeMidiProcessorError::FutureQueueCapacityExceeded)?;
        self.slots[free] = Some(ScheduledMidiEvent {
            due,
            owner_generation: self.owner_generation,
            sequence,
            kind,
        });
        Ok(())
    }

    /// Cancel any queued future event matching `occurrence_id` (e.g., when a
    /// note is manually released or cancelled before its scheduled gate end).
    pub fn cancel_occurrence(&mut self, occurrence_id: u64) -> usize {
        let mut removed = 0_usize;
        for slot in &mut self.slots[..self.capacity] {
            let matches = slot.is_some_and(|ev| match ev.kind {
                ScheduledMidiKind::NoteOn {
                    occurrence_id: id, ..
                }
                | ScheduledMidiKind::NoteOff { occurrence_id: id } => id == occurrence_id,
                ScheduledMidiKind::ControlChange { .. } => false,
            });
            if matches {
                *slot = None;
                removed += 1;
            }
        }
        removed
    }

    /// Read-only count of queued events due in `[0, frame_count)`.
    pub fn count_due(
        &self,
        frame_count: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        let mut due_count = 0_usize;
        for ev in self.slots[..self.capacity].iter().flatten() {
            if resolve_future_time_in_block(ev.due, frame_count, clock)?.is_some() {
                due_count += 1;
            }
        }
        Ok(due_count)
    }

    /// Drain all queued events due in `[0, frame_count)` into `out`, stably
    /// sorted by `(frame_offset, priority_rank, sequence)` where
    /// `ControlChange` < `NoteOff` < `NoteOn`. Allocation-free.
    pub fn drain_due(
        &mut self,
        frame_count: usize,
        clock: Option<&dyn NativeInstrumentClock>,
        out: &mut [DueMidiEvent],
    ) -> Result<usize, NativeMidiProcessorError> {
        let mut len = 0_usize;
        for slot in &mut self.slots[..self.capacity] {
            let Some(ev) = *slot else {
                continue;
            };
            if let Some(frame_offset) = resolve_future_time_in_block(ev.due, frame_count, clock)? {
                let dst = out
                    .get_mut(len)
                    .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
                *dst = DueMidiEvent {
                    frame_offset,
                    sequence: ev.sequence,
                    kind: ev.kind,
                };
                len += 1;
                *slot = None;
            }
        }
        // Allocation-free insertion sort preserving exact (offset, priority, sequence) order.
        for i in 1..len {
            let current = out[i];
            let key = current.sort_key();
            let mut j = i;
            while j > 0 && out[j - 1].sort_key() > key {
                out[j] = out[j - 1];
                j -= 1;
            }
            out[j] = current;
        }
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempoRampClock {
        start_frame: u64,
        /// Two-segment tempo: `bpm1` before `change_frame`, `bpm2` at/after `change_frame`.
        change_frame: u64,
        sample_rate: u32,
        micro_bpm_1: u32,
        micro_bpm_2: u32,
    }

    impl TempoRampClock {
        fn frame_to_beat_q32(&self, frame: u64) -> u64 {
            let f1 = frame.min(self.change_frame);
            let f2 = frame.saturating_sub(self.change_frame);
            let denom = u128::from(self.sample_rate) * 60_000_000;
            let b1 = (u128::from(f1) * u128::from(self.micro_bpm_1) * (1_u128 << 32)) / denom;
            let b2 = (u128::from(f2) * u128::from(self.micro_bpm_2) * (1_u128 << 32)) / denom;
            (b1 + b2) as u64
        }
    }

    impl NativeInstrumentClock for TempoRampClock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.frame_to_beat_q32(self.start_frame + offset as u64))
        }

        fn performance_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some(self.start_frame + offset as u64)
        }

        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
    }

    #[test]
    fn horizon_reserved_note_off_priority_and_owner_invalidation() {
        let clock = TempoRampClock {
            start_frame: 0,
            change_frame: 100_000,
            sample_rate: 48_000,
            micro_bpm_1: 120_000_000,
            micro_bpm_2: 120_000_000,
        };
        let mut queue = BoundedFutureMidiQueue::new(4, 2).unwrap();

        // Exceeding beat or frame horizon refuses explicitly.
        assert_eq!(
            queue.schedule(
                FutureMidiTime::BeatQ32(FUTURE_MIDI_MAX_HORIZON_BEATS_Q32 + 1),
                ScheduledMidiKind::NoteOn {
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                },
                Some(&clock),
            ),
            Err(NativeMidiProcessorError::FutureEventHorizonExceeded)
        );
        assert_eq!(
            queue.schedule(
                FutureMidiTime::PerformanceFrame(FUTURE_MIDI_MAX_HORIZON_FRAMES + 1),
                ScheduledMidiKind::NoteOn {
                    occurrence_id: 1,
                    pitch: 60,
                    velocity: 100,
                },
                Some(&clock),
            ),
            Err(NativeMidiProcessorError::FutureEventHorizonExceeded)
        );

        // Non-terminal slots cap at capacity - reserved_note_offs (4 - 2 = 2).
        for id in 1..=2 {
            queue
                .schedule(
                    FutureMidiTime::PerformanceFrame(10),
                    ScheduledMidiKind::NoteOn {
                        occurrence_id: id,
                        pitch: 60,
                        velocity: 100,
                    },
                    Some(&clock),
                )
                .unwrap();
        }
        assert_eq!(
            queue.schedule(
                FutureMidiTime::PerformanceFrame(10),
                ScheduledMidiKind::NoteOn {
                    occurrence_id: 3,
                    pitch: 60,
                    velocity: 100,
                },
                Some(&clock),
            ),
            Err(NativeMidiProcessorError::FutureQueueCapacityExceeded)
        );

        // Reserved NoteOff slots remain available even when NoteOn capacity is saturated!
        queue
            .schedule(
                FutureMidiTime::PerformanceFrame(10),
                ScheduledMidiKind::NoteOff { occurrence_id: 1 },
                Some(&clock),
            )
            .unwrap();
        queue
            .schedule(
                FutureMidiTime::PerformanceFrame(20),
                ScheduledMidiKind::NoteOff { occurrence_id: 2 },
                Some(&clock),
            )
            .unwrap();

        // Equal-offset ordering places NoteOff strictly before NoteOn!
        let mut due = [DueMidiEvent {
            frame_offset: 0,
            sequence: 0,
            kind: ScheduledMidiKind::NoteOff { occurrence_id: 1 },
        }; 8];
        let count = queue.drain_due(16, Some(&clock), &mut due).unwrap();
        assert_eq!(count, 3);
        assert!(matches!(
            due[0].kind,
            ScheduledMidiKind::NoteOff { occurrence_id: 1 }
        ));
        assert!(matches!(
            due[1].kind,
            ScheduledMidiKind::NoteOn {
                occurrence_id: 1,
                ..
            }
        ));
        assert!(matches!(
            due[2].kind,
            ScheduledMidiKind::NoteOn {
                occurrence_id: 2,
                ..
            }
        ));

        // Owner invalidation drops pending NoteOn/CC while keeping pending NoteOff for occurrence 2.
        queue
            .schedule(
                FutureMidiTime::PerformanceFrame(25),
                ScheduledMidiKind::NoteOn {
                    occurrence_id: 9,
                    pitch: 64,
                    velocity: 90,
                },
                Some(&clock),
            )
            .unwrap();
        queue.invalidate_non_terminal_events();
        assert_eq!(queue.pending_count(), 1);
        assert_eq!(queue.pending_note_off_count(), 1);
    }

    #[test]
    fn beat_clock_resolution_is_partition_invariant_across_tempo_step() {
        // Tempo jumps from 120 BPM to 240 BPM at frame 1000.
        let target_beat = 1_u64 << 30; // 1/4 beat (6000 frames at 120 BPM, faster after frame 1000).
        let probe = TempoRampClock {
            start_frame: 0,
            change_frame: 1_000,
            sample_rate: 48_000,
            micro_bpm_1: 120_000_000,
            micro_bpm_2: 240_000_000,
        };
        let whole_offset =
            resolve_future_time_in_block(FutureMidiTime::BeatQ32(target_beat), 8_000, Some(&probe))
                .unwrap()
                .unwrap();

        for chunk_size in [1, 7, 19, 64, 257] {
            let mut found = None;
            let mut cursor = 0_usize;
            while cursor < 8_000 {
                let len = chunk_size.min(8_000 - cursor);
                let block_clock = TempoRampClock {
                    start_frame: cursor as u64,
                    ..probe
                };
                if let Some(off) = resolve_future_time_in_block(
                    FutureMidiTime::BeatQ32(target_beat),
                    len,
                    Some(&block_clock),
                )
                .unwrap()
                {
                    found = Some(cursor + off);
                    break;
                }
                cursor += len;
            }
            assert_eq!(
                found,
                Some(whole_offset),
                "chunk size {chunk_size} must hit exact same absolute frame across tempo change"
            );
        }
    }
}
