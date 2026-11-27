//! Bounded native External Instrument consumer combining selected hardware MIDI
//! output (through [`ExternalMidiSender`]) with selected hardware audio return
//! (through [`crate::hardware_send_return`]).
//!
//! # Contract
//! - Persists strict versioned device state ([`ExternalInstrumentState`],
//!   `type_id = "hex.external_instrument"`, `state_version = 1`) through
//!   [`hex_lattice_contract::NativeDeviceState`] for one-step `hex-project`
//!   Undo/Redo and lossless `hex-storage` save/reopen.
//! - Routes ordered block MIDI events (`frame_offset` converted to monotonic
//!   nanoseconds via [`frame_offset_to_timestamp_ns`]) to the existing control
//!   MIDI output owner without duplicating endpoint workers.
//! - Returns selected hardware audio channels into the prepared track buffer
//!   with explicit gain, safe monitoring, declared round-trip + manual PDC
//!   latency, independent clock policy, feedback-cycle refusal, and offline
//!   render refusal.
//! - Audio-callback execution performs zero heap allocations, zero locks, zero
//!   OS calls, and zero final drops; retired routes drop strictly on control
//!   via [`ExternalInstrumentHost::reclaim_retired`].

use crate::hardware_send_return::{
    ClockPolicy, HARDWARE_RETIREMENT_CAPACITY, HARDWARE_RETURN_RING_FRAMES, HARDWARE_SWAP_CAPACITY,
    HardwareAudioPortId, HardwareChannelSelection, HardwareDeviceCapabilities,
    HardwareLatencyPolicy, HardwareRouteError, MAX_DECLARED_LATENCY_FRAMES,
    MAX_HARDWARE_BLOCK_FRAMES, MAX_HARDWARE_UID_BYTES,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Canonical native device type identifier for External Instrument.
pub const EXTERNAL_INSTRUMENT_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("hex.external_instrument");
/// Current binary state schema version for [`ExternalInstrumentState`].
pub const EXTERNAL_INSTRUMENT_STATE_VERSION: u32 = 1;
/// Maximum byte length of a stored MIDI output port display name.
pub const MAX_MIDI_PORT_NAME_BYTES: usize = 1024;
/// Minimum allowed return gain in dB.
pub const MIN_RETURN_GAIN_DB: f32 = -36.0;
/// Maximum allowed return gain in dB.
pub const MAX_RETURN_GAIN_DB: f32 = 12.0;

/// Converts a block-relative `frame_offset` at `sample_rate_hz` into an
/// absolute monotonic nanosecond timestamp starting from `block_start_ns`.
///
/// Returns `None` if `sample_rate_hz == 0` or the result overflows `u64`.
pub fn frame_offset_to_timestamp_ns(
    block_start_ns: u64,
    frame_offset: usize,
    sample_rate_hz: u32,
) -> Option<u64> {
    if sample_rate_hz == 0 {
        return None;
    }
    let offset_ns = (u128::try_from(frame_offset).ok()?).checked_mul(1_000_000_000)?
        / u128::from(sample_rate_hz);
    let offset_u64 = u64::try_from(offset_ns).ok()?;
    block_start_ns.checked_add(offset_u64)
}

/// Ordered block-relative MIDI event forwarded to an external hardware synth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalMidiEvent {
    NoteOn {
        frame_offset: usize,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        frame_offset: usize,
        pitch: u8,
        velocity: u8,
    },
    ControlChange {
        frame_offset: usize,
        cc: u8,
        value: u8,
    },
    PitchBend {
        frame_offset: usize,
        value: u16,
    },
    ChannelPressure {
        frame_offset: usize,
        value: u8,
    },
    PolyPressure {
        frame_offset: usize,
        pitch: u8,
        value: u8,
    },
}

