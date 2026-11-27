//! Bounded nested native Audio Rack with latency-compensated parallel sum.
//!
//! An Audio Rack owns two ordered serial effect branches (`AUDIO_RACK_BRANCH_COUNT == 2`)
//! prepared through the existing `NativeDeviceRegistry` (never a parallel registry)
//! and summed in exact phase alignment by delaying the lower-latency branch by
//! `rack_latency_frames - branch_latency_frames`.
//!
//! Nesting is explicitly bounded at `AUDIO_RACK_MAX_DEPTH == 2`: an outer Audio
//! Rack branch may host up to `AUDIO_RACK_MAX_NESTED_RACKS_PER_BRANCH` nested
//! depth-2 Audio Racks inline with serial leaf devices. Deeper nesting
//! (`depth > 2`), duplicate rack/branch/device identities across levels
//! (self-reference/cycles), unknown latencies, and budget overflows are rejected
//! atomically on control before touching active state. All branch chains, nested
//! racks, compensation delay rings, rack-bypass dry delays, and block scratch
//! buffers are preallocated on control so callback rendering performs zero
//! allocations, locks, I/O, or final drops.

#[cfg(test)]
mod tests;

use crate::{
    DeviceMeter, DevicePreparationContext, EMPTY_DEVICE_RESOURCES, MAX_MAIN_PASS_DELAY_FRAMES,
    MAX_PROCESSOR_BANK_RUNTIME_BYTES, NativeDeviceRegistry, NativeRuntimeChain,
    PreparedParameterAutomation, PreparedRuntimeInstance, ProcessorChainOwner,
    ProcessorInstanceIdentity, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, MAX_NATIVE_DEVICE_STATE_BYTES, NativeDeviceState,
};
use std::{fmt, num::NonZeroU64, sync::Arc};

/// Reserved native device type identifier for an Audio Rack.
pub const AUDIO_RACK_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rack.audio");
/// Prior flat-only wire format version migrated cleanly to `AUDIO_RACK_STATE_VERSION`.
pub const AUDIO_RACK_STATE_VERSION_V1_FLAT: u32 = 1;
/// Current persisted wire format version supporting depth-2 nested Audio Racks.
pub const AUDIO_RACK_STATE_VERSION: u32 = 2;
const AUDIO_RACK_STATE_MAGIC: &[u8; 4] = b"HXAR";
/// Fixed parallel branch count for this bounded Audio Rack contract.
pub const AUDIO_RACK_BRANCH_COUNT: usize = 2;
/// Explicit nesting ceiling: outer Audio Rack (depth 1) may host nested Audio
/// Racks (depth 2) whose branches contain flat serial devices only.
pub const AUDIO_RACK_MAX_DEPTH: usize = 2;
/// Maximum serial effect devices admitted in one branch.
pub const AUDIO_RACK_MAX_DEVICES_PER_BRANCH: usize = 16;
/// Maximum nested Audio Racks admitted inside one outer branch.
pub const AUDIO_RACK_MAX_NESTED_RACKS_PER_BRANCH: usize = 2;
/// Maximum total device slots across an entire nested Audio Rack tree.
pub const AUDIO_RACK_MAX_TOTAL_DEVICES: usize = 64;
/// Maximum per-branch latency compensation delay in frames.
pub const AUDIO_RACK_MAX_COMPENSATION_DELAY_FRAMES: u32 = MAX_MAIN_PASS_DELAY_FRAMES;
/// Maximum audio block length admitted by preallocated rack scratch storage.
pub const AUDIO_RACK_MAX_BLOCK_FRAMES: usize = 4_096;
/// Default combined runtime + scratch byte ceiling for one Audio Rack tree.
pub const AUDIO_RACK_MAX_PAIR_BYTES: usize = MAX_PROCESSOR_BANK_RUNTIME_BYTES;
/// Maximum linear branch gain (`+12 dB`).
pub const AUDIO_RACK_MAX_BRANCH_GAIN: f32 = 4.0;
/// Default work ceiling per block across outer and nested stages.
pub const AUDIO_RACK_MAX_WORK_UNITS_PER_BLOCK: usize =
    (AUDIO_RACK_MAX_TOTAL_DEVICES + 16) * AUDIO_RACK_MAX_BLOCK_FRAMES;

/// Caller-supplied finite ceilings validated on control before preparing a rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioRackLimits {
    pub max_pair_bytes: usize,
    pub max_devices_per_branch: usize,
    pub max_depth: usize,
    pub max_nested_racks_per_branch: usize,
    pub max_compensation_delay_frames: u32,
    pub max_block_frames: usize,
    pub max_work_units_per_block: usize,
}

impl Default for AudioRackLimits {
    fn default() -> Self {
        Self {
            max_pair_bytes: AUDIO_RACK_MAX_PAIR_BYTES,
            max_devices_per_branch: AUDIO_RACK_MAX_DEVICES_PER_BRANCH,
            max_depth: AUDIO_RACK_MAX_DEPTH,
            max_nested_racks_per_branch: AUDIO_RACK_MAX_NESTED_RACKS_PER_BRANCH,
            max_compensation_delay_frames: AUDIO_RACK_MAX_COMPENSATION_DELAY_FRAMES,
            max_block_frames: AUDIO_RACK_MAX_BLOCK_FRAMES,
            max_work_units_per_block: AUDIO_RACK_MAX_WORK_UNITS_PER_BLOCK,
        }
    }
}

/// Errors reported by Audio Rack validation, preparation, history, or rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioRackError {
    InvalidSampleRate,
    InvalidGain,
    DuplicateRackId,
    DuplicateBranchId,
    DuplicateDeviceId,
    RecursionProhibited,
    BranchDeviceCapacityExceeded,
    NestedRackCapacityExceeded,
    UnknownBranchLatency,
    CompensationDelayExceeded,
    RuntimeBudgetExceeded,
    WorkBudgetExceeded,
    AllocationFailed,
    BlockNotStarted,
    BlockTooLarge,
    InvalidEditIndex,
    UnknownTargetId,
    InvalidNestedState,
    DeviceContract(DeviceContractError),
}

impl AudioRackError {
    /// Allocation-free lowercase diagnostic message.
    pub const fn diagnostic_message(self) -> &'static str {
        match self {
            Self::InvalidSampleRate => "audio rack sample rate is out of range",
            Self::InvalidGain => "audio rack branch gain must be finite and within range",
            Self::DuplicateRackId => {
                "audio rack hierarchy must not repeat rack identities or cycle"
            }
            Self::DuplicateBranchId => "audio rack branches must have distinct identities",
            Self::DuplicateDeviceId => "audio rack child devices must have distinct identities",
            Self::RecursionProhibited => "nested audio rack exceeds maximum nesting depth",
            Self::BranchDeviceCapacityExceeded => "audio rack branch device limit is exceeded",
            Self::NestedRackCapacityExceeded => "audio rack branch nested rack limit is exceeded",
            Self::UnknownBranchLatency => "audio rack child device has unknown algorithmic latency",
            Self::CompensationDelayExceeded => {
                "audio rack branch latency difference exceeds compensation delay limit"
            }
            Self::RuntimeBudgetExceeded => "audio rack runtime exceeds caller storage budget",
            Self::WorkBudgetExceeded => "audio rack exceeds caller block work budget",
            Self::AllocationFailed => "audio rack control-side allocation failed",
            Self::BlockNotStarted => "audio rack transactional block was not started",
            Self::BlockTooLarge => "audio rack block length exceeds preallocated scratch capacity",
            Self::InvalidEditIndex => "audio rack edit target index is out of range",
            Self::UnknownTargetId => "audio rack edit target identity was not found",
            Self::InvalidNestedState => {
                "audio rack state payload is malformed or unsupported version"
            }
            Self::DeviceContract(_) => "audio rack child device contract failed",
        }
    }
}

