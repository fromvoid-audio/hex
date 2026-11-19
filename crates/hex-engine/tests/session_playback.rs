use hex_devices::utility::{UTILITY_DEFINITION, parameter as utility_parameter};
use hex_engine::playback::{Playback, PlaybackHandoffError};
use hex_engine::{
    Audio, LaunchQuantization, MidiInstrumentRenderContext, MidiSpanBuffer, PlaybackAction,
    PlaybackActionError, PlaybackCommand, PreparedRevision, SessionCapability, SessionPlayback,
    SessionPlaybackCommand, SessionPlaybackError, SessionPlaybackOwner, SineTable,
};
use hex_lattice_contract::NativeDeviceMetadata;
use hex_project::{AssetId, BeatTime, Command, Created, Editor, Project, Tempo};
use std::{collections::BTreeMap, sync::Arc};

fn beat(value: u64) -> BeatTime {
    BeatTime::from_quarters(value).expect("test beat is in range")
}

struct Fixture {
    project: Project,
    revision: Arc<PreparedRevision>,
    track: hex_project::TrackId,
    scene: hex_project::SceneId,
    empty_scene: hex_project::SceneId,
    stop_scene: hex_project::SceneId,
    session_clip: hex_project::ClipId,
}

fn fixture(loop_region: Option<(u64, u64)>, multi_tempo_at: Option<u64>) -> Fixture {
    fixture_with_session_note_duration(loop_region, multi_tempo_at, beat(4))
}

fn fixture_with_session_note_duration(
    loop_region: Option<(u64, u64)>,
    multi_tempo_at: Option<u64>,
    session_note_duration: BeatTime,
) -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("instrument track")
    {
        Created::Track(track) => track,
        _ => panic!("expected track"),
    };
    let arrangement_clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: beat(8),
        })
        .expect("arrangement clip")
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected arrangement clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement_clip,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("arrangement note");
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .expect("scene")
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected scene"),
    };
    let session_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session keys".into(),
            source_length: beat(4),
        })
        .expect("session clip")
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    let empty_scene = match editor
        .execute(Command::AddScene {
            name: "empty".into(),
        })
        .expect("empty scene")
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected empty scene"),
    };
    let stop_scene = match editor
        .execute(Command::AddScene {
            name: "stop".into(),
        })
        .expect("stop scene")
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected stop scene"),
    };
    editor
        .execute(Command::SetSessionStopCell {
            track,
            scene: stop_scene,
        })
        .expect("stop cell");
    editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: session_note_duration,
            pitch: 64,
            channel: 0,
            velocity: 96,
        })
        .expect("Session note");
    if let Some(position) = multi_tempo_at {
        editor
            .execute(Command::SetTempoPoint {
                beat: beat(position),
                tempo: Tempo::new(90_000_000).expect("tempo"),
            })
            .expect("tempo marker");
    }
    // The loop is given in frames, so set it after the tempo marker: a tempo
    // edit keeps an existing loop brace on its beats and would move its end.
    if let Some((start, end)) = loop_region {
        editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled: true,
            })
            .expect("loop region");
    }
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepared revision"),
    );
    Fixture {
        project: editor.project().clone(),
        revision,
        track,
        scene,
        empty_scene,
        stop_scene,
        session_clip,
    }
}

#[test]
fn session_audio_and_midi_routes_use_the_prepared_track_device_matrix() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let audio_track = match editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .expect("audio track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected audio track: {other:?}"),
    };
    let midi_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "midi".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("midi track")
    {
        Created::Track(id) => id,
        other => panic!("unexpected midi track: {other:?}"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "both".into(),
        })
        .expect("scene")
    {
        Created::Scene(id) => id,
        other => panic!("unexpected scene: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/session-device.wav".into(),
            sample_rate: 48_000,
            frames: 256,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        other => panic!("unexpected asset: {other:?}"),
    };
    let audio_clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track: audio_track,
            scene,
            name: "audio one shot".into(),
            asset,
            source_start: 0,
            source_end: 256,
        })
        .expect("session audio")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected audio clip: {other:?}"),
    };
    let midi_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track: midi_track,
            scene,
            name: "midi one shot".into(),
            source_length: beat(4),
        })
        .expect("session midi")
    {
        Created::SessionClip(id) => id,
        other => panic!("unexpected midi clip: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 64,
            channel: 0,
            velocity: 100,
        })
        .expect("session note");
    let utility = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().expect("utility default"),
            utility_parameter::INVERT_RIGHT,
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .expect("utility parameter");
    editor
        .execute(Command::InsertDevice {
            track: audio_track,
            before: None,
            state: utility.clone(),
        })
        .expect("audio utility");
    editor
        .execute(Command::InsertDevice {
            track: midi_track,
            before: None,
            state: utility,
        })
        .expect("midi utility");
    let media = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.25, 0.25]; 256]).expect("audio source"),
    )]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .expect("prepared revision"),
    );
    let mut playback = SessionPlayback::new(revision).expect("session playback");
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch both sources");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut output = [[0.0; 2]; 128];
    playback
        .render(&mut output)
        .expect("render session sources");
    assert!(output.iter().any(|frame| frame[0].abs() > 1.0e-5));
    assert!(output.iter().all(|frame| frame[1] == -frame[0]));
    assert_eq!(
        playback.owner(audio_track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
    assert_eq!(
        playback.owner(midi_track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
}

fn audible(output: &[[f32; 2]]) -> bool {
    output.iter().flatten().any(|sample| sample.abs() > 1.0e-6)
}

#[test]
fn one_hybrid_track_switches_audio_midi_audio_and_releases_the_midi_tail() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 16).expect("editor");
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "hybrid".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("hybrid track")
    {
        Created::Track(track) => track,
        other => panic!("unexpected track: {other:?}"),
    };
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/session-audio.wav".into(),
            sample_rate: 48_000,
            frames: 4_096,
        })
        .expect("audio asset")
    {
        Created::Asset(asset) => asset,
        other => panic!("unexpected asset: {other:?}"),
    };
    let audio_scene = match editor
        .execute(Command::AddScene {
            name: "audio".into(),
        })
        .expect("audio scene")
    {
        Created::Scene(scene) => scene,
        other => panic!("unexpected scene: {other:?}"),
    };
    let midi_scene = match editor
        .execute(Command::AddScene {
            name: "midi".into(),
        })
        .expect("midi scene")
    {
        Created::Scene(scene) => scene,
        other => panic!("unexpected scene: {other:?}"),
    };
    let audio_clip = match editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene: audio_scene,
            name: "audio one shot".into(),
            asset,
            source_start: 0,
            source_end: 4_096,
        })
        .expect("audio cell on hybrid track")
    {
        Created::SessionClip(clip) => clip,
        other => panic!("unexpected audio clip: {other:?}"),
    };
    let midi_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene: midi_scene,
            name: "midi phrase".into(),
            source_length: beat(4),
        })
        .expect("midi cell on hybrid track")
    {
        Created::SessionClip(clip) => clip,
        other => panic!("unexpected midi clip: {other:?}"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: midi_clip,
            start: BeatTime::ZERO,
            duration: beat(2),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .expect("midi note");
    let media = BTreeMap::from([(
        asset,
        Audio::new(48_000, vec![[0.25, 0.25]; 4_096]).expect("resident source"),
    )]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .expect("prepared hybrid revision"),
    );

    let mut playback = SessionPlayback::new(Arc::clone(&revision)).expect("playback");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch audio");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut audio = [[0.0; 2]; 64];
    playback.render(&mut audio).expect("render audio");
    assert!(audible(&audio));
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );

    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: midi_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("switch to midi");
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip: midi_clip })
    );
    let mut midi = [[0.0; 2]; 128];
    playback.render(&mut midi).expect("render midi");
    assert!(audible(&midi));

    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: audio_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("switch back to audio");
    assert!(playback.release_voice_count() > 0);
    let mut transition = vec![[0.0; 2]; 3_000];
    playback
        .render(&mut transition)
        .expect("render midi release tail");
    assert!(audible(&transition[..500]));
    assert!(
        transition[2_700..]
            .iter()
            .all(|frame| *frame == [0.25, 0.25])
    );
    assert_eq!(playback.release_voice_count(), 0);
    assert_eq!(
        playback.owner(track),
        Some(SessionPlaybackOwner::Session { clip: audio_clip })
    );
}

