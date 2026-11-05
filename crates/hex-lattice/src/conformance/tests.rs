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

struct AuxiliaryFixture;
impl NativeDeviceMetadata for AuxiliaryFixture {
    fn type_id(&self) -> DeviceTypeId {
        ID
    }
    fn auxiliary_input(&self) -> hex_lattice_contract::AuxiliaryInput {
        hex_lattice_contract::AuxiliaryInput::Stereo
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        DESCRIPTORS
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        Fixture(Fault::None).default_state()
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Fixture(Fault::None).migrate_state(state)
    }
    fn read_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
    ) -> Result<ParameterValue, DeviceContractError> {
        Fixture(Fault::None).read_parameter(state, id)
    }
    fn patch_parameter(
        &self,
        state: &NativeDeviceState,
        id: ParameterId,
        value: ParameterValue,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        Fixture(Fault::None).patch_parameter(state, id, value)
    }
}
impl NativeDeviceDefinition for AuxiliaryFixture {
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
        Ok(Some(Box::new(AuxiliaryPrepared)))
    }
}
struct AuxiliaryPrepared;
impl PreparedNativeProcessor for AuxiliaryPrepared {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<super::auxiliary::AuxiliaryProbe>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(super::auxiliary::AuxiliaryProbe::default())
    }
}

#[test]
fn auxiliary_capability_and_prepared_runtime_are_explicit() {
    use hex_lattice_contract::AuxiliaryInput;
    assert_eq!(Fixture(Fault::None).auxiliary_input(), AuxiliaryInput::None);
    assert_eq!(AuxiliaryFixture.auxiliary_input(), AuxiliaryInput::Stereo);
    let tuning = crate::TuningTable::default();
    let context = crate::DevicePreparationContext {
        sample_rate: 48000,
        tuning: &tuning,
        resources: &crate::EMPTY_DEVICE_RESOURCES,
    };
    let state = AuxiliaryFixture.default_state().unwrap();
    let prepared = AuxiliaryFixture
        .prepare_processor(&state, &context)
        .unwrap()
        .unwrap();
    let mut runtime = prepared.create_runtime();
    let mut main = [[2.0, 4.0]; 1];
    let key = [[0.5, 0.25]; 1];
    let mut silence = [[0.0; 2]; 1];
    let block = crate::StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut silence, 1).unwrap();
    runtime
        .process_block_with_auxiliary(block, None, None, false, &crate::EMPTY_DEVICE_RESOURCES)
        .unwrap();
    assert_eq!(main, [[1.0, 1.0]]);

    let mut legacy = Prepared(false).create_runtime();
    let block = crate::StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut silence, 1).unwrap();
    assert!(matches!(
        legacy.process_block_with_auxiliary(
            block,
            None,
            None,
            false,
            &crate::EMPTY_DEVICE_RESOURCES
        ),
        Err(DeviceContractError::UnsupportedProcessingClass)
    ));
    assert_eq!(main, [[1.0, 1.0]]);
}
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

#[test]
fn auxiliary_stage_delivers_independent_pcm_and_rolls_back_with_the_chain() {
    use crate::{
        NativeRuntimeChain, NodeSignalPhase, PreparedProcessorInstance, ProcessorChainOwner,
    };
    use std::sync::Arc;

    for bypassed in [false, true] {
        let mut chain = NativeRuntimeChain::new(vec![PreparedProcessorInstance {
            meter: Arc::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: 2,
            device_type: ID,
            matrix: StereoMatrix::identity(),
            bypassed,
            sample_rate: 48_000,
            configuration: None,
            definition: Some(Arc::new(AuxiliaryPrepared)),
        }])
        .unwrap();
        let mut main = [[2.0, -4.0]; 4];
        let key = [[0.5, 0.25]; 4];
        let mut silence = [];
        let mut taps = Vec::new();
        chain.begin_block();
        for (i, (main, key)) in main.chunks_mut(2).zip(key.chunks(2)).enumerate() {
            let block = crate::StereoAuxiliaryBlock::new(main, Some(key), &mut silence, 4).unwrap();
            chain
                .process_auxiliary_stage(
                    0,
                    block,
                    i * 2,
                    None,
                    None,
                    None,
                    None,
                    &mut |owner, device, kind, phase, offset, frames| {
                        assert_eq!(
                            (owner, device, kind),
                            (ProcessorChainOwner::Track(1), 2, ID)
                        );
                        taps.push((phase, offset, frames[0]));
                    },
                )
                .unwrap();
        }
        let wet = if bypassed { [2.0, -4.0] } else { [1.0, -1.0] };
        assert_eq!(main, [wet; 4]);
        assert_eq!(
            taps,
            [
                (NodeSignalPhase::Input, 0, [2.0, -4.0]),
                (NodeSignalPhase::Output, 0, wet),
                (NodeSignalPhase::Input, 2, [2.0, -4.0]),
                (NodeSignalPhase::Output, 2, wet),
            ]
        );
        chain.commit_block();
        chain.begin_block();
        let block =
            crate::StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut silence, 4).unwrap();
        chain
            .process_auxiliary_stage(
                0,
                block,
                0,
                None,
                None,
                None,
                None,
                &mut |_, _, _, _, _, _| {},
            )
            .unwrap();
        // A later refusal lets the host discard the successful key on this block.
        let block =
            crate::StereoAuxiliaryBlock::new(&mut main, Some(&key), &mut silence, 4).unwrap();
        assert_eq!(
            chain.process_auxiliary_stage(
                1,
                block,
                0,
                None,
                None,
                None,
                None,
                &mut |_, _, _, _, _, _| {}
            ),
            Err(DeviceContractError::InvalidDescriptor)
        );
        chain.rollback_block();
        let crate::RuntimeOwnership::Legacy { primary, scratch } = &chain.entries[0].runtime else {
            panic!("auxiliary fixture runtime")
        };
        for processor in [primary, scratch] {
            let probe = processor
                .as_any()
                .downcast_ref::<super::auxiliary::AuxiliaryProbe>()
                .unwrap();
            assert_eq!(probe.frames_seen, 4);
        }
    }
}
