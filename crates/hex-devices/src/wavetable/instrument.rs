//! Registered native wavetable instrument (`hex.instrument.wavetable`).
//!
//! Original bounded wavetable instrument built on top of `#3481`'s
//! `PreparedWavetableSource` and `hex_lattice`'s borrowed-resource host lease
//! contract. Exposes one finite `table position` control (`0.0..=1.0`),
//! linear/cubic-Hermite interpolation, linear attack/release envelope,
//! bounded polyphony with oldest-releasing voice stealing, and exact host
//! resource identity verification.
//!
//! # Ownership & Supported Persisted Schemas
//! - All wavetable PCM and source leases stay exclusively with the host
//!   (`ResidentDeviceAssets` / `PreparedDeviceResources`). The opaque
//!   `NativeDeviceState` payload stores only scalar settings and binds its
//!   asset slot via `NativeDeviceState::asset_references()`.
//! - `PreparedWavetableInstrumentRuntime` and `WavetableInstrumentRuntime`
//!   are 100% inline fixed storage with zero heap children.
//! - **Supported persisted state versions:**
//!   - **v1 (`HWT1`, 28 bytes, current):** initial wavetable instrument schema.
//!   - Version 0 and versions > 1 are explicitly rejected (`UnsupportedStateVersion`).

use super::{
    MAX_SAMPLE_RATE, MIN_FRAME_SIZE, MIN_SAMPLE_RATE, PreparedWavetableSource, TWO_POW_64,
    WavetableError, WavetableInterpolation, WavetableLimits, sample_cycle, step_from_frequency,
    table_indices,
};
use hex_lattice::{
    DevicePreparationContext, DeviceResourceView, DeviceResources, MutableInstrumentRuntime,
    NativeInstrumentClock, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentExpression, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError,
    NativeInstrumentRuntimeLimits, PreparedAnalyticNote, PreparedMutableInstrumentRuntime,
    PreparedNativeInstrument, StereoFrame,
};
use hex_lattice_contract::{
    DeviceAssetReference, DeviceAssetSlot, DeviceContractError, DeviceTypeId,
    MAX_DEVICE_ASSET_REFERENCES, NativeDeviceMetadata, NativeDeviceState, ParameterDescriptor,
    ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::any::Any;

pub const WAVETABLE_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.wavetable");
pub const WAVETABLE_STATE_VERSION: u32 = 1;
pub const MAX_WAVETABLE_VOICES: usize = 32;
pub const DEFAULT_WAVETABLE_VOICES: u8 = 16;
pub const DEFAULT_WAVETABLE_FRAME_SIZE: u16 = 256;
pub const MAX_WAVETABLE_EVENTS: usize = 1024;
const DEFAULT_MAX_EVENTS_PER_BLOCK: usize = 259;

const PAYLOAD_MAGIC_V1: [u8; 4] = *b"HWT1";
const PAYLOAD_BYTES_V1: usize = 28;

/// Normalized table morph position (`0.0..=1.0`).
pub const TABLE_POSITION: ParameterId = ParameterId::from_static(1);
/// Master linear output gain (`0.0..=4.0`).
pub const GAIN: ParameterId = ParameterId::from_static(2);
/// Linear amplitude attack time in seconds (`0.0..=10.0 s`).
pub const ATTACK: ParameterId = ParameterId::from_static(3);
/// Linear amplitude release time in seconds (`0.0..=10.0 s`).
pub const RELEASE: ParameterId = ParameterId::from_static(4);
/// Intra-cycle waveform interpolation (`0 = linear`, `1 = cubic hermite`).
pub const INTERPOLATION: ParameterId = ParameterId::from_static(5);
/// Bounded voice count (`1..=32`).
pub const VOICE_LIMIT: ParameterId = ParameterId::from_static(6);

const INTERPOLATION_LABELS: [&str; 2] = ["linear", "cubic hermite"];

const DESCRIPTORS: [ParameterDescriptor; 6] = [
    ParameterDescriptor {
        id: TABLE_POSITION,
        label: "table position",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 1.0,
            unit: None,
        },
        default: ParameterValue::Float(0.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: GAIN,
        label: "gain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 4.0,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: ATTACK,
        label: "attack",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 10.0,
            unit: Some("s"),
        },
        default: ParameterValue::Float(0.002),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: RELEASE,
        label: "release",
        kind: ParameterKind::Float {
            min: 0.0,
            max: 10.0,
            unit: Some("s"),
        },
        default: ParameterValue::Float(0.02),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: INTERPOLATION,
        label: "interpolation",
        kind: ParameterKind::Choice {
            labels: &INTERPOLATION_LABELS,
        },
        default: ParameterValue::Choice(1),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: VOICE_LIMIT,
        label: "voice limit",
        kind: ParameterKind::Float {
            min: 1.0,
            max: MAX_WAVETABLE_VOICES as f32,
            unit: None,
        },
        default: ParameterValue::Float(DEFAULT_WAVETABLE_VOICES as f32),
        presentation: ParameterPresentation::Linear,
    },
];

