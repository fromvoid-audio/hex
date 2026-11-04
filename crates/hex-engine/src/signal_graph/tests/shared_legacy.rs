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
fn retained_receiver_session_mixes_printed_audio_and_peer_without_its_own_midi() {
    for latency in [0, 5] {
        for peer_count in [1, 2] {
            retained_receiver_session_at_latency(latency, peer_count, false, Selection::Normal);
        }
    }
}

#[test]
fn canonical_retained_receiver_prepares_and_renders_without_receiver_midi() {
    for latency in [0, 5] {
        for peer_count in [0, 1, 2] {
            retained_receiver_session_at_latency(latency, peer_count, true, Selection::Normal);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Selection {
    Normal,
    MuteReceiver,
    MutePeer,
    SoloReceiver,
    SoloPeer,
}

#[test]
fn canonical_retained_receiver_preserves_mute_and_solo_source_routing() {
    for latency in [0, 5] {
        for selection in [
            Selection::MuteReceiver,
            Selection::MutePeer,
            Selection::SoloReceiver,
            Selection::SoloPeer,
        ] {
            retained_receiver_session_at_latency(latency, 1, true, selection);
        }
    }
}

fn retained_receiver_session_at_latency(
    latency: u32,
    peer_count: usize,
    canonical: bool,
    selection: Selection,
) {
    let receiver_chain = [latency];
    let mut chains: Vec<&[u32]> = vec![&receiver_chain];
    chains.extend(std::iter::repeat_n(&[][..], peer_count));
    let mut f = fixture_at_rate(&chains, 32, 8_000);
    f.editor
        .execute(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(1_000_000_000).unwrap(),
        })
        .unwrap();
    let mut state = LegacyInstrumentDefinition.default_state().unwrap();
    if canonical {
        state = state.with_output_gain_millidb(-6_000).unwrap();
    }
    let source_gain = if canonical {
        state.output_gain_linear()
    } else {
        0.5
    };
    let Created::NativeSource(source) = f
        .editor
        .execute(Command::CreateNativeSource {
            name: "shared".into(),
            devices: vec![state],
            instrument_index: 0,
        })
        .unwrap()
    else {
        panic!("source")
    };
    for &track in &f.tracks {
        f.editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
    }
    match selection {
        Selection::Normal => {}
        Selection::MuteReceiver => {
            f.editor
                .execute(Command::Mute {
                    track: f.tracks[0],
                    muted: true,
                })
                .unwrap();
        }
        Selection::MutePeer => {
            f.editor
                .execute(Command::Mute {
                    track: f.tracks[1],
                    muted: true,
                })
                .unwrap();
        }
        Selection::SoloReceiver => {
            f.editor
                .execute(Command::ToggleSolo {
                    track: f.tracks[0],
                    exclusive: true,
                })
                .unwrap();
        }
        Selection::SoloPeer => {
            f.editor
                .execute(Command::ToggleSolo {
                    track: f.tracks[1],
                    exclusive: true,
                })
                .unwrap();
        }
    }
    f.editor
        .execute(Command::Gain {
            track: f.tracks[0],
            gain: 0.5,
        })
        .unwrap();
    let mut clips = vec![(
        f.tracks[0],
        BeatTime::ONE,
        BeatTime::ONE,
        vec![BeatTime::ZERO],
    )];
    clips.extend(f.tracks[1..].iter().copied().map(|track| {
        (
            track,
            BeatTime::ZERO,
            BeatTime::from_quarters(2).unwrap(),
            vec![BeatTime::ZERO, BeatTime::ONE],
        )
    }));
    for (track, start, length, notes) in clips {
        let Created::Clip(clip) = f
            .editor
            .execute(Command::PlaceMidiClip {
                track,
                start,
                length,
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        for note_start in notes {
            f.editor
                .execute(Command::AddMidiNote {
                    clip,
                    start: note_start,
                    duration: BeatTime::ONE,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                })
                .unwrap();
        }
    }
    let effects: [&dyn NativeDeviceDefinition; 1] = [&Definition];
    let instruments: [&dyn hex_lattice::NativeInstrumentDefinition; 1] =
        [&LegacyInstrumentDefinition];
    let project = if canonical {
        let mut snapshot = f.editor.project().snapshot();
        snapshot.retained_source_paths.push(f.tracks[0].value());
        hex_project::Project::from_snapshot(snapshot).unwrap()
    } else {
        f.editor.project().clone()
    };
    let mut revision = PreparedRevision::prepare_with_registries(
        &project,
        &f.media,
        Arc::new(SineTable::new()),
        None,
        &NativeDeviceRegistry::new(&effects).unwrap(),
        &hex_lattice::NativeInstrumentRegistry::new(&instruments).unwrap(),
    )
    .unwrap();
    if !canonical {
        revision
            .test_install_retained_receiver(&project, f.tracks[0])
            .unwrap();
    }
    let revision = Arc::new(revision);
    let mut expected = vec![[0.0; 2]; 500];
    let delay = latency as usize;
    if matches!(selection, Selection::Normal | Selection::SoloPeer) {
        for offset in 0..8 {
            let peer = [
                0.125 * source_gain * peer_count as f32,
                -0.25 * source_gain * peer_count as f32,
            ];
            expected[delay + offset] = peer;
            expected[480 + delay + offset] = peer;
        }
    }
    if matches!(
        selection,
        Selection::Normal | Selection::MutePeer | Selection::SoloReceiver
    ) {
        expected[delay][0] += 0.125;
        expected[delay][1] -= 0.25;
        expected[31 + delay] = [-0.0625, 0.1875];
    }
    if canonical {
        let output = render(revision.arrangement(), expected.len());
        for (frame, (actual, expected)) in output.iter().zip(&expected).enumerate() {
            for channel in 0..2 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 0.000_001,
                    "renderer latency {latency}, peers {peer_count}, selection {selection:?}, frame {frame}: {actual:?} != {expected:?}"
                );
            }
        }
    }
    for block in [7, 64, 257] {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let plan = playback
            .output_capture_plan(&[crate::OutputCaptureId::Track(f.tracks[0])], block)
            .unwrap();
        let mut output = vec![[0.0; 2]; expected.len()];
        let mut captured = output.clone();
        for (main, tap) in output.chunks_mut(block).zip(captured.chunks_mut(block)) {
            let mut captures = [tap];
            playback
                .render_with_capture(main, &mut crate::OutputCapture::new(&plan, &mut captures))
                .unwrap();
        }
        for (frame, ((main, capture), expected)) in
            output.iter().zip(&captured).zip(&expected).enumerate()
        {
            for channel in 0..2 {
                assert!(
                    (main[channel] - expected[channel]).abs() < 0.000_001,
                    "main latency {latency}, peers {peer_count}, selection {selection:?}, block {block}, frame {frame}: {main:?} != {expected:?}"
                );
                assert!(
                    (capture[channel] - expected[channel]).abs() < 0.000_001,
                    "capture latency {latency}, peers {peer_count}, selection {selection:?}, block {block}, frame {frame}: {capture:?} != {expected:?}"
                );
            }
        }
    }
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