impl ExternalMidiEvent {
    pub const fn frame_offset(self) -> usize {
        match self {
            Self::NoteOn { frame_offset, .. }
            | Self::NoteOff { frame_offset, .. }
            | Self::ControlChange { frame_offset, .. }
            | Self::PitchBend { frame_offset, .. }
            | Self::ChannelPressure { frame_offset, .. }
            | Self::PolyPressure { frame_offset, .. } => frame_offset,
        }
    }
}

/// Wait-free result from [`ExternalMidiSender`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalMidiEnqueueStatus {
    Queued,
    Overflowed,
    StaleActivation,
    Disconnected,
    InvalidMessage,
}

/// Narrow wait-free, allocation-free bridge to the existing control-side
/// hardware MIDI output producer (`hex_midi::OutputProducer` in `hex-desktop`).
pub trait ExternalMidiSender {
    fn active_activation(&self) -> u64;

    fn try_send_event(
        &self,
        activation: u64,
        timestamp_ns: u64,
        channel: u8,
        event: ExternalMidiEvent,
    ) -> ExternalMidiEnqueueStatus;

    fn request_stop(&self, activation: u64) -> ExternalMidiEnqueueStatus;
}

/// Authored persistent state for one native External Instrument instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalInstrumentState {
    /// Stable platform MIDI output endpoint ID (`None` = inactive/unassigned).
    pub midi_port_id: Option<u32>,
    /// Display name metadata only; never used for endpoint selection.
    pub midi_port_name: String,
    /// Zero-based MIDI output channel (`0..=15`).
    pub midi_channel: u8,
    /// Stable CPAL audio return device UID (empty = unconfigured).
    pub return_port_uid: String,
    /// Explicit physical channel selection on the return device.
    pub return_channels: HardwareChannelSelection,
    /// Return level trim in dB (`-36..=+12`).
    pub return_gain_db: f32,
    /// Declared round-trip hardware latency and manual correction for PDC.
    pub latency_policy: HardwareLatencyPolicy,
    /// Explicit clock policy for hardware return buffering.
    pub clock_policy: ClockPolicy,
    /// Explicit safe monitoring gate for live audio return.
    pub monitoring_enabled: bool,
}

impl Default for ExternalInstrumentState {
    fn default() -> Self {
        Self {
            midi_port_id: None,
            midi_port_name: String::new(),
            midi_channel: 0,
            return_port_uid: String::new(),
            return_channels: HardwareChannelSelection::StereoPair {
                left_index: 0,
                right_index: 1,
            },
            return_gain_db: 0.0,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames: 0,
                manual_correction_frames: 0,
            },
            clock_policy: ClockPolicy::IndependentClock {
                max_drift_frames: 128,
            },
            monitoring_enabled: false,
        }
    }
}

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

fn write_str(out: &mut Vec<u8>, s: &str, limit: usize) -> Result<(), DeviceContractError> {
    if s.len() > limit {
        return Err(DeviceContractError::MalformedState);
    }
    let len = u32::try_from(s.len()).map_err(|_| DeviceContractError::MalformedState)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    Ok(())
}

fn read_str(cursor: &mut &[u8], limit: usize) -> Result<String, DeviceContractError> {
    if cursor.len() < 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let len = u32::from_le_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]) as usize;
    *cursor = &cursor[4..];
    if len > limit || cursor.len() < len {
        return Err(DeviceContractError::MalformedState);
    }
    let text = std::str::from_utf8(&cursor[..len])
        .map_err(|_| DeviceContractError::MalformedState)?
        .to_owned();
    *cursor = &cursor[len..];
    Ok(text)
}