fn interp_to_u8(mode: WavetableInterpolation) -> u8 {
    match mode {
        WavetableInterpolation::Linear => 0,
        WavetableInterpolation::CubicHermite => 1,
    }
}

fn interp_from_u8(raw: u8) -> Result<WavetableInterpolation, DeviceContractError> {
    match raw {
        0 => Ok(WavetableInterpolation::Linear),
        1 => Ok(WavetableInterpolation::CubicHermite),
        _ => Err(DeviceContractError::MalformedState),
    }
}

fn map_wavetable_error(error: WavetableError) -> DeviceContractError {
    match error {
        WavetableError::InvalidLimits
        | WavetableError::InvalidSampleRate
        | WavetableError::InvalidFrameSize
        | WavetableError::InvalidOscillatorSettings
        | WavetableError::WorkLimitExceeded => DeviceContractError::InvalidParameterValue,
        WavetableError::InvalidIdentity
        | WavetableError::UnalignedFrameCount
        | WavetableError::TableCountOutOfBounds
        | WavetableError::MemoryLimitExceeded
        | WavetableError::NonFiniteSample
        | WavetableError::SourceUnavailable => DeviceContractError::AssetResourcesUnavailable,
    }
}

/// Authored scalar state for `hex.instrument.wavetable` (no PCM or leases).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavetableState {
    pub slot: Option<DeviceAssetSlot>,
    pub frame_size: u16,
    pub table_position: f32,
    pub gain: f32,
    pub attack_seconds: f32,
    pub release_seconds: f32,
    pub interpolation: WavetableInterpolation,
    pub voice_limit: u8,
}

impl WavetableState {
    /// Explicit empty silent state (unbound slot, 0 asset references).
    pub const fn empty() -> Self {
        Self {
            slot: None,
            frame_size: DEFAULT_WAVETABLE_FRAME_SIZE,
            table_position: 0.0,
            gain: 1.0,
            attack_seconds: 0.002,
            release_seconds: 0.02,
            interpolation: WavetableInterpolation::CubicHermite,
            voice_limit: DEFAULT_WAVETABLE_VOICES,
        }
    }

