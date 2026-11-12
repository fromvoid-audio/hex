use super::{
    AppliedCandidateSource, applied_clip_candidate_source, latest_applied_canonical_candidate,
    pending_playback_for_state, preview_source_is_current, promote_playback_candidates,
    reconcile_verified_applied_source, rejected_candidate_project, rejected_preview_candidate,
    restore_verified_recovery_from_applied, retained_install_fresh_policy,
};
use crate::editor::PlaybackCandidate;
use crate::editor::audio_admission::{AudioAdmissionStatus, ProjectRevision};
use hex_engine::playback::Playback;
use hex_project::Project;
use hex_session::device_gesture::GestureKey;
use hex_session::service::{DevicePreviewIdentity, ViewState};
use std::{collections::VecDeque, sync::Arc};

#[test]
fn delayed_preview_application_distinguishes_superseded_and_invalid_current_sources() {
    use super::{AppliedCandidateSource, applied_candidate_source};
    let preview = preview_candidate(17, 40, 3);
    let identity = preview.device_preview_identity.unwrap();
    let active = Some((identity.key, identity.sequence));
    let mut queued = VecDeque::from([candidate(16, 40, 3), preview.clone()]);
    let applied = promote_playback_candidates(&mut queued, 17, None).unwrap();
    assert_eq!(
        applied_candidate_source(&applied, 3, Some(40), active),
        AppliedCandidateSource::Current
    );
    for (generation, revision, active) in [
        (3, Some(40), None),   // cancel/commit terminal retired the gesture
        (3, Some(41), active), // canonical commit overtook callback receipt
        (4, Some(40), active), // project/owner replaced
        (3, Some(40), Some((identity.key, identity.sequence + 1))),
        (
            3,
            Some(40),
            Some((
                GestureKey {
                    generation: 3,
                    token: identity.key.token + 1,
                },
                identity.sequence,
            )),
        ),
    ] {
        assert_eq!(
            applied_candidate_source(&applied, generation, revision, active),
            AppliedCandidateSource::SupersededPreview
        );
    }
    let mut wrong_generation = preview.clone();
    wrong_generation.project_generation = 2;
    assert_eq!(
        applied_candidate_source(&wrong_generation, 3, Some(40), active),
        AppliedCandidateSource::InvalidPreview
    );
    let mut wrong_revision = preview_candidate(18, 39, 3);
    wrong_revision.device_preview_identity = Some(identity);
    assert_eq!(
        applied_candidate_source(&wrong_revision, 3, Some(40), active),
        AppliedCandidateSource::InvalidPreview
    );
    let mut rejected = VecDeque::from([preview]);
    assert!(promote_playback_candidates(&mut rejected, 17, Some(17)).is_none());
    assert_eq!(
        applied_candidate_source(&candidate(18, 40, 3), 3, Some(40), None),
        AppliedCandidateSource::Current
    );
}
#[test]
fn fresh_project_publication_survives_same_generation_candidate_coalescing_only() {
    assert!(retained_install_fresh_policy(true, 4, None));
    assert!(retained_install_fresh_policy(false, 4, Some((4, true))));
    assert!(!retained_install_fresh_policy(false, 4, None));
    assert!(!retained_install_fresh_policy(false, 4, Some((4, false))));
    assert!(!retained_install_fresh_policy(false, 5, Some((4, true))));
    assert!(retained_install_fresh_policy(true, 5, Some((4, true))));
}

#[test]
fn exact_rejection_mapping_precedes_coalesced_applied_candidate_promotion() {
    let old = ProjectRevision {
        generation: 8,
        revision: 40,
    };
    let current = ProjectRevision {
        generation: 9,
        revision: 0,
    };
    let mut candidates = VecDeque::from([candidate(17, 40, 8), candidate(18, 0, 9)]);
    let rejected = rejected_candidate_project(&candidates, 17)
        .expect("rejected audio revision maps before candidate pruning");
    assert_eq!(rejected, old);

    let mut status = AudioAdmissionStatus::default();
    status.canonical_revision(current);
    status.published(current, 18);
    assert!(!status.rejected(rejected, 17, "stale generation rejection"));

    let audible = promote_playback_candidates(&mut candidates, 18, Some(17))
        .expect("new accepted candidate survives older rejection telemetry");
    assert_eq!(audible.audio_revision, 18);
    assert_eq!(audible.project_generation, current.generation);
    status.applied(current, audible.audio_revision);
    assert_eq!(
        status.current().unwrap().state,
        crate::editor::audio_admission::AdmissionState::Applied
    );
    assert_eq!(status.last_audible().unwrap().project, current);
}

