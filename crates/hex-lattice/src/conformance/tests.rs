use super::*;
use crate::{NativeProcessorRuntime, PreparedNativeProcessor, StereoMatrix};
use hex_lattice_contract::{ParameterDescriptor, ParameterId, ParameterPresentation};
use std::any::Any;
const ID: DeviceTypeId = DeviceTypeId::from_static("fixture.conformance");
const PARAMETER: ParameterId = ParameterId::from_static(1);
const DESCRIPTORS: &[ParameterDescriptor] = &[ParameterDescriptor {
    id: PARAMETER,
    label: "gain",
    kind: ParameterKind::Float {
        min: 0.0,
        max: 1.0,
        unit: None,
    },
    default: ParameterValue::Float(0.5),
    presentation: ParameterPresentation::Linear,
}];
#[derive(Clone, Copy)]
enum Fault {
    None,
    NonfiniteRead,
    InvalidEdit,
    BlockDependent,
}
struct Fixture(Fault);
impl NativeDeviceMetadata for Fixture {
    fn type_id(&self) -> DeviceTypeId {
        ID
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(ID, 1, 0.5_f32.to_le_bytes().to_vec())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != ID {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != 1 {
            return Err(DeviceContractError::UnsupportedStateVersion);
        }
        let bytes: [u8; 4] = state
            .payload()
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        DESCRIPTORS[0].validate(ParameterValue::Float(f32::from_le_bytes(bytes)))?;
        Ok(state.clone())
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        self.migrate_state(state)?;
        if id != PARAMETER {
            return Err(DeviceContractError::UnknownParameter);
        }
        let bytes: [u8; 4] = state
            .payload()
            .try_into()
            .map_err(|_| DeviceContractError::MalformedState)?;
        Ok(ParameterValue::Float(
            if matches!(self.0, Fault::NonfiniteRead) {
                f32::NAN
            } else {
                f32::from_le_bytes(bytes)
            },
        ))
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        self.migrate_state(state)?;
        if id != PARAMETER {
            return Err(DeviceContractError::UnknownParameter);
        }
        if matches!(self.0, Fault::InvalidEdit) {
            return Ok(state.clone());
        }
        DESCRIPTORS[0].validate(value)?;
        match value {
            ParameterValue::Float(value) => {
                NativeDeviceState::new(ID, 1, value.to_le_bytes().to_vec())
            }
            _ => Err(DeviceContractError::InvalidParameterValue),
        }
    }
}
impl NativeDeviceDefinition for Fixture {
    fn prepare_matrix(
        &self,
        _state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_processor(
        &self,
        _state: &NativeDeviceState,
        _context: &crate::DevicePreparationContext<'_>,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        Ok(Some(Box::new(Prepared(matches!(
            self.0,
            Fault::BlockDependent
        )))))
    }
}
struct Prepared(bool);
impl PreparedNativeProcessor for Prepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<Runtime>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(Runtime {
            broken: self.0,
            calls: 0,
        })
    }
}
struct Runtime {
    broken: bool,
    calls: u32,
}
impl NativeProcessorRuntime for Runtime {
    fn process_block(
        &mut self,
        frames: &mut [StereoFrame],
        _resources: &dyn crate::DeviceResources,
    ) {
        self.calls += 1;
        if self.broken {
            for frame in frames {
                frame[0] *= self.calls as f32;
            }
        }
    }
    fn reset(&mut self) {
        self.calls = 0;
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|p| p.broken == self.broken)
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.calls = previous.calls;
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
#[test]
fn authoring_checks_detect_broken_values_edits_and_block_laws()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture(Fault::None);
    let state = fixture.default_state()?;
    assert_eq!(check_metadata(&fixture, &state)?, 1);
    check_effect(&fixture, &state, 48_000)?;
    assert_eq!(
        check_metadata(&Fixture(Fault::NonfiniteRead), &state)
            .expect_err("nonfinite read must fail")
            .stage,
        "read value domain"
    );
    assert_eq!(
        check_metadata(&Fixture(Fault::InvalidEdit), &state)
            .expect_err("invalid edit must fail")
            .stage,
        "invalid parameter accepted"
    );
    assert_eq!(
        check_effect(&Fixture(Fault::BlockDependent), &state, 48_000)
            .expect_err("block-dependent law must fail")
            .stage,
        "finite partition agreement"
    );
    Ok(())
}
