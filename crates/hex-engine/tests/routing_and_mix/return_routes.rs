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

fn extra_return(fixture: &mut Fixture) -> hex_project::ReturnId {
    let Created::ReturnTrack(id) = fixture
        .editor
        .execute(Command::AddReturnTrack {
            name: "destination".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    id
}
fn feed(
    fixture: &mut Fixture,
    source: hex_project::ReturnId,
    destination: hex_project::ReturnId,
    gain: f32,
) {
    fixture
        .editor
        .execute(Command::SetReturnFeed {
            source,
            destination,
            gain,
        })
        .unwrap();
}
fn session(fixture: &Fixture, samples: &[[f32; 2]]) -> SessionPlayback {
    let prepared = Arc::new(
        PreparedRevision::prepare(
            fixture.editor.project(),
            &media(fixture, samples),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(prepared).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback
}
fn arrangement_pcm(fixture: &Fixture, samples: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let compiled = compile::project(fixture.editor.project(), &media(fixture, samples)).unwrap();
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut output = vec![[0.0; 2]; samples.len()];
    renderer.render(&mut output).unwrap();
    output
}

#[test]
fn authored_backward_feed_is_post_chain_gain_mute_and_pre_destination_chain() {
    let source = [[0.25, -0.5]; 8];
    let mut f = fixture(&source, 0.5, 0.0, 2.0);
    let destination = f.return_track; // Pinned before source, but must process after it.
    let source_return = extra_return(&mut f);
    let track = f.editor.project().tracks()[0].id();
    f.editor
        .execute(Command::SetReturnSend {
            track,
            return_track: source_return,
            gain: 0.5,
        })
        .unwrap();
    f.editor
        .execute(Command::ReturnGain {
            return_track: source_return,
            gain: 0.5,
        })
        .unwrap();
    for (return_track, parameter) in [
        (source_return, parameter::INVERT_RIGHT),
        (destination, parameter::INVERT_LEFT),
    ] {
        let state = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter,
                ParameterValue::Bool(true),
            )
            .unwrap();
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            })
            .unwrap();
    }
    feed(&mut f, source_return, destination, 0.5);
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let expected = vec![[0.0625, -0.0625]; source.len()];
    assert_eq!(arrangement_pcm(&f, &source), expected);
    let mut playback = session(&f, &source);
    let mut actual = vec![[0.0; 2]; source.len()];
    playback.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    f.editor
        .execute(Command::ReturnMute {
            return_track: source_return,
            muted: true,
        })
        .unwrap();
    assert_eq!(
        arrangement_pcm(&f, &source),
        vec![[0.0625, -0.125]; source.len()]
    );
    let mut playback = session(&f, &source);
    playback.render(&mut actual).unwrap();
    assert_eq!(actual, vec![[0.0625, -0.125]; source.len()]);
}

#[test]
fn diamond_and_route_record_permutation_keep_pinned_main_sum_and_deterministic_inputs() {
    let source = [[0.125, -0.25]; 13];
    let mut f = fixture(&source, 1.0, 1.0, 1.0);
    let a = f.return_track;
    let b = extra_return(&mut f);
    let c = extra_return(&mut f);
    let d = extra_return(&mut f);
    for (s, t, g) in [(a, b, 0.5), (a, c, 0.25), (b, d, 0.5), (c, d, 0.5)] {
        feed(&mut f, s, t, g);
    }
    let expected = vec![[0.390625, -0.78125]; source.len()];
    assert_eq!(arrangement_pcm(&f, &source), expected);
    let mut actual = vec![[0.0; 2]; source.len()];
    session(&f, &source).render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let mut snapshot = f.editor.project().snapshot();
    snapshot.return_feeds.reverse();
    f.editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 4).unwrap();
    assert_eq!(arrangement_pcm(&f, &source), expected);
    session(&f, &source).render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn feed_filter_history_is_block_partition_invariant_and_pause_seek_stop_are_truthful() {
    let source = [[0.25, -0.5]; 803];
    let mut f = fixture(&source, 1.0, 0.5, 1.0);
    let a = f.return_track;
    let b = extra_return(&mut f);
    for return_track in [a, b] {
        let state = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track,
                before: None,
                state,
            })
            .unwrap();
    }
    feed(&mut f, a, b, 0.5);
    let expected = arrangement_pcm(&f, &source);
    let mut whole = session(&f, &source);
    let mut partitioned = session(&f, &source);
    let mut actual = vec![[0.0; 2]; source.len()];
    whole.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let mut pieces = actual.clone();
    for chunk in pieces.chunks_mut(17) {
        partitioned.render(chunk).unwrap();
    }
    assert_eq!(pieces, expected);
    let mut paused = session(&f, &source);
    let mut reference = session(&f, &source);
    paused.render(&mut [[0.0; 2]; 7]).unwrap();
    reference.render(&mut [[0.0; 2]; 7]).unwrap();
    paused.apply(SessionPlaybackCommand::Pause).unwrap();
    let mut silence = [[1.0; 2]; 23];
    paused.render(&mut silence).unwrap();
    assert_eq!(silence, [[0.0; 2]; 23]);
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    let mut resumed = [[0.0; 2]; 11];
    let mut continued = resumed;
    paused.render(&mut resumed).unwrap();
    reference.render(&mut continued).unwrap();
    assert_eq!(resumed, continued);
    paused.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    paused.render(&mut resumed).unwrap();
    assert_eq!(resumed.as_slice(), &expected[..11]);
    paused.apply(SessionPlaybackCommand::Stop).unwrap();
    paused.apply(SessionPlaybackCommand::Play).unwrap();
    paused.render(&mut resumed).unwrap();
    assert_eq!(resumed.as_slice(), &expected[..11]);
}

