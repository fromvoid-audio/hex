//! Explicit source assignment for the selected track.
use super::*;
use crate::context_menu::{MenuItem, PopupMenu};
use crate::editor::context_menu::EditorContextAction;
use crate::editor::mode::EditorMode;
use gpui::{FocusHandle, Point, SharedString, canvas, point};
use hex_project::SourceId;
use std::{cell::Cell, rc::Rc};

gpui::actions!(hex_source_picker, [OpenSourcePicker]);

pub(in crate::editor) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        gpui::KeyBinding::new("enter", OpenSourcePicker, Some("hex_source_picker")),
        gpui::KeyBinding::new("space", OpenSourcePicker, Some("hex_source_picker")),
    ]);
}

#[derive(Default)]
pub(super) struct State {
    pub(super) focus: Option<FocusHandle>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

#[derive(Clone)]
pub(in crate::editor) struct Choice {
    generation: u64,
    revision: u64,
    track: TrackId,
    source: SourceId,
    expected_source: Option<SourceId>,
    unlink: bool,
    destination: Option<TrackId>,
    receiver: Option<(Option<TrackId>, TrackId)>,
    pub(in crate::editor) label: SharedString,
}

impl Choice {
    // Rows are built only from the captured immutable project after checking
    // the target. An unchanged generation/revision keeps those facts current.
    pub(in crate::editor) fn current(&self, view: &EditorView) -> bool {
        view.project_generation == self.generation
            && view.selected_track() == Some(self.track)
            && view.track_selection.only_or_empty(self.track)
            && view.device_panel.owner == Some(DevicePanelOwner::Track(self.track))
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision == self.revision)
    }

    pub(in crate::editor) fn perform(
        self,
        view: &mut EditorView,
        window: &mut Window,
        cx: &mut Context<EditorView>,
    ) {
        if !self.current(view)
            || !view.state.as_ref().is_some_and(|state| {
                state.project.native_source(self.source).is_some()
                    && self.receiver.is_none_or(|(expected, target)| {
                        state
                            .project
                            .native_source(self.source)
                            .is_some_and(|source| source.receiver() == expected)
                            && state
                                .project
                                .track(target)
                                .is_some_and(|track| track.source_id() == Some(self.source))
                    })
                    && self.destination.is_none_or(|target| {
                        state
                            .project
                            .track(target)
                            .is_some_and(|linked| linked.source_id() == Some(self.source))
                    })
                    && state.project.track(self.track).is_some_and(|track| {
                        !track.is_group() && track.source_id() == self.expected_source
                    })
            })
        {
            view.error = Some(if self.destination.is_some() {
                "source navigation canceled · selection changed".into()
            } else if self.receiver.is_some() {
                "source receiver selection canceled · selection changed".into()
            } else {
                "source assignment canceled · selection changed".into()
            });
            cx.notify();
            return;
        }
        if view.edit_pending()
            || view.tuning.client.snapshot().is_some()
            || view.tuning.client.has_pending_requests()
        {
            view.error = Some("source assignment canceled · finish the current edit".into());
            cx.notify();
            return;
        }
        if let Some(track) = self.destination {
            navigate_to_track(view, track, window, cx);
            return;
        }
        if let Some((expected_receiver, track)) = self.receiver {
            let _ = view.enqueue(Request::ChooseNativeSourceReceiver {
                project_generation: self.generation,
                revision: self.revision,
                source: self.source,
                expected_receiver,
                track,
            });
            cx.notify();
            return;
        }
        if self.expected_source == Some(self.source) && !self.unlink {
            return;
        }
        let request = if self.expected_source.is_some() {
            Request::RebindNativeSource {
                project_generation: self.generation,
                revision: self.revision,
                track: self.track,
                expected_source: self.expected_source,
                source: (!self.unlink).then_some(self.source),
            }
        } else {
            Request::BindUnassignedNativeSource {
                project_generation: self.generation,
                revision: self.revision,
                track: self.track,
                source: self.source,
            }
        };
        let _ = view.enqueue(request);
        cx.notify();
    }
}

