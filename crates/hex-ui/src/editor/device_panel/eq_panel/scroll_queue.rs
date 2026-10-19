//! One bounded FIFO of physical node streams, never a second worker gesture.
use super::*;
use hex_session::device_gesture::{GestureAck, GestureKey, GesturePhase};
use hex_widgets::{ResponseGraphScrollEvent as Event, ResponseGraphScrollPhase as Phase};
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Queue {
    waiting: Option<Retiring>,
    streams: VecDeque<Stream>,
    latest_input: Option<(u64, u32)>,
}

struct Retiring {
    key: GestureKey,
    owner: Option<DevicePanelOwner>,
    variant: hex_project::DeviceVariant,
    before: NativeDeviceState,
    expected: NativeDeviceState,
    revision: u64,
    expected_revision: u64,
}

struct Stream {
    id: u64,
    node: u32,
    steps: i32,
    // Compose the four possible initial choices. This retains saturation and
    // immediate reversal without storing one event per wheel delta.
    choices: [u32; 4],
    ended: bool,
}

pub(super) fn cancel_queued(view: &mut EditorView, device: DeviceId) {
    if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
        panel.scroll_queue.streams.clear();
        // The graph's stream counter survives cancellation. Keep its watermark
        // so delayed old Begin/Cancel deliveries cannot restore revoked intent.
    }
}

pub(super) fn intercept(view: &mut EditorView, device: DeviceId, event: Event) -> bool {
    if event.phase == Phase::Cancel {
        let queued = view.device_panel.eq_panels.get(&device).is_some_and(|p| {
            p.scroll_queue
                .streams
                .iter()
                .any(|s| s.id == event.gesture_id && s.node == event.node_id)
        });
        let owns_context = view.device_panel.eq_panels.get(&device).is_some_and(|p| {
            p.scroll_queue.latest_input == Some((event.gesture_id, event.node_id))
        });
        if owns_context {
            cancel_queued(view, device);
        }
        return owns_context && queued;
    }
    let Some(panel) = view.device_panel.eq_panels.get_mut(&device) else {
        return false;
    };
    let queue = &mut panel.scroll_queue;
    if event.phase == Phase::Begin && queue.waiting.is_some() {
        if queue
            .latest_input
            .is_some_and(|(id, _)| event.gesture_id <= id)
        {
            return true;
        }
        // Retain refused-stream identity too: its real Escape/Cancel still
        // cancels this queued context, while an older token cannot clear it.
        queue.latest_input = Some((event.gesture_id, event.node_id));
        if queue.streams.len() >= super::super::super::UI_REQUEST_CAPACITY
            || queue.streams.try_reserve(1).is_err()
        {
            view.device_panel.message =
                Some("too many pending node edits; wait, then retry".into());
            return true;
        }
        queue.streams.push_back(Stream {
            id: event.gesture_id,
            node: event.node_id,
            steps: 0,
            choices: [0, 1, 2, 3],
            ended: false,
        });
        return true;
    }
    let Some(stream) = queue
        .streams
        .back_mut()
        .filter(|s| s.id == event.gesture_id && s.node == event.node_id && !s.ended)
    else {
        return false;
    };
    let delta = event.steps.saturating_sub(stream.steps);
    for choice in &mut stream.choices {
        *choice = (i64::from(*choice) + i64::from(delta)).clamp(0, 3) as u32;
    }
    stream.steps = event.steps;
    stream.ended = event.phase == Phase::Commit;
    true
}

pub(super) fn retiring(view: &mut EditorView, device: DeviceId, scroll: gestures::SlopeScroll) {
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let Some(entry) = state.project.device(device) else {
        return;
    };
    let before = entry.state().clone();
    let Ok(expected) = hex_engine::patch_native_device_parameter(
        &before,
        scroll.parameter,
        ParameterValue::Choice(scroll.value),
    ) else {
        return;
    };
    // Match the existing Service revision law, including its saturated limit.
    let expected_revision = state.revision.saturating_add(u64::from(expected != before));
    let waiting = Retiring {
        key: scroll.key,
        owner: view.device_panel.owner,
        variant: entry.active_variant(),
        before,
        expected,
        revision: state.revision,
        expected_revision,
    };
    if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
        panel.scroll_queue.waiting = Some(waiting);
    }
}

