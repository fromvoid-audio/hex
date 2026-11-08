//! Native dialogs only collect paths; preset file I/O runs on the control worker.
use super::{EditorView, context_menu::EditorContextAction, device_action_owner::Target};
use crate::context_menu::{MenuItem, PopupMenu};
use crate::theme;
use gpui::{
    Context, Div, MouseButton, PathPromptOptions, Pixels, Point, Window, div, prelude::*, px,
};
use hex_engine::NativeDeviceClass;
use hex_project::DeviceId;
use hex_session::service::{Request, preset::Owner};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(super) enum Action {
    Load,
    Save,
    SaveDefault,
}
#[derive(Clone, Copy)]
pub(super) struct Intent {
    target: Target,
    action: Action,
}
pub(super) fn current(view: &EditorView, intent: &Intent) -> bool {
    intent.target.current(view)
}

pub(super) fn actions(
    view: &EditorView,
    device: DeviceId,
    class: NativeDeviceClass,
    selected: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let Some(target) = Target::capture(view, device, class) else {
        return div();
    };
    let content = super::device_panel::action_button(
        format!("device-preset-menu-{}", device.value()),
        "▲",
        "device presets and defaults",
        theme::DeviceActionRole::Normal,
        selected,
    )
    .on_mouse_down(
        MouseButton::Left,
        cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
            cx.stop_propagation();
            if target.current(view) {
                view.device_panel.focus_action_card(device, window, cx);
            }
            view.open_device_preset_menu(target, event.position, window, cx);
            cx.notify();
        }),
    );
    div().flex().flex_col().items_center().child(content)
}

/// The directly focused card opens its menu with Shift-F10. Descendant numeric,
/// text and graph input retains its keys. Measurements belong to this rendered
/// frame, not a second persistent device/focus registry.
pub(super) fn card(
    view: &EditorView,
    cx: &Context<EditorView>,
    device: DeviceId,
    class: NativeDeviceClass,
    focus: gpui::FocusHandle,
    card: gpui::Stateful<Div>,
) -> gpui::Stateful<Div> {
    let Some(target) = Target::capture(view, device, class) else {
        return card;
    };
    let bounds = std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::<Pixels>::default()));
    let measured = bounds.clone();
    let content = gpui::canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| ())
        .absolute()
        .size_full();
    card.relative().child(content).on_key_down(cx.listener(
        move |view, event: &gpui::KeyDownEvent, window, cx| {
            let modifiers = event.keystroke.modifiers;
            if event.keystroke.key == "f10"
                && modifiers.shift
                && !modifiers.control
                && !modifiers.alt
                && !modifiers.platform
                && focus.is_focused(window)
            {
                cx.stop_propagation();
                if !event.is_held && view.device_panel.selected_device == Some(device) {
                    let bounds = bounds.get();
                    view.open_device_preset_menu(
                        target,
                        gpui::point(
                            bounds.left() + px(hex_widgets::DEVICE_FRAME_HEADER_WIDTH),
                            bounds.top() + px(18.0),
                        ),
                        window,
                        cx,
                    );
                    cx.notify();
                }
            }
        },
    ))
}

fn save_path(mut path: PathBuf) -> Result<PathBuf, &'static str> {
    if !path
        .as_os_str()
        .to_string_lossy()
        .ends_with(".hex-device-preset.json")
    {
        let mut name = path
            .file_name()
            .ok_or("device preset target needs a filename")?
            .to_os_string();
        name.push(".hex-device-preset.json");
        path.set_file_name(name);
    }
    if path.as_os_str().len() > 4096 {
        return Err("device preset path exceeds the 4096-byte limit");
    }
    Ok(path)
}

impl EditorView {
    fn preset_owner(&self, device: DeviceId, class: NativeDeviceClass) -> Option<Owner> {
        Target::capture(self, device, class).map(|target| target.owner)
    }

    fn device_preset_ready(
        &mut self,
        device: DeviceId,
        class: NativeDeviceClass,
        cx: &mut Context<Self>,
    ) -> Option<Owner> {
        if self.preset_dialog_open || self.opening_project || self.close_prompt_open {
            self.error = Some("finish the current dialog before opening a device preset".into());
            return None;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            return None;
        }
        if self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
        {
            self.error = Some(
                "device edit is still applying · open the preset dialog again when finished".into(),
            );
            return None;
        }
        let owner = self.preset_owner(device, class);
        if owner.is_none() {
            self.error = Some("device preset target no longer exists".into());
        }
        owner
    }

