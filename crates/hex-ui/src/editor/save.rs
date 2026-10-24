use super::EditorView;
use crate::theme;
use gpui::{AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::service::Request;
use std::path::{Path, PathBuf};

const MAX_PROJECT_PATH_BYTES: usize = 4_096;

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests;

/// A format overwrite decision belongs to one selected document. It grants no
/// permission to replace a different project or bypass the worker's file token.
pub(super) struct FormatUpgradeIntent {
    generation: u64,
    path: PathBuf,
}

fn upgraded_project_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("untitled");
    format!("{stem}-upgraded.hex")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveAdmission {
    Current,
    ChoosePath,
    Reject(&'static str),
}

fn save_admission(
    has_document: bool,
    saving: bool,
    dialog_open: bool,
    editor_blocked: bool,
) -> SaveAdmission {
    if editor_blocked {
        SaveAdmission::Reject("finish the current dialog before saving")
    } else if saving {
        SaveAdmission::Reject("save already pending")
    } else if dialog_open {
        SaveAdmission::Reject("save dialog already open")
    } else if has_document {
        SaveAdmission::Current
    } else {
        SaveAdmission::ChoosePath
    }
}

fn prepare_save_path(mut path: PathBuf) -> Result<PathBuf, &'static str> {
    if path.extension().is_none() {
        path.set_extension("hex");
    }
    if path.as_os_str().len() > MAX_PROJECT_PATH_BYTES {
        return Err("project path exceeds the 4096-byte limit");
    }
    Ok(path)
}

impl EditorView {
    pub(super) fn save_project(&mut self, cx: &mut Context<Self>) {
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.state.is_none() {
            self.error = Some("project is still loading".into());
            return;
        }
        match self.save_admission() {
            SaveAdmission::Current if self.document_format_upgrade_required => {
                if let Some(path) = self
                    .state
                    .as_ref()
                    .and_then(|state| state.document_path.clone())
                {
                    self.save_dialog_open = true;
                    self.format_upgrade_prompt = Some(FormatUpgradeIntent {
                        generation: self.project_generation,
                        path,
                    });
                    cx.notify();
                }
            }
            SaveAdmission::Current => {
                if self.enqueue_device_barrier(Request::SaveCurrent) {
                    self.saving_project = true;
                    self.status = "saving project".into();
                    self.error = None;
                }
            }
            SaveAdmission::ChoosePath => self.save_as_dialog(cx),
            SaveAdmission::Reject(message) => self.error = Some(message.into()),
        }
    }

    pub(super) fn reconcile_format_upgrade_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(intent) = self.format_upgrade_prompt.take() else {
            return;
        };
        cx.defer_in(window, move |view, window, cx| {
            view.show_format_upgrade_prompt(intent, window, cx);
        });
    }

    fn show_format_upgrade_prompt(
        &mut self,
        intent: FormatUpgradeIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_generation != intent.generation
            || self
                .state
                .as_ref()
                .and_then(|state| state.document_path.as_ref())
                != Some(&intent.path)
        {
            self.save_dialog_open = false;
            self.cancel_open_after_save();
            self.error = Some("project changed · format upgrade cancelled".into());
            cx.notify();
            return;
        }
        let title = theme::text("save this project in the newer format?");
        let detail = theme::text(
            "overwriting upgrades the saved format. older hex versions that cannot read the newer format will no longer be able to open this file. save as a new or versioned name to preserve the original.",
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &[
                PromptButton::new(theme::text("save as…")),
                PromptButton::cancel(theme::text("cancel")),
                PromptButton::new(theme::text("overwrite older format")),
            ],
            cx,
        );
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let answer = answer.await.ok();
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.save_dialog_open = false;
                    if window.focused(cx).is_none() {
                        view.focus.focus(window);
                    }
                    let current = view.project_generation == intent.generation
                        && view
                            .state
                            .as_ref()
                            .and_then(|state| state.document_path.as_ref())
                            == Some(&intent.path);
                    if !current {
                        view.cancel_open_after_save();
                        view.error = Some("project changed · format upgrade cancelled".into());
                    } else {
                        match answer {
                            Some(0) => view.save_as_dialog(cx),
                            Some(2) => {
                                if super::device_panel::finalize_controls(view, cx)
                                    && view.enqueue_device_barrier(
                                        Request::SaveCurrentWithFormatUpgrade {
                                            project_generation: intent.generation,
                                            path: intent.path,
                                        },
                                    )
                                {
                                    view.saving_project = true;
                                    view.status = "saving project in the newer format".into();
                                    view.error = None;
                                }
                            }
                            _ => view.status = "save cancelled".into(),
                        }
                        if !view.saving_project && !view.save_dialog_open {
                            view.cancel_open_after_save();
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn save_as_dialog(&mut self, cx: &mut Context<Self>) {
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.state.is_none() {
            self.error = Some("project is still loading".into());
            return;
        }
        match self.save_admission() {
            SaveAdmission::Current | SaveAdmission::ChoosePath => {}
            SaveAdmission::Reject(message) => {
                self.error = Some(message.into());
                return;
            }
        }

        let fallback_directory = self
            .state
            .as_ref()
            .and_then(|state| state.document_path.as_deref())
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let suggested_name = if self.document_format_upgrade_required {
            self.state
                .as_ref()
                .and_then(|state| state.document_path.as_deref())
                .map(upgraded_project_name)
        } else {
            None
        };
        self.save_dialog_open = true;
        let generation = self.project_generation;
        self.with_project_picker_directory(cx, move |view, directory, cx| {
            if !view.save_dialog_open {
                return;
            }
            if view.project_generation != generation {
                view.save_dialog_open = false;
                cx.notify();
                return;
            }
            let directory = directory.unwrap_or(fallback_directory);
            let response = cx.prompt_for_new_path(
                &directory,
                Some(suggested_name.as_deref().unwrap_or("untitled.hex")),
            );
            cx.spawn(async move |this, cx| {
                let picked = response.await;
                let _ = this.update(cx, |view, cx| {
                    view.save_dialog_open = false;
                    if view.project_generation != generation {
                        cx.notify();
                        return;
                    }
                    match picked {
                        Ok(Ok(Some(path))) => {
                            if super::device_panel::finalize_controls(view, cx) {
                                view.begin_save_new(path);
                            }
                        }
                        Ok(Err(error)) => {
                            view.error = Some(error.to_string().to_lowercase().into())
                        }
                        _ => {}
                    }
                    if !view.saving_project {
                        view.cancel_open_after_save();
                    }
                    cx.notify();
                });
            })
            .detach();
        });
    }

    fn save_admission(&self) -> SaveAdmission {
        save_admission(
            self.state
                .as_ref()
                .and_then(|state| state.document_path.as_ref())
                .is_some(),
            self.saving_project,
            self.save_dialog_open,
            self.close_prompt_open
                || self.opening_project
                || (self.project_open.busy() && !self.project_open.permits_save())
                || self.pending_open_path.is_some(),
        )
    }

    fn begin_save_new(&mut self, path: PathBuf) {
        let path = match prepare_save_path(path) {
            Ok(path) => path,
            Err(message) => {
                self.error = Some(message.into());
                return;
            }
        };
        if self.enqueue_device_barrier(Request::SaveNew(path)) {
            self.saving_project = true;
            self.status = "saving project as".into();
            self.error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SaveAdmission, prepare_save_path, save_admission};
    use std::path::PathBuf;

    #[test]
    fn save_uses_current_path_or_requests_save_as() {
        assert_eq!(
            save_admission(true, false, false, false),
            SaveAdmission::Current
        );
        assert_eq!(
            save_admission(false, false, false, false),
            SaveAdmission::ChoosePath
        );
    }

    #[test]
    fn active_dialog_save_or_project_operation_blocks_reentry() {
        assert_eq!(
            save_admission(true, true, false, false),
            SaveAdmission::Reject("save already pending")
        );
        assert_eq!(
            save_admission(false, false, true, false),
            SaveAdmission::Reject("save dialog already open")
        );
        assert_eq!(
            save_admission(false, false, false, true),
            SaveAdmission::Reject("finish the current dialog before saving")
        );
        // Cancelling the native prompt clears its only admission flag.
        assert_eq!(
            save_admission(false, false, false, false),
            SaveAdmission::ChoosePath
        );
    }

    #[test]
    fn save_path_adds_extension_only_when_missing_and_checks_final_length() {
        assert_eq!(
            prepare_save_path(PathBuf::from("take")),
            Ok(PathBuf::from("take.hex"))
        );
        assert_eq!(
            prepare_save_path(PathBuf::from("Take.project")),
            Ok(PathBuf::from("Take.project"))
        );
        let too_long = PathBuf::from(format!("{}.hex", "x".repeat(4_096)));
        assert_eq!(
            prepare_save_path(too_long),
            Err("project path exceeds the 4096-byte limit")
        );
    }
}