/// Called once after canonical installation and the existing generic ACK match.
/// Only an accepted, matching predecessor Commit can admit the next stream.
pub(in crate::editor) fn after_worker_event(
    view: &mut EditorView,
    matched: Option<&GestureAck>,
    project_replaced: bool,
) {
    // UI event path only, bounded by the actual existing panel inventory.
    let devices: Vec<_> = view
        .device_panel
        .eq_panels
        .iter()
        .filter(|(_, p)| p.scroll_queue.waiting.is_some())
        .map(|(id, _)| *id)
        .collect();
    for device in devices {
        let Some(waiting) = view
            .device_panel
            .eq_panels
            .get_mut(&device)
            .and_then(|p| p.scroll_queue.waiting.take())
        else {
            continue;
        };
        let context_valid = !project_replaced
            && waiting.key.generation == view.project_generation
            && waiting.owner == view.device_panel.owner
            && view.state.as_ref().is_some_and(|state| {
                state
                    .project
                    .device(device)
                    .is_some_and(|entry| entry.active_variant() == waiting.variant)
            });
        let terminal = matched.filter(|ack| {
            ack.key == waiting.key
                && matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel)
        });
        let canonical_valid = view.state.as_ref().is_some_and(|state| {
            let Some(entry) = state.project.device(device) else {
                return false;
            };
            if terminal.is_some() {
                state.revision == waiting.expected_revision && entry.state() == &waiting.expected
            } else {
                state.revision == waiting.revision && entry.state() == &waiting.before
            }
        });
        if !context_valid
            || !canonical_valid
            || terminal.is_some_and(|ack| !ack.accepted || ack.phase != GesturePhase::Commit)
            || (terminal.is_none()
                && !view
                    .device_gesture
                    .snapshot()
                    .is_some_and(|s| s.key == waiting.key))
        {
            cancel_queued(view, device);
            continue;
        }
        if terminal.is_none() {
            if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
                panel.scroll_queue.waiting = Some(waiting);
            }
            continue;
        }
        if view.worker_requests_pending != 0
            || !view.pending_requests.is_empty()
            || !view.device_gesture_barriers.is_empty()
            || view.device_gesture.snapshot().is_some()
        {
            cancel_queued(view, device);
            view.device_panel.message =
                Some("node edit context changed; retry the pending edit".into());
            continue;
        }
        let Some(stream) = view
            .device_panel
            .eq_panels
            .get_mut(&device)
            .and_then(|p| p.scroll_queue.streams.pop_front())
        else {
            continue;
        };
        // Canonical data was installed before this notification. Synchronize the
        // private authored mirror, never a speculative snapshot or old preview.
        if let Some(panel) = view.device_panel.eq_panels.get_mut(&device) {
            panel.authored_state = waiting.expected;
        }
        let generation = view.project_generation;
        let event = Event {
            phase: Phase::Begin,
            node_id: stream.node,
            gesture_id: stream.id,
            steps: 0,
        };
        gestures::scroll_event_core(view, device, generation, event);
        let Some(scroll) = view
            .device_panel
            .eq_panels
            .get(&device)
            .and_then(|p| p.slope_scroll)
            .filter(|s| s.stream == stream.id)
        else {
            cancel_queued(view, device);
            continue;
        };
        let Some(value) = stream.choices.get(scroll.value as usize).copied() else {
            cancel_queued(view, device);
            let _ = view.cancel_device_gesture();
            continue;
        };
        gestures::scroll_event_core(
            view,
            device,
            generation,
            Event {
                phase: Phase::Change,
                steps: value as i32 - scroll.value as i32,
                ..event
            },
        );
        if let Some(scroll) = view
            .device_panel
            .eq_panels
            .get_mut(&device)
            .and_then(|p| p.slope_scroll.as_mut())
        {
            scroll.last_steps = stream.steps;
        }
        if stream.ended {
            gestures::scroll_event_core(
                view,
                device,
                generation,
                Event {
                    phase: Phase::Commit,
                    steps: stream.steps,
                    ..event
                },
            );
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
pub(super) fn counts(view: &EditorView, device: DeviceId) -> (usize, bool) {
    let queue = &view.device_panel.eq_panels[&device].scroll_queue;
    (queue.streams.len(), queue.waiting.is_some())
}
