//! Instrumented headless callback allocation regression; no device is opened.
#![cfg(feature = "allocation-probe")]

#[path = "../../hex-engine/examples/support/session_audio_fixture.rs"]
#[allow(dead_code)]
mod session_audio_fixture;

use allocation_counter::{AllocationInfo, measure};
use hex_audio::prepared::{self, PublicationPolicy, SessionRouteOwner};
use hex_engine::{
    Audio, Frame, LaunchQuantization, PreparedRevision, Renderer, SessionPlaybackCommand,
    SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{
    AssetId, BeatTime, Command, Created, Editor, Project, SceneId, Tempo, TrackContentSnapshot,
    TrackId,
};
use session_audio_fixture as fixture;
use std::{collections::BTreeMap, error::Error, sync::Arc};

const RATE: u32 = fixture::RATE;
const BLOCK: usize = 1_024;
type Candidate = (Arc<PreparedRevision>, Project);

fn measured_block(callback: &mut prepared::Callback, output: &mut [Frame]) -> AllocationInfo {
    let mut render_failed = false;
    let info = measure(|| {
        callback.begin_block();
        render_failed = callback.render(output).is_err();
    });
    assert!(!render_failed, "callback render returned an error");
    assert_eq!(info.count_total, 0, "callback allocated");
    assert_eq!(info.bytes_total, 0, "callback allocated bytes");
    assert_eq!(
        info.count_current, 0,
        "callback changed live allocation count"
    );
    assert_eq!(
        info.bytes_current, 0,
        "callback changed live allocation bytes"
    );
    info
}

fn measured_shutdown(callback: &mut prepared::Callback, output: &mut [Frame]) -> AllocationInfo {
    measured_block(callback, output)
}

fn compare_output(actual: &[Frame], expected: &[Frame]) {
    assert_eq!(actual, expected);
}

fn assert_session_owner(
    control: &prepared::Control,
    routes: &mut [prepared::SessionRouteEntry],
    track_count: usize,
    clip: u64,
) {
    let snapshot = control
        .status
        .session_routes(routes)
        .expect("route snapshot is available");
    assert_eq!(snapshot.count, track_count);
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session { clip_id: clip }
    );
}

fn launch_scene(control: &mut prepared::Control, scene: SceneId) -> Result<(), &'static str> {
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .map(|_| ())
        .map_err(|_| "scene launch admission failed")
}

fn prepare_fixture(
    workload: &fixture::Fixture,
) -> Result<Arc<PreparedRevision>, hex_engine::Invalid> {
    Ok(Arc::new(PreparedRevision::prepare(
        &workload.project,
        &workload.media,
        Arc::new(SineTable::new()),
        None,
    )?))
}

fn assert_scene_audio_oracle(
    output: &[Frame],
    workload: &fixture::Fixture,
    scene: SceneId,
    elapsed: u64,
) -> Result<(), Box<dyn Error>> {
    let expected = fixture::finite_session_oracle(
        &workload.project,
        &workload.media,
        scene,
        elapsed,
        output.len(),
    )?;
    compare_output(output, &expected);
    Ok(())
}

fn assert_project_audio_oracle(
    actual: &[Frame],
    project: &Project,
    media: &BTreeMap<AssetId, Audio>,
    scene: SceneId,
    elapsed: u64,
) -> Result<(), Box<dyn Error>> {
    let expected = fixture::finite_session_oracle(project, media, scene, elapsed, actual.len())?;
    compare_output(actual, &expected);
    Ok(())
}

