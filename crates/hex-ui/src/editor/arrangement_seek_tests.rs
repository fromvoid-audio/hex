use super::{
    PendingSeek, SeekIntent, SeekStage, ToggleAction, cancel_published_seek,
    capture_arrangement_fence, desired_playing_with_pending, latch_latest_intent, requires_preroll,
    resolve_prepared_seek_terminal, routes_are_current, toggle_action,
};
use hex_audio::prepared::{
    self, FreshRuntimeCancellationResult, PublicationPolicy, SessionPendingRoute,
    SessionRouteEntry, SessionRouteOwner, SessionRouteTarget,
};
use hex_engine::{NativeDeviceClass, PreparedRevision, SineTable, playback::Playback};
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, Editor, Project};
use hex_session::{
    arrangement_seek::{ArrangementSeekMode, ArrangementSeekSpec},
    service::Service,
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn one_track_project() -> Project {
    let mut editor =
        Editor::new(Project::new(48_000).expect("project"), 8).expect("project editor");
    editor
        .execute(Command::AddTrack {
            name: "arrangement".into(),
        })
        .expect("track");
    editor.project().clone()
}

fn stateful_project() -> Project {
    let mut editor =
        Editor::new(Project::new(48_000).expect("project"), 8).expect("project editor");
    let track = match editor
        .execute(Command::AddTrack {
            name: "arrangement".into(),
        })
        .expect("track")
    {
        hex_project::Created::Track(track) => track,
        _ => panic!("track result"),
    };
    let catalog = hex_engine::native_device_catalog_for_class(NativeDeviceClass::Effect)
        .expect("effect catalog");
    let utility = catalog
        .iter()
        .find(|entry| entry.name == "utility")
        .expect("utility device");
    let state = hex_engine::default_native_device_state(utility.type_id).expect("utility default");
    let dc_remove = utility
        .parameters
        .iter()
        .find(|parameter| parameter.label == "dc remove")
        .expect("dc remove parameter");
    let state =
        hex_engine::patch_native_device_parameter(&state, dc_remove.id, ParameterValue::Bool(true))
            .expect("stateful utility state");
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .expect("insert stateful utility");
    editor.project().clone()
}

fn active_arrangement_control() -> (prepared::Control, prepared::Callback, Project, u64) {
    let project = one_track_project();
    let prepared = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("prepared revision"),
    );
    let playback =
        Playback::try_from_prepared_with_session(Arc::clone(&prepared)).expect("session playback");
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    let audio_revision = control.publish(playback).expect("publish arrangement");
    let mut block = [[0.0_f32; 2]; 128];
    callback.begin_block();
    callback.render(&mut block).expect("render route snapshot");
    (control, callback, project, audio_revision)
}

#[test]
fn exact_active_all_arrangement_snapshot_captures_fresh_fence() {
    let (control, _callback, project, audio_revision) = active_arrangement_control();
    let fence = capture_arrangement_fence(&control, &project, audio_revision)
        .expect("current Arrangement graph is seekable");
    assert_eq!(fence.expected_active_revision(), audio_revision);
}

#[test]
fn route_gate_rejects_pending_or_non_arrangement_owners() {
    let project = one_track_project();
    let track_id = project.tracks()[0].id().value();
    let arrangement = SessionRouteEntry {
        track_id,
        owner: SessionRouteOwner::Arrangement,
        active_launch_beat_raw: None,
        pending: None,
    };
    assert!(routes_are_current(&[arrangement], &project));

    let session = SessionRouteEntry {
        owner: SessionRouteOwner::Session { clip_id: 3 },
        ..arrangement
    };
    assert!(!routes_are_current(&[session], &project));

    let pending = SessionRouteEntry {
        pending: Some(SessionPendingRoute {
            target: SessionRouteTarget::Arrangement,
            performance_frame: 64,
            beat_raw: 0,
        }),
        ..arrangement
    };
    assert!(!routes_are_current(&[pending], &project));
    assert!(!routes_are_current(&[], &project));
}