impl ExternalInstrumentState {
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if self.midi_channel > 15 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if self.midi_port_name.len() > MAX_MIDI_PORT_NAME_BYTES
            || self.return_port_uid.len() > MAX_HARDWARE_UID_BYTES
        {
            return Err(DeviceContractError::MalformedState);
        }
        if matches!(
            self.return_channels,
            HardwareChannelSelection::StereoPair {
                left_index,
                right_index,
            } if left_index == right_index
        ) {
            return Err(DeviceContractError::MalformedState);
        }
        if !self.return_gain_db.is_finite()
            || !(MIN_RETURN_GAIN_DB..=MAX_RETURN_GAIN_DB).contains(&self.return_gain_db)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if self.latency_policy.effective_pdc_frames().is_err() {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        if let ClockPolicy::IndependentClock { max_drift_frames } = self.clock_policy
            && (max_drift_frames == 0 || max_drift_frames > MAX_DECLARED_LATENCY_FRAMES)
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut payload =
            Vec::with_capacity(48 + self.midi_port_name.len() + self.return_port_uid.len());
        match self.midi_port_id {
            None => {
                payload.push(0);
                payload.extend_from_slice(&0_u32.to_le_bytes());
            }
            Some(id) => {
                payload.push(1);
                payload.extend_from_slice(&id.to_le_bytes());
            }
        }
        write_str(&mut payload, &self.midi_port_name, MAX_MIDI_PORT_NAME_BYTES)?;
        payload.push(self.midi_channel);
        write_str(&mut payload, &self.return_port_uid, MAX_HARDWARE_UID_BYTES)?;
        match self.return_channels {
            HardwareChannelSelection::Mono { channel_index } => {
                payload.push(0);
                payload.extend_from_slice(&channel_index.to_le_bytes());
                payload.extend_from_slice(&0_u16.to_le_bytes());
            }
            HardwareChannelSelection::StereoPair {
                left_index,
                right_index,
            } => {
                payload.push(1);
                payload.extend_from_slice(&left_index.to_le_bytes());
                payload.extend_from_slice(&right_index.to_le_bytes());
            }
        }
        payload.extend_from_slice(&self.return_gain_db.to_le_bytes());
        payload.extend_from_slice(&self.latency_policy.declared_round_trip_frames.to_le_bytes());
        payload.extend_from_slice(&self.latency_policy.manual_correction_frames.to_le_bytes());
        match self.clock_policy {
            ClockPolicy::SharedClock => {
                payload.push(0);
                payload.extend_from_slice(&0_u32.to_le_bytes());
            }
            ClockPolicy::IndependentClock { max_drift_frames } => {
                payload.push(1);
                payload.extend_from_slice(&max_drift_frames.to_le_bytes());
            }
        }
        payload.push(u8::from(self.monitoring_enabled));
        NativeDeviceState::new(
            EXTERNAL_INSTRUMENT_TYPE_ID,
            EXTERNAL_INSTRUMENT_STATE_VERSION,
            payload,
        )
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != EXTERNAL_INSTRUMENT_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != EXTERNAL_INSTRUMENT_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let mut c = state.payload();
        if c.len() < 5 {
            return Err(DeviceContractError::MalformedState);
        }
        let has_midi = c[0];
        let raw_midi_id = u32::from_le_bytes([c[1], c[2], c[3], c[4]]);
        c = &c[5..];
        let midi_port_id = match has_midi {
            0 if raw_midi_id == 0 => None,
            1 => Some(raw_midi_id),
            _ => return Err(DeviceContractError::MalformedState),
        };
        let midi_port_name = read_str(&mut c, MAX_MIDI_PORT_NAME_BYTES)?;
        if c.is_empty() {
            return Err(DeviceContractError::MalformedState);
        }
        let midi_channel = c[0];
        c = &c[1..];
        let return_port_uid = read_str(&mut c, MAX_HARDWARE_UID_BYTES)?;
        if c.len() < 5 + 4 + 4 + 4 + 5 + 1 {
            return Err(DeviceContractError::MalformedState);
        }
        let ch_tag = c[0];
        let ch_a = u16::from_le_bytes([c[1], c[2]]);
        let ch_b = u16::from_le_bytes([c[3], c[4]]);
        c = &c[5..];
        let return_channels = match ch_tag {
            0 if ch_b == 0 => HardwareChannelSelection::Mono {
                channel_index: ch_a,
            },
            1 if ch_a != ch_b => HardwareChannelSelection::StereoPair {
                left_index: ch_a,
                right_index: ch_b,
            },
            _ => return Err(DeviceContractError::MalformedState),
        };
        let return_gain_db = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        let declared_round_trip_frames = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
        let manual_correction_frames = i32::from_le_bytes([c[8], c[9], c[10], c[11]]);
        let clock_tag = c[12];
        let clock_param = u32::from_le_bytes([c[13], c[14], c[15], c[16]]);
        let monitoring_byte = c[17];
        c = &c[18..];
        if !c.is_empty() || monitoring_byte > 1 {
            return Err(DeviceContractError::MalformedState);
        }
        let clock_policy = match clock_tag {
            0 if clock_param == 0 => ClockPolicy::SharedClock,
            1 => ClockPolicy::IndependentClock {
                max_drift_frames: clock_param,
            },
            _ => return Err(DeviceContractError::MalformedState),
        };
        let decoded = Self {
            midi_port_id,
            midi_port_name,
            midi_channel,
            return_port_uid,
            return_channels,
            return_gain_db,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames,
                manual_correction_frames,
            },
            clock_policy,
            monitoring_enabled: monitoring_byte != 0,
        };
        decoded.validate()?;
        Ok(decoded)
    }
}

