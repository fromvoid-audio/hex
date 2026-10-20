//! Bounded control-to-callback lease for transient EQ8 band audition.
use crate::native_callback_owner::atomic_signal::{self, Signal};
use crate::native_callback_owner::{
    NativePreparedConsumer as Consumer, NativePreparedProducer as Producer, native_prepared_ring,
    registered_prepared_ring,
};
use hex_engine::{PostNodeIdentity, ProcessorAuditionSelection, ProcessorChainOwner};
use std::sync::atomic::Ordering::{Acquire, Release};

pub const TARGET_COMMAND_CAPACITY: usize = 8;
pub const MAX_TARGET_COMMANDS_PER_BLOCK: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessorAuditionTarget {
    pub identity: PostNodeIdentity,
    pub project_generation: u64,
    /// First audio revision prepared from the UI's current project state.
    pub activation_floor: u64,
    pub sample_rate_hz: u32,
    pub selection: ProcessorAuditionSelection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetUpdate {
    Queued,
    PendingQueueFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearMode {
    Smooth,
    Immediate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetError {
    OutputClosed,
    InvalidDevice,
    InvalidOwner,
    InvalidActivationFloor,
    UnsupportedSampleRate(u32),
    EpochExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TargetCommand {
    epoch: u64,
    target: Option<ProcessorAuditionTarget>,
    clear_mode: ClearMode,
}

pub struct ProcessorAuditionTargetControl {
    producer: Producer<TargetCommand>,
    desired_intent: Signal,
    hard_cancel_epoch: Signal,
    rejected_epoch: Signal,
    desired: Option<ProcessorAuditionTarget>,
    next_epoch: u64,
    pending: Option<TargetCommand>,
    terminal: Option<TargetError>,
}

pub struct ProcessorAuditionTargetCallback {
    consumer: Consumer<TargetCommand>,
    desired_intent: Signal,
    hard_cancel_epoch: Signal,
    rejected_epoch: Signal,
    active_epoch: u64,
    active: Option<ProcessorAuditionTarget>,
    future: Option<TargetCommand>,
    boundary: Option<TargetBoundary>,
    observed_hard_cancel_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetBoundary {
    pub epoch: u64,
    pub previous: Option<ProcessorAuditionTarget>,
    pub next: Option<ProcessorAuditionTarget>,
    pub clear_mode: ClearMode,
}

pub(crate) fn constructor_cost() -> Option<(usize, usize)> {
    let retained = crate::prepared::factory::ring_bytes::<TargetCommand>(TARGET_COMMAND_CAPACITY)?
        .checked_add(crate::prepared::factory::shared_atomic_bytes()?.checked_mul(3)?)?;
    let peak = retained
        .checked_add(crate::prepared::factory::ring_scratch::<TargetCommand>()?)?
        .checked_add(size_of::<(
            ProcessorAuditionTargetControl,
            ProcessorAuditionTargetCallback,
        )>())?
        .checked_add(crate::prepared::factory::atomic_signal_scratch()?.checked_mul(3)?)?;
    Some((retained, peak))
}

pub fn channel() -> (
    ProcessorAuditionTargetControl,
    ProcessorAuditionTargetCallback,
) {
    construct_channel(None).expect("unqualified target channel has no fallible registration")
}
pub(crate) fn registered_channel(
    root: &crate::candidate_custody::CandidateRoot,
    slot: usize,
) -> Result<
    (
        ProcessorAuditionTargetControl,
        ProcessorAuditionTargetCallback,
    ),
    hex_engine::Invalid,
> {
    construct_channel(Some((root, slot)))
}
fn construct_channel(
    root: Option<(&crate::candidate_custody::CandidateRoot, usize)>,
) -> Result<
    (
        ProcessorAuditionTargetControl,
        ProcessorAuditionTargetCallback,
    ),
    hex_engine::Invalid,
> {
    let (producer, consumer) = if let Some((root, slot)) = root {
        let (producer, consumer, record) = registered_prepared_ring(TARGET_COMMAND_CAPACITY)
            .map_err(|_| hex_engine::Invalid("target queue constructor pairing refused"))?;
        root.retain_prepared_ring(slot, record)
            .map_err(hex_engine::Invalid)?;
        (producer, consumer)
    } else {
        native_prepared_ring(TARGET_COMMAND_CAPACITY)
    };
    let [desired_intent_control, desired_intent] =
        atomic_signal::pair(root.map(|(root, _)| root), 1)?;
    let [hard_cancel_epoch_control, hard_cancel_epoch] =
        atomic_signal::pair(root.map(|(root, _)| root), 2)?;
    let [rejected_epoch_control, rejected_epoch] =
        atomic_signal::pair(root.map(|(root, _)| root), 3)?;
    Ok((
        ProcessorAuditionTargetControl {
            producer,
            desired_intent: desired_intent_control,
            hard_cancel_epoch: hard_cancel_epoch_control,
            rejected_epoch: rejected_epoch_control,
            desired: None,
            next_epoch: 1,
            pending: None,
            terminal: None,
        },
        ProcessorAuditionTargetCallback {
            consumer,
            desired_intent,
            hard_cancel_epoch,
            rejected_epoch,
            active_epoch: 0,
            active: None,
            future: None,
            boundary: None,
            observed_hard_cancel_epoch: 0,
        },
    ))
}

impl ProcessorAuditionTargetControl {
    /// Actual constructor channel-field correlation only. Other owning
    /// descendants retain their independent mandatory inventory contracts.
    pub(crate) fn matches_constructor_channel_fields_on_control(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        root.matches_prepared_producer(4, &self.producer)
            && root.matches_atomic_signal(1, &self.desired_intent, 0)
            && root.matches_atomic_signal(2, &self.hard_cancel_epoch, 0)
            && root.matches_atomic_signal(3, &self.rejected_epoch, 0)
    }

    pub fn desired(&self) -> Option<ProcessorAuditionTarget> {
        let epoch = decode_intent(self.desired_intent.load(Acquire)).0;
        (epoch > self.hard_cancel_epoch.load(Acquire))
            .then_some(self.desired)
            .flatten()
    }

    /// Atomically fence every request admitted so far. Used by priority Stop,
    /// which must retain its shared-reference API and cannot depend on the
    /// bounded command ring or mutate the control-side producer state.
    pub fn invalidate_current_immediately(&self) {
        let epoch = decode_intent(self.desired_intent.load(Acquire)).0;
        self.hard_cancel_epoch.fetch_max(epoch, Release);
    }

    pub fn select(
        &mut self,
        identity: PostNodeIdentity,
        project_generation: u64,
        activation_floor: u64,
        sample_rate_hz: u32,
        selection: ProcessorAuditionSelection,
    ) -> Result<(ProcessorAuditionTarget, TargetUpdate), TargetError> {
        validate(identity, activation_floor, sample_rate_hz, selection)?;
        if let Some(error) = self.terminal {
            return Err(error);
        }
        let target = ProcessorAuditionTarget {
            identity,
            project_generation,
            activation_floor,
            sample_rate_hz,
            selection,
        };
        if self.desired == Some(target)
            && self.rejected_epoch.load(Acquire)
                != decode_intent(self.desired_intent.load(Acquire)).0
            && decode_intent(self.desired_intent.load(Acquire)).0
                > self.hard_cancel_epoch.load(Acquire)
        {
            return Ok((target, self.retry_pending()));
        }
        let epoch = self.reserve_epoch()?;
        self.desired = Some(target);
        self.rejected_epoch.store(0, Release);
        self.publish(TargetCommand {
            epoch,
            target: Some(target),
            clear_mode: ClearMode::Smooth,
        });
        Ok((target, self.retry_pending()))
    }

    pub fn release(&mut self) -> Result<TargetUpdate, TargetError> {
        self.clear(ClearMode::Smooth)
    }

    pub fn cancel_immediately(&mut self) -> Result<TargetUpdate, TargetError> {
        self.clear(ClearMode::Immediate)
    }

    fn clear(&mut self, mode: ClearMode) -> Result<TargetUpdate, TargetError> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        if mode == ClearMode::Smooth && self.desired.is_none() && self.pending.is_none() {
            return Ok(TargetUpdate::Queued);
        }
        let epoch = self.reserve_epoch()?;
        self.desired = None;
        self.publish(TargetCommand {
            epoch,
            target: None,
            clear_mode: mode,
        });
        Ok(self.retry_pending())
    }

    fn publish(&mut self, command: TargetCommand) {
        self.pending = Some(command);
        if command.clear_mode == ClearMode::Immediate {
            // Preserve a hard cancellation if a fresh Begin replaces the
            // desired command before the callback drains the queue.
            self.hard_cancel_epoch.store(command.epoch, Release);
        }
        self.desired_intent
            .store(encode_intent(command.epoch, command.clear_mode), Release);
    }

    pub fn retry_pending(&mut self) -> TargetUpdate {
        let Some(command) = self.pending else {
            return TargetUpdate::Queued;
        };
        match self.producer.push(command) {
            Ok(()) => {
                self.pending = None;
                TargetUpdate::Queued
            }
            Err(_) => TargetUpdate::PendingQueueFull,
        }
    }

    fn reserve_epoch(&mut self) -> Result<u64, TargetError> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        let epoch = self.next_epoch;
        let Some(next) = epoch.checked_add(1).filter(|next| *next <= (u64::MAX >> 2)) else {
            self.desired = None;
            self.pending = None;
            self.terminal = Some(TargetError::EpochExhausted);
            self.desired_intent.store(u64::MAX, Release);
            return Err(TargetError::EpochExhausted);
        };
        self.next_epoch = next;
        Ok(epoch)
    }
}

impl ProcessorAuditionTargetCallback {
    /// Actual constructor channel-field correlation only. Other owning
    /// descendants retain their independent mandatory inventory contracts.
    pub(crate) fn matches_constructor_channel_fields_on_control(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        root.matches_prepared_consumer(4, &self.consumer)
            && root.matches_atomic_signal(1, &self.desired_intent, 1)
            && root.matches_atomic_signal(2, &self.hard_cancel_epoch, 1)
            && root.matches_atomic_signal(3, &self.rejected_epoch, 1)
    }

    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.consumer.buffer())?
            .plus(ChannelStorage::paired_payload(self.desired_intent.as_ref()))?
            .plus(ChannelStorage::paired_payload(
                self.hard_cancel_epoch.as_ref(),
            ))?
            .plus(ChannelStorage::paired_payload(self.rejected_epoch.as_ref()))
    }

    /// Apply at most eight queued target changes at a callback block boundary.
    /// Atomic epochs invalidate old Begin requests immediately, even when a
    /// release command cannot enter the bounded queue.
    pub fn begin_block(&mut self) {
        let observed_word = self.desired_intent.load(Acquire);
        let (observed, mut mode) = decode_intent(observed_word);
        let previous = self.active;
        let mut changed = false;
        if observed != self.active_epoch {
            self.active = None;
            self.active_epoch = observed;
            changed = true;
        }
        for _ in 0..MAX_TARGET_COMMANDS_PER_BLOCK {
            let Ok(command) = self.consumer.pop() else {
                break;
            };
            if command.epoch < observed {
                continue;
            }
            if self
                .future
                .is_none_or(|future| command.epoch >= future.epoch)
            {
                self.future = Some(command);
            }
        }
        let current_word = self.desired_intent.load(Acquire);
        let (current, current_mode) = decode_intent(current_word);
        let hard_cancel_epoch = self.hard_cancel_epoch.load(Acquire);
        if current != self.active_epoch {
            self.active = None;
            self.active_epoch = current;
            mode = current_mode;
            changed = true;
        }
        match self.future {
            Some(command) if command.epoch == current && command.epoch > hard_cancel_epoch => {
                self.active = command.target;
                mode = command.clear_mode;
                changed = true;
                self.future = None;
            }
            Some(command) if command.epoch < current => self.future = None,
            Some(command) if command.epoch <= hard_cancel_epoch => self.future = None,
            _ => {}
        }
        if current <= hard_cancel_epoch
            && (self.active.is_some()
                || current != self.active_epoch
                || hard_cancel_epoch > self.observed_hard_cancel_epoch)
        {
            self.active = None;
            mode = ClearMode::Immediate;
            changed = true;
        }
        if hard_cancel_epoch > self.observed_hard_cancel_epoch {
            self.observed_hard_cancel_epoch = hard_cancel_epoch;
            mode = ClearMode::Immediate;
            changed = true;
        }
        if changed {
            self.boundary = Some(TargetBoundary {
                epoch: self.active_epoch,
                previous,
                next: self.active,
                clear_mode: mode,
            });
        }
    }

    pub fn current(&self) -> Option<ProcessorAuditionTarget> {
        let epoch = decode_intent(self.desired_intent.load(Acquire)).0;
        (epoch == self.active_epoch && epoch > self.hard_cancel_epoch.load(Acquire))
            .then_some(self.active)
            .flatten()
    }

    pub fn current_epoch(&self) -> u64 {
        self.active_epoch
    }

    /// Whether the latest explicitly published intent is newer than the
    /// lifecycle fence. A press admitted after Stop/Seek must survive that
    /// earlier discontinuity even if the callback consumes both together.
    pub fn has_request_after_hard_cancel(&self) -> bool {
        decode_intent(self.desired_intent.load(Acquire)).0 > self.hard_cancel_epoch.load(Acquire)
    }

    /// Fail closed on a callback-local render fault while still allowing an
    /// explicit later user gesture to mint a newer epoch.
    pub fn invalidate_current_immediately(&self) {
        let epoch = decode_intent(self.desired_intent.load(Acquire)).0;
        self.hard_cancel_epoch.fetch_max(epoch, Release);
    }

    pub fn reject_current(&self, epoch: u64) {
        self.rejected_epoch.store(epoch, Release);
    }

    pub fn take_boundary(&mut self) -> Option<TargetBoundary> {
        self.boundary.take()
    }
}

fn validate(
    identity: PostNodeIdentity,
    activation_floor: u64,
    sample_rate_hz: u32,
    _selection: ProcessorAuditionSelection,
) -> Result<(), TargetError> {
    if identity.device_id == 0 {
        return Err(TargetError::InvalidDevice);
    }
    if match identity.owner {
        ProcessorChainOwner::Track(id) | ProcessorChainOwner::Return(id) => id == 0,
        ProcessorChainOwner::Main => false,
    } {
        return Err(TargetError::InvalidOwner);
    }
    if identity.device_type.as_str().is_err() {
        return Err(TargetError::InvalidDevice);
    }
    if activation_floor == 0 {
        return Err(TargetError::InvalidActivationFloor);
    }
    if !(8_000..=192_000).contains(&sample_rate_hz) {
        return Err(TargetError::UnsupportedSampleRate(sample_rate_hz));
    }
    // The channel is device-neutral. The exact active runtime capability
    // performs band/lane and availability validation at admission.
    Ok(())
}

fn encode_intent(epoch: u64, mode: ClearMode) -> u64 {
    (epoch << 2)
        | match mode {
            ClearMode::Smooth => 1,
            ClearMode::Immediate => 2,
        }
}

fn decode_intent(intent: u64) -> (u64, ClearMode) {
    if intent == u64::MAX {
        return (u64::MAX, ClearMode::Immediate);
    }
    let mode = if intent & 0b11 == 2 {
        ClearMode::Immediate
    } else {
        ClearMode::Smooth
    };
    (intent >> 2, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(device_id: u64) -> PostNodeIdentity {
        PostNodeIdentity::new(
            ProcessorChainOwner::Track(9),
            device_id,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        )
    }

    fn selection(index: u8) -> ProcessorAuditionSelection {
        ProcessorAuditionSelection {
            index: u32::from(index),
            lane: 0,
        }
    }

    fn select(
        control: &mut ProcessorAuditionTargetControl,
        device_id: u64,
        generation: u64,
        floor: u64,
        index: u8,
    ) -> TargetUpdate {
        control
            .select(
                identity(device_id),
                generation,
                floor,
                48_000,
                selection(index),
            )
            .expect("valid target")
            .1
    }

    #[test]
    fn main_owner_preserves_exact_target_and_rejects_invalid_inputs() {
        let (mut control, mut callback) = channel();
        let main = PostNodeIdentity::new(
            ProcessorChainOwner::Main,
            17,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        );
        let chosen = selection(3);
        let (target, _) = control.select(main, 7, 12, 48_000, chosen).unwrap();
        callback.begin_block();
        assert_eq!(callback.current(), Some(target));
        assert_eq!(target.identity, main);
        assert_eq!(target.project_generation, 7);
        assert_eq!(target.activation_floor, 12);
        assert_eq!(target.selection, chosen);
        for (identity, floor, rate, error) in [
            (
                PostNodeIdentity::new(ProcessorChainOwner::Main, 0, main.device_type),
                12,
                48_000,
                TargetError::InvalidDevice,
            ),
            (main, 0, 48_000, TargetError::InvalidActivationFloor),
            (main, 12, 7_999, TargetError::UnsupportedSampleRate(7_999)),
            (
                PostNodeIdentity::new(ProcessorChainOwner::Track(0), 17, main.device_type),
                12,
                48_000,
                TargetError::InvalidOwner,
            ),
            (
                PostNodeIdentity::new(ProcessorChainOwner::Return(0), 17, main.device_type),
                12,
                48_000,
                TargetError::InvalidOwner,
            ),
            (
                PostNodeIdentity::new(
                    ProcessorChainOwner::Main,
                    17,
                    hex_engine::DeviceTypeId::from_static(
                        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                    ),
                ),
                12,
                48_000,
                TargetError::InvalidDevice,
            ),
        ] {
            assert_eq!(control.select(identity, 7, floor, rate, chosen), Err(error));
            assert_eq!(control.desired(), Some(target));
            callback.begin_block();
            assert_eq!(callback.current(), Some(target));
        }
    }

    #[test]
    fn hard_cancel_survives_a_new_begin_before_the_callback_boundary() {
        let (mut control, mut callback) = channel();
        let first = select(&mut control, 4, 2, 5, 0);
        assert_eq!(first, TargetUpdate::Queued);
        callback.begin_block();
        let started = callback.take_boundary().expect("begin boundary");
        assert_eq!(started.next, control.desired());
        assert_eq!(started.clear_mode, ClearMode::Smooth);

        control.cancel_immediately().expect("hard cancel");
        select(&mut control, 7, 3, 100, 1);
        callback.begin_block();
        let superseded = callback.take_boundary().expect("coalesced boundary");
        assert_eq!(superseded.previous, started.next);
        assert_eq!(superseded.next, control.desired());
        assert_eq!(superseded.clear_mode, ClearMode::Immediate);
    }

    #[test]
    fn full_queue_release_invalidates_queued_begins_and_retries_latest_clear() {
        let (mut control, mut callback) = channel();
        for index in 0..TARGET_COMMAND_CAPACITY as u8 {
            assert_eq!(
                select(&mut control, u64::from(index) + 1, 1, 1, index),
                TargetUpdate::Queued
            );
        }
        assert_eq!(
            control.cancel_immediately(),
            Ok(TargetUpdate::PendingQueueFull)
        );
        assert_eq!(control.desired(), None);
        callback.begin_block();
        let boundary = callback.take_boundary().expect("immediate invalidation");
        assert_eq!(boundary.next, None);
        assert_eq!(boundary.clear_mode, ClearMode::Immediate);
        assert_eq!(control.retry_pending(), TargetUpdate::Queued);
        callback.begin_block();
        assert_eq!(callback.take_boundary(), None);
        assert_eq!(callback.current(), None);
    }

    #[test]
    fn later_hard_cancel_clears_a_smoothly_released_fade_even_with_new_target() {
        let (mut control, mut callback) = channel();
        select(&mut control, 4, 2, 5, 0);
        callback.begin_block();
        assert!(callback.take_boundary().is_some());
        control.release().expect("soft release");
        callback.begin_block();
        let released = callback.take_boundary().expect("release boundary");
        assert_eq!(released.previous.unwrap().identity, identity(4));
        assert_eq!(released.clear_mode, ClearMode::Smooth);

        control.cancel_immediately().expect("hard clear fade");
        select(&mut control, 7, 3, 100, 1);
        callback.begin_block();
        let boundary = callback
            .take_boundary()
            .expect("hard cancellation preserved");
        assert_eq!(boundary.previous, None);
        assert_eq!(boundary.next, control.desired());
        assert_eq!(boundary.clear_mode, ClearMode::Immediate);
    }

    #[test]
    fn idle_blocks_and_observed_hard_cancel_do_not_emit_repeated_boundaries() {
        let (mut control, mut callback) = channel();
        callback.begin_block();
        assert_eq!(callback.take_boundary(), None);
        callback.begin_block();
        assert_eq!(callback.take_boundary(), None);

        select(&mut control, 4, 2, 5, 0);
        callback.begin_block();
        assert_eq!(callback.take_boundary().unwrap().next, control.desired());
        control.cancel_immediately().expect("hard cancel");
        callback.begin_block();
        assert_eq!(
            callback.take_boundary().unwrap().clear_mode,
            ClearMode::Immediate
        );
        callback.begin_block();
        assert_eq!(callback.take_boundary(), None);
        callback.begin_block();
        assert_eq!(callback.take_boundary(), None);
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod native_signal_field_tests {
    use super::*;
    #[test]
    fn actual_foreign_signal_field_refuses_before_native_carrier_birth() {
        let mut completed = 0;
        for field in 0..3 {
            let limits = crate::AudioCandidatePublicationReservation {
                runtime_slots: 3,
                archive_slots: 3,
                member_rows: 128,
                transfer_rows: 128,
                receipt_rows: 3,
                data_rows: 512,
                handle_rows: 512,
                max_retained_data_bytes: 4 << 20,
                max_construction_peak_data_bytes: 8 << 20,
            };
            let (mut control, callback) =
                crate::prepared::channel_with_factory_reservation(48000, None, Some(limits))
                    .unwrap();
            let root = control.native_candidate_root();
            // The real Root queue fields remain intact: only this actual atomic
            // owning field changes. Identical scalar value/layout is not origin.
            let foreign = Signal::Legacy(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)));
            match field {
                0 => control.processor_audition.desired_intent = foreign,
                1 => control.processor_audition.hard_cancel_epoch = foreign,
                2 => control.processor_audition.rejected_epoch = foreign,
                _ => unreachable!(),
            }
            let mut refused = false;
            let birth = allocation_counter::measure(|| {
                refused =
                    crate::native_callback_owner::reserve_native_worker(control, callback, None)
                        .is_err();
            });
            assert!(
                refused,
                "actual foreign signal field {field} must not inherit Root origin"
            );
            assert_eq!(
                birth.count_total, 0,
                "signal field {field}: before any carrier birth"
            );
            assert_eq!(root.test_native_carrier_facts().unwrap().0, 0);
            completed += 1;
            eprintln!("SUPPORTED actual signal field{field} refused before carrier births");
        }
        assert_eq!(completed, 3);
    }
}