impl fmt::Display for AudioRackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.diagnostic_message())
    }
}

impl std::error::Error for AudioRackError {}

impl From<DeviceContractError> for AudioRackError {
    fn from(error: DeviceContractError) -> Self {
        Self::DeviceContract(error)
    }
}

/// One serial effect device inside an Audio Rack branch.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRackDeviceSpec {
    pub device_id: NonZeroU64,
    pub state: NativeDeviceState,
    pub bypassed: bool,
}

/// One parallel effect branch inside an Audio Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRackBranchSpec {
    pub branch_id: NonZeroU64,
    pub gain: f32,
    pub muted: bool,
    pub devices: Vec<AudioRackDeviceSpec>,
}

/// Complete control-side specification for one bounded two-chain Audio Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioRackSpec {
    pub rack_id: NonZeroU64,
    pub sample_rate: u32,
    pub bypassed: bool,
    pub branches: [AudioRackBranchSpec; AUDIO_RACK_BRANCH_COUNT],
}

impl AudioRackSpec {
    /// Validate rack metadata, unique rack/branch/child identities across all
    /// levels, finite gains, device/nested caps, and bounded nesting depth.
    pub fn validate(&self, limits: &AudioRackLimits) -> Result<(), AudioRackError> {
        if limits.max_block_frames == 0
            || limits.max_block_frames > AUDIO_RACK_MAX_BLOCK_FRAMES
            || limits.max_devices_per_branch == 0
            || limits.max_devices_per_branch > AUDIO_RACK_MAX_DEVICES_PER_BRANCH
            || limits.max_depth == 0
            || limits.max_depth > AUDIO_RACK_MAX_DEPTH
            || limits.max_nested_racks_per_branch > AUDIO_RACK_MAX_NESTED_RACKS_PER_BRANCH
            || limits.max_compensation_delay_frames > AUDIO_RACK_MAX_COMPENSATION_DELAY_FRAMES
            || limits.max_pair_bytes == 0
            || limits.max_work_units_per_block == 0
        {
            return Err(AudioRackError::RuntimeBudgetExceeded);
        }
        let mut seen_racks = Vec::new();
        let mut seen_branches = Vec::new();
        let mut seen_devices = Vec::new();
        let mut total_devices = 0_usize;
        self.validate_at_depth(
            limits,
            1,
            self.sample_rate,
            &mut seen_racks,
            &mut seen_branches,
            &mut seen_devices,
            &mut total_devices,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_at_depth(
        &self,
        limits: &AudioRackLimits,
        depth: usize,
        expected_rate: u32,
        seen_racks: &mut Vec<u64>,
        seen_branches: &mut Vec<u64>,
        seen_devices: &mut Vec<u64>,
        total_devices: &mut usize,
    ) -> Result<(), AudioRackError> {
        if !(8_000..=192_000).contains(&self.sample_rate) || self.sample_rate != expected_rate {
            return Err(AudioRackError::InvalidSampleRate);
        }
        if depth == 0 || depth > limits.max_depth {
            return Err(AudioRackError::RecursionProhibited);
        }
        let rack_raw = self.rack_id.get();
        if seen_racks.contains(&rack_raw) {
            return Err(AudioRackError::DuplicateRackId);
        }
        seen_racks.push(rack_raw);

        for branch in &self.branches {
            let branch_raw = branch.branch_id.get();
            if seen_branches.contains(&branch_raw) {
                return Err(AudioRackError::DuplicateBranchId);
            }
            seen_branches.push(branch_raw);

            if !branch.gain.is_finite()
                || !(0.0..=AUDIO_RACK_MAX_BRANCH_GAIN).contains(&branch.gain)
            {
                return Err(AudioRackError::InvalidGain);
            }
            if branch.devices.len() > limits.max_devices_per_branch {
                return Err(AudioRackError::BranchDeviceCapacityExceeded);
            }
            let mut nested_count = 0_usize;
            for device in &branch.devices {
                let raw_id = device.device_id.get();
                if seen_devices.contains(&raw_id) {
                    return Err(AudioRackError::DuplicateDeviceId);
                }
                seen_devices.push(raw_id);
                *total_devices = total_devices
                    .checked_add(1)
                    .ok_or(AudioRackError::BranchDeviceCapacityExceeded)?;
                if *total_devices > AUDIO_RACK_MAX_TOTAL_DEVICES {
                    return Err(AudioRackError::BranchDeviceCapacityExceeded);
                }
                if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                    if depth >= limits.max_depth {
                        return Err(AudioRackError::RecursionProhibited);
                    }
                    nested_count += 1;
                    if nested_count > limits.max_nested_racks_per_branch {
                        return Err(AudioRackError::NestedRackCapacityExceeded);
                    }
                    let nested = Self::decode_state_raw(&device.state, false)?;
                    nested.validate_at_depth(
                        limits,
                        depth + 1,
                        expected_rate,
                        seen_racks,
                        seen_branches,
                        seen_devices,
                        total_devices,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Encode this Audio Rack spec into a strictly validated v2 `NativeDeviceState`
    /// supporting bounded depth-2 nesting.
    pub fn encode_state(&self) -> Result<NativeDeviceState, AudioRackError> {
        self.validate(&AudioRackLimits::default())?;
        self.encode_state_version(AUDIO_RACK_STATE_VERSION)
    }

    /// Encode a flat (depth-1 only) Audio Rack spec into the prior v1 wire format
    /// for migration testing. Rejects any spec containing nested Audio Racks.
    pub fn encode_v1_flat_state(&self) -> Result<NativeDeviceState, AudioRackError> {
        let flat_limits = AudioRackLimits {
            max_depth: 1,
            ..AudioRackLimits::default()
        };
        self.validate(&flat_limits)?;
        self.encode_state_version(AUDIO_RACK_STATE_VERSION_V1_FLAT)
    }

    fn encode_state_version(&self, version: u32) -> Result<NativeDeviceState, AudioRackError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(AUDIO_RACK_STATE_MAGIC);
        bytes.extend_from_slice(&self.rack_id.get().to_le_bytes());
        bytes.extend_from_slice(&self.sample_rate.to_le_bytes());
        bytes.push(u8::from(self.bypassed));
        for branch in &self.branches {
            bytes.extend_from_slice(&branch.branch_id.get().to_le_bytes());
            bytes.extend_from_slice(&branch.gain.to_le_bytes());
            bytes.push(u8::from(branch.muted));
            let count = u16::try_from(branch.devices.len())
                .map_err(|_| AudioRackError::BranchDeviceCapacityExceeded)?;
            bytes.extend_from_slice(&count.to_le_bytes());
            for device in &branch.devices {
                bytes.extend_from_slice(&device.device_id.get().to_le_bytes());
                bytes.push(u8::from(device.bypassed));
                let child_type = device.state.type_id();
                let type_str = child_type.as_str()?;
                let type_len = u16::try_from(type_str.len())
                    .map_err(|_| AudioRackError::InvalidNestedState)?;
                let payload_len = u32::try_from(device.state.payload().len())
                    .map_err(|_| AudioRackError::InvalidNestedState)?;
                bytes.extend_from_slice(&type_len.to_le_bytes());
                bytes.extend_from_slice(type_str.as_bytes());
                bytes.extend_from_slice(&device.state.state_version().to_le_bytes());
                bytes.extend_from_slice(&device.state.input_gain_millidb().to_le_bytes());
                bytes.extend_from_slice(&device.state.output_gain_millidb().to_le_bytes());
                bytes.extend_from_slice(&payload_len.to_le_bytes());
                bytes.extend_from_slice(device.state.payload());
                if bytes.len() > MAX_NATIVE_DEVICE_STATE_BYTES {
                    return Err(AudioRackError::RuntimeBudgetExceeded);
                }
            }
        }
        Ok(NativeDeviceState::new(AUDIO_RACK_TYPE_ID, version, bytes)?)
    }

    /// Strictly decode and validate a current v2 Audio Rack `NativeDeviceState`.
    pub fn decode_state(state: &NativeDeviceState) -> Result<Self, AudioRackError> {
        let spec = Self::decode_state_raw(state, false)?;
        spec.validate(&AudioRackLimits::default())?;
        Ok(spec)
    }

    /// Decode and migrate a v1 flat or v2 nested Audio Rack `NativeDeviceState`
    /// into the current canonical `AudioRackSpec`, rejecting unsupported or
    /// malformed payloads explicitly.
    pub fn decode_and_migrate_state(state: &NativeDeviceState) -> Result<Self, AudioRackError> {
        let spec = Self::decode_state_raw(state, true)?;
        if state.state_version() == AUDIO_RACK_STATE_VERSION_V1_FLAT {
            let flat_limits = AudioRackLimits {
                max_depth: 1,
                ..AudioRackLimits::default()
            };
            spec.validate(&flat_limits)?;
        } else {
            spec.validate(&AudioRackLimits::default())?;
        }
        Ok(spec)
    }

    fn decode_state_raw(state: &NativeDeviceState, allow_v1: bool) -> Result<Self, AudioRackError> {
        if state.type_id() != AUDIO_RACK_TYPE_ID {
            return Err(AudioRackError::InvalidNestedState);
        }
        let version = state.state_version();
        let valid_version = version == AUDIO_RACK_STATE_VERSION
            || (allow_v1 && version == AUDIO_RACK_STATE_VERSION_V1_FLAT);
        if !valid_version || !state.asset_references().is_empty() {
            return Err(AudioRackError::InvalidNestedState);
        }
        let payload = state.payload();
        let mut cursor = 0_usize;
        let read_bytes = |cursor: &mut usize, len: usize| -> Result<&[u8], AudioRackError> {
            let end = cursor
                .checked_add(len)
                .ok_or(AudioRackError::InvalidNestedState)?;
            let slice = payload
                .get(*cursor..end)
                .ok_or(AudioRackError::InvalidNestedState)?;
            *cursor = end;
            Ok(slice)
        };
        if read_bytes(&mut cursor, 4)? != AUDIO_RACK_STATE_MAGIC {
            return Err(AudioRackError::InvalidNestedState);
        }
        let rack_raw = u64::from_le_bytes(
            read_bytes(&mut cursor, 8)?
                .try_into()
                .map_err(|_| AudioRackError::InvalidNestedState)?,
        );
        let rack_id = NonZeroU64::new(rack_raw).ok_or(AudioRackError::InvalidNestedState)?;
        let sample_rate = u32::from_le_bytes(
            read_bytes(&mut cursor, 4)?
                .try_into()
                .map_err(|_| AudioRackError::InvalidNestedState)?,
        );
        let bypassed = match read_bytes(&mut cursor, 1)?[0] {
            0 => false,
            1 => true,
            _ => return Err(AudioRackError::InvalidNestedState),
        };

        let mut branches = Vec::with_capacity(AUDIO_RACK_BRANCH_COUNT);
        for _ in 0..AUDIO_RACK_BRANCH_COUNT {
            let branch_raw = u64::from_le_bytes(
                read_bytes(&mut cursor, 8)?
                    .try_into()
                    .map_err(|_| AudioRackError::InvalidNestedState)?,
            );
            let branch_id =
                NonZeroU64::new(branch_raw).ok_or(AudioRackError::InvalidNestedState)?;
            let gain = f32::from_le_bytes(
                read_bytes(&mut cursor, 4)?
                    .try_into()
                    .map_err(|_| AudioRackError::InvalidNestedState)?,
            );
            let muted = match read_bytes(&mut cursor, 1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(AudioRackError::InvalidNestedState),
            };
            let dev_count = usize::from(u16::from_le_bytes(
                read_bytes(&mut cursor, 2)?
                    .try_into()
                    .map_err(|_| AudioRackError::InvalidNestedState)?,
            ));
            if dev_count > AUDIO_RACK_MAX_DEVICES_PER_BRANCH {
                return Err(AudioRackError::BranchDeviceCapacityExceeded);
            }
            let mut devices = Vec::with_capacity(dev_count);
            for _ in 0..dev_count {
                let dev_raw = u64::from_le_bytes(
                    read_bytes(&mut cursor, 8)?
                        .try_into()
                        .map_err(|_| AudioRackError::InvalidNestedState)?,
                );
                let device_id =
                    NonZeroU64::new(dev_raw).ok_or(AudioRackError::InvalidNestedState)?;
                let dev_bypassed = match read_bytes(&mut cursor, 1)?[0] {
                    0 => false,
                    1 => true,
                    _ => return Err(AudioRackError::InvalidNestedState),
                };
                let type_len = usize::from(u16::from_le_bytes(
                    read_bytes(&mut cursor, 2)?
                        .try_into()
                        .map_err(|_| AudioRackError::InvalidNestedState)?,
                ));
                let type_str = std::str::from_utf8(read_bytes(&mut cursor, type_len)?)
                    .map_err(|_| AudioRackError::InvalidNestedState)?;
                let child_type =
                    DeviceTypeId::new(type_str).map_err(|_| AudioRackError::InvalidNestedState)?;
                let child_version = u32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| AudioRackError::InvalidNestedState)?,
                );
                let input_gain_millidb = i32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| AudioRackError::InvalidNestedState)?,
                );
                let output_gain_millidb = i32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| AudioRackError::InvalidNestedState)?,
                );
                let payload_len = usize::try_from(u32::from_le_bytes(
                    read_bytes(&mut cursor, 4)?
                        .try_into()
                        .map_err(|_| AudioRackError::InvalidNestedState)?,
                ))
                .map_err(|_| AudioRackError::InvalidNestedState)?;
                let child_payload = read_bytes(&mut cursor, payload_len)?.to_vec();
                let child_state = NativeDeviceState::new(child_type, child_version, child_payload)
                    .map_err(|_| AudioRackError::InvalidNestedState)?
                    .with_input_gain_millidb(input_gain_millidb)
                    .map_err(|_| AudioRackError::InvalidNestedState)?
                    .with_output_gain_millidb(output_gain_millidb)
                    .map_err(|_| AudioRackError::InvalidNestedState)?;
                devices.push(AudioRackDeviceSpec {
                    device_id,
                    state: child_state,
                    bypassed: dev_bypassed,
                });
            }
            branches.push(AudioRackBranchSpec {
                branch_id,
                gain,
                muted,
                devices,
            });
        }
        if cursor != payload.len() {
            return Err(AudioRackError::InvalidNestedState);
        }
        let branches: [AudioRackBranchSpec; AUDIO_RACK_BRANCH_COUNT] = branches
            .try_into()
            .map_err(|_| AudioRackError::InvalidNestedState)?;
        Ok(Self {
            rack_id,
            sample_rate,
            bypassed,
            branches,
        })
    }
}

