//! Bounded engine event ownership for prepared mutable note runtimes.
//!
//! The lattice owns each runtime's fixed voice state. This module owns only
//! occurrence identity and the authored gate-off frame needed to pair source
//! spans with checked event IDs across render partitions.

use crate::midi_compile::MAX_SIMULTANEOUS_VOICES_PER_TRACK;
use hex_lattice::{
    NativeInstrumentEvent, NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits,
    NativeInstrumentRuntimePair, PreparedMutableInstrumentRuntime,
};
use hex_project::{ClipId, NoteId, TrackId};

pub(crate) const MAX_MUTABLE_EVENTS_PER_QUERY: usize = 256;
pub(crate) const MAX_MUTABLE_SPANS_PER_QUERY: usize = 192;
const MAX_MUTABLE_TAIL_SECONDS: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MutableOccurrenceKey {
    track: TrackId,
    clip: ClipId,
    note: NoteId,
    iteration: u64,
    loop_pass: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MutableNoteOnset {
    pub(crate) key: MutableOccurrenceKey,
    pub(crate) source_start: u64,
    pub(crate) source_gate_end: u64,
    pub(crate) pitch: u8,
    pub(crate) velocity: u8,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MutableOccurrence {
    key: MutableOccurrenceKey,
    id: u64,
    gate_end_frame: u64,
}

/// One source query's maximum admitted sounding intervals. This is shared by
/// tracks, so its fixed storage does not multiply by the project track count.
#[derive(Clone)]
pub(crate) struct MutableOnsetBuffer {
    spans: [Option<MutableNoteOnset>; MAX_MUTABLE_SPANS_PER_QUERY],
    len: usize,
}

impl Default for MutableOnsetBuffer {
    fn default() -> Self {
        Self {
            spans: [None; MAX_MUTABLE_SPANS_PER_QUERY],
            len: 0,
        }
    }
}

impl MutableOnsetBuffer {
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }

    pub(crate) fn push(&mut self, onset: MutableNoteOnset) -> Result<(), ()> {
        let Some(slot) = self.spans.get_mut(self.len) else {
            return Err(());
        };
        *slot = Some(onset);
        self.len += 1;
        Ok(())
    }

    fn iter(&self) -> impl Iterator<Item = MutableNoteOnset> + '_ {
        self.spans[..self.len].iter().filter_map(|span| *span)
    }

    fn sort_canonically(&mut self) {
        self.spans[..self.len].sort_unstable_by_key(|span| {
            span.map(|onset| {
                (
                    onset.source_start,
                    onset.key.track.value(),
                    onset.key.clip.value(),
                    onset.key.note.value(),
                    onset.key.iteration,
                    onset.key.loop_pass,
                )
            })
            .unwrap_or((u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX, u64::MAX))
        });
    }
}

impl MutableOccurrenceKey {
    pub(crate) fn new(
        track: TrackId,
        clip: ClipId,
        note: NoteId,
        iteration: u64,
        loop_pass: u64,
    ) -> Self {
        Self {
            track,
            clip,
            note,
            iteration,
            loop_pass,
        }
    }
}

/// Per-track mutable pair and transactional occurrence ledger. Both ledger
/// buffers are reserved during control-side construction.
pub(crate) struct MutableInstrumentTrackRuntime {
    pair: NativeInstrumentRuntimePair,
    primary: Vec<MutableOccurrence>,
    scratch: Vec<MutableOccurrence>,
    next_id: u64,
    next_id_scratch: u64,
}

/// A moved mutable instrument owner and its captured route identity. Tail
/// owners remain allocated in their graph until control-side retirement.
pub(crate) struct MutableInstrumentTail {
    pub(crate) track_id: TrackId,
    pub(crate) runtime: MutableInstrumentTrackRuntime,
}

impl MutableInstrumentTrackRuntime {
    pub(crate) fn new(
        prepared: &dyn PreparedMutableInstrumentRuntime,
        sample_rate: u32,
    ) -> Result<Self, MutableInstrumentRuntimeError> {
        let max_tail_frames = u64::from(sample_rate)
            .checked_mul(MAX_MUTABLE_TAIL_SECONDS)
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        let pair = NativeInstrumentRuntimePair::new(
            prepared,
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES * 2,
                max_events_per_block: MAX_MUTABLE_EVENTS_PER_QUERY,
                max_active_occurrences: MAX_SIMULTANEOUS_VOICES_PER_TRACK,
                max_tail_frames,
            },
        )
        .map_err(map_runtime_error)?;
        let pair_bytes = pair
            .fixed_storage_pair_bytes()
            .ok_or(MutableInstrumentRuntimeError::Budget)?;
        if pair_bytes > pair.declared_pair_bytes() {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        let mut primary = Vec::new();
        primary
            .try_reserve_exact(MAX_SIMULTANEOUS_VOICES_PER_TRACK)
            .map_err(|_| MutableInstrumentRuntimeError::Allocation)?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(MAX_SIMULTANEOUS_VOICES_PER_TRACK)
            .map_err(|_| MutableInstrumentRuntimeError::Allocation)?;
        if primary.capacity() > MAX_SIMULTANEOUS_VOICES_PER_TRACK
            || scratch.capacity() > MAX_SIMULTANEOUS_VOICES_PER_TRACK
        {
            return Err(MutableInstrumentRuntimeError::Budget);
        }
        Ok(Self {
            pair,
            primary,
            scratch,
            next_id: 0,
            next_id_scratch: 0,
        })
    }