/// Device-side return stream endpoint owned by the audio input backend.
pub struct ExternalInstrumentReturnEndpoint {
    pub return_tx: Producer<[f32; 2]>,
    pub state: Arc<ExternalInstrumentRuntimeState>,
    pub return_channels: HardwareChannelSelection,
    pub return_device_channels: u16,
    pub sample_rate_hz: u32,
}

impl ExternalInstrumentReturnEndpoint {
    /// Extracts selected physical return channels from an interleaved hardware
    /// input buffer into the bounded return ring (sanitizing non-finite samples).
    pub fn push_interleaved_return_buffer(&mut self, input: &[f32]) {
        let ch_count = usize::from(self.return_device_channels);
        if ch_count == 0 || self.state.is_stopped() || self.state.is_audio_disconnected() {
            return;
        }
        let frames = input.len() / ch_count;
        let mut overruns = 0_u64;
        for f in 0..frames {
            let base = f * ch_count;
            let stereo = match self.return_channels {
                HardwareChannelSelection::Mono { channel_index } => {
                    let s = input
                        .get(base + usize::from(channel_index))
                        .copied()
                        .unwrap_or(0.0);
                    let s = if s.is_finite() { s } else { 0.0 };
                    [s, s]
                }
                HardwareChannelSelection::StereoPair {
                    left_index,
                    right_index,
                } => {
                    let l = input
                        .get(base + usize::from(left_index))
                        .copied()
                        .unwrap_or(0.0);
                    let r = input
                        .get(base + usize::from(right_index))
                        .copied()
                        .unwrap_or(0.0);
                    [
                        if l.is_finite() { l } else { 0.0 },
                        if r.is_finite() { r } else { 0.0 },
                    ]
                }
            };
            if self.return_tx.push(stereo).is_err() {
                overruns = overruns.saturating_add(1);
            }
        }
        if overruns > 0 {
            self.state
                .return_overruns
                .fetch_add(overruns, Ordering::Relaxed);
        }
    }
}

/// Return-stream backend for [`ExternalInstrumentHost`].
pub trait ExternalInstrumentAudioBackend {
    type ReturnStream;

    fn resolve_return_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError>;

    fn open_return_stream(
        &mut self,
        port: &HardwareAudioPortId,
        endpoint: ExternalInstrumentReturnEndpoint,
    ) -> Result<Self::ReturnStream, HardwareRouteError>;
}

/// Shared atomic runtime flags for one prepared External Instrument route.
pub struct ExternalInstrumentRuntimeState {
    generation: u64,
    midi_activation: u64,
    effective_pdc_frames: u32,
    stop_requested: AtomicBool,
    audio_disconnected: AtomicBool,
    return_underruns: AtomicU64,
    return_overruns: AtomicU64,
}

