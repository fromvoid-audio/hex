use super::EditorView;
use gpui::Context;
use hex_session::service::Request;
use std::path::{Path, PathBuf};

const MAX_PROJECT_PATH_BYTES: usize = 4_096;

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

        let directory = self
            .state
            .as_ref()
            .and_then(|state| state.document_path.as_deref())
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        self.save_dialog_open = true;
        let response = cx.prompt_for_new_path(&directory, Some("untitled.hex"));
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                view.save_dialog_open = false;
                match picked {
                    Ok(Ok(Some(path))) => {
                        if super::device_panel::finalize_controls(view, cx) {
                            view.begin_save_new(path);
                        }
                    }
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase().into()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_admission(&self) -> SaveAdmission {
        save_admission(
            self.state
                .as_ref()
                .and_then(|state| state.document_path.as_ref())
                .is_some(),
            self.saving_project,
            self.save_dialog_open,
            self.close_prompt_open || self.opening_project,
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
