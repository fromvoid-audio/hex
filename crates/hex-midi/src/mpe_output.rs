//! Bounded MPE hardware output channel allocator and per-occurrence expression
//! router built on [`crate::MpeConfig`] and [`crate::OutputProducer`].
//!
//! # Contract
//! - Allocates one dedicated MPE member channel per active
//!   [`MpeOutputOccurrence`] (`(source_id, occurrence_id)`), never sharing a
//!   member channel silently across simultaneous notes (including same-pitch
//!   overlaps).
//! - Uses deterministic least-recently-used (LRU) member-channel reuse so
//!   releasing voices on recently freed channels are disturbed last.
//! - Before every `NoteOn`, resets per-member Pitch Bend, Slide (CC74), and
//!   Channel Pressure to the note's initial values (neutral by default) at the
//!   same timestamp so reused channels never inherit prior expression state.
//! - Emits standard MPE Configuration Message (RPN `00 06`) and master/member
//!   Pitch Bend Sensitivity (RPN `00 00`) on explicit [`MpeOutputAllocator::send_zone_setup`].
//! - All operations on [`MpeOutputAllocator`] use fixed-size inline storage and
//!   perform zero heap allocations, zero locks, and zero blocking calls.
//! - Endpoint discovery/selection remain owned by [`crate::OutputWorker`] keyed
//!   strictly by stable [`crate::OutputPort::id`]; Stop delegates to
//!   [`crate::OutputProducer::request_stop`].

use crate::mpe::{MAX_MPE_PITCH_BEND_CENTS, MpeConfig, MpeZone};
use crate::output::{
    OutputEnqueueResult, OutputError, OutputMessage, OutputProducer, TimestampedOutputMessage,
};

/// Maximum number of MPE member channels in a single Lower or Upper zone.
pub const MAX_MPE_MEMBER_CHANNELS: usize = 15;
/// Center (zero bend) value for 14-bit unsigned MIDI Pitch Bend.
pub const PITCH_BEND_CENTER: u16 = 8192;
/// Default neutral MPE per-note Slide / Timbre (CC74) value.
pub const DEFAULT_MPE_TIMBRE: u8 = 64;

/// Unique source and occurrence identity for one sounding MPE output note.
///
/// Pairing `source_id` with a non-zero `occurrence_id` keeps overlapping notes
/// of identical pitch completely isolated across distinct MPE member channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MpeOutputOccurrence {
    pub source_id: u64,
    pub occurrence_id: u64,
}

/// Typed parameters for allocating and emitting one MPE member-channel note-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MpeNoteOn {
    pub occurrence: MpeOutputOccurrence,
    pub pitch: u8,
    pub velocity: u8,
    pub initial: MpeInitialExpression,
}

/// Optional initial per-note expression emitted immediately before `NoteOn` on
/// the allocated member channel so reused channels start from known state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MpeInitialExpression {
    pub pitch_bend_cents: i16,
    pub timbre: u8,
    pub pressure: u8,
}

impl Default for MpeInitialExpression {
    fn default() -> Self {
        Self {
            pitch_bend_cents: 0,
            timbre: DEFAULT_MPE_TIMBRE,
            pressure: 0,
        }
    }
}

/// Deterministic errors returned by [`MpeOutputAllocator`] without heap allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MpeOutputError {
    InvalidConfig,
    InvalidOccurrence,
    InvalidPitch,
    InvalidVelocity,
    InvalidValue,
    DuplicateOccurrence,
    UnknownOccurrence,
    ChannelsExhausted,
    StaleActivation,
    Disconnected,
    QueueOverflow,
    InvalidMessage(OutputError),
}