/// Atomic one-step edit commands over an `AudioRackSpec` (including depth-2
/// group, ungroup, and stable-ID nested rack edits).
#[derive(Clone, Debug, PartialEq)]
pub enum AudioRackEdit {
    ReorderDevice {
        branch: usize,
        from: usize,
        to: usize,
    },
    RemoveDevice {
        branch: usize,
        index: usize,
    },
    SetDeviceBypassed {
        branch: usize,
        index: usize,
        bypassed: bool,
    },
    SetBranchGain {
        branch: usize,
        gain: f32,
    },
    SetBranchMuted {
        branch: usize,
        muted: bool,
    },
    ClearBranch {
        branch: usize,
    },
    SetRackBypassed {
        bypassed: bool,
    },
    ReplaceBranchDevices {
        branch: usize,
        devices: Vec<AudioRackDeviceSpec>,
    },
    /// Group contiguous leaf devices `start..end` in `branch` into a nested
    /// depth-2 Audio Rack in one atomic step, preserving stable child IDs.
    GroupDevices {
        branch: usize,
        start: usize,
        end: usize,
        rack_id: NonZeroU64,
        host_device_id: NonZeroU64,
        branch_ids: [NonZeroU64; AUDIO_RACK_BRANCH_COUNT],
    },
    /// Replace a nested Audio Rack at `(branch, index)` with the serial
    /// devices from `keep_branch` (`0` or `1`) in one atomic step.
    UngroupNestedRack {
        branch: usize,
        index: usize,
        keep_branch: usize,
    },
    /// Apply an atomic edit to a nested Audio Rack identified by its stable
    /// `nested_rack_id`, refusing stale or missing IDs explicitly.
    EditNestedRackById {
        nested_rack_id: NonZeroU64,
        edit: Box<AudioRackEdit>,
    },
}

