//! Bounded nested native Instrument Rack for one retained native source
//! (`INSTRUMENT_RACK_MAX_DEPTH == 2`).
//!
//! Fans a single host event stream (`NativeInstrumentEvent` with stable
//! `occurrence_id`s, paired note-offs, MPE, and shared controller events) to two
//! independent native instrument branches (or bounded depth-2 nested Instrument
//! Racks) prepared through the existing `NativeInstrumentRegistry` (no parallel
//! registry, no per-track clones), aligns branch algorithmic latencies across
//! levels with exact-capacity control-allocated delay rings, and sums weighted
//! branch outputs once.
//!
//! Nesting is explicitly bounded at `INSTRUMENT_RACK_MAX_DEPTH == 2`: deeper
//! nesting (`depth > 2`), duplicate rack/branch/device identities across levels
//! (cycles/self-reference), and budget overflows are refused on control before
//! touching active state. All child runtimes, delay rings, and chunk scratch
//! buffers are allocated on control; callback processing performs zero
//! allocations, locks, I/O, or final drops.

#[cfg(test)]
mod tests;

use crate::{
    DevicePreparationContext, InstrumentHistoryRetargetPreparation, MAX_MAIN_PASS_DELAY_FRAMES,
    MutableInstrumentRuntime, NativeInstrumentClock, NativeInstrumentDefinition,
    NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRegistry,
    NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits, OffsetClock, PreparedAnalyticNote,
    PreparedInstrumentHistoryRetarget, PreparedMutableInstrumentRuntime, PreparedNativeInstrument,
    StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
use std::{any::Any, num::NonZeroU64};

/// Reserved native instrument type ID for a bounded two-branch Instrument Rack.
pub const INSTRUMENT_RACK_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rack.instrument");
/// Prior v1 wire format (full-range layer routing) migrated cleanly to v2.
pub const INSTRUMENT_RACK_STATE_VERSION_V1: u32 = 1;
/// Current v2 wire format adding per-branch key/velocity zones and overlap policy.
pub const INSTRUMENT_RACK_STATE_VERSION: u32 = 2;
pub const INSTRUMENT_RACK_RUNTIME_VERSION: u32 = 2;
pub const INSTRUMENT_RACK_BRANCH_COUNT: usize = 2;
/// Explicit nesting ceiling: outer Instrument Rack (depth 1) may host nested
/// Instrument Racks (depth 2) whose branches contain flat native instruments only.
pub const INSTRUMENT_RACK_MAX_DEPTH: usize = 2;
pub const INSTRUMENT_RACK_MAX_BRANCH_GAIN: f32 = 4.0;
pub const INSTRUMENT_RACK_MAX_COMPENSATION_DELAY_FRAMES: u32 = MAX_MAIN_PASS_DELAY_FRAMES;
pub const INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK: usize = 259;
pub const INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES: usize = 64;
const INSTRUMENT_RACK_CHUNK_FRAMES: usize = 256;
const RACK_STATE_MAGIC: &[u8; 4] = b"HXRK";

pub const PARAM_BRANCH_1_GAIN: ParameterId = ParameterId::from_static(1);
pub const PARAM_BRANCH_1_BYPASS: ParameterId = ParameterId::from_static(2);
pub const PARAM_BRANCH_2_GAIN: ParameterId = ParameterId::from_static(3);
pub const PARAM_BRANCH_2_BYPASS: ParameterId = ParameterId::from_static(4);

const DESCRIPTORS: [ParameterDescriptor; 4] = [
    ParameterDescriptor {
        id: PARAM_BRANCH_1_GAIN,
        label: "branch 1 gain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: INSTRUMENT_RACK_MAX_BRANCH_GAIN,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PARAM_BRANCH_1_BYPASS,
        label: "branch 1 bypass",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PARAM_BRANCH_2_GAIN,
        label: "branch 2 gain",
        kind: ParameterKind::Float {
            min: 0.0,
            max: INSTRUMENT_RACK_MAX_BRANCH_GAIN,
            unit: None,
        },
        default: ParameterValue::Float(1.0),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PARAM_BRANCH_2_BYPASS,
        label: "branch 2 bypass",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
];

/// Canonical key and velocity selection zone (with optional linear fade edges)
/// for one Instrument Rack branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstrumentRackZone {
    pub key_low: u8,
    pub key_fade_low: u8,
    pub key_fade_high: u8,
    pub key_high: u8,
    pub vel_low: u8,
    pub vel_fade_low: u8,
    pub vel_fade_high: u8,
    pub vel_high: u8,
}

impl Default for InstrumentRackZone {
    fn default() -> Self {
        Self {
            key_low: 0,
            key_fade_low: 0,
            key_fade_high: 127,
            key_high: 127,
            vel_low: 1,
            vel_fade_low: 1,
            vel_fade_high: 127,
            vel_high: 127,
        }
    }
}

impl InstrumentRackZone {
    /// Hard-edged key and velocity range with unity gain inside bounds.
    pub const fn hard(key_low: u8, key_high: u8, vel_low: u8, vel_high: u8) -> Self {
        Self {
            key_low,
            key_fade_low: key_low,
            key_fade_high: key_high,
            key_high,
            vel_low,
            vel_fade_low: vel_low,
            vel_fade_high: vel_high,
            vel_high,
        }
    }

    pub const fn has_fade(&self) -> bool {
        self.key_fade_low > self.key_low
            || self.key_fade_high < self.key_high
            || self.vel_fade_low > self.vel_low
            || self.vel_fade_high < self.vel_high
    }

    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if self.key_low > self.key_fade_low
            || self.key_fade_low > self.key_fade_high
            || self.key_fade_high > self.key_high
            || self.key_high > 127
            || self.vel_low == 0
            || self.vel_low > self.vel_fade_low
            || self.vel_fade_low > self.vel_fade_high
            || self.vel_fade_high > self.vel_high
            || self.vel_high > 127
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(())
    }

    /// Evaluate deterministic zone membership and linear fade gain in `(0.0, 1.0]`.
    /// Returns `None` when the note falls outside either key or velocity bounds.
    pub fn evaluate_gain(&self, pitch: u8, velocity: u8) -> Option<f32> {
        if pitch < self.key_low
            || pitch > self.key_high
            || velocity < self.vel_low
            || velocity > self.vel_high
        {
            return None;
        }
        let key_gain = if pitch < self.key_fade_low {
            (f32::from(pitch - self.key_low) + 1.0)
                / (f32::from(self.key_fade_low - self.key_low) + 1.0)
        } else if pitch > self.key_fade_high {
            (f32::from(self.key_high - pitch) + 1.0)
                / (f32::from(self.key_high - self.key_fade_high) + 1.0)
        } else {
            1.0
        };
        let vel_gain = if velocity < self.vel_fade_low {
            (f32::from(velocity - self.vel_low) + 1.0)
                / (f32::from(self.vel_fade_low - self.vel_low) + 1.0)
        } else if velocity > self.vel_fade_high {
            (f32::from(self.vel_high - velocity) + 1.0)
                / (f32::from(self.vel_high - self.vel_fade_high) + 1.0)
        } else {
            1.0
        };
        Some((key_gain * vel_gain).clamp(0.0, 1.0))
    }
}

