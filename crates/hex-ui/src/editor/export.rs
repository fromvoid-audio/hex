//! Native export admission and completion display; never applies canonical state.
use super::EditorView;
use gpui::{AnyWindowHandle, AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::service::{ExportControl, ExportSignal, Request, Stem};
use std::{path::PathBuf, sync::mpsc::TryRecvError};

impl EditorView {
    #[cfg(test)]
    pub(super) fn selected_export_tracks(&self) -> Vec<Stem> {
        self.state()
            .and_then(|state| {
                state
                    .read_with(|snapshot| {
                        let project = snapshot.model();
                        self.track_selection
                            .ordered(&project)
                            .into_iter()
                            .filter_map(|id| {
                                project.track(id).map(|track| Stem {
                                    id: hex_engine::OutputCaptureId::Track(id),
                                    name: track.name().to_owned(),
                                })
                            })
                            .collect()
                    })
                    .ok()
            })
            .unwrap_or_default()
    }

    pub(super) fn export_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_pending {
            if let Some(control) = &self.export_control {
                self.status = if control.cancel() {
                    "cancelling export"
                } else {
                    "export publication already started"
                }
                .into();
                cx.notify();
            }
            return;
        }
        if self.export_dialog_open {
            self.error = Some("an export is already pending".into());
            return;
        }
        if self.close_prompt_open || self.opening_project {
            self.error = Some("finish the current dialog before exporting".into());
            return;
        }
        // Capture output scope only after a successful current read. Pressure
        // must not turn an existing time range/track selection into a full mix.
        let Some(state) = self.state() else {
            return;
        };
        let Ok((range, selected, returns, stems)) = state.read_with(|snapshot| {
            let project = snapshot.model();
            let range = self.normalized_time_selection_in(project);
            let selected = self.track_selection.ordered(&project);
            let returns: Vec<_> = project
                .returns()
                .map(|bus| hex_engine::OutputCaptureId::Return(bus.id()))
                .collect();
            let stems: Vec<_> = selected
                .iter()
                .filter_map(|id| {
                    project.track(*id).map(|track| Stem {
                        id: hex_engine::OutputCaptureId::Track(*id),
                        name: track.name().to_owned(),
                    })
                })
                .chain(project.returns().map(|bus| Stem {
                    id: hex_engine::OutputCaptureId::Return(bus.id()),
                    name: bus.name().to_owned(),
                }))
                .collect();
            (range, selected, returns, stems)
        }) else {
            self.error = Some("project read unavailable · cannot export".into());
            return;
        };
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        self.export_dialog_open = true;
        let start = range.as_ref().map_or(0, |range| range.start());
        let frames = range.as_ref().map(|range| range.end() - range.start());
        let handle = window.window_handle();
        let scope = window.prompt(
            PromptLevel::Info,
            "export output",
            Some("time selection sets the range. choose output scope explicitly."),
            &[
                PromptButton::cancel("cancel"),
                PromptButton::new("main mix"),
                PromptButton::new("selected tracks"),
                PromptButton::new("shared returns"),
                PromptButton::new("selected stems and shared returns"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let choice = scope.await;
            let _ = this.update(cx, |view, cx| {
                match choice {
                    Ok(1) => view.export_path_dialog(handle, start, frames, ExportSignal::Main, cx),
                    Ok(2) if !selected.is_empty() => view.export_path_dialog(
                        handle,
                        start,
                        frames,
                        ExportSignal::CapturedMix(
                            selected
                                .iter()
                                .copied()
                                .map(hex_engine::OutputCaptureId::Track)
                                .collect(),
                        ),
                        cx,
                    ),
                    Ok(3) if !returns.is_empty() => view.export_path_dialog(
                        handle,
                        start,
                        frames,
                        ExportSignal::CapturedMix(returns),
                        cx,
                    ),
                    Ok(4) if !selected.is_empty() => view.export_path_dialog(
                        handle,
                        start,
                        frames,
                        ExportSignal::Stems(stems),
                        cx,
                    ),
                    Ok(2..=4) => {
                        view.export_dialog_open = false;
                        view.error = Some("export output selection is empty".into());
                    }
                    _ => view.export_dialog_open = false,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_path_dialog(
        &mut self,
        handle: AnyWindowHandle,
        start: u64,
        frames: Option<u64>,
        signal: ExportSignal,
        cx: &mut Context<Self>,
    ) {
        let batch = matches!(&signal, ExportSignal::Stems(_));
        let Some(directory) = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
        else {
            self.export_dialog_open = false;
            self.error = Some("could not locate your user folder for export".into());
            cx.notify();
            return;
        };
        let response = cx.prompt_for_new_path(
            &directory,
            Some(if batch {
                "untitled-stems"
            } else {
                "untitled.wav"
            }),
        );
        cx.spawn(async move |this, cx| {
            let path = match response.await {
                // Keep the exact native-confirmed path. Appending an extension
                // afterward could target a different existing, unconfirmed file.
                Ok(Ok(Some(path))) => path,
                picked => {
                    let _ = this.update(cx, |view, cx| {
                        view.export_dialog_open = false;
                        if let Ok(Err(error)) = picked {
                            view.error = Some(error.to_string().to_lowercase().into());
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, |view, cx| {
                view.export_dialog_open = false;
                // The native save dialog already owns concrete-path overwrite
                // confirmation. Never ask for that same consent a second time.
                if path.as_os_str().len() > 4096 {
                    view.error = Some("export path exceeds the 4096-byte limit".into());
                } else if super::device_panel::finalize_controls(view, cx) {
                    let control = ExportControl::default();
                    if view.enqueue_device_barrier(Request::ExportRange {
                        path,
                        start,
                        frames,
                        signal,
                        control: control.clone(),
                        overwrite: !batch,
                    }) {
                        view.export_pending = true;
                        view.export_control = Some(control);
                        view.export_window = Some(handle);
                        view.status = "export requested".into();
                        view.error = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn poll_export(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if self.export_pending {
            if let Some(control) = &self.export_control {
                let progress = control.progress();
                let status = format!(
                    "{} {} / {} frames · click export to cancel",
                    if control.is_cancelled() {
                        "cancelling export"
                    } else {
                        "export"
                    },
                    progress.processed_frames,
                    progress.total_frames
                );
                changed = self.status != status;
                self.status = status;
            }
        }
        match self.service.try_export_event() {
            Ok(event) => {
                self.export_pending = false;
                self.export_control = None;
                // Native message bodies preserve case-sensitive recovery paths;
                // status chrome normalizes app text through theme::text.
                let recovery_message = match &event.result {
                    Ok(report) => report
                        .recovery_path
                        .as_ref()
                        .map(|path| format!("previous file is preserved at {}", path.display())),
                    Err(error) => Some(error.clone()),
                };
                if let (Some(handle), Some(message)) = (self.export_window.take(), recovery_message)
                {
                    if let Ok(response) = cx.update_window(handle, |_, window, cx| {
                        window.prompt(
                            PromptLevel::Info,
                            "export result",
                            Some(&message),
                            &[PromptButton::new("ok")],
                            cx,
                        )
                    }) {
                        cx.spawn(async move |_, _| {
                            let _ = response.await;
                        })
                        .detach();
                    }
                }
                match event.result {
                    Ok(report) => {
                        self.status = format!(
                            "exported rev {} · {} frames{}",
                            event.revision,
                            report.frames,
                            report
                                .recovery_path
                                .map(|path| format!(" · previous file at {}", path.display()))
                                .unwrap_or_default()
                        );
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error.into()),
                }
                true
            }
            Err(TryRecvError::Disconnected) if self.export_pending => {
                self.export_pending = false;
                self.export_control = None;
                self.export_window = None;
                self.error = Some("export worker stopped".into());
                true
            }
            Err(_) => changed,
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::super::clip_drag_ui_test;
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn export_dialog_cancel_reopen_starts_in_user_folder_and_native_save_needs_no_second_consent(
        cx: &mut TestAppContext,
    ) {
        let (project, _, _, _) = clip_drag_ui_test::midi_project();
        let (handle, editor) =
            clip_drag_ui_test::editor_window(cx, project, std::sync::Arc::default());
        let begin = |cx: &mut TestAppContext| {
            cx.update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| view.export_dialog(window, cx))
            })
            .unwrap();
            cx.run_until_parked();
        };
        begin(cx);
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        assert!(!editor.read_with(cx, |view, _| view.export_dialog_open));
        begin(cx);
        cx.simulate_prompt_answer("main mix");
        cx.run_until_parked();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|directory| {
            assert_eq!(directory, PathBuf::from(std::env::var_os("HOME").unwrap()));
            None
        });
        cx.run_until_parked();
        assert!(!editor.read_with(cx, |view, _| view.export_dialog_open));
        begin(cx);
        cx.simulate_prompt_answer("main mix");
        cx.run_until_parked();
        cx.simulate_new_path_selection(|_| {
            Some(std::path::PathBuf::from(
                "/tmp/hex-nonexistent-export-parent/no-extension",
            ))
        });
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        editor.read_with(cx, |view, _| {
            assert!(view.export_pending);
            assert!(!view.export_dialog_open);
            assert!(view.export_control.is_some());
        });
    }

    #[gpui::test]
    fn export_button_cancels_pending_control_without_opening_another_dialog(
        cx: &mut TestAppContext,
    ) {
        let (project, _, _, _) = clip_drag_ui_test::midi_project();
        let (handle, editor) =
            clip_drag_ui_test::editor_window(cx, project, std::sync::Arc::default());
        let control = ExportControl::default();
        editor.update(cx, |view, _| {
            view.export_pending = true;
            view.export_control = Some(control.clone());
        });
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| view.export_dialog(window, cx))
        })
        .unwrap();
        assert!(control.is_cancelled());
        assert!(!cx.has_pending_prompt());
        assert!(!editor.read_with(cx, |view, _| view.export_dialog_open));
    }
}