#[test]
fn route_gate_refuses_audio_or_revision_mismatch() {
    let (control, _callback, project, audio_revision) = active_arrangement_control();
    assert!(capture_arrangement_fence(&control, &project, audio_revision + 1).is_none());
    let mut changed_project = project.clone();
    let mut editor = Editor::new(changed_project, 8).expect("project editor");
    editor
        .execute(Command::AddTrack {
            name: "other".into(),
        })
        .expect("second track");
    changed_project = editor.project().clone();
    assert!(capture_arrangement_fence(&control, &changed_project, audio_revision).is_none());
}

#[test]
fn faulted_or_shutdown_audio_cannot_admit_a_prepared_seek() {
    let (control, _callback, project, revision) = active_arrangement_control();
    for flag in [
        &control.status.device_fault,
        &control.status.render_fault,
        &control.status.shutdown_complete,
    ] {
        flag.store(true, std::sync::atomic::Ordering::Release);
        assert!(capture_arrangement_fence(&control, &project, revision).is_none());
        flag.store(false, std::sync::atomic::Ordering::Release);
        assert!(capture_arrangement_fence(&control, &project, revision).is_some());
    }
}

#[test]
fn prepared_revision_selects_analytic_or_preroll_route() {
    let project = one_track_project();
    let prepared = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("prepared revision"),
    );
    assert_eq!(requires_preroll(Some(&prepared)), Some(false));
    assert_eq!(requires_preroll(None), None);
}

#[test]
fn queued_play_intent_controls_the_next_toggle_and_seek() {
    assert!(desired_playing_with_pending(false, Some(true), None));
    assert!(desired_playing_with_pending(false, None, Some(true)));
    assert!(!desired_playing_with_pending(true, Some(false), None));
    assert!(desired_playing_with_pending(true, None, None));
    assert!(desired_playing_with_pending(false, Some(false), Some(true)));
}

#[test]
fn second_spacebar_cancels_pending_stateful_play_instead_of_restarting_it() {
    assert_eq!(
        toggle_action(false, None, None, 24_000, None),
        ToggleAction::Unavailable,
        "an unknown source must not use the direct fast-seek path"
    );
    let first = toggle_action(false, None, None, 24_000, Some(true));
    assert_eq!(first, ToggleAction::PreparedPlayFrom(24_000));

    // Audio has not admitted PlayFrom yet, so its actual transport bit is
    // still false. The pending intent must nevertheless make this Pause.
    let second = toggle_action(false, Some(true), None, 24_000, Some(true));
    assert_eq!(second, ToggleAction::Pause);

    // A seek while that intent remains pending inherits the playing state.
    assert!(desired_playing_with_pending(false, Some(true), None));
}

