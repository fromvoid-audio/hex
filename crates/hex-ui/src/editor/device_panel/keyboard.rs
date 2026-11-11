//! Device-selection keys own only the directly focused card, never its editors.
//! C/zero read live canonical bypass and use the same terminal barrier as pointer
//! bypass; rendered generation/owner/type identity refuses retired card handlers.
//! Left/right step the selection through the chain, and paste targets a chosen
//! insertion gap, else the position after the selection. Duplicate uses the
//! worker clipboard edit and follows only its own committed insertion.
use super::*;
use crate::diagnostics::ActionTraceContext as _;
use crate::editor::pending_edit::{Disposition, Receipt};
use gpui::{FocusHandle, KeyBinding};
use hex_project::{Created, DeviceSelectionIds};
use hex_session::service::{DeviceClipboardAction, Event};

gpui::actions!(
    hex_device_selection,
    [
        RemoveSelectedDevice,
        RemoveSelectedDeviceX,
        CopyDevices,
        CutDevices,
        PasteDevices,
        DuplicateDevices,
        SelectPreviousDevice,
        SelectNextDevice
    ]
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeleteKey {
    Backspace,
    X,
}

#[derive(Clone, Copy)]
pub(super) struct RefusedDelete {
    key: DeleteKey,
    generation: u64,
    owner: Option<DevicePanelOwner>,
    device: DeviceId,
}

fn suspend_until_release(
    view: &mut EditorView,
    intent: RefusedDelete,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &Context<EditorView>,
) {
    if focus.is_focused(window) {
        view.device_panel.chain_window.refused_delete = Some(intent);
        view.device_panel
            .chain_window
            .selection_focus(cx)
            .focus(window);
    }
}

/// Keep the editor's keyboard route before an accepted pointer removal retires
/// its directly focused card. Nested numeric/graph focus is never redirected.
pub(super) fn after_pointer_removal(
    view: &mut EditorView,
    device: DeviceId,
    window: &mut Window,
    cx: &Context<EditorView>,
) {
    if view
        .device_panel
        .chain_window
        .device_is_focused(device, window)
    {
        view.device_panel.chain_window.refused_delete = None;
        view.device_panel
            .chain_window
            .selection_focus(cx)
            .focus(window);
    }
}

pub(in crate::editor) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
            "backspace",
            RemoveSelectedDevice,
            Some("hex_device_selection"),
        ),
        KeyBinding::new(
            "x",
            RemoveSelectedDeviceX,
            Some("hex_device_selection && !hex_text_editing && !hex_text_input && !hex_response_graph_remove"),
        ),
        KeyBinding::new("cmd-c", CopyDevices, Some("hex_device_selection")),
        KeyBinding::new("cmd-x", CutDevices, Some("hex_device_selection")),
        KeyBinding::new("cmd-v", PasteDevices, Some("hex_device_selection")),
        KeyBinding::new(
            "d",
            DuplicateDevices,
            Some("hex_device_selection && !hex_text_editing && !hex_text_input"),
        ),
        // Deeper than the editor's clip/session arrows; nested controls that
        // own focus receive them through propagation.
        KeyBinding::new("left", SelectPreviousDevice, Some("hex_device_selection")),
        KeyBinding::new("right", SelectNextDevice, Some("hex_device_selection")),
    ]);
}

#[derive(Clone, Copy)]
enum ClipboardIntent {
    Copy,
    Cut,
    Paste,
}