#[test]
fn same_revision_preview_apply_reject_and_cancel_do_not_change_canonical_admission() {
    let canonical = ProjectRevision {
        generation: 3,
        revision: 40,
    };
    let mut status = AudioAdmissionStatus::default();
    status.canonical_revision(canonical);
    status.published(canonical, 16);

    let mut preview = preview_candidate(17, 40, 3);
    let active = Some((
        GestureKey {
            generation: 3,
            token: 1,
        },
        1,
    ));
    assert!(rejected_candidate_project(&VecDeque::from([preview.clone()]), 17).is_none());
    assert!(rejected_preview_candidate(
        &VecDeque::from([preview.clone()]),
        17,
        3,
        Some(40),
        active,
    ));
    let current_identity = preview.device_preview_identity.expect("preview identity");
    assert!(preview_source_is_current(
        current_identity,
        3,
        40,
        3,
        Some(40),
        active,
    ));
    let stale_generation = preview_candidate(18, 40, 2);
    assert!(!rejected_preview_candidate(
        &VecDeque::from([stale_generation]),
        18,
        3,
        Some(40),
        active,
    ));
    assert!(!preview_source_is_current(
        current_identity,
        3,
        40,
        4,
        Some(0),
        active,
    ));
    let mut stale_sequence = preview_candidate(19, 40, 3);
    stale_sequence
        .device_preview_identity
        .as_mut()
        .expect("preview identity")
        .sequence = 0;
    assert!(!rejected_preview_candidate(
        &VecDeque::from([stale_sequence]),
        19,
        3,
        Some(40),
        active,
    ));
    // Cancel/commit retires the active gesture, so a delayed terminal
    // preview refusal cannot overwrite a newer status message.
    assert!(!rejected_preview_candidate(
        &VecDeque::from([preview.clone()]),
        17,
        3,
        Some(40),
        None,
    ));
    assert!(!preview_source_is_current(
        current_identity,
        3,
        40,
        3,
        Some(40),
        None,
    ));
    assert!(!rejected_preview_candidate(
        &VecDeque::from([preview.clone()]),
        17,
        4,
        Some(0),
        active,
    ));

    // A valid preview can become audible for this exact canonical source,
    // while its transient audio revision remains outside canonical status.
    let mut preview_candidates = VecDeque::from([candidate(16, 40, 3), preview.clone()]);
    let (applied_project, applied_audio) =
        latest_applied_canonical_candidate(&preview_candidates, 17, None)
            .expect("the canonical candidate applied before the transient preview");
    status.applied(applied_project, applied_audio);
    let audible_preview = promote_playback_candidates(&mut preview_candidates, 17, None)
        .expect("the callback promoted the preview graph");
    assert!(audible_preview.device_preview_identity.is_some());
    assert_eq!(
        status.current().unwrap().state,
        crate::editor::audio_admission::AdmissionState::Applied
    );
    assert_eq!(status.last_audible().unwrap().audio_revision, 16);

    // Cancellation restores canonical playback at the same project
    // revision; that terminal canonical candidate remains identifiable.
    preview = candidate(18, 40, 3);
    let mut restore_candidates = VecDeque::from([preview]);
    let restored = promote_playback_candidates(&mut restore_candidates, 18, None)
        .expect("cancel restored the canonical graph");
    assert_eq!(
        rejected_candidate_project(&VecDeque::from([restored]), 18),
        Some(canonical)
    );
    status.published(canonical, 18);
    status.applied(canonical, 18);
    assert_eq!(
        status.current().unwrap().state,
        crate::editor::audio_admission::AdmissionState::Applied
    );
    assert_eq!(status.last_audible().unwrap().audio_revision, 18);

    // A newer canonical revision stays pending when only its numeric
    // preview is accepted, then becomes applied on the terminal commit.
    let committed = ProjectRevision {
        generation: 3,
        revision: 41,
    };
    status.canonical_revision(committed);
    status.published(committed, 19);
    let preview = preview_candidate(20, 41, 3);
    let mut candidates = VecDeque::from([preview.clone()]);
    assert!(rejected_candidate_project(&candidates, 20).is_none());
    assert!(latest_applied_canonical_candidate(&candidates, 20, None).is_none());
    let audible_preview = promote_playback_candidates(&mut candidates, 20, None)
        .expect("the callback promoted the latest numeric preview");
    assert!(audible_preview.device_preview_identity.is_some());
    assert_eq!(status.current().unwrap().project, committed);
    assert_eq!(
        status.current().unwrap().state,
        crate::editor::audio_admission::AdmissionState::Pending
    );

    let canonical_candidate = candidate(21, 41, 3);
    assert_eq!(
        rejected_candidate_project(&VecDeque::from([canonical_candidate.clone()]), 21),
        Some(committed)
    );
    status.published(committed, 21);
    let applied =
        latest_applied_canonical_candidate(&VecDeque::from([canonical_candidate]), 21, None)
            .expect("terminal canonical candidate applied");
    status.applied(applied.0, applied.1);
    assert_eq!(
        status.current().unwrap().state,
        crate::editor::audio_admission::AdmissionState::Applied
    );
    assert_eq!(status.last_audible().unwrap().audio_revision, 21);
}

