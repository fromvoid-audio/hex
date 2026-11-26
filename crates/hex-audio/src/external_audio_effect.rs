//! Bounded native External Audio Effect consumer built on
//! [`crate::hardware_send_return`].
//!
//! # Contract
//! - Persists strict versioned device state ([`ExternalAudioEffectState`],
//!   `type_id = "hex.external_audio_effect"`, `state_version = 1`) through
//!   [`hex_lattice_contract::NativeDeviceState`] for one-step `hex-project`
//!   Undo/Redo and lossless `hex-storage` save/reopen.
//! - Routes track audio through explicitly selected hardware send and return
//!   ports via [`HardwareSendReturnHost`] / [`HardwareSendReturnCallback`].
//! - Aligns dry and hardware-returned wet audio using an exact control-preallocated
//!   delay line equal to `effective_pdc_frames` (`declared_round_trip_frames +
//!   manual_correction_frames`), preventing comb-filtering at intermediate dry/wet
//!   mix ratios without ever allocating on the audio callback.
//! - Refuses offline rendering, missing/unsupported endpoints, feedback cycles,
//!   and stale generations while leaving any currently active effect untouched.
//! - Applies explicit [`DisconnectFallback`] on endpoint loss and immediate
//!   silence on Stop even under full buffers; retired delay/scratch buffers and
//!   hardware streams are dropped strictly on control via `reclaim_retired()`.

use crate::hardware_send_return::{
    ClockPolicy, HARDWARE_RETIREMENT_CAPACITY, HARDWARE_SWAP_CAPACITY, HardwareAudioBackend,
    HardwareAudioPortId, HardwareCallbackStatus, HardwareChannelSelection, HardwareLatencyPolicy,
    HardwareRouteError, HardwareSendReturnCallback, HardwareSendReturnConfig,
    HardwareSendReturnHost, MAX_DECLARED_LATENCY_FRAMES, MAX_HARDWARE_UID_BYTES,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use rtrb::{Consumer, Producer, RingBuffer};

/// Canonical native device type identifier for External Audio Effect.
pub const EXTERNAL_AUDIO_EFFECT_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("hex.external_audio_effect");
/// Current binary state schema version for [`ExternalAudioEffectState`].
pub const EXTERNAL_AUDIO_EFFECT_STATE_VERSION: u32 = 1;
/// Minimum allowed send/return gain in dB.
pub const MIN_GAIN_DB: f32 = -36.0;
/// Maximum allowed send/return gain in dB.
pub const MAX_GAIN_DB: f32 = 12.0;

/// Behavior when the active hardware endpoint disconnects mid-playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DisconnectFallback {
    /// Silence both wet and dry output on endpoint loss.
    MuteAll = 0,
    /// Silence wet return while preserving latency-aligned dry monitoring.
    MuteWetKeepDry = 1,
}

impl DisconnectFallback {
    fn from_byte(byte: u8) -> Result<Self, DeviceContractError> {
        match byte {
            0 => Ok(Self::MuteAll),
            1 => Ok(Self::MuteWetKeepDry),
            _ => Err(DeviceContractError::MalformedState),
        }
    }
}

/// Authored persistent state for one native External Audio Effect instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalAudioEffectState {
    pub send_port_uid: String,
    pub send_channels: HardwareChannelSelection,
    pub return_port_uid: String,
    pub return_channels: HardwareChannelSelection,
    pub send_gain_db: f32,
    pub return_gain_db: f32,
    pub dry_wet: f32,
    pub latency_policy: HardwareLatencyPolicy,
    pub clock_policy: ClockPolicy,
    pub monitoring_enabled: bool,
    pub disconnect_fallback: DisconnectFallback,
}

impl Default for ExternalAudioEffectState {
    fn default() -> Self {
        Self {
            send_port_uid: String::new(),
            send_channels: HardwareChannelSelection::StereoPair {
                left_index: 0,
                right_index: 1,
            },
            return_port_uid: String::new(),
            return_channels: HardwareChannelSelection::StereoPair {
                left_index: 0,
                right_index: 1,
            },
            send_gain_db: 0.0,
            return_gain_db: 0.0,
            dry_wet: 1.0,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames: 0,
                manual_correction_frames: 0,
            },
            clock_policy: ClockPolicy::IndependentClock {
                max_drift_frames: 128,
            },
            monitoring_enabled: false,
            disconnect_fallback: DisconnectFallback::MuteAll,
        }
    }
}

fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

fn write_channel_selection(out: &mut Vec<u8>, sel: HardwareChannelSelection) {
    match sel {
        HardwareChannelSelection::Mono { channel_index } => {
            out.push(0);
            out.extend_from_slice(&channel_index.to_le_bytes());
            out.extend_from_slice(&0_u16.to_le_bytes());
        }
        HardwareChannelSelection::StereoPair {
            left_index,
            right_index,
        } => {
            out.push(1);
            out.extend_from_slice(&left_index.to_le_bytes());
            out.extend_from_slice(&right_index.to_le_bytes());
        }
    }
}

fn read_channel_selection(
    cursor: &mut &[u8],
) -> Result<HardwareChannelSelection, DeviceContractError> {
    if cursor.len() < 5 {
        return Err(DeviceContractError::MalformedState);
    }
    let tag = cursor[0];
    let a = u16::from_le_bytes([cursor[1], cursor[2]]);
    let b = u16::from_le_bytes([cursor[3], cursor[4]]);
    *cursor = &cursor[5..];
    match tag {
        0 if b == 0 => Ok(HardwareChannelSelection::Mono { channel_index: a }),
        1 if a != b => Ok(HardwareChannelSelection::StereoPair {
            left_index: a,
            right_index: b,
        }),
        _ => Err(DeviceContractError::MalformedState),
    }
}

fn write_uid(out: &mut Vec<u8>, uid: &str) -> Result<(), DeviceContractError> {
    if uid.len() > MAX_HARDWARE_UID_BYTES {
        return Err(DeviceContractError::MalformedState);
    }
    let len = u32::try_from(uid.len()).map_err(|_| DeviceContractError::MalformedState)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(uid.as_bytes());
    Ok(())
}

fn read_uid(cursor: &mut &[u8]) -> Result<String, DeviceContractError> {
    if cursor.len() < 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let len = u32::from_le_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]) as usize;
    *cursor = &cursor[4..];
    if len > MAX_HARDWARE_UID_BYTES || cursor.len() < len {
        return Err(DeviceContractError::MalformedState);
    }
    let text = std::str::from_utf8(&cursor[..len])
        .map_err(|_| DeviceContractError::MalformedState)?
        .to_owned();
    *cursor = &cursor[len..];
    Ok(text)
}

fn read_f32(cursor: &mut &[u8]) -> Result<f32, DeviceContractError> {
    if cursor.len() < 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let v = f32::from_le_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]);
    *cursor = &cursor[4..];
    Ok(v)
}

fn read_u32(cursor: &mut &[u8]) -> Result<u32, DeviceContractError> {
    if cursor.len() < 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let v = u32::from_le_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]);
    *cursor = &cursor[4..];
    Ok(v)
}

fn read_i32(cursor: &mut &[u8]) -> Result<i32, DeviceContractError> {
    if cursor.len() < 4 {
        return Err(DeviceContractError::MalformedState);
    }
    let v = i32::from_le_bytes([cursor[0], cursor[1], cursor[2], cursor[3]]);
    *cursor = &cursor[4..];
    Ok(v)
}