    pub(crate) fn begin_block(&mut self) {
        self.pair.begin_block();
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.primary);
        self.next_id_scratch = self.next_id;
    }

    pub(crate) fn committed_is_quiescent(&self) -> bool {
        self.primary.is_empty() && self.pair.committed_is_quiescent()
    }

    #[cfg(test)]
    pub(crate) fn committed_ledger_is_empty(&self) -> bool {
        self.primary.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn committed_feedback_tail_is_active(&self) -> bool {
        self.primary.is_empty() && !self.pair.committed_is_quiescent()
    }

    #[cfg(test)]
    pub(crate) fn next_occurrence_id(&self) -> u64 {
        self.next_id
    }

    /// Check whether this freshly prepared owner can take the exact committed
    /// mutable history from `previous` without allocating or changing either
    /// owner. The canonical revision descriptor and route checks remain the
    /// caller's responsibility.
    pub(crate) fn can_inherit_from(&self, previous: &Self) -> bool {
        self.primary.capacity() >= previous.primary.len()
            && self.scratch.capacity() >= previous.primary.len()
            && self.pair.can_inherit_from(&previous.pair)
    }

    /// Copy a preflighted committed history into both candidate twins and
    /// both occurrence ledgers. Keeping the old owner intact until graph
    /// retirement makes handoff rollback and callback ownership safe.
    pub(crate) fn inherit_from(
        &mut self,
        previous: &Self,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        if self.primary.capacity() < previous.primary.len()
            || self.scratch.capacity() < previous.primary.len()
            || !self.pair.can_inherit_from(&previous.pair)
        {
            return Err(MutableInstrumentRuntimeError::Runtime(
                NativeInstrumentRuntimeError::IncompatibleHistory,
            ));
        }
        self.pair
            .inherit_from(&previous.pair)
            .map_err(map_runtime_error)?;
        self.primary.clear();
        self.primary.extend_from_slice(&previous.primary);
        self.scratch.clear();
        self.scratch.extend_from_slice(&previous.primary);
        self.next_id = previous.next_id;
        self.next_id_scratch = previous.next_id;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn ledger_capacity_bytes(&self) -> Option<usize> {
        self.primary
            .capacity()
            .checked_add(self.scratch.capacity())?
            .checked_mul(std::mem::size_of::<MutableOccurrence>())
    }

    #[cfg(test)]
    pub(crate) fn runtime_pair_storage_bytes(&self) -> Option<usize> {
        self.pair.fixed_storage_pair_bytes()
    }

    pub(crate) fn speculative_is_quiescent(&self) -> bool {
        self.scratch.is_empty() && self.pair.speculative_is_quiescent()
    }

    pub(crate) fn declared_pair_bytes(&self) -> usize {
        self.pair.declared_pair_bytes()
    }

    pub(crate) fn max_active_occurrences(&self) -> usize {
        self.pair.max_active_occurrences()
    }

    /// Cut still-gated occurrences at a publication boundary when the
    /// candidate has no enabled instrument. The runtime receives the matching
    /// frame-zero NoteOff in its next segment; feedback and release state stay
    /// owned and are not reset.
    pub(crate) fn cut_gate_deadlines_at(&mut self, performance_frame: u64) {
        for occurrence in &mut self.primary {
            if occurrence.gate_end_frame > performance_frame {
                occurrence.gate_end_frame = performance_frame;
            }
        }
    }

    pub(crate) fn process_segment(
        &mut self,
        track: TrackId,
        performance_start: u64,
        source_start: u64,
        frames: &mut [[f32; 2]],
        onsets: &mut MutableOnsetBuffer,
        events: &mut Vec<NativeInstrumentEvent>,
    ) -> Result<(), MutableInstrumentRuntimeError> {
        let width =
            u64::try_from(frames.len()).map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
        let performance_end = performance_start
            .checked_add(width)
            .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
        events.clear();
        onsets.sort_canonically();

        let mut remove_count = 0;
        for occurrence in &self.scratch {
            if occurrence.gate_end_frame < performance_start {
                return Err(MutableInstrumentRuntimeError::OrphanedGate);
            }
            if occurrence.gate_end_frame < performance_end {
                let offset = usize::try_from(occurrence.gate_end_frame - performance_start)
                    .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
                push_event(
                    events,
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: offset,
                        occurrence_id: occurrence.id,
                    },
                )?;
                remove_count += 1;
            }
        }
        if remove_count > 0 {
            let mut write = 0;
            for read in 0..self.scratch.len() {
                let occurrence = self.scratch[read];
                if occurrence.gate_end_frame >= performance_end {
                    self.scratch[write] = occurrence;
                    write += 1;
                }
            }
            self.scratch.truncate(write);
        }

        for (onset_index, onset) in onsets.iter().enumerate() {
            if onset.key.track != track || onset.source_start < source_start {
                continue;
            }
            let Some(onset_delta) = onset.source_start.checked_sub(source_start) else {
                continue;
            };
            if onset_delta >= width {
                continue;
            }
            if self.scratch.iter().any(|active| active.key == onset.key)
                || onsets
                    .iter()
                    .take(onset_index)
                    .any(|previous| previous.key == onset.key)
            {
                return Err(MutableInstrumentRuntimeError::DuplicateOccurrence);
            }
            self.next_id_scratch = self
                .next_id_scratch
                .checked_add(1)
                .ok_or(MutableInstrumentRuntimeError::IdentityExhausted)?;
            let gate_delta = onset
                .source_gate_end
                .checked_sub(source_start)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            let gate_end_frame = performance_start
                .checked_add(gate_delta)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            let onset_frame = performance_start
                .checked_add(onset_delta)
                .ok_or(MutableInstrumentRuntimeError::Arithmetic)?;
            if gate_end_frame <= onset_frame {
                return Err(MutableInstrumentRuntimeError::InvalidSourceSpan);
            }
            let onset_offset = usize::try_from(onset_delta)
                .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
            let id = self.next_id_scratch;
            push_event(
                events,
                NativeInstrumentEvent::NoteOn {
                    frame_offset: onset_offset,
                    occurrence_id: id,
                    pitch: onset.pitch,
                    velocity: onset.velocity,
                },
            )?;
            if gate_end_frame < performance_end {
                let offset = usize::try_from(gate_end_frame - performance_start)
                    .map_err(|_| MutableInstrumentRuntimeError::Arithmetic)?;
                push_event(
                    events,
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: offset,
                        occurrence_id: id,
                    },
                )?;
            } else {
                // Gateoffs at the exclusive block end are emitted at offset
                // zero of the next segment, preserving half-open semantics.
                if self.scratch.len() >= MAX_SIMULTANEOUS_VOICES_PER_TRACK {
                    return Err(MutableInstrumentRuntimeError::OccurrenceCapacity);
                }
                self.scratch.push(MutableOccurrence {
                    key: onset.key,
                    id,
                    gate_end_frame,
                });
            }
        }

        events.sort_unstable_by_key(|event| {
            let (offset, is_on, occurrence_id) = match *event {
                NativeInstrumentEvent::NoteOff {
                    frame_offset,
                    occurrence_id,
                } => (frame_offset, false, occurrence_id),
                NativeInstrumentEvent::NoteOn {
                    frame_offset,
                    occurrence_id,
                    ..
                } => (frame_offset, true, occurrence_id),
            };
            (offset, is_on, occurrence_id)
        });
        self.pair
            .process_block(events, frames)
            .map_err(map_runtime_error)
    }

    pub(crate) fn commit_block(&mut self) {
        self.pair.commit_block();
        std::mem::swap(&mut self.primary, &mut self.scratch);
        self.next_id = self.next_id_scratch;
    }

    pub(crate) fn rollback_block(&mut self) {
        self.pair.rollback_block();
    }

    pub(crate) fn reset(&mut self) {
        self.pair.reset();
        self.primary.clear();
        self.scratch.clear();
        self.next_id = 0;
        self.next_id_scratch = 0;
    }
}