#[test]
fn hybrid_arrangement_audio_and_midi_callback_is_allocation_free_and_exact()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::AddTrack {
        name: "hybrid".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected hybrid track".into()),
    };
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/hybrid.wav".into(),
        sample_rate: RATE,
        frames: BLOCK as u64,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: BLOCK as u64,
        source_offset: 0,
        source_length: BLOCK as u64,
        looping: false,
    })?;
    let midi_clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected arrangement midi clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip: midi_clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 64,
        channel: 0,
        velocity: 100,
    })?;
    editor.execute(Command::Gain { track, gain: 0.5 })?;
    editor.execute(Command::Balance {
        track,
        balance: -0.25,
    })?;
    let utility_type = DeviceTypeId::new("hex.utility")?;
    let utility_state = hex_engine::default_native_device_state(utility_type)?;
    let utility_state = hex_engine::patch_native_device_parameter(
        &utility_state,
        ParameterId::new(6)?,
        ParameterValue::Bool(true),
    )?;
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility_state,
    })?;
    let project = editor.project().clone();
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..BLOCK)
                .map(|frame| {
                    let sample = (frame % 31) as f32 / 64.0 - 0.2;
                    [sample, sample]
                })
                .collect(),
        )?,
    )]);
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &media,
        Arc::new(SineTable::new()),
        None,
    )?);

    // Independent source-only renders preserve the canonical IDs and shared
    // track mix. Their exact sum is the oracle for the hybrid compiled track.
    let mut audio_project_snapshot = project.snapshot();
    let TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips,
    } = &audio_project_snapshot.tracks[0].content
    else {
        return Err("expected hybrid snapshot".into());
    };
    let audio_clips = audio_clips.clone();
    let midi_clips = midi_clips.clone();
    audio_project_snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips,
        midi_clips: Vec::new(),
    };
    let audio_project = Project::from_snapshot(audio_project_snapshot)?;
    let mut midi_project_snapshot = project.snapshot();
    midi_project_snapshot.tracks[0].content = TrackContentSnapshot::Hybrid {
        audio_clips: Vec::new(),
        midi_clips,
    };
    let midi_project = Project::from_snapshot(midi_project_snapshot)?;
    let audio_revision =
        PreparedRevision::prepare(&audio_project, &media, Arc::new(SineTable::new()), None)?;
    let midi_revision =
        PreparedRevision::prepare(&midi_project, &media, Arc::new(SineTable::new()), None)?;
    let mut audio_oracle = [[0.0; 2]; BLOCK];
    let mut midi_oracle = [[0.0; 2]; BLOCK];
    let mut audio_renderer = Renderer::new(audio_revision.arrangement());
    audio_renderer.play();
    audio_renderer.render(&mut audio_oracle)?;
    let mut midi_renderer = Renderer::new(midi_revision.arrangement());
    midi_renderer.play();
    midi_renderer.render(&mut midi_oracle)?;
    assert!(
        audio_oracle
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    assert!(
        midi_oracle
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-6)
    );
    let expected: Vec<Frame> = audio_oracle
        .iter()
        .zip(midi_oracle.iter())
        .map(|(audio, midi)| [audio[0] + midi[0], audio[1] + midi[1]])
        .collect();

    let playback = Playback::try_from_prepared_with_session(revision)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "Play command admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn play_from_callback_is_allocation_free_and_renders_from_requested_frame()
-> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let revision = prepare_fixture(&workload)?;
    let playback = Playback::try_from_prepared_with_session(revision)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    let start = 1_024;
    let ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(start))
        .map_err(|_| "PlayFrom command admission failed")?;
    let mut actual = [[0.0; 2]; BLOCK];
    measured_block(&mut callback, &mut actual);
    let expected =
        fixture::finite_arrangement_oracle(&workload.project, &workload.media, start, BLOCK)?;
    compare_output(&actual, &expected);
    assert_eq!(
        control
            .status
            .consumed_command
            .load(std::sync::atomic::Ordering::Acquire),
        ticket
    );
    assert_eq!(
        control
            .status
            .position
            .load(std::sync::atomic::Ordering::Acquire),
        start + BLOCK as u64
    );

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn shared_and_distinct_session_audio_callback_blocks_are_allocation_free()
-> Result<(), Box<dyn Error>> {
    for distinct in [false, true] {
        let workload = fixture::fixture(distinct)?;
        let revision = prepare_fixture(&workload)?;
        let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
        let (mut control, mut callback) = prepared::channel(RATE)?;
        control
            .publish(playback)
            .map_err(|_| "initial playback publication failed")?;

        // Thirty pre-cut seeks plus a post-cut scene launch and Play fill the
        // bounded queue. The additional request is refused before measurement.
        for _ in 0..30 {
            control
                .command(PlaybackCommand::Seek(0))
                .map_err(|_| "queue full early")?;
        }
        control.stop();
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene: workload.scene_a,
                quantization: LaunchQuantization::Immediate,
            })
            .map_err(|_| "post-stop scene launch admission failed")?;
        control
            .command(PlaybackCommand::Play)
            .map_err(|_| "post-stop Play admission failed")?;
        assert!(control.command(PlaybackCommand::Pause).is_err());

        let mut output = [[0.0; 2]; BLOCK];
        measured_block(&mut callback, &mut output[..128]);
        assert_scene_audio_oracle(&output[..128], &workload, workload.scene_a, 0)?;

        let partitions = [1_usize, 127, 257, 511, 1_024, 1_024, 1_024];
        let mut elapsed = 128_u64;
        for frames in partitions {
            measured_block(&mut callback, &mut output[..frames]);
            assert_scene_audio_oracle(&output[..frames], &workload, workload.scene_a, elapsed)?;
            elapsed += u64::try_from(frames)?;
        }
        assert_eq!(elapsed, 4_096);

        measured_block(&mut callback, &mut output[..128]);
        assert!(output[..128].iter().all(|frame| *frame == [0.0; 2]));

        // A scene relaunch re-anchors every one-shot to its source start.
        launch_scene(&mut control, workload.scene_a)?;
        measured_block(&mut callback, &mut output[..257]);
        assert_scene_audio_oracle(&output[..257], &workload, workload.scene_a, 0)?;

        // Execute a full 32-action burst; the final alternating scene is B.
        for index in 0..32 {
            let scene = if index % 2 == 0 {
                workload.scene_a
            } else {
                workload.scene_b
            };
            control
                .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                    scene,
                    quantization: LaunchQuantization::Immediate,
                })
                .map_err(|_| "32-action burst admission failed")?;
        }
        assert!(control.command(PlaybackCommand::Pause).is_err());
        measured_block(&mut callback, &mut output[..257]);
        assert_scene_audio_oracle(&output[..257], &workload, workload.scene_b, 0)?;

        control.request_shutdown();
        measured_shutdown(&mut callback, &mut output[..128]);
        control.reclaim();
    }
    Ok(())
}

