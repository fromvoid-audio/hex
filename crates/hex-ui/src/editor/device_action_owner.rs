//! Captured device-action ownership shared by native preset and A/B controls.
//! Membership, type and revision are checked against the visible panel; this
//! module owns no DSP state or filesystem work.
use super::{EditorView, context_menu::EditorContextAction, device_panel};
use gpui::{Context, Window};
use hex_engine::NativeDeviceClass;
use hex_project::DeviceId;
use hex_session::service::preset::Owner;

#[derive(Clone, Copy)]
pub(super) struct Target {
    pub(super) owner: Owner,
    pub(super) panel: device_panel::DevicePanelOwner,
}
impl Target {
    pub(super) fn capture(
        view: &EditorView,
        device: DeviceId,
        class: NativeDeviceClass,
    ) -> Option<Self> {
        if !view.clip_view_open || !view.device_panel_active {
            return None;
        }
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
        Some(Self {
            owner: Owner {
                project_generation: view.project_generation,
                revision: state.revision,
                device,
                class,
                type_id: entry.state().type_id(),
            },
            panel,
        })
    }
    pub(super) fn current(self, view: &EditorView) -> bool {
        Self::capture(view, self.owner.device, self.owner.class).is_some_and(|now| {
            now.owner.project_generation == self.owner.project_generation
                && now.owner.revision == self.owner.revision
                && now.owner.type_id == self.owner.type_id
                && now.panel == self.panel
        })
    }
}

impl EditorView {
    /// Reconcile before composing menu-key context, including hidden owners.
    pub(super) fn reconcile_device_action_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stale = self.context_menu.as_ref().is_some_and(|menu| {
            menu.items.iter().any(|item| match &item.action {
                EditorContextAction::DeviceComparison(target) => {
                    !super::device_comparison::current(self, target)
                }
                EditorContextAction::DevicePreset(target) => {
                    !super::device_preset::current(self, target)
                }
                _ => false,
            })
        });
        if stale {
            if let Some(menu) = &mut self.context_menu {
                menu.previous_focus = Some(self.focus.clone());
            }
            self.dismiss_context_menu(window, cx);
        }
    }
}