#[test]
fn routes_switch_sources_without_releasing_an_unchanged_arrangement_voice() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut first = [[0.0; 2]; 64];
    playback.render(&mut first).expect("Arrangement render");
    assert!(audible(&first));

    playback
        .apply(SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        })
        .expect("no-op Arrangement return");
    assert_eq!(playback.release_voice_count(), 0);

    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch Session clip");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );
    assert_eq!(playback.release_voice_count(), 1);
    let mut switched = [[0.0; 2]; 128];
    playback.render(&mut switched).expect("mixed transition");
    assert!(audible(&switched));

    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("stop track source");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(playback.release_voice_count(), 2);
    let mut release = vec![[0.0; 2]; 2_500];
    playback.render(&mut release).expect("release render");
    assert!(audible(&release[..1_000]));
    assert!(!audible(&release[2_450..]));
    assert_eq!(playback.release_voice_count(), 0);
}

#[test]
fn transport_stop_keeps_owner_and_reanchors_session_for_resume() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select Session source while paused");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut output = [[0.0; 2]; 31];
    playback.render(&mut output).expect("render");
    playback
        .apply(SessionPlaybackCommand::Stop)
        .expect("transport stop");
    assert_eq!(playback.performance_frame(), 0);
    assert_eq!(playback.arrangement_frame(), 0);
    assert_eq!(playback.release_voice_count(), 0);
    assert!(!playback.playing());
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );
    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("resume");
    playback.render(&mut output).expect("restarted render");
    assert_eq!(playback.performance_frame(), 31);
    assert!(audible(&output));
}

#[test]
fn pause_empty_render_stop_all_and_seek_have_distinct_transport_effects() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select source");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut output = [[0.0; 2]; 31];
    playback.render(&mut output).expect("initial render");
    assert_eq!(playback.performance_frame(), 31);

    let mut released = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("release oracle");
    released
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    released.apply(SessionPlaybackCommand::Play).unwrap();
    released.render(&mut [[0.0; 2]; 31]).unwrap();
    released
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    playback
        .apply(SessionPlaybackCommand::Pause)
        .expect("pause");
    output.fill([7.0; 2]);
    playback.render(&mut output).expect("paused render");
    let mut expected_release = [[0.0; 2]; 31];
    released.render(&mut expected_release).unwrap();
    assert_eq!(
        output, expected_release,
        "pause releases notes through the existing source instead of freezing its audio"
    );
    let frame = playback.performance_frame();
    playback.render(&mut []).expect("empty render");
    assert_eq!(playback.performance_frame(), frame);
    assert!(!playback.playing());

    playback
        .apply(SessionPlaybackCommand::Play)
        .expect("resume");
    playback
        .apply(SessionPlaybackCommand::StopAllClips {
            quantization: LaunchQuantization::Immediate,
        })
        .expect("stop all clip routes");
    assert!(playback.playing());
    assert_eq!(playback.performance_frame(), frame);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );
    playback
        .apply(SessionPlaybackCommand::Seek(24_000))
        .expect("seek while playing");
    assert!(playback.playing());
    assert_eq!(playback.performance_frame(), 24_000);
    assert_eq!(playback.arrangement_frame(), 24_000);
    assert_eq!(playback.release_voice_count(), 0);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );
}

#[test]
fn play_from_seeks_and_starts_as_one_transport_action() {
    let fixture = fixture(None, None);
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision))
        .expect("prepared session playback");

    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(24_000)))
        .expect("play from target");
    assert!(playback.playing());
    assert_eq!(playback.position(), 24_000);
    let mut output = [[0.0; 2]; 13];
    playback.render(&mut output).expect("render from target");
    assert_eq!(playback.position(), 24_013);

    playback.apply(PlaybackCommand::Pause);
    assert!(!playback.playing());
    assert_eq!(playback.position(), 24_013);
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(48_000)))
        .expect("restart from target");
    assert!(playback.playing());
    assert_eq!(playback.position(), 48_000);
}

#[test]
fn play_from_updates_legacy_arrangement_position_before_starting() {
    let project = Arc::new(hex_engine::Project::new(48_000, Vec::new()).expect("project"));
    let mut playback = Playback::from_shared(project);
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(7_200)))
        .expect("legacy play from");
    assert!(playback.playing());
    assert_eq!(playback.position(), 7_200);
    assert_eq!(playback.performance_frame(), 7_200);
}

