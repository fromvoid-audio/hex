//! Exact closed constructor for one typed Off stage, never source admission.
use super::*;
use crate::{Audio, AudioCandidateEffects, AudioCandidatePreparation, AudioSourceContext};
use hex_lattice::{
    DevicePreparationContext, NativeDeviceDefinition, NativeDeviceRegistry,
    PreparedProcessorFactory, ProcessorInstanceIdentity, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{Command, Created, Editor, Project};

struct Definition(bool);
impl NativeDeviceMetadata for Definition {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("hex.vst3")
    }
    fn display_name(&self) -> Option<&'static str> {
        None
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![7, 8])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
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
    fn prepare_matrix(&self, _: &NativeDeviceState) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_instance_runtime_factory(
        &self,
        _: ProcessorInstanceIdentity,
        _: &NativeDeviceState,
        _: &DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(self.0.then_some(PreparedProcessorFactory::PoweredOff))
    }
}

#[test]
fn typed_main_off_seals_actual_data_and_reports_its_distinct_status() {
    verify_typed_off(false);
}

#[test]
fn typed_return_off_seals_actual_data_and_reports_its_distinct_status() {
    verify_typed_off(true);
}

fn verify_typed_off(on_return: bool) {
    let definition = Definition(true);
    let defs: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&defs).unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "input".into(),
            path: "memory/off.wav".into(),
            sample_rate: 48_000,
            frames: 514,
        })
        .unwrap()
    else {
        panic!("audio")
    };
    let state = definition.default_state().unwrap();
    let (command, owner) = if on_return {
        let Created::ReturnTrack(bus) = editor
            .execute(Command::AddReturnTrack {
                name: "send".into(),
            })
            .unwrap()
        else {
            panic!("return");
        };
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track: bus,
                gain: 0.5,
            })
            .unwrap();
        (
            Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: state.clone(),
            },
            hex_lattice::ProcessorChainOwner::Return(bus.value()),
        )
    } else {
        (
            Command::InsertMainDevice {
                before: None,
                state: state.clone(),
            },
            hex_lattice::ProcessorChainOwner::Main,
        )
    };
    let Created::Device(device) = editor.execute(command).unwrap() else {
        panic!("off device")
    };
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let quote = effects.quote(editor.project(), 1).unwrap();
    let mut candidate = AudioCandidatePreparation::try_new_on_control(
        1,
        quote
            .storage_reservation(514 * std::mem::size_of::<crate::Frame>())
            .unwrap(),
    )
    .unwrap();
    candidate
        .copy_resident_source_on_control(asset, &Audio::new(48_000, vec![[0.125; 2]; 514]).unwrap())
        .unwrap();
    effects
        .prepare(
            &mut candidate,
            editor.project(),
            3,
            AudioSourceContext {
                project_generation: 2,
                editor_revision: 8,
            },
            None,
        )
        .unwrap();
    let candidate_owner = candidate.owner_on_control().unwrap();
    let sealed = candidate_owner
        .seal_constructor_on_control(reservation())
        .unwrap();
    assert_eq!(sealed.data.unknown_storage, 0);
    assert!(sealed.data.data_allocations <= quote.data_rows);
    let mut cap = reservation();
    cap.data_rows = sealed.data.data_allocations - 1;
    assert_eq!(
        candidate_owner.seal_constructor_on_control(cap),
        Err(AudioStorageError::Capacity)
    );
    let playback = &candidate_owner.inner.runtime.as_ref().unwrap().playback;
    assert_eq!(playback.main_output_latency_frames(), 0);
    let revision = playback.prepared_revision().unwrap();
    assert_eq!(
        revision.sole_powered_off_device_on_control(),
        Some(ProcessorInstanceIdentity::new(owner, device.value()).unwrap())
    );
    assert_eq!(playback.foreign_nodes_on_control().count(), 0);
    if !on_return {
        let chain = playback.test_main_chain().unwrap();
        assert_eq!(chain.stateful_count(), 0);
        assert_eq!(chain.declared_pair_bytes(), 0);
        assert!(
            chain
                .storage_view()
                .entries()
                .next()
                .unwrap()
                .is_powered_off()
        );
        assert_eq!(
            revision.sole_powered_off_main_device_on_control(),
            Some(device.value())
        );
    }
    let observation = playback.prepare_observation_on_control().unwrap();
    assert_eq!(
        observation.processor_status(device.value(), state.type_id()),
        Some(crate::PreparedDeviceProcessing::PoweredOff)
    );
    assert_eq!(
        observation.foreign_processor(device.value(), state.type_id()),
        Some(false)
    );
    let arrangement = revision.arrangement();
    let stage = if on_return {
        &arrangement.return_buses()[0].device_chain[0]
    } else {
        &arrangement.main_device_chain[0]
    };
    assert_eq!(stage.configuration.as_deref(), Some(&state));
    // The old generic factoryless hosted path remains unsupported.
    let missing = Definition(false);
    let defs: [&dyn NativeDeviceDefinition; 1] = [&missing];
    let registry = NativeDeviceRegistry::new(&defs).unwrap();
    let effects = AudioCandidateEffects::new(&registry);
    let mut other = AudioCandidatePreparation::try_new_on_control(
        2,
        quote
            .storage_reservation(514 * std::mem::size_of::<crate::Frame>())
            .unwrap(),
    )
    .unwrap();
    other
        .copy_resident_source_on_control(asset, &Audio::new(48_000, vec![[0.125; 2]; 514]).unwrap())
        .unwrap();
    assert!(
        effects
            .prepare(
                &mut other,
                editor.project(),
                3,
                AudioSourceContext {
                    project_generation: 2,
                    editor_revision: 8
                },
                None
            )
            .is_err()
    );
}

#[test]
fn ordinary_main_off_does_not_consume_the_full_stateful_budget() {
    let definition = Definition(true);
    let native = &hex_devices::overdrive::OVERDRIVE_DEFINITION;
    let definitions: [&dyn NativeDeviceDefinition; 2] = [&definition, native];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 1).unwrap();
    let state = native.default_state().unwrap();
    for _ in 0..hex_lattice::MAX_STATEFUL_PROCESSORS / hex_project::MAX_DEVICES_PER_TRACK {
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "budget".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        for _ in 0..hex_project::MAX_DEVICES_PER_TRACK {
            editor
                .execute(Command::InsertDevice {
                    track,
                    before: None,
                    state: state.clone(),
                })
                .unwrap();
        }
    }
    let compile = |project: &Project| {
        crate::compile::project_with_registry(
            project,
            &std::collections::BTreeMap::new(),
            &registry,
        )
    };
    let before = compile(editor.project()).unwrap();
    assert_eq!(
        before
            .tracks
            .iter()
            .map(|track| track.device_chain.len())
            .sum::<usize>(),
        hex_lattice::MAX_STATEFUL_PROCESSORS
    );
    let Created::Device(device) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: definition.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("main")
    };
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let off = compile(editor.project()).unwrap();
    assert!(
        off.main_device_chain[0]
            .definition
            .as_ref()
            .unwrap()
            .is_powered_off()
    );
    // A genuine additional runtime still exceeds the same global limit.
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "overflow".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    assert_eq!(
        compile(editor.project()).err(),
        Some(crate::Invalid(
            "stateful device runtime exceeds its bounded budget"
        ))
    );
}
