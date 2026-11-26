//! Bounded two-branch native Instrument Rack for one retained native source.
//!
//! Fans a single host event stream (`NativeInstrumentEvent` with stable
//! `occurrence_id`s, paired note-offs, MPE, and shared controller events) to two
//! independent native instrument branches prepared through the existing
//! `NativeInstrumentRegistry` (no parallel registry, no per-track clones),
//! aligns branch algorithmic latencies with exact-capacity control-allocated
//! delay rings, and sums weighted branch outputs once.
//!
//! Nesting is explicitly prohibited (`INSTRUMENT_RACK_MAX_DEPTH == 1`): any
//! child branch referencing `INSTRUMENT_RACK_TYPE_ID` is refused on control.
//! All child runtimes, delay rings, and chunk scratch buffers are allocated on
//! control; callback processing performs zero allocations, locks, I/O, or final drops.

#[cfg(test)]
mod tests;

use crate::{
    DevicePreparationContext, MAX_MAIN_PASS_DELAY_FRAMES, MutableInstrumentRuntime,
    NativeInstrumentClock, NativeInstrumentDefinition, NativeInstrumentEvent,
    NativeInstrumentOccurrenceGain, NativeInstrumentRegistry, NativeInstrumentRuntimeError,
    NativeInstrumentRuntimeLimits, OffsetClock, PreparedAnalyticNote,
    PreparedMutableInstrumentRuntime, PreparedNativeInstrument, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
use std::{any::Any, num::NonZeroU64};

/// Reserved native instrument type ID for a bounded two-branch Instrument Rack.
pub const INSTRUMENT_RACK_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rack.instrument");
pub const INSTRUMENT_RACK_STATE_VERSION: u32 = 1;
pub const INSTRUMENT_RACK_RUNTIME_VERSION: u32 = 1;
pub const INSTRUMENT_RACK_BRANCH_COUNT: usize = 2;
/// Explicit nesting prohibition: branches contain flat native instruments only.
pub const INSTRUMENT_RACK_MAX_DEPTH: usize = 1;
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

/// Authored specification for one branch of a bounded two-branch Instrument Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentRackBranchSpec {
    pub branch_id: NonZeroU64,
    pub device_id: NonZeroU64,
    pub gain: f32,
    pub bypassed: bool,
    /// `None` represents an empty or removed instrument slot.
    pub state: Option<NativeDeviceState>,
}

/// Authored specification for a bounded two-branch Instrument Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct InstrumentRackSpec {
    pub rack_id: NonZeroU64,
    pub branches: [InstrumentRackBranchSpec; INSTRUMENT_RACK_BRANCH_COUNT],
}

