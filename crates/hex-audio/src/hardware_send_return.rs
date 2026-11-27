//! Bounded hardware audio send and return path with explicit port identity,
//! channel selection, independent clock/latency policy, feedback-cycle refusal,
//! and control-owned reclamation.
//!
//! # Contract
//! - Send and return endpoints are addressed strictly by stable host device UID
//!   ([`HardwareAudioPortId`]) and explicit physical channel indices
//!   ([`HardwareChannelSelection`]). Never assumes 48 kHz, stereo channels 0..1,
//!   shared send/return hardware clocks, or offline rendering when hardware is absent.
//! - Preparation, device discovery/validation, ring allocation, stream lifecycle,
//!   and final destruction run strictly on control ([`HardwareSendReturnHost`]).
//! - Audio callbacks interact only with [`HardwareSendReturnCallback`], using
//!   preallocated bounded `rtrb` SPSC rings and atomics: zero allocations, zero
//!   mutexes, zero OS/CPAL calls, and zero final drops inside callbacks.
//! - Failed, malformed, offline, cyclic, or stale-generation publications leave
//!   any currently active route and its audio completely intact.
//! - Stop and device disconnection remain effective immediately even when send
//!   and return rings are completely full.

use crate::input_devices::{InputDevice, InputDevices, input_devices_on_control};
use crate::output_devices::{OutputDevicesSnapshot, probe_output_devices_on_control};
use rtrb::{Consumer, Producer, RingBuffer};
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Maximum UTF-8 byte length of a validated hardware audio device UID.
pub const MAX_HARDWARE_UID_BYTES: usize = 4096;
/// Maximum supported block size in frames for one callback invocation.
pub const MAX_HARDWARE_BLOCK_FRAMES: usize = 4096;
/// Maximum declared or effective hardware round-trip PDC latency in frames.
pub const MAX_DECLARED_LATENCY_FRAMES: u32 = 48_000;
/// Bounded SPSC ring capacity (stereo frames) for outbound hardware send audio.
pub const HARDWARE_SEND_RING_FRAMES: usize = 8_192;
/// Bounded SPSC ring capacity (stereo frames) for inbound hardware return audio.
pub const HARDWARE_RETURN_RING_FRAMES: usize = 8_192;
/// Bounded capacity of the control -> callback publication queue.
pub const HARDWARE_SWAP_CAPACITY: usize = 4;
/// Bounded capacity of the callback -> control retirement queue.
pub const HARDWARE_RETIREMENT_CAPACITY: usize = 16;

/// Stable host-qualified hardware audio device identifier (CoreAudio UID on macOS).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HardwareAudioPortId(String);

impl HardwareAudioPortId {
    pub fn new(uid: impl Into<String>) -> Result<Self, HardwareRouteError> {
        let uid = uid.into();
        if uid.is_empty() || uid.len() > MAX_HARDWARE_UID_BYTES {
            return Err(HardwareRouteError::InvalidPortIdentity);
        }
        Ok(Self(uid))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit physical channel selection within a hardware audio endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HardwareChannelSelection {
    Mono { channel_index: u16 },
    StereoPair { left_index: u16, right_index: u16 },
}

impl HardwareChannelSelection {
    pub fn validate_for_device_channels(
        self,
        device_channels: u16,
    ) -> Result<(), HardwareRouteError> {
        if device_channels == 0 {
            return Err(HardwareRouteError::InvalidChannelSelection);
        }
        match self {
            Self::Mono { channel_index } => {
                if channel_index >= device_channels {
                    return Err(HardwareRouteError::InvalidChannelSelection);
                }
            }
            Self::StereoPair {
                left_index,
                right_index,
            } => {
                if left_index == right_index
                    || left_index >= device_channels
                    || right_index >= device_channels
                {
                    return Err(HardwareRouteError::InvalidChannelSelection);
                }
            }
        }
        Ok(())
    }

    pub fn overlaps(self, other: Self) -> bool {
        let contains = |sel: Self, ch: u16| match sel {
            Self::Mono { channel_index } => channel_index == ch,
            Self::StereoPair {
                left_index,
                right_index,
            } => left_index == ch || right_index == ch,
        };
        match self {
            Self::Mono { channel_index } => contains(other, channel_index),
            Self::StereoPair {
                left_index,
                right_index,
            } => contains(other, left_index) || contains(other, right_index),
        }
    }
}

/// Clock-domain relationship between the selected send and return devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockPolicy {
    /// Send and return share one hardware clock domain (e.g. same interface or
    /// hardware word-clock-synced aggregate device).
    SharedClock,
    /// Send and return run on independent device clocks; bounded drift headroom
    /// absorbs small jitter while excess slip is explicitly counted and muted.
    IndependentClock { max_drift_frames: u32 },
}

/// Explicit round-trip latency and manual correction policy used for PDC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HardwareLatencyPolicy {
    /// Declared hardware + converter + buffer round-trip latency in frames.
    pub declared_round_trip_frames: u32,
    /// Optional signed manual latency correction in frames.
    pub manual_correction_frames: i32,
}

impl HardwareLatencyPolicy {
    /// Computes validated nonnegative effective PDC frames (`0..=MAX_DECLARED_LATENCY_FRAMES`).
    pub fn effective_pdc_frames(self) -> Result<u32, HardwareRouteError> {
        if self.declared_round_trip_frames > MAX_DECLARED_LATENCY_FRAMES {
            return Err(HardwareRouteError::InvalidLatencyPolicy);
        }
        let adjusted = i64::from(self.declared_round_trip_frames)
            .checked_add(i64::from(self.manual_correction_frames))
            .ok_or(HardwareRouteError::InvalidLatencyPolicy)?;
        if adjusted < 0 || adjusted > i64::from(MAX_DECLARED_LATENCY_FRAMES) {
            return Err(HardwareRouteError::InvalidLatencyPolicy);
        }
        Ok(adjusted as u32)
    }
}