fn edited_candidates(workload: &fixture::Fixture) -> Result<Vec<Candidate>, Box<dyn Error>> {
    let table = Arc::new(SineTable::new());
    let initial = prepare_fixture(workload)?;
    let mut editor = Editor::new(workload.project.clone(), 8)?;
    let mut candidates = vec![(Arc::clone(&initial), workload.project.clone())];

    editor.execute(Command::Gain {
        track: workload.tracks[0],
        gain: 0.125,
    })?;
    let gain = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &workload.media,
        Arc::clone(&table),
        Some(initial.session()),
    )?);
    candidates.push((Arc::clone(&gain), editor.project().clone()));

    let clip = workload.scene_a_clips[0];
    let source = editor
        .project()
        .session_audio_clip(clip)
        .ok_or("missing Session audio clip")?;
    editor.execute(Command::SetSessionAudioClipSource {
        clip,
        asset: source.asset(),
        source_start: source.source_start() + 1,
        source_end: source.source_end(),
    })?;
    let source_revision = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &workload.media,
        Arc::clone(&table),
        Some(gain.session()),
    )?);
    candidates.push((Arc::clone(&source_revision), editor.project().clone()));

    editor.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(60_000_000)?,
    })?;
    let tempo = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &workload.media,
        table,
        Some(source_revision.session()),
    )?);
    candidates.push((tempo, editor.project().clone()));
    Ok(candidates)
}

#[test]
fn prepared_edits_inherit_through_a_full_retirement_ring_without_callback_allocation()
-> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let candidates = edited_candidates(&workload)?;
    let initial = Playback::try_from_prepared_with_session(Arc::clone(&candidates[0].0))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(initial)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: workload.scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "initial launch admission failed")?;

    let mut output = [[0.0; 2]; 128];
    let expected_initial = fixture::finite_session_oracle(
        &candidates[0].1,
        &workload.media,
        workload.scene_a,
        0,
        output.len(),
    )?;
    measured_block(&mut callback, &mut output);
    compare_output(&output, &expected_initial);
    assert_eq!(
        control
            .status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        128
    );
    let mut routes = [prepared::SessionRouteEntry::default(); prepared::SESSION_ROUTE_CAPACITY];
    let first_clip = workload.scene_a_clips[0].value();
    assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    for (revision, (candidate, project)) in candidates.iter().enumerate().skip(1).take(2) {
        let playback = Playback::try_from_prepared_with_session(Arc::clone(candidate))?;
        assert_eq!(
            control
                .publish(playback)
                .map_err(|_| "candidate publication failed")?,
            revision as u64 + 1
        );
        measured_block(&mut callback, &mut output);
        assert_project_audio_oracle(
            &output,
            project,
            &workload.media,
            workload.scene_a,
            128 * revision as u64,
        )?;
        assert_eq!(
            control
                .status
                .applied_revision
                .load(std::sync::atomic::Ordering::Acquire),
            revision as u64 + 1
        );
        assert_eq!(
            control
                .status
                .performance_frame
                .load(std::sync::atomic::Ordering::Acquire),
            128 * (revision as u64 + 1)
        );
        assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    }

    let pending = Playback::try_from_prepared_with_session(Arc::clone(&candidates[3].0))?;
    assert_eq!(
        control
            .publish(pending)
            .map_err(|_| "tempo publication failed")?,
        4
    );
    measured_block(&mut callback, &mut output);
    assert_project_audio_oracle(
        &output,
        &candidates[2].1,
        &workload.media,
        workload.scene_a,
        384,
    )?;
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        3
    );
    assert_eq!(
        control
            .status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        512
    );
    assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    assert_eq!(control.reclaim(), 2);
    measured_block(&mut callback, &mut output);
    assert_project_audio_oracle(
        &output,
        &candidates[3].1,
        &workload.media,
        workload.scene_a,
        512,
    )?;
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        4
    );
    assert_eq!(
        control
            .status
            .performance_frame
            .load(std::sync::atomic::Ordering::Acquire),
        640
    );
    assert_session_owner(&control, &mut routes, workload.tracks.len(), first_clip);
    assert_eq!(control.reclaim(), 1);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    // Avoid keeping candidate revisions alive beyond the callback assertions.
    drop(candidates);
    Ok(())
}

