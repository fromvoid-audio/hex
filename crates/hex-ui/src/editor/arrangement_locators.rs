//! Arrangement locator projection, hit targets, and revision-bound pointer input.
//! Only visible markers are painted. A released move retains its final position
//! until its own ordered worker terminal replaces it with canonical state.
//! Accepted adds paint immediately at the requested frame. Matched committed
//! views supply identities before the ordered terminal installs canonical state.
use super::{
    EditorMode, EditorView, ViewState, browser_input,
    pending_edit::{Disposition, Receipt},
    viewport::Viewport,
};
use crate::theme;
use gpui::{
    Context, Div, Entity, FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Subscription, Window, canvas, div, prelude::*, px, relative, rgb,
};
use hex_project::{ArrangementLocatorAppearance, Command, LocatorId, Project};
use hex_session::service::{Event, Request};
use std::sync::{Arc, Weak, atomic::Ordering::Acquire};

const HEIGHT: f32 = 18.0;
const DRAG_THRESHOLD: f32 = 3.0;

#[derive(Clone, Copy)]
struct Source {
    generation: u64,
    revision: u64,
    id: LocatorId,
}

impl Source {
    fn current(self, view: &EditorView) -> bool {
        self.generation == view.project_generation
            && view.mode == EditorMode::Arrangement
            && view.locator_state().is_some_and(|state| {
                self.revision == state.revision
                    && state.project.arrangement_locator(self.id).is_some()
            })
    }
}

struct Drag {
    source: Source,
    viewport: Viewport,
    width: f32,
    pointer: gpui::Point<gpui::Pixels>,
    frame: u64,
    preview: u64,
    moved: bool,
    click_disqualified: bool,
}

struct Pending {
    receipt: Receipt,
    id: LocatorId,
    frame: u64,
}

struct PendingAdd {
    receipt: Receipt,
    frame: u64,
}

impl PendingAdd {
    fn matches(&self, request: &Request) -> bool {
        self.receipt.is_queued()
            && matches!(request, Request::Edit(Command::AddArrangementLocator { frame })
                if *frame == self.frame)
    }
}

struct NameEdit {
    source: Source,
    input: Entity<hex_widgets::TextInput>,
    _blur: Subscription,
}

pub(super) struct Locators {
    focus: FocusHandle,
    selected: Option<(u64, LocatorId)>,
    cache: Option<(Weak<Project>, Vec<ArrangementLocatorAppearance>)>,
    drag: Option<Drag>,
    pending: Option<Pending>,
    adds: Vec<PendingAdd>,
    committed: Option<Arc<ViewState>>,
    name: Option<NameEdit>,
}

impl Locators {
    pub(super) fn new(cx: &mut Context<EditorView>) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected: None,
            cache: None,
            drag: None,
            pending: None,
            adds: Vec::new(),
            committed: None,
            name: None,
        }
    }

    pub(super) fn cancel_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }

    pub(super) fn naming(&self) -> bool {
        self.name.is_some()
    }

    fn visible(&self, viewport: Viewport) -> &[ArrangementLocatorAppearance] {
        let Some((_, rows)) = &self.cache else {
            return &[];
        };
        let Some(end) = viewport.end() else {
            return &[];
        };
        let first = rows.partition_point(|locator| locator.frame < viewport.start);
        let last = rows.partition_point(|locator| locator.frame < end);
        &rows[first..last]
    }

    fn preview_frame(&self, id: LocatorId, frame: u64) -> u64 {
        if let Some(drag) = &self.drag
            && drag.source.id == id
        {
            return drag.preview;
        }
        self.pending
            .as_ref()
            .filter(|pending| pending.id == id)
            .map_or(frame, |pending| pending.frame)
    }

    pub(super) fn matches(&self, request: &Request) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.receipt.is_queued()
                && matches!(request,
                Request::Edit(Command::MoveArrangementLocator { locator, frame })
                if *locator == pending.id && *frame == pending.frame)
        })
    }

    pub(super) fn submitted(&mut self, pending_requests: usize) {
        if let Some(pending) = &mut self.pending {
            pending.receipt.submitted(pending_requests);
        }
    }

    pub(super) fn refuse(&mut self) -> bool {
        self.pending.take().is_some()
    }

    pub(super) fn queued_add_frame(&self, request: &Request) -> Option<u64> {
        self.adds
            .iter()
            .find(|add| add.matches(request))
            .map(|add| add.frame)
    }

    pub(super) fn add_submitted(&mut self, frame: u64, pending_requests: usize) {
        if let Some(add) = self.adds.iter_mut().find(|add| add.frame == frame) {
            add.receipt.submitted(pending_requests);
        }
    }

    pub(super) fn refuse_add(&mut self, frame: u64) -> bool {
        let before = self.adds.len();
        self.adds.retain(|add| add.frame != frame);
        self.adds.len() != before
    }

    pub(super) fn clear_adds(&mut self) -> bool {
        let adds = !std::mem::take(&mut self.adds).is_empty();
        self.committed.take().is_some() || adds
    }

    fn visible_adds(&self, viewport: Viewport) -> impl Iterator<Item = u64> + '_ {
        let end = viewport.end();
        self.adds.iter().map(|add| add.frame).filter(move |frame| {
            *frame >= viewport.start
                && end.is_some_and(|end| *frame < end)
                && self.cache.as_ref().is_none_or(|(_, rows)| {
                    rows.binary_search_by_key(frame, |locator| locator.frame)
                        .is_err()
                })
        })
    }

    pub(super) fn observe(&mut self, generation: u64, event: &Event) {
        if self.pending.as_mut().is_some_and(|pending| {
            pending.receipt.observe(
                generation,
                event.project_replaced,
                event.request_complete && !event.plugin_parameter_observation,
            ) != Disposition::Waiting
        }) {
            self.pending = None;
        }
        self.adds.retain_mut(|add| {
            add.receipt.observe(
                generation,
                event.project_replaced,
                event.request_complete && !event.plugin_parameter_observation,
            ) == Disposition::Waiting
        });
        if event.project_replaced
            || self
                .committed
                .as_ref()
                .is_some_and(|view| event.state.revision >= view.revision)
        {
            self.committed = None;
        }
        if event.project_replaced {
            self.selected = None;
            self.drag = None;
            self.name = None;
            self.cache = None;
        }
    }
}

fn position(frame: u64, viewport: Viewport) -> f32 {
    (frame.saturating_sub(viewport.start) as f64 / viewport.length.max(1) as f64) as f32
}

mod actions;
mod render;

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
