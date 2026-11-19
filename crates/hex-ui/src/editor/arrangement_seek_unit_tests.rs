use super::{
    ToggleAction, VerifiedSeekCandidateContext, desired_playing_with_pending, routes_are_current,
    seek_requires_preroll_for_mode, toggle_action, verified_audio_candidate_is_current,
};
use hex_audio::prepared::{
    SessionPendingRoute, SessionRouteEntry, SessionRouteOwner, SessionRouteTarget,
};
use hex_project::{Command, Editor, Project};

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
