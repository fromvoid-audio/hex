//! Registered native resident multisample instrument (`hex.instrument.multisample`).
//!
//! Distinct from `hex.instrument.sampler` (Simpler-equivalent single-sample
//! instrument): maps finite resident key-and-velocity zones (`1..=64` zones
//! across host-leased `DeviceResources`) with inclusive `[low_key, high_key]`
//! and `[low_velocity, high_velocity]` (`1..=127`) bounds, per-zone root pitch,
//! detune, gain, release, explicit deterministic overlap policies (`FirstMatch`,
//! `HighestRootProximity`, `LayerAll`), simultaneous same-note occurrences,
//! and bounded voice-limit with oldest-releasing voice stealing.
//!
//! # Ownership & Supported Persisted Schemas
//! - Original PCM and source leases stay exclusively with the host
//!   (`ResidentDeviceAssets` / `PreparedDeviceResources`). The opaque
//!   `NativeDeviceState` payload stores only authored zone metadata and
//!   references host assets strictly via `NativeDeviceState::asset_references()`.
//! - `PreparedMultisampleInstrumentRuntime` and `MultisampleInstrumentRuntime`
//!   are 100% fixed inline storage with zero heap children so
//!   `resource_factory_inline_bytes` and `fixed_storage_bytes` are exact and
//!   callbacks perform zero allocations or frees.
//! - **Supported persisted state versions:**
//!   - **v1 (`HMS1`, 20 bytes/zone):** historical key-zone-only schema from `#3468`.
//!     Migrated losslessly in memory to v2 (`low_velocity = 1`, `high_velocity = 127`)
//!     while preserving host gains and `asset_references`.
//!   - **v2 (`HMS2`, 24 bytes/zone, current):** key-plus-velocity zone schema from
//!     `#3480`.
//!   - Version 0 and versions > 2 are explicitly rejected (`UnsupportedStateVersion`).