    pub fn validate(self) -> Result<(), DeviceContractError> {
        let frame_size = usize::from(self.frame_size);
        if !(MIN_FRAME_SIZE..=WavetableLimits::DEFAULT.max_frame_size).contains(&frame_size)
            || !frame_size.is_power_of_two()
        {
            return Err(DeviceContractError::MalformedState);
        }
        if !self.table_position.is_finite() || !(0.0..=1.0).contains(&self.table_position) {
            return Err(DeviceContractError::MalformedState);
        }
        if !self.gain.is_finite() || !(0.0..=4.0).contains(&self.gain) {
            return Err(DeviceContractError::MalformedState);
        }
        if !self.attack_seconds.is_finite() || !(0.0..=10.0).contains(&self.attack_seconds) {
            return Err(DeviceContractError::MalformedState);
        }
        if !self.release_seconds.is_finite() || !(0.0..=10.0).contains(&self.release_seconds) {
            return Err(DeviceContractError::MalformedState);
        }
        if self.voice_limit == 0 || usize::from(self.voice_limit) > MAX_WAVETABLE_VOICES {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(())
    }

    pub fn encode_payload(self) -> Result<Vec<u8>, DeviceContractError> {
        self.validate()?;
        let mut out = Vec::new();
        out.try_reserve_exact(PAYLOAD_BYTES_V1)
            .map_err(|_| DeviceContractError::MalformedState)?;
        out.extend_from_slice(&PAYLOAD_MAGIC_V1);
        out.push(interp_to_u8(self.interpolation));
        out.push(self.voice_limit);
        out.extend_from_slice(&self.frame_size.to_le_bytes());
        let raw_slot = self.slot.map_or(0_u32, DeviceAssetSlot::value);
        out.extend_from_slice(&raw_slot.to_le_bytes());
        out.extend_from_slice(&self.table_position.to_le_bytes());
        out.extend_from_slice(&self.gain.to_le_bytes());
        out.extend_from_slice(&self.attack_seconds.to_le_bytes());
        out.extend_from_slice(&self.release_seconds.to_le_bytes());
        Ok(out)
    }

    pub fn decode_payload(payload: &[u8]) -> Result<Self, DeviceContractError> {
        if payload.len() != PAYLOAD_BYTES_V1 || payload[0..4] != PAYLOAD_MAGIC_V1 {
            return Err(DeviceContractError::MalformedState);
        }
        let interpolation = interp_from_u8(payload[4])?;
        let voice_limit = payload[5];
        let frame_size = u16::from_le_bytes([payload[6], payload[7]]);
        let raw_slot = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
        let slot = if raw_slot == 0 {
            None
        } else {
            Some(DeviceAssetSlot::new(raw_slot).map_err(|_| DeviceContractError::MalformedState)?)
        };
        let table_position =
            f32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        let gain = f32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
        let attack_seconds =
            f32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]);
        let release_seconds =
            f32::from_le_bytes([payload[24], payload[25], payload[26], payload[27]]);
        let state = Self {
            slot,
            frame_size,
            table_position,
            gain,
            attack_seconds,
            release_seconds,
            interpolation,
            voice_limit,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn to_native(self) -> Result<NativeDeviceState, DeviceContractError> {
        let payload = self.encode_payload()?;
        NativeDeviceState::new(WAVETABLE_TYPE_ID, WAVETABLE_STATE_VERSION, payload)
    }

    /// Encode bound wavetable state with its single validated host `DeviceAssetReference`.
    pub fn to_native_with_asset(
        self,
        reference: DeviceAssetReference,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if self.slot != Some(reference.slot) {
            return Err(DeviceContractError::MalformedState);
        }
        self.to_native()?.with_asset_references(vec![reference])
    }

    pub fn from_native(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != WAVETABLE_TYPE_ID {
            return Err(DeviceContractError::MalformedState);
        }
        if state.state_version() != WAVETABLE_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let decoded = Self::decode_payload(state.payload())?;
        validate_asset_binding(decoded.slot, state.asset_references())?;
        Ok(decoded)
    }
}

fn validate_asset_binding(
    slot: Option<DeviceAssetSlot>,
    references: &[DeviceAssetReference],
) -> Result<(), DeviceContractError> {
    match slot {
        None => {
            if !references.is_empty() {
                return Err(DeviceContractError::MalformedState);
            }
        }
        Some(expected_slot) => {
            if references.len() != 1 || references[0].slot != expected_slot {
                return Err(DeviceContractError::MalformedState);
            }
        }
    }
    Ok(())
}

pub static WAVETABLE_DEFINITION: WavetableDefinition = WavetableDefinition;

pub struct WavetableDefinition;

impl NativeDeviceMetadata for WavetableDefinition {
    fn type_id(&self) -> DeviceTypeId {
        WAVETABLE_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("wavetable")
    }

    fn state_version(&self) -> u32 {
        WAVETABLE_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::STEREO_INSTRUMENT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        WavetableState::empty().to_native()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        WavetableState::from_native(state)?;
        Ok(state.clone())
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let authored = WavetableState::from_native(state)?;
        match parameter {
            TABLE_POSITION => Ok(ParameterValue::Float(authored.table_position)),
            GAIN => Ok(ParameterValue::Float(authored.gain)),
            ATTACK => Ok(ParameterValue::Float(authored.attack_seconds)),
            RELEASE => Ok(ParameterValue::Float(authored.release_seconds)),
            INTERPOLATION => Ok(ParameterValue::Choice(u32::from(interp_to_u8(
                authored.interpolation,
            )))),
            VOICE_LIMIT => Ok(ParameterValue::Float(f32::from(authored.voice_limit))),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let index = match parameter {
            TABLE_POSITION => 0,
            GAIN => 1,
            ATTACK => 2,
            RELEASE => 3,
            INTERPOLATION => 4,
            VOICE_LIMIT => 5,
            _ => return Err(DeviceContractError::UnknownParameter),
        };
        DESCRIPTORS[index].validate(value)?;
        let mut authored = WavetableState::from_native(state)?;
        match (parameter, value) {
            (TABLE_POSITION, ParameterValue::Float(v)) => authored.table_position = v,
            (GAIN, ParameterValue::Float(v)) => authored.gain = v,
            (ATTACK, ParameterValue::Float(v)) => authored.attack_seconds = v,
            (RELEASE, ParameterValue::Float(v)) => authored.release_seconds = v,
            (INTERPOLATION, ParameterValue::Choice(raw)) => {
                let raw_u8 =
                    u8::try_from(raw).map_err(|_| DeviceContractError::InvalidParameterValue)?;
                authored.interpolation = interp_from_u8(raw_u8)?;
            }
            (VOICE_LIMIT, ParameterValue::Float(v)) => {
                let rounded = v.round();
                if !rounded.is_finite() || rounded < 1.0 || rounded > MAX_WAVETABLE_VOICES as f32 {
                    return Err(DeviceContractError::InvalidParameterValue);
                }
                authored.voice_limit = rounded as u8;
            }
            _ => return Err(DeviceContractError::InvalidParameterValue),
        }
        authored
            .validate()
            .map_err(|_| DeviceContractError::InvalidParameterValue)?;
        authored.to_native()?.with_host_metadata_from(state)
    }
}

impl NativeInstrumentDefinition for WavetableDefinition {
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        Some(std::mem::size_of::<PreparedWavetableInstrumentRuntime>())
    }

