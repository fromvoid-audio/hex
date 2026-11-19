//! Bounded ordered live MIDI note ingress for one prepared runtime activation.
//!
//! Control constructs and destroys the input. A native input producer appends
//! complete note events; the sole runtime consumer snapshots at most 128 events
//! and commits their token only after the corresponding runtime transaction
//! succeeds. The ring never overwrites an uncommitted event. Invalid payloads,
//! full capacity, counter exhaustion and concurrent producers fault the exact
//! activation out of band. Stale publishers and consumer commits cannot fault a
//! newer activation. No operation allocates, waits, schedules playback, or owns
//! project/audio identity.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

/// Capacity of one live note input queue.
pub const LIVE_NOTE_INPUT_CAPACITY: usize = 256;
/// Maximum number of ordered events copied for one runtime transaction.
pub const LIVE_NOTE_BATCH_CAPACITY: usize = 128;
const ACTIVATION_STATE_BITS: u64 = 2;
const ACTIVATION_STATE_MASK: u64 = (1 << ACTIVATION_STATE_BITS) - 1;
const ACTIVATION_PREPARING: u64 = 0;
const ACTIVATION_ACTIVE: u64 = 1;
const ACTIVATION_CANCELLED: u64 = 2;
const ACTIVATION_FAULTED: u64 = 3;
const WRITER_IDLE: u8 = 0;
const WRITER_PRODUCER: u8 = 1;
const WRITER_CONTROL: u8 = 2;

/// Scalar identity for one live input activation. It carries no ownership or
/// authority beyond matching events and acknowledgements to that activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveNoteActivation(u64);

impl LiveNoteActivation {
    /// Monotonic activation epoch, useful for correlating host input events.
    pub fn epoch(self) -> u64 {
        self.0 >> ACTIVATION_STATE_BITS
    }
}

/// One ordered live MIDI input event. Velocity is kept at the full MIDI value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveNoteEvent {
    /// A new note, with MIDI velocity in `1..=127`.
    NoteOn {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    /// A note release, preserving MIDI release velocity in `0..=127`.
    NoteOff {
        channel: u8,
        pitch: u8,
        velocity: u8,
    },
    /// Polyphonic pressure for the currently held note on one channel and key.
    /// A later NoteOn on the same key creates a new occurrence.
    PolyPressure { channel: u8, pitch: u8, value: u8 },
    /// Release every live note owned by this input.
    #[default]
    AllNotesOff,
}

impl LiveNoteEvent {
    fn encode(self) -> Option<u64> {
        let (kind, channel, pitch, velocity) = match self {
            Self::NoteOn {
                channel,
                pitch,
                velocity,
            } if channel <= 15 && pitch <= 127 && (1..=127).contains(&velocity) => {
                (1_u64, channel, pitch, velocity)
            }
            Self::NoteOff {
                channel,
                pitch,
                velocity,
            } if channel <= 15 && pitch <= 127 && velocity <= 127 => {
                (2_u64, channel, pitch, velocity)
            }
            Self::AllNotesOff => (3_u64, 0, 0, 0),
            Self::PolyPressure {
                channel,
                pitch,
                value,
            } if channel <= 15 && pitch <= 127 && value <= 127 => (4_u64, channel, pitch, value),
            _ => return None,
        };
        Some(
            (kind << 18)
                | (u64::from(channel) << 14)
                | (u64::from(pitch) << 7)
                | u64::from(velocity),
        )
    }

    fn decode(word: u64) -> Option<Self> {
        let kind = word >> 18;
        let channel = ((word >> 14) & 0x0f) as u8;
        let pitch = ((word >> 7) & 0x7f) as u8;
        let velocity = (word & 0x7f) as u8;
        match kind {
            1 if (1..=127).contains(&velocity) => Some(Self::NoteOn {
                channel,
                pitch,
                velocity,
            }),
            2 => Some(Self::NoteOff {
                channel,
                pitch,
                velocity,
            }),
            3 if word == (3_u64 << 18) => Some(Self::AllNotesOff),
            4 => Some(Self::PolyPressure {
                channel,
                pitch,
                value: velocity,
            }),
            _ => None,
        }
    }
}

