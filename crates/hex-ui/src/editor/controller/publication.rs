//! Pure playback candidate promotion, verification, and recovery state transitions.
use super::super::{PendingPlayback, PlaybackCandidate};
use hex_engine::playback::Playback;
use hex_session::device_gesture::GestureKey;
use hex_session::service::{DevicePreviewIdentity, ViewState};
use std::{
    collections::VecDeque,
    sync::{Arc, atomic::Ordering::Acquire},
};

pub(super) fn restore_verified_recovery_from_applied(
    recovery: &mut super::super::recovery_state::RecoveryState,
    pending: Option<u64>,
) {
    if let Some(generation) = pending {
        recovery.restored(generation);
    }
}

pub(super) fn reconcile_verified_source_binding(
    admission: &mut super::super::audio_admission::AudioAdmissionStatus,
    mode: hex_session::DocumentAudioMode,
    output_identity: Option<u64>,
    generation: u64,
    current: Option<&ViewState>,
    source: Option<hex_audio::prepared::AppliedAudioSource>,
) -> (bool, Option<hex_audio::prepared::AppliedAudioSource>) {
    if let Some(source) = source
        && (mode == hex_session::DocumentAudioMode::VerifiedStreaming
            || current.is_some_and(|state| {
                hex_engine::native_device_resources::requires_private_sampler_sources(
                    &state.project,
                )
            }))
        && output_identity == Some(source.output_identity)
        && source.context.project_generation == generation
        && let Some(revision) =
            current.and_then(|state| state.audio_authoring_revision(source.context))
    {
        return (
            admission.applied(
                super::super::audio_admission::ProjectRevision {
                    generation,
                    revision,
                },
                source.audio_revision,
            ),
            Some(source),
        );
    }
    // Preserve factual historical ACKs, without accepting any old context for
    // the current authoring view. Exact source/output matching above is required.
    reconcile_verified_applied_source(admission, mode, output_identity, generation, None, source)
}

pub(super) fn reconcile_verified_applied_source(
    admission: &mut super::super::audio_admission::AudioAdmissionStatus,
    mode: hex_session::DocumentAudioMode,
    output_identity: Option<u64>,
    generation: u64,
    current_revision: Option<u64>,
    source: Option<hex_audio::prepared::AppliedAudioSource>,
) -> (bool, Option<hex_audio::prepared::AppliedAudioSource>) {
    let Some(source) = source.filter(|source| {
        mode == hex_session::DocumentAudioMode::VerifiedStreaming
            && output_identity == Some(source.output_identity)
            && source.context.project_generation == generation
    }) else {
        return (false, None);
    };
    let changed = admission.applied(
        super::super::audio_admission::ProjectRevision {
            generation,
            revision: source.context.editor_revision,
        },
        source.audio_revision,
    );
    (
        changed,
        (current_revision == Some(source.context.editor_revision)).then_some(source),
    )
}

pub(super) fn audio_command_prefix(
    status: &hex_audio::prepared::Status,
    mode: hex_session::DocumentAudioMode,
) -> u64 {
    if mode == hex_session::DocumentAudioMode::VerifiedStreaming {
        status.stream_applied_command_prefix.load(Acquire)
    } else {
        status.consumed_command.load(Acquire)
    }
}

pub(super) fn promote_playback_candidates(
    candidates: &mut VecDeque<PlaybackCandidate>,
    applied_high_water: u64,
    rejected_audio_revision: Option<u64>,
) -> Option<PlaybackCandidate> {
    let mut latest = None;
    while candidates
        .front()
        .is_some_and(|candidate| candidate.audio_revision <= applied_high_water)
    {
        if let Some(candidate) = candidates.pop_front()
            && Some(candidate.audio_revision) != rejected_audio_revision
        {
            latest = Some(candidate);
        }
    }
    latest
}

pub(super) fn latest_applied_canonical_candidate(
    candidates: &VecDeque<PlaybackCandidate>,
    applied_high_water: u64,
    rejected_audio_revision: Option<u64>,
) -> Option<(super::super::audio_admission::ProjectRevision, u64)> {
    candidates
        .iter()
        .rfind(|candidate| {
            candidate.seek_request_id.is_none()
                && candidate.audio_revision <= applied_high_water
                && Some(candidate.audio_revision) != rejected_audio_revision
                && candidate.device_preview_identity.is_none()
                && candidate.tuning_preview_identity.is_none()
                && candidate.clip_preview_identity.is_none()
        })
        .map(|candidate| {
            (
                super::super::audio_admission::ProjectRevision {
                    generation: candidate.project_generation,
                    revision: candidate.state.revision,
                },
                candidate.audio_revision,
            )
        })
}

pub(super) fn rejected_candidate_project(
    candidates: &VecDeque<PlaybackCandidate>,
    audio_revision: u64,
) -> Option<super::super::audio_admission::ProjectRevision> {
    candidates
        .iter()
        .find(|candidate| {
            candidate.audio_revision == audio_revision
                && candidate.seek_request_id.is_none()
                && candidate.device_preview_identity.is_none()
                && candidate.tuning_preview_identity.is_none()
                && candidate.clip_preview_identity.is_none()
        })
        .map(|candidate| super::super::audio_admission::ProjectRevision {
            generation: candidate.project_generation,
            revision: candidate.state.revision,
        })
}

pub(super) fn rejected_preview_candidate(
    candidates: &VecDeque<PlaybackCandidate>,
    audio_revision: u64,
    project_generation: u64,
    canonical_revision: Option<u64>,
    active_gesture: Option<(GestureKey, u64)>,
) -> bool {
    candidates.iter().any(|candidate| {
        candidate.audio_revision == audio_revision
            && candidate.device_preview_identity.is_some_and(|identity| {
                preview_source_is_current(
                    identity,
                    candidate.project_generation,
                    candidate.state.revision,
                    project_generation,
                    canonical_revision,
                    active_gesture,
                )
            })
    })
}