#[test]
fn fresh_project_and_shutdown_reclaim_graph_and_pcm_only_on_control() -> Result<(), Box<dyn Error>>
{
    let workload = fixture::fixture(false)?;
    let revision = prepare_fixture(&workload)?;
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let weak_revision = Arc::downgrade(
        playback
            .prepared_revision()
            .ok_or("prepared revision missing from playback")?,
    );
    drop(revision);
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: workload.scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "initial launch admission failed")?;
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(weak_revision.upgrade().is_some());

    let fresh_project = Project::new(RATE)?;
    let fresh_revision = Arc::new(PreparedRevision::prepare(
        &fresh_project,
        &BTreeMap::<AssetId, Audio>::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let fresh_playback = Playback::try_from_prepared_with_session(Arc::clone(&fresh_revision))?;
    let weak_fresh_revision = Arc::downgrade(&fresh_revision);
    drop(fresh_revision);
    control
        .publish_with_policy(fresh_playback, PublicationPolicy::InstallFreshRuntime)
        .map_err(|_| "fresh playback publication failed")?;
    drop(workload);
    measured_block(&mut callback, &mut output);
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert!(weak_fresh_revision.upgrade().is_some());
    assert!(
        weak_revision.upgrade().is_some(),
        "retired graph stays alive before reclaim"
    );

    let mut reclaimed = 0;
    let released = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 1);
    assert_eq!(released.count_total, 0);
    assert!(released.count_current < 0);
    assert!(
        released.bytes_current <= -((fixture::SOURCE_FRAMES * std::mem::size_of::<Frame>()) as i64)
    );
    assert!(
        weak_revision.upgrade().is_none(),
        "old graph drops only after control reclaim"
    );

    let mut routes = [prepared::SessionRouteEntry::default(); prepared::SESSION_ROUTE_CAPACITY];
    let fresh_routes = control
        .status
        .session_routes(&mut routes)
        .ok_or("fresh route snapshot")?;
    assert_eq!(fresh_routes.count, 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    assert!(weak_fresh_revision.upgrade().is_some());
    let mut reclaimed_fresh = 0;
    let fresh_release = measure(|| reclaimed_fresh = control.reclaim());
    assert_eq!(reclaimed_fresh, 1);
    assert_eq!(fresh_release.count_total, 0);
    assert!(fresh_release.count_current < 0);
    assert!(weak_fresh_revision.upgrade().is_none());
    Ok(())
}

#[test]
fn priority_stop_is_allocation_free_while_retirement_ring_is_full() -> Result<(), Box<dyn Error>> {
    let workload = fixture::fixture(false)?;
    let candidates = edited_candidates(&workload)?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    let initial = Playback::try_from_prepared_with_session(Arc::clone(&candidates[0].0))?;
    control
        .publish(initial)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: workload.scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "initial launch admission failed")?;

    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    for candidate in candidates.iter().skip(1).take(2) {
        let playback = Playback::try_from_prepared_with_session(Arc::clone(&candidate.0))?;
        control
            .publish(playback)
            .map_err(|_| "retirement-filling publication failed")?;
        measured_block(&mut callback, &mut output);
    }
    let pending = Playback::try_from_prepared_with_session(Arc::clone(&candidates[3].0))?;
    control
        .publish(pending)
        .map_err(|_| "pending candidate admission failed")?;

    // Stop is a priority path and remains effective even while publication is
    // backpressured by a full callback-to-control retirement ring.
    control.stop();
    measured_block(&mut callback, &mut output);
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    assert!(
        !control
            .status
            .playing
            .load(std::sync::atomic::Ordering::Acquire)
    );

    assert_eq!(control.reclaim(), 2);
    measured_block(&mut callback, &mut output);
    assert!(output.iter().all(|frame| *frame == [0.0; 2]));
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    drop(candidates);
    drop(workload);
    Ok(())
}

