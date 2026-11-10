//! Correlate audio results and publication observations with current source ownership.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AudioEventDisposition {
    Current,
    Future,
    Ignore,
}

pub(super) fn audio_event_disposition(
    current_generation: u64,
    pending_restart: Option<(u64, u64)>,
    event_generation: u64,
    restart_request: Option<u64>,
) -> AudioEventDisposition {
    if event_generation < current_generation {
        return AudioEventDisposition::Ignore;
    }
    if event_generation == current_generation {
        return match restart_request {
            Some(request_id) if pending_restart == Some((event_generation, request_id)) => {
                AudioEventDisposition::Current
            }
            Some(_) => AudioEventDisposition::Ignore,
            None if pending_restart
                .is_some_and(|(generation, _)| generation == event_generation) =>
            {
                AudioEventDisposition::Ignore
            }
            None => AudioEventDisposition::Current,
        };
    }
    match restart_request {
        Some(request_id) if pending_restart == Some((event_generation, request_id)) => {
            AudioEventDisposition::Future
        }
        Some(_) => AudioEventDisposition::Ignore,
        None => AudioEventDisposition::Future,
    }
}

pub(super) fn future_audio_result_replaces(
    slot_generation: Option<u64>,
    event_generation: u64,
) -> bool {
    slot_generation.is_none_or(|slot_generation| event_generation >= slot_generation)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum PublicationObservationDisposition {
    Apply,
    Wait,
    Stale,
}

pub(super) fn publication_observation_replaces(
    existing: Option<hex_engine::AudioSourceContext>,
    incoming: hex_engine::AudioSourceContext,
) -> bool {
    existing.is_none_or(|existing| {
        (incoming.project_generation, incoming.editor_revision)
            >= (existing.project_generation, existing.editor_revision)
    })
}

pub(super) fn publication_observation_disposition(
    current_generation: u64,
    current_revision: Option<u64>,
    current_context: Option<hex_engine::AudioSourceContext>,
    current_output_identity: Option<u64>,
    observation_context: hex_engine::AudioSourceContext,
    observation_output_identity: u64,
) -> PublicationObservationDisposition {
    let observed = (
        observation_context.project_generation,
        observation_context.editor_revision,
    );
    if observation_context.project_generation < current_generation
        || current_revision.is_some_and(|revision| {
            observation_context.project_generation == current_generation
                && observation_context.editor_revision < revision
        })
    {
        return PublicationObservationDisposition::Stale;
    }
    let Some(current_context) = current_context else {
        return PublicationObservationDisposition::Wait;
    };
    let current = (
        current_context.project_generation,
        current_context.editor_revision,
    );
    if observed < current {
        return PublicationObservationDisposition::Stale;
    }
    if observed > current {
        return PublicationObservationDisposition::Wait;
    }
    match current_output_identity {
        Some(identity) if identity == observation_output_identity => {
            PublicationObservationDisposition::Apply
        }
        Some(_) => PublicationObservationDisposition::Stale,
        None => PublicationObservationDisposition::Wait,
    }
}

#[cfg(all(test, feature = "ui-unit-tier"))]
mod tests {
    use super::{
        AudioEventDisposition as D, PublicationObservationDisposition as P,
        audio_event_disposition, future_audio_result_replaces, publication_observation_disposition,
        publication_observation_replaces,
    };
    use hex_engine::AudioSourceContext;

    fn context(generation: u64, revision: u64) -> AudioSourceContext {
        AudioSourceContext {
            project_generation: generation,
            editor_revision: revision,
        }
    }

    #[test]
    fn automatic_same_generation_result_cannot_replace_explicit_restart() {
        let pending = Some((7, 41));
        assert_eq!(audio_event_disposition(7, pending, 7, None), D::Ignore);
        // Both automatic success and automatic failure use the same correlation rule.
        assert_eq!(audio_event_disposition(7, pending, 7, Some(41)), D::Current);
        assert_eq!(audio_event_disposition(7, pending, 7, Some(40)), D::Ignore);
    }

    #[test]
    fn future_unavailable_result_is_current_after_canonical_generation_advances() {
        assert_eq!(audio_event_disposition(7, None, 8, None), D::Future);
        assert_eq!(audio_event_disposition(8, None, 8, None), D::Current);
    }

    #[test]
    fn stale_results_and_unmatched_restart_requests_are_ignored() {
        assert_eq!(audio_event_disposition(8, None, 7, None), D::Ignore);
        assert_eq!(
            audio_event_disposition(8, Some((8, 41)), 8, Some(40)),
            D::Ignore
        );
        assert_eq!(
            audio_event_disposition(8, Some((8, 41)), 9, Some(41)),
            D::Ignore
        );
    }

    #[test]
    fn future_auto_result_supersedes_older_restart_and_replays_on_new_generation() {
        let older_restart = Some((7, 41));
        assert_eq!(
            audio_event_disposition(7, older_restart, 8, None),
            D::Future
        );
        assert!(future_audio_result_replaces(Some(7), 8));
        assert_eq!(audio_event_disposition(8, None, 8, None), D::Current);
        assert!(!future_audio_result_replaces(Some(9), 8));
    }

    #[test]
    fn future_result_stays_queued_until_its_generation_and_replaces_only_older_results() {
        assert!(future_audio_result_replaces(None, 10));
        assert!(!future_audio_result_replaces(Some(10), 9));
        assert!(future_audio_result_replaces(Some(10), 11));
        assert_eq!(audio_event_disposition(8, None, 10, None), D::Future);
        assert_eq!(audio_event_disposition(10, None, 10, None), D::Current);
    }

    #[test]
    fn submitted_waits_for_state_and_opened_client_in_either_order_then_consumes_once() {
        let submitted = context(5, 12);

        let mut pending = Some((submitted, 70));
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(11),
                Some(context(5, 11)),
                None,
                submitted,
                70
            ),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(5, Some(12), Some(submitted), None, submitted, 70),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(12),
                Some(submitted),
                Some(70),
                submitted,
                70
            ),
            P::Apply
        );
        if pending.is_some_and(|(context, identity)| {
            publication_observation_disposition(
                5,
                Some(12),
                Some(submitted),
                Some(70),
                context,
                identity,
            ) == P::Apply
        }) {
            pending.take();
        }
        assert!(
            pending.is_none(),
            "a matching submitted observation is consumed"
        );

        // The reverse arrival order (matching client before the canonical state).
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(11),
                Some(context(5, 11)),
                Some(70),
                submitted,
                70
            ),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(12),
                Some(submitted),
                Some(70),
                submitted,
                70
            ),
            P::Apply
        );
    }

    #[test]
    fn same_document_newer_revision_is_held_until_that_revision_arrives() {
        let newer = context(5, 13);
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(12),
                Some(context(5, 12)),
                Some(70),
                newer,
                70
            ),
            P::Wait
        );
        assert_eq!(
            publication_observation_disposition(5, Some(13), Some(newer), Some(70), newer, 70),
            P::Apply
        );
    }

    #[test]
    fn stale_contexts_and_wrong_output_roots_are_discarded() {
        let current = context(5, 13);
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(13),
                Some(current),
                Some(70),
                context(4, 99),
                70
            ),
            P::Stale
        );
        assert_eq!(
            publication_observation_disposition(
                5,
                Some(13),
                Some(current),
                Some(70),
                context(5, 12),
                70
            ),
            P::Stale
        );
        assert_eq!(
            publication_observation_disposition(5, Some(13), Some(current), Some(71), current, 70),
            P::Stale
        );
    }

    #[test]
    fn newer_publication_context_cannot_be_overwritten_by_an_older_observation() {
        let newest = context(6, 2);
        let older = context(5, 900);
        assert!(publication_observation_replaces(None, newest));
        assert!(!publication_observation_replaces(Some(newest), older));
        assert!(publication_observation_replaces(Some(older), newest));
        assert!(publication_observation_replaces(Some(newest), newest));
    }
}