/// Control-side atomic one-step Undo/Redo history over `AudioRackSpec`.
///
/// Failed or invalid edits never mutate the current spec or push onto the undo
/// stack. Each valid edit records one reversible step.
#[derive(Clone, Debug)]
pub struct AudioRackHistory {
    current: AudioRackSpec,
    limits: AudioRackLimits,
    undo_stack: Vec<AudioRackSpec>,
    redo_stack: Vec<AudioRackSpec>,
}

impl AudioRackHistory {
    pub fn new(initial: AudioRackSpec, limits: AudioRackLimits) -> Result<Self, AudioRackError> {
        initial.validate(&limits)?;
        Ok(Self {
            current: initial,
            limits,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        })
    }

    pub fn current(&self) -> &AudioRackSpec {
        &self.current
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Validate and apply `edit` atomically; on any error, `current` and both
    /// history stacks remain unchanged.
    pub fn apply(&mut self, edit: AudioRackEdit) -> Result<&AudioRackSpec, AudioRackError> {
        let mut candidate = self.current.clone();
        Self::apply_edit_to_spec(&mut candidate, edit)?;
        candidate.validate(&self.limits)?;
        let previous = std::mem::replace(&mut self.current, candidate);
        self.undo_stack.push(previous);
        self.redo_stack.clear();
        Ok(&self.current)
    }

    fn apply_edit_to_spec(
        candidate: &mut AudioRackSpec,
        edit: AudioRackEdit,
    ) -> Result<(), AudioRackError> {
        match edit {
            AudioRackEdit::ReorderDevice { branch, from, to } => {
                let devices = &mut candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?
                    .devices;
                if from >= devices.len() || to >= devices.len() {
                    return Err(AudioRackError::InvalidEditIndex);
                }
                let item = devices.remove(from);
                devices.insert(to, item);
            }
            AudioRackEdit::RemoveDevice { branch, index } => {
                let devices = &mut candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?
                    .devices;
                if index >= devices.len() {
                    return Err(AudioRackError::InvalidEditIndex);
                }
                devices.remove(index);
            }
            AudioRackEdit::SetDeviceBypassed {
                branch,
                index,
                bypassed,
            } => {
                let device = candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?
                    .devices
                    .get_mut(index)
                    .ok_or(AudioRackError::InvalidEditIndex)?;
                device.bypassed = bypassed;
            }
            AudioRackEdit::SetBranchGain { branch, gain } => {
                let slot = candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?;
                slot.gain = gain;
            }
            AudioRackEdit::SetBranchMuted { branch, muted } => {
                let slot = candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?;
                slot.muted = muted;
            }
            AudioRackEdit::ClearBranch { branch } => {
                let slot = candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?;
                slot.devices.clear();
            }
            AudioRackEdit::SetRackBypassed { bypassed } => {
                candidate.bypassed = bypassed;
            }
            AudioRackEdit::ReplaceBranchDevices { branch, devices } => {
                let slot = candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?;
                slot.devices = devices;
            }
            AudioRackEdit::GroupDevices {
                branch,
                start,
                end,
                rack_id,
                host_device_id,
                branch_ids,
            } => {
                let sample_rate = candidate.sample_rate;
                let devices = &mut candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?
                    .devices;
                if start >= end || end > devices.len() {
                    return Err(AudioRackError::InvalidEditIndex);
                }
                if devices[start..end]
                    .iter()
                    .any(|d| d.state.type_id() == AUDIO_RACK_TYPE_ID)
                {
                    return Err(AudioRackError::RecursionProhibited);
                }
                let grouped: Vec<AudioRackDeviceSpec> = devices.drain(start..end).collect();
                let nested = AudioRackSpec {
                    rack_id,
                    sample_rate,
                    bypassed: false,
                    branches: [
                        AudioRackBranchSpec {
                            branch_id: branch_ids[0],
                            gain: 1.0,
                            muted: false,
                            devices: grouped,
                        },
                        AudioRackBranchSpec {
                            branch_id: branch_ids[1],
                            gain: 1.0,
                            muted: true,
                            devices: Vec::new(),
                        },
                    ],
                };
                let nested_state = nested.encode_state()?;
                devices.insert(
                    start,
                    AudioRackDeviceSpec {
                        device_id: host_device_id,
                        state: nested_state,
                        bypassed: false,
                    },
                );
            }
            AudioRackEdit::UngroupNestedRack {
                branch,
                index,
                keep_branch,
            } => {
                if keep_branch >= AUDIO_RACK_BRANCH_COUNT {
                    return Err(AudioRackError::InvalidEditIndex);
                }
                let devices = &mut candidate
                    .branches
                    .get_mut(branch)
                    .ok_or(AudioRackError::InvalidEditIndex)?
                    .devices;
                let slot = devices.get(index).ok_or(AudioRackError::InvalidEditIndex)?;
                if slot.state.type_id() != AUDIO_RACK_TYPE_ID {
                    return Err(AudioRackError::InvalidEditIndex);
                }
                let nested = AudioRackSpec::decode_state(&slot.state)?;
                let mut kept = nested.branches[keep_branch].devices.clone();
                devices.remove(index);
                for (offset, item) in kept.drain(..).enumerate() {
                    devices.insert(index + offset, item);
                }
            }
            AudioRackEdit::EditNestedRackById {
                nested_rack_id,
                edit,
            } => {
                let mut pending = Some(*edit);
                for branch in &mut candidate.branches {
                    for device in &mut branch.devices {
                        if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                            let mut nested = AudioRackSpec::decode_state(&device.state)?;
                            if nested.rack_id == nested_rack_id {
                                let inner_edit =
                                    pending.take().ok_or(AudioRackError::UnknownTargetId)?;
                                Self::apply_edit_to_spec(&mut nested, inner_edit)?;
                                device.state =
                                    nested.encode_state_version(AUDIO_RACK_STATE_VERSION)?;
                                break;
                            }
                        }
                    }
                    if pending.is_none() {
                        break;
                    }
                }
                if pending.is_some() {
                    return Err(AudioRackError::UnknownTargetId);
                }
            }
        }
        Ok(())
    }

