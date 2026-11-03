use super::{
    PendingSeek, SeekIntent, SeekStage, ToggleAction, VerifiedSeekCandidateContext,
    cancel_published_seek, capture_arrangement_fence, desired_playing_with_pending,
    latch_latest_intent, requires_preroll, resolve_prepared_seek_terminal, routes_are_current,
    seek_requires_preroll_for_mode, toggle_action, verified_audio_candidate_is_current,
};
use hex_audio::prepared::{
    self, FreshRuntimeCancellationResult, PublicationPolicy, SessionPendingRoute,
    SessionRouteEntry, SessionRouteOwner, SessionRouteTarget,
};
use hex_engine::{NativeDeviceClass, PreparedRevision, SineTable, playback::Playback};
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, Editor, Project};
use hex_session::{arrangement_seek::ArrangementSeekMode, service::Service};
use std::{collections::BTreeMap, sync::Arc, thread, time::Instant};

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

#[test]
fn post_loop_cursor_survives_actual_seek_worker_and_fenced_callback_installation() {
    let mut snapshot = stateful_project().snapshot();
    let region = hex_project::LoopRegion::new(128, 512, true).unwrap();
    snapshot.loop_region = Some(region);
    let project = Project::from_snapshot(snapshot).unwrap();
    let graph = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    assert!(graph.arrangement_seek_requires_preroll());
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let active = control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&graph)).unwrap())
        .unwrap();
    let mut block = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut block).unwrap();
    let fence = capture_arrangement_fence(&control, &project, active).unwrap();
    let service = Service::new(48_000, 8, 1 << 20).unwrap();
    let intent = SeekIntent {
        section: None,
        recovery_generation: None,
        destination_frame: 768,
        start_playing: true,
        project_generation: 7,
        project_revision: 3,
        prepared_revision: Some(Arc::clone(&graph)),
        device_preview_identity: None,
    };
    let request = service
        .request_arrangement_seek(intent.spec().unwrap())
        .unwrap();
    let pending = PendingSeek::from_intent(request, &intent, fence);
    let deadline = crate::test_wait::hang_deadline();
    let result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "actual seek worker timed out");
        thread::yield_now();
    };
    assert!(pending.matches_result(&result));
    let playback = result.result.unwrap();
    assert_eq!(playback.position(), 768);
    let applied = control
        .publish_with_policy(
            playback,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut block).unwrap();
    use std::sync::atomic::Ordering;
    assert_eq!(
        control.status.applied_revision.load(Ordering::Acquire),
        applied
    );
    assert_eq!(control.status.position.load(Ordering::Acquire), 896);
    assert_eq!(graph.arrangement().loop_region(), Some(region));
    control.request_shutdown();
    callback.begin_block();
    callback.render(&mut block).unwrap();
    control.reclaim();
}

