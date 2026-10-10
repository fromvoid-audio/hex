use hex_devices::utility::{UTILITY_DEFINITION, UTILITY_TYPE_ID, parameter};
use hex_engine::{
    Audio, PlaybackAction, PlaybackCommand, PreparedRevision, Renderer, compile, playback::Playback,
};
use hex_lattice::{
    NativeDeviceDefinition, NativeDeviceRegistry, NativeProcessorRuntime, NativeRuntimeChain,
    PreparedNativeProcessor, PreparedProcessorInstance, StereoMatrix,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;
use std::sync::Arc;

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
        if state.state_version() != 1
            || !(state.payload().is_empty() || matches!(state.payload(), [2] | [3]))
        {
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

    fn prepare_processor(
        &self,
        state: &NativeDeviceState,
        _sample_rate: u32,
    ) -> Result<Option<Box<dyn PreparedNativeProcessor>>, DeviceContractError> {
        self.migrate_state(state)?;
        let (scale, latency) = match state.payload() {
            [] => (1.0, None),
            [2] => (0.5, None),
            [3] => (2.0, Some(1)),
            _ => return Err(DeviceContractError::MalformedState),
        };
        Ok(Some(Box::new(SquarePrepared { scale, latency })))
    }
}

struct SquarePrepared {
    scale: f32,
    latency: Option<u32>,
}
impl PreparedNativeProcessor for SquarePrepared {
    fn runtime_version(&self) -> u32 {
        1
    }

    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<SquareRuntime>()
    }

    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(SquareRuntime {
            previous_left: 0.0,
            scale: self.scale,
        })
    }

    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.latency
    }
}

struct SquareRuntime {
    previous_left: f32,
    scale: f32,
}
impl NativeProcessorRuntime for SquareRuntime {
    fn process_block(&mut self, frames: &mut [hex_lattice::StereoFrame]) {
        for frame in frames {
            let [left, right] = *frame;
            *frame = [
                (left * left + self.previous_left) * self.scale,
                (right * right + self.previous_left) * self.scale,
            ];
            self.previous_left = left;
        }
    }

    fn reset(&mut self) {
        self.previous_left = 0.0;
    }

    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }

    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.previous_left = previous.previous_left;
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
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
    assert_eq!(output[0], [0.25, 0.0625]);
}

#[test]
fn registered_stateful_device_processes_the_summed_track_once() {
    let (mut editor, asset, track) = imported_audio_project();
    add_device(&mut editor, track, TestDevice.default_state().unwrap());
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
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.25, -0.5]]).unwrap())]);
    let definitions: [&dyn NativeDeviceDefinition; 2] = [&UTILITY_DEFINITION, &TestDevice];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let compiled = compile::project_with_registry(editor.project(), &media, &registry).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 1];
    renderer.render(&mut output).unwrap();
    // The clips sum to [0.5, -1.0], the matrix swaps them, then the one
    // stateful pass squares the summed signal: [-1, .5] -> [1, .25].
    assert_eq!(output, [[1.0, 0.25]]);
}

#[test]
fn independent_nonlinear_runtimes_keep_order_history_and_partition_exactness() {
    let definitions: [&dyn NativeDeviceDefinition; 2] = [&UTILITY_DEFINITION, &TestDevice];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let instance = |device_id, state: NativeDeviceState, sample_rate| {
        let processor = registry
            .prepare_processor(&state, sample_rate)
            .unwrap()
            .unwrap();
        PreparedProcessorInstance {
            owner: hex_lattice::ProcessorChainOwner::Track(3),
            device_id,
            device_type: TEST_TYPE,
            matrix: TestDevice.prepare_matrix(&state).unwrap(),
            bypassed: false,
            sample_rate,
            definition: Some(Arc::from(processor)),
        }
    };
    let default = TestDevice.default_state().unwrap();
    let edited = NativeDeviceState::new(TEST_TYPE, 1, vec![2]).unwrap();
    let mut old = NativeRuntimeChain::new(vec![instance(9, default.clone(), 48_000)]).unwrap();
    old.begin_block();
    let mut prefix = [[0.0, 3.0]];
    old.process_block(&mut prefix);
    old.commit_block();
    assert_eq!(prefix, [[9.0, 0.0]]);

    // A compatible parameter edit preserves physical history while the new
    // immutable scale remains active on the candidate processor.
    let mut compatible =
        NativeRuntimeChain::new(vec![instance(9, edited.clone(), 48_000)]).unwrap();
    assert!(compatible.can_inherit_from(&old));
    compatible.inherit_from(&old);
    compatible.begin_block();
    let mut continued = [[0.0, 2.0]];
    compatible.process_block(&mut continued);
    compatible.commit_block();
    assert_eq!(continued, [[3.5, 1.5]]);

    // The same device and state at another sample rate start with clean
    // history; compatibility is checked without refusing publication.
    let mut rate_changed =
        NativeRuntimeChain::new(vec![instance(9, edited.clone(), 44_100)]).unwrap();
    assert!(rate_changed.can_inherit_from(&old));
    rate_changed.inherit_from(&old);
    rate_changed.begin_block();
    let mut fresh = [[0.0, 2.0]];
    rate_changed.process_block(&mut fresh);
    rate_changed.commit_block();
    assert_eq!(fresh, [[2.0, 0.0]]);

    // Two independently registered instances run in serial order. Splitting
    // one prepared block at an arbitrary boundary preserves both histories.
    let make_pair = || {
        NativeRuntimeChain::new(vec![
            instance(9, default.clone(), 48_000),
            instance(10, edited.clone(), 48_000),
        ])
        .unwrap()
    };
    let input = [[0.5, -1.0], [1.0, 0.0]];
    let mut whole = make_pair();
    whole.begin_block();
    let mut whole_frames = input;
    whole.process_block(&mut whole_frames);
    whole.commit_block();

    let mut partitioned = make_pair();
    partitioned.begin_block();
    let mut split_frames = input;
    partitioned.process_block(&mut split_frames[..1]);
    partitioned.process_block(&mut split_frames[1..]);
    partitioned.commit_block();
    assert_eq!(whole_frames, split_frames);
    assert_eq!(whole_frames, [[0.03125, 0.5], [0.125, 0.625]]);
}

