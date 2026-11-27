//! Bounded two-branch native MIDI Rack before one instrument, supporting up to
//! depth 2 nesting (`MIDI_RACK_MAX_DEPTH == 2`).
//!
//! Fans a single host event stream (`NativeInstrumentEvent` with stable
//! `occurrence_id`s, paired note-offs, MPE, and shared controller events) to
//! two ordered MIDI-processing branches prepared through the existing
//! `NativeMidiProcessorRegistry` (never a parallel registry or second
//! scheduler) and merges their outputs deterministically before the track's
//! instrument.
//!
//! Each rack level translates its emitted downstream occurrences into a
//! disjoint rack-owned `occurrence_id` namespace (`>= GENERATED_OCCURRENCE_ID_BASE`)
//! so simultaneous overlapping notes, transformed pitches, and duplicate pitch
//! outputs across nested branches stay independent and release cleanly with
//! zero stuck notes.
//!
//! Nesting is explicitly bounded at `MIDI_RACK_MAX_DEPTH == 2`: an outer MIDI
//! Rack branch may host one nested MIDI Rack whose branches contain flat leaf
//! MIDI processors. Deeper nesting (`depth > 2`), duplicate rack/branch/device
//! identities across levels (cycles/self-references), and capacity overflows
//! are explicitly refused on control before touching active state. All child
//! runtimes and scratch storage are allocated on control; callback processing
//! performs zero allocations, locks, I/O, or final drops.

use crate::{
    GENERATED_OCCURRENCE_ID_BASE, MidiProcessorOutput, MutableMidiProcessorRuntime,
    NativeInstrumentClock, NativeInstrumentEvent, NativeMidiProcessorDefinition,
    NativeMidiProcessorError, NativeMidiProcessorRegistry, PreparedNativeMidiProcessor,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation,
    ParameterValue,
};
use std::{any::Any, num::NonZeroU64};

/// Reserved native MIDI processor type ID for a bounded two-branch MIDI Rack.
pub const MIDI_RACK_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rack.midi");
pub const MIDI_RACK_STATE_VERSION: u32 = 1;
pub const MIDI_RACK_RUNTIME_VERSION: u32 = 1;
pub const MIDI_RACK_BRANCH_COUNT: usize = 2;
/// Explicit nesting ceiling: outer MIDI Rack (depth 1) may host nested MIDI
/// Racks (depth 2) whose branches contain leaf MIDI processors only.
pub const MIDI_RACK_MAX_DEPTH: usize = 2;
pub const MIDI_RACK_MAX_INPUT_EVENTS: usize = 259;
pub const MIDI_RACK_MAX_OUTPUT_EVENTS: usize = 259;
pub const MIDI_RACK_MAX_ACTIVE_OCCURRENCES: usize = 64;
/// Per-branch active occurrence ceiling (`32 * 2 == 64`), guaranteeing combined
/// downstream occurrences never exceed the host instrument voice budget.
pub const MIDI_RACK_MAX_BRANCH_ACTIVE_OCCURRENCES: usize =
    MIDI_RACK_MAX_ACTIVE_OCCURRENCES / MIDI_RACK_BRANCH_COUNT;
pub const MIDI_RACK_MAX_WORK_UNITS: usize = MIDI_RACK_MAX_INPUT_EVENTS * 4;
const MIDI_RACK_STATE_MAGIC: &[u8; 4] = b"HXMR";

pub const PARAM_MIDI_RACK_BRANCH_1_BYPASS: ParameterId = ParameterId::from_static(1);
pub const PARAM_MIDI_RACK_BRANCH_2_BYPASS: ParameterId = ParameterId::from_static(2);

const DESCRIPTORS: [ParameterDescriptor; 2] = [
    ParameterDescriptor {
        id: PARAM_MIDI_RACK_BRANCH_1_BYPASS,
        label: "branch 1 bypass",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
    ParameterDescriptor {
        id: PARAM_MIDI_RACK_BRANCH_2_BYPASS,
        label: "branch 2 bypass",
        kind: ParameterKind::Bool,
        default: ParameterValue::Bool(false),
        presentation: ParameterPresentation::Linear,
    },
];

/// Authored specification for one branch of a bounded two-branch MIDI Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiRackBranchSpec {
    pub branch_id: NonZeroU64,
    pub device_id: NonZeroU64,
    /// When `true`, new onsets/CCs are muted on this branch while already-held
    /// occurrences still receive their paired `NoteOff`s.
    pub bypassed: bool,
    /// `None` represents a dry pass-through branch; `Some` prepares a child
    /// MIDI processor from `NativeMidiProcessorRegistry`.
    pub state: Option<NativeDeviceState>,
}

/// Authored specification for a bounded two-branch MIDI Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiRackSpec {
    pub rack_id: NonZeroU64,
    pub branches: [MidiRackBranchSpec; MIDI_RACK_BRANCH_COUNT],
}

impl Default for MidiRackSpec {
    fn default() -> Self {
        Self {
            rack_id: NonZeroU64::MIN,
            branches: [
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(1).unwrap_or(NonZeroU64::MIN),
                    device_id: NonZeroU64::new(101).unwrap_or(NonZeroU64::MIN),
                    bypassed: false,
                    state: None,
                },
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(2).unwrap_or(NonZeroU64::MIN),
                    device_id: NonZeroU64::new(102).unwrap_or(NonZeroU64::MIN),
                    bypassed: true,
                    state: None,
                },
            ],
        }
    }
}