/// Why a live event could not be appended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveNotePublishError {
    InvalidEvent,
    Cancelled,
    Retired,
    Busy,
    Full,
    ConcurrentProducer,
    IdentityExhausted,
    StaleCommit,
    StaleActivation,
}

/// One fixed-size consumer snapshot. Its token is valid only until the input
/// head advances; committing it acknowledges all included events atomically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveNoteBatch {
    pub(crate) events: [LiveNoteEvent; LIVE_NOTE_BATCH_CAPACITY],
    pub(crate) len: usize,
    pub(crate) activation: LiveNoteActivation,
    start: u64,
    end: u64,
}

impl LiveNoteBatch {
    pub(crate) fn events(&self) -> &[LiveNoteEvent] {
        &self.events[..self.len]
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Ordered, nonblocking note-event queue for one runtime activation.
///
/// A single producer is supported. Concurrent or recursive publishing is a
/// misuse and cancels the input immediately rather than waiting for the active
/// producer. The runtime is the sole consumer. `snapshot` is speculative and
/// may be repeated after a failed runtime transaction; only `commit` advances
/// the queue. Construct and drop this value on control, after callback readers
/// have quiesced.
#[derive(Debug)]
pub struct LiveNoteInput {
    slots: [AtomicU64; LIVE_NOTE_INPUT_CAPACITY],
    tail: AtomicU64,
    head: AtomicU64,
    writer: AtomicU8,
    activation: AtomicU64,
    retired: AtomicBool,
}

impl Default for LiveNoteInput {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
            tail: AtomicU64::new(0),
            head: AtomicU64::new(0),
            writer: AtomicU8::new(WRITER_IDLE),
            activation: AtomicU64::new((1 << ACTIVATION_STATE_BITS) | ACTIVATION_ACTIVE),
            retired: AtomicBool::new(false),
        }
    }
}

impl LiveNoteInput {
    fn state(raw: u64) -> u64 {
        raw & ACTIVATION_STATE_MASK
    }

    fn error_for(&self, token: LiveNoteActivation) -> LiveNotePublishError {
        if self.retired.load(Ordering::Acquire) {
            return LiveNotePublishError::Retired;
        }
        let raw = self.activation.load(Ordering::Acquire);
        if raw >> ACTIVATION_STATE_BITS != token.epoch() {
            LiveNotePublishError::StaleActivation
        } else {
            match Self::state(raw) {
                ACTIVATION_PREPARING => LiveNotePublishError::Busy,
                ACTIVATION_ACTIVE => LiveNotePublishError::StaleCommit,
                ACTIVATION_CANCELLED | ACTIVATION_FAULTED => LiveNotePublishError::Cancelled,
                _ => LiveNotePublishError::IdentityExhausted,
            }
        }
    }