impl ExternalInstrumentRuntimeState {
    fn new(generation: u64, midi_activation: u64, effective_pdc_frames: u32) -> Self {
        Self {
            generation,
            midi_activation,
            effective_pdc_frames,
            stop_requested: AtomicBool::new(false),
            audio_disconnected: AtomicBool::new(false),
            return_underruns: AtomicU64::new(0),
            return_overruns: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn midi_activation(&self) -> u64 {
        self.midi_activation
    }

    pub fn effective_pdc_frames(&self) -> u32 {
        self.effective_pdc_frames
    }

    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    pub fn mark_audio_disconnected(&self) {
        self.audio_disconnected.store(true, Ordering::Release);
    }

    pub fn is_audio_disconnected(&self) -> bool {
        self.audio_disconnected.load(Ordering::Acquire)
    }

    pub fn return_underruns(&self) -> u64 {
        self.return_underruns.load(Ordering::Relaxed)
    }

    pub fn return_overruns(&self) -> u64 {
        self.return_overruns.load(Ordering::Relaxed)
    }
}

/// Callback status reported by [`ExternalInstrumentCallback::process_block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalInstrumentStatus {
    Inactive,
    Active {
        generation: u64,
        effective_pdc_frames: u32,
        midi_sent: usize,
        midi_overflowed: usize,
        returned_frames: usize,
        underrun_frames: usize,
    },
    Stopped {
        generation: u64,
    },
    Disconnected {
        generation: u64,
    },
}

struct PreparedExternalInstrumentRoute {
    return_rx: Consumer<[f32; 2]>,
    state: Arc<ExternalInstrumentRuntimeState>,
    midi_channel: u8,
    sample_rate_hz: u32,
    max_block_frames: usize,
    return_gain: f32,
    monitoring_enabled: bool,
    stop_forwarded_to_midi: bool,
}

/// Real-time audio-callback processor for one External Instrument.
pub struct ExternalInstrumentCallback {
    active: Option<PreparedExternalInstrumentRoute>,
    publish_rx: Consumer<PreparedExternalInstrumentRoute>,
    retire_tx: Producer<PreparedExternalInstrumentRoute>,
}

impl ExternalInstrumentCallback {
    pub fn poll_publication(&mut self) {
        while let Ok(next) = self.publish_rx.pop() {
            if let Some(old) = self.active.replace(next) {
                let _ = self.retire_tx.push(old);
            }
        }
    }

    pub fn active_generation(&self) -> u64 {
        self.active.as_ref().map_or(0, |r| r.state.generation())
    }

