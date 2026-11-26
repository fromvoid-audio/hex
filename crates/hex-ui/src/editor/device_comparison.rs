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
    let target = super::device_action_owner::Target::capture(view, device, class)?;
    let entry = view.state.as_ref()?.project.device(device)?;
    Some(Intent {
        owner: target.owner,
        panel: target.panel,
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
    let label = match target.expected {
        Slot::A => "a",
        Slot::B => "b",
    };
    let tooltip = if target.owner.type_id == hex_lattice_contract::VST3_DEVICE_TYPE_ID {
        "click to switch · right-click to copy"
    } else {
        "click to switch · right-click to copy · , selects a · . selects b · opt-, copies a to b · opt-. copies b to a"
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

/// Source housings have no chain drag target to select and focus their device.
pub(super) fn source_title(
    device: DeviceId,
    label: impl AsRef<str>,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let selector = format!("device-title-{}", device.value());
    div()
        .id(gpui::ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .child(theme::text(label))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _, window, cx| {
                view.device_panel.select_only(Some(device));
                view.device_panel.focus_action_card(device, window, cx);
                cx.notify();
            }),
        )
}

/// Only the directly focused card consumes comparison shortcuts;
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
    if target.owner.type_id == hex_lattice_contract::VST3_DEVICE_TYPE_ID {
        return card;
    }
    card.on_key_down(
        cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
            if !focus.is_focused(window) {
                return;
            }
            let slot = match event.keystroke.key.as_str() {
                "," => Slot::A,
                "." => Slot::B,
                _ => return,
            };
            let modifiers = event.keystroke.modifiers;
            let action = if modifiers == gpui::Modifiers::alt() {
                Some(Action::CopyFrom(slot))
            } else if !modifiers.modified() {
                (slot != target.expected).then_some(Action::Switch)
            } else {
                return;
            };
            cx.stop_propagation();
            if event.is_held
                || view.device_panel.selected_device != Some(device)
                || !view.audio_detail.clip_view_open
                || !view.device_panel.active
            {
                return;
            }
            if let Some(action) = action {
                view.perform_device_comparison(Intent { action, ..target }, cx);
            }
        }),
    )
}

impl EditorView {
    fn comparison_ready(&mut self, target: Intent, cx: &mut Context<Self>) -> bool {
        if !current(self, &target) {
            self.error = Some("device comparison changed · retry from the current device".into());
            cx.notify();
            return false;
        }
        if self.persistence.opening_project
            || self.persistence.close_prompt_open
            || self.persistence.save_dialog_open
            || self.persistence.export_dialog_open
            || self.device_panel.preset_dialog_open
            || self.settings.theme.open
            || self.settings.tuning.open
            || (self.gain_recorder.blocks_edits() || self.midi_editor.recorder.blocks_edits())
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
            || !self.device_panel.gesture_barriers.is_empty()
            || self.device_panel.gesture.snapshot().is_some()
            || self.settings.tuning.client.has_pending_requests()
            || self.settings.tuning.client.snapshot().is_some()
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
