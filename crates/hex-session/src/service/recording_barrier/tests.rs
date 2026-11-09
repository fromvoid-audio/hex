use super::*;
use crate::device_gesture::{GestureKey, GesturePhase, PluginParameterValue};

fn source(revision: u64) -> hex_engine::AudioSourceContext {
    hex_engine::AudioSourceContext {
        project_generation: 3,
        editor_revision: revision,
    }
}
fn terminal() -> GestureRequest {
    GestureRequest::CommitPlugin {
        key: GestureKey {
            generation: 3,
            token: 11,
        },
        sequence: 7,
        value: PluginParameterValue::new(0.375).unwrap(),
    }
}
fn ack() -> GestureAck {
    GestureAck {
        key: terminal().key(),
        phase: GesturePhase::Commit,
        sequence: 7,
        accepted: true,
        preview_retained: false,
        source_lost: false,
        reason: None,
    }
}
fn queued(request: Request) -> QueuedRequest {
    QueuedRequest::continuation(request, &AtomicU64::new(9))
}
fn begin(request: Request) -> State {
    let mut state = State::default();
    let result = state.admit(queued(request), Some(terminal()), source(4), 9);
    assert!(matches!(result.request, Request::DeviceGesture(_)));
    assert_eq!(result.document_intent, 9);
    state
}

#[test]
fn save_keeps_original_path_and_correlation_across_only_its_take_revision() {
    let correlation = SaveCorrelation {
        request_id: 31,
        project_generation: 3,
        revision: 4,
    };
    let mut original = queued(Request::SaveNew(PathBuf::from("original.hex")));
    original.save_correlation = Some(correlation);
    let mut state = State::default();
    state.admit(original, Some(terminal()), source(4), 9);
    assert!(state.poll(source(4), 9).is_none());
    state.complete(&ack(), source(5));
    let result = state.poll(source(5), 9).unwrap();
    assert!(matches!(result.request, Request::SaveNew(path) if path == Path::new("original.hex")));
    assert_eq!(result.save_correlation, Some(correlation));
    assert_eq!(result.document_intent, 9);
    assert!(save_matches(
        result.save_correlation,
        result.recording_barrier,
        source(5)
    ));
    assert!(!save_matches(
        result.save_correlation,
        result.recording_barrier,
        source(6)
    ));
    assert!(state.poll(source(5), 9).is_none());
}

#[test]
fn stale_save_and_unrecorded_requests_never_start_a_terminal() {
    let mut stale = queued(Request::SaveCurrent);
    stale.save_correlation = Some(SaveCorrelation {
        request_id: 31,
        project_generation: 3,
        revision: 2,
    });
    let mut state = State::default();
    let result = state.admit(stale, Some(terminal()), source(4), 9);
    assert!(matches!(result.request, Request::SaveCurrent));
    assert!(!state.active());
    for request in [
        Request::Undo,
        Request::Redo,
        Request::Export(PathBuf::from("out.wav")),
    ] {
        let result = state.admit(queued(request), None, source(4), 9);
        assert!(!matches!(result.request, Request::DeviceGesture(_)));
        assert!(!state.active());
    }
}

#[test]
fn only_exact_terminal_ack_releases_original_and_empty_does_not_advance_revision() {
    let mut state = begin(Request::Undo);
    for wrong in [
        GestureAck {
            sequence: 8,
            ..ack()
        },
        GestureAck {
            phase: GesturePhase::Change,
            ..ack()
        },
        GestureAck {
            key: GestureKey {
                token: 12,
                ..ack().key
            },
            ..ack()
        },
    ] {
        state.complete(&wrong, source(4));
        assert!(state.poll(source(4), 9).is_none());
    }
    state.complete(&ack(), source(4));
    let result = state.poll(source(4), 9).unwrap();
    assert!(matches!(result.request, Request::Undo));
    assert!(failure(result.recording_barrier).is_none());
}

#[test]
fn cancellation_timeout_and_source_changes_refuse_original_without_fabricating_success() {
    let mut cancelled = begin(Request::SaveNew(PathBuf::from("cancelled.hex")));
    let result = cancelled.poll(source(4), 10).unwrap();
    assert!(matches!(result.request, Request::SaveNew(path) if path == Path::new("cancelled.hex")));
    assert!(
        failure(result.recording_barrier)
            .unwrap()
            .contains("project changed")
    );
    let mut changed = begin(Request::Undo);
    assert!(failure(changed.poll(source(5), 9).unwrap().recording_barrier).is_some());
    let mut expired = begin(Request::Redo);
    expired.pending.as_mut().unwrap().started = Instant::now() - DEADLINE;
    assert!(
        failure(expired.poll(source(4), 9).unwrap().recording_barrier)
            .unwrap()
            .contains("timed out")
    );
}

#[test]
fn refused_release_and_unrelated_revision_cannot_authorize_barrier() {
    for (ack, current) in [
        (
            GestureAck {
                accepted: false,
                preview_retained: true,
                ..ack()
            },
            source(4),
        ),
        (ack(), source(6)),
        (
            ack(),
            hex_engine::AudioSourceContext {
                project_generation: 4,
                editor_revision: 5,
            },
        ),
    ] {
        let mut state = begin(Request::Export(PathBuf::from("original.wav")));
        state.complete(&ack, current);
        let result = state.poll(current, 9).unwrap();
        assert!(
            matches!(result.request, Request::Export(path) if path == Path::new("original.wav"))
        );
        assert!(failure(result.recording_barrier).is_some());
    }
}

#[test]
fn parked_request_cannot_be_replaced_and_continuation_keeps_accepted_intent() {
    let mut state = begin(Request::Undo);
    let later = state.admit(queued(Request::Redo), Some(terminal()), source(4), 9);
    assert!(matches!(later.request, Request::Redo));
    let continuation = state.continuation(terminal()).unwrap();
    assert_eq!(continuation.document_intent, 9);
    state.complete(&ack(), source(5));
    assert!(matches!(
        state.poll(source(5), 9).unwrap().request,
        Request::Undo
    ));
}
