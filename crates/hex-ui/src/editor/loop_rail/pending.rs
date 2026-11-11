//! One accepted loop intent, correlated with the existing worker FIFO.
//! Section focus is a view-only consequence of its successful canonical install,
//! guarded by the captured selection intent and current stable section scope.
use super::{EditorView, LoopRegion, PendingLoop, Request};
use hex_project::Command;
use hex_session::arrangement_seek::SectionLaunch;
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
            focus_intent: None,
            start_intent: None,
            launch: None,
        });
        if !self.enqueue(Request::Edit(Command::SetLoopRegion {
            start: region.start(),
            end: region.end(),
            enabled: region.enabled(),
        })) {
            self.pending_loop = None;
        }
    }

    pub(in crate::editor) fn submit_section_launch_loop(&mut self, launch: SectionLaunch) {
        self.submit_loop_preview(self.project_generation, launch.region);
        if let Some(pending) = self.pending_loop.as_mut() {
            pending.launch = Some(launch);
        }
    }

    pub(in crate::editor) fn pending_loop_launch(&self) -> bool {
        self.pending_loop
            .is_some_and(|pending| pending.launch.is_some())
    }

    pub(in crate::editor) fn cancel_pending_loop_launch(&mut self) {
        if let Some(pending) = self.pending_loop.as_mut() {
            pending.launch = None;
        }
    }

    pub(in crate::editor) fn submit_selection_loop_preview(
        &mut self,
        region: LoopRegion,
        section: bool,
    ) {
        let intent = self.selection_intent_epoch;
        self.submit_loop_preview(self.project_generation, region);
        if let Some(pending) = self.pending_loop.as_mut() {
            pending.start_intent = intent;
            if section {
                pending.focus_intent = intent;
            }
        }
    }

    pub(in crate::editor) fn pending_selection_loop_has_newer_intent(&self) -> bool {
        self.pending_loop.is_some_and(|pending| {
            pending.start_intent.is_some() && pending.start_intent != self.selection_intent_epoch
        })
    }

    pub(in crate::editor) fn cancel_pending_loop_focus(&mut self) {
        if let Some(pending) = self.pending_loop.as_mut() {
            pending.focus_intent = None;
        }
    }

    /// Called only after the exact terminal's canonical snapshot is installed.
    pub(in crate::editor) fn loop_state_installed(&mut self, succeeded: bool) {
        let Some(pending) = self.pending_loop.take() else {
            return;
        };
        if succeeded
            && pending.generation == self.project_generation
            && let Some(launch) = pending.launch
        {
            self.section_loop_committed(launch);
        }
        if succeeded
            && pending.generation == self.project_generation
            && pending.start_intent.is_some()
            && pending.start_intent == self.selection_intent_epoch
            && self
                .state
                .as_ref()
                .and_then(|state| state.project.loop_region())
                == Some(pending.region)
            && let Some(epoch) = pending.start_intent
        {
            self.selection_loop_start = Some((epoch, pending.region));
        }
        if !succeeded
            || pending.generation != self.project_generation
            || pending.focus_intent.is_none()
            || pending.focus_intent != self.selection_intent_epoch
            || self
                .state
                .as_ref()
                .and_then(|state| state.project.loop_region())
                != Some(pending.region)
            || self.section_loop_bounds()
                != Ok(Some((pending.region.start(), pending.region.end())))
        {
            return;
        }
        if !self.arrangement_focus.projection.enabled {
            self.toggle_arrangement_focus();
        } else {
            self.reconcile_arrangement_focus();
        }
    }
}
