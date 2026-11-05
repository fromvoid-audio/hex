use hex_audio::prepared;
use hex_engine::{
    Audio, Clip, PlaybackActionRejection, PlaybackHandoffRejection, Project,
    SessionCapabilityStatus, SessionPlaybackCommand, Track,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{BeatTime, Command, Created, Editor, Project as CanonicalProject};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering::Acquire},
};

fn graph(rate: u32, sample: f32) -> Playback {
    let audio = Audio::new(rate, vec![[sample; 2]]).unwrap();
    Playback::new(
        Project::new(
            rate,
            vec![Track::new(vec![Clip::new(audio, 0, 100, true).unwrap()], 1.0, false).unwrap()],
        )
        .unwrap(),
    )
}
fn publish(control: &mut prepared::Control, playback: Playback) -> u64 {
    match control.publish(playback) {
        Ok(revision) => revision,
        Err(error) => panic!("{}", error.reason),
    }
}

fn session_project() -> (CanonicalProject, hex_project::TrackId, hex_project::ClipId) {
    let mut editor = Editor::new(CanonicalProject::new(48_000).unwrap(), 8).unwrap();
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "keys".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let arrangement = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected arrangement clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 48,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    let scene = match editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "session keys".into(),
            source_length: BeatTime::from_quarters(2).unwrap(),
        })
        .unwrap()
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 72,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    // This track has no Session cell, so scene launches must retain its
    // Arrangement source while the first track plays independent Session MIDI.
    let companion = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "arrangement bass".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected companion track"),
    };
    let bass = match editor
        .execute(Command::PlaceMidiClip {
            track: companion,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected companion clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip: bass,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(4).unwrap(),
            pitch: 36,
            channel: 0,
            velocity: 63,
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 48_000,
            enabled: true,
        })
        .unwrap();
    (editor.project().clone(), track, clip)
}