    pub fn undo(&mut self) -> Option<&AudioRackSpec> {
        let previous = self.undo_stack.pop()?;
        let current = std::mem::replace(&mut self.current, previous);
        self.redo_stack.push(current);
        Some(&self.current)
    }

    pub fn redo(&mut self) -> Option<&AudioRackSpec> {
        let next = self.redo_stack.pop()?;
        let current = std::mem::replace(&mut self.current, next);
        self.undo_stack.push(current);
        Some(&self.current)
    }
}

/// Exact-capacity control-allocated stereo delay ring for branch PDC alignment
/// and latency-aligned rack bypass. Callback operations never reallocate or drop.
struct CompensationDelay {
    frames: Vec<StereoFrame>,
    next: usize,
}

impl CompensationDelay {
    fn new_on_control(len: u32) -> Result<Self, AudioRackError> {
        let count = usize::try_from(len).map_err(|_| AudioRackError::CompensationDelayExceeded)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(count)
            .map_err(|_| AudioRackError::AllocationFailed)?;
        frames.resize(count, [0.0; 2]);
        Ok(Self { frames, next: 0 })
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

    fn copy_from(&mut self, source: &Self) {
        if self.frames.len() == source.frames.len() {
            self.frames.copy_from_slice(&source.frames);
            self.next = source.next;
        }
    }

    fn reset(&mut self) {
        self.frames.fill([0.0; 2]);
        self.next = 0;
    }

    fn is_quiet(&self) -> bool {
        self.frames
            .iter()
            .all(|frame| frame[0] == 0.0 && frame[1] == 0.0)
    }
}

enum PreparedBranchStage {
    Chain(NativeRuntimeChain),
    NestedRack {
        device_id: NonZeroU64,
        runtime: Box<AudioRackRuntime>,
    },
}

struct PreparedBranchRuntime {
    branch_id: NonZeroU64,
    gain: f32,
    muted: bool,
    stages: Vec<PreparedBranchStage>,
    branch_latency_frames: u32,
    compensation_delay_frames: u32,
    committed_delay: Option<CompensationDelay>,
    scratch_delay: Option<CompensationDelay>,
    scratch: Vec<StereoFrame>,
}

/// Control-prepared, real-time-safe nested native Audio Rack runtime.
///
/// All branch chains, depth-2 nested racks, compensation delay rings,
/// rack-bypass dry delays, and scratch buffers are allocated on control during
/// `prepare_on_control`. Callback execution (`begin_block`, `process_block`,
/// `commit_block`, `rollback_block`, `reset`, `begin_output_tail`,
/// `inherit_compatible_branches_from`) performs zero heap allocations, locks,
/// I/O, or final drops.
pub struct AudioRackRuntime {
    rack_id: NonZeroU64,
    sample_rate: u32,
    bypassed: bool,
    rack_latency_frames: u32,
    max_block_frames: usize,
    declared_storage_bytes: usize,
    total_work_stages: usize,
    branches: [PreparedBranchRuntime; AUDIO_RACK_BRANCH_COUNT],
    committed_rack_dry: Option<CompensationDelay>,
    scratch_rack_dry: Option<CompensationDelay>,
    input_scratch: Vec<StereoFrame>,
    block_started: bool,
    tail_mode: bool,
}

impl AudioRackRuntime {
    /// Validate `spec` and prepare both serial/nested effect branches, PDC
    /// delay alignment, and preallocated scratch buffers on the control thread.
    pub fn prepare_on_control(
        owner: ProcessorChainOwner,
        spec: &AudioRackSpec,
        registry: &NativeDeviceRegistry<'_>,
        limits: AudioRackLimits,
    ) -> Result<Self, AudioRackError> {
        spec.validate(&limits)?;
        if !owner.is_valid() {
            return Err(AudioRackError::RuntimeBudgetExceeded);
        }
        Self::prepare_on_control_at_depth(owner, spec, registry, limits, 1)
    }

    fn prepare_on_control_at_depth(
        owner: ProcessorChainOwner,
        spec: &AudioRackSpec,
        registry: &NativeDeviceRegistry<'_>,
        limits: AudioRackLimits,
        depth: usize,
    ) -> Result<Self, AudioRackError> {
        if depth == 0 || depth > limits.max_depth {
            return Err(AudioRackError::RecursionProhibited);
        }
        let tuning = crate::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: spec.sample_rate,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };
        struct PreparedBranchParts {
            stages: Vec<PreparedBranchStage>,
            latency_frames: u32,
            storage_bytes: usize,
            work_stages: usize,
        }
        let mut prepared_branches: [Option<PreparedBranchParts>; AUDIO_RACK_BRANCH_COUNT] =
            [None, None];