#[test]
fn rejected_play_from_does_not_start_or_partially_seek_session() {
    let fixture = fixture(None, None);
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision))
        .expect("prepared session playback");
    playback
        .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(12_000)))
        .expect("establish position");
    let previous = playback.position();

    assert!(
        playback
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(
                u64::MAX
            )))
            .is_err()
    );
    assert!(!playback.playing());
    assert_eq!(playback.position(), previous);
}

#[test]
fn sparse_scene_absence_is_noop_while_an_explicit_stop_cell_silences() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch clip");
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene: fixture.empty_scene,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch sparse empty scene");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene: fixture.stop_scene,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch explicit Stop scene");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );
}

#[test]
fn four_quarter_launch_replaces_pending_action_per_track_and_exact_boundary_is_immediate() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::Seek(24_000))
        .expect("seek to beat one");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("queue launch");
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("replace queued launch");
    let mut to_boundary = vec![[0.0; 2]; 72_000];
    playback
        .render(&mut to_boundary)
        .expect("render to boundary");
    assert_eq!(playback.performance_frame(), 96_000);
    let mut boundary = [[0.0; 2]; 1];
    playback
        .render(&mut boundary)
        .expect("apply boundary route");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );

    playback
        .apply(SessionPlaybackCommand::Seek(96_000))
        .expect("seek to exact four-quarter boundary");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("exact-boundary launch applies immediately while playing");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );
}

#[test]
fn compensated_arrangement_midi_matches_single_frame_through_loop_wraps() {
    let fixture = fixture(Some((0, 257)), None);
    let mut editor = Editor::new(fixture.project, 16).expect("editor");
    let clip = editor.project().track(fixture.track).unwrap().midi_clips()[0].id();
    let volume = hex_project::MidiClipVolume::new(
        vec![
            hex_project::MidiClipVolumePoint::linear(
                BeatTime::ZERO,
                hex_project::MidiClipGain::new(0.25).unwrap(),
            ),
            hex_project::MidiClipVolumePoint::linear(
                beat(4),
                hex_project::MidiClipGain::new(1.0).unwrap(),
            ),
        ],
        beat(8),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume { clip, volume })
        .unwrap();
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .expect("limiter state");
    editor
        .execute(Command::InsertDevice {
            track: fixture.track,
            before: None,
            state: limiter,
        })
        .expect("track limiter");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("revision"),
    );
    let render = |partition| {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).expect("playback");
        playback.apply(SessionPlaybackCommand::Play).expect("play");
        let mut output = [[0.0; 2]; 1024];
        for chunk in output.chunks_mut(partition) {
            playback.render(chunk).expect("render");
        }
        (
            output,
            playback.performance_frame(),
            playback.arrangement_frame(),
        )
    };
    let expected = render(1);
    assert!(audible(&expected.0));
    assert_eq!((expected.1, expected.2), (1024, 253));
    for partition in [17, 64, 256, 512] {
        assert_eq!(render(partition), expected, "partition {partition}");
    }
}

#[test]
fn compensated_arrangement_midi_capture_matches_single_frame() {
    let fixture = fixture(None, None);
    let mut editor = Editor::new(fixture.project, 16).unwrap();
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track: fixture.track,
            before: None,
            state: limiter,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let render = |partition| {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let plan = playback
            .output_capture_plan(
                &[hex_engine::session_playback::OutputCaptureId::Track(
                    fixture.track,
                )],
                partition,
            )
            .unwrap();
        let mut output = [[0.0; 2]; 512];
        let mut captured = output;
        for (chunk, captured_chunk) in output
            .chunks_mut(partition)
            .zip(captured.chunks_mut(partition))
        {
            let mut buffers: [&mut [hex_engine::Frame]; 1] = [captured_chunk];
            let mut capture = hex_engine::session_playback::OutputCapture::new(&plan, &mut buffers);
            playback.render_with_capture(chunk, &mut capture).unwrap();
        }
        (
            output,
            captured,
            playback.performance_frame(),
            playback.arrangement_frame(),
        )
    };
    let expected = render(1);
    assert!(audible(&expected.0));
    assert!(audible(&expected.1));
    for partition in [17, 64, 256, 512] {
        assert_eq!(render(partition), expected, "partition {partition}");
    }
}

#[test]
fn compensated_session_midi_matches_single_frame_across_tempo_and_relaunch() {
    let fixture = fixture(Some((0, 48_013)), Some(1));
    let mut editor = Editor::new(fixture.project, 16).unwrap();
    let volume = hex_project::MidiClipVolume::new(
        vec![
            hex_project::MidiClipVolumePoint::linear(
                BeatTime::ZERO,
                hex_project::MidiClipGain::new(0.25).unwrap(),
            ),
            hex_project::MidiClipVolumePoint::linear(
                beat(4),
                hex_project::MidiClipGain::new(1.0).unwrap(),
            ),
        ],
        beat(4),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: fixture.session_clip,
            volume,
        })
        .unwrap();
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    editor
        .execute(Command::InsertDevice {
            track: fixture.track,
            before: None,
            state: limiter,
        })
        .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let render = |partition| {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback
            .apply(SessionPlaybackCommand::Seek(23_990))
            .unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        let launch = SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        };
        playback.apply(launch).unwrap();
        let mut output = [[0.0; 2]; 1024];
        for half in output.chunks_mut(512) {
            for chunk in half.chunks_mut(partition) {
                playback.render(chunk).unwrap();
            }
            playback.apply(launch).unwrap();
        }
        (
            output,
            playback.performance_frame(),
            playback.arrangement_frame(),
        )
    };
    let expected = render(1);
    assert!(audible(&expected.0));
    for partition in [17, 64, 256, 512] {
        assert_eq!(render(partition), expected, "partition {partition}");
    }
}