fn prepare_session(project: &CanonicalProject) -> Arc<hex_engine::PreparedRevision> {
    Arc::new(
        hex_engine::PreparedRevision::prepare(
            project,
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    )
}

fn session_revision() -> (
    Arc<hex_engine::PreparedRevision>,
    hex_project::TrackId,
    hex_project::ClipId,
) {
    let (project, track, clip) = session_project();
    (prepare_session(&project), track, clip)
}

fn unknown_session_clip() -> hex_project::ClipId {
    let mut editor = Editor::new(CanonicalProject::new(48_000).unwrap(), 4).unwrap();
    let track = match editor
        .execute(Command::AddMidiTrack {
            name: "other keys".into(),
        })
        .unwrap()
    {
        Created::Track(track) => track,
        _ => panic!("expected instrument track"),
    };
    let scene = match editor
        .execute(Command::AddScene {
            name: "removed scene".into(),
        })
        .unwrap()
    {
        Created::Scene(scene) => scene,
        _ => panic!("expected scene"),
    };
    let clip = match editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "removed clip".into(),
            source_length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    {
        Created::SessionClip(clip) => clip,
        _ => panic!("expected Session clip"),
    };
    editor.execute(Command::RemoveScene(scene)).unwrap();
    clip
}
#[test]
fn replacement_preserves_transport_and_returns_old_graph_off_callback() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    assert_eq!(publish(&mut control, graph(48_000, 0.25)), 1);
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 3];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]; 3]);
    assert_eq!(publish(&mut control, graph(48_000, 0.5)), 2);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert_eq!(control.status.position.load(Acquire), 3);
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.5; 2]; 3]);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    assert!(control.status.shutdown_complete.load(Acquire));
    assert_eq!(control.reclaim(), 1);
}
#[test]
fn full_retirement_queue_backpressures_replacement_without_losing_graphs() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    for revision in 1..=3 {
        assert_eq!(
            publish(&mut control, graph(48_000, revision as f32)),
            revision
        );
        callback.begin_block();
    }
    assert_eq!(publish(&mut control, graph(48_000, 4.0)), 4);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 3);
    let rejected = control.publish(graph(48_000, 5.0)).err().unwrap();
    assert_eq!(rejected.reason, "playback publication already pending");
    assert_eq!(control.reclaim(), 2);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 4);
    assert_eq!(publish(&mut control, rejected.playback), 5);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 5);
    assert_eq!(control.reclaim(), 2);
    control.request_shutdown();
    callback.begin_block();
    assert_eq!(control.reclaim(), 1);
}
#[test]
fn rate_rejection_and_priority_stop_preserve_ordering() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    assert_eq!(
        control.publish(graph(44_100, 0.1)).err().unwrap().reason,
        "playback rate differs from device"
    );
    assert_eq!(publish(&mut control, graph(48_000, 0.1)), 1);
    for _ in 0..32 {
        control.command(PlaybackCommand::Play).unwrap();
    }
    assert!(control.command(PlaybackCommand::Play).is_err());
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    let mut output = [[9.0; 2]; 4];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 4]);
    control.command(PlaybackCommand::Seek(10)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(control.status.position.load(Acquire), 14);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn route_snapshot_tracks_launch_pending_cancel_stop_and_revision_capability() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, track, clip) = session_revision();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap(),
    );
    callback.begin_block();
    let mut entries = [prepared::SessionRouteEntry {
        track_id: 0,
        owner: prepared::SessionRouteOwner::Silence,
        active_origin: None,
        audio_launch_frame: None,
        pending: None,
    }; 2];
    let initial = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(initial.audio_revision, 1);
    assert_eq!(initial.capability, SessionCapabilityStatus::Available);
    assert_eq!(initial.count, 2);
    assert_eq!(entries[0].track_id, track.value());
    assert_eq!(entries[0].owner, prepared::SessionRouteOwner::Arrangement);
    let mut short = [prepared::SessionRouteEntry::default(); 1];
    assert!(control.status.session_routes(&mut short).is_none());

    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    let launched = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(
        launched.route_state_generation,
        initial.route_state_generation + 1
    );
    assert_eq!(
        entries[0].owner,
        prepared::SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    assert_eq!(
        entries[0].active_origin,
        Some(hex_engine::SessionMidiOrigin::launch(
            hex_project::BeatTime::ZERO
        ))
    );

    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 128];
    callback.render(&mut output).unwrap();
    let after_unchanged_render = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(
        after_unchanged_render.route_state_generation,
        launched.route_state_generation
    );
    assert_eq!(
        after_unchanged_render.performance_at_update,
        launched.performance_at_update
    );
    assert_eq!(control.status.performance_frame.load(Acquire), 128);

    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::FourQuarter,
        })
        .unwrap();
    callback.begin_block();
    let pending = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(
        pending.route_state_generation,
        launched.route_state_generation + 1
    );
    assert_eq!(entries[0].pending.unwrap().performance_frame, 96_000);
    assert_eq!(entries[0].pending.unwrap().beat_raw, 4_u64 << 32);

    control
        .session_command_with_ticket(SessionPlaybackCommand::CancelPending { track: None })
        .unwrap();
    callback.begin_block();
    let cancelled = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(
        cancelled.route_state_generation,
        pending.route_state_generation + 1
    );
    assert_eq!(entries[0].pending, None);

    control.stop();
    callback.begin_block();
    let stopped = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(stopped.performance_at_update, 0);
    control
        .session_command_with_ticket(SessionPlaybackCommand::Seek(50_000))
        .unwrap();
    control
        .session_command_with_ticket(SessionPlaybackCommand::BackToArrangement {
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    let before_fallback = control.status.session_routes(&mut entries).unwrap();
    assert!(
        entries[..before_fallback.count]
            .iter()
            .all(|route| route.owner == prepared::SessionRouteOwner::Arrangement)
    );
    control
        .session_command_with_ticket(SessionPlaybackCommand::Stop)
        .unwrap();
    callback.begin_block();
    let before_unavailable = control.status.session_routes(&mut entries).unwrap();
    let before_unavailable_entries = entries;

    publish(
        &mut control,
        Playback::from_prepared_with_session_unavailable(
            revision,
            hex_engine::SessionPlaybackError::RevisionTrackMismatch,
        ),
    );
    callback.begin_block();
    let unavailable = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(unavailable, before_unavailable);
    assert_eq!(entries, before_unavailable_entries);
    assert_eq!(unavailable.audio_revision, 1);
    assert_eq!(unavailable.capability, SessionCapabilityStatus::Available);
    assert_eq!(unavailable.count, before_unavailable.count);
    assert_eq!(
        control.status.rejected_revision(),
        Some((2, PlaybackHandoffRejection::SessionRuntime))
    );
    assert_eq!(control.reclaim(), 1);

    control.request_shutdown();
    callback.begin_block();
    let closed = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(closed.audio_revision, 0);
    assert_eq!(closed.count, 0);
    assert_eq!(closed.capability, SessionCapabilityStatus::Legacy);
    control.reclaim();

    let (fresh_control, _fresh_callback) = prepared::channel(48_000).unwrap();
    let fresh = fresh_control.status.session_routes(&mut entries).unwrap();
    assert_eq!(fresh.audio_revision, 0);
    assert_eq!(fresh.capability, SessionCapabilityStatus::Legacy);
    assert_eq!(fresh.count, 0);
}

#[test]
fn launch_and_play_is_atomic_for_unknown_clip_and_nonlaunch_payloads() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, track, clip) = session_revision();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(revision).unwrap(),
    );
    callback.begin_block();
    let unknown = unknown_session_clip();
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip: unknown,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action().unwrap().1,
        PlaybackActionRejection::UnknownClip
    );

    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    control.stop();
    let invalid_after_stop = control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip: unknown,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action(),
        Some((invalid_after_stop, PlaybackActionRejection::UnknownClip))
    );

    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    let mut routes = [prepared::SessionRouteEntry {
        track_id: 0,
        owner: prepared::SessionRouteOwner::Silence,
        active_origin: None,
        audio_launch_frame: None,
        pending: None,
    }; 2];
    let snapshot = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(snapshot.count, 2);
    assert_eq!(routes[0].track_id, track.value());
    assert_eq!(
        routes[0].owner,
        prepared::SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );

    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::Stop)
        .unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action().unwrap().1,
        PlaybackActionRejection::InvalidSessionAction
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn accepted_reorder_publishes_route_entries_in_new_project_order_by_stable_id() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (project, _track, clip) = session_project();
    let original = prepare_session(&project);
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(Arc::clone(&original)).unwrap(),
    );
    callback.begin_block();
    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();

    let mut snapshot = project.snapshot();
    snapshot.tracks.reverse();
    let reordered = CanonicalProject::from_snapshot(snapshot).unwrap();
    let reordered_revision = prepare_session(&reordered);
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(reordered_revision).unwrap(),
    );
    callback.begin_block();

    let mut entries = [prepared::SessionRouteEntry {
        track_id: 0,
        owner: prepared::SessionRouteOwner::Silence,
        active_origin: None,
        audio_launch_frame: None,
        pending: None,
    }; 2];
    let routes = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(routes.audio_revision, 2);
    assert_eq!(routes.count, 2);
    assert_eq!(entries[0].track_id, reordered.tracks()[0].id().value());
    assert_eq!(entries[1].track_id, reordered.tracks()[1].id().value());
    assert_eq!(
        entries[1].owner,
        prepared::SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
#[test]
fn shutdown_drains_active_and_pending_despite_full_retirement_queue() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    for _ in 0..3 {
        publish(&mut control, graph(48_000, 0.1));
        callback.begin_block();
    }
    publish(&mut control, graph(48_000, 0.2));
    control.request_shutdown();
    callback.begin_block();
    assert!(!control.status.shutdown_complete.load(Acquire));
    assert!(control.publish(graph(48_000, 0.3)).is_err());
    assert!(control.command(PlaybackCommand::Play).is_err());
    assert_eq!(control.reclaim(), 2);
    callback.begin_block();
    assert!(control.status.shutdown_complete.load(Acquire));
    assert_eq!(control.reclaim(), 2);
    let mut output = [[9.0; 2]; 1];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 1]);
}