use super::{
    KeyZoneSpec, MAX_LAYERED_ZONES_PER_KEY, MAX_SAMPLE_RATE, MAX_VELOCITY, MAX_VOICES, MAX_ZONES,
    MIN_SAMPLE_RATE, MIN_VELOCITY, MultisampleError, MultisampleLimits, PreparedMultisampleMap,
    SelectedZones, ZoneOverlapPolicy, ZoneSourceKey, interpolate_linear, select_matching_zones,
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

pub const MULTISAMPLE_TYPE_ID: DeviceTypeId =
    DeviceTypeId::from_static("hex.instrument.multisample");
pub const MULTISAMPLE_STATE_VERSION_V1: u32 = 1;
pub const MULTISAMPLE_STATE_VERSION: u32 = 2;
pub const MAX_INSTRUMENT_EVENTS: usize = 1024;
pub const DEFAULT_VOICE_LIMIT: u8 = 32;

const PAYLOAD_MAGIC_V1: [u8; 4] = *b"HMS1";
const PAYLOAD_MAGIC_V2: [u8; 4] = *b"HMS2";
const HEADER_BYTES: usize = 12;
const ZONE_BYTES_V1: usize = 20;
const ZONE_BYTES_V2: usize = 24;

/// Master linear output gain (`0.0..=4.0`).
pub const GAIN: ParameterId = ParameterId::from_static(1);
/// Overlap selection policy (`0 = first match`, `1 = highest root proximity`, `2 = layer all`).
pub const OVERLAP_POLICY: ParameterId = ParameterId::from_static(2);
/// Maximum simultaneous sounding voices (`1..=64`).
pub const VOICE_LIMIT: ParameterId = ParameterId::from_static(3);

const OVERLAP_LABELS: [&str; 3] = ["first match", "highest root proximity", "layer all"];

const DESCRIPTORS: [ParameterDescriptor; 3] = [
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
        id: OVERLAP_POLICY,
        label: "overlap policy",
        kind: ParameterKind::Choice {
            labels: &OVERLAP_LABELS,
        },
        default: ParameterValue::Choice(0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: VOICE_LIMIT,
        label: "voice limit",
        kind: ParameterKind::Float {
            min: 1.0,
            max: MAX_VOICES as f32,
            unit: None,
        },
        default: ParameterValue::Float(DEFAULT_VOICE_LIMIT as f32),
        presentation: ParameterPresentation::Linear,
    },
];

fn overlap_to_u8(policy: ZoneOverlapPolicy) -> u8 {
    match policy {
        ZoneOverlapPolicy::FirstMatch => 0,
        ZoneOverlapPolicy::HighestRootProximity => 1,
        ZoneOverlapPolicy::LayerAll => 2,
    }
}

fn overlap_from_u8(raw: u8) -> Result<ZoneOverlapPolicy, DeviceContractError> {
    match raw {
        0 => Ok(ZoneOverlapPolicy::FirstMatch),
        1 => Ok(ZoneOverlapPolicy::HighestRootProximity),
        2 => Ok(ZoneOverlapPolicy::LayerAll),
        _ => Err(DeviceContractError::MalformedState),
    }
}

fn map_multisample_error(error: MultisampleError) -> DeviceContractError {
    match error {
        MultisampleError::InvalidLimits
        | MultisampleError::InvalidSampleRate
        | MultisampleError::InvalidZoneSpec
        | MultisampleError::OverlapLayerLimitExceeded => DeviceContractError::InvalidParameterValue,
        MultisampleError::EmptyZoneMap | MultisampleError::TooManyZones => {
            DeviceContractError::MalformedState
        }
        MultisampleError::MissingSource
        | MultisampleError::InvalidSource
        | MultisampleError::NonFiniteSample
        | MultisampleError::MemoryLimitExceeded
        | MultisampleError::SourceUnavailable => DeviceContractError::AssetResourcesUnavailable,
        MultisampleError::InvalidEventBatch
        | MultisampleError::EventCapacityExceeded
        | MultisampleError::VoiceCapacityExceeded => DeviceContractError::InvalidParameterValue,
    }
}

/// Authored multisample instrument state (no PCM, paths, or host leases).
#[derive(Clone, Debug, PartialEq)]
pub struct MultisampleState {
    pub gain: f32,
    pub overlap_policy: ZoneOverlapPolicy,
    pub voice_limit: u8,
    pub zones: Vec<KeyZoneSpec>,
}

impl MultisampleState {
    /// Explicit empty silent state (0 zones, 0 asset references).
    pub fn empty() -> Self {
        Self {
            gain: 1.0,
            overlap_policy: ZoneOverlapPolicy::FirstMatch,
            voice_limit: DEFAULT_VOICE_LIMIT,
            zones: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if !self.gain.is_finite() || !(0.0..=4.0).contains(&self.gain) {
            return Err(DeviceContractError::MalformedState);
        }
        if self.voice_limit == 0 || usize::from(self.voice_limit) > MAX_VOICES {
            return Err(DeviceContractError::MalformedState);
        }
        if self.zones.len() > MAX_ZONES {
            return Err(DeviceContractError::MalformedState);
        }
        for zone in &self.zones {
            zone.validate()
                .map_err(|_| DeviceContractError::MalformedState)?;
        }
        if self.overlap_policy == ZoneOverlapPolicy::LayerAll {
            for key in 0..=127_u8 {
                let key_count = self.zones.iter().filter(|z| z.contains_key(key)).count();
                if key_count > MAX_LAYERED_ZONES_PER_KEY {
                    for velocity in MIN_VELOCITY..=MAX_VELOCITY {
                        let count = self
                            .zones
                            .iter()
                            .filter(|z| z.contains(key, velocity))
                            .count();
                        if count > MAX_LAYERED_ZONES_PER_KEY {
                            return Err(DeviceContractError::MalformedState);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn encode_payload(&self) -> Result<Vec<u8>, DeviceContractError> {
        self.validate()?;
        let total_bytes = HEADER_BYTES
            .checked_add(
                self.zones
                    .len()
                    .checked_mul(ZONE_BYTES_V2)
                    .ok_or(DeviceContractError::MalformedState)?,
            )
            .ok_or(DeviceContractError::MalformedState)?;
        let mut out = Vec::new();
        out.try_reserve_exact(total_bytes)
            .map_err(|_| DeviceContractError::MalformedState)?;
        out.extend_from_slice(&PAYLOAD_MAGIC_V2);
        out.push(overlap_to_u8(self.overlap_policy));
        out.push(self.voice_limit);
        let zone_count =
            u16::try_from(self.zones.len()).map_err(|_| DeviceContractError::MalformedState)?;
        out.extend_from_slice(&zone_count.to_le_bytes());
        out.extend_from_slice(&self.gain.to_le_bytes());
        for zone in &self.zones {
            out.extend_from_slice(&zone.slot.value().to_le_bytes());
            out.push(zone.low_key);
            out.push(zone.high_key);
            out.push(zone.root_key);
            out.push(0); // reserved flags byte
            out.push(zone.low_velocity);
            out.push(zone.high_velocity);
            out.extend_from_slice(&[0, 0]); // reserved padding bytes
            out.extend_from_slice(&zone.detune_cents.to_le_bytes());
            out.extend_from_slice(&zone.gain.to_le_bytes());
            out.extend_from_slice(&zone.release_seconds.to_le_bytes());
        }
        Ok(out)
    }

    /// Encode historical v1 (`HMS1`, key-zone-only) payload for explicit
    /// prior-format migration testing. Refuses zones whose velocity range is
    /// narrower than full (`1..=127`).
    #[cfg(test)]
    pub fn encode_v1_payload(&self) -> Result<Vec<u8>, DeviceContractError> {
        self.validate()?;
        for zone in &self.zones {
            if zone.low_velocity != MIN_VELOCITY || zone.high_velocity != MAX_VELOCITY {
                return Err(DeviceContractError::MalformedState);
            }
        }
        let total_bytes = HEADER_BYTES + self.zones.len() * ZONE_BYTES_V1;
        let mut out = Vec::with_capacity(total_bytes);
        out.extend_from_slice(&PAYLOAD_MAGIC_V1);
        out.push(overlap_to_u8(self.overlap_policy));
        out.push(self.voice_limit);
        out.extend_from_slice(&(self.zones.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.gain.to_le_bytes());
        for zone in &self.zones {
            out.extend_from_slice(&zone.slot.value().to_le_bytes());
            out.push(zone.low_key);
            out.push(zone.high_key);
            out.push(zone.root_key);
            out.push(0);
            out.extend_from_slice(&zone.detune_cents.to_le_bytes());
            out.extend_from_slice(&zone.gain.to_le_bytes());
            out.extend_from_slice(&zone.release_seconds.to_le_bytes());
        }
        Ok(out)
    }

    pub fn decode_payload(payload: &[u8]) -> Result<Self, DeviceContractError> {
        Self::decode_versioned_payload(MULTISAMPLE_STATE_VERSION, payload)
    }

    pub fn decode_versioned_payload(
        state_version: u32,
        payload: &[u8],
    ) -> Result<Self, DeviceContractError> {
        let (expected_magic, zone_stride, has_velocity) = match state_version {
            MULTISAMPLE_STATE_VERSION_V1 => (PAYLOAD_MAGIC_V1, ZONE_BYTES_V1, false),
            MULTISAMPLE_STATE_VERSION => (PAYLOAD_MAGIC_V2, ZONE_BYTES_V2, true),
            _ => return Err(DeviceContractError::UnsupportedStateVersion),
        };
        if payload.len() < HEADER_BYTES || payload[0..4] != expected_magic {
            return Err(DeviceContractError::MalformedState);
        }
        let overlap_policy = overlap_from_u8(payload[4])?;
        let voice_limit = payload[5];
        let zone_count = usize::from(u16::from_le_bytes([payload[6], payload[7]]));
        if zone_count > MAX_ZONES {
            return Err(DeviceContractError::MalformedState);
        }
        let expected_len = HEADER_BYTES
            .checked_add(
                zone_count
                    .checked_mul(zone_stride)
                    .ok_or(DeviceContractError::MalformedState)?,
            )
            .ok_or(DeviceContractError::MalformedState)?;
        if payload.len() != expected_len {
            return Err(DeviceContractError::MalformedState);
        }
        let gain = f32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
        let mut zones = Vec::new();
        zones
            .try_reserve_exact(zone_count)
            .map_err(|_| DeviceContractError::MalformedState)?;
        for index in 0..zone_count {
            let base = HEADER_BYTES + index * zone_stride;
            let raw_slot = u32::from_le_bytes([
                payload[base],
                payload[base + 1],
                payload[base + 2],
                payload[base + 3],
            ]);
            let slot =
                DeviceAssetSlot::new(raw_slot).map_err(|_| DeviceContractError::MalformedState)?;
            let low_key = payload[base + 4];
            let high_key = payload[base + 5];
            let root_key = payload[base + 6];
            let reserved = payload[base + 7];
            if reserved != 0 {
                return Err(DeviceContractError::MalformedState);
            }
            let (low_velocity, high_velocity, floats_offset) = if has_velocity {
                if payload[base + 10] != 0 || payload[base + 11] != 0 {
                    return Err(DeviceContractError::MalformedState);
                }
                (payload[base + 8], payload[base + 9], base + 12)
            } else {
                (MIN_VELOCITY, MAX_VELOCITY, base + 8)
            };
            let detune_cents = f32::from_le_bytes([
                payload[floats_offset],
                payload[floats_offset + 1],
                payload[floats_offset + 2],
                payload[floats_offset + 3],
            ]);
            let zone_gain = f32::from_le_bytes([
                payload[floats_offset + 4],
                payload[floats_offset + 5],
                payload[floats_offset + 6],
                payload[floats_offset + 7],
            ]);
            let release_seconds = f32::from_le_bytes([
                payload[floats_offset + 8],
                payload[floats_offset + 9],
                payload[floats_offset + 10],
                payload[floats_offset + 11],
            ]);
            zones.push(KeyZoneSpec {
                slot,
                low_key,
                high_key,
                low_velocity,
                high_velocity,
                root_key,
                detune_cents,
                gain: zone_gain,
                release_seconds,
            });
        }
        let state = Self {
            gain,
            overlap_policy,
            voice_limit,
            zones,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn to_native(&self) -> Result<NativeDeviceState, DeviceContractError> {
        let payload = self.encode_payload()?;
        NativeDeviceState::new(MULTISAMPLE_TYPE_ID, MULTISAMPLE_STATE_VERSION, payload)
    }

    /// Encode authored state and attach validated host `DeviceAssetReference`s.
    pub fn to_native_with_assets(
        &self,
        references: Vec<DeviceAssetReference>,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        validate_asset_references(&self.zones, &references)?;
        self.to_native()?.with_asset_references(references)
    }

    pub fn from_native(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != MULTISAMPLE_TYPE_ID {
            return Err(DeviceContractError::MalformedState);
        }
        let decoded = Self::decode_versioned_payload(state.state_version(), state.payload())?;
        validate_asset_references(&decoded.zones, state.asset_references())?;
        Ok(decoded)
    }
}

fn validate_asset_references(
    zones: &[KeyZoneSpec],
    references: &[DeviceAssetReference],
) -> Result<(), DeviceContractError> {
    if zones.is_empty() {
        if !references.is_empty() {
            return Err(DeviceContractError::MalformedState);
        }
        return Ok(());
    }
    if references.is_empty() || references.len() > MAX_DEVICE_ASSET_REFERENCES {
        return Err(DeviceContractError::MalformedState);
    }
    for zone in zones {
        if !references.iter().any(|r| r.slot == zone.slot) {
            return Err(DeviceContractError::MalformedState);
        }
    }
    for reference in references {
        if !zones.iter().any(|z| z.slot == reference.slot) {
            return Err(DeviceContractError::MalformedState);
        }
    }
    Ok(())
}

pub static MULTISAMPLE_DEFINITION: MultisampleDefinition = MultisampleDefinition;

pub struct MultisampleDefinition;

impl NativeDeviceMetadata for MultisampleDefinition {
    fn type_id(&self) -> DeviceTypeId {
        MULTISAMPLE_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("multisample")
    }

    fn state_version(&self) -> u32 {
        MULTISAMPLE_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::STEREO_INSTRUMENT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        MultisampleState::empty().to_native()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let authored = MultisampleState::from_native(state)?;
        if state.state_version() == MULTISAMPLE_STATE_VERSION {
            return Ok(state.clone());
        }
        authored.to_native()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let authored = MultisampleState::from_native(state)?;
        match parameter {
            GAIN => Ok(ParameterValue::Float(authored.gain)),
            OVERLAP_POLICY => Ok(ParameterValue::Choice(u32::from(overlap_to_u8(
                authored.overlap_policy,
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
            GAIN => 0,
            OVERLAP_POLICY => 1,
            VOICE_LIMIT => 2,
            _ => return Err(DeviceContractError::UnknownParameter),
        };
        DESCRIPTORS[index].validate(value)?;
        let mut authored = MultisampleState::from_native(state)?;
        match (parameter, value) {
            (GAIN, ParameterValue::Float(v)) => authored.gain = v,
            (OVERLAP_POLICY, ParameterValue::Choice(raw)) => {
                let raw_u8 =
                    u8::try_from(raw).map_err(|_| DeviceContractError::InvalidParameterValue)?;
                authored.overlap_policy = overlap_from_u8(raw_u8)?;
            }
            (VOICE_LIMIT, ParameterValue::Float(v)) => {
                let rounded = v.round();
                if !rounded.is_finite() || rounded < 1.0 || rounded > MAX_VOICES as f32 {
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

impl NativeInstrumentDefinition for MultisampleDefinition {
    fn resource_factory_inline_bound(&self) -> Option<usize> {
        Some(std::mem::size_of::<PreparedMultisampleInstrumentRuntime>())
    }

    fn supports_asset_resources(&self) -> bool {
        true
    }

    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        prepare_multisample(state, context, super::MAX_EVENTS_PER_BLOCK)
    }

    fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        prepare_multisample(state, context, limits.max_events_per_block)
    }
}

fn prepare_multisample(
    state: &NativeDeviceState,
    context: &DevicePreparationContext<'_>,
    max_events: usize,
) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
    if !(1..=MAX_INSTRUMENT_EVENTS).contains(&max_events)
        || !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&context.sample_rate)
    {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    let authored = MultisampleState::from_native(state)?;
    if authored.zones.is_empty() {
        if !context.resources.is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
        return Ok(Box::new(EmptyMultisample));
    }
    // Every declared host asset reference must exist in the borrowed inventory.
    for reference in state.asset_references() {
        let Some(view) = context.resources.slot(reference.slot) else {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        };
        if view.identity.asset != reference.asset {
            return Err(DeviceContractError::AssetResourcesUnavailable);
        }
    }
    let map_limits = MultisampleLimits {
        max_zones: MAX_ZONES,
        max_voices: usize::from(authored.voice_limit),
        max_events_per_block: max_events.min(super::MAX_EVENTS_PER_BLOCK),
        ..MultisampleLimits::DEFAULT
    };
    let map = PreparedMultisampleMap::prepare_on_control(
        &authored.zones,
        context.resources,
        context.sample_rate,
        context.tuning,
        authored.overlap_policy,
        map_limits,
    )
    .map_err(map_multisample_error)?;

    let mut zones = [None; MAX_ZONES];
    let mut max_tail_frames = 0_u64;
    let tuning_hz = *context.tuning.frequencies();
    for (index, &spec) in authored.zones.iter().enumerate() {
        let source = map
            .source_key(index)
            .ok_or(DeviceContractError::AssetResourcesUnavailable)?;
        let root_hz = tuning_hz[usize::from(spec.root_key)];
        let detune_ratio = 2.0_f64.powf(f64::from(spec.detune_cents) / 1200.0);
        let rate_ratio = f64::from(source.sample_rate()) / f64::from(context.sample_rate);
        let root_rate_scale = (detune_ratio * rate_ratio) / root_hz;
        let release_frames =
            (f64::from(spec.release_seconds) * f64::from(context.sample_rate)).round() as u64;
        max_tail_frames = max_tail_frames.max(release_frames);
        zones[index] = Some(InlinePreparedZone {
            low_key: spec.low_key,
            high_key: spec.high_key,
            low_velocity: spec.low_velocity,
            high_velocity: spec.high_velocity,
            root_key: spec.root_key,
            gain: f64::from(spec.gain),
            release_frames,
            root_rate_scale,
            source,
        });
    }

    let steal_fade_frames = ((u64::from(context.sample_rate) + 500) / 1000).max(1);
    max_tail_frames = max_tail_frames.max(steal_fade_frames);

    let factory = PreparedMultisampleInstrumentRuntime {
        output_sample_rate: context.sample_rate,
        overlap_policy: authored.overlap_policy,
        master_gain: f64::from(authored.gain),
        voice_limit: usize::from(authored.voice_limit),
        max_events,
        max_tail_frames,
        steal_fade_frames,
        zone_count: authored.zones.len(),
        tuning_hz,
        zones,
    };
    Ok(Box::new(PreparedMultisampleInstrument(factory)))
}

struct EmptyMultisample;

impl PreparedNativeInstrument for EmptyMultisample {
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

struct PreparedMultisampleInstrument(PreparedMultisampleInstrumentRuntime);

impl PreparedNativeInstrument for PreparedMultisampleInstrument {
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
        Ok(Some(Box::new(self.0.clone())))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct InlinePreparedZone {
    low_key: u8,
    high_key: u8,
    low_velocity: u8,
    high_velocity: u8,
    root_key: u8,
    gain: f64,
    release_frames: u64,
    root_rate_scale: f64,
    source: ZoneSourceKey,
}

impl InlinePreparedZone {
    #[inline]
    const fn contains(self, key: u8, velocity: u8) -> bool {
        key >= self.low_key
            && key <= self.high_key
            && velocity >= self.low_velocity
            && velocity <= self.high_velocity
    }
}

/// 100% inline fixed-storage factory for `hex.instrument.multisample`.
/// Holds zero PCM buffers, zero `Arc`s, and zero heap allocations.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedMultisampleInstrumentRuntime {
    output_sample_rate: u32,
    overlap_policy: ZoneOverlapPolicy,
    master_gain: f64,
    voice_limit: usize,
    max_events: usize,
    max_tail_frames: u64,
    steal_fade_frames: u64,
    zone_count: usize,
    tuning_hz: [f64; 128],
    zones: [Option<InlinePreparedZone>; MAX_ZONES],
}

impl PreparedMultisampleInstrumentRuntime {
    #[inline]
    fn select_zones(&self, key: u8, velocity: u8) -> SelectedZones {
        if key > 127 || !(MIN_VELOCITY..=MAX_VELOCITY).contains(&velocity) {
            return SelectedZones::default();
        }
        select_matching_zones(self.overlap_policy, key, self.zone_count, |idx| {
            self.zones[idx]
                .filter(|z| z.contains(key, velocity))
                .map(|z| z.root_key)
        })
        .unwrap_or_default()
    }
    #[inline]
    fn zone_step(&self, zone_index: usize, key: u8) -> f64 {
        let Some(zone) = self.zones[zone_index] else {
            return 0.0;
        };
        self.tuning_hz[usize::from(key)] * zone.root_rate_scale
    }

    fn resources_valid(&self, resources: &dyn DeviceResources) -> bool {
        let limit = resources.len().min(MAX_DEVICE_ASSET_REFERENCES);
        for index in 0..self.zone_count {
            let Some(zone) = self.zones[index] else {
                return false;
            };
            let found = (0..limit)
                .filter_map(|i| resources.get(i))
                .any(|view| zone.source.matches(view));
            if !found {
                return false;
            }
        }
        true
    }

    #[inline]
    fn find_resource<'a>(
        zone: &InlinePreparedZone,
        resources: &'a dyn DeviceResources,
    ) -> Option<DeviceResourceView<'a>> {
        let limit = resources.len().min(MAX_DEVICE_ASSET_REFERENCES);
        (0..limit)
            .filter_map(|i| resources.get(i))
            .find(|view| zone.source.matches(*view))
    }
}

impl PreparedMutableInstrumentRuntime for PreparedMultisampleInstrumentRuntime {
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
        MULTISAMPLE_STATE_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        std::mem::size_of::<MultisampleInstrumentRuntime>()
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
        Box::new(MultisampleInstrumentRuntime {
            prepared: self.clone(),
            active: [None; MAX_VOICES],
            fading: [None; MAX_VOICES],
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
    fn pitch_ratio(&self, voice_pitch_offset_cents: i16) -> f64 {
        if voice_pitch_offset_cents == 0 && self.channel_pitch_bend == 0 {
            return 1.0;
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
        2.0_f64.powf((f64::from(voice_pitch_offset_cents) + channel_cents) / 1200.0)
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
    zone_index: u8,
    velocity: u8,
    position: f64,
    base_step: f64,
    zone_gain: f64,
    age: u64,
    gate_age: Option<u64>,
    release_frames: u64,
    source_frames: usize,
    pitch_offset_cents: i16,
    pedal_held: bool,
}

#[derive(Clone, Copy, Debug)]
struct FadingVoice {
    voice: ActiveVoice,
    fade_start_age: u64,
    frozen_occurrence_gain: f64,
}

type ActivePool = [Option<ActiveVoice>; MAX_VOICES];
type FadingPool = [Option<FadingVoice>; MAX_VOICES];

enum StealOutcome {
    Ready,
    AllHeld,
    FadePoolFull,
}

/// Fixed-storage mutable runtime for `hex.instrument.multisample`.
pub struct MultisampleInstrumentRuntime {
    prepared: PreparedMultisampleInstrumentRuntime,
    active: ActivePool,
    fading: FadingPool,
    midi: MidiState,
}

impl MultisampleInstrumentRuntime {
    fn advance_voice(voice: &mut ActiveVoice, midi: &MidiState, frames: u64) -> bool {
        voice.age = voice.age.saturating_add(frames);
        let step = voice.base_step * midi.pitch_ratio(voice.pitch_offset_cents);
        voice.position += step * (frames as f64);
        let source_ended = voice.position >= (voice.source_frames as f64);
        let release_ended = voice
            .gate_age
            .is_some_and(|gate| voice.age.saturating_sub(gate) >= voice.release_frames);
        !(source_ended || release_ended)
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
                && !Self::advance_voice(voice, midi, frames)
            {
                *slot = None;
            }
        }
        let fade_len = self.prepared.steal_fade_frames;
        for slot in fading.iter_mut() {
            if let Some(fading_voice) = slot
                && (!Self::advance_voice(&mut fading_voice.voice, midi, frames)
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

    /// Ensure at least `needed` slots are free in `active[..voice_limit]` by
    /// stealing oldest releasing voices atomically into `fading`. If atomic
    /// admission fails, both `active` and `fading` remain untouched.
    fn ensure_voice_capacity(
        &self,
        active: &mut ActivePool,
        fading: &mut FadingPool,
        needed: usize,
        gain_at: &dyn Fn(u64) -> f64,
    ) -> StealOutcome {
        if needed > self.prepared.voice_limit {
            return StealOutcome::AllHeld;
        }
        let free = active[..self.prepared.voice_limit]
            .iter()
            .filter(|v| v.is_none())
            .count();
        if free >= needed {
            return StealOutcome::Ready;
        }
        let deficits = needed - free;
        let mut trial_active = *active;
        let mut trial_fading = *fading;
        for _ in 0..deficits {
            let mut oldest: Option<(usize, u64)> = None;
            for (index, entry) in trial_active[..self.prepared.voice_limit].iter().enumerate() {
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
            let Some(fade_idx) = trial_fading.iter().position(Option::is_none) else {
                return StealOutcome::FadePoolFull;
            };
            if let Some(victim) = trial_active[victim_idx].take() {
                trial_fading[fade_idx] = Some(FadingVoice {
                    frozen_occurrence_gain: gain_at(victim.occurrence_id),
                    fade_start_age: victim.age,
                    voice: victim,
                });
            }
        }
        *active = trial_active;
        *fading = trial_fading;
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
                                    if voice.gate_age.is_none() {
                                        voice.gate_age = Some(voice.age);
                                    }
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
                                    if voice.gate_age.is_none() {
                                        voice.gate_age = Some(voice.age);
                                    }
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
                    } else if voice.gate_age.is_none() {
                        voice.gate_age = Some(voice.age);
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
                let selection = self.prepared.select_zones(pitch, velocity);
                let selected = selection.as_slice();
                if selected.is_empty() {
                    self.advance_pools(active, fading, midi, 0);
                    return Ok(());
                }
                match self.ensure_voice_capacity(active, fading, selected.len(), gain_at) {
                    StealOutcome::Ready => {}
                    StealOutcome::FadePoolFull => {
                        self.advance_pools(active, fading, midi, 0);
                        return Ok(());
                    }
                    StealOutcome::AllHeld => {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    }
                }
                for &zone_idx in selected {
                    let Some(zone) = self.prepared.zones[usize::from(zone_idx)] else {
                        return Err(NativeInstrumentRuntimeError::ResourceUnavailable);
                    };
                    let step = self.prepared.zone_step(usize::from(zone_idx), pitch);
                    if let Some(slot) = active[..self.prepared.voice_limit]
                        .iter_mut()
                        .find(|entry| entry.is_none())
                    {
                        *slot = Some(ActiveVoice {
                            occurrence_id,
                            zone_index: zone_idx,
                            velocity,
                            position: 0.0,
                            base_step: step,
                            zone_gain: zone.gain,
                            age: 0,
                            gate_age: None,
                            release_frames: zone.release_frames,
                            source_frames: zone.source.frames(),
                            pitch_offset_cents: 0,
                            pedal_held: false,
                        });
                    }
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

    fn sample_voice(
        &self,
        voice: &ActiveVoice,
        extra_gain: f64,
        resources: &dyn DeviceResources,
    ) -> [f64; 2] {
        let Some(zone) = self.prepared.zones[usize::from(voice.zone_index)] else {
            return [0.0; 2];
        };
        let Some(view) = PreparedMultisampleInstrumentRuntime::find_resource(&zone, resources)
        else {
            return [0.0; 2];
        };
        let env = match voice.gate_age {
            None => 1.0,
            Some(gate) => {
                let rel = voice.age.saturating_sub(gate);
                if voice.release_frames == 0 || rel >= voice.release_frames {
                    0.0
                } else {
                    1.0 - (rel as f64 / voice.release_frames as f64)
                }
            }
        };
        if env <= 0.0 || voice.position >= (voice.source_frames as f64) {
            return [0.0; 2];
        }
        let sample = interpolate_linear(view.pcm, voice.position);
        let vel = f64::from(voice.velocity) / 127.0;
        let scale = env
            * voice.zone_gain
            * vel
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
        if !self.prepared.resources_valid(resources) {
            frames.fill([0.0; 2]);
            return;
        }
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
                let s = self.sample_voice(voice, occ_gain, resources);
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
                        resources,
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

impl MutableInstrumentRuntime for MultisampleInstrumentRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        true
    }

    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        let mut ids = [0_u64; MAX_VOICES];
        let mut count = 0_usize;
        for voice in self.active[..self.prepared.voice_limit].iter().flatten() {
            if !ids[..count].contains(&voice.occurrence_id) {
                ids[count] = voice.occurrence_id;
                count += 1;
            }
        }
        if output.len() < count {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        ids[..count].sort_unstable();
        output[..count].copy_from_slice(&ids[..count]);
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
            || gains.len() > MAX_INSTRUMENT_EVENTS + MAX_VOICES
        {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut known = [false; MAX_INSTRUMENT_EVENTS + MAX_VOICES];
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
        if !self.prepared.resources_valid(resources) {
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
