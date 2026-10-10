//! Native export admission and completion display; never applies canonical state.
use super::EditorView;
use gpui::Context;
use hex_session::service::Request;
use std::{path::Path, sync::mpsc::TryRecvError};

impl EditorView {
    pub(super) fn export_dialog(&mut self, cx: &mut Context<Self>) {
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        if self.export_pending || self.export_dialog_open {
            self.error = Some("an export is already pending".into());
            return;
        }
        if self.close_prompt_open || self.opening_project {
            self.error = Some("finish the current dialog before exporting".into());
            return;
        }
        self.export_dialog_open = true;
        let response = cx.prompt_for_new_path(Path::new("."), Some("untitled.wav"));
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                view.export_dialog_open = false;
                match picked {
                    Ok(Ok(Some(mut path))) => {
                        if path.extension().is_none() {
                            path.set_extension("wav");
                        }
                        if path.as_os_str().len() > 4_096 {
                            view.error = Some("export path exceeds the 4096-byte limit".into());
                        } else if !super::device_panel::finalize_controls(view, cx) {
                            // Keep the export unapplied if a field became invalid while
                            // the native path dialog was open.
                        } else if view.enqueue_device_barrier(Request::Export(path)) {
                            view.export_pending = true;
                            view.status = "export requested".into();
                            view.error = None;
                        }
                    }
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn poll_export(&mut self) -> bool {
        match self.service.try_export_event() {
            Ok(event) => {
                self.export_pending = false;
                match event.result {
                    Ok(report) => {
                        self.status =
                            format!("exported rev {} · {} frames", event.revision, report.frames);
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error.to_lowercase()),
                }
                true
            }
            Err(TryRecvError::Disconnected) if self.export_pending => {
                self.export_pending = false;
                self.error = Some("export worker stopped".into());
                true
            }
            Err(_) => false,
        }
    }
}