/// Deterministic overlap resolution policy when multiple branches match a note.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstrumentRackOverlapPolicy {
    /// Every matching active branch sounds with its evaluated zone fade gain.
    #[default]
    Layer,
    /// Only the first matching active branch in canonical order (branch 0 before 1) sounds.
    ExclusiveFirstMatch,
}

/// Authored specification for one branch of a bounded two-branch Instrument Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentRackBranchSpec {
    pub branch_id: NonZeroU64,
    pub device_id: NonZeroU64,
    pub gain: f32,
    pub bypassed: bool,
    pub zone: InstrumentRackZone,
    /// `None` represents an empty or removed instrument slot.
    pub state: Option<NativeDeviceState>,
}

/// Authored specification for a bounded two-branch Instrument Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentRackSpec {
    pub rack_id: NonZeroU64,
    pub overlap_policy: InstrumentRackOverlapPolicy,
    pub branches: [InstrumentRackBranchSpec; INSTRUMENT_RACK_BRANCH_COUNT],
}

impl InstrumentRackSpec {
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        let mut seen_racks = [0_u64; 3];
        let mut seen_racks_len = 0_usize;
        let mut seen_branches = [0_u64; 6];
        let mut seen_branches_len = 0_usize;
        let mut seen_devices = [0_u64; 6];
        let mut seen_devices_len = 0_usize;
        self.validate_at_depth(
            1,
            &mut seen_racks,
            &mut seen_racks_len,
            &mut seen_branches,
            &mut seen_branches_len,
            &mut seen_devices,
            &mut seen_devices_len,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_at_depth(
        &self,
        depth: usize,
        seen_racks: &mut [u64; 3],
        seen_racks_len: &mut usize,
        seen_branches: &mut [u64; 6],
        seen_branches_len: &mut usize,
        seen_devices: &mut [u64; 6],
        seen_devices_len: &mut usize,
    ) -> Result<(), DeviceContractError> {
        if depth == 0 || depth > INSTRUMENT_RACK_MAX_DEPTH {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let rack_raw = self.rack_id.get();
        if seen_racks[..*seen_racks_len].contains(&rack_raw) {
            return Err(DeviceContractError::InvalidId);
        }
        if *seen_racks_len >= seen_racks.len() {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        seen_racks[*seen_racks_len] = rack_raw;
        *seen_racks_len += 1;

        for branch in &self.branches {
            let branch_raw = branch.branch_id.get();
            if seen_branches[..*seen_branches_len].contains(&branch_raw) {
                return Err(DeviceContractError::InvalidId);
            }
            if *seen_branches_len >= seen_branches.len() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            seen_branches[*seen_branches_len] = branch_raw;
            *seen_branches_len += 1;

            let dev_raw = branch.device_id.get();
            if seen_devices[..*seen_devices_len].contains(&dev_raw) {
                return Err(DeviceContractError::InvalidId);
            }
            if *seen_devices_len >= seen_devices.len() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            seen_devices[*seen_devices_len] = dev_raw;
            *seen_devices_len += 1;

            if !branch.gain.is_finite()
                || !(0.0..=INSTRUMENT_RACK_MAX_BRANCH_GAIN).contains(&branch.gain)
            {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            branch.zone.validate()?;
            if let Some(state) = &branch.state {
                state.type_id().validate()?;
                if state.state_version() == 0 {
                    return Err(DeviceContractError::InvalidStateVersion);
                }
                if !state.asset_references().is_empty() {
                    return Err(DeviceContractError::AssetResourcesUnavailable);
                }
                if state.type_id() == INSTRUMENT_RACK_TYPE_ID {
                    if depth >= INSTRUMENT_RACK_MAX_DEPTH {
                        return Err(DeviceContractError::UnsupportedProcessingClass);
                    }
                    let nested = Self::decode_state_raw(state, true)?;
                    nested.validate_at_depth(
                        depth + 1,
                        seen_racks,
                        seen_racks_len,
                        seen_branches,
                        seen_branches_len,
                        seen_devices,
                        seen_devices_len,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Encode this rack spec into a strictly validated v2 `NativeDeviceState`
    /// envelope compatible with `hex-project`'s retained `NativeSource` storage.
    pub fn encode_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        self.encode_state_version(INSTRUMENT_RACK_STATE_VERSION)
    }

    /// Encode a default-zone layer rack into the prior v1 wire format for
    /// migration testing. Refuses non-default zones or overlap policies.
    pub fn encode_v1_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        if self.overlap_policy != InstrumentRackOverlapPolicy::Layer
            || self
                .branches
                .iter()
                .any(|b| b.zone != InstrumentRackZone::default())
        {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        self.encode_state_version(INSTRUMENT_RACK_STATE_VERSION_V1)
    }

    fn encode_state_version(&self, version: u32) -> Result<NativeDeviceState, DeviceContractError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(RACK_STATE_MAGIC);
        bytes.extend_from_slice(&self.rack_id.get().to_le_bytes());
        if version >= INSTRUMENT_RACK_STATE_VERSION {
            bytes.push(match self.overlap_policy {
                InstrumentRackOverlapPolicy::Layer => 0,
                InstrumentRackOverlapPolicy::ExclusiveFirstMatch => 1,
            });
        }
        for branch in &self.branches {
            bytes.extend_from_slice(&branch.branch_id.get().to_le_bytes());
            bytes.extend_from_slice(&branch.device_id.get().to_le_bytes());
            bytes.extend_from_slice(&branch.gain.to_le_bytes());
            bytes.push(u8::from(branch.bypassed));
            if version >= INSTRUMENT_RACK_STATE_VERSION {
                bytes.extend_from_slice(&[
                    branch.zone.key_low,
                    branch.zone.key_fade_low,
                    branch.zone.key_fade_high,
                    branch.zone.key_high,
                    branch.zone.vel_low,
                    branch.zone.vel_fade_low,
                    branch.zone.vel_fade_high,
                    branch.zone.vel_high,
                ]);
            }
            match &branch.state {
                None => bytes.push(0),
                Some(state) => {
                    bytes.push(1);
                    let child_type = state.type_id();
                    let type_str = child_type.as_str()?;
                    let type_len = u16::try_from(type_str.len())
                        .map_err(|_| DeviceContractError::StateTooLarge)?;
                    let payload_len = u32::try_from(state.payload().len())
                        .map_err(|_| DeviceContractError::StateTooLarge)?;
                    bytes.extend_from_slice(&type_len.to_le_bytes());
                    bytes.extend_from_slice(type_str.as_bytes());
                    bytes.extend_from_slice(&state.state_version().to_le_bytes());
                    bytes.extend_from_slice(&state.input_gain_millidb().to_le_bytes());
                    bytes.extend_from_slice(&state.output_gain_millidb().to_le_bytes());
                    bytes.extend_from_slice(&payload_len.to_le_bytes());
                    bytes.extend_from_slice(state.payload());
                }
            }
            if bytes.len() > MAX_NATIVE_DEVICE_STATE_BYTES {
                return Err(DeviceContractError::StateTooLarge);
            }
        }
        NativeDeviceState::new(INSTRUMENT_RACK_TYPE_ID, version, bytes)
    }

    /// Strictly decode and validate a current v2 Instrument Rack `NativeDeviceState`.
    pub fn decode_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        let spec = Self::decode_state_raw(state, false)?;
        spec.validate()?;
        Ok(spec)
    }

    /// Decode and migrate a v1 or v2 Instrument Rack `NativeDeviceState` into
    /// the current canonical `InstrumentRackSpec`.
    pub fn decode_and_migrate_state(
        state: &NativeDeviceState,
    ) -> Result<Self, DeviceContractError> {
        let spec = Self::decode_state_raw(state, true)?;
        spec.validate()?;
        Ok(spec)
    }

    fn decode_state_raw(
        state: &NativeDeviceState,
        allow_v1: bool,
    ) -> Result<Self, DeviceContractError> {
        if state.type_id() != INSTRUMENT_RACK_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        let version = state.state_version();
        let valid_version = version == INSTRUMENT_RACK_STATE_VERSION
            || (allow_v1 && version == INSTRUMENT_RACK_STATE_VERSION_V1);
        if !valid_version {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let payload = state.payload();
        let mut cursor = 0_usize;
        let read_bytes = |cursor: &mut usize, len: usize| -> Result<&[u8], DeviceContractError> {
            let end = cursor
                .checked_add(len)
                .ok_or(DeviceContractError::MalformedState)?;
            let slice = payload
                .get(*cursor..end)
                .ok_or(DeviceContractError::MalformedState)?;
            *cursor = end;
            Ok(slice)
        };

        if read_bytes(&mut cursor, 4)? != RACK_STATE_MAGIC {
            return Err(DeviceContractError::MalformedState);
        }
        let rack_raw = u64::from_le_bytes(
            read_bytes(&mut cursor, 8)?
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        let rack_id = NonZeroU64::new(rack_raw).ok_or(DeviceContractError::InvalidId)?;
        let overlap_policy = if version >= INSTRUMENT_RACK_STATE_VERSION {
            match read_bytes(&mut cursor, 1)?[0] {
                0 => InstrumentRackOverlapPolicy::Layer,
                1 => InstrumentRackOverlapPolicy::ExclusiveFirstMatch,
                _ => return Err(DeviceContractError::MalformedState),
            }
        } else {
            InstrumentRackOverlapPolicy::Layer
        };

        let mut branches = Vec::with_capacity(INSTRUMENT_RACK_BRANCH_COUNT);
        for _ in 0..INSTRUMENT_RACK_BRANCH_COUNT {
            let branch_raw = u64::from_le_bytes(
                read_bytes(&mut cursor, 8)?
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            let branch_id = NonZeroU64::new(branch_raw).ok_or(DeviceContractError::InvalidId)?;
            let device_raw = u64::from_le_bytes(
                read_bytes(&mut cursor, 8)?
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            let device_id = NonZeroU64::new(device_raw).ok_or(DeviceContractError::InvalidId)?;
            let gain = f32::from_le_bytes(
                read_bytes(&mut cursor, 4)?
                    .try_into()
                    .map_err(|_| DeviceContractError::MalformedState)?,
            );
            let bypassed = match read_bytes(&mut cursor, 1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
            let zone = if version >= INSTRUMENT_RACK_STATE_VERSION {
                let z = read_bytes(&mut cursor, 8)?;
                InstrumentRackZone {
                    key_low: z[0],
                    key_fade_low: z[1],
                    key_fade_high: z[2],
                    key_high: z[3],
                    vel_low: z[4],
                    vel_fade_low: z[5],
                    vel_fade_high: z[6],
                    vel_high: z[7],
                }
            } else {
                InstrumentRackZone::default()
            };
            let has_state = match read_bytes(&mut cursor, 1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(DeviceContractError::MalformedState),
            };
            let child_state = if has_state {
                let type_len = usize::from(u16::from_le_bytes(
                    read_bytes(&mut cursor, 2)?
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                ));
                let type_str = std::str::from_utf8(read_bytes(&mut cursor, type_len)?)
                    .map_err(|_| DeviceContractError::MalformedState)?;
                let child_type = DeviceTypeId::new(type_str)?;
                let child_version = u32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                );
                let input_gain_millidb = i32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                );
                let output_gain_millidb = i32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                );
                let child_payload_len = usize::try_from(u32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| DeviceContractError::MalformedState)?,
                ))
                .map_err(|_| DeviceContractError::MalformedState)?;
                let child_payload = read_bytes(&mut cursor, child_payload_len)?.to_vec();
                let decoded = NativeDeviceState::new(child_type, child_version, child_payload)?
                    .with_input_gain_millidb(input_gain_millidb)?
                    .with_output_gain_millidb(output_gain_millidb)?;
                Some(decoded)
            } else {
                None
            };
            branches.push(InstrumentRackBranchSpec {
                branch_id,
                device_id,
                gain,
                bypassed,
                zone,
                state: child_state,
            });
        }
        if cursor != payload.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let [branch_a, branch_b]: [InstrumentRackBranchSpec; INSTRUMENT_RACK_BRANCH_COUNT] =
            branches
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(Self {
            rack_id,
            overlap_policy,
            branches: [branch_a, branch_b],
        })
    }
}

/// Registered native Instrument Rack definition delegating child preparation
/// directly to an existing `NativeInstrumentRegistry` (no parallel registry).
pub struct InstrumentRackDefinition<'a> {
    children: NativeInstrumentRegistry<'a>,
}

impl<'a> InstrumentRackDefinition<'a> {
    pub fn new(children: NativeInstrumentRegistry<'a>) -> Result<Self, DeviceContractError> {
        if children.definition(INSTRUMENT_RACK_TYPE_ID).is_some() {
            return Err(DeviceContractError::DuplicateDeviceType);
        }
        Ok(Self { children })
    }

    fn prepare_rack(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        limits: Option<NativeInstrumentRuntimeLimits>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let spec = InstrumentRackSpec::decode_and_migrate_state(state)?;
        self.prepare_spec_at_depth(&spec, context, limits, 1)
    }

    fn migrate_spec_at_depth(
        &self,
        spec: &mut InstrumentRackSpec,
        depth: usize,
    ) -> Result<(), DeviceContractError> {
        if depth == 0 || depth > INSTRUMENT_RACK_MAX_DEPTH {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        for branch in &mut spec.branches {
            if let Some(child) = &branch.state {
                if child.type_id() == INSTRUMENT_RACK_TYPE_ID {
                    if depth >= INSTRUMENT_RACK_MAX_DEPTH {
                        return Err(DeviceContractError::UnsupportedProcessingClass);
                    }
                    let mut nested = InstrumentRackSpec::decode_and_migrate_state(child)?;
                    self.migrate_spec_at_depth(&mut nested, depth + 1)?;
                    branch.state = Some(nested.encode_state()?);
                } else {
                    branch.state = Some(self.children.migrate_state(child)?);
                }
            }
        }
        Ok(())
    }

    fn prepare_spec_at_depth(
        &self,
        spec: &InstrumentRackSpec,
        context: &DevicePreparationContext<'_>,
        limits: Option<NativeInstrumentRuntimeLimits>,
        depth: usize,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        if depth == 0 || depth > INSTRUMENT_RACK_MAX_DEPTH {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let mut prepared_branches: [Option<PreparedBranchFactory>; INSTRUMENT_RACK_BRANCH_COUNT] =
            [None, None];
        let mut branch_latencies = [0_u32; INSTRUMENT_RACK_BRANCH_COUNT];

        for (index, branch) in spec.branches.iter().enumerate() {
            let Some(child_state) = &branch.state else {
                continue;
            };
            let prepared_inst = if child_state.type_id() == INSTRUMENT_RACK_TYPE_ID {
                if depth >= INSTRUMENT_RACK_MAX_DEPTH {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
                let nested_spec = InstrumentRackSpec::decode_and_migrate_state(child_state)?;
                self.prepare_spec_at_depth(&nested_spec, context, limits, depth + 1)?
            } else {
                match limits {
                    Some(host_limits) => self.children.prepare_instrument_with_runtime_limits(
                        child_state,
                        context,
                        host_limits,
                    )?,
                    None => self.children.prepare_instrument(child_state, context)?,
                }
            };
            let factory = prepared_inst
                .prepare_mutable_runtime()?
                .ok_or(DeviceContractError::UnsupportedProcessingClass)?;
            if branch.zone.has_fade() && !factory.supports_occurrence_gain() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            let latency = factory
                .algorithmic_latency_frames()
                .or_else(|| prepared_inst.algorithmic_latency_frames())
                .unwrap_or(0);
            branch_latencies[index] = latency;
            prepared_branches[index] = Some(PreparedBranchFactory {
                branch_id: branch.branch_id,
                device_id: branch.device_id,
                gain: branch.gain,
                bypassed: branch.bypassed,
                zone: branch.zone,
                compensation_delay_frames: 0,
                factory,
            });
        }

        let active_lat_a = prepared_branches[0]
            .as_ref()
            .filter(|b| !b.bypassed)
            .map_or(0, |_| branch_latencies[0]);
        let active_lat_b = prepared_branches[1]
            .as_ref()
            .filter(|b| !b.bypassed)
            .map_or(0, |_| branch_latencies[1]);
        let rack_latency_frames = active_lat_a.max(active_lat_b);
        if rack_latency_frames > INSTRUMENT_RACK_MAX_COMPENSATION_DELAY_FRAMES {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }

        for (index, slot) in prepared_branches.iter_mut().enumerate() {
            if let Some(branch) = slot.as_mut()
                && !branch.bypassed
            {
                let comp = rack_latency_frames.saturating_sub(branch_latencies[index]);
                if comp > INSTRUMENT_RACK_MAX_COMPENSATION_DELAY_FRAMES {
                    return Err(DeviceContractError::RuntimeBudgetExceeded);
                }
                branch.compensation_delay_frames = comp;
            }
        }

        let mut max_events = INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK;
        let mut max_active = INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES;
        let mut max_tail = 0_u64;
        let mut supports_gain = true;
        let mut supports_active_ids = true;
        let mut steals_releasing = true;
        let mut any_active = false;
        let mut twin_bytes = std::mem::size_of::<InstrumentRackRuntime>();

        for branch in prepared_branches.iter().flatten() {
            if branch.bypassed {
                continue;
            }
            any_active = true;
            max_events = max_events.min(branch.factory.max_events_per_block());
            max_active = max_active.min(branch.factory.max_active_occurrences());
            let branch_tail = branch
                .factory
                .max_release_tail_frames()
                .checked_add(u64::from(branch.compensation_delay_frames))
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            max_tail = max_tail.max(branch_tail);
            supports_gain &= branch.factory.supports_occurrence_gain();
            supports_active_ids &= branch.factory.supports_active_occurrence_ids();
            steals_releasing &= branch.factory.steals_releasing_voices();
            let delay_bytes = usize::try_from(branch.compensation_delay_frames)
                .map_err(|_| DeviceContractError::RuntimeBudgetExceeded)?
                .checked_mul(std::mem::size_of::<StereoFrame>())
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            twin_bytes = twin_bytes
                .checked_add(branch.factory.declared_twin_bytes())
                .and_then(|sum| sum.checked_add(delay_bytes))
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        }

        if !any_active {
            supports_gain = true;
            supports_active_ids = true;
            steals_releasing = false;
        }
        if max_events == 0 || max_active == 0 {
            return Err(DeviceContractError::RuntimeBudgetExceeded);
        }
        if let Some(host_limits) = limits {
            let pair_bytes = twin_bytes
                .checked_mul(2)
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
            if pair_bytes > host_limits.max_pair_bytes
                || max_tail > host_limits.max_tail_frames
                || max_events > host_limits.max_events_per_block
                || max_active > host_limits.max_active_occurrences
            {
                return Err(DeviceContractError::RuntimeBudgetExceeded);
            }
        }

        Ok(Box::new(PreparedInstrumentRack {
            rack_id: spec.rack_id,
            overlap_policy: spec.overlap_policy,
            sample_rate: context.sample_rate,
            rack_latency_frames,
            max_events_per_block: max_events,
            max_active_occurrences: max_active,
            max_release_tail_frames: max_tail,
            declared_twin_bytes: twin_bytes,
            supports_occurrence_gain: supports_gain,
            supports_active_occurrence_ids: supports_active_ids,
            steals_releasing_voices: steals_releasing,
            branches: std::sync::Arc::new(prepared_branches),
        }))
    }
}

impl NativeDeviceMetadata for InstrumentRackDefinition<'_> {
    fn type_id(&self) -> DeviceTypeId {
        INSTRUMENT_RACK_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("instrument rack")
    }

    fn state_version(&self) -> u32 {
        INSTRUMENT_RACK_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        let Some(rack_id) = NonZeroU64::new(1) else {
            return Err(DeviceContractError::InvalidId);
        };
        let Some(branch_1) = NonZeroU64::new(1) else {
            return Err(DeviceContractError::InvalidId);
        };
        let Some(branch_2) = NonZeroU64::new(2) else {
            return Err(DeviceContractError::InvalidId);
        };
        let Some(dev_1) = NonZeroU64::new(1) else {
            return Err(DeviceContractError::InvalidId);
        };
        let Some(dev_2) = NonZeroU64::new(2) else {
            return Err(DeviceContractError::InvalidId);
        };
        InstrumentRackSpec {
            rack_id,
            overlap_policy: InstrumentRackOverlapPolicy::Layer,
            branches: [
                InstrumentRackBranchSpec {
                    branch_id: branch_1,
                    device_id: dev_1,
                    gain: 1.0,
                    bypassed: false,
                    zone: InstrumentRackZone::default(),
                    state: None,
                },
                InstrumentRackBranchSpec {
                    branch_id: branch_2,
                    device_id: dev_2,
                    gain: 1.0,
                    bypassed: false,
                    zone: InstrumentRackZone::default(),
                    state: None,
                },
            ],
        }
        .encode_state()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut spec = InstrumentRackSpec::decode_and_migrate_state(state)?;
        self.migrate_spec_at_depth(&mut spec, 1)?;
        spec.encode_state()
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let spec = InstrumentRackSpec::decode_and_migrate_state(state)?;
        match parameter {
            PARAM_BRANCH_1_GAIN => Ok(ParameterValue::Float(spec.branches[0].gain)),
            PARAM_BRANCH_1_BYPASS => Ok(ParameterValue::Bool(spec.branches[0].bypassed)),
            PARAM_BRANCH_2_GAIN => Ok(ParameterValue::Float(spec.branches[1].gain)),
            PARAM_BRANCH_2_BYPASS => Ok(ParameterValue::Bool(spec.branches[1].bypassed)),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut spec = InstrumentRackSpec::decode_and_migrate_state(state)?;
        match (parameter, value) {
            (PARAM_BRANCH_1_GAIN, ParameterValue::Float(v))
                if v.is_finite() && (0.0..=INSTRUMENT_RACK_MAX_BRANCH_GAIN).contains(&v) =>
            {
                spec.branches[0].gain = v;
            }
            (PARAM_BRANCH_1_BYPASS, ParameterValue::Bool(v)) => {
                spec.branches[0].bypassed = v;
            }
            (PARAM_BRANCH_2_GAIN, ParameterValue::Float(v))
                if v.is_finite() && (0.0..=INSTRUMENT_RACK_MAX_BRANCH_GAIN).contains(&v) =>
            {
                spec.branches[1].gain = v;
            }
            (PARAM_BRANCH_2_BYPASS, ParameterValue::Bool(v)) => {
                spec.branches[1].bypassed = v;
            }
            (
                PARAM_BRANCH_1_GAIN
                | PARAM_BRANCH_1_BYPASS
                | PARAM_BRANCH_2_GAIN
                | PARAM_BRANCH_2_BYPASS,
                _,
            ) => return Err(DeviceContractError::InvalidParameterValue),
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        spec.encode_state()
    }
}

impl NativeInstrumentDefinition for InstrumentRackDefinition<'_> {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_rack(state, context, None)
    }

    fn prepare_instrument_with_runtime_limits(
        &self,
        state: &NativeDeviceState,
        context: &DevicePreparationContext<'_>,
        limits: NativeInstrumentRuntimeLimits,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        self.prepare_rack(state, context, Some(limits))
    }
}

struct PreparedBranchFactory {
    branch_id: NonZeroU64,
    device_id: NonZeroU64,
    gain: f32,
    bypassed: bool,
    zone: InstrumentRackZone,
    compensation_delay_frames: u32,
    factory: Box<dyn PreparedMutableInstrumentRuntime>,
}

struct PreparedInstrumentRack {
    rack_id: NonZeroU64,
    overlap_policy: InstrumentRackOverlapPolicy,
    sample_rate: u32,
    rack_latency_frames: u32,
    max_events_per_block: usize,
    max_active_occurrences: usize,
    max_release_tail_frames: u64,
    declared_twin_bytes: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    steals_releasing_voices: bool,
    branches: std::sync::Arc<[Option<PreparedBranchFactory>; INSTRUMENT_RACK_BRANCH_COUNT]>,
}

impl PreparedNativeInstrument for PreparedInstrumentRack {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.rack_latency_frames)
    }

    fn prepare_note(
        &self,
        _pitch: u8,
        _velocity: u8,
    ) -> Result<PreparedAnalyticNote, DeviceContractError> {
        Err(DeviceContractError::UnsupportedProcessingClass)
    }

    fn prepare_mutable_runtime(
        &self,
    ) -> Result<Option<Box<dyn PreparedMutableInstrumentRuntime>>, DeviceContractError> {
        Ok(Some(Box::new(PreparedInstrumentRackRuntime {
            rack_id: self.rack_id,
            overlap_policy: self.overlap_policy,
            sample_rate: self.sample_rate,
            rack_latency_frames: self.rack_latency_frames,
            max_events_per_block: self.max_events_per_block,
            max_active_occurrences: self.max_active_occurrences,
            max_release_tail_frames: self.max_release_tail_frames,
            declared_twin_bytes: self.declared_twin_bytes,
            supports_occurrence_gain: self.supports_occurrence_gain,
            supports_active_occurrence_ids: self.supports_active_occurrence_ids,
            steals_releasing_voices: self.steals_releasing_voices,
            branches: std::sync::Arc::clone(&self.branches),
        })))
    }
}

struct PreparedInstrumentRackRuntime {
    rack_id: NonZeroU64,
    overlap_policy: InstrumentRackOverlapPolicy,
    sample_rate: u32,
    rack_latency_frames: u32,
    max_events_per_block: usize,
    max_active_occurrences: usize,
    max_release_tail_frames: u64,
    declared_twin_bytes: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    steals_releasing_voices: bool,
    branches: std::sync::Arc<[Option<PreparedBranchFactory>; INSTRUMENT_RACK_BRANCH_COUNT]>,
}

struct InstrumentRackZoneRetargetProof {
    rack_id: NonZeroU64,
    sample_rate: u32,
}

impl PreparedInstrumentHistoryRetarget for InstrumentRackZoneRetargetProof {
    fn declared_fixed_storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl PreparedMutableInstrumentRuntime for PreparedInstrumentRackRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.supports_active_occurrence_ids
    }

    fn supports_occurrence_gain(&self) -> bool {
        self.supports_occurrence_gain
    }

    fn steals_releasing_voices(&self) -> bool {
        self.steals_releasing_voices
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(self.rack_latency_frames)
    }

    fn runtime_version(&self) -> u32 {
        INSTRUMENT_RACK_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        self.declared_twin_bytes
    }

    fn max_events_per_block(&self) -> usize {
        self.max_events_per_block
    }

    fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    fn max_release_tail_frames(&self) -> u64 {
        self.max_release_tail_frames
    }

    fn create_runtime(&self) -> Box<dyn MutableInstrumentRuntime> {
        let mut branch_runtimes: [Option<BranchRuntime>; INSTRUMENT_RACK_BRANCH_COUNT] =
            [None, None];
        for (index, slot) in self.branches.iter().enumerate() {
            if let Some(prepared) = slot.as_ref()
                && !prepared.bypassed
            {
                let delay = if prepared.compensation_delay_frames > 0 {
                    Some(RackCompensationDelay::new_on_control(
                        prepared.compensation_delay_frames as usize,
                    ))
                } else {
                    None
                };
                branch_runtimes[index] = Some(BranchRuntime {
                    branch_id: prepared.branch_id,
                    device_id: prepared.device_id,
                    gain: prepared.gain,
                    zone: prepared.zone,
                    compensation_delay_frames: prepared.compensation_delay_frames,
                    runtime: prepared.factory.create_runtime(),
                    delay,
                    routed_occurrences: [None; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
                });
            }
        }
        Box::new(InstrumentRackRuntime {
            rack_id: self.rack_id,
            overlap_policy: self.overlap_policy,
            sample_rate: self.sample_rate,
            max_events_per_block: self.max_events_per_block,
            supports_occurrence_gain: self.supports_occurrence_gain,
            supports_active_occurrence_ids: self.supports_active_occurrence_ids,
            branches: branch_runtimes,
            branch_scratch: [[0.0; 2]; INSTRUMENT_RACK_CHUNK_FRAMES],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn prepare_history_retarget_target(
        &self,
    ) -> Result<InstrumentHistoryRetargetPreparation, DeviceContractError> {
        Ok(InstrumentHistoryRetargetPreparation::Prepared(Box::new(
            InstrumentRackZoneRetargetProof {
                rack_id: self.rack_id,
                sample_rate: self.sample_rate,
            },
        )))
    }
}

struct RackCompensationDelay {
    frames: Vec<StereoFrame>,
    next: usize,
}

impl RackCompensationDelay {
    fn new_on_control(len: usize) -> Self {
        Self {
            frames: vec![[0.0; 2]; len],
            next: 0,
        }
    }

    fn process(&mut self, samples: &mut [StereoFrame]) {
        let len = self.frames.len();
        if len == 0 {
            return;
        }
        for sample in samples {
            let out = self.frames[self.next];
            self.frames[self.next] = *sample;
            self.next = (self.next + 1) % len;
            *sample = out;
        }
    }

    fn reset(&mut self) {
        self.frames.fill([0.0; 2]);
        self.next = 0;
    }

    fn is_quiet(&self) -> bool {
        self.frames.iter().all(|f| f[0] == 0.0 && f[1] == 0.0)
    }

    fn copy_from(&mut self, other: &Self) {
        if self.frames.len() == other.frames.len() {
            self.frames.copy_from_slice(&other.frames);
            self.next = other.next;
        }
    }

    fn storage_bytes(&self) -> usize {
        self.frames
            .capacity()
            .saturating_mul(std::mem::size_of::<StereoFrame>())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RoutedOccurrence {
    occurrence_id: u64,
    zone_gain: f32,
    note_off_sent: bool,
}

struct BranchRuntime {
    branch_id: NonZeroU64,
    device_id: NonZeroU64,
    gain: f32,
    zone: InstrumentRackZone,
    compensation_delay_frames: u32,
    runtime: Box<dyn MutableInstrumentRuntime>,
    delay: Option<RackCompensationDelay>,
    routed_occurrences: [Option<RoutedOccurrence>; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
}

struct InstrumentRackRuntime {
    rack_id: NonZeroU64,
    overlap_policy: InstrumentRackOverlapPolicy,
    sample_rate: u32,
    max_events_per_block: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    branches: [Option<BranchRuntime>; INSTRUMENT_RACK_BRANCH_COUNT],
    branch_scratch: [StereoFrame; INSTRUMENT_RACK_CHUNK_FRAMES],
}

impl InstrumentRackRuntime {
    fn evaluate_branch_gains(
        &self,
        pitch: u8,
        velocity: u8,
    ) -> [Option<f32>; INSTRUMENT_RACK_BRANCH_COUNT] {
        let g0 = self.branches[0]
            .as_ref()
            .and_then(|b| b.zone.evaluate_gain(pitch, velocity));
        let g1 = match self.overlap_policy {
            InstrumentRackOverlapPolicy::Layer => self.branches[1]
                .as_ref()
                .and_then(|b| b.zone.evaluate_gain(pitch, velocity)),
            InstrumentRackOverlapPolicy::ExclusiveFirstMatch => {
                if g0.is_some() {
                    None
                } else {
                    self.branches[1]
                        .as_ref()
                        .and_then(|b| b.zone.evaluate_gain(pitch, velocity))
                }
            }
        };
        [g0, g1]
    }

    fn find_routed(
        slots: &[Option<RoutedOccurrence>; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
        occurrence_id: u64,
    ) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|r| r.occurrence_id == occurrence_id))
    }

    fn find_free_routed(
        slots: &[Option<RoutedOccurrence>; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
    ) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn filter_events_for_branch(
        &self,
        branch_index: usize,
        routed: &mut [Option<RoutedOccurrence>; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
        events: &[NativeInstrumentEvent],
        out: &mut [NativeInstrumentEvent; INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        let mut len = 0_usize;
        for &event in events {
            match event {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch,
                    velocity,
                    ..
                } => {
                    let gains = self.evaluate_branch_gains(pitch, velocity);
                    if let Some(zone_gain) = gains[branch_index] {
                        if let Some(existing) = Self::find_routed(routed, occurrence_id) {
                            routed[existing] = Some(RoutedOccurrence {
                                occurrence_id,
                                zone_gain,
                                note_off_sent: false,
                            });
                        } else if let Some(free) = Self::find_free_routed(routed) {
                            routed[free] = Some(RoutedOccurrence {
                                occurrence_id,
                                zone_gain,
                                note_off_sent: false,
                            });
                        } else {
                            return Err(
                                NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded,
                            );
                        }
                        if len >= out.len() {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        }
                        out[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_routed(routed, occurrence_id)
                        && let Some(entry) = routed[slot].as_mut()
                        && !entry.note_off_sent
                    {
                        entry.note_off_sent = true;
                        if len >= out.len() {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        }
                        out[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_routed(routed, occurrence_id.get()).is_some() {
                        if len >= out.len() {
                            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                        }
                        out[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    if len >= out.len() {
                        return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
                    }
                    out[len] = event;
                    len += 1;
                }
            }
        }
        Ok(len)
    }

    fn build_branch_gains(
        routed: &[Option<RoutedOccurrence>; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
        caller_gains: &[NativeInstrumentOccurrenceGain],
        chunk_len: usize,
        out: &mut [NativeInstrumentOccurrenceGain; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES],
    ) -> usize {
        let mut len = 0_usize;
        let span = chunk_len.max(1) as u64;
        for &g in caller_gains {
            let Some(slot) = Self::find_routed(routed, g.occurrence_id()) else {
                continue;
            };
            let Some(entry) = routed[slot] else {
                continue;
            };
            let combined = if entry.zone_gain < 1.0 {
                g.with_attenuation(
                    f64::from(entry.zone_gain),
                    f64::from(entry.zone_gain),
                    span,
                    0,
                )
                .unwrap_or(g)
            } else {
                g
            };
            if len < out.len() {
                out[len] = combined;
                len += 1;
            }
        }
        for entry in routed.iter().flatten() {
            if entry.zone_gain < 1.0
                && !caller_gains
                    .iter()
                    .any(|g| g.occurrence_id() == entry.occurrence_id)
                && len < out.len()
                && let Ok(gain) = NativeInstrumentOccurrenceGain::new(
                    entry.occurrence_id,
                    f64::from(entry.zone_gain),
                    f64::from(entry.zone_gain),
                    span,
                    0,
                )
            {
                out[len] = gain;
                len += 1;
            }
        }
        len
    }

    fn prune_finished_routed(branch: &mut BranchRuntime) {
        if branch.runtime.is_quiescent() {
            branch.routed_occurrences.fill(None);
            return;
        }
        if branch.runtime.supports_active_occurrence_ids() {
            let mut active = [0_u64; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES];
            if let Ok(n) = branch.runtime.active_occurrence_ids(&mut active) {
                for slot in &mut branch.routed_occurrences {
                    if let Some(entry) = *slot
                        && entry.note_off_sent
                        && !active[..n].contains(&entry.occurrence_id)
                    {
                        *slot = None;
                    }
                }
            }
        }
    }
}

fn shift_event_offset(event: NativeInstrumentEvent, local_offset: usize) -> NativeInstrumentEvent {
    match event {
        NativeInstrumentEvent::Expression { expression, .. } => NativeInstrumentEvent::Expression {
            frame_offset: local_offset,
            expression,
        },
        NativeInstrumentEvent::ControlChange { cc, value, .. } => {
            NativeInstrumentEvent::ControlChange {
                frame_offset: local_offset,
                cc,
                value,
            }
        }
        NativeInstrumentEvent::PitchBend { value, .. } => NativeInstrumentEvent::PitchBend {
            frame_offset: local_offset,
            value,
        },
        NativeInstrumentEvent::NotePressure {
            occurrence_id,
            value,
            ..
        } => NativeInstrumentEvent::NotePressure {
            frame_offset: local_offset,
            occurrence_id,
            value,
        },
        NativeInstrumentEvent::NotePitchBend {
            occurrence_id,
            cents,
            ..
        } => NativeInstrumentEvent::NotePitchBend {
            frame_offset: local_offset,
            occurrence_id,
            cents,
        },
        NativeInstrumentEvent::NoteTimbre {
            occurrence_id,
            value,
            ..
        } => NativeInstrumentEvent::NoteTimbre {
            frame_offset: local_offset,
            occurrence_id,
            value,
        },
        NativeInstrumentEvent::NoteOn {
            occurrence_id,
            pitch,
            velocity,
            ..
        } => NativeInstrumentEvent::NoteOn {
            frame_offset: local_offset,
            occurrence_id,
            pitch,
            velocity,
        },
        NativeInstrumentEvent::NoteOff { occurrence_id, .. } => NativeInstrumentEvent::NoteOff {
            frame_offset: local_offset,
            occurrence_id,
        },
    }
}

impl MutableInstrumentRuntime for InstrumentRackRuntime {
    fn supports_active_occurrence_ids(&self) -> bool {
        self.supports_active_occurrence_ids
    }

    fn supports_occurrence_gain(&self) -> bool {
        self.supports_occurrence_gain
    }

    fn active_occurrence_ids(
        &self,
        output: &mut [u64],
    ) -> Result<usize, NativeInstrumentRuntimeError> {
        if !self.supports_active_occurrence_ids {
            return Err(NativeInstrumentRuntimeError::UnsupportedActiveOccurrenceIds);
        }
        let mut buf_a = [0_u64; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES];
        let mut buf_b = [0_u64; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES];
        let len_a = match &self.branches[0] {
            Some(branch) => branch.runtime.active_occurrence_ids(&mut buf_a)?,
            None => 0,
        };
        let len_b = match &self.branches[1] {
            Some(branch) => branch.runtime.active_occurrence_ids(&mut buf_b)?,
            None => 0,
        };
        let mut merged = [0_u64; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES * 2];
        let mut count = 0_usize;
        let mut ia = 0_usize;
        let mut ib = 0_usize;
        while ia < len_a || ib < len_b {
            let next = match (ia < len_a, ib < len_b) {
                (true, true) => {
                    if buf_a[ia] < buf_b[ib] {
                        let v = buf_a[ia];
                        ia += 1;
                        v
                    } else if buf_b[ib] < buf_a[ia] {
                        let v = buf_b[ib];
                        ib += 1;
                        v
                    } else {
                        let v = buf_a[ia];
                        ia += 1;
                        ib += 1;
                        v
                    }
                }
                (true, false) => {
                    let v = buf_a[ia];
                    ia += 1;
                    v
                }
                (false, true) => {
                    let v = buf_b[ib];
                    ib += 1;
                    v
                }
                (false, false) => break,
            };
            merged[count] = next;
            count += 1;
        }
        if count > output.len() {
            return Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded);
        }
        output[..count].copy_from_slice(&merged[..count]);
        Ok(count)
    }

    fn preflight_events(
        &self,
        events: &[NativeInstrumentEvent],
        frame_count: usize,
        resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if events.len() > self.max_events_per_block {
            return Err(NativeInstrumentRuntimeError::EventCapacityExceeded);
        }
        let mut branch_events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK];
        for (index, slot) in self.branches.iter().enumerate() {
            if let Some(branch) = slot {
                let mut sim_routed = branch.routed_occurrences;
                let len = self.filter_events_for_branch(
                    index,
                    &mut sim_routed,
                    events,
                    &mut branch_events,
                )?;
                branch
                    .runtime
                    .preflight_events(&branch_events[..len], frame_count, resources)?;
            }
        }
        Ok(())
    }

    fn preflight_clock(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        frames: usize,
        resources: &dyn crate::DeviceResources,
    ) -> Result<(), NativeInstrumentRuntimeError> {
        for branch in self.branches.iter().flatten() {
            branch.runtime.preflight_clock(clock, frames, resources)?;
        }
        Ok(())
    }

    fn preflight_occurrence_gains(
        &self,
        events: &[NativeInstrumentEvent],
        gains: &[NativeInstrumentOccurrenceGain],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        if gains.is_empty() {
            return Ok(());
        }
        if !self.supports_occurrence_gain {
            return Err(NativeInstrumentRuntimeError::UnsupportedOccurrenceGain);
        }
        if gains.len() > INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES {
            return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
        }
        let mut branch_events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK];
        let mut branch_gains = [gains[0]; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES];
        for (index, slot) in self.branches.iter().enumerate() {
            if let Some(branch) = slot {
                let mut sim_routed = branch.routed_occurrences;
                let ev_len = self.filter_events_for_branch(
                    index,
                    &mut sim_routed,
                    events,
                    &mut branch_events,
                )?;
                let g_len = Self::build_branch_gains(&sim_routed, gains, 1, &mut branch_gains);
                if g_len > 0 {
                    branch.runtime.preflight_occurrence_gains(
                        &branch_events[..ev_len],
                        &branch_gains[..g_len],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn process_block(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        resources: &dyn crate::DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, frames, None, &[], resources);
    }

    fn process_block_with_clock(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn crate::DeviceResources,
    ) {
        self.process_block_with_clock_and_occurrence_gains(events, frames, clock, &[], resources);
    }

    fn process_block_with_clock_and_occurrence_gains(
        &mut self,
        events: &[NativeInstrumentEvent],
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        gains: &[NativeInstrumentOccurrenceGain],
        resources: &dyn crate::DeviceResources,
    ) {
        let total_frames = frames.len();
        if total_frames == 0 {
            return;
        }
        let mut chunk_events = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK];
        let mut chunk_gains_storage = gains
            .first()
            .copied()
            .map(|first| [first; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES]);

        let mut event_cursor = 0_usize;
        let mut chunk_start = 0_usize;
        while chunk_start < total_frames {
            let chunk_len = (total_frames - chunk_start).min(INSTRUMENT_RACK_CHUNK_FRAMES);
            let chunk_end = chunk_start + chunk_len;

            let mut chunk_event_len = 0_usize;
            while event_cursor < events.len() && events[event_cursor].frame_offset() < chunk_end {
                if chunk_event_len < chunk_events.len() {
                    let local_offset = events[event_cursor]
                        .frame_offset()
                        .saturating_sub(chunk_start);
                    chunk_events[chunk_event_len] =
                        shift_event_offset(events[event_cursor], local_offset);
                    chunk_event_len += 1;
                }
                event_cursor += 1;
            }

            let active_gains: &[NativeInstrumentOccurrenceGain] =
                if let Some(storage) = chunk_gains_storage.as_mut() {
                    let gain_len = gains.len().min(storage.len());
                    for (dst, &src) in storage[..gain_len].iter_mut().zip(&gains[..gain_len]) {
                        *dst = src.advance_frames(chunk_start as u64).unwrap_or(src);
                    }
                    &storage[..gain_len]
                } else {
                    &[]
                };

            let offset_clock = clock.map(|base| OffsetClock {
                clock: base,
                offset: chunk_start,
            });
            let chunk_clock_ref: Option<&dyn NativeInstrumentClock> = offset_clock
                .as_ref()
                .map(|c| c as &dyn NativeInstrumentClock);
            let active_events = &chunk_events[..chunk_event_len];
            let out_slice = &mut frames[chunk_start..chunk_end];

            // Pre-evaluate branch routing before mutating each branch runtime so
            // ExclusiveFirstMatch inspects both branches deterministically.
            let mut branch_events_buf = [[NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            }; INSTRUMENT_RACK_MAX_EVENTS_PER_BLOCK];
                INSTRUMENT_RACK_BRANCH_COUNT];
            let mut branch_event_lens = [0_usize; INSTRUMENT_RACK_BRANCH_COUNT];
            let mut next_routed =
                [[None; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES]; INSTRUMENT_RACK_BRANCH_COUNT];
            for index in 0..INSTRUMENT_RACK_BRANCH_COUNT {
                if let Some(branch) = &self.branches[index] {
                    next_routed[index] = branch.routed_occurrences;
                    if let Ok(n) = self.filter_events_for_branch(
                        index,
                        &mut next_routed[index],
                        active_events,
                        &mut branch_events_buf[index],
                    ) {
                        branch_event_lens[index] = n;
                    }
                }
            }

            let Ok(dummy_gain) = NativeInstrumentOccurrenceGain::new(1, 1.0, 1.0, 1, 0) else {
                return;
            };
            let mut branch_gains_buf = [dummy_gain; INSTRUMENT_RACK_MAX_ACTIVE_OCCURRENCES];

            for index in 0..INSTRUMENT_RACK_BRANCH_COUNT {
                let Some(branch) = self.branches[index].as_mut() else {
                    continue;
                };
                branch.routed_occurrences = next_routed[index];
                let b_gains_len = if self.supports_occurrence_gain {
                    Self::build_branch_gains(
                        &branch.routed_occurrences,
                        active_gains,
                        chunk_len,
                        &mut branch_gains_buf,
                    )
                } else {
                    0
                };
                let scratch = &mut self.branch_scratch[..chunk_len];
                scratch.fill([0.0; 2]);
                branch
                    .runtime
                    .process_block_with_clock_and_occurrence_gains(
                        &branch_events_buf[index][..branch_event_lens[index]],
                        scratch,
                        chunk_clock_ref,
                        &branch_gains_buf[..b_gains_len],
                        resources,
                    );
                Self::prune_finished_routed(branch);
                if let Some(delay) = &mut branch.delay {
                    delay.process(scratch);
                }
                let gain = branch.gain;
                for (dst, &src) in out_slice.iter_mut().zip(scratch.iter()) {
                    dst[0] += src[0] * gain;
                    dst[1] += src[1] * gain;
                }
            }

            chunk_start = chunk_end;
        }
    }

    fn reset(&mut self) {
        for branch in self.branches.iter_mut().flatten() {
            branch.runtime.reset();
            branch.routed_occurrences.fill(None);
            if let Some(delay) = &mut branch.delay {
                delay.reset();
            }
        }
    }

    fn is_quiescent(&self) -> bool {
        self.branches.iter().flatten().all(|branch| {
            branch.runtime.is_quiescent()
                && branch
                    .delay
                    .as_ref()
                    .is_none_or(RackCompensationDelay::is_quiet)
        })
    }

    fn can_copy_history_from(&self, previous: &dyn MutableInstrumentRuntime) -> bool {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        if self.rack_id != other.rack_id
            || self.overlap_policy != other.overlap_policy
            || self.sample_rate != other.sample_rate
            || self.max_events_per_block != other.max_events_per_block
        {
            return false;
        }
        self.branches
            .iter()
            .zip(&other.branches)
            .all(|(dst, src)| match (dst, src) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.branch_id == b.branch_id
                        && a.device_id == b.device_id
                        && a.gain.to_bits() == b.gain.to_bits()
                        && a.zone == b.zone
                        && a.compensation_delay_frames == b.compensation_delay_frames
                        && a.runtime.can_copy_history_from(b.runtime.as_ref())
                }
                _ => false,
            })
    }

    fn copy_history_from(&mut self, previous: &dyn MutableInstrumentRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        if !self.can_copy_history_from(previous) {
            return;
        }
        for (dst, src) in self.branches.iter_mut().zip(&other.branches) {
            if let (Some(a), Some(b)) = (dst.as_mut(), src.as_ref()) {
                if a.runtime.can_copy_history_from(b.runtime.as_ref()) {
                    a.runtime.copy_history_from(b.runtime.as_ref());
                } else if let Some(inner_dst) = a.runtime.as_any().downcast_ref::<Self>() {
                    let inner_proof = InstrumentRackZoneRetargetProof {
                        rack_id: inner_dst.rack_id,
                        sample_rate: inner_dst.sample_rate,
                    };
                    a.runtime
                        .retarget_history_from(b.runtime.as_ref(), &inner_proof);
                }
                a.routed_occurrences = b.routed_occurrences;
                if let (Some(dst_delay), Some(src_delay)) = (&mut a.delay, &b.delay) {
                    dst_delay.copy_from(src_delay);
                }
            }
        }
    }

    fn can_retarget_history_from(
        &self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) -> bool {
        let Some(zone_proof) = proof
            .as_any()
            .downcast_ref::<InstrumentRackZoneRetargetProof>()
        else {
            return false;
        };
        if zone_proof.rack_id != self.rack_id || zone_proof.sample_rate != self.sample_rate {
            return false;
        }
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        if self.rack_id != other.rack_id
            || self.sample_rate != other.sample_rate
            || self.max_events_per_block != other.max_events_per_block
        {
            return false;
        }
        self.branches
            .iter()
            .zip(&other.branches)
            .all(|(dst, src)| match (dst, src) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    let child_compatible = if a.runtime.can_copy_history_from(b.runtime.as_ref()) {
                        true
                    } else if let Some(inner_dst) = a.runtime.as_any().downcast_ref::<Self>() {
                        let inner_proof = InstrumentRackZoneRetargetProof {
                            rack_id: inner_dst.rack_id,
                            sample_rate: inner_dst.sample_rate,
                        };
                        a.runtime
                            .can_retarget_history_from(b.runtime.as_ref(), &inner_proof)
                    } else {
                        false
                    };
                    a.branch_id == b.branch_id
                        && a.device_id == b.device_id
                        && a.gain.to_bits() == b.gain.to_bits()
                        && a.compensation_delay_frames == b.compensation_delay_frames
                        && child_compatible
                }
                _ => false,
            })
    }

    fn retarget_history_from(
        &mut self,
        previous: &dyn MutableInstrumentRuntime,
        proof: &dyn PreparedInstrumentHistoryRetarget,
    ) {
        if !self.can_retarget_history_from(previous, proof) {
            return;
        }
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        for (dst, src) in self.branches.iter_mut().zip(&other.branches) {
            if let (Some(a), Some(b)) = (dst.as_mut(), src.as_ref()) {
                if a.runtime.can_copy_history_from(b.runtime.as_ref()) {
                    a.runtime.copy_history_from(b.runtime.as_ref());
                } else if let Some(inner_dst) = a.runtime.as_any().downcast_ref::<Self>() {
                    let inner_proof = InstrumentRackZoneRetargetProof {
                        rack_id: inner_dst.rack_id,
                        sample_rate: inner_dst.sample_rate,
                    };
                    a.runtime
                        .retarget_history_from(b.runtime.as_ref(), &inner_proof);
                }
                a.routed_occurrences = b.routed_occurrences;
                if let (Some(dst_delay), Some(src_delay)) = (&mut a.delay, &b.delay) {
                    dst_delay.copy_from(src_delay);
                }
            }
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for branch in self.branches.iter().flatten() {
            total = total.saturating_add(branch.runtime.fixed_storage_bytes());
            if let Some(delay) = &branch.delay {
                total = total.saturating_add(delay.storage_bytes());
            }
        }
        total
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
