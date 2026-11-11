//! Control-producer ordered physical keys. Desired words remain the recovery/monitor
//! owner. A pending activation suppresses new snapshot attacks until callback Arm;
//! sealing is out of band and drains only the finite admitted prefix.
use super::*;
use std::sync::atomic::Ordering::{AcqRel, SeqCst};

pub(super) const CAPACITY: usize = 256;
pub(super) const BATCH: usize = 128;
const BITS: u32 = 3;
const IDLE: u64 = 0;
const PENDING: u64 = 1;
const ACTIVE: u64 = 2;
const SEALED: u64 = 3;
const FAULTED: u64 = 4;

/// Scalar correlation for one ordered physical-key activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiKeyActivation(u64);
impl MidiKeyActivation {
    pub(crate) const fn placeholder() -> Self {
        Self(0)
    }
    pub fn epoch(self) -> u64 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyEvent {
    pub key: usize,
    pub word: u64,
    pub on: bool,
}
impl KeyEvent {
    fn encode(self) -> Option<u64> {
        if self.key >= COMPUTER_KEY_COUNT || !valid_key_word(self.word) || self.word == 0 {
            return None;
        }
        let token = self.word >> 16;
        let payload = if self.on {
            let velocity = ((self.word >> 8) & 255) as u8;
            let index = COMPUTER_KEY_VELOCITIES
                .iter()
                .position(|v| *v == velocity)?;
            (1 << 63) | ((index as u64) << 12) | (((self.word & 255) - 1) << 5)
        } else {
            0
        };
        Some(payload | (token << 15) | self.key as u64)
    }
    fn decode(raw: u64) -> Option<Self> {
        let key = (raw & 31) as usize;
        let token = (raw >> 15) & ((1_u64 << 48) - 1);
        if key >= COMPUTER_KEY_COUNT || token == 0 {
            return None;
        }
        let on = raw >> 63 != 0;
        let word = if on {
            let velocity = *COMPUTER_KEY_VELOCITIES.get(((raw >> 12) & 7) as usize)?;
            (token << 16) | (u64::from(velocity) << 8) | (((raw >> 5) & 127) + 1)
        } else {
            if raw & 0x7fe0 != 0 {
                return None;
            }
            // Off compares the full press token; pitch/velocity are carried by the voice.
            token << 16
        };
        Some(Self { key, word, on })
    }
}
#[derive(Clone, Copy)]
pub(crate) struct KeyBatch {
    pub activation: MidiKeyActivation,
    pub output_epoch: u64,
    pub events: [Option<KeyEvent>; BATCH],
    pub len: usize,
    pub start: u64,
    pub sealed: bool,
    pub end: u64,
}
#[derive(Debug)]
pub(crate) struct OrderedKeys {
    slots: [AtomicU64; CAPACITY],
    head: AtomicU64,
    tail: AtomicU64,
    state: AtomicU64,
    sealed_tail: AtomicU64,
    output_epoch: AtomicU64,
    baseline: [AtomicU64; COMPUTER_KEY_COUNT],
}
impl Default for OrderedKeys {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| AtomicU64::new(0)),
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
            state: AtomicU64::new(IDLE),
            sealed_tail: AtomicU64::new(0),
            output_epoch: AtomicU64::new(0),
            baseline: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}
