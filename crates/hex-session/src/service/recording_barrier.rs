//! One original FIFO request parked behind an admitted recorded SDK terminal.
//! No later request is consumed while it waits. Accepted document replacement
//! cancels the parked request on the next control poll; Stop remains owned by
//! the audio endpoint. The exact gesture ACK is delivered before redispatch.
//! A private receipt authorizes only the recording's own revision transition,
//! without changing the caller's SaveCorrelation or accepted document intent.
use super::*;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(3);
pub(super) const POLL: Duration = Duration::from_millis(2);

#[derive(Clone, Copy)]
pub(super) enum Completion {
    Finished {
        before: hex_engine::AudioSourceContext,
        after: hex_engine::AudioSourceContext,
    },
    Failed(&'static str),
}

pub(super) fn failure(completion: Option<Completion>) -> Option<&'static str> {
    match completion {
        Some(Completion::Failed(reason)) => Some(reason),
        _ => None,
    }
}

pub(super) fn save_matches(
    correlation: Option<SaveCorrelation>,
    completion: Option<Completion>,
    current: hex_engine::AudioSourceContext,
) -> bool {
    correlation.is_none_or(|identity| {
        let captured = hex_engine::AudioSourceContext {
            project_generation: identity.project_generation,
            editor_revision: identity.revision,
        };
        match completion {
            Some(Completion::Finished { before, after }) => captured == before && current == after,
            _ => captured == current,
        }
    })
}

struct Pending {
    request: QueuedRequest,
    source: hex_engine::AudioSourceContext,
    terminal: GestureRequest,
    started: Instant,
    completion: Option<Completion>,
}

#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
}

impl State {
    pub(super) fn active(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn continuation(&self, ready: GestureRequest) -> Option<QueuedRequest> {
        let pending = self.pending.as_ref()?;
        (ready.key() == pending.terminal.key()
            && ready.phase() == pending.terminal.phase()
            && ready.sequence() == pending.terminal.sequence())
        .then(|| {
            QueuedRequest::continuation(
                Request::DeviceGesture(pending.terminal),
                &AtomicU64::new(pending.request.document_intent),
            )
        })
    }

    pub(super) fn admit(
        &mut self,
        mut queued: QueuedRequest,
        terminal: Option<GestureRequest>,
        source: hex_engine::AudioSourceContext,
        document_intent: u64,
    ) -> QueuedRequest {
        let Some(terminal) = terminal else {
            return queued;
        };
        if self.active()
            || queued.recording_barrier.is_some()
            || queued.publication_wait_failure.is_some()
            || !matches!(
                queued.request,
                Request::SaveNew(_)
                    | Request::SaveProjectAs { .. }
                    | Request::SaveCurrent
                    | Request::SaveCurrentWithFormatUpgrade { .. }
                    | Request::Undo
                    | Request::Redo
                    | Request::Export(_)
                    | Request::ExportRange { .. }
            )
            || !matches!(
                terminal.phase(),
                crate::device_gesture::GesturePhase::Commit
                    | crate::device_gesture::GesturePhase::Cancel
            )
            || !save_matches(queued.save_correlation, None, source)
        {
            return queued;
        }
        if queued.document_intent != document_intent {
            queued.recording_barrier = Some(Completion::Failed(
                "recording action cancelled · project changed",
            ));
            return queued;
        }
        let continuation = QueuedRequest::continuation(
            Request::DeviceGesture(terminal),
            &AtomicU64::new(queued.document_intent),
        );
        self.pending = Some(Pending {
            request: queued,
            source,
            terminal,
            started: Instant::now(),
            completion: None,
        });
        continuation
    }

    /// Called only after the canonical state/revision for this event is installed.
    pub(super) fn complete(&mut self, ack: &GestureAck, after: hex_engine::AudioSourceContext) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        if pending.completion.is_some()
            || ack.key != pending.terminal.key()
            || ack.phase != pending.terminal.phase()
            || ack.sequence != pending.terminal.sequence()
        {
            return;
        }
        pending.completion = Some(
            if ack.accepted
                && !ack.source_lost
                && !ack.preview_retained
                && after.project_generation == pending.source.project_generation
                && (after.editor_revision == pending.source.editor_revision
                    || pending.source.editor_revision.checked_add(1) == Some(after.editor_revision))
            {
                Completion::Finished {
                    before: pending.source,
                    after,
                }
            } else {
                Completion::Failed(
                    "recording could not finish · original take retained for recovery",
                )
            },
        );
    }

    pub(super) fn poll(
        &mut self,
        current: hex_engine::AudioSourceContext,
        document_intent: u64,
    ) -> Option<QueuedRequest> {
        let pending = self.pending.as_mut()?;
        let completion = if pending.request.document_intent != document_intent {
            Completion::Failed("recording action cancelled · project changed")
        } else if let Some(completion) = pending.completion {
            match completion {
                Completion::Finished { after, .. } if current != after => {
                    Completion::Failed("recording action cancelled · source changed")
                }
                completion => completion,
            }
        } else if pending.source != current {
            Completion::Failed("recording action cancelled · source changed")
        } else if pending.started.elapsed() >= DEADLINE {
            Completion::Failed("recording action timed out · original take retained for recovery")
        } else {
            return None;
        };
        let mut pending = self.pending.take()?;
        pending.request.recording_barrier = Some(completion);
        Some(pending.request)
    }
}

#[cfg(test)]
mod tests;