    fn supports_asset_resources(&self) -> bool {
        true
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        prepare_wavetable(state, context, DEFAULT_MAX_EVENTS_PER_BLOCK)
    }

    fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        prepare_wavetable(state, context, limits.max_events_per_block)
    }
}

fn prepare_wavetable(
    state: &NativeDeviceState,
    context: &DevicePreparationContext<'_>,
    max_events: usize,
) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
    if !(1..=MAX_WAVETABLE_EVENTS).contains(&max_events)
        || !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&context.sample_rate)
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let authored = WavetableState::from_native(state)?;
    let Some(slot) = authored.slot else {
        if !context.resources.is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        return Ok(Box::new(EmptyWavetable));
    };
    let expected_ref = state
        .asset_references()
        .first()
        .ok_or(DeviceContractError::MalformedState)?;
    let view = context
        .resources
        .slot(slot)
        .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
    if view.identity.asset != expected_ref.asset {
        return Err(DeviceContractError::AssetResourcesUnavailable);
    }
    let source = PreparedWavetableSource::prepare_on_control(
        view,
        usize::from(authored.frame_size),
        WavetableLimits::DEFAULT,
    )
    .map_err(map_wavetable_error)?;

    let mut phase_steps = [None; 128];
    let freqs = context.tuning.frequencies();
    for (pitch, entry) in phase_steps.iter_mut().enumerate() {
        if let Ok(step) = step_from_frequency(freqs[pitch], context.sample_rate) {
            *entry = Some(step);
        }
    }

    let attack_frames =
        (f64::from(authored.attack_seconds) * f64::from(context.sample_rate)).round() as u64;
    let release_frames =
        (f64::from(authored.release_seconds) * f64::from(context.sample_rate)).round() as u64;
    let steal_fade_frames = ((u64::from(context.sample_rate) + 500) / 1000).max(1);
    let max_tail_frames = release_frames.max(steal_fade_frames);

    let factory = PreparedWavetableInstrumentRuntime {
        source,
        output_sample_rate: context.sample_rate,
        table_position: f64::from(authored.table_position),
        master_gain: f64::from(authored.gain),
        attack_frames,
        release_frames,
        steal_fade_frames,
        max_tail_frames,
        interpolation: authored.interpolation,
        voice_limit: usize::from(authored.voice_limit),
        max_events,
        phase_steps,
    };
    Ok(Box::new(PreparedWavetableInstrument(factory)))
}

struct EmptyWavetable;

impl PreparedNativeInstrument for EmptyWavetable {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        if pitch > 127 || velocity > 127 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
            silence,
        ))
    }
}

fn silence(
    _: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    _: u64,
    _: u64,
    _: u64,
    _: &dyn DeviceResources,
) -> StereoFrame {
    [0.0; 2]
}

struct PreparedWavetableInstrument(PreparedWavetableInstrumentRuntime);

impl PreparedNativeInstrument for PreparedWavetableInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn validate_event_note(&self, pitch: u8, velocity: u8) -> Result<(), DeviceContractError> {
        if pitch > 127 || velocity > 127 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(self.0)))
    }
}

/// 100% inline fixed-storage factory for `hex.instrument.wavetable`.
/// Owns zero PCM buffers, zero `Arc`s, and zero heap children.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedWavetableInstrumentRuntime {
    source: PreparedWavetableSource,
    output_sample_rate: u32,
    table_position: f64,
    master_gain: f64,
    attack_frames: u64,
    release_frames: u64,
    steal_fade_frames: u64,
    max_tail_frames: u64,
    interpolation: WavetableInterpolation,
    voice_limit: usize,
    max_events: usize,
    phase_steps: [Option<u64>; 128],
}

