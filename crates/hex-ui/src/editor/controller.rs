#[cfg(test)]
use super::EnqueuedNativeDeviceDrop;
use super::play_intent::AdmissionError;
use super::seek::seek_is_pending;
use super::selection;
use super::{
    EditorView, PendingEditorAudioResult, PendingEditorPublicationObservation, PendingPlayback,
    PendingPlaybackAction, PendingResidentPublication, PlaybackCandidate, UI_EVENT_BUDGET,
    UI_REQUEST_CAPACITY,
};
use gpui::{App, Context};
use hex_audio::prepared::PublicationPolicy;
use hex_engine::playback::{PlaybackAction, PlaybackCommand};
use hex_session::service::{
    ArrangementEditViewKind, DeviceClipboardAction, Event, Request, SubmitError,
    TrackClipboardAction, ViewState,
};
use hex_session::tuning_gesture::TuningLibraryOperation;
use std::{
    sync::Arc,
    sync::atomic::Ordering::{Acquire, Relaxed},
    time::Instant,
};

#[cfg(all(test, feature = "ui-test-support"))]
mod export_refusal_tests;
#[cfg(all(test, feature = "ui-test-support"))]
mod play_metadata_race_tests;
#[cfg(all(test, feature = "ui-test-support"))]
mod project_replacement_tests;
mod submission;
#[cfg(all(test, feature = "ui-test-support"))]
mod verified_gain_tests;

mod session_feedback;
use session_feedback::{SessionTicketOutcome, reconcile_session_receipt, reconcile_session_ticket};

mod audio_event_correlation;
pub(super) use audio_event_correlation::PublicationObservationDisposition;
use audio_event_correlation::{
    AudioEventDisposition, audio_event_disposition, future_audio_result_replaces,
    publication_observation_disposition, publication_observation_replaces,
};

mod audio_poll;
mod receive;
mod tick;
mod transport;

use super::user_errors::clear_stale_playback_unavailable_error;
pub(super) use audio_poll::publication_observation_for_view;
#[cfg(all(test, not(feature = "ui-test-support")))]
use audio_poll::relinquish_client_matches;
use transport::{is_paused_republish_rejection, record_paused_republish_rejection};

mod publication;
use publication::*;

fn intent_desired_playing(action: PlaybackAction) -> Option<bool> {
    match action {
        PlaybackAction::Transport(
            PlaybackCommand::Play
            | PlaybackCommand::PlayFrom(_)
            | PlaybackCommand::PlayFromWithNoteChase { .. },
        )
        | PlaybackAction::LaunchAndPlay(_) => Some(true),
        PlaybackAction::Transport(PlaybackCommand::Pause) => Some(false),
        PlaybackAction::Transport(PlaybackCommand::Stop)
        | PlaybackAction::Transport(PlaybackCommand::Seek(_))
        | PlaybackAction::Session(_)
        | PlaybackAction::SetMetronome(_) => None,
    }
}

fn queued_action_is_current(
    pending: PendingPlaybackAction,
    project_edit_pending: bool,
    current_generation: u64,
    project: Option<&hex_project::Project>,
    current_revision: Option<u64>,
) -> bool {
    if pending.generation != current_generation {
        return false;
    }
    match pending.action {
        PlaybackAction::Transport(_) | PlaybackAction::SetMetronome(_) => true,
        PlaybackAction::Session(command) | PlaybackAction::LaunchAndPlay(command) => {
            !project_edit_pending
                && pending.revision.is_some_and(|revision| {
                    super::session_grid::action_is_current(
                        project,
                        command,
                        pending.generation,
                        current_generation,
                        revision,
                        current_revision,
                    )
                })
        }
    }
}

fn fault_with_action_errors(
    existing: &Option<super::display_error::DisplayError>,
    diagnostic: &str,
    action_errors: &str,
) -> String {
    if !action_errors.is_empty() {
        return format!("{diagnostic} · {action_errors}");
    }
    if let Some(existing) = existing.as_deref()
        && existing
            .strip_prefix(diagnostic)
            .is_some_and(|rest| rest.starts_with(" · "))
    {
        return existing.to_owned();
    }
    diagnostic.to_owned()
}

/// Shown while a refused latency edit in an instrument/MIDI project waits for
/// Pause or Stop, which apply it without cutting sounding voices.
pub(super) const LATENCY_EDIT_WAITING: &str =
    "edit waiting · latency layout could not change live · applies on pause or stop";

fn replace_error(error: &mut Option<super::display_error::DisplayError>, message: &str) -> bool {
    if error.as_deref() == Some(message) {
        false
    } else {
        *error = Some(message.into());
        true
    }
}

fn replace_status(status: &mut String, message: &str) -> bool {
    if status == message {
        false
    } else {
        *status = message.to_owned();
        true
    }
}

fn clear_rejected_scene_duplicate(
    pending: &mut Option<super::PendingSceneDuplicate>,
    request: &Request,
) {
    if matches!(
        request,
        Request::Edit(hex_project::Command::DuplicateScene(_))
    ) {
        *pending = None;
    }
}

fn allows_non_open_request(
    close_prompt_open: bool,
    opening_project: bool,
    duplicate_continuation: bool,
) -> bool {
    !opening_project && (!close_prompt_open || duplicate_continuation)
}

#[cfg(all(test, not(feature = "ui-test-support")))]
#[path = "controller/session_action_unit_tests.rs"]
mod session_action_unit_tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod tuning_audition_error_tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod comparison_source_binding_tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod sampler_native_ui_protocol_tests;

#[cfg(all(test, not(feature = "ui-test-support")))]
mod source_binding_unit_tests;

#[cfg(test)]
mod paused_republish_unit_tests;
