//! Bounded control-to-callback selection for one EQ8 post-node spectrum tap.
use crate::native_callback_owner::atomic_signal::{self, Signal};
use crate::native_callback_owner::{
    NativePreparedConsumer as Consumer, NativePreparedProducer as Producer, native_prepared_ring,
    registered_prepared_ring,
};
use crate::spectrum_worker::SpectrumTargetId;
use hex_engine::{PostNodeIdentity, ProcessorChainOwner};
use std::sync::atomic::Ordering::{Acquire, Release};

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TargetCommand {
    epoch: u64,
    target: Option<SpectrumTarget>,
}

/// Control-side target publisher. A failed queue push retains the latest full
/// command for the caller's next UI/control tick, while the epoch immediately
/// invalidates stale callback output.
pub struct SpectrumTargetControl {
    producer: Producer<TargetCommand>,
    desired_epoch: Signal,
    desired: Option<SpectrumTarget>,
    next_epoch: u64,
    next_token: u64,
    pending: Option<TargetCommand>,
    terminal: Option<TargetError>,
}

/// Callback-side bounded command receiver and currently selected EQ8 owner.
pub struct SpectrumTargetCallback {
    consumer: Consumer<TargetCommand>,
    desired_epoch: Signal,
    active_epoch: u64,
    active: Option<SpectrumTarget>,
    future: Option<TargetCommand>,
}

pub(crate) fn constructor_cost() -> Option<(usize, usize)> {
    let retained = crate::prepared::factory::ring_bytes::<TargetCommand>(TARGET_COMMAND_CAPACITY)?
        .checked_add(crate::prepared::factory::shared_atomic_bytes()?)?;
    let peak = retained
        .checked_add(crate::prepared::factory::ring_scratch::<TargetCommand>()?)?
        .checked_add(size_of::<(SpectrumTargetControl, SpectrumTargetCallback)>())?
        .checked_add(crate::prepared::factory::atomic_signal_scratch()?)?;
    Some((retained, peak))
}

pub fn channel() -> (SpectrumTargetControl, SpectrumTargetCallback) {
    construct_channel(None).expect("unqualified target channel has no fallible registration")
}
pub(crate) fn registered_channel(
    root: &crate::candidate_custody::CandidateRoot,
    slot: usize,
) -> Result<(SpectrumTargetControl, SpectrumTargetCallback), hex_engine::Invalid> {
    construct_channel(Some((root, slot)))
}
fn construct_channel(
    root: Option<(&crate::candidate_custody::CandidateRoot, usize)>,
) -> Result<(SpectrumTargetControl, SpectrumTargetCallback), hex_engine::Invalid> {
    let (producer, consumer) = if let Some((root, slot)) = root {
        let (producer, consumer, record) = registered_prepared_ring(TARGET_COMMAND_CAPACITY)
            .map_err(|_| hex_engine::Invalid("target queue constructor pairing refused"))?;
        root.retain_prepared_ring(slot, record)
            .map_err(hex_engine::Invalid)?;
        (producer, consumer)
    } else {
        native_prepared_ring(TARGET_COMMAND_CAPACITY)
    };
    let [desired_epoch_control, desired_epoch] =
        atomic_signal::pair(root.map(|(root, _)| root), 0)?;
    Ok((
        SpectrumTargetControl {
            producer,
            desired_epoch: desired_epoch_control,
            desired: None,
            next_epoch: 1,
            next_token: 1,
            pending: None,
            terminal: None,
        },
        SpectrumTargetCallback {
            consumer,
            desired_epoch,
            active_epoch: 0,
            active: None,
            future: None,
        },
    ))
}

impl SpectrumTargetControl {
    /// Actual constructor channel-field correlation only. Other owning
    /// descendants retain their independent mandatory inventory contracts.
    pub(crate) fn matches_constructor_channel_fields_on_control(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        root.matches_prepared_producer(3, &self.producer)
            && root.matches_atomic_signal(0, &self.desired_epoch, 0)
    }

    pub fn desired(&self) -> Option<SpectrumTarget> {
        self.desired
    }