#[test]
fn feed_gain_edit_retains_return_history_and_zero_removal_restores_legacy_pcm() {
    let source = [[0.25, -0.5]; 64];
    let mut f = fixture(&source, 1.0, 0.5, 1.0);
    let a = f.return_track;
    let b = extra_return(&mut f);
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut f, state);
    feed(&mut f, a, b, 0.5);
    let mut old = session(&f, &source);
    old.render(&mut [[0.0; 2]; 7]).unwrap();
    let identity = f.editor.project().authored_routing()[0].id();
    feed(&mut f, a, b, 0.25);
    assert_eq!(f.editor.project().authored_routing()[0].id(), identity);
    let mut next = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &media(&f, &source),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    let mut oracle = session(&f, &source);
    oracle.render(&mut [[0.0; 2]; 7]).unwrap();
    let mut actual = [[0.0; 2]; 19];
    let mut expected = actual;
    next.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    feed(&mut f, a, b, 0.0);
    let mut legacy = f.editor.project().snapshot();
    legacy.return_feeds.clear();
    let without = Fixture {
        editor: Editor::new(Project::from_snapshot(legacy).unwrap(), 4).unwrap(),
        asset: f.asset,
        return_track: a,
    };
    assert_eq!(
        arrangement_pcm(&f, &source),
        arrangement_pcm(&without, &source)
    );
}

