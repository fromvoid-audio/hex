use hex_devices::SineTable;
use hex_engine::{PreparedSessionInventory, Renderer, compile};
use hex_lattice::{
    NativeInstrumentDefinition, PreparedAnalyticNote, PreparedNativeInstrument,
    PreparedSharedAnalyticContext, PreparedSharedAnalyticNote, StereoFrame,
};
use hex_lattice_contract::{
    DeviceContractError, DeviceTypeId, NativeDeviceMetadata, NativeDeviceState,
    ParameterDescriptor, ParameterId, ParameterValue,
};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

struct ConstantDefinition {
    id: DeviceTypeId,
    stereo: StereoFrame,
}
struct ConstantPrepared {
    stereo: StereoFrame,
}

impl NativeDeviceMetadata for ConstantDefinition {
    fn type_id(&self) -> DeviceTypeId {
        self.id
    }
    fn display_name(&self) -> Option<&'static str> {
        Some("test tone")
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(self.id, 1, Vec::new())
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() == self.id && state.state_version() == 1 && state.payload().is_empty() {
            Ok(state.clone())
        } else {
            Err(DeviceContractError::MalformedState)
        }
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
impl NativeInstrumentDefinition for ConstantDefinition {
    fn prepare_instrument(
        &self,
        _: &NativeDeviceState,
        context: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn PreparedNativeInstrument>, DeviceContractError> {
        let sample_rate = context.sample_rate;
        if !context.tuning.is_neutral() {
            return Err(hex_lattice_contract::DeviceContractError::UnsupportedProcessingClass);
        }
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        Ok(Box::new(ConstantPrepared {
            stereo: self.stereo,
        }))
    }
}
impl PreparedNativeInstrument for ConstantPrepared {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }

    fn prepare_note(&self, _: u8, _: u8) -> Result<PreparedAnalyticNote, DeviceContractError> {
        let mut payload = [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(self.stereo[0].to_bits());
        payload[1] = u64::from(self.stereo[1].to_bits());
        Ok(PreparedAnalyticNote::new(payload, 4, constant_sample))
    }

    fn prepare_shared_analytic_context(
        &self,
    ) -> Result<Option<Box<dyn PreparedSharedAnalyticContext>>, DeviceContractError> {
        Ok(Some(Box::new(ConstantSharedContext {
            stereo: self.stereo,
        })))
    }
}

struct ConstantSharedContext {
    stereo: StereoFrame,
}

impl PreparedSharedAnalyticContext for ConstantSharedContext {
    fn declared_context_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<PreparedSharedAnalyticNote, DeviceContractError> {
        let mut payload = [0; hex_lattice::SHARED_ANALYTIC_NOTE_PAYLOAD_WORDS];
        payload[0] = u64::from(self.stereo[0].to_bits());
        payload[1] = u64::from(self.stereo[1].to_bits());
        Ok(PreparedSharedAnalyticNote::new(payload, 4))
    }

