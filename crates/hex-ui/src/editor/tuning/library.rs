//! Native library controls send bounded operations to the Session worker.
use super::super::browser_input;
use super::*;
use gpui::PathPromptOptions;
use hex_session::tuning_gesture::{TuningLibraryOperation as Operation, TuningLibraryResult};

pub(super) struct DeviceTerminalContext {
    operation_id: u64,
    key: hex_session::device_gesture::GestureKey,
    refresh: bool,
}
impl EditorView {
    pub(in crate::editor) fn bind_tuning_library_device_terminal(
        &mut self,
        key: hex_session::device_gesture::GestureKey,
        accepted: bool,
    ) {
        if !self
            .tuning
            .library_terminal
            .as_ref()
            .is_some_and(|context| context.key == key)
        {
            return;
        }
        let Some(context) = self.tuning.library_terminal.take() else {
            return;
        };
        if !accepted || !context.refresh || key.generation != self.project_generation {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        for request in &mut self.device_gesture_barriers {
            if let Request::TuningLibrary {
                project_generation,
                revision,
                operation_id,
                ..
            } = request
                && *operation_id == context.operation_id
                && *project_generation == key.generation
            {
                // Rebind exactly this queued current-context action, in place.
                // Previously opened dialogs retain their original context.
                *revision = state.revision;
                break;
            }
        }
    }
    pub(in crate::editor) fn retire_tuning_library_device_terminal(&mut self) {
        if let Some(context) = self.tuning.library_terminal.take()
            && self.tuning.library_pending == Some(context.operation_id)
        {
            self.tuning.library_pending = None;
        }
    }

    pub(super) fn request_tuning_library_list(&mut self) {
        self.request_tuning_library(Operation::List, None);
    }
    pub(super) fn request_tuning_library(
        &mut self,
        operation: Operation,
        owner: Option<(u64, u64)>,
    ) {
        if self.tuning.client.snapshot().is_some() {
            if self.tuning.deferred_library.is_some() {
                self.tuning.error = Some("a library action is already queued".into());
                return;
            }
            let _ = self.tuning.client.cancel();
            self.tuning.deferred_library = Some((operation, owner));
            return;
        }
        if self.tuning.library_pending.is_some() {
            self.tuning.error = Some("a tuning library operation is still running".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(id) = self.tuning.library_next.checked_add(1) else {
            self.tuning.error = Some("tuning operation identity exhausted".into());
            return;
        };
        let (generation, revision) = owner.unwrap_or((self.project_generation, state.revision));
        let device_terminal =
            self.device_gesture
                .snapshot()
                .map(|snapshot| DeviceTerminalContext {
                    operation_id: id,
                    key: snapshot.key,
                    refresh: owner.is_none(),
                });
        if self.enqueue(Request::TuningLibrary {
            project_generation: generation,
            revision,
            operation_id: id,
            operation,
        }) {
            self.tuning.library_next = id;
            self.tuning.library_pending = Some(id);
            self.tuning.library_terminal = device_terminal;
            self.tuning.error = None;
        } else {
            self.tuning.error = Some("project worker is busy · retry the library action".into());
        }
    }
    pub(in crate::editor) fn tuning_library_complete(&mut self, result: &TuningLibraryResult) {
        if self.tuning.library_pending != Some(result.operation_id) {
            return;
        }
        self.tuning.library_pending = None;
        self.tuning.library_terminal = None;
        self.tuning.slots = result
            .slots
            .iter()
            .map(|s| SlotView {
                number: s.number,
                name: s.name.clone(),
                error: s.error.clone(),
            })
            .collect();
        if let Some(slot) = result.selected_slot {
            self.tuning.slot = slot;
        }
        if let Some(error) = result.error.as_ref() {
            self.tuning.error = Some(error.clone());
        } else {
            self.tuning.error = None;
        }
    }
    fn tuning_slot_name(&self) -> String {
        self.tuning
            .slots
            .iter()
            .find(|s| s.number == self.tuning.slot)
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| format!("preset {:02}", self.tuning.slot))
    }
    fn begin_tuning_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.tuning_slot_name();
        let input = cx.new(|cx| browser_input::name(name, cx));
        input.read(cx).focus(window);
        self.tuning.name = Some(input);
        cx.notify();
    }
    pub(in crate::editor) fn cancel_tuning_name(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tuning.name = None;
        self.tuning.focus.focus(window);
        cx.notify();
    }
    pub(in crate::editor) fn commit_tuning_name(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.tuning.name.as_ref() else {
            return;
        };
        let name = input.read(cx).text().to_owned();
        if let Err(reason) = hex_tuning::preset::validate_name(&name) {
            self.tuning.error = Some(reason.to_string());
            cx.notify();
            return;
        }
        self.request_tuning_library(
            Operation::Rename {
                slot: self.tuning.slot,
                name,
            },
            None,
        );
        self.tuning.name = None;
        self.tuning.focus.focus(window);
        cx.notify();
    }
    pub(super) fn tuning_file_dialog(&mut self, save: bool, cx: &mut Context<Self>) {
        if self.device_gesture.snapshot().is_some() {
            if self.pending_requests.len() + self.device_gesture_barriers.len()
                >= super::super::UI_REQUEST_CAPACITY
            {
                self.tuning.error =
                    Some("device edit queue full · retry the library action".into());
                return;
            }
            if !self.device_gesture.has_pending_terminal()
                && let Err(error) = self.device_gesture.finish_current()
            {
                self.tuning.error = Some(error.into());
                return;
            }
            if self.tuning.client.snapshot().is_some() {
                let _ = self.tuning.client.cancel();
            }
            self.tuning.deferred_dialog = Some(save);
            return;
        }
        if self.tuning.client.snapshot().is_some() {
            let _ = self.tuning.client.cancel();
            self.tuning.deferred_dialog = Some(save);
            return;
        }
        if self.preset_dialog_open || self.tuning.library_pending.is_some() {
            self.tuning.error = Some("finish the current library operation first".into());
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let owner = (self.project_generation, state.revision);
        let name = self.tuning_slot_name();
        self.preset_dialog_open = true;
        if save {
            let directory = state
                .document_path
                .as_deref()
                .and_then(std::path::Path::parent)
                .unwrap_or(std::path::Path::new("."));
            let response = cx.prompt_for_new_path(directory, Some("scale.freqs"));
            cx.spawn(async move |this, cx| {
                let picked = response.await;
                let _ = this.update(cx, |view, cx| {
                    view.preset_dialog_open = false;
                    match picked {
                        Ok(Ok(Some(path))) => view
                            .request_tuning_library(Operation::Export { path, name }, Some(owner)),
                        Ok(Err(_)) => {
                            view.tuning.error = Some("tuning export dialog failed".into())
                        }
                        _ => {}
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            let response = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(theme::text("import freqs scale (.freqs)")),
            });
            cx.spawn(async move |this, cx| {
                let picked = response.await;
                let _ = this.update(cx, |view, cx| {
                    view.preset_dialog_open = false;
                    match picked {
                        Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                            if let Some(path) = paths.pop() {
                                view.request_tuning_library(
                                    Operation::Import { path },
                                    Some(owner),
                                );
                            }
                        }
                        Ok(Err(_)) => {
                            view.tuning.error = Some("tuning import dialog failed".into())
                        }
                        _ => {}
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }
    pub(super) fn tuning_library_panel(&self, cx: &mut Context<Self>) -> Div {
        let mut panel = tuning_section("local presets · eight slots");
        let mut tuning_slots = div().flex().gap_1();
        for number in 1..=8 {
            tuning_slots = tuning_slots.child(
                button(format!("{number}"), self.tuning.slot == number).on_click(cx.listener(
                    move |view, _, _, cx| {
                        view.tuning.slot = number;
                        view.tuning.name = None;
                        cx.notify();
                    },
                )),
            );
        }
        panel = panel.child(tuning_slots);
        if let Some(tuning_name_input) = self.tuning.name.as_ref() {
            panel = panel.child(
                div()
                    .id("tuning-preset-name")
                    .key_context("hex_tuning_name hex_text_editing")
                    .child(tuning_name_input.clone()),
            );
        } else {
            panel = panel.child(theme::text(self.tuning_slot_name()));
        }
        if let Some(error) = self
            .tuning
            .slots
            .iter()
            .find(|s| s.number == self.tuning.slot)
            .and_then(|s| s.error.as_ref())
        {
            panel = panel.child(theme::text(error.clone()));
        }
        panel = panel.child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(
                    button("load", false).on_click(cx.listener(|view, _, _, cx| {
                        view.request_tuning_library(
                            Operation::Load {
                                slot: view.tuning.slot,
                            },
                            None,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    button("save", false).on_click(cx.listener(|view, _, _, cx| {
                        view.request_tuning_library(
                            Operation::Save {
                                slot: view.tuning.slot,
                                name: view.tuning_slot_name(),
                            },
                            None,
                        );
                        cx.notify();
                    })),
                )
                .child(button("rename", false).on_click(
                    cx.listener(|view, _, window, cx| view.begin_tuning_name(window, cx)),
                ))
                .child(
                    button("duplicate", false).on_click(cx.listener(|view, _, _, cx| {
                        view.request_tuning_library(
                            Operation::Duplicate {
                                slot: view.tuning.slot,
                            },
                            None,
                        );
                        cx.notify();
                    })),
                )
                .child(
                    button("import", false)
                        .on_click(cx.listener(|view, _, _, cx| view.tuning_file_dialog(false, cx))),
                )
                .child(
                    button("export", false)
                        .on_click(cx.listener(|view, _, _, cx| view.tuning_file_dialog(true, cx))),
                ),
        );
        if self.tuning.library_pending.is_some() {
            panel = panel.child(theme::text("working…"));
        }
        panel
    }
}