        for (branch_index, branch) in spec.branches.iter().enumerate() {
            let mut stages = Vec::new();
            let mut pending_leaves = Vec::new();
            let mut branch_latency = 0_u32;
            let mut branch_bytes = 0_usize;
            let mut branch_work_stages = 0_usize;

            let flush_leaves = |pending: &mut Vec<PreparedRuntimeInstance>,
                                stages: &mut Vec<PreparedBranchStage>,
                                branch_bytes: &mut usize,
                                branch_work_stages: &mut usize|
             -> Result<(), AudioRackError> {
                if pending.is_empty() {
                    return Ok(());
                }
                let count = pending.len();
                let instances = std::mem::take(pending);
                let chain = NativeRuntimeChain::new_prepared(instances)?;
                let chain_bytes = chain
                    .storage_view()
                    .entry_capacity_bytes()
                    .ok_or(AudioRackError::RuntimeBudgetExceeded)?
                    .checked_add(chain.declared_pair_bytes())
                    .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
                *branch_bytes = branch_bytes
                    .checked_add(chain_bytes)
                    .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
                *branch_work_stages = branch_work_stages
                    .checked_add(count)
                    .ok_or(AudioRackError::WorkBudgetExceeded)?;
                stages.push(PreparedBranchStage::Chain(chain));
                Ok(())
            };

            for device in &branch.devices {
                if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                    flush_leaves(
                        &mut pending_leaves,
                        &mut stages,
                        &mut branch_bytes,
                        &mut branch_work_stages,
                    )?;
                    let mut nested_spec = AudioRackSpec::decode_state(&device.state)?;
                    if device.bypassed {
                        nested_spec.bypassed = true;
                    }
                    let nested_runtime = Self::prepare_on_control_at_depth(
                        owner,
                        &nested_spec,
                        registry,
                        limits,
                        depth + 1,
                    )?;
                    branch_latency = branch_latency
                        .checked_add(nested_runtime.rack_latency_frames())
                        .ok_or(AudioRackError::CompensationDelayExceeded)?;
                    branch_bytes = branch_bytes
                        .checked_add(nested_runtime.declared_storage_bytes())
                        .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
                    branch_work_stages = branch_work_stages
                        .checked_add(nested_runtime.total_work_stages)
                        .ok_or(AudioRackError::WorkBudgetExceeded)?;
                    stages.push(PreparedBranchStage::NestedRack {
                        device_id: device.device_id,
                        runtime: Box::new(nested_runtime),
                    });
                } else {
                    let identity = ProcessorInstanceIdentity {
                        owner,
                        device_id: device.device_id.get(),
                    };
                    let prepared =
                        registry.prepare_runtime_instance(identity, &device.state, &context)?;
                    let migrated = registry.migrate_state(&device.state)?;
                    let instance = PreparedRuntimeInstance {
                        modulation: None,
                        automation: PreparedParameterAutomation::default(),
                        meter: Arc::new(DeviceMeter::default()),
                        owner,
                        device_id: device.device_id.get(),
                        device_type: migrated.type_id(),
                        matrix: prepared.matrix,
                        bypassed: device.bypassed,
                        sample_rate: spec.sample_rate,
                        configuration: Some(Arc::new(migrated)),
                        definition: prepared.processor,
                    };
                    let latency = instance
                        .algorithmic_latency_frames()
                        .ok_or(AudioRackError::UnknownBranchLatency)?;
                    branch_latency = branch_latency
                        .checked_add(latency)
                        .ok_or(AudioRackError::CompensationDelayExceeded)?;
                    pending_leaves.push(instance);
                }
            }
            flush_leaves(
                &mut pending_leaves,
                &mut stages,
                &mut branch_bytes,
                &mut branch_work_stages,
            )?;
            stages.shrink_to_fit();
            prepared_branches[branch_index] = Some(PreparedBranchParts {
                stages,
                latency_frames: branch_latency,
                storage_bytes: branch_bytes,
                work_stages: branch_work_stages,
            });
        }

        let [
            Some(PreparedBranchParts {
                stages: stages_a,
                latency_frames: lat_a,
                storage_bytes: bytes_a,
                work_stages: work_a,
            }),
            Some(PreparedBranchParts {
                stages: stages_b,
                latency_frames: lat_b,
                storage_bytes: bytes_b,
                work_stages: work_b,
            }),
        ] = prepared_branches
        else {
            return Err(AudioRackError::AllocationFailed);
        };

        let rack_latency_frames = lat_a.max(lat_b);
        let comp_a = rack_latency_frames - lat_a;
        let comp_b = rack_latency_frames - lat_b;
        if comp_a > limits.max_compensation_delay_frames
            || comp_b > limits.max_compensation_delay_frames
            || rack_latency_frames > limits.max_compensation_delay_frames
        {
            return Err(AudioRackError::CompensationDelayExceeded);
        }

        let total_work_stages = work_a
            .checked_add(work_b)
            .and_then(|s| s.checked_add(4))
            .ok_or(AudioRackError::WorkBudgetExceeded)?;
        let work_units = total_work_stages
            .checked_mul(limits.max_block_frames)
            .ok_or(AudioRackError::WorkBudgetExceeded)?;
        if work_units > limits.max_work_units_per_block {
            return Err(AudioRackError::WorkBudgetExceeded);
        }

        let scratch_bytes = limits
            .max_block_frames
            .checked_mul(std::mem::size_of::<StereoFrame>())
            .and_then(|one| one.checked_mul(3))
            .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
        let delay_frames_total = usize::try_from(comp_a)
            .ok()
            .and_then(|a| a.checked_add(usize::try_from(comp_b).ok()?))
            .and_then(|ab| ab.checked_add(usize::try_from(rack_latency_frames).ok()?))
            .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
        let delay_bytes = delay_frames_total
            .checked_mul(2)
            .and_then(|pairs| pairs.checked_mul(std::mem::size_of::<StereoFrame>()))
            .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
        let declared_storage_bytes = bytes_a
            .checked_add(bytes_b)
            .and_then(|sum| sum.checked_add(scratch_bytes))
            .and_then(|sum| sum.checked_add(delay_bytes))
            .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
        if declared_storage_bytes > limits.max_pair_bytes {
            return Err(AudioRackError::RuntimeBudgetExceeded);
        }

        let make_delay_pair = |frames: u32| -> Result<
            (Option<CompensationDelay>, Option<CompensationDelay>),
            AudioRackError,
        > {
            if frames == 0 {
                Ok((None, None))
            } else {
                Ok((
                    Some(CompensationDelay::new_on_control(frames)?),
                    Some(CompensationDelay::new_on_control(frames)?),
                ))
            }
        };
        let make_scratch = |len: usize| -> Result<Vec<StereoFrame>, AudioRackError> {
            let mut buf = Vec::new();
            buf.try_reserve_exact(len)
                .map_err(|_| AudioRackError::AllocationFailed)?;
            buf.resize(len, [0.0; 2]);
            Ok(buf)
        };

        let (committed_delay_a, scratch_delay_a) = make_delay_pair(comp_a)?;
        let (committed_delay_b, scratch_delay_b) = make_delay_pair(comp_b)?;
        let (committed_rack_dry, scratch_rack_dry) = make_delay_pair(rack_latency_frames)?;

        let branch_a = PreparedBranchRuntime {
            branch_id: spec.branches[0].branch_id,
            gain: spec.branches[0].gain,
            muted: spec.branches[0].muted,
            stages: stages_a,
            branch_latency_frames: lat_a,
            compensation_delay_frames: comp_a,
            committed_delay: committed_delay_a,
            scratch_delay: scratch_delay_a,
            scratch: make_scratch(limits.max_block_frames)?,
        };
        let branch_b = PreparedBranchRuntime {
            branch_id: spec.branches[1].branch_id,
            gain: spec.branches[1].gain,
            muted: spec.branches[1].muted,
            stages: stages_b,
            branch_latency_frames: lat_b,
            compensation_delay_frames: comp_b,
            committed_delay: committed_delay_b,
            scratch_delay: scratch_delay_b,
            scratch: make_scratch(limits.max_block_frames)?,
        };