#[test]
fn removed_source_tail_is_main_only_and_never_replays_its_old_feed() {
    let source = [[1.0, 0.5]; 32];
    let mut f = fixture(&source, 1.0, 1.0, 1.0);
    let a = f.return_track;
    let b = extra_return(&mut f);
    let filter = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    add_return_device(&mut f, filter);
    feed(&mut f, a, b, 1.0); // Destination is memoryless: any old replay is immediately audible.
    let mut previous = session(&f, &source);
    previous.render(&mut [[0.0; 2]; 4]).unwrap();
    // Independent no-feed oracle primes only the source chain with the same input.
    let mut no_feed = f.editor.project().snapshot();
    no_feed.return_feeds.clear();
    let oracle_fixture = Fixture {
        editor: Editor::new(Project::from_snapshot(no_feed).unwrap(), 4).unwrap(),
        asset: f.asset,
        return_track: a,
    };
    let mut ended = source;
    ended[4..].fill([0.0; 2]);
    let mut oracle = session(&oracle_fixture, &ended);
    oracle.render(&mut [[0.0; 2]; 4]).unwrap();
    let mut expected = [[0.0; 2]; 12];
    oracle.render(&mut expected).unwrap();
    for frame in &mut expected {
        frame[0] += 1.0;
        frame[1] += 0.5;
    }
    f.editor.execute(Command::RemoveReturnTrack(a)).unwrap();
    assert!(f.editor.project().authored_routing().is_empty());
    let mut next = SessionPlayback::new(Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &media(&f, &source),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    next.try_inherit_state_from(&mut previous).unwrap();
    let mut actual = [[0.0; 2]; 12];
    next.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
    assert!(f.editor.undo());
    assert_eq!(f.editor.project().authored_routing().len(), 1);
}

#[test]
fn return_feed_transport_wrap_keeps_history_and_linear_export_ignores_transport_loop() {
    let source = [[0.25, -0.5]; 600];
    let mut f = fixture(&source, 1.0, 0.5, 1.0);
    let a = f.return_track;
    let b = extra_return(&mut f);
    add_return_device(
        &mut f,
        UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap(),
    );
    feed(&mut f, a, b, 0.5);
    let expected = arrangement_pcm(&f, &source);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 2,
            enabled: true,
        })
        .unwrap();
    let mut playback = session(&f, &source);
    let mut actual = vec![[0.0; 2]; 600];
    playback.render(&mut actual).unwrap();
    // The two-frame loop alternates its full attack with a zero end
    // sample. Render that input linearly through the same routing graph:
    // return processors must retain history across every transport wrap.
    let mut enveloped_source = source;
    for frame in enveloped_source.iter_mut().skip(1).step_by(2) {
        *frame = [0.0; 2];
    }
    let loop_expected = arrangement_pcm(&f, &enveloped_source);
    assert_ne!(loop_expected, expected);
    assert_eq!(actual, loop_expected);
    let prepared = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &media(&f, &source),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut linear = SessionPlayback::new_linear_arrangement_export(prepared).unwrap();
    linear.apply(SessionPlaybackCommand::Play).unwrap();
    linear.render(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn feed_removal_keeps_destination_history_and_changed_source_tail_cannot_feed_destination() {
    for change_source in [false, true] {
        let source = [[0.25, -0.5]; 64];
        let mut f = fixture(&source, 1.0, 1.0, 1.0);
        let a = f.return_track;
        let b = extra_return(&mut f);
        let filter_owner = if change_source { a } else { b };
        let state = UTILITY_DEFINITION
            .patch_parameter(
                &UTILITY_DEFINITION.default_state().unwrap(),
                parameter::DC_REMOVE,
                ParameterValue::Bool(true),
            )
            .unwrap();
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track: filter_owner,
                before: None,
                state,
            })
            .unwrap();
        feed(&mut f, a, b, 1.0);
        let mut previous = session(&f, &source);
        previous.render(&mut [[0.0; 2]; 4]).unwrap();
        let mut oracle_snapshot = f.editor.project().snapshot();
        // For source replacement the oracle must never forward its residual.
        if change_source {
            oracle_snapshot.return_feeds.clear();
        }
        let oracle_f = Fixture {
            editor: Editor::new(Project::from_snapshot(oracle_snapshot).unwrap(), 4).unwrap(),
            asset: f.asset,
            return_track: a,
        };
        let mut ended = source;
        ended[4..].fill([0.0; 2]);
        let mut oracle = session(&oracle_f, &ended);
        oracle.render(&mut [[0.0; 2]; 4]).unwrap();
        let mut expected = [[0.0; 2]; 12];
        oracle.render(&mut expected).unwrap();
        if change_source {
            let device = f.editor.project().return_tracks()[0].devices()[0].id();
            f.editor.execute(Command::RemoveDevice(device)).unwrap();
            // Dry + current source + current destination; old source chain is Main-only.
            for frame in &mut expected {
                frame[0] += 3.0 * 0.25;
                frame[1] += 3.0 * -0.5;
            }
        } else {
            feed(&mut f, a, b, 0.0);
            // Dry + unchanged memoryless source, plus destination's retained decay.
            for frame in &mut expected {
                frame[0] += 2.0 * 0.25;
                frame[1] += 2.0 * -0.5;
            }
        }
        let mut next = SessionPlayback::new(Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &media(&f, &source),
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        ))
        .unwrap();
        next.try_inherit_state_from(&mut previous).unwrap();
        let mut actual = [[0.0; 2]; 12];
        next.render(&mut actual).unwrap();
        assert_eq!(actual, expected, "changed source={change_source}");
    }
}
#[test]
fn main_observation_is_post_master_asymmetric_and_pcm_identical() {
    let samples = [[0.8, -0.2]; 384];
    let mut f = fixture(&samples, 0.5, 0.25, 0.5);
    add_return_device(&mut f, UTILITY_DEFINITION.default_state().unwrap());
    f.editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let bank = media(&f, &samples);
    let compiled = compile::project(f.editor.project(), &bank).unwrap();
    for metered in [false, true] {
        let mut actual = Renderer::new(&compiled);
        let mut plain = Renderer::new(&compiled);
        actual.set_effect_metering(metered);
        plain.set_effect_metering(metered);
        actual.play();
        plain.play();
        let mut output = [[9.0; 2]; 384];
        let mut expected = output;
        let mut observed = output;
        let mut calls = 0;
        actual
            .render_with_main_observer(
                &mut output,
                Some(&mut |frames: &[[f32; 2]]| {
                    calls += 1;
                    observed.copy_from_slice(frames);
                }),
            )
            .unwrap();
        plain.render(&mut expected).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(observed, output);
        assert_eq!(output, expected);
        assert_eq!(actual.position(), 384);
        for frame in output {
            assert!((frame[0] - 0.225).abs() < 1e-7);
            assert!((frame[1] + 0.05625).abs() < 1e-7);
        }
    }
    let prepared = Arc::new(
        PreparedRevision::prepare(f.editor.project(), &bank, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    let mut actual = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut plain = SessionPlayback::new(prepared).unwrap();
    actual.apply(SessionPlaybackCommand::Play).unwrap();
    plain.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[9.0; 2]; 384];
    let mut expected = output;
    let mut observed = output;
    let mut calls = 0;
    let mut nodes = 0;
    actual
        .render_with_post_node_and_main_observers(
            &mut output,
            Some(&mut |_, _, _: &[[f32; 2]]| nodes += 1),
            Some(&mut |frames: &[[f32; 2]]| {
                calls += 1;
                observed.copy_from_slice(frames);
            }),
        )
        .unwrap();
    plain.render(&mut expected).unwrap();
    assert!(nodes > 0);
    assert_eq!(calls, 1);
    assert_eq!(observed, output);
    assert_eq!(output, expected);
    assert_eq!(actual.performance_frame(), 384);
    for frame in output {
        assert!((frame[0] - 0.225).abs() < 1e-7);
        assert!((frame[1] + 0.05625).abs() < 1e-7);
    }
}
#[test]
fn main_observer_none_pause_empty_and_overflow_keep_existing_render_law() {
    let samples = [[0.8, -0.2]; 4];
    let f = fixture(&samples, 1.0, 0.0, 1.0);
    let compiled = compile::project(f.editor.project(), &media(&f, &samples)).unwrap();
    let mut renderer = Renderer::new(&compiled);
    let mut output = [[9.0; 2]; 2];
    let mut calls = 0;
    renderer
        .render_with_main_observer(
            &mut output,
            Some(&mut |frames: &[[f32; 2]]| {
                assert_eq!(frames, &[[0.0; 2]; 2]);
                calls += 1;
            }),
        )
        .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(renderer.position(), 0);
    renderer
        .render_with_main_observer(
            &mut [],
            Some(&mut |frames: &[[f32; 2]]| {
                assert!(frames.is_empty());
                calls += 1;
            }),
        )
        .unwrap();
    assert_eq!(calls, 2);
    renderer.play();
    renderer
        .render_with_main_observer(&mut output, None)
        .unwrap();
    assert_eq!(renderer.position(), 2);
    renderer.seek(u64::MAX);
    let original = output;
    assert!(
        renderer
            .render_with_main_observer(&mut output, Some(&mut |_: &[[f32; 2]]| calls += 1))
            .is_err()
    );
    assert_eq!(calls, 2);
    assert_eq!(output, original);
    assert_eq!(renderer.position(), u64::MAX);
}
