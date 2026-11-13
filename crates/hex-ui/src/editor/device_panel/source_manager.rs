//! Project-wide retained-source inventory and explicit creation, naming, and deletion.
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use crate::editor::context_menu::EditorContextAction;
use gpui::{Entity, FocusHandle, Point, SharedString, canvas, point};
use hex_project::SourceId;
use hex_widgets::TextInput;
use std::{cell::Cell, rc::Rc};

gpui::actions!(
    hex_source_manager,
    [OpenSourceManager, CommitSourceName, CancelSourceName]
);

pub(in crate::editor) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("enter", OpenSourceManager, Some("hex_source_manager")),
        gpui::KeyBinding::new("space", OpenSourceManager, Some("hex_source_manager")),
        gpui::KeyBinding::new("enter", CommitSourceName, Some("hex_source_name")),
        gpui::KeyBinding::new("escape", CancelSourceName, Some("hex_source_name")),
    ]);
    cx.bind_keys(
        [
            "cmd-z",
            "cmd-shift-z",
            "cmd-n",
            "cmd-o",
            "cmd-s",
            "cmd-shift-s",
        ]
        .map(|key| gpui::KeyBinding::new(key, gpui::NoAction, Some("hex_source_name"))),
    );
}

#[derive(Default)]
pub(super) struct State {
    pub(super) focus: Option<FocusHandle>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    name: Option<NameEdit>,
}

struct NameEdit {
    generation: u64,
    revision: u64,
    source: SourceId,
    original: String,
    input: Entity<TextInput>,
}

pub(in crate::editor) fn naming(view: &EditorView) -> bool {
    view.device_panel.source_manager.name.is_some()
}

