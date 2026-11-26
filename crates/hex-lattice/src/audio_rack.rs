//! Bounded two-chain native Audio Rack with latency-compensated parallel sum.
//!
//! An Audio Rack owns two ordered serial effect branches (`AUDIO_RACK_BRANCH_COUNT == 2`)
//! prepared through the existing `NativeDeviceRegistry` (never a parallel registry)
//! and summed in exact phase alignment by delaying the lower-latency branch by
//! `rack_latency_frames - branch_latency_frames`.
//!
//! Recursion is explicitly prohibited (`AUDIO_RACK_MAX_DEPTH == 1`): any child
//! device declaring `AUDIO_RACK_TYPE_ID` is rejected on control before touching
//! active state. All branch runtimes, compensation delay rings, rack-bypass dry
//! delay, and block scratch buffers are preallocated on control so callback
//! rendering performs zero allocations, locks, I/O, or final drops.

#[cfg(test)]
mod tests;

use crate::{
    DeviceMeter, DevicePreparationContext, EMPTY_DEVICE_RESOURCES, MAX_MAIN_PASS_DELAY_FRAMES,
    MAX_PROCESSOR_BANK_RUNTIME_BYTES, NativeDeviceRegistry, NativeRuntimeChain,
    PreparedParameterAutomation, PreparedRuntimeInstance, ProcessorChainOwner,
    ProcessorInstanceIdentity, StereoFrame,
};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use std::{fmt, num::NonZeroU64, sync::Arc};

/// Reserved native device type identifier for an Audio Rack. Rejected inside
/// any branch to enforce the depth-1 recursion prohibition.
pub const AUDIO_RACK_TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("hex.rack.audio");
/// Fixed parallel branch count for this bounded Audio Rack contract.
pub const AUDIO_RACK_BRANCH_COUNT: usize = 2;
/// Explicit nesting ceiling: a rack contains flat serial effect branches only.
pub const AUDIO_RACK_MAX_DEPTH: usize = 1;
/// Maximum serial effect devices admitted in one branch.
pub const AUDIO_RACK_MAX_DEVICES_PER_BRANCH: usize = 16;
/// Maximum per-branch latency compensation delay in frames.
pub const AUDIO_RACK_MAX_COMPENSATION_DELAY_FRAMES: u32 = MAX_MAIN_PASS_DELAY_FRAMES;
/// Maximum audio block length admitted by preallocated rack scratch storage.
pub const AUDIO_RACK_MAX_BLOCK_FRAMES: usize = 4_096;
/// Default combined runtime + scratch byte ceiling for one Audio Rack.
pub const AUDIO_RACK_MAX_PAIR_BYTES: usize = MAX_PROCESSOR_BANK_RUNTIME_BYTES;
/// Maximum linear branch gain (`+12 dB`).
pub const AUDIO_RACK_MAX_BRANCH_GAIN: f32 = 4.0;
/// Default work ceiling per block (`(devices + 2) * frames`).
pub const AUDIO_RACK_MAX_WORK_UNITS_PER_BLOCK: usize =
    (AUDIO_RACK_MAX_DEVICES_PER_BRANCH * AUDIO_RACK_BRANCH_COUNT + 4) * AUDIO_RACK_MAX_BLOCK_FRAMES;

/// Caller-supplied finite ceilings validated on control before preparing a rack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioRackLimits {
    pub max_pair_bytes: usize,
    pub max_devices_per_branch: usize,
    pub max_compensation_delay_frames: u32,
    pub max_block_frames: usize,
    pub max_work_units_per_block: usize,
}

