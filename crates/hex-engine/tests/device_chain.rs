use hex_devices::utility::{UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter};
use hex_engine::{Audio, Renderer, compile};
use hex_lattice::{NativeDeviceDefinition, NativeDeviceRegistry, StereoMatrix};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn utility_patch(
    state: &NativeDeviceState,
    parameter: ParameterId,
    value: ParameterValue,
) -> NativeDeviceState {
    UTILITY_DEFINITION
        .patch_parameter(state, parameter, value)
        .unwrap()
}

fn imported_audio_project() -> (Editor, hex_project::AssetId, hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/device-chain.wav".into(),
            sample_rate: 48_000,
            frames: 1,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    let track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
        })
        .unwrap();
    (editor, asset, track)
}
fn rendered(project: &Project, asset: hex_project::AssetId) -> [f32; 2] {
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]]).unwrap())]);
    let compiled = compile::project(project, &media).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 1];
    renderer.render(&mut output).unwrap();
    output[0]
}
fn add_device(
    editor: &mut Editor,
    track: hex_project::TrackId,
    state: NativeDeviceState,
) -> hex_project::DeviceId {
    match editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap()
    {
        Created::Device(id) => id,
        _ => panic!("device"),
    }
}

#[test]
fn arrangement_utility_instances_process_audio_in_serial_order_and_honor_bypass_removal() {
    let (mut editor, asset, track) = imported_audio_project();
    let initial = UTILITY_DEFINITION.default_state().unwrap();
    let invert = add_device(
        &mut editor,
        track,
        utility_patch(
            &initial,
            parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        ),
    );
    let swap_state = utility_patch(&initial, parameter::CHANNEL, ParameterValue::Choice(3));
    let swap = add_device(&mut editor, track, swap_state);
    assert_eq!(rendered(editor.project(), asset), [0.5, 0.25]);
    editor
        .execute(Command::MoveDevice {
            device: swap,
            track,
            before: Some(invert),
        })
        .unwrap();
    assert_eq!(rendered(editor.project(), asset), [-0.5, -0.25]);
    editor
        .execute(Command::SetDeviceBypass {
            device: invert,
            bypassed: true,
        })
        .unwrap();
    assert_eq!(rendered(editor.project(), asset), [-0.5, 0.25]);
    editor.execute(Command::RemoveDevice(swap)).unwrap();
    assert_eq!(rendered(editor.project(), asset), [0.25, -0.5]);
}

#[test]
fn arrangement_midi_receives_the_same_fused_device_transform() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match editor
        .execute(Command::AddTrack {
            name: "midi".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    {
        Created::Clip(id) => id,
        _ => panic!("clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let initial = UTILITY_DEFINITION.default_state().unwrap();
    add_device(
        &mut editor,
        track,
        utility_patch(
            &initial,
            parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        ),
    );
    let compiled = compile::project(editor.project(), &BTreeMap::new()).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 512];
    renderer.render(&mut output).unwrap();
    assert!(output.iter().any(|frame| frame[0].abs() > 1.0e-5));
    assert!(output.iter().all(|frame| frame[1] == -frame[0]));
}

#[test]
fn unknown_types_and_unsupported_versions_refuse_compile_even_when_bypassed() {
    let (mut editor, asset, track) = imported_audio_project();
    let unknown =
        NativeDeviceState::new(DeviceTypeId::new("vendor.unknown").unwrap(), 9, vec![1, 2])
            .unwrap();
    let device = add_device(&mut editor, track, unknown);
    editor
        .execute(Command::SetDeviceBypass {
            device,
            bypassed: true,
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]]).unwrap())]);
    assert!(compile::project(editor.project(), &media).is_err());

    editor.execute(Command::RemoveDevice(device)).unwrap();
    let future = NativeDeviceState::new(UTILITY_TYPE_ID, 99, vec![1]).unwrap();
    let future_device = add_device(&mut editor, track, future);
    editor
        .execute(Command::SetDeviceBypass {
            device: future_device,
            bypassed: true,
        })
        .unwrap();
    assert!(compile::project(editor.project(), &media).is_err());

    editor
        .execute(Command::RemoveDevice(future_device))
        .unwrap();
    let malformed = NativeDeviceState::new(UTILITY_TYPE_ID, 1, vec![0; 18]).unwrap();
    add_device(&mut editor, track, malformed);
    assert!(compile::project(editor.project(), &media).is_err());
}

#[test]
fn serial_maximum_gain_refuses_matrix_overflow_and_valid_boundary_compiles() {
    let (mut editor, asset, track) = imported_audio_project();
    let mut state = UTILITY_DEFINITION.default_state().unwrap();
    for _ in 0..24 {
        state = utility_patch(
            &state,
            parameter::GAIN,
            ParameterValue::Float(hex_devices::utility::MAX_UTILITY_GAIN),
        );
        add_device(&mut editor, track, state.clone());
    }
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]]).unwrap())]);
    assert!(compile::project(editor.project(), &media).is_err());
    let mut one = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = match one
        .execute(Command::AddTrack {
            name: "gain".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        _ => panic!("track"),
    };
    state = UTILITY_DEFINITION.default_state().unwrap();
    state = utility_patch(
        &state,
        parameter::GAIN,
        ParameterValue::Float(hex_devices::utility::MAX_UTILITY_GAIN),
    );
    add_device(&mut one, track, state);
    assert_eq!(
        one.project().track(track).unwrap().devices()[0]
            .state()
            .type_id(),
        UTILITY_TYPE_ID
    );
    assert!(compile::project(one.project(), &BTreeMap::new()).is_ok());
}

struct TestDevice;
const TEST_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.swap");
impl NativeDeviceMetadata for TestDevice {
    fn type_id(&self) -> DeviceTypeId {
        TEST_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(TEST_TYPE, 1, Vec::new())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() != TEST_TYPE {
            return Err(DeviceContractError::WrongDeviceType);
        }
        if state.state_version() != 1 || !state.payload().is_empty() {
            return Err(DeviceContractError::UnsupportedStateVersion);
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
impl NativeDeviceDefinition for TestDevice {
    fn prepare_matrix(
        &self,
        state: &NativeDeviceState,
    ) -> Result<StereoMatrix, DeviceContractError> {
        self.migrate_state(state)?;
        StereoMatrix::new([[0.0, 1.0], [1.0, 0.0]]).map_err(|_| DeviceContractError::MalformedState)
    }
}

#[test]
fn second_registered_definition_compiles_through_generic_registry() {
    let (mut editor, asset, track) = imported_audio_project();
    let test_state = TestDevice.default_state().unwrap();
    add_device(&mut editor, track, test_state);
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]]).unwrap())]);
    let definitions: [&dyn NativeDeviceDefinition; 2] = [&UTILITY_DEFINITION, &TestDevice];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let compiled = compile::project_with_registry(editor.project(), &media, &registry).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 1];
    renderer.render(&mut output).unwrap();
    assert_eq!(output[0], [-0.5, 0.25]);
}
