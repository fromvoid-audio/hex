//! Bounded atomic control-to-callback membership for visible EQ post-node taps.
mod targets;
use crate::spectrum_worker::SpectrumTargetId;
use hex_engine::{PostNodeIdentity, ProcessorChainOwner};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{
        AtomicU64,
        Ordering::{Acquire, Release},
    },
};
pub use targets::{MAX_SPECTRUM_TARGETS, SpectrumTargetRequest, SpectrumTargets};

pub const TARGET_COMMAND_CAPACITY: usize = 8;
pub const MAX_TARGET_COMMANDS_PER_BLOCK: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpectrumTarget {
    pub token: SpectrumTargetId,
    pub identity: PostNodeIdentity,
    pub project_generation: u64,
    /// First audio publication known to belong to this project context. The
    /// callback waits for this revision before it accepts any samples.
    pub activation_floor: u64,
    pub sample_rate_hz: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetUpdate {
    Queued,
    PendingQueueFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetError {
    OutputClosed,
    InvalidDevice,
    InvalidOwner,
    WrongDeviceType,
    InvalidActivationFloor,
    UnsupportedSampleRate(u32),
    EpochExhausted,
    TokenExhausted,
    TooManyTargets,
    DuplicateTarget,
    InconsistentContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TargetCommand {
    epoch: u64,
    targets: SpectrumTargets,
}

/// Control-side target publisher. A failed queue push retains the latest full
/// command for the caller's next UI/control tick, while the epoch immediately
/// invalidates stale callback output.
pub struct SpectrumTargetControl {
    producer: Producer<TargetCommand>,
    desired_epoch: Arc<AtomicU64>,
    desired: SpectrumTargets,
    next_epoch: u64,
    next_token: u64,
    pending: Option<TargetCommand>,
    terminal: Option<TargetError>,
}

/// Callback-side bounded command receiver and admitted visible EQ membership.
pub struct SpectrumTargetCallback {
    consumer: Consumer<TargetCommand>,
    desired_epoch: Arc<AtomicU64>,
    active_epoch: u64,
    active: SpectrumTargets,
    future: Option<TargetCommand>,
}

pub fn channel() -> (SpectrumTargetControl, SpectrumTargetCallback) {
    let (producer, consumer) = RingBuffer::new(TARGET_COMMAND_CAPACITY);
    let desired_epoch = Arc::new(AtomicU64::new(0));
    (
        SpectrumTargetControl {
            producer,
            desired_epoch: Arc::clone(&desired_epoch),
            desired: SpectrumTargets::empty(),
            next_epoch: 1,
            next_token: 1,
            pending: None,
            terminal: None,
        },
        SpectrumTargetCallback {
            consumer,
            desired_epoch,
            active_epoch: 0,
            active: SpectrumTargets::empty(),
            future: None,
        },
    )
}

impl SpectrumTargetControl {
    /// First slot for callers that explicitly select a single target.
    pub fn desired(&self) -> Option<SpectrumTarget> {
        self.desired.iter().next().map(|(_, target)| target)
    }
    pub fn desired_targets(&self) -> &SpectrumTargets {
        &self.desired
    }

    /// Publish a complete desired set. Unchanged requests retain tokens and
    /// slots; invalid input leaves the accepted set intact. Full queues retain
    /// the newest whole set and immediately invalidate the old callback epoch.
    pub fn set_targets(
        &mut self,
        requests: &[SpectrumTargetRequest],
    ) -> Result<TargetUpdate, TargetError> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        let (targets, next_token) = match self.desired.replacing(requests, self.next_token) {
            Ok(next) => next,
            Err(error @ TargetError::TokenExhausted) => {
                self.fail_closed(error);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        if targets == self.desired {
            return Ok(self.retry_pending());
        }
        let epoch = self.reserve_epoch()?;
        self.next_token = next_token;
        self.desired = targets;
        self.pending = Some(TargetCommand { epoch, targets });
        self.desired_epoch.store(epoch, Release);
        Ok(self.retry_pending())
    }

    /// Select exactly one target through the same whole-set protocol.
    pub fn select(
        &mut self,
        identity: PostNodeIdentity,
        project_generation: u64,
        activation_floor: u64,
        sample_rate_hz: u32,
    ) -> Result<(SpectrumTarget, TargetUpdate), TargetError> {
        let update = self.set_targets(&[SpectrumTargetRequest {
            identity,
            project_generation,
            activation_floor,
            sample_rate_hz,
        }])?;
        self.desired()
            .map(|target| (target, update))
            .ok_or(TargetError::InvalidDevice)
    }

    /// Disable every tap and invalidate callback-held targets immediately.
    pub fn clear(&mut self) -> Result<TargetUpdate, TargetError> {
        self.set_targets(&[])
    }

    /// Retry a previously full queue. Call from the control/UI pump, never from
    /// the audio callback.
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
        let Some(next_epoch) = epoch.checked_add(1) else {
            self.fail_closed(TargetError::EpochExhausted);
            return Err(TargetError::EpochExhausted);
        };
        self.next_epoch = next_epoch;
        Ok(epoch)
    }

    fn fail_closed(&mut self, error: TargetError) {
        self.desired = SpectrumTargets::empty();
        self.pending = None;
        self.terminal = Some(error);
        // MAX is reserved as the terminal invalidation epoch and is never
        // assigned to a selectable target.
        self.desired_epoch.store(u64::MAX, Release);
    }
}

impl SpectrumTargetCallback {
    pub(crate) fn constructor_channel_storage(
        &self,
    ) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        ChannelStorage::ring(self.consumer.buffer())?
            .plus(ChannelStorage::paired_payload(self.desired_epoch.as_ref()))
    }

    /// Apply at most eight queued target changes at a render-block boundary.
    /// A future-epoch command is retained until the matching epoch becomes
    /// visible, covering the load-epoch/enqueue-command race without blocking.
    pub fn begin_block(&mut self) {
        let observed_epoch = self.desired_epoch.load(Acquire);
        self.begin_block_after_epoch_load(observed_epoch);
    }

    fn begin_block_after_epoch_load(&mut self, observed_epoch: u64) {
        if observed_epoch != self.active_epoch {
            self.active = SpectrumTargets::empty();
            self.active_epoch = observed_epoch;
        }
        for _ in 0..MAX_TARGET_COMMANDS_PER_BLOCK {
            let Ok(command) = self.consumer.pop() else {
                break;
            };
            if command.epoch < observed_epoch {
                continue;
            }
            if self
                .future
                .is_none_or(|future| command.epoch >= future.epoch)
            {
                self.future = Some(command);
            }
        }

        let current_epoch = self.desired_epoch.load(Acquire);
        if current_epoch != self.active_epoch {
            self.active = SpectrumTargets::empty();
            self.active_epoch = current_epoch;
        }
        match self.future {
            Some(command) if command.epoch == current_epoch => {
                self.active = command.targets;
                self.future = None;
            }
            Some(command) if command.epoch < current_epoch => self.future = None,
            _ => {}
        }
    }

    /// Return active membership only while the complete desired epoch matches.
    pub fn current_targets(&self) -> Option<&SpectrumTargets> {
        (self.desired_epoch.load(Acquire) == self.active_epoch).then_some(&self.active)
    }
    pub fn current(&self) -> Option<SpectrumTarget> {
        self.current_targets()?
            .iter()
            .next()
            .map(|(_, target)| target)
    }

    pub fn current_epoch(&self) -> u64 {
        self.active_epoch
    }
}

/// Fresh fixed channel payload quote; opaque allocator/OS costs are excluded.
pub(crate) fn constructor_storage_quote() -> Option<usize> {
    use crate::native_callback_owner::ChannelStorage;
    ChannelStorage::ring_bytes::<TargetCommand>(TARGET_COMMAND_CAPACITY)?
        .checked_add(std::mem::size_of::<AtomicU64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(device_id: u64, owner: ProcessorChainOwner) -> PostNodeIdentity {
        PostNodeIdentity::new(
            owner,
            device_id,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        )
    }

    fn select(control: &mut SpectrumTargetControl, device_id: u64) -> SpectrumTarget {
        control
            .select(
                identity(device_id, ProcessorChainOwner::Track(1)),
                0,
                1,
                48_000,
            )
            .unwrap()
            .0
    }

    fn request(device: u64) -> SpectrumTargetRequest {
        SpectrumTargetRequest {
            identity: identity(device, ProcessorChainOwner::Track(1)),
            project_generation: 7,
            activation_floor: 1,
            sample_rate_hz: 48_000,
        }
    }

    #[test]
    fn complete_chain_membership_preserves_slots_and_tokens_across_focus_order_and_removal() {
        assert_eq!(MAX_SPECTRUM_TARGETS, hex_project::MAX_DEVICES_PER_TRACK);
        let (mut control, mut callback) = channel();
        let mut requests: Vec<_> = (1..=MAX_SPECTRUM_TARGETS as u64).map(request).collect();
        control.set_targets(&requests).unwrap();
        callback.begin_block();
        let original = *callback.current_targets().unwrap();
        assert_eq!(original.len(), MAX_SPECTRUM_TARGETS);
        for (slot, target) in original.iter() {
            assert_eq!(original.find(target.identity), Some((slot, target)));
        }
        requests.reverse();
        control.set_targets(&requests).unwrap();
        assert_eq!(
            callback.current_targets(),
            Some(&original),
            "order is not membership"
        );
        requests.retain(|request| request.identity.device_id > 2);
        control.set_targets(&requests).unwrap();
        assert!(
            callback.current_targets().is_none(),
            "publish invalidates the entire old epoch"
        );
        callback.begin_block();
        assert_eq!(
            callback.current_targets().unwrap().len(),
            MAX_SPECTRUM_TARGETS - 2
        );
        requests.extend([request(65), request(66)]);
        control.set_targets(&requests).unwrap();
        callback.begin_block();
        let current = callback.current_targets().unwrap();
        for (slot, target) in original.iter().filter(|(_, t)| t.identity.device_id > 2) {
            assert_eq!(
                current.get(slot),
                Some(target),
                "unrelated windows keep their capture identity"
            );
        }
        assert!(current.find(request(1).identity).is_none());
        assert!(current.find(request(65).identity).is_some());
        assert_eq!(current.len(), MAX_SPECTRUM_TARGETS);
    }

    #[test]
    fn malformed_sets_are_atomic_and_full_queues_retry_the_latest_whole_membership() {
        let (mut control, mut callback) = channel();
        control.set_targets(&[request(1), request(2)]).unwrap();
        callback.begin_block();
        let original = *control.desired_targets();
        let mut foreign = request(3);
        foreign.project_generation += 1;
        for (requests, error) in [
            (vec![request(1), request(1)], TargetError::DuplicateTarget),
            (vec![request(1), foreign], TargetError::InconsistentContext),
            (
                (1..=MAX_SPECTRUM_TARGETS as u64 + 1).map(request).collect(),
                TargetError::TooManyTargets,
            ),
        ] {
            assert_eq!(control.set_targets(&requests), Err(error));
            assert_eq!(control.desired_targets(), &original);
            assert_eq!(callback.current_targets(), Some(&original));
        }
        for i in 0..TARGET_COMMAND_CAPACITY {
            control
                .set_targets(&[request(10 + i as u64), request(100 + i as u64)])
                .unwrap();
        }
        assert_eq!(
            control
                .set_targets(&[request(300), request(400), request(500)])
                .unwrap(),
            TargetUpdate::PendingQueueFull
        );
        assert!(callback.current_targets().is_none());
        callback.begin_block();
        assert_eq!(control.retry_pending(), TargetUpdate::Queued);
        callback.begin_block();
        let current = callback.current_targets().unwrap();
        assert_eq!(current, control.desired_targets());
        assert_eq!(current.len(), 3);
        assert!(current.find(request(500).identity).is_some());
    }

    #[test]
    fn main_owner_preserves_exact_target_and_rejects_invalid_inputs() {
        let (mut control, mut callback) = channel();
        let main = identity(17, ProcessorChainOwner::Main);
        let (target, _) = control.select(main, 7, 12, 48_000).unwrap();
        callback.begin_block();
        assert_eq!(callback.current(), Some(target));
        assert_eq!(target.identity, main);
        assert_eq!(target.project_generation, 7);
        assert_eq!(target.activation_floor, 12);
        for (identity, floor, rate, error) in [
            (
                identity(0, ProcessorChainOwner::Main),
                12,
                48_000,
                TargetError::InvalidDevice,
            ),
            (main, 0, 48_000, TargetError::InvalidActivationFloor),
            (main, 12, 7_999, TargetError::UnsupportedSampleRate(7_999)),
            (
                identity(17, ProcessorChainOwner::Track(0)),
                12,
                48_000,
                TargetError::InvalidOwner,
            ),
            (
                identity(17, ProcessorChainOwner::Return(0)),
                12,
                48_000,
                TargetError::InvalidOwner,
            ),
            (
                PostNodeIdentity::new(
                    ProcessorChainOwner::Main,
                    17,
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                ),
                12,
                48_000,
                TargetError::WrongDeviceType,
            ),
        ] {
            assert_eq!(control.select(identity, 7, floor, rate), Err(error));
            assert_eq!(control.desired(), Some(target));
            callback.begin_block();
            assert_eq!(callback.current(), Some(target));
        }
    }

    #[test]
    fn target_switch_is_stable_and_invalidates_stale_selection() {
        let (mut control, mut callback) = channel();
        let first = select(&mut control, 10);
        callback.begin_block();
        assert_eq!(callback.current(), Some(first));
        let same = select(&mut control, 10);
        assert_eq!(same, first);
        let second = select(&mut control, 11);
        assert_eq!(callback.current(), None);
        callback.begin_block();
        assert_eq!(callback.current(), Some(second));
        control.clear().unwrap();
        assert_eq!(callback.current(), None);
        callback.begin_block();
        assert_eq!(callback.current(), None);
    }

    #[test]
    fn same_device_id_under_a_different_owner_gets_a_new_capture_token() {
        let (mut control, mut callback) = channel();
        let first = select(&mut control, 10);
        callback.begin_block();
        assert_eq!(callback.current(), Some(first));

        let (second, _) = control
            .select(identity(10, ProcessorChainOwner::Return(1)), 0, 1, 48_000)
            .unwrap();
        assert_ne!(second.token, first.token);
        assert_ne!(second.identity.owner, first.identity.owner);
        callback.begin_block();
        assert_eq!(callback.current(), Some(second));
    }

    #[test]
    fn command_enqueued_after_epoch_load_is_retained_for_next_block() {
        let (mut control, mut callback) = channel();
        let first = select(&mut control, 10);
        callback.begin_block();
        assert_eq!(callback.current(), Some(first));

        // Model callback loading epoch 1, then UI advancing and successfully
        // enqueueing epoch 2 before the callback's bounded queue drain.
        let observed_epoch = callback.desired_epoch.load(Acquire);
        let second = select(&mut control, 11);
        assert_eq!(observed_epoch, 1);
        callback.begin_block_after_epoch_load(observed_epoch);
        assert_eq!(callback.current(), Some(second));
    }

    #[test]
    fn queue_full_invalidates_audio_tap_and_retains_latest_command_for_retry() {
        let (mut control, mut callback) = channel();
        for id in 1..=TARGET_COMMAND_CAPACITY as u64 {
            select(&mut control, id);
        }
        let (target, outcome) = control
            .select(identity(99, ProcessorChainOwner::Track(1)), 0, 1, 48_000)
            .unwrap();
        assert_eq!(outcome, TargetUpdate::PendingQueueFull);
        assert_eq!(callback.current(), None);
        callback.begin_block();
        assert_ne!(callback.current(), Some(target));
        assert_eq!(control.retry_pending(), TargetUpdate::Queued);
        callback.begin_block();
        assert_eq!(callback.current(), Some(target));
    }

    #[test]
    fn unsupported_rate_and_identity_are_refused_without_mutating_selection() {
        let (mut control, _) = channel();
        assert_eq!(
            control.select(identity(0, ProcessorChainOwner::Track(1)), 0, 1, 48_000),
            Err(TargetError::InvalidDevice)
        );
        assert_eq!(
            control.select(identity(1, ProcessorChainOwner::Track(0)), 0, 1, 48_000),
            Err(TargetError::InvalidOwner)
        );
        assert_eq!(
            control.select(identity(1, ProcessorChainOwner::Track(1)), 0, 1, 7_999),
            Err(TargetError::UnsupportedSampleRate(7_999))
        );
        assert_eq!(
            control.select(
                PostNodeIdentity::new(
                    ProcessorChainOwner::Track(1),
                    1,
                    hex_engine::DeviceTypeId::from_static("hex.utility"),
                ),
                0,
                1,
                48_000,
            ),
            Err(TargetError::WrongDeviceType)
        );
        assert_eq!(
            control.select(identity(1, ProcessorChainOwner::Track(1)), 0, 0, 48_000),
            Err(TargetError::InvalidActivationFloor)
        );
        assert_eq!(control.desired(), None);
    }

    #[test]
    fn token_exhaustion_invalidates_the_previous_selection() {
        let (mut control, mut callback) = channel();
        let first = select(&mut control, 10);
        callback.begin_block();
        assert_eq!(callback.current(), Some(first));
        control.next_token = u64::MAX;
        assert_eq!(
            control.select(identity(11, ProcessorChainOwner::Track(1)), 0, 1, 48_000),
            Err(TargetError::TokenExhausted)
        );
        assert_eq!(control.desired(), None);
        callback.begin_block();
        assert_eq!(callback.current(), None);
        assert_eq!(
            control.select(identity(12, ProcessorChainOwner::Track(1)), 0, 1, 48_000),
            Err(TargetError::TokenExhausted)
        );
        assert_eq!(control.clear(), Err(TargetError::TokenExhausted));
        assert_eq!(control.desired_epoch.load(Acquire), u64::MAX);
        assert_eq!(control.retry_pending(), TargetUpdate::Queued);
        callback.begin_block();
        assert_eq!(callback.current(), None);
    }

    #[test]
    fn epoch_exhaustion_fails_closed_and_invalidates_callback_target() {
        let (mut control, mut callback) = channel();
        let first = select(&mut control, 10);
        callback.begin_block();
        assert_eq!(callback.current(), Some(first));

        control.next_epoch = u64::MAX;
        assert_eq!(control.clear(), Err(TargetError::EpochExhausted));
        assert_eq!(control.desired(), None);
        callback.begin_block();
        assert_eq!(callback.current(), None);
        assert_eq!(control.clear(), Err(TargetError::EpochExhausted));
        assert_eq!(
            control.select(identity(11, ProcessorChainOwner::Track(1)), 0, 1, 48_000),
            Err(TargetError::EpochExhausted)
        );
        assert_eq!(control.desired_epoch.load(Acquire), u64::MAX);
        assert_eq!(control.retry_pending(), TargetUpdate::Queued);
        callback.begin_block();
        assert_eq!(callback.current(), None);
    }
}
