//! Native export admission and completion display; never applies canonical state.
use super::EditorView;
use gpui::{AnyWindowHandle, AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::service::{ExportControl, ExportSignal, Request, Stem};
use std::{path::Path, sync::mpsc::TryRecvError};

impl EditorView {
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
        if !super::device_panel::finalize_controls(self, cx) {
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
        self.export_dialog_open = true;
        // The selection is canonical frame timing, captured when requesting
        // the dialog. A time highlight never silently changes output scope.
        let range = self.normalized_time_selection();
        let start = range.as_ref().map_or(0, |range| range.start());
        let frames = range.as_ref().map(|range| range.end() - range.start());
        let selected: Vec<_> = range
            .as_ref()
            .map(|range| range.tracks().to_vec())
            .unwrap_or_else(|| self.selected_track.into_iter().collect());
        let returns: Vec<_> = self
            .state()
            .map(|state| {
                state
                    .project
                    .return_tracks()
                    .iter()
                    .map(|bus| hex_engine::OutputCaptureId::Return(bus.id()))
                    .collect()
            })
            .unwrap_or_default();
        let stems: Vec<_> = self
            .state()
            .map(|state| {
                selected
                    .iter()
                    .filter_map(|id| state.project.track(*id))
                    .map(|track| Stem {
                        id: hex_engine::OutputCaptureId::Track(track.id()),
                        name: track.name().to_owned(),
                    })
                    .chain(state.project.return_tracks().iter().map(|bus| Stem {
                        id: hex_engine::OutputCaptureId::Return(bus.id()),
                        name: bus.name().to_owned(),
                    }))
                    .collect()
            })
            .unwrap_or_default();
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
                    Ok(2 | 3 | 4) => {
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
        let response = cx.prompt_for_new_path(
            Path::new("."),
            Some(if batch {
                "untitled-stems"
            } else {
                "untitled.wav"
            }),
        );
        cx.spawn(async move |this, cx| {
            let path = match response.await {
                Ok(Ok(Some(mut path))) => {
                    if !batch && path.extension().is_none() {
                        path.set_extension("wav");
                    }
                    path
                }
                picked => {
                    let _ = this.update(cx, |view, cx| {
                        view.export_dialog_open = false;
                        if let Ok(Err(error)) = picked {
                            view.error = Some(error.to_string().to_lowercase());
                        }
                        cx.notify();
                    });
                    return;
                }
            };
            let message = format!(
                "{}{}",
                path.display(),
                if batch {
                    " · new batch directory only"
                } else {
                    " · replacing retains the previous file as recovery data"
                }
            );
            let answer = cx.update_window(handle, |_, window, cx| {
                let mut buttons = vec![
                    PromptButton::cancel("cancel"),
                    PromptButton::new("create new"),
                ];
                if !batch {
                    buttons.push(PromptButton::new("replace this file"));
                }
                window.prompt(
                    PromptLevel::Warning,
                    "confirm export destination",
                    Some(&message),
                    &buttons,
                    cx,
                )
            });
            let choice = match answer {
                Ok(answer) => answer.await.ok(),
                Err(_) => None,
            };
            let _ = this.update(cx, |view, cx| {
                view.export_dialog_open = false;
                if matches!(choice, Some(1 | 2)) {
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
                            overwrite: choice == Some(2),
                        }) {
                            view.export_pending = true;
                            view.export_control = Some(control);
                            view.status = "export requested".into();
                            view.error = None;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn poll_export(&mut self) -> bool {
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
                    Err(error) => self.error = Some(error.to_lowercase()),
                }
                true
            }
            Err(TryRecvError::Disconnected) if self.export_pending => {
                self.export_pending = false;
                self.export_control = None;
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
    fn export_dialog_cancel_reopen_and_concrete_destination_cancel_never_reserve_job(
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
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
        assert!(!editor.read_with(cx, |view, _| view.export_dialog_open));
        begin(cx);
        cx.simulate_prompt_answer("main mix");
        cx.run_until_parked();
        cx.simulate_new_path_selection(|_| {
            Some(std::path::PathBuf::from(
                "/tmp/hex-concrete-export-test.wav",
            ))
        });
        cx.run_until_parked();
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(!view.export_pending);
            assert!(!view.export_dialog_open);
            assert!(view.export_control.is_none());
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
