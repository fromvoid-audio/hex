//! Legacy Arrangement rendering routes shared source MIDI through one receiver.
use super::*;
use hex_lattice_contract::{DeviceContractError, DeviceTypeId, NativeDeviceState};
use hex_project::{BeatTime, SourceId};

const LEGACY_INSTRUMENT_TYPE: DeviceTypeId = DeviceTypeId::from_static("test.shared.legacy");
struct LegacyInstrumentDefinition;
struct LegacyInstrument;
impl NativeDeviceMetadata for LegacyInstrumentDefinition {
    fn type_id(&self) -> DeviceTypeId {
        LEGACY_INSTRUMENT_TYPE
    }
    fn state_version(&self) -> u32 {
        1
    }
    fn descriptors(&self) -> &'static [ParameterDescriptor] {
        &[]
    }
    fn default_state(&self) -> Result<NativeDeviceState, DeviceContractError> {
        NativeDeviceState::new(LEGACY_INSTRUMENT_TYPE, 1, vec![])
    }
    fn migrate_state(
        &self,
        state: &NativeDeviceState,
    ) -> Result<NativeDeviceState, DeviceContractError> {
        if state.type_id() == LEGACY_INSTRUMENT_TYPE
            && state.state_version() == 1
            && state.payload().is_empty()
        {
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
impl hex_lattice::NativeInstrumentDefinition for LegacyInstrumentDefinition {
    fn prepare_instrument(
        &self,
        state: &NativeDeviceState,
        _: &hex_lattice::DevicePreparationContext<'_>,
    ) -> Result<Box<dyn hex_lattice::PreparedNativeInstrument>, DeviceContractError> {
        self.migrate_state(state)?;
        Ok(Box::new(LegacyInstrument))
    }
}
impl hex_lattice::PreparedNativeInstrument for LegacyInstrument {
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, DeviceContractError> {
        Ok(hex_lattice::PreparedAnalyticNote::new(
            [0; hex_lattice::ANALYTIC_NOTE_PAYLOAD_WORDS],
            0,
            |_, age, _, _, _| {
                if age < 8 { [0.25, -0.5] } else { [0.0; 2] }
            },
        ))
    }
}

fn shared_fixture(receiver_effects: &[u32], contributor_effects: &[u32]) -> (Fixture, SourceId) {
    let mut f = fixture(&[receiver_effects, contributor_effects], 32);
    let Created::NativeSource(source) = f
        .editor
        .execute(Command::CreateNativeSource {
            name: "shared source".into(),
            devices: vec![LegacyInstrumentDefinition.default_state().unwrap()],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    for (index, track) in f.tracks.iter().copied().enumerate() {
        let asset = f.editor.project().assets().nth(index).unwrap().id();
        f.editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 30_000,
                duration: 32,
                source_offset: 0,
                source_length: 32,
                looping: false,
            })
            .unwrap();
        assert_eq!(f.editor.project().track(track).unwrap().clips().len(), 2);
        f.editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
        assert_eq!(f.editor.project().track(track).unwrap().clips().len(), 2);
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        assert_eq!(f.editor.project().track(track).unwrap().clips().len(), 1);
        f.editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60 + index as u8,
                velocity: 100,
                channel: 0,
            })
            .unwrap();
    }
    (f, source)
}

fn compiled(f: &Fixture) -> Project {
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let instruments: [&dyn hex_lattice::NativeInstrumentDefinition; 1] =
        [&LegacyInstrumentDefinition];
    crate::compile::project_with_midi_preview_and_table_and_instrument_registry(
        f.editor.project(),
        &f.media,
        None,
        &NativeDeviceRegistry::new(&effects).unwrap(),
        &hex_lattice::NativeInstrumentRegistry::new(&instruments).unwrap(),
    )
    .unwrap()
    .0
}

fn render(project: &Project, frames: usize) -> Vec<Frame> {
    let mut renderer = Renderer::new(project);
    renderer.play();
    let mut output = vec![[0.0; 2]; frames];
    renderer.render(&mut output).unwrap();
    output
}

fn sum_frames(frames: &[Frame]) -> Frame {
    frames.iter().fold([0.0; 2], |mut sum, frame| {
        sum[0] += frame[0];
        sum[1] += frame[1];
        sum
    })
}

#[test]
fn renderer_sums_shared_midi_at_receiver_and_keeps_contributor_audio_local() {
    let (f, _) = shared_fixture(&[0], &[0]);
    let project = compiled(&f);
    assert!(
        project
            .signal_graph
            .as_ref()
            .is_none_or(|graph| !graph.needs_runtime())
    );
    assert_eq!(project.tracks[0].device_chain.len(), 1);
    assert_eq!(project.tracks[1].device_chain.len(), 1);

    let output = render(&project, 30_100);
    assert_eq!(
        output[0],
        [0.5, -1.0],
        "both contributor generators reach the receiver"
    );
    assert_eq!(output[5], [0.5, -1.0], "both MIDI banks feed one generator");
    assert_eq!(output[30_000], [0.5, -1.0], "local audio remains audible");
    assert_eq!(sum_frames(&output), [4.25, -8.25]);
}

#[test]
fn renderer_return_path_receives_the_single_combined_source_output() {
    let (mut f, _) = shared_fixture(&[0], &[0]);
    let Created::ReturnTrack(bus) = f
        .editor
        .execute(Command::AddReturnTrack {
            name: "send".into(),
        })
        .unwrap()
    else {
        panic!("return track")
    };
    f.editor
        .execute(Command::SetReturnSend {
            track: f.tracks[0],
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    let project = compiled(&f);
    let output = render(&project, 30_100);
    assert_eq!(output[5], [1.0, -2.0]);
}

#[test]
fn renderer_compensated_path_routes_contributors_before_receiver_effects() {
    let (f, _) = shared_fixture(&[5], &[0]);
    let project = compiled(&f);
    assert!(
        project
            .signal_graph
            .as_ref()
            .is_some_and(|graph| graph.needs_runtime())
    );
    let output = render(&project, 30_100);
    assert_eq!(sum_frames(&output), [4.25, -8.25]);
}

#[test]
fn muting_a_contributor_suppresses_its_midi_and_local_audio_without_affecting_receiver() {
    let (mut f, _) = shared_fixture(&[0], &[0]);
    f.editor
        .execute(Command::Mute {
            track: f.tracks[1],
            muted: true,
        })
        .unwrap();
    let project = compiled(&f);
    let output = render(&project, 30_100);
    assert_eq!(output[0], [0.25, -0.5]);
    assert_eq!(output[5], [0.25, -0.5]);
    assert_eq!(output[30_000], [0.25, -0.5]);
}

#[test]
fn source_without_selected_receiver_keeps_local_audio_but_emits_no_generator() {
    let (mut f, source) = shared_fixture(&[0], &[0]);
    f.editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    let project = compiled(&f);
    let output = render(&project, 30_100);
    assert_eq!(output[30_000], [0.5, -1.0]);
    assert_eq!(output[5], [0.0, 0.0]);
}