        Ok(Self {
            rack_id: spec.rack_id,
            sample_rate: spec.sample_rate,
            bypassed: spec.bypassed,
            rack_latency_frames,
            max_block_frames: limits.max_block_frames,
            declared_storage_bytes,
            total_work_stages,
            branches: [branch_a, branch_b],
            committed_rack_dry,
            scratch_rack_dry,
            input_scratch: make_scratch(limits.max_block_frames)?,
            block_started: false,
            tail_mode: false,
        })
    }

    pub const fn rack_id(&self) -> NonZeroU64 {
        self.rack_id
    }

    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub const fn is_bypassed(&self) -> bool {
        self.bypassed
    }

    pub const fn rack_latency_frames(&self) -> u32 {
        self.rack_latency_frames
    }

    pub const fn max_block_frames(&self) -> usize {
        self.max_block_frames
    }

    pub const fn declared_storage_bytes(&self) -> usize {
        self.declared_storage_bytes
    }

    pub fn branch_id(&self, branch: usize) -> Option<NonZeroU64> {
        self.branches.get(branch).map(|b| b.branch_id)
    }

    pub fn branch_latency_frames(&self, branch: usize) -> Option<u32> {
        self.branches.get(branch).map(|b| b.branch_latency_frames)
    }

    pub fn branch_compensation_delay_frames(&self, branch: usize) -> Option<u32> {
        self.branches
            .get(branch)
            .map(|b| b.compensation_delay_frames)
    }

    pub fn nested_rack_count(&self) -> usize {
        self.branches
            .iter()
            .flat_map(|b| b.stages.iter())
            .filter(|s| matches!(s, PreparedBranchStage::NestedRack { .. }))
            .count()
    }

    pub fn nested_rack_latency_frames(&self, nested_rack_id: NonZeroU64) -> Option<u32> {
        for branch in &self.branches {
            for stage in &branch.stages {
                if let PreparedBranchStage::NestedRack { runtime, .. } = stage
                    && runtime.rack_id == nested_rack_id
                {
                    return Some(runtime.rack_latency_frames);
                }
            }
        }
        None
    }

    /// Enter residual output-tail draining mode before control-side retirement.
    pub fn begin_output_tail(&mut self) {
        self.tail_mode = true;
        for branch in &mut self.branches {
            for stage in &mut branch.stages {
                match stage {
                    PreparedBranchStage::Chain(chain) => chain.begin_output_tail(),
                    PreparedBranchStage::NestedRack { runtime, .. } => {
                        runtime.begin_output_tail();
                    }
                }
            }
        }
    }

    /// Start a speculative transactional block across both branches, nested
    /// racks, and delay rings.
    pub fn begin_block(&mut self) {
        for branch in &mut self.branches {
            for stage in &mut branch.stages {
                match stage {
                    PreparedBranchStage::Chain(chain) => chain.begin_block(),
                    PreparedBranchStage::NestedRack { runtime, .. } => runtime.begin_block(),
                }
            }
            if let (Some(scratch), Some(committed)) =
                (&mut branch.scratch_delay, &branch.committed_delay)
            {
                scratch.copy_from(committed);
            }
        }
        if let (Some(scratch), Some(committed)) =
            (&mut self.scratch_rack_dry, &self.committed_rack_dry)
        {
            scratch.copy_from(committed);
        }
        self.block_started = true;
    }

    /// Process one audio block in place through both latency-aligned branches
    /// (including nested racks) and sum their weighted outputs. Zero callback
    /// allocations.
    pub fn process_block(&mut self, frames: &mut [StereoFrame]) -> Result<(), AudioRackError> {
        if !self.block_started {
            return Err(AudioRackError::BlockNotStarted);
        }
        let len = frames.len();
        if len == 0 {
            return Ok(());
        }
        if len > self.max_block_frames {
            return Err(AudioRackError::BlockTooLarge);
        }

        self.input_scratch[..len].copy_from_slice(frames);
        if self.bypassed && !self.tail_mode {
            if let Some(dry) = &mut self.scratch_rack_dry {
                dry.process(&mut self.input_scratch[..len]);
            }
            frames.copy_from_slice(&self.input_scratch[..len]);
            return Ok(());
        }

        // Advance latency-aligned dry history so switching into bypass stays phase-aligned.
        if let Some(dry) = &mut self.scratch_rack_dry {
            for &sample in &self.input_scratch[..len] {
                let mut dry_probe = sample;
                dry.process(std::slice::from_mut(&mut dry_probe));
            }
        }

        for branch in &mut self.branches {
            let buf = &mut branch.scratch[..len];
            buf.copy_from_slice(&self.input_scratch[..len]);
            for stage in &mut branch.stages {
                match stage {
                    PreparedBranchStage::Chain(chain) => {
                        if !chain.is_empty() {
                            chain.process_block(buf)?;
                        }
                    }
                    PreparedBranchStage::NestedRack { runtime, .. } => {
                        runtime.process_block(buf)?;
                    }
                }
            }
            if let Some(delay) = &mut branch.scratch_delay {
                delay.process(buf);
            }
        }

        let gain_a = if self.branches[0].muted {
            0.0
        } else {
            self.branches[0].gain
        };
        let gain_b = if self.branches[1].muted {
            0.0
        } else {
            self.branches[1].gain
        };
        let buf_a = &self.branches[0].scratch[..len];
        let buf_b = &self.branches[1].scratch[..len];
        for index in 0..len {
            frames[index] = [
                buf_a[index][0] * gain_a + buf_b[index][0] * gain_b,
                buf_a[index][1] * gain_a + buf_b[index][1] * gain_b,
            ];
        }
        Ok(())
    }

    /// Commit speculative block histories for both branches, nested racks, and
    /// all delay rings.
    pub fn commit_block(&mut self) {
        if self.block_started {
            for branch in &mut self.branches {
                for stage in &mut branch.stages {
                    match stage {
                        PreparedBranchStage::Chain(chain) => chain.commit_block(),
                        PreparedBranchStage::NestedRack { runtime, .. } => runtime.commit_block(),
                    }
                }
                std::mem::swap(&mut branch.committed_delay, &mut branch.scratch_delay);
            }
            std::mem::swap(&mut self.committed_rack_dry, &mut self.scratch_rack_dry);
            self.block_started = false;
        }
    }

    /// Discard speculative block changes and retain committed histories.
    pub fn rollback_block(&mut self) {
        if self.block_started {
            for branch in &mut self.branches {
                for stage in &mut branch.stages {
                    match stage {
                        PreparedBranchStage::Chain(chain) => chain.rollback_block(),
                        PreparedBranchStage::NestedRack { runtime, .. } => runtime.rollback_block(),
                    }
                }
                if let (Some(scratch), Some(committed)) =
                    (&mut branch.scratch_delay, &branch.committed_delay)
                {
                    scratch.copy_from(committed);
                }
            }
            if let (Some(scratch), Some(committed)) =
                (&mut self.scratch_rack_dry, &self.committed_rack_dry)
            {
                scratch.copy_from(committed);
            }
            self.block_started = false;
        }
    }

    /// Hard reset (Stop): clears all branch chains, nested racks, and delay rings.
    pub fn reset(&mut self) {
        for branch in &mut self.branches {
            for stage in &mut branch.stages {
                match stage {
                    PreparedBranchStage::Chain(chain) => chain.reset(),
                    PreparedBranchStage::NestedRack { runtime, .. } => runtime.reset(),
                }
            }
            if let Some(delay) = &mut branch.committed_delay {
                delay.reset();
            }
            if let Some(delay) = &mut branch.scratch_delay {
                delay.reset();
            }
        }
        if let Some(dry) = &mut self.committed_rack_dry {
            dry.reset();
        }
        if let Some(dry) = &mut self.scratch_rack_dry {
            dry.reset();
        }
        self.block_started = false;
        self.tail_mode = false;
    }

    /// True only when no uncommitted block is open, all branch chains and
    /// nested racks are quiescent, and every delay ring holds silence.
    pub fn committed_is_quiescent(&self) -> bool {
        !self.block_started
            && self.branches.iter().all(|branch| {
                branch.stages.iter().all(|stage| match stage {
                    PreparedBranchStage::Chain(chain) => {
                        chain.committed_range_is_quiescent(0..chain.len())
                    }
                    PreparedBranchStage::NestedRack { runtime, .. } => {
                        runtime.committed_is_quiescent()
                    }
                }) && branch
                    .committed_delay
                    .as_ref()
                    .is_none_or(CompensationDelay::is_quiet)
            })
            && self
                .committed_rack_dry
                .as_ref()
                .is_none_or(CompensationDelay::is_quiet)
    }

    /// Speculative quiescence inside an open block.
    pub fn speculative_is_quiescent(&self) -> bool {
        self.block_started
            && self.branches.iter().all(|branch| {
                branch.stages.iter().all(|stage| match stage {
                    PreparedBranchStage::Chain(chain) => {
                        chain.speculative_range_is_quiescent(0..chain.len())
                    }
                    PreparedBranchStage::NestedRack { runtime, .. } => {
                        runtime.speculative_is_quiescent()
                    }
                }) && branch
                    .scratch_delay
                    .as_ref()
                    .is_none_or(CompensationDelay::is_quiet)
            })
            && self
                .scratch_rack_dry
                .as_ref()
                .is_none_or(CompensationDelay::is_quiet)
    }

    /// Read-only compatibility preflight for transferring committed DSP and
    /// compensation-delay history for `branch` from `previous`.
    pub fn can_inherit_branch_from(&self, branch: usize, previous: &Self) -> bool {
        if self.block_started
            || previous.block_started
            || self.rack_id != previous.rack_id
            || self.sample_rate != previous.sample_rate
        {
            return false;
        }
        let (Some(dst), Some(src)) = (self.branches.get(branch), previous.branches.get(branch))
        else {
            return false;
        };
        if dst.branch_id != src.branch_id
            || dst.compensation_delay_frames != src.compensation_delay_frames
            || dst.stages.len() != src.stages.len()
        {
            return false;
        }
        dst.stages
            .iter()
            .zip(src.stages.iter())
            .all(|(d, s)| match (d, s) {
                (PreparedBranchStage::Chain(dc), PreparedBranchStage::Chain(sc)) => {
                    dc.len() == sc.len()
                        && dc.configuration_matches_range(sc, 0..dc.len())
                        && dc.can_inherit_from(sc)
                }
                (
                    PreparedBranchStage::NestedRack {
                        device_id: d_id,
                        runtime: dr,
                    },
                    PreparedBranchStage::NestedRack {
                        device_id: s_id,
                        runtime: sr,
                    },
                ) => {
                    d_id == s_id
                        && dr.rack_id == sr.rack_id
                        && dr.sample_rate == sr.sample_rate
                        && dr.rack_latency_frames == sr.rack_latency_frames
                        && (0..AUDIO_RACK_BRANCH_COUNT)
                            .all(|inner| dr.can_inherit_branch_from(inner, sr))
                }
                _ => false,
            })
    }

    /// Transfer committed histories for each compatible branch (and unchanged
    /// nested sub-branches) without allocating.
    pub fn inherit_compatible_branches_from(
        &mut self,
        previous: &Self,
    ) -> Result<[bool; AUDIO_RACK_BRANCH_COUNT], AudioRackError> {
        if self.block_started || previous.block_started {
            return Err(AudioRackError::BlockNotStarted);
        }
        let mut inherited = [false; AUDIO_RACK_BRANCH_COUNT];
        for (branch, slot) in inherited.iter_mut().enumerate() {
            if self.can_inherit_branch_from(branch, previous) {
                let dst = &mut self.branches[branch];
                let src = &previous.branches[branch];
                for (d_stage, s_stage) in dst.stages.iter_mut().zip(src.stages.iter()) {
                    match (d_stage, s_stage) {
                        (PreparedBranchStage::Chain(dc), PreparedBranchStage::Chain(sc)) => {
                            dc.inherit_from(sc);
                        }
                        (
                            PreparedBranchStage::NestedRack { runtime: dr, .. },
                            PreparedBranchStage::NestedRack { runtime: sr, .. },
                        ) => {
                            let _ = dr.inherit_compatible_branches_from(sr)?;
                        }
                        _ => {}
                    }
                }
                if let (Some(dst_committed), Some(dst_scratch), Some(src_committed)) = (
                    &mut dst.committed_delay,
                    &mut dst.scratch_delay,
                    &src.committed_delay,
                ) {
                    dst_committed.copy_from(src_committed);
                    dst_scratch.copy_from(src_committed);
                }
                *slot = true;
            } else if self.rack_id == previous.rack_id
                && self.sample_rate == previous.sample_rate
                && self.branches[branch].branch_id == previous.branches[branch].branch_id
                && self.branches[branch].compensation_delay_frames
                    == previous.branches[branch].compensation_delay_frames
                && self.branches[branch].stages.len() == previous.branches[branch].stages.len()
            {
                // Partial nested inheritance: preserve unchanged sibling inner
                // branches inside a matching nested Audio Rack.
                let dst = &mut self.branches[branch];
                let src = &previous.branches[branch];
                for (d_stage, s_stage) in dst.stages.iter_mut().zip(src.stages.iter()) {
                    if let (
                        PreparedBranchStage::NestedRack {
                            device_id: d_id,
                            runtime: dr,
                        },
                        PreparedBranchStage::NestedRack {
                            device_id: s_id,
                            runtime: sr,
                        },
                    ) = (d_stage, s_stage)
                        && d_id == s_id
                        && dr.rack_id == sr.rack_id
                    {
                        let _ = dr.inherit_compatible_branches_from(sr)?;
                    }
                }
            }
        }
        if self.rack_id == previous.rack_id
            && self.sample_rate == previous.sample_rate
            && self.rack_latency_frames == previous.rack_latency_frames
            && let (Some(dst_committed), Some(dst_scratch), Some(src_committed)) = (
                &mut self.committed_rack_dry,
                &mut self.scratch_rack_dry,
                &previous.committed_rack_dry,
            )
        {
            dst_committed.copy_from(src_committed);
            dst_scratch.copy_from(src_committed);
        }
        Ok(inherited)
    }
}