fn midi_session_project() -> Result<(Project, TrackId, SceneId, SceneId), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::AddMidiTrack {
        name: "keys".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let scene_a = match editor.execute(Command::AddScene { name: "a".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    let scene_b = match editor.execute(Command::AddScene { name: "b".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    for (scene, name, pitch) in [(scene_a, "source a", 60), (scene_b, "source b", 67)] {
        let clip = match editor.execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: name.into(),
            source_length: BeatTime::from_quarters(4)?,
        })? {
            Created::SessionClip(clip) => clip,
            _ => return Err("expected Session MIDI clip".into()),
        };
        editor.execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2)?,
            pitch,
            channel: 0,
            velocity: 120,
        })?;
    }
    Ok((editor.project().clone(), track, scene_a, scene_b))
}

fn insert_stateful_effects_at(
    editor: &mut Editor,
    track: TrackId,
    instrument_position: usize,
) -> Result<(), Box<dyn Error>> {
    let utility_type = DeviceTypeId::new("hex.utility")?;
    let utility = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(utility_type)?,
        ParameterId::new(11)?,
        ParameterValue::Bool(true),
    )?;
    editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility.clone(),
    })?;
    let suffix = match editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility,
    })? {
        Created::Device(id) => id,
        _ => return Err("expected suffix device".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected instrument slot")?;
    let instrument = instrument.id();
    if instrument_position == 1 {
        editor.execute(Command::MoveTrackDevice {
            device: instrument,
            track,
            before: Some(suffix),
        })?;
    } else if instrument_position != 0 {
        return Err("unsupported instrument position in allocation fixture".into());
    }
    Ok(())
}

#[test]
fn sounding_midi_scene_switches_use_the_prepared_release_bank_without_allocations()
-> Result<(), Box<dyn Error>> {
    for instrument_position in [0, 1] {
        sounding_midi_scene_switches_at_position(instrument_position)?;
    }
    Ok(())
}

fn sounding_midi_scene_switches_at_position(
    instrument_position: usize,
) -> Result<(), Box<dyn Error>> {
    let (project, track, scene_a, scene_b) = midi_session_project()?;
    let mut editor = Editor::new(project, 16)?;
    insert_stateful_effects_at(&mut editor, track, instrument_position)?;
    let project = editor.project().clone();
    let revision = Arc::new(PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
    )?);
    let mut oracle = hex_engine::SessionPlayback::new(Arc::clone(&revision))?;
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene: scene_a,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "MIDI launch admission failed")?;
    let mut output = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.apply(SessionPlaybackCommand::LaunchScene {
        scene: scene_a,
        quantization: LaunchQuantization::Immediate,
    })?;
    oracle.apply(SessionPlaybackCommand::Play)?;
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected);
    assert!(control.status.rejected_action().is_none());
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    for scene in [scene_b, scene_a, scene_b, scene_a, scene_b, scene_a] {
        control
            .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            })
            .map_err(|_| "MIDI scene switch admission failed")?;
        oracle.apply(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })?;
        oracle.apply(SessionPlaybackCommand::Play)?;
        oracle.render(&mut expected)?;
        measured_block(&mut callback, &mut output);
        assert_eq!(output, expected);
        assert!(control.status.rejected_action().is_none());
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    }
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn stateful_utility_processing_and_revision_handoff_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    for instrument_position in [0, 1] {
        stateful_utility_handoff_at_position(instrument_position, false)?;
        stateful_utility_handoff_at_position(instrument_position, true)?;
    }
    Ok(())
}