pub(super) fn navigate_to_track(
    view: &mut EditorView,
    track: TrackId,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    view.select_track(track, window);
    view.focus.focus(window);
    if view.mode == EditorMode::Arrangement {
        let row = view.state.as_ref().and_then(|state| {
            state
                .project
                .tracks()
                .iter()
                .position(|item| item.id() == track)
                .and_then(|index| view.arrangement_row_index(index))
        });
        if let Some(row) = row {
            view.track_scroll
                .scroll_to_item(row, gpui::ScrollStrategy::Top);
        } else {
            view.status = "track selected · hidden by the current view".into();
        }
    } else {
        let visible = view.state.as_ref().is_some_and(|state| {
            view.arrangement_focus
                .projection
                .console_indices
                .iter()
                .any(|index| {
                    state
                        .project
                        .tracks()
                        .get(*index)
                        .is_some_and(|item| item.id() == track)
                })
        });
        view.reveal_session_position(Some(track), None);
        if !visible {
            view.status = "track selected · hidden by the current view".into();
        }
    }
    cx.notify();
}

pub(in crate::editor) fn focus_from_workspace(view: &EditorView, window: &mut Window) -> bool {
    let Some(track) = view.selected_track() else {
        return false;
    };
    if !view.clip_view_open
        || !view.device_panel_active
        || view.device_panel.owner != Some(DevicePanelOwner::Track(track))
        || !view.track_selection.only_or_empty(track)
        || !view.state.as_ref().is_some_and(|state| {
            !state.project.native_sources().is_empty()
                && state
                    .project
                    .track(track)
                    .is_some_and(|track| !track.is_group())
        })
    {
        return false;
    }
    let Some(focus) = view.device_panel.source_picker.focus.as_ref() else {
        return false;
    };
    focus.focus(window);
    true
}