pub(super) fn preview_source_is_current(
    identity: DevicePreviewIdentity,
    candidate_generation: u64,
    candidate_revision: u64,
    current_generation: u64,
    current_revision: Option<u64>,
    active_gesture: Option<(GestureKey, u64)>,
) -> bool {
    identity.project_generation == candidate_generation
        && identity.project_generation == current_generation
        && identity.canonical_revision == candidate_revision
        && Some(identity.canonical_revision) == current_revision
        && active_gesture == Some((identity.key, identity.sequence))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppliedCandidateSource {
    Current,
    SupersededPreview,
    InvalidPreview,
}

pub(super) fn applied_clip_candidate_source(
    candidate: &PlaybackCandidate,
    identity: hex_session::service::ClipMovePreviewIdentity,
    applied_audio_revision: u64,
    generation: u64,
    revision: Option<u64>,
    expected: Option<hex_session::service::ClipMovePreviewIdentity>,
) -> AppliedCandidateSource {
    if candidate.audio_revision == applied_audio_revision
        && identity.context.project_generation == generation
        && candidate.project_generation == generation
        && identity.context.editor_revision == candidate.state.revision
        && revision == Some(candidate.state.revision)
        && expected.is_some_and(|current| {
            current.context == identity.context
                && current.gesture == identity.gesture
                && (identity.sequence < current.sequence || identity == current)
        })
    {
        AppliedCandidateSource::Current
    } else {
        AppliedCandidateSource::SupersededPreview
    }
}

pub(super) fn applied_candidate_source(
    candidate: &PlaybackCandidate,
    generation: u64,
    revision: Option<u64>,
    active: Option<(GestureKey, u64)>,
) -> AppliedCandidateSource {
    let Some(identity) = candidate.device_preview_identity else {
        return AppliedCandidateSource::Current;
    };
    // A terminal gesture or newer owner/sequence can overtake callback receipt.
    // Keep such notifications out of current state and error reporting.
    if identity.project_generation != generation
        || Some(identity.canonical_revision) != revision
        || active != Some((identity.key, identity.sequence))
    {
        return AppliedCandidateSource::SupersededPreview;
    }
    if preview_source_is_current(
        identity,
        candidate.project_generation,
        candidate.state.revision,
        generation,
        revision,
        active,
    ) {
        AppliedCandidateSource::Current
    } else {
        AppliedCandidateSource::InvalidPreview
    }
}

pub(super) fn tuning_preview_current(
    identity: hex_session::service::TuningPreviewIdentity,
    candidate_generation: u64,
    candidate_revision: u64,
    generation: u64,
    revision: Option<u64>,
    active: Option<(hex_session::tuning_gesture::TuningGestureKey, u64)>,
) -> bool {
    identity.project_generation == candidate_generation
        && identity.project_generation == generation
        && identity.canonical_revision == candidate_revision
        && Some(identity.canonical_revision) == revision
        && active == Some((identity.key, identity.sequence))
}
pub(super) fn applied_tuning_candidate_source(
    candidate: &PlaybackCandidate,
    generation: u64,
    revision: Option<u64>,
    active: Option<(hex_session::tuning_gesture::TuningGestureKey, u64)>,
) -> Option<AppliedCandidateSource> {
    let identity = candidate.tuning_preview_identity?;
    Some(
        if identity.project_generation != generation
            || Some(identity.canonical_revision) != revision
            || active != Some((identity.key, identity.sequence))
        {
            AppliedCandidateSource::SupersededPreview
        } else if candidate.device_preview_identity.is_none()
            && tuning_preview_current(
                identity,
                candidate.project_generation,
                candidate.state.revision,
                generation,
                revision,
                active,
            )
        {
            AppliedCandidateSource::Current
        } else {
            AppliedCandidateSource::InvalidPreview
        },
    )
}
pub(super) fn rejected_tuning_preview_candidate(
    candidates: &VecDeque<PlaybackCandidate>,
    audio_revision: u64,
    generation: u64,
    revision: Option<u64>,
    active: Option<(hex_session::tuning_gesture::TuningGestureKey, u64)>,
) -> bool {
    candidates.iter().any(|candidate| {
        candidate.audio_revision == audio_revision
            && candidate.tuning_preview_identity.is_some_and(|identity| {
                tuning_preview_current(
                    identity,
                    candidate.project_generation,
                    candidate.state.revision,
                    generation,
                    revision,
                    active,
                )
            })
    })
}

pub(super) fn pending_playback_for_state(
    state: Arc<hex_session::service::ViewState>,
    project_generation: u64,
    playback: Playback,
) -> PendingPlayback {
    PendingPlayback {
        project_revision: state.revision,
        project_generation,
        state,
        playback,
        install_fresh_runtime: false,
        fresh_runtime_fence: None,
        prepared_revision: None,
        seek_request_id: None,
        device_preview_identity: None,
        tuning_preview_identity: None,
    }
}

pub(super) fn retained_install_fresh_policy(
    event_replaced_project: bool,
    project_generation: u64,
    previous: Option<(u64, bool)>,
) -> bool {
    event_replaced_project
        || previous.is_some_and(|(generation, reset)| generation == project_generation && reset)
}

#[cfg(all(test, feature = "ui-unit-tier"))]
#[path = "publication_unit_tests.rs"]
mod publication_unit_tests;