#[test]
fn compensated_pending_launch_crosses_its_boundary_at_the_same_sample_for_each_partition() {
    let fixture = fixture(None, None);
    let mut editor = Editor::new(fixture.project, 16).expect("editor");
    let limiter = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
    )
    .expect("limiter state");
    editor
        .execute(Command::InsertDevice {
            track: fixture.track,
            before: None,
            state: limiter,
        })
        .expect("track limiter");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepared revision"),
    );
    let render = |partition| {
        let mut playback = SessionPlayback::new(Arc::clone(&revision)).expect("kernel");
        playback
            .apply(SessionPlaybackCommand::Seek(95_990))
            .expect("seek");
        playback.apply(SessionPlaybackCommand::Play).expect("play");
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: fixture.session_clip,
                quantization: LaunchQuantization::FourQuarter,
            })
            .expect("queue launch");
        let mut output = [[0.0; 2]; 512];
        for block in output.chunks_mut(partition) {
            playback.render(block).expect("render");
        }
        assert_eq!(
            playback.owner(fixture.track),
            Some(SessionPlaybackOwner::Session {
                clip: fixture.session_clip
            })
        );
        output
    };
    let expected = render(1);
    assert!(audible(&expected));
    for partition in [17, 64, 256, 512] {
        assert_eq!(render(partition), expected);
    }
}

#[test]
fn release_capacity_reports_the_shared_preallocated_bank() {
    let fixture = fixture(None, None);
    let playback = SessionPlayback::new(fixture.revision).expect("kernel");
    assert_eq!(
        playback.release_voice_capacity(),
        hex_project::MAX_TRACKS * 64
    );
    assert_eq!(
        playback.release_storage_capacity(),
        hex_project::MAX_TRACKS * 64
    );
}

#[test]
fn inherited_tail_occupancy_does_not_block_another_tracks_pending_transition() {
    let fixture = fixture(None, None);
    let mut editor = Editor::new(fixture.project, 32).expect("editor from prepared fixture");
    let other_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "other keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("second instrument track")
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let arrangement_clip = match editor
        .execute(Command::PlaceMidiClip {
            track: other_track,
            start: BeatTime::ZERO,
            length: beat(8),
        })
        .expect("second Arrangement clip")
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected Arrangement clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement_clip,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 58,
            channel: 0,
            velocity: 100,
        })
        .expect("second Arrangement note");
    let other_session = match editor
        .execute(Command::CreateSessionMidiClip {
            track: other_track,
            scene: fixture.scene,
            name: "other session".into(),
            source_length: beat(4),
        })
        .expect("second Session clip")
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: other_session,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 67,
            channel: 0,
            velocity: 96,
        })
        .expect("second Session note");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("two-track revision"),
    );
    let mut playback = SessionPlayback::new(revision).expect("two-track kernel");
    playback
        .apply(SessionPlaybackCommand::Seek(120_000))
        .expect("seek beyond Arrangement notes");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("queue first track transition");
    let pending = playback
        .route_status(0)
        .expect("first track route")
        .pending
        .expect("queued first-track stop");
    assert_eq!(pending.performance_frame, 192_000);
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: other_session,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select second Session source");
    for _ in 0..64 {
        let mut onset = [[0.0; 2]; 1];
        playback
            .render(&mut onset)
            .expect("sound second-track onset");
        playback
            .apply(SessionPlaybackCommand::StopTrack {
                track: other_track,
                quantization: LaunchQuantization::Immediate,
            })
            .expect("capture second-track release");
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: other_session,
                quantization: LaunchQuantization::Immediate,
            })
            .expect("relaunch second Session source");
    }
    assert_eq!(playback.release_voice_count(), 64);
    let mut onset = [[0.0; 2]; 1];
    playback
        .render(&mut onset)
        .expect("sound final second-track onset");
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: other_track,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("global bank has room beyond this track's carried tails");
    assert_eq!(playback.release_voice_count(), 65);
    assert_eq!(
        playback.owner(other_track),
        Some(SessionPlaybackOwner::Silence)
    );

    assert_eq!(
        playback.route_status(0).expect("first track route").pending,
        Some(pending)
    );
    assert_eq!(playback.performance_frame(), 120_065);
    let remaining = usize::try_from(192_001 - playback.performance_frame())
        .expect("remaining frames fit the fixture buffer");
    let mut to_pending_frame = vec![[0.0; 2]; remaining];
    playback
        .render(&mut to_pending_frame)
        .expect("expire reservations and apply pending transition");
    assert!(
        to_pending_frame
            .iter()
            .take(128)
            .any(|frame| frame[0].abs() > 1.0e-6)
    );
    assert_eq!(to_pending_frame.last(), Some(&[0.0; 2]));
    assert_eq!(playback.performance_frame(), 192_001);
    assert_eq!(playback.release_voice_count(), 0);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(
        playback.owner(other_track),
        Some(SessionPlaybackOwner::Silence)
    );
}

#[test]
fn outgoing_voice_order_remains_partition_exact_after_repeated_same_track_cuts() {
    let fixture = fixture(None, None);
    let mut editor = Editor::new(fixture.project, 32).expect("editor");
    editor
        .execute(Command::AddMidiNote {
            clip: fixture.session_clip,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 72,
            channel: 0,
            velocity: 9,
        })
        .expect("second voice");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("two-voice revision"),
    );
    let mut whole = SessionPlayback::new(Arc::clone(&revision)).expect("whole kernel");
    let mut split = SessionPlayback::new(revision).expect("split kernel");
    for playback in [&mut whole, &mut split] {
        playback
            .apply(SessionPlaybackCommand::Seek(120_000))
            .expect("seek past Arrangement MIDI");
        playback
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: fixture.session_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .expect("select Session source");
        playback.apply(SessionPlaybackCommand::Play).expect("play");
        for _ in 0..8 {
            let mut onset = [[0.0; 2]; 1];
            playback
                .render(&mut onset)
                .expect("sound both source onsets");
            playback
                .apply(SessionPlaybackCommand::StopTrack {
                    track: fixture.track,
                    quantization: LaunchQuantization::Immediate,
                })
                .expect("cut active voices");
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: fixture.session_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .expect("relaunch source");
        }
        assert_eq!(playback.release_voice_count(), 16);
    }
    let mut expected = vec![[0.0; 2]; 128];
    let mut actual = expected.clone();
    whole.render(&mut expected).expect("whole render");
    for chunk in actual.chunks_mut(31) {
        split.render(chunk).expect("partitioned render");
    }
    assert_eq!(actual, expected);
}