#[test]
fn stateful_seek_candidate_uses_captured_fence_and_latest_intent_waits_for_terminal() {
    let mut snapshot = stateful_project().snapshot();
    snapshot.loop_region = Some(hex_project::LoopRegion::new(128, 512, true).expect("loop"));
    let project = Project::from_snapshot(snapshot).expect("stateful looping project");
    let prepared = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("stateful prepared revision"),
    );
    assert!(prepared.arrangement_seek_requires_preroll());
    let (mut control, mut callback) = prepared::channel(48_000).expect("audio channel");
    let original = Playback::try_from_prepared_with_session(Arc::clone(&prepared))
        .expect("original session playback");
    let active_revision = control.publish(original).expect("initial graph");
    let mut block = [[0.0_f32; 2]; 128];
    callback.begin_block();
    callback.render(&mut block).expect("initial graph apply");
    let fence = capture_arrangement_fence(&control, &project, active_revision)
        .expect("admission-time route fence");

    let intent = SeekIntent {
        destination_frame: 256,
        start_playing: true,
        project_generation: 7,
        project_revision: 3,
        prepared_revision: Arc::clone(&prepared),
    };
    let mut pending = PendingSeek::from_intent(1, &intent, fence);
    let service = Service::new(48_000, 8, 1 << 20).expect("session service");
    let request_id = service
        .request_arrangement_seek(ArrangementSeekSpec {
            mode: ArrangementSeekMode::Interactive,
            generation: intent.project_generation,
            revision: intent.project_revision,
            prepared_revision: Arc::clone(&prepared),
            destination_frame: intent.destination_frame,
            work_budget_frames: intent.destination_frame,
            start_playing: intent.start_playing,
        })
        .expect("admit seek worker request");
    pending.request_id = request_id;
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "interactive seek worker timed out"
        );
        thread::yield_now();
    };
    assert!(pending.matches_result(&result));
    let playback = result.result.expect("prepared stateful playback");
    pending.stage = SeekStage::CandidateReady;

    let mut latched = None;
    latch_latest_intent(
        &mut latched,
        SeekIntent {
            destination_frame: 512,
            ..intent.clone()
        },
    );
    latch_latest_intent(
        &mut latched,
        SeekIntent {
            destination_frame: 768,
            ..intent
        },
    );
    assert!(pending.defers_replacement());
    assert_eq!(
        pending.destination_frame, 256,
        "in-flight candidate stays fixed"
    );
    assert_eq!(
        latched.as_ref().map(|intent| intent.destination_frame),
        Some(768)
    );

    let candidate_revision = control
        .publish_with_policy(
            playback,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("candidate passes captured fence");
    callback.begin_block();
    callback
        .render(&mut block)
        .expect("stateful candidate apply");
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        candidate_revision
    );
    pending.stage = SeekStage::Published {
        audio_revision: candidate_revision,
    };
    let mut pending_slot = Some(pending);
    let mut status = String::new();
    let mut error = None;
    assert!(resolve_prepared_seek_terminal(
        &mut pending_slot,
        &mut status,
        &mut error,
        candidate_revision,
        None,
    ));
    assert!(pending_slot.is_none());
    assert_eq!(status, "cursor ready");
    assert_eq!(error, None);

    let next_fence = capture_arrangement_fence(&control, &project, candidate_revision)
        .expect("latched intent captures a new fence after terminal apply");
    assert_eq!(next_fence.expected_active_revision(), candidate_revision);
    assert_eq!(
        latched.take().map(|intent| intent.destination_frame),
        Some(768)
    );
    control.reclaim();
}

#[test]
fn invalidating_published_seek_cancels_callback_candidate_and_waits_for_terminal() {
    let (mut control, mut callback, project, active_revision) = active_arrangement_control();
    let prepared = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .expect("prepared revision"),
    );
    let fence = capture_arrangement_fence(&control, &project, active_revision)
        .expect("current Arrangement graph is seekable");
    let playback = Playback::try_from_prepared_with_session(Arc::clone(&prepared))
        .expect("fresh session playback");
    let audio_revision = control
        .publish_with_policy(
            playback,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .expect("publish conditional seek candidate");
    let intent = SeekIntent {
        destination_frame: 256,
        start_playing: true,
        project_generation: 4,
        project_revision: 7,
        prepared_revision: prepared,
    };
    let mut pending = PendingSeek::from_intent(11, &intent, fence);
    pending.stage = SeekStage::Published { audio_revision };

    assert_eq!(
        cancel_published_seek(Some(&control), &mut pending),
        Some(FreshRuntimeCancellationResult::Cancelled)
    );
    assert!(pending.cancelled);
    callback.begin_block();
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        active_revision,
        "the stale seek graph never becomes active"
    );
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            audio_revision,
            hex_engine::playback::PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );

    let mut pending_slot = Some(pending);
    let mut status = String::from("arrangement seek ready · waiting for audio");
    let mut error = None;
    assert!(resolve_prepared_seek_terminal(
        &mut pending_slot,
        &mut status,
        &mut error,
        active_revision,
        control.status.rejected_revision(),
    ));
    assert!(pending_slot.is_none());
    assert_eq!(status, "arrangement seek ready · waiting for audio");
    assert_eq!(error, None, "intentional cancellation is not an error");
    assert_eq!(
        control.reclaim(),
        1,
        "candidate ownership returns to control"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