    /// Dispatches ordered track MIDI events with exact nanosecond offsets to
    /// `midi_sender` and fills `output` with returned hardware audio.
    ///
    /// Zero heap allocations, zero locks, zero OS calls, zero final drops.
    pub fn process_block<M: ExternalMidiSender>(
        &mut self,
        block_start_ns: u64,
        midi_events: &[ExternalMidiEvent],
        output: &mut [[f32; 2]],
        midi_sender: &M,
    ) -> ExternalInstrumentStatus {
        self.poll_publication();
        output.fill([0.0, 0.0]);
        let Some(route) = self.active.as_mut() else {
            return ExternalInstrumentStatus::Inactive;
        };
        let generation = route.state.generation();
        if route.state.is_stopped() {
            if !route.stop_forwarded_to_midi {
                route.stop_forwarded_to_midi = true;
                let _ = midi_sender.request_stop(route.state.midi_activation());
            }
            return ExternalInstrumentStatus::Stopped { generation };
        }
        let midi_live = midi_sender.active_activation() == route.state.midi_activation()
            && route.state.midi_activation() != 0;
        if route.state.is_audio_disconnected() || !midi_live {
            return ExternalInstrumentStatus::Disconnected { generation };
        }

        let frames = output.len().min(route.max_block_frames);
        let mut midi_sent = 0_usize;
        let mut midi_overflowed = 0_usize;
        for &event in midi_events {
            let offset = event.frame_offset().min(frames.saturating_sub(1));
            let ts = frame_offset_to_timestamp_ns(block_start_ns, offset, route.sample_rate_hz)
                .unwrap_or(block_start_ns);
            match midi_sender.try_send_event(
                route.state.midi_activation(),
                ts,
                route.midi_channel,
                event,
            ) {
                ExternalMidiEnqueueStatus::Queued => midi_sent += 1,
                ExternalMidiEnqueueStatus::Overflowed => midi_overflowed += 1,
                ExternalMidiEnqueueStatus::Disconnected
                | ExternalMidiEnqueueStatus::StaleActivation => {
                    return ExternalInstrumentStatus::Disconnected { generation };
                }
                ExternalMidiEnqueueStatus::InvalidMessage => {}
            }
        }

        let mut returned_frames = 0_usize;
        let mut underrun_frames = 0_usize;
        let gain = if route.monitoring_enabled {
            route.return_gain
        } else {
            0.0
        };
        for dst in output.iter_mut().take(frames) {
            match route.return_rx.pop() {
                Ok(sample) => {
                    let l = if sample[0].is_finite() {
                        sample[0]
                    } else {
                        0.0
                    };
                    let r = if sample[1].is_finite() {
                        sample[1]
                    } else {
                        0.0
                    };
                    *dst = [l * gain, r * gain];
                    returned_frames += 1;
                }
                Err(_) => {
                    *dst = [0.0, 0.0];
                    underrun_frames += 1;
                }
            }
        }
        if underrun_frames > 0 {
            route
                .state
                .return_underruns
                .fetch_add(underrun_frames as u64, Ordering::Relaxed);
        }

        ExternalInstrumentStatus::Active {
            generation,
            effective_pdc_frames: route.state.effective_pdc_frames(),
            midi_sent,
            midi_overflowed,
            returned_frames,
            underrun_frames,
        }
    }

    pub fn retire_active_for_shutdown(&mut self) {
        self.poll_publication();
        if let Some(old) = self.active.take() {
            let _ = self.retire_tx.push(old);
        }
    }
}

/// Control-side host owning preparation, transactional publication, Stop, and
/// bounded reclamation for one External Instrument.
pub struct ExternalInstrumentHost<B: ExternalInstrumentAudioBackend> {
    backend: B,
    publish_tx: Producer<PreparedExternalInstrumentRoute>,
    retire_rx: Consumer<PreparedExternalInstrumentRoute>,
    committed_generation: u64,
    active_state: Option<Arc<ExternalInstrumentRuntimeState>>,
    active_stream: Option<B::ReturnStream>,
    retired_streams: Vec<B::ReturnStream>,
    in_flight_retirements: usize,
    authored: ExternalInstrumentState,
}

impl<B: ExternalInstrumentAudioBackend> ExternalInstrumentHost<B> {
    pub fn new(backend: B) -> (Self, ExternalInstrumentCallback) {
        let (publish_tx, publish_rx) = RingBuffer::new(HARDWARE_SWAP_CAPACITY);
        let (retire_tx, retire_rx) = RingBuffer::new(HARDWARE_RETIREMENT_CAPACITY);
        let host = Self {
            backend,
            publish_tx,
            retire_rx,
            committed_generation: 0,
            active_state: None,
            active_stream: None,
            retired_streams: Vec::with_capacity(HARDWARE_RETIREMENT_CAPACITY),
            in_flight_retirements: 0,
            authored: ExternalInstrumentState::default(),
        };
        let cb = ExternalInstrumentCallback {
            active: None,
            publish_rx,
            retire_tx,
        };
        (host, cb)
    }

    pub fn authored_state(&self) -> &ExternalInstrumentState {
        &self.authored
    }

    pub fn active_state(&self) -> Option<&Arc<ExternalInstrumentRuntimeState>> {
        self.active_state.as_ref()
    }

