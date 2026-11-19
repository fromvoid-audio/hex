//! A real closed constructor and real Root queue. Scalar facts deliberately
//! have no native worker/endpoint: an empty native pool must not admit them.
use super::*;
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, PreparedForeignProcessor,
    PreparedProcessorFactory, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, ForeignProcessorBinding, NativeDeviceMetadata,
    NativeDeviceState, ParameterDescriptor, ParameterId, ParameterValue,
};
struct Unbacked;
impl NativeDeviceMetadata for Unbacked {
    fn type_id(&self) -> DeviceTypeId {
        DeviceTypeId::from_static("test.unbacked-foreign")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(self.type_id(), 1, vec![])
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
impl NativeDeviceDefinition for Unbacked {
    fn prepare_matrix(&self, _: &NativeDeviceState) -> Result<StereoMatrix, DeviceContractError> {
        Ok(StereoMatrix::identity())
    }
    fn prepare_instance_runtime_factory(
        &self,
        _: hex_lattice::ProcessorInstanceIdentity,
        _: &NativeDeviceState,
        _: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Option<PreparedProcessorFactory>, DeviceContractError> {
        Ok(Some(PreparedProcessorFactory::Foreign(
            PreparedForeignProcessor::new(8, 3, 16)?.with_binding(ForeignProcessorBinding {
                root: 7,
                instance: 91,
                graph: 12,
                epoch: 1,
            })?,
        )))
    }
}
#[test]
fn empty_native_pool_refuses_actual_foreign_constructor_before_queue_or_ack() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    use hex_project::{Command, Created, Editor};
    let mut editor = Editor::new(hex_project::Project::new(48000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: Unbacked.default_state().unwrap(),
        })
        .unwrap();
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&Unbacked];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![],
        hex_engine::AudioStorageReservation {
            allocation_items: 64,
            handle_obligations: 256,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            hex_engine::AudioArrangementInput::new(editor.project(), 11, bank)
                .with_audio_prepared_revision()
                .with_effect_registry(&registry),
        )
        .ok()
        .unwrap();
    let mut owner = hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
        .ok()
        .unwrap();
    let identity = owner.identity();
    let reservation = reservation(&mut owner);
    let (mut control, data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(reservation)
        .unwrap();
    let refused = control
        .stage_eligible_audio_candidate(owner, vec![], PublicationPolicy::InheritRuntime, 41)
        .err()
        .expect("no original native source exists");
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(refused.reason, "native graph source differs");
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    assert_eq!(
        refused.reason,
        "effect graph source and runtime witness is not sealed"
    );
    assert_eq!(refused.owner.identity(), identity);
    assert_eq!(control.submitted_revision(), 0);
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.audio_candidate_custody().occupied_slots, 0);
    let mut owner = refused.owner;
    owner.dispose_on_control().unwrap();
    assert!(owner.recover_inputs_on_control().is_some());
    drop(control);
    drop(data);
    drop(errors);
    finish(&mut retirement);
}
