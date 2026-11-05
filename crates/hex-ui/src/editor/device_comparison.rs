//! Shared authored A/B controls and exact-card keyboard ownership.
use super::{EditorView, context_menu::EditorContextAction, device_panel};
use crate::context_menu::{MenuItem, PopupMenu};
use crate::theme;
use gpui::{
    Context, Div, FocusHandle, MouseButton, Pixels, Point, Stateful, Window, div, prelude::*,
};
use hex_engine::NativeDeviceClass;
use hex_project::{DeviceComparisonAction as Action, DeviceId, DeviceVariant as Slot};
use hex_session::service::{Request, preset::Owner};

#[derive(Clone, Copy)]
pub(super) struct Intent {
    owner: Owner,
    panel: device_panel::DevicePanelOwner,
    expected: Slot,
    action: Action,
}

fn intent(
    view: &EditorView,
    device: DeviceId,
    class: NativeDeviceClass,
    action: Action,
) -> Option<Intent> {
    let state = view.state.as_ref()?;
    let entry = state.project.device(device)?;
    let panel = view.device_panel.owner?;
    let actual_class = match panel {
        device_panel::DevicePanelOwner::Main => {
            if !state
                .project
                .main_devices()
                .iter()
                .any(|entry| entry.id() == device)
            {
                return None;
            }
            NativeDeviceClass::Effect
        }
        device_panel::DevicePanelOwner::Track(track) => {
            let track = state.project.track(track)?;
            if track.instrument().is_some_and(|entry| entry.id() == device) {
                NativeDeviceClass::Instrument
            } else if track.devices().iter().any(|entry| entry.id() == device) {
                NativeDeviceClass::Effect
            } else {
                return None;
            }
        }
        device_panel::DevicePanelOwner::Return(bus) => {
            let bus = state
                .project
                .return_tracks()
                .iter()
                .find(|entry| entry.id() == bus)?;
            if !bus.devices().iter().any(|entry| entry.id() == device) {
                return None;
            }
            NativeDeviceClass::Effect
        }
    };
    if actual_class != class {
        return None;
    }
    Some(Intent {
        owner: Owner {
            project_generation: view.project_generation,
            revision: state.revision,
            device,
            class,
            type_id: entry.state().type_id(),
        },
        panel,
        expected: entry.active_variant(),
        action,
    })
}

pub(super) fn current(view: &EditorView, target: &Intent) -> bool {
    intent(view, target.owner.device, target.owner.class, target.action).is_some_and(|now| {
        now.owner.project_generation == target.owner.project_generation
            && now.owner.revision == target.owner.revision
            && now.owner.type_id == target.owner.type_id
            && now.panel == target.panel
            && now.expected == target.expected
    })
}

pub(super) fn actions(
    view: &EditorView,
    device: &hex_project::Device,
    class: NativeDeviceClass,
    selected: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let Some(target) = intent(view, device.id(), class, Action::Switch) else {
        return div();
    };
    let (label, tooltip) = match target.expected {
        Slot::A => (
            "a",
            "settings a · click to switch to b · right-click to copy · p",
        ),
        Slot::B => (
            "b",
            "settings b · click to switch to a · right-click to copy · p",
        ),
    };
    div().child(
        super::device_panel::segment_button(
            format!("device-comparison-{}", device.id().value()),
            label,
            tooltip,
            theme::DeviceActionRole::Normal,
            selected,
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            view.perform_device_comparison(target, cx);
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                view.open_device_comparison_menu(target, event.position, window, cx);
            }),
        ),
    )
}

/// No global P binding. Only the directly focused card consumes this character;
/// text/numeric/graph descendants and the musical keyboard keep their own routes.
pub(super) fn card(
    view: &EditorView,
    cx: &Context<EditorView>,
    device: DeviceId,
    class: NativeDeviceClass,
    focus: FocusHandle,
    card: Stateful<Div>,
) -> Stateful<Div> {
    let Some(target) = intent(view, device, class, Action::Switch) else {
        return card;
    };
    card.on_key_down(
        cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key != "p"
                || event.keystroke.modifiers.modified()
                || !focus.is_focused(window)
            {
                return;
            }
            cx.stop_propagation();
            if event.is_held
                || view.device_panel.selected_device != Some(device)
                || !view.clip_view_open
                || !view.device_panel_active
            {
                return;
            }
            view.perform_device_comparison(target, cx);
        }),
    )
}

impl EditorView {
    /// Reconcile before the root's key context is composed. Dismissing during
    /// overlay construction would leave the painted tree in menu-key mode.
    pub(super) fn reconcile_device_comparison_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stale = self.context_menu.as_ref().is_some_and(|menu| menu.items.iter().any(|item| {
            matches!(&item.action, EditorContextAction::DeviceComparison(target) if !current(self, target))
        }));
        if stale {
            if let Some(menu) = &mut self.context_menu {
                menu.previous_focus = Some(self.focus.clone());
            }
            self.dismiss_context_menu(window, cx);
        }
    }
    fn comparison_ready(&mut self, target: Intent, cx: &mut Context<Self>) -> bool {
        if !current(self, &target) {
            self.error = Some("device comparison changed · retry from the current device".into());
            cx.notify();
            return false;
        }
        if self.opening_project
            || self.close_prompt_open
            || self.save_dialog_open
            || self.export_dialog_open
            || self.preset_dialog_open
            || self.theme_settings.open
            || self.tuning.open
            || (self.gain_recorder.blocks_edits() || self.midi_recorder.blocks_edits())
        {
            self.error = Some("finish the current action before comparing device settings".into());
            cx.notify();
            return false;
        }
        if !device_panel::finalize_controls(self, cx) {
            return false;
        }
        if self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
            || self.tuning.client.has_pending_requests()
            || self.tuning.client.snapshot().is_some()
        {
            self.error =
                Some("device edit is still applying · retry comparison when finished".into());
            cx.notify();
            return false;
        }
        true
    }

    pub(super) fn perform_device_comparison(&mut self, target: Intent, cx: &mut Context<Self>) {
        if self.comparison_ready(target, cx) {
            self.enqueue(Request::CompareDevice {
                owner: target.owner,
                expected: target.expected,
                action: target.action,
            });
            cx.notify();
        }
    }

    fn open_device_comparison_menu(
        &mut self,
        target: Intent,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.comparison_ready(target, cx) {
            return;
        }
        let card_focus = self
            .device_panel
            .comparison_return_focus(target.owner.device, window, cx);
        self.device_panel.select_only(Some(target.owner.device));
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|menu| menu.previous_focus)
            .or_else(|| window.focused(cx));
        let previous_focus = card_focus.or(previous_focus);
        let focus = cx.focus_handle();
        focus.focus(window);
        let labels = match target.expected {
            Slot::A => ["switch to b", "copy a to b"],
            Slot::B => ["switch to a", "copy b to a"],
        };
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            icon_grid: None,
            position,
            selected: 0,
            focus,
            previous_focus,
            items: [Action::Switch, Action::CopyToInactive]
                .into_iter()
                .zip(labels)
                .map(|(action, label)| MenuItem {
                    label,
                    swatch: None,
                    checked: false,
                    action: EditorContextAction::DeviceComparison(Intent { action, ..target }),
                })
                .collect(),
        });
        cx.notify();
    }
}