fn stateful_utility_handoff_at_position(
    instrument_position: usize,
    bypass_instrument: bool,
) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::AddMidiTrack {
        name: "keys".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let scene = match editor.execute(Command::AddScene { name: "a".into() })? {
        Created::Scene(scene) => scene,
        _ => return Err("expected Session scene".into()),
    };
    let clip = match editor.execute(Command::CreateSessionMidiClip {
        track,
        scene,
        name: "source".into(),
        source_length: BeatTime::from_quarters(4)?,
    })? {
        Created::SessionClip(clip) => clip,
        _ => return Err("expected Session MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(1)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;
    insert_stateful_effects_at(&mut editor, track, instrument_position)?;
    let first_project = editor.project().clone();
    editor.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(60_000_000)?,
    })?;
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("expected instrument slot")?
        .id();
    editor.execute(Command::SetDeviceBypass {
        device: instrument,
        bypassed: bypass_instrument,
    })?;
    let second_project = editor.project().clone();
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        &first_project,
        &media,
        Arc::clone(&table),
        None,
    )?);
    let second_revision = Arc::new(PreparedRevision::prepare(
        &second_project,
        &media,
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);

    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial playback publication failed")?;
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchScene {
            scene,
            quantization: LaunchQuantization::Immediate,
        })
        .map_err(|_| "Session launch admission failed")?;
    let mut output = [[0.0; 2]; 128];
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "stateful revision publication failed")?;
    measured_block(&mut callback, &mut output);
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    for _ in 0..256 {
        measured_block(&mut callback, &mut output);
    }
    assert!(control.status.rejected_revision().is_none());

    if bypass_instrument {
        editor.execute(Command::SetDeviceBypass {
            device: instrument,
            bypassed: false,
        })?;
        let third_project = editor.project().clone();
        let third_revision = Arc::new(PreparedRevision::prepare(
            &third_project,
            &media,
            Arc::clone(&table),
            Some(second_revision.session()),
        )?);
        let reenabled = Playback::try_from_prepared_with_session(Arc::clone(&third_revision))?;
        control
            .publish(reenabled)
            .map_err(|_| "instrument re-enable publication failed")?;
        measured_block(&mut callback, &mut output);
        assert!(control.status.rejected_revision().is_none());
    }

    // The prior graph retains completed stateful tail owners until a later
    // graph publication retires it. Callback transfer stays allocation-free;
    // control reclamation performs only the expected deallocation.
    let final_playback = Playback::try_from_prepared_with_session(Arc::clone(&second_revision))?;
    control
        .publish(final_playback)
        .map_err(|_| "final stateful publication failed")?;
    measured_block(&mut callback, &mut output);
    let mut reclaimed = 0;
    let retired = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 2);
    assert_eq!(retired.count_total, 0);
    assert!(retired.count_current < 0);
    assert!(retired.bytes_current < 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}

#[test]
fn arrangement_instrument_bypass_and_reenable_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    for instrument_position in [0, 1] {
        arrangement_instrument_bypass_at_position(instrument_position)?;
    }
    Ok(())
}

#[test]
fn builtin_fm4_render_state_handoff_and_retirement_are_callback_allocation_free()
-> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::AddMidiTrack {
        name: "fm4 arrangement".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected FM4 instrument track".into()),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("default instrument slot missing")?
        .id();
    let fm4_type = DeviceTypeId::from_static("hex.instrument.fm4");
    let fm4_default = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        fm4_type,
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: fm4_default,
    })?;
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;
    let first_project = editor.project().clone();
    let media = BTreeMap::new();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        &first_project,
        &media,
        Arc::clone(&table),
        None,
    )?);
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(playback)
        .map_err(|_| "initial FM4 graph publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    oracle.apply(PlaybackCommand::Play);
    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let changed_state = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Instrument,
        editor
            .project()
            .device(instrument)
            .ok_or("FM4 disappeared")?
            .state(),
        ParameterId::from_static(29),
        ParameterValue::Float(0.5),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device: instrument,
        state: changed_state,
    })?;
    let changed_project = editor.project().clone();
    let changed_revision = Arc::new(PreparedRevision::prepare(
        &changed_project,
        &media,
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);
    let mut changed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    changed_oracle
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "FM4 state-change oracle handoff failed")?;
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "FM4 state-change publication failed")?;
    changed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut completed = 0;
    let retired = measure(|| completed = control.reclaim());
    assert!(completed > 0, "old FM4 graph retires on the control side");
    assert_eq!(retired.count_total, 0, "retirement does not allocate");
    assert!(
        retired.count_current < 0,
        "retirement performs its drops on control"
    );
    assert!(retired.bytes_current < 0);

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    drop((changed_revision, first_revision, changed_oracle, oracle));
    Ok(())
}

