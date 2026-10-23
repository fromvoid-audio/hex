//! One accepted loop intent, correlated with the existing worker FIFO.
use super::{EditorView, LoopRegion, PendingLoop, Request};
use hex_project::Command;
use hex_session::service::Event;

impl EditorView {
    pub(super) fn pending_loop_region(&self) -> Option<LoopRegion> {
        self.pending_loop
            .filter(|pending| pending.generation == self.project_generation)
            .map(|pending| pending.region)
    }

    pub(in crate::editor) fn pending_loop_matches(&self, request: &Request) -> bool {
        self.pending_loop.is_some_and(|pending| {
            pending.generation == self.project_generation
                && pending.remaining.is_none()
                && matches!(request, Request::Edit(Command::SetLoopRegion { start, end, enabled })
                    if (*start, *end, *enabled) == (pending.region.start(), pending.region.end(), pending.region.enabled()))
        })
    }

    pub(in crate::editor) fn loop_request_submitted(&mut self) {
        if let Some(pending) = self.pending_loop.as_mut() {
            pending.remaining = Some(self.worker_requests_pending);
        }
    }

    /// Earlier completions advance the count, never retire our displayed range.
    /// The caller clears a matching terminal only after installing its state.
    pub(in crate::editor) fn loop_request_complete(&mut self, event: &Event) -> bool {
        let Some(pending) = self.pending_loop.as_mut() else {
            return false;
        };
        if event.project_replaced || pending.generation != self.project_generation {
            self.pending_loop = None;
            return false;
        }
        if event.request_complete
            && let Some(remaining) = pending.remaining.as_mut()
        {
            *remaining = remaining.saturating_sub(1);
            return *remaining == 0;
        }
        false
    }

    pub(super) fn submit_loop_preview(&mut self, generation: u64, region: LoopRegion) {
        self.pending_loop = Some(PendingLoop {
            generation,
            region,
            remaining: None,
        });
        if !self.enqueue(Request::Edit(Command::SetLoopRegion {
            start: region.start(),
            end: region.end(),
            enabled: region.enabled(),
        })) {
            self.pending_loop = None;
        }
    }
}