/// Maximum fixed host-side storage attributable to mutable instrument
/// ownership for a graph with `track_count` routes, including the complete
/// tail bank and two ledgers per active or retained owner. Runtime pair
/// payloads are separately checked against the lattice DSP budget.
pub(crate) fn host_storage_upper_bound(track_count: usize) -> Option<usize> {
    if track_count > hex_project::MAX_TRACKS {
        return None;
    }
    let runtime_slots =
        track_count.checked_mul(std::mem::size_of::<Option<MutableInstrumentTrackRuntime>>())?;
    let active_ledger_entries = track_count
        .checked_mul(2)?
        .checked_mul(MAX_SIMULTANEOUS_VOICES_PER_TRACK)?
        .checked_mul(std::mem::size_of::<MutableOccurrence>())?;
    let tail_slots = hex_lattice::MAX_STATEFUL_PROCESSORS;
    let tail_owner_slots =
        tail_slots.checked_mul(std::mem::size_of::<Option<MutableInstrumentTail>>())?;
    let tail_ledger_entries = tail_slots
        .checked_mul(2)?
        .checked_mul(MAX_SIMULTANEOUS_VOICES_PER_TRACK)?
        .checked_mul(std::mem::size_of::<MutableOccurrence>())?;
    let events =
        MAX_MUTABLE_EVENTS_PER_QUERY.checked_mul(std::mem::size_of::<NativeInstrumentEvent>())?;
    let vector_headers = std::mem::size_of::<Vec<Option<MutableInstrumentTrackRuntime>>>()
        .checked_add(std::mem::size_of::<Vec<NativeInstrumentEvent>>())?
        .checked_add(std::mem::size_of::<Vec<Option<MutableInstrumentTail>>>())?;
    runtime_slots
        .checked_add(active_ledger_entries)?
        .checked_add(tail_owner_slots)?
        .checked_add(tail_ledger_entries)?
        .checked_add(std::mem::size_of::<MutableOnsetBuffer>())?
        .checked_add(events)?
        .checked_add(vector_headers)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MutableInstrumentRuntimeError {
    Allocation,
    Budget,
    Arithmetic,
    OrphanedGate,
    DuplicateOccurrence,
    OccurrenceCapacity,
    IdentityExhausted,
    InvalidSourceSpan,
    EventCapacity,
    Runtime(NativeInstrumentRuntimeError),
}

impl From<MutableInstrumentRuntimeError> for crate::SessionPlaybackError {
    fn from(error: MutableInstrumentRuntimeError) -> Self {
        match error {
            MutableInstrumentRuntimeError::Allocation => Self::Allocation,
            MutableInstrumentRuntimeError::Budget | MutableInstrumentRuntimeError::Runtime(_) => {
                Self::MutableInstrumentRuntime
            }
            MutableInstrumentRuntimeError::Arithmetic
            | MutableInstrumentRuntimeError::IdentityExhausted => Self::ArithmeticOverflow,
            MutableInstrumentRuntimeError::OrphanedGate
            | MutableInstrumentRuntimeError::DuplicateOccurrence
            | MutableInstrumentRuntimeError::OccurrenceCapacity
            | MutableInstrumentRuntimeError::InvalidSourceSpan
            | MutableInstrumentRuntimeError::EventCapacity => Self::MutableInstrumentEvents,
        }
    }
}

fn push_event(
    events: &mut Vec<NativeInstrumentEvent>,
    event: NativeInstrumentEvent,
) -> Result<(), MutableInstrumentRuntimeError> {
    if events.len() >= MAX_MUTABLE_EVENTS_PER_QUERY {
        return Err(MutableInstrumentRuntimeError::EventCapacity);
    }
    events.push(event);
    Ok(())
}

fn map_runtime_error(error: NativeInstrumentRuntimeError) -> MutableInstrumentRuntimeError {
    match error {
        NativeInstrumentRuntimeError::RuntimeBudgetExceeded => {
            MutableInstrumentRuntimeError::Budget
        }
        _ => MutableInstrumentRuntimeError::Runtime(error),
    }
}

/// Checked complete active-pair budget. The already-prepared effect graph
/// admits at most 256 stateful instances with two 4 KiB twins each.
pub(crate) fn validate_mutable_graph_budget(
    existing_count: usize,
    existing_bytes: usize,
    pair_count: usize,
    pair_bytes: usize,
) -> Result<(), MutableInstrumentRuntimeError> {
    let total_count = existing_count
        .checked_add(pair_count)
        .ok_or(MutableInstrumentRuntimeError::Budget)?;
    let total_bytes = existing_bytes
        .checked_add(pair_bytes)
        .ok_or(MutableInstrumentRuntimeError::Budget)?;
    if total_count > hex_lattice::MAX_STATEFUL_PROCESSORS
        || total_bytes
            > hex_lattice::MAX_STATEFUL_PROCESSORS * hex_lattice::MAX_PROCESSOR_RUNTIME_BYTES * 2
    {
        return Err(MutableInstrumentRuntimeError::Budget);
    }
    Ok(())
}