impl OrderedKeys {
    fn token(raw: u64) -> MidiKeyActivation {
        MidiKeyActivation(raw >> BITS)
    }
    fn phase(raw: u64) -> u64 {
        raw & 7
    }
    fn exact(&self, token: MidiKeyActivation, phase: u64) -> bool {
        self.state.load(Acquire) == ((token.0 << BITS) | phase)
    }
    pub(super) fn begin(
        &self,
        words: [u64; COMPUTER_KEY_COUNT],
        output_epoch: u64,
    ) -> Result<MidiKeyActivation, Invalid> {
        let raw = self.state.load(Acquire);
        if matches!(Self::phase(raw), PENDING | ACTIVE | SEALED) {
            return Err(Invalid("ordered keys are already active"));
        }
        let epoch = Self::token(raw)
            .0
            .checked_add(1)
            .filter(|epoch| *epoch <= u64::MAX >> BITS)
            .ok_or(Invalid("ordered key identity exhausted"))?;
        let tail = self.tail.load(Acquire);
        self.head.store(tail, Release);
        for (target, word) in self.baseline.iter().zip(words) {
            target.store(word, Release);
        }
        self.output_epoch.store(output_epoch, Release);
        self.state.store((epoch << BITS) | PENDING, Release);
        Ok(MidiKeyActivation(epoch))
    }
    pub(super) fn publish_change(&self, key: usize, before: u64, after: u64) {
        let raw = self.state.load(Acquire);
        if !matches!(Self::phase(raw), PENDING | ACTIVE) || before == after {
            return;
        }
        for event in [
            (before != 0).then_some(KeyEvent {
                key,
                word: before,
                on: false,
            }),
            (after != 0).then_some(KeyEvent {
                key,
                word: after,
                on: true,
            }),
        ]
        .into_iter()
        .flatten()
        {
            let Some(encoded) = event.encode() else {
                self.fault(Self::token(raw));
                return;
            };
            let tail = self.tail.load(Acquire);
            let Some(next) = tail.checked_add(1) else {
                self.fault(Self::token(raw));
                return;
            };
            if tail.saturating_sub(self.head.load(Acquire)) >= CAPACITY as u64 {
                self.fault(Self::token(raw));
                return;
            }
            self.slots[tail as usize % CAPACITY].store(encoded, Release);
            self.tail.store(next, Release);
        }
    }
    pub(super) fn accept(
        &self,
        token: MidiKeyActivation,
        output_epoch: u64,
    ) -> Result<(), Invalid> {
        if self.output_epoch.load(Acquire) != output_epoch {
            return Err(Invalid("ordered key output activation is stale"));
        }
        self.state
            .compare_exchange(
                (token.0 << BITS) | PENDING,
                (token.0 << BITS) | ACTIVE,
                AcqRel,
                Acquire,
            )
            .map(|_| ())
            .map_err(|_| Invalid("ordered key activation is stale or faulted"))
    }
    pub(super) fn seal(&self, token: MidiKeyActivation) -> Result<(), Invalid> {
        let raw = self.state.load(Acquire);
        if Self::token(raw) != token {
            return Err(Invalid("ordered key activation is stale"));
        }
        if Self::phase(raw) == FAULTED {
            return Ok(());
        }
        if Self::phase(raw) == PENDING {
            self.head.store(self.tail.load(Acquire), Release);
            self.state.store(token.0 << BITS, Release);
            return Ok(());
        }
        if !matches!(Self::phase(raw), PENDING | ACTIVE) {
            return Err(Invalid("ordered keys are not active"));
        }
        self.sealed_tail.store(self.tail.load(Acquire), Release);
        self.state
            .compare_exchange(raw, (token.0 << BITS) | SEALED, AcqRel, Acquire)
            .map(|_| ())
            .map_err(|_| Invalid("ordered key seal raced activation"))
    }
    pub(super) fn fault(&self, token: MidiKeyActivation) {
        let raw = self.state.load(Acquire);
        if Self::token(raw) == token {
            let _ = self
                .state
                .compare_exchange(raw, (token.0 << BITS) | FAULTED, AcqRel, Acquire);
        }
    }
    pub(super) fn faulted(&self, token: MidiKeyActivation) -> bool {
        self.exact(token, FAULTED)
    }
    pub(crate) fn active(&self, token: MidiKeyActivation) -> bool {
        self.exact(token, ACTIVE)
    }
    pub(super) fn has_recording_prefix(&self) -> bool {
        matches!(
            Self::phase(self.state.load(Acquire)),
            PENDING | ACTIVE | SEALED
        )
    }