#[test]
fn utility_stateful_arrangement_renderer_and_session_playback_match() {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/stateful.wav".into(),
            sample_rate: 48_000,
            frames: 512,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        _ => panic!("asset"),
    };
    let track = match editor
        .execute(Command::AddTrack {
            name: "filters".into(),
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
            duration: 512,
            source_offset: 0,
            source_length: 512,
            looping: false,
        })
        .unwrap();
    let initial = UTILITY_DEFINITION.default_state().unwrap();
    let state = utility_patch(
        &utility_patch(&initial, parameter::DC_REMOVE, ParameterValue::Bool(true)),
        parameter::BASS_MONO,
        ParameterValue::Bool(true),
    );
    add_device(&mut editor, track, state);
    let samples = (0..512)
        .map(|index| {
            let phase = index as f32 * 0.137;
            [0.3 + phase.sin() * 0.2, -0.25 + (phase * 0.73).cos() * 0.31]
        })
        .collect();
    let media = BTreeMap::from([(asset, Audio::new(48_000, samples).unwrap())]);
    let compiled = compile::project(editor.project(), &media).unwrap();
    let mut offline = Renderer::new(&compiled);
    offline.play();
    let mut expected = [[0.0; 2]; 512];
    offline.render(&mut expected).unwrap();

    let prepared = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_devices::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = Playback::try_from_prepared_with_session(prepared).unwrap();
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
        .unwrap();
    let mut native = [[0.0; 2]; 512];
    playback.render(&mut native).unwrap();
    assert_eq!(native, expected);
}

#[test]
fn active_stateful_budget_is_global_across_track_chains() {
    let filter_state = utility_patch(
        &UTILITY_DEFINITION.default_state().unwrap(),
        parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    );
    let prepare_tracks = |track_count: usize| {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        for index in 0..track_count {
            let track = match editor
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .unwrap()
            {
                Created::Track(track) => track,
                _ => panic!("track"),
            };
            for _ in 0..9 {
                add_device(&mut editor, track, filter_state.clone());
            }
        }
        editor
    };

    let within_limit = prepare_tracks(28);
    assert!(compile::project(within_limit.project(), &BTreeMap::new()).is_ok());
    let over_limit = prepare_tracks(29);
    assert!(compile::project(over_limit.project(), &BTreeMap::new()).is_err());
}

#[test]
fn typed_utility_filter_edit_round_trips_and_undoes_as_one_project_command() {
    let (mut editor, _, track) = imported_audio_project();
    let device = add_device(
        &mut editor,
        track,
        UTILITY_DEFINITION.default_state().unwrap(),
    );
    let before = editor.project().snapshot();
    let changed_state = utility_patch(
        editor.project().track(track).unwrap().devices()[0].state(),
        parameter::DC_REMOVE,
        ParameterValue::Bool(true),
    );
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state: changed_state.clone(),
        })
        .unwrap();
    assert_eq!(changed_state.state_version(), 2);
    assert_eq!(
        Project::from_snapshot(editor.project().snapshot()).unwrap(),
        *editor.project()
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert_eq!(
        editor.project().track(track).unwrap().devices()[0].state(),
        &UTILITY_DEFINITION.default_state().unwrap()
    );
    assert!(editor.redo());
    assert_eq!(
        editor.project().track(track).unwrap().devices()[0].state(),
        &changed_state
    );
}

#[test]
fn return_effect_compilation_requires_explicit_zero_algorithmic_latency() {
    let definitions: [&dyn NativeDeviceDefinition; 2] = [&UTILITY_DEFINITION, &TestDevice];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    for state in [
        TestDevice.default_state().unwrap(),
        NativeDeviceState::new(TEST_TYPE, 1, vec![3]).unwrap(),
    ] {
        let (mut editor, asset, track) = imported_audio_project();
        let return_track = match editor
            .execute(Command::AddReturnTrack {
                name: "return".into(),
            })
            .unwrap()
        {
            Created::ReturnTrack(id) => id,
            other => panic!("unexpected result: {other:?}"),
        };
        editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            })
            .unwrap();
        editor
            .execute(Command::SetReturnSend {
                track,
                return_track,
                gain: 0.5,
            })
            .unwrap();
        let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.5, -0.25]]).unwrap())]);
        let error =
            compile::project_with_registry(editor.project(), &media, &registry).unwrap_err();
        assert!(error.0.contains("unknown or nonzero algorithmic latency"));
    }
}