    fn open_device_preset_menu(
        &mut self,
        target: Target,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !target.current(self)
            || self
                .device_preset_ready(target.owner.device, target.owner.class, cx)
                .is_none()
        {
            return;
        }
        let previous_focus = self
            .device_panel
            .comparison_return_focus(target.owner.device, window, cx)
            .or_else(|| window.focused(cx));
        self.device_panel.select_only(Some(target.owner.device));
        let focus = cx.focus_handle();
        focus.focus(window);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected: 0,
            focus,
            previous_focus,
            items: [
                (Action::Load, "load preset"),
                (Action::Save, "save preset"),
                (Action::SaveDefault, "save as default"),
            ]
            .into_iter()
            .map(|(action, label)| MenuItem {
                label,
                swatch: None,
                checked: false,
                action: EditorContextAction::DevicePreset(Intent { target, action }),
            })
            .collect(),
        });
        cx.notify();
    }

    pub(super) fn perform_device_preset_action(&mut self, intent: Intent, cx: &mut Context<Self>) {
        if !current(self, &intent) {
            self.error =
                Some("device preset target changed · retry from the current device".into());
            cx.notify();
            return;
        }
        let owner = intent.target.owner;
        match intent.action {
            Action::Load => self.device_preset_dialog(owner.device, owner.class, false, cx),
            Action::Save => self.device_preset_dialog(owner.device, owner.class, true, cx),
            Action::SaveDefault => {
                if let Some(owner) = self.device_preset_ready(owner.device, owner.class, cx) {
                    if let Some(root) = self.browser.data_directory().map(Path::to_path_buf) {
                        if self.enqueue_device_barrier(Request::SaveDeviceDefault { owner, root }) {
                            self.error = None;
                            self.status = "saving device default".into();
                        }
                    } else {
                        self.error = Some("device default data folder is unavailable".into());
                    }
                }
            }
        }
        cx.notify();
    }

    fn device_preset_dialog(
        &mut self,
        device: DeviceId,
        class: NativeDeviceClass,
        save: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.device_preset_ready(device, class, cx) else {
            return;
        };
        self.preset_dialog_open = true;
        if save {
            let directory = self
                .state
                .as_ref()
                .and_then(|state| state.document_path.as_deref())
                .and_then(Path::parent)
                .unwrap_or(Path::new("."));
            let response =
                cx.prompt_for_new_path(directory, Some("untitled.hex-device-preset.json"));
            cx.spawn(async move |this, cx| {
                let picked = response.await;
                let _ = this.update(cx, |view, cx| {
                    view.preset_dialog_open = false;
                    match picked {
                        Ok(Ok(Some(path))) => view.choose_device_preset(owner, path, true, cx),
                        Ok(Err(_)) => {
                            view.error = Some("device preset dialog failed · try again".into())
                        }
                        _ => {}
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            // GPUI's current PathPromptOptions has no extension-filter field.
            // The descriptive prompt names the format; worker validation is strict.
            let response = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("load device preset (.hex-device-preset.json)".into()),
            });
            cx.spawn(async move |this, cx| {
                let picked = response.await;
                let _ = this.update(cx, |view, cx| {
                    view.preset_dialog_open = false;
                    match picked {
                        Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                            if let Some(path) = paths.pop() {
                                view.choose_device_preset(owner, path, false, cx);
                            }
                        }
                        Ok(Err(_)) => {
                            view.error = Some("device preset dialog failed · try again".into())
                        }
                        _ => {}
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(super) fn choose_device_preset(
        &mut self,
        owner: Owner,
        path: PathBuf,
        save: bool,
        cx: &mut Context<Self>,
    ) {
        let current = self.preset_owner(owner.device, owner.class);
        if !current.is_some_and(|current| {
            current.project_generation == owner.project_generation
                && current.revision == owner.revision
                && current.type_id == owner.type_id
        }) {
            self.error = Some(
                "device preset canceled · project changed; open the preset dialog again".into(),
            );
            return;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        let path = if save {
            save_path(path)
        } else if path.as_os_str().len() > 4096 {
            Err("device preset path exceeds the 4096-byte limit")
        } else {
            Ok(path)
        };
        match path {
            Ok(path) => {
                let request = if save {
                    Request::SaveDevicePreset { owner, path }
                } else {
                    Request::LoadDevicePreset { owner, path }
                };
                if self.enqueue_device_barrier(request) {
                    self.error = None;
                    self.status = if save {
                        "saving device preset"
                    } else {
                        "loading device preset"
                    }
                    .into();
                }
            }
            Err(reason) => self.error = Some(reason.into()),
        }
    }
}