    pub(super) fn baseline(&self) -> Option<[u64; COMPUTER_KEY_COUNT]> {
        let raw = self.state.load(Acquire);
        if Self::phase(raw) != PENDING {
            return None;
        }
        let words = std::array::from_fn(|index| self.baseline[index].load(Acquire));
        (self.state.load(Acquire) == raw).then_some(words)
    }
    pub(super) fn snapshot(&self) -> Option<KeyBatch> {
        let raw = self.state.load(Acquire);
        if !matches!(Self::phase(raw), ACTIVE | SEALED) {
            return None;
        }
        let start = self.head.load(Acquire);
        let end = if Self::phase(raw) == SEALED {
            self.sealed_tail.load(Acquire)
        } else {
            self.tail.load(Acquire)
        };
        let available = end.checked_sub(start)?;
        if available > CAPACITY as u64 {
            self.fault(Self::token(raw));
            return None;
        }
        let len = (available as usize).min(BATCH);
        let mut events = [None; BATCH];
        for (index, event) in events.iter_mut().enumerate().take(len) {
            *event =
                KeyEvent::decode(self.slots[(start as usize + index) % CAPACITY].load(Acquire));
            if event.is_none() {
                self.fault(Self::token(raw));
                return None;
            }
        }
        (self.state.load(Acquire) == raw).then_some(KeyBatch {
            activation: Self::token(raw),
            output_epoch: self.output_epoch.load(Acquire),
            events,
            len,
            start,
            sealed: Self::phase(raw) == SEALED,
            end,
        })
    }
    pub(super) fn commit(&self, batch: &KeyBatch, consumed: usize) {
        if consumed > batch.len {
            return;
        }
        let raw = self.state.load(Acquire);
        if Self::token(raw) != batch.activation || !matches!(Self::phase(raw), ACTIVE | SEALED) {
            return;
        }
        let next = batch.start + consumed as u64;
        if self
            .head
            .compare_exchange(batch.start, next, AcqRel, Acquire)
            .is_ok()
            && batch.sealed
            && next == batch.end
        {
            let _ = self.state.compare_exchange(
                (batch.activation.0 << BITS) | SEALED,
                batch.activation.0 << BITS,
                AcqRel,
                Acquire,
            );
        }
    }
}
impl ComputerKeyboardInput {
    /// Control-only opt-in before preparation; ordinary monitoring roots allocate neither
    /// an ordered key ring nor accepted-event storage.
    pub fn with_recording_support(mut self) -> Result<Self, Invalid> {
        if self.native_activation.is_some()
            || self.audition.is_some()
            || self.ordered_keys.is_some()
            || self.sequence.load(Acquire) != 0
            || self.is_cancelled()
        {
            return Err(Invalid("recording requires a fresh ordinary input"));
        }
        self.ordered_keys = Some(Box::new(OrderedKeys::default()));
        Ok(self)
    }
    /// Begin routing changed physical words before sending callback Arm. Existing held
    /// words remain monitor-only; short new presses survive the acknowledgement delay.
    pub fn begin_ordered_keys_on_control(&self) -> Result<MidiKeyActivation, Invalid> {
        if self.is_cancelled() {
            return Err(Invalid("computer keyboard input is cancelled"));
        }
        let after = self.begin_write()?;
        let result = self
            .ordered_keys
            .as_ref()
            .ok_or(Invalid("recording input is unavailable"))
            .and_then(|queue| {
                queue.begin(
                    std::array::from_fn(|index| self.notes[index].load(SeqCst)),
                    self.output_epoch_on_control().unwrap_or(0),
                )
            });
        if result.is_ok() {
            self.cancel_live_note_audition_on_control();
        }
        self.sequence.store(after, SeqCst);
        result
    }
    /// Stop adding events, even when the ring is full. The callback drains the finite
    /// sealed prefix and then resumes desired-word reconciliation without retriggering.
    pub fn seal_ordered_keys_on_control(&self, token: MidiKeyActivation) -> Result<(), Invalid> {
        self.ordered_keys
            .as_ref()
            .ok_or(Invalid("recording input is unavailable"))?
            .seal(token)
    }
    pub fn ordered_keys_faulted(&self, token: MidiKeyActivation) -> bool {
        self.is_cancelled()
            || self
                .ordered_keys
                .as_ref()
                .is_none_or(|queue| queue.faulted(token))
    }
    /// Negative-only control observation for choosing Finish versus Cancel.
    /// Call before the caller revokes input/output for Stop. This grants no
    /// graph admission or delivery proof; changes after this observation race
    /// the caller's finish decision. Pending Arm remains a valid empty take.
    pub fn recording_input_current_on_control(
        &self,
        keys: MidiKeyActivation,
        output_epoch: u64,
        hardware: Option<LiveNoteActivation>,
    ) -> bool {
        !self.is_cancelled()
            && self.output_activation.as_ref().is_some_and(|activation| {
                activation.epoch.load(Acquire) == output_epoch
                    && activation.clock.permits(output_epoch)
            })
            && self.ordered_keys.as_ref().is_some_and(|queue| {
                (queue.exact(keys, PENDING) || queue.active(keys))
                    && queue.output_epoch.load(Acquire) == output_epoch
            })
            && match hardware {
                Some(token) => {
                    self.live_notes
                        .as_ref()
                        .is_some_and(|live| live.same_current_activation(token))
                        && self.live_output_epoch.load(Acquire) == output_epoch
                }
                None => self
                    .live_notes
                    .as_ref()
                    .is_none_or(|live| live.is_cancelled()),
            }
    }
    pub fn output_epoch_on_control(&self) -> Option<u64> {
        self.output_activation
            .as_ref()
            .map(|activation| activation.epoch.load(Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finish_precheck_distinguishes_pending_active_faulted_replaced_and_revoked_inputs() {
        let clock = Arc::new(KeyboardActivationClock::default());
        let input = ComputerKeyboardInput::with_output_activation(Arc::clone(&clock))
            .unwrap()
            .with_live_notes()
            .unwrap()
            .with_recording_support()
            .unwrap();
        let hardware = input
            .live_notes()
            .unwrap()
            .current_activation_on_control()
            .unwrap();
        let keys = input.begin_ordered_keys_on_control().unwrap();
        let output = input.output_epoch_on_control().unwrap();
        assert!(input.recording_input_current_on_control(keys, output, Some(hardware)));
        assert!(!input.recording_input_current_on_control(keys, output, None));
        input
            .ordered_keys
            .as_ref()
            .unwrap()
            .accept(keys, output)
            .unwrap();
        assert!(input.recording_input_current_on_control(keys, output, Some(hardware)));
        input
            .live_notes()
            .unwrap()
            .cancel_activation(hardware)
            .unwrap();
        assert!(!input.recording_input_current_on_control(keys, output, Some(hardware)));
        let next_hardware = input.begin_live_notes_activation_on_control().unwrap();
        assert!(input.recording_input_current_on_control(keys, output, Some(next_hardware)));
        for token in 1..=129 {
            input.set_key(0, Some((60, 100, token))).unwrap();
            input.set_key(0, None).unwrap();
        }
        assert!(!input.recording_input_current_on_control(keys, output, Some(next_hardware)));
        input.seal_ordered_keys_on_control(keys).unwrap();
        let next_keys = input.begin_ordered_keys_on_control().unwrap();
        assert!(!input.recording_input_current_on_control(keys, output, Some(next_hardware)));
        assert!(input.recording_input_current_on_control(next_keys, output, Some(next_hardware)));
        clock.revoke_reserved_prefix();
        assert!(!input.recording_input_current_on_control(next_keys, output, Some(next_hardware)));
    }
    fn word(pitch: u8, velocity: u8, token: u64) -> u64 {
        (token << 16) | (u64::from(velocity) << 8) | u64::from(pitch + 1)
    }
    #[test]
    fn exact_word_encoding_and_slot_publication_preserve_short_presses() {
        for key in [0, COMPUTER_KEY_COUNT - 1] {
            for token in [1, u64::MAX >> 16] {
                for velocity in COMPUTER_KEY_VELOCITIES {
                    let event = KeyEvent {
                        key,
                        word: word(127, velocity, token),
                        on: true,
                    };
                    assert_eq!(KeyEvent::decode(event.encode().unwrap()), Some(event));
                    let off = KeyEvent { on: false, ..event };
                    assert_eq!(
                        KeyEvent::decode(off.encode().unwrap()).unwrap().word >> 16,
                        token
                    );
                }
            }
        }
        let input = ComputerKeyboardInput::default()
            .with_recording_support()
            .unwrap();
        let token = input.begin_ordered_keys_on_control().unwrap();
        for _ in 0..17 {
            input.set_key(0, Some((60, 100, 7))).unwrap();
        }
        input.set_key(0, None).unwrap();
        let queue = input.ordered_keys.as_ref().unwrap();
        assert!(
            queue.snapshot().is_none(),
            "pending keys cannot be consumed before Arm"
        );
        queue
            .accept(token, queue.output_epoch.load(Acquire))
            .unwrap();
        let batch = queue.snapshot().unwrap();
        assert_eq!(batch.len, 2, "rewritten unchanged slots do not retrigger");
        assert!(batch.events[0].unwrap().on);
        assert!(!batch.events[1].unwrap().on);
        assert_eq!(
            batch.events[0].unwrap().word >> 16,
            batch.events[1].unwrap().word >> 16
        );
    }
    #[test]
    fn full_queue_is_scoped_and_return_is_out_of_band() {
        let input = ComputerKeyboardInput::default()
            .with_live_notes()
            .unwrap()
            .with_recording_support()
            .unwrap();
        let token = input.begin_ordered_keys_on_control().unwrap();
        input
            .ordered_keys
            .as_ref()
            .unwrap()
            .accept(token, 0)
            .unwrap();
        for token in 1..=129 {
            input.set_key(0, Some((60, 100, token))).unwrap();
            input.set_key(0, None).unwrap();
        }
        assert!(input.ordered_keys_faulted(token));
        input.seal_ordered_keys_on_control(token).unwrap();
        assert!(!input.is_cancelled());
        assert!(!input.live_notes().unwrap().is_cancelled());
        let next = input.begin_ordered_keys_on_control().unwrap();
        assert_ne!(next, token);
        assert!(input.seal_ordered_keys_on_control(token).is_err());
        input.seal_ordered_keys_on_control(next).unwrap();
    }
    #[test]
    fn a_full_unfaulted_queue_can_be_sealed_and_drained_without_a_control_slot() {
        let queue = OrderedKeys::default();
        let token = queue.begin([0; COMPUTER_KEY_COUNT], 1).unwrap();
        queue
            .accept(token, queue.output_epoch.load(Acquire))
            .unwrap();
        for token in 1..=128 {
            let word = word(60, 100, token);
            queue.publish_change(0, 0, word);
            queue.publish_change(0, word, 0);
        }
        queue.seal(token).unwrap();
        assert!(!queue.faulted(token));
        for _ in 0..2 {
            let batch = queue.snapshot().unwrap();
            assert_eq!(batch.len, BATCH);
            queue.commit(&batch, BATCH);
        }
        assert!(queue.snapshot().is_none());
        assert!(queue.begin([0; COMPUTER_KEY_COUNT], 1).is_ok());
    }
    #[test]
    fn sealing_drains_only_its_finite_prefix_and_stale_commit_cannot_consume_new_activation() {
        let queue = OrderedKeys::default();
        let token = queue.begin([0; COMPUTER_KEY_COUNT], 3).unwrap();
        queue
            .accept(token, queue.output_epoch.load(Acquire))
            .unwrap();
        queue.publish_change(0, 0, word(60, 100, 1));
        let retry = queue.snapshot().unwrap();
        assert_eq!(queue.snapshot().unwrap().len, retry.len);
        queue.seal(token).unwrap();
        queue.publish_change(1, 0, word(64, 100, 2));
        let sealed = queue.snapshot().unwrap();
        assert_eq!(sealed.len, 1);
        queue.commit(&sealed, 1);
        assert!(queue.snapshot().is_none());
        let next = queue.begin([0; COMPUTER_KEY_COUNT], 4).unwrap();
        queue.accept(next, 4).unwrap();
        queue.publish_change(2, 0, word(67, 100, 3));
        queue.commit(&retry, 1);
        assert_eq!(queue.snapshot().unwrap().len, 1);
        assert_eq!(queue.snapshot().unwrap().output_epoch, 4);
    }
}