#[test]
fn held_voice_release_uses_the_original_phase_after_a_route_cut() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::Seek(120_000))
        .expect("seek past the Arrangement note");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select Session clip");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut before_cut = [[0.0; 2]; 100];
    playback.render(&mut before_cut).expect("render held note");
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("cut held note");

    let voice = fixture
        .revision
        .session()
        .sine_table()
        .prepare_voice(48_000, 64, 96)
        .expect("prepared fixture voice");
    let expected = fixture
        .revision
        .session()
        .sine_table()
        .sample(voice, 100, 100);
    let mut tail = [[0.0; 2]; 1];
    playback.render(&mut tail).expect("first release frame");
    assert_eq!(tail[0], [expected; 2]);
}

#[test]
fn replacing_an_already_releasing_voice_preserves_its_original_end_frame() {
    let fixture = fixture(None, None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::Seek(96_000))
        .expect("seek to beat four after Arrangement gate");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select Session clip");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut through_note_off = vec![[0.0; 2]; 96_100];
    playback
        .render(&mut through_note_off)
        .expect("render through and beyond note-off");
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("preserve active release tail");
    assert_eq!(playback.release_voice_count(), 1);

    let mut before_original_end = vec![[0.0; 2]; 2_299];
    playback
        .render(&mut before_original_end)
        .expect("render remaining release");
    assert_eq!(playback.release_voice_count(), 1);
    let mut original_end = [[0.0; 2]; 1];
    playback
        .render(&mut original_end)
        .expect("render original exclusive end");
    assert_eq!(playback.release_voice_count(), 0);
}

#[test]
fn late_chunk_midi_domain_refusal_zeros_output_and_rolls_back_runtime_state() {
    let duration = BeatTime::from_raw(1_u64 << 24);
    let fixture = fixture_with_session_note_duration(None, None, duration);
    let mut previous = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    let timing = fixture.revision.session().timing();
    let last_beat_frame = timing
        .beat_to_frame(BeatTime::from_raw(u64::MAX))
        .expect("maximum beat frame");
    let start = last_beat_frame - 255;
    previous
        .apply(SessionPlaybackCommand::Seek(start))
        .expect("seek near beat-domain edge");
    previous
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch near beat-domain edge");
    previous.apply(SessionPlaybackCommand::Play).expect("play");
    let mut first_chunk = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    first_chunk
        .try_inherit_state_from(&mut previous)
        .expect("prepare inherited state");
    let mut successful_chunk = [[0.0; 2]; 256];
    first_chunk
        .render(&mut successful_chunk)
        .expect("first 256-frame query succeeds before domain exhaustion");
    assert_eq!(first_chunk.performance_frame(), start + 256);

    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .try_inherit_state_from(&mut previous)
        .expect("prepare inherited state");
    let mut output = [[9.0; 2]; 512];
    assert_eq!(
        playback.render(&mut output),
        Err(SessionPlaybackError::Midi(
            hex_engine::MidiTemplateError::BeatDomainExhausted
        ))
    );
    assert_eq!(output, [[0.0; 2]; 512]);
    assert_eq!(playback.performance_frame(), start);
    assert_eq!(playback.arrangement_frame(), start);
    assert!(playback.playing());
    assert_eq!(playback.release_voice_count(), 0);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );
}

#[test]
fn a_late_chunk_failure_rolls_back_a_due_pending_route_and_zeros_earlier_audio() {
    let duration = BeatTime::from_raw(1_u64 << 24);
    let fixture = fixture_with_session_note_duration(None, None, duration);
    let mut editor = Editor::new(fixture.project, 32).expect("editor");
    let other_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "second keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("second instrument track")
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let arrangement_clip = match editor
        .execute(Command::PlaceMidiClip {
            track: other_track,
            start: BeatTime::ZERO,
            length: beat(8),
        })
        .expect("Arrangement clip")
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected Arrangement clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement_clip,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 58,
            channel: 0,
            velocity: 100,
        })
        .expect("Arrangement note");
    let other_session = match editor
        .execute(Command::CreateSessionMidiClip {
            track: other_track,
            scene: fixture.scene,
            name: "second source".into(),
            source_length: beat(4),
        })
        .expect("Session source")
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: other_session,
            start: BeatTime::ZERO,
            duration,
            pitch: 67,
            channel: 0,
            velocity: 96,
        })
        .expect("short Session note");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("two-source revision"),
    );
    let beat_domain_end = revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(u64::MAX))
        .expect("last beat frame");
    let last_boundary_raw = (u64::MAX / (4_u64 << 32)) * (4_u64 << 32);
    let boundary_frame = revision
        .session()
        .timing()
        .beat_to_frame(BeatTime::from_raw(last_boundary_raw))
        .expect("last four-quarter boundary");
    let start = boundary_frame - 128;

    let mut prefix = SessionPlayback::new(Arc::clone(&revision)).expect("prefix kernel");
    prefix
        .apply(SessionPlaybackCommand::Seek(start))
        .expect("seek before last boundary");
    prefix
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select first source");
    prefix
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: other_session,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select second source");
    prefix.apply(SessionPlaybackCommand::Play).expect("play");
    prefix
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("queue transition at last boundary");
    let mut first_chunk = [[0.0; 2]; 256];
    prefix
        .render(&mut first_chunk)
        .expect("first chunk and pending transition are valid");
    assert!(audible(&first_chunk));
    assert_eq!(
        prefix.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );

    let mut playback = SessionPlayback::new(Arc::clone(&revision)).expect("failure kernel");
    playback
        .apply(SessionPlaybackCommand::Seek(start))
        .expect("seek before last boundary");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select first source");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: other_session,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("select second source");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    playback
        .apply(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("queue transition at last boundary");
    let width = usize::try_from(beat_domain_end + 512 - start).expect("bounded test render");
    let mut output = vec![[5.0; 2]; width];
    let render_result = playback.render(&mut output);
    assert_eq!(
        render_result,
        Err(SessionPlaybackError::Midi(
            hex_engine::MidiTemplateError::BeatDomainExhausted
        ))
    );
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(playback.performance_frame(), start);
    assert_eq!(playback.arrangement_frame(), start);
    assert_eq!(playback.release_voice_count(), 0);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );

    let mut reach_pending = [[0.0; 2]; 129];
    playback
        .render(&mut reach_pending)
        .expect("rolled-back pending action remains schedulable");
    assert_eq!(reach_pending, first_chunk[..129]);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert_eq!(playback.release_voice_count(), prefix.release_voice_count());
}