    fn require_active(&self, token: LiveNoteActivation) -> Result<(), LiveNotePublishError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(LiveNotePublishError::Retired);
        }
        match self.activation.load(Ordering::Acquire) {
            raw if raw == token.0 && Self::state(raw) == ACTIVATION_ACTIVE => Ok(()),
            raw if raw >> ACTIVATION_STATE_BITS != token.epoch() => {
                Err(LiveNotePublishError::StaleActivation)
            }
            raw if Self::state(raw) == ACTIVATION_PREPARING => Err(LiveNotePublishError::Busy),
            _ => Err(LiveNotePublishError::Cancelled),
        }
    }

    fn fault_activation(
        &self,
        token: LiveNoteActivation,
        reason: LiveNotePublishError,
    ) -> LiveNotePublishError {
        if self.retired.load(Ordering::Acquire) {
            return LiveNotePublishError::Retired;
        }
        if self
            .activation
            .compare_exchange(
                token.0,
                (token.epoch() << ACTIVATION_STATE_BITS) | ACTIVATION_FAULTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            reason
        } else {
            self.error_for(token)
        }
    }

    /// Fail only the named activation, for a runtime-side bounded refusal.
    pub(crate) fn fail_activation(
        &self,
        activation: LiveNoteActivation,
    ) -> Result<(), LiveNotePublishError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(LiveNotePublishError::Retired);
        }
        match self.activation.compare_exchange(
            activation.0,
            (activation.epoch() << ACTIVATION_STATE_BITS) | ACTIVATION_FAULTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(_) => Err(self.error_for(activation)),
        }
    }

    /// Token for the current active activation, for scalar event correlation.
    pub fn current_activation_on_control(
        &self,
    ) -> Result<LiveNoteActivation, LiveNotePublishError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(LiveNotePublishError::Retired);
        }
        let raw = self.activation.load(Ordering::Acquire);
        if Self::state(raw) == ACTIVATION_ACTIVE {
            Ok(LiveNoteActivation(raw))
        } else {
            match Self::state(raw) {
                ACTIVATION_PREPARING => Err(LiveNotePublishError::Busy),
                ACTIVATION_CANCELLED | ACTIVATION_FAULTED => Err(LiveNotePublishError::Cancelled),
                _ => Err(LiveNotePublishError::IdentityExhausted),
            }
        }
    }

    /// Start a fresh input activation and discard any unconsumed events from
    /// the previous one. The writer guard is tried once; contention is retryable
    /// and does not cancel either activation.
    pub fn begin_activation_on_control(&self) -> Result<LiveNoteActivation, LiveNotePublishError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(LiveNotePublishError::Retired);
        }
        if self
            .writer
            .compare_exchange(
                WRITER_IDLE,
                WRITER_CONTROL,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            return Err(LiveNotePublishError::Busy);
        }
        let result = self.begin_activation_locked();
        self.writer.store(WRITER_IDLE, Ordering::Release);
        result
    }

    fn begin_activation_locked(&self) -> Result<LiveNoteActivation, LiveNotePublishError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(LiveNotePublishError::Retired);
        }
        let previous = self.activation.load(Ordering::Acquire);
        let previous_epoch = previous >> ACTIVATION_STATE_BITS;
        let Some(epoch) = previous_epoch
            .checked_add(1)
            .filter(|epoch| *epoch <= (u64::MAX >> ACTIVATION_STATE_BITS))
        else {
            let token =
                LiveNoteActivation((previous_epoch << ACTIVATION_STATE_BITS) | ACTIVATION_ACTIVE);
            return Err(self.fault_activation(token, LiveNotePublishError::IdentityExhausted));
        };
        let preparing = epoch << ACTIVATION_STATE_BITS;
        if self
            .activation
            .compare_exchange(previous, preparing, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(if self.retired.load(Ordering::Acquire) {
                LiveNotePublishError::Retired
            } else {
                LiveNotePublishError::Busy
            });
        }
        let tail = self.tail.load(Ordering::Acquire);
        self.head.store(tail, Ordering::Release);
        if self.retired.load(Ordering::Acquire) {
            let _ = self.activation.compare_exchange(
                preparing,
                preparing | ACTIVATION_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            return Err(LiveNotePublishError::Retired);
        }
        let active = preparing | ACTIVATION_ACTIVE;
        if self
            .activation
            .compare_exchange(preparing, active, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(self.error_for(LiveNoteActivation(active)));
        }
        if self.retired.load(Ordering::Acquire) {
            let _ = self.activation.compare_exchange(
                active,
                preparing | ACTIVATION_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            return Err(LiveNotePublishError::Retired);
        }
        Ok(LiveNoteActivation(active))
    }

    /// Append one event only for the activation that observed it. Stale native
    /// callbacks cannot fault or write into a reconnected activation.
    pub fn publish(
        &self,
        activation: LiveNoteActivation,
        event: LiveNoteEvent,
    ) -> Result<(), LiveNotePublishError> {
        self.require_active(activation)?;
        let Some(word) = event.encode() else {
            return Err(self.fault_activation(activation, LiveNotePublishError::InvalidEvent));
        };
        if self
            .writer
            .compare_exchange(
                WRITER_IDLE,
                WRITER_PRODUCER,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_err()
        {
            if self.writer.load(Ordering::Acquire) == WRITER_CONTROL {
                return Err(LiveNotePublishError::Busy);
            }
            return Err(self.fault_activation(activation, LiveNotePublishError::ConcurrentProducer));
        }
        let result = self.publish_locked(activation, word);
        self.writer.store(WRITER_IDLE, Ordering::Release);
        result
    }

    fn publish_locked(
        &self,
        activation: LiveNoteActivation,
        word: u64,
    ) -> Result<(), LiveNotePublishError> {
        self.require_active(activation)?;
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        let Some(used) = tail.checked_sub(head) else {
            return Err(self.fault_activation(activation, LiveNotePublishError::IdentityExhausted));
        };
        if used > LIVE_NOTE_INPUT_CAPACITY as u64 {
            return Err(self.fault_activation(activation, LiveNotePublishError::IdentityExhausted));
        }
        if used == LIVE_NOTE_INPUT_CAPACITY as u64 {
            return Err(self.fault_activation(activation, LiveNotePublishError::Full));
        }
        let Some(next) = tail.checked_add(1) else {
            return Err(self.fault_activation(activation, LiveNotePublishError::IdentityExhausted));
        };
        let index = (tail % LIVE_NOTE_INPUT_CAPACITY as u64) as usize;
        // A speculative old reader may overlap slot reuse after rearm. Reading
        // this replacement with Acquire must also observe the new activation;
        // the tail release alone cannot fence that old reader's earlier tail.
        self.slots[index].store(word, Ordering::Release);
        self.require_active(activation)?;
        self.tail.store(next, Ordering::Release);
        Ok(())
    }

    /// Cancel only the matching activation. A late cancellation cannot affect
    /// a newer connection on the same input root.
    pub fn cancel_activation(
        &self,
        activation: LiveNoteActivation,
    ) -> Result<(), LiveNotePublishError> {
        if self.retired.load(Ordering::Acquire) {
            return Err(LiveNotePublishError::Retired);
        }
        match self.activation.compare_exchange(
            activation.0,
            (activation.epoch() << ACTIVATION_STATE_BITS) | ACTIVATION_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(_) => Err(self.error_for(activation)),
        }
    }

    /// Retire the entire root permanently. Reconnection requires a new root.
    pub fn cancel(&self) {
        self.retired.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.retired.load(Ordering::Acquire)
            || Self::state(self.activation.load(Ordering::Acquire)) != ACTIVATION_ACTIVE
    }

    pub fn rejected(&self) -> bool {
        Self::state(self.activation.load(Ordering::Acquire)) == ACTIVATION_FAULTED
    }

    pub(crate) fn activation_rejected(&self, activation: LiveNoteActivation) -> bool {
        self.activation.load(Ordering::Acquire)
            == ((activation.epoch() << ACTIVATION_STATE_BITS) | ACTIVATION_FAULTED)
    }

    pub fn same_current_activation(&self, activation: LiveNoteActivation) -> bool {
        !self.retired.load(Ordering::Acquire)
            && self.activation.load(Ordering::Acquire) == activation.0
    }

    /// Copy at most 128 committed events without advancing the queue head.
    /// A final activation check rejects mixed snapshots across reconnects.
    pub(crate) fn snapshot(&self) -> Result<LiveNoteBatch, LiveNotePublishError> {
        let raw = self.activation.load(Ordering::Acquire);
        let activation = LiveNoteActivation(raw);
        self.require_active(activation)?;
        let start = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        self.require_active(activation)?;
        let Some(available) = tail.checked_sub(start) else {
            return Err(self.fault_activation(activation, LiveNotePublishError::IdentityExhausted));
        };
        if available > LIVE_NOTE_INPUT_CAPACITY as u64 {
            return Err(self.fault_activation(activation, LiveNotePublishError::IdentityExhausted));
        }
        let len = match usize::try_from(available.min(LIVE_NOTE_BATCH_CAPACITY as u64)) {
            Ok(len) => len,
            Err(_) => {
                return Err(
                    self.fault_activation(activation, LiveNotePublishError::IdentityExhausted)
                );
            }
        };
        let Some(end) = start.checked_add(len as u64) else {
            return Err(self.fault_activation(activation, LiveNotePublishError::IdentityExhausted));
        };
        let mut batch = LiveNoteBatch {
            events: [LiveNoteEvent::AllNotesOff; LIVE_NOTE_BATCH_CAPACITY],
            len,
            activation,
            start,
            end,
        };
        for offset in 0..len {
            let index = ((start + offset as u64) % LIVE_NOTE_INPUT_CAPACITY as u64) as usize;
            let word = self.slots[index].load(Ordering::Acquire);
            self.require_active(activation)?;
            let Some(event) = LiveNoteEvent::decode(word) else {
                return Err(
                    self.fault_activation(activation, LiveNotePublishError::IdentityExhausted)
                );
            };
            batch.events[offset] = event;
        }
        self.require_active(activation)?;
        Ok(batch)
    }

    /// Acknowledge a prefix of one activation's snapshot. Old acknowledgements
    /// never fault or move the head of a newer activation.
    pub(crate) fn commit(
        &self,
        batch: &LiveNoteBatch,
        consumed: usize,
    ) -> Result<(), LiveNotePublishError> {
        self.require_active(batch.activation)?;
        if consumed > batch.len || batch.len > LIVE_NOTE_BATCH_CAPACITY {
            return Err(self.fault_activation(batch.activation, LiveNotePublishError::StaleCommit));
        }
        if consumed == 0 {
            return Ok(());
        }
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        self.require_active(batch.activation)?;
        if head != batch.start
            || batch.end < batch.start
            || batch.end > tail
            || batch.end - batch.start != batch.len as u64
        {
            return Err(self.fault_activation(batch.activation, LiveNotePublishError::StaleCommit));
        }
        let Some(next_head) = batch.start.checked_add(consumed as u64) else {
            return Err(self.fault_activation(batch.activation, LiveNotePublishError::StaleCommit));
        };
        if self
            .head
            .compare_exchange(batch.start, next_head, Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            self.require_active(batch.activation)?;
            return Err(self.fault_activation(batch.activation, LiveNotePublishError::StaleCommit));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(pitch: u8, velocity: u8) -> LiveNoteEvent {
        LiveNoteEvent::NoteOn {
            channel: 2,
            pitch,
            velocity,
        }
    }

    fn push(input: &LiveNoteInput, event: LiveNoteEvent) -> Result<(), LiveNotePublishError> {
        let activation = input.current_activation_on_control()?;
        input.publish(activation, event)
    }

    #[test]
    fn preserves_exact_velocity_order_and_short_note_pairs() {
        let input = LiveNoteInput::default();
        push(&input, on(60, 37)).unwrap();
        push(
            &input,
            LiveNoteEvent::NoteOff {
                channel: 2,
                pitch: 60,
                velocity: 9,
            },
        )
        .unwrap();
        push(&input, on(64, 121)).unwrap();

        let batch = input.snapshot().unwrap();
        assert_eq!(
            batch.events(),
            &[
                on(60, 37),
                LiveNoteEvent::NoteOff {
                    channel: 2,
                    pitch: 60,
                    velocity: 9
                },
                on(64, 121)
            ]
        );
    }

    #[test]
    fn packed_event_round_trips_all_boundary_fields_without_overlap() {
        let input = LiveNoteInput::default();
        let mut expected = Vec::new();
        for channel in [0, 15] {
            for pitch in [0, 127] {
                for velocity in [1, 127] {
                    let event = LiveNoteEvent::NoteOn {
                        channel,
                        pitch,
                        velocity,
                    };
                    assert_eq!(LiveNoteEvent::decode(event.encode().unwrap()), Some(event));
                    push(&input, event).unwrap();
                    expected.push(event);
                }
                for velocity in [0, 127] {
                    let event = LiveNoteEvent::NoteOff {
                        channel,
                        pitch,
                        velocity,
                    };
                    assert_eq!(LiveNoteEvent::decode(event.encode().unwrap()), Some(event));
                    push(&input, event).unwrap();
                    expected.push(event);
                }
            }
        }
        let batch = input.snapshot().unwrap();
        assert_eq!(batch.events(), expected.as_slice());
    }

    #[test]
    fn snapshot_can_be_retried_without_consuming_then_commit_turns_over_slots() {
        let input = LiveNoteInput::default();
        for pitch in 0..LIVE_NOTE_BATCH_CAPACITY as u8 {
            push(&input, on(pitch, 100)).unwrap();
        }
        let first = input.snapshot().unwrap();
        let retry = input.snapshot().unwrap();
        assert_eq!(first.events(), retry.events());
        assert_eq!(first.start, retry.start);
        assert_eq!(first.end, retry.end);
        input.commit(&first, first.len).unwrap();
        for pitch in 0..LIVE_NOTE_BATCH_CAPACITY as u8 {
            push(&input, on(pitch, 90)).unwrap();
        }
        let next = input.snapshot().unwrap();
        assert_eq!(next.len, LIVE_NOTE_BATCH_CAPACITY);
        assert_eq!(next.events()[0], on(0, 90));
    }

    #[test]
    fn partial_commit_leaves_the_exact_suffix_for_the_next_snapshot() {
        let input = LiveNoteInput::default();
        push(&input, on(60, 100)).unwrap();
        push(&input, on(61, 101)).unwrap();
        push(&input, on(62, 102)).unwrap();

        let first = input.snapshot().unwrap();
        input.commit(&first, 1).unwrap();
        let suffix = input.snapshot().unwrap();
        assert_eq!(suffix.events(), &[on(61, 101), on(62, 102)]);
        input.commit(&suffix, 0).unwrap();
        assert_eq!(input.snapshot().unwrap().events(), suffix.events());
        input.commit(&suffix, suffix.len).unwrap();
        assert!(input.snapshot().unwrap().is_empty());
    }

    #[test]
    fn full_queue_cancels_even_when_the_rejected_event_is_a_note_off() {
        let input = LiveNoteInput::default();
        let activation = input.current_activation_on_control().unwrap();
        for pitch in 0..LIVE_NOTE_INPUT_CAPACITY {
            input
                .publish(activation, on((pitch % 128) as u8, 100))
                .unwrap();
        }
        assert_eq!(
            input.publish(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 0,
                    pitch: 60,
                    velocity: 0
                }
            ),
            Err(LiveNotePublishError::Full)
        );
        assert!(input.rejected());
        assert!(input.is_cancelled());
        assert_eq!(input.snapshot(), Err(LiveNotePublishError::Cancelled));
    }

    #[test]
    fn invalid_payload_cancels_and_rejects_late_note_offs() {
        let input = LiveNoteInput::default();
        let activation = input.current_activation_on_control().unwrap();
        assert_eq!(
            input.publish(activation, on(60, 0)),
            Err(LiveNotePublishError::InvalidEvent)
        );
        assert!(input.rejected());
        assert!(input.is_cancelled());
        assert_eq!(
            input.publish(
                activation,
                LiveNoteEvent::NoteOff {
                    channel: 2,
                    pitch: 60,
                    velocity: 0,
                }
            ),
            Err(LiveNotePublishError::Cancelled)
        );
    }

    #[test]
    fn overlapping_producer_fails_closed_without_waiting() {
        let input = LiveNoteInput::default();
        let activation = input.current_activation_on_control().unwrap();
        input.writer.store(WRITER_PRODUCER, Ordering::Relaxed);
        assert_eq!(
            input.publish(activation, on(60, 100)),
            Err(LiveNotePublishError::ConcurrentProducer)
        );
        assert!(input.is_cancelled());
        assert!(input.rejected());
    }

    #[test]
    fn stale_duplicate_commit_cancels_instead_of_skipping_events() {
        let input = LiveNoteInput::default();
        push(&input, on(60, 100)).unwrap();
        let first = input.snapshot().unwrap();
        input.commit(&first, 1).unwrap();
        assert_eq!(
            input.commit(&first, 1),
            Err(LiveNotePublishError::StaleCommit)
        );
        assert!(input.is_cancelled());
    }

    #[test]
    fn all_notes_off_is_ordered_as_a_distinct_event() {
        let input = LiveNoteInput::default();
        push(&input, on(60, 100)).unwrap();
        push(&input, LiveNoteEvent::AllNotesOff).unwrap();
        let batch = input.snapshot().unwrap();
        assert_eq!(batch.events(), &[on(60, 100), LiveNoteEvent::AllNotesOff]);
    }

    #[test]
    fn reactivation_discards_old_prefix_and_stale_calls_cannot_affect_new_epoch() {
        let input = LiveNoteInput::default();
        let old = input.current_activation_on_control().unwrap();
        input.publish(old, on(60, 100)).unwrap();
        let old_batch = input.snapshot().unwrap();

        let fresh = input.begin_activation_on_control().unwrap();
        assert_eq!(fresh.epoch(), old.epoch() + 1);
        assert_eq!(
            input.publish(
                old,
                LiveNoteEvent::NoteOff {
                    channel: 2,
                    pitch: 60,
                    velocity: 0,
                }
            ),
            Err(LiveNotePublishError::StaleActivation)
        );
        assert_eq!(
            input.publish(old, on(61, 0)),
            Err(LiveNotePublishError::StaleActivation)
        );
        assert_eq!(
            input.cancel_activation(old),
            Err(LiveNotePublishError::StaleActivation)
        );
        assert_eq!(
            input.fail_activation(old),
            Err(LiveNotePublishError::StaleActivation)
        );
        assert_eq!(
            input.commit(&old_batch, 1),
            Err(LiveNotePublishError::StaleActivation)
        );
        assert!(!input.rejected());
        assert!(!input.is_cancelled());
        assert!(input.same_current_activation(fresh));

        input.publish(fresh, on(64, 127)).unwrap();
        assert_eq!(input.snapshot().unwrap().events(), &[on(64, 127)]);
    }

    #[test]
    fn busy_reactivation_is_retryable_and_does_not_cancel_current_activation() {
        let input = LiveNoteInput::default();
        let current = input.current_activation_on_control().unwrap();
        input.writer.store(WRITER_CONTROL, Ordering::Relaxed);
        assert_eq!(
            input.begin_activation_on_control(),
            Err(LiveNotePublishError::Busy)
        );
        assert!(input.same_current_activation(current));
        assert!(!input.is_cancelled());
        input.writer.store(WRITER_IDLE, Ordering::Relaxed);
        let fresh = input.begin_activation_on_control().unwrap();
        assert_eq!(fresh.epoch(), current.epoch() + 1);
    }

    #[test]
    fn whole_root_cancellation_is_terminal_across_activation_changes() {
        let input = LiveNoteInput::default();
        let current = input.current_activation_on_control().unwrap();
        input.cancel_activation(current).unwrap();
        input.begin_activation_on_control().unwrap();
        input.cancel();
        assert_eq!(
            input.begin_activation_on_control(),
            Err(LiveNotePublishError::Retired)
        );
        assert_eq!(
            input.current_activation_on_control(),
            Err(LiveNotePublishError::Retired)
        );
        assert!(input.is_cancelled());
    }
}