#[test]
fn event_playback_retains_its_exact_read_only_view_until_publication_retry() {
    let captured = candidate(3, 12, 4).state;
    let exact = Arc::clone(&captured);
    let playback = Playback::new(hex_engine::Project::new(48_000, Vec::new()).unwrap());
    let pending = pending_playback_for_state(captured, 4, playback);
    assert!(Arc::ptr_eq(&pending.state, &exact));
    assert_eq!(pending.project_revision, 12);
    assert_eq!(pending.project_generation, 4);
}

#[test]
fn applied_candidates_promote_before_coalesced_rejection_pruning() {
    let mut candidates = VecDeque::from([
        candidate(4, 40, 2),
        candidate(5, 50, 2),
        candidate(6, 60, 2),
        candidate(7, 70, 2),
    ]);
    assert!(promote_playback_candidates(&mut candidates, 3, None).is_none());
    let audible = promote_playback_candidates(&mut candidates, 5, None).expect("accepted view");
    assert_eq!(audible.audio_revision, 5);
    assert_eq!(audible.state.revision, 50);
    candidates.retain(|item| item.audio_revision > 7);
    assert!(candidates.is_empty());
    assert_eq!(audible.project_generation, 2);
}

#[test]
fn rejected_unapplied_candidate_never_becomes_audible() {
    let mut candidates = VecDeque::from([candidate(4, 40, 2)]);
    assert!(promote_playback_candidates(&mut candidates, 3, None).is_none());
    candidates.retain(|item| item.audio_revision > 4);
    assert!(candidates.is_empty());
}

#[test]
fn clip_preview_requires_exact_applied_revision_and_current_intent() {
    let identity = hex_session::service::ClipMovePreviewIdentity {
        context: hex_engine::AudioSourceContext {
            project_generation: 2,
            editor_revision: 40,
        },
        gesture: 9,
        sequence: 3,
    };
    let mut preview = candidate(5, 40, 2);
    preview.clip_preview_identity = Some(identity);
    let classify = |applied, generation, revision, expected| {
        applied_clip_candidate_source(&preview, identity, applied, generation, revision, expected)
    };
    assert_eq!(
        classify(4, 2, Some(40), Some(identity)),
        AppliedCandidateSource::SupersededPreview
    );
    assert_eq!(
        classify(5, 2, Some(40), Some(identity)),
        AppliedCandidateSource::Current
    );
    assert_eq!(
        classify(6, 2, Some(40), Some(identity)),
        AppliedCandidateSource::SupersededPreview
    );
    assert_eq!(
        classify(5, 3, Some(40), Some(identity)),
        AppliedCandidateSource::SupersededPreview
    );
    assert_eq!(
        classify(5, 2, Some(41), Some(identity)),
        AppliedCandidateSource::SupersededPreview
    );
    assert_eq!(
        classify(5, 2, Some(40), None),
        AppliedCandidateSource::SupersededPreview
    );
    let mut newer = identity;
    newer.sequence += 1;
    assert_eq!(
        classify(5, 2, Some(40), Some(newer)),
        AppliedCandidateSource::SupersededPreview
    );
    let mut queue = VecDeque::from([preview]);
    assert!(latest_applied_canonical_candidate(&queue, 5, None).is_none());
    assert!(rejected_candidate_project(&queue, 5).is_none());
    assert!(promote_playback_candidates(&mut queue, 4, None).is_none());
    let applied = promote_playback_candidates(&mut queue, 5, None).unwrap();
    assert_eq!(
        applied_clip_candidate_source(&applied, identity, 5, 2, Some(40), Some(identity)),
        AppliedCandidateSource::Current
    );
}

#[test]
fn applied_clip_preview_invalidates_only_the_previous_canonical_audio() {
    use super::super::audio_admission::AdmissionState;
    let project = ProjectRevision {
        generation: 2,
        revision: 40,
    };
    let mut admission = AudioAdmissionStatus::default();
    admission.canonical_revision(project);
    admission.published(project, 4);
    admission.applied(project, 4);
    assert!(admission.preview_applied(5));
    let current = admission.current().unwrap();
    assert_eq!(current.state, AdmissionState::Pending);
    assert_eq!(current.audio_revision, None);
    assert!(admission.last_audible().is_none());
    admission.published(project, 7);
    admission.preview_applied(6);
    assert_eq!(admission.current().unwrap().audio_revision, Some(7));
}