impl InstrumentRackSpec {
    pub fn validate(&self) -> Result<(), DeviceContractError> {
        if self.branches[0].branch_id == self.branches[1].branch_id
            || self.branches[0].device_id == self.branches[1].device_id
        {
            return Err(DeviceContractError::InvalidId);
        }
        for branch in &self.branches {
            if !branch.gain.is_finite()
                || !(0.0..=INSTRUMENT_RACK_MAX_BRANCH_GAIN).contains(&branch.gain)
            {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            if let Some(state) = &branch.state {
                if state.type_id() == INSTRUMENT_RACK_TYPE_ID {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
                state.type_id().validate()?;
                if state.state_version() == 0 {
                    return Err(DeviceContractError::InvalidStateVersion);
                }
                if !state.asset_references().is_empty() {
                    return Err(DeviceContractError::AssetResourcesUnavailable);
                }
            }
        }
        Ok(())
    }

    /// Encode this rack spec into a strictly validated v1 `NativeDeviceState`
    /// envelope compatible with `hex-project`'s retained `NativeSource` storage.
    pub fn encode_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(RACK_STATE_MAGIC);
        bytes.extend_from_slice(&self.rack_id.get().to_le_bytes());
        for branch in &self.branches {
            bytes.extend_from_slice(&branch.branch_id.get().to_le_bytes());
            bytes.extend_from_slice(&branch.device_id.get().to_le_bytes());
            bytes.extend_from_slice(&branch.gain.to_le_bytes());
            bytes.push(u8::from(branch.bypassed));
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
        NativeDeviceState::new(
            INSTRUMENT_RACK_TYPE_ID,
            INSTRUMENT_RACK_STATE_VERSION,
            bytes,
        )
    }

    /// Strictly decode and validate a v1 Instrument Rack `NativeDeviceState`.
    pub fn decode_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != INSTRUMENT_RACK_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != INSTRUMENT_RACK_STATE_VERSION {
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
        let spec = Self {
            rack_id,
            branches: [branch_a, branch_b],
        };
        spec.validate()?;
        Ok(spec)
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
        let spec = InstrumentRackSpec::decode_state(state)?;
        let mut prepared_branches: [Option<PreparedBranchFactory>; INSTRUMENT_RACK_BRANCH_COUNT] =
            [None, None];
        let mut branch_latencies = [0_u32; INSTRUMENT_RACK_BRANCH_COUNT];

        for (index, branch) in spec.branches.iter().enumerate() {
            let Some(child_state) = &branch.state else {
                continue;
            };
            let prepared_inst = match limits {
                Some(host_limits) => self.children.prepare_instrument_with_runtime_limits(
                    child_state,
                    context,
                    host_limits,
                )?,
                None => self.children.prepare_instrument(child_state, context)?,
            };
            let factory = prepared_inst
                .prepare_mutable_runtime()?
                .ok_or(DeviceContractError::UnsupportedProcessingClass)?;
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
            branches: [
                InstrumentRackBranchSpec {
                    branch_id: branch_1,
                    device_id: dev_1,
                    gain: 1.0,
                    bypassed: false,
                    state: None,
                },
                InstrumentRackBranchSpec {
                    branch_id: branch_2,
                    device_id: dev_2,
                    gain: 1.0,
                    bypassed: false,
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
        let mut spec = InstrumentRackSpec::decode_state(state)?;
        for branch in &mut spec.branches {
            if let Some(child) = &branch.state {
                branch.state = Some(self.children.migrate_state(child)?);
            }
        }
        spec.encode_state()
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let spec = InstrumentRackSpec::decode_state(state)?;
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
        let mut spec = InstrumentRackSpec::decode_state(state)?;
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
    compensation_delay_frames: u32,
    factory: Box<dyn PreparedMutableInstrumentRuntime>,
}

struct PreparedInstrumentRack {
    rack_id: NonZeroU64,
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
                    compensation_delay_frames: prepared.compensation_delay_frames,
                    runtime: prepared.factory.create_runtime(),
                    delay,
                });
            }
        }
        Box::new(InstrumentRackRuntime {
            rack_id: self.rack_id,
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

struct BranchRuntime {
    branch_id: NonZeroU64,
    device_id: NonZeroU64,
    gain: f32,
    compensation_delay_frames: u32,
    runtime: Box<dyn MutableInstrumentRuntime>,
    delay: Option<RackCompensationDelay>,
}

struct InstrumentRackRuntime {
    rack_id: NonZeroU64,
    sample_rate: u32,
    max_events_per_block: usize,
    supports_occurrence_gain: bool,
    supports_active_occurrence_ids: bool,
    branches: [Option<BranchRuntime>; INSTRUMENT_RACK_BRANCH_COUNT],
    branch_scratch: [StereoFrame; INSTRUMENT_RACK_CHUNK_FRAMES],
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
        for branch in self.branches.iter().flatten() {
            branch
                .runtime
                .preflight_events(events, frame_count, resources)?;
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
        for branch in self.branches.iter().flatten() {
            branch.runtime.preflight_occurrence_gains(events, gains)?;
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

            for branch in self.branches.iter_mut().flatten() {
                let scratch = &mut self.branch_scratch[..chunk_len];
                scratch.fill([0.0; 2]);
                branch
                    .runtime
                    .process_block_with_clock_and_occurrence_gains(
                        active_events,
                        scratch,
                        chunk_clock_ref,
                        active_gains,
                        resources,
                    );
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
                a.runtime.copy_history_from(b.runtime.as_ref());
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
