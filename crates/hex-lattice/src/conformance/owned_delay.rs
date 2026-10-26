//! Original registered test provider for owned-effect transactions. Not shipped
//! in the device browser. This fixture retains a narrow live-copy bound;
//! production providers declare and validate their own owned-history footprint.
use crate::{
    NativeDeviceDefinition, OutputTail, OwnedProcessorRuntime, PreparedOwnedProcessorRuntime,
    PreparedProcessorFactory, StereoFrame, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterKind, ParameterPresentation, ParameterValue,
};
use std::{any::Any, sync::Arc};

pub const TYPE_ID: DeviceTypeId = DeviceTypeId::from_static("fixture.owned-delay");
pub const FRAMES: ParameterId = ParameterId::from_static(1);
const MAX_FRAMES: u32 = 4_000_000;
const DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
    id: FRAMES,
    label: "delay",
    kind: ParameterKind::Float {
        min: 1.0,
        max: MAX_FRAMES as f32,
        unit: Some("frames"),
    },
    default: ParameterValue::Float(17.0),
    presentation: ParameterPresentation::Linear,
}];

pub struct OwnedDelayDefinition;
pub static DEFINITION: OwnedDelayDefinition = OwnedDelayDefinition;
fn decode(state: &NativeDeviceState) -> Result<usize, DeviceContractError> {
    if state.type_id() != TYPE_ID {
        return Err(DeviceContractError::WrongDeviceType);
    }
    if state.state_version() != 1 {
        return Err(DeviceContractError::UnsupportedStateVersion);
    }
    let bytes: [u8; 4] = state
        .payload()
        .try_into()
        .map_err(|_| DeviceContractError::MalformedState)?;
    let frames = u32::from_le_bytes(bytes);
    if !(1..=MAX_FRAMES).contains(&frames) {
        return Err(DeviceContractError::InvalidParameterValue);
    }
    usize::try_from(frames).map_err(|_| DeviceContractError::InvalidParameterValue)
}
pub fn state(frames: u32) -> Result<NativeDeviceState, DeviceContractError> {
    let value = NativeDeviceState::new(TYPE_ID, 1, frames.to_le_bytes().to_vec())?;
    decode(&value)?;
    Ok(value)
}
impl NativeDeviceMetadata for OwnedDelayDefinition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE_ID
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        state(17)
    }
    fn migrate_state(
        &self,
        value: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        decode(value)?;
        Ok(value.clone())
    }
    fn read_parameter(
        &self,
        value: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        if id != FRAMES {
            return Err(DeviceContractError::UnknownParameter);
        }
        Ok(ParameterValue::Float(decode(value)? as f32))
    }
    fn patch_parameter(
        &self,
        original: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        decode(original)?;
        if id != FRAMES {
            return Err(DeviceContractError::UnknownParameter);
        }
        DESCRIPTORS[0].validate(value)?;
        let ParameterValue::Float(value) = value else {
            return Err(DeviceContractError::InvalidParameterValue);
        };
        if value.fract() != 0.0 {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        state(value as u32)
    }
}
impl NativeDeviceDefinition for OwnedDelayDefinition {
    fn prepare_matrix(
        &self,
        value: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(value)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_effect_matrix(
        &self,
        value: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        decode(value)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        value: &NativeDeviceState,
        _context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn crate::PreparedNativeProcessor>>, DeviceContractError> {
        decode(value)?;
        Err(DeviceContractError::UnsupportedProcessingClass)
    }
    fn prepare_runtime_factory(
        &self,
        value: &NativeDeviceState,
        context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        let rate = context.sample_rate;
        if !(8_000..=192_000).contains(&rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        let frames = decode(value)?;
        let bytes = pair_bytes(frames, frames).ok_or(DeviceContractError::RuntimeBudgetExceeded)?;
        Ok(Some(PreparedProcessorFactory::Owned(Arc::new(Factory {
            frames,
            bytes,
        }))))
    }
}
struct Factory {
    frames: usize,
    bytes: usize,
}
impl PreparedOwnedProcessorRuntime for Factory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        self.bytes
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        Ok(Box::new(DelayRuntime::new(self.frames)?))
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite {
            max_tail_frames: self.frames as u64,
        }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    } // Intentional musical delay.
}
fn pair_bytes(a: usize, b: usize) -> Option<usize> {
    std::mem::size_of::<DelayRuntime>().checked_add(
        a.checked_add(b)?
            .checked_mul(std::mem::size_of::<StereoFrame>())?,
    )
}
fn ring(frames: usize) -> Result<Vec<StereoFrame>, DeviceContractError> {
    let mut value = Vec::new();
    value
        .try_reserve_exact(frames)
        .map_err(|_| DeviceContractError::RuntimeAllocation)?;
    value.resize(frames, [0.0; 2]);
    Ok(value)
}
/// Two private preallocated banks; terminal synchronization touches only the
/// written circular interval, even after several wraps or long transactions.
pub struct DelayRuntime {
    committed: Vec<StereoFrame>,
    scratch: Vec<StereoFrame>,
    committed_next: usize,
    scratch_next: usize,
    dirty_start: usize,
    dirty_len: usize,
    committed_nonzero: usize,
    scratch_nonzero: usize,
    open: bool,
}
impl DelayRuntime {
    fn new(frames: usize) -> Result<Self, DeviceContractError> {
        if frames == 0 || frames > MAX_FRAMES as usize {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Self {
            committed: ring(frames)?,
            scratch: ring(frames)?,
            committed_next: 0,
            scratch_next: 0,
            dirty_start: 0,
            dirty_len: 0,
            committed_nonzero: 0,
            scratch_nonzero: 0,
            open: false,
        })
    }
    fn copy_dirty(
        source: &[StereoFrame],
        destination: &mut [StereoFrame],
        start: usize,
        length: usize,
    ) {
        let first = length.min(source.len() - start);
        destination[start..start + first].copy_from_slice(&source[start..start + first]);
        let second = length - first;
        destination[..second].copy_from_slice(&source[..second]);
    }
    pub fn transaction_is_open(&self) -> bool {
        self.open
    }
}
impl OwnedProcessorRuntime for DelayRuntime {
    fn fixed_storage_pair_bytes(&self) -> Option<usize> {
        pair_bytes(self.committed.capacity(), self.scratch.capacity())
    }
    fn begin_block(&mut self) {
        if self.open {
            self.rollback_block();
        }
        self.dirty_start = self.scratch_next;
        self.dirty_len = 0;
        self.open = true;
    }
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        if !self.open {
            self.begin_block();
        }
        let capacity = self.scratch.len();
        for frame in frames {
            let old = self.scratch[self.scratch_next];
            self.scratch_nonzero -= usize::from(old != [0.0; 2]);
            self.scratch_nonzero += usize::from(*frame != [0.0; 2]);
            self.scratch[self.scratch_next] = *frame;
            *frame = old;
            self.scratch_next += 1;
            if self.scratch_next == capacity {
                self.scratch_next = 0;
            }
            self.dirty_len = self.dirty_len.saturating_add(1).min(capacity);
        }
    }
    fn commit_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_dirty(
            &self.scratch,
            &mut self.committed,
            self.dirty_start,
            self.dirty_len,
        );
        self.committed_next = self.scratch_next;
        self.committed_nonzero = self.scratch_nonzero;
        self.dirty_len = 0;
        self.open = false;
    }
    fn rollback_block(&mut self) {
        if !self.open {
            return;
        }
        Self::copy_dirty(
            &self.committed,
            &mut self.scratch,
            self.dirty_start,
            self.dirty_len,
        );
        self.scratch_next = self.committed_next;
        self.scratch_nonzero = self.committed_nonzero;
        self.dirty_len = 0;
        self.open = false;
    }
    fn reset(&mut self) {
        self.committed.fill([0.0; 2]);
        self.scratch.fill([0.0; 2]);
        self.committed_next = 0;
        self.scratch_next = 0;
        self.committed_nonzero = 0;
        self.scratch_nonzero = 0;
        self.dirty_start = 0;
        self.dirty_len = 0;
        self.open = false;
    }
    fn committed_is_quiescent(&self) -> bool {
        self.committed_nonzero == 0
    }
    fn speculative_is_quiescent(&self) -> bool {
        self.scratch_nonzero == 0
    }
    fn can_inherit_from(&self, previous: &dyn OwnedProcessorRuntime) -> bool {
        previous.as_any().downcast_ref::<Self>().is_some_and(|old| {
            !self.open
                && !old.open
                && self.scratch.len() == old.scratch.len()
                && self
                    .fixed_storage_pair_bytes()
                    .is_some_and(|bytes| bytes <= crate::MAX_PROCESSOR_RUNTIME_BYTES * 2)
        })
    }
    fn inherit_from(&mut self, previous: &dyn OwnedProcessorRuntime) {
        if !self.can_inherit_from(previous) {
            return;
        }
        if let Some(old) = previous.as_any().downcast_ref::<Self>() {
            self.committed.copy_from_slice(&old.committed);
            self.scratch.copy_from_slice(&old.committed);
            self.committed_next = old.committed_next;
            self.scratch_next = old.committed_next;
            self.committed_nonzero = old.committed_nonzero;
            self.scratch_nonzero = old.committed_nonzero;
            self.dirty_len = 0;
            self.dirty_start = self.scratch_next;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