    fn sample(&self, note: &PreparedSharedAnalyticNote, age: u64, gate: u64) -> StereoFrame {
        if age < gate.saturating_add(note.release_frames()) {
            [
                f32::from_bits(note.payload()[0] as u32),
                f32::from_bits(note.payload()[1] as u32),
            ]
        } else {
            [0.0; 2]
        }
    }
}
fn constant_sample(
    payload: &[u64; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
    age: u64,
    gate: u64,
    release: u64,
) -> StereoFrame {
    if age < gate.saturating_add(release) {
        [
            f32::from_bits(payload[0] as u32),
            f32::from_bits(payload[1] as u32),
        ]
    } else {
        [0.0; 2]
    }
}

#[test]
fn two_registered_analytic_definitions_prepare_arrangement_and_session_notes() {
    let left = ConstantDefinition {
        id: DeviceTypeId::from_static("test.instrument.left"),
        stereo: [0.25, 0.5],
    };
    let right = ConstantDefinition {
        id: DeviceTypeId::from_static("test.instrument.right"),
        stereo: [-0.25, 0.75],
    };
    let definitions: [&dyn NativeInstrumentDefinition; 2] = [&left, &right];
    let registry = hex_lattice::NativeInstrumentRegistry::new(&definitions).unwrap();

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    for (name, definition) in [("left", &left), ("right", &right)] {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        let instrument = editor
            .project()
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        editor
            .execute(Command::ReplaceDeviceState {
                device: instrument,
                state: registry.default_state(definition.type_id()).unwrap(),
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::from_quarters(4).unwrap(),
            })
            .unwrap()
        else {
            panic!("arrangement clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        tracks.push(track);
    }
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let session_clips: Vec<_> = tracks
        .iter()
        .map(|track| {
            let Created::SessionClip(clip) = editor
                .execute(Command::CreateSessionMidiClip {
                    track: *track,
                    scene,
                    name: "source".into(),
                    source_length: BeatTime::from_quarters(4).unwrap(),
                })
                .unwrap()
            else {
                panic!("session clip")
            };
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: BeatTime::ZERO,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
            clip
        })
        .collect();

    let arrangement =
        compile::project_with_instrument_registry(editor.project(), &BTreeMap::new(), &registry)
            .unwrap();
    let mut renderer = Renderer::new(&arrangement);
    renderer.play();
    let mut arranged = [[0.0; 2]; 1];
    renderer.render(&mut arranged).unwrap();
    assert_eq!(arranged[0], [0.0, 1.25]);

    let inventory = PreparedSessionInventory::prepare_with_media_and_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &registry,
    )
    .unwrap();
    let mut session_mix = [0.0_f32; 2];
    for (clip, expected) in session_clips
        .iter()
        .copied()
        .zip([[0.25, 0.5], [-0.25, 0.75]])
    {
        let prepared = inventory.midi_clip(clip).unwrap();
        let mut output = [[0.0; 2]; 1];
        prepared
            .instrument_template()
            .render(
                BeatTime::ZERO,
                inventory.timing(),
                inventory.sine_table(),
                0,
                &mut output,
                &mut Default::default(),
            )
            .unwrap();
        assert_eq!(output[0], expected);
        session_mix[0] += output[0][0];
        session_mix[1] += output[0][1];
    }
    assert_eq!(session_mix, [0.0, 1.25]);

    let first_track = tracks[0];
    let first_clip = session_clips[0];
    let first_device = editor
        .project()
        .track(first_track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: first_device,
            state: registry.default_state(right.type_id()).unwrap(),
        })
        .unwrap();
    let changed_type = PreparedSessionInventory::prepare_with_media_and_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        Some(&inventory),
        &registry,
    )
    .unwrap();
    let mut changed_type_output = [[0.0_f32; 2]; 1];
    changed_type
        .midi_clip(first_clip)
        .unwrap()
        .instrument_template()
        .render(
            BeatTime::ZERO,
            changed_type.timing(),
            changed_type.sine_table(),
            0,
            &mut changed_type_output,
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(changed_type_output, [[-0.25, 0.75]]);
    let mut untouched_track_output = [[0.0_f32; 2]; 1];
    changed_type
        .midi_clip(session_clips[1])
        .unwrap()
        .instrument_template()
        .render(
            BeatTime::ZERO,
            changed_type.timing(),
            changed_type.sine_table(),
            0,
            &mut untouched_track_output,
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(untouched_track_output, [[-0.25, 0.75]]);

    editor
        .execute(Command::SetDeviceBypass {
            device: first_device,
            bypassed: true,
        })
        .unwrap();
    let bypassed = PreparedSessionInventory::prepare_with_media_and_instrument_registry(
        editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        Some(&changed_type),
        &registry,
    )
    .unwrap();
    let mut bypassed_output = [[0.0_f32; 2]; 1];
    bypassed
        .midi_clip(first_clip)
        .unwrap()
        .instrument_template()
        .render(
            BeatTime::ZERO,
            bypassed.timing(),
            bypassed.sine_table(),
            0,
            &mut bypassed_output,
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(bypassed_output, [[0.0; 2]]);
}
