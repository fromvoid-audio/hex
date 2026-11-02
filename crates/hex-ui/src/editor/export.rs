//! Native export admission and completion display; never applies canonical state.
use super::EditorView;
use gpui::{AnyWindowHandle, AppContext, Context, PromptButton, PromptLevel, Window};
use hex_session::service::{ExportControl, ExportFormat, ExportSignal, Request, Stem};
use std::{path::PathBuf, sync::mpsc::TryRecvError};

impl EditorView {
    pub(super) fn selected_export_tracks(&self) -> Vec<Stem> {
        self.state().map_or_else(Vec::new, |state| {
            self.selected_track_ids()
                .into_iter()
                .filter_map(|id| {
                    state.project.track(id).map(|track| Stem {
                        id: hex_engine::OutputCaptureId::Track(id),
                        name: track.name().to_owned(),
                    })
                })
                .collect()
        })
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
        let selected = self.selected_track_ids();
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
            .selected_export_tracks()
            .into_iter()
            .chain(self.state().into_iter().flat_map(|state| {
                state.project.return_tracks().iter().map(|bus| Stem {
                    id: hex_engine::OutputCaptureId::Return(bus.id()),
                    name: bus.name().to_owned(),
                })
            }))
            .collect();
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
                    Ok(1) => {
                        view.export_format_dialog(handle, start, frames, ExportSignal::Main, cx)
                    }
                    Ok(2) if !selected.is_empty() => view.export_format_dialog(
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
                    Ok(3) if !returns.is_empty() => view.export_format_dialog(
                        handle,
                        start,
                        frames,
                        ExportSignal::CapturedMix(returns),
                        cx,
                    ),
                    Ok(4) if !selected.is_empty() => view.export_format_dialog(
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

    /// Second native prompt: the sample format of the file or of every stem
    /// file. The choice is per export and is not saved in the project.
    fn export_format_dialog(
        &mut self,
        handle: AnyWindowHandle,
        start: u64,
        frames: Option<u64>,
        signal: ExportSignal,
        cx: &mut Context<Self>,
    ) {
        let mut buttons = vec![PromptButton::cancel("cancel")];
        buttons.extend(
            ExportFormat::ALL
                .iter()
                .map(|format| PromptButton::new(format_label(*format))),
        );
        let prompt = cx.update_window(handle, |_, window, cx| {
            window.prompt(
                PromptLevel::Info,
                "export format",
                Some(
                    "16-bit is dithered. 24-bit and float keep more resolution; float keeps overs.",
                ),
                &buttons,
                cx,
            )
        });
        let Ok(prompt) = prompt else {
            self.export_dialog_open = false;
            self.error = Some("export window closed before choosing a format".into());
            return;
        };
        cx.spawn(async move |this, cx| {
            let choice = prompt.await;
            let _ = this.update(cx, |view, cx| {
                match choice.ok().and_then(format_choice) {
                    Some(format) => {
                        view.export_path_dialog(handle, start, frames, signal, format, cx)
                    }
                    None => view.export_dialog_open = false,
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
        format: ExportFormat,
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
                            view.error = Some(
                                hex_session::service::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )
                                .into(),
                            );
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
                        format,
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
                // status chrome and the prompt normalize app text through theme::text.
                let recovery_message = match &event.result {
                    Ok(report) => report.recovery_path.as_ref().map(|path| {
                        format!(
                            "previous file is preserved at {}",
                            hex_session::service::text::verbatim(path.display())
                        )
                    }),
                    Err(error) => Some(error.clone()),
                };
                if let (Some(handle), Some(message)) = (self.export_window.take(), recovery_message)
                {
                    if let Ok(response) = cx.update_window(handle, |_, window, cx| {
                        window.prompt(
                            PromptLevel::Info,
                            "export result",
                            Some(&crate::theme::text(&message)),
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
                        self.status = export_status(
                            event.revision,
                            report.frames,
                            report.clipped_samples,
                            report
                                .recovery_path
                                .map(|path| hex_session::service::text::verbatim(path.display())),
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

/// Lowercase format prompt button label.
fn format_label(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Pcm16 => "16-bit pcm",
        ExportFormat::Pcm24 => "24-bit pcm",
        ExportFormat::Float32 => "32-bit float",
    }
}

/// Format for a format prompt answer: index 0 is cancel, then
/// [`ExportFormat::ALL`] in button order.
fn format_choice(answer: usize) -> Option<ExportFormat> {
    answer
        .checked_sub(1)
        .and_then(|index| ExportFormat::ALL.get(index).copied())
}

/// Completed-export status line. Integer exports that clamped overs report the
/// clipped channel-sample count so a hot master is never silently flattened.
fn export_status(
    revision: u64,
    frames: u64,
    clipped_samples: u64,
    previous_file: Option<String>,
) -> String {
    let mut status = format!("exported rev {revision} · {frames} frames");
    match clipped_samples {
        0 => {}
        1 => status.push_str(" · 1 sample clipped"),
        count => status.push_str(&format!(" · {count} samples clipped")),
    }
    if let Some(path) = previous_file {
        status.push_str(&format!(" · previous file at {path}"));
    }
    status
}

#[cfg(test)]
mod status_tests {
    use super::{ExportFormat, export_status, format_choice, format_label};

    #[test]
    fn format_prompt_answers_map_to_lowercase_labelled_formats_and_cancel() {
        assert_eq!(format_choice(0), None, "cancel");
        assert_eq!(format_choice(1), Some(ExportFormat::Pcm16));
        assert_eq!(format_choice(2), Some(ExportFormat::Pcm24));
        assert_eq!(format_choice(3), Some(ExportFormat::Float32));
        assert_eq!(format_choice(4), None);
        let labels: Vec<_> = ExportFormat::ALL.iter().map(|f| format_label(*f)).collect();
        assert_eq!(labels, ["16-bit pcm", "24-bit pcm", "32-bit float"]);
        assert!(labels.iter().all(|label| *label == label.to_lowercase()));
    }

    #[test]
    fn export_status_reports_clipped_samples_only_when_present() {
        assert_eq!(
            export_status(3, 480, 0, None),
            "exported rev 3 · 480 frames"
        );
        assert_eq!(
            export_status(3, 480, 1, None),
            "exported rev 3 · 480 frames · 1 sample clipped"
        );
        assert_eq!(
            export_status(4, 2, 2, Some("/Old/Take.wav".into())),
            "exported rev 4 · 2 frames · 2 samples clipped · previous file at /Old/Take.wav"
        );
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
        // Cancelling the format prompt ends the dialog without a path prompt.
        cx.simulate_prompt_answer("cancel");
        cx.run_until_parked();
        assert!(!cx.did_prompt_for_new_path());
        assert!(!editor.read_with(cx, |view, _| view.export_dialog_open));
        begin(cx);
        cx.simulate_prompt_answer("main mix");
        cx.run_until_parked();
        cx.simulate_prompt_answer("16-bit pcm");
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
        cx.simulate_prompt_answer("24-bit pcm");
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