#[derive(Clone, Copy)]
enum Operation {
    Open,
    Details(SourceId),
    Delete(SourceId),
    ChooseInstrument,
    Create(hex_engine::DeviceTypeId),
    Rename(SourceId),
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
                        Operation::Open | Operation::ChooseInstrument | Operation::Create(_) => {
                            true
                        }
                        Operation::Details(source)
                        | Operation::Delete(source)
                        | Operation::Rename(source) => {
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
            Operation::Rename(source) => begin_name(view, source, window, cx),
            Operation::ChooseInstrument => instruments(view, self.position, window, cx),
            Operation::Create(type_id) => {
                if view.edit_pending()
                    || view.tuning.client.snapshot().is_some()
                    || view.tuning.client.has_pending_requests()
                {
                    view.error = Some("finish the current edit before creating a source".into());
                    cx.notify();
                    return;
                }
                let _ = view.enqueue(Request::CreateRetainedNativeSource {
                    project_generation: self.generation,
                    revision: self.revision,
                    type_id: Box::new(type_id),
                    name: self.label.to_string(),
                });
                cx.notify();
            }
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
    let mut items = state
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
    items.push(MenuItem {
        label: "create source",
        swatch: None,
        checked: false,
        action: EditorContextAction::SourceManager(Action {
            generation,
            revision,
            position,
            operation: Operation::ChooseInstrument,
            label: "create source".into(),
        }),
    });
    show(view, position, items, window, cx);
}

fn instruments(
    view: &mut EditorView,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let catalog = view
        .device_panel
        .instrument_catalog
        .clone()
        .unwrap_or_else(|| {
            hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Instrument)
                .map_err(|error| error.to_string())
        });
    let catalog = match catalog {
        Ok(catalog) => catalog,
        Err(error) => {
            view.error = Some(format!("device catalog error · {error}").into());
            cx.notify();
            return;
        }
    };
    let items = catalog
        .into_iter()
        .map(|entry| MenuItem {
            label: "instrument",
            swatch: None,
            checked: false,
            action: EditorContextAction::SourceManager(Action {
                generation: view.project_generation,
                revision: state.revision,
                position,
                operation: Operation::Create(entry.type_id),
                label: entry.name.into(),
            }),
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        view.error = Some("no native instruments available".into());
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
        vec![
            MenuItem {
                label: "delete source",
                swatch: None,
                checked: false,
                action: EditorContextAction::SourceManager(action),
            },
            MenuItem {
                label: "rename source",
                swatch: None,
                checked: false,
                action: EditorContextAction::SourceManager(Action {
                    generation: view.project_generation,
                    revision: state.revision,
                    position,
                    operation: Operation::Rename(source),
                    label: "rename source".into(),
                }),
            },
        ],
        window,
        cx,
    );
}

fn begin_name(
    view: &mut EditorView,
    source: SourceId,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if view.edit_pending()
        || view.tuning.client.snapshot().is_some()
        || view.tuning.client.has_pending_requests()
        || !super::finalize_controls(view, cx)
        || view.edit_pending()
    {
        view.error = Some("finish the current edit before naming a source".into());
        cx.notify();
        return;
    }
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let Some(original) = state
        .project
        .native_source(source)
        .map(|s| s.name().to_owned())
    else {
        return;
    };
    let generation = view.project_generation;
    let revision = state.revision;
    if !view.admit_independent_edit_with_local_draft(&Request::RenameRetainedSource {
        project_generation: generation,
        revision,
        source,
        name: original.clone(),
    }) {
        cx.notify();
        return;
    }
    let input = cx.new(|cx| super::super::browser_input::name(original.clone(), cx));
    input.read(cx).focus_handle().focus(window);
    view.device_panel.source_manager.name = Some(NameEdit {
        generation,
        revision,
        source,
        original,
        input,
    });
    cx.notify();
}

fn finish_name(
    view: &mut EditorView,
    commit: bool,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(edit) = view.device_panel.source_manager.name.take() else {
        return;
    };
    if commit {
        let name = edit.input.read(cx).text().to_owned();
        let valid = !name.trim().is_empty()
            && name.len() <= hex_project::MAX_NAME_BYTES
            && !name.chars().any(char::is_control);
        let current = edit.generation == view.project_generation
            && view.state.as_ref().is_some_and(|state| {
                state.revision == edit.revision
                    && state
                        .project
                        .native_source(edit.source)
                        .is_some_and(|source| source.name() == edit.original)
            });
        if !valid || !current || view.edit_pending() {
            view.error = Some(
                if !valid {
                    "source name is invalid"
                } else {
                    "source name canceled · project or edit changed"
                }
                .into(),
            );
            view.device_panel.source_manager.name = Some(edit);
            cx.notify();
            return;
        }
        if name != edit.original
            && !view.enqueue(Request::RenameRetainedSource {
                project_generation: edit.generation,
                revision: edit.revision,
                source: edit.source,
                name,
            })
        {
            view.device_panel.source_manager.name = Some(edit);
            cx.notify();
            return;
        }
    }
    view.focus.focus(window);
    cx.notify();
}

pub(in crate::editor) fn name_overlay(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Option<Stateful<Div>> {
    let input = view
        .device_panel
        .source_manager
        .name
        .as_ref()?
        .input
        .clone();
    super::super::browser_input::restyle(&input, cx);
    Some(
        div()
            .id("source-name-modal")
            .debug_selector(|| "source-name-modal".into())
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(gpui::rgba(0x00000077))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("source-name-editor")
                    .debug_selector(|| "source-name-editor".into())
                    .w(px(320.0))
                    .max_w_full()
                    .p(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .key_context("hex_source_name hex_text_editing")
                    .child(theme::text("rename source"))
                    .child(input)
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                hex_widgets::action(theme::text("cancel"), theme::action_style())
                                    .id("source-name-cancel")
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        finish_name(view, false, window, cx)
                                    })),
                            )
                            .child(
                                hex_widgets::action(theme::text("rename"), theme::action_style())
                                    .id("source-name-commit")
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        finish_name(view, true, window, cx)
                                    })),
                            ),
                    )
                    .on_action(cx.listener(|view, _: &CommitSourceName, window, cx| {
                        finish_name(view, true, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|view, _: &CancelSourceName, window, cx| {
                        finish_name(view, false, window, cx);
                        cx.stop_propagation();
                    })),
            ),
    )
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
