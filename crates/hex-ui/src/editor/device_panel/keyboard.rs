//! Device-selection keys own only the directly focused card, never its editors.
use super::*;
use gpui::{FocusHandle, KeyBinding};

gpui::actions!(hex_device_selection, [RemoveSelectedDevice]);

pub(in crate::editor) fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "backspace",
        RemoveSelectedDevice,
        Some("hex_device_selection"),
    )]);
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
    card.key_context("hex_device_selection")
        .on_action(
            cx.listener(move |view, _: &RemoveSelectedDevice, window, cx| {
                cx.stop_propagation();
                if !focus.is_focused(window)
                    || generation != view.project_generation
                    || owner != view.device_panel.owner
                    || view.device_panel.selected_device != Some(device)
                    || !view.clip_view_open
                    || !view.device_panel_active
                {
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
                    cx.notify();
                    return;
                }
                let command = owner.and_then(|owner| {
                    view.state
                        .as_ref()
                        .and_then(|state| removal(&state.project, owner, device))
                });
                let Some(command) = command else { return };
                if finalize_controls(view, cx) {
                    // Canonical reconciliation clears selection only after removal.
                    // Pending work refuses repeat; no next device is selected.
                    view.enqueue_device_barrier(Request::Edit(command));
                }
                cx.notify();
            }),
        )
}