#[test]
fn stateful_seek_beyond_the_linear_replay_ceiling_installs_at_its_destination() {
    let project = stateful_project();
    let graph = Arc::new(
        PreparedRevision::prepare(&project, &BTreeMap::new(), Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    assert!(graph.arrangement_seek_requires_preroll());
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    let active = control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&graph)).unwrap())
        .unwrap();
    let mut block = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut block).unwrap();
    let fence = capture_arrangement_fence(&control, &project, active).unwrap();
    let service = Service::new(48_000, 8, 1 << 20).unwrap();
    // About 11:39 at 48 kHz, past the 8_388_608-frame linear export replay budget.
    let destination = hex_engine::MAX_ARRANGEMENT_PREROLL_FRAMES * 4 + 37;
    let intent = SeekIntent {
        section: None,
        recovery_generation: None,
        destination_frame: destination,
        start_playing: true,
        project_generation: 7,
        project_revision: 3,
        prepared_revision: Some(Arc::clone(&graph)),
        device_preview_identity: None,
    };
    let spec = intent.spec().expect("long stateful seek is admitted");
    assert_eq!(spec.destination_frame, destination);
    let request = service.request_arrangement_seek(spec).unwrap();
    let pending = PendingSeek::from_intent(request, &intent, fence);
    let deadline = crate::test_wait::hang_deadline();
    let result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(Instant::now() < deadline, "actual seek worker timed out");
        thread::yield_now();
    };
    assert!(pending.matches_result(&result));
    let playback = result.result.expect("long stateful seek prepares");
    assert_eq!(playback.position(), destination);
    control
        .publish_with_policy(
            playback,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut block).unwrap();
    use std::sync::atomic::Ordering;
    assert_eq!(
        control.status.position.load(Ordering::Acquire),
        destination + 128
    );
    control.request_shutdown();
    callback.begin_block();
    callback.render(&mut block).unwrap();
    control.reclaim();
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
        active_origin: None,
        audio_launch_frame: None,
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
fn verified_seek_uses_direct_route_only_for_current_applied_candidate() {
    let current = VerifiedSeekCandidateContext {
        client_mode: hex_session::DocumentAudioMode::VerifiedStreaming,
        output_identity: Some(7),
        candidate_audio_revision: 11,
        applied_audio_revision: 11,
        candidate_generation: 4,
        current_generation: 4,
        candidate_project_revision: 9,
        current_project_revision: 9,
        applied_project_revision: Some(9),
        has_prepared_revision: false,
        is_canonical_candidate: true,
    };
    assert!(verified_audio_candidate_is_current(current));
    assert_eq!(
        seek_requires_preroll_for_mode(
            hex_session::DocumentAudioMode::VerifiedStreaming,
            None,
            true,
        ),
        Some(false),
        "an applied Verified candidate uses the direct seek path"
    );

    let not_applied = VerifiedSeekCandidateContext {
        applied_audio_revision: 10,
        ..current
    };
    assert!(!verified_audio_candidate_is_current(not_applied));
    assert_eq!(
        seek_requires_preroll_for_mode(
            hex_session::DocumentAudioMode::VerifiedStreaming,
            None,
            false,
        ),
        None,
        "a submitted but unapplied Verified candidate cannot seek"
    );

    for stale in [
        VerifiedSeekCandidateContext {
            output_identity: None,
            ..current
        },
        VerifiedSeekCandidateContext {
            candidate_generation: 3,
            ..current
        },
        VerifiedSeekCandidateContext {
            candidate_project_revision: 8,
            ..current
        },
        VerifiedSeekCandidateContext {
            applied_project_revision: None,
            ..current
        },
        VerifiedSeekCandidateContext {
            has_prepared_revision: true,
            ..current
        },
        VerifiedSeekCandidateContext {
            is_canonical_candidate: false,
            ..current
        },
        VerifiedSeekCandidateContext {
            client_mode: hex_session::DocumentAudioMode::Resident,
            ..current
        },
    ] {
        assert!(
            !verified_audio_candidate_is_current(stale),
            "stale, missing-client, or owning candidates must not use Verified direct seek"
        );
    }

    assert_eq!(
        seek_requires_preroll_for_mode(
            hex_session::DocumentAudioMode::VerifiedStreaming,
            None,
            false,
        ),
        None
    );
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
fn stateful_spacebar_uses_the_transport_queue_and_second_press_pauses() {
    assert_eq!(
        toggle_action(false, None, None, 24_000, None),
        ToggleAction::Unavailable,
        "an unknown source must not use the direct fast-seek path"
    );
    let first = toggle_action(false, None, None, 24_000, Some(true));
    assert_eq!(first, ToggleAction::PlayFrom(24_000));

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
        section: Some(hex_session::arrangement_seek::SectionLaunch {
            region: hex_project::LoopRegion::new(256, 512, true).unwrap(),
            quantization: hex_engine::LaunchQuantization::Quarter,
        }),
        recovery_generation: None,
        destination_frame: 256,
        start_playing: true,
        project_generation: 7,
        project_revision: 3,
        prepared_revision: Some(Arc::clone(&prepared)),
        device_preview_identity: None,
    };
    let mut pending = PendingSeek::from_intent(1, &intent, fence);
    let worker_intent = SeekIntent {
        section: None,
        prepared_revision: None,
        ..intent.clone()
    };
    let worker_pending = PendingSeek::from_intent(2, &worker_intent, fence);
    assert!(
        worker_intent.spec().is_none(),
        "native intent cannot expose a headless graph request"
    );
    assert!(
        worker_pending.defers_replacement(),
        "a second native intent waits for scalar acceptance/refusal before replacing cancellation correlation"
    );
    let service = Service::new(48_000, 8, 1 << 20).expect("session service");
    let request_id = service
        .request_arrangement_seek(intent.spec().expect("captured section worker spec"))
        .expect("admit seek worker request");
    pending.request_id = request_id;
    let deadline = crate::test_wait::hang_deadline();
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
            section: Some(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(512, 768, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::Half,
            }),
            destination_frame: 512,
            ..intent.clone()
        },
    );
    latch_latest_intent(
        &mut latched,
        SeekIntent {
            section: Some(hex_session::arrangement_seek::SectionLaunch {
                region: hex_project::LoopRegion::new(768, 1024, true).unwrap(),
                quantization: hex_engine::LaunchQuantization::EighthTriplet,
            }),
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
            PublicationPolicy::InstallSectionWhenReady {
                fence,
                quantization: hex_engine::LaunchQuantization::Quarter,
            },
        )
        .expect("candidate passes captured fence");
    pending.stage = SeekStage::Published {
        audio_revision: candidate_revision,
    };
    assert_eq!(
        cancel_published_seek(Some(&control), &mut pending),
        Some(FreshRuntimeCancellationResult::Cancelled)
    );
    assert!(pending.cancelled);
    callback.begin_block();
    callback
        .render(&mut block)
        .expect("cancelled first section keeps predecessor");
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        active_revision
    );
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            candidate_revision,
            hex_engine::playback::PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    let mut pending_slot = Some(pending);
    let mut status = String::new();
    let mut error = None;
    assert!(resolve_prepared_seek_terminal(
        &mut pending_slot,
        &mut status,
        &mut error,
        active_revision,
        control.status.rejected_revision(),
    ));
    assert!(pending_slot.is_none());
    assert!(
        status.is_empty(),
        "intentional supersession is not reported as applied"
    );
    assert_eq!(error, None);

    let next_fence = capture_arrangement_fence(&control, &project, active_revision)
        .expect("latest intent captures a new fence after first cancellation terminal");
    assert_eq!(next_fence.expected_active_revision(), active_revision);
    assert_eq!(control.reclaim(), 1, "first section returns to control");
    let latest = latched.take().unwrap();
    assert_eq!(
        latest.section.unwrap().quantization,
        hex_engine::LaunchQuantization::EighthTriplet
    );
    let latest_spec = latest.spec().unwrap();
    assert_eq!(
        latest_spec.mode,
        ArrangementSeekMode::SectionLoop(hex_project::LoopRegion::new(768, 1024, true).unwrap())
    );
    assert_eq!(
        (
            latest_spec.destination_frame,
            latest_spec.work_budget_frames
        ),
        (768, 768)
    );
    let request = service.request_arrangement_seek(latest_spec).unwrap();
    let deadline = crate::test_wait::hang_deadline();
    let latest_result = loop {
        if let Some(result) = service.try_arrangement_seek_result() {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "latest section preparation timed out"
        );
        thread::yield_now();
    };
    assert_eq!(latest_result.request_id, request);
    assert_eq!(
        latest_result.mode,
        ArrangementSeekMode::SectionLoop(latest.section.unwrap().region)
    );
    let latest_playback = latest_result.result.unwrap();
    assert_eq!(
        latest_playback.section_loop(),
        Some(latest.section.unwrap().region)
    );
    assert_eq!(latest_playback.position(), 768);
    let latest_revision = control
        .publish_with_policy(
            latest_playback,
            PublicationPolicy::InstallSectionWhenReady {
                fence: next_fence,
                quantization: latest.section.unwrap().quantization,
            },
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut block).unwrap();
    assert_eq!(
        control
            .status
            .applied_revision
            .load(std::sync::atomic::Ordering::Acquire),
        latest_revision
    );
    assert_eq!(
        control
            .status
            .position
            .load(std::sync::atomic::Ordering::Relaxed),
        896
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
        section: None,
        recovery_generation: None,
        destination_frame: 256,
        start_playing: true,
        project_generation: 4,
        project_revision: 7,
        prepared_revision: Some(prepared),
        device_preview_identity: None,
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

    assert_eq!(
        cancel_published_seek(Some(&control), &mut pending),
        Some(FreshRuntimeCancellationResult::Complete)
    );
    assert!(
        pending.cancelled,
        "completed return is not an applied runtime"
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
