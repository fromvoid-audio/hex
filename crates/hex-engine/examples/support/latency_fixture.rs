//! Original fixed seven-frame delay used by export and native callback tests.
use hex_lattice::{
    NativeDeviceDefinition, NativeProcessorRuntime, OutputTail, PreparedNativeProcessor,
    StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use std::any::Any;
const TYPE: DeviceTypeId = DeviceTypeId::from_static("test.export.fixed.delay");
pub struct Definition;
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
        NativeDeviceState::new(TYPE, 1, vec![])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != TYPE || state.state_version() != 1 || !state.payload().is_empty() {
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
        Ok(Some(Box::new(Prepared)))
    }
}
struct Prepared;
impl PreparedNativeProcessor for Prepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(7)
    }
    fn output_tail(&self) -> OutputTail {
        OutputTail::Finite { max_tail_frames: 0 }
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(Runtime {
            ring: [[0.0; 2]; 7],
            cursor: 0,
        })
    }
}
struct Runtime {
    ring: [[f32; 2]; 7],
    cursor: usize,
}
impl NativeProcessorRuntime for Runtime {
    fn process_block(&mut self, frames: &mut [[f32; 2]]) {
        for frame in frames {
            std::mem::swap(frame, &mut self.ring[self.cursor]);
            self.cursor = (self.cursor + 1) % 7;
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
        old.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, old: &dyn NativeProcessorRuntime) {
        if let Some(old) = old.as_any().downcast_ref::<Self>() {
            self.ring = old.ring;
            self.cursor = old.cursor;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