#[test]
fn mixed_audio_session_midi_and_explicit_silence_match_independent_source_composition() {
    let mut editor = Editor::new(Project::new(48_000).expect("project"), 32).expect("editor");
    let (audio_track, asset): (_, AssetId) = match editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 192_000,
        })
        .expect("audio import")
    {
        Created::Imported { track, asset, .. } => (track, asset),
        _ => panic!("expected imported audio track"),
    };
    editor
        .execute(Command::Gain {
            track: audio_track,
            gain: 0.5,
        })
        .expect("audio gain");
    editor
        .execute(Command::Balance {
            track: audio_track,
            balance: 0.25,
        })
        .expect("audio balance");
    let session_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "session midi".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("Session instrument track")
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let stopped_track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "stopped midi".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("silenced instrument track")
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    for track in [session_track, stopped_track] {
        let clip = match editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: beat(4),
            })
            .expect("Arrangement MIDI clip")
        {
            Created::Clip(clip) => clip,
            _ => panic!("expected Arrangement MIDI clip"),
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: beat(4),
                pitch: 55,
                channel: 0,
                velocity: 100,
            })
            .expect("Arrangement MIDI note");
    }
    let scene = match editor
        .execute(Command::AddScene {
            name: "mixed".into(),
        })
        .expect("scene")
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected scene"),
    };
    let session_clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track: session_track,
            scene,
            name: "session source".into(),
            source_length: beat(4),
        })
        .expect("Session source")
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: beat(4),
            pitch: 64,
            channel: 0,
            velocity: 92,
        })
        .expect("Session MIDI note");
    editor
        .execute(Command::SetSessionStopCell {
            track: stopped_track,
            scene,
        })
        .expect("explicit Stop cell");
    editor
        .execute(Command::MasterGain { gain: 0.8 })
        .expect("master gain");

    let audio = Audio::new(48_000, vec![[0.25, -0.5]; 192_000]).expect("audio data");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, audio)]),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepared revision"),
    );
    let mut playback = SessionPlayback::new(Arc::clone(&revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::Seek(120_000))
        .expect("seek beyond both Arrangement notes");
    playback
        .apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch mixed scene");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut actual = [[0.0; 2]; 64];
    playback.render(&mut actual).expect("mixed render");

    let tracks = revision.arrangement().tracks();
    let mut expected = [[0.0; 2]; 64];
    tracks[0]
        .render_additive(120_000, &mut expected)
        .expect("independent audio source");
    let prepared_session_clip = revision
        .session()
        .midi_clip(session_clip)
        .expect("prepared Session clip");
    let table = revision.session().sine_table().as_ref();
    let context = MidiInstrumentRenderContext::new(
        BeatTime::from_quarters(5).expect("launch beat"),
        revision.session().timing(),
        table,
    );
    prepared_session_clip
        .instrument_template()
        .render_additive(
            context,
            tracks[1].mix(),
            120_000,
            &mut expected,
            &mut MidiSpanBuffer::default(),
        )
        .expect("independent Session MIDI source");
    revision.arrangement().master_mix().apply(&mut expected);
    assert_eq!(actual, expected);
    assert!(audible(&actual));
    assert_eq!(
        playback.owner(session_track),
        Some(SessionPlaybackOwner::Session { clip: session_clip })
    );
    assert_eq!(
        playback.owner(stopped_track),
        Some(SessionPlaybackOwner::Silence)
    );
    assert!(
        actual
            .iter()
            .all(|frame| frame[0] != 0.0 || frame[1] != 0.0)
    );
}

#[test]
fn arrangement_loop_cursor_wraps_while_session_performance_frame_is_monotonic() {
    let fixture = fixture(Some((0, 48_000)), None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut output = vec![[0.0; 2]; 96_000];
    playback.render(&mut output).expect("render across wraps");
    assert_eq!(playback.performance_frame(), 96_000);
    assert_eq!(playback.arrangement_frame(), 0);
    assert!(audible(&output));
}

#[test]
fn prepared_session_capability_matches_finite_arrangement_pcm_across_loops_and_partitions() {
    for (loop_region, tempo_marker) in [
        (None, None),
        (Some((0, 48_000)), None),
        (Some((10, 11)), None),
        (Some((0, 48_000)), Some(1)),
    ] {
        let fixture = fixture(loop_region, tempo_marker);
        let mut finite = Playback::from_prepared(Arc::clone(&fixture.revision));
        let mut mixed = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision))
            .expect("Session-capable playback");
        assert_eq!(mixed.session_capability(), SessionCapability::Available);
        finite.apply(PlaybackCommand::Play);
        mixed.apply(PlaybackCommand::Play);

        let mut expected = vec![[0.0; 2]; 49_037];
        let mut actual = expected.clone();
        finite.render(&mut expected).expect("finite render");
        for chunk in actual.chunks_mut(31) {
            mixed.render(chunk).expect("Session-capable render");
        }
        assert_eq!(actual, expected);
        assert_eq!(mixed.position(), finite.position());
        assert_eq!(mixed.performance_frame(), finite.performance_frame());
        assert_eq!(mixed.playing(), finite.playing());
    }
}

#[test]
fn legacy_promotion_refuses_uncaptured_arrangement_voices_and_can_retry_after_stop() {
    let fixture = fixture(None, None);
    let project = Arc::clone(fixture.revision.arrangement());
    let mut legacy = Playback::from_shared(Arc::clone(&project));
    legacy.apply(PlaybackCommand::Seek(24_000));
    legacy.apply(PlaybackCommand::Play);
    let mut candidate = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision))
        .expect("Session runtime");
    assert_eq!(
        candidate.try_inherit_runtime_from(&mut legacy),
        Err(PlaybackHandoffError::ArrangementStateCannotBeCaptured)
    );
    assert_eq!(candidate.position(), 0);
    assert_eq!(candidate.performance_frame(), 0);
    assert!(!candidate.playing());
    assert_eq!(legacy.position(), 24_000);
    assert!(legacy.playing());

    let mut output_after_refusal = [[0.0; 2]; 64];
    legacy
        .render(&mut output_after_refusal)
        .expect("legacy graph remains usable after refusal");
    assert!(audible(&output_after_refusal));
    legacy.apply(PlaybackCommand::Stop);
    legacy.apply(PlaybackCommand::Seek(120_000));
    candidate
        .try_inherit_runtime_from(&mut legacy)
        .expect("safe promotion can be retried after the voice ends");
    assert_eq!(candidate.position(), 120_000);
    assert_eq!(candidate.performance_frame(), 120_000);
    assert!(!candidate.playing());
}