fn arrangement_instrument_bypass_at_position(
    instrument_position: usize,
) -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let track = match editor.execute(Command::AddMidiTrack {
        name: "arrangement keys".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected instrument track".into()),
    };
    let clip = match editor.execute(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(8)?,
    })? {
        Created::Clip(clip) => clip,
        _ => return Err("expected Arrangement MIDI clip".into()),
    };
    editor.execute(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration: BeatTime::from_quarters(4)?,
        pitch: 60,
        channel: 0,
        velocity: 120,
    })?;
    insert_stateful_effects_at(&mut editor, track, instrument_position)?;
    let first_project = editor.project().clone();

    let mut changed_editor = Editor::new(first_project.clone(), 16)?;
    changed_editor.execute(Command::RemoveInstrument { track })?;
    changed_editor.execute(Command::InsertInstrument {
        track,
        state: hex_project::default_instrument_state()?,
    })?;
    let changed_instrument = changed_editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .ok_or("replacement instrument slot is missing")?
        .id();
    if instrument_position == 1 {
        let before = changed_editor
            .project()
            .track(track)
            .and_then(|track| track.devices().get(1))
            .map(|device| device.id())
            .ok_or("instrument suffix anchor is missing")?;
        changed_editor.execute(Command::MoveTrackDevice {
            device: changed_instrument,
            track,
            before: Some(before),
        })?;
    }
    let changed_project = changed_editor.project().clone();
    let mut bypassed_editor = Editor::new(changed_project.clone(), 16)?;
    bypassed_editor.execute(Command::SetDeviceBypass {
        device: changed_instrument,
        bypassed: true,
    })?;
    let bypassed_project = bypassed_editor.project().clone();
    let table = Arc::new(SineTable::new());
    let first_revision = Arc::new(PreparedRevision::prepare(
        &first_project,
        &BTreeMap::new(),
        Arc::clone(&table),
        None,
    )?);
    let changed_revision = Arc::new(PreparedRevision::prepare(
        &changed_project,
        &BTreeMap::new(),
        Arc::clone(&table),
        Some(first_revision.session()),
    )?);
    let bypassed_revision = Arc::new(PreparedRevision::prepare(
        &bypassed_project,
        &BTreeMap::new(),
        Arc::clone(&table),
        Some(changed_revision.session()),
    )?);

    let initial = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&first_revision))?;
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(initial)
        .map_err(|_| "initial Arrangement publication failed")?;
    control.command(PlaybackCommand::Play).unwrap();
    oracle.apply(PlaybackCommand::Play);

    let mut actual = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(actual.iter().flatten().any(|sample| sample.abs() > 1.0e-6));

    let mut changed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    changed_oracle
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "Arrangement instrument identity handoff was refused")?;
    let identity_replacement =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(identity_replacement)
        .map_err(|_| "Arrangement instrument identity publication failed")?;
    changed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut bypassed_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&bypassed_revision))?;
    bypassed_oracle
        .try_inherit_runtime_from(&mut changed_oracle)
        .map_err(|_| "Arrangement bypass oracle handoff was refused")?;
    let replacement = Playback::try_from_prepared_with_session(Arc::clone(&bypassed_revision))?;
    control
        .publish(replacement)
        .map_err(|_| "Arrangement bypass publication failed")?;
    bypassed_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    let mut reenabled_oracle =
        Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    reenabled_oracle
        .try_inherit_runtime_from(&mut bypassed_oracle)
        .map_err(|_| "Arrangement re-enable oracle handoff was refused")?;
    let reenabled = Playback::try_from_prepared_with_session(Arc::clone(&changed_revision))?;
    control
        .publish(reenabled)
        .map_err(|_| "Arrangement re-enable publication failed")?;
    reenabled_oracle.render(&mut expected)?;
    measured_block(&mut callback, &mut actual);
    compare_output(&actual, &expected);
    assert!(control.status.rejected_revision().is_none());

    control.request_shutdown();
    measured_shutdown(&mut callback, &mut actual);
    control.reclaim();
    Ok(())
}