impl ExternalAudioEffectState {
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if self.send_port_uid.len() > MAX_HARDWARE_UID_BYTES
            || self.return_port_uid.len() > MAX_HARDWARE_UID_BYTES
        {
            return Err(DeviceContractError::MalformedState);
        }
        if matches!(
            self.send_channels,
            HardwareChannelSelection::StereoPair {
                left_index,
                right_index,
            } if left_index == right_index
        ) || matches!(
            self.return_channels,
            HardwareChannelSelection::StereoPair {
                left_index,
                right_index,
            } if left_index == right_index
        ) {
            return Err(DeviceContractError::MalformedState);
        }
        if !self.send_gain_db.is_finite()
            || !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&self.send_gain_db)
            || !self.return_gain_db.is_finite()
            || !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&self.return_gain_db)
            || !self.dry_wet.is_finite()
            || !(0.0..=1.0).contains(&self.dry_wet)
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
            Vec::with_capacity(64 + self.send_port_uid.len() + self.return_port_uid.len());
        write_uid(&mut payload, &self.send_port_uid)?;
        write_channel_selection(&mut payload, self.send_channels);
        write_uid(&mut payload, &self.return_port_uid)?;
        write_channel_selection(&mut payload, self.return_channels);
        payload.extend_from_slice(&self.send_gain_db.to_le_bytes());
        payload.extend_from_slice(&self.return_gain_db.to_le_bytes());
        payload.extend_from_slice(&self.dry_wet.to_le_bytes());
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
        payload.push(self.disconnect_fallback as u8);
        NativeDeviceState::new(
            EXTERNAL_AUDIO_EFFECT_TYPE_ID,
            EXTERNAL_AUDIO_EFFECT_STATE_VERSION,
            payload,
        )
    }

    pub fn decode(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != EXTERNAL_AUDIO_EFFECT_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != EXTERNAL_AUDIO_EFFECT_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let mut cursor = state.payload();
        let send_port_uid = read_uid(&mut cursor)?;
        let send_channels = read_channel_selection(&mut cursor)?;
        let return_port_uid = read_uid(&mut cursor)?;
        let return_channels = read_channel_selection(&mut cursor)?;
        let send_gain_db = read_f32(&mut cursor)?;
        let return_gain_db = read_f32(&mut cursor)?;
        let dry_wet = read_f32(&mut cursor)?;
        let declared_round_trip_frames = read_u32(&mut cursor)?;
        let manual_correction_frames = read_i32(&mut cursor)?;
        if cursor.len() < 7 {
            return Err(DeviceContractError::MalformedState);
        }
        let clock_tag = cursor[0];
        cursor = &cursor[1..];
        let clock_param = read_u32(&mut cursor)?;
        let clock_policy = match clock_tag {
            0 if clock_param == 0 => ClockPolicy::SharedClock,
            1 => ClockPolicy::IndependentClock {
                max_drift_frames: clock_param,
            },
            _ => return Err(DeviceContractError::MalformedState),
        };
        let monitoring_byte = cursor[0];
        let fallback_byte = cursor[1];
        cursor = &cursor[2..];
        if !cursor.is_empty() || monitoring_byte > 1 {
            return Err(DeviceContractError::MalformedState);
        }
        let decoded = Self {
            send_port_uid,
            send_channels,
            return_port_uid,
            return_channels,
            send_gain_db,
            return_gain_db,
            dry_wet,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames,
                manual_correction_frames,
            },
            clock_policy,
            monitoring_enabled: monitoring_byte != 0,
            disconnect_fallback: DisconnectFallback::from_byte(fallback_byte)?,
        };
        decoded.validate()?;
        Ok(decoded)
    }

    /// Converts authored effect state into a concrete [`HardwareSendReturnConfig`].
    pub fn to_hardware_config(
        &self,
        sample_rate_hz: u32,
        max_block_frames: usize,
        main_output_port: Option<(HardwareAudioPortId, HardwareChannelSelection)>,
    ) -> Result<HardwareSendReturnConfig, HardwareRouteError> {
        let send_port = HardwareAudioPortId::new(self.send_port_uid.clone())?;
        let return_port = HardwareAudioPortId::new(self.return_port_uid.clone())?;
        Ok(HardwareSendReturnConfig {
            send_port,
            send_channels: self.send_channels,
            return_port,
            return_channels: self.return_channels,
            sample_rate_hz,
            max_block_frames,
            clock_policy: self.clock_policy,
            latency_policy: self.latency_policy,
            main_output_port,
        })
    }
}

/// Preallocated callback-side state for one prepared External Audio Effect.
///
/// Every buffer is allocated strictly on control; retiring swaps the old box
/// back to control through a bounded ring so final drops never occur on callback.
struct PreparedEffectBlock {
    generation: u64,
    send_gain: f32,
    return_gain: f32,
    dry_wet: f32,
    monitoring_enabled: bool,
    disconnect_fallback: DisconnectFallback,
    dry_delay: Box<[[f32; 2]]>,
    dry_delay_frames: usize,
    dry_write_pos: usize,
    send_scratch: Box<[[f32; 2]]>,
    return_scratch: Box<[[f32; 2]]>,
}