#[test]
fn transport_before_initial_publication_is_rejected_not_silently_lost() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    assert_eq!(
        control.command(PlaybackCommand::Play),
        Err(PlaybackCommand::Play)
    );
    callback.begin_block();
    let mut output = [[9.0; 2]; 1];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 1]);
    publish(&mut control, graph(48_000, 0.25));
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]; 1]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn command_tickets_acknowledge_consumption_after_transport_and_stop_discard() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    assert!(
        control
            .command_with_ticket(PlaybackCommand::Seek(9))
            .is_err()
    );
    publish(&mut control, graph(48_000, 0.25));
    let first = control
        .command_with_ticket(PlaybackCommand::Seek(9))
        .unwrap();
    assert_eq!(first, 1, "failed admission must not advance tickets");
    assert_eq!(control.status.consumed_command.load(Acquire), 0);
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), first);
    assert_eq!(control.status.position.load(Acquire), 9);
    assert!(!control.status.playing.load(Acquire));

    let mut last = first;
    for frame in 0..32 {
        last = control
            .command_with_ticket(PlaybackCommand::Seek(frame))
            .unwrap();
    }
    assert!(
        control
            .command_with_ticket(PlaybackCommand::Seek(99))
            .is_err()
    );
    assert_eq!(control.status.consumed_command.load(Acquire), first);
    control.stop();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), last);
    assert_eq!(control.status.position.load(Acquire), 0);
    let next = control
        .command_with_ticket(PlaybackCommand::Seek(21))
        .unwrap();
    assert_eq!(next, last + 1);
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), play);
    assert_eq!(control.status.position.load(Acquire), 21);
    assert!(control.status.playing.load(Acquire));
    let mut output = [[0.0; 2]; 7];
    callback.render(&mut output).unwrap();
    assert_eq!(control.status.position.load(Acquire), 28);
    assert_eq!(control.status.consumed_command.load(Acquire), play);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn commands_admitted_after_stop_follow_it_in_callback_order() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    publish(&mut control, graph(48_000, 0.25));
    let old_play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    control.stop();
    let play_after_stop = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        play_after_stop
    );
    assert!(old_play < play_after_stop);

    control.stop();
    control.command(PlaybackCommand::Pause).unwrap();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn stop_discards_prior_session_route_then_accepts_atomic_launch_and_play() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, track, clip) = session_revision();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(revision).unwrap(),
    );
    control
        .command_with_ticket(PlaybackCommand::Seek(9))
        .unwrap();
    let old_session_launch = control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    control.stop();
    callback.begin_block();
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        old_session_launch
    );
    assert!(control.status.rejected_action().is_none());
    let mut entries = [prepared::SessionRouteEntry::default(); 4];
    let stopped_routes = control.status.session_routes(&mut entries).unwrap();
    let stopped_route = entries[..stopped_routes.count]
        .iter()
        .find(|entry| entry.track_id == track.value())
        .unwrap();
    assert_eq!(
        stopped_route.owner,
        prepared::SessionRouteOwner::Arrangement
    );
    assert!(stopped_route.pending.is_none());

    let launch_and_play = control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        launch_and_play
    );
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(control.status.playing.load(Acquire));
    assert!(control.status.rejected_action().is_none());
    let routes = control.status.session_routes(&mut entries).unwrap();
    let route = entries[..routes.count]
        .iter()
        .find(|entry| entry.track_id == track.value())
        .unwrap();
    assert_eq!(
        route.owner,
        prepared::SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    assert!(route.pending.is_none());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn callback_applies_queued_session_actions_and_reports_dual_cursors() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, track, clip) = session_revision();
    let playback = Playback::try_from_prepared_with_session(revision).unwrap();
    assert_eq!(publish(&mut control, playback), 1);
    let launch = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), play);
    assert!(control.status.rejected_action().is_none());
    assert_eq!(
        control.status.session_capability(),
        Some(SessionCapabilityStatus::Available)
    );

    let mut output = vec![[0.0; 2]; 48_128];
    callback.render(&mut output).unwrap();
    assert!(output.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    assert_eq!(control.status.performance_frame.load(Acquire), 48_128);
    assert_eq!(control.status.position.load(Acquire), 128);

    let stop_track = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::StopTrack {
            track,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), stop_track);
    assert!(control.status.rejected_action().is_none());
    assert!(launch < play);
}