impl Default for AudioRackLimits {
    fn default() -> Self {
        Self {
            max_pair_bytes: AUDIO_RACK_MAX_PAIR_BYTES,
            max_devices_per_branch: AUDIO_RACK_MAX_DEVICES_PER_BRANCH,
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
    DuplicateBranchId,
    DuplicateDeviceId,
    RecursionProhibited,
    BranchDeviceCapacityExceeded,
    UnknownBranchLatency,
    CompensationDelayExceeded,
    RuntimeBudgetExceeded,
    WorkBudgetExceeded,
    AllocationFailed,
    BlockNotStarted,
    BlockTooLarge,
    InvalidEditIndex,
    DeviceContract(DeviceContractError),
}

impl AudioRackError {
    /// Allocation-free lowercase diagnostic message.
    pub const fn diagnostic_message(self) -> &'static str {
        match self {
            Self::InvalidSampleRate => "audio rack sample rate is out of range",
            Self::InvalidGain => "audio rack branch gain must be finite and within range",
            Self::DuplicateBranchId => "audio rack branches must have distinct identities",
            Self::DuplicateDeviceId => "audio rack child devices must have distinct identities",
            Self::RecursionProhibited => "nested audio racks are prohibited at depth 1",
            Self::BranchDeviceCapacityExceeded => "audio rack branch device limit is exceeded",
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
    /// Validate rack metadata, unique child/branch identities, finite gains,
    /// device caps, and depth-1 recursion prohibition before any preparation.
    pub fn validate(&self, limits: &AudioRackLimits) -> Result<(), AudioRackError> {
        if !(8_000..=192_000).contains(&self.sample_rate) {
            return Err(AudioRackError::InvalidSampleRate);
        }
        if limits.max_block_frames == 0
            || limits.max_block_frames > AUDIO_RACK_MAX_BLOCK_FRAMES
            || limits.max_devices_per_branch == 0
            || limits.max_devices_per_branch > AUDIO_RACK_MAX_DEVICES_PER_BRANCH
            || limits.max_compensation_delay_frames > AUDIO_RACK_MAX_COMPENSATION_DELAY_FRAMES
            || limits.max_pair_bytes == 0
            || limits.max_work_units_per_block == 0
        {
            return Err(AudioRackError::RuntimeBudgetExceeded);
        }
        if self.branches[0].branch_id == self.branches[1].branch_id {
            return Err(AudioRackError::DuplicateBranchId);
        }
        let mut seen_ids = [0_u64; AUDIO_RACK_MAX_DEVICES_PER_BRANCH * AUDIO_RACK_BRANCH_COUNT];
        let mut seen_len = 0_usize;
        for branch in &self.branches {
            if !branch.gain.is_finite()
                || !(0.0..=AUDIO_RACK_MAX_BRANCH_GAIN).contains(&branch.gain)
            {
                return Err(AudioRackError::InvalidGain);
            }
            if branch.devices.len() > limits.max_devices_per_branch {
                return Err(AudioRackError::BranchDeviceCapacityExceeded);
            }
            for device in &branch.devices {
                if device.state.type_id() == AUDIO_RACK_TYPE_ID {
                    return Err(AudioRackError::RecursionProhibited);
                }
                let raw_id = device.device_id.get();
                if seen_ids[..seen_len].contains(&raw_id) {
                    return Err(AudioRackError::DuplicateDeviceId);
                }
                seen_ids[seen_len] = raw_id;
                seen_len += 1;
            }
        }
        Ok(())
    }
}

/// Atomic one-step edit commands over an `AudioRackSpec`.
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
        }
        candidate.validate(&self.limits)?;
        let previous = std::mem::replace(&mut self.current, candidate);
        self.undo_stack.push(previous);
        self.redo_stack.clear();
        Ok(&self.current)
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

struct PreparedBranchRuntime {
    branch_id: NonZeroU64,
    gain: f32,
    muted: bool,
    chain: NativeRuntimeChain,
    branch_latency_frames: u32,
    compensation_delay_frames: u32,
    committed_delay: Option<CompensationDelay>,
    scratch_delay: Option<CompensationDelay>,
    scratch: Vec<StereoFrame>,
}

/// Control-prepared, real-time-safe two-chain native Audio Rack runtime.
///
/// All branch chains, compensation delay rings, rack-bypass dry delay, and
/// scratch buffers are allocated on control during `prepare_on_control`.
/// Callback execution (`begin_block`, `process_block`, `commit_block`,
/// `rollback_block`, `reset`, `begin_output_tail`, `inherit_compatible_branches_from`)
/// performs zero heap allocations, locks, I/O, or final drops.
pub struct AudioRackRuntime {
    rack_id: NonZeroU64,
    sample_rate: u32,
    bypassed: bool,
    rack_latency_frames: u32,
    max_block_frames: usize,
    declared_storage_bytes: usize,
    branches: [PreparedBranchRuntime; AUDIO_RACK_BRANCH_COUNT],
    committed_rack_dry: Option<CompensationDelay>,
    scratch_rack_dry: Option<CompensationDelay>,
    input_scratch: Vec<StereoFrame>,
    block_started: bool,
    tail_mode: bool,
}

impl AudioRackRuntime {
    /// Validate `spec` and prepare both serial effect branches, PDC delay
    /// alignment, and preallocated scratch buffers on the control thread.
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

        let tuning = crate::TuningTable::default();
        let context = DevicePreparationContext {
            sample_rate: spec.sample_rate,
            tuning: &tuning,
            resources: &EMPTY_DEVICE_RESOURCES,
        };
        let mut prepared_chains: [Option<(NativeRuntimeChain, u32, usize)>;
            AUDIO_RACK_BRANCH_COUNT] = [None, None];
        let mut total_devices = 0_usize;

        for (branch_index, branch) in spec.branches.iter().enumerate() {
            let mut instances = Vec::new();
            instances
                .try_reserve_exact(branch.devices.len())
                .map_err(|_| AudioRackError::AllocationFailed)?;
            let mut branch_latency = 0_u32;

            for device in &branch.devices {
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
                instances.push(instance);
            }

            total_devices = total_devices
                .checked_add(branch.devices.len())
                .ok_or(AudioRackError::WorkBudgetExceeded)?;
            let chain = NativeRuntimeChain::new_prepared(instances)?;
            let chain_bytes = chain
                .storage_view()
                .entry_capacity_bytes()
                .ok_or(AudioRackError::RuntimeBudgetExceeded)?
                .checked_add(chain.declared_pair_bytes())
                .ok_or(AudioRackError::RuntimeBudgetExceeded)?;
            prepared_chains[branch_index] = Some((chain, branch_latency, chain_bytes));
        }

        let [
            Some((chain_a, lat_a, bytes_a)),
            Some((chain_b, lat_b, bytes_b)),
        ] = prepared_chains
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

        let work_units = total_devices
            .checked_add(4)
            .and_then(|stages| stages.checked_mul(limits.max_block_frames))
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
            chain: chain_a,
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
            chain: chain_b,
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

    /// Enter residual output-tail draining mode before control-side retirement.
    pub fn begin_output_tail(&mut self) {
        self.tail_mode = true;
        for branch in &mut self.branches {
            branch.chain.begin_output_tail();
        }
    }

    /// Start a speculative transactional block across both branches and delay rings.
    pub fn begin_block(&mut self) {
        for branch in &mut self.branches {
            branch.chain.begin_block();
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
    /// and sum their weighted outputs. Zero callback allocations.
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
            if !branch.chain.is_empty() {
                branch.chain.process_block(buf)?;
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

    /// Commit speculative block histories for both branches and all delay rings.
    pub fn commit_block(&mut self) {
        if self.block_started {
            for branch in &mut self.branches {
                branch.chain.commit_block();
                std::mem::swap(&mut branch.committed_delay, &mut branch.scratch_delay);
            }
            std::mem::swap(&mut self.committed_rack_dry, &mut self.scratch_rack_dry);
            self.block_started = false;
        }
    }

    /// Discard speculative block changes and retain committed branch/delay histories.
    pub fn rollback_block(&mut self) {
        if self.block_started {
            for branch in &mut self.branches {
                branch.chain.rollback_block();
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

    /// Hard reset (Stop): clears both branch chains and all compensation/dry rings.
    pub fn reset(&mut self) {
        for branch in &mut self.branches {
            branch.chain.reset();
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

    /// True only when no uncommitted block is open, both branch chains are
    /// quiescent, and every delay ring holds silence.
    pub fn committed_is_quiescent(&self) -> bool {
        !self.block_started
            && self.branches.iter().all(|branch| {
                branch
                    .chain
                    .committed_range_is_quiescent(0..branch.chain.len())
                    && branch
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
                branch
                    .chain
                    .speculative_range_is_quiescent(0..branch.chain.len())
                    && branch
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
        dst.branch_id == src.branch_id
            && dst.compensation_delay_frames == src.compensation_delay_frames
            && dst.chain.len() == src.chain.len()
            && dst
                .chain
                .configuration_matches_range(&src.chain, 0..dst.chain.len())
            && dst.chain.can_inherit_from(&src.chain)
    }

    /// Transfer committed histories for each compatible branch without
    /// allocating. Unchanged sibling branches keep playing seamlessly while
    /// edited branches start fresh (or let the predecessor drain its tail).
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
                dst.chain.inherit_from(&src.chain);
                if let (Some(dst_committed), Some(dst_scratch), Some(src_committed)) = (
                    &mut dst.committed_delay,
                    &mut dst.scratch_delay,
                    &src.committed_delay,
                ) {
                    dst_committed.copy_from(src_committed);
                    dst_scratch.copy_from(src_committed);
                }
                *slot = true;
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