    pub fn active_stream_mut(&mut self) -> Option<&mut B::ReturnStream> {
        self.active_stream.as_mut()
    }

    pub fn request_stop(&self) {
        if let Some(state) = &self.active_state {
            state.request_stop();
        }
    }

    /// Prepares and transactionally publishes an External Instrument route.
    ///
    /// Refuses offline rendering, missing/unsupported MIDI or audio endpoints,
    /// loopback feedback cycles, and stale generations while leaving any active
    /// route and authored state completely untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_and_publish(
        &mut self,
        generation: u64,
        midi_activation: u64,
        state: &ExternalInstrumentState,
        sample_rate_hz: u32,
        max_block_frames: usize,
        main_output_port: Option<(HardwareAudioPortId, HardwareChannelSelection)>,
        offline_mode: bool,
    ) -> Result<u32, HardwareRouteError> {
        if offline_mode {
            return Err(HardwareRouteError::OfflineRenderUnsupported);
        }
        if generation == 0 || generation <= self.committed_generation {
            return Err(HardwareRouteError::StaleGeneration);
        }
        if midi_activation == 0 || state.midi_port_id.is_none() {
            return Err(HardwareRouteError::InvalidPortIdentity);
        }
        state
            .validate()
            .map_err(|_| HardwareRouteError::InvalidLatencyPolicy)?;
        if !(8_000..=192_000).contains(&sample_rate_hz) {
            return Err(HardwareRouteError::InvalidSampleRate);
        }
        if max_block_frames == 0 || max_block_frames > MAX_HARDWARE_BLOCK_FRAMES {
            return Err(HardwareRouteError::InvalidBlockSize);
        }
        if self.publish_tx.is_full() {
            return Err(HardwareRouteError::SwapQueueFull);
        }
        if self.active_state.is_some()
            && self.in_flight_retirements >= HARDWARE_RETIREMENT_CAPACITY - 1
        {
            return Err(HardwareRouteError::RetirementQueueFull);
        }

        let effective_pdc = state.latency_policy.effective_pdc_frames()?;
        let return_port = HardwareAudioPortId::new(state.return_port_uid.clone())?;
        let return_caps = self.backend.resolve_return_device(&return_port)?;
        if !return_caps.supports_sample_rate(sample_rate_hz) {
            return Err(HardwareRouteError::UnsupportedReturnSampleRate);
        }
        state
            .return_channels
            .validate_for_device_channels(return_caps.channels)?;
        if return_caps.is_software_loopback
            && let Some((main_port, main_channels)) = &main_output_port
            && main_port == &return_port
            && state.return_channels.overlaps(*main_channels)
        {
            return Err(HardwareRouteError::FeedbackCycleRisk(
                "external instrument return loops back from main output",
            ));
        }

        let (return_tx, return_rx) = RingBuffer::new(HARDWARE_RETURN_RING_FRAMES);
        let rt_state = Arc::new(ExternalInstrumentRuntimeState::new(
            generation,
            midi_activation,
            effective_pdc,
        ));
        let endpoint = ExternalInstrumentReturnEndpoint {
            return_tx,
            state: Arc::clone(&rt_state),
            return_channels: state.return_channels,
            return_device_channels: return_caps.channels,
            sample_rate_hz,
        };
        let stream = self.backend.open_return_stream(&return_port, endpoint)?;

        let callback_route = PreparedExternalInstrumentRoute {
            return_rx,
            state: Arc::clone(&rt_state),
            midi_channel: state.midi_channel,
            sample_rate_hz,
            max_block_frames,
            return_gain: db_to_linear(state.return_gain_db),
            monitoring_enabled: state.monitoring_enabled,
            stop_forwarded_to_midi: false,
        };
        self.publish_tx
            .push(callback_route)
            .map_err(|_| HardwareRouteError::SwapQueueFull)?;

        if let Some(old_stream) = self.active_stream.replace(stream) {
            self.retired_streams.push(old_stream);
            self.in_flight_retirements = self.in_flight_retirements.saturating_add(1);
        }
        self.active_state = Some(rt_state);
        self.committed_generation = generation;
        self.authored = state.clone();
        Ok(effective_pdc)
    }