fn open(
    view: &mut EditorView,
    track: TrackId,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if !view.track_selection.only_or_empty(track) {
        view.error = Some("select one track to choose a source".into());
        cx.notify();
        return;
    }
    if view.edit_pending()
        || view.tuning.client.snapshot().is_some()
        || view.tuning.client.has_pending_requests()
    {
        view.error = Some("finish the current edit before assigning a source".into());
        cx.notify();
        return;
    }
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let Some(expected_source) = state
        .project
        .track(track)
        .filter(|track| !track.is_group())
        .map(|track| track.source_id())
    else {
        return;
    };
    let generation = view.project_generation;
    let items = state
        .project
        .native_sources()
        .iter()
        .flat_map(|source| {
            let count = state.project.source_contributors(source.id()).count();
            let receiver = source
                .receiver()
                .and_then(|id| state.project.track(id))
                .map(|track| hex_session::service::text::verbatim(track.name()))
                .unwrap_or_else(|| "none".into());
            let label = format!(
                "{count} midi · {} · receiver {receiver}",
                hex_session::service::text::verbatim(source.name())
            );
            let assignment = MenuItem {
                label: "source",
                swatch: None,
                checked: expected_source == Some(source.id()),
                action: EditorContextAction::SourcePicker(Choice {
                    generation,
                    revision: state.revision,
                    track,
                    source: source.id(),
                    expected_source,
                    unlink: false,
                    destination: None,
                    receiver: None,
                    label: label.into(),
                }),
            };
            let tracks = state
                .project
                .tracks()
                .iter()
                .filter(move |linked| linked.source_id() == Some(source.id()))
                .flat_map(move |linked| {
                    let role = if source.receiver() == Some(linked.id()) {
                        "receiver"
                    } else {
                        "midi"
                    };
                    let navigate = MenuItem {
                        label: "source track",
                        swatch: None,
                        checked: false,
                        action: EditorContextAction::SourcePicker(Choice {
                            generation,
                            revision: state.revision,
                            track,
                            source: source.id(),
                            expected_source,
                            unlink: false,
                            destination: Some(linked.id()),
                            receiver: None,
                            label: format!(
                                "  go to · {role} · {}",
                                hex_session::service::text::verbatim(linked.name())
                            )
                            .into(),
                        }),
                    };
                    let choose_receiver = (expected_source == Some(source.id())
                        && source.receiver() != Some(linked.id()))
                    .then(|| MenuItem {
                        label: "choose receiver",
                        swatch: None,
                        checked: false,
                        action: EditorContextAction::SourcePicker(Choice {
                            generation,
                            revision: state.revision,
                            track,
                            source: source.id(),
                            expected_source,
                            unlink: false,
                            destination: None,
                            receiver: Some((source.receiver(), linked.id())),
                            label: format!(
                                "  make receiver · {}",
                                hex_session::service::text::verbatim(linked.name())
                            )
                            .into(),
                        }),
                    });
                    std::iter::once(navigate).chain(choose_receiver)
                });
            std::iter::once(assignment).chain(tracks)
        })
        .chain(expected_source.map(|source| MenuItem {
            label: "unlink source",
            swatch: None,
            checked: false,
            action: EditorContextAction::SourcePicker(Choice {
                generation,
                revision: state.revision,
                track,
                source,
                expected_source,
                unlink: true,
                destination: None,
                receiver: None,
                label: "unlink source".into(),
            }),
        }))
        .chain(std::iter::once(MenuItem {
            label: "manage sources",
            swatch: None,
            checked: false,
            action: EditorContextAction::SourceManager(super::source_manager::Action::open(
                generation,
                state.revision,
                position,
            )),
        }))
        .collect::<Vec<_>>();
    if items.iter().any(|item| match &item.action {
        EditorContextAction::SourcePicker(choice) => !choice.current(view),
        _ => false,
    }) {
        view.error = Some("select one track to choose a source".into());
        cx.notify();
        return;
    }
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

pub(super) fn button(
    view: &mut EditorView,
    window: Option<&Window>,
    cx: &mut Context<EditorView>,
    track: TrackId,
    summary: Option<Stateful<Div>>,
) -> Stateful<Div> {
    let bound = summary.is_some();
    let single_track = view.track_selection.only_or_empty(track);
    let available = single_track
        && view
            .state
            .as_ref()
            .is_some_and(|state| !state.project.native_sources().is_empty());
    let focus = view
        .device_panel
        .source_picker
        .focus
        .get_or_insert_with(|| cx.focus_handle().tab_index(0))
        .clone()
        .tab_stop(available);
    let bounds = Rc::clone(&view.device_panel.source_picker.bounds);
    let measured = Rc::clone(&bounds);
    let focused = window.is_some_and(|window| focus.is_focused(window));
    div()
        .id("device-source-picker")
        .debug_selector(|| "device-source-picker".into())
        .relative()
        .flex()
        .items_center()
        .min_w_0()
        .track_focus(&focus)
        .key_context("hex_source_picker")
        .px(px(4.0))
        .text_xs()
        .when(focused, |style| {
            style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
        })
        .text_color(rgb(theme::color(theme::PaletteRole::SecondaryText)))
        .when(available, |style| {
            style.hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Raised))))
        })
        .children(summary)
        .when(!bound, |style| {
            style.child(theme::text(if !single_track {
                "source"
            } else if available {
                "source · choose"
            } else {
                "source · none"
            }))
        })
        .when(bound && available, |style| style.child(theme::text(" ▾")))
        .when(!single_track, |style| {
            style.child(
                div()
                    .debug_selector(|| "source-picker-multiselect-hint".into())
                    .child(theme::text(" · select one track")),
            )
        })
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: if !single_track {
                    "select one track to choose a source".into()
                } else if available {
                    "choose an existing source · shift-tab from workspace".into()
                } else {
                    "no retained sources".into()
                },
            })
            .into()
        })
        .child(
            canvas(move |rect, _, _| measured.set(Some(rect)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .on_click(
            cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                if available {
                    open(view, track, event.position(), window, cx);
                }
            }),
        )
        .on_action(cx.listener(move |view, _: &OpenSourcePicker, window, cx| {
            if available && let Some(rect) = bounds.get() {
                open(view, track, point(rect.left(), rect.bottom()), window, cx);
            }
            cx.stop_propagation();
        }))
        .on_key_down(
            cx.listener(move |_, event: &gpui::KeyDownEvent, window, cx| {
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
            }),
        )
}