#[test]
fn legacy_promotion_does_not_fence_an_already_disabled_instrument() {
    let fixture = fixture(None, None);
    let mut snapshot = fixture.project.snapshot();
    snapshot.tracks[0]
        .instrument
        .as_mut()
        .expect("MIDI track has an instrument slot")
        .bypassed = true;
    let muted_project = Project::from_legacy_snapshot(snapshot).expect("valid disabled instrument");
    let muted_revision = Arc::new(
        PreparedRevision::prepare(
            &muted_project,
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("disabled Arrangement revision"),
    );
    let mut legacy = Playback::from_shared(Arc::clone(muted_revision.arrangement()));
    legacy.apply(PlaybackCommand::Seek(24_000));
    legacy.apply(PlaybackCommand::Play);
    let mut candidate =
        Playback::try_from_prepared_with_session(muted_revision).expect("Session runtime");
    candidate
        .try_inherit_runtime_from(&mut legacy)
        .expect("disabled instrument has no held voice to capture");
    let mut output = [[0.0; 2]; 64];
    candidate
        .render(&mut output)
        .expect("render disabled route");
    assert_eq!(output, [[0.0; 2]; 64]);
}

#[test]
fn session_demotion_refuses_arrangement_voices_and_carried_releases() {
    let fixture = fixture(None, None);
    let mut previous = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision))
        .expect("Session runtime");
    previous.apply(PlaybackCommand::Play);
    let mut sounding = [[0.0; 2]; 64];
    previous.render(&mut sounding).expect("Arrangement render");
    assert!(audible(&sounding));

    let mut candidate = Playback::from_shared(Arc::clone(fixture.revision.arrangement()));
    assert_eq!(
        candidate.try_inherit_runtime_from(&mut previous),
        Err(PlaybackHandoffError::ActiveArrangementStateCannotBeDropped)
    );
    assert_eq!(previous.position(), 64);
    assert_eq!(candidate.position(), 0);

    previous.apply(PlaybackCommand::Seek(120_000));
    candidate
        .try_inherit_runtime_from(&mut previous)
        .expect("demotion succeeds after Arrangement voice expires");
    assert_eq!(candidate.position(), 120_000);

    let mut releasing = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision))
        .expect("Session runtime");
    releasing
        .try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip: fixture.session_clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .expect("launch Session note");
    releasing.apply(PlaybackCommand::Play);
    let mut prefix = [[0.0; 2]; 64];
    releasing.render(&mut prefix).expect("Session render");
    releasing
        .try_apply_action(PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
            track: fixture.track,
            quantization: LaunchQuantization::Immediate,
        }))
        .expect("release Session note");
    let mut no_drop = Playback::from_shared(Arc::clone(fixture.revision.arrangement()));
    assert_eq!(
        no_drop.try_inherit_runtime_from(&mut releasing),
        Err(PlaybackHandoffError::ActiveSessionStateCannotBeDropped)
    );
    assert_eq!(releasing.position(), 64);
}

#[test]
fn legacy_promotion_validates_session_time_before_render_or_launch() {
    let fixture = fixture(Some((0, 48_000)), None);
    let mut legacy = Playback::from_shared(Arc::clone(fixture.revision.arrangement()));
    legacy.apply(PlaybackCommand::Seek(u64::MAX));
    assert!(!legacy.performance_clock_exhausted());
    let mut candidate =
        Playback::try_from_prepared_with_session(fixture.revision).expect("Session runtime");
    assert_eq!(
        candidate.try_inherit_runtime_from(&mut legacy),
        Err(PlaybackHandoffError::Session(
            SessionPlaybackError::TempoConversion
        ))
    );
    assert_eq!(candidate.position(), 0);
    assert_eq!(candidate.performance_frame(), 0);
    assert!(!candidate.playing());
    legacy.apply(PlaybackCommand::Seek(120_000));
    legacy.apply(PlaybackCommand::Play);
    candidate
        .try_inherit_runtime_from(&mut legacy)
        .expect("valid promotion after Arrangement voices ended");
    assert_eq!(candidate.position(), 120_000);
    assert_eq!(candidate.performance_frame(), 120_000);
    assert!(candidate.playing());
}

#[test]
fn exhausted_legacy_clock_keeps_finite_playback_but_refuses_session_promotion() {
    let fixture = fixture(Some((0, 48_000)), None);
    // Exhaust the performance clock by rendering a tiny real loop near the
    // numeric limit. An absolute post-loop seek no longer normalizes backwards.
    let project = Arc::new(
        hex_engine::Project::with_loop_region(
            48_000,
            fixture.revision.arrangement().tracks().to_vec(),
            Some(hex_project::LoopRegion::new(u64::MAX - 8, u64::MAX - 4, true).unwrap()),
        )
        .unwrap(),
    );
    let mut legacy = Playback::from_shared(project);
    legacy.apply(PlaybackCommand::Seek(u64::MAX - 7));
    legacy.apply(PlaybackCommand::Play);
    let mut output = [[0.0; 2]; 8];
    legacy.render(&mut output).expect("legacy finite render");
    assert_eq!(legacy.performance_frame(), u64::MAX);
    assert!(legacy.performance_clock_exhausted());

    let mut candidate = Playback::try_from_prepared_with_session(fixture.revision)
        .expect("session capability prepared off-thread");
    assert_eq!(
        candidate.try_inherit_runtime_from(&mut legacy),
        Err(PlaybackHandoffError::PerformanceClockExhausted)
    );
    legacy
        .render(&mut output)
        .expect("legacy playback remains usable");
    assert_eq!(legacy.performance_frame(), u64::MAX);
}