#[derive(Clone, Copy, Debug, Default)]
struct MemberSlot {
    active: Option<ActiveMemberNote>,
    last_released_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveMemberNote {
    occurrence: MpeOutputOccurrence,
    pitch: u8,
}

/// Converts a signed pitch-bend offset in cents into an unsigned 14-bit MIDI
/// pitch-bend value (`0..=16383`, center `8192`) for `range_semitones` (`1..=96`).
pub fn cents_to_pitch_bend_u14(cents: i16, range_semitones: u8) -> u16 {
    if range_semitones == 0 {
        return PITCH_BEND_CENTER;
    }
    let clamped_cents = cents.clamp(-MAX_MPE_PITCH_BEND_CENTS, MAX_MPE_PITCH_BEND_CENTS);
    let range_cents = i64::from(range_semitones) * 100;
    if clamped_cents >= 0 {
        let delta = ((i64::from(clamped_cents) * 8191 + range_cents / 2) / range_cents).min(8191);
        (8192 + delta) as u16
    } else {
        let magnitude = i64::from(clamped_cents).unsigned_abs() as i64;
        let delta = ((magnitude * 8192 + range_cents / 2) / range_cents).min(8192);
        (8192 - delta) as u16
    }
}

/// Fixed-capacity, allocation-free MPE output member-channel allocator and
/// per-occurrence expression emitter.
pub struct MpeOutputAllocator {
    config: MpeConfig,
    slots: [MemberSlot; 16],
    active_count: u8,
    release_seq: u64,
    last_timestamp_ns: u64,
    bound_activation: u64,
}

impl MpeOutputAllocator {
    /// Constructs a validated MPE output allocator for `config`.
    pub fn new(config: MpeConfig) -> Result<Self, MpeOutputError> {
        if !config.is_valid() {
            return Err(MpeOutputError::InvalidConfig);
        }
        Ok(Self {
            config,
            slots: [MemberSlot::default(); 16],
            active_count: 0,
            release_seq: 0,
            last_timestamp_ns: 0,
            bound_activation: 0,
        })
    }

    pub const fn config(&self) -> MpeConfig {
        self.config
    }

    pub const fn capacity(&self) -> usize {
        match self.config.zone {
            MpeZone::Lower { member_channels } | MpeZone::Upper { member_channels } => {
                member_channels as usize
            }
        }
    }

    pub const fn active_count(&self) -> usize {
        self.active_count as usize
    }

    /// Returns the currently allocated MPE member channel (`0..=15`) for
    /// `occurrence`, if sounding.
    pub fn channel_for(&self, occurrence: MpeOutputOccurrence) -> Option<u8> {
        self.find_occurrence(occurrence)
    }

    /// Clears local channel allocation state when switching to a fresh output
    /// connection activation after disconnect/reconnect.
    pub fn reset_for_activation(&mut self, activation: u64) {
        self.slots = [MemberSlot::default(); 16];
        self.active_count = 0;
        self.release_seq = 0;
        self.last_timestamp_ns = 0;
        self.bound_activation = activation;
    }

    /// Emits the MPE Configuration Message (RPN `00 06`) on the zone's master
    /// channel plus master and member Pitch Bend Sensitivity (RPN `00 00`).
    pub fn send_zone_setup(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
    ) -> Result<(), MpeOutputError> {
        self.check_activation(activation, producer)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        let master = self.config.zone.master_channel();
        let member_count = self.capacity() as u8;
        let first_member = self.first_member_channel();

        // 1. MPE Configuration Message on master channel: RPN 00 06, Data Entry MSB = member count.
        self.send_rpn_msb(activation, producer, ts, master, 0, 6, member_count)?;
        // 2. Master channel Pitch Bend Sensitivity: RPN 00 00, MSB = master bend semitones.
        self.send_rpn_msb(
            activation,
            producer,
            ts,
            master,
            0,
            0,
            self.config.master_pitch_bend_range_semitones,
        )?;
        // 3. Member zone Pitch Bend Sensitivity on first member channel: RPN 00 00.
        self.send_rpn_msb(
            activation,
            producer,
            ts,
            first_member,
            0,
            0,
            self.config.pitch_bend_range_semitones,
        )?;
        self.bound_activation = activation;
        Ok(())
    }

