use super::EditorView;
use gpui::Context;
use hex_session::service::Request;
use hex_session::snapshot_exports::SnapshotTransportError;
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
    fn report_save_read(&mut self, error: SnapshotTransportError) {
        if error == SnapshotTransportError::Busy {
            self.error = Some("project read busy · try saving again".into());
        } else {
            self.error = Some("project read is unavailable".into());
        }
    }

    pub(super) fn save_project(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.save_project_checked(cx) {
            self.report_save_read(error);
            cx.notify();
        }
    }

    pub(super) fn save_project_checked(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<bool, SnapshotTransportError> {
        let Some(admission) = self.save_admission()? else {
            self.error = Some("project is still loading".into());
            return Ok(false);
        };
        match admission {
            SaveAdmission::Current => {
                if !super::device_panel::finalize_controls_checked(self, cx)? {
                    return Ok(false);
                }
                if self.enqueue_device_barrier(Request::SaveCurrent) {
                    self.saving_project = true;
                    self.status = "saving project".into();
                    self.error = None;
                    return Ok(true);
                }
                Ok(false)
            }
            SaveAdmission::ChoosePath => self.save_as_dialog_checked(cx),
            SaveAdmission::Reject(message) => {
                self.error = Some(message.into());
                Ok(false)
            }
        }
    }

    pub(super) fn save_as_dialog(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.save_as_dialog_checked(cx) {
            self.report_save_read(error);
            cx.notify();
        }
    }

    fn save_as_dialog_checked(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<bool, SnapshotTransportError> {
        let Some(admission) = self.save_admission()? else {
            self.error = Some("project is still loading".into());
            return Ok(false);
        };
        if let SaveAdmission::Reject(message) = admission {
            self.error = Some(message.into());
            return Ok(false);
        }
        // Reserve bounded path backing before borrowing snapshot metadata.
        // PathBuf::push copies the exact platform path without growth here.
        let mut fallback_directory = PathBuf::with_capacity(MAX_PROJECT_PATH_BYTES);
        let copied = self.dialog_snapshot(|snapshot| {
            let Some(parent) = snapshot.document_path().and_then(Path::parent) else {
                return Ok(false);
            };
            if parent.as_os_str().len() > fallback_directory.capacity() {
                return Err("project path exceeds the 4096-byte limit");
            }
            fallback_directory.push(parent);
            Ok(true)
        })?;
        match copied {
            Some(Ok(true)) => {}
            Some(Ok(false)) => fallback_directory.push("."),
            Some(Err(message)) => {
                self.error = Some(message.into());
                return Ok(false);
            }
            None => return Ok(false),
        }
        if !super::device_panel::finalize_controls_checked(self, cx)? {
            return Ok(false);
        }
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
            let response = cx.prompt_for_new_path(&directory, Some("untitled.hex"));
            cx.spawn(async move |this, cx| {
                let mut picked = Some(response.await);
                loop {
                    let retry = this
                        .update(cx, |view, cx| {
                            if !view.save_dialog_open {
                                return false;
                            }
                            if view.project_generation != generation {
                                view.save_dialog_open = false;
                                cx.notify();
                                return false;
                            }
                            let has_path = matches!(picked.as_ref(), Some(Ok(Ok(Some(_)))));
                            let finalized = if has_path {
                                match super::device_panel::finalize_controls_checked(view, cx) {
                                    Ok(finalized) => finalized,
                                    Err(SnapshotTransportError::Busy) => return true,
                                    Err(error) => {
                                        view.report_save_read(error);
                                        false
                                    }
                                }
                            } else {
                                false
                            };
                            view.save_dialog_open = false;
                            match picked.take() {
                                Some(Ok(Ok(Some(path)))) if finalized => view.begin_save_new(path),
                                Some(Ok(Err(error))) => {
                                    view.error = Some(error.to_string().to_lowercase().into())
                                }
                                _ => {}
                            }
                            if !view.saving_project {
                                view.cancel_open_after_save();
                            }
                            cx.notify();
                            false
                        })
                        .unwrap_or(false);
                    if !retry {
                        break;
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(5))
                        .await;
                }
            })
            .detach();
        });
        Ok(true)
    }

    fn save_admission(&self) -> Result<Option<SaveAdmission>, SnapshotTransportError> {
        self.dialog_snapshot(|snapshot| {
            save_admission(
                snapshot.document_path().is_some(),
                self.saving_project,
                self.save_dialog_open,
                self.close_prompt_open
                    || self.opening_project
                    || (self.project_open.busy() && !self.project_open.permits_save())
                    || self.pending_open_path.is_some(),
            )
        })
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
