use hex_devices::{
    SineTable,
    utility::{UTILITY_DEFINITION, parameter},
};
use hex_engine::{
    Audio, PreparedRevision, Renderer, SessionPlayback, SessionPlaybackCommand, compile,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

struct Fixture {
    editor: Editor,
    asset: hex_project::AssetId,
    return_track: hex_project::ReturnId,
}

fn fixture(samples: &[[f32; 2]], gain: f32, send: f32, return_gain: f32) -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/return.wav".into(),
            sample_rate: 48_000,
            frames: samples.len() as u64,
        })
        .unwrap()
    {
        Created::Asset(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let track = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    {
        Created::Track(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: samples.len() as u64,
            source_offset: 0,
            source_length: samples.len() as u64,
            looping: false,
        })
        .unwrap();
    editor.execute(Command::Gain { track, gain }).unwrap();
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
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: send,
        })
        .unwrap();
    editor
        .execute(Command::ReturnGain {
            return_track,
            gain: return_gain,
        })
        .unwrap();
    Fixture {
        editor,
        asset,
        return_track,
    }
}

fn media(fixture: &Fixture, samples: &[[f32; 2]]) -> BTreeMap<hex_project::AssetId, Audio> {
    BTreeMap::from([(fixture.asset, Audio::new(48_000, samples.to_vec()).unwrap())])
}

fn add_return_device(fixture: &mut Fixture, state: hex_lattice_contract::NativeDeviceState) {
    fixture
        .editor
        .execute(Command::InsertReturnDevice {
            return_track: fixture.return_track,
            before: None,
            state,
        })
        .unwrap();
}

fn render_arrangement(fixture: &Fixture, samples: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut editor = Editor::new(fixture.editor.project().clone(), 8).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let compiled = compile::project(editor.project(), &media(fixture, samples)).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = vec![[0.0; 2]; samples.len()];
    renderer.render(&mut output).unwrap();
    output
}

#[test]
fn arrangement_send_is_post_track_fader_and_master_gain_applies_once() {
    let source = [[0.25, -0.5]];
    let mut fixture = fixture(&source, 0.5, 0.5, 1.0);
    let matrix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, matrix);
    let mut editor = Editor::new(fixture.editor.project().clone(), 8).unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let compiled = compile::project(editor.project(), &media(&fixture, &source)).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = [[0.0; 2]; 1];
    renderer.render(&mut output).unwrap();
    // Main receives [.125, -.25]. The post-fader .5 send is [.0625, -.125],
    // Utility inverts its right channel, and master .5 applies after summing.
    assert_eq!(output, [[0.09375, -0.0625]]);
}

#[test]
fn zero_send_return_preserves_the_existing_arrangement_pcm_exactly() {
    let source = [[0.1875, -0.3125]; 5];
    let with_return = fixture(&source, 0.75, 0.0, 0.5);
    let mut without_return = Editor::new(with_return.editor.project().clone(), 8).unwrap();
    without_return
        .execute(Command::RemoveReturnTrack(with_return.return_track))
        .unwrap();
    let with_returns =
        compile::project(with_return.editor.project(), &media(&with_return, &source)).unwrap();
    let without_returns =
        compile::project(without_return.project(), &media(&with_return, &source)).unwrap();
    let mut expected = Renderer::new(&without_returns);
    let mut actual = Renderer::new(&with_returns);
    expected.play();
    actual.play();
    let mut expected_output = [[0.0; 2]; 5];
    let mut actual_output = [[0.0; 2]; 5];
    expected.render(&mut expected_output).unwrap();
    actual.render(&mut actual_output).unwrap();
    assert_eq!(actual_output, expected_output);
}

#[test]
fn return_gain_and_mute_only_affect_the_return_contribution() {
    let source = [[0.25, -0.5]];
    let mut fixture = fixture(&source, 0.5, 0.5, 0.5);
    let matrix = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, matrix);
    assert_eq!(
        render_arrangement(&fixture, &source),
        [[0.078125, -0.09375]]
    );
    fixture
        .editor
        .execute(Command::ReturnMute {
            return_track: fixture.return_track,
            muted: true,
        })
        .unwrap();
    assert_eq!(render_arrangement(&fixture, &source), [[0.0625, -0.125]]);
}

#[test]
fn utility_filter_return_runs_when_sources_are_silent_and_is_partition_invariant() {
    let source = [[1.0, 1.0]; 257];
    let mut fixture = fixture(&source, 1.0, 1.0, 1.0);
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, filter);
    let compiled = compile::project(fixture.editor.project(), &media(&fixture, &source)).unwrap();
    let mut whole = Renderer::new(&compiled);
    let mut split = Renderer::new(&compiled);
    whole.play();
    split.play();
    let mut one = vec![[0.0; 2]; 320];
    let mut parts = vec![[0.0; 2]; 320];
    whole.render(&mut one).unwrap();
    for chunk in parts.chunks_mut(37) {
        split.render(chunk).unwrap();
    }
    assert_eq!(one, parts);
    assert_eq!(one[0], [2.0, 2.0]);
    assert_ne!(one[300], [0.0, 0.0]);
}

#[test]
fn session_playback_routes_arrangement_audio_through_return_and_resets_on_seek_and_stop() {
    let source = [[0.25, -0.5]; 4];
    let mut fixture = fixture(&source, 0.5, 0.5, 1.0);
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, filter);
    let prepared = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &media(&fixture, &source),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 4];
    playback.render(&mut output).unwrap();
    assert!(output.iter().all(|frame| frame[0] > 0.0 && frame[1] < 0.0));
    playback.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    let mut after_seek = [[0.0; 2]; 1];
    playback.render(&mut after_seek).unwrap();
    assert_eq!(after_seek[0], output[0]);
    playback.apply(SessionPlaybackCommand::Stop).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut after_stop = [[0.0; 2]; 1];
    playback.render(&mut after_stop).unwrap();
    assert_eq!(after_stop[0], output[0]);
}