    /// Allocates a free MPE member channel for `note.occurrence`, resets member
    /// expression to `note.initial`, and emits `NoteOn` in strict timestamp order.
    ///
    /// Returns the allocated zero-based MIDI channel (`0..=15`) on success, or
    /// [`MpeOutputError::ChannelsExhausted`] if all member channels are busy.
    pub fn note_on(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        note: MpeNoteOn,
    ) -> Result<u8, MpeOutputError> {
        let MpeNoteOn {
            occurrence,
            pitch,
            velocity,
            initial,
        } = note;
        if occurrence.occurrence_id == 0 {
            return Err(MpeOutputError::InvalidOccurrence);
        }
        if pitch > 127 {
            return Err(MpeOutputError::InvalidPitch);
        }
        if velocity == 0 || velocity > 127 {
            return Err(MpeOutputError::InvalidVelocity);
        }
        if initial.timbre > 127 || initial.pressure > 127 {
            return Err(MpeOutputError::InvalidValue);
        }
        self.check_activation(activation, producer)?;
        if self.find_occurrence(occurrence).is_some() {
            return Err(MpeOutputError::DuplicateOccurrence);
        }
        let channel = self
            .select_lru_free_channel()
            .ok_or(MpeOutputError::ChannelsExhausted)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        let bend_u14 = cents_to_pitch_bend_u14(
            initial.pitch_bend_cents,
            self.config.pitch_bend_range_semitones,
        );

        // Reset member expression before NoteOn so reused channels start cleanly.
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::PitchBend {
                channel,
                value: bend_u14,
            },
        )?;
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::ControlChange {
                channel,
                cc: 74,
                value: initial.timbre,
            },
        )?;
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::ChannelPressure {
                channel,
                value: initial.pressure,
            },
        )?;
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::NoteOn {
                channel,
                pitch,
                velocity,
            },
        )?;

        self.slots[usize::from(channel)].active = Some(ActiveMemberNote { occurrence, pitch });
        self.active_count = self.active_count.saturating_add(1);
        self.bound_activation = activation;
        Ok(channel)
    }

    /// Sends per-note Pitch Bend (`cents`) to the member channel owning `occurrence`.
    pub fn send_note_pitch_bend(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        occurrence: MpeOutputOccurrence,
        cents: i16,
    ) -> Result<(), MpeOutputError> {
        self.check_activation(activation, producer)?;
        let channel = self
            .find_occurrence(occurrence)
            .ok_or(MpeOutputError::UnknownOccurrence)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        let value = cents_to_pitch_bend_u14(cents, self.config.pitch_bend_range_semitones);
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::PitchBend { channel, value },
        )
    }

    /// Sends per-note Pressure (`0..=127`) as member Channel Pressure for `occurrence`.
    pub fn send_note_pressure(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        occurrence: MpeOutputOccurrence,
        value: u8,
    ) -> Result<(), MpeOutputError> {
        if value > 127 {
            return Err(MpeOutputError::InvalidValue);
        }
        self.check_activation(activation, producer)?;
        let channel = self
            .find_occurrence(occurrence)
            .ok_or(MpeOutputError::UnknownOccurrence)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::ChannelPressure { channel, value },
        )
    }

    /// Sends per-note Slide / Timbre (`0..=127`, CC74) for `occurrence`.
    pub fn send_note_timbre(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        occurrence: MpeOutputOccurrence,
        value: u8,
    ) -> Result<(), MpeOutputError> {
        if value > 127 {
            return Err(MpeOutputError::InvalidValue);
        }
        self.check_activation(activation, producer)?;
        let channel = self
            .find_occurrence(occurrence)
            .ok_or(MpeOutputError::UnknownOccurrence)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::ControlChange {
                channel,
                cc: 74,
                value,
            },
        )
    }

    /// Releases the note owned by `occurrence` on its allocated member channel
    /// (using the output worker's priority NoteOff lane under saturation) and
    /// returns the freed channel for LRU reuse.
    pub fn note_off(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        occurrence: MpeOutputOccurrence,
        velocity: u8,
    ) -> Result<u8, MpeOutputError> {
        if velocity > 127 {
            return Err(MpeOutputError::InvalidVelocity);
        }
        let current = producer.active_activation();
        if current == 0 {
            self.clear_slots();
            return Err(MpeOutputError::Disconnected);
        }
        if activation == 0 || activation != current {
            return Err(MpeOutputError::StaleActivation);
        }
        let channel = self
            .find_occurrence(occurrence)
            .ok_or(MpeOutputError::UnknownOccurrence)?;
        let pitch = self.slots[usize::from(channel)]
            .active
            .map(|note| note.pitch)
            .ok_or(MpeOutputError::UnknownOccurrence)?;
        let ts = self.monotonic_timestamp(timestamp_ns);

        match producer.try_send(
            activation,
            TimestampedOutputMessage {
                timestamp_ns: ts,
                message: OutputMessage::NoteOff {
                    channel,
                    pitch,
                    velocity,
                },
            },
        ) {
            OutputEnqueueResult::Queued | OutputEnqueueResult::QueuedPriority => {
                self.release_slot(channel);
                Ok(channel)
            }
            OutputEnqueueResult::PriorityOverflowCleanupLatched => {
                // Worker will emit full active-note cleanup across all channels.
                self.clear_slots();
                Ok(channel)
            }
            OutputEnqueueResult::Disconnected => {
                self.clear_slots();
                Err(MpeOutputError::Disconnected)
            }
            OutputEnqueueResult::StaleActivation => Err(MpeOutputError::StaleActivation),
            OutputEnqueueResult::Overflowed => Err(MpeOutputError::QueueOverflow),
            OutputEnqueueResult::InvalidMessage(err) => Err(MpeOutputError::InvalidMessage(err)),
        }
    }

    /// Sends global pitch bend (`cents`) on the zone's master channel.
    pub fn send_master_pitch_bend(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        cents: i16,
    ) -> Result<(), MpeOutputError> {
        self.check_activation(activation, producer)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        let value = cents_to_pitch_bend_u14(cents, self.config.master_pitch_bend_range_semitones);
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::PitchBend {
                channel: self.config.zone.master_channel(),
                value,
            },
        )
    }

    /// Sends global Channel Pressure (`0..=127`) on the zone's master channel.
    pub fn send_master_pressure(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        value: u8,
    ) -> Result<(), MpeOutputError> {
        if value > 127 {
            return Err(MpeOutputError::InvalidValue);
        }
        self.check_activation(activation, producer)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::ChannelPressure {
                channel: self.config.zone.master_channel(),
                value,
            },
        )
    }

    /// Sends a global Control Change on the zone's master channel (e.g. Sustain CC64).
    pub fn send_master_cc(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        cc: u8,
        value: u8,
    ) -> Result<(), MpeOutputError> {
        if cc > 127 || value > 127 {
            return Err(MpeOutputError::InvalidValue);
        }
        self.check_activation(activation, producer)?;
        let ts = self.monotonic_timestamp(timestamp_ns);
        self.send_normal(
            activation,
            producer,
            ts,
            OutputMessage::ControlChange {
                channel: self.config.zone.master_channel(),
                cc,
                value,
            },
        )
    }

    /// Requests priority Stop on the existing [`OutputProducer`] and clears all
    /// local member-channel allocations so no stale occurrences persist.
    pub fn stop(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
    ) -> Result<(), MpeOutputError> {
        match producer.request_stop(activation) {
            OutputEnqueueResult::QueuedPriority | OutputEnqueueResult::Queued => {
                self.clear_slots();
                Ok(())
            }
            OutputEnqueueResult::Disconnected => {
                self.clear_slots();
                Err(MpeOutputError::Disconnected)
            }
            OutputEnqueueResult::StaleActivation => Err(MpeOutputError::StaleActivation),
            OutputEnqueueResult::Overflowed
            | OutputEnqueueResult::PriorityOverflowCleanupLatched => {
                self.clear_slots();
                Ok(())
            }
            OutputEnqueueResult::InvalidMessage(err) => Err(MpeOutputError::InvalidMessage(err)),
        }
    }

    fn first_member_channel(&self) -> u8 {
        match self.config.zone {
            MpeZone::Lower { .. } => 1,
            MpeZone::Upper { .. } => 14,
        }
    }

    fn member_channel_range(&self) -> (u8, u8) {
        match self.config.zone {
            MpeZone::Lower { member_channels } => (1, member_channels),
            MpeZone::Upper { member_channels } => (15 - member_channels, 14),
        }
    }

    fn find_occurrence(&self, occurrence: MpeOutputOccurrence) -> Option<u8> {
        let (start, end) = self.member_channel_range();
        for ch in start..=end {
            if let Some(note) = self.slots[usize::from(ch)].active
                && note.occurrence == occurrence
            {
                return Some(ch);
            }
        }
        None
    }

    fn select_lru_free_channel(&self) -> Option<u8> {
        let (start, end) = self.member_channel_range();
        let mut best: Option<(u8, u64)> = None;
        for ch in start..=end {
            let slot = self.slots[usize::from(ch)];
            if slot.active.is_none() {
                match best {
                    None => best = Some((ch, slot.last_released_seq)),
                    Some((_, best_seq)) if slot.last_released_seq < best_seq => {
                        best = Some((ch, slot.last_released_seq));
                    }
                    _ => {}
                }
            }
        }
        best.map(|(ch, _)| ch)
    }

    fn release_slot(&mut self, channel: u8) {
        let slot = &mut self.slots[usize::from(channel)];
        if slot.active.take().is_some() {
            self.active_count = self.active_count.saturating_sub(1);
            self.release_seq = self.release_seq.saturating_add(1);
            slot.last_released_seq = self.release_seq;
        }
    }

    fn clear_slots(&mut self) {
        for slot in &mut self.slots {
            if slot.active.take().is_some() {
                self.release_seq = self.release_seq.saturating_add(1);
                slot.last_released_seq = self.release_seq;
            }
        }
        self.active_count = 0;
    }

    fn monotonic_timestamp(&mut self, timestamp_ns: u64) -> u64 {
        if timestamp_ns == 0 {
            return 0;
        }
        if timestamp_ns > self.last_timestamp_ns {
            self.last_timestamp_ns = timestamp_ns;
        }
        self.last_timestamp_ns
    }

    fn check_activation(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
    ) -> Result<(), MpeOutputError> {
        let current = producer.active_activation();
        if current == 0 {
            self.clear_slots();
            return Err(MpeOutputError::Disconnected);
        }
        if activation == 0 || activation != current {
            return Err(MpeOutputError::StaleActivation);
        }
        if self.bound_activation != 0 && self.bound_activation != activation {
            // Reconnected to a fresh activation generation: start with fresh slots.
            self.reset_for_activation(activation);
        }
        Ok(())
    }

    fn send_normal(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        message: OutputMessage,
    ) -> Result<(), MpeOutputError> {
        match producer.try_send(
            activation,
            TimestampedOutputMessage {
                timestamp_ns,
                message,
            },
        ) {
            OutputEnqueueResult::Queued | OutputEnqueueResult::QueuedPriority => Ok(()),
            OutputEnqueueResult::Disconnected => {
                self.clear_slots();
                Err(MpeOutputError::Disconnected)
            }
            OutputEnqueueResult::StaleActivation => Err(MpeOutputError::StaleActivation),
            OutputEnqueueResult::Overflowed
            | OutputEnqueueResult::PriorityOverflowCleanupLatched => {
                Err(MpeOutputError::QueueOverflow)
            }
            OutputEnqueueResult::InvalidMessage(err) => Err(MpeOutputError::InvalidMessage(err)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_rpn_msb(
        &mut self,
        activation: u64,
        producer: &OutputProducer,
        timestamp_ns: u64,
        channel: u8,
        rpn_msb: u8,
        rpn_lsb: u8,
        data_msb: u8,
    ) -> Result<(), MpeOutputError> {
        for (cc, value) in [
            (101, rpn_msb),
            (100, rpn_lsb),
            (6, data_msb),
            (38, 0),
            (101, 127),
            (100, 127),
        ] {
            self.send_normal(
                activation,
                producer,
                timestamp_ns,
                OutputMessage::ControlChange { channel, cc, value },
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpe::{MpeDecoder, MpeExpressionEvent};
    use crate::output::{
        EncodedMidiMessage, OutputBackend, OutputCommand, OutputConnectionState, OutputEvent,
        OutputHandle, OutputPort, OutputSendError, OutputWorker,
    };
    use std::sync::{Arc, Condvar, Mutex, mpsc::TryRecvError};
    use std::time::{Duration, Instant};

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct WireEvent {
        port_id: u32,
        timestamp_ns: u64,
        bytes: Vec<u8>,
    }

    #[derive(Default)]
    struct LedgerState {
        ports: Vec<OutputPort>,
        events: Vec<WireEvent>,
        paused: bool,
        notifier: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    #[derive(Clone)]
    struct SyntheticMpeLedger {
        inner: Arc<(Mutex<LedgerState>, Condvar)>,
    }

    impl SyntheticMpeLedger {
        fn new(ports: Vec<OutputPort>) -> Self {
            Self {
                inner: Arc::new((
                    Mutex::new(LedgerState {
                        ports,
                        ..LedgerState::default()
                    }),
                    Condvar::new(),
                )),
            }
        }

        fn backend(&self) -> LedgerBackend {
            LedgerBackend {
                inner: Arc::clone(&self.inner),
            }
        }

        fn set_ports(&self, ports: Vec<OutputPort>) {
            let notifier = {
                let mut state = self.inner.0.lock().unwrap();
                state.ports = ports;
                state.notifier.clone()
            };
            if let Some(notify) = notifier {
                notify();
            }
        }

        fn set_paused(&self, paused: bool) {
            let mut state = self.inner.0.lock().unwrap();
            state.paused = paused;
            if !paused {
                self.inner.1.notify_all();
            }
        }

        fn wait_for(&self, predicate: impl Fn(&[WireEvent]) -> bool) -> Vec<WireEvent> {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut state = self.inner.0.lock().unwrap();
            while !predicate(&state.events) {
                let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                    break;
                };
                let (next, _) = self.inner.1.wait_timeout(state, rem).unwrap();
                state = next;
            }
            state.events.clone()
        }
    }

    struct LedgerBackend {
        inner: Arc<(Mutex<LedgerState>, Condvar)>,
    }

    impl OutputBackend for LedgerBackend {
        type Endpoint = u32;

        fn install_setup_notifier(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
            self.inner.0.lock().unwrap().notifier = Some(notify);
        }

        fn discover(&mut self) -> Result<Vec<OutputPort>, String> {
            Ok(self.inner.0.lock().unwrap().ports.clone())
        }

        fn connect(&mut self, port: &OutputPort) -> Result<Self::Endpoint, String> {
            let state = self.inner.0.lock().unwrap();
            if state.ports.iter().any(|p| p.id == port.id) {
                Ok(port.id)
            } else {
                Err("port unavailable".into())
            }
        }

        fn is_present(&mut self, endpoint: &Self::Endpoint) -> bool {
            self.inner
                .0
                .lock()
                .unwrap()
                .ports
                .iter()
                .any(|p| p.id == *endpoint)
        }

        fn send(
            &mut self,
            endpoint: &mut Self::Endpoint,
            timestamp_ns: u64,
            message: EncodedMidiMessage,
        ) -> Result<(), OutputSendError> {
            let mut state = self.inner.0.lock().unwrap();
            while state.paused {
                let (next, _) = self
                    .inner
                    .1
                    .wait_timeout(state, Duration::from_millis(5))
                    .unwrap();
                state = next;
            }
            if !state.ports.iter().any(|p| p.id == *endpoint) {
                return Err(OutputSendError::Disconnected("removed".into()));
            }
            state.events.push(WireEvent {
                port_id: *endpoint,
                timestamp_ns,
                bytes: message.as_slice().to_vec(),
            });
            self.inner.1.notify_all();
            Ok(())
        }

        fn close(&mut self, _endpoint: Self::Endpoint) {}
    }

    fn next_event(handle: &OutputHandle) -> OutputEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match handle.try_event() {
                Ok(event) => return event,
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("expected event: {error:?}"),
            }
        }
    }

    fn connect(handle: &OutputHandle, activation: u64, id: u32, name: &str) {
        let port = OutputPort {
            id,
            name: name.into(),
        };
        handle
            .send(OutputCommand::Connect {
                activation,
                port: port.clone(),
            })
            .unwrap();
        assert_eq!(
            next_event(handle),
            OutputEvent::Connection {
                activation,
                result: Ok(OutputConnectionState::Connected(port)),
            }
        );
    }

    #[test]
    fn simultaneous_same_pitch_notes_get_distinct_channels_and_independent_expression() {
        let ledger = SyntheticMpeLedger::new(vec![OutputPort {
            id: 42,
            name: "mpe synth".into(),
        }]);
        let mut worker = OutputWorker::start_with_backend(ledger.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();
        connect(&handle, 1, 42, "mpe synth");

        let config = MpeConfig::with_ranges(MpeZone::Lower { member_channels: 4 }, 48, 2).unwrap();
        let mut allocator = MpeOutputAllocator::new(config).unwrap();
        allocator.send_zone_setup(1, &producer, 100).unwrap();

        let occ_a = MpeOutputOccurrence {
            source_id: 1,
            occurrence_id: 10,
        };
        let occ_b = MpeOutputOccurrence {
            source_id: 1,
            occurrence_id: 20,
        };

        // Two overlapping notes on the EXACT SAME PITCH (60) must get distinct member channels.
        let ch_a = allocator
            .note_on(
                1,
                &producer,
                1_000,
                MpeNoteOn {
                    occurrence: occ_a,
                    pitch: 60,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        let ch_b = allocator
            .note_on(
                1,
                &producer,
                1_100,
                MpeNoteOn {
                    occurrence: occ_b,
                    pitch: 60,
                    velocity: 115,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        assert_ne!(ch_a, ch_b);
        assert_eq!((ch_a, ch_b), (1, 2));

        // Independent per-note pitch bend, pressure, and timbre on each occurrence.
        allocator
            .send_note_pitch_bend(1, &producer, 1_200, occ_a, 1200)
            .unwrap();
        allocator
            .send_note_pressure(1, &producer, 1_250, occ_a, 90)
            .unwrap();
        allocator
            .send_note_pitch_bend(1, &producer, 1_300, occ_b, -700)
            .unwrap();
        allocator
            .send_note_timbre(1, &producer, 1_350, occ_b, 110)
            .unwrap();

        // Release both occurrences independently.
        assert_eq!(
            allocator.note_off(1, &producer, 2_000, occ_a, 12).unwrap(),
            ch_a
        );
        assert_eq!(
            allocator.note_off(1, &producer, 2_100, occ_b, 34).unwrap(),
            ch_b
        );
        assert_eq!(allocator.active_count(), 0);

        // Late expression/off after release must be refused without hitting the wire.
        assert_eq!(
            allocator.send_note_pitch_bend(1, &producer, 2_200, occ_a, 500),
            Err(MpeOutputError::UnknownOccurrence)
        );
        assert_eq!(
            allocator.note_off(1, &producer, 2_300, occ_b, 0),
            Err(MpeOutputError::UnknownOccurrence)
        );

        // Wait for all emitted wire packets (18 RPN setup + 8 note_on/reset + 4 expr + 2 note_off = 32).
        let wire = ledger.wait_for(|e| e.len() >= 32);
        assert_eq!(wire.len(), 32);

        // Independently decode the wire stream through MpeDecoder and verify exact isolation!
        let mut decoder = MpeDecoder::new(config);
        let mut decoded = Vec::new();
        for pkt in &wire {
            assert_eq!(pkt.port_id, 42);
            if let Some(ev) = decoder.decode_message(&pkt.bytes) {
                decoded.push(ev);
            }
        }

        assert!(decoded.contains(&MpeExpressionEvent::NoteOn {
            channel: 1,
            pitch: 60,
            velocity: 100,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NoteOn {
            channel: 2,
            pitch: 60,
            velocity: 115,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NotePitchBend {
            channel: 1,
            pitch: 60,
            cents: 1200,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NotePressure {
            channel: 1,
            pitch: 60,
            value: 90,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NotePitchBend {
            channel: 2,
            pitch: 60,
            cents: -700,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NoteSlide {
            channel: 2,
            pitch: 60,
            value: 110,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NoteOff {
            channel: 1,
            pitch: 60,
            velocity: 12,
        }));
        assert!(decoded.contains(&MpeExpressionEvent::NoteOff {
            channel: 2,
            pitch: 60,
            velocity: 34,
        }));

        // Verify MCM RPN 00 06 -> 4 members on master channel 0.
        assert_eq!(wire[0].bytes, vec![0xb0, 101, 0]);
        assert_eq!(wire[1].bytes, vec![0xb0, 100, 6]);
        assert_eq!(wire[2].bytes, vec![0xb0, 6, 4]);
    }

    #[test]
    fn channel_exhaustion_refuses_silently_sharing_and_reuses_lru_with_neutral_reset() {
        let ledger = SyntheticMpeLedger::new(vec![OutputPort {
            id: 7,
            name: "two-voice".into(),
        }]);
        let mut worker = OutputWorker::start_with_backend(ledger.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();
        connect(&handle, 1, 7, "two-voice");

        let config = MpeConfig::with_ranges(MpeZone::Upper { member_channels: 2 }, 48, 2).unwrap();
        let mut allocator = MpeOutputAllocator::new(config).unwrap();
        assert_eq!(allocator.capacity(), 2);

        let o1 = MpeOutputOccurrence {
            source_id: 1,
            occurrence_id: 1,
        };
        let o2 = MpeOutputOccurrence {
            source_id: 1,
            occurrence_id: 2,
        };
        let o3 = MpeOutputOccurrence {
            source_id: 1,
            occurrence_id: 3,
        };

        // Upper zone with 2 members uses channels 13 and 14 (master = 15).
        let c1 = allocator
            .note_on(
                1,
                &producer,
                100,
                MpeNoteOn {
                    occurrence: o1,
                    pitch: 60,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        let c2 = allocator
            .note_on(
                1,
                &producer,
                200,
                MpeNoteOn {
                    occurrence: o2,
                    pitch: 64,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        assert_eq!((c1, c2), (13, 14));

        // Duplicate occurrence refused.
        assert_eq!(
            allocator.note_on(
                1,
                &producer,
                250,
                MpeNoteOn {
                    occurrence: o1,
                    pitch: 60,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            ),
            Err(MpeOutputError::DuplicateOccurrence)
        );
        // Third simultaneous note refused with ChannelsExhausted (never shares member!).
        assert_eq!(
            allocator.note_on(
                1,
                &producer,
                300,
                MpeNoteOn {
                    occurrence: o3,
                    pitch: 67,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            ),
            Err(MpeOutputError::ChannelsExhausted)
        );

        // Bend channel 13 heavily, release o1 then o2; LRU reuse picks o1's channel (13) first.
        allocator
            .send_note_pitch_bend(1, &producer, 350, o1, 2400)
            .unwrap();
        allocator.note_off(1, &producer, 400, o1, 0).unwrap();
        allocator.note_off(1, &producer, 450, o2, 0).unwrap();

        let c3 = allocator
            .note_on(
                1,
                &producer,
                500,
                MpeNoteOn {
                    occurrence: o3,
                    pitch: 72,
                    velocity: 95,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        assert_eq!(c3, 13);

        // Wait for all packets (4 + 4 + 1 + 1 + 1 + 4 = 15) and verify neutral reset precedes NoteOn(72).
        let wire = ledger.wait_for(|e| e.len() >= 15);
        let tail: Vec<Vec<u8>> = wire[11..15].iter().map(|w| w.bytes.clone()).collect();
        assert_eq!(
            tail,
            vec![
                vec![0xe0 | 13, 0, 64],  // PitchBend center (8192)
                vec![0xb0 | 13, 74, 64], // Neutral Timbre CC74 = 64
                vec![0xd0 | 13, 0],      // Neutral Channel Pressure = 0
                vec![0x90 | 13, 72, 95], // NoteOn pitch 72
            ]
        );
    }

    #[test]
    fn stale_activation_disconnect_reconnect_and_stop_leave_no_stuck_notes() {
        let ledger = SyntheticMpeLedger::new(vec![OutputPort {
            id: 99,
            name: "ext-mpe".into(),
        }]);
        let mut worker = OutputWorker::start_with_backend(ledger.backend()).unwrap();
        let handle = worker.handle();
        let producer = worker.take_producer().unwrap();
        connect(&handle, 1, 99, "ext-mpe");

        let config = MpeConfig::with_ranges(MpeZone::Lower { member_channels: 3 }, 48, 2).unwrap();
        let mut allocator = MpeOutputAllocator::new(config).unwrap();
        let occ = MpeOutputOccurrence {
            source_id: 2,
            occurrence_id: 1,
        };
        allocator
            .note_on(
                1,
                &producer,
                100,
                MpeNoteOn {
                    occurrence: occ,
                    pitch: 60,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        ledger.wait_for(|e| e.len() >= 4);

        // Stale activation refused without disturbing sounding note.
        assert_eq!(
            allocator.send_note_pressure(99, &producer, 150, occ, 50),
            Err(MpeOutputError::StaleActivation)
        );
        assert_eq!(allocator.active_count(), 1);

        // Priority Stop under queue pressure clears local slots and releases member channel 1.
        ledger.set_paused(true);
        let _ = allocator.send_master_cc(1, &producer, 160, 1, 10);
        std::thread::sleep(Duration::from_millis(10));
        allocator.stop(1, &producer).unwrap();
        assert_eq!(allocator.active_count(), 0);
        ledger.set_paused(false);

        let after_stop = ledger.wait_for(|e| e.iter().any(|w| w.bytes == [0xb1, 120, 0]));
        assert!(after_stop.iter().any(|w| w.bytes == [0x81, 60, 0]));
        assert!(after_stop.iter().any(|w| w.bytes == [0xb1, 123, 0]));
        assert!(after_stop.iter().any(|w| w.bytes == [0xb1, 120, 0]));

        // Device removal invalidates allocator cleanly; reconnect with fresh activation works.
        ledger.set_ports(Vec::new());
        assert!(matches!(
            next_event(&handle),
            OutputEvent::Connection {
                activation: 1,
                result: Err(_),
            }
        ));
        assert_eq!(
            allocator.note_on(
                1,
                &producer,
                200,
                MpeNoteOn {
                    occurrence: occ,
                    pitch: 62,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            ),
            Err(MpeOutputError::Disconnected)
        );

        ledger.set_ports(vec![OutputPort {
            id: 99,
            name: "ext-mpe".into(),
        }]);
        connect(&handle, 2, 99, "ext-mpe");
        allocator.reset_for_activation(2);
        let ch = allocator
            .note_on(
                2,
                &producer,
                300,
                MpeNoteOn {
                    occurrence: occ,
                    pitch: 62,
                    velocity: 100,
                    initial: MpeInitialExpression::default(),
                },
            )
            .unwrap();
        assert_eq!(ch, 1);
        let wire = ledger.wait_for(|e| e.iter().any(|w| w.bytes == [0x91, 62, 100]));
        assert!(wire.iter().any(|w| w.bytes == [0x91, 62, 100]));
    }

    #[test]
    fn cents_to_pitch_bend_round_trips_with_mpe_decoder() {
        for range in [2_u8, 12, 24, 48, 96] {
            assert_eq!(cents_to_pitch_bend_u14(0, range), PITCH_BEND_CENTER);
            assert_eq!(
                cents_to_pitch_bend_u14(i16::from(range) * 100, range),
                16_383
            );
            assert_eq!(cents_to_pitch_bend_u14(-i16::from(range) * 100, range), 0);
            for cents in [-1200_i16, -200, -50, 0, 50, 200, 1200] {
                if cents.abs() <= i16::from(range) * 100 {
                    let u14 = cents_to_pitch_bend_u14(cents, range);
                    let signed = i16::try_from(i32::from(u14) - 8192).unwrap();
                    let back = MpeDecoder::bend_to_cents_with_range(signed, range);
                    assert!(
                        (back - cents).abs() <= 2,
                        "range={range} cents={cents} u14={u14} back={back}"
                    );
                }
            }
        }
    }
}