    /// Select a device. Repeating the same selection reuses its opaque token;
    /// selecting a different device or project generation creates a new token.
    pub fn select(
        &mut self,
        identity: PostNodeIdentity,
        project_generation: u64,
        activation_floor: u64,
        sample_rate_hz: u32,
    ) -> Result<(SpectrumTarget, TargetUpdate), TargetError> {
        if identity.device_id == 0 {
            return Err(TargetError::InvalidDevice);
        }
        if match identity.owner {
            ProcessorChainOwner::Track(id) | ProcessorChainOwner::Return(id) => id == 0,
        } {
            return Err(TargetError::InvalidOwner);
        }
        if !matches!(identity.device_type.as_str(), Ok("hex.eq8" | "hex.eq")) {
            return Err(TargetError::WrongDeviceType);
        }
        if activation_floor == 0 {
            return Err(TargetError::InvalidActivationFloor);
        }
        if !(8_000..=192_000).contains(&sample_rate_hz) {
            return Err(TargetError::UnsupportedSampleRate(sample_rate_hz));
        }
        if let Some(error) = self.terminal {
            return Err(error);
        }
        if let Some(current) = self.desired
            && current.identity == identity
            && current.project_generation == project_generation
            && current.activation_floor == activation_floor
            && current.sample_rate_hz == sample_rate_hz
        {
            return Ok((current, self.retry_pending()));
        }
        let token = self.next_token;
        let Some(next_token) = token.checked_add(1) else {
            self.fail_closed(TargetError::TokenExhausted);
            return Err(TargetError::TokenExhausted);
        };
        self.next_token = next_token;
        let epoch = self.reserve_epoch()?;
        let target = SpectrumTarget {
            token: SpectrumTargetId(token),
            identity,
            project_generation,
            activation_floor,
            sample_rate_hz,
        };
        self.desired = Some(target);
        self.pending = Some(TargetCommand {
            epoch,
            target: Some(target),
        });
        self.desired_epoch.store(epoch, Release);
        Ok((target, self.retry_pending()))
    }

    /// Disable the analyzer and invalidate any callback-held target immediately.
    pub fn clear(&mut self) -> Result<TargetUpdate, TargetError> {
        if let Some(error) = self.terminal {
            return Err(error);
        }
        if self.desired.is_none() && self.pending.is_none() {
            return Ok(TargetUpdate::Queued);
        }
        let epoch = self.reserve_epoch()?;
        self.desired = None;
        self.pending = Some(TargetCommand {
            epoch,
            target: None,
        });
        self.desired_epoch.store(epoch, Release);
        Ok(self.retry_pending())
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
        self.desired = None;
        self.pending = None;
        self.terminal = Some(error);
        // MAX is reserved as the terminal invalidation epoch and is never
        // assigned to a selectable target.
        self.desired_epoch.store(u64::MAX, Release);
    }
}

impl SpectrumTargetCallback {
    /// Actual constructor channel-field correlation only. Other owning
    /// descendants retain their independent mandatory inventory contracts.
    pub(crate) fn matches_constructor_channel_fields_on_control(
        &self,
        root: &crate::candidate_custody::CandidateRoot,
    ) -> bool {
        root.matches_prepared_consumer(3, &self.consumer)
            && root.matches_atomic_signal(0, &self.desired_epoch, 1)
    }

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
            self.active = None;
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
            self.active = None;
            self.active_epoch = current_epoch;
        }
        match self.future {
            Some(command) if command.epoch == current_epoch => {
                self.active = command.target;
                self.future = None;
            }
            Some(command) if command.epoch < current_epoch => self.future = None,
            _ => {}
        }
    }

    /// Return the selected target only while it remains the desired epoch.
    pub fn current(&self) -> Option<SpectrumTarget> {
        (self.desired_epoch.load(Acquire) == self.active_epoch)
            .then_some(self.active)
            .flatten()
    }

    pub fn current_epoch(&self) -> u64 {
        self.active_epoch
    }
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

#[cfg(all(test, feature = "allocation-probe"))]
mod native_signal_field_tests {
    use super::*;
    #[test]
    fn actual_foreign_signal_field_refuses_before_native_carrier_birth() {
        let mut completed = 0;
        for field in 0..1 {
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
                0 => control.spectrum_target.desired_epoch = foreign,
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
        assert_eq!(completed, 1);
    }
}