#[test]
fn queued_scene_stop_and_arrangement_return_match_headless_pcm() {
    use hex_engine::{LaunchQuantization::Immediate, SessionPlayback, SessionPlaybackCommand};
    let (revision, track, _) = session_revision();
    let scene = revision.session().scenes().first().unwrap().id;
    let mut expected = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    expected.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(revision).unwrap(),
    );
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    for (command, frames) in [
        (
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: Immediate,
            },
            48_017,
        ),
        (
            SessionPlaybackCommand::StopTrack {
                track,
                quantization: Immediate,
            },
            517,
        ),
        (
            SessionPlaybackCommand::BackToArrangement {
                quantization: Immediate,
            },
            49_037,
        ),
        (
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: Immediate,
            },
            313,
        ),
    ] {
        expected.apply(command).unwrap();
        let ticket = control.session_command_with_ticket(command).unwrap();
        callback.begin_block();
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
        assert!(control.status.rejected_action().is_none());
        let mut reference = vec![[0.0; 2]; frames];
        let mut actual = reference.clone();
        expected.render(&mut reference).unwrap();
        // Exercise device-style partitions rather than mirroring the oracle block.
        for block in actual.chunks_mut(127) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
        assert_eq!(actual, reference);
        assert_eq!(
            control.status.position.load(Acquire),
            expected.arrangement_frame()
        );
        assert_eq!(
            control.status.performance_frame.load(Acquire),
            expected.performance_frame()
        );
    }
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn callback_revision_transfer_matches_kernel_after_tempo_and_source_edits() {
    use hex_engine::{LaunchQuantization::Immediate, SessionPlayback, SessionPlaybackCommand};
    let (project, track, clip) = session_project();
    let mut editor = Editor::new(project, 8).unwrap();
    let initial = prepare_session(editor.project());
    let mut expected = SessionPlayback::new(Arc::clone(&initial)).unwrap();
    let launch = SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: Immediate,
    };
    expected.apply(launch).unwrap();
    expected.apply(SessionPlaybackCommand::Play).unwrap();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(initial).unwrap(),
    );
    control.session_command_with_ticket(launch).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut reference = vec![[0.0; 2]; 51_337];
    let mut actual = reference.clone();
    expected.render(&mut reference).unwrap();
    callback.render(&mut actual).unwrap();
    assert_eq!(actual, reference);

    for (index, edit) in [
        Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(90_000_000).unwrap(),
        },
        Command::SetSessionMidiClipSource {
            clip,
            source_length: BeatTime::from_quarters(2).unwrap(),
            source_start: BeatTime::ONE,
            loop_start: BeatTime::ONE,
            loop_end: BeatTime::from_quarters(2).unwrap(),
            looping: true,
        },
        Command::Gain { track, gain: 0.5 },
    ]
    .into_iter()
    .enumerate()
    {
        editor.execute(edit).unwrap();
        let revision = prepare_session(editor.project());
        let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        candidate.try_inherit_state_from(&mut expected).unwrap();
        expected = candidate;
        let revision_number = publish(
            &mut control,
            Playback::try_from_prepared_with_session(revision).unwrap(),
        );
        assert_eq!(revision_number, index as u64 + 2);
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            revision_number
        );
        assert!(control.status.rejected_revision().is_none());
        assert_eq!(
            control.status.performance_frame.load(Acquire),
            expected.performance_frame()
        );
        let mut reference = vec![[0.0; 2]; 49_517];
        let mut actual = reference.clone();
        expected.render(&mut reference).unwrap();
        for block in actual.chunks_mut(113) {
            callback.begin_block();
            callback.render(block).unwrap();
        }
        assert_eq!(actual, reference);
        assert_eq!(
            control.status.position.load(Acquire),
            expected.arrangement_frame()
        );
        assert_eq!(control.reclaim(), 1);
    }
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn unavailable_capability_and_failed_session_action_have_truthful_acknowledgments() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, _track, clip) = session_revision();
    let playback = Playback::from_prepared_with_session_unavailable(
        Arc::clone(&revision),
        hex_engine::SessionPlaybackError::TempoConversion,
    );
    assert_eq!(publish(&mut control, playback), 1);
    let ticket = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    let play_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), play_ticket);
    assert!(control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::SessionUnavailable))
    );
    assert_eq!(
        control.status.session_capability(),
        Some(SessionCapabilityStatus::TimingUnavailable)
    );
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::SessionUnavailable))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn ordinary_prepared_replacement_preserves_active_session_state_and_pcm() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, _track, clip) = session_revision();
    let mut oracle = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap(),
    );
    control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    oracle
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    oracle.apply(PlaybackCommand::Play);
    callback.begin_block();
    let mut before = [[0.0; 2]; 128];
    let mut expected_before = [[0.0; 2]; 128];
    callback.render(&mut before).unwrap();
    oracle.render(&mut expected_before).unwrap();
    assert_eq!(before, expected_before);
    assert!(before.iter().flatten().any(|sample| sample.abs() > 1.0e-6));
    let mut route_entries = [prepared::SessionRouteEntry::default(); 2];
    let accepted_routes = control.status.session_routes(&mut route_entries).unwrap();
    let accepted_entries = route_entries;

    assert_eq!(
        publish(&mut control, Playback::from_prepared(Arc::clone(&revision))),
        2
    );
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(control.status.rejected_revision().is_none());
    let after_replacement = control.status.session_routes(&mut route_entries).unwrap();
    assert_eq!(after_replacement.audio_revision, 2);
    assert_eq!(after_replacement.count, accepted_routes.count);
    assert_eq!(after_replacement.capability, accepted_routes.capability);
    assert_eq!(
        after_replacement.route_state_generation,
        accepted_routes.route_state_generation + 1
    );
    assert_eq!(after_replacement.performance_at_update, 128);
    assert_eq!(route_entries, accepted_entries);
    let mut after = [[0.0; 2]; 128];
    let mut expected_after = [[0.0; 2]; 128];
    callback.render(&mut after).unwrap();
    oracle.render(&mut expected_after).unwrap();
    assert_eq!(after, expected_after);
    assert_ne!(after, [[0.0; 2]; 128]);
    assert_eq!(control.reclaim(), 1);
    assert_eq!(
        publish(
            &mut control,
            Playback::try_from_prepared_with_session(revision).unwrap()
        ),
        3
    );
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 3);

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn midi_cue_callback_ack_retains_clocks_pause_and_unrelated_route() {
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let (revision, track, clip) = session_revision();
    publish(
        &mut control,
        Playback::try_from_prepared_with_session(revision).unwrap(),
    );
    callback.begin_block();
    control
        .session_command_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 128]).unwrap();
    control.command(PlaybackCommand::Pause).unwrap();
    callback.begin_block();
    let mut entries = [prepared::SessionRouteEntry::default(); 2];
    let before = control.status.session_routes(&mut entries).unwrap();
    let companion = entries[1];
    let performance = control.status.performance_frame.load(Acquire);
    let arrangement = control.status.position.load(Acquire);
    let cue = control
        .session_command_with_ticket(SessionPlaybackCommand::CueMidiClip {
            track,
            clip,
            source_beat: BeatTime::ONE,
            expected_route_generation: before.route_state_generation,
        })
        .unwrap();
    // Admission alone cannot make the UI claim that the source has moved.
    assert_eq!(control.status.session_routes(&mut entries).unwrap(), before);
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(measured.count_total, 0);
        assert_eq!(measured.count_current, 0);
        assert_eq!(measured.bytes_total, 0);
        assert_eq!(measured.bytes_current, 0);
    }
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
    let accepted = control.status.session_routes(&mut entries).unwrap();
    assert_eq!(control.status.consumed_command.load(Acquire), cue);
    assert_eq!(
        accepted.route_state_generation,
        before.route_state_generation + 1
    );
    assert_eq!(
        entries[0].active_origin.unwrap().elapsed_at_anchor,
        BeatTime::ONE
    );
    assert_eq!(entries[1], companion);
    assert_eq!(control.status.performance_frame.load(Acquire), performance);
    assert_eq!(control.status.position.load(Acquire), arrangement);
    assert!(!control.status.playing.load(Acquire));
    callback.render(&mut [[0.0; 2]; 128]).unwrap();
    callback.begin_block();
    assert_eq!(
        control.status.session_routes(&mut entries).unwrap(),
        accepted
    );
    assert_eq!(
        entries[0].active_origin.unwrap().elapsed_at_anchor,
        BeatTime::ONE
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
