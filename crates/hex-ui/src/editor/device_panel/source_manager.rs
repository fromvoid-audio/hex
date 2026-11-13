//! Project-wide retained-source inventory and explicit deletion.
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use crate::editor::context_menu::EditorContextAction;
use gpui::{FocusHandle, Point, SharedString, canvas, point};
use hex_project::SourceId;
use std::{cell::Cell, rc::Rc};

gpui::actions!(hex_source_manager, [OpenSourceManager]);

pub(in crate::editor) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("enter", OpenSourceManager, Some("hex_source_manager")),
        gpui::KeyBinding::new("space", OpenSourceManager, Some("hex_source_manager")),
    ]);
}

#[derive(Default)]
pub(super) struct State {
    pub(super) focus: Option<FocusHandle>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

#[derive(Clone, Copy)]
enum Operation {
    Open,
    Details(SourceId),
    Delete(SourceId),
}

#[derive(Clone)]
pub(in crate::editor) struct Action {
    generation: u64,
    revision: u64,
    position: Point<Pixels>,
    operation: Operation,
    pub(in crate::editor) label: SharedString,
}

impl Action {
    pub(super) fn open(generation: u64, revision: u64, position: Point<Pixels>) -> Self {
        Self {
            generation,
            revision,
            position,
            operation: Operation::Open,
            label: "manage sources".into(),
        }
    }

    pub(in crate::editor) fn current(&self, view: &EditorView) -> bool {
        view.project_generation == self.generation
            && view.state.as_ref().is_some_and(|state| {
                state.revision == self.revision
                    && match self.operation {
                        Operation::Open => true,
                        Operation::Details(source) | Operation::Delete(source) => {
                            state.project.native_source(source).is_some()
                        }
                    }
            })
    }

    pub(in crate::editor) fn perform(
        self,
        view: &mut EditorView,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        if !self.current(view) {
            view.error = Some("source manager canceled · project changed".into());
            cx.notify();
            return;
        }
        match self.operation {
            Operation::Open => open(view, self.position, window, cx),
            Operation::Details(source) => details(view, source, self.position, window, cx),
            Operation::Delete(source) => {
                if view.edit_pending()
                    || view.tuning.client.snapshot().is_some()
                    || view.tuning.client.has_pending_requests()
                {
                    view.error = Some("finish the current edit before deleting a source".into());
                    cx.notify();
                    return;
                }
                let _ = view.enqueue(Request::DeleteRetainedSource {
                    project_generation: self.generation,
                    revision: self.revision,
                    source,
                });
                cx.notify();
            }
        }
    }
}

fn show(
    view: &mut EditorView,
    position: Point<Pixels>,
    items: Vec<MenuItem<EditorContextAction>>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let previous_focus = view
        .context_menu
        .take()
        .and_then(|menu| menu.previous_focus)
        .or_else(|| window.focused(cx));
    let focus = cx.focus_handle();
    window.focus(&focus);
    view.context_menu = Some(PopupMenu {
        scroll: gpui::ScrollHandle::new(),
        icon_grid: None,
        position,
        items,
        selected: 0,
        focus,
        previous_focus,
    });
    cx.notify();
}

fn open(
    view: &mut EditorView,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let generation = view.project_generation;
    let revision = state.revision;
    let items = state
        .project
        .native_sources()
        .iter()
        .map(|source| {
            let count = state.project.source_contributors(source.id()).count();
            let receiver = source
                .receiver()
                .and_then(|id| state.project.track(id))
                .map(|track| hex_session::service::text::verbatim(track.name()))
                .unwrap_or_else(|| "none".into());
            let label = format!(
                "{} · {count} midi · receiver {receiver}",
                hex_session::service::text::verbatim(source.name())
            );
            MenuItem {
                label: "source",
                swatch: None,
                checked: false,
                action: EditorContextAction::SourceManager(Action {
                    generation,
                    revision,
                    position,
                    operation: Operation::Details(source.id()),
                    label: label.into(),
                }),
            }
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        view.status = "no retained sources".into();
        cx.notify();
        return;
    }
    show(view, position, items, window, cx);
}

fn details(
    view: &mut EditorView,
    source: SourceId,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let Some(definition) = state.project.native_source(source) else {
        return;
    };
    let label = format!(
        "delete · {}",
        hex_session::service::text::verbatim(definition.name())
    );
    let action = Action {
        generation: view.project_generation,
        revision: state.revision,
        position,
        operation: Operation::Delete(source),
        label: label.into(),
    };
    show(
        view,
        position,
        vec![MenuItem {
            label: "delete source",
            swatch: None,
            checked: false,
            action: EditorContextAction::SourceManager(action),
        }],
        window,
        cx,
    );
}

pub(super) fn button(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let focus = view
        .device_panel
        .source_manager
        .focus
        .get_or_insert_with(|| cx.focus_handle().tab_index(0))
        .clone()
        .tab_stop(true);
    let bounds = Rc::clone(&view.device_panel.source_manager.bounds);
    let measured = Rc::clone(&bounds);
    let focused = window.is_some_and(|window| focus.is_focused(window));
    div()
        .id("source-manager-button")
        .debug_selector(|| "source-manager-button".into())
        .relative()
        .ml(px(8.0))
        .px(px(4.0))
        .text_xs()
        .track_focus(&focus)
        .key_context("hex_source_manager")
        .when(focused, |style| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
        .child(theme::text("sources ▾"))
        .child(
            canvas(move |rect, _, _| measured.set(Some(rect)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .on_click(cx.listener(|view, event: &gpui::ClickEvent, window, cx| {
            open(view, event.position(), window, cx)
        }))
        .on_action(cx.listener(move |view, _: &OpenSourceManager, window, cx| {
            if let Some(rect) = bounds.get() {
                open(view, point(rect.left(), rect.bottom()), window, cx);
            }
            cx.stop_propagation();
        }))
        .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key == "tab"
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt
                && !event.keystroke.modifiers.platform
            {
                if event.keystroke.modifiers.shift {
                    window.focus_prev();
                } else {
                    window.focus_next();
                }
                cx.stop_propagation();
            }
        }))
}
