//! Explicit source assignment for the currently unassigned track.
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use crate::editor::context_menu::EditorContextAction;
use gpui::{FocusHandle, Point, SharedString, canvas, point};
use hex_project::SourceId;
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
pub(super) struct State {
    pub(super) focus: Option<FocusHandle>,
    pub(super) bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

#[derive(Clone)]
pub(in crate::editor) struct Choice {
    generation: u64,
    revision: u64,
    track: TrackId,
    source: SourceId,
    pub(in crate::editor) label: SharedString,
}

impl Choice {
    pub(in crate::editor) fn current(&self, view: &EditorView) -> bool {
        let selected = view.selected_track_ids();
        view.project_generation == self.generation
            && view.selected_track() == Some(self.track)
            && (selected.is_empty() || selected.as_slice() == [self.track])
            && view.device_panel.owner == Some(DevicePanelOwner::Track(self.track))
            && view.state.as_ref().is_some_and(|state| {
                state.revision == self.revision
                    && state.project.native_source(self.source).is_some()
                    && state.project.track(self.track).is_some_and(|track| {
                        !track.is_group() && track.source_id().is_none()
                    })
            })
    }

    pub(in crate::editor) fn perform(self, view: &mut EditorView, cx: &mut Context<EditorView>) {
        if !self.current(view)
            || view.edit_pending()
            || view.tuning.client.snapshot().is_some()
            || view.tuning.client.has_pending_requests()
        {
            view.error = Some("source assignment canceled · finish the current edit".into());
            cx.notify();
            return;
        }
        if !view.enqueue(Request::BindUnassignedNativeSource {
            project_generation: self.generation,
            revision: self.revision,
            track: self.track,
            source: self.source,
        }) {
            view.error = Some("could not assign source".into());
        }
        cx.notify();
    }
}

fn open(
    view: &mut EditorView,
    track: TrackId,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if view.edit_pending()
        || view.tuning.client.snapshot().is_some()
        || view.tuning.client.has_pending_requests()
    {
        view.error = Some("finish the current edit before assigning a source".into());
        cx.notify();
        return;
    }
    let Some(state) = view.state.as_ref() else { return };
    let items = state.project.native_sources().iter().map(|source| {
        let count = state.project.source_contributors(source.id()).count();
        let receiver = source.receiver().and_then(|id| state.project.track(id))
            .map(|track| hex_session::service::text::verbatim(track.name()))
            .unwrap_or_else(|| "none".into());
        let label = format!("{} · {count} midi · receiver {receiver}", hex_session::service::text::verbatim(source.name()));
        MenuItem {
            label: "source",
            swatch: None,
            checked: false,
            action: EditorContextAction::SourcePicker(Choice {
                generation: view.project_generation,
                revision: state.revision,
                track,
                source: source.id(),
                label: label.into(),
            }),
        }
    }).collect::<Vec<_>>();
    if items.is_empty() { return; }
    if items.iter().any(|item| match &item.action {
        EditorContextAction::SourcePicker(choice) => !choice.current(view),
        _ => false,
    }) {
        view.error = Some("select one unassigned track to choose a source".into());
        cx.notify();
        return;
    }
    let previous_focus = view.context_menu.take().and_then(|menu| menu.previous_focus)
        .or_else(|| window.focused(cx));
    let focus = cx.focus_handle();
    window.focus(&focus);
    view.context_menu = Some(PopupMenu {
        scroll: gpui::ScrollHandle::new(), icon_grid: None, position, items,
        selected: 0, focus, previous_focus,
    });
    cx.notify();
}

pub(super) fn button(view: &mut EditorView, window: Option<&Window>, cx: &mut Context<EditorView>, track: TrackId) -> Stateful<Div> {
    let available = view.state.as_ref().is_some_and(|state| !state.project.native_sources().is_empty());
    let focus = view.device_panel.source_picker.focus.get_or_insert_with(|| cx.focus_handle().tab_index(0)).clone().tab_stop(available);
    let key_focus = focus.clone();
    let bounds = Rc::clone(&view.device_panel.source_picker.bounds);
    let measured = Rc::clone(&bounds);
    let focused = window.is_some_and(|window| focus.is_focused(window));
    div().id("device-source-picker").debug_selector(|| "device-source-picker".into())
        .relative().track_focus(&focus).px(px(4.0)).text_xs()
        .when(focused, |style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .when(available, |style| style.hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised)))))
        .child(theme::text(if available { "source · choose" } else { "source · none" }))
        .child(canvas(move |rect, _, _| measured.set(Some(rect)), |_, _, _, _| {})
            .absolute().inset_0())
        .on_click(cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
            if available { open(view, track, event.position(), window, cx); }
        }))
        .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
            if available && key_focus.is_focused(window) && !event.keystroke.modifiers.modified()
                && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                if let Some(rect) = bounds.get() {
                    open(view, track, point(rect.left(), rect.bottom()), window, cx);
                    cx.stop_propagation();
                }
            }
        }))
}