#[test]
fn session_return_filter_history_continues_across_arrangement_loop_wrap() {
    let source = [[1.0, 0.5]; 4];
    let mut fixture = fixture(&source, 1.0, 1.0, 1.0);
    fixture
        .editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 2,
            enabled: true,
        })
        .unwrap();
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, filter);
    let prepared = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &media(&fixture, &source),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 4];
    playback.render(&mut output).unwrap();
    assert_ne!(
        output[0], output[2],
        "loop wrap incorrectly reset the return filter"
    );
}

#[test]
fn compatible_return_filter_history_survives_prepared_revision_handoff() {
    let source = [[1.0, 1.0]; 16];
    let mut fixture = fixture(&source, 1.0, 1.0, 1.0);
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, filter);
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &media(&fixture, &source),
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    );
    let mut previous = SessionPlayback::new(Arc::clone(&first_revision)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut first_block = [[0.0; 2]; 4];
    previous.render(&mut first_block).unwrap();

    let mut reference = SessionPlayback::new(first_revision).unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    reference.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut expected = [[0.0; 2]; 4];
    reference.render(&mut expected).unwrap();

    let mut revised = Editor::new(fixture.editor.project().clone(), 32).unwrap();
    revised
        .execute(Command::RenameReturnTrack {
            return_track: fixture.return_track,
            name: "renamed return".into(),
        })
        .unwrap();
    let next_revision = Arc::new(
        PreparedRevision::prepare(revised.project(), &media(&fixture, &source), table, None)
            .unwrap(),
    );
    let mut next = SessionPlayback::new(next_revision).unwrap();
    next.try_inherit_state_from(&mut previous).unwrap();
    let mut actual = [[0.0; 2]; 4];
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn removed_return_keeps_zero_fed_filter_residual_at_revision_boundary() {
    let source = [[1.0, 1.0]; 16];
    let mut fixture = fixture(&source, 1.0, 1.0, 1.0);
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, filter);
    let table = Arc::new(SineTable::new());
    let decoded = media(&fixture, &source);
    let mut previous = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(fixture.editor.project(), &decoded, Arc::clone(&table), None)
            .unwrap(),
    ))
    .unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    previous.render(&mut [[0.0; 2]; 4]).unwrap();

    // Independent oracle keeps its Return and ends the send input after the
    // same four priming frames. It never performs a revision handoff.
    let mut reference_source = source;
    reference_source[4..].fill([0.0; 2]);
    let mut reference = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &media(&fixture, &reference_source),
            Arc::clone(&table),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    reference.apply(SessionPlaybackCommand::Play).unwrap();
    reference.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut expected = [[0.0; 2]; 32];
    reference.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|value| value.abs() > 1.0e-6));
    // The real track continues its authored dry input until frame16.
    for frame in &mut expected[..12] {
        frame[0] += 1.0;
        frame[1] += 1.0;
    }

    let mut revised = Editor::new(fixture.editor.project().clone(), 32).unwrap();
    revised
        .execute(Command::RemoveReturnTrack(fixture.return_track))
        .unwrap();
    let mut next = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(revised.project(), &decoded, table, None).unwrap(),
    ))
    .unwrap();
    next.try_inherit_state_from(&mut previous).unwrap();
    let mut output = [[0.0; 2]; 32];
    next.render(&mut output).unwrap();
    assert_eq!(output, expected);
}

#[test]
fn session_audio_clip_uses_the_same_post_fader_return_path() {
    let source = [[0.25, -0.5]; 8];
    let mut fixture = fixture(&source, 0.5, 0.5, 1.0);
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::INVERT_RIGHT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut fixture, filter);
    let track = fixture.editor.project().tracks()[0].id();
    let arrangement_clip = fixture.editor.project().track(track).unwrap().clips()[0].id();
    fixture
        .editor
        .execute(Command::RemoveClip(arrangement_clip))
        .unwrap();
    assert!(
        fixture
            .editor
            .project()
            .track(track)
            .unwrap()
            .clips()
            .is_empty()
    );
    let scene = match fixture
        .editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let clip = match fixture
        .editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "session audio".into(),
            asset: fixture.asset,
            source_start: 0,
            source_end: source.len() as u64,
        })
        .unwrap()
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected result: {other:?}"),
    };
    let prepared = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &media(&fixture, &source),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut output = [[0.0; 2]; 1];
    playback.render(&mut output).unwrap();
    // Session playback keeps the default unit master gain: dry [.125, -.25]
    // plus Utility-filtered return [.0625, .125].
    assert_eq!(output[0], [0.1875, -0.125]);
}

#[test]
fn empty_return_projects_still_compile_and_unregistered_effects_refuse() {
    let source = [[0.5, -0.25]];
    let mut empty = fixture(&source, 0.5, 0.0, 1.0);
    empty
        .editor
        .execute(Command::RemoveReturnTrack(empty.return_track))
        .unwrap();
    assert!(compile::project(empty.editor.project(), &media(&empty, &source)).is_ok());

    let mut unsupported = fixture(&source, 0.5, 0.5, 1.0);
    let unknown = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("hex.unknown.effect"),
        1,
        Vec::new(),
    )
    .unwrap();
    add_return_device(&mut unsupported, unknown);
    let error =
        compile::project(unsupported.editor.project(), &media(&unsupported, &source)).unwrap_err();
    assert!(error.0.contains("unsupported or malformed"));
}