#[test]
fn legacy_fallback_owns_persistent_stateful_renderer_off_callback() -> Result<(), Box<dyn Error>> {
    let mut editor = Editor::new(Project::new(RATE)?, 16)?;
    let asset = match editor.execute(Command::AddAsset {
        path: "memory/fallback-stateful.wav".into(),
        sample_rate: RATE,
        frames: 256,
    })? {
        Created::Asset(asset) => asset,
        _ => return Err("expected audio asset".into()),
    };
    let track = match editor.execute(Command::AddTrack {
        name: "audio".into(),
    })? {
        Created::Track(track) => track,
        _ => return Err("expected audio track".into()),
    };
    editor.execute(Command::PlaceClip {
        track,
        asset,
        start: 0,
        duration: 1_000_000,
        source_offset: 0,
        source_length: 256,
        looping: true,
    })?;
    editor.execute(Command::SetLoopRegion {
        start: 0,
        end: 256,
        enabled: true,
    })?;
    let utility = hex_engine::patch_native_device_parameter(
        &hex_engine::default_native_device_state(DeviceTypeId::new("hex.utility")?)?,
        ParameterId::new(11)?,
        ParameterValue::Bool(true),
    )?;
    let device = match editor.execute(Command::InsertDevice {
        track,
        before: None,
        state: utility,
    })? {
        Created::Device(device) => device,
        _ => return Err("expected Utility device".into()),
    };
    let media = BTreeMap::from([(
        asset,
        Audio::new(
            RATE,
            (0..256)
                .map(|index| {
                    let phase = index as f32 * 0.137;
                    [0.3 + phase.sin() * 0.2, -0.25 + (phase * 0.73).cos() * 0.31]
                })
                .collect(),
        )?,
    )]);
    let compiled = Arc::new(hex_engine::compile::project(editor.project(), &media)?);

    let mut oracle = Playback::from_shared(Arc::clone(&compiled));
    oracle.apply(PlaybackCommand::Play);
    let mut expected = [[0.0; 2]; 512];
    oracle.render(&mut expected)?;

    let mut partitioned = Playback::from_shared(Arc::clone(&compiled));
    partitioned.apply(PlaybackCommand::Play);
    let mut partitioned_output = [[0.0; 2]; 512];
    for chunk in partitioned_output.chunks_exact_mut(128) {
        partitioned.render(chunk)?;
    }
    assert_eq!(partitioned_output, expected);

    let callback_playback = Playback::from_shared(Arc::clone(&compiled));
    let (mut control, mut callback) = prepared::channel(RATE)?;
    control
        .publish(callback_playback)
        .map_err(|_| "legacy fallback publication failed")?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "legacy fallback Play admission failed")?;
    let mut output = [[0.0; 2]; 128];
    for block in expected.chunks_exact(128) {
        measured_block(&mut callback, &mut output);
        assert_eq!(output, block);
    }

    let changed_state = hex_engine::patch_native_device_parameter(
        editor.project().track(track).unwrap().devices()[0].state(),
        ParameterId::new(12)?,
        ParameterValue::Bool(true),
    )?;
    editor.execute(Command::ReplaceDeviceState {
        device,
        state: changed_state,
    })?;
    let changed_project = Arc::new(hex_engine::compile::project(editor.project(), &media)?);
    let mut inherited = Playback::from_shared(Arc::clone(&changed_project));
    inherited
        .try_inherit_runtime_from(&mut oracle)
        .map_err(|_| "legacy fallback compatible handoff was refused")?;
    let mut expected_handoff = [[0.0; 2]; 128];
    inherited.render(&mut expected_handoff)?;
    control
        .publish(Playback::from_shared(Arc::clone(&changed_project)))
        .map_err(|_| "legacy fallback compatible replacement failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_handoff);

    oracle = inherited;
    oracle.apply(PlaybackCommand::Seek(0));
    let mut expected_seek = [[0.0; 2]; 128];
    oracle.render(&mut expected_seek)?;
    control
        .command(PlaybackCommand::Seek(0))
        .map_err(|_| "legacy fallback seek admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_seek);

    oracle.apply(PlaybackCommand::Stop);
    let mut expected_stop = [[1.0; 2]; 128];
    oracle.render(&mut expected_stop)?;
    control
        .command(PlaybackCommand::Stop)
        .map_err(|_| "legacy fallback Stop admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_stop);
    oracle.apply(PlaybackCommand::Play);
    let mut expected_restart = [[0.0; 2]; 128];
    oracle.render(&mut expected_restart)?;
    control
        .command(PlaybackCommand::Play)
        .map_err(|_| "legacy fallback resumed Play admission failed")?;
    measured_block(&mut callback, &mut output);
    assert_eq!(output, expected_restart);

    let empty = Arc::new(hex_engine::compile::project(
        &Project::new(RATE)?,
        &BTreeMap::new(),
    )?);
    control
        .publish(Playback::from_shared(empty))
        .map_err(|_| "legacy fallback retirement publication failed")?;
    measured_block(&mut callback, &mut output);
    let mut reclaimed = 0;
    let retired = measure(|| reclaimed = control.reclaim());
    assert_eq!(reclaimed, 2);
    assert_eq!(retired.count_total, 0);
    assert!(retired.count_current < 0);
    assert!(retired.bytes_current < 0);
    control.request_shutdown();
    measured_shutdown(&mut callback, &mut output);
    control.reclaim();
    Ok(())
}