impl PreparedWavetableInstrumentRuntime {
    #[inline]
    fn find_resource<'a>(
        &self,
        resources: &'a dyn DeviceResources,
    ) -> Option<DeviceResourceView<'a>> {
        let key = self.source.key();
        let limit = resources.len().min(MAX_DEVICE_ASSET_REFERENCES);
        (0..limit)
            .filter_map(|i| resources.get(i))
            .find(|view| key.matches(*view))
    }
}

impl PreparedMutableInstrumentRuntime for PreparedWavetableInstrumentRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }

    fn supports_occurrence_gain(&self) -> bool {
        true
    }

    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn runtime_version(&self) -> u32 {
        WAVETABLE_STATE_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<WavetableInstrumentRuntime>()
    }

    fn max_events_per_block(&self) -> usize {
        self.max_events
    }

    fn max_active_occurrences(&self) -> usize {
        self.voice_limit
    }

    fn max_release_tail_frames(&self) -> u64 {
        self.max_tail_frames
    }

    fn steals_releasing_voices(&self) -> bool {
        true
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        Box::new(WavetableInstrumentRuntime {
            prepared: *self,
            active: [None; MAX_WAVETABLE_VOICES],
            fading: [None; MAX_WAVETABLE_VOICES],
            midi: MidiState::default(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MidiState {
    channel_pitch_bend: i16,
    cc_volume: u8,
    cc_expression: u8,
    sustain_pedal: bool,
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
            channel_pitch_bend: 0,
            cc_volume: 127,
            cc_expression: 127,
            sustain_pedal: false,
        }
    }
}

impl MidiState {
    #[inline]
    fn effective_step(&self, base_step: u64, voice_pitch_offset_cents: i16) -> u64 {
        if voice_pitch_offset_cents == 0 && self.channel_pitch_bend == 0 {
            return base_step;
        }
        let channel_cents = if self.channel_pitch_bend == 0 {
            0.0
        } else {
            f64::from(self.channel_pitch_bend)
                / if self.channel_pitch_bend < 0 {
                    8192.0
                } else {
                    8191.0
                }
                * 200.0
        };
        let ratio = 2.0_f64.powf((f64::from(voice_pitch_offset_cents) + channel_cents) / 1200.0);
        ((base_step as f64) * ratio)
            .round()
            .clamp(1.0, (1_u64 << 63) as f64 - 1.0) as u64
    }

    #[inline]
    fn gain_multiplier(&self) -> f64 {
        if self.cc_volume == 127 && self.cc_expression == 127 {
            return 1.0;
        }
        (f64::from(self.cc_volume) / 127.0) * (f64::from(self.cc_expression) / 127.0)
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveVoice {
    occurrence_id: u64,
    velocity: u8,
    phase: u64,
    base_step: u64,
    age: u64,
    gate_age: Option<u64>,
    release_start_env: f64,
    pitch_offset_cents: i16,
    pedal_held: bool,
}

#[derive(Clone, Copy, Debug)]
struct FadingVoice {
    voice: ActiveVoice,
    fade_start_age: u64,
    frozen_occurrence_gain: f64,
}

type ActivePool = [Option<ActiveVoice>; MAX_WAVETABLE_VOICES];
type FadingPool = [Option<FadingVoice>; MAX_WAVETABLE_VOICES];

#[derive(Clone, Copy)]
struct TableSampleSlice {
    base_a: usize,
    base_b: usize,
    morph: f64,
    frame_size: usize,
    mask: usize,
}

enum StealOutcome {
    Ready,
    AllHeld,
    FadePoolFull,
}

/// Fixed-storage mutable runtime for `hex.instrument.wavetable`.
pub struct WavetableInstrumentRuntime {
    prepared: PreparedWavetableInstrumentRuntime,
    active: ActivePool,
    fading: FadingPool,
    midi: MidiState,
}

impl WavetableInstrumentRuntime {
    #[inline]
    fn envelope_level(&self, voice: &ActiveVoice) -> f64 {
        match voice.gate_age {
            None => {
                if self.prepared.attack_frames == 0 || voice.age >= self.prepared.attack_frames {
                    1.0
                } else {
                    (voice.age as f64) / (self.prepared.attack_frames as f64)
                }
            }
            Some(gate) => {
                let rel = voice.age.saturating_sub(gate);
                if self.prepared.release_frames == 0 || rel >= self.prepared.release_frames {
                    0.0
                } else {
                    voice.release_start_env
                        * (1.0 - (rel as f64 / self.prepared.release_frames as f64))
                }
            }
        }
    }

    fn begin_release(&self, voice: &mut ActiveVoice) {
        if voice.gate_age.is_none() {
            voice.release_start_env = self.envelope_level(voice);
            voice.gate_age = Some(voice.age);
        }
    }

    fn advance_voice(&self, voice: &mut ActiveVoice, midi: &MidiState, frames: u64) -> bool {
        voice.age = voice.age.saturating_add(frames);
        let step = midi.effective_step(voice.base_step, voice.pitch_offset_cents);
        voice.phase = voice.phase.wrapping_add(step.wrapping_mul(frames));
        let release_ended = voice
            .gate_age
            .is_some_and(|gate| voice.age.saturating_sub(gate) >= self.prepared.release_frames);
        !release_ended
    }

    fn advance_pools(
        &self,
        active: &mut ActivePool,
        fading: &mut FadingPool,
        midi: &MidiState,
        frames: u64,
    ) {
        for slot in &mut active[..self.prepared.voice_limit] {
            if let Some(voice) = slot
                && !self.advance_voice(voice, midi, frames)
            {
                *slot = None;
            }
        }
        let fade_len = self.prepared.steal_fade_frames;
        for slot in fading.iter_mut() {
            if let Some(fading_voice) = slot
                && (!self.advance_voice(&mut fading_voice.voice, midi, frames)
                    || fading_voice
                        .voice
                        .age
                        .saturating_sub(fading_voice.fade_start_age)
                        >= fade_len)
            {
                *slot = None;
            }
        }
    }

    fn ensure_voice_slot(
        &self,
        active: &mut ActivePool,
        fading: &mut FadingPool,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> StealOutcome {
        if active[..self.prepared.voice_limit]
            .iter()
            .any(Option::is_none)
        {
            return StealOutcome::Ready;
        }
        let mut oldest: Option<(usize, u64)> = None;
        for (index, entry) in active[..self.prepared.voice_limit].iter().enumerate() {
            if let Some(voice) = entry
                && voice.gate_age.is_some()
                && oldest.is_none_or(|(_, best_age)| voice.age > best_age)
            {
                oldest = Some((index, voice.age));
            }
        }
        let Some((victim_idx, _)) = oldest else {
            return StealOutcome::AllHeld;
        };
        let Some(fade_idx) = fading.iter().position(Option::is_none) else {
            return StealOutcome::FadePoolFull;
        };
        if let Some(victim) = active[victim_idx].take() {
            fading[fade_idx] = Some(FadingVoice {
                frozen_occurrence_gain: gain_at(victim.occurrence_id),
                fade_start_age: victim.age,
                voice: victim,
            });
        }
        StealOutcome::Ready
    }

    fn apply_event(
        &self,
        active: &mut ActivePool,
        fading: &mut FadingPool,
        midi: &mut MidiState,
        event: NativeInstrumentEvent,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        match event {
            NativeInstrumentEvent::NotePressure { .. } => {
                return Err(NativeInstrumentRuntimeError::UnsupportedNotePressure);
            }
            NativeInstrumentEvent::ControlChange { cc, value, .. } => {
                if cc > 127 || value > 127 {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                match cc {
                    7 => midi.cc_volume = value,
                    11 => midi.cc_expression = value,
                    64 => {
                        let down = value >= 64;
                        if midi.sustain_pedal && !down {
                            for voice in active[..self.prepared.voice_limit].iter_mut().flatten() {
                                if voice.pedal_held {
                                    voice.pedal_held = false;
                                    self.begin_release(voice);
                                }
                            }
                        }
                        midi.sustain_pedal = down;
                    }
                    121 => {
                        let was_pedal = midi.sustain_pedal;
                        *midi = MidiState::default();
                        if was_pedal {
                            for voice in active[..self.prepared.voice_limit].iter_mut().flatten() {
                                if voice.pedal_held {
                                    voice.pedal_held = false;
                                    self.begin_release(voice);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            NativeInstrumentEvent::PitchBend { value, .. } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                midi.channel_pitch_bend = value;
            }
            NativeInstrumentEvent::NotePitchBend {
                occurrence_id,
                cents,
                ..
            } => {
                if !(-4_800..=4_800).contains(&cents) {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                for voice in active[..self.prepared.voice_limit]
                    .iter_mut()
                    .flatten()
                    .filter(|v| v.occurrence_id == occurrence_id.get())
                {
                    voice.pitch_offset_cents = cents;
                }
            }
            NativeInstrumentEvent::NoteTimbre { value, .. } => {
                if value > 127 {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
            }
            NativeInstrumentEvent::Expression { expression, .. } => {
                if !expression.is_valid() {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                if let NativeInstrumentExpression::PitchBend(value) = expression {
                    midi.channel_pitch_bend = value;
                }
            }
            NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                for voice in active[..self.prepared.voice_limit]
                    .iter_mut()
                    .flatten()
                    .filter(|v| v.occurrence_id == occurrence_id)
                {
                    if midi.sustain_pedal {
                        if voice.gate_age.is_none() {
                            voice.pedal_held = true;
                        }
                    } else {
                        self.begin_release(voice);
                    }
                }
            }
            NativeInstrumentEvent::NoteOn {
                occurrence_id,
                pitch,
                velocity,
                ..
            } => {
                if occurrence_id == 0
                    || pitch > 127
                    || velocity > 127
                    || active[..self.prepared.voice_limit]
                        .iter()
                        .flatten()
                        .any(|v| v.occurrence_id == occurrence_id)
                {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                if velocity == 0 {
                    self.advance_pools(active, fading, midi, 0);
                    return Ok(());
                }
                let Some(base_step) = self.prepared.phase_steps[usize::from(pitch)] else {
                    self.advance_pools(active, fading, midi, 0);
                    return Ok(());
                };
                match self.ensure_voice_slot(active, fading, gain_at) {
                    StealOutcome::Ready => {}
                    StealOutcome::FadePoolFull => {
                        self.advance_pools(active, fading, midi, 0);
                        return Ok(());
                    }
                    StealOutcome::AllHeld => {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    }
                }
                if let Some(slot) = active[..self.prepared.voice_limit]
                    .iter_mut()
                    .find(|entry| entry.is_none())
                {
                    *slot = Some(ActiveVoice {
                        occurrence_id,
                        velocity,
                        phase: 0,
                        base_step,
                        age: 0,
                        gate_age: None,
                        release_start_env: 1.0,
                        pitch_offset_cents: 0,
                        pedal_held: false,
                    });
                }
            }
        }
        self.advance_pools(active, fading, midi, 0);
        Ok(())
    }

    #[inline]
    fn occurrence_gain_at(
        gains: &[NativeInstrumentOccurrenceGain],
        occurrence_id: u64,
        frame_offset: usize,
    ) -> f64 {
        gains
            .binary_search_by_key(&occurrence_id, |g| g.occurrence_id())
            .ok()
            .map_or(1.0, |idx| gains[idx].value_at_offset(frame_offset as u64))
    }

    #[inline]
    fn sample_voice(
        &self,
        voice: &ActiveVoice,
        extra_gain: f64,
        view: DeviceResourceView<'_>,
        slice: TableSampleSlice,
    ) -> [f64; 2] {
        let env = self.envelope_level(voice);
        if env <= 0.0 {
            return [0.0; 2];
        }
        let phase_turns = (voice.phase as f64) / TWO_POW_64;
        let scaled = phase_turns * (slice.frame_size as f64);
        let idx = (scaled.floor() as usize) & slice.mask;
        let frac = scaled - scaled.floor();
        let sample_a = sample_cycle(
            view.pcm,
            slice.base_a,
            slice.mask,
            idx,
            frac,
            self.prepared.interpolation,
        );
        let sample = if slice.base_a == slice.base_b || slice.morph == 0.0 {
            sample_a
        } else {
            let sample_b = sample_cycle(
                view.pcm,
                slice.base_b,
                slice.mask,
                idx,
                frac,
                self.prepared.interpolation,
            );
            [
                sample_a[0] + (sample_b[0] - sample_a[0]) * slice.morph,
                sample_a[1] + (sample_b[1] - sample_a[1]) * slice.morph,
            ]
        };
        let scale = env
            * (f64::from(voice.velocity) / 127.0)
            * self.prepared.master_gain
            * self.midi.gain_multiplier()
            * extra_gain;
        [sample[0] * scale, sample[1] * scale]
    }

    fn render_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
    ) {
        let Some(view) = self.prepared.find_resource(resources) else {
            frames.fill([0.0; 2]);
            return;
        };
        let layout = self.prepared.source.layout();
        let frame_size = layout.frame_size();
        let (table_a, table_b, morph) =
            table_indices(layout.table_count(), self.prepared.table_position);
        let slice = TableSampleSlice {
            base_a: table_a * frame_size,
            base_b: table_b * frame_size,
            morph,
            frame_size,
            mask: frame_size - 1,
        };
        let voice_limit = self.prepared.voice_limit;
        let fade_len = self.prepared.steal_fade_frames;
        let mut cursor = 0_usize;

        for (frame_offset, out_slot) in frames.iter_mut().enumerate() {
            while let Some(&event) = events.get(cursor)
                && event.frame_offset() == frame_offset
            {
                let mut active = self.active;
                let mut fading = self.fading;
                let mut midi = self.midi;
                let gain_lookup =
                    |occ_id: u64| Self::occurrence_gain_at(gains, occ_id, frame_offset);
                if self
                    .apply_event(&mut active, &mut fading, &mut midi, event, &gain_lookup)
                    .is_ok()
                {
                    self.active = active;
                    self.fading = fading;
                    self.midi = midi;
                }
                cursor += 1;
            }

            let mut sum = [0.0_f64; 2];
            for voice in self.active[..voice_limit].iter().flatten() {
                let occ_gain = Self::occurrence_gain_at(gains, voice.occurrence_id, frame_offset);
                let s = self.sample_voice(voice, occ_gain, view, slice);
                sum[0] += s[0];
                sum[1] += s[1];
            }
            for fading_voice in self.fading.iter().flatten() {
                let elapsed = fading_voice
                    .voice
                    .age
                    .saturating_sub(fading_voice.fade_start_age);
                if elapsed < fade_len {
                    let declick = 1.0 - (elapsed as f64 / fade_len as f64);
                    let s = self.sample_voice(
                        &fading_voice.voice,
                        fading_voice.frozen_occurrence_gain * declick,
                        view,
                        slice,
                    );
                    sum[0] += s[0];
                    sum[1] += s[1];
                }
            }
            *out_slot = [
                if sum[0].is_finite() {
                    sum[0] as f32
                } else {
                    0.0
                },
                if sum[1].is_finite() {
                    sum[1] as f32
                } else {
                    0.0
                },
            ];
            let mut active = self.active;
            let mut fading = self.fading;
            let midi = self.midi;
            self.advance_pools(&mut active, &mut fading, &midi, 1);
            self.active = active;
            self.fading = fading;
        }
    }
}

impl MutableInstrumentRuntime for WavetableInstrumentRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }

    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        let count = self.active[..self.prepared.voice_limit]
            .iter()
            .flatten()
            .count();
        if output.len() < count {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        for (slot, voice) in output[..count]
            .iter_mut()
            .zip(self.active[..self.prepared.voice_limit].iter().flatten())
        {
            *slot = voice.occurrence_id;
        }
        output[..count].sort_unstable();
        Ok(count)
    }

    fn supports_occurrence_gain(&self) -> bool {
        true
    }

    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > self.prepared.max_events
            || gains.len() > MAX_WAVETABLE_EVENTS + MAX_WAVETABLE_VOICES
        {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut known = [false; MAX_WAVETABLE_EVENTS + MAX_WAVETABLE_VOICES];
        let mut mark = |id: u64| {
            if let Ok(idx) = gains.binary_search_by_key(&id, |g| g.occurrence_id()) {
                known[idx] = true;
            }
        };
        for voice in self.active[..self.prepared.voice_limit].iter().flatten() {
            mark(voice.occurrence_id);
        }
        for event in events {
            if let NativeInstrumentEvent::NoteOn { occurrence_id, .. } = event {
                mark(*occurrence_id);
            }
        }
        if !known[..gains.len()].iter().all(|&k| k) {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        Ok(())
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frames: usize,
        resources: &dyn DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > self.prepared.max_events {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        if self.prepared.find_resource(resources).is_none() {
            return Err(NativeInstrumentRuntimeError::ResourceUnavailable);
        }
        if events.is_empty() && self.is_quiescent() {
            return Ok(());
        }
        let mut active = self.active;
        let mut fading = self.fading;
        let mut midi = self.midi;
        let mut cursor = 0_usize;
        let unity = |_: u64| 1.0;
        for &event in events {
            let offset = event.frame_offset();
            if offset < cursor || offset >= frames {
                return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
            }
            self.advance_pools(&mut active, &mut fading, &midi, (offset - cursor) as u64);
            self.apply_event(&mut active, &mut fading, &mut midi, event, &unity)?;
            cursor = offset;
        }
        self.advance_pools(&mut active, &mut fading, &midi, (frames - cursor) as u64);
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn DeviceResources,
    ) {
        self.render_block(events, frames, &[], resources);
    }

    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        _clock: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn DeviceResources,
    ) {
        self.render_block(events, frames, gains, resources);
    }

    fn reset(&mut self) {
        self.active.fill(None);
        self.fading.fill(None);
        self.midi = MidiState::default();
    }

    fn is_quiescent(&self) -> bool {
        self.active[..self.prepared.voice_limit]
            .iter()
            .all(Option::is_none)
            && self.fading.iter().all(Option::is_none)
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|prev| self.prepared == prev.prepared)
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        if let Some(prev) = previous
            .as_any()
            .downcast_ref::<Self>()
            .filter(|prev| self.prepared == prev.prepared)
        {
            self.active = prev.active;
            self.fading = prev.fading;
            self.midi = prev.midi;
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests;