#[test]
fn unavailable_session_capability_refuses_instrument_render_and_session_actions() {
    let fixture = fixture(Some((u64::MAX - 1, u64::MAX)), None);
    let prepared = Playback::try_from_prepared_with_session(Arc::clone(&fixture.revision));
    assert!(matches!(
        prepared,
        Err(SessionPlaybackError::SessionTempoUnavailable(_))
    ));
    let error = prepared.err().expect("unavailable loop timing");
    let mut fallback =
        Playback::from_prepared_with_session_unavailable(Arc::clone(&fixture.revision), error);
    assert_eq!(
        fallback.session_capability(),
        SessionCapability::Unavailable(error)
    );
    fallback.apply(PlaybackCommand::Play);
    let mut output = [[0.125; 2]; 64];
    assert!(fallback.render(&mut output).is_err());
    assert_eq!(output, [[0.125; 2]; 64]);
    assert_eq!(fallback.position(), 0);
    assert_eq!(fallback.performance_frame(), 0);
    assert!(matches!(
        fallback.try_apply_action(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip: fixture.session_clip,
                quantization: LaunchQuantization::Immediate,
            }
        )),
        Err(PlaybackActionError::SessionUnavailable(reason)) if reason == error
    ));
}

#[test]
fn unavailable_instrument_candidate_cannot_retire_a_renderable_session_graph() {
    let previous_fixture = fixture(None, None);
    let unavailable_fixture = fixture(Some((u64::MAX - 1, u64::MAX)), None);
    let error =
        match Playback::try_from_prepared_with_session(Arc::clone(&unavailable_fixture.revision)) {
            Err(error @ SessionPlaybackError::SessionTempoUnavailable(_)) => error,
            _ => panic!("expected unavailable Session timing"),
        };
    let mut previous = Playback::try_from_prepared_with_session(previous_fixture.revision)
        .expect("renderable old graph");
    previous.apply(PlaybackCommand::Seek(120_000));
    previous.apply(PlaybackCommand::Play);
    let mut candidate =
        Playback::from_prepared_with_session_unavailable(unavailable_fixture.revision, error);
    assert_eq!(
        candidate.try_inherit_runtime_from(&mut previous),
        Err(PlaybackHandoffError::Session(error))
    );
    assert_eq!(previous.position(), 120_000);
    assert!(previous.playing());
    assert_eq!(candidate.position(), 0);
    assert_eq!(
        candidate.session_capability(),
        SessionCapability::Unavailable(error)
    );
}

#[test]
fn changing_tempo_with_enabled_loop_uses_periodic_session_clock() {
    for marker in [1, 2, 8] {
        let fixture = fixture(Some((0, 48_000)), Some(marker));
        let mut whole = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
        let mut split = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
        for playback in [&mut whole, &mut split] {
            playback
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: fixture.session_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .expect("launch");
            playback.apply(SessionPlaybackCommand::Play).expect("play");
        }
        let mut expected = vec![[0.0; 2]; 97_003];
        let mut actual = expected.clone();
        whole.render(&mut expected).expect("whole loop render");
        for chunk in actual.chunks_mut(31) {
            split.render(chunk).expect("partitioned loop render");
        }
        assert_eq!(actual, expected, "tempo marker at beat {marker}");
        assert!(audible(&actual));
    }
}

#[test]
fn offset_one_frame_arrangement_loop_prepares_and_renders_session_source() {
    let fixture = fixture(Some((10, 11)), None);
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::Immediate,
        })
        .expect("launch");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut output = [[0.0; 2]; 18];
    playback.render(&mut output).expect("tiny loop render");
    assert!(audible(&output));
    assert_eq!(playback.performance_frame(), 18);
    assert_eq!(playback.arrangement_frame(), 10);
}

#[test]
fn four_quarter_launch_quantization_uses_the_repeated_tempo_across_a_seam() {
    let fixture = fixture(Some((0, 48_000)), Some(1));
    // Premise: the 48 000-frame loop crosses the beat-1 tempo seam, so one pass
    // is 1.75 beats and the fourth beat falls 0.5 beats into the third pass.
    assert_eq!(
        fixture
            .project
            .loop_region()
            .map(|region| (region.start(), region.end())),
        Some((0, 48_000))
    );
    let mut playback = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("kernel");
    playback.apply(SessionPlaybackCommand::Play).expect("play");
    let mut prefix = vec![[0.0; 2]; 47_999];
    playback.render(&mut prefix).expect("reach loop seam");
    playback
        .apply(SessionPlaybackCommand::LaunchClip {
            clip: fixture.session_clip,
            quantization: LaunchQuantization::FourQuarter,
        })
        .expect("queue launch");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    let mut to_boundary = vec![[0.0; 2]; 60_001];
    playback
        .render(&mut to_boundary)
        .expect("render to periodic four-quarter boundary");
    assert_eq!(playback.performance_frame(), 108_000);
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Arrangement)
    );
    let mut after_boundary = [[0.0; 2]; 64];
    playback
        .render(&mut after_boundary)
        .expect("apply boundary launch");
    assert_eq!(
        playback.owner(fixture.track),
        Some(SessionPlaybackOwner::Session {
            clip: fixture.session_clip
        })
    );
    assert!(audible(&after_boundary));
}

#[test]
fn unrepresentable_session_loop_does_not_reject_arrangement_revision() {
    let mut editor =
        Editor::new(hex_project::Project::new(48_000).expect("project"), 16).expect("editor");
    editor
        .execute(Command::SetLoopRegion {
            start: u64::MAX - 1,
            end: u64::MAX,
            enabled: true,
        })
        .expect("stored loop region");
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("Arrangement-only revision remains preparable"),
    );
    assert!(matches!(
        SessionPlayback::new(revision),
        Err(SessionPlaybackError::SessionTempoUnavailable(
            hex_engine::PreparedTempoError::FrameToBeat(_)
        ))
    ));
}

#[test]
fn session_render_matches_irregular_partitions_across_loop_wraps() {
    let fixture = fixture(Some((0, 48_000)), None);
    let mut whole = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("whole kernel");
    let mut split = SessionPlayback::new(Arc::clone(&fixture.revision)).expect("split kernel");
    for playback in [&mut whole, &mut split] {
        playback
            .apply(SessionPlaybackCommand::LaunchScene {
                scene: fixture.scene,
                quantization: LaunchQuantization::Immediate,
            })
            .expect("launch scene");
        playback.apply(SessionPlaybackCommand::Play).expect("play");
    }
    let mut expected = vec![[0.0; 2]; 72_013];
    let mut actual = expected.clone();
    whole.render(&mut expected).expect("whole render");
    for chunk in actual.chunks_mut(31) {
        split.render(chunk).expect("split render");
    }
    assert_eq!(actual, expected);
    assert_eq!(whole.performance_frame(), split.performance_frame());
    assert_eq!(whole.arrangement_frame(), split.arrangement_frame());
}
