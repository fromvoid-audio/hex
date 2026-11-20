//! Seven-frame registered processor for output-correlated MIDI capture tests.
use super::*;
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use std::any::Any;

const TYPE: DeviceTypeId = DeviceTypeId::from_static("test.compensation.delay");
pub(crate) struct Definition;
impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        state(0)
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != TYPE || state.state_version() != 1 || state.payload().len() != 4 {
            return Err(DeviceContractError::MalformedState);
        }
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
    fn patch_parameter(
        &self,
        _: &NativeDeviceState,
        _: ParameterId,
        _: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Err(DeviceContractError::UnknownParameter)
    }
}
pub(crate) fn state(delay: u32) -> Result<NativeDeviceState, DeviceContractError> {
    NativeDeviceState::new(TYPE, 1, delay.to_le_bytes().to_vec())
}
impl NativeDeviceDefinition for Definition {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        self.migrate_state(state)?;
        let delay = u32::from_le_bytes(
            state
                .payload()
                .try_into()
                .map_err(|_| DeviceContractError::MalformedState)?,
        );
        Ok(Some(Box::new(Prepared {
            delay: (delay != u32::MAX).then_some(delay),
        })))
    }
}
struct Prepared {
    delay: Option<u32>,
}
impl PreparedNativeProcessor for Prepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.delay
    }
    fn declared_runtime_bytes(&self) -> usize {
        size_of::<Runtime>() + self.delay.unwrap_or(0) as usize * size_of::<Frame>()
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(Runtime {
            ring: vec![[0.0; 2]; self.delay.unwrap_or(0) as usize],
            cursor: 0,
        })
    }
}
struct Runtime {
    ring: Vec<Frame>,
    cursor: usize,
}
impl NativeProcessorRuntime for Runtime {
    fn process_block(
        &mut self,
        frames: &mut [Frame],
        _resources: &dyn hex_lattice::DeviceResources,
    ) {
        for frame in frames {
            if !self.ring.is_empty() {
                std::mem::swap(frame, &mut self.ring[self.cursor]);
                self.cursor = (self.cursor + 1) % self.ring.len();
            }
        }
    }
    fn reset(&mut self) {
        self.ring.fill([0.0; 2]);
        self.cursor = 0;
    }
    fn is_quiescent(&self) -> bool {
        self.ring.iter().all(|frame| *frame == [0.0; 2])
    }
    fn can_copy_history_from(&self, old: &dyn NativeProcessorRuntime) -> bool {
        old.as_any()
            .downcast_ref::<Self>()
            .is_some_and(|old| old.ring.len() == self.ring.len())
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            self.ring.copy_from_slice(&old.ring);
            self.cursor = old.cursor;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