/// Complete validated configuration for one selected hardware send/return path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareSendReturnConfig {
    pub send_port: HardwareAudioPortId,
    pub send_channels: HardwareChannelSelection,
    pub return_port: HardwareAudioPortId,
    pub return_channels: HardwareChannelSelection,
    pub sample_rate_hz: u32,
    pub max_block_frames: usize,
    pub clock_policy: ClockPolicy,
    pub latency_policy: HardwareLatencyPolicy,
    /// Active main output UID and channels, used to refuse accidental feedback loops
    /// when send channels overlap the live main monitor outputs.
    pub main_output_port: Option<(HardwareAudioPortId, HardwareChannelSelection)>,
}

/// Capabilities of one discovered send or return audio device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HardwareDeviceCapabilities {
    pub port: HardwareAudioPortId,
    pub channels: u16,
    pub supported_sample_rates: Vec<u32>,
    /// True when the backend knows this return endpoint is a direct loopback alias
    /// of the same device's output (which would form an uncontrolled feedback cycle).
    pub is_software_loopback: bool,
}

impl HardwareDeviceCapabilities {
    pub fn supports_sample_rate(&self, rate_hz: u32) -> bool {
        self.supported_sample_rates.contains(&rate_hz)
    }
}

/// Deterministic refusal reasons for hardware send/return admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HardwareRouteError {
    InvalidPortIdentity,
    InvalidChannelSelection,
    InvalidSampleRate,
    InvalidBlockSize,
    InvalidClockPolicy,
    InvalidLatencyPolicy,
    OfflineRenderUnsupported,
    StaleGeneration,
    MissingSendDevice,
    MissingReturnDevice,
    UnsupportedSendSampleRate,
    UnsupportedReturnSampleRate,
    FeedbackCycleRisk(&'static str),
    RetirementQueueFull,
    SwapQueueFull,
    BackendOpenFailed(String),
}

impl fmt::Display for HardwareRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPortIdentity => f.write_str("invalid hardware audio port identity"),
            Self::InvalidChannelSelection => {
                f.write_str("selected hardware audio channels are out of range or duplicated")
            }
            Self::InvalidSampleRate => f.write_str("unsupported hardware audio sample rate"),
            Self::InvalidBlockSize => f.write_str("unsupported hardware audio block size"),
            Self::InvalidClockPolicy => f.write_str("invalid hardware audio clock policy"),
            Self::InvalidLatencyPolicy => f.write_str("invalid hardware audio latency policy"),
            Self::OfflineRenderUnsupported => {
                f.write_str("hardware audio send/return cannot run during offline rendering")
            }
            Self::StaleGeneration => f.write_str("stale hardware audio route generation"),
            Self::MissingSendDevice => f.write_str("selected hardware send device is unavailable"),
            Self::MissingReturnDevice => {
                f.write_str("selected hardware return device is unavailable")
            }
            Self::UnsupportedSendSampleRate => {
                f.write_str("selected hardware send device does not support the requested rate")
            }
            Self::UnsupportedReturnSampleRate => {
                f.write_str("selected hardware return device does not support the requested rate")
            }
            Self::FeedbackCycleRisk(reason) => {
                write!(f, "hardware feedback cycle refused: {reason}")
            }
            Self::RetirementQueueFull => {
                f.write_str("hardware audio retirement queue is full; reclaim retired routes first")
            }
            Self::SwapQueueFull => f.write_str("hardware audio swap queue is full"),
            Self::BackendOpenFailed(reason) => {
                write!(f, "hardware audio endpoint open failed: {reason}")
            }
        }
    }
}

impl std::error::Error for HardwareRouteError {}

/// Status reported by [`HardwareSendReturnCallback::process_block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareCallbackStatus {
    Inactive,
    Active {
        generation: u64,
        effective_pdc_frames: u32,
        returned_frames: usize,
        underrun_frames: usize,
        send_dropped_frames: usize,
    },
    Stopped {
        generation: u64,
    },
    Disconnected {
        generation: u64,
    },
}

/// Shared atomic control/telemetry state for one prepared hardware route.
pub struct HardwareRouteState {
    generation: u64,
    effective_pdc_frames: u32,
    stop_requested: AtomicBool,
    disconnected: AtomicBool,
    send_overruns: AtomicU64,
    return_underruns: AtomicU64,
    return_overruns: AtomicU64,
}