impl MidiRackSpec {
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
        if depth == 0 || depth > MIDI_RACK_MAX_DEPTH {
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
            let b_raw = branch.branch_id.get();
            if seen_branches[..*seen_branches_len].contains(&b_raw) {
                return Err(DeviceContractError::InvalidId);
            }
            if *seen_branches_len >= seen_branches.len() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            seen_branches[*seen_branches_len] = b_raw;
            *seen_branches_len += 1;

            let d_raw = branch.device_id.get();
            if seen_devices[..*seen_devices_len].contains(&d_raw) {
                return Err(DeviceContractError::InvalidId);
            }
            if *seen_devices_len >= seen_devices.len() {
                return Err(DeviceContractError::UnsupportedProcessingClass);
            }
            seen_devices[*seen_devices_len] = d_raw;
            *seen_devices_len += 1;

            if let Some(state) = &branch.state {
                state.type_id().validate()?;
                if state.state_version() == 0 {
                    return Err(DeviceContractError::InvalidStateVersion);
                }
                if !state.asset_references().is_empty() {
                    return Err(DeviceContractError::AssetResourcesUnavailable);
                }
                if state.type_id() == MIDI_RACK_TYPE_ID {
                    if depth >= MIDI_RACK_MAX_DEPTH {
                        return Err(DeviceContractError::UnsupportedProcessingClass);
                    }
                    let nested = Self::decode_state_raw(state)?;
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

    /// Encode this rack spec into a strictly validated v1 `NativeDeviceState`.
    pub fn encode_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        self.validate()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MIDI_RACK_STATE_MAGIC);
        bytes.extend_from_slice(&self.rack_id.get().to_le_bytes());
        for branch in &self.branches {
            bytes.extend_from_slice(&branch.branch_id.get().to_le_bytes());
            bytes.extend_from_slice(&branch.device_id.get().to_le_bytes());
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
        NativeDeviceState::new(MIDI_RACK_TYPE_ID, MIDI_RACK_STATE_VERSION, bytes)
    }

    /// Strictly decode and validate a v1 MIDI Rack `NativeDeviceState` (up to
    /// `MIDI_RACK_MAX_DEPTH == 2`).
    pub fn decode_state(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        let spec = Self::decode_state_raw(state)?;
        spec.validate()?;
        Ok(spec)
    }

    fn decode_state_raw(state: &NativeDeviceState) -> Result<Self, DeviceContractError> {
        if state.type_id() != MIDI_RACK_TYPE_ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != MIDI_RACK_STATE_VERSION {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        if !state.asset_references().is_empty() {
            return Err(DeviceContractError::AssetResourcesUnavailable);
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

        if read_bytes(&mut cursor, 4)? != MIDI_RACK_STATE_MAGIC {
            return Err(DeviceContractError::MalformedState);
        }
        let rack_raw = u64::from_le_bytes(
            read_bytes(&mut cursor, 8)?
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        let rack_id = NonZeroU64::new(rack_raw).ok_or(DeviceContractError::InvalidId)?;

        let mut branches = Vec::with_capacity(MIDI_RACK_BRANCH_COUNT);
        for _ in 0..MIDI_RACK_BRANCH_COUNT {
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
            branches.push(MidiRackBranchSpec {
                branch_id,
                device_id,
                bypassed,
                state: child_state,
            });
        }
        if cursor != payload.len() {
            return Err(DeviceContractError::MalformedState);
        }
        let branches: [MidiRackBranchSpec; MIDI_RACK_BRANCH_COUNT] = branches
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(Self { rack_id, branches })
    }
}

/// Control-side definition for the bounded two-branch MIDI Rack wrapping the
/// existing `NativeMidiProcessorRegistry`.
pub struct MidiRackDefinition<'a> {
    children: NativeMidiProcessorRegistry<'a>,
}

impl<'a> MidiRackDefinition<'a> {
    pub fn new(children: NativeMidiProcessorRegistry<'a>) -> Result<Self, DeviceContractError> {
        if children.definition(MIDI_RACK_TYPE_ID).is_some() {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Ok(Self { children })
    }

    fn migrate_spec_at_depth(
        &self,
        spec: &mut MidiRackSpec,
        depth: usize,
    ) -> Result<(), DeviceContractError> {
        if depth == 0 || depth > MIDI_RACK_MAX_DEPTH {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        for branch in &mut spec.branches {
            if let Some(child) = &branch.state {
                if child.type_id() == MIDI_RACK_TYPE_ID {
                    if depth >= MIDI_RACK_MAX_DEPTH {
                        return Err(DeviceContractError::UnsupportedProcessingClass);
                    }
                    let mut nested = MidiRackSpec::decode_state_raw(child)?;
                    self.migrate_spec_at_depth(&mut nested, depth + 1)?;
                    branch.state = Some(nested.encode_state()?.with_host_metadata_from(child)?);
                } else {
                    branch.state = Some(self.children.migrate_state(child)?);
                }
            }
        }
        Ok(())
    }

    fn prepare_spec_at_depth(
        &self,
        spec: &MidiRackSpec,
        sample_rate: u32,
        depth: usize,
        max_active_occurrences: usize,
    ) -> Result<PreparedMidiRack, DeviceContractError> {
        if depth == 0 || depth > MIDI_RACK_MAX_DEPTH {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let branch_active_cap = max_active_occurrences / MIDI_RACK_BRANCH_COUNT;
        let mut prepared_branches: [Option<Box<dyn PreparedNativeMidiProcessor>>;
            MIDI_RACK_BRANCH_COUNT] = [None, None];
        for (i, branch) in spec.branches.iter().enumerate() {
            if let Some(child_state) = &branch.state {
                let child: Box<dyn PreparedNativeMidiProcessor> =
                    if child_state.type_id() == MIDI_RACK_TYPE_ID {
                        if depth >= MIDI_RACK_MAX_DEPTH {
                            return Err(DeviceContractError::UnsupportedProcessingClass);
                        }
                        let nested = MidiRackSpec::decode_state_raw(child_state)?;
                        Box::new(self.prepare_spec_at_depth(
                            &nested,
                            sample_rate,
                            depth + 1,
                            branch_active_cap,
                        )?)
                    } else {
                        self.children
                            .prepare_midi_processor(child_state, sample_rate)?
                    };
                prepared_branches[i] = Some(child);
            }
        }
        PreparedMidiRack::with_caps(
            spec.rack_id,
            [spec.branches[0].branch_id, spec.branches[1].branch_id],
            [spec.branches[0].bypassed, spec.branches[1].bypassed],
            prepared_branches,
            MIDI_RACK_MAX_INPUT_EVENTS,
            MIDI_RACK_MAX_OUTPUT_EVENTS,
            max_active_occurrences,
            MIDI_RACK_MAX_WORK_UNITS,
        )
    }
}

impl NativeDeviceMetadata for MidiRackDefinition<'_> {
    fn type_id(&self) -> DeviceTypeId {
        MIDI_RACK_TYPE_ID
    }

    fn display_name(&self) -> Option<&'static str> {
        Some("midi rack")
    }

    fn state_version(&self) -> u32 {
        MIDI_RACK_STATE_VERSION
    }

    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &DESCRIPTORS
    }

    fn ports(&self) -> &'static [hex_lattice_contract::DevicePortDescriptor] {
        hex_lattice_contract::MIDI_EFFECT_PORTS
    }

    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        MidiRackSpec::default().encode_state()
    }

    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut spec = MidiRackSpec::decode_state(state)?;
        self.migrate_spec_at_depth(&mut spec, 1)?;
        spec.encode_state()?.with_host_metadata_from(state)
    }

    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        let spec = MidiRackSpec::decode_state(state)?;
        match parameter {
            PARAM_MIDI_RACK_BRANCH_1_BYPASS => Ok(ParameterValue::Bool(spec.branches[0].bypassed)),
            PARAM_MIDI_RACK_BRANCH_2_BYPASS => Ok(ParameterValue::Bool(spec.branches[1].bypassed)),
            _ => Err(DeviceContractError::UnknownParameter),
        }
    }

    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        parameter: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        let mut spec = MidiRackSpec::decode_state(state)?;
        match (parameter, value) {
            (PARAM_MIDI_RACK_BRANCH_1_BYPASS, ParameterValue::Bool(b)) => {
                spec.branches[0].bypassed = b;
            }
            (PARAM_MIDI_RACK_BRANCH_2_BYPASS, ParameterValue::Bool(b)) => {
                spec.branches[1].bypassed = b;
            }
            (PARAM_MIDI_RACK_BRANCH_1_BYPASS | PARAM_MIDI_RACK_BRANCH_2_BYPASS, _) => {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            _ => return Err(DeviceContractError::UnknownParameter),
        }
        spec.encode_state()?.with_host_metadata_from(state)
    }
}