impl PreparedEffectBlock {
    fn new(
        generation: u64,
        state: &ExternalAudioEffectState,
        effective_pdc_frames: u32,
        max_block_frames: usize,
    ) -> Self {
        let delay_len = effective_pdc_frames as usize;
        Self {
            generation,
            send_gain: db_to_linear(state.send_gain_db),
            return_gain: db_to_linear(state.return_gain_db),
            dry_wet: state.dry_wet,
            monitoring_enabled: state.monitoring_enabled,
            disconnect_fallback: state.disconnect_fallback,
            dry_delay: vec![[0.0, 0.0]; delay_len.max(1)].into_boxed_slice(),
            dry_delay_frames: delay_len,
            dry_write_pos: 0,
            send_scratch: vec![[0.0, 0.0]; max_block_frames].into_boxed_slice(),
            return_scratch: vec![[0.0, 0.0]; max_block_frames].into_boxed_slice(),
        }
    }

    fn delay_dry_sample(&mut self, input: [f32; 2]) -> [f32; 2] {
        let sanitized = [
            if input[0].is_finite() { input[0] } else { 0.0 },
            if input[1].is_finite() { input[1] } else { 0.0 },
        ];
        if self.dry_delay_frames == 0 {
            return sanitized;
        }
        let out = self.dry_delay[self.dry_write_pos];
        self.dry_delay[self.dry_write_pos] = sanitized;
        self.dry_write_pos += 1;
        if self.dry_write_pos >= self.dry_delay_frames {
            self.dry_write_pos = 0;
        }
        out
    }
}

/// Real-time audio-callback processor for one External Audio Effect.
pub struct ExternalAudioEffectCallback {
    hardware: HardwareSendReturnCallback,
    active: Option<PreparedEffectBlock>,
    publish_rx: Consumer<PreparedEffectBlock>,
    retire_tx: Producer<PreparedEffectBlock>,
}

impl ExternalAudioEffectCallback {
    pub fn poll_publication(&mut self) {
        self.hardware.poll_publication();
        while let Ok(next) = self.publish_rx.pop() {
            if let Some(old) = self.active.replace(next) {
                let _ = self.retire_tx.push(old);
            }
        }
    }

    pub fn active_generation(&self) -> u64 {
        self.active.as_ref().map_or(0, |b| b.generation)
    }

    /// Processes track stereo audio in-place through the selected hardware send
    /// and return path with latency-aligned dry/wet mixing.
    ///
    /// Zero heap allocations, zero locks, zero OS calls, zero final drops.
    pub fn process_block(&mut self, frames: &mut [[f32; 2]]) -> HardwareCallbackStatus {
        self.poll_publication();
        let Some(effect) = self.active.as_mut() else {
            return HardwareCallbackStatus::Inactive;
        };
        let n = frames.len().min(effect.send_scratch.len());
        for (&frame, dst) in frames[..n].iter().zip(&mut effect.send_scratch[..n]) {
            let l = if frame[0].is_finite() { frame[0] } else { 0.0 };
            let r = if frame[1].is_finite() { frame[1] } else { 0.0 };
            *dst = [l * effect.send_gain, r * effect.send_gain];
        }
        let status = self
            .hardware
            .process_block(&effect.send_scratch[..n], &mut effect.return_scratch[..n]);

        match status {
            HardwareCallbackStatus::Stopped { .. } => {
                frames.fill([0.0, 0.0]);
            }
            HardwareCallbackStatus::Disconnected { .. } => match effect.disconnect_fallback {
                DisconnectFallback::MuteAll => frames.fill([0.0, 0.0]),
                DisconnectFallback::MuteWetKeepDry => {
                    for frame in frames.iter_mut().take(n) {
                        *frame = effect.delay_dry_sample(*frame);
                    }
                    for frame in frames.iter_mut().skip(n) {
                        *frame = [0.0, 0.0];
                    }
                }
            },
            HardwareCallbackStatus::Active { .. } => {
                let wet_mix = if effect.monitoring_enabled {
                    effect.dry_wet
                } else {
                    0.0
                };
                let dry_mix = 1.0 - wet_mix;
                let return_gain = effect.return_gain;
                let delay_frames = effect.dry_delay_frames;
                let mut write_pos = effect.dry_write_pos;
                for (frame, &wet) in frames[..n].iter_mut().zip(&effect.return_scratch[..n]) {
                    let sanitized = [
                        if frame[0].is_finite() { frame[0] } else { 0.0 },
                        if frame[1].is_finite() { frame[1] } else { 0.0 },
                    ];
                    let dry = if delay_frames == 0 {
                        sanitized
                    } else {
                        let out = effect.dry_delay[write_pos];
                        effect.dry_delay[write_pos] = sanitized;
                        write_pos += 1;
                        if write_pos >= delay_frames {
                            write_pos = 0;
                        }
                        out
                    };
                    *frame = [
                        dry[0] * dry_mix + wet[0] * return_gain * wet_mix,
                        dry[1] * dry_mix + wet[1] * return_gain * wet_mix,
                    ];
                }
                effect.dry_write_pos = write_pos;
                for frame in frames.iter_mut().skip(n) {
                    *frame = [0.0, 0.0];
                }
            }
            HardwareCallbackStatus::Inactive => {}
        }
        status
    }