impl HardwareRouteState {
    fn new(generation: u64, effective_pdc_frames: u32) -> Self {
        Self {
            generation,
            effective_pdc_frames,
            stop_requested: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
            send_overruns: AtomicU64::new(0),
            return_underruns: AtomicU64::new(0),
            return_overruns: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn effective_pdc_frames(&self) -> u32 {
        self.effective_pdc_frames
    }

    /// Wait-free Stop signal; effective immediately on the next callback block
    /// even when send and return rings are 100% saturated.
    pub fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    /// Marks the hardware endpoint lost (e.g. device unplug or stream error).
    pub fn mark_disconnected(&self) {
        self.disconnected.store(true, Ordering::Release);
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    pub fn send_overruns(&self) -> u64 {
        self.send_overruns.load(Ordering::Relaxed)
    }

    pub fn return_underruns(&self) -> u64 {
        self.return_underruns.load(Ordering::Relaxed)
    }

    pub fn return_overruns(&self) -> u64 {
        self.return_overruns.load(Ordering::Relaxed)
    }
}

/// Device-side SPSC endpoints owned by the opened hardware backend streams.
pub struct HardwareStreamEndpoints {
    pub send_rx: Consumer<[f32; 2]>,
    pub return_tx: Producer<[f32; 2]>,
    pub state: Arc<HardwareRouteState>,
    pub send_channels: HardwareChannelSelection,
    pub return_channels: HardwareChannelSelection,
    pub send_device_channels: u16,
    pub return_device_channels: u16,
    pub sample_rate_hz: u32,
    pub clock_policy: ClockPolicy,
}

impl HardwareStreamEndpoints {
    /// Helper for hardware send callbacks: reads stereo frames from `send_rx`
    /// and writes them into the exact selected physical channels of an
    /// interleaved output buffer (zero-filling unselected channels and sanitizing
    /// non-finite samples). Writes pure silence when stopped or disconnected.
    pub fn fill_interleaved_send_buffer(&mut self, output: &mut [f32]) {
        output.fill(0.0);
        let ch_count = usize::from(self.send_device_channels);
        if ch_count == 0 || self.state.is_stopped() || self.state.is_disconnected() {
            return;
        }
        let frames = output.len() / ch_count;
        for frame_idx in 0..frames {
            let [l, r] = self.send_rx.pop().unwrap_or([0.0, 0.0]);
            let l = if l.is_finite() { l } else { 0.0 };
            let r = if r.is_finite() { r } else { 0.0 };
            let base = frame_idx * ch_count;
            match self.send_channels {
                HardwareChannelSelection::Mono { channel_index } => {
                    let idx = base + usize::from(channel_index);
                    if idx < output.len() {
                        output[idx] = 0.5 * (l + r);
                    }
                }
                HardwareChannelSelection::StereoPair {
                    left_index,
                    right_index,
                } => {
                    let li = base + usize::from(left_index);
                    let ri = base + usize::from(right_index);
                    if li < output.len() {
                        output[li] = l;
                    }
                    if ri < output.len() {
                        output[ri] = r;
                    }
                }
            }
        }
    }

    /// Helper for hardware return callbacks: extracts the exact selected physical
    /// channels from an interleaved input buffer and pushes sanitized stereo frames
    /// into `return_tx`. Counts bounded overruns without blocking or allocating.
    pub fn push_interleaved_return_buffer(&mut self, input: &[f32]) {
        let ch_count = usize::from(self.return_device_channels);
        if ch_count == 0 || self.state.is_stopped() || self.state.is_disconnected() {
            return;
        }
        let frames = input.len() / ch_count;
        let mut overruns: u64 = 0;
        for frame_idx in 0..frames {
            let base = frame_idx * ch_count;
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

/// Control-side hardware audio send/return backend trait.
///
/// Implemented by [`CpalHardwareAudioBackend`] for host discovery/validation and
/// by synthetic loopback fixtures for deterministic rate/channel/latency/fault tests.
pub trait HardwareAudioBackend {
    type OpenedStreams;

    fn resolve_send_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError>;

    fn resolve_return_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError>;

    fn open_streams(
        &mut self,
        config: &HardwareSendReturnConfig,
        endpoints: HardwareStreamEndpoints,
    ) -> Result<Self::OpenedStreams, HardwareRouteError>;
}

/// Control-side CPAL discovery/validation backend.
///
/// Resolves exact host-qualified output/input device UIDs on control without
/// opening live physical streams inside unit tests or audio callbacks.
pub struct CpalHardwareAudioBackend {
    outputs: Option<OutputDevicesSnapshot>,
    inputs: Option<InputDevices>,
}

impl Default for CpalHardwareAudioBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalHardwareAudioBackend {
    pub fn new() -> Self {
        Self {
            outputs: None,
            inputs: None,
        }
    }

    pub fn refresh_on_control(&mut self) -> Result<(), HardwareRouteError> {
        let outputs =
            probe_output_devices_on_control().map_err(HardwareRouteError::BackendOpenFailed)?;
        let inputs = input_devices_on_control()
            .map_err(|err| HardwareRouteError::BackendOpenFailed(err.to_string()))?;
        self.outputs = Some(outputs);
        self.inputs = Some(inputs);
        Ok(())
    }

    fn input_caps(device: &InputDevice) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
        let port = HardwareAudioPortId::new(device.id.to_string())?;
        let mut channels = 0_u16;
        let mut rates = Vec::new();
        for fmt in &device.formats {
            if fmt.sample_format == cpal::SampleFormat::F32 {
                channels = channels.max(fmt.channels);
                for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
                    if (fmt.min_sample_rate..=fmt.max_sample_rate).contains(&rate)
                        && !rates.contains(&rate)
                    {
                        rates.push(rate);
                    }
                }
            }
        }
        Ok(HardwareDeviceCapabilities {
            port,
            channels,
            supported_sample_rates: rates,
            is_software_loopback: false,
        })
    }
}

impl HardwareAudioBackend for CpalHardwareAudioBackend {
    type OpenedStreams = HardwareStreamEndpoints;

    fn resolve_send_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
        if self.outputs.is_none() {
            self.refresh_on_control()?;
        }
        let snapshot = self
            .outputs
            .as_ref()
            .ok_or(HardwareRouteError::MissingSendDevice)?;
        let info = snapshot
            .find_by_uid(port.as_str())
            .ok_or(HardwareRouteError::MissingSendDevice)?;
        Ok(HardwareDeviceCapabilities {
            port: HardwareAudioPortId::new(info.uid.clone())?,
            channels: info.channels,
            supported_sample_rates: info.supported_sample_rates.clone(),
            is_software_loopback: false,
        })
    }

    fn resolve_return_device(
        &mut self,
        port: &HardwareAudioPortId,
    ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
        if self.inputs.is_none() {
            self.refresh_on_control()?;
        }
        let inputs = self
            .inputs
            .as_ref()
            .ok_or(HardwareRouteError::MissingReturnDevice)?;
        let device = inputs
            .devices()
            .iter()
            .find(|dev| dev.id.to_string() == port.as_str())
            .ok_or(HardwareRouteError::MissingReturnDevice)?;
        Self::input_caps(device)
    }

    fn open_streams(
        &mut self,
        _config: &HardwareSendReturnConfig,
        endpoints: HardwareStreamEndpoints,
    ) -> Result<Self::OpenedStreams, HardwareRouteError> {
        // Returns the validated preallocated stream endpoints for the audio
        // lifecycle owner to attach to CPAL streams on control.
        Ok(endpoints)
    }
}

/// Callback-side preallocated send/return route (zero heap allocs/drops on callback).
pub struct PreparedCallbackRoute {
    send_tx: Producer<[f32; 2]>,
    return_rx: Consumer<[f32; 2]>,
    state: Arc<HardwareRouteState>,
    max_block_frames: usize,
}

/// Real-time audio-callback owner for one active hardware send/return path.
///
/// Receives newly prepared routes from control and hands replaced routes back
/// through a preallocated retirement ring so final drops never occur on callback.
pub struct HardwareSendReturnCallback {
    active: Option<PreparedCallbackRoute>,
    publish_rx: Consumer<PreparedCallbackRoute>,
    retire_tx: Producer<PreparedCallbackRoute>,
}

impl HardwareSendReturnCallback {
    /// Installs any newly published route from control, retiring the prior route
    /// into the control-owned reclamation ring without dropping it on callback.
    pub fn poll_publication(&mut self) {
        while let Ok(next) = self.publish_rx.pop() {
            if let Some(previous) = self.active.replace(next) {
                // Control enforces retirement queue headroom before publishing.
                let _ = self.retire_tx.push(previous);
            }
        }
    }

    /// Returns the currently active route generation (`0` if inactive).
    pub fn active_generation(&self) -> u64 {
        self.active
            .as_ref()
            .map_or(0, |route| route.state.generation())
    }

    /// Processes one audio block (`send_input` -> hardware send ring; hardware
    /// return ring -> `return_output`). Zero allocations, zero locks, zero waits.
    pub fn process_block(
        &mut self,
        send_input: &[[f32; 2]],
        return_output: &mut [[f32; 2]],
    ) -> HardwareCallbackStatus {
        self.poll_publication();
        return_output.fill([0.0, 0.0]);
        let Some(route) = self.active.as_mut() else {
            return HardwareCallbackStatus::Inactive;
        };
        let generation = route.state.generation();
        if route.state.is_disconnected() {
            return HardwareCallbackStatus::Disconnected { generation };
        }
        if route.state.is_stopped() {
            return HardwareCallbackStatus::Stopped { generation };
        }

        let frames = send_input
            .len()
            .min(return_output.len())
            .min(route.max_block_frames);
        let mut send_dropped = 0_usize;
        for &frame in send_input.iter().take(frames) {
            let sanitized = [
                if frame[0].is_finite() { frame[0] } else { 0.0 },
                if frame[1].is_finite() { frame[1] } else { 0.0 },
            ];
            if route.send_tx.push(sanitized).is_err() {
                send_dropped += 1;
            }
        }
        if send_dropped > 0 {
            route
                .state
                .send_overruns
                .fetch_add(send_dropped as u64, Ordering::Relaxed);
        }

        let mut returned_frames = 0_usize;
        let mut underrun_frames = 0_usize;
        for dst in return_output.iter_mut().take(frames) {
            match route.return_rx.pop() {
                Ok(sample) => {
                    *dst = [
                        if sample[0].is_finite() {
                            sample[0]
                        } else {
                            0.0
                        },
                        if sample[1].is_finite() {
                            sample[1]
                        } else {
                            0.0
                        },
                    ];
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

        HardwareCallbackStatus::Active {
            generation,
            effective_pdc_frames: route.state.effective_pdc_frames(),
            returned_frames,
            underrun_frames,
            send_dropped_frames: send_dropped,
        }
    }

    /// Hands any remaining active route back to control before callback teardown
    /// so final destruction happens strictly on control.
    pub fn retire_active_for_shutdown(&mut self) {
        self.poll_publication();
        if let Some(active) = self.active.take() {
            let _ = self.retire_tx.push(active);
        }
    }
}

/// Validates a hardware send/return configuration against resolved device
/// capabilities, enforcing rate, channel, clock, latency, offline, and
/// feedback-cycle safety invariants.
pub fn validate_hardware_send_return(
    config: &HardwareSendReturnConfig,
    send_caps: &HardwareDeviceCapabilities,
    return_caps: &HardwareDeviceCapabilities,
    offline_mode: bool,
) -> Result<u32, HardwareRouteError> {
    if offline_mode {
        return Err(HardwareRouteError::OfflineRenderUnsupported);
    }
    if !(8_000..=192_000).contains(&config.sample_rate_hz) {
        return Err(HardwareRouteError::InvalidSampleRate);
    }
    if config.max_block_frames == 0 || config.max_block_frames > MAX_HARDWARE_BLOCK_FRAMES {
        return Err(HardwareRouteError::InvalidBlockSize);
    }
    if let ClockPolicy::IndependentClock { max_drift_frames } = config.clock_policy
        && (max_drift_frames == 0 || max_drift_frames as usize >= HARDWARE_RETURN_RING_FRAMES)
    {
        return Err(HardwareRouteError::InvalidClockPolicy);
    }
    let effective_pdc = config.latency_policy.effective_pdc_frames()?;
    if !send_caps.supports_sample_rate(config.sample_rate_hz) {
        return Err(HardwareRouteError::UnsupportedSendSampleRate);
    }
    if !return_caps.supports_sample_rate(config.sample_rate_hz) {
        return Err(HardwareRouteError::UnsupportedReturnSampleRate);
    }
    config
        .send_channels
        .validate_for_device_channels(send_caps.channels)?;
    config
        .return_channels
        .validate_for_device_channels(return_caps.channels)?;

    if let Some((main_port, main_channels)) = &config.main_output_port
        && main_port == &config.send_port
        && config.send_channels.overlaps(*main_channels)
    {
        return Err(HardwareRouteError::FeedbackCycleRisk(
            "send channels overlap main output monitor channels",
        ));
    }
    if return_caps.is_software_loopback
        && config.send_port == config.return_port
        && config.send_channels.overlaps(config.return_channels)
    {
        return Err(HardwareRouteError::FeedbackCycleRisk(
            "return endpoint loops back directly from selected send channels",
        ));
    }
    Ok(effective_pdc)
}

/// Control-side owner for bounded hardware audio send/return preparation,
/// transactional publication, Stop/disconnect signaling, and bounded reclamation.
pub struct HardwareSendReturnHost<B: HardwareAudioBackend> {
    backend: B,
    publish_tx: Producer<PreparedCallbackRoute>,
    retire_rx: Consumer<PreparedCallbackRoute>,
    committed_generation: u64,
    active_state: Option<Arc<HardwareRouteState>>,
    active_streams: Option<B::OpenedStreams>,
    retired_streams: Vec<B::OpenedStreams>,
    in_flight_retirements: usize,
}

impl<B: HardwareAudioBackend> HardwareSendReturnHost<B> {
    pub fn new(backend: B) -> (Self, HardwareSendReturnCallback) {
        let (publish_tx, publish_rx) = RingBuffer::new(HARDWARE_SWAP_CAPACITY);
        let (retire_tx, retire_rx) = RingBuffer::new(HARDWARE_RETIREMENT_CAPACITY);
        let host = Self {
            backend,
            publish_tx,
            retire_rx,
            committed_generation: 0,
            active_state: None,
            active_streams: None,
            retired_streams: Vec::with_capacity(HARDWARE_RETIREMENT_CAPACITY),
            in_flight_retirements: 0,
        };
        let callback = HardwareSendReturnCallback {
            active: None,
            publish_rx,
            retire_tx,
        };
        (host, callback)
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn committed_generation(&self) -> u64 {
        self.committed_generation
    }

    pub fn active_state(&self) -> Option<&Arc<HardwareRouteState>> {
        self.active_state.as_ref()
    }

    pub fn active_streams_mut(&mut self) -> Option<&mut B::OpenedStreams> {
        self.active_streams.as_mut()
    }

    /// Wait-free Stop signal on the active route (safe even when rings are full).
    pub fn request_stop(&self) {
        if let Some(state) = &self.active_state {
            state.request_stop();
        }
    }

    /// Prepares and transactionally publishes a new hardware send/return route.
    ///
    /// On any validation error, stale generation, offline mode, missing device,
    /// feedback cycle, queue saturation, or backend open error, the existing
    /// active route and its audio remain completely untouched.
    pub fn prepare_and_publish(
        &mut self,
        generation: u64,
        config: &HardwareSendReturnConfig,
        offline_mode: bool,
    ) -> Result<u32, HardwareRouteError> {
        if generation == 0 || generation <= self.committed_generation {
            return Err(HardwareRouteError::StaleGeneration);
        }
        if self.publish_tx.is_full() {
            return Err(HardwareRouteError::SwapQueueFull);
        }
        if self.active_state.is_some()
            && self.in_flight_retirements >= HARDWARE_RETIREMENT_CAPACITY - 1
        {
            return Err(HardwareRouteError::RetirementQueueFull);
        }

        let send_caps = self.backend.resolve_send_device(&config.send_port)?;
        let return_caps = self.backend.resolve_return_device(&config.return_port)?;
        let effective_pdc =
            validate_hardware_send_return(config, &send_caps, &return_caps, offline_mode)?;

        let (send_tx, send_rx) = RingBuffer::new(HARDWARE_SEND_RING_FRAMES);
        let (return_tx, return_rx) = RingBuffer::new(HARDWARE_RETURN_RING_FRAMES);
        let state = Arc::new(HardwareRouteState::new(generation, effective_pdc));
        let endpoints = HardwareStreamEndpoints {
            send_rx,
            return_tx,
            state: Arc::clone(&state),
            send_channels: config.send_channels,
            return_channels: config.return_channels,
            send_device_channels: send_caps.channels,
            return_device_channels: return_caps.channels,
            sample_rate_hz: config.sample_rate_hz,
            clock_policy: config.clock_policy,
        };
        let opened = self.backend.open_streams(config, endpoints)?;

        let callback_route = PreparedCallbackRoute {
            send_tx,
            return_rx,
            state: Arc::clone(&state),
            max_block_frames: config.max_block_frames,
        };
        self.publish_tx
            .push(callback_route)
            .map_err(|_| HardwareRouteError::SwapQueueFull)?;

        if let Some(old_streams) = self.active_streams.replace(opened) {
            self.retired_streams.push(old_streams);
            self.in_flight_retirements = self.in_flight_retirements.saturating_add(1);
        }
        self.active_state = Some(state);
        self.committed_generation = generation;
        Ok(effective_pdc)
    }

    /// Reclaims retired callback routes and backend streams strictly on control.
    /// Returns the number of retired routes reclaimed.
    pub fn reclaim_retired(&mut self) -> usize {
        let mut reclaimed = 0;
        while let Ok(old_callback_route) = self.retire_rx.pop() {
            drop(old_callback_route);
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
    use std::collections::{BTreeMap, VecDeque};

    struct SyntheticLoopbackStreams {
        endpoints: HardwareStreamEndpoints,
        delay_line: VecDeque<Vec<f32>>,
        send_buf: Vec<f32>,
        return_buf: Vec<f32>,
        drop_counter: Arc<AtomicU64>,
    }

    impl Drop for SyntheticLoopbackStreams {
        fn drop(&mut self) {
            self.drop_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl SyntheticLoopbackStreams {
        /// Simulates one hardware I/O quantum: reads physical send channels,
        /// routes them through an exact `delay_frames` external loopback cable
        /// into the selected physical return channels, and feeds `return_tx`.
        fn pump_loopback_block(&mut self, frames: usize, return_frame_override: Option<usize>) {
            let send_ch = usize::from(self.endpoints.send_device_channels);
            let ret_ch = usize::from(self.endpoints.return_device_channels);
            self.send_buf.resize(frames * send_ch, 0.0);
            self.endpoints
                .fill_interleaved_send_buffer(&mut self.send_buf);

            for f in 0..frames {
                let mut ret_frame = vec![0.0_f32; ret_ch];
                let (l, r) = match self.endpoints.send_channels {
                    HardwareChannelSelection::Mono { channel_index } => {
                        let s = self.send_buf[f * send_ch + usize::from(channel_index)];
                        (s, s)
                    }
                    HardwareChannelSelection::StereoPair {
                        left_index,
                        right_index,
                    } => (
                        self.send_buf[f * send_ch + usize::from(left_index)],
                        self.send_buf[f * send_ch + usize::from(right_index)],
                    ),
                };
                match self.endpoints.return_channels {
                    HardwareChannelSelection::Mono { channel_index } => {
                        ret_frame[usize::from(channel_index)] = l;
                    }
                    HardwareChannelSelection::StereoPair {
                        left_index,
                        right_index,
                    } => {
                        ret_frame[usize::from(left_index)] = l;
                        ret_frame[usize::from(right_index)] = r;
                    }
                }
                self.delay_line.push_back(ret_frame);
            }

            let ret_frames = return_frame_override.unwrap_or(frames);
            self.return_buf.clear();
            self.return_buf.resize(ret_frames * ret_ch, 0.0);
            for f in 0..ret_frames {
                if let Some(frame) = self.delay_line.pop_front() {
                    self.return_buf[f * ret_ch..(f + 1) * ret_ch].copy_from_slice(&frame);
                }
            }
            self.endpoints
                .push_interleaved_return_buffer(&self.return_buf);
        }
    }

    struct SyntheticLoopbackBackend {
        sends: BTreeMap<String, HardwareDeviceCapabilities>,
        returns: BTreeMap<String, HardwareDeviceCapabilities>,
        delay_frames: usize,
        fail_open: bool,
        drop_counter: Arc<AtomicU64>,
    }

    impl SyntheticLoopbackBackend {
        fn new(delay_frames: usize, drop_counter: Arc<AtomicU64>) -> Self {
            Self {
                sends: BTreeMap::new(),
                returns: BTreeMap::new(),
                delay_frames,
                fail_open: false,
                drop_counter,
            }
        }

        fn add_send(&mut self, uid: &str, channels: u16, rates: &[u32]) {
            self.sends.insert(
                uid.to_owned(),
                HardwareDeviceCapabilities {
                    port: HardwareAudioPortId::new(uid).unwrap(),
                    channels,
                    supported_sample_rates: rates.to_vec(),
                    is_software_loopback: false,
                },
            );
        }

        fn add_return(&mut self, uid: &str, channels: u16, rates: &[u32], loopback: bool) {
            self.returns.insert(
                uid.to_owned(),
                HardwareDeviceCapabilities {
                    port: HardwareAudioPortId::new(uid).unwrap(),
                    channels,
                    supported_sample_rates: rates.to_vec(),
                    is_software_loopback: loopback,
                },
            );
        }
    }

    impl HardwareAudioBackend for SyntheticLoopbackBackend {
        type OpenedStreams = SyntheticLoopbackStreams;

        fn resolve_send_device(
            &mut self,
            port: &HardwareAudioPortId,
        ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
            self.sends
                .get(port.as_str())
                .cloned()
                .ok_or(HardwareRouteError::MissingSendDevice)
        }

        fn resolve_return_device(
            &mut self,
            port: &HardwareAudioPortId,
        ) -> Result<HardwareDeviceCapabilities, HardwareRouteError> {
            self.returns
                .get(port.as_str())
                .cloned()
                .ok_or(HardwareRouteError::MissingReturnDevice)
        }

        fn open_streams(
            &mut self,
            _config: &HardwareSendReturnConfig,
            endpoints: HardwareStreamEndpoints,
        ) -> Result<Self::OpenedStreams, HardwareRouteError> {
            if self.fail_open {
                return Err(HardwareRouteError::BackendOpenFailed(
                    "synthetic open failure".into(),
                ));
            }
            let ret_ch = usize::from(endpoints.return_device_channels);
            let mut delay_line = VecDeque::with_capacity(self.delay_frames + 256);
            for _ in 0..self.delay_frames {
                delay_line.push_back(vec![0.0_f32; ret_ch]);
            }
            Ok(SyntheticLoopbackStreams {
                endpoints,
                delay_line,
                send_buf: Vec::new(),
                return_buf: Vec::new(),
                drop_counter: Arc::clone(&self.drop_counter),
            })
        }
    }

    #[test]
    fn synthetic_loopback_impulse_verifies_explicit_latency_rates_and_non_default_channels() {
        let cases = [
            // 44.1 kHz mono non-default channels (out ch 3 -> in ch 2), 64-frame loopback
            (
                44_100_u32,
                8_u16,
                6_u16,
                HardwareChannelSelection::Mono { channel_index: 3 },
                HardwareChannelSelection::Mono { channel_index: 2 },
                64_usize,
                16_i32,
                [0.8_f32, 0.4_f32],
                [0.6_f32, 0.6_f32],
            ),
            // 48 kHz stereo non-default pair (out ch 2,3 -> in ch 4,5), 192-frame loopback
            (
                48_000,
                8,
                8,
                HardwareChannelSelection::StereoPair {
                    left_index: 2,
                    right_index: 3,
                },
                HardwareChannelSelection::StereoPair {
                    left_index: 4,
                    right_index: 5,
                },
                192,
                -12,
                [1.0, -0.5],
                [1.0, -0.5],
            ),
            // 96 kHz stereo (out ch 6,7 -> in ch 0,1), 320-frame loopback
            (
                96_000,
                8,
                4,
                HardwareChannelSelection::StereoPair {
                    left_index: 6,
                    right_index: 7,
                },
                HardwareChannelSelection::StereoPair {
                    left_index: 0,
                    right_index: 1,
                },
                320,
                0,
                [-0.75, 0.25],
                [-0.75, 0.25],
            ),
        ];

        for (rate, send_ch, ret_ch, send_sel, ret_sel, delay, correction, impulse, expected) in
            cases
        {
            let drops = Arc::new(AtomicU64::new(0));
            let mut backend = SyntheticLoopbackBackend::new(delay, Arc::clone(&drops));
            backend.add_send("coreaudio:iface-out", send_ch, &[rate]);
            backend.add_return("coreaudio:iface-in", ret_ch, &[rate], false);
            let (mut host, mut cb) = HardwareSendReturnHost::new(backend);

            let block = 64_usize;
            let config = HardwareSendReturnConfig {
                send_port: HardwareAudioPortId::new("coreaudio:iface-out").unwrap(),
                send_channels: send_sel,
                return_port: HardwareAudioPortId::new("coreaudio:iface-in").unwrap(),
                return_channels: ret_sel,
                sample_rate_hz: rate,
                max_block_frames: block,
                clock_policy: ClockPolicy::SharedClock,
                latency_policy: HardwareLatencyPolicy {
                    declared_round_trip_frames: (delay + block) as u32,
                    manual_correction_frames: correction,
                },
                main_output_port: Some((
                    HardwareAudioPortId::new("coreaudio:iface-out").unwrap(),
                    HardwareChannelSelection::StereoPair {
                        left_index: 0,
                        right_index: 1,
                    },
                )),
            };
            let pdc = host.prepare_and_publish(1, &config, false).unwrap();
            assert_eq!(pdc, ((delay + block) as i32 + correction) as u32);

            let mut recorded = Vec::new();
            for block_idx in 0..10 {
                let mut input = vec![[0.0_f32; 2]; block];
                if block_idx == 0 {
                    input[0] = impulse;
                }
                let mut output = vec![[0.0_f32; 2]; block];
                let status = cb.process_block(&input, &mut output);
                assert!(matches!(
                    status,
                    HardwareCallbackStatus::Active {
                        generation: 1,
                        effective_pdc_frames,
                        ..
                    } if effective_pdc_frames == pdc
                ));
                recorded.extend_from_slice(&output);
                host.active_streams_mut()
                    .unwrap()
                    .pump_loopback_block(block, None);
            }

            // One callback block separates send push from pump_loopback_block + next return pop.
            let expected_index = block + delay;
            assert_eq!(
                recorded[expected_index], expected,
                "rate={rate} delay={delay} expected impulse at frame {expected_index}"
            );
            for (idx, &frame) in recorded.iter().enumerate() {
                if idx != expected_index {
                    assert_eq!(frame, [0.0, 0.0], "unexpected energy at frame {idx}");
                }
            }
        }
    }

    #[test]
    fn offline_missing_device_cycle_and_stale_refusals_retain_old_active_route() {
        let drops = Arc::new(AtomicU64::new(0));
        let mut backend = SyntheticLoopbackBackend::new(64, Arc::clone(&drops));
        backend.add_send("coreaudio:iface", 8, &[48_000]);
        backend.add_return("coreaudio:iface", 8, &[48_000], false);
        backend.add_return("coreaudio:loopback", 4, &[48_000], true);
        backend.add_send("coreaudio:loopback", 4, &[48_000]);
        let (mut host, mut cb) = HardwareSendReturnHost::new(backend);

        let good = HardwareSendReturnConfig {
            send_port: HardwareAudioPortId::new("coreaudio:iface").unwrap(),
            send_channels: HardwareChannelSelection::StereoPair {
                left_index: 2,
                right_index: 3,
            },
            return_port: HardwareAudioPortId::new("coreaudio:iface").unwrap(),
            return_channels: HardwareChannelSelection::StereoPair {
                left_index: 4,
                right_index: 5,
            },
            sample_rate_hz: 48_000,
            max_block_frames: 64,
            clock_policy: ClockPolicy::SharedClock,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames: 128,
                manual_correction_frames: 0,
            },
            main_output_port: Some((
                HardwareAudioPortId::new("coreaudio:iface").unwrap(),
                HardwareChannelSelection::StereoPair {
                    left_index: 0,
                    right_index: 1,
                },
            )),
        };
        host.prepare_and_publish(1, &good, false).unwrap();

        // 1. Offline mode refused.
        assert_eq!(
            host.prepare_and_publish(2, &good, true),
            Err(HardwareRouteError::OfflineRenderUnsupported)
        );
        // 2. Stale / zero generation refused.
        assert_eq!(
            host.prepare_and_publish(0, &good, false),
            Err(HardwareRouteError::StaleGeneration)
        );
        assert_eq!(
            host.prepare_and_publish(1, &good, false),
            Err(HardwareRouteError::StaleGeneration)
        );
        // 3. Missing send & return devices refused.
        let mut missing_send = good.clone();
        missing_send.send_port = HardwareAudioPortId::new("coreaudio:missing").unwrap();
        assert_eq!(
            host.prepare_and_publish(2, &missing_send, false),
            Err(HardwareRouteError::MissingSendDevice)
        );
        let mut missing_return = good.clone();
        missing_return.return_port = HardwareAudioPortId::new("coreaudio:missing").unwrap();
        assert_eq!(
            host.prepare_and_publish(2, &missing_return, false),
            Err(HardwareRouteError::MissingReturnDevice)
        );
        // 4. Unsupported sample rate refused.
        let mut bad_rate = good.clone();
        bad_rate.sample_rate_hz = 96_000;
        assert_eq!(
            host.prepare_and_publish(2, &bad_rate, false),
            Err(HardwareRouteError::UnsupportedSendSampleRate)
        );
        // 5. Feedback cycle (main output overlap and software loopback alias) refused.
        let mut main_overlap = good.clone();
        main_overlap.send_channels = HardwareChannelSelection::StereoPair {
            left_index: 1,
            right_index: 2,
        };
        assert!(matches!(
            host.prepare_and_publish(2, &main_overlap, false),
            Err(HardwareRouteError::FeedbackCycleRisk(_))
        ));
        let mut loopback_cycle = good.clone();
        loopback_cycle.send_port = HardwareAudioPortId::new("coreaudio:loopback").unwrap();
        loopback_cycle.return_port = HardwareAudioPortId::new("coreaudio:loopback").unwrap();
        loopback_cycle.send_channels = HardwareChannelSelection::StereoPair {
            left_index: 2,
            right_index: 3,
        };
        loopback_cycle.return_channels = HardwareChannelSelection::StereoPair {
            left_index: 2,
            right_index: 3,
        };
        assert!(matches!(
            host.prepare_and_publish(2, &loopback_cycle, false),
            Err(HardwareRouteError::FeedbackCycleRisk(_))
        ));
        // 6. Backend open failure retains generation 1 untouched.
        host.backend_mut().fail_open = true;
        assert!(matches!(
            host.prepare_and_publish(2, &good, false),
            Err(HardwareRouteError::BackendOpenFailed(_))
        ));
        host.backend_mut().fail_open = false;

        // Generation 1 is still active and passes audio cleanly!
        assert_eq!(host.committed_generation(), 1);
        let mut out = [[0.0_f32; 2]; 64];
        let mut imp = [[0.0_f32; 2]; 64];
        imp[0] = [0.5, -0.25];
        let status = cb.process_block(&imp, &mut out);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active { generation: 1, .. }
        ));
    }

    #[test]
    fn full_buffer_stop_disconnect_clock_drift_and_control_reclamation() {
        let drops = Arc::new(AtomicU64::new(0));
        let mut backend = SyntheticLoopbackBackend::new(0, Arc::clone(&drops));
        backend.add_send("coreaudio:a", 4, &[48_000]);
        backend.add_return("coreaudio:b", 4, &[48_000], false);
        let (mut host, mut cb) = HardwareSendReturnHost::new(backend);

        let config = HardwareSendReturnConfig {
            send_port: HardwareAudioPortId::new("coreaudio:a").unwrap(),
            send_channels: HardwareChannelSelection::StereoPair {
                left_index: 2,
                right_index: 3,
            },
            return_port: HardwareAudioPortId::new("coreaudio:b").unwrap(),
            return_channels: HardwareChannelSelection::StereoPair {
                left_index: 0,
                right_index: 1,
            },
            sample_rate_hz: 48_000,
            max_block_frames: 256,
            clock_policy: ClockPolicy::IndependentClock {
                max_drift_frames: 128,
            },
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames: 256,
                manual_correction_frames: 0,
            },
            main_output_port: None,
        };
        host.prepare_and_publish(1, &config, false).unwrap();

        // Saturate send ring completely without draining, then trigger Stop.
        let loud = [[0.9_f32; 2]; 256];
        let mut out = [[0.0_f32; 2]; 256];
        for _ in 0..40 {
            cb.process_block(&loud, &mut out);
        }
        assert!(host.active_state().unwrap().send_overruns() > 0);
        assert!(host.active_state().unwrap().return_underruns() > 0);

        host.request_stop();
        let status = cb.process_block(&loud, &mut out);
        assert_eq!(status, HardwareCallbackStatus::Stopped { generation: 1 });
        assert!(out.iter().all(|&f| f == [0.0, 0.0]));

        // Publish generation 2; generation 1 must NOT drop until control calls reclaim_retired().
        host.prepare_and_publish(2, &config, false).unwrap();
        let status = cb.process_block(&loud, &mut out);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active { generation: 2, .. }
        ));
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        assert_eq!(host.reclaim_retired(), 1);
        assert_eq!(drops.load(Ordering::Relaxed), 1);

        // Independent clock drift: return delivers fewer frames than requested -> bounded underrun + silence.
        host.active_streams_mut()
            .unwrap()
            .pump_loopback_block(256, Some(128));
        let status = cb.process_block(&loud, &mut out);
        assert!(matches!(
            status,
            HardwareCallbackStatus::Active {
                generation: 2,
                returned_frames: 128,
                underrun_frames: 128,
                ..
            }
        ));
        assert!(out[128..].iter().all(|&f| f == [0.0, 0.0]));

        // Device removal / disconnection silences callback immediately.
        host.active_state().unwrap().mark_disconnected();
        let status = cb.process_block(&loud, &mut out);
        assert_eq!(
            status,
            HardwareCallbackStatus::Disconnected { generation: 2 }
        );
        assert!(out.iter().all(|&f| f == [0.0, 0.0]));

        cb.retire_active_for_shutdown();
        assert_eq!(host.reclaim_retired(), 1);
    }
}