impl NativeMidiProcessorDefinition for MidiRackDefinition<'_> {
    fn prepare_midi_processor(
        &self,
        state: &NativeDeviceState,
        sample_rate: u32,
    ) -> Result<Box<dyn PreparedNativeMidiProcessor>, DeviceContractError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let spec = MidiRackSpec::decode_state(state)?;
        let prepared =
            self.prepare_spec_at_depth(&spec, sample_rate, 1, MIDI_RACK_MAX_ACTIVE_OCCURRENCES)?;
        Ok(Box::new(prepared))
    }
}

/// Control-prepared immutable factory for a bounded two-branch MIDI Rack.
pub struct PreparedMidiRack {
    rack_id: NonZeroU64,
    branch_ids: [NonZeroU64; MIDI_RACK_BRANCH_COUNT],
    branch_bypassed: [bool; MIDI_RACK_BRANCH_COUNT],
    branches: [Option<Box<dyn PreparedNativeMidiProcessor>>; MIDI_RACK_BRANCH_COUNT],
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_work_units: usize,
    declared_twin_bytes: usize,
}

impl PreparedMidiRack {
    pub fn new(
        rack_id: NonZeroU64,
        branch_ids: [NonZeroU64; MIDI_RACK_BRANCH_COUNT],
        branch_bypassed: [bool; MIDI_RACK_BRANCH_COUNT],
        branches: [Option<Box<dyn PreparedNativeMidiProcessor>>; MIDI_RACK_BRANCH_COUNT],
    ) -> Result<Self, DeviceContractError> {
        Self::with_caps(
            rack_id,
            branch_ids,
            branch_bypassed,
            branches,
            MIDI_RACK_MAX_INPUT_EVENTS,
            MIDI_RACK_MAX_OUTPUT_EVENTS,
            MIDI_RACK_MAX_ACTIVE_OCCURRENCES,
            MIDI_RACK_MAX_WORK_UNITS,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_caps(
        rack_id: NonZeroU64,
        branch_ids: [NonZeroU64; MIDI_RACK_BRANCH_COUNT],
        branch_bypassed: [bool; MIDI_RACK_BRANCH_COUNT],
        branches: [Option<Box<dyn PreparedNativeMidiProcessor>>; MIDI_RACK_BRANCH_COUNT],
        max_input_events: usize,
        max_output_events: usize,
        max_active_occurrences: usize,
        max_work_units: usize,
    ) -> Result<Self, DeviceContractError> {
        if branch_ids[0] == branch_ids[1] {
            return Err(DeviceContractError::InvalidId);
        }
        if max_input_events == 0
            || max_output_events == 0
            || !(MIDI_RACK_BRANCH_COUNT..=MIDI_RACK_MAX_ACTIVE_OCCURRENCES)
                .contains(&max_active_occurrences)
            || max_work_units == 0
        {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let mut declared_twin_bytes = std::mem::size_of::<MidiRackRuntime>();
        for branch in branches.iter().flatten() {
            declared_twin_bytes = declared_twin_bytes
                .checked_add(branch.declared_twin_bytes())
                .ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        }
        Ok(Self {
            rack_id,
            branch_ids,
            branch_bypassed,
            branches,
            max_input_events,
            max_output_events,
            max_active_occurrences,
            max_work_units,
            declared_twin_bytes,
        })
    }
}

impl PreparedNativeMidiProcessor for PreparedMidiRack {
    fn runtime_version(&self) -> u32 {
        MIDI_RACK_RUNTIME_VERSION
    }

    fn declared_twin_bytes(&self) -> usize {
        self.declared_twin_bytes
    }

    fn max_input_events_per_block(&self) -> usize {
        self.max_input_events
    }

    fn max_output_events_per_block(&self) -> usize {
        self.max_output_events
    }

    fn max_active_occurrences(&self) -> usize {
        self.max_active_occurrences
    }

    fn max_work_units_per_block(&self) -> usize {
        self.max_work_units
    }

    fn create_runtime(&self) -> Box<dyn MutableMidiProcessorRuntime> {
        let default_event = NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        };
        let branch_runtimes = [
            self.branches[0].as_deref().map(|b| b.create_runtime()),
            self.branches[1].as_deref().map(|b| b.create_runtime()),
        ];
        Box::new(MidiRackRuntime {
            rack_id: self.rack_id,
            branch_ids: self.branch_ids,
            branch_bypassed: self.branch_bypassed,
            max_input_events: self.max_input_events,
            max_output_events: self.max_output_events,
            max_active_occurrences: self.max_active_occurrences,
            max_branch_active: self.max_active_occurrences / MIDI_RACK_BRANCH_COUNT,
            max_work_units: self.max_work_units,
            branch_runtimes,
            active_inputs: [None; MIDI_RACK_MAX_ACTIVE_OCCURRENCES],
            branch_routed_inputs: [[None; MIDI_RACK_MAX_ACTIVE_OCCURRENCES];
                MIDI_RACK_BRANCH_COUNT],
            branch_out_map: [[None; MIDI_RACK_MAX_BRANCH_ACTIVE_OCCURRENCES];
                MIDI_RACK_BRANCH_COUNT],
            next_rack_id: GENERATED_OCCURRENCE_ID_BASE,
            branch_in_scratch: [default_event; MIDI_RACK_MAX_INPUT_EVENTS],
            branch_out_scratch: [default_event; MIDI_RACK_MAX_OUTPUT_EVENTS],
            merged_scratch: [default_event; MIDI_RACK_MAX_OUTPUT_EVENTS],
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BranchOccurrenceMapping {
    branch_id: u64,
    rack_id: u64,
}

struct MidiRackRuntime {
    rack_id: NonZeroU64,
    branch_ids: [NonZeroU64; MIDI_RACK_BRANCH_COUNT],
    branch_bypassed: [bool; MIDI_RACK_BRANCH_COUNT],
    max_input_events: usize,
    max_output_events: usize,
    max_active_occurrences: usize,
    max_branch_active: usize,
    max_work_units: usize,
    branch_runtimes: [Option<Box<dyn MutableMidiProcessorRuntime>>; MIDI_RACK_BRANCH_COUNT],
    active_inputs: [Option<u64>; MIDI_RACK_MAX_ACTIVE_OCCURRENCES],
    branch_routed_inputs: [[Option<u64>; MIDI_RACK_MAX_ACTIVE_OCCURRENCES]; MIDI_RACK_BRANCH_COUNT],
    branch_out_map: [[Option<BranchOccurrenceMapping>; MIDI_RACK_MAX_BRANCH_ACTIVE_OCCURRENCES];
        MIDI_RACK_BRANCH_COUNT],
    next_rack_id: u64,
    branch_in_scratch: [NativeInstrumentEvent; MIDI_RACK_MAX_INPUT_EVENTS],
    branch_out_scratch: [NativeInstrumentEvent; MIDI_RACK_MAX_OUTPUT_EVENTS],
    merged_scratch: [NativeInstrumentEvent; MIDI_RACK_MAX_OUTPUT_EVENTS],
}

impl MidiRackRuntime {
    fn find_u64(slots: &[Option<u64>], id: u64) -> Option<usize> {
        slots.iter().position(|slot| *slot == Some(id))
    }

    fn find_free_u64(slots: &[Option<u64>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_u64(slots: &[Option<u64>]) -> usize {
        slots.iter().flatten().count()
    }

    fn find_map(slots: &[Option<BranchOccurrenceMapping>], branch_id: u64) -> Option<usize> {
        slots
            .iter()
            .position(|slot| slot.is_some_and(|m| m.branch_id == branch_id))
    }

    fn find_free_map(slots: &[Option<BranchOccurrenceMapping>]) -> Option<usize> {
        slots.iter().position(Option::is_none)
    }

    fn count_map(slots: &[Option<BranchOccurrenceMapping>]) -> usize {
        slots.iter().flatten().count()
    }

    /// Build the filtered input batch for branch `b` into `dst` without
    /// modifying `self`. Returns `(len, next_routed_inputs)`.
    fn build_branch_input(
        bypassed: bool,
        routed_in: &[Option<u64>; MIDI_RACK_MAX_ACTIVE_OCCURRENCES],
        input_cap: usize,
        input: &[NativeInstrumentEvent],
        dst: &mut [NativeInstrumentEvent; MIDI_RACK_MAX_INPUT_EVENTS],
    ) -> Result<(usize, [Option<u64>; MIDI_RACK_MAX_ACTIVE_OCCURRENCES]), NativeMidiProcessorError>
    {
        let mut next_routed = *routed_in;
        let mut len = 0_usize;
        for &event in input {
            match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if !bypassed {
                        let free = Self::find_free_u64(&next_routed[..input_cap])
                            .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                        next_routed[free] = Some(occurrence_id);
                        dst[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_u64(&next_routed[..input_cap], occurrence_id) {
                        next_routed[slot] = None;
                        dst[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_u64(&next_routed[..input_cap], occurrence_id.get()).is_some() {
                        dst[len] = event;
                        len += 1;
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {
                    if !bypassed {
                        dst[len] = event;
                        len += 1;
                    }
                }
            }
        }
        Ok((len, next_routed))
    }

    fn sort_merged(events: &mut [NativeInstrumentEvent]) {
        let sort_key = |ev: NativeInstrumentEvent| -> (usize, u8) {
            let rank = match ev {
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => 0,
                NativeInstrumentEvent::NoteOff { .. } => 1,
                NativeInstrumentEvent::NoteOn { .. } => 2,
                NativeInstrumentEvent::NotePitchBend { .. } => 3,
                NativeInstrumentEvent::NoteTimbre { .. } => 4,
                NativeInstrumentEvent::NotePressure { .. } => 5,
            };
            (ev.frame_offset(), rank)
        };
        for i in 1..events.len() {
            let cur = events[i];
            let key = sort_key(cur);
            let mut j = i;
            while j > 0 && sort_key(events[j - 1]) > key {
                events[j] = events[j - 1];
                j -= 1;
            }
            events[j] = cur;
        }
    }
}

impl MutableMidiProcessorRuntime for MidiRackRuntime {
    fn preflight_block(
        &self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output_capacity: usize,
        clock: Option<&dyn NativeInstrumentClock>,
    ) -> Result<usize, NativeMidiProcessorError> {
        if frame_count == 0 {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        if input.len() > self.max_input_events {
            return Err(NativeMidiProcessorError::InputEventCapacityExceeded);
        }
        let mut sim_inputs = self.active_inputs;
        let in_cap = self.max_active_occurrences;
        for &event in input {
            match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if Self::find_u64(&sim_inputs[..in_cap], occurrence_id).is_some() {
                        return Err(NativeMidiProcessorError::DuplicateOccurrence);
                    }
                    let free = Self::find_free_u64(&sim_inputs[..in_cap])
                        .ok_or(NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded)?;
                    sim_inputs[free] = Some(occurrence_id);
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    let slot = Self::find_u64(&sim_inputs[..in_cap], occurrence_id)
                        .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                    sim_inputs[slot] = None;
                }
                NativeInstrumentEvent::NotePressure { occurrence_id, .. }
                | NativeInstrumentEvent::NotePitchBend { occurrence_id, .. }
                | NativeInstrumentEvent::NoteTimbre { occurrence_id, .. } => {
                    if Self::find_u64(&sim_inputs[..in_cap], occurrence_id.get()).is_none() {
                        return Err(NativeMidiProcessorError::InvalidInputEventBatch);
                    }
                }
                NativeInstrumentEvent::Expression { .. }
                | NativeInstrumentEvent::ControlChange { .. }
                | NativeInstrumentEvent::PitchBend { .. } => {}
            }
        }

        let effective_cap = output_capacity.min(self.max_output_events);
        let mut total_predicted = 0_usize;
        let mut work_units = input.len();
        let default_event = NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        };
        let mut branch_in = [default_event; MIDI_RACK_MAX_INPUT_EVENTS];

        for b in 0..MIDI_RACK_BRANCH_COUNT {
            let (b_len, _) = Self::build_branch_input(
                self.branch_bypassed[b],
                &self.branch_routed_inputs[b],
                self.max_branch_active,
                input,
                &mut branch_in,
            )?;
            let remaining_cap = effective_cap.saturating_sub(total_predicted);
            let predicted = if let Some(runtime) = self.branch_runtimes[b].as_deref() {
                if b_len > 0 || !runtime.is_quiescent() {
                    runtime.preflight_block(
                        &branch_in[..b_len],
                        frame_count,
                        remaining_cap,
                        clock,
                    )?
                } else {
                    0
                }
            } else {
                // Dry pass-through branch emits exactly its filtered input events;
                // verify active downstream occurrence count stays within per-branch cap.
                let mut sim_map = self.branch_out_map[b];
                for &ev in &branch_in[..b_len] {
                    match ev {
                        NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                            let free = Self::find_free_map(&sim_map[..self.max_branch_active])
                                .ok_or(
                                    NativeMidiProcessorError::ActiveOccurrenceCapacityExceeded,
                                )?;
                            sim_map[free] = Some(BranchOccurrenceMapping {
                                branch_id: occurrence_id,
                                rack_id: 1,
                            });
                        }
                        NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                            let slot =
                                Self::find_map(&sim_map[..self.max_branch_active], occurrence_id)
                                    .ok_or(NativeMidiProcessorError::OrphanedNoteOff)?;
                            sim_map[slot] = None;
                        }
                        _ => {}
                    }
                }
                b_len
            };
            total_predicted = total_predicted
                .checked_add(predicted)
                .ok_or(NativeMidiProcessorError::OutputEventCapacityExceeded)?;
            work_units = work_units
                .checked_add(b_len)
                .and_then(|w| w.checked_add(predicted))
                .ok_or(NativeMidiProcessorError::WorkBudgetExceeded)?;
            if total_predicted > effective_cap {
                return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
            }
            if work_units > self.max_work_units {
                return Err(NativeMidiProcessorError::WorkBudgetExceeded);
            }
        }
        Ok(total_predicted)
    }

    fn process_block(
        &mut self,
        input: &[NativeInstrumentEvent],
        frame_count: usize,
        output: &mut MidiProcessorOutput<'_>,
        clock: Option<&dyn NativeInstrumentClock>,
    ) {
        let in_cap = self.max_active_occurrences;
        let _ = output.charge_work(input.len());
        for &event in input {
            match event {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => {
                    if let Some(free) = Self::find_free_u64(&self.active_inputs[..in_cap]) {
                        self.active_inputs[free] = Some(occurrence_id);
                    }
                }
                NativeInstrumentEvent::NoteOff { occurrence_id, .. } => {
                    if let Some(slot) = Self::find_u64(&self.active_inputs[..in_cap], occurrence_id)
                    {
                        self.active_inputs[slot] = None;
                    }
                }
                _ => {}
            }
        }

        let out_cap = output.remaining_capacity().min(self.max_output_events);
        let mut merged_len = 0_usize;

        for b in 0..MIDI_RACK_BRANCH_COUNT {
            let Ok((b_len, next_routed)) = Self::build_branch_input(
                self.branch_bypassed[b],
                &self.branch_routed_inputs[b],
                self.max_branch_active,
                input,
                &mut self.branch_in_scratch,
            ) else {
                return;
            };
            self.branch_routed_inputs[b] = next_routed;
            let _ = output.charge_work(b_len);

            let remaining = out_cap.saturating_sub(merged_len);
            let b_out_len = if let Some(runtime) = self.branch_runtimes[b].as_deref_mut() {
                if b_len > 0 || !runtime.is_quiescent() {
                    let mut sink = MidiProcessorOutput::new(
                        &mut self.branch_out_scratch[..remaining],
                        remaining,
                        self.max_work_units,
                    );
                    runtime.process_block(
                        &self.branch_in_scratch[..b_len],
                        frame_count,
                        &mut sink,
                        clock,
                    );
                    sink.len()
                } else {
                    0
                }
            } else {
                let n = b_len.min(remaining);
                self.branch_out_scratch[..n].copy_from_slice(&self.branch_in_scratch[..n]);
                n
            };

            // Translate branch-local occurrence IDs into disjoint rack-owned IDs.
            let b_cap = self.max_branch_active;
            for idx in 0..b_out_len {
                let raw = self.branch_out_scratch[idx];
                let translated = match raw {
                    NativeInstrumentEvent::NoteOn {
                        frame_offset,
                        occurrence_id,
                        pitch,
                        velocity,
                    } => {
                        if let Some(free) = Self::find_free_map(&self.branch_out_map[b][..b_cap]) {
                            let rack_id = self.next_rack_id;
                            self.next_rack_id =
                                rack_id.wrapping_add(1).max(GENERATED_OCCURRENCE_ID_BASE);
                            self.branch_out_map[b][free] = Some(BranchOccurrenceMapping {
                                branch_id: occurrence_id,
                                rack_id,
                            });
                            Some(NativeInstrumentEvent::NoteOn {
                                frame_offset,
                                occurrence_id: rack_id,
                                pitch,
                                velocity,
                            })
                        } else {
                            None
                        }
                    }
                    NativeInstrumentEvent::NoteOff {
                        frame_offset,
                        occurrence_id,
                    } => Self::find_map(&self.branch_out_map[b][..b_cap], occurrence_id)
                        .and_then(|slot| self.branch_out_map[b][slot].take())
                        .map(|m| NativeInstrumentEvent::NoteOff {
                            frame_offset,
                            occurrence_id: m.rack_id,
                        }),
                    NativeInstrumentEvent::NotePressure {
                        frame_offset,
                        occurrence_id,
                        value,
                    } => Self::find_map(&self.branch_out_map[b][..b_cap], occurrence_id.get())
                        .and_then(|slot| self.branch_out_map[b][slot])
                        .and_then(|m| NonZeroU64::new(m.rack_id))
                        .map(|rack_id| NativeInstrumentEvent::NotePressure {
                            frame_offset,
                            occurrence_id: rack_id,
                            value,
                        }),
                    NativeInstrumentEvent::NotePitchBend {
                        frame_offset,
                        occurrence_id,
                        cents,
                    } => Self::find_map(&self.branch_out_map[b][..b_cap], occurrence_id.get())
                        .and_then(|slot| self.branch_out_map[b][slot])
                        .and_then(|m| NonZeroU64::new(m.rack_id))
                        .map(|rack_id| NativeInstrumentEvent::NotePitchBend {
                            frame_offset,
                            occurrence_id: rack_id,
                            cents,
                        }),
                    NativeInstrumentEvent::NoteTimbre {
                        frame_offset,
                        occurrence_id,
                        value,
                    } => Self::find_map(&self.branch_out_map[b][..b_cap], occurrence_id.get())
                        .and_then(|slot| self.branch_out_map[b][slot])
                        .and_then(|m| NonZeroU64::new(m.rack_id))
                        .map(|rack_id| NativeInstrumentEvent::NoteTimbre {
                            frame_offset,
                            occurrence_id: rack_id,
                            value,
                        }),
                    other => Some(other),
                };
                if let Some(ev) = translated
                    && merged_len < out_cap
                {
                    self.merged_scratch[merged_len] = ev;
                    merged_len += 1;
                }
            }
        }

        Self::sort_merged(&mut self.merged_scratch[..merged_len]);
        let _ = output.charge_work(merged_len);
        for idx in 0..merged_len {
            let _ = output.push(self.merged_scratch[idx]);
        }
    }

    fn preflight_flush_active_note_offs(
        &self,
        frame_offset: usize,
        frame_count: usize,
        output_capacity: usize,
    ) -> Result<usize, NativeMidiProcessorError> {
        if frame_count == 0 || frame_offset >= frame_count {
            return Err(NativeMidiProcessorError::InvalidInputEventBatch);
        }
        let active: usize = (0..MIDI_RACK_BRANCH_COUNT)
            .map(|b| Self::count_map(&self.branch_out_map[b][..self.max_branch_active]))
            .sum();
        if active > output_capacity || active > self.max_output_events {
            return Err(NativeMidiProcessorError::OutputEventCapacityExceeded);
        }
        if active > self.max_work_units {
            return Err(NativeMidiProcessorError::WorkBudgetExceeded);
        }
        Ok(active)
    }

    fn flush_active_note_offs(
        &mut self,
        frame_offset: usize,
        output: &mut MidiProcessorOutput<'_>,
    ) {
        let mut ids = [0_u64; MIDI_RACK_MAX_ACTIVE_OCCURRENCES];
        let mut len = 0_usize;
        for b in 0..MIDI_RACK_BRANCH_COUNT {
            for slot in &mut self.branch_out_map[b][..self.max_branch_active] {
                if let Some(m) = slot.take()
                    && len < ids.len()
                {
                    ids[len] = m.rack_id;
                    len += 1;
                }
            }
            self.branch_routed_inputs[b].fill(None);
            if let Some(runtime) = self.branch_runtimes[b].as_deref_mut() {
                runtime.reset();
            }
        }
        self.active_inputs.fill(None);
        ids[..len].sort_unstable();
        for &occurrence_id in &ids[..len] {
            let _ = output.charge_work(1);
            let _ = output.push(NativeInstrumentEvent::NoteOff {
                frame_offset,
                occurrence_id,
            });
        }
    }

    fn active_occurrence_count(&self) -> usize {
        let downstream: usize = (0..MIDI_RACK_BRANCH_COUNT)
            .map(|b| Self::count_map(&self.branch_out_map[b][..self.max_branch_active]))
            .sum();
        let inputs = Self::count_u64(&self.active_inputs[..self.max_active_occurrences]);
        downstream.max(inputs)
    }

    fn is_quiescent(&self) -> bool {
        self.active_occurrence_count() == 0
            && self
                .branch_runtimes
                .iter()
                .flatten()
                .all(|runtime| runtime.is_quiescent())
    }

    fn reset(&mut self) {
        self.active_inputs.fill(None);
        for b in 0..MIDI_RACK_BRANCH_COUNT {
            self.branch_routed_inputs[b].fill(None);
            self.branch_out_map[b].fill(None);
            if let Some(runtime) = self.branch_runtimes[b].as_deref_mut() {
                runtime.reset();
            }
        }
        self.next_rack_id = GENERATED_OCCURRENCE_ID_BASE;
    }

    fn can_copy_history_from(&self, previous: &dyn MutableMidiProcessorRuntime) -> bool {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return false;
        };
        if self.rack_id != other.rack_id || self.branch_ids != other.branch_ids {
            return false;
        }
        if other.active_occurrence_count() > self.max_active_occurrences {
            return false;
        }
        for b in 0..MIDI_RACK_BRANCH_COUNT {
            match (&self.branch_runtimes[b], &other.branch_runtimes[b]) {
                (Some(dst), Some(src)) => {
                    if !dst.can_copy_history_from(src.as_ref()) {
                        return false;
                    }
                }
                (None, None) => {}
                _ => return false,
            }
        }
        true
    }

    fn copy_history_from(&mut self, previous: &dyn MutableMidiProcessorRuntime) {
        let Some(other) = previous.as_any().downcast_ref::<Self>() else {
            return;
        };
        self.active_inputs = other.active_inputs;
        self.branch_routed_inputs = other.branch_routed_inputs;
        self.branch_out_map = other.branch_out_map;
        self.next_rack_id = other.next_rack_id;
        for b in 0..MIDI_RACK_BRANCH_COUNT {
            if let (Some(dst), Some(src)) = (
                self.branch_runtimes[b].as_deref_mut(),
                other.branch_runtimes[b].as_deref(),
            ) {
                dst.copy_history_from(src);
            }
        }
    }

    fn fixed_storage_bytes(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();
        for runtime in self.branch_runtimes.iter().flatten() {
            total = total.saturating_add(runtime.fixed_storage_bytes());
        }
        total
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NativeMidiProcessorLimits, NativeMidiProcessorRuntimePair,
        conformance::midi_transpose::{PreparedTransposeMidiProcessor, TransposeOutOfRangePolicy},
    };

    fn test_limits() -> NativeMidiProcessorLimits {
        NativeMidiProcessorLimits {
            max_pair_bytes: 256 * 1024,
            max_input_events_per_block: MIDI_RACK_MAX_INPUT_EVENTS,
            max_output_events_per_block: MIDI_RACK_MAX_OUTPUT_EVENTS,
            max_active_occurrences: MIDI_RACK_MAX_ACTIVE_OCCURRENCES,
            max_work_units_per_block: MIDI_RACK_MAX_WORK_UNITS,
        }
    }

    #[test]
    fn midi_rack_codec_validation_depth_2_admission_depth_3_and_cycle_refusal() {
        let default = MidiRackSpec::default();
        let encoded = default.encode_state().unwrap();
        assert_eq!(MidiRackSpec::decode_state(&encoded).unwrap(), default);

        // Duplicate branch_id or device_id refuses explicitly.
        let mut dup_branch = default.clone();
        dup_branch.branches[1].branch_id = dup_branch.branches[0].branch_id;
        assert_eq!(
            dup_branch.encode_state(),
            Err(DeviceContractError::InvalidId)
        );

        let mut dup_dev = default.clone();
        dup_dev.branches[1].device_id = dup_dev.branches[0].device_id;
        assert_eq!(dup_dev.encode_state(), Err(DeviceContractError::InvalidId));

        // Depth-2 nested MIDI Rack with distinct IDs round-trips cleanly.
        let inner = MidiRackSpec {
            rack_id: NonZeroU64::new(2).unwrap(),
            branches: [
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(21).unwrap(),
                    device_id: NonZeroU64::new(201).unwrap(),
                    bypassed: false,
                    state: None,
                },
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(22).unwrap(),
                    device_id: NonZeroU64::new(202).unwrap(),
                    bypassed: false,
                    state: None,
                },
            ],
        };
        let inner_encoded = inner.encode_state().unwrap();
        let mut outer = default.clone();
        outer.branches[0].state = Some(inner_encoded.clone());
        let outer_encoded = outer.encode_state().unwrap();
        assert_eq!(MidiRackSpec::decode_state(&outer_encoded).unwrap(), outer);

        // Self-reference / cycle (duplicate rack_id, branch_id, or device_id across levels) refuses.
        let mut cycle_rack = outer.clone();
        cycle_rack.rack_id = inner.rack_id;
        assert_eq!(
            cycle_rack.encode_state(),
            Err(DeviceContractError::InvalidId)
        );
        let mut dup_nested_branch = outer.clone();
        dup_nested_branch.branches[1].branch_id = inner.branches[0].branch_id;
        assert_eq!(
            dup_nested_branch.encode_state(),
            Err(DeviceContractError::InvalidId)
        );
        let mut dup_nested_dev = outer.clone();
        dup_nested_dev.branches[1].device_id = inner.branches[1].device_id;
        assert_eq!(
            dup_nested_dev.encode_state(),
            Err(DeviceContractError::InvalidId)
        );

        // Depth-3 nesting exceeds MIDI_RACK_MAX_DEPTH == 2 and refuses explicitly.
        let mut depth3 = MidiRackSpec {
            rack_id: NonZeroU64::new(9).unwrap(),
            branches: [
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(91).unwrap(),
                    device_id: NonZeroU64::new(901).unwrap(),
                    bypassed: false,
                    state: Some(outer_encoded),
                },
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(92).unwrap(),
                    device_id: NonZeroU64::new(902).unwrap(),
                    bypassed: false,
                    state: None,
                },
            ],
        };
        assert_eq!(
            depth3.encode_state(),
            Err(DeviceContractError::UnsupportedProcessingClass)
        );
        // Ensure a valid sibling nested rack in both outer branches (still depth 2) is admitted.
        let inner_b = MidiRackSpec {
            rack_id: NonZeroU64::new(3).unwrap(),
            branches: [
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(31).unwrap(),
                    device_id: NonZeroU64::new(301).unwrap(),
                    bypassed: false,
                    state: None,
                },
                MidiRackBranchSpec {
                    branch_id: NonZeroU64::new(32).unwrap(),
                    device_id: NonZeroU64::new(302).unwrap(),
                    bypassed: false,
                    state: None,
                },
            ],
        };
        depth3.branches[0].state = Some(inner_encoded);
        depth3.branches[1].state = Some(inner_b.encode_state().unwrap());
        assert!(depth3.encode_state().is_ok());

        // Malformed payload refuses explicitly.
        let bad = NativeDeviceState::new(MIDI_RACK_TYPE_ID, 1, vec![0, 1, 2]).unwrap();
        assert_eq!(
            MidiRackSpec::decode_state(&bad),
            Err(DeviceContractError::MalformedState)
        );
    }

    #[test]
    fn nested_two_level_midi_rack_preserves_ids_same_offset_order_bypass_saturation_and_budget() {
        // Outer rack hosts TWO nested inner racks (4 parallel leaf transposes total):
        //   Inner A: +4 st, +7 st
        //   Inner B: +7 st (duplicate pitch!), +12 st
        let build_two_level = |inner_a_b1_bypass: bool| {
            let inner_a = PreparedMidiRack::with_caps(
                NonZeroU64::new(2).unwrap(),
                [NonZeroU64::new(21).unwrap(), NonZeroU64::new(22).unwrap()],
                [false, inner_a_b1_bypass],
                [
                    Some(Box::new(
                        PreparedTransposeMidiProcessor::new(4, TransposeOutOfRangePolicy::Refuse)
                            .unwrap(),
                    )),
                    Some(Box::new(
                        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse)
                            .unwrap(),
                    )),
                ],
                MIDI_RACK_MAX_INPUT_EVENTS,
                MIDI_RACK_MAX_OUTPUT_EVENTS,
                MIDI_RACK_MAX_BRANCH_ACTIVE_OCCURRENCES,
                MIDI_RACK_MAX_WORK_UNITS,
            )
            .unwrap();
            let inner_b = PreparedMidiRack::with_caps(
                NonZeroU64::new(3).unwrap(),
                [NonZeroU64::new(31).unwrap(), NonZeroU64::new(32).unwrap()],
                [false, false],
                [
                    Some(Box::new(
                        PreparedTransposeMidiProcessor::new(7, TransposeOutOfRangePolicy::Refuse)
                            .unwrap(),
                    )),
                    Some(Box::new(
                        PreparedTransposeMidiProcessor::new(12, TransposeOutOfRangePolicy::Refuse)
                            .unwrap(),
                    )),
                ],
                MIDI_RACK_MAX_INPUT_EVENTS,
                MIDI_RACK_MAX_OUTPUT_EVENTS,
                MIDI_RACK_MAX_BRANCH_ACTIVE_OCCURRENCES,
                MIDI_RACK_MAX_WORK_UNITS,
            )
            .unwrap();
            PreparedMidiRack::new(
                NonZeroU64::new(1).unwrap(),
                [NonZeroU64::new(10).unwrap(), NonZeroU64::new(20).unwrap()],
                [false, false],
                [Some(Box::new(inner_a)), Some(Box::new(inner_b))],
            )
            .unwrap()
        };

        let outer = build_two_level(false);
        // Full 2-level rack (3 racks + 4 leaves per twin) fits within 256 KiB pair budget!
        let mut pair = NativeMidiProcessorRuntimePair::new(&outer, test_limits()).unwrap();
        let mut out_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 64];

        // Simultaneous NoteOn at frame 0 -> 4 distinct NoteOns in deterministic branch order:
        // 64 (inner A b0), 67 (inner A b1), 67 (inner B b0), 72 (inner B b1).
        let on = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch: 60,
            velocity: 100,
        }];
        pair.begin_block();
        let n = pair.process_block(&on, 64, &mut out_buf, None).unwrap();
        pair.commit_block();
        assert_eq!(n, 4);
        let pitches: Vec<u8> = out_buf[..n]
            .iter()
            .filter_map(|e| match *e {
                NativeInstrumentEvent::NoteOn { pitch, .. } => Some(pitch),
                _ => None,
            })
            .collect();
        assert_eq!(pitches, vec![64, 67, 67, 72]);
        let mut ids: Vec<u64> = out_buf[..n]
            .iter()
            .filter_map(|e| match *e {
                NativeInstrumentEvent::NoteOn { occurrence_id, .. } => Some(occurrence_id),
                _ => None,
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);

        // Saturated terminal delivery: output slice too small for 4 active NoteOffs refuses cleanly
        // without mutating committed active state; full flush afterwards leaves zero stuck notes!
        let mut tiny_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 2];
        pair.begin_block();
        assert_eq!(
            pair.flush_active_note_offs(0, 64, &mut tiny_buf),
            Err(NativeMidiProcessorError::OutputEventCapacityExceeded)
        );
        pair.rollback_block();
        assert_eq!(pair.committed_active_occurrences(), 4);

        // Mid-note inner branch bypass via live history inheritance:
        // held note releases all 4 voices; next note emits only 3 (64, 67, 72).
        let outer_inner_bypass = build_two_level(true);
        let mut pair_bypass =
            NativeMidiProcessorRuntimePair::new(&outer_inner_bypass, test_limits()).unwrap();
        pair_bypass.inherit_from(&pair).unwrap();
        let rel_and_new = [
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 2,
                pitch: 60,
                velocity: 95,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 20,
                occurrence_id: 2,
            },
        ];
        pair_bypass.begin_block();
        let n2 = pair_bypass
            .process_block(&rel_and_new, 64, &mut out_buf, None)
            .unwrap();
        pair_bypass.commit_block();
        // Same-offset ordering at frame 4: all 4 NoteOffs precede the 3 NoteOns, then 3 NoteOffs at frame 20.
        assert_eq!(n2, 10);
        assert!(out_buf[..4].iter().all(|e| matches!(
            e,
            NativeInstrumentEvent::NoteOff {
                frame_offset: 4,
                ..
            }
        )));
        let new_pitches: Vec<u8> = out_buf[4..7]
            .iter()
            .filter_map(|e| match *e {
                NativeInstrumentEvent::NoteOn {
                    frame_offset: 4,
                    pitch,
                    ..
                } => Some(pitch),
                _ => None,
            })
            .collect();
        assert_eq!(new_pitches, vec![64, 67, 72]);
        assert!(pair_bypass.committed_is_quiescent());

        // Full flush on original pair with adequate capacity releases all 4 held voices cleanly.
        pair.begin_block();
        let flushed = pair.flush_active_note_offs(0, 64, &mut out_buf).unwrap();
        pair.commit_block();
        assert_eq!(flushed, 4);
        assert!(pair.committed_is_quiescent());
    }

    #[test]
    fn two_branch_midi_rack_preserves_distinct_ids_duplicate_pitches_bypass_and_refusal() {
        // Branch 0: +4 st, Branch 1: +4 st (identical duplicate pitch outputs!)
        let b0 = PreparedTransposeMidiProcessor::new(4, TransposeOutOfRangePolicy::Refuse).unwrap();
        let b1 = PreparedTransposeMidiProcessor::new(4, TransposeOutOfRangePolicy::Refuse).unwrap();
        let rack = PreparedMidiRack::new(
            NonZeroU64::new(1).unwrap(),
            [NonZeroU64::new(10).unwrap(), NonZeroU64::new(20).unwrap()],
            [false, false],
            [Some(Box::new(b0)), Some(Box::new(b1))],
        )
        .unwrap();
        let mut pair = NativeMidiProcessorRuntimePair::new(&rack, test_limits()).unwrap();
        let mut out_buf = [NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: 1,
        }; 32];

        // Simultaneous overlapping C4 notes -> 4 NoteOns on pitch 64 with 4 DISTINCT rack IDs!
        let on_batch = [
            NativeInstrumentEvent::NoteOn {
                frame_offset: 0,
                occurrence_id: 1,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 4,
                occurrence_id: 2,
                pitch: 60,
                velocity: 90,
            },
        ];
        pair.begin_block();
        let n = pair
            .process_block(&on_batch, 64, &mut out_buf, None)
            .unwrap();
        pair.commit_block();
        assert_eq!(n, 4);
        let on_ids: Vec<u64> = out_buf[..n]
            .iter()
            .filter_map(|e| match *e {
                NativeInstrumentEvent::NoteOn {
                    occurrence_id,
                    pitch: 64,
                    ..
                } => Some(occurrence_id),
                _ => None,
            })
            .collect();
        assert_eq!(on_ids.len(), 4);
        let mut sorted = on_ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            4,
            "duplicate pitches must carry distinct rack IDs"
        );

        // Toggle branch 1 bypass ON mid-note via live history inheritance:
        // held notes from BOTH branches still release cleanly; new note emits only branch 0!
        let rack_b1_bypass = PreparedMidiRack::new(
            NonZeroU64::new(1).unwrap(),
            [NonZeroU64::new(10).unwrap(), NonZeroU64::new(20).unwrap()],
            [false, true],
            [
                Some(Box::new(
                    PreparedTransposeMidiProcessor::new(4, TransposeOutOfRangePolicy::Refuse)
                        .unwrap(),
                )),
                Some(Box::new(
                    PreparedTransposeMidiProcessor::new(4, TransposeOutOfRangePolicy::Refuse)
                        .unwrap(),
                )),
            ],
        )
        .unwrap();
        let mut pair_bypass =
            NativeMidiProcessorRuntimePair::new(&rack_b1_bypass, test_limits()).unwrap();
        pair_bypass.inherit_from(&pair).unwrap();

        let release_and_new = [
            NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: 1,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 2,
                occurrence_id: 2,
            },
            NativeInstrumentEvent::NoteOn {
                frame_offset: 10,
                occurrence_id: 3,
                pitch: 60,
                velocity: 100,
            },
            NativeInstrumentEvent::NoteOff {
                frame_offset: 20,
                occurrence_id: 3,
            },
        ];
        pair_bypass.begin_block();
        let n2 = pair_bypass
            .process_block(&release_and_new, 64, &mut out_buf, None)
            .unwrap();
        pair_bypass.commit_block();
        // 4 NoteOffs for held notes + 1 NoteOn (branch 0 only) + 1 NoteOff = 6 events!
        assert_eq!(n2, 6);
        assert!(pair_bypass.committed_is_quiescent());

        // Refusal (out-of-range transpose in branch) keeps committed state intact!
        let bad_note = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 99,
            pitch: 126,
            velocity: 100,
        }];
        pair.begin_block();
        assert!(
            pair.process_block(&bad_note, 64, &mut out_buf, None)
                .is_err()
        );
        pair.rollback_block();
        assert_eq!(pair.committed_active_occurrences(), 4);
    }
}
