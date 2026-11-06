//! Native dialogs only collect paths; preset file I/O runs on the control worker.
use super::EditorView;
use crate::theme;
use gpui::{
    Context, Div, ParentElement, PathPromptOptions, StatefulInteractiveElement, Styled, div, px,
};
use hex_engine::NativeDeviceClass;
use hex_project::DeviceId;
use hex_session::service::{Request, preset::Owner};
use std::path::{Path, PathBuf};

pub(super) fn actions(
    device: DeviceId,
    class: NativeDeviceClass,
    selected: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let mut actions = div().flex().flex_col().items_center().gap(px(2.0));
    for (save, icon, label) in [
        (false, "↓", "load device preset (.hex-device-preset.json)"),
        (true, "↑", "save device preset"),
    ] {
        actions = actions.child(
            super::device_panel::action_button(
                format!(
                    "device-preset-{}-{}",
                    if save { "save" } else { "load" },
                    device.value()
                ),
                icon,
                label,
                theme::DeviceActionRole::Normal,
                selected,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                view.device_preset_dialog(device, class, save, cx);
                cx.notify();
            })),
        );
    }
    actions
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
        let state = self.state.as_ref()?;
        Some(Owner {
            project_generation: self.project_generation,
            revision: state.revision,
            device,
            class,
            type_id: state.project.device(device)?.state().type_id(),
        })
    }

    fn device_preset_dialog(
        &mut self,
        device: DeviceId,
        class: NativeDeviceClass,
        save: bool,
        cx: &mut Context<Self>,
    ) {
        if self.preset_dialog_open || self.opening_project || self.close_prompt_open {
            self.error = Some("finish the current dialog before opening a device preset".into());
            return;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.worker_requests_pending != 0
            || !self.pending_requests.is_empty()
            || !self.device_gesture_barriers.is_empty()
            || self.device_gesture.snapshot().is_some()
        {
            self.error = Some(
                "device edit is still applying · open the preset dialog again when finished".into(),
            );
            return;
        }
        let Some(owner) = self.preset_owner(device, class) else {
            self.error = Some("device preset target no longer exists".into());
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