fn candidate(audio_revision: u64, project_revision: u64, generation: u64) -> PlaybackCandidate {
    let project = Arc::new(Project::new(48_000).expect("project"));
    PlaybackCandidate {
        clip_preview_identity: None,
        worker_requires_preroll: None,
        observation: None,
        audio_revision,
        project_generation: generation,
        state: Arc::new(ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project,
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::new(hex_engine::MidiArrangementPreview::default()),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames: 0,
            can_undo: false,
            can_redo: false,
            revision: project_revision,
            unsaved: false,
            document_path: None,
        }),
        prepared_revision: None,
        seek_request_id: None,
        device_preview_identity: None,
        tuning_preview_identity: None,
    }
}

fn preview_candidate(
    audio_revision: u64,
    project_revision: u64,
    generation: u64,
) -> PlaybackCandidate {
    let mut candidate = candidate(audio_revision, project_revision, generation);
    candidate.device_preview_identity = Some(DevicePreviewIdentity {
        project_generation: generation,
        key: GestureKey {
            generation,
            token: 1,
        },
        sequence: 1,
        canonical_revision: project_revision,
    });
    candidate
}

#[test]
fn actual_current_ack_restores_recovery_without_a_submitted_event() {
    let mut recovery = super::super::recovery_state::RecoveryState::default();
    let generation = recovery.begin(640).unwrap();
    assert!(recovery.opened(generation));
    assert_eq!(recovery.seek_needed(true), None);
    restore_verified_recovery_from_applied(&mut recovery, Some(generation + 1));
    assert_eq!(recovery.seek_needed(true), None);
    restore_verified_recovery_from_applied(&mut recovery, Some(generation));
    restore_verified_recovery_from_applied(&mut recovery, Some(generation));
    assert_eq!(recovery.seek_needed(true), Some(640));
    assert_eq!(recovery.seek_needed(false), None);
}

#[test]
fn coalesced_submission_keeps_actual_audible_context_when_newer_edit_refuses() {
    use super::super::audio_admission::{AudioAdmissionStatus, ProjectRevision};
    let mut admission = AudioAdmissionStatus::default();
    let mode = hex_session::DocumentAudioMode::VerifiedStreaming;
    let source = hex_audio::prepared::AppliedAudioSource {
        context: hex_engine::AudioSourceContext {
            project_generation: 2,
            editor_revision: 7,
        },
        audio_revision: 11,
        output_identity: 13,
    };
    // Submitted(A) was never received; B is already canonical/refused.
    admission.canonical_revision(ProjectRevision {
        generation: 2,
        revision: 8,
    });
    assert!(admission.unpublished_refusal(
        ProjectRevision {
            generation: 2,
            revision: 8
        },
        "audio publication refused",
    ));
    let (changed, current) =
        reconcile_verified_applied_source(&mut admission, mode, Some(13), 2, Some(8), Some(source));
    assert!(changed && current.is_none());
    assert_eq!(admission.last_audible().unwrap().project.revision, 7);
    assert_eq!(admission.current().unwrap().project.revision, 8);
    assert!(admission.label().contains("rev 8 rejected"));
    assert!(admission.label().contains("rev 7 audible"));
    assert_eq!(
        reconcile_verified_applied_source(&mut admission, mode, Some(13), 2, Some(8), None),
        (false, None)
    );
    assert!(admission.label().contains("rev 7 audible"));
    // Only exact current state/client correlation may recover seek metadata.
    assert_eq!(
        reconcile_verified_applied_source(&mut admission, mode, Some(99), 2, Some(7), Some(source)),
        (false, None)
    );
    let (_, current) =
        reconcile_verified_applied_source(&mut admission, mode, Some(13), 2, Some(7), Some(source));
    assert_eq!(current, Some(source));
    assert_eq!(
        reconcile_verified_applied_source(&mut admission, mode, Some(13), 3, Some(7), Some(source)),
        (false, None)
    );
    let actual_b = hex_audio::prepared::AppliedAudioSource {
        context: hex_engine::AudioSourceContext {
            project_generation: 2,
            editor_revision: 8,
        },
        audio_revision: 12,
        ..source
    };
    reconcile_verified_applied_source(&mut admission, mode, Some(13), 2, Some(8), Some(actual_b));
    assert_eq!(
        admission.current().unwrap().state,
        super::super::audio_admission::AdmissionState::Applied
    );
    assert!(!admission.unpublished_refusal(
        ProjectRevision {
            generation: 2,
            revision: 8
        },
        "late refusal"
    ));
}