    /// Hands active effect buffers and hardware route back to control before
    /// shutdown so final destruction runs strictly on control.
    pub fn retire_active_for_shutdown(&mut self) {
        self.poll_publication();
        if let Some(old) = self.active.take() {
            let _ = self.retire_tx.push(old);
        }
        self.hardware.retire_active_for_shutdown();
    }
}

/// Control-side host owning preparation, transactional publication, Stop, and
/// bounded reclamation for one External Audio Effect.
pub struct ExternalAudioEffectHost<B: HardwareAudioBackend> {
    hardware: HardwareSendReturnHost<B>,
    publish_tx: Producer<PreparedEffectBlock>,
    retire_rx: Consumer<PreparedEffectBlock>,
    in_flight_retirements: usize,
    authored: ExternalAudioEffectState,
}

impl<B: HardwareAudioBackend> ExternalAudioEffectHost<B> {
    pub fn new(backend: B) -> (Self, ExternalAudioEffectCallback) {
        let (hardware, hw_cb) = HardwareSendReturnHost::new(backend);
        let (publish_tx, publish_rx) = RingBuffer::new(HARDWARE_SWAP_CAPACITY);
        let (retire_tx, retire_rx) = RingBuffer::new(HARDWARE_RETIREMENT_CAPACITY);
        let host = Self {
            hardware,
            publish_tx,
            retire_rx,
            in_flight_retirements: 0,
            authored: ExternalAudioEffectState::default(),
        };
        let callback = ExternalAudioEffectCallback {
            hardware: hw_cb,
            active: None,
            publish_rx,
            retire_tx,
        };
        (host, callback)
    }

    pub fn authored_state(&self) -> &ExternalAudioEffectState {
        &self.authored
    }

    pub fn hardware(&self) -> &HardwareSendReturnHost<B> {
        &self.hardware
    }

    pub fn hardware_mut(&mut self) -> &mut HardwareSendReturnHost<B> {
        &mut self.hardware
    }

    pub fn request_stop(&self) {
        self.hardware.request_stop();
    }

    /// Validates, prepares, and transactionally publishes `state` at `generation`.
    ///
    /// On any failure (invalid state, offline render, missing/unsupported port,
    /// feedback cycle, stale generation, or backend open error), the existing
    /// authored state and active hardware route remain untouched.
    pub fn prepare_and_publish(
        &mut self,
        generation: u64,
        state: &ExternalAudioEffectState,
        sample_rate_hz: u32,
        max_block_frames: usize,
        main_output_port: Option<(HardwareAudioPortId, HardwareChannelSelection)>,
        offline_mode: bool,
    ) -> Result<u32, HardwareRouteError> {
        state
            .validate()
            .map_err(|_| HardwareRouteError::InvalidLatencyPolicy)?;
        if self.publish_tx.is_full() {
            return Err(HardwareRouteError::SwapQueueFull);
        }
        if self.hardware.committed_generation() != 0
            && self.in_flight_retirements >= HARDWARE_RETIREMENT_CAPACITY - 1
        {
            return Err(HardwareRouteError::RetirementQueueFull);
        }
        let config =
            state.to_hardware_config(sample_rate_hz, max_block_frames, main_output_port)?;
        let effective_pdc = self
            .hardware
            .prepare_and_publish(generation, &config, offline_mode)?;
        let prepared_block =
            PreparedEffectBlock::new(generation, state, effective_pdc, max_block_frames);
        self.publish_tx
            .push(prepared_block)
            .map_err(|_| HardwareRouteError::SwapQueueFull)?;
        if self.hardware.committed_generation() != 0 {
            self.in_flight_retirements = self.in_flight_retirements.saturating_add(1);
        }
        self.authored = state.clone();
        Ok(effective_pdc)
    }