fn clipboard(
    view: &mut EditorView,
    intent: ClipboardIntent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    use hex_session::service::DeviceClipboardAction as Action;
    if view.edit_pending() || view.export_pending || view.device_panel.has_pending_draft(cx) {
        view.error = Some("finish pending edits before copying devices".into());
        cx.notify();
        return;
    }
    let Some(owner) = view.device_panel.owner else {
        return;
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    if selected_owner(view, state) != Some(owner) {
        return;
    }
    let order = chain_ids(&state.project, owner);
    let selected: Vec<_> = order
        .iter()
        .copied()
        .filter(|id| view.device_panel.is_selected(*id))
        .collect();
    let before = view
        .device_panel
        .insertion_before(&order)
        .unwrap_or_else(|| {
            order
                .iter()
                .rposition(|id| selected.contains(id))
                .and_then(|index| order.get(index + 1).copied())
        });
    let action = match intent {
        ClipboardIntent::Copy | ClipboardIntent::Cut => {
            let Ok(selected) = hex_project::DeviceSelectionIds::new(&selected) else {
                return;
            };
            match intent {
                ClipboardIntent::Copy => Action::Copy {
                    owner: project_owner(owner),
                    selected,
                },
                ClipboardIntent::Cut => Action::Cut {
                    owner: project_owner(owner),
                    selected,
                },
                ClipboardIntent::Paste => return,
            }
        }
        ClipboardIntent::Paste => Action::Paste {
            owner: project_owner(owner),
            before,
        },
    };
    let request = Request::DeviceClipboard {
        project_generation: view.project_generation,
        revision: state.revision,
        action,
    };
    if view.enqueue(request) && matches!(intent, ClipboardIntent::Cut) {
        view.device_panel
            .chain_window
            .selection_focus(cx)
            .focus(window);
    }
    cx.notify();
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ChainStamp {
    generation: u64,
    revision: u64,
    owner: DevicePanelOwner,
}

impl ChainStamp {
    fn current(view: &EditorView) -> Option<Self> {
        Some(Self {
            generation: view.project_generation,
            revision: view.state.as_ref()?.revision,
            owner: view.device_panel.owner?,
        })
    }
}

pub(in crate::editor) struct PendingDuplicate {
    stamp: ChainStamp,
    action: DeviceClipboardAction,
    receipt: Receipt,
    select_copies: bool,
}

impl DevicePanelState {
    pub(super) fn cancel_duplicate_selection_update(&mut self) {
        if let Some(pending) = &mut self.pending_duplicate {
            pending.select_copies = false;
        }
    }

    pub(in crate::editor) fn pending_duplicate_matches(&self, request: &Request) -> bool {
        self.pending_duplicate.as_ref().is_some_and(|pending| {
            pending.receipt.is_queued()
                && matches!(request, Request::DeviceClipboard { project_generation, revision, action }
                    if *project_generation == pending.stamp.generation
                        && *revision == pending.stamp.revision
                        && *action == pending.action)
        })
    }

    pub(in crate::editor) fn duplicate_submitted(&mut self, pending_count: usize) {
        if let Some(pending) = &mut self.pending_duplicate {
            pending.receipt.submitted(pending_count);
        }
    }
}

fn duplicate(
    view: &mut EditorView,
    painted: Option<ChainStamp>,
    focused_device: Option<DeviceId>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    const STALE: &str = "device copy canceled · selection changed";
    let Some(primary) = view.device_panel.selected_device else {
        return;
    };
    let current = ChainStamp::current(view);
    let (Some(stamp), Some(state)) = (current, view.state.as_ref()) else {
        return;
    };
    if painted != current
        || view.device_panel.generation != stamp.generation
        || focused_device.is_some_and(|device| device != primary)
        || !view.clip_view_open
        || !view.device_panel_active
        || selected_owner(view, state) != Some(stamp.owner)
    {
        view.error = Some(STALE.into());
        cx.notify();
        return;
    }
    if view.edit_pending()
        || view.export_pending
        || view.device_panel.pending_duplicate.is_some()
        || view.device_panel.has_pending_draft(cx)
    {
        view.error = Some("finish pending edits before copying devices".into());
        cx.notify();
        return;
    }
    let order = chain_ids(&state.project, stamp.owner);
    let selected: Vec<_> = order
        .iter()
        .copied()
        .filter(|id| view.device_panel.is_selected(*id))
        .collect();
    if !selected.contains(&primary) || selected.len() != 1 + view.device_panel.selected_others.len()
    {
        view.error = Some(STALE.into());
        cx.notify();
        return;
    }
    let Ok(selected) = DeviceSelectionIds::new(&selected) else {
        view.error = Some(STALE.into());
        cx.notify();
        return;
    };
    let last = order.iter().rposition(|id| selected.ids().contains(id));
    // Clipboard copies are roots at the destination. Use the next gap
    // outside the enclosing serial unit so its existing subtree stays intact.
    let before = last.and_then(|index| {
        order[index + 1..].iter().copied().find(|id| {
            owned_device(&state.project, stamp.owner, *id)
                .is_some_and(|device| device.parent().is_none())
        })
    });
    let action = match duplicate_device_action(
        &state.project,
        stamp.owner,
        selected,
        project_owner(stamp.owner),
        before,
    ) {
        Ok(action) => action,
        Err(reason) => {
            view.error = Some(reason.into());
            cx.notify();
            return;
        }
    };
    view.device_panel.pending_duplicate = Some(PendingDuplicate {
        stamp,
        action: action.clone(),
        receipt: Receipt::new(stamp.generation),
        select_copies: true,
    });
    if view.enqueue(Request::DeviceClipboard {
        project_generation: stamp.generation,
        revision: stamp.revision,
        action,
    }) {
        // Keep keyboard ownership stable even when the new cards land outside
        // the old scroll window. A subsequent user focus change wins.
        view.device_panel
            .chain_window
            .selection_focus(cx)
            .focus(window);
    } else {
        view.device_panel.pending_duplicate = None;
    }
    cx.notify();
}

impl EditorView {
    pub(in crate::editor) fn retire_abandoned_device_duplicate(&mut self) -> bool {
        if self
            .device_panel
            .pending_duplicate
            .as_ref()
            .is_some_and(|p| p.receipt.is_queued())
            && !self
                .pending_requests
                .iter()
                .chain(self.device_gesture_barriers.iter())
                .any(|request| self.device_panel.pending_duplicate_matches(request))
        {
            return self.device_panel.pending_duplicate.take().is_some();
        }
        false
    }

    pub(in crate::editor) fn complete_device_duplicate(&mut self, event: &Event) {
        let Some(pending) = &mut self.device_panel.pending_duplicate else {
            return;
        };
        let disposition = pending.receipt.observe(
            self.project_generation,
            event.project_replaced,
            event.request_complete,
        );
        if disposition == Disposition::Waiting {
            return;
        }
        let Some(pending) = self.device_panel.pending_duplicate.take() else {
            return;
        };
        let stamp = pending.stamp;
        if disposition != Disposition::Terminal
            || !pending.select_copies
            || event.error.is_some()
            || stamp.revision.checked_add(1) != Some(event.state.revision)
            || self.device_panel.owner != Some(stamp.owner)
            || selected_owner(self, &event.state) != Some(stamp.owner)
            || !self.device_panel_active
            || !self.clip_view_open
            || self.opening_project
            || self.close_prompt_open
        {
            return;
        }
        let (Some(Created::Device(first)), DeviceClipboardAction::Duplicate { before, .. }) =
            (&event.created, pending.action)
        else {
            return;
        };
        // Paste creates one contiguous run. Its first birth and the retained
        // destination gap identify every inserted card, including descendants.
        let order = chain_ids(&event.state.project, stamp.owner);
        let Some(start) = order.iter().position(|id| id == first) else {
            return;
        };
        let Some(end) = before.map_or(Some(order.len()), |before| {
            order.iter().position(|id| *id == before)
        }) else {
            return;
        };
        let Some((primary, rest)) = order.get(start..end).and_then(<[DeviceId]>::split_first)
        else {
            return;
        };
        self.device_panel.select_only(Some(*primary));
        self.device_panel.selected_others.extend(rest);
        self.device_panel
            .chain_window
            .reveal_after_prepare(*primary);
    }
}

/// Select the neighboring device and move keyboard focus to its card.
/// Selection is view state; no parameter draft is finalized or dispatched.
fn step(view: &mut EditorView, forward: bool, window: &mut Window, cx: &mut Context<EditorView>) {
    let Some(owner) = view.device_panel.owner else {
        return;
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    if selected_owner(view, state) != Some(owner) {
        return;
    }
    let order = chain_ids(&state.project, owner);
    let Some(device) = view.device_panel.step_target(&order, forward) else {
        return;
    };
    view.device_panel.select_only(Some(device));
    view.device_panel.chain_window.reveal(device);
    view.device_panel
        .chain_window
        .focus(device, cx)
        .focus(window);
    cx.notify();
}

pub(super) fn rail(view: &mut EditorView, cx: &Context<EditorView>, rail: Div) -> Div {
    let focus = view.device_panel.chain_window.selection_focus(cx);
    let duplicate_focus = focus.clone();
    let painted = ChainStamp::current(view);
    rail.track_focus(&focus)
        .key_context("hex_device_selection")
        // Other actions, including Undo and transport, still reach the editor.
        .on_action(cx.trace(|_, _: &RemoveSelectedDevice, _, cx| cx.stop_propagation()))
        .on_action(cx.trace(|_, _: &RemoveSelectedDeviceX, _, cx| cx.stop_propagation()))
        .on_action(cx.trace(|view, _: &CopyDevices, window, cx| {
            if view
                .device_panel
                .chain_window
                .selection_focus(cx)
                .is_focused(window)
            {
                clipboard(view, ClipboardIntent::Copy, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(|view, _: &CutDevices, window, cx| {
            if view
                .device_panel
                .chain_window
                .selection_focus(cx)
                .is_focused(window)
            {
                clipboard(view, ClipboardIntent::Cut, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(|view, _: &PasteDevices, window, cx| {
            if view
                .device_panel
                .chain_window
                .selection_focus(cx)
                .is_focused(window)
            {
                clipboard(view, ClipboardIntent::Paste, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(move |view, _: &DuplicateDevices, window, cx| {
            if duplicate_focus.is_focused(window) {
                duplicate(view, painted, None, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(
            cx.trace(|view, _: &SelectPreviousDevice, window, cx| {
                rail_step(view, false, window, cx)
            }),
        )
        .on_action(
            cx.trace(|view, _: &SelectNextDevice, window, cx| rail_step(view, true, window, cx)),
        )
}

fn rail_step(
    view: &mut EditorView,
    forward: bool,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if view
        .device_panel
        .chain_window
        .selection_focus(cx)
        .is_focused(window)
    {
        step(view, forward, window, cx);
    } else {
        cx.propagate();
    }
}

/// Observe release even after focus moved from the neutral rail into text.
/// This is bookkeeping only; never consume the destination editor's key event.
pub(in crate::editor) fn key_up(
    view: &mut EditorView,
    event: &gpui::KeyUpEvent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let key = match event.keystroke.key.as_str() {
        "backspace" => DeleteKey::Backspace,
        "x" => DeleteKey::X,
        _ => return,
    };
    if !view
        .device_panel
        .chain_window
        .refused_delete
        .is_some_and(|intent| intent.key == key)
    {
        return;
    }
    let Some(intent) = view.device_panel.chain_window.refused_delete.take() else {
        return;
    };
    let live = intent.generation == view.project_generation
        && intent.owner == view.device_panel.owner
        && view.device_panel.selected_device == Some(intent.device)
        && view.clip_view_open
        && view.device_panel_active
        && intent.owner.is_some_and(|owner| {
            view.state
                .as_ref()
                .is_some_and(|state| removal(&state.project, owner, intent.device).is_some())
        });
    let focus = view.device_panel.chain_window.selection_focus(cx);
    // A new text/graph focus or changed project/selection always wins.
    if live && focus.is_focused(window) {
        view.device_panel
            .chain_window
            .focus(intent.device, cx)
            .focus(window);
    }
    cx.notify();
}

fn owned_device(
    project: &hex_project::Project,
    owner: DevicePanelOwner,
    device: DeviceId,
) -> Option<&hex_project::Device> {
    match owner {
        DevicePanelOwner::Main => project
            .main_devices()
            .iter()
            .find(|item| item.id() == device),
        DevicePanelOwner::Track(id) => project.track(id)?.ordered_device_chain().find_map(|item| {
            let item = match item {
                hex_project::TrackDeviceRef::Instrument(item)
                | hex_project::TrackDeviceRef::Effect(item) => item,
            };
            (item.id() == device).then_some(item)
        }),
        DevicePanelOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)?
            .devices()
            .iter()
            .find(|item| item.id() == device),
    }
}

fn removal(
    project: &hex_project::Project,
    owner: DevicePanelOwner,
    device: DeviceId,
) -> Option<Command> {
    owned_device(project, owner, device)?;
    if let DevicePanelOwner::Track(id) = owner
        && project
            .track(id)?
            .instrument()
            .is_some_and(|item| item.id() == device)
    {
        Some(Command::RemoveInstrument { track: id })
    } else {
        Some(Command::RemoveDevice(device))
    }
}

fn remove_selected_device(
    view: &mut EditorView,
    intent: RefusedDelete,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let RefusedDelete {
        generation,
        owner,
        device,
        ..
    } = intent;
    cx.stop_propagation();
    if !focus.is_focused(window) {
        return;
    }
    if generation != view.project_generation
        || owner != view.device_panel.owner
        || view.device_panel.selected_device != Some(device)
        || !view.clip_view_open
        || !view.device_panel_active
        || view.device_panel.chain_window.refused_delete.is_some()
    {
        suspend_until_release(view, intent, focus, window, cx);
        return;
    }
    if !view.device_panel.selected_others.is_empty() {
        view.error = Some("select one device to remove it".into());
        suspend_until_release(view, intent, focus, window, cx);
        cx.notify();
        return;
    }
    // Refuse unrelated work; an active numeric gesture may be finalized
    // through its existing terminal barrier before this exact removal.
    if view.opening_project
        || view.close_prompt_open
        || view.gain_recorder.blocks_edits()
        || !view.pending_requests.is_empty()
        || !view.device_gesture_barriers.is_empty()
        || view.worker_requests_pending > 1
        || (view.worker_requests_pending != 0 && view.device_gesture.snapshot().is_none())
    {
        view.error = Some("finish pending edits before removing the selected device".into());
        suspend_until_release(view, intent, focus, window, cx);
        cx.notify();
        return;
    }
    let command = owner.and_then(|owner| {
        view.state
            .as_ref()
            .and_then(|state| removal(&state.project, owner, device))
    });
    let Some(command) = command else {
        suspend_until_release(view, intent, focus, window, cx);
        return;
    };
    if finalize_controls(view, cx) {
        // Canonical reconciliation clears selection only after removal.
        // Pending work refuses repeat; no next device is selected.
        if view.enqueue_device_barrier(Request::Edit(command)) {
            view.device_panel.chain_window.refused_delete = None;
            view.device_panel
                .chain_window
                .selection_focus(cx)
                .focus(window);
        } else {
            suspend_until_release(view, intent, focus, window, cx);
        }
    } else {
        suspend_until_release(view, intent, focus, window, cx);
    }
    cx.notify();
}

pub(super) fn card(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    device: DeviceId,
    focus: FocusHandle,
    card: Stateful<Div>,
) -> Stateful<Div> {
    let generation = view.project_generation;
    let owner = view.device_panel.owner;
    let intent = RefusedDelete {
        key: DeleteKey::Backspace,
        generation,
        owner,
        device,
    };
    let x_focus = focus.clone();
    let toggle_focus = focus.clone();
    let type_id = view.state.as_ref().and_then(|state| {
        owned_device(&state.project, owner?, device).map(|item| item.state().type_id())
    });
    let copy_focus = focus.clone();
    let cut_focus = focus.clone();
    let paste_focus = focus.clone();
    let duplicate_focus = focus.clone();
    let painted = ChainStamp::current(view);
    let previous_focus = focus.clone();
    let next_focus = focus.clone();
    card.key_context("hex_device_selection")
        .on_action(cx.trace(move |view, _: &SelectPreviousDevice, window, cx| {
            if previous_focus.is_focused(window) {
                step(view, false, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(move |view, _: &SelectNextDevice, window, cx| {
            if next_focus.is_focused(window) {
                step(view, true, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(move |view, _: &CopyDevices, window, cx| {
            if copy_focus.is_focused(window) {
                clipboard(view, ClipboardIntent::Copy, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(move |view, _: &CutDevices, window, cx| {
            if cut_focus.is_focused(window) {
                clipboard(view, ClipboardIntent::Cut, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(move |view, _: &PasteDevices, window, cx| {
            if paste_focus.is_focused(window) {
                clipboard(view, ClipboardIntent::Paste, window, cx);
            } else {
                cx.propagate();
            }
        }))
        .on_action(cx.trace(move |view, _: &DuplicateDevices, window, cx| {
            if duplicate_focus.is_focused(window) {
                duplicate(view, painted, Some(device), window, cx);
            } else {
                cx.propagate();
            }
        }))
        // A character binding in the ancestor key context would swallow c/0 in
        // nested text editors. Handle only the card's own exact focus instead.
        .on_key_down(
            cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                if !super::super::activation::is_activation_key(event)
                    || !toggle_focus.is_focused(window)
                {
                    return;
                }
                cx.stop_propagation();
                if generation != view.project_generation
                    || owner != view.device_panel.owner
                    || view.device_panel.selected_device != Some(device)
                    || !view.clip_view_open
                    || !view.device_panel_active
                {
                    return;
                }
                if !view.device_panel.selected_others.is_empty() {
                    view.error = Some("select one device to toggle it".into());
                    cx.notify();
                    return;
                }
                if view.opening_project
                    || view.close_prompt_open
                    || view.gain_recorder.blocks_edits()
                    || ((!view.pending_requests.is_empty()
                        || !view.device_gesture_barriers.is_empty()
                        || view.worker_requests_pending > 1
                        || (view.worker_requests_pending != 0
                            && view.device_gesture.snapshot().is_none()))
                        && !view.only_bypass_requests_pending())
                {
                    view.error =
                        Some("finish pending edits before toggling the selected device".into());
                    cx.notify();
                    return;
                }
                let valid = view.state.as_ref().and_then(|state| {
                    let owner = owner?;
                    if selected_owner(view, state) != Some(owner) {
                        return None;
                    }
                    let item = owned_device(&state.project, owner, device)?;
                    (Some(item.state().type_id()) == type_id).then_some(())
                });
                if valid.is_some() && finalize_controls(view, cx) {
                    view.toggle_device_bypass(device);
                    cx.notify();
                }
            }),
        )
        .on_action(cx.trace(move |view, _: &RemoveSelectedDevice, window, cx| {
            remove_selected_device(view, intent, &focus, window, cx);
        }))
        .on_action(
            cx.trace(move |view, _: &RemoveSelectedDeviceX, window, cx| {
                remove_selected_device(
                    view,
                    RefusedDelete {
                        key: DeleteKey::X,
                        ..intent
                    },
                    &x_focus,
                    window,
                    cx,
                );
            }),
        )
}