    /// Reclaims retired callback routes and return streams strictly on control.
    pub fn reclaim_retired(&mut self) -> usize {
        let mut reclaimed = 0;
        while let Ok(old) = self.retire_rx.pop() {
            drop(old);
            if !self.retired_streams.is_empty() {
                let _ = self.retired_streams.remove(0);
            }
            self.in_flight_retirements = self.in_flight_retirements.saturating_sub(1);
            reclaimed += 1;
        }
        reclaimed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Editor, Project};

    #[test]
    fn offset_conversion_is_exact_and_guards_zero_rate_and_overflow() {
        assert_eq!(frame_offset_to_timestamp_ns(1_000, 0, 48_000), Some(1_000));
        // 48 frames at 48 kHz = exactly 1_000_000 ns (1 ms).
        assert_eq!(
            frame_offset_to_timestamp_ns(10_000_000, 48, 48_000),
            Some(11_000_000)
        );
        // 441 frames at 44.1 kHz = exactly 10_000_000 ns (10 ms).
        assert_eq!(
            frame_offset_to_timestamp_ns(0, 441, 44_100),
            Some(10_000_000)
        );
        assert_eq!(frame_offset_to_timestamp_ns(0, 10, 0), None);
        assert_eq!(frame_offset_to_timestamp_ns(u64::MAX, 48, 48_000), None);
    }

    #[test]
    fn external_instrument_state_round_trips_through_history_and_storage() {
        let initial = ExternalInstrumentState {
            midi_port_id: Some(404),
            midi_port_name: "prophet rev2".into(),
            midi_channel: 3,
            return_port_uid: "coreaudio:iface-in".into(),
            return_channels: HardwareChannelSelection::StereoPair {
                left_index: 2,
                right_index: 3,
            },
            return_gain_db: -3.5,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames: 256,
                manual_correction_frames: -16,
            },
            clock_policy: ClockPolicy::SharedClock,
            monitoring_enabled: true,
        };
        let edited = ExternalInstrumentState {
            midi_channel: 7,
            return_gain_db: 2.0,
            monitoring_enabled: false,
            ..initial.clone()
        };

        let project = Project::new(48_000).unwrap();
        let mut editor = Editor::new(project, 16).unwrap();
        editor
            .execute(Command::AddTrack {
                name: "ext synth".into(),
            })
            .unwrap();
        let track = editor.project().tracks()[0].id();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: initial.encode().unwrap(),
            })
            .unwrap();
        let device = editor.project().track(track).unwrap().devices()[0].id();
        editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: edited.encode().unwrap(),
            })
            .unwrap();

        let saved = hex_storage::encode(editor.project()).unwrap();
        let reopened = hex_storage::decode(&saved).unwrap();
        let reopened_state =
            ExternalInstrumentState::decode(reopened.track(track).unwrap().devices()[0].state())
                .unwrap();
        assert_eq!(reopened_state, edited);

        assert!(editor.undo());
        assert_eq!(
            ExternalInstrumentState::decode(
                editor.project().track(track).unwrap().devices()[0].state()
            )
            .unwrap(),
            initial
        );
        assert!(editor.redo());
        assert_eq!(
            ExternalInstrumentState::decode(
                editor.project().track(track).unwrap().devices()[0].state()
            )
            .unwrap(),
            edited
        );

        let future = NativeDeviceState::new(
            EXTERNAL_INSTRUMENT_TYPE_ID,
            EXTERNAL_INSTRUMENT_STATE_VERSION + 1,
            initial.encode().unwrap().payload().to_vec(),
        )
        .unwrap();
        assert_eq!(
            ExternalInstrumentState::decode(&future),
            Err(DeviceContractError::UnsupportedStateVersion)
        );
    }
}
