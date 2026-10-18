//! Device-selection keys own only the directly focused card, never its editors.
use super::*;
use gpui::{FocusHandle, KeyBinding};

gpui::actions!(hex_device_selection, [RemoveSelectedDevice]);

#[derive(Clone, Copy)]
pub(super) struct RefusedDelete {
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
    cx.bind_keys([KeyBinding::new(
        "backspace",
        RemoveSelectedDevice,
        Some("hex_device_selection"),
    )]);
}

pub(super) fn rail(view: &mut EditorView, cx: &Context<EditorView>, rail: Div) -> Div {
    let focus = view.device_panel.chain_window.selection_focus(cx);
    rail.track_focus(&focus)
        .key_context("hex_device_selection")
        // Other actions, including Undo and transport, still reach the editor.
        .on_action(cx.listener(|_, _: &RemoveSelectedDevice, _, cx| cx.stop_propagation()))
}

/// Observe release even after focus moved from the neutral rail into text.
/// This is bookkeeping only; never consume the destination editor's key event.
pub(in crate::editor) fn key_up(
    view: &mut EditorView,
    event: &gpui::KeyUpEvent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if event.keystroke.key != "backspace" {
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

fn removal(
    project: &hex_project::Project,
    owner: DevicePanelOwner,
    device: DeviceId,
) -> Option<Command> {
    match owner {
        DevicePanelOwner::Track(id) => {
            let track = project.track(id)?;
            if track
                .instrument()
                .is_some_and(|instrument| instrument.id() == device)
            {
                Some(Command::RemoveInstrument { track: id })
            } else {
                track
                    .devices()
                    .iter()
                    .any(|effect| effect.id() == device)
                    .then_some(Command::RemoveDevice(device))
            }
        }
        DevicePanelOwner::Return(id) => project
            .return_tracks()
            .iter()
            .find(|track| track.id() == id)?
            .devices()
            .iter()
            .any(|effect| effect.id() == device)
            .then_some(Command::RemoveDevice(device)),
    }
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
        generation,
        owner,
        device,
    };
    card.key_context("hex_device_selection")
        .on_action(
            cx.listener(move |view, _: &RemoveSelectedDevice, window, cx| {
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
                    suspend_until_release(view, intent, &focus, window, cx);
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
                    || (view.worker_requests_pending != 0
                        && view.device_gesture.snapshot().is_none())
                {
                    view.error =
                        Some("finish pending edits before removing the selected device".into());
                    suspend_until_release(view, intent, &focus, window, cx);
                    cx.notify();
                    return;
                }
                let command = owner.and_then(|owner| {
                    view.state
                        .as_ref()
                        .and_then(|state| removal(&state.project, owner, device))
                });
                let Some(command) = command else {
                    suspend_until_release(view, intent, &focus, window, cx);
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
                        suspend_until_release(view, intent, &focus, window, cx);
                    }
                } else {
                    suspend_until_release(view, intent, &focus, window, cx);
                }
                cx.notify();
            }),
        )
}