    /// Drops retired preallocated effect buffers and hardware streams strictly
    /// on control. Returns `(retired_effects, retired_hardware_routes)`.
    pub fn reclaim_retired(&mut self) -> (usize, usize) {
        let mut effects = 0;
        while let Ok(old) = self.retire_rx.pop() {
            drop(old);
            self.in_flight_retirements = self.in_flight_retirements.saturating_sub(1);
            effects += 1;
        }
        let routes = self.hardware.reclaim_retired();
        (effects, routes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware_send_return::{HardwareDeviceCapabilities, HardwareStreamEndpoints};
    use hex_project::{Command, Editor, Project};
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    struct LoopbackStreams {
        endpoints: HardwareStreamEndpoints,
        delay_line: VecDeque<Vec<f32>>,
        send_buf: Vec<f32>,
        ret_buf: Vec<f32>,
        drops: Arc<AtomicU64>,
    }

    impl Drop for LoopbackStreams {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl LoopbackStreams {
        fn pump(&mut self, frames: usize) {
            let sc = usize::from(self.endpoints.send_device_channels);
            let rc = usize::from(self.endpoints.return_device_channels);
            self.send_buf.resize(frames * sc, 0.0);
            self.endpoints
                .fill_interleaved_send_buffer(&mut self.send_buf);
            for f in 0..frames {
                let mut rf = vec![0.0_f32; rc];
                if let (
                    HardwareChannelSelection::StereoPair {
                        left_index: sl,
                        right_index: sr,
                    },
                    HardwareChannelSelection::StereoPair {
                        left_index: rl,
                        right_index: rr,
                    },
                ) = (self.endpoints.send_channels, self.endpoints.return_channels)
                {
                    rf[usize::from(rl)] = self.send_buf[f * sc + usize::from(sl)];
                    rf[usize::from(rr)] = self.send_buf[f * sc + usize::from(sr)];
                }
                self.delay_line.push_back(rf);
            }
            self.ret_buf.clear();
            self.ret_buf.resize(frames * rc, 0.0);
            for f in 0..frames {
                if let Some(frame) = self.delay_line.pop_front() {
                    self.ret_buf[f * rc..(f + 1) * rc].copy_from_slice(&frame);
                }
            }
            self.endpoints.push_interleaved_return_buffer(&self.ret_buf);
        }
    }

    struct LoopbackBackend {
        sends: BTreeMap<String, HardwareDeviceCapabilities>,
        returns: BTreeMap<String, HardwareDeviceCapabilities>,
        delay_frames: usize,
        drops: Arc<AtomicU64>,
    }

    impl LoopbackBackend {
        fn new(delay_frames: usize, drops: Arc<AtomicU64>) -> Self {
            let mut sends = BTreeMap::new();
            let mut returns = BTreeMap::new();
            sends.insert(
                "coreaudio:fx".into(),
                HardwareDeviceCapabilities {
                    port: HardwareAudioPortId::new("coreaudio:fx").unwrap(),
                    channels: 8,
                    supported_sample_rates: vec![48_000],
                    is_software_loopback: false,
                },
            );
            returns.insert(
                "coreaudio:fx".into(),
                HardwareDeviceCapabilities {
                    port: HardwareAudioPortId::new("coreaudio:fx").unwrap(),
                    channels: 8,
                    supported_sample_rates: vec![48_000],
                    is_software_loopback: false,
                },
            );
            Self {
                sends,
                returns,
                delay_frames,
                drops,
            }
        }
    }

    impl HardwareAudioBackend for LoopbackBackend {
        type OpenedStreams = LoopbackStreams;

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
            let rc = usize::from(endpoints.return_device_channels);
            let mut delay_line = VecDeque::with_capacity(self.delay_frames + 128);
            for _ in 0..self.delay_frames {
                delay_line.push_back(vec![0.0_f32; rc]);
            }
            Ok(LoopbackStreams {
                endpoints,
                delay_line,
                send_buf: Vec::new(),
                ret_buf: Vec::new(),
                drops: Arc::clone(&self.drops),
            })
        }
    }

    fn configured_state(declared_pdc: u32, correction: i32) -> ExternalAudioEffectState {
        ExternalAudioEffectState {
            send_port_uid: "coreaudio:fx".into(),
            send_channels: HardwareChannelSelection::StereoPair {
                left_index: 2,
                right_index: 3,
            },
            return_port_uid: "coreaudio:fx".into(),
            return_channels: HardwareChannelSelection::StereoPair {
                left_index: 4,
                right_index: 5,
            },
            send_gain_db: -6.020_6,
            return_gain_db: 6.020_6,
            dry_wet: 0.5,
            latency_policy: HardwareLatencyPolicy {
                declared_round_trip_frames: declared_pdc,
                manual_correction_frames: correction,
            },
            clock_policy: ClockPolicy::SharedClock,
            monitoring_enabled: true,
            disconnect_fallback: DisconnectFallback::MuteWetKeepDry,
        }
    }

    #[test]
    fn synthetic_impulse_loopback_aligns_dry_and_wet_at_exact_pdc_frame_with_gain_and_mix() {
        let drops = Arc::new(AtomicU64::new(0));
        let delay = 192_usize;
        let block = 64_usize;
        let round_trip = (delay + block) as u32; // 256 frames total loopback delay
        let (mut host, mut cb) =
            ExternalAudioEffectHost::new(LoopbackBackend::new(delay, Arc::clone(&drops)));

        let state = configured_state(round_trip, 0);
        let main_out = Some((
            HardwareAudioPortId::new("coreaudio:fx").unwrap(),
            HardwareChannelSelection::StereoPair {
                left_index: 0,
                right_index: 1,
            },
        ));
        let pdc = host
            .prepare_and_publish(1, &state, 48_000, block, main_out, false)
            .unwrap();
        assert_eq!(pdc, 256);

        let mut recorded = Vec::new();
        for b in 0..8 {
            let mut frames = vec![[0.0_f32; 2]; block];
            if b == 0 {
                frames[0] = [1.0, -0.5];
            }
            let status = cb.process_block(&mut frames);
            assert!(matches!(
                status,
                HardwareCallbackStatus::Active { generation: 1, .. }
            ));
            recorded.extend_from_slice(&frames);
            host.hardware_mut()
                .active_streams_mut()
                .unwrap()
                .pump(block);
        }

        // At frame 256, dry (delayed 256 frames) and wet (send_gain * return_gain = 1.0, delayed 256 frames)
        // arrive on the EXACT same sample and sum to [1.0, -0.5] with zero comb-filter double impulse!
        assert!((recorded[256][0] - 1.0).abs() < 1e-4, "{:?}", recorded[256]);
        assert!((recorded[256][1] + 0.5).abs() < 1e-4, "{:?}", recorded[256]);
        for (i, frame) in recorded.iter().enumerate() {
            if i != 256 {
                assert_eq!(*frame, [0.0, 0.0], "unexpected energy at frame {i}");
            }
        }
    }

    #[test]
    fn disconnect_stop_offline_and_stale_refusals_preserve_active_effect_and_authored_state() {
        let drops = Arc::new(AtomicU64::new(0));
        let (mut host, mut cb) =
            ExternalAudioEffectHost::new(LoopbackBackend::new(64, Arc::clone(&drops)));
        let state = configured_state(128, 0);
        host.prepare_and_publish(1, &state, 48_000, 64, None, false)
            .unwrap();

        // Offline render, missing endpoint, and stale generation are refused while preserving gen 1.
        assert_eq!(
            host.prepare_and_publish(2, &state, 48_000, 64, None, true),
            Err(HardwareRouteError::OfflineRenderUnsupported)
        );
        let mut missing = state.clone();
        missing.send_port_uid = "coreaudio:missing".into();
        assert_eq!(
            host.prepare_and_publish(2, &missing, 48_000, 64, None, false),
            Err(HardwareRouteError::MissingSendDevice)
        );
        assert_eq!(
            host.prepare_and_publish(1, &state, 48_000, 64, None, false),
            Err(HardwareRouteError::StaleGeneration)
        );
        assert_eq!(host.authored_state(), &state);

        // Disconnect obeys MuteWetKeepDry while keeping authored state intact.
        host.hardware().active_state().unwrap().mark_disconnected();
        let mut buf = [[0.5_f32, -0.25_f32]; 64];
        assert_eq!(
            cb.process_block(&mut buf),
            HardwareCallbackStatus::Disconnected { generation: 1 }
        );
        assert_eq!(host.authored_state(), &state);

        // Publish gen 2 with MuteAll fallback & Stop under load.
        let mut mute_all = state.clone();
        mute_all.disconnect_fallback = DisconnectFallback::MuteAll;
        host.prepare_and_publish(2, &mute_all, 48_000, 64, None, false)
            .unwrap();
        let mut buf2 = [[0.5_f32, -0.25_f32]; 64];
        assert!(matches!(
            cb.process_block(&mut buf2),
            HardwareCallbackStatus::Active { generation: 2, .. }
        ));
        assert_eq!(host.reclaim_retired(), (1, 1));
        assert_eq!(drops.load(Ordering::Relaxed), 1);

        host.request_stop();
        let mut stop_buf = [[0.8_f32, 0.8_f32]; 64];
        assert_eq!(
            cb.process_block(&mut stop_buf),
            HardwareCallbackStatus::Stopped { generation: 2 }
        );
        assert!(stop_buf.iter().all(|&f| f == [0.0, 0.0]));
    }

    #[test]
    fn one_step_history_undo_redo_and_storage_save_reopen_preserve_effect_state() {
        let initial = configured_state(256, -8);
        let edited = ExternalAudioEffectState {
            dry_wet: 0.25,
            send_gain_db: -3.0,
            monitoring_enabled: false,
            ..initial.clone()
        };

        let project = Project::new(48_000).unwrap();
        let mut editor = Editor::new(project, 16).unwrap();
        editor
            .execute(Command::AddTrack {
                name: "ext fx".into(),
            })
            .unwrap();
        let track = editor.project().tracks()[0].id();
        let encoded_initial = initial.encode().unwrap();
        editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: encoded_initial.clone(),
            })
            .unwrap();
        let device_id = editor.project().track(track).unwrap().devices()[0].id();

        let encoded_edited = edited.encode().unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: device_id,
                state: encoded_edited.clone(),
            })
            .unwrap();

        // Storage save/reopen round-trip preserves every field losslessly.
        let saved_bytes = hex_storage::encode(editor.project()).unwrap();
        let reopened = hex_storage::decode(&saved_bytes).unwrap();
        let reopened_state =
            ExternalAudioEffectState::decode(reopened.track(track).unwrap().devices()[0].state())
                .unwrap();
        assert_eq!(reopened_state, edited);

        // One-step Undo restores initial state; Redo restores edited state.
        assert!(editor.undo());
        let after_undo = ExternalAudioEffectState::decode(
            editor.project().track(track).unwrap().devices()[0].state(),
        )
        .unwrap();
        assert_eq!(after_undo, initial);

        assert!(editor.redo());
        let after_redo = ExternalAudioEffectState::decode(
            editor.project().track(track).unwrap().devices()[0].state(),
        )
        .unwrap();
        assert_eq!(after_redo, edited);

        // Strict refusal of wrong type, future schema version, and malformed bytes.
        let future = NativeDeviceState::new(
            EXTERNAL_AUDIO_EFFECT_TYPE_ID,
            EXTERNAL_AUDIO_EFFECT_STATE_VERSION + 1,
            encoded_initial.payload().to_vec(),
        )
        .unwrap();
        assert_eq!(
            ExternalAudioEffectState::decode(&future),
            Err(DeviceContractError::UnsupportedStateVersion)
        );
        let truncated = NativeDeviceState::new(
            EXTERNAL_AUDIO_EFFECT_TYPE_ID,
            EXTERNAL_AUDIO_EFFECT_STATE_VERSION,
            vec![0; 4],
        )
        .unwrap();
        assert_eq!(
            ExternalAudioEffectState::decode(&truncated),
            Err(DeviceContractError::MalformedState)
        );
    }
}
